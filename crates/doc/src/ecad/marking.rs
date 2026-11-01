//! Board markings: the board's identity stamped on its silkscreen as one block (see
//! `ecad.board.marking`), and optionally a mark on each copper layer.
//!
//! A [`MarkingTemplate`] says what the block holds, and lives in the app's preferences, not in a
//! design: the title, revision and date (from the design info the print export uses), a part
//! number made from one pattern (`MM-{project}-{number}`: `{project}` the title, `{number}` the
//! marking's own number, `{revision}`, `{date}`, `{company}` and `{author}` too), and, each when
//! asked, an empty serial-number box, a QR code of the part number and revision, a short "made
//! by" text, and a placeholder where the fab prints its order number. Placing one keeps a
//! [`Marking`] on the board (its side, where, its number, the template as it was) and makes its
//! drawings, each tagged with the marking's id so the block stays one group; [`refresh`] makes
//! them again when the revision or the date changes.
//!
//! Layer marks, when asked: "TOP" and "BOT" on a 2-layer board, "1" to "n" on more, in copper on
//! each layer, side by side so all of them show held against the light, each in a keepout that
//! keeps pours away from it (a copper drawing joins no net; the design rule check keeps other
//! copper its clearance away). Top-side text reads from the top; the bottom silk and the bottom
//! copper mark are mirrored, as text read from below is.
//!
//! The block's top left is `at`; lines go down from it, [`LINE`] heights apart. The QR code is
//! filled squares of [`QR_MODULE`] (each row's dark run one rectangle), at most version 6.

use serde::{Deserialize, Serialize};

use super::Electronics;
use super::board::BoardGraphic;
use super::footprint::Side;
use super::graphics::{Graphic, GraphicType};
use super::layers::{CopperLayer, DrawLayer};
use super::pour::{Forbid, Keepout};

/// Line pitch, as a share of the text height.
pub const LINE: f64 = 1.5;
/// A QR module's side, in millimetres: the silk's finest legible dot.
pub const QR_MODULE: f64 = 0.4;
/// The most characters a template's text or pattern holds.
pub const MAX_TEXT: usize = 64;
/// The most markings a board holds.
pub const MAX_MARKINGS: usize = 16;
/// The text height when the template gives none, in millimetres.
pub const DEFAULT_HEIGHT: f64 = 1.5;
/// The layer marks' height, and the clear border round each, in millimetres.
pub const MARK_HEIGHT: f64 = 1.5;
pub const MARK_CLEAR: f64 = 0.5;

/// What a marking holds (see the module notes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MarkingTemplate {
    pub title: bool,
    pub revision: bool,
    pub date: bool,
    /// The part number's pattern; empty for none.
    pub part_number: String,
    pub serial_box: bool,
    pub qr: bool,
    /// A short "made by" text; empty for none.
    pub made_by: String,
    /// What the fab replaces with its order number (as its order form asks); empty for none.
    pub order_placeholder: String,
    /// Text height in millimetres.
    pub height: f64,
}

impl Default for MarkingTemplate {
    fn default() -> Self {
        MarkingTemplate {
            title: true,
            revision: true,
            date: true,
            part_number: String::new(),
            serial_box: false,
            qr: false,
            made_by: String::new(),
            order_placeholder: String::new(),
            height: DEFAULT_HEIGHT,
        }
    }
}

impl MarkingTemplate {
    /// Why not, when a text is too long or the height out of range.
    pub fn validate(&self) -> Result<(), String> {
        for (name, v) in [("part_number", &self.part_number), ("made_by", &self.made_by), ("order_placeholder", &self.order_placeholder)] {
            if v.chars().count() > MAX_TEXT || v.chars().any(char::is_control) {
                return Err(format!("the template's `{name}` must be at most {MAX_TEXT} characters, with no control characters"));
            }
        }
        if !(self.height.is_finite() && (0.5..=5.0).contains(&self.height)) {
            return Err("the template's `height` must be 0.5 to 5 mm".into());
        }
        Ok(())
    }
}

/// A marking placed on the board (see the module notes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marking {
    pub id: u64,
    pub side: Side,
    /// The block's top left, in millimetres.
    pub at: [f64; 2],
    /// The value of `{number}` in the part number.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub number: String,
    pub template: MarkingTemplate,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub layer_marks: bool,
}

/// What the block prints that comes from the design: as the print export's title block has it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fields {
    pub title: String,
    pub revision: String,
    pub date: String,
    pub company: String,
    pub author: String,
}

