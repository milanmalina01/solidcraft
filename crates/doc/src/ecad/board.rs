//! The circuit board: its outline, the footprints placed on it (one per schematic part), the copper
//! tracks and vias that join their pads, mounting holes, drawings on its layers, and the design
//! rules. Saved under `ecad.board`; nothing is saved while it is empty. Which pad is on which net,
//! and the airwires still to route, follow from the schematic and are never saved (see
//! `airwires`).
//!
//! A stack of 2 to 16 copper layers: top, the inner layers, bottom (see [`super::layers`]), with
//! what it is made of (see [`super::stackup`]). Tracks are on one copper layer; surface-mount pads
//! on the top or the bottom; through-hole pads go through every layer, and vias through the
//! layers from theirs `from` to their `to` (every layer for a through via). A track's segments and
//! the outline's edges may be arcs (see [`super::path`]). Copper pours fill areas of a layer with
//! a net's copper, and keepouts keep areas clear (see [`super::pour`]). Board objects (footprints,
//! tracks, vias, holes, drawings, pours and keepouts) share one id space of their own, and an id is
//! never reused. A footprint names its schematic part by id and shows
//! the footprint of the part's variant as the library has it now, so a library edit moves its pads.
//! Seen from the top of the board, a footprint on the bottom is mirrored left to right (x → −x)
//! before it is turned, as [`Placement`] places a mirrored part, and its pads and graphics change
//! sides with it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::drc::{CheckLevel, find_rule};
use super::profiles::{self, DEFAULT_PROFILE, FabChoice};
use super::quality::ParamSetting;

use super::footprint::{FootprintItem, MIN_DRILL, Side};
use super::graphics::{Graphic, GraphicType, shapes_bbox};
use super::layers::{CopperLayer, DrawLayer, MAX_COPPER_LAYERS, MIN_COPPER_LAYERS, stack};
use super::path::{self, check_curves};
use super::place::{Motion, Placement, is_quarter, lattice, snap, snap_point};
use super::pour::{Connect, Keepout, MAX_KEEPOUTS, MAX_POURS, Pour};
use super::rules::{Rules, check_net_name};
use super::schematic::{Approval, MAX_APPROVALS, MAX_KEY, MAX_PARTS, Schematic};
use super::stackup::{StackLayer, THICKNESS_SLACK, check_stackup, default_stackup, total};
use super::{MAX_ID, bbox};

/// The most footprints a board holds: one per schematic part.
pub const MAX_BOARD_FOOTPRINTS: usize = MAX_PARTS;
/// The most tracks a board holds.
pub const MAX_TRACKS: usize = 16384;
/// The most points one track has, its two ends included.
pub const MAX_TRACK_POINTS: usize = 256;
/// The most points all the tracks have together.
pub const MAX_ALL_TRACK_POINTS: usize = 65536;
/// The most vias a board holds.
pub const MAX_VIAS: usize = 8192;
/// The most legs one routed track has (`ecad.board.track`'s first and its `legs`, one per layer
/// it runs on).
pub const MAX_LEGS: usize = 64;
/// The most corners the outline has.
pub const MAX_OUTLINE_POINTS: usize = 1024;
/// The most mounting holes a board holds.
pub const MAX_HOLES: usize = 4096;
/// The most drawings a board holds.
pub const MAX_BOARD_GRAPHICS: usize = 4096;
/// The widest mounting hole, in millimetres.
pub const MAX_HOLE: f64 = 20.0;
/// A mounting hole's drill when none is given (an M3 screw's clearance), in millimetres.
pub const DEFAULT_HOLE_DRILL: f64 = 3.2;
/// How far from the origin a board point may be, in millimetres.
pub const MAX_BOARD: f64 = 2000.0;
/// The board's thickness when none is given, in millimetres.
pub const DEFAULT_THICKNESS: f64 = 1.6;
/// The thinnest and thickest board, in millimetres.
pub const THICKNESS: std::ops::RangeInclusive<f64> = 0.1..=10.0;
/// The narrowest and widest track, in millimetres.
pub const TRACK_WIDTH: std::ops::RangeInclusive<f64> = 0.01..=20.0;
/// The widest via, in millimetres.
pub const MAX_VIA: f64 = 10.0;
/// The board grid, 0.05 in: footprints, track points and vias land on it.
pub const BOARD_GRID: f64 = 1.27;
/// The gap the layout leaves between footprints it places, in millimetres.
pub const BOARD_GAP: f64 = 2.54;

