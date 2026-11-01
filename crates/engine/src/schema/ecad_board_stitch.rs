//! Contracts and result types for the commands in `cmd/ecad_board_stitch.rs`: stitching vias,
//! placed in a grid, a fence or at layer changes, and taken away again.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::ecad_board::{BoardResult, CopperLayerParam, ViaProtectionParam};
use super::ecad_schematic::ItemRef;

/// How the vias are set out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StitchPattern {
    /// Staggered rows over an area.
    Grid,
    /// Rows along both sides of tracks.
    Fence,
    /// A via beside each signal via that changes reference plane.
    LayerChange,
    /// A via at a time where the net's planes are most weakly tied, solved again after each.
    Needed,
}

/// The word `board`: the whole board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BoardWord {
    Board,
}

/// An area by a pour's id, a rectangle's two corners, or a polygon's corners: one of them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct RegionArea {
    /// A pour's fill, by the pour's id.
    pub pour: Option<u64>,
    /// Two opposite corners, in millimetres.
    pub rect: Option<[[f64; 2]; 2]>,
    /// Three corners or more, in millimetres.
    pub polygon: Option<Vec<[f64; 2]>>,
}

/// Where stitching vias go, or which ones a removal takes: `"board"`, or `{pour}`, `{rect}` or
/// `{polygon}`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum RegionParam {
    Board(BoardWord),
    Area(RegionArea),
}

impl<'de> Deserialize<'de> for RegionParam {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match Value::deserialize(d)? {
            Value::String(s) if s == "board" => Ok(RegionParam::Board(BoardWord::Board)),
            v @ Value::Object(_) => serde_json::from_value(v).map(RegionParam::Area).map_err(D::Error::custom),
            _ => Err(D::Error::custom("must be \"board\" or one of {pour}, {rect} or {polygon}")),
        }
    }
}

/// The vias' size and span; each left out is the net class's via, through the board (the rules'
/// micro via between two neighbouring layers).
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct StitchViaSpec {
    /// In millimetres.
    pub diameter: Option<f64>,
    pub drill: Option<f64>,
    /// The top layer of the span.
    pub from: Option<CopperLayerParam>,
    /// The bottom layer of the span.
    pub to: Option<CopperLayerParam>,
}

/// `ecad.board.stitch`: stitching vias of one net, in one step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct StitchParams {
    /// The net the vias are on, by name (GND as a rule).
    pub net: String,
    pub pattern: StitchPattern,
    /// Where they may go; the whole board when left out.
    pub region: Option<RegionParam>,
    /// A fence's tracks, by id.
    pub tracks: Option<Vec<u64>>,
    /// A fence's nets (their tracks), or the signal nets a layer change looks at, by name.
    pub nets: Option<Vec<String>>,
    /// The nets of a net class, by name, as `nets`.
    pub class: Option<String>,
    /// How far apart neighbouring vias are, in millimetres; from the intent's frequencies when
    /// left out.
    pub pitch: Option<f64>,
    /// A fence's distance from each track's centreline, in millimetres; left out, half the width,
    /// the clearance between the two nets and the via's radius.
    pub offset: Option<f64>,
    pub via: Option<StitchViaSpec>,
    /// The least gap kept to other nets' copper, in millimetres; never less than the rules'.
    pub clearance: Option<f64>,
    /// What fills or covers the holes; tented when left out.
    pub protection: Option<ViaProtectionParam>,
    /// Work it out and say where the vias would go, changing nothing.
    pub dry_run: Option<bool>,
    /// A grid's or fence's vias may stand under parts (inside the courtyard of a footprint on a
    /// side the via shows on); left out, they keep out.
    pub under_parts: Option<bool>,
    /// Needed stitching: how far the planes' most weakly tied spot may rise, in millivolts when
    /// every mm² of the net's copper draws 10 mA (0.5 when left out).
    pub target: Option<f64>,
    /// Needed stitching: the most vias it places (20 when left out, at most 200).
    pub budget: Option<usize>,
}

