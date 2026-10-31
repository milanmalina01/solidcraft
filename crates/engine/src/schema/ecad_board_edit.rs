//! Contracts and result types for the commands in `cmd/ecad_board_edit.rs` and
//! `cmd/ecad_board_draw.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_board::{BoardResult, DrawLayerParam, StackLayerOut};
use super::ecad_schematic::{ItemRef, nullable};

/// What a stack-up layer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StackKindParam {
    Copper,
    /// A cured dielectric.
    Core,
    /// The dielectric bonding cores and outer copper.
    Prepreg,
    /// Solder mask, outside the outer copper.
    Mask,
}

/// One layer of the stack-up, top first.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct StackLayerSpec {
    pub kind: StackKindParam,
    /// In millimetres: copper 0.005 to 0.5, a dielectric 0.01 to 5, mask 0.001 to 0.2.
    pub thickness: f64,
    /// Relative permittivity of a dielectric (4.3 when left out) or the mask (3.5); none for
    /// copper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub er: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_tangent: Option<f64>,
}

/// `ecad.board.stack`: the copper layer count, the board's thickness and its stack-up.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct StackParams {
    /// Even, 2 to 16; left out, it stays.
    pub copper_layers: Option<u32>,
    /// In millimetres; left out, it stays (or follows a given `stackup`).
    pub thickness: Option<f64>,
    /// What the board is made of, top to bottom: optional mask, then copper and dielectrics in
    /// turn, one copper layer each of `copper_layers`; `null` goes back to the default stack-up.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<Vec<StackLayerSpec>>")]
    pub stackup: Option<Option<Vec<StackLayerSpec>>>,
}

/// Where the board's thickness came from on a call that gave a stack-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThicknessFrom {
    /// The `thickness` given, which the stack-up is within 10 % of.
    Given,
    /// The stack-up's total.
    Stackup,
}

/// What `ecad.board.stack` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StackResult {
    pub copper_layers: u8,
    pub thickness: f64,
    /// The copper layers, top to bottom.
    pub stack: Vec<StackLayerOut>,
    /// What the board is made of, top to bottom.
    pub stackup: Vec<StackLayerSpec>,
    /// Its total thickness.
    pub stackup_thickness: f64,
    pub custom_stackup: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thickness_from: Option<ThicknessFrom>,
    /// The copper layers changed, so the board's own stack-up went back to the default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stackup_reset: bool,
    /// The thickness changed with a stack-up of the board's own: its dielectrics were scaled.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dielectrics_scaled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// `ecad.board.hole`: an unplated mounting hole.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct HoleParams {
    pub at: [f64; 2],
    /// In millimetres; 3.2 when left out.
    pub drill: Option<f64>,
}

/// One footprint's name and value texts.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct FootprintEdit {
    /// By its part's designator, or by id.
    pub footprint: ItemRef,
    /// Where the name text goes, from the footprint's `at` along the board's axes; `null` is the
    /// footprint's own spot.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<[f64; 2]>")]
    pub name_at: Option<Option<[f64; 2]>>,
    /// Where the value text goes, as `name_at`.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<[f64; 2]>")]
    pub value_at: Option<Option<[f64; 2]>>,
    pub show_name: Option<bool>,
    pub show_value: Option<bool>,
}

/// `ecad.board.edit`: change footprints' name and value texts, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct EditParams {
    pub footprints: Vec<FootprintEdit>,
}

/// `ecad.board.names`: the footprints' name and value texts across the board, all in one step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct NamesParams {
    /// Every name's and value's height, in millimetres; `null` sizes each to its footprint.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<f64>")]
    pub height: Option<Option<f64>>,
    /// Every footprint's name shown, or every one hidden.
    pub show_names: Option<bool>,
    /// Every footprint's value shown, or every one hidden.
    pub show_values: Option<bool>,
}

/// `ecad.board.track.reshape`: a track's new points and sweeps.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ReshapeParams {
    pub track: u64,
    /// Two or more, in millimetres.
    pub points: Vec<[f64; 2]>,
    /// Each segment's sweep in degrees; left out, all straight.
    pub curves: Option<Vec<f64>>,
    /// Refuse the change when it brings the track closer to another net's copper than the
    /// clearance, where it wasn't before.
    pub check: Option<bool>,
}