/// The board, saved under `ecad.board`; nothing is saved while it is empty.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Board {
    /// The board's edge, a closed polygon (the last corner joins the first); empty while it has none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<[f64; 2]>,
    /// Each outline edge's sweep in degrees (see [`super::path`]), the closing edge last; empty
    /// while every edge is straight.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outline_curves: Vec<f64>,
    /// In millimetres.
    #[serde(skip_serializing_if = "is_default_thickness")]
    pub thickness: f64,
    /// How many copper layers the stack has: even, 2 to 16.
    #[serde(skip_serializing_if = "is_two")]
    pub copper_layers: u8,
    /// The placed footprints, in the order they were placed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub footprints: Vec<BoardFootprint>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Track>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vias: Vec<Via>,
    /// Unplated mounting holes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<Hole>,
    /// Lines, shapes and text drawn on the board's layers.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub graphics: Vec<BoardGraphic>,
    /// Copper pours, in the order they were made.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pours: Vec<Pour>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keepouts: Vec<Keepout>,
    /// What the board is made of, top to bottom; empty for the default stack-up (see
    /// [`super::stackup`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stackup: Vec<StackLayer>,
    #[serde(skip_serializing_if = "Rules::is_default")]
    pub rules: Rules,
    /// How high the footprints' names and values are, in millimetres; without one, each in step
    /// with its footprint (see [`super::names`]), or as its library footprint's spot for it says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_height: Option<f64>,
    /// The silkscreen names mode the board's names were last laid out by (see
    /// [`super::labels`]); `None` while they are as each footprint has them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub names_mode: Option<super::labels::NamesMode>,
    /// The board's markings (see [`super::marking`]): their drawings are in `graphics` and
    /// `keepouts`, tagged with the marking's id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markings: Vec<super::marking::Marking>,
    /// Design rule check findings someone accepted, by key (see [`super::drc`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<Approval>,
    /// The fab profile it is checked against (see [`super::profiles`]): the standard one on a new
    /// board; `None` on a board saved before profiles were kept, or once taken away, and the
    /// check then says none is chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fab_profile: Option<FabChoice>,
    /// The levels the design rule check gives codes in place of their own.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub drc_levels: BTreeMap<String, CheckLevel>,
    /// Design-quality findings someone accepted, by key (see [`super::quality`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub quality_approvals: Vec<Approval>,
    /// The levels the quality checks give codes in place of their own.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub quality_levels: BTreeMap<String, CheckLevel>,
    /// The quality checks' parameters the board sets, by `code.param` (`pi.layer_change.reach`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub quality_params: BTreeMap<String, ParamSetting>,
    /// The id the next board object gets; ids are never reused.
    #[serde(skip_serializing_if = "is_zero")]
    pub next_id: u64,
}

impl Default for Board {
    fn default() -> Self {
        Board {
            outline: Vec::new(),
            outline_curves: Vec::new(),
            thickness: DEFAULT_THICKNESS,
            copper_layers: MIN_COPPER_LAYERS,
            footprints: Vec::new(),
            tracks: Vec::new(),
            vias: Vec::new(),
            holes: Vec::new(),
            graphics: Vec::new(),
            pours: Vec::new(),
            keepouts: Vec::new(),
            stackup: Vec::new(),
            rules: Rules::default(),
            name_height: None,
            names_mode: None,
            markings: Vec::new(),
            approvals: Vec::new(),
            fab_profile: profiles::find(DEFAULT_PROFILE).map(FabChoice::of),
            drc_levels: BTreeMap::new(),
            quality_approvals: Vec::new(),
            quality_levels: BTreeMap::new(),
            quality_params: BTreeMap::new(),
            next_id: 0,
        }
    }
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn is_zero_turn(v: &u16) -> bool {
    *v == 0
}

fn is_top(side: &Side) -> bool {
    *side == Side::Top
}

fn is_default_thickness(t: &f64) -> bool {
    *t == DEFAULT_THICKNESS
}

fn is_two(n: &u8) -> bool {
    *n == MIN_COPPER_LAYERS
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_true(b: &bool) -> bool {
    *b
}

fn yes() -> bool {
    true
}

/// A schematic part's footprint on the board.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardFootprint {
    pub id: u64,
    /// The schematic part's id; its pads and graphics are those of the part's variant's footprint.
    pub part: u64,
    /// Where the footprint's origin goes, in millimetres.
    pub at: [f64; 2],
    /// Degrees counter-clockwise: 0, 90, 180 or 270.
    #[serde(default, skip_serializing_if = "is_zero_turn")]
    pub rotation: u16,
    /// On the bottom it is mirrored left to right (seen from the top) before the turn.
    #[serde(default, skip_serializing_if = "is_top")]
    pub side: Side,
    /// Where the name (designator) text goes, from `at` along the board's axes; without one, the
    /// footprint's own spot for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_at: Option<[f64; 2]>,
    /// Where the value text goes, as `name_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_at: Option<[f64; 2]>,
    /// The name shows on the board (it does unless hidden).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub show_name: bool,
    /// The value shows on the board (it does not unless shown).
    #[serde(default, skip_serializing_if = "is_false")]
    pub show_value: bool,
    /// The name's spot (`name_at`) and whether it shows were set by the board's names mode (see
    /// [`super::labels`]), not by hand: the mode may move it again. A spot set by hand clears it,
    /// and the mode never moves such a name.
    #[serde(default, skip_serializing_if = "is_false")]
    pub name_placed: bool,
    /// How a pour of their net joins its pads, over the pour's own `connect`; `None`: as the pour
    /// says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pour_connect: Option<Connect>,
    /// The same for single pads, by pad number, over `pour_connect`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pad_connect: BTreeMap<String, Connect>,
}

/// A copper track on one layer, through its points; each segment straight or an arc.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub id: u64,
    pub layer: CopperLayer,
    /// In millimetres.
    pub width: f64,
    /// Two or more, in order.
    pub points: Vec<[f64; 2]>,
    /// Each segment's sweep in degrees (see [`super::path`]); empty while all are straight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<f64>,
}

/// An unplated mounting hole: no copper.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hole {
    pub id: u64,
    pub at: [f64; 2],
    /// In millimetres.
    pub drill: f64,
}

