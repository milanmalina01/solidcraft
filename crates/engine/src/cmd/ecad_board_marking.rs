//! Board markings (see `solidcraft_doc::ecad::marking`): `ecad.board.marking` stamps the board's
//! title, revision, date, part number and the template's extras on its silkscreen as one tagged
//! group, and optionally a mark in copper on each layer, in one step; `ecad.design.info` makes
//! them again when the revision or the date changes. The template itself lives in the app's
//! preferences and comes as a parameter.

use serde_json::Value;
use solidcraft_doc::ecad::Electronics;
use solidcraft_doc::ecad::checks::Severity;
use solidcraft_doc::ecad::footprint::Side as DocSide;
use solidcraft_doc::ecad::marking::{self, Fields, MAX_MARKINGS, Marking, MarkingTemplate};
use solidcraft_doc::metadata;

use super::CommandSpec;
use super::ecad::{parse, reply};
use super::ecad_board::{board_point, board_result, store_board};
use crate::params::bad;
use crate::schema::ecad_board_marking::{MarkingParams, MarkingResult, MarkingTemplateParam};
use crate::schema::ecad_component::Side;
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("ecad.board.marking", "Board Marking", crate::checked!("ecad.board.marking", place))
    .icon("text")
    .doc_only()
    .contract(&CONTRACT)
    .output(output_schema_for::<MarkingResult>)
    .params(
        "side: top|bottom; at: [x, y] (the block's top left, mm); template? {title?, revision?, date? (shown by default), part_number? (pattern: {project}/{title}, {number}, {revision}, {date}, {company}, {author}), serial_box?, qr? (of the part number and revision), made_by?, order_placeholder? (the text the fab replaces with its order number), height? (mm, 0.5–5, 1.5)}; number? (the pattern's {number}); layer_marks? (copper TOP/BOT, or 1..n, side by side below the block, each in a keepout pours stay out of, on no net); dry_run? → {marking, lines, part_number, graphics, keepouts, bbox, warnings, created, …}: one grouped block on the side's silkscreen (mirrored on the bottom) from the design info the print export uses; ecad.design.info makes it again when the title, revision or date changes. Refused when it reaches off the board or its copper breaks a design rule (clearance to other copper, the edge). One undo step",
    )];

const CONTRACT: TypedContract<MarkingParams> = TypedContract::new();

/// What the block prints from the design: the print export's title block fields, from `ecad`'s
/// design info, else the document's properties.
pub fn fields(s: &Session, ecad: &Electronics) -> Fields {
    let i = &ecad.info;
    let m = &s.doc.metadata;
    let or = |own: &str, key: &str| if own.is_empty() { m.text(key).unwrap_or_default().to_string() } else { own.to_string() };
    let mut title = or(&i.title, metadata::PROJECT);
    if title.is_empty() {
        title = s.doc.name.clone();
    }
    Fields {
        title,
        revision: or(&i.revision, metadata::REVISION),
        date: i.date.clone(),
        company: or(&i.company, metadata::ORGANIZATION),
        author: or(&i.author, metadata::AUTHOR),
    }
}

/// The template a parameter gives (the usual for what it leaves out).
pub fn template_of(p: Option<MarkingTemplateParam>) -> MarkingTemplate {
    let d = MarkingTemplate::default();
    let Some(p) = p else { return d };
    MarkingTemplate {
        title: p.title.unwrap_or(d.title),
        revision: p.revision.unwrap_or(d.revision),
        date: p.date.unwrap_or(d.date),
        part_number: p.part_number.unwrap_or(d.part_number),
        serial_box: p.serial_box.unwrap_or(d.serial_box),
        qr: p.qr.unwrap_or(d.qr),
        made_by: p.made_by.unwrap_or(d.made_by),
        order_placeholder: p.order_placeholder.unwrap_or(d.order_placeholder),
        height: p.height.unwrap_or(d.height),
    }
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0 + 0.0
}

