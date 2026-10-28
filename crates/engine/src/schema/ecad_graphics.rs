//! Contracts and result types for the commands in `cmd/ecad_graphics.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_component::{ComponentId, ComponentRef, CourtyardOut, GraphicSpec, Layer, Side};

/// `ecad.symbol.set_graphics`: the symbol's body, replacing what it had.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SymbolParams {
    pub component: ComponentRef,
    /// Empty clears the body.
    pub graphics: Vec<GraphicSpec>,
}

/// `ecad.footprint.set_graphics`: one board layer's graphics on one side, replacing what it had.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FootprintParams {
    pub component: ComponentRef,
    pub layer: Layer,
    /// `top` when left out.
    pub side: Option<Side>,
    /// Empty clears the layer.
    pub graphics: Vec<GraphicSpec>,
}

/// `ecad.footprint.courtyard`: a rectangle `margin` outside the pads and the fabrication outline,
/// replacing the courtyard on that side.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CourtyardParams {
    pub component: ComponentRef,
    /// In millimetres, 0 to 5; 0.25 when left out.
    pub margin: Option<f64>,
    /// `top` when left out.
    pub side: Option<Side>,
}

/// What `ecad.symbol.set_graphics` and `ecad.footprint.set_graphics` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GraphicsResult {
    pub component: ComponentId,
    /// How many graphics the symbol, or the layer, has now.
    pub count: usize,
    /// The box around them, `[[x0, y0], [x1, y1]]`; null when there are none.
    pub bbox: Option<[[f64; 2]; 2]>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.footprint.courtyard` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CourtyardResult {
    pub component: ComponentId,
    pub courtyard: CourtyardOut,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{CourtyardParams, FootprintParams, SymbolParams};
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn schemas_and_the_key_check_agree() {
        assert_keys(input_schema_for::<SymbolParams>(), &["component", "graphics"]);
        assert_keys(input_schema_for::<FootprintParams>(), &["component", "graphics", "layer", "side"]);
        assert_keys(input_schema_for::<CourtyardParams>(), &["component", "margin", "side"]);
    }
}
