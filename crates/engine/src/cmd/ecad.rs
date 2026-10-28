//! Electronics: commands on the design's electronics (`doc.ecad`), the Electronics workspace's data.

use serde_json::Value;

use super::CommandSpec;
use crate::schema::ecad::{InspectParams, InspectResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("ecad.inspect", "Inspect Electronics", crate::checked!("ecad.inspect", inspect))
    .icon("inspect")
    .noundo()
    .doc_only()
    .contract(&INSPECT_CONTRACT)
    .output(output_schema_for::<InspectResult>)
    .params(
        "No parameters. Returns {empty} (and param_warnings in the warn parameter-check mode): what the design's electronics hold, as data for agents. Read-only",
    )];

const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();

/// A summary of the design's electronics. Reads the document only.
fn inspect(s: &mut Session, _p: &Value) -> Result<Value> {
    let reply = InspectResult { empty: s.doc.ecad.is_empty(), param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

#[cfg(test)]
#[path = "ecad_tests.rs"]
mod tests;
