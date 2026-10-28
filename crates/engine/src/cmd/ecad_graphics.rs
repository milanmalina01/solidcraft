//! Graphics on an electronic component: its symbol's body, and its footprint's silkscreen,
//! fabrication and courtyard layers. Each call replaces one whole set, so it can be run again with
//! the same result; graphics never connect anything.

use serde_json::Value;
use solidcraft_doc::ecad::graphics::{DEFAULT_WIDTH, graphics_bbox, shapes_bbox, validate_all};
use solidcraft_doc::ecad::{BoardLayer, ElectronicComponent, Graphic, GraphicType};

use super::CommandSpec;
use super::ecad::{id_of, load, parse, reply, side, store};
use crate::params::bad;
use crate::schema::ecad_component::{CourtyardOut, GraphicKind, GraphicSpec, Layer};
use crate::schema::ecad_graphics::{CourtyardParams, CourtyardResult, FootprintParams, GraphicsResult, SymbolParams};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.symbol.set_graphics", "Set Symbol Graphics", crate::checked!("ecad.symbol.set_graphics", symbol))
        .icon("edit")
        .doc_only()
        .contract(&SYMBOL_CONTRACT)
        .output(output_schema_for::<GraphicsResult>)
        .params(
            "component: id|name; graphics: [{type: line|rect|circle|arc|polygon|text, points?, from?, to?, center?, radius?, start?, sweep?, at?, text?, height?, rotation?, width? (0.15), fill?}] (replaces the body) → {component, count, bbox}",
        ),
    CommandSpec::new("ecad.footprint.set_graphics", "Set Footprint Graphics", crate::checked!("ecad.footprint.set_graphics", footprint))
        .icon("edit")
        .doc_only()
        .contract(&FOOTPRINT_CONTRACT)
        .output(output_schema_for::<GraphicsResult>)
        .params("component: id|name; layer: silkscreen|fab|courtyard; side? (top); graphics: [as for the symbol] (replaces that layer) → {component, count, bbox}"),
    CommandSpec::new("ecad.footprint.courtyard", "Make Courtyard", crate::checked!("ecad.footprint.courtyard", courtyard))
        .icon("new")
        .doc_only()
        .contract(&COURTYARD_CONTRACT)
        .output(output_schema_for::<CourtyardResult>)
        .params("component: id|name; margin? (0.25 mm); side? (top) → {component, courtyard: {from, to, area}}: a rectangle round the pads and fab outline"),
];

const SYMBOL_CONTRACT: TypedContract<SymbolParams> = TypedContract::new();
const FOOTPRINT_CONTRACT: TypedContract<FootprintParams> = TypedContract::new();
const COURTYARD_CONTRACT: TypedContract<CourtyardParams> = TypedContract::new();

/// The width of a courtyard's line, in millimetres.
const COURTYARD_WIDTH: f64 = 0.05;

pub(super) fn layer(l: Layer) -> BoardLayer {
    match l {
        Layer::Silkscreen => BoardLayer::Silkscreen,
        Layer::Fab => BoardLayer::Fab,
        Layer::Courtyard => BoardLayer::Courtyard,
    }
}

pub(super) fn layer_out(l: BoardLayer) -> Layer {
    match l {
        BoardLayer::Silkscreen => Layer::Silkscreen,
        BoardLayer::Fab => Layer::Fab,
        BoardLayer::Courtyard => Layer::Courtyard,
    }
}

fn kind(k: GraphicKind) -> GraphicType {
    match k {
        GraphicKind::Line => GraphicType::Line,
        GraphicKind::Rect => GraphicType::Rect,
        GraphicKind::Circle => GraphicType::Circle,
        GraphicKind::Arc => GraphicType::Arc,
        GraphicKind::Polygon => GraphicType::Polygon,
        GraphicKind::Text => GraphicType::Text,
    }
}

fn kind_out(k: GraphicType) -> GraphicKind {
    match k {
        GraphicType::Line => GraphicKind::Line,
        GraphicType::Rect => GraphicKind::Rect,
        GraphicType::Circle => GraphicKind::Circle,
        GraphicType::Arc => GraphicKind::Arc,
        GraphicType::Polygon => GraphicKind::Polygon,
        GraphicType::Text => GraphicKind::Text,
    }
}

/// A graphic as the design saves it, on `on` (a layer and side), or on the symbol with `None`.
fn graphic(g: GraphicSpec, on: Option<(BoardLayer, solidcraft_doc::ecad::Side)>) -> Graphic {
    Graphic {
        kind: kind(g.kind),
        layer: on.map(|(l, _)| l),
        side: on.map(|(_, s)| s).unwrap_or_default(),
        points: g.points.unwrap_or_default(),
        from: g.from,
        to: g.to,
        center: g.center,
        radius: g.radius,
        start: g.start,
        sweep: g.sweep,
        at: g.at,
        text: g.text.unwrap_or_default(),
        height: g.height,
        rotation: g.rotation.unwrap_or(0.0),
        width: g.width.unwrap_or(DEFAULT_WIDTH),
        fill: g.fill.unwrap_or(false),
    }
}

