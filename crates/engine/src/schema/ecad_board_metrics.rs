//! Contracts and result types for the commands in `cmd/ecad_board_metrics.rs`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A metric `ecad.board.net.inspect` works out beyond the cheap ones (length by layer, delay,
/// vias, bends and the pair's skew always come).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MetricParam {
    Length,
    Layers,
    Vias,
    Bends,
    Delay,
    /// Each segment's reference plane, its coverage and the dielectric height.
    Reference,
    /// Each segment's impedance.
    Impedance,
    /// The neighbours on the same layer and how far they run alongside.
    Coupling,
    /// List the segments.
    Segments,
    Pair,
}

/// `ecad.board.net.inspect`: the numbers of nets' copper. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct NetInspectParams {
    /// These nets; or a net class's (`class`), or a pair's (`pair`); left out, every net with
    /// copper.
    pub nets: Option<Vec<String>>,
    pub class: Option<String>,
    /// A pair of the intent, by name.
    pub pair: Option<String>,
    pub metrics: Option<Vec<MetricParam>>,
    /// The most nets (25 when left out, at most 256), after `offset` of them.
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BendsOut {
    #[serde(rename = "45")]
    pub deg45: usize,
    #[serde(rename = "90")]
    pub deg90: usize,
    /// Sharper than a right angle.
    pub acute: usize,
    pub other: usize,
    /// Straight segments longer than 0.5 mm off the 0/45/90° directions.
    pub any_angle: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SegmentOut {
    pub track: u64,
    pub layer: String,
    pub width: f64,
    pub length_mm: f64,
    pub delay_ps: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub z0: Option<f64>,
    /// The layer of the plane it references, the plane's net and the share of it over the plane.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_net: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_coverage: Option<f64>,
    /// The dielectric to the plane, in millimetres.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h: Option<f64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NeighbourOut {
    pub net: String,
    pub layer: String,
    pub coupled_mm: f64,
    /// Edge to edge.
    pub min_gap_mm: f64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PairOut {
    pub name: String,
    pub partner: String,
    pub skew_mm: f64,
    pub skew_ps: f64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NetOut {
    pub net: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// T0–T3, left out while unverified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// The tier and where it comes from.
    pub regime: String,
    pub length_mm: f64,
    pub by_layer: BTreeMap<String, f64>,
    pub delay_ps: f64,
    pub vias: usize,
    pub bends: BendsOut,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<SegmentOut>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub segments_more: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub neighbours: Vec<NeighbourOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pair: Option<PairOut>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// What `ecad.board.net.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NetInspectResult {
    pub nets: Vec<NetOut>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// A kind of current loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoopKindParam {
    Decap,
    /// A regulator's hot loops: buck_hot and boost_hot.
    Hot,
    BuckHot,
    BoostHot,
    Gate,
    Crystal,
    Tvs,
    Sense,
}

/// `ecad.board.loop.inspect`: the current loops, ranked. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct LoopInspectParams {
    /// These loops by name (the intent's, and `C3→U1.VDD` for a decoupling loop).
    pub loops: Option<Vec<String>>,
    pub kind: Option<LoopKindParam>,
    /// Loops with a member on this part.
    pub part: Option<String>,
    /// The most loops (25 when left out, at most 512), after `offset`.
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MemberOut {
    pub pin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LoopOut {
    pub name: String,
    pub kind: String,
    /// `intent` (a loop record) or `decoupling` (from a decoupling capacitor's record).
    pub from: String,
    pub members: Vec<MemberOut>,
    /// The members' pads in order (the loop closes back to the first).
    pub path: Vec<[f64; 2]>,
    pub length_mm: f64,
    pub area_mm2: f64,
    pub vias: usize,
    pub layers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_coverage: Option<f64>,
    /// The share of the members' pads' hull over a ground plane on the next layer: what
    /// `pi.smps_hot_loop` judges.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ground_under: Option<f64>,
    /// μ0·h·(perimeter/2)/w: to rank loops and see a change help, not a measurement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub est_nh: Option<f64>,
    /// 1 for the largest estimate.
    pub rank: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
}

/// What `ecad.board.loop.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LoopInspectResult {
    /// Best rank first.
    pub loops: Vec<LoopOut>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
