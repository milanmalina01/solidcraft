//! Contracts and result types for the commands in `cmd/ecad.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `ecad.inspect` takes no parameters: it reports the design's electronics as a whole.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct InspectParams {}

/// What `ecad.inspect` returns. Later fields describe each part of the electronics as it is added.
#[derive(Debug, Serialize, JsonSchema)]
pub struct InspectResult {
    /// Whether the design has no electronics yet.
    pub empty: bool,
    /// How many library items and electronic components there are.
    pub counts: super::ecad_library::Counts,
    /// The library's symbols, in the order they were made; left out when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<ItemSummary>,
    /// The library's footprints; left out when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub footprints: Vec<ItemSummary>,
    /// The library's 3D packages; left out when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<ItemSummary>,
    /// The electronic components, in the order they were made; left out when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ComponentSummary>,
    /// What the schematic holds; left out when the sheet is empty (`ecad.schematic.inspect` has
    /// the rest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schematic: Option<SchematicCounts>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// One library item in the `ecad.inspect` lists (`ecad.library.inspect` says who uses it).
#[derive(Debug, Serialize, JsonSchema)]
pub struct ItemSummary {
    pub id: u64,
    pub name: String,
    /// A symbol's pins, a footprint's pads; left out for a 3D package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<usize>,
}

/// One electronic component in the `ecad.inspect` list (`ecad.component.inspect` has the rest).
#[derive(Debug, Serialize, JsonSchema)]
pub struct ComponentSummary {
    pub id: u64,
    pub name: String,
    pub prefix: String,
    /// `""` when it has none.
    pub value: String,
    /// The library symbol's name; null without one.
    pub symbol: Option<String>,
    /// The variants' names, the default first.
    pub variants: Vec<String>,
    /// How many pins its symbol has.
    pub pins: usize,
    /// How many pads its first variant's footprint has.
    pub pads: usize,
    /// A symbol with pins and every variant ready (`ecad.component.inspect` lists the problems).
    pub ready: bool,
}

/// How many of each sheet object the schematic holds.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SchematicCounts {
    pub parts: usize,
    pub wires: usize,
    pub labels: usize,
    pub markers: usize,
    /// Findings of the electrical rules check accepted.
    pub approvals: usize,
    /// The electrical rules check's errors and warnings not approved (`ecad.erc.inspect` lists
    /// them).
    pub errors: usize,
    pub warnings: usize,
}

/// `ecad.view.schematic`, `ecad.view.board` and `ecad.view.board_3d` take no parameters.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ViewParams {}

/// What the view commands return: the view an electronics design's page shows.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ViewResult {
    /// `schematic` or `board`.
    pub view: String,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{InspectParams, InspectResult};
    use crate::Session;
    use crate::schema::tests::{assert_keys, conforms};
    use crate::schema::{ParamCheck, input_schema_for, output_schema_for};

    #[test]
    fn schemas_and_the_key_check_agree() {
        assert_keys(input_schema_for::<InspectParams>(), &[]);
    }

    #[test]
    fn the_reply_matches_its_schema_and_strict_refuses_unknown_keys() {
        let output = output_schema_for::<InspectResult>().to_value();
        assert_eq!(output["properties"]["empty"]["type"], json!("boolean"));
        let mut s = Session { param_check: ParamCheck::Warn, ..Default::default() };
        let clean = s.execute("ecad.inspect", &json!({})).unwrap();
        assert_eq!(clean["empty"], json!(true));
        assert!(conforms(&output, &output, &clean), "{clean} doesn't match {output}");
        let warned = s.execute("ecad.inspect", &json!({"bogus": 1})).unwrap();
        assert_eq!(warned["empty"], json!(true));
        assert!(conforms(&output, &output, &warned), "{warned} doesn't match {output}");
        assert!(warned["param_warnings"].as_array().is_some_and(|w| !w.is_empty()), "{warned}");
        s.param_check = ParamCheck::Strict;
        let e = s.execute("ecad.inspect", &json!({"bogus": 1})).unwrap_err().to_string();
        assert!(e.starts_with("invalid parameters for `ecad.inspect`: unknown key `bogus`"), "{e}");
    }
}
