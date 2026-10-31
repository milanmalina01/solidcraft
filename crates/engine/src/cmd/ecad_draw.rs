//! Drawing one graphic at a time on a library symbol or a library footprint's layer: the commands
//! the Electronics toolbar's Line, Rectangle, Circle and Text tools run, and an agent can too. The whole-list
//! commands (`ecad.symbol.set_graphics`, `ecad.footprint.set_graphics`) stay for replacing a layer.

use serde_json::Value;
use solidcraft_doc::ecad::graphics::{DEFAULT_WIDTH, validate_all};
use solidcraft_doc::ecad::{Graphic, GraphicType};

use super::CommandSpec;
use super::ecad::{item_id, load_footprint, load_symbol, parse, reply, side, store_footprint, store_symbol};
use super::ecad_graphics::layer;
use crate::params::bad;
use crate::schema::ecad_component::{ComponentRef, Layer, Side};
use crate::schema::ecad_draw::{CircleParams, DrawResult, LineParams, RectParams, TextParams};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.draw.line", "Line", crate::checked!("ecad.draw.line", line))
        .icon("line")
        .doc_only()
        .contract(&LINE_CONTRACT)
        .output(output_schema_for::<DrawResult>)
        .params(
            "symbol: id|name | footprint: id|name with layer: silkscreen|fab|courtyard and side? (top); points: [[x, y], …] (2 or more, mm); width? (0.15) → {symbol | footprint, count}",
        ),
    CommandSpec::new("ecad.draw.rect", "Rectangle", crate::checked!("ecad.draw.rect", rect))
        .icon("rect")
        .doc_only()
        .contract(&RECT_CONTRACT)
        .output(output_schema_for::<DrawResult>)
        .params("symbol | footprint, layer, side? (as for ecad.draw.line); from: [x, y]; to: [x, y] (opposite corners, mm); width? (0.15); fill? → {symbol | footprint, count}"),
    CommandSpec::new("ecad.draw.circle", "Circle", crate::checked!("ecad.draw.circle", circle))
        .icon("circle")
        .doc_only()
        .contract(&CIRCLE_CONTRACT)
        .output(output_schema_for::<DrawResult>)
        .params("symbol | footprint, layer, side? (as for ecad.draw.line); center: [x, y]; radius (mm); width? (0.15); fill? → {symbol | footprint, count}"),
    CommandSpec::new("ecad.draw.text", "Text", crate::checked!("ecad.draw.text", text))
        .icon("text")
        .doc_only()
        .contract(&TEXT_CONTRACT)
        .output(output_schema_for::<DrawResult>)
        .params(
            "symbol | footprint, layer, side? (as for ecad.draw.line); at: [x, y]; text ({designator} and {value} stand for the placed part's); height (mm); rotation? (degrees) → {symbol | footprint, count}",
        ),
];

const LINE_CONTRACT: TypedContract<LineParams> = TypedContract::new();
const RECT_CONTRACT: TypedContract<RectParams> = TypedContract::new();
const CIRCLE_CONTRACT: TypedContract<CircleParams> = TypedContract::new();
const TEXT_CONTRACT: TypedContract<TextParams> = TypedContract::new();

/// An empty graphic of `kind`, with no layer yet.
pub(super) fn blank(kind: GraphicType, width: Option<f64>) -> Graphic {
    Graphic {
        kind,
        layer: None,
        side: Default::default(),
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
        width: width.unwrap_or(DEFAULT_WIDTH),
        fill: false,
    }
}

/// What a graphic goes on: a library symbol, or a library footprint's layer and side.
struct On<'a> {
    symbol: Option<&'a ComponentRef>,
    footprint: Option<&'a ComponentRef>,
    layer: Option<Layer>,
    side: Option<Side>,
}

/// Add `g` (its layer and side still to set) to the symbol or the footprint layer `on` names, and
/// store it.
fn add(s: &mut Session, on: On, mut g: Graphic, cmd: &str) -> Result<Value> {
    match (on.symbol, on.footprint, on.layer) {
        (Some(_), Some(_), _) => Err(bad(cmd, "give `symbol` or `footprint`, not both")),
        (None, None, _) => Err(bad(cmd, "give the `symbol` to draw on, or the `footprint` and its `layer`")),
        (Some(_), None, Some(_)) => Err(bad(cmd, "a symbol has no layers: leave out `layer`, or draw on a `footprint`")),
        (Some(_), None, None) if on.side.is_some() => Err(bad(cmd, "a symbol has no sides: leave out `side`")),
        (None, Some(_), None) => Err(bad(cmd, "give the footprint `layer`: silkscreen, fab or courtyard")),
        (Some(r), None, None) => {
            let mut sym = load_symbol(s, r, cmd)?;
            sym.graphics.push(g);
            validate_all(&sym.graphics, false).map_err(|e| bad(cmd, e))?;
            let count = sym.graphics.len();
            let symbol = Some(item_id(sym.id, &sym.name));
            store_symbol(s, sym, &[], false, cmd)?;
            reply(DrawResult { symbol, footprint: None, count, param_warnings: Vec::new() })
        }
        (None, Some(r), Some(l)) => {
            let mut fp = load_footprint(s, r, cmd)?;
            g.layer = Some(layer(l));
            g.side = on.side.map(side).unwrap_or_default();
            let at = (g.layer, g.side);
            fp.graphics.push(g);
            validate_all(&fp.graphics, true).map_err(|e| bad(cmd, e))?;
            let count = fp.graphics.iter().filter(|x| (x.layer, x.side) == at).count();
            let footprint = Some(item_id(fp.id, &fp.name));
            store_footprint(s, fp, &[], false, cmd)?;
            reply(DrawResult { symbol: None, footprint, count, param_warnings: Vec::new() })
        }
    }
}

fn line(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.draw.line";
    let p: LineParams = parse(CMD, p)?;
    if p.points.len() < 2 {
        return Err(bad(CMD, "a line needs two or more `points`"));
    }
    let g = Graphic { points: p.points, ..blank(GraphicType::Line, p.width) };
    add(s, On { symbol: p.symbol.as_ref(), footprint: p.footprint.as_ref(), layer: p.layer, side: p.side }, g, CMD)
}

fn rect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.draw.rect";
    let p: RectParams = parse(CMD, p)?;
    let g = Graphic { from: Some(p.from), to: Some(p.to), fill: p.fill.unwrap_or(false), ..blank(GraphicType::Rect, p.width) };
    add(s, On { symbol: p.symbol.as_ref(), footprint: p.footprint.as_ref(), layer: p.layer, side: p.side }, g, CMD)
}

fn circle(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.draw.circle";
    let p: CircleParams = parse(CMD, p)?;
    let g = Graphic { center: Some(p.center), radius: Some(p.radius), fill: p.fill.unwrap_or(false), ..blank(GraphicType::Circle, p.width) };
    add(s, On { symbol: p.symbol.as_ref(), footprint: p.footprint.as_ref(), layer: p.layer, side: p.side }, g, CMD)
}

fn text(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.draw.text";
    let p: TextParams = parse(CMD, p)?;
    // Text strokes are a tenth of its height.
    let width = (p.height.is_finite() && p.height > 0.0).then_some(p.height / 10.0);
    let g = Graphic { at: Some(p.at), text: p.text, height: Some(p.height), rotation: p.rotation.unwrap_or(0.0), ..blank(GraphicType::Text, width) };
    add(s, On { symbol: p.symbol.as_ref(), footprint: p.footprint.as_ref(), layer: p.layer, side: p.side }, g, CMD)
}
