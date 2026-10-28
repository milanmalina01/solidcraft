//! An electronic component's schematic symbol: add, change and delete its pins. Each call checks
//! everything before it changes anything, so a refused call leaves the design as it was, and a call
//! is one undo step however many pins it touches.

use serde_json::Value;
use solidcraft_doc::ecad::symbol::{DEFAULT_PIN_LENGTH, MAX_PINS};
use solidcraft_doc::ecad::{Pin, check_number};

use super::CommandSpec;
use super::ecad::{changed, direction, electrical, id_of, load, parse, reply, row_items, row_total, store};
use crate::params::bad;
use crate::schema::ecad_symbol::{AddParams, AddResult, DeleteParams, DeleteResult, EditParams, EditResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.pin.add", "Add Symbol Pins", crate::checked!("ecad.pin.add", add))
        .icon("new")
        .doc_only()
        .contract(&ADD_CONTRACT)
        .output(output_schema_for::<AddResult>)
        .params(
            "component: id|name; pins: [{number, name?, electrical? (passive), at: [x, y] (where a wire connects, mm), direction: right|left|up|down (toward the body), length? (2.54), row? {count, pitch: [dx, dy]} (numbers on from `number`)}]; replace? (pin_map_cleared: a map naming dropped pins is dropped) → {component, added, pins, pin_map_cleared}",
        ),
    CommandSpec::new("ecad.pin.edit", "Edit Symbol Pin", crate::checked!("ecad.pin.edit", edit))
        .icon("edit")
        .doc_only()
        .contract(&EDIT_CONTRACT)
        .output(output_schema_for::<EditResult>)
        .params("component: id|name; pin: number; number?, name?, electrical?, at?, direction?, length? → {component, pin, changed: [keys]}"),
    CommandSpec::new("ecad.pin.delete", "Delete Symbol Pins", crate::checked!("ecad.pin.delete", delete))
        .icon("delete")
        .doc_only()
        .contract(&DELETE_CONTRACT)
        .output(output_schema_for::<DeleteResult>)
        .params("component: id|name; pins: [numbers] (all or none) → {component, deleted: [numbers]}"),
];

const ADD_CONTRACT: TypedContract<AddParams> = TypedContract::new();
const EDIT_CONTRACT: TypedContract<EditParams> = TypedContract::new();
const DELETE_CONTRACT: TypedContract<DeleteParams> = TypedContract::new();

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.pin.add";
    let p: AddParams = parse(CMD, p)?;
    let mut c = load(s, &p.component, CMD)?;
    let replace = p.replace.unwrap_or(false);
    if p.pins.is_empty() && !replace {
        return Err(bad(CMD, "give at least one pin in `pins`"));
    }
    let mut pins = if replace { Vec::new() } else { c.symbol.pins.clone() };
    let added = row_total(p.pins.iter().map(|s| s.row));
    if pins.len().saturating_add(added) > MAX_PINS {
        return Err(bad(CMD, format!("a symbol has at most {MAX_PINS} pins")));
    }
    let mut numbers: std::collections::BTreeSet<String> = pins.iter().map(|q| q.number.clone()).collect();
    for (i, spec) in p.pins.into_iter().enumerate() {
        let template = Pin {
            number: String::new(),
            name: spec.name.unwrap_or_default(),
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
    c.symbol.pins = pins;
    // Replacing the list drops a pin map that named what was there, as the generators do.
    let pin_map_cleared = replace && super::ecad::clear_stale_map(&mut c);
    let (component, total) = (id_of(&c), c.symbol.pins.len());
    store(s, c, CMD)?;
    reply(AddResult { component, added, pins: total, pin_map_cleared, param_warnings: Vec::new() })
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.pin.edit";
    let p: EditParams = parse(CMD, p)?;
    let mut c = load(s, &p.component, CMD)?;
    let number = p.pin.text();
    let Some(index) = c.symbol.pins.iter().position(|q| q.number == number) else {
        return Err(bad(CMD, format!("`{}` has no pin numbered `{number}`", c.name)));
    };
    let old = c.symbol.pins[index].clone();
    let mut pin = old.clone();
    if let Some(n) = p.number {
        let n = n.text();
        check_number("pin", &n).map_err(|e| bad(CMD, e))?;
        if n != old.number && c.symbol.pin(&n).is_some() {
            return Err(bad(CMD, format!("`{}` already has a pin numbered `{n}`", c.name)));
        }
        pin.number = n;
    }
    if let Some(name) = p.name {
        pin.name = name;
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
        ("electrical", old.electrical != pin.electrical),
        ("at", old.at != pin.at),
        ("direction", old.direction != pin.direction),
        ("length", old.length != pin.length),
    ]);
    let (component, number) = (id_of(&c), pin.number.clone());
    if !changed.is_empty() {
        if old.number != pin.number {
            c.pin_renamed(&old.number, Some(&pin.number));
        }
        c.symbol.pins[index] = pin;
        store(s, c, CMD)?;
    }
    reply(EditResult { component, pin: number, changed, param_warnings: Vec::new() })
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.pin.delete";
    let p: DeleteParams = parse(CMD, p)?;
    let mut c = load(s, &p.component, CMD)?;
    if p.pins.is_empty() || p.pins.len() > MAX_PINS {
        return Err(bad(CMD, format!("give 1 to {MAX_PINS} pin numbers in `pins`")));
    }
    let mut deleted: Vec<String> = Vec::new();
    for (i, n) in p.pins.iter().enumerate() {
        let n = n.text();
        if c.symbol.pin(&n).is_none() {
            return Err(bad(CMD, format!("pins[{i}]: `{}` has no pin numbered `{n}`", c.name)));
        }
        if !deleted.contains(&n) {
            deleted.push(n);
        }
    }
    c.symbol.pins.retain(|q| !deleted.contains(&q.number));
    for n in &deleted {
        c.pin_renamed(n, None);
    }
    let component = id_of(&c);
    store(s, c, CMD)?;
    reply(DeleteResult { component, deleted, param_warnings: Vec::new() })
}
