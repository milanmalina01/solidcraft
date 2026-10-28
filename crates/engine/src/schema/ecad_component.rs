//! Contracts and result types for the commands in `cmd/ecad_component.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// An electronic component, by its id (as a reply gave it) or by its name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ComponentRef {
    /// Its id.
    Id(u64),
    /// Its name, exactly.
    Name(String),
}

impl<'de> Deserialize<'de> for ComponentRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::String(s) => Ok(ComponentRef::Name(s)),
            v => v.as_u64().map(ComponentRef::Id).ok_or_else(|| serde::de::Error::custom("must be an electronic component's id or name")),
        }
    }
}

/// `ecad.component.create`: a new electronic component, with no symbol or footprint yet.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CreateParams {
    /// Unique in the design, such as `R0603` or `LM358`; up to 64 characters.
    pub name: String,
    /// The designator prefix, 1 to 8 letters: `R` for R1, R2…; `U` when left out.
    pub prefix: Option<String>,
    /// The value printed on the schematic, such as `10k`.
    pub value: Option<String>,
    /// The manufacturer part number.
    pub mpn: Option<String>,
    pub manufacturer: Option<String>,
    /// Up to 256 characters.
    pub description: Option<String>,
}

/// `ecad.component.edit`: change the fields given; an empty text clears `value`, `mpn`,
/// `manufacturer` or `description`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct EditParams {
    /// The electronic component to change.
    pub component: ComponentRef,
    /// A new name, unique in the design.
    pub name: Option<String>,
    pub prefix: Option<String>,
    pub value: Option<String>,
    pub mpn: Option<String>,
    pub manufacturer: Option<String>,
    pub description: Option<String>,
}

/// `ecad.component.copy`: a new electronic component with the same fields and a new name.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CopyParams {
    /// The electronic component to copy.
    pub component: ComponentRef,
    /// The copy's name, unique in the design.
    pub name: String,
}

/// `ecad.component.delete`: delete electronic components, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DeleteParams {
    /// At least one, by id or name.
    pub components: Vec<ComponentRef>,
}

/// `ecad.component.inspect`: everything saved for one electronic component. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct InspectParams {
    pub component: ComponentRef,
}

/// An electronic component as replies name it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ComponentId {
    pub id: u64,
    pub name: String,
}

/// What `ecad.component.create` and `ecad.component.copy` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CreateResult {
    /// The new electronic component.
    pub component: ComponentId,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.component.edit` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct EditResult {
    pub component: ComponentId,
    /// The fields that changed, by key; empty when the call changed nothing (then there is no
    /// undo step).
    pub changed: Vec<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.component.delete` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DeleteResult {
    pub deleted: Vec<ComponentId>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// An electronic component's saved fields; empty texts are `""`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ComponentRecord {
    pub id: u64,
    pub name: String,
    pub prefix: String,
    pub value: String,
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
}

/// What `ecad.component.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct InspectResult {
    pub component: ComponentRecord,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
#[path = "ecad_component_tests.rs"]
mod tests;
