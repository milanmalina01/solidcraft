//! Electronic components: make, change, copy, delete and read the parts the design's electronics
//! are built from, each a library symbol and variants (footprints and 3D packages) it is built
//! with. Names and texts follow `solidcraft_doc::ecad`'s rules, checked before anything changes, so
//! a refused call leaves the design as it was.

use serde_json::Value;
use solidcraft_doc::ecad::{ElectronicComponent, MAX_COMPONENTS, Resolved, Variant, check_name, check_prefix};

use super::CommandSpec;
use super::ecad::{
    changed, commit, detached_now, direction_out, electrical_out, find, find_footprint, find_package, find_symbol, id_of, item_id, load, parse,
    reply, store,
};
use crate::params::bad;
use crate::schema::ecad_component::{
    ComponentId, ComponentRecord, Connection, CopyParams, CreateParams, CreateResult, DeleteParams, DeleteResult, EditParams, EditResult,
    InspectParams, InspectResult, MapPinsParams, MapPinsResult, PinOut, ProblemOut, Severity, VariantRecord,
};
use crate::schema::ecad_schematic::{ItemKind, ItemOut};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.component.create", "New Component", crate::checked!("ecad.component.create", create))
        .icon("new")
        .doc_only()
        .contract(&CREATE_CONTRACT)
        .output(output_schema_for::<CreateResult>)
        .params(
            "name (unique, such as R0603); prefix? (designator letters, U when left out); value?, mpn?, manufacturer?, description?; symbol?: id|name (a library symbol); footprint?: id|name (a library footprint: the first variant, named after it); package?: id|name (a library 3D package for that variant) → {component: {id, name}}",
        ),
    CommandSpec::new("ecad.component.edit", "Edit Electronic Component", crate::checked!("ecad.component.edit", edit))
        .icon("edit")
        .doc_only()
        .contract(&EDIT_CONTRACT)
        .output(output_schema_for::<EditResult>)
        .params(
            "component: id|name; name?, prefix?, value?, mpn?, manufacturer?, description? (an empty text clears it); symbol?: id|name|null (another library symbol: on the schematic, what is on pins it numbers alike, or names alike where the number went, stays on them, wires following a pin that moved with a jog; the rest lets go where it was; a pin map naming pins it lacks is dropped; parts' moved designators and values go back to where a symbol with another box puts them) → {component, changed: [keys], detached?, netlist_changed?: [{net, lost, gained, more?}] (the nets whose pins the edit changed on the schematic, should a moved pin land on another wire or leave its own), texts_reset?: [designators]}",
        ),
    CommandSpec::new("ecad.component.copy", "Copy Component", crate::checked!("ecad.component.copy", copy))
        .icon("copy")
        .doc_only()
        .contract(&COPY_CONTRACT)
        .output(output_schema_for::<CreateResult>)
        .params("component: id|name; name (the copy's, unique) → {component: {id, name}}: the copy uses the same symbol, footprints and 3D packages"),
    CommandSpec::new("ecad.component.delete", "Delete Components", crate::checked!("ecad.component.delete", delete))
        .icon("delete")
        .doc_only()
        .contract(&DELETE_CONTRACT)
        .output(output_schema_for::<DeleteResult>)
        .params(
            "components: [id|name] (all or none); with_parts? (a component placed on the schematic is refused without it) → {deleted: [{id, name}], parts?, detached?}: wires and labels on deleted parts stay where the pins were; the library items they used stay",
        ),
    CommandSpec::new("ecad.component.inspect", "Inspect", crate::checked!("ecad.component.inspect", inspect))
        .icon("inspect")
        .noundo()
        .contract(&INSPECT_CONTRACT)
        .output(output_schema_for::<InspectResult>)
        .params(
            "component: id|name → {component: {id, name, prefix, value, mpn, manufacturer, description, symbol: {id, name}|null, pins, variants: [{name, footprint: {id, name}|null, pads, package, pin_map, connections, unconnected_pins, unconnected_pads, problems, ready}], problems, ready}}. Read-only",
        ),
    CommandSpec::new("ecad.component.map_pins", "Connect", crate::checked!("ecad.component.map_pins", map_pins))
        .icon("chain")
        .doc_only()
        .contract(&MAP_PINS_CONTRACT)
        .output(output_schema_for::<MapPinsResult>)
        .params(
            "component: id|name; variant? (by name; the first when left out); map: {pin: [pads]} (replaces the variant's map; a pin left out connects to no pad) | by_number: true (each pin to the pad with its number) → {component, variant, connections, unconnected_pins, unconnected_pads}",
        ),
];

