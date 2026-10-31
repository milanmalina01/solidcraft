//! A library footprint's pads: add, change and delete them. Each call checks everything before it
//! changes anything, so a refused call leaves the design as it was, and a call is one undo step
//! however many pads it touches. Every variant built on the footprint follows: its pin map keeps
//! naming the pads it meant.

use serde_json::Value;
use solidcraft_doc::ecad::footprint::MAX_PADS;
use solidcraft_doc::ecad::{Pad, PadKind, PadShape, check_number};

use super::CommandSpec;
use super::ecad::{changed, item_id, kind, kind_out, load_footprint, parse, reply, row_items, row_total, shape, side, store_footprint};
use crate::params::bad;
use crate::schema::ecad_footprint::{AddParams, AddResult, DeleteParams, DeleteResult, EditParams, EditResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.pad.add", "Pad", crate::checked!("ecad.pad.add", add))
        .icon("rect_rounded")
        .doc_only()
        .contract(&ADD_CONTRACT)
        .output(output_schema_for::<AddResult>)
        .params(
            "footprint: id|name (a library footprint); pads: [{number, at: [x, y] (centre, mm), size: [w, h] (mm), rotation? (degrees ccw, 0), side? (top|bottom, surface-mount), shape? (rect|round|oval|rounded_rect), corner? (0.25 of the shorter side for rounded_rect), kind? (smd|through_hole|hole; through_hole with a drill), drill? (d or [w, h] mm), row? {count, pitch: [dx, dy]} (numbers on from `number`)}]; replace? (pin_map_cleared: a variant's map naming dropped pads is dropped) → {footprint, added, pads, pin_map_cleared}",
        ),
    CommandSpec::new("ecad.pad.add.through_hole", "Through-hole Pad", crate::checked!("ecad.pad.add.through_hole", add_through_hole))
        .icon("c_concentric")
        .doc_only()
        .contract(&ADD_CONTRACT)
        .output(output_schema_for::<AddResult>)
        .params(
            "as ecad.pad.add, with each pad's kind? through_hole when left out and its drill? 0.6 of its shorter side when left out → {footprint, added, pads, pin_map_cleared}",
        ),
    CommandSpec::new("ecad.pad.add.hole", "Hole", crate::checked!("ecad.pad.add.hole", add_hole))
        .icon("hole")
        .doc_only()
        .contract(&ADD_CONTRACT)
        .output(output_schema_for::<AddResult>)
        .params(
            "as ecad.pad.add, with each pad's kind? hole (unplated: no copper, connects to nothing) when left out and its drill? its shorter side when left out → {footprint, added, pads, pin_map_cleared}",
        ),
    CommandSpec::new("ecad.pad.edit", "Edit Footprint Pad", crate::checked!("ecad.pad.edit", edit))
        .icon("edit")
        .doc_only()
        .contract(&EDIT_CONTRACT)
        .output(output_schema_for::<EditResult>)
        .params(
            "footprint: id|name; pad: number; number?, at?, size?, rotation?, side?, shape?, corner?, kind?, drill? → {footprint, pad, changed: [keys]}; pin maps follow a renumbered pad",
        ),
    CommandSpec::new("ecad.pad.delete", "Delete Footprint Pads", crate::checked!("ecad.pad.delete", delete))
        .icon("delete")
        .doc_only()
        .contract(&DELETE_CONTRACT)
        .output(output_schema_for::<DeleteResult>)
        .params("footprint: id|name; pads: [numbers] (all or none) → {footprint, deleted: [numbers]}: pin maps forget them"),
];

const ADD_CONTRACT: TypedContract<AddParams> = TypedContract::new();
const EDIT_CONTRACT: TypedContract<EditParams> = TypedContract::new();
const DELETE_CONTRACT: TypedContract<DeleteParams> = TypedContract::new();

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    add_kind(s, p, "ecad.pad.add", None)
}

fn add_through_hole(s: &mut Session, p: &Value) -> Result<Value> {
    add_kind(s, p, "ecad.pad.add.through_hole", Some(PadKind::ThroughHole))
}

fn add_hole(s: &mut Session, p: &Value) -> Result<Value> {
    add_kind(s, p, "ecad.pad.add.hole", Some(PadKind::Hole))
}

