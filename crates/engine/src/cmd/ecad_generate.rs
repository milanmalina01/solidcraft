//! Generators: a whole footprint for a standard package from its drawing's few dimensions, and a
//! whole symbol from a pin list. They replace what the component had, so running one again with
//! other numbers is how a part is tuned; everything they make can then be edited like hand-made
//! pins, pads and graphics.

use serde_json::Value;
use solidcraft_doc::ecad::BoardLayer;

use super::CommandSpec;
use super::ecad::{electrical, id_of, load, parse, reply, store};
use super::ecad_generate_geometry::{self as geo, Drawn, Land, PinSide as GeoSide};
use crate::params::bad;
use crate::schema::ecad_generate::{FootprintParams, FootprintResult, Package, PinSide, SymbolParams, SymbolResult, SymbolStyle};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.footprint.generate", "Generate Footprint", crate::checked!("ecad.footprint.generate", footprint))
        .icon("new")
        .doc_only()
        .contract(&FOOTPRINT_CONTRACT)
        .output(output_schema_for::<FootprintResult>)
        .params(
            "component: id|name; package: chip (size: 0603…) | sot23 | soic (pins) | qfn (pins, body, thermal?) | dip (pins) | header (pins, rows?); pitch?, span?, lead?, lead_width?, body?, drill?, pad?, toe?, heel?, side?, margin? (mm) → {component, pads, courtyard, pin_map_cleared}. Replaces the pads and the fab, silkscreen and courtyard graphics",
        ),
    CommandSpec::new("ecad.symbol.generate", "Generate Symbol", crate::checked!("ecad.symbol.generate", symbol))
        .icon("new")
        .doc_only()
        .contract(&SYMBOL_CONTRACT)
        .output(output_schema_for::<SymbolResult>)
        .params(
            "component: id|name; style: box (pins: [{number, name?, electrical?, side? left|right|top|bottom}]) | resistor | capacitor → {component, pins, bbox, pin_map_cleared}. Replaces the pins and the body; pin ends land on the 2.54 mm grid",
        ),
];

const FOOTPRINT_CONTRACT: TypedContract<FootprintParams> = TypedContract::new();
const SYMBOL_CONTRACT: TypedContract<SymbolParams> = TypedContract::new();

/// The most pins a generated package has.
const MAX_PACKAGE_PINS: u32 = 1024;

/// A length the caller gave or the package's default: finite, more than 0 and at most 100 mm.
fn length(cmd: &str, key: &str, given: Option<f64>, default: f64) -> Result<f64> {
    let v = given.unwrap_or(default);
    if !(v.is_finite() && v > 0.0 && v <= 100.0) {
        return Err(bad(cmd, format!("`{key}` must be more than 0 and at most 100 mm")));
    }
    Ok(v)
}

/// A land rule the caller gave or the package's default: 0 to 5 mm.
fn reach(cmd: &str, key: &str, given: Option<f64>, default: f64) -> Result<f64> {
    let v = given.unwrap_or(default);
    if !(v.is_finite() && (0.0..=5.0).contains(&v)) {
        return Err(bad(cmd, format!("`{key}` must be 0 to 5 mm")));
    }
    Ok(v)
}

fn size2(cmd: &str, key: &str, given: Option<[f64; 2]>, default: [f64; 2]) -> Result<[f64; 2]> {
    // Only a size the caller gave is held to the limits: a long header's own body may be longer.
    let Some([x, y]) = given else { return Ok(default) };
    Ok([length(cmd, key, Some(x), 1.0)?, length(cmd, key, Some(y), 1.0)?])
}

/// How many pins: given, within `1..=MAX_PACKAGE_PINS` and a multiple of `of`.
fn pins(cmd: &str, given: Option<u32>, of: u32, what: &str) -> Result<usize> {
    let Some(n) = given else { return Err(bad(cmd, format!("a {what} needs `pins`"))) };
    if n == 0 || n > MAX_PACKAGE_PINS || n % of != 0 {
        return Err(bad(cmd, format!("a {what} has 1 to {MAX_PACKAGE_PINS} pins, a multiple of {of}")));
    }
    Ok(n as usize)
}

fn land(cmd: &str, p: &FootprintParams, [toe, heel, side]: [f64; 3]) -> Result<Land> {
    Ok(Land { toe: reach(cmd, "toe", p.toe, toe)?, heel: reach(cmd, "heel", p.heel, heel)?, side: reach(cmd, "side", p.side, side)? })
}

