//! Contracts and result types for the commands in `cmd/ecad_package.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_component::{ComponentId, ComponentRef, PackageOut};

/// `ecad.package.set`: make a SolidCraft component the electronic component's 3D package.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SetParams {
    pub component: ComponentRef,
    /// The SolidCraft component, by id or name (one built with solid commands, or one `file.insert_step` made).
    pub model: ComponentRef,
    /// Where the model's origin goes, from the footprint's origin, in millimetres; the board's top is z = 0.
    pub translate: Option<[f64; 3]>,
    /// Degrees about z, counter-clockwise.
    pub rotation: Option<f64>,
}

/// `ecad.package.clear`: no 3D package.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ClearParams {
    pub component: ComponentRef,
}

/// What `ecad.package.set` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SetResult {
    pub component: ComponentId,
    pub package: PackageOut,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.package.clear` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ClearResult {
    pub component: ComponentId,
    /// Whether there was a package to clear.
    pub cleared: bool,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{ClearParams, SetParams};
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn schemas_and_the_key_check_agree() {
        assert_keys(input_schema_for::<SetParams>(), &["component", "model", "rotation", "translate"]);
        assert_keys(input_schema_for::<ClearParams>(), &["component"]);
    }
}
