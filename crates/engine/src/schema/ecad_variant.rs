//! Contracts and result types for the commands in `cmd/ecad_variant.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_component::{ComponentId, ComponentRef};

/// `ecad.component.variant.add`: another way to build an electronic component.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct AddParams {
    pub component: ComponentRef,
    /// Unique in the component, such as `SOT-23`; up to 64 characters.
    pub name: String,
    /// The library footprint.
    pub footprint: Option<ComponentRef>,
    /// The library 3D package.
    pub package: Option<ComponentRef>,
}

/// `ecad.component.variant.edit`: change the fields given of one variant; `null` clears the
/// footprint or the 3D package.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct EditParams {
    pub component: ComponentRef,
    /// The variant to change, by name.
    pub variant: String,
    /// A new name, unique in the component; parts on the schematic built with it follow.
    pub name: Option<String>,
    #[serde(default, deserialize_with = "super::ecad_schematic::nullable")]
    #[schemars(with = "Option<ComponentRef>")]
    pub footprint: Option<Option<ComponentRef>>,
    #[serde(default, deserialize_with = "super::ecad_schematic::nullable")]
    #[schemars(with = "Option<ComponentRef>")]
    pub package: Option<Option<ComponentRef>>,
}

/// `ecad.component.variant.delete`: delete one variant.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DeleteParams {
    pub component: ComponentRef,
    pub variant: String,
}

/// What the variant commands return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct VariantResult {
    pub component: ComponentId,
    /// The variant's name after the call (the deleted one's, for a delete).
    pub variant: String,
    /// The fields that changed; empty when nothing did (then there is no undo step).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    /// Whether a pin map that named pads the new footprint doesn't have was dropped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pin_map_cleared: bool,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{AddParams, DeleteParams, EditParams};
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn schemas_and_the_key_check_agree() {
        assert_keys(input_schema_for::<AddParams>(), &["component", "footprint", "name", "package"]);
        assert_keys(input_schema_for::<EditParams>(), &["component", "footprint", "name", "package", "variant"]);
        assert_keys(input_schema_for::<DeleteParams>(), &["component", "variant"]);
    }
}