/// The part number `pattern` with its placeholders filled.
pub fn part_number(pattern: &str, f: &Fields, number: &str) -> String {
    pattern
        .replace("{project}", &f.title)
        .replace("{title}", &f.title)
        .replace("{number}", number)
        .replace("{revision}", &f.revision)
        .replace("{date}", &f.date)
        .replace("{company}", &f.company)
        .replace("{author}", &f.author)
}

/// What `m` draws: its drawings (silkscreen and copper) and its keepouts, ids left 0.
pub fn shapes(m: &Marking, f: &Fields, copper_layers: u8) -> (Vec<BoardGraphic>, Vec<Keepout>) {
    let t = &m.template;
    let h = t.height;
    let bottom = m.side == Side::Bottom;
    let silk = if bottom { DrawLayer::SilkscreenBottom } else { DrawLayer::SilkscreenTop };
    let mut out: Vec<BoardGraphic> = Vec::new();
    let text = |layer: DrawLayer, at: [f64; 2], s: &str, height: f64, mirror: bool| BoardGraphic {
        id: 0,
        layer,
        mirror,
        graphic: Graphic { kind: GraphicType::Text, at: Some(at), text: s.to_string(), height: Some(height), width: height / 10.0, ..blank() },
        marking: Some(m.id),
    };
    let mut lines: Vec<String> = Vec::new();
    if t.title && !f.title.is_empty() {
        lines.push(f.title.clone());
    }
    let rev_date: Vec<String> =
        [(t.revision && !f.revision.is_empty()).then(|| format!("REV {}", f.revision)), (t.date && !f.date.is_empty()).then(|| f.date.clone())]
            .into_iter()
            .flatten()
            .collect();
    if !rev_date.is_empty() {
        lines.push(rev_date.join("  "));
    }
    let pn = part_number(&t.part_number, f, &m.number);
    if !pn.trim().is_empty() {
        lines.push(pn.clone());
    }
    if !t.made_by.trim().is_empty() {
        lines.push(t.made_by.clone());
    }
    if !t.order_placeholder.trim().is_empty() {
        lines.push(t.order_placeholder.clone());
    }
    let mut y = m.at[1];
    let x = m.at[0];
    let mut widest = 0.0f64;
    for l in &lines {
        y -= h * LINE;
        let w = h * 0.6 * l.chars().count() as f64;
        widest = widest.max(w);
        // Mirrored text grows to the left of its spot: start it at the block's right edge.
        out.push(text(silk, [x, y], l, h, bottom));
    }
    if bottom {
        for g in &mut out {
            if let Some(at) = g.graphic.at.as_mut() {
                at[0] += widest;
            }
        }
    }
    if t.serial_box {
        y -= h * LINE;
        let label = "S/N";
        let lw = h * 0.6 * 3.0 + h * 0.4;
        let (bx0, bx1) = if bottom { (x, x + widest.max(lw + 8.0) - lw) } else { (x + lw, x + widest.max(lw + 8.0)) };
        out.push(text(silk, [if bottom { bx1 + lw - h * 0.4 } else { x }, y], label, h, bottom));
        out.push(BoardGraphic {
            id: 0,
            layer: silk,
            mirror: false,
            graphic: Graphic {
                kind: GraphicType::Rect,
                from: Some([bx0, y - h * 0.2]),
                to: Some([bx1, y + h * 1.1]),
                width: (h / 10.0).max(0.15),
                ..blank()
            },
            marking: Some(m.id),
        });
        widest = widest.max(lw + 8.0);
    }
    if t.qr
        && let Some(cells) = qr_cells(format!("{} {}", pn.trim(), f.revision).trim())
    {
        let n = cells.len();
        let side = n as f64 * QR_MODULE;
        let left = if bottom { x - side - h } else { x + widest + h };
        let top = m.at[1] - h * 0.5;
        for (r, row) in cells.iter().enumerate() {
            let mut c = 0;
            while c < n {
                if !row.get(c).copied().unwrap_or(false) {
                    c += 1;
                    continue;
                }
                let start = c;
                while c < n && row.get(c).copied().unwrap_or(false) {
                    c += 1;
                }
                // Mirrored on the bottom, so it scans from below.
                let (c0, c1) = if bottom { (n - c, n - start) } else { (start, c) };
                let y0 = top - (r + 1) as f64 * QR_MODULE;
                out.push(BoardGraphic {
                    id: 0,
                    layer: silk,
                    mirror: false,
                    graphic: Graphic {
                        kind: GraphicType::Rect,
                        from: Some([left + c0 as f64 * QR_MODULE, y0]),
                        to: Some([left + c1 as f64 * QR_MODULE, y0 + QR_MODULE]),
                        width: 0.0,
                        fill: true,
                        ..blank()
                    },
                    marking: Some(m.id),
                });
            }
        }
    }
    let mut keepouts = Vec::new();
    if m.layer_marks {
        let layers = super::layers::stack(copper_layers);
        let names: Vec<String> =
            if layers.len() == 2 { vec!["TOP".into(), "BOT".into()] } else { (1..=layers.len()).map(|i| i.to_string()).collect() };
        let mut mx = x;
        let my = y - h * LINE - MARK_CLEAR - MARK_HEIGHT;
        for (layer, name) in layers.iter().zip(names) {
            let w = MARK_HEIGHT * 0.6 * name.chars().count() as f64;
            let under = *layer == CopperLayer::Bottom;
            let at = if under { [mx + w, my] } else { [mx, my] };
            out.push(text(DrawLayer::Copper(*layer), at, &name, MARK_HEIGHT, under));
            let (x0, y0, x1, y1) = (mx - MARK_CLEAR, my - MARK_CLEAR, mx + w + MARK_CLEAR, my + MARK_HEIGHT + MARK_CLEAR);
            keepouts.push(Keepout {
                id: 0,
                layers: vec![*layer],
                outline: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
                curves: Vec::new(),
                forbid: Forbid { tracks: false, vias: false, pours: true, components: false },
                marking: Some(m.id),
            });
            // Side by side: the next layer's mark starts past this one's cleared area.
            mx = x1 + MARK_CLEAR;
        }
    }
    (out, keepouts)
}

