//! Contracts and result types for the commands in `cmd/ecad_board_pour.rs`: copper pours,
//! keepouts and via edits.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use super::ecad_board::{BoardResult, CircleSpec, CopperLayerParam};
use super::ecad_schematic::nullable;

/// How a pour joins the pads of its net.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectParam {
    /// A gap round each pad, crossed by spokes.
    Thermal,
    /// Straight into the pad.
    Solid,
    /// Not at all: the pads are cleared like another net's.
    None,
}

/// A thermal connection: each value left out stays (0.3 mm spokes, a 0.3 mm gap, 4 spokes on a
/// new pour).
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ThermalSpec {
    /// In millimetres.
    pub spoke_width: Option<f64>,
    /// Between the pad and the pour, in millimetres.
    pub gap: Option<f64>,
    /// 2 (along the pad's width) or 4.
    pub spokes: Option<u8>,
}

/// A hatched fill: lines `width` wide every `pitch`, both ways, in millimetres.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct HatchSpec {
    pub width: f64,
    pub pitch: f64,
}

/// `ecad.board.pour`: a copper pour on one layer.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PourParams {
    pub layer: CopperLayerParam,
    /// The schematic net it fills with, by name; left out, a pour on no net (it joins nothing).
    pub net: Option<String>,
    /// The outline's corners in order, closed back to the first (three or more, two with an
    /// arc); in place of `rect` and `circle`.
    pub points: Option<Vec<[f64; 2]>>,
    /// With `points`: each edge's sweep in degrees (counter-clockwise when positive), the closing
    /// edge last.
    pub curves: Option<Vec<f64>>,
    /// A rectangle, width and height in millimetres, centred on `center`.
    pub rect: Option<[f64; 2]>,
    /// The rectangle's middle; the origin when left out.
    pub center: Option<[f64; 2]>,
    pub circle: Option<CircleSpec>,
    /// The gap to other nets' copper, in millimetres; left out, the net class's (or the rules')
    /// clearance. The larger of it and the other copper's class clearance applies.
    pub clearance: Option<f64>,
    /// The thinnest copper the fill keeps, in millimetres; 0.2 when left out.
    pub min_width: Option<f64>,
    /// thermal when left out.
    pub connect: Option<ConnectParam>,
    pub thermal: Option<ThermalSpec>,
    /// Higher fills first where pours on a layer overlap; 0 when left out.
    pub priority: Option<u32>,
    /// Fill islands that touch no copper of the net go; true when left out.
    pub remove_islands: Option<bool>,
    /// A hatched fill in place of solid copper.
    pub hatch: Option<HatchSpec>,
}

/// `ecad.board.pour.edit`: change a pour; what is left out stays.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PourEditParams {
    /// The pour, by id.
    pub pour: u64,
    pub layer: Option<CopperLayerParam>,
    /// `null` takes the pour off its net.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<String>")]
    pub net: Option<Option<String>>,
    pub points: Option<Vec<[f64; 2]>>,
    pub curves: Option<Vec<f64>>,
    pub rect: Option<[f64; 2]>,
    pub center: Option<[f64; 2]>,
    pub circle: Option<CircleSpec>,
    /// `null` goes back to the net class's clearance.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<f64>")]
    pub clearance: Option<Option<f64>>,
    pub min_width: Option<f64>,
    pub connect: Option<ConnectParam>,
    pub thermal: Option<ThermalSpec>,
    pub priority: Option<u32>,
    pub remove_islands: Option<bool>,
    /// `null` makes the fill solid.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<HatchSpec>")]
    pub hatch: Option<Option<HatchSpec>>,
}

/// `ecad.board.pour.inspect`: pours' fills. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PourInspectParams {
    /// One pour, by id; left out, every pour.
    pub pour: Option<u64>,
    /// List the fill's polygons; true when left out.
    pub polygons: Option<bool>,
}

/// A thermal connection as replies give it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ThermalOut {
    pub spoke_width: f64,
    pub gap: f64,
    pub spokes: u8,
}

/// What a pour's fill comes to.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FillSummary {
    /// How many separate pieces of copper.
    pub polygons: usize,
    /// In square millimetres.
    pub area: f64,
    /// Islands that went for touching no copper of the net.
    pub islands_removed: usize,
    /// The pour was past the fill's budget, so it is unfilled or filled solid.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// A copper pour.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PourOut {
    pub id: u64,
    pub layer: String,
    pub net: Option<String>,
    /// The schematic has the net (false for a name it has no net for).
    pub net_found: bool,
    pub outline: Vec<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<f64>,
    /// Its own clearance; null for the net class's.
    pub clearance: Option<f64>,
    /// The clearance it keeps.
    pub effective_clearance: f64,
    pub min_width: f64,
    pub connect: ConnectParam,
    pub thermal: ThermalOut,
    pub priority: u32,
    pub remove_islands: bool,
    pub hatch: Option<HatchSpec>,
    pub fill: FillSummary,
}