fn drawn(cmd: &str, p: &FootprintParams) -> Result<Drawn> {
    Ok(match p.package {
        Package::Chip => {
            let code = p.size.as_deref().unwrap_or_default();
            let Some([l, w, t]) = geo::chip_size(code) else {
                return Err(bad(cmd, format!("a chip needs a `size` code: 0201, 0402, 0603, 0805, 1206, 1210, 2010 or 2512 (got `{code}`)")));
            };
            let [l, w] = size2(cmd, "body", p.body, [l, w])?;
            let t = length(cmd, "lead", p.lead, t)?;
            if 2.0 * t >= l {
                return Err(bad(cmd, "a chip's terminals (`lead`) must be shorter than half its length"));
            }
            geo::chip([l, w, t], land(cmd, p, [0.3, 0.0, 0.05])?)
        }
        Package::Sot23 => geo::sot23(
            length(cmd, "pitch", p.pitch, 0.95)?,
            length(cmd, "span", p.span, 2.4)?,
            length(cmd, "lead", p.lead, 0.45)?,
            length(cmd, "lead_width", p.lead_width, 0.4)?,
            size2(cmd, "body", p.body, [2.9, 1.3])?,
            land(cmd, p, [0.3, 0.3, 0.05])?,
        ),
        Package::Soic => {
            let n = pins(cmd, p.pins, 2, "SOIC")?;
            let pitch = length(cmd, "pitch", p.pitch, 1.27)?;
            let body = size2(cmd, "body", p.body, [3.9, (n / 2) as f64 * pitch - 0.2])?;
            geo::dual(
                [n / 2, n / 2],
                pitch,
                length(cmd, "span", p.span, 6.0)?,
                length(cmd, "lead", p.lead, 0.835)?,
                length(cmd, "lead_width", p.lead_width, 0.41)?,
                body,
                land(cmd, p, [0.3, 0.3, 0.05])?,
            )
        }
        Package::Qfn => {
            let n = pins(cmd, p.pins, 4, "QFN")?;
            let Some(body) = p.body else { return Err(bad(cmd, "a QFN needs its `body` size, such as [3, 3]")) };
            let body = size2(cmd, "body", Some(body), body)?;
            let thermal = match p.thermal {
                Some(t) => Some(size2(cmd, "thermal", Some(t), t)?),
                None => None,
            };
            geo::qfn(
                n / 4,
                length(cmd, "pitch", p.pitch, 0.5)?,
                body,
                length(cmd, "lead", p.lead, 0.4)?,
                length(cmd, "lead_width", p.lead_width, 0.25)?,
                thermal,
                land(cmd, p, [0.3, 0.05, 0.05])?,
            )
        }
        Package::Dip | Package::Header => {
            let dip = p.package == Package::Dip;
            let rows = if dip { 2 } else { p.rows.unwrap_or(1) };
            if !(1..=2).contains(&rows) {
                return Err(bad(cmd, "a header has 1 or 2 `rows`"));
            }
            let n = pins(cmd, p.pins, rows, if dip { "DIP" } else { "header" })?;
            let per_row = n / rows as usize;
            let pitch = length(cmd, "pitch", p.pitch, 2.54)?;
            let spacing = length(cmd, "span", p.span, if dip { 7.62 } else { 2.54 })?;
            let pad = length(cmd, "pad", p.pad, if dip { 1.6 } else { 1.7 })?;
            let drill = length(cmd, "drill", p.drill, if dip { 0.8 } else { 1.0 })?;
            let body = if dip { [spacing - 1.27, per_row as f64 * pitch] } else { [rows as f64 * pitch, per_row as f64 * pitch] };
            geo::through_hole(dip, rows as usize, per_row, pitch, spacing, [pad, drill], size2(cmd, "body", p.body, body)?)
        }
    })
}

fn footprint(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.footprint.generate";
    let p: FootprintParams = parse(CMD, p)?;
    let mut c = load(s, &p.component, CMD)?;
    let Drawn { pads, graphics } = drawn(CMD, &p)?;
    let count = pads.len();
    c.footprint.pads = pads;
    c.footprint.graphics.retain(|g| !matches!(g.layer, Some(BoardLayer::Fab | BoardLayer::Silkscreen | BoardLayer::Courtyard)));
    c.footprint.graphics.extend(graphics);
    let courtyard =
        super::ecad_graphics::make_courtyard(&mut c, p.margin.unwrap_or(0.25), solidcraft_doc::ecad::Side::Top).map_err(|e| bad(CMD, e))?;
    let pin_map_cleared = super::ecad::clear_stale_map(&mut c);
    let component = id_of(&c);
    store(s, c, CMD)?;
    reply(FootprintResult { component, pads: count, courtyard, pin_map_cleared, param_warnings: Vec::new() })
}

fn symbol(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.symbol.generate";
    let p: SymbolParams = parse(CMD, p)?;
    let mut c = load(s, &p.component, CMD)?;
    let (pins, graphics) = match p.style {
        SymbolStyle::Resistor | SymbolStyle::Capacitor => {
            if p.pins.is_some() {
                return Err(bad(CMD, "a resistor or capacitor symbol has pins 1 and 2: leave out `pins`"));
            }
            geo::two_terminal(p.style == SymbolStyle::Capacitor)
        }
        SymbolStyle::Box => {
            let list = p.pins.unwrap_or_default();
            if list.is_empty() || list.len() > MAX_PACKAGE_PINS as usize {
                return Err(bad(CMD, format!("a box symbol needs 1 to {MAX_PACKAGE_PINS} `pins`")));
            }
            let half = list.len().div_ceil(2);
            let pins: Vec<_> = list
                .into_iter()
                .enumerate()
                .map(|(i, g)| {
                    let side = match g.side {
                        Some(PinSide::Left) => GeoSide::Left,
                        Some(PinSide::Right) => GeoSide::Right,
                        Some(PinSide::Top) => GeoSide::Top,
                        Some(PinSide::Bottom) => GeoSide::Bottom,
                        None if i < half => GeoSide::Left,
                        None => GeoSide::Right,
                    };
                    (g.number.text(), g.name.unwrap_or_default(), g.electrical.map(electrical).unwrap_or_default(), side)
                })
                .collect();
            geo::box_symbol(&pins)
        }
    };
    for (i, pin) in pins.iter().enumerate() {
        pin.validate().map_err(|e| bad(CMD, format!("pins[{i}]: {e}")))?;
    }
    let count = pins.len();
    c.symbol.pins = pins;
    c.symbol.graphics = graphics;
    let pin_map_cleared = super::ecad::clear_stale_map(&mut c);
    let bbox = c.symbol.bbox();
    let component = id_of(&c);
    store(s, c, CMD)?;
    reply(SymbolResult { component, pins: count, bbox, pin_map_cleared, param_warnings: Vec::new() })
}

#[cfg(test)]
#[path = "ecad_generate_tests.rs"]
mod tests;
