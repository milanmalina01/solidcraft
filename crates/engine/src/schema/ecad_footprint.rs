//! Contracts and result types for the commands in `cmd/ecad_footprint.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_component::{ComponentId, ComponentRef, DrillSpec, Kind, Number, RowSpec, Shape, Side};

/// One pad, or a row of them.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct PadSpec {
    /// Unique in the footprint, such as `1` or `A3`; the symbol's pin with the same number connects to it.
    pub number: Number,
    /// The pad's centre, in millimetres from the footprint's origin.
    pub at: [f64; 2],
    /// Width along x and height along y before the rotation, in millimetres.
    pub size: [f64; 2],
    /// Degrees counter-clockwise about the centre; 0 when left out.
    pub rotation: Option<f64>,
    /// For a surface-mount pad; `top` when left out.
    pub side: Option<Side>,
    /// `rect` when left out.
    pub shape: Option<Shape>,
    /// A rounded rectangle's corner radius, as a share of the shorter side (more than 0, at most 0.5).
    pub corner: Option<f64>,
    /// `smd` when left out, or `through_hole` when a `drill` is given.
    pub kind: Option<Kind>,
    /// A through-hole pad's or a hole's drill.
    pub drill: Option<DrillSpec>,
    /// Repeat this pad, numbering on from `number`.
    pub row: Option<RowSpec>,
}

/// `ecad.pad.add`: add pads to an electronic component's footprint, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct AddParams {
    pub component: ComponentRef,
    pub pads: Vec<PadSpec>,
    /// Replace every pad the footprint has with these.
    pub replace: Option<bool>,
}

/// `ecad.pad.edit`: change the fields given of one pad.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct EditParams {
    pub component: ComponentRef,
    /// The pad to change, by number.
    pub pad: Number,
    /// A new number, unique in the footprint.
    pub number: Option<Number>,
    pub at: Option<[f64; 2]>,
    pub size: Option<[f64; 2]>,
    pub rotation: Option<f64>,
    pub side: Option<Side>,
    pub shape: Option<Shape>,
    pub corner: Option<f64>,
    pub kind: Option<Kind>,
    /// A drill; a `kind` of `smd` drops it.
    pub drill: Option<DrillSpec>,
}

/// `ecad.pad.delete`: delete pads by number, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DeleteParams {
    pub component: ComponentRef,
    pub pads: Vec<Number>,
}

/// What `ecad.pad.add` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AddResult {
    pub component: ComponentId,
    /// How many pads the call added.
    pub added: usize,
    /// How many pads the footprint has now.
    pub pads: usize,
    /// With `replace`: whether a pin-to-pad map that named pads no longer there was dropped.
    pub pin_map_cleared: bool,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.pad.edit` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct EditResult {
    pub component: ComponentId,
    /// The pad's number after the edit.
    pub pad: String,
    /// The fields that changed; empty when nothing did (then there is no undo step).
    pub changed: Vec<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.pad.delete` returns.
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
        assert_keys(input_schema_for::<AddParams>(), &["component", "pads", "replace"]);
        assert_keys(
            input_schema_for::<EditParams>(),
            &["at", "component", "corner", "drill", "kind", "number", "pad", "rotation", "shape", "side", "size"],
        );
        assert_keys(input_schema_for::<DeleteParams>(), &["component", "pads"]);
    }
}
