//! A library symbol's pins: add, change and delete them. Each call checks everything before it
//! changes anything, so a refused call leaves the design as it was, and a call is one undo step
//! however many pins it touches. Every electronic component that shows the symbol follows: its
//! pin maps, and on the schematic what hangs on its parts' pins.

use serde_json::Value;
use solidcraft_doc::ecad::symbol::{DEFAULT_PIN_LENGTH, MAX_PINS};
use solidcraft_doc::ecad::{Pin, check_number};

use super::CommandSpec;
use super::ecad::{changed, direction, electrical, item_id, load_symbol, parse, reply, row_items, row_total, store_symbol};
use crate::params::bad;
use crate::schema::ecad_symbol::{AddParams, AddResult, DeleteParams, DeleteResult, EditParams, EditResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.pin.add", "Pin", crate::checked!("ecad.pin.add", add))
        .icon("point")
        .doc_only()
        .contract(&ADD_CONTRACT)
        .output(output_schema_for::<AddResult>)
        .params(
            "symbol: id|name (a library symbol); pins: [{number, name?, name_hidden? (kept, not drawn), electrical? (passive), at: [x, y] (where a wire connects, mm), direction: right|left|up|down (toward the body), length? (2.54), row? {count, pitch: [dx, dy]} (numbers on from `number`)}]; replace? (pin_map_cleared: a variant's map naming dropped pins is dropped, in every component showing it) → {symbol, added, pins, pin_map_cleared, detached?, netlist_changed?: [{net, lost, gained, more?}] (the nets whose pins the edit changed on the schematic, should a moved pin land on another wire or leave its own), texts_reset?: [designators] (with replace, parts whose moved designator or value went back to where the redrawn symbol puts it)}: what the schematic had on a dropped pin stays where the pin was",
        ),
    CommandSpec::new("ecad.pin.edit", "Edit Symbol Pin", crate::checked!("ecad.pin.edit", edit))
        .icon("edit")
        .doc_only()
        .contract(&EDIT_CONTRACT)
        .output(output_schema_for::<EditResult>)
        .params(
            "symbol: id|name; pin: number; number?, name?, name_hidden?, electrical?, at?, direction?, length? → {symbol, pin, changed: [keys], netlist_changed?: [{net, lost, gained, more?}] (the nets whose pins the edit changed on the schematic, should a moved pin land on another wire or leave its own)}; pin maps and wires on the schematic follow a renumbered or moved pin, in every component showing it",
        ),
    CommandSpec::new("ecad.pin.delete", "Delete Symbol Pins", crate::checked!("ecad.pin.delete", delete))
        .icon("delete")
        .doc_only()
        .contract(&DELETE_CONTRACT)
        .output(output_schema_for::<DeleteResult>)
        .params(
            "symbol: id|name; pins: [numbers] (all or none) → {symbol, deleted: [numbers], detached?, netlist_changed?: [{net, lost, gained, more?}] (the nets whose pins the edit changed on the schematic, should a moved pin land on another wire or leave its own)}: pin maps forget them; what the schematic had on them stays where they were",
        ),
];

