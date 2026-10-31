//! Contracts and result types for the commands in `cmd/ecad_board.rs`.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use solidcraft_doc::ecad::ViaProtection;
use solidcraft_doc::ecad::layers::{CopperLayer, DrawLayer, MAX_COPPER_LAYERS, stack};

use super::ecad_board_edit::StackLayerSpec;
use super::ecad_board_pour::{KeepoutOut, PourOut};
use super::ecad_board_rules::{NetClassOut, RulesOut};
use super::ecad_component::{GraphicSpec, Side};
use super::ecad_schematic::ItemRef;

/// What fills or covers a via's hole: `open`, `tented` (mask over it), `plugged`, `filled`, or
/// `filled_capped` (plated over: the only kind that may sit in a pad).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViaProtectionParam {
    Open,
    Tented,
    Plugged,
    Filled,
    FilledCapped,
}

impl ViaProtectionParam {
    pub fn doc(self) -> ViaProtection {
        match self {
            ViaProtectionParam::Open => ViaProtection::Open,
            ViaProtectionParam::Tented => ViaProtection::Tented,
            ViaProtectionParam::Plugged => ViaProtection::Plugged,
            ViaProtectionParam::Filled => ViaProtection::Filled,
            ViaProtectionParam::FilledCapped => ViaProtection::FilledCapped,
        }
    }

    pub fn of(p: ViaProtection) -> Self {
        match p {
            ViaProtection::Open => ViaProtectionParam::Open,
            ViaProtection::Tented => ViaProtectionParam::Tented,
            ViaProtection::Plugged => ViaProtectionParam::Plugged,
            ViaProtection::Filled => ViaProtectionParam::Filled,
            ViaProtection::FilledCapped => ViaProtectionParam::FilledCapped,
        }
    }

    fn is_open(&self) -> bool {
        *self == ViaProtectionParam::Open
    }
}

/// Every copper layer's name, top to bottom, as the largest stack has them.
pub(super) fn copper_names() -> Vec<String> {
    stack(MAX_COPPER_LAYERS).into_iter().map(CopperLayer::name).collect()
}

/// A copper layer by name: `top`, `inner1` … `inner14`, `bottom`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CopperLayerParam(pub CopperLayer);

impl<'de> Deserialize<'de> for CopperLayerParam {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::String(s) => CopperLayer::try_from(s).map(CopperLayerParam).map_err(D::Error::custom),
            _ => Err(D::Error::custom("must be a copper layer: top, inner1 … inner14, bottom")),
        }
    }
}

impl JsonSchema for CopperLayerParam {
    fn schema_name() -> Cow<'static, str> {
        "CopperLayer".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A copper layer: top, inner1 … inner14 (those the stack has), bottom.",
            "type": "string",
            "enum": copper_names(),
        })
    }
}

/// A board drawing's layer by name: `silkscreen_top`, `silkscreen_bottom`, `documentation`, or a
/// copper layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawLayerParam(pub DrawLayer);

impl<'de> Deserialize<'de> for DrawLayerParam {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::String(s) => DrawLayer::try_from(s).map(DrawLayerParam).map_err(D::Error::custom),
            _ => Err(D::Error::custom("must be a board layer: silkscreen_top, silkscreen_bottom, documentation or a copper layer")),
        }
    }
}

impl JsonSchema for DrawLayerParam {
    fn schema_name() -> Cow<'static, str> {
        "BoardDrawLayer".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let mut names: Vec<String> = ["silkscreen_top", "silkscreen_bottom", "documentation"].map(String::from).to_vec();
        names.extend(copper_names());
        json_schema!({
            "description": "The layer to draw on: silkscreen_top, silkscreen_bottom, documentation (notes for the fabrication drawing), or a copper layer the stack has.",
            "type": "string",
            "enum": names,
        })
    }
}

/// A circle: its centre and radius, in millimetres.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct CircleSpec {
    pub center: [f64; 2],
    pub radius: f64,
}

/// `ecad.board.outline`: the board's edge, as a rectangle, a circle or corners.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct OutlineParams {
    /// Width and height in millimetres, centred on `center`; in place of `points` and `circle`.
    pub rect: Option<[f64; 2]>,
    /// The rectangle's middle; the origin when left out.
    pub center: Option<[f64; 2]>,
    /// With `rect`: its corners rounded to this radius in millimetres (at most half its shorter
    /// side).
    pub corner_radius: Option<f64>,
    /// A round board; in place of `rect` and `points`.
    pub circle: Option<CircleSpec>,
    /// The corners in order, closed back to the first (at least three, or two with an arc); `[]`
    /// takes the outline away.
    pub points: Option<Vec<[f64; 2]>>,
    /// With `points`: each edge's sweep in degrees (counter-clockwise when positive, 0 straight),
    /// the closing edge last; left out, every edge is straight.
    pub curves: Option<Vec<f64>>,
    /// The board's thickness in millimetres; left out, it stays.
    pub thickness: Option<f64>,
}

