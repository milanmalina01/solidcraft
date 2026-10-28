//! Electronic components: make, change, copy, delete and read the parts the design's electronics
//! are built from. Names and texts follow `solidcraft_doc::ecad`'s rules, checked before anything
//! changes, so a refused call leaves the design as it was.

use serde_json::Value;
use solidcraft_doc::ecad::{ElectronicComponent, MAX_COMPONENTS, check_name, check_prefix};

use super::CommandSpec;
use super::ecad::{find, parse, reply};
use crate::params::bad;
use crate::schema::ecad_component::{
    ComponentId, ComponentRecord, CopyParams, CreateParams, CreateResult, DeleteParams, DeleteResult, EditParams, EditResult, InspectParams,
    InspectResult,
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
        .doc_only()
        .contract(&INSPECT_CONTRACT)
        .output(output_schema_for::<InspectResult>)
        .params("component: id|name → {component: {id, name, prefix, value, mpn, manufacturer, description}}. Read-only"),
];

const CREATE_CONTRACT: TypedContract<CreateParams> = TypedContract::new();
const EDIT_CONTRACT: TypedContract<EditParams> = TypedContract::new();
const COPY_CONTRACT: TypedContract<CopyParams> = TypedContract::new();
const DELETE_CONTRACT: TypedContract<DeleteParams> = TypedContract::new();
const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();

fn id_of(c: &ElectronicComponent) -> ComponentId {
    ComponentId { id: c.id, name: c.name.clone() }
}

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

/// Store `c` (new, or in place of the record with its id) after checking it as the design file
/// does.
fn store(s: &mut Session, c: ElectronicComponent, cmd: &str) -> Result<()> {
    c.validate().map_err(|e| bad(cmd, e))?;
    let ecad = &mut s.doc_mut().ecad;
    match ecad.components.iter_mut().find(|o| o.id == c.id) {
        Some(old) => *old = c,
        None => ecad.components.push(c),
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
    let id = find(&s.doc, &p.component, CMD)?;
    let Some(old) = s.doc.ecad.component(id).cloned() else { return Err(bad(CMD, format!("no electronic component has the id {id}"))) };
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
    let changed: Vec<String> = [
        ("name", old.name != c.name),
        ("prefix", old.prefix != c.prefix),
        ("value", old.value != c.value),
        ("mpn", old.mpn != c.mpn),
        ("manufacturer", old.manufacturer != c.manufacturer),
        ("description", old.description != c.description),
    ]
    .into_iter()
    .filter(|(_, differs)| *differs)
    .map(|(key, _)| key.to_string())
    .collect();
    let component = id_of(&c);
    if !changed.is_empty() {
        store(s, c, CMD)?;
    }
    reply(EditResult { component, changed, param_warnings: Vec::new() })
}

fn copy(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.component.copy";
    let p: CopyParams = parse(CMD, p)?;
    let id = find(&s.doc, &p.component, CMD)?;
    room(s, CMD)?;
    free_name(s, &p.name, None, CMD)?;
    let Some(mut c) = s.doc.ecad.component(id).cloned() else { return Err(bad(CMD, format!("no electronic component has the id {id}"))) };
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
    let id = find(&s.doc, &p.component, CMD)?;
    let Some(c) = s.doc.ecad.component(id) else { return Err(bad(CMD, format!("no electronic component has the id {id}"))) };
    let component = ComponentRecord {
        id: c.id,
        name: c.name.clone(),
        prefix: c.prefix.clone(),
        value: c.value.clone(),
        mpn: c.mpn.clone(),
        manufacturer: c.manufacturer.clone(),
        description: c.description.clone(),
    };
    reply(InspectResult { component, param_warnings: Vec::new() })
}

#[cfg(test)]
#[path = "ecad_component_tests.rs"]
mod tests;
