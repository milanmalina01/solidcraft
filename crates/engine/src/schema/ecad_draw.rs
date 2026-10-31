//! Contracts and result types for the commands in `cmd/ecad_draw.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_component::{ComponentId, ComponentRef, Layer, Side};

/// `ecad.draw.line`: a line through `points`, on a library symbol, or on a library footprint's
/// `layer`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct LineParams {
    /// The symbol to draw on (or give `footprint`).
    pub symbol: Option<ComponentRef>,
    /// The footprint to draw on, with its `layer`.
    pub footprint: Option<ComponentRef>,
    /// The footprint layer.
    pub layer: Option<Layer>,
    /// The footprint side, `top` when left out.
    pub side: Option<Side>,
    /// Two or more points `[x, y]` in millimetres.
    pub points: Vec<[f64; 2]>,
    /// Line width in millimetres, 0.15 when left out.
    pub width: Option<f64>,
}

/// `ecad.draw.rect`: the rectangle with corners `from` and `to`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RectParams {
    pub symbol: Option<ComponentRef>,
    pub footprint: Option<ComponentRef>,
    pub layer: Option<Layer>,
    pub side: Option<Side>,
    pub from: [f64; 2],
    pub to: [f64; 2],
    pub width: Option<f64>,
    pub fill: Option<bool>,
}

/// `ecad.draw.circle`: a circle by its `center` and `radius`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CircleParams {
    pub symbol: Option<ComponentRef>,
    pub footprint: Option<ComponentRef>,
    pub layer: Option<Layer>,
    pub side: Option<Side>,
    pub center: [f64; 2],
    pub radius: f64,
    pub width: Option<f64>,
    pub fill: Option<bool>,
}

/// `ecad.draw.text`: `text` from `at`, `height` tall. `{designator}` and `{value}` stand for the
/// placed part's.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct TextParams {
    pub symbol: Option<ComponentRef>,
    pub footprint: Option<ComponentRef>,
    pub layer: Option<Layer>,
    pub side: Option<Side>,
    pub at: [f64; 2],
    pub text: String,
    pub height: f64,
    /// Degrees counter-clockwise.
    pub rotation: Option<f64>,
}

/// What the draw commands return: the symbol or the footprint drawn on.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DrawResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<ComponentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footprint: Option<ComponentId>,
    /// How many graphics the symbol, or the footprint layer and side, has now.
    pub count: usize,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