/// `ecad.board.sync` takes no parameters.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SyncParams {}

/// `ecad.board.move`: move, turn or flip board objects rigidly.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct MoveParams {
    /// Footprints by their part's designator, or any board objects by id.
    pub items: Vec<ItemRef>,
    /// How far, in millimetres.
    pub by: Option<[f64; 2]>,
    /// Where the first item's position (a footprint's origin, a track's first point, a via's
    /// centre) goes, after any turn and flip; in place of `by`.
    pub to: Option<[f64; 2]>,
    /// Degrees counter-clockwise about `about`: a multiple of 90.
    pub rotation: Option<i32>,
    /// Flip to the other side of the board: mirrored left to right about the vertical line
    /// through `about` (before the turn); footprints change side, tracks and drawings change
    /// layer.
    pub flip: Option<bool>,
    /// The point to turn and flip about; left out, the one item's position, or the middle of the
    /// items' positions on the 1.27 mm grid.
    pub about: Option<[f64; 2]>,
    /// Refuse the change when it brings copper of the moved objects (or of the tracks whose ends
    /// follow them) closer to another net's copper than the clearance, where it wasn't before.
    pub check: Option<bool>,
}

/// `ecad.board.rotate`: turn board objects, each footprint about its own origin.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RotateParams {
    pub items: Vec<ItemRef>,
    /// Degrees counter-clockwise: a multiple of 90; 90 when left out.
    pub rotation: Option<i32>,
    /// Refuse the change when it brings copper of the moved objects (or of the tracks whose ends
    /// follow them) closer to another net's copper than the clearance, where it wasn't before.
    pub check: Option<bool>,
}

/// `ecad.board.flip`: the board objects to act on.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ItemsParams {
    pub items: Vec<ItemRef>,
    /// Refuse the change when it brings copper of the moved objects (or of the tracks whose ends
    /// follow them) closer to another net's copper than the clearance, where it wasn't before.
    pub check: Option<bool>,
}

/// One segment of a track: segment 0 runs from its first point to its second.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct SegmentRef {
    pub track: u64,
    pub segment: usize,
}

/// `ecad.board.delete`: board objects, and single track segments, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DeleteParams {
    #[serde(default)]
    pub items: Vec<ItemRef>,
    /// Segments cut out of their tracks: what is left before a cut keeps the track's id, each
    /// piece after it is a new track.
    #[serde(default)]
    pub segments: Vec<SegmentRef>,
}

/// `ecad.board.track`: a copper track through points.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct TrackParams {
    pub layer: CopperLayerParam,
    /// In millimetres; left out, the track width of the net class of the copper at the first
    /// point (the rules' when it is on no net or in no class).
    pub width: Option<f64>,
    /// Two or more, in millimetres; an end inside a pad joins that pad. One alone, with `via`, is
    /// just the via there: a route that changes layer before its first segment.
    pub points: Vec<[f64; 2]>,
    /// Each segment's sweep in degrees (counter-clockwise when positive, 0 straight); left out,
    /// all straight.
    pub curves: Option<Vec<f64>>,
    /// A via at the last point, in the same step, for a route going on on another layer; none is
    /// added where a via whose span covers this one's or a through-hole pad is already.
    pub via: Option<TrackVia>,
    /// The route's next legs, each made as this one is, in the same step: a route over several
    /// layers is one undo step.
    pub legs: Option<Vec<TrackLeg>>,
}

/// A further leg of the route `ecad.board.track` makes: as its first, usually from the via the leg
/// before ends on.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct TrackLeg {
    pub layer: CopperLayerParam,
    /// In millimetres; left out, as the route's first leg's.
    pub width: Option<f64>,
    /// Two or more, in millimetres; one alone, with `via`, is just the via there.
    pub points: Vec<[f64; 2]>,
    pub curves: Option<Vec<f64>>,
    pub via: Option<TrackVia>,
}