const ADD_CONTRACT: TypedContract<AddParams> = TypedContract::new();
const EDIT_CONTRACT: TypedContract<EditParams> = TypedContract::new();
const DELETE_CONTRACT: TypedContract<DeleteParams> = TypedContract::new();

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.pin.add";
    let p: AddParams = parse(CMD, p)?;
    let mut c = load_symbol(s, &p.symbol, CMD)?;
    let replace = p.replace.unwrap_or(false);
    if p.pins.is_empty() && !replace {
        return Err(bad(CMD, "give at least one pin in `pins`"));
    }
    let mut pins = if replace { Vec::new() } else { c.pins.clone() };
    let added = row_total(p.pins.iter().map(|s| s.row));
    if pins.len().saturating_add(added) > MAX_PINS {
        return Err(bad(CMD, format!("a symbol has at most {MAX_PINS} pins")));
    }
    let mut numbers: std::collections::BTreeSet<String> = pins.iter().map(|q| q.number.clone()).collect();
    for (i, spec) in p.pins.into_iter().enumerate() {
        let template = Pin {
            number: String::new(),
            name: spec.name.unwrap_or_default(),
            name_hidden: spec.name_hidden.unwrap_or(false),
            electrical: spec.electrical.map(electrical).unwrap_or_default(),
            at: spec.at,
            direction: direction(spec.direction),
            length: spec.length.unwrap_or(DEFAULT_PIN_LENGTH),
        };
        for (number, at) in row_items(&spec.number, spec.at, spec.row, CMD, &format!("pins[{i}]"))? {
            let pin = Pin { number, at, ..template.clone() };
            pin.validate().map_err(|e| bad(CMD, format!("pins[{i}]: {e}")))?;
            if !numbers.insert(pin.number.clone()) {
                return Err(bad(CMD, format!("pins[{i}]: the symbol already has a pin numbered `{}`", pin.number)));
            }
            pins.push(pin);
        }
    }
    c.pins = pins;
    // Replacing the list drops a pin map that named what was there, as the generators do.
    let (symbol, total) = (item_id(c.id, &c.name), c.pins.len());
    let before = super::ecad::sheet_before(s);
    let (detached, pin_map_cleared) = store_symbol(s, c, &[], replace, CMD)?;
    let (netlist_changed, texts_reset) = super::ecad::sheet_after(s, before);
    reply(AddResult { symbol, added, pins: total, pin_map_cleared, detached, netlist_changed, texts_reset, param_warnings: Vec::new() })
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.pin.edit";
    let p: EditParams = parse(CMD, p)?;
    let mut c = load_symbol(s, &p.symbol, CMD)?;
    let number = p.pin.text();
    let Some(index) = c.pins.iter().position(|q| q.number == number) else {
        return Err(bad(CMD, format!("`{}` has no pin numbered `{number}`", c.name)));
    };
    let Some(old) = c.pins.get(index).cloned() else { return Err(bad(CMD, format!("`{}` has no pin numbered `{number}`", c.name))) };
    let mut pin = old.clone();
    if let Some(n) = p.number {
        let n = n.text();
        check_number("pin", &n).map_err(|e| bad(CMD, e))?;
        if n != old.number && c.pin(&n).is_some() {
            return Err(bad(CMD, format!("`{}` already has a pin numbered `{n}`", c.name)));
        }
        pin.number = n;
    }
    if let Some(name) = p.name {
        pin.name = name;
    }
    if let Some(hidden) = p.name_hidden {
        pin.name_hidden = hidden;
    }
    if let Some(e) = p.electrical {
        pin.electrical = electrical(e);
    }
    if let Some(at) = p.at {
        pin.at = at;
    }
    if let Some(d) = p.direction {
        pin.direction = direction(d);
    }
    if let Some(length) = p.length {
        pin.length = length;
    }
    pin.validate().map_err(|e| bad(CMD, e))?;
    let changed = changed([
        ("number", old.number != pin.number),
        ("name", old.name != pin.name),
        ("name_hidden", old.name_hidden != pin.name_hidden),
        ("electrical", old.electrical != pin.electrical),
        ("at", old.at != pin.at),
        ("direction", old.direction != pin.direction),
        ("length", old.length != pin.length),
    ]);
    let (symbol, number) = (item_id(c.id, &c.name), pin.number.clone());
    let mut netlist_changed = Vec::new();
    if !changed.is_empty() {
        let before = super::ecad::sheet_before(s);
        let renames = if old.number != pin.number { vec![(old.number.clone(), pin.number.clone())] } else { Vec::new() };
        if let Some(slot) = c.pins.get_mut(index) {
            *slot = pin;
        }
        store_symbol(s, c, &renames, false, CMD)?;
        netlist_changed = super::ecad::sheet_after(s, before).0;
    }
    reply(EditResult { symbol, pin: number, changed, netlist_changed, param_warnings: Vec::new() })
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.pin.delete";
    let p: DeleteParams = parse(CMD, p)?;
    let mut c = load_symbol(s, &p.symbol, CMD)?;
    if p.pins.is_empty() || p.pins.len() > MAX_PINS {
        return Err(bad(CMD, format!("give 1 to {MAX_PINS} pin numbers in `pins`")));
    }
    let mut deleted: Vec<String> = Vec::new();
    for (i, n) in p.pins.iter().enumerate() {
        let n = n.text();
        if c.pin(&n).is_none() {
            return Err(bad(CMD, format!("pins[{i}]: `{}` has no pin numbered `{n}`", c.name)));
        }
        if !deleted.contains(&n) {
            deleted.push(n);
        }
    }
    c.pins.retain(|q| !deleted.contains(&q.number));
    let symbol = item_id(c.id, &c.name);
    let before = super::ecad::sheet_before(s);
    let (detached, _) = store_symbol(s, c, &[], false, CMD)?;
    let netlist_changed = super::ecad::sheet_after(s, before).0;
    reply(DeleteResult { symbol, deleted, detached, netlist_changed, param_warnings: Vec::new() })
}