/// A line, shape or text drawn on a board layer. Copper drawn so joins nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardGraphic {
    pub id: u64,
    pub layer: DrawLayer,
    /// Text only: drawn mirrored left to right about its `at` before its turn, as text on the
    /// bottom reads seen from the top.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mirror: bool,
    /// The shape, in board millimetres, with no footprint `layer` or `side` of its own.
    pub graphic: Graphic,
    /// The board marking it is part of, by the marking's id (see [`super::marking`]): made again
    /// with it when the design's revision or date changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marking: Option<u64>,
}

fn top() -> CopperLayer {
    CopperLayer::Top
}

fn bottom() -> CopperLayer {
    CopperLayer::Bottom
}

fn is_top_layer(l: &CopperLayer) -> bool {
    *l == CopperLayer::Top
}

fn is_bottom_layer(l: &CopperLayer) -> bool {
    *l == CopperLayer::Bottom
}

/// A plated hole joining the copper it passes on the layers from `from` down to `to`: a through
/// via goes from the top to the bottom, a blind one from an outer layer to an inner one, a buried
/// one between two inner layers (see [`ViaKind`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Via {
    pub id: u64,
    pub at: [f64; 2],
    /// The copper ring's outer diameter, in millimetres.
    pub diameter: f64,
    /// The hole's diameter, smaller than `diameter`.
    pub drill: f64,
    /// The top layer of its span, above `to` in the stack.
    #[serde(default = "top", skip_serializing_if = "is_top_layer")]
    pub from: CopperLayer,
    #[serde(default = "bottom", skip_serializing_if = "is_bottom_layer")]
    pub to: CopperLayer,
    /// A net it is on even when no track joins it to a pad of the net (a stitching via in a
    /// ground pour), by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
    /// What fills or covers its hole; open when left out.
    #[serde(default, skip_serializing_if = "ViaProtection::is_open")]
    pub protection: ViaProtection,
    /// What it is for: a stitching via (see [`super::stitch`]) is found and removed with the
    /// others of its row or grid.
    #[serde(default, skip_serializing_if = "ViaRole::is_plain")]
    pub role: ViaRole,
    /// A stitching via's pattern, so it is removed with the others of its pattern; left out for
    /// any other via (and for one stitched before patterns were kept, which may be of any: a
    /// removal of one pattern leaves it unless told to take it too).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<super::stitch::Pattern>,
}

/// What a via is for: an ordinary one (a layer change, a via placed by hand), or one of the
/// stitching vias that tie a net's copper on several layers together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViaRole {
    #[default]
    Plain,
    Stitch,
}

impl ViaRole {
    pub fn is_plain(&self) -> bool {
        *self == ViaRole::Plain
    }
}

/// What fills or covers a via's hole (the via protection types of IPC-4761, named plainly): left
/// open, tented (solder mask over it), plugged, filled, or filled and capped (plated over, so it
/// may sit in a pad).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViaProtection {
    #[default]
    Open,
    Tented,
    Plugged,
    Filled,
    FilledCapped,
}

impl ViaProtection {
    fn is_open(&self) -> bool {
        *self == ViaProtection::Open
    }

    pub fn name(self) -> &'static str {
        match self {
            ViaProtection::Open => "open",
            ViaProtection::Tented => "tented",
            ViaProtection::Plugged => "plugged",
            ViaProtection::Filled => "filled",
            ViaProtection::FilledCapped => "filled_capped",
        }
    }

    /// Its place from open (0) to filled and capped (4), as findings rank it.
    pub fn rank(self) -> f64 {
        match self {
            ViaProtection::Open => 0.0,
            ViaProtection::Tented => 1.0,
            ViaProtection::Plugged => 2.0,
            ViaProtection::Filled => 3.0,
            ViaProtection::FilledCapped => 4.0,
        }
    }
}

/// What kind of via, from its span (and size).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViaKind {
    /// Top to bottom.
    Through,
    /// From an outer layer to an inner one.
    Blind,
    /// Between two inner layers.
    Buried,
    /// Between two neighbouring layers, and no wider than the rules' micro via (a laser-drilled
    /// via; its drill may be as small as the rules' micro via drill).
    Micro,
}

impl ViaKind {
    pub fn name(self) -> &'static str {
        match self {
            ViaKind::Through => "through",
            ViaKind::Blind => "blind",
            ViaKind::Buried => "buried",
            ViaKind::Micro => "micro",
        }
    }
}

impl Via {
    /// The indexes of its top and bottom layers in a stack of `copper_layers`.
    pub fn span(&self, copper_layers: u8) -> (usize, usize) {
        let (a, b) = (self.from.index(copper_layers), self.to.index(copper_layers));
        (a.min(b), a.max(b))
    }

    /// Whether it joins copper on `layer` (of a stack of `copper_layers`).
    pub fn on(&self, layer: CopperLayer, copper_layers: u8) -> bool {
        let (a, b) = self.span(copper_layers);
        (a..=b).contains(&layer.index(copper_layers))
    }

    /// Whether it goes from the top to the bottom.
    pub fn is_through(&self) -> bool {
        self.from == CopperLayer::Top && self.to == CopperLayer::Bottom
    }

