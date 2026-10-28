//! Electronics: commands on the design's electronics (`doc.ecad`), the Electronics workspace's data,
//! and what the `ecad.*` command files share.

use serde::de::DeserializeOwned;
use serde_json::Value;
use solidcraft_doc::Document;
use solidcraft_doc::ecad::{ElectronicComponent, PadKind, PadShape, PinDirection, PinType};

use super::CommandSpec;
use crate::params::bad;
use crate::schema::ecad::{ComponentSummary, InspectParams, InspectResult};
use crate::schema::ecad_component::{ComponentId, ComponentRef, Direction, Electrical, Kind, Number, RowSpec, Shape, Side};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("ecad.inspect", "Inspect Electronics", crate::checked!("ecad.inspect", inspect))
    .icon("inspect")
    .noundo()
    .doc_only()
    .contract(&INSPECT_CONTRACT)
    .output(output_schema_for::<InspectResult>)
    .params(
        "No parameters. Returns {empty, components?: [{id, name, prefix, value, pins, pads, ready}]} (and param_warnings in the warn parameter-check mode): what the design's electronics hold, as data for agents. Read-only",
    )];

const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();

/// A summary of the design's electronics. Reads the document only.
fn inspect(s: &mut Session, _p: &Value) -> Result<Value> {
    let ecad = &s.doc.ecad;
    let components = ecad
        .components
        .iter()
        .map(|c| ComponentSummary {
            id: c.id,
            name: c.name.clone(),
            prefix: c.prefix.clone(),
            value: c.value.clone(),
            pins: c.symbol.pins.len(),
            pads: c.footprint.pads.len(),
            ready: ready(s, c),
        })
        .collect();
    reply(InspectResult { empty: !ecad.has_content(), components, param_warnings: Vec::new() })
}

/// Drop a pin map that names pins or pads no longer there; `true` when one was dropped.
pub(super) fn clear_stale_map(c: &mut ElectronicComponent) -> bool {
    if c.check_pin_map().is_err() {
        c.pin_map = None;
        return true;
    }
    false
}

/// Whether `c` links a 3D package whose SolidCraft component is gone.
pub(super) fn package_missing(s: &Session, c: &ElectronicComponent) -> bool {
    c.package.as_ref().is_some_and(|p| !s.doc.components.iter().any(|m| m.id == p.model))
}

/// Ready to place: the component's own checks pass, and its 3D package (if any) is there.
pub(super) fn ready(s: &Session, c: &ElectronicComponent) -> bool {
    c.ready() && !package_missing(s, c)
}

/// The call's parameters as the command's contract types; a mismatch is the caller's error, worded
/// as the strict parameter check words it (`` `components[1]` must be … ``).
pub(super) fn parse<T: DeserializeOwned>(cmd: &str, p: &Value) -> Result<T> {
    serde_path_to_error::deserialize(p).map_err(|e| bad(cmd, crate::schema::parse_issue(&e).message))
}

/// A reply type as the command's result.
pub(super) fn reply(v: impl serde::Serialize) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| EngineError::Other(e.to_string()))
}

/// The id of the electronic component `r` names, or an error that says what there is: the name of
/// a SolidCraft component is called that, and a name a few letters off is suggested.
pub(super) fn find(doc: &Document, r: &ComponentRef, cmd: &str) -> Result<u64> {
    let ecad = &doc.ecad;
    match r {
        ComponentRef::Id(id) => ecad.component(*id).map(|c| c.id).ok_or_else(|| bad(cmd, format!("no electronic component has the id {id}"))),
        ComponentRef::Name(name) => {
            if let Some(c) = ecad.named(name) {
                return Ok(c.id);
            }
            let mut msg = format!("no electronic component is named `{name}`");
            if doc.components.iter().any(|c| c.name == *name) {
                msg.push_str(&format!(" (`{name}` is a SolidCraft component, which holds bodies; ecad.inspect lists the electronic components)"));
            } else if let Some(near) = closest(name, ecad.components.iter().map(|c| c.name.as_str())) {
                msg.push_str(&format!(" (did you mean `{near}`?)"));
            }
            Err(bad(cmd, msg))
        }
    }
}

/// A copy of the electronic component `r` names, to change and [`store`].
pub(super) fn load(s: &Session, r: &ComponentRef, cmd: &str) -> Result<ElectronicComponent> {
    let id = find(&s.doc, r, cmd)?;
    s.doc.ecad.component(id).cloned().ok_or_else(|| bad(cmd, format!("no electronic component has the id {id}")))
}

/// Store `c` (new, or in place of the record with its id) after checking it as the design file
/// does; a refused record, or one equal to what is stored, changes nothing (no undo step).
pub(super) fn store(s: &mut Session, c: ElectronicComponent, cmd: &str) -> Result<()> {
    c.validate().map_err(|e| bad(cmd, e))?;
    if s.doc.ecad.component(c.id) == Some(&c) {
        return Ok(());
    }
    let ecad = &mut s.doc_mut().ecad;
    match ecad.components.iter_mut().find(|o| o.id == c.id) {
        Some(old) => *old = c,
        None => ecad.components.push(c),
    }
    Ok(())
}

pub(super) fn id_of(c: &ElectronicComponent) -> ComponentId {
    ComponentId { id: c.id, name: c.name.clone() }
}

pub(super) fn electrical(e: Electrical) -> PinType {
    match e {
        Electrical::Input => PinType::Input,
        Electrical::Output => PinType::Output,
        Electrical::Bidirectional => PinType::Bidirectional,
        Electrical::TriState => PinType::TriState,
        Electrical::Passive => PinType::Passive,
        Electrical::PowerInput => PinType::PowerInput,
        Electrical::PowerOutput => PinType::PowerOutput,
        Electrical::OpenCollector => PinType::OpenCollector,
        Electrical::OpenEmitter => PinType::OpenEmitter,
        Electrical::Unspecified => PinType::Unspecified,
        Electrical::NotConnected => PinType::NotConnected,
    }
}

