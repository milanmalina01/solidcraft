//! Drawing on the board's layers, one graphic at a time: lines, rectangles, circles and text on the
//! silkscreen, the documentation layer or copper (drawn copper joins nothing). Each drawing is a
//! board object of its own: moved, turned, flipped and deleted with the `ecad.board.*` commands.
//! Text on a bottom layer is made mirrored, so it reads right seen from the bottom. A drawing's
//! layer, text, size and fill change with `ecad.board.draw.edit`.

use serde_json::Value;
use solidcraft_doc::ecad::{BoardGraphic, DrawLayer, Graphic, GraphicType};

use super::CommandSpec;
use super::ecad::{parse, reply};
use super::ecad_board::{board_point, board_result, no_layer, store_board};
use super::ecad_draw::blank;
use crate::params::bad;
use crate::schema::ecad_board::BoardResult;
use crate::schema::ecad_board_edit::{CircleParams, DrawEditParams, LineParams, RectParams, TextParams};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.board.draw.line", "Line", crate::checked!("ecad.board.draw.line", line))
        .icon("line")
        .doc_only()
        .contract(&LINE_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "layer: silkscreen_top|silkscreen_bottom|documentation|<copper layer>; points: [[x, y], …] (2 or more, mm); width? (0.15) → {created: [{id, kind: graphic}]}",
        ),
    CommandSpec::new("ecad.board.draw.rect", "Rectangle", crate::checked!("ecad.board.draw.rect", rect))
        .icon("rect")
        .doc_only()
        .contract(&RECT_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params("layer (as for ecad.board.draw.line); from: [x, y]; to: [x, y] (opposite corners, mm); width? (0.15); fill? → {created}"),
    CommandSpec::new("ecad.board.draw.circle", "Circle", crate::checked!("ecad.board.draw.circle", circle))
        .icon("circle")
        .doc_only()
        .contract(&CIRCLE_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params("layer (as for ecad.board.draw.line); center: [x, y]; radius (mm); width? (0.15); fill? → {created}"),
    CommandSpec::new("ecad.board.draw.text", "Text", crate::checked!("ecad.board.draw.text", text))
        .icon("text")
        .doc_only()
        .contract(&TEXT_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "layer (as for ecad.board.draw.line); at: [x, y] (its lower left before the turn); text; height (mm); rotation? (degrees); mirror? (mirrored on the bottom layers when left out) → {created}",
        ),
    CommandSpec::new("ecad.board.draw.edit", "Edit Drawing", crate::checked!("ecad.board.draw.edit", edit))
        .icon("edit")
        .doc_only()
        .contract(&EDIT_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "graphic: id; layer? (as for ecad.board.draw.line); text?, height? (mm), mirror? (text only; a text moved to another layer is mirrored on the bottom layers unless mirror is given); width? (mm; lines and shapes); fill? (rectangles and circles) → {changed}",
        ),
];

const LINE_CONTRACT: TypedContract<LineParams> = TypedContract::new();
const RECT_CONTRACT: TypedContract<RectParams> = TypedContract::new();
const CIRCLE_CONTRACT: TypedContract<CircleParams> = TypedContract::new();
const TEXT_CONTRACT: TypedContract<TextParams> = TypedContract::new();
const EDIT_CONTRACT: TypedContract<DrawEditParams> = TypedContract::new();

/// Add `g` on `layer` as a new board drawing and store it.
fn add(s: &mut Session, layer: DrawLayer, mirror: bool, g: Graphic, cmd: &str) -> Result<Value> {
    let old = s.doc.ecad.board.clone();
    if !layer.exists_in(old.copper_layers) {
        return Err(bad(cmd, no_layer(&old, layer.name())));
    }
    let mut board = old.clone();
    let id = board.take_id().map_err(|e| bad(cmd, e))?;
    board.graphics.push(BoardGraphic { id, layer, mirror, graphic: g, marking: None });
    store_board(s, board, cmd)?;
    reply(board_result(s, &old))
}

fn line(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.draw.line";
    let p: LineParams = parse(CMD, p)?;
    if p.points.len() < 2 {
        return Err(bad(CMD, "a line needs two or more `points`"));
    }
    let points = p.points.iter().enumerate().map(|(i, q)| board_point(*q, CMD, &format!("points[{i}]"))).collect::<Result<Vec<_>>>()?;
    add(s, p.layer.0, false, Graphic { points, ..blank(GraphicType::Line, p.width) }, CMD)
}

fn rect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.draw.rect";
    let p: RectParams = parse(CMD, p)?;
    let (from, to) = (board_point(p.from, CMD, "`from`")?, board_point(p.to, CMD, "`to`")?);
    let g = Graphic { from: Some(from), to: Some(to), fill: p.fill.unwrap_or(false), ..blank(GraphicType::Rect, p.width) };
    add(s, p.layer.0, false, g, CMD)
}

fn circle(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.draw.circle";
    let p: CircleParams = parse(CMD, p)?;
    let center = board_point(p.center, CMD, "`center`")?;
    let g = Graphic { center: Some(center), radius: Some(p.radius), fill: p.fill.unwrap_or(false), ..blank(GraphicType::Circle, p.width) };
    add(s, p.layer.0, false, g, CMD)
}

fn text(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.draw.text";
    let p: TextParams = parse(CMD, p)?;
    let at = board_point(p.at, CMD, "`at`")?;
    // Text strokes are a tenth of its height.
    let width = (p.height.is_finite() && p.height > 0.0).then_some(p.height / 10.0);
    let g = Graphic { at: Some(at), text: p.text, height: Some(p.height), rotation: p.rotation.unwrap_or(0.0), ..blank(GraphicType::Text, width) };
    let layer = p.layer.0;
    add(s, layer, p.mirror.unwrap_or(layer.is_bottom()), g, CMD)
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.draw.edit";
    let p: DrawEditParams = parse(CMD, p)?;
    let old = s.doc.ecad.board.clone();
    let mut board = old.clone();
    let Some(g) = board.graphics.iter_mut().find(|g| g.id == p.graphic) else {
        return Err(bad(CMD, format!("the board has no drawing with the id {}", p.graphic)));
    };
    let text = g.graphic.kind == GraphicType::Text;
    if !text && (p.text.is_some() || p.height.is_some() || p.mirror.is_some()) {
        return Err(bad(CMD, "`text`, `height` and `mirror` are for text"));
    }
    if text && (p.width.is_some() || p.fill.is_some()) {
        return Err(bad(CMD, "text has no `width` or `fill` of its own: give its `height`"));
    }
    if let Some(layer) = p.layer {
        if !layer.0.exists_in(old.copper_layers) {
            return Err(bad(CMD, no_layer(&old, layer.0.name())));
        }
        if text && layer.0 != g.layer && p.mirror.is_none() {
            g.mirror = layer.0.is_bottom();
        }
        g.layer = layer.0;
    }
    if let Some(t) = p.text {
        g.graphic.text = t;
    }
    if let Some(h) = p.height {
        g.graphic.height = Some(h);
        // Text strokes are a tenth of its height.
        if h.is_finite() && h > 0.0 {
            g.graphic.width = h / 10.0;
        }
    }
    if let Some(m) = p.mirror {
        g.mirror = m;
    }
    if let Some(w) = p.width {
        g.graphic.width = w;
    }
    if let Some(f) = p.fill {
        g.graphic.fill = f;
    }
    store_board(s, board, CMD)?;
    reply(board_result(s, &old))
}