    /// What kind it is in a stack of `copper_layers` under `rules`.
    pub fn kind(&self, copper_layers: u8, rules: &Rules) -> ViaKind {
        let (a, b) = self.span(copper_layers);
        if self.is_through() {
            ViaKind::Through
        } else if b == a + 1 && self.diameter <= rules.microvia_diameter + 1e-9 {
            ViaKind::Micro
        } else if self.from.is_inner() && self.to.is_inner() {
            ViaKind::Buried
        } else {
            ViaKind::Blind
        }
    }
}

/// What a board object is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BoardKind {
    Footprint,
    Track,
    Via,
    Hole,
    Graphic,
    Pour,
    Keepout,
}

/// A via's diameter and drill: the drill at least [`MIN_DRILL`], the via wider than its drill and at
/// most [`MAX_VIA`].
pub fn check_via(diameter: f64, drill: f64) -> Result<(), String> {
    if !(drill.is_finite() && drill >= MIN_DRILL) {
        return Err(format!("the drill must be at least {MIN_DRILL} mm"));
    }
    if !(diameter.is_finite() && diameter > drill && diameter <= MAX_VIA) {
        return Err(format!("the diameter must be more than the drill and at most {MAX_VIA} mm"));
    }
    Ok(())
}

/// A point within [`MAX_BOARD`] of the origin on both axes.
pub fn check_board_point(what: &str, [x, y]: [f64; 2]) -> Result<(), String> {
    if !(x.is_finite() && y.is_finite() && x.abs() <= MAX_BOARD && y.abs() <= MAX_BOARD) {
        return Err(format!("{what} must be within {MAX_BOARD} mm of the origin"));
    }
    Ok(())
}

/// A track width within [`TRACK_WIDTH`].
pub fn check_track_width(width: f64) -> Result<(), String> {
    if !TRACK_WIDTH.contains(&width) {
        return Err(format!("a track's `width` must be {} to {} mm", TRACK_WIDTH.start(), TRACK_WIDTH.end()));
    }
    Ok(())
}

/// A mounting hole's drill: at least [`MIN_DRILL`], at most [`MAX_HOLE`].
pub fn check_hole(drill: f64) -> Result<(), String> {
    if !(drill.is_finite() && (MIN_DRILL..=MAX_HOLE).contains(&drill)) {
        return Err(format!("a hole's `drill` must be {MIN_DRILL} to {MAX_HOLE} mm"));
    }
    Ok(())
}

/// A closed outline (the board's, a pour's or a keepout's): three or more corners (two when an
/// edge is an arc) on the board, no corner twice in a row, a sweep per edge or none, enclosing an
/// area.
pub fn check_closed(what: &str, points: &[[f64; 2]], curves: &[f64]) -> Result<(), String> {
    if points.len() < 2 || points.len() > MAX_OUTLINE_POINTS {
        return Err(format!("{what} needs 3 to {MAX_OUTLINE_POINTS} corners (two when an edge is an arc)"));
    }
    for p in points {
        check_board_point(&format!("a corner of {what}"), *p)?;
    }
    let n = points.len();
    if (0..n).any(|i| points.get(i).map(|p| lattice(*p)) == points.get((i + 1) % n).map(|p| lattice(*p))) {
        return Err(format!("{what} has the same corner twice in a row"));
    }
    check_curves(curves, n).map_err(|e| format!("{what}: {e}"))?;
    check_reach(what, points, curves, true)?;
    if path::twice_area(points, curves).abs() < 1e-6 {
        return Err(format!("{what} encloses no area"));
    }
    Ok(())
}

/// A line's segments, arcs included, within [`MAX_BOARD`] of the origin.
fn check_reach(what: &str, points: &[[f64; 2]], curves: &[f64], closed: bool) -> Result<(), String> {
    match path::bbox(points, curves, closed) {
        Some([[x0, y0], [x1, y1]]) if [x0, y0, x1, y1].iter().all(|v| v.is_finite() && v.abs() <= MAX_BOARD) => Ok(()),
        None => Ok(()),
        Some(_) => Err(format!("{what} reaches more than {MAX_BOARD} mm from the origin")),
    }
}

/// A graphic moved: points by `m`; on a mirror, an arc runs the other way and text turns the
/// other way and changes `mirror`.
fn move_graphic(g: &mut BoardGraphic, m: &Motion) {
    let s = &mut g.graphic;
    s.points = s.points.iter().map(|p| m.point(*p)).collect();
    s.from = s.from.map(|p| m.point(p));
    s.to = s.to.map(|p| m.point(p));
    s.center = s.center.map(|p| m.point(p));
    s.at = s.at.map(|p| m.point(p));
    let turn = f64::from(m.rotation % 360);
    if s.kind == GraphicType::Arc {
        s.start = s.start.map(|a| snap((if m.mirror { 180.0 - a } else { a } + turn).rem_euclid(360.0)));
        s.sweep = s.sweep.map(|w| if m.mirror { -w } else { w });
    }
    if s.kind == GraphicType::Text {
        let r = if m.mirror { -s.rotation } else { s.rotation } + turn;
        // In (−180, 180].
        let r = (r + 180.0).rem_euclid(360.0) - 180.0;
        s.rotation = snap(if r <= -180.0 { r + 360.0 } else { r });
        if m.mirror {
            g.mirror = !g.mirror;
        }
    }
}

impl BoardGraphic {
    /// Its position: a line's first point, a rectangle's `from`, a circle's or an arc's centre,
    /// a text's `at`.
    pub fn position(&self) -> Option<[f64; 2]> {
        let g = &self.graphic;
        g.points.first().copied().or(g.from).or(g.center).or(g.at)
    }