fn blank() -> Graphic {
    Graphic {
        kind: GraphicType::Line,
        layer: None,
        side: Side::Top,
        points: Vec::new(),
        from: None,
        to: None,
        center: None,
        radius: None,
        start: None,
        sweep: None,
        at: None,
        text: String::new(),
        height: None,
        rotation: 0.0,
        width: 0.15,
        fill: false,
    }
}

/// The QR code of `data` as rows of dark modules, a quiet zone of none (the silk round it is
/// bare); `None` for empty data or more than version 6 holds.
pub fn qr_cells(data: &str) -> Option<Vec<Vec<bool>>> {
    if data.is_empty() {
        return None;
    }
    let code = qrcode::QrCode::with_error_correction_level(data.as_bytes(), qrcode::EcLevel::M).ok()?;
    let n = code.width();
    if n > 41 {
        return None;
    }
    let colors = code.to_colors();
    Some(colors.chunks(n).map(|row| row.iter().map(|c| *c == qrcode::Color::Dark).collect()).collect())
}

/// `ecad`'s markings' drawings and keepouts made again from `fields` (the revision or the date
/// changed): each marking's old ones go, the new ones take fresh ids. A marking whose drawings
/// are all gone (deleted) goes too. Why not, when the board runs out of ids.
pub fn refresh(ecad: &mut Electronics, fields: &Fields) -> Result<(), String> {
    let board = &mut ecad.board;
    if board.markings.is_empty() {
        return Ok(());
    }
    let all: Vec<u64> = board.markings.iter().map(|m| m.id).collect();
    let alive: Vec<u64> = all.iter().copied().filter(|id| board.graphics.iter().any(|g| g.marking == Some(*id))).collect();
    board.markings.retain(|m| alive.contains(&m.id));
    // Everything of every marking goes (a deleted one's keepouts too); the live ones come back.
    board.graphics.retain(|g| g.marking.is_none_or(|id| !all.contains(&id)));
    board.keepouts.retain(|k| k.marking.is_none_or(|id| !all.contains(&id)));
    let markings = board.markings.clone();
    for m in &markings {
        stamp(board, m, fields)?;
    }
    Ok(())
}

/// Add `m`'s drawings and keepouts to `board`, with fresh ids.
pub fn stamp(board: &mut super::Board, m: &Marking, fields: &Fields) -> Result<(), String> {
    let (graphics, keepouts) = shapes(m, fields, board.copper_layers);
    for mut g in graphics {
        g.id = board.take_id()?;
        board.graphics.push(g);
    }
    for mut k in keepouts {
        k.id = board.take_id()?;
        board.keepouts.push(k);
    }
    Ok(())
}