/// A fill's piece of copper: its outline (counter-clockwise) and its holes (clockwise).
#[derive(Debug, Serialize, JsonSchema)]
pub struct PolygonOut {
    pub outline: Vec<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<Vec<[f64; 2]>>,
}

/// One pour's fill in full.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FillOut {
    pub pour: u64,
    pub layer: String,
    pub net: Option<String>,
    #[serde(flatten)]
    pub summary: FillSummary,
    /// Left out with `polygons: false`, or past the reply's corner budget (`truncated`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapes: Vec<PolygonOut>,
}

/// What `ecad.board.pour.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PourInspectResult {
    pub fills: Vec<FillOut>,
    /// A fill was past its budget, or the polygons past the reply's corner budget.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.board.pour` and `ecad.board.pour.edit` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PourResult {
    pub pour: PourOut,
    #[serde(flatten)]
    pub board: BoardResult,
}

/// What a keepout forbids; each left out is forbidden (or stays, on an edit).
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ForbidSpec {
    pub tracks: Option<bool>,
    pub vias: Option<bool>,
    pub pours: Option<bool>,
    /// Footprints, by their courtyard.
    pub components: Option<bool>,
}

/// `ecad.board.keepout`: an area kept clear.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct KeepoutParams {
    /// The copper layers it keeps clear; left out or `[]`, every layer.
    pub layers: Option<Vec<CopperLayerParam>>,
    pub points: Option<Vec<[f64; 2]>>,
    pub curves: Option<Vec<f64>>,
    pub rect: Option<[f64; 2]>,
    pub center: Option<[f64; 2]>,
    pub circle: Option<CircleSpec>,
    pub forbid: Option<ForbidSpec>,
}

/// `ecad.board.keepout.edit`: change a keepout; what is left out stays.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct KeepoutEditParams {
    /// The keepout, by id.
    pub keepout: u64,
    /// `[]`: every layer.
    pub layers: Option<Vec<CopperLayerParam>>,
    pub points: Option<Vec<[f64; 2]>>,
    pub curves: Option<Vec<f64>>,
    pub rect: Option<[f64; 2]>,
    pub center: Option<[f64; 2]>,
    pub circle: Option<CircleSpec>,
    pub forbid: Option<ForbidSpec>,
}

/// What a keepout forbids, as replies give it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ForbidOut {
    pub tracks: bool,
    pub vias: bool,
    pub pours: bool,
    pub components: bool,
}

/// A keepout.
#[derive(Debug, Serialize, JsonSchema)]
pub struct KeepoutOut {
    pub id: u64,
    /// The copper layers it keeps clear; every layer when `all_layers`.
    pub layers: Vec<String>,
    pub all_layers: bool,
    pub outline: Vec<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<f64>,
    pub forbid: ForbidOut,
}

/// What `ecad.board.keepout` and `ecad.board.keepout.edit` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct KeepoutResult {
    pub keepout: KeepoutOut,
    #[serde(flatten)]
    pub board: BoardResult,
}

/// `ecad.board.via.edit`: change vias' span, size or own net, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ViaEditParams {
    /// The vias, by id.
    pub vias: Vec<u64>,
    /// The top layer of the span.
    pub from: Option<CopperLayerParam>,
    /// The bottom layer of the span.
    pub to: Option<CopperLayerParam>,
    /// In millimetres.
    pub diameter: Option<f64>,
    pub drill: Option<f64>,
    /// A net of its own, by name (a stitching via); `null` takes it away.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<String>")]
    pub net: Option<Option<String>>,
    /// What fills or covers the hole.
    pub protection: Option<super::ecad_board::ViaProtectionParam>,
}

/// `ecad.board.pour.connect`: how pours of their nets join a footprint's pads, over each pour's
/// own `connect`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PourConnectParams {
    /// By its part's designator, or by id.
    pub footprint: super::ecad_schematic::ItemRef,
    /// The pads, by number; left out, the footprint as a whole (a pad's own setting goes over it).
    pub pads: Option<Vec<String>>,
    /// `null`: back to the pour's own (for pads, the footprint's when it has one). Always sent.
    #[serde(deserialize_with = "nullable")]
    #[schemars(with = "ConnectOrNull")]
    pub connect: Option<Option<ConnectParam>>,
}

/// The schema of a `connect` that must be sent but may be null (a plain `Option` would make it one
/// that may be left out).
enum ConnectOrNull {}

impl JsonSchema for ConnectOrNull {
    fn schema_name() -> Cow<'static, str> {
        "ConnectOrNull".into()
    }
    fn inline_schema() -> bool {
        true
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "thermal, solid or none; null for the pour's own (for pads, the footprint's when it has one).",
            "enum": ["thermal", "solid", "none", null],
        })
    }
}