/// How weakly a ground net's planes are tied (see `ecad.board.ground.inspect`).
#[derive(Debug, Serialize, JsonSchema)]
pub struct TieOut {
    /// The most weakly tied plane spot, in millivolts; null with no plane.
    pub worst_mv: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// The share of the net's copper joined to the main ground by nothing, and of it past the
    /// target, in percent.
    pub isolated_pct: f64,
    pub weak_pct: f64,
    /// The solve finished.
    pub converged: bool,
}

/// What needed stitching did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NeededOut {
    pub before: TieOut,
    pub after: TieOut,
    pub target_mv: f64,
    pub budget: usize,
    /// The worst plane spot is within the target.
    pub reached: bool,
    pub solves: usize,
    /// The grid's cell in millimetres, and whether the board was too large for the usual one.
    pub cell: f64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub coarse: bool,
}

/// The vias' size and span as placed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StitchViaOut {
    pub diameter: f64,
    pub drill: f64,
    pub from: String,
    pub to: String,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// What `ecad.board.stitch` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StitchResult {
    /// How many vias it placed (or would place, on a dry run).
    pub count: usize,
    /// On a dry run: where they would go.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub would_create: Vec<[f64; 2]>,
    /// The spots it left out, by why: `other_copper`, `hole_to_hole`, `board_edge`, `keepout`,
    /// `in_pad`, `no_copper` (the net's copper on fewer than two layers there), `near_via` (a via
    /// of the net near enough already), `splits_plane` (a via there would cut another net's
    /// plane in two), `outside` (the region), `no_room` (no spot fits: a fence's within 1 mm of a
    /// stop or a quarter pitch of its spot, a layer change's within reach),
    /// `other_reference` (a layer change between planes of another net: a stitching capacitor's
    /// job) and `short_span` (the vias don't reach both planes).
    pub skipped: BTreeMap<String, usize>,
    /// How many spots it looked at.
    pub looked_at: usize,
    /// A grid's vias placed a little off their spot (up to 0.16 pitch), where the spot itself
    /// failed.
    #[serde(skip_serializing_if = "is_zero")]
    pub moved: usize,
    pub pitch_used: Option<f64>,
    /// A fence's offset, when one served every track.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_used: Option<f64>,
    pub via: StitchViaOut,
    /// Why: the pitch and where it comes from, the pattern, what a layer change found.
    pub why: Vec<String>,
    /// A fence's tracks that got no via, by id.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unfenced: Vec<u64>,
    /// Needed stitching: how weakly tied the planes were before, and are after.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ground: Option<NeededOut>,
    #[serde(flatten)]
    pub board: BoardResult,
}

/// `ecad.board.stitch.remove`: take stitching vias away, in one step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct StitchRemoveParams {
    /// Only those on this net.
    pub net: Option<String>,
    /// Only those in this area.
    pub region: Option<RegionParam>,
    /// Only these vias, by id (each must be a stitching via).
    pub ids: Option<Vec<u64>>,
    /// Only those under these parts: footprints by their part's designator (or by id), a via
    /// under one when its pad meets the footprint's courtyard (else its box) on a side the via
    /// shows on, as a grid or a fence keeps out of.
    pub parts: Option<Vec<ItemRef>>,
    /// Only those of this pattern. A via stitched before vias kept their pattern has none on
    /// record, and a pattern takes it only with `unrecorded`.
    pub pattern: Option<StitchPattern>,
    /// With `pattern`: those with no pattern on record too (they may be of any pattern).
    pub unrecorded: Option<bool>,
}

/// What `ecad.board.stitch.remove` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StitchRemoveResult {
    pub count: usize,
    /// Stitching vias that match all else given but have no pattern on record, left by a
    /// `pattern` without `unrecorded`.
    #[serde(skip_serializing_if = "is_zero")]
    pub unrecorded: usize,
    #[serde(flatten)]
    pub board: BoardResult,
}