/// `ecad.pad.add`, with each pad that gives no kind of its own made `default` (with a drill that
/// fits it when it gives none either): the toolbar's Through-hole Pad and Hole.
fn add_kind(s: &mut Session, p: &Value, cmd: &'static str, default: Option<PadKind>) -> Result<Value> {
    let mut p: AddParams = parse(cmd, p)?;
    if let Some(kind) = default {
        for spec in p.pads.iter_mut().filter(|spec| spec.kind.is_none()) {
            spec.kind = Some(kind_out(kind));
            if spec.drill.is_none() {
                let shorter = spec.size[0].min(spec.size[1]);
                let d = if kind == PadKind::Hole { shorter } else { (shorter * 0.6 * 1e6).round() / 1e6 };
                spec.drill = Some(crate::schema::ecad_component::DrillSpec::Round(d));
            }
        }
    }
    let mut c = load_footprint(s, &p.footprint, cmd)?;
    let replace = p.replace.unwrap_or(false);
    if p.pads.is_empty() && !replace {
        return Err(bad(cmd, "give at least one pad in `pads`"));
    }
    let mut pads = if replace { Vec::new() } else { c.pads.clone() };
    let added = row_total(p.pads.iter().map(|s| s.row));
    if pads.len().saturating_add(added) > MAX_PADS {
        return Err(bad(cmd, format!("a footprint has at most {MAX_PADS} pads")));
    }
    let mut numbers: std::collections::BTreeSet<String> = pads.iter().map(|q| q.number.clone()).collect();
    for (i, spec) in p.pads.into_iter().enumerate() {
        let drill = spec.drill.map(|d| d.sides());
        let template = Pad {
            number: String::new(),
            at: spec.at,
            size: spec.size,
            rotation: spec.rotation.unwrap_or(0.0),
            side: spec.side.map(side).unwrap_or_default(),
            shape: spec.shape.map(shape).unwrap_or_default(),
            corner: spec.corner.unwrap_or(if spec.shape.map(shape) == Some(PadShape::RoundedRect) { 0.25 } else { 0.0 }),
            kind: spec.kind.map(kind).unwrap_or(if drill.is_some() { PadKind::ThroughHole } else { PadKind::Smd }),
            drill,
        };
        for (number, at) in row_items(&spec.number, spec.at, spec.row, cmd, &format!("pads[{i}]"))? {
            let pad = Pad { number, at, ..template.clone() };
            pad.validate().map_err(|e| bad(cmd, format!("pads[{i}]: {e}")))?;
            if !numbers.insert(pad.number.clone()) {
                return Err(bad(cmd, format!("pads[{i}]: the footprint already has a pad numbered `{}`", pad.number)));
            }
            pads.push(pad);
        }
    }
    c.pads = pads;
    // Replacing the list drops a pin map that named what was there, as the generators do.
    let (footprint, total) = (item_id(c.id, &c.name), c.pads.len());
    let pin_map_cleared = store_footprint(s, c, &[], replace, cmd)?;
    reply(AddResult { footprint, added, pads: total, pin_map_cleared, param_warnings: Vec::new() })
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.pad.edit";
    let p: EditParams = parse(CMD, p)?;
    let mut c = load_footprint(s, &p.footprint, CMD)?;
    let number = p.pad.text();
    let Some(index) = c.pads.iter().position(|q| q.number == number) else {
        return Err(bad(CMD, format!("`{}` has no pad numbered `{number}`", c.name)));
    };
    let Some(old) = c.pads.get(index).cloned() else { return Err(bad(CMD, format!("`{}` has no pad numbered `{number}`", c.name))) };
    let mut pad = old.clone();
    if let Some(n) = p.number {
        let n = n.text();
        check_number("pad", &n).map_err(|e| bad(CMD, e))?;
        if n != old.number && c.pad(&n).is_some() {
            return Err(bad(CMD, format!("`{}` already has a pad numbered `{n}`", c.name)));
        }
        pad.number = n;
    }
    if let Some(at) = p.at {
        pad.at = at;
    }
    if let Some(size) = p.size {
        pad.size = size;
    }
    if let Some(rotation) = p.rotation {
        pad.rotation = rotation;
    }
    if let Some(sd) = p.side {
        pad.side = side(sd);
    }
    if let Some(sh) = p.shape {
        pad.shape = shape(sh);
        if pad.shape != PadShape::RoundedRect && p.corner.is_none() {
            pad.corner = 0.0;
        } else if pad.shape == PadShape::RoundedRect && pad.corner == 0.0 && p.corner.is_none() {
            pad.corner = 0.25;
        }
    }
    if let Some(corner) = p.corner {
        pad.corner = corner;
    }
    if let Some(d) = p.drill {
        pad.drill = Some(d.sides());
        if pad.kind == PadKind::Smd && p.kind.is_none() {
            pad.kind = PadKind::ThroughHole;
        }
    }
    if let Some(k) = p.kind {
        pad.kind = kind(k);
        if pad.kind == PadKind::Smd {
            pad.drill = None;
        }
    }
    pad.validate().map_err(|e| bad(CMD, e))?;
    let changed = changed([
        ("number", old.number != pad.number),
        ("at", old.at != pad.at),
        ("size", old.size != pad.size),
        ("rotation", old.rotation != pad.rotation),
        ("side", old.side != pad.side),
        ("shape", old.shape != pad.shape),
        ("corner", old.corner != pad.corner),
        ("kind", old.kind != pad.kind),
        ("drill", old.drill != pad.drill),
    ]);
    let (footprint, number) = (item_id(c.id, &c.name), pad.number.clone());
    if !changed.is_empty() {
        let renames = if old.number != pad.number { vec![(old.number.clone(), pad.number.clone())] } else { Vec::new() };
        if let Some(slot) = c.pads.get_mut(index) {
            *slot = pad;
        }
        store_footprint(s, c, &renames, false, CMD)?;
    }
    reply(EditResult { footprint, pad: number, changed, param_warnings: Vec::new() })
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.pad.delete";
    let p: DeleteParams = parse(CMD, p)?;
    let mut c = load_footprint(s, &p.footprint, CMD)?;
    if p.pads.is_empty() || p.pads.len() > MAX_PADS {
        return Err(bad(CMD, format!("give 1 to {MAX_PADS} pad numbers in `pads`")));
    }
    let mut deleted: Vec<String> = Vec::new();
    for (i, n) in p.pads.iter().enumerate() {
        let n = n.text();
        if c.pad(&n).is_none() {
            return Err(bad(CMD, format!("pads[{i}]: `{}` has no pad numbered `{n}`", c.name)));
        }
        if !deleted.contains(&n) {
            deleted.push(n);
        }
    }
    c.pads.retain(|q| !deleted.contains(&q.number));
    let footprint = item_id(c.id, &c.name);
    store_footprint(s, c, &[], false, CMD)?;
    reply(DeleteResult { footprint, deleted, param_warnings: Vec::new() })
}

#[cfg(test)]
#[path = "ecad_footprint_tests.rs"]
mod tests;