pub(super) fn electrical_out(e: PinType) -> Electrical {
    match e {
        PinType::Input => Electrical::Input,
        PinType::Output => Electrical::Output,
        PinType::Bidirectional => Electrical::Bidirectional,
        PinType::TriState => Electrical::TriState,
        PinType::Passive => Electrical::Passive,
        PinType::PowerInput => Electrical::PowerInput,
        PinType::PowerOutput => Electrical::PowerOutput,
        PinType::OpenCollector => Electrical::OpenCollector,
        PinType::OpenEmitter => Electrical::OpenEmitter,
        PinType::Unspecified => Electrical::Unspecified,
        PinType::NotConnected => Electrical::NotConnected,
    }
}

pub(super) fn direction(d: Direction) -> PinDirection {
    match d {
        Direction::Right => PinDirection::Right,
        Direction::Left => PinDirection::Left,
        Direction::Up => PinDirection::Up,
        Direction::Down => PinDirection::Down,
    }
}

pub(super) fn direction_out(d: PinDirection) -> Direction {
    match d {
        PinDirection::Right => Direction::Right,
        PinDirection::Left => Direction::Left,
        PinDirection::Up => Direction::Up,
        PinDirection::Down => Direction::Down,
    }
}

pub(super) fn side(s: Side) -> solidcraft_doc::ecad::Side {
    match s {
        Side::Top => solidcraft_doc::ecad::Side::Top,
        Side::Bottom => solidcraft_doc::ecad::Side::Bottom,
    }
}

pub(super) fn side_out(s: solidcraft_doc::ecad::Side) -> Side {
    match s {
        solidcraft_doc::ecad::Side::Top => Side::Top,
        solidcraft_doc::ecad::Side::Bottom => Side::Bottom,
    }
}

pub(super) fn shape(s: Shape) -> PadShape {
    match s {
        Shape::Rect => PadShape::Rect,
        Shape::Round => PadShape::Round,
        Shape::Oval => PadShape::Oval,
        Shape::RoundedRect => PadShape::RoundedRect,
    }
}

pub(super) fn shape_out(s: PadShape) -> Shape {
    match s {
        PadShape::Rect => Shape::Rect,
        PadShape::Round => Shape::Round,
        PadShape::Oval => Shape::Oval,
        PadShape::RoundedRect => Shape::RoundedRect,
    }
}

pub(super) fn kind(k: Kind) -> PadKind {
    match k {
        Kind::Smd => PadKind::Smd,
        Kind::ThroughHole => PadKind::ThroughHole,
        Kind::Hole => PadKind::Hole,
    }
}

pub(super) fn kind_out(k: PadKind) -> Kind {
    match k {
        PadKind::Smd => Kind::Smd,
        PadKind::ThroughHole => Kind::ThroughHole,
        PadKind::Hole => Kind::Hole,
    }
}

/// The most items one `row` makes.
pub(super) const MAX_ROW: u32 = 4096;

/// How many items a list of specs with optional rows makes.
pub(super) fn row_total(rows: impl Iterator<Item = Option<RowSpec>>) -> usize {
    rows.fold(0usize, |n, row| n.saturating_add(row.map_or(1, |r| r.count as usize)))
}

/// One numbered item at `at`, or a row of `count` of them `pitch` apart, numbering on from
/// `number` (which must then be a whole number). `what` names the item in errors (`pads[2]`).
pub(super) fn row_items(number: &Number, at: [f64; 2], row: Option<RowSpec>, cmd: &str, what: &str) -> Result<Vec<(String, [f64; 2])>> {
    let Some(RowSpec { count, pitch }) = row else { return Ok(vec![(number.text(), at)]) };
    if count == 0 || count > MAX_ROW {
        return Err(bad(cmd, format!("{what}: `row.count` must be 1 to {MAX_ROW}")));
    }
    if !(pitch[0].is_finite() && pitch[1].is_finite()) {
        return Err(bad(cmd, format!("{what}: `row.pitch` must be two numbers in millimetres")));
    }
    let first = match number {
        Number::Whole(n) => *n,
        Number::Text(t) => t.parse::<u64>().map_err(|_| bad(cmd, format!("{what}: a row numbers on from a whole number, not `{t}`")))?,
    };
    (0..u64::from(count))
        .map(|k| {
            let n = first.checked_add(k).ok_or_else(|| bad(cmd, format!("{what}: the row's numbers run out")))?;
            let k = k as f64;
            Ok((n.to_string(), [at[0] + k * pitch[0], at[1] + k * pitch[1]]))
        })
        .collect()
}

/// The names of the fields that differ, from `(name, differs)` pairs.
pub(super) fn changed<'a>(fields: impl IntoIterator<Item = (&'a str, bool)>) -> Vec<String> {
    fields.into_iter().filter(|(_, differs)| *differs).map(|(name, _)| name.to_string()).collect()
}

/// The name in `names` nearest to `name`, ignoring case, when it is only a few letters off.
fn closest<'a>(name: &str, names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let name = name.to_lowercase();
    if name.chars().count() > solidcraft_doc::ecad::MAX_TEXT {
        return None;
    }
    let near = (name.chars().count() / 4).max(2);
    names.map(|n| (super::edit_distance(&name, &n.to_lowercase()), n)).filter(|(d, _)| *d <= near).min_by_key(|(d, _)| *d).map(|(_, n)| n)
}

#[cfg(test)]
#[path = "ecad_tests.rs"]
mod tests;
