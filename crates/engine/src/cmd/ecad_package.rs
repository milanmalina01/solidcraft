//! The library's 3D packages: links to ordinary SolidCraft components (made with the solid
//! commands, or the component `file.insert_step` makes, with the components under it) and where
//! each sits on a footprint. A variant of an electronic component uses one by id, so one model
//! serves every variant that names it.
//! Nothing is copied or stored twice: deleting the model deletes the package and clears the
//! variants' references to it, and undo brings both back.

use serde_json::Value;
use solidcraft_doc::Document;
use solidcraft_doc::ecad::{PackageItem, check_item_name};

use super::CommandSpec;
use super::ecad::{changed, commit, find, find_package, item_id, load_package, parse, reply, set_variant};
use super::ecad_library::{free_item_name, some_names};
use crate::params::bad;
use crate::schema::ecad_component::{ComponentRef, PackageModel, PackageOut};
use crate::schema::ecad_library::{ItemDeleteResult, VariantUse};
use crate::schema::ecad_package::{CreateParams, CreateResult, DeleteParams, EditParams, EditResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.package.create", "3D Package", crate::checked!("ecad.package.create", create))
        .icon("box")
        .contract(&CREATE_CONTRACT)
        .output(output_schema_for::<CreateResult>)
        .params(
            "name (unique among the 3D packages); model: id|name (a SolidCraft component); translate? [x, y, z] mm (the board's top is z = 0); rotation? degrees about z; component?: id|name, variant? (gives it to that variant: the first when left out, made when missing) → {package: {id, name, model, translate, rotation, bbox, height}, component?, variant?}",
        ),
    CommandSpec::new("ecad.package.edit", "Edit 3D Package", crate::checked!("ecad.package.edit", edit))
        .icon("edit")
        .contract(&EDIT_CONTRACT)
        .output(output_schema_for::<EditResult>)
        .params("package: id|name; name?, model?: id|name, translate?, rotation? → {package, changed: [keys]}: every variant that uses it follows"),
    CommandSpec::new("ecad.package.delete", "Delete 3D Package", crate::checked!("ecad.package.delete", delete))
        .icon("delete")
        .doc_only()
        .contract(&DELETE_CONTRACT)
        .output(output_schema_for::<ItemDeleteResult>)
        .params(
            "package: id|name; force? (a 3D package a variant uses is refused without it; with it those variants are left with none) → {deleted: {id, name}, cleared?: [{component, variant}]}",
        ),
];

const CREATE_CONTRACT: TypedContract<CreateParams> = TypedContract::new();
const EDIT_CONTRACT: TypedContract<EditParams> = TypedContract::new();
const DELETE_CONTRACT: TypedContract<DeleteParams> = TypedContract::new();

/// The SolidCraft component `r` names (never the design itself).
fn model(doc: &Document, r: &ComponentRef, cmd: &str) -> Result<u64> {
    let found = match r {
        ComponentRef::Id(id) => doc.components.iter().find(|c| c.id == *id),
        ComponentRef::Name(name) => doc.components.iter().find(|c| c.name == *name),
    };
    match (found, r) {
        (Some(c), _) => Ok(c.id),
        (None, ComponentRef::Id(0)) => Err(bad(cmd, "the design itself can't be a 3D package: make a component for it")),
        (None, ComponentRef::Name(name)) if doc.ecad.named(name).is_some() => {
            Err(bad(cmd, format!("`{name}` is an electronic component; `model` is the SolidCraft component that holds the package's bodies")))
        }
        (None, _) => Err(bad(cmd, "`model` must be a SolidCraft component of this design, by id or name")),
    }
}

/// What the package measures: its model's bodies, placed on the footprint.
pub(super) fn package_out(s: &Session, p: &PackageItem) -> PackageOut {
    let name = s.doc.components.iter().find(|c| c.id == p.model).map(|c| c.name.clone());
    let mut corners = Vec::new();
    if name.is_some() {
        let state = s.model.state();
        // The model's own bodies and those of the components under it (an imported assembly's).
        for b in state.bodies.iter().filter(|b| s.doc.component_within(s.doc.body_component(&b.name, b.feature), p.model)) {
            let Ok(m) = solidcraft_kernel::measure(&b.body) else { continue };
            let (lo, hi) = (m.bbox.min, m.bbox.max);
            for x in [lo.x, hi.x] {
                for y in [lo.y, hi.y] {
                    for z in [lo.z, hi.z] {
                        corners.push(p.place([x, y, z]));
                    }
                }
            }
        }
    }
    let bbox = corners.iter().all(|c| c.iter().all(|v| v.is_finite())).then(|| {
        corners.iter().fold(None, |acc: Option<[[f64; 3]; 2]>, c| {
            Some(match acc {
                None => [*c, *c],
                Some([lo, hi]) => [[lo[0].min(c[0]), lo[1].min(c[1]), lo[2].min(c[2])], [hi[0].max(c[0]), hi[1].max(c[1]), hi[2].max(c[2])]],
            })
        })
    });
    let bbox = bbox.flatten();
    PackageOut {
        id: p.id,
        name: p.name.clone(),
        model: PackageModel { id: p.model, name },
        translate: p.translate,
        rotation: p.rotation,
        bbox,
        height: bbox.map(|[_, hi]| hi[2]),
    }
}

