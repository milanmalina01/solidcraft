//! Generators: a whole library footprint for a standard package from its drawing's few
//! dimensions, and a whole library symbol from a pin list. They replace what the item had (or make
//! it, given a new name), so running one again with
//! other numbers is how a part is tuned; everything they make can then be edited like hand-made
//! pins, pads and graphics.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use solidcraft_doc::ecad::{BoardLayer, Graphic, Pin, PinType, SymbolItem};

use super::CommandSpec;
use super::ecad::{
    commit, electrical, find, id_of, item_id, load_footprint, load_symbol, parse, reply, set_symbol, set_variant, store_footprint, store_symbol,
};
use super::ecad_generate_geometry::{self as geo, Drawn, Land, PinSide as GeoSide};
use super::ecad_generate_shapes::{self as shapes, Kind, Shape};
use super::ecad_library::{free_item_name, new_footprint, new_symbol};
use crate::params::bad;
use crate::schema::ecad_component::{ComponentRef, Number};
use crate::schema::ecad_generate::{
    CommonPin, FootprintParams, FootprintResult, GeneratedPin, GeneratedPinOut, Package, PinSide, SymbolParams, SymbolResult, SymbolStyle,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.footprint.generate", "Generate Footprint", crate::checked!("ecad.footprint.generate", footprint))
        .icon("pattern_rect")
        .doc_only()
        .contract(&FOOTPRINT_CONTRACT)
        .output(output_schema_for::<FootprintResult>)
        .params(
            "footprint: id|name (a name no footprint has makes one); component?: id|name, variant? (gives it to that variant: the first when left out, made when missing); package: chip (size: 0603…) | sot23 | soic (pins) | qfn (pins, body, thermal?) | dip (pins) | header (pins, rows?); pitch?, span?, lead?, lead_width?, body?, drill?, pad?, toe?, heel?, side?, margin? (mm) → {footprint, created, component?, variant?, pads, courtyard, pin_map_cleared}. Replaces the pads and the fab, silkscreen and courtyard graphics",
        ),
    CommandSpec::new("ecad.symbol.generate", "Generate Symbol", crate::checked!("ecad.symbol.generate", symbol))
        .icon("library")
        .doc_only()
        .contract(&SYMBOL_CONTRACT)
        .output(output_schema_for::<SymbolResult>)
        .params(
            "symbol: id|name (a name no symbol has makes one); component?: id|name (it shows the symbol then); style: box (pins: [{number, name?, electrical?, side? left|right|top|bottom}]) | resistor | capacitor | polarized_capacitor | inductor | ferrite | fuse | diode | led (count? 1-4 LEDs, common? anode|cathode) | zener | schottky | tvs (bidirectional?) | crystal (count? 2|4) | resonator | switch (count? 2|4) | battery | antenna | test_point | npn | pnp | nmos | pmos; a shape's pins?: [{number, name?, electrical?}] one for each of its pins (a name that is a pin's role, A K B C E G D S + -, picks it; else in pin_list order; the names are kept, not drawn on the pins, but several LEDs in one symbol print a name that is not its role beside its LED, such as K_RED); renames?: {old: new} → {symbol, created, component?, pins, pin_list: [{number, name, role?, at}], bbox, pin_map_cleared, detached?, netlist_changed?: [{net, lost, gained, more?}] (the nets whose pins the edit changed on the schematic, should a moved pin land on another wire or leave its own), texts_reset?: [designators] (parts whose moved designator or value went back to where the new drawing puts it, when its box changed)}. Replaces the pins and the body; pin ends land on the 2.54 mm grid; wires on a pin that moved follow it with a jog; a pin whose number goes follows its name (or renames); what the schematic had on a pin no longer there stays where it was",
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
    // A chip has two terminals and the SOT-23 three: another count was a different package.
    match (p.package, p.pins) {
        (Package::Chip, Some(n)) if n != 2 => return Err(bad(cmd, format!("a chip has 2 terminals, not `pins` {n}"))),
        (Package::Sot23, Some(n)) if n != 3 => {
            return Err(bad(cmd, format!("sot23 draws the 3-lead SOT-23, not `pins` {n}: give a SOT-23-5 or -6 its pads with ecad.pad.add")));
        }
        _ => {}
    }
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

/// Whether `r` is a name no library item among `names` has: the generators make that one.
fn is_new<'a>(r: &ComponentRef, mut names: impl Iterator<Item = &'a str>) -> Option<String> {
    match r {
        ComponentRef::Name(name) if names.all(|n| n != name) => Some(name.clone()),
        _ => None,
    }
}

fn footprint(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.footprint.generate";
    let p: FootprintParams = parse(CMD, p)?;
    let component = p.component.as_ref().map(|r| find(&s.doc, r, CMD)).transpose()?;
    if p.variant.is_some() && component.is_none() {
        return Err(bad(CMD, "`variant` is a variant of `component`: give the component too"));
    }
    let ecad = &s.doc.ecad;
    let made = is_new(&p.footprint, ecad.footprints.iter().map(|x| x.name.as_str()));
    let mut fp = match made.clone() {
        Some(name) => {
            free_item_name("footprint", ecad.footprints.iter().map(|x| (x.id, x.name.as_str())), &name, None, CMD)?;
            new_footprint(ecad.new_id().map_err(|e| bad(CMD, e))?, name, String::new())
        }
        None => load_footprint(s, &p.footprint, CMD)?,
    };
    let Drawn { pads, graphics } = drawn(CMD, &p)?;
    let count = pads.len();
    fp.pads = pads;
    fp.graphics.retain(|g| !matches!(g.layer, Some(BoardLayer::Fab | BoardLayer::Silkscreen | BoardLayer::Courtyard)));
    fp.graphics.extend(graphics);
    let courtyard =
        super::ecad_graphics::make_courtyard(&mut fp, p.margin.unwrap_or(0.25), solidcraft_doc::ecad::Side::Top).map_err(|e| bad(CMD, e))?;
    let (id, footprint) = (fp.id, item_id(fp.id, &fp.name));
    let mut pin_map_cleared = store_footprint(s, fp, &[], true, CMD)?;
    let (mut component_out, mut variant) = (None, None);
    if let Some(c) = component {
        let mut next = s.doc.ecad.clone();
        let (c, v, cleared) = set_variant(&mut next, c, p.variant.as_deref(), &footprint.name, |v| v.footprint = Some(id), CMD)?;
        commit(s, next, CMD)?;
        (component_out, variant, pin_map_cleared) = (Some(c), Some(v), pin_map_cleared || cleared);
    }
    reply(FootprintResult {
        footprint,
        created: made.is_some(),
        component: component_out,
        variant,
        pads: count,
        courtyard,
        pin_map_cleared,
        param_warnings: Vec::new(),
    })
}

/// The pins (with their roles in a shape) and body `p` draws.
type Drawing = (Vec<Pin>, Vec<Graphic>, Vec<Option<String>>);

/// The standard shape `p` asks for, from its style and variant fields; `None` for a box.
fn shape_kind(cmd: &str, p: &SymbolParams) -> Result<Option<Kind>> {
    use SymbolStyle as S;
    let given = p.pins.as_ref().map_or(0, Vec::len);
    if p.count.is_some() && !matches!(p.style, S::Crystal | S::Switch | S::Led) {
        return Err(bad(cmd, "`count` is for a crystal, a switch or an led"));
    }
    if p.bidirectional.is_some() && p.style != S::Tvs {
        return Err(bad(cmd, "`bidirectional` is for a tvs"));
    }
    if p.common.is_some() && p.style != S::Led {
        return Err(bad(cmd, "`common` is for an led"));
    }
    // A crystal's or a switch's pins: 2 or 4, from `pins` when `count` is left out.
    let four = |what: &str| -> Result<bool> {
        match p.count.map_or(if given == 4 { 4 } else { 2 }, |n| n as usize) {
            2 => Ok(false),
            4 => Ok(true),
            _ => Err(bad(cmd, format!("a {what} symbol has 2 or 4 pins (`count`)"))),
        }
    };
    Ok(Some(match p.style {
        S::Box => return Ok(None),
        S::Resistor => Kind::Resistor,
        S::Capacitor => Kind::Capacitor,
        S::PolarizedCapacitor => Kind::PolarizedCapacitor,
        S::Inductor => Kind::Inductor,
        S::Ferrite => Kind::Ferrite,
        S::Fuse => Kind::Fuse,
        S::Diode => Kind::Diode,
        S::Zener => Kind::Zener,
        S::Schottky => Kind::Schottky,
        S::Tvs => Kind::Tvs { bidirectional: p.bidirectional.unwrap_or(false) },
        S::Led => {
            let count = p.count.map_or(given.saturating_sub(1).max(1), |n| n as usize);
            if !(1..=MAX_LEDS).contains(&count) {
                return Err(bad(cmd, format!("an led symbol has 1 to {MAX_LEDS} LEDs (`count`)")));
            }
            Kind::Led { count, common_cathode: p.common == Some(CommonPin::Cathode) }
        }
        S::Crystal => Kind::Crystal { case: four("crystal")? },
        S::Resonator => Kind::Resonator,
        S::Switch => Kind::Switch { four: four("switch")? },
        S::Battery => Kind::Battery,
        S::Antenna => Kind::Antenna,
        S::TestPoint => Kind::TestPoint,
        S::Npn => Kind::Npn,
        S::Pnp => Kind::Pnp,
        S::Nmos => Kind::Nmos,
        S::Pmos => Kind::Pmos,
    }))
}

/// The most LEDs one generated symbol has.
const MAX_LEDS: usize = 4;

/// A shape's pins numbered as `given` says: none given keeps its usual numbers; else one for each
/// of its pins, a given pin whose name is a pin's role taking that pin, the others the rest in
/// order. A given name replaces the shape's; the shape keeps the names but doesn't draw them on
/// its pins (they would print over its body, and the shape says what each pin is), except that
/// several LEDs in one symbol show the names that are not their roles beside each LED (an RGB
/// LED's `K_RED`), so which die is which reads on the sheet.
fn shaped(cmd: &str, kind: Kind, given: Option<Vec<GeneratedPin>>) -> Result<Drawing> {
    let Shape { pins: style, mut graphics } = shapes::shape(kind);
    let leds = matches!(kind, Kind::Led { count, .. } if count > 1);
    let given = given.unwrap_or_default();
    if !given.is_empty() && given.len() != style.len() {
        let list: Vec<String> =
            style.iter().map(|q| if q.role == q.number { q.number.clone() } else { format!("{} {}", q.role, q.number) }).collect();
        return Err(bad(cmd, format!("this shape has {} pins ({}): give `pins` for each, or leave it out", style.len(), list.join(", "))));
    }
    if let Some(i) = given.iter().position(|g| g.side.is_some()) {
        return Err(bad(cmd, format!("pins[{i}]: `side` is for a box symbol's pins")));
    }
    // The shape pin each given pin numbers: by role first, then the rest in order.
    let mut slot: Vec<Option<usize>> = vec![None; given.len()];
    let mut taken = vec![false; style.len()];
    for (s, g) in slot.iter_mut().zip(&given) {
        let Some(name) = g.name.as_deref() else { continue };
        if let Some((j, t)) = style.iter().zip(taken.iter_mut()).enumerate().find_map(|(j, (q, t))| (q.role == name && !*t).then_some((j, t))) {
            (*s, *t) = (Some(j), true);
        }
    }
    for s in slot.iter_mut().filter(|s| s.is_none()) {
        if let Some((j, t)) = taken.iter_mut().enumerate().find(|(_, t)| !**t) {
            (*s, *t) = (Some(j), true);
        }
    }
    let mut given: Vec<Option<GeneratedPin>> = given.into_iter().map(Some).collect();
    let mut pins = Vec::with_capacity(style.len());
    let mut roles = Vec::with_capacity(style.len());
    for (j, q) in style.into_iter().enumerate() {
        let g = slot.iter().position(|s| *s == Some(j)).and_then(|i| given.get_mut(i)).and_then(Option::take);
        let (number, name, kind) = match g {
            Some(g) => (g.number.text(), g.name.unwrap_or(q.name), g.electrical.map(electrical).unwrap_or_default()),
            None => (q.number, q.name, PinType::Passive),
        };
        // Beside its LED, over the lead and clear of the light arrows: right of a cathode on the
        // right, left of an anode on the left.
        if leds && !name.is_empty() && name != q.role && q.role.len() == 2 {
            let wide = 0.6 * geo::NAME_HEIGHT * solidcraft_doc::ecad::overbar::plain(&name).chars().count() as f64;
            let x = if q.direction == solidcraft_doc::ecad::PinDirection::Left { 0.8 * geo::GRID } else { -0.8 * geo::GRID - wide };
            graphics.push(geo::name_text(&name, [x, q.at[1] + 1.6]));
        }
        roles.push(Some(q.role.to_string()));
        pins.push(Pin { number, name_hidden: !name.is_empty(), name, electrical: kind, at: q.at, direction: q.direction, length: geo::GRID });
    }
    Ok((pins, graphics, roles))
}

/// The pins and body `p` draws (with `pins` its pin list).
fn drawing(cmd: &str, p: &SymbolParams, pins: Option<Vec<GeneratedPin>>) -> Result<Drawing> {
    if let Some(kind) = shape_kind(cmd, p)? {
        return shaped(cmd, kind, pins);
    }
    let list = pins.unwrap_or_default();
    if list.is_empty() || list.len() > MAX_PACKAGE_PINS as usize {
        return Err(bad(cmd, format!("a box symbol needs 1 to {MAX_PACKAGE_PINS} `pins`")));
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
    let (pins, graphics) = geo::box_symbol(&pins);
    let roles = vec![None; pins.len()];
    Ok((pins, graphics, roles))
}

/// The renumbering `renames` asks for, from the pins `old` the symbol has to the `new` ones: each
/// from a number that goes, to one that comes, no two to one.
fn pin_renames(cmd: &str, old: &[Pin], new: &[Pin], renames: Option<BTreeMap<String, String>>) -> Result<Vec<(String, String)>> {
    let Some(renames) = renames else { return Ok(Vec::new()) };
    if renames.len() > old.len() {
        return Err(bad(cmd, format!("`renames` names more pins than the symbol has ({})", old.len())));
    }
    let (was, now): (BTreeSet<&str>, BTreeSet<&str>) =
        (old.iter().map(|q| q.number.as_str()).collect(), new.iter().map(|q| q.number.as_str()).collect());
    let mut to_seen = BTreeSet::new();
    for (from, to) in &renames {
        if !was.contains(from.as_str()) {
            return Err(bad(cmd, format!("`renames`: the symbol has no pin `{from}`")));
        }
        if now.contains(from.as_str()) {
            return Err(bad(cmd, format!("`renames`: the new symbol still has a pin `{from}`: only a number that goes is renamed")));
        }
        if !now.contains(to.as_str()) {
            return Err(bad(cmd, format!("`renames`: the new symbol has no pin `{to}`")));
        }
        if was.contains(to.as_str()) {
            return Err(bad(cmd, format!("`renames`: the symbol has a pin `{to}` now: numbers are not swapped")));
        }
        if !to_seen.insert(to.as_str()) {
            return Err(bad(cmd, format!("`renames`: two pins are renamed `{to}`")));
        }
    }
    Ok(renames.into_iter().collect())
}

/// What `ecad.symbol.generate` with `params` draws (its `symbol`, `component` and `renames` left
/// out; a box given no `pins` has four), as a symbol with no id or name, to show it small; `None`
/// when the parameters draw nothing.
pub fn symbol_style_preview(params: &Value) -> Option<SymbolItem> {
    let mut params = params.as_object()?.clone();
    params.insert("symbol".into(), "preview".into());
    params.remove("component");
    params.remove("renames");
    let mut p: SymbolParams = serde_json::from_value(Value::Object(params)).ok()?;
    let pins = p.pins.take().or_else(|| {
        (p.style == SymbolStyle::Box)
            .then(|| (1..=4).map(|n| GeneratedPin { number: Number::Whole(n), name: None, electrical: None, side: None }).collect())
    });
    let (pins, graphics, _) = drawing("ecad.symbol.generate", &p, pins).ok()?;
    Some(SymbolItem { pins, graphics, ..SymbolItem::default() })
}

fn symbol(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.symbol.generate";
    let mut p: SymbolParams = parse(CMD, p)?;
    let component = p.component.as_ref().map(|r| find(&s.doc, r, CMD)).transpose()?;
    let ecad = &s.doc.ecad;
    let made = is_new(&p.symbol, ecad.symbols.iter().map(|x| x.name.as_str()));
    let mut sym = match made.clone() {
        Some(name) => {
            free_item_name("symbol", ecad.symbols.iter().map(|x| (x.id, x.name.as_str())), &name, None, CMD)?;
            new_symbol(ecad.new_id().map_err(|e| bad(CMD, e))?, name, String::new())
        }
        None => load_symbol(s, &p.symbol, CMD)?,
    };
    let given = p.pins.take();
    let (pins, graphics, roles) = drawing(CMD, &p, given)?;
    for (i, pin) in pins.iter().enumerate() {
        pin.validate().map_err(|e| bad(CMD, format!("pins[{i}]: {e}")))?;
    }
    let renames = pin_renames(CMD, &sym.pins, &pins, p.renames.take())?;
    let pin_list =
        pins.iter().zip(roles).map(|(q, role)| GeneratedPinOut { number: q.number.clone(), name: q.name.clone(), role, at: q.at }).collect();
    let count = pins.len();
    sym.pins = pins;
    sym.graphics = graphics;
    let bbox = sym.bbox();
    let (id, symbol) = (sym.id, item_id(sym.id, &sym.name));
    let before = super::ecad::sheet_before(s);
    let (mut detached, mut pin_map_cleared) = store_symbol(s, sym, &renames, true, CMD)?;
    let mut component_out = None;
    if let Some(c) = component {
        let mut next = s.doc.ecad.clone();
        let (gone, cleared) = set_symbol(&s.doc.ecad, &mut next, c, Some(id));
        component_out = next.component(c).map(id_of);
        commit(s, next, CMD)?;
        detached.extend(super::ecad::detached_now(s, &gone));
        pin_map_cleared |= cleared;
    }
    let (netlist_changed, texts_reset) = super::ecad::sheet_after(s, before);
    reply(SymbolResult {
        symbol,
        created: made.is_some(),
        component: component_out,
        pins: count,
        pin_list,
        bbox,
        pin_map_cleared,
        detached,
        netlist_changed,
        texts_reset,
        param_warnings: Vec::new(),
    })
}

#[cfg(test)]
#[path = "ecad_generate_tests.rs"]
mod tests;