const CREATE_CONTRACT: TypedContract<CreateParams> = TypedContract::new();
const EDIT_CONTRACT: TypedContract<EditParams> = TypedContract::new();
const COPY_CONTRACT: TypedContract<CopyParams> = TypedContract::new();
const DELETE_CONTRACT: TypedContract<DeleteParams> = TypedContract::new();
const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();
const MAP_PINS_CONTRACT: TypedContract<MapPinsParams> = TypedContract::new();

/// `name` is free for a new electronic component, or for `except` renamed.
fn free_name(s: &Session, name: &str, except: Option<u64>, cmd: &str) -> Result<()> {
    check_name(name).map_err(|e| bad(cmd, e))?;
    match s.doc.ecad.named(name) {
        Some(c) if Some(c.id) != except => Err(bad(cmd, format!("an electronic component is already named `{name}`"))),
        _ => Ok(()),
    }
}

/// Room for one more electronic component.
fn room(s: &Session, cmd: &str) -> Result<()> {
    if s.doc.ecad.components.len() >= MAX_COMPONENTS {
        return Err(bad(cmd, format!("a design holds at most {MAX_COMPONENTS} electronic components")));
    }
    Ok(())
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.component.create";
    let p: CreateParams = parse(CMD, p)?;
    room(s, CMD)?;
    free_name(s, &p.name, None, CMD)?;
    let prefix = p.prefix.unwrap_or_else(|| "U".into());
    check_prefix(&prefix).map_err(|e| bad(CMD, e))?;
    let symbol = p.symbol.as_ref().map(|r| find_symbol(&s.doc, r, CMD)).transpose()?;
    let footprint = p.footprint.as_ref().map(|r| find_footprint(&s.doc, r, CMD)).transpose()?;
    let package = p.package.as_ref().map(|r| find_package(&s.doc, r, CMD)).transpose()?;
    let ecad = &s.doc.ecad;
    // The first variant is named after its footprint, else its 3D package.
    let named = footprint
        .and_then(|id| ecad.footprint(id))
        .map(|f| f.name.clone())
        .or_else(|| package.and_then(|id| ecad.package(id)).map(|x| x.name.clone()));
    let variants = named.map(|name| Variant { name, footprint, package, pin_map: None }).into_iter().collect();
    let mut next = ecad.clone();
    let c = ElectronicComponent {
        id: next.take_id().map_err(|e| bad(CMD, e))?,
        name: p.name,
        prefix,
        value: p.value.unwrap_or_default(),
        mpn: p.mpn.unwrap_or_default(),
        manufacturer: p.manufacturer.unwrap_or_default(),
        description: p.description.unwrap_or_default(),
        symbol,
        variants,
    };
    c.validate().map_err(|e| bad(CMD, e))?;
    let component = id_of(&c);
    next.components.push(c);
    commit(s, next, CMD)?;
    reply(CreateResult { component, param_warnings: Vec::new() })
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.component.edit";
    let p: EditParams = parse(CMD, p)?;
    let old = load(s, &p.component, CMD)?;
    let id = old.id;
    let mut c = old.clone();
    if let Some(name) = p.name {
        free_name(s, &name, Some(id), CMD)?;
        c.name = name;
    }
    if let Some(prefix) = p.prefix {
        check_prefix(&prefix).map_err(|e| bad(CMD, e))?;
        c.prefix = prefix;
    }
    for (field, new) in [(&mut c.value, p.value), (&mut c.mpn, p.mpn), (&mut c.manufacturer, p.manufacturer), (&mut c.description, p.description)] {
        if let Some(new) = new {
            *field = new;
        }
    }
    if let Some(symbol) = &p.symbol {
        c.symbol = symbol.as_ref().map(|r| find_symbol(&s.doc, r, CMD)).transpose()?;
    }
    let changed = changed([
        ("name", old.name != c.name),
        ("prefix", old.prefix != c.prefix),
        ("value", old.value != c.value),
        ("mpn", old.mpn != c.mpn),
        ("manufacturer", old.manufacturer != c.manufacturer),
        ("description", old.description != c.description),
        ("symbol", old.symbol != c.symbol),
    ]);
    let component = id_of(&c);
    let before = if changed.is_empty() { None } else { super::ecad::sheet_before(s) };
    let detached = if changed.is_empty() { Vec::new() } else { store(s, c, CMD)? };
    let (netlist_changed, texts_reset) = super::ecad::sheet_after(s, before);
    reply(EditResult { component, changed, detached, netlist_changed, texts_reset, param_warnings: Vec::new() })
}

