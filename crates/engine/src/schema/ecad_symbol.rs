//! Contracts and result types for the commands in `cmd/ecad_symbol.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_component::{ComponentId, ComponentRef, Direction, Electrical, Number, RowSpec};

/// One pin, or a row of them: a line from `at`, where a wire connects, `length` long toward the body.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct PinSpec {
    /// Unique in the symbol, such as `1` or `A3`; it connects to the footprint's pad with the same number.
    pub number: Number,
    /// What the pin does, such as `VCC`; several pins may share a name.
    pub name: Option<String>,
    /// `passive` when left out.
    pub electrical: Option<Electrical>,
    /// Where a wire connects, in millimetres.
    pub at: [f64; 2],
    /// From `at` toward the body.
    pub direction: Direction,
    /// In millimetres; 2.54 when left out.
    pub length: Option<f64>,
    /// Repeat this pin, numbering on from `number`.
    pub row: Option<RowSpec>,
}

/// `ecad.pin.add`: add pins to an electronic component's symbol, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct AddParams {
    pub component: ComponentRef,
    pub pins: Vec<PinSpec>,
    /// Replace every pin the symbol has with these.
    pub replace: Option<bool>,
}

/// `ecad.pin.edit`: change the fields given of one pin.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct EditParams {
    pub component: ComponentRef,
    /// The pin to change, by number.
    pub pin: Number,
    /// A new number, unique in the symbol.
    pub number: Option<Number>,
    pub name: Option<String>,
    pub electrical: Option<Electrical>,
    pub at: Option<[f64; 2]>,
    pub direction: Option<Direction>,
    pub length: Option<f64>,
}

/// `ecad.pin.delete`: delete pins by number, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DeleteParams {
    pub component: ComponentRef,
    pub pins: Vec<Number>,
}

/// What `ecad.pin.add` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AddResult {
    pub component: ComponentId,
    /// How many pins the call added.
    pub added: usize,
    /// How many pins the symbol has now.
    pub pins: usize,
    /// With `replace`: whether a pin-to-pad map that named pins no longer there was dropped.
    pub pin_map_cleared: bool,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.pin.edit` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct EditResult {
    pub component: ComponentId,
    /// The pin's number after the edit.
    pub pin: String,
    /// The fields that changed; empty when nothing did (then there is no undo step).
    pub changed: Vec<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.pin.delete` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DeleteResult {
    pub component: ComponentId,
    pub deleted: Vec<String>,
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
        assert_keys(input_schema_for::<AddParams>(), &["component", "pins", "replace"]);
        assert_keys(input_schema_for::<EditParams>(), &["at", "component", "direction", "electrical", "length", "name", "number", "pin"]);
        assert_keys(input_schema_for::<DeleteParams>(), &["component", "pins"]);
    }
}
