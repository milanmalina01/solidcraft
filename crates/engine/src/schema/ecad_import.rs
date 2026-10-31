//! Contracts and result types for the command in `cmd/ecad_import.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ecad_component::ComponentId;

/// `ecad.library.import`: copy library items, as an electronics library saves them, into this
/// document's electronics.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ImportParams {
    pub items: ImportItems,
}

/// The records to copy, each as a design file saves it under `ecad` (its own id and name; a
/// component names its symbol, footprints and 3D packages by those ids).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ImportItems {
    /// Symbols: `{id, name, description?, pins?, graphics?}`.
    #[serde(default)]
    pub symbols: Vec<Value>,
    /// Footprints: `{id, name, description?, pads?, graphics?}`.
    #[serde(default)]
    pub footprints: Vec<Value>,
    /// 3D packages: `{id, name, model, translate?, rotation?}`; `model` stays the SolidCraft
    /// component it names.
    #[serde(default)]
    pub packages: Vec<Value>,
    /// Electronic components: `{id, name, prefix, value?, mpn?, manufacturer?, description?,
    /// symbol?, variants?}`; the symbol, footprints and 3D packages they name must be among these
    /// items.
    #[serde(default)]
    pub components: Vec<Value>,
}

/// One record given: its id there and here.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ImportedOut {
    /// Its id among the items given.
    pub from: u64,
    /// Its id here.
    pub to: u64,
    /// Its name here: the given one, or with a number when another record has that name.
    pub name: String,
    /// One with the same name and content was here already, so it is that one (nothing copied).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reused: bool,
}

/// What `ecad.library.import` returns: every record given, in the order given.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct ImportResult {
    pub symbols: Vec<ImportedOut>,
    pub footprints: Vec<ImportedOut>,
    pub packages: Vec<ImportedOut>,
    pub components: Vec<ImportedOut>,
    /// 3D packages whose model (a SolidCraft component) this document doesn't have.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models_missing: Vec<ComponentId>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
