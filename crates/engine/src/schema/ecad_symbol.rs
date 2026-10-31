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
    /// Keep the name but don't draw it (the body's shape says it); false when left out.
    pub name_hidden: Option<bool>,
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

/// `ecad.pin.add`: add pins to a library symbol, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct AddParams {
    pub symbol: ComponentRef,
    pub pins: Vec<PinSpec>,
    /// Replace every pin the symbol has with these.
    pub replace: Option<bool>,
}

/// `ecad.pin.edit`: change the fields given of one pin.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct EditParams {
    pub symbol: ComponentRef,
    /// The pin to change, by number.
    pub pin: Number,
    /// A new number, unique in the symbol.
    pub number: Option<Number>,
    pub name: Option<String>,
    /// Keep the name but don't draw it.
    pub name_hidden: Option<bool>,
    pub electrical: Option<Electrical>,
    pub at: Option<[f64; 2]>,
    pub direction: Option<Direction>,
    pub length: Option<f64>,
}

/// `ecad.pin.delete`: delete pins by number, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DeleteParams {
    pub symbol: ComponentRef,
    pub pins: Vec<Number>,
}

/// What `ecad.pin.add` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AddResult {
    pub symbol: ComponentId,
    /// How many pins the call added.
    pub added: usize,
    /// How many pins the symbol has now.
    pub pins: usize,
    /// With `replace`: whether a variant's pin-to-pad map that named pins no longer there was
    /// dropped (in any component that shows the symbol).
    pub pin_map_cleared: bool,
    /// Wire ends, labels and markers on the schematic that let go of a pin that is gone, at the
    /// point where it was (left out when there are none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<super::ecad_schematic::DetachedOut>,
    /// The nets whose pins the edit changed on the schematic: pins that moved within their parts
    /// can land on other wires or leave their own (left out when none did).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub netlist_changed: Vec<super::ecad_schematic::NetChangeOut>,
    /// The parts whose moved designator or value went back to where the newly drawn symbol puts
    /// it, by designator (left out when none did).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts_reset: Vec<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.pin.edit` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct EditResult {
    pub symbol: ComponentId,
    /// The pin's number after the edit.
    pub pin: String,
    /// The fields that changed; empty when nothing did (then there is no undo step).
    pub changed: Vec<String>,
    /// The nets whose pins the edit changed on the schematic: pins that moved within their parts
    /// can land on other wires or leave their own (left out when none did).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub netlist_changed: Vec<super::ecad_schematic::NetChangeOut>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.pin.delete` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DeleteResult {
    pub symbol: ComponentId,
    pub deleted: Vec<String>,
    /// Wire ends, labels and markers on the schematic that let go of a pin that is gone, at the
    /// point where it was (left out when there are none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<super::ecad_schematic::DetachedOut>,
    /// The nets whose pins the edit changed on the schematic: pins that moved within their parts
    /// can land on other wires or leave their own (left out when none did).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub netlist_changed: Vec<super::ecad_schematic::NetChangeOut>,
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
        assert_keys(input_schema_for::<AddParams>(), &["pins", "replace", "symbol"]);
        assert_keys(input_schema_for::<EditParams>(), &["at", "direction", "electrical", "length", "name", "name_hidden", "number", "pin", "symbol"]);
        assert_keys(input_schema_for::<DeleteParams>(), &["pins", "symbol"]);
    }
}