    /// The smallest box around it, its line width included: mirrored text runs back from its
    /// `at` before its turn, as the fab files draw it.
    pub fn bbox(&self) -> Option<[[f64; 2]; 2]> {
        let g = &self.graphic;
        let (true, GraphicType::Text, Some([x, y]), Some(h)) = (self.mirror, g.kind, g.at, g.height) else { return g.bbox() };
        let w = h * 0.6 * super::overbar::plain(&g.text).chars().count() as f64;
        let (s, c) = g.rotation.to_radians().sin_cos();
        let corners = [[0.0, 0.0], [-w, 0.0], [-w, h], [0.0, h]].map(|[u, v]| [x + u * c - v * s, y + u * s + v * c]);
        let [[x0, y0], [x1, y1]] = bbox(corners)?;
        let half = g.width / 2.0;
        Some([[x0 - half, y0 - half], [x1 + half, y1 + half]])
    }
}

/// The box round a footprint in its own frame: its copper and its graphics' geometry.
pub fn footprint_box(f: &FootprintItem) -> Option<[[f64; 2]; 2]> {
    bbox(f.copper_bbox().into_iter().flatten().chain(shapes_bbox(&f.graphics).into_iter().flatten()))
}

impl BoardFootprint {
    /// Where it sits as a placement: on the bottom, mirrored.
    pub fn placement(&self) -> Placement {
        Placement { at: self.at, rotation: self.rotation, mirror: self.side == Side::Bottom }
    }

    /// Take `p`'s position, turn and side (mirrored is the bottom).
    pub fn set_placement(&mut self, p: Placement) {
        self.at = p.at;
        self.rotation = p.rotation;
        self.side = if p.mirror { Side::Bottom } else { Side::Top };
    }

    /// Whether it changes how a pour joins any of its pads.
    pub fn overrides_pours(&self) -> bool {
        self.pour_connect.is_some() || !self.pad_connect.is_empty()
    }

    /// How a pour whose own setting is `pour` joins its pad `number`: the pad's override, else the
    /// footprint's, else the pour's.
    pub fn connect_for(&self, number: &str, pour: Connect) -> Connect {
        self.pad_connect.get(number).copied().or(self.pour_connect).unwrap_or(pour)
    }
}

impl Board {
    /// Whether nothing is saved for the board.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether the board has anything on it: an outline, footprints, tracks, vias, holes or
    /// drawings.
    pub fn has_content(&self) -> bool {
        !(self.outline.is_empty()
            && self.footprints.is_empty()
            && self.tracks.is_empty()
            && self.vias.is_empty()
            && self.holes.is_empty()
            && self.graphics.is_empty()
            && self.pours.is_empty()
            && self.keepouts.is_empty())
    }

    /// Its stack-up: its own, or the default one for its thickness and copper layers.
    pub fn stackup_or_default(&self) -> Vec<StackLayer> {
        if self.stackup.is_empty() { default_stackup(self.copper_layers, self.thickness) } else { self.stackup.clone() }
    }

    /// Make the board `t` millimetres thick. With a stack-up of its own, its dielectrics grow or
    /// shrink in proportion so it stays as thick as the board (copper and mask keep theirs); why
    /// not when the copper and mask alone are that thick.
    pub fn set_thickness(&mut self, t: f64) -> Result<(), String> {
        if !self.stackup.is_empty() {
            let dielectric: f64 = self.stackup.iter().filter(|l| l.is_dielectric()).map(|l| l.thickness).sum();
            let fixed = total(&self.stackup) - dielectric;
            if !(t > fixed && dielectric > 0.0) {
                return Err(format!("the stack-up's copper and mask are {} mm thick already", snap(fixed)));
            }
            let scale = (t - fixed) / dielectric;
            for l in self.stackup.iter_mut().filter(|l| l.is_dielectric()) {
                l.thickness = snap(l.thickness * scale);
            }
        }
        self.thickness = t;
        Ok(())
    }

    pub fn pour(&self, id: u64) -> Option<&Pour> {
        self.pours.iter().find(|p| p.id == id)
    }

    pub fn keepout(&self, id: u64) -> Option<&Keepout> {
        self.keepouts.iter().find(|k| k.id == id)
    }

    /// The copper layers, top to bottom.
    pub fn stack(&self) -> Vec<CopperLayer> {
        stack(self.copper_layers)
    }

    pub fn has_layer(&self, layer: CopperLayer) -> bool {
        layer.exists_in(self.copper_layers)
    }

    pub fn footprint(&self, id: u64) -> Option<&BoardFootprint> {
        self.footprints.iter().find(|f| f.id == id)
    }

    /// The footprint of the schematic part `part`.
    pub fn footprint_of(&self, part: u64) -> Option<&BoardFootprint> {
        self.footprints.iter().find(|f| f.part == part)
    }

    pub fn track(&self, id: u64) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn via(&self, id: u64) -> Option<&Via> {
        self.vias.iter().find(|v| v.id == id)
    }

    pub fn hole(&self, id: u64) -> Option<&Hole> {
        self.holes.iter().find(|h| h.id == id)
    }

    pub fn graphic(&self, id: u64) -> Option<&BoardGraphic> {
        self.graphics.iter().find(|g| g.id == id)
    }