/// A saved graphic in the shape the commands take.
pub(super) fn graphic_out(g: &Graphic) -> GraphicSpec {
    GraphicSpec {
        kind: kind_out(g.kind),
        points: (!g.points.is_empty()).then(|| g.points.clone()),
        from: g.from,
        to: g.to,
        center: g.center,
        radius: g.radius,
        start: g.start,
        sweep: g.sweep,
        at: g.at,
        text: (!g.text.is_empty()).then(|| g.text.clone()),
        height: g.height,
        rotation: (g.rotation != 0.0).then_some(g.rotation),
        width: Some(g.width),
        fill: g.fill.then_some(true),
    }
}

fn symbol(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.symbol.set_graphics";
    let p: SymbolParams = parse(CMD, p)?;
    let mut c = load(s, &p.component, CMD)?;
    let graphics: Vec<Graphic> = p.graphics.into_iter().map(|g| graphic(g, None)).collect();
    validate_all(&graphics, false).map_err(|e| bad(CMD, e))?;
    let bbox = graphics_bbox(&graphics);
    let count = graphics.len();
    c.symbol.graphics = graphics;
    let component = id_of(&c);
    store(s, c, CMD)?;
    reply(GraphicsResult { component, count, bbox, param_warnings: Vec::new() })
}

fn footprint(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.footprint.set_graphics";
    let p: FootprintParams = parse(CMD, p)?;
    let mut c = load(s, &p.component, CMD)?;
    let on = (layer(p.layer), p.side.map(side).unwrap_or_default());
    let graphics: Vec<Graphic> = p.graphics.into_iter().map(|g| graphic(g, Some(on))).collect();
    validate_all(&graphics, true).map_err(|e| bad(CMD, e))?;
    let bbox = graphics_bbox(&graphics);
    let count = graphics.len();
    c.footprint.graphics.retain(|g| (g.layer, g.side) != (Some(on.0), on.1));
    c.footprint.graphics.extend(graphics);
    let component = id_of(&c);
    store(s, c, CMD)?;
    reply(GraphicsResult { component, count, bbox, param_warnings: Vec::new() })
}

fn courtyard(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.footprint.courtyard";
    let p: CourtyardParams = parse(CMD, p)?;
    let mut c = load(s, &p.component, CMD)?;
    let courtyard = make_courtyard(&mut c, p.margin.unwrap_or(0.25), p.side.map(side).unwrap_or_default()).map_err(|e| bad(CMD, e))?;
    let component = id_of(&c);
    store(s, c, CMD)?;
    reply(CourtyardResult { component, courtyard, param_warnings: Vec::new() })
}

/// Replace the courtyard on `on_side` with a rectangle `margin` outside the pads and the
/// fabrication outline on that side.
pub(super) fn make_courtyard(
    c: &mut ElectronicComponent,
    margin: f64,
    on_side: solidcraft_doc::ecad::Side,
) -> std::result::Result<CourtyardOut, String> {
    if !(margin.is_finite() && (0.0..=5.0).contains(&margin)) {
        return Err("`margin` must be 0 to 5 mm".into());
    }
    let pads = c.footprint.pads.iter().filter(|pad| pad.kind != solidcraft_doc::ecad::PadKind::Smd || pad.side == on_side).flat_map(|pad| pad.bbox());
    let fab = shapes_bbox(c.footprint.layer(BoardLayer::Fab, on_side)).map(|[a, b]| vec![a, b]).unwrap_or_default();
    let Some([[x0, y0], [x1, y1]]) = solidcraft_doc::ecad::bbox(pads.chain(fab)) else {
        return Err(format!("`{}` has no pads or fabrication outline on that side to put a courtyard round", c.name));
    };
    let (from, to) = ([x0 - margin, y0 - margin], [x1 + margin, y1 + margin]);
    let mut rect = super::ecad_generate_geometry::rect(Some(BoardLayer::Courtyard), from, to, COURTYARD_WIDTH);
    rect.side = on_side;
    rect.validate()?;
    c.footprint.graphics.retain(|g| (g.layer, g.side) != (Some(BoardLayer::Courtyard), on_side));
    c.footprint.graphics.push(rect);
    Ok(CourtyardOut { from, to, area: (to[0] - from[0]) * (to[1] - from[1]) })
}

#[cfg(test)]
#[path = "ecad_graphics_tests.rs"]
mod tests;
