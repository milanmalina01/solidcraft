//! Electronic components: make, change, copy, delete and read the parts the design's electronics
//! are built from. Names and texts follow `solidcraft_doc::ecad`'s rules, checked before anything
//! changes, so a refused call leaves the design as it was.

use serde_json::Value;
use solidcraft_doc::ecad::{ElectronicComponent, MAX_COMPONENTS, check_name, check_prefix};

use super::CommandSpec;
use super::ecad::{changed, direction_out, electrical_out, find, id_of, kind_out, load, parse, reply, shape_out, side_out, store};
use crate::params::bad;
use crate::schema::ecad_component::{
    ComponentId, ComponentRecord, Connection, CopyParams, CourtyardOut, CreateParams, CreateResult, DeleteParams, DeleteResult, EditParams,
    EditResult, GapOut, InspectParams, InspectResult, LayerGraphicOut, MapPinsParams, MapPinsResult, Measured, PadMeasure, PadOut, PinMeasure,
    PinOut, ProblemOut, Severity,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.component.create", "New Electronic Component", crate::checked!("ecad.component.create", create))
        .icon("new")
        .doc_only()
        .contract(&CREATE_CONTRACT)
        .output(output_schema_for::<CreateResult>)
        .params("name (unique, such as R0603); prefix? (designator letters, U when left out); value?, mpn?, manufacturer?, description? → {component: {id, name}}"),
    CommandSpec::new("ecad.component.edit", "Edit Electronic Component", crate::checked!("ecad.component.edit", edit))
        .icon("edit")
        .doc_only()
        .contract(&EDIT_CONTRACT)
        .output(output_schema_for::<EditResult>)
        .params("component: id|name; name?, prefix?, value?, mpn?, manufacturer?, description? (an empty text clears it) → {component, changed: [keys]}"),
    CommandSpec::new("ecad.component.copy", "Copy Electronic Component", crate::checked!("ecad.component.copy", copy))
        .icon("copy")
        .doc_only()
        .contract(&COPY_CONTRACT)
        .output(output_schema_for::<CreateResult>)
        .params("component: id|name; name (the copy's, unique) → {component: {id, name}}"),
    CommandSpec::new("ecad.component.delete", "Delete Electronic Components", crate::checked!("ecad.component.delete", delete))
        .icon("delete")
        .doc_only()
        .contract(&DELETE_CONTRACT)
        .output(output_schema_for::<DeleteResult>)
        .params("components: [id|name] (all or none) → {deleted: [{id, name}]}"),
    CommandSpec::new("ecad.component.inspect", "Inspect Electronic Component", crate::checked!("ecad.component.inspect", inspect))
        .icon("inspect")
        .noundo()
        .contract(&INSPECT_CONTRACT)
        .output(output_schema_for::<InspectResult>)
        .params(
            "component: id|name → {component: {id, name, prefix, value, mpn, manufacturer, description, pins, pads, measured, connections, unconnected_pins, unconnected_pads, pin_map, symbol_graphics, footprint_graphics, package, problems, ready}}. Read-only",
        ),
    CommandSpec::new("ecad.component.map_pins", "Map Pins to Pads", crate::checked!("ecad.component.map_pins", map_pins))
        .icon("edit")
        .doc_only()
        .contract(&MAP_PINS_CONTRACT)
        .output(output_schema_for::<MapPinsResult>)
        .params(
            "component: id|name; map: {pin: [pads]} (replaces the map; a pin left out connects to no pad) | by_number: true (each pin to the pad with its number) → {component, connections, unconnected_pins, unconnected_pads}",
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
    let mut c = ElectronicComponent {
        id: 1,
        name: p.name,
        prefix,
        value: p.value.unwrap_or_default(),
        mpn: p.mpn.unwrap_or_default(),
        manufacturer: p.manufacturer.unwrap_or_default(),
        description: p.description.unwrap_or_default(),
        symbol: Default::default(),
        footprint: Default::default(),
        pin_map: None,
        package: None,
    };
    c.validate().map_err(|e| bad(CMD, e))?;
    c.id = s.doc_mut().ecad.take_id().map_err(|e| bad(CMD, e))?;
    let component = id_of(&c);
    store(s, c, CMD)?;
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
    let changed = changed([
        ("name", old.name != c.name),
        ("prefix", old.prefix != c.prefix),
        ("value", old.value != c.value),
        ("mpn", old.mpn != c.mpn),
        ("manufacturer", old.manufacturer != c.manufacturer),
        ("description", old.description != c.description),
    ]);
    let component = id_of(&c);
    if !changed.is_empty() {
        store(s, c, CMD)?;
    }
    reply(EditResult { component, changed, param_warnings: Vec::new() })
}

fn copy(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.component.copy";
    let p: CopyParams = parse(CMD, p)?;
    let mut c = load(s, &p.component, CMD)?;
    room(s, CMD)?;
    free_name(s, &p.name, None, CMD)?;
    c.name = p.name;
    c.validate().map_err(|e| bad(CMD, e))?;
    c.id = s.doc_mut().ecad.take_id().map_err(|e| bad(CMD, e))?;
    let component = id_of(&c);
    store(s, c, CMD)?;
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
    s.doc_mut().ecad.components.retain(|c| !ids.contains(&c.id));
    reply(DeleteResult { deleted, param_warnings: Vec::new() })
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
    c.pin_map = match (p.map, p.by_number.unwrap_or(false)) {
        (Some(_), true) => return Err(bad(CMD, "give `map` or `by_number`, not both")),
        (None, false) => return Err(bad(CMD, "give a `map` of pins to pads, or `by_number: true`")),
        (None, true) => None,
        (Some(map), false) => Some(map.into_iter().map(|(pin, pads)| (pin, pads.iter().map(|n| n.text()).collect())).collect()),
    };
    c.check_pin_map().map_err(|e| bad(CMD, e))?;
    let holes = c.pin_map.iter().flatten().flat_map(|(pin, pads)| pads.iter().map(move |pad| (pin, pad)));
    for (pin, pad) in holes {
        if c.footprint.pad(pad).is_some_and(|p| p.kind == solidcraft_doc::ecad::PadKind::Hole) {
            return Err(bad(CMD, format!("pad `{pad}` is an unplated hole, with no copper for pin {pin} to connect to")));
        }
    }
    let text = |v: Vec<&str>| v.into_iter().map(str::to_string).collect::<Vec<_>>();
    let reply_ = MapPinsResult {
        component: id_of(&c),
        connections: c.connections().into_iter().map(|(pin, pads)| Connection { pin: pin.to_string(), pads: text(pads) }).collect(),
        unconnected_pins: text(c.unconnected_pins()),
        unconnected_pads: text(c.unconnected_pads()),
        param_warnings: Vec::new(),
    };
    store(s, c, CMD)?;
    reply(reply_)
}

/// Everything saved for `c`, and what is measured from it (its 3D package from the model in `s`).
pub(super) fn record(s: &Session, c: &ElectronicComponent) -> ComponentRecord {
    let pins = c
        .symbol
        .pins
        .iter()
        .map(|p| PinOut {
            number: p.number.clone(),
            name: p.name.clone(),
            electrical: electrical_out(p.electrical),
            at: p.at,
            direction: direction_out(p.direction),
            length: p.length,
        })
        .collect();
    let pads = c
        .footprint
        .pads
        .iter()
        .map(|p| PadOut {
            number: p.number.clone(),
            at: p.at,
            size: p.size,
            rotation: p.rotation,
            side: side_out(p.side),
            shape: shape_out(p.shape),
            corner: p.corner,
            kind: kind_out(p.kind),
            drill: p.drill,
        })
        .collect();
    let measured = Measured {
        pins: c.symbol.pins.iter().map(|p| PinMeasure { number: p.number.clone(), body_end: p.body_end() }).collect(),
        pads: c
            .footprint
            .pads
            .iter()
            .map(|p| PadMeasure { number: p.number.clone(), area: p.area(), bbox: p.bbox(), annular_ring: p.annular_ring() })
            .collect(),
        symbol_bbox: c.symbol.bbox(),
        copper_bbox: c.footprint.copper_bbox(),
        min_pad_gap: c.min_pad_gap().map(|g| GapOut { pads: g.pads, gap: g.gap }),
        courtyard: c.courtyard(solidcraft_doc::ecad::Side::Top).map(|[from, to]| CourtyardOut {
            from,
            to,
            area: (to[0] - from[0]) * (to[1] - from[1]),
        }),
    };
    let package = c.package.as_ref().map(|p| super::ecad_package::package_out(s, p));
    let mut problems: Vec<ProblemOut> = c
        .problems()
        .into_iter()
        .map(|p| ProblemOut {
            code: p.code.to_string(),
            severity: match p.severity {
                solidcraft_doc::ecad::Severity::Error => Severity::Error,
                solidcraft_doc::ecad::Severity::Warning => Severity::Warning,
            },
            items: p.items,
            message: p.message,
        })
        .collect();
    if let Some(p) = package.as_ref().filter(|_| super::ecad::package_missing(s, c)) {
        problems.insert(
            0,
            ProblemOut {
                code: "package_missing".into(),
                severity: Severity::Error,
                items: Vec::new(),
                message: format!("the 3D package's component {} is gone: set another with ecad.package.set, or ecad.package.clear", p.model.id),
            },
        );
    }
    let text = |v: Vec<&str>| v.into_iter().map(str::to_string).collect::<Vec<_>>();
    ComponentRecord {
        id: c.id,
        name: c.name.clone(),
        prefix: c.prefix.clone(),
        value: c.value.clone(),
        mpn: c.mpn.clone(),
        manufacturer: c.manufacturer.clone(),
        description: c.description.clone(),
        pins,
        pads,
        measured,
        connections: c.connections().into_iter().map(|(pin, pads)| Connection { pin: pin.to_string(), pads: text(pads) }).collect(),
        unconnected_pins: text(c.unconnected_pins()),
        unconnected_pads: text(c.unconnected_pads()),
        pin_map: c.pin_map.clone(),
        symbol_graphics: c.symbol.graphics.iter().map(super::ecad_graphics::graphic_out).collect(),
        footprint_graphics: c
            .footprint
            .graphics
            .iter()
            .filter_map(|g| {
                Some(LayerGraphicOut {
                    layer: super::ecad_graphics::layer_out(g.layer?),
                    side: side_out(g.side),
                    graphic: super::ecad_graphics::graphic_out(g),
                })
            })
            .collect(),
        ready: super::ecad::ready(s, c),
        package,
        problems,
    }
}

#[cfg(test)]
#[path = "ecad_component_tests.rs"]
mod tests;