fn copy(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.component.copy";
    let p: CopyParams = parse(CMD, p)?;
    let mut c = load(s, &p.component, CMD)?;
    room(s, CMD)?;
    free_name(s, &p.name, None, CMD)?;
    c.name = p.name;
    c.validate().map_err(|e| bad(CMD, e))?;
    let mut next = s.doc.ecad.clone();
    c.id = next.take_id().map_err(|e| bad(CMD, e))?;
    let component = id_of(&c);
    next.components.push(c);
    commit(s, next, CMD)?;
    reply(CreateResult { component, param_warnings: Vec::new() })
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.component.delete";
    let p: DeleteParams = parse(CMD, p)?;
    if p.components.is_empty() {
        return Err(bad(CMD, "name at least one electronic component in `components`"));
    }
    let mut ids = Vec::new();
    for r in &p.components {
        let id = find(&s.doc, r, CMD)?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let deleted: Vec<ComponentId> = s.doc.ecad.components.iter().filter(|c| ids.contains(&c.id)).map(id_of).collect();
    let sheet = &s.doc.ecad.schematic;
    let placed: Vec<&solidcraft_doc::ecad::Part> = sheet.parts.iter().filter(|q| ids.contains(&q.component)).collect();
    let mut parts = Vec::new();
    let mut next = s.doc.ecad.clone();
    let mut gone = Vec::new();
    if !placed.is_empty() {
        if !p.with_parts.unwrap_or(false) {
            let mut names: Vec<&str> = placed.iter().take(8).map(|q| q.designator.as_str()).collect();
            if placed.len() > names.len() {
                names.push("…");
            }
            return Err(bad(
                CMD,
                format!(
                    "{} placed on the schematic as {}: delete those parts first (ecad.schematic.delete), or give `with_parts: true`",
                    if deleted.len() == 1 { "it is" } else { "they are" },
                    names.join(", ")
                ),
            ));
        }
        let going: std::collections::BTreeSet<u64> = placed.iter().map(|q| q.id).collect();
        parts = placed.iter().map(|q| ItemOut { id: q.id, kind: ItemKind::Part, name: Some(q.designator.clone()), net: None }).collect();
        gone = next.schematic.remove(&going, &s.doc.ecad.library());
    }
    next.components.retain(|c| !ids.contains(&c.id));
    commit(s, next, CMD)?;
    let detached = detached_now(s, &gone);
    reply(DeleteResult { deleted, parts, detached, param_warnings: Vec::new() })
}

fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.component.inspect";
    let p: InspectParams = parse(CMD, p)?;
    let c = load(s, &p.component, CMD)?;
    reply(InspectResult { component: record(s, &c), param_warnings: Vec::new() })
}

fn map_pins(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.component.map_pins";
    let p: MapPinsParams = parse(CMD, p)?;
    let mut c = load(s, &p.component, CMD)?;
    let at = match &p.variant {
        Some(name) => {
            c.variants.iter().position(|v| v.name == *name).ok_or_else(|| bad(CMD, format!("`{}` has no variant named `{name}`", c.name)))?
        }
        None if c.variants.is_empty() => {
            return Err(bad(CMD, format!("`{}` has no variant to connect yet: give it a footprint (ecad.component.variant.add)", c.name)));
        }
        None => 0,
    };
    let map = match (p.map, p.by_number.unwrap_or(false)) {
        (Some(_), true) => return Err(bad(CMD, "give `map` or `by_number`, not both")),
        (None, false) => return Err(bad(CMD, "give a `map` of pins to pads, or `by_number: true`")),
        (None, true) => None,
        (Some(map), false) => Some(map.into_iter().map(|(pin, pads)| (pin, pads.iter().map(|n| n.text()).collect())).collect()),
    };
    if let Some(v) = c.variants.get_mut(at) {
        v.pin_map = map;
    }
    let ecad = &s.doc.ecad;
    let r = ecad.resolve_variant(&c, c.variants.get(at));
    r.check_pin_map().map_err(|e| bad(CMD, e))?;
    let holes = r.pin_map().into_iter().flatten().flat_map(|(pin, pads)| pads.iter().map(move |pad| (pin, pad)));
    for (pin, pad) in holes {
        if r.footprint.pad(pad).is_some_and(|p| p.kind == solidcraft_doc::ecad::PadKind::Hole) {
            return Err(bad(CMD, format!("pad `{pad}` is an unplated hole, with no copper for pin {pin} to connect to")));
        }
    }
    let text = |v: Vec<&str>| v.into_iter().map(str::to_string).collect::<Vec<_>>();
    let reply_ = MapPinsResult {
        component: id_of(&c),
        variant: r.variant.map(|v| v.name.clone()).unwrap_or_default(),
        connections: r.connections().into_iter().map(|(pin, pads)| Connection { pin: pin.to_string(), pads: text(pads) }).collect(),
        unconnected_pins: text(r.unconnected_pins()),
        unconnected_pads: text(r.unconnected_pads()),
        param_warnings: Vec::new(),
    };
    store(s, c, CMD)?;
    reply(reply_)
}

