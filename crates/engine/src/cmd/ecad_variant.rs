//! An electronic component's variants: the ways it can be built, each a library footprint, a 3D
//! package on it and its own pin-to-pad map. A part on the schematic is built with one (the
//! component's first unless it names another), so a variant a part uses can't go.

use serde_json::Value;
use solidcraft_doc::ecad::{MAX_VARIANTS, Variant, check_item_name};

use super::CommandSpec;
use super::ecad::{changed, clear_stale_maps, commit, find_footprint, find_package, id_of, load, parse, reply};
use crate::params::bad;
use crate::schema::ecad_variant::{AddParams, DeleteParams, EditParams, VariantResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.component.variant.add", "Add Variant", crate::checked!("ecad.component.variant.add", add))
        .icon("plus")
        .doc_only()
        .contract(&ADD_CONTRACT)
        .output(output_schema_for::<VariantResult>)
        .params("component: id|name; name (unique in the component, such as SOT-23); footprint?: id|name; package?: id|name (library items) → {component, variant}"),
    CommandSpec::new("ecad.component.variant.edit", "Edit Variant", crate::checked!("ecad.component.variant.edit", edit))
        .icon("edit")
        .doc_only()
        .contract(&EDIT_CONTRACT)
        .output(output_schema_for::<VariantResult>)
        .params(
            "component: id|name; variant: name; name? (parts built with it follow); footprint?: id|name|null; package?: id|name|null → {component, variant, changed: [keys], pin_map_cleared?}: a pin map naming pads the new footprint lacks is dropped",
        ),
    CommandSpec::new("ecad.component.variant.delete", "Delete Variant", crate::checked!("ecad.component.variant.delete", delete))
        .icon("delete")
        .doc_only()
        .contract(&DELETE_CONTRACT)
        .output(output_schema_for::<VariantResult>)
        .params(
            "component: id|name; variant: name → {component, variant}; refused while a part on the schematic is built with it (the first variant: a part that names none)",
        ),
];

const ADD_CONTRACT: TypedContract<AddParams> = TypedContract::new();
const EDIT_CONTRACT: TypedContract<EditParams> = TypedContract::new();
const DELETE_CONTRACT: TypedContract<DeleteParams> = TypedContract::new();

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.component.variant.add";
    let p: AddParams = parse(CMD, p)?;
    let c = load(s, &p.component, CMD)?;
    check_item_name("variant", &p.name).map_err(|e| bad(CMD, e))?;
    if c.variant(&p.name).is_some() {
        return Err(bad(CMD, format!("`{}` already has a variant named `{}`", c.name, p.name)));
    }
    if c.variants.len() >= MAX_VARIANTS {
        return Err(bad(CMD, format!("`{}` has {MAX_VARIANTS} variants already", c.name)));
    }
    let footprint = p.footprint.as_ref().map(|r| find_footprint(&s.doc, r, CMD)).transpose()?;
    let package = p.package.as_ref().map(|r| find_package(&s.doc, r, CMD)).transpose()?;
    let mut next = s.doc.ecad.clone();
    if let Some(slot) = next.components.iter_mut().find(|o| o.id == c.id) {
        slot.variants.push(Variant { name: p.name.clone(), footprint, package, pin_map: None });
    }
    commit(s, next, CMD)?;
    reply(VariantResult { component: id_of(&c), variant: p.name, changed: Vec::new(), pin_map_cleared: false, param_warnings: Vec::new() })
}

/// The parts on the schematic built with the variant `name` of the component `id`: those that
/// name it, and, for its first variant, those that name none.
fn built_with(s: &Session, id: u64, name: &str, first: bool) -> Vec<String> {
    let parts = s.doc.ecad.schematic.placed(id);
    parts.filter(|q| q.variant.as_deref() == Some(name) || (first && q.variant.is_none())).map(|q| q.designator.clone()).collect()
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.component.variant.edit";
    let p: EditParams = parse(CMD, p)?;
    let c = load(s, &p.component, CMD)?;
    let Some(old) = c.variant(&p.variant).cloned() else {
        return Err(bad(CMD, format!("`{}` has no variant named `{}`", c.name, p.variant)));
    };
    let mut v = old.clone();
    if let Some(name) = p.name {
        check_item_name("variant", &name).map_err(|e| bad(CMD, e))?;
        if name != old.name && c.variant(&name).is_some() {
            return Err(bad(CMD, format!("`{}` already has a variant named `{name}`", c.name)));
        }
        v.name = name;
    }
    if let Some(footprint) = &p.footprint {
        v.footprint = footprint.as_ref().map(|r| find_footprint(&s.doc, r, CMD)).transpose()?;
    }
    if let Some(package) = &p.package {
        v.package = package.as_ref().map(|r| find_package(&s.doc, r, CMD)).transpose()?;
    }
    let changed = changed([("name", old.name != v.name), ("footprint", old.footprint != v.footprint), ("package", old.package != v.package)]);
    let mut pin_map_cleared = false;
    if !changed.is_empty() {
        let mut next = s.doc.ecad.clone();
        if let Some(slot) = next.components.iter_mut().find(|o| o.id == c.id).and_then(|o| o.variants.iter_mut().find(|x| x.name == old.name)) {
            *slot = v.clone();
        }
        // Parts built with it by name follow a rename.
        if old.name != v.name {
            for q in next.schematic.parts.iter_mut().filter(|q| q.component == c.id && q.variant.as_deref() == Some(old.name.as_str())) {
                q.variant = Some(v.name.clone());
            }
        }
        pin_map_cleared = clear_stale_maps(&mut next, c.id);
        commit(s, next, CMD)?;
    }
    reply(VariantResult { component: id_of(&c), variant: v.name, changed, pin_map_cleared, param_warnings: Vec::new() })
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.component.variant.delete";
    let p: DeleteParams = parse(CMD, p)?;
    let c = load(s, &p.component, CMD)?;
    let Some(at) = c.variants.iter().position(|v| v.name == p.variant) else {
        return Err(bad(CMD, format!("`{}` has no variant named `{}`", c.name, p.variant)));
    };
    let parts = built_with(s, c.id, &p.variant, at == 0);
    if !parts.is_empty() {
        let mut names: Vec<&str> = parts.iter().take(8).map(String::as_str).collect();
        if parts.len() > names.len() {
            names.push("…");
        }
        return Err(bad(
            CMD,
            format!(
                "{} on the schematic {} built with it: give them another variant first (ecad.schematic.edit), or delete them",
                names.join(", "),
                if parts.len() == 1 { "is" } else { "are" }
            ),
        ));
    }
    let mut next = s.doc.ecad.clone();
    if let Some(slot) = next.components.iter_mut().find(|o| o.id == c.id) {
        slot.variants.retain(|v| v.name != p.variant);
    }
    commit(s, next, CMD)?;
    reply(VariantResult { component: id_of(&c), variant: p.variant, changed: Vec::new(), pin_map_cleared: false, param_warnings: Vec::new() })
}
