//! Contracts and result types for the commands in `cmd/ecad_package.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_component::{ComponentId, ComponentRef, PackageOut};

/// `ecad.package.create`: a library 3D package, a SolidCraft component placed on a footprint.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CreateParams {
    /// Unique among the 3D packages; up to 64 characters.
    pub name: String,
    /// The SolidCraft component, by id or name (one built with solid commands, or one `file.insert_step` made).
    pub model: ComponentRef,
    /// Where the model's origin goes, from the footprint's origin, in millimetres; the board's top is z = 0.
    pub translate: Option<[f64; 3]>,
    /// Degrees about z, counter-clockwise.
    pub rotation: Option<f64>,
    /// An electronic component to give it to, as the 3D package of `variant`.
    pub component: Option<ComponentRef>,
    /// The component's variant, by name: its first when left out, made (named after the package)
    /// when the component has none or none with that name.
    pub variant: Option<String>,
}

/// `ecad.package.edit`: change the fields given of a library 3D package; every variant that uses
/// it follows.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct EditParams {
    pub package: ComponentRef,
    pub name: Option<String>,
    pub model: Option<ComponentRef>,
    pub translate: Option<[f64; 3]>,
    pub rotation: Option<f64>,
}

/// `ecad.package.delete`: delete a library 3D package.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DeleteParams {
    pub package: ComponentRef,
    /// Clear the variants' references to it; without it, a 3D package a variant uses is refused.
    pub force: Option<bool>,
}

/// What `ecad.package.create` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CreateResult {
    pub package: PackageOut,
    /// With `component`: the electronic component and the variant that use it now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.package.edit` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct EditResult {
    pub package: PackageOut,
    /// The fields that changed; empty when nothing did (then there is no undo step).
    pub changed: Vec<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{CreateParams, DeleteParams, EditParams};
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn schemas_and_the_key_check_agree() {
        assert_keys(input_schema_for::<CreateParams>(), &["component", "model", "name", "rotation", "translate", "variant"]);
        assert_keys(input_schema_for::<EditParams>(), &["model", "name", "package", "rotation", "translate"]);
        assert_keys(input_schema_for::<DeleteParams>(), &["force", "package"]);
    }
}
