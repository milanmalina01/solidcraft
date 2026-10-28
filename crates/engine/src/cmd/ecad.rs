//! Electronics: commands on the design's electronics (`doc.ecad`), the Electronics workspace's data,
//! and what the `ecad.*` command files share.

use serde::de::DeserializeOwned;
use serde_json::Value;
use solidcraft_doc::Document;

use super::CommandSpec;
use crate::params::bad;
use crate::schema::ecad::{ComponentSummary, InspectParams, InspectResult};
use crate::schema::ecad_component::ComponentRef;
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("ecad.inspect", "Inspect Electronics", crate::checked!("ecad.inspect", inspect))
    .icon("inspect")
    .noundo()
    .doc_only()
    .contract(&INSPECT_CONTRACT)
    .output(output_schema_for::<InspectResult>)
    .params(
        "No parameters. Returns {empty, components?: [{id, name, prefix, value}]} (and param_warnings in the warn parameter-check mode): what the design's electronics hold, as data for agents. Read-only",
    )];

const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();

/// A summary of the design's electronics. Reads the document only.
fn inspect(s: &mut Session, _p: &Value) -> Result<Value> {
    let ecad = &s.doc.ecad;
    let components = ecad
        .components
        .iter()
        .map(|c| ComponentSummary { id: c.id, name: c.name.clone(), prefix: c.prefix.clone(), value: c.value.clone() })
        .collect();
    reply(InspectResult { empty: !ecad.has_content(), components, param_warnings: Vec::new() })
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