/// Problems as replies give them.
pub(super) fn problems_out(problems: Vec<solidcraft_doc::ecad::Problem>) -> Vec<ProblemOut> {
    problems
        .into_iter()
        .map(|p| ProblemOut {
            code: p.code.to_string(),
            severity: match p.severity {
                solidcraft_doc::ecad::Severity::Error => Severity::Error,
                solidcraft_doc::ecad::Severity::Warning | solidcraft_doc::ecad::Severity::Info => Severity::Warning,
            },
            items: p.items,
            message: p.message,
        })
        .collect()
}

/// A symbol's pins as `ecad.pin.add` takes them.
pub(super) fn pins_out(symbol: &solidcraft_doc::ecad::SymbolItem) -> Vec<PinOut> {
    symbol
        .pins
        .iter()
        .map(|p| PinOut {
            number: p.number.clone(),
            name: p.name.clone(),
            name_hidden: p.name_hidden,
            electrical: electrical_out(p.electrical),
            at: p.at,
            direction: direction_out(p.direction),
            length: p.length,
        })
        .collect()
}

/// One variant and what connects in it (its 3D package measured from the model in `s`).
fn variant_record(s: &Session, r: &Resolved) -> VariantRecord {
    let text = |v: Vec<&str>| v.into_iter().map(str::to_string).collect::<Vec<_>>();
    let package = r.package.map(|p| super::ecad_package::package_out(s, p));
    let mut problems = problems_out(r.problems());
    if let Some(p) = package.as_ref().filter(|_| super::ecad::package_missing(s, r.package)) {
        problems.insert(
            0,
            ProblemOut {
                code: "package_missing".into(),
                severity: Severity::Error,
                items: Vec::new(),
                message: format!(
                    "the 3D package's component {} is gone: give the package another model with ecad.package.edit, or take it off with ecad.component.variant.edit",
                    p.model.id
                ),
            },
        );
    }
    VariantRecord {
        name: r.variant.map(|v| v.name.clone()).unwrap_or_default(),
        footprint: r.variant.and_then(|v| v.footprint).map(|_| item_id(r.footprint.id, &r.footprint.name)),
        pads: r.footprint.pads.len(),
        package,
        pin_map: r.pin_map().cloned(),
        connections: r.connections().into_iter().map(|(pin, pads)| Connection { pin: pin.to_string(), pads: text(pads) }).collect(),
        unconnected_pins: text(r.unconnected_pins()),
        unconnected_pads: text(r.unconnected_pads()),
        problems,
        ready: super::ecad::variant_ready(s, r),
    }
}

/// Everything saved for `c`, and what follows from the library items it uses.
pub(super) fn record(s: &Session, c: &ElectronicComponent) -> ComponentRecord {
    let ecad = &s.doc.ecad;
    let first = ecad.resolve(c, None);
    ComponentRecord {
        id: c.id,
        name: c.name.clone(),
        prefix: c.prefix.clone(),
        value: c.value.clone(),
        mpn: c.mpn.clone(),
        manufacturer: c.manufacturer.clone(),
        description: c.description.clone(),
        symbol: c.symbol.map(|_| item_id(first.symbol.id, &first.symbol.name)),
        pins: pins_out(first.symbol),
        variants: c.variants.iter().map(|v| variant_record(s, &ecad.resolve_variant(c, Some(v)))).collect(),
        problems: problems_out(first.symbol_problems()),
        ready: super::ecad::ready(s, c),
    }
}

#[cfg(test)]
#[path = "ecad_component_tests.rs"]
mod tests;