    /// What the board object `id` is.
    pub fn kind_of(&self, id: u64) -> Option<BoardKind> {
        if self.footprint(id).is_some() {
            Some(BoardKind::Footprint)
        } else if self.track(id).is_some() {
            Some(BoardKind::Track)
        } else if self.via(id).is_some() {
            Some(BoardKind::Via)
        } else if self.hole(id).is_some() {
            Some(BoardKind::Hole)
        } else if self.graphic(id).is_some() {
            Some(BoardKind::Graphic)
        } else if self.pour(id).is_some() {
            Some(BoardKind::Pour)
        } else if self.keepout(id).is_some() {
            Some(BoardKind::Keepout)
        } else {
            None
        }
    }

    /// Every board object's id.
    fn ids(&self) -> impl Iterator<Item = u64> + '_ {
        let footprints = self.footprints.iter().map(|f| f.id);
        let copper = footprints.chain(self.tracks.iter().map(|t| t.id)).chain(self.vias.iter().map(|v| v.id));
        let areas = self.pours.iter().map(|p| p.id).chain(self.keepouts.iter().map(|k| k.id));
        copper.chain(self.holes.iter().map(|h| h.id)).chain(self.graphics.iter().map(|g| g.id)).chain(areas)
    }

    /// One past the largest id in use (1 on an empty board).
    pub fn floor(&self) -> u64 {
        self.ids().map(|id| id.saturating_add(1)).max().unwrap_or(1)
    }

    /// A new board object's id, which no other one has had in this design.
    pub fn take_id(&mut self) -> Result<u64, String> {
        let ids = self.free_ids(1)?;
        self.next_id = ids.end;
        Ok(ids.start)
    }

    /// `n` ids in a row that no board object has had in this design, not taken yet (one look at
    /// the ids in use, however many).
    pub fn free_ids(&self, n: usize) -> Result<std::ops::Range<u64>, String> {
        let first = self.next_id.max(self.floor());
        let end = u64::try_from(n).ok().and_then(|n| first.checked_add(n)).filter(|end| *end <= MAX_ID);
        let Some(end) = end else { return Err("no board ids are left in this design".into()) };
        Ok(first..end)
    }

    /// The outline's box, arcs included; `None` without an outline.
    pub fn outline_bbox(&self) -> Option<[[f64; 2]; 2]> {
        path::bbox(&self.outline, &self.outline_curves, true)
    }

    /// An object's position: a footprint's origin, a track's first point, a via's or a hole's
    /// centre, a drawing's (see [`BoardGraphic::position`]), a pour's or a keepout's first corner.
    pub fn position(&self, id: u64) -> Option<[f64; 2]> {
        match self.kind_of(id)? {
            BoardKind::Footprint => self.footprint(id).map(|f| f.at),
            BoardKind::Track => self.track(id).and_then(|t| t.points.first().copied()),
            BoardKind::Via => self.via(id).map(|v| v.at),
            BoardKind::Hole => self.hole(id).map(|h| h.at),
            BoardKind::Graphic => self.graphic(id).and_then(BoardGraphic::position),
            BoardKind::Pour => self.pour(id).and_then(|p| p.outline.first().copied()),
            BoardKind::Keepout => self.keepout(id).and_then(|k| k.outline.first().copied()),
        }
    }

    /// Everything the design file and the commands require: the caps; an outline of three or more
    /// corners (two when an edge is an arc) that encloses an area; the stack, thickness and rules
    /// in range; unique, non-zero ids below `next_id`; one footprint per part, each of a part
    /// `sheet` has; tracks of two or more points with no point twice in a row, on a layer of the
    /// stack; sweeps less than a full turn; vias with a drill smaller than the via and a span
    /// down the stack; holes, drawings, pours and keepouts in range; a stack-up that fits the
    /// copper layers and is within 10 % of the thickness; approval keys in their form; points on
    /// the board.
    pub fn validate(&self, sheet: &Schematic) -> Result<(), String> {
        for (what, n, max) in [
            ("footprints", self.footprints.len(), MAX_BOARD_FOOTPRINTS),
            ("tracks", self.tracks.len(), MAX_TRACKS),
            ("vias", self.vias.len(), MAX_VIAS),
            ("outline corners", self.outline.len(), MAX_OUTLINE_POINTS),
            ("holes", self.holes.len(), MAX_HOLES),
            ("drawings", self.graphics.len(), MAX_BOARD_GRAPHICS),
            ("pours", self.pours.len(), MAX_POURS),
            ("keepouts", self.keepouts.len(), MAX_KEEPOUTS),
            ("approvals", self.approvals.len(), MAX_APPROVALS),
        ] {
            if n > max {
                return Err(format!("a board has at most {max} {what}"));
            }
        }
        if self.next_id > MAX_ID {
            return Err(format!("the board id counter {} is out of range", self.next_id));
        }
        if !(self.copper_layers.is_multiple_of(2) && (MIN_COPPER_LAYERS..=MAX_COPPER_LAYERS).contains(&self.copper_layers)) {
            return Err(format!("the board's `copper_layers` must be an even number, {MIN_COPPER_LAYERS} to {MAX_COPPER_LAYERS}"));
        }
        if !self.outline.is_empty() {
            check_closed("the board outline", &self.outline, &self.outline_curves)?;
        } else if !self.outline_curves.is_empty() {
            return Err("the board has `outline_curves` but no outline".into());
        }
        if !THICKNESS.contains(&self.thickness) {
            return Err(format!("the board's `thickness` must be {} to {} mm", THICKNESS.start(), THICKNESS.end()));
        }
        self.rules.validate()?;
        if let Some(h) = self.name_height
            && !super::names::NAME_HEIGHT.contains(&h)
        {
            let range = &super::names::NAME_HEIGHT;
            return Err(format!("the board's `name_height` must be {} to {} mm", range.start(), range.end()));
        }
        if !self.stackup.is_empty() {
            check_stackup(&self.stackup, self.copper_layers)?;
            let sum = total(&self.stackup);
            if (sum - self.thickness).abs() > THICKNESS_SLACK * self.thickness + 1e-9 {
                return Err(format!("the stack-up is {} mm thick, more than 10 % off the board's {} mm", snap(sum), self.thickness));
            }
        }
        let mut keys = BTreeSet::new();
        for a in &self.approvals {
            if a.key.is_empty() || a.key.chars().count() > MAX_KEY || a.key.chars().any(char::is_control) {
                return Err(format!("a design rule approval's key must be 1 to {MAX_KEY} characters with no control characters"));
            }
            if !keys.insert(a.key.as_str()) {
                return Err(format!("two design rule approvals have the key `{}`", a.key));
            }
            super::check_text("note", &a.note, super::MAX_DESCRIPTION)?;
        }
        if let Some(p) = &self.fab_profile {
            p.validate()?;
        }
        for code in self.drc_levels.keys() {
            if find_rule(code).is_none() {
                return Err(format!("the design rule check has no code `{code}` to give a level"));
            }
        }
        super::quality::validate_settings(self)?;
        let mut ids = BTreeSet::new();
        let mut check_id = |kind: &str, id: u64| {
            if id == 0 || id >= self.next_id {
                return Err(format!("board {kind} {id}: the id must be more than 0 and less than `next_id` ({})", self.next_id));
            }
            if !ids.insert(id) {
                return Err(format!("two board objects have the id {id}"));
            }
            Ok(())
        };
        let parts: BTreeSet<u64> = sheet.parts.iter().map(|p| p.id).collect();
        let mut placed = BTreeSet::new();
        for f in &self.footprints {
            check_id("footprint", f.id)?;
            let at = |e: String| format!("board footprint {}: {e}", f.id);
            if !parts.contains(&f.part) {
                return Err(at(format!("its schematic part {} is not on the schematic", f.part)));
            }
            if !placed.insert(f.part) {
                return Err(at(format!("schematic part {} has two footprints on the board", f.part)));
            }
            check_board_point("`at`", f.at).map_err(at)?;
            if !is_quarter(f.rotation) {
                return Err(at("`rotation` must be 0, 90, 180 or 270".into()));
            }
            for (what, v) in [("`name_at`", f.name_at), ("`value_at`", f.value_at)] {
                if let Some(v) = v {
                    check_board_point(what, v).map_err(at)?;
                }
            }
            if f.pad_connect.len() > super::footprint::MAX_PADS {
                return Err(at(format!("`pad_connect` names at most {} pads", super::footprint::MAX_PADS)));
            }
            for number in f.pad_connect.keys() {
                super::check_number("pad", number).map_err(at)?;
            }
        }
        let mut total = 0usize;
        for t in &self.tracks {
            check_id("track", t.id)?;
            let at = |e: String| format!("board track {}: {e}", t.id);
            check_track_width(t.width).map_err(at)?;
            if t.points.len() < 2 || t.points.len() > MAX_TRACK_POINTS {
                return Err(at(format!("a track has 2 to {MAX_TRACK_POINTS} points")));
            }
            total = total.saturating_add(t.points.len());
            for p in &t.points {
                check_board_point("a track point", *p).map_err(at)?;
            }
            if t.points.windows(2).any(|w| lattice(w[0]) == lattice(w[1])) {
                return Err(at("it has the same point twice in a row".into()));
            }
            if !self.has_layer(t.layer) {
                return Err(at(format!("the stack has no copper layer `{}`", t.layer.name())));
            }
            check_curves(&t.curves, t.points.len() - 1).map_err(at)?;
            check_reach("it", &t.points, &t.curves, false).map_err(at)?;
        }
        if total > MAX_ALL_TRACK_POINTS {
            return Err(format!("a board's tracks have at most {MAX_ALL_TRACK_POINTS} points together"));
        }
        for v in &self.vias {
            check_id("via", v.id)?;
            let at = |e: String| format!("board via {}: {e}", v.id);
            check_board_point("`at`", v.at).map_err(at)?;
            check_via(v.diameter, v.drill).map_err(at)?;
            for l in [v.from, v.to] {
                if !self.has_layer(l) {
                    return Err(at(format!("the stack has no copper layer `{}`", l.name())));
                }
            }
            if v.from.index(self.copper_layers) >= v.to.index(self.copper_layers) {
                return Err(at("`from` must be a layer above `to`".into()));
            }
            if let Some(n) = &v.net {
                check_net_name(n).map_err(at)?;
            }
        }
        for h in &self.holes {
            check_id("hole", h.id)?;
            let at = |e: String| format!("board hole {}: {e}", h.id);
            check_board_point("`at`", h.at).map_err(at)?;
            check_hole(h.drill).map_err(at)?;
        }
        for g in &self.graphics {
            check_id("drawing", g.id)?;
            let at = |e: String| format!("board drawing {}: {e}", g.id);
            g.graphic.validate_within(MAX_BOARD).map_err(at)?;
            if g.graphic.layer.is_some() || g.graphic.side != Side::Top {
                return Err(at("a board drawing has its own `layer`: leave out the graphic's `layer` and `side`".into()));
            }
            if !g.layer.exists_in(self.copper_layers) {
                return Err(at(format!("the stack has no copper layer `{}`", g.layer.name())));
            }
            if g.mirror && g.graphic.kind != GraphicType::Text {
                return Err(at("only text is drawn mirrored".into()));
            }
        }
        for p in &self.pours {
            check_id("pour", p.id)?;
            let at = |e: String| format!("board pour {}: {e}", p.id);
            if !self.has_layer(p.layer) {
                return Err(at(format!("the stack has no copper layer `{}`", p.layer.name())));
            }
            check_closed("its outline", &p.outline, &p.curves).map_err(at)?;
            let span = path::bbox(&p.outline, &p.curves, true).map_or(0.0, |[[x0, y0], [x1, y1]]| (x1 - x0).max(y1 - y0));
            p.check_settings(span).map_err(at)?;
        }
        for k in &self.keepouts {
            check_id("keepout", k.id)?;
            let at = |e: String| format!("board keepout {}: {e}", k.id);
            let mut seen = BTreeSet::new();
            for l in &k.layers {
                if !self.has_layer(*l) {
                    return Err(at(format!("the stack has no copper layer `{}`", l.name())));
                }
                if !seen.insert(*l) {
                    return Err(at(format!("it names layer `{}` twice", l.name())));
                }
            }
            check_closed("its outline", &k.outline, &k.curves).map_err(at)?;
        }
        Ok(())
    }

    /// Remove the footprints whose schematic part `sheet` no longer has; returns them.
    pub fn forget_parts(&mut self, sheet: &Schematic) -> Vec<BoardFootprint> {
        let parts: BTreeSet<u64> = sheet.parts.iter().map(|p| p.id).collect();
        let (keep, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.footprints).into_iter().partition(|f| parts.contains(&f.part));
        self.footprints = keep;
        gone
    }

    /// Remove the board objects `ids`. Tracks on a removed footprint's pads stay.
    pub fn remove(&mut self, ids: &BTreeSet<u64>) {
        self.footprints.retain(|f| !ids.contains(&f.id));
        self.tracks.retain(|t| !ids.contains(&t.id));
        self.vias.retain(|v| !ids.contains(&v.id));
        self.holes.retain(|h| !ids.contains(&h.id));
        self.graphics.retain(|g| !ids.contains(&g.id));
        self.pours.retain(|p| !ids.contains(&p.id));
        self.keepouts.retain(|k| !ids.contains(&k.id));
    }

    /// Move the board objects `ids` rigidly. A mirror flips them to the other side: footprints
    /// change side (their name and value texts go with them), tracks, drawings and pours change
    /// layer to the one in their place on the turned-over stack and their arcs run the other way,
    /// keepouts' layers and vias' spans turn over too (holes go through, so they only move).
    /// Tracks left behind keep their points; see [`super::rework`] for the move that drags their
    /// ends along.
    pub fn move_items(&mut self, ids: &BTreeSet<u64>, m: &Motion) {
        let layers = self.copper_layers;
        for f in self.footprints.iter_mut().filter(|f| ids.contains(&f.id)) {
            let p = m.placement(&f.placement());
            f.set_placement(p);
            f.name_at = f.name_at.map(|v| snap_point(m.vector(v)));
            f.value_at = f.value_at.map(|v| snap_point(m.vector(v)));
        }
        for t in self.tracks.iter_mut().filter(|t| ids.contains(&t.id)) {
            t.points = t.points.iter().map(|p| m.point(*p)).collect();
            if m.mirror {
                t.layer = t.layer.flipped(layers);
                t.curves = t.curves.iter().map(|c| -c + 0.0).collect();
            }
        }
        for v in self.vias.iter_mut().filter(|v| ids.contains(&v.id)) {
            v.at = m.point(v.at);
            if m.mirror {
                (v.from, v.to) = (v.to.flipped(layers), v.from.flipped(layers));
            }
        }
        for p in self.pours.iter_mut().filter(|p| ids.contains(&p.id)) {
            p.outline = p.outline.iter().map(|q| m.point(*q)).collect();
            if m.mirror {
                p.layer = p.layer.flipped(layers);
                p.curves = p.curves.iter().map(|c| -c + 0.0).collect();
            }
        }
        for k in self.keepouts.iter_mut().filter(|k| ids.contains(&k.id)) {
            k.outline = k.outline.iter().map(|q| m.point(*q)).collect();
            if m.mirror {
                k.layers = k.layers.iter().map(|l| l.flipped(layers)).collect();
                k.curves = k.curves.iter().map(|c| -c + 0.0).collect();
            }
        }
        for h in self.holes.iter_mut().filter(|h| ids.contains(&h.id)) {
            h.at = m.point(h.at);
        }
        for g in self.graphics.iter_mut().filter(|g| ids.contains(&g.id)) {
            move_graphic(g, m);
            if m.mirror {
                g.layer = g.layer.flipped(layers);
            }
        }
    }
}

/// The other side of the board.
pub fn other(side: Side) -> Side {
    match side {
        Side::Top => Side::Bottom,
        Side::Bottom => Side::Top,
    }
}