/// The via `ecad.board.track` ends on.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct TrackVia {
    /// The top layer of its span; left out, the top (a through via).
    pub from: Option<CopperLayerParam>,
    /// The bottom layer of its span; left out, the bottom.
    pub to: Option<CopperLayerParam>,
    /// In millimetres; left out, the net class's via diameter (the rules' micro via between
    /// neighbouring layers other than top to bottom).
    pub diameter: Option<f64>,
    /// In millimetres; left out, as the diameter.
    pub drill: Option<f64>,
    /// Open when left out.
    pub protection: Option<ViaProtectionParam>,
}

/// `ecad.board.track.edit`: change tracks' layer or width, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct TrackEditParams {
    /// The tracks, by id.
    pub tracks: Vec<u64>,
    pub layer: Option<CopperLayerParam>,
    /// In millimetres.
    pub width: Option<f64>,
}

/// `ecad.board.via`: a via, through every copper layer or a span of them.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ViaParams {
    pub at: [f64; 2],
    /// The top layer of its span; left out, the top.
    pub from: Option<CopperLayerParam>,
    /// The bottom layer of its span; left out, the bottom.
    pub to: Option<CopperLayerParam>,
    /// In millimetres; left out, the net class's via diameter (the rules' micro via between
    /// neighbouring layers other than top to bottom).
    pub diameter: Option<f64>,
    /// In millimetres; left out, as the diameter.
    pub drill: Option<f64>,
    /// A net of its own, by name, for a via no track joins to a pad (a stitching via in a
    /// pour).
    pub net: Option<String>,
    /// Open when left out.
    pub protection: Option<ViaProtectionParam>,
}

/// `ecad.board.inspect` takes no parameters.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct InspectParams {}

/// What a board object is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BoardKind {
    Footprint,
    Track,
    Via,
    Hole,
    Graphic,
    Pour,
    Keepout,
}

/// A board object as replies name it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct BoardItemOut {
    pub id: u64,
    pub kind: BoardKind,
    /// A footprint's part's designator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The net a track or via is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
}

/// Which copper a pad has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Copper {
    Top,
    Bottom,
    /// A through-hole pad.
    Both,
    /// A bare hole.
    None,
}

/// A pad as it lies on the board.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PadOut {
    pub number: String,
    /// Its centre, in millimetres.
    pub at: [f64; 2],
    pub copper: Copper,
    /// The schematic net its pin is on; null when none.
    pub net: Option<String>,
    /// How pours join it, set for this pad (see `ecad.board.pour.connect`); left out, the
    /// footprint's or the pour's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect: Option<super::ecad_board_pour::ConnectParam>,
}

/// A footprint on the board, with its pads where they lie.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FootprintOut {
    pub id: u64,
    /// The schematic part's id.
    pub part: u64,
    pub designator: String,
    /// The value the part shows.
    pub value: String,
    /// The library footprint's name; `""` when the part's variant has none.
    pub footprint: String,
    pub at: [f64; 2],
    /// Degrees counter-clockwise.
    pub rotation: u16,
    pub side: Side,
    /// The box round its copper and graphics on the board.
    pub bbox: Option<[[f64; 2]; 2]>,
    /// The name text's offset from `at`; null for the footprint's own spot.
    pub name_at: Option<[f64; 2]>,
    /// The value text's offset from `at`; null for the footprint's own spot.
    pub value_at: Option<[f64; 2]>,
    pub show_name: bool,
    pub show_value: bool,
    /// How pours join its pads, set for the footprint (see `ecad.board.pour.connect`); left out,
    /// each pour's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pour_connect: Option<super::ecad_board_pour::ConnectParam>,
    pub pads: Vec<PadOut>,
    /// Pad overrides saved for pad numbers its footprint doesn't have (its part's footprint
    /// changed since); they apply again to a footprint with such a pad. `ecad.board.pour.connect`
    /// with those `pads` and `connect` null clears them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stale_connect: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TrackOut {
    pub id: u64,
    /// The copper layer: top, inner1 …, bottom.
    pub layer: String,
    pub width: f64,
    pub points: Vec<[f64; 2]>,
    /// Each segment's sweep in degrees; left out while all are straight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<f64>,
    /// Along its segments and arcs, in millimetres.
    pub length: f64,
    /// The net of the copper it joins; null when it joins no pad.
    pub net: Option<String>,
}

/// An unplated mounting hole.
#[derive(Debug, Serialize, JsonSchema)]
pub struct HoleOut {
    pub id: u64,
    pub at: [f64; 2],
    pub drill: f64,
}

/// A drawing on a board layer.
#[derive(Debug, Serialize, JsonSchema)]
pub struct BoardGraphicOut {
    pub id: u64,
    /// silkscreen_top, silkscreen_bottom, documentation or a copper layer.
    pub layer: String,
    /// Text drawn mirrored, as on the bottom.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mirror: bool,
    #[serde(flatten)]
    pub graphic: GraphicSpec,
}