fn package_names(s: &Session) -> impl ExactSizeIterator<Item = (u64, &str)> {
    s.doc.ecad.packages.iter().map(|x| (x.id, x.name.as_str()))
}

fn create(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.package.create";
    let p: CreateParams = parse(CMD, p)?;
    free_item_name("3D package", package_names(s), &p.name, None, CMD)?;
    let component = p.component.as_ref().map(|r| find(&s.doc, r, CMD)).transpose()?;
    if p.variant.is_some() && component.is_none() {
        return Err(bad(CMD, "`variant` is a variant of `component`: give the component too"));
    }
    let mut next = s.doc.ecad.clone();
    let package = PackageItem {
        id: next.take_id().map_err(|e| bad(CMD, e))?,
        name: p.name,
        model: model(&s.doc, &p.model, CMD)?,
        translate: p.translate.unwrap_or_default(),
        rotation: p.rotation.unwrap_or(0.0),
    };
    package.validate().map_err(|e| bad(CMD, e))?;
    let out = package_out(s, &package);
    let (id, name) = (package.id, package.name.clone());
    next.packages.push(package);
    let (mut component_out, mut variant) = (None, None);
    if let Some(c) = component {
        let (c, v, _) = set_variant(&mut next, c, p.variant.as_deref(), &name, |v| v.package = Some(id), CMD)?;
        (component_out, variant) = (Some(c), Some(v));
    }
    commit(s, next, CMD)?;
    reply(CreateResult { package: out, component: component_out, variant, param_warnings: Vec::new() })
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.package.edit";
    let p: EditParams = parse(CMD, p)?;
    let old = load_package(s, &p.package, CMD)?;
    let mut package = old.clone();
    if let Some(name) = p.name {
        free_item_name("3D package", package_names(s), &name, Some(old.id), CMD)?;
        package.name = name;
    }
    if let Some(r) = &p.model {
        package.model = model(&s.doc, r, CMD)?;
    }
    if let Some(t) = p.translate {
        package.translate = t;
    }
    if let Some(r) = p.rotation {
        package.rotation = r;
    }
    check_item_name("3D package", &package.name).map_err(|e| bad(CMD, e))?;
    package.validate_placement().map_err(|e| bad(CMD, e))?;
    let changed = changed([
        ("name", old.name != package.name),
        ("model", old.model != package.model),
        ("translate", old.translate != package.translate),
        ("rotation", old.rotation != package.rotation),
    ]);
    let out = package_out(s, &package);
    if !changed.is_empty() {
        let mut next = s.doc.ecad.clone();
        if let Some(slot) = next.packages.iter_mut().find(|x| x.id == package.id) {
            *slot = package;
        }
        commit(s, next, CMD)?;
    }
    reply(EditResult { package: out, changed, param_warnings: Vec::new() })
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.package.delete";
    let p: DeleteParams = parse(CMD, p)?;
    let id = find_package(&s.doc, &p.package, CMD)?;
    let ecad = &s.doc.ecad;
    let deleted = ecad.package(id).map(|x| item_id(x.id, &x.name)).unwrap_or_else(|| item_id(id, ""));
    let users: Vec<VariantUse> =
        ecad.package_users(id).map(|(c, v)| VariantUse { component: super::ecad::id_of(c), variant: v.name.clone() }).collect();
    if !users.is_empty() && !p.force.unwrap_or(false) {
        let names: Vec<String> = users.iter().map(|u| format!("{} ({})", u.component.name, u.variant)).collect();
        return Err(bad(
            CMD,
            format!(
                "{} {} it: take it off them first (ecad.component.variant.edit), or give `force: true`",
                some_names(&names),
                if users.len() == 1 { "uses" } else { "use" }
            ),
        ));
    }
    let mut next = ecad.clone();
    for v in next.components.iter_mut().flat_map(|c| c.variants.iter_mut()).filter(|v| v.package == Some(id)) {
        v.package = None;
    }
    next.packages.retain(|x| x.id != id);
    commit(s, next, CMD)?;
    reply(ItemDeleteResult { deleted, cleared: users, param_warnings: Vec::new() })
}

#[cfg(test)]
#[path = "ecad_package_tests.rs"]
mod tests;