/// `ecad.board.track.split`: a new corner on a segment.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SplitParams {
    pub track: u64,
    /// Segment 0 runs from the first point to the second.
    pub segment: usize,
    /// Taken to the nearest point of the segment.
    pub at: [f64; 2],
}

/// What `ecad.board.track.split` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SplitResult {
    /// The new corner's index in the track's points.
    pub corner: usize,
    /// Where it is.
    pub at: [f64; 2],
    #[serde(flatten)]
    pub board: BoardResult,
}

/// How a mitered corner is cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MiterStyle {
    /// An arc tangent to both segments.
    Round,
    /// A straight chamfer.
    Straight,
}

/// `ecad.board.track.miter`: cut a track's corners.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct MiterParams {
    pub track: u64,
    /// The corner, by its point's index (1 to the point count − 2); left out, every corner.
    pub corner: Option<usize>,
    /// In millimetres: the round cut's radius, or the radius whose touch points the straight cut
    /// joins.
    pub radius: f64,
    /// `round` when left out.
    pub style: Option<MiterStyle>,
}

/// What `ecad.board.track.miter` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MiterResult {
    /// How many corners were cut; corners next to an arc, nearly straight or with no room are left.
    pub mitered: usize,
    #[serde(flatten)]
    pub board: BoardResult,
}

/// `ecad.board.unroute`: take routed copper away, so the airwires come back.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct UnrouteParams {
    /// Tracks and vias by id: each with the whole run joined to it through tracks and vias (not
    /// through a pad).
    pub items: Option<Vec<u64>>,
    /// Every track and via of these nets, by name.
    pub nets: Option<Vec<String>>,
    /// Every track and via on the board.
    pub all: Option<bool>,
}

/// `ecad.board.draw.line`: a line through `points` on a board layer.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct LineParams {
    pub layer: DrawLayerParam,
    /// Two or more points `[x, y]` in millimetres.
    pub points: Vec<[f64; 2]>,
    /// In millimetres; 0.15 when left out.
    pub width: Option<f64>,
}

/// `ecad.board.draw.rect`: the rectangle with corners `from` and `to`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RectParams {
    pub layer: DrawLayerParam,
    pub from: [f64; 2],
    pub to: [f64; 2],
    pub width: Option<f64>,
    pub fill: Option<bool>,
}

/// `ecad.board.draw.circle`: a circle by its `center` and `radius`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CircleParams {
    pub layer: DrawLayerParam,
    pub center: [f64; 2],
    pub radius: f64,
    pub width: Option<f64>,
    pub fill: Option<bool>,
}

/// `ecad.board.draw.text`: `text` from `at`, `height` tall.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct TextParams {
    pub layer: DrawLayerParam,
    /// Its lower left before the turn, in millimetres.
    pub at: [f64; 2],
    pub text: String,
    /// In millimetres.
    pub height: f64,
    /// Degrees counter-clockwise.
    pub rotation: Option<f64>,
    /// Drawn mirrored, to read from the bottom; left out, mirrored on the bottom layers.
    pub mirror: Option<bool>,
}

/// `ecad.board.hole.edit`: change mounting holes' drill.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct HoleEditParams {
    /// The holes, by id.
    pub holes: Vec<u64>,
    /// In millimetres.
    pub drill: f64,
}

/// `ecad.board.draw.edit`: change one board drawing; what is left out stays.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DrawEditParams {
    /// The drawing, by id.
    pub graphic: u64,
    pub layer: Option<DrawLayerParam>,
    /// Text only.
    pub text: Option<String>,
    /// Text only, in millimetres.
    pub height: Option<f64>,
    /// Lines, rectangles and circles, in millimetres.
    pub width: Option<f64>,
    /// Rectangles and circles.
    pub fill: Option<bool>,
    /// Text only: drawn mirrored; left out, a text moved to another layer is mirrored on the
    /// bottom layers.
    pub mirror: Option<bool>,
}
