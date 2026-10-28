//! An electronic component's 3D package: a link to an ordinary SolidCraft component (made with the
//! solid commands, or the component `file.insert_step` makes, with the components under it) and
//! where it sits on the footprint.
//! Nothing is copied or stored twice: deleting the model clears the link, and undo brings both back.

use serde_json::Value;
use solidcraft_doc::Document;
use solidcraft_doc::ecad::Package;

use super::CommandSpec;
use super::ecad::{id_of, load, parse, reply, store};
use crate::params::bad;
use crate::schema::ecad_component::{ComponentRef, PackageModel, PackageOut};
use crate::schema::ecad_package::{ClearParams, ClearResult, SetParams, SetResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.package.set", "Set 3D Package", crate::checked!("ecad.package.set", set))
        .icon("component")
        .contract(&SET_CONTRACT)
        .output(output_schema_for::<SetResult>)
        .params(
            "component: id|name (the electronic component); model: id|name (a SolidCraft component); translate? [x, y, z] mm (the board's top is z = 0); rotation? degrees about z → {component, package: {model, translate, rotation, bbox, height}}",
        ),
    CommandSpec::new("ecad.package.clear", "Clear 3D Package", crate::checked!("ecad.package.clear", clear))
        .icon("delete")
        .doc_only()
        .contract(&CLEAR_CONTRACT)
        .output(output_schema_for::<ClearResult>)
        .params("component: id|name → {component, cleared}"),
];

const SET_CONTRACT: TypedContract<SetParams> = TypedContract::new();
const CLEAR_CONTRACT: TypedContract<ClearParams> = TypedContract::new();

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
pub(super) fn package_out(s: &Session, p: &Package) -> PackageOut {
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
    PackageOut { model: PackageModel { id: p.model, name }, translate: p.translate, rotation: p.rotation, bbox, height: bbox.map(|[_, hi]| hi[2]) }
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.package.set";
    let p: SetParams = parse(CMD, p)?;
    let mut c = load(s, &p.component, CMD)?;
    let package = Package { model: model(&s.doc, &p.model, CMD)?, translate: p.translate.unwrap_or_default(), rotation: p.rotation.unwrap_or(0.0) };
    package.validate().map_err(|e| bad(CMD, e))?;
    let out = package_out(s, &package);
    c.package = Some(package);
    let component = id_of(&c);
    store(s, c, CMD)?;
    reply(SetResult { component, package: out, param_warnings: Vec::new() })
}

fn clear(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.package.clear";
    let p: ClearParams = parse(CMD, p)?;
    let mut c = load(s, &p.component, CMD)?;
    let cleared = c.package.take().is_some();
    let component = id_of(&c);
    store(s, c, CMD)?;
    reply(ClearResult { component, cleared, param_warnings: Vec::new() })
}

#[cfg(test)]
#[path = "ecad_package_tests.rs"]
mod tests;