fn place(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.marking";
    let p: MarkingParams = parse(CMD, p)?;
    let at = board_point(p.at, CMD, "`at`")?;
    let template = template_of(p.template);
    template.validate().map_err(|e| bad(CMD, e))?;
    let number = p.number.unwrap_or_default();
    if number.chars().count() > marking::MAX_TEXT || number.chars().any(char::is_control) {
        return Err(bad(CMD, format!("`number` must be at most {} characters, with no control characters", marking::MAX_TEXT)));
    }
    let old = s.doc.ecad.board.clone();
    if old.markings.len() >= MAX_MARKINGS {
        return Err(bad(CMD, format!("the board has {MAX_MARKINGS} markings already")));
    }
    if old.outline.is_empty() {
        return Err(bad(CMD, "the board has no outline to mark: ecad.board.outline sets one"));
    }
    let f = fields(s, &s.doc.ecad);
    let side = match p.side {
        Side::Top => DocSide::Top,
        Side::Bottom => DocSide::Bottom,
    };
    let mut board = old.clone();
    let id = board.take_id().map_err(|e| bad(CMD, e))?;
    let m = Marking { id, side, at, number, template, layer_marks: p.layer_marks.unwrap_or(false) };
    marking::stamp(&mut board, &m, &f).map_err(|e| bad(CMD, e))?;
    board.markings.push(m.clone());
    let mine: Vec<u64> = board
        .graphics
        .iter()
        .filter(|g| g.marking == Some(id))
        .map(|g| g.id)
        .chain(board.keepouts.iter().filter(|k| k.marking == Some(id)).map(|k| k.id))
        .collect();
    let (graphics, keepouts) =
        (board.graphics.iter().filter(|g| g.marking == Some(id)).count(), board.keepouts.iter().filter(|k| k.marking == Some(id)).count());
    let bbox = solidcraft_doc::ecad::bbox(board.graphics.iter().filter(|g| g.marking == Some(id)).filter_map(|g| g.graphic.bbox()).flatten())
        .unwrap_or([at, at]);
    // On the board, and its copper within the rules.
    let ring = solidcraft_doc::ecad::path::tessellate(&board.outline, &board.outline_curves, true, 0.01);
    let corners = [bbox[0], [bbox[1][0], bbox[0][1]], bbox[1], [bbox[0][0], bbox[1][1]]];
    if !corners.iter().all(|c| solidcraft_doc::ecad::copper::inside(*c, &ring)) {
        return Err(bad(CMD, "the marking reaches off the board there: give another `at`, or leave out what it doesn't need"));
    }
    let mut probe = s.doc.ecad.clone();
    probe.board = board.clone();
    let drc = probe.drc();
    let keys: Vec<String> = mine.iter().map(|i| format!("g{i}")).collect();
    let ours = |key: &str| key.split('|').skip(1).any(|k| keys.iter().any(|m| m == k));
    let errors: Vec<String> = drc.findings.iter().filter(|x| x.severity == Severity::Error && ours(&x.key)).map(|x| x.message.clone()).collect();
    if let Some(first) = errors.first() {
        return Err(bad(CMD, format!("its copper marks break a design rule there ({first}): give another `at`")));
    }
    let warnings: Vec<String> = drc.findings.iter().filter(|x| x.severity == Severity::Warning && ours(&x.key)).map(|x| x.message.clone()).collect();
    let lines: Vec<String> = board
        .graphics
        .iter()
        .filter(|g| {
            g.marking == Some(id)
                && g.graphic.kind == solidcraft_doc::ecad::graphics::GraphicType::Text
                && !matches!(g.layer, solidcraft_doc::ecad::layers::DrawLayer::Copper(_))
        })
        .map(|g| g.graphic.text.clone())
        .collect();
    let pn = marking::part_number(&m.template.part_number, &f, &m.number);
    let out = |marking: u64, board| MarkingResult {
        marking,
        lines: lines.clone(),
        part_number: pn.clone(),
        graphics,
        keepouts,
        bbox: [[round3(bbox[0][0]), round3(bbox[0][1])], [round3(bbox[1][0]), round3(bbox[1][1])]],
        warnings: warnings.clone(),
        board,
    };
    if p.dry_run.unwrap_or(false) {
        return reply(out(0, board_result(s, &old)));
    }
    store_board(s, board, CMD)?;
    reply(out(id, board_result(s, &old)))
}