/// Whether a copper layer is on the outside of the board or inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LayerKind {
    Outer,
    Inner,
}

/// A copper layer of the stack.
#[derive(Debug, Serialize, JsonSchema)]
pub struct StackLayerOut {
    /// top, inner1 …, bottom.
    pub name: String,
    pub kind: LayerKind,
}

/// What kind of via.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViaKindOut {
    Through,
    Blind,
    Buried,
    Micro,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ViaOut {
    pub id: u64,
    pub at: [f64; 2],
    pub diameter: f64,
    pub drill: f64,
    /// Its span, top layer first.
    pub from: String,
    pub to: String,
    pub kind: ViaKindOut,
    /// The net of the copper it joins.
    pub net: Option<String>,
    /// The net it is given of its own (a stitching via).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub own_net: Option<String>,
    /// Left out while open.
    #[serde(skip_serializing_if = "ViaProtectionParam::is_open")]
    pub protection: ViaProtectionParam,
    /// `stitch` for a stitching via (ecad.board.stitch); left out for any other.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// A stitching via's pattern; left out for any other via, or one stitched before patterns
    /// were kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<super::ecad_board_stitch::StitchPattern>,
}

/// A net on the board: its pads, and how many airwires it still has.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NetOut {
    pub name: String,
    /// `R1.2`: designator and pad number.
    pub pads: Vec<String>,
    pub airwires: usize,
}

/// A connection still to route.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AirwireOut {
    pub net: String,
    pub from: [f64; 2],
    pub to: [f64; 2],
}

/// What the commands that change the board return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct BoardResult {
    /// New board objects, in board order.
    pub created: Vec<BoardItemOut>,
    /// Board objects that changed.
    pub changed: Vec<BoardItemOut>,
    pub removed: Vec<BoardItemOut>,
    /// The created and changed footprints in full, with their pads on the board.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub footprints: Vec<FootprintOut>,
    /// How many airwires the board still has.
    pub unrouted: usize,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.board.outline` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct OutlineResult {
    pub outline: Vec<[f64; 2]>,
    /// Each edge's sweep in degrees, the closing edge last; left out while all are straight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<f64>,
    /// The outline's width and height; null without an outline.
    pub size: Option<[f64; 2]>,
    pub thickness: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.board.sync` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SyncResult {
    /// Footprints placed for parts that had none, laid out under the outline.
    pub added: Vec<FootprintOut>,
    /// Footprints whose part is gone from the schematic.
    pub removed: Vec<BoardItemOut>,
    /// Footprints left where they were.
    pub kept: usize,
    /// Parts whose variant has no footprint, so nothing was placed for them, by designator.
    pub no_footprint: Vec<String>,
    pub unrouted: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.board.inspect` returns, in board order.
#[derive(Debug, Serialize, JsonSchema)]
pub struct InspectResult {
    pub outline: Vec<[f64; 2]>,
    /// Each outline edge's sweep in degrees; left out while all are straight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline_curves: Vec<f64>,
    /// The outline's width and height; null without an outline.
    pub size: Option<[f64; 2]>,
    pub thickness: f64,
    pub copper_layers: u8,
    /// The copper layers, top to bottom.
    pub stack: Vec<StackLayerOut>,
    /// What the board is made of, top to bottom (the default stack-up unless it has its own).
    pub stackup: Vec<StackLayerSpec>,
    /// The board has a stack-up of its own.
    pub custom_stackup: bool,
    pub rules: RulesOut,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub net_classes: Vec<NetClassOut>,
    /// The height of every footprint's name and value text (`ecad.board.names`); left out, each is
    /// sized to its footprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_height: Option<f64>,
    pub footprints: Vec<FootprintOut>,
    pub tracks: Vec<TrackOut>,
    pub vias: Vec<ViaOut>,
    pub holes: Vec<HoleOut>,
    pub graphics: Vec<BoardGraphicOut>,
    /// Copper pours with what their fill comes to (`ecad.board.pour.inspect` has the polygons).
    pub pours: Vec<PourOut>,
    pub keepouts: Vec<KeepoutOut>,
    /// The schematic's nets that have pads on the board.
    pub nets: Vec<NetOut>,
    pub airwires: Vec<AirwireOut>,
    /// How many airwires there are: connections still to route.
    pub unrouted: usize,
    /// The board is too large to work out every connection.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
