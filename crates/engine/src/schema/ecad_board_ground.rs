//! Contracts and result types for the command in `cmd/ecad_board_ground.rs`: how well a ground
//! net's copper is tied to its main ground.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_board::CopperLayerParam;

/// `ecad.board.ground.inspect`: the ground connectivity map. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct GroundInspectParams {
    /// The net, by name; left out, the intent's ground, else a net named as one (GND).
    pub net: Option<String>,
    /// Only these layers in the reply (the solve always takes every layer).
    pub layers: Option<Vec<CopperLayerParam>>,
    /// The grid's cell, 0.1 to 0.25 mm (0.25 when left out; wider on a board too large for it).
    pub cell: Option<f64>,
    /// What counts as weakly tied, in millivolts (0.5 when left out).
    pub target: Option<f64>,
    /// The most worst spots listed (8 when left out, at most 64).
    pub spots: Option<usize>,
    /// The coarse map's most cells each way (32 when left out, 4 to 128); 0 leaves the map out.
    pub map: Option<usize>,
}

/// A weakly tied spot.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GroundSpotOut {
    pub at: [f64; 2],
    pub layer: String,
    /// Millivolts it rises when every mm² of the net's copper draws 10 mA into the main ground.
    pub mv: f64,
}

/// One layer's copper of the net.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GroundLayerOut {
    pub layer: String,
    pub copper_mm2: f64,
    /// Shares of the layer's copper of the net, in percent: tied to the main ground, joined to
    /// it by nothing, and tied but past the target.
    pub tied_pct: f64,
    pub isolated_pct: f64,
    pub weak_pct: f64,
    /// The coarse map: rows from the bottom (lowest y) up, each cell the worst millivolts of the
    /// grid cells it holds; null where there is no copper of the net, -1 where it is isolated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map: Option<Vec<Vec<Option<f64>>>>,
}

/// What the potentials are measured from.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GroundReferenceOut {
    /// `entry` (a power-entry connector's ground pads) or `plane` (the largest shape of the
    /// net's pour fills).
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pour: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_mm2: Option<f64>,
}

/// What `ecad.board.ground.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GroundInspectResult {
    pub net: String,
    pub reference: GroundReferenceOut,
    /// The grid's cell, and whether the board needed a wider one than asked.
    pub cell: f64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub coarse: bool,
    /// The coarse map's lower left corner and its cells' side, in millimetres.
    pub origin: [f64; 2],
    pub map_cell: f64,
    pub target_mv: f64,
    /// The planes' most weakly tied spots, worst first, at least 1 mm apart.
    pub worst: Vec<GroundSpotOut>,
    /// The worst over all the net's copper (tracks and pads too), when it is worse than the
    /// planes'.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst_copper: Option<GroundSpotOut>,
    pub layers: Vec<GroundLayerOut>,
    pub converged: bool,
    pub iterations: usize,
    pub unknowns: usize,
}
