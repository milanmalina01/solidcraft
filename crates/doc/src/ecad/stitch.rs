//! Stitching vias: vias of one net (ground, as a rule) set out in a pattern to tie that net's
//! copper on several layers together, each marked as a stitching via (see
//! [`super::board::ViaRole`]) so a row or a grid can be found and removed together. Three
//! patterns:
//! - **grid**: over an area (a pour's fill of the net, a rectangle, a polygon or the whole
//!   board), in staggered rows: neighbours `pitch` apart, the rows `pitch`·√3/2 apart and every
//!   other one shifted half a pitch. For as many vias, that leaves a smaller largest via-free
//!   circle than a square grid does (2/√3 ≈ 1.15 pitches across, against √2 ≈ 1.41), which is
//!   what a grid is for. The rows and columns are centred in the area, the via's radius and the
//!   edge gap in from its sides, so they look even and the outer ones aren't lost to the edge. A
//!   spot that fails is tried again a little off it (up to [`NUDGE`]'s last, in pitches), and a
//!   spot that close to a via of the net that already joins the span (or a through-hole pad) is
//!   left to it, so no via stands farther than that from its spot. Unless given, the pitch keeps
//!   the largest via-free circle within λg/10 at the board's highest frequency of concern (the
//!   intent's `f_concern_mhz` or the top of its highest radio band) where no spot is skipped, the
//!   offsets included (see [`grid_pitch`]); with neither, [`DEFAULT_GRID_PITCH`].
//! - **fence**: rows on both sides of tracks (RF lines), `offset` from the centreline: by default
//!   on the net's copper just past the gap (half the track's width, the clearance between the
//!   two nets and the via's radius) where that copper runs beside the line on its layer (a
//!   coplanar line); a stripline's at least [`STRIPLINE_H`] dielectric heights off, so the fence
//!   doesn't change its impedance; a microstrip's with no ground beside it at least
//!   [`MICROSTRIP_W`] widths and [`MICROSTRIP_H`] heights off, so the vias don't load it. A via
//!   stands beside each bend, tee, pad of the line and open end (the first spot that fits within
//!   [`FENCE_KEEP`] of it), then the runs between (straight and arcs) get vias at no more than
//!   `pitch` apart (less the offset in a run next to a bend, so round the outside of a corner
//!   the vias stay within the pitch as the crow flies): λg/20 at the intent's frequency of
//!   concern, else at twice the carrier (the
//!   fenced nets' own `f_mhz`, else the board's highest band); [`DEFAULT_FENCE_PITCH`] without
//!   any. Tracks that get no via, and have none of the net's beside them already, are listed
//!   (`unfenced`).
//! - **layer_change**: beside each signal via that changes reference plane (the nearest plane
//!   above or below the layers the signal runs on there are different layers), a via of the net
//!   that joins those planes, within [`CHANGE_REACH`] ([`CHANGE_NEAR`] preferred), where none is
//!   yet; an RF net's change gets [`RF_CHANGE_VIAS`] of them on opposite sides, within
//!   [`CHANGE_NEAR`] (or λg/20, if less). The signal nets are those given, else those the intent
//!   calls RF, clocks, fast, differential, strobes or switch nodes or gives edges of 5 ns or
//!   less, else every net with no pour. Planes of another net than the stitching one need a
//!   stitching capacitor, not a via: those are skipped.
//!
//! Every spot is checked as a placed via is (see [`Site`]): the clearance to other nets' copper
//! on every layer of its span (class-aware, and on inner layers the fab profile's drill to
//! copper), hole to hole with every drill, the board's edge and unplated holes
//! (`copper_to_edge`), keepouts that forbid vias, never in a surface-mount pad, a grid's or a
//! fence's not under a part (in the courtyard of a footprint on a side the via shows on, unless
//! `under_parts` allows it: a module often asks for nothing under it), and the net's copper (a
//! pour's fill, a track, a pad or a via) on two layers of its span at least, so none is a stub. Nor does a via stand where it would cut another net's plane in two: in its fill where
//! that is narrower, across the via, than the via's hole in it and the fill's thinnest copper on
//! both sides (`splits_plane`). Stitching runs against the pours' fills as they are, islands
//! they drop included. A spot that fails is skipped and counted by why. The work is bounded: at
//! most [`MAX_SPOTS`] spots looked at, [`MAX_TESTS`] tests (pairs, and points looked up in
//! fills and areas), [`MAX_STITCH`] vias placed by one call, [`MAX_NAMED`] tracks and nets named.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::Electronics;
use super::airwires::{BoardNets, BoardPad};
use super::board::{Board, BoardFootprint, Via, footprint_box};
use super::checks::gap;
use super::copper::{TOLERANCE, core_bbox, disc, drill_core, graphic_cores, inside, pad_core, pieces, track_cores};
use super::drc::DrcItem;
use super::fill::PourFill;
use super::footprint::{Core, PadKind, Side as FootprintSide};
use super::grid::{Grid, boxes_meet, grow};
use super::intent::{Intent, NetRole};
use super::layers::{CopperLayer, DrawLayer};
use super::path;
use super::place::{lattice, snap, snap_point};
use super::profiles::FabProfile;
use super::rules::{Sizes, clearance_between};
use super::stackup::surroundings;

/// The most vias one call places.
pub const MAX_STITCH: usize = 2000;
/// The most spots one call looks at.
pub const MAX_SPOTS: usize = 100_000;
/// The most tests one call makes.
pub const MAX_TESTS: usize = 1 << 24;
/// The most tracks, and nets, one call names.
pub const MAX_NAMED: usize = 4096;
/// A grid's pitch, and a fence's, when the intent gives no frequency, in millimetres.
pub const DEFAULT_GRID_PITCH: f64 = 5.0;
pub const DEFAULT_FENCE_PITCH: f64 = 2.0;
/// A fence via stands within this (millimetres, along the line) of each bend, tee, pad of the
/// line and open end.
pub const FENCE_KEEP: f64 = 1.0;
/// A layer change's ground via goes within this of the signal via, in millimetres, this far
/// preferred.
pub const CHANGE_REACH: f64 = 2.0;
pub const CHANGE_NEAR: f64 = 1.0;
/// The ground vias an RF net's layer change gets, within [`CHANGE_NEAR`] (or λg/20).
pub const RF_CHANGE_VIAS: usize = 2;
/// The finest and widest pitch, and the widest fence offset, in millimetres.
pub const MIN_PITCH: f64 = 0.2;
pub const MAX_PITCH: f64 = 100.0;
pub const MAX_OFFSET: f64 = 10.0;
/// A stripline's fence stands at least this many dielectric heights off it; a microstrip's with
/// no ground beside it at least this many widths and heights.
pub const STRIPLINE_H: f64 = 4.0;
pub const MICROSTRIP_W: f64 = 2.0;
pub const MICROSTRIP_H: f64 = 3.0;
/// The edges a signal's net has at most to count as T1 or faster, in nanoseconds.
const FAST_EDGE_NS: f64 = 5.0;
/// A grid spot that fails is tried again this far off it, in pitches (in eight directions, the
/// nearest first), before it is skipped: a via a little off the grid beats a gap in it. A spot
/// with a via of the net within the last of them is left to that via.
const NUDGE: [f64; 2] = [0.08, 0.16];
const NUDGE_TURNS: usize = 8;
/// The default grid pitch over λg/10: staggered rows leave a via-free circle 2/√3 pitches
/// across, and vias up to [`NUDGE`]'s last off their spots widen it by twice that.
const GRID_SPREAD: f64 = 1.0 / (1.154_700_538_379_251_5 + 2.0 * 0.16);
/// How many of a grid's failed spots are tried again off their spot at once.
const NUDGE_BATCH: usize = 4096;
/// The grid's cell, in millimetres.
const CELL: f64 = 1.0;
/// Light's speed in millimetres per microsecond: λ in millimetres is this over the frequency in
/// MHz, over √εr.
const LIGHT: f64 = 299_792.458;
/// How far apart staggered rows are, in pitches.
const ROW: f64 = 0.866_025_403_784_438_6;
/// The steps a fence's via is looked for in, from a stop along the line.
const KEEP_STEP: f64 = 0.05;
/// Where a layer change's via is tried: distances from the signal via (millimetres, in the order
/// tried) and directions round it.
const CHANGE_DISTANCES: [f64; 10] = [1.0, 0.9, 1.1, 0.8, 1.25, 0.7, 1.5, 0.6, 1.75, 2.0];
const CHANGE_TURNS: usize = 16;
/// A plane is looked for round a signal via at this many points, this far (millimetres) past the
/// via's ring and the widest clearance, and is there when this many of them are in its fill.
const PLANE_PROBES: usize = 8;
const PLANE_MARGIN: f64 = 0.25;
const PLANE_HITS: usize = 3;
/// Another net's fill round a spot is looked at in this many directions for a neck.
const NECK_TURNS: usize = 8;
/// What looking points up in one fill or area costs against the budget, past one a point.
const LOCATE_COST: usize = 64;

/// How the vias are set out (see the module notes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    Grid,
    Fence,
    LayerChange,
}

/// Where the vias go: a pour's fill (by the pour's id), a polygon (a rectangle given as one), or
/// the whole board.
#[derive(Clone, Debug, PartialEq)]
pub enum Area {
    Board,
    Pour(u64),
    Polygon(Vec<[f64; 2]>),
}

/// The vias' size and span, top layer first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViaSpec {
    pub diameter: f64,
    pub drill: f64,
    pub from: CopperLayer,
    pub to: CopperLayer,
}

/// What a call asks for (see [`plan`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    /// The net the vias are on, by name.
    pub net: String,
    pub pattern: Pattern,
    /// Where they may go; anywhere on the board when left out (a grid needs the board's outline
    /// then).
    pub area: Option<Area>,
    /// A fence's tracks, by id; or its nets, by name (a layer change's signal nets).
    pub tracks: Vec<u64>,
    pub nets: Vec<String>,
    pub pitch: Option<f64>,
    pub offset: Option<f64>,
    pub via: ViaSpec,
    /// The least gap kept to other nets' copper, past the rules' (which it never lowers).
    pub clearance: Option<f64>,
    /// A grid's or fence's vias may stand under parts: inside the courtyard of a footprint on a
    /// side the via shows on (left out, they keep out; a module or a part with copper under it
    /// often asks for nothing there).
    pub under_parts: bool,
}

/// Where the vias go, and what was skipped and why.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub spots: Vec<[f64; 2]>,
    /// By reason (see [`Blocked::reason`], and `near_via`, `splits_plane`, `outside`, `no_room`,
    /// `other_reference` and `short_span`).
    pub skipped: BTreeMap<&'static str, usize>,
    /// The pitch used (grid and fence), and the fence's offset when one offset served every track.
    pub pitch: Option<f64>,
    pub offset: Option<f64>,
    /// Why, in English: the pitch's reason, the pattern, what a layer change found.
    pub why: Vec<String>,
    /// How many spots were looked at.
    pub looked_at: usize,
    /// How many of a grid's vias stand off their spot (see [`NUDGE`]).
    pub moved: usize,
    /// A fence's tracks that got no via, by id.
    pub unfenced: Vec<u64>,
}

/// What a stitching via at a spot meets in the pours' fills (see [`Site::look`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Look {
    /// The layers of the span (bit `i` the span's `i`-th) where a fill of the net holds it.
    pub filled: u32,
    /// The pour of another net whose fill a via there would cut in two.
    pub cuts: Option<u64>,
}

impl Plan {
    fn skip(&mut self, reason: &'static str) {
        *self.skipped.entry(reason).or_default() += 1;
    }
}

/// Why a via can't go at a spot.
#[derive(Clone, Debug, PartialEq)]
pub enum Blocked {
    /// Closer than the clearance to another net's copper on `layer` (or, on an inner layer, its
    /// hole closer than the fab profile's drill to copper).
    Copper { item: DrcItem, net: Option<usize>, layer: CopperLayer, gap: f64, required: f64 },
    /// Its drill closer than the rules' hole to hole to another drill (`None`: one placed by the
    /// same call).
    Hole { item: Option<DrcItem>, gap: f64, required: f64 },
    /// Off the board, or closer than `copper_to_edge` to its edge or an unplated hole.
    Edge { gap: f64, required: f64 },
    /// In a keepout that forbids vias.
    Keepout(u64),
    /// In a surface-mount pad.
    Pad(DrcItem),
    /// The net's copper on fewer than two layers of its span.
    Stub { layers: usize },
    /// Under a part: in the courtyard of the footprint (by id) on a side the via shows on.
    Under(u64),
}

impl Blocked {
    /// The reason's name, as replies count skipped spots.
    pub fn reason(&self) -> &'static str {
        match self {
            Blocked::Copper { .. } => "other_copper",
            Blocked::Hole { .. } => "hole_to_hole",
            Blocked::Edge { .. } => "board_edge",
            Blocked::Keepout(_) => "keepout",
            Blocked::Pad(_) => "in_pad",
            Blocked::Stub { .. } => "no_copper",
            Blocked::Under(_) => "under_part",
        }
    }
}

/// Which of a via's checks a [`Site`] makes beyond the clearance, the drills, the edge and the
/// keepouts: a stitching via keeps out of pads and must join two layers; a via placed by hand
/// may sit in a pad and join one layer for now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checks {
    pub pads: bool,
    pub stub: bool,
}

impl Checks {
    pub const STITCH: Checks = Checks { pads: true, stub: true };
    pub const PLACED: Checks = Checks { pads: false, stub: false };
}

/// A piece of copper on one layer of the span.
struct Piece {
    item: DrcItem,
    net: Option<usize>,
    core: Core,
    /// A surface-mount pad.
    smd: bool,
}

struct LayerCopper {
    layer: CopperLayer,
    pieces: Vec<Piece>,
    grid: Grid,
}

struct Drill {
    item: Option<DrcItem>,
    core: Core,
    /// A via's span (stack indexes); `None` for a hole through the board.
    span: Option<(usize, usize)>,
}

struct Area2 {
    id: u64,
    ring: Vec<[f64; 2]>,
    edges: Vec<([f64; 2], [f64; 2])>,
    bbox: [[f64; 2]; 2],
}

fn pad_item(p: &BoardPad) -> DrcItem {
    DrcItem::Pad { footprint: p.footprint, number: p.pad.number.clone(), reference: p.reference() }
}

/// Whether `core` meets the closed polygon `ring` (whose edges are `edges`): inside it, or across
/// an edge.
fn hits(core: &Core, ring: &[[f64; 2]], edges: &[([f64; 2], [f64; 2])]) -> bool {
    core.points.iter().any(|p| inside(*p, ring)) || edges.iter().any(|(a, b)| gap(core, &Core { points: vec![*a, *b], radius: 0.0 }) <= 0.0)
}

/// A placed footprint's courtyard (else its box) on the board, as stitching keeps out of it.
fn yard(ecad: &Electronics, f: &BoardFootprint) -> Option<Area2> {
    let item = ecad.placed(f)?.resolved.footprint;
    let [[x0, y0], [x1, y1]] = item.courtyard(FootprintSide::Top).or_else(|| footprint_box(item))?;
    let pl = f.placement();
    let ring: Vec<[f64; 2]> = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(|p| pl.apply(p)).to_vec();
    let bbox = super::bbox(ring.iter().copied())?;
    let edges = ring.iter().copied().zip(ring.iter().copied().cycle().skip(1)).collect();
    Some(Area2 { id: f.id, ring, edges, bbox })
}

/// Which of `vias` stand under one of the footprints `footprints` (by id), as a grid or a fence
/// keeps out of (see [`Site::keep_off_parts`]): the via's pad meets the footprint's courtyard
/// (else its box) on a side the via shows on, the top when its span starts there, the bottom
/// when it ends there. Each via is tested against each footprint named, by their boxes first.
pub fn under_parts(ecad: &Electronics, footprints: &BTreeSet<u64>, vias: &[&Via]) -> Vec<bool> {
    let board = &ecad.board;
    let last = usize::from(board.copper_layers).saturating_sub(1);
    let yards: Vec<(FootprintSide, Area2)> =
        board.footprints.iter().filter(|f| footprints.contains(&f.id)).filter_map(|f| Some((f.side, yard(ecad, f)?))).collect();
    vias.iter()
        .map(|v| {
            let (a, b) = v.span(board.copper_layers);
            let land = disc(v.at, v.diameter / 2.0);
            let reach = core_bbox(&land);
            yards.iter().any(|(side, y)| {
                let shows = match side {
                    FootprintSide::Top => a == 0,
                    FootprintSide::Bottom => b >= last,
                };
                shows && boxes_meet(reach, y.bbox) && hits(&land, &y.ring, &y.edges)
            })
        })
        .collect()
}

/// The board as one via of a given size, span and net would meet it, for testing many spots (see
/// the module notes): its copper on each layer of the span, the drills, the edge, unplated
/// holes, keepouts and the net's fills, each in a grid.
pub struct Site<'a> {
    board: &'a Board,
    nets: &'a BoardNets,
    net: Option<usize>,
    via: ViaSpec,
    span: (usize, usize),
    sizes: Vec<Sizes>,
    /// The least clearance kept, past the rules'.
    floor: f64,
    max_gap: f64,
    layers: Vec<LayerCopper>,
    drills: Vec<Drill>,
    drill_grid: Grid,
    unplated: Vec<Core>,
    unplated_grid: Grid,
    ring: Vec<[f64; 2]>,
    edges: Vec<([f64; 2], [f64; 2])>,
    edge_grid: Grid,
    keepouts: Vec<Area2>,
    /// The courtyards a via keeps out of (see [`Site::keep_off_parts`]), each by its footprint's id.
    yards: Vec<Area2>,
    yard_grid: Grid,
    /// The net's fills on the span's layers: the layer's place in the span, the fill's index.
    fills: Vec<(usize, usize)>,
    /// Other nets' fills on the span's layers, each with how far round a via its copper must
    /// reach on both sides for the via not to cut it in two (see [`Site::look`]).
    others: Vec<(usize, f64)>,
    /// The fab profile's drill to copper on inner layers, and the hole it is measured from.
    drill_to_copper: Option<(f64, f64)>,
    /// Pair tests left.
    budget: usize,
}

impl<'a> Site<'a> {
    /// The site for a via `via` on the net `net` (an index into `nets`' names), keeping at least
    /// `floor` from other nets' copper.
    pub fn new(board: &'a Board, nets: &'a BoardNets, net: Option<usize>, via: ViaSpec, floor: f64) -> Self {
        Self::build(board, nets, net, via, floor, None)
    }

    /// The site for one via at `at` (the Via tool's, each frame): only what is near it.
    pub fn around(board: &'a Board, nets: &'a BoardNets, net: Option<usize>, via: ViaSpec, floor: f64, at: [f64; 2]) -> Self {
        let rules = &board.rules;
        let gaps = rules.net_classes.iter().map(|c| c.clearance).fold(rules.clearance, f64::max).max(floor);
        let reach = via.diameter + gaps + rules.copper_to_edge.max(rules.hole_to_hole) + 1.0;
        Self::build(board, nets, net, via, floor, Some(grow([at, at], reach)))
    }

    fn build(board: &'a Board, nets: &'a BoardNets, net: Option<usize>, via: ViaSpec, floor: f64, bound: Option<[[f64; 2]; 2]>) -> Self {
        let n = board.copper_layers;
        let (a, b) = (via.from.index(n), via.to.index(n));
        let span = (a.min(b), a.max(b));
        let rules = &board.rules;
        let sizes = rules.sizes_by_net(&nets.names);
        let max_gap = sizes.iter().map(|s| s.clearance).fold(rules.clearance, f64::max).max(floor);
        let near = |b: [[f64; 2]; 2]| bound.is_none_or(|k| boxes_meet(k, b));
        let mut layers = Vec::new();
        for layer in board.stack().into_iter().filter(|l| (span.0..=span.1).contains(&l.index(n))) {
            let mut pieces: Vec<Piece> = Vec::new();
            for p in nets.pads.iter().filter(|p| p.on(layer) && p.pad.kind != PadKind::Hole && near(p.bbox())) {
                pieces.push(Piece { item: pad_item(p), net: p.net, core: pad_core(p), smd: p.pad.kind == PadKind::Smd });
            }
            for (t, tn) in board.tracks.iter().zip(&nets.track_nets).filter(|(t, _)| t.layer == layer) {
                if path::bbox(&t.points, &t.curves, false).is_some_and(|b| near(grow(b, t.width / 2.0))) {
                    pieces.extend(track_cores(t).map(|core| Piece { item: DrcItem::Track(t.id), net: *tn, core, smd: false }));
                }
            }
            for (v, vn) in board.vias.iter().zip(&nets.via_nets).filter(|(v, _)| v.on(layer, n)) {
                let core = disc(v.at, v.diameter / 2.0);
                if near(core_bbox(&core)) {
                    pieces.push(Piece { item: DrcItem::Via(v.id), net: *vn, core, smd: false });
                }
            }
            for g in board.graphics.iter().filter(|g| g.layer == DrawLayer::Copper(layer)) {
                pieces.extend(graphic_cores(&g.graphic, &|p| p).into_iter().filter(|c| near(core_bbox(c))).map(|core| Piece {
                    item: DrcItem::Graphic(g.id),
                    net: None,
                    core,
                    smd: false,
                }));
            }
            let mut grid = Grid::new(CELL);
            for (i, p) in pieces.iter().enumerate() {
                grid.insert(i, core_bbox(&p.core));
            }
            layers.push(LayerCopper { layer, pieces, grid });
        }
        let mut drills: Vec<Drill> = board
            .vias
            .iter()
            .map(|v| Drill { item: Some(DrcItem::Via(v.id)), core: disc(v.at, v.drill / 2.0), span: Some(v.span(n)) })
            .filter(|d| near(core_bbox(&d.core)))
            .collect();
        let mut unplated: Vec<Core> = Vec::new();
        for p in &nets.pads {
            if let Some(core) = drill_core(p).filter(|c| near(core_bbox(c))) {
                if p.pad.kind == PadKind::Hole {
                    unplated.push(core.clone());
                }
                drills.push(Drill { item: Some(pad_item(p)), core, span: None });
            }
        }
        for h in board.holes.iter().filter(|h| near(core_bbox(&disc(h.at, h.drill / 2.0)))) {
            drills.push(Drill { item: Some(DrcItem::Hole(h.id)), core: disc(h.at, h.drill / 2.0), span: None });
            unplated.push(disc(h.at, h.drill / 2.0));
        }
        let mut drill_grid = Grid::new(CELL);
        for (i, d) in drills.iter().enumerate() {
            drill_grid.insert(i, core_bbox(&d.core));
        }
        let mut unplated_grid = Grid::new(CELL);
        for (i, u) in unplated.iter().enumerate() {
            unplated_grid.insert(i, core_bbox(u));
        }
        let (ring, edges) = if board.outline.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            (path::tessellate(&board.outline, &board.outline_curves, true, TOLERANCE), pieces(&board.outline, &board.outline_curves, true))
        };
        let mut edge_grid = Grid::new(CELL);
        for (i, (p, q)) in edges.iter().enumerate() {
            let b = path::segment_bbox(*p, *q, 0.0);
            if near(b) {
                edge_grid.insert(i, b);
            }
        }
        let keepouts = board
            .keepouts
            .iter()
            .filter(|k| k.forbid.vias && board.stack().into_iter().any(|l| k.on(l) && (span.0..=span.1).contains(&l.index(n))))
            .filter_map(|k| {
                let ring = path::tessellate(&k.outline, &k.curves, true, TOLERANCE);
                let bbox = super::bbox(ring.iter().copied()).filter(|b| near(*b))?;
                Some(Area2 { id: k.id, edges: pieces(&k.outline, &k.curves, true), ring, bbox })
            })
            .collect();
        let fills = nets
            .fills
            .iter()
            .enumerate()
            .filter(|(_, f)| net.is_some() && f.net == net)
            .filter_map(|(i, f)| {
                let at = f.layer.index(n);
                at.checked_sub(span.0).filter(|_| at <= span.1).map(|k| (k, i))
            })
            .collect();
        // The via's hole in the fill, and the fill's thinnest copper on each side of it.
        let others = nets
            .fills
            .iter()
            .enumerate()
            .filter(|(_, f)| f.net.is_some() && f.net != net && (span.0..=span.1).contains(&f.layer.index(n)))
            .filter(|(_, f)| f.region.bbox().is_some_and(near))
            .map(|(i, f)| (i, via.diameter / 2.0 + f.clearance + board.pour(f.pour).map_or(0.0, |p| p.min_width)))
            .collect();
        let micro = via.diameter <= rules.microvia_diameter + 1e-9 && span.1 == span.0 + 1 && !(span.0 == 0 && span.1 + 1 == usize::from(n));
        let drill_to_copper = board.fab_profile.as_ref().and_then(|p| p.profile()).map(|p: &FabProfile| {
            let hole = if micro { via.drill } else { p.hole(via.drill) };
            (p.drill_to_copper.value, hole)
        });
        Site {
            board,
            nets,
            net,
            via,
            span,
            sizes,
            floor,
            max_gap,
            layers,
            drills,
            drill_grid,
            unplated,
            unplated_grid,
            ring,
            edges,
            edge_grid,
            keepouts,
            yards: Vec::new(),
            yard_grid: Grid::new(CELL),
            fills,
            others,
            drill_to_copper,
            budget: MAX_TESTS,
        }
    }

    /// Keep the vias out of the courtyards of the footprints on each side they show on: the top
    /// when the span starts there, the bottom when it ends there. A footprint with no courtyard
    /// keeps them out of its box, as the design rule check takes it.
    pub fn keep_off_parts(&mut self, ecad: &Electronics) {
        let last = usize::from(self.board.copper_layers).saturating_sub(1);
        let (top, bottom) = (self.span.0 == 0, self.span.1 >= last);
        self.yards = self
            .board
            .footprints
            .iter()
            .filter(|f| match f.side {
                FootprintSide::Top => top,
                FootprintSide::Bottom => bottom,
            })
            .filter_map(|f| yard(ecad, f))
            .collect();
        self.yard_grid = Grid::new(CELL);
        for (i, y) in self.yards.iter().enumerate() {
            self.yard_grid.insert(i, y.bbox);
        }
    }

    /// What a stitching via at each of `points` would meet in the pours' fills (see [`Look`]);
    /// spends the budget on the look-ups (nothing is found once it is spent).
    pub fn look(&mut self, points: &[[f64; 2]]) -> Vec<Look> {
        let mut out = vec![Look::default(); points.len()];
        let calls = self.fills.len() + 2 * self.others.len();
        let cost = points.len().saturating_mul(1 + self.fills.len() + self.others.len() * (1 + NECK_TURNS)).saturating_add(calls * LOCATE_COST);
        if !self.spend(cost) {
            return out;
        }
        for (k, i) in &self.fills {
            let Some(f) = self.nets.fills.get(*i) else { continue };
            for (slot, hit) in out.iter_mut().zip(f.region.locate(points)) {
                if hit.is_some() {
                    slot.filled |= 1 << *k;
                }
            }
        }
        for (i, reach) in &self.others {
            let Some(f) = self.nets.fills.get(*i) else { continue };
            let held = f.region.locate(points);
            let inside: Vec<usize> = (0..points.len()).filter(|j| held.get(*j).is_some_and(Option::is_some)).collect();
            if inside.is_empty() {
                continue;
            }
            let probes: Vec<[f64; 2]> = inside
                .iter()
                .filter_map(|j| points.get(*j))
                .flat_map(|p| {
                    (0..NECK_TURNS).map(move |t| {
                        let a = std::f64::consts::TAU * t as f64 / NECK_TURNS as f64;
                        [p[0] + reach * a.cos(), p[1] + reach * a.sin()]
                    })
                })
                .collect();
            let hits = f.region.locate(&probes);
            for (m, j) in inside.iter().enumerate() {
                let fill = |t: usize| hits.get(m * NECK_TURNS + t).is_some_and(Option::is_some);
                // The fill is missing on both sides of the via, across one direction or another.
                let neck = (0..NECK_TURNS / 2).any(|t| !fill(t) && !fill(t + NECK_TURNS / 2));
                if let Some(slot) = out.get_mut(*j).filter(|s| neck && s.cuts.is_none()) {
                    slot.cuts = Some(f.pour);
                }
            }
        }
        out
    }

    /// Whether the pair tests ran out.
    pub fn spent(&self) -> bool {
        self.budget == 0
    }

    /// Spend `n` tests of the budget on other work (looking points up in an area); `false`, and
    /// nothing left, when there aren't that many.
    pub fn charge(&mut self, n: usize) -> bool {
        self.spend(n)
    }

    fn spend(&mut self, n: usize) -> bool {
        if self.budget < n {
            self.budget = 0;
            return false;
        }
        self.budget -= n;
        true
    }

    /// Whether a via at `at` fits (see the module notes); `filled` is the layers of the span
    /// (bit `i` the span's `i`-th) where a fill of the net holds it, as [`Site::look`] gives them. The first thing in the way: the edge, a keepout, a pad, other copper (the
    /// nearest), a drill, too few layers of the net's copper.
    pub fn check(&mut self, at: [f64; 2], filled: u32, checks: Checks) -> Result<(), Blocked> {
        let r = self.via.diameter / 2.0;
        let land = disc(at, r);
        let reach = core_bbox(&land);
        let rules = &self.board.rules;
        let edge = rules.copper_to_edge;
        if self.ring.len() >= 3 {
            if !inside(at, &self.ring) {
                return Err(Blocked::Edge { gap: -r, required: edge });
            }
            let near = self.edge_grid.near_box(grow(reach, edge), self.edges.len());
            if !self.spend(near.len()) {
                return Err(Blocked::Edge { gap: 0.0, required: edge });
            }
            for i in near {
                let Some((a, b)) = self.edges.get(i) else { continue };
                let g = gap(&land, &Core { points: vec![*a, *b], radius: 0.0 });
                if g < edge - 1e-9 {
                    return Err(Blocked::Edge { gap: g, required: edge });
                }
            }
        }
        let near = self.unplated_grid.near_box(grow(reach, edge), self.unplated.len());
        let rings: usize = self.keepouts.iter().filter(|k| boxes_meet(reach, k.bbox)).map(|k| k.ring.len() + k.edges.len()).sum();
        if !self.spend(near.len().saturating_add(rings)) {
            return Err(Blocked::Edge { gap: 0.0, required: edge });
        }
        for i in near {
            let Some(u) = self.unplated.get(i) else { continue };
            let g = gap(&land, u);
            if g < edge - 1e-9 {
                return Err(Blocked::Edge { gap: g, required: edge });
            }
        }
        for k in &self.keepouts {
            if boxes_meet(reach, k.bbox) && hits(&land, &k.ring, &k.edges) {
                return Err(Blocked::Keepout(k.id));
            }
        }
        let near = self.yard_grid.near_box(reach, self.yards.len());
        if !self.spend(near.len()) {
            return Err(Blocked::Edge { gap: 0.0, required: edge });
        }
        for y in near.into_iter().filter_map(|i| self.yards.get(i)) {
            if boxes_meet(reach, y.bbox) && hits(&land, &y.ring, &y.edges) {
                return Err(Blocked::Under(y.id));
            }
        }
        let same = |n: Option<usize>| self.net.is_some() && n == self.net;
        let mut joined = filled;
        let mut worst: Option<Blocked> = None;
        let mut tests = 0usize;
        for (k, lc) in self.layers.iter().enumerate() {
            let near = lc.grid.near_box(grow(reach, self.max_gap), lc.pieces.len());
            tests = tests.saturating_add(near.len());
            for i in near {
                let Some(p) = lc.pieces.get(i) else { continue };
                let g = gap(&land, &p.core);
                if checks.pads && p.smd && g < -1e-9 {
                    return Err(Blocked::Pad(p.item.clone()));
                }
                if same(p.net) {
                    if g <= 1e-9 {
                        joined |= 1 << k;
                    }
                    continue;
                }
                let required = clearance_between(rules, &self.sizes, self.net, p.net).max(self.floor);
                if g < required - 1e-6 && worst.as_ref().is_none_or(|w| matches!(w, Blocked::Copper { gap, .. } if g < *gap)) {
                    worst = Some(Blocked::Copper { item: p.item.clone(), net: p.net, layer: lc.layer, gap: g.max(0.0), required });
                }
            }
            // On an inner layer, the hole keeps the profile's distance from copper it doesn't join.
            if let Some((least, hole)) = self.drill_to_copper
                && lc.layer.is_inner()
                && worst.is_none()
            {
                let core = disc(at, hole / 2.0);
                for i in lc.grid.near_box(grow(core_bbox(&core), least), lc.pieces.len()) {
                    let Some(p) = lc.pieces.get(i).filter(|p| !same(p.net)) else { continue };
                    tests = tests.saturating_add(1);
                    let g = gap(&core, &p.core);
                    if g < least - 1e-6 {
                        worst = Some(Blocked::Copper { item: p.item.clone(), net: p.net, layer: lc.layer, gap: g.max(0.0), required: least });
                        break;
                    }
                }
            }
        }
        if !self.spend(tests) {
            return Err(worst.unwrap_or(Blocked::Stub { layers: 0 }));
        }
        if let Some(w) = worst {
            return Err(w);
        }
        let least = rules.hole_to_hole;
        let hole = disc(at, self.via.drill / 2.0);
        let (a0, a1) = self.span;
        for i in self.drill_grid.near_box(grow(core_bbox(&hole), least), self.drills.len()) {
            let Some(d) = self.drills.get(i) else { continue };
            // Vias drilled through no dielectric in common (a stack of micro vias) are two holes.
            if let Some((b0, b1)) = d.span
                && (a1 <= b0 || b1 <= a0)
            {
                continue;
            }
            let g = gap(&hole, &d.core);
            if g < least - 1e-9 {
                return Err(Blocked::Hole { item: d.item.clone(), gap: g.max(0.0), required: least });
            }
        }
        let layers = joined.count_ones() as usize;
        if checks.stub && layers < 2 {
            return Err(Blocked::Stub { layers });
        }
        Ok(())
    }

    /// A via placed at `at`: later spots keep their drills clear of its drill.
    pub fn add(&mut self, at: [f64; 2]) {
        let core = disc(at, self.via.drill / 2.0);
        self.drill_grid.insert(self.drills.len(), core_bbox(&core));
        self.drills.push(Drill { item: None, core, span: Some(self.span) });
    }
}

/// The wavelength in the board, in millimetres, at `mhz` in a dielectric of `er`.
fn wavelength(mhz: f64, er: f64) -> f64 {
    LIGHT / mhz / er.max(1.0).sqrt()
}

/// The board's dielectric εr the wavelengths use: its stack-up's highest (the shortest wavelength).
fn board_er(board: &Board) -> f64 {
    board.stackup_or_default().iter().filter(|l| l.is_dielectric()).map(|l| l.er()).fold(1.0, f64::max)
}

/// A frequency in MHz, and where it comes from in English.
type Frequency = (f64, String);

/// The board's highest frequency of concern in MHz and where it comes from: the intent's
/// `f_concern_mhz` or the top of its highest radio band, whichever is higher.
fn concern(intent: &Intent) -> Option<Frequency> {
    let facts = &intent.board;
    let band =
        facts.bands.iter().flatten().max_by(|a, b| a.hi_mhz.total_cmp(&b.hi_mhz)).map(|b| (b.hi_mhz, format!("the top of radio band {}", b.name)));
    let declared = facts.f_concern_mhz.filter(|f| f.is_finite() && *f > 0.0).map(|f| (f, "the intent's frequency of concern".to_string()));
    match (band, declared) {
        (Some(b), Some(d)) => Some(if b.0 > d.0 { b } else { d }),
        (b, d) => b.or(d),
    }
}

/// Millimetres as the reasons give them.
fn mm(v: f64) -> String {
    let s = format!("{:.3}", v + 0.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A pitch rounded down to 0.05 mm (no finer than [`MIN_PITCH`]).
fn tidy_pitch(v: f64) -> f64 {
    snap(((v / 0.05 + 1e-9).floor() * 0.05).max(MIN_PITCH))
}

/// The closest vias `via` wide can sit, as a pitch: the via and the rules' hole to hole.
fn least_pitch(board: &Board, via: f64) -> f64 {
    tidy_pitch(via + board.rules.hole_to_hole + 0.05)
}

/// Where the vias may go, as a test of points.
enum Within<'a> {
    Fill(&'a PourFill),
    Ring(Vec<[f64; 2]>),
}

impl Within<'_> {
    fn bbox(&self) -> Option<[[f64; 2]; 2]> {
        match self {
            Within::Fill(f) => f.region.bbox(),
            Within::Ring(r) => super::bbox(r.iter().copied()),
        }
    }

    fn holds(&self, points: &[[f64; 2]]) -> Vec<bool> {
        match self {
            Within::Fill(f) => f.region.locate(points).into_iter().map(|s| s.is_some()).collect(),
            Within::Ring(r) => points.iter().map(|p| inside(*p, r)).collect(),
        }
    }

    /// What a look-up of `points` costs against a site's budget: a point against a ring's
    /// corners is far cheaper than a pair test, so 16 corners count as one.
    fn cost(&self, points: usize) -> usize {
        match self {
            Within::Fill(_) => points.saturating_add(LOCATE_COST),
            Within::Ring(r) => points.saturating_mul(1 + r.len() / 16),
        }
    }
}

/// The area `a` on the board for vias of the net `net`, or why not.
fn within<'a>(board: &Board, nets: &'a BoardNets, a: &Area, span: (usize, usize), net: &str) -> Result<Within<'a>, String> {
    match a {
        Area::Board => {
            if board.outline.is_empty() {
                return Err("the board has no outline to stitch: give a `region` (ecad.board.outline sets one)".into());
            }
            Ok(Within::Ring(path::tessellate(&board.outline, &board.outline_curves, true, TOLERANCE)))
        }
        Area::Pour(id) => {
            let Some(p) = board.pour(*id) else { return Err(format!("the board has no pour with the id {id}")) };
            if p.net.as_deref() != Some(net) {
                let on = p.net.as_deref().map_or_else(|| "on no net".to_string(), |n| format!("on {n}"));
                return Err(format!("pour {id} is {on}, not {net}: stitch a pour of {net}, or give a `rect` or a `polygon`"));
            }
            let at = p.layer.index(board.copper_layers);
            if !(span.0..=span.1).contains(&at) {
                return Err(format!("pour {id} is on {}, outside the vias' span", p.layer.name()));
            }
            let Some(f) = nets.fills.iter().find(|f| f.pour == *id) else { return Err(format!("pour {id} has no fill")) };
            if f.region.is_empty() {
                return Err(format!("pour {id} fills nothing to stitch"));
            }
            Ok(Within::Fill(f))
        }
        Area::Polygon(points) => {
            if points.len() < 3 {
                return Err("a region's polygon needs 3 corners or more".into());
            }
            Ok(Within::Ring(points.clone()))
        }
    }
}

/// Where stitching vias of `ask` go on `ecad`'s board, whose connections are `nets` (see the
/// module notes); why not when the net, a track, a pour or the area isn't there, a size is out of
/// range, or the work would be past [`MAX_SPOTS`] spots, [`MAX_TESTS`] tests or [`MAX_STITCH`]
/// vias.
pub fn plan(ecad: &Electronics, nets: &BoardNets, ask: &Ask) -> Result<Plan, String> {
    let board = &ecad.board;
    let Some(net) = nets.names.iter().position(|n| *n == ask.net) else { return Err(format!("the schematic has no net `{}`", ask.net)) };
    if ask.tracks.len() > MAX_NAMED || ask.nets.len() > MAX_NAMED {
        return Err(format!("name at most {MAX_NAMED} tracks and {MAX_NAMED} nets"));
    }
    if let Some(p) = ask.pitch
        && !(p.is_finite() && (MIN_PITCH..=MAX_PITCH).contains(&p) && p > ask.via.diameter)
    {
        return Err(format!("`pitch` must be {MIN_PITCH} to {MAX_PITCH} mm and wider than the via ({} mm)", ask.via.diameter));
    }
    if let Some(o) = ask.offset
        && !(o.is_finite() && o > 0.0 && o <= MAX_OFFSET)
    {
        return Err(format!("`offset` must be more than 0 and at most {MAX_OFFSET} mm"));
    }
    if let Some(c) = ask.clearance
        && !(c.is_finite() && (0.0..=MAX_OFFSET).contains(&c))
    {
        return Err(format!("`clearance` must be 0 to {MAX_OFFSET} mm"));
    }
    let mut site = Site::new(board, nets, Some(net), ask.via, ask.clearance.unwrap_or(0.0));
    if !ask.under_parts && matches!(ask.pattern, Pattern::Grid | Pattern::Fence) {
        site.keep_off_parts(ecad);
    }
    let area = ask.area.as_ref().map(|a| within(board, nets, a, site.span, &ask.net)).transpose()?;
    let mut out = match ask.pattern {
        Pattern::Grid => grid(ecad, nets, ask, net, site, area)?,
        Pattern::Fence => fence(ecad, nets, ask, net, site, area.as_ref())?,
        Pattern::LayerChange => layer_change(ecad, nets, ask, net, site, area.as_ref())?,
    };
    out.spots.iter_mut().for_each(|p| *p = snap_point(*p));
    if out.skipped.contains_key("splits_plane") {
        out.why.push("spots where a via would cut another net's plane in two (its hole in the fill leaving less than the fill's thinnest copper on both sides) are skipped".into());
    }
    Ok(out)
}

/// Positions of the net's vias (and its through-hole pads), in a grid, for "a via is near".
struct Near {
    points: Vec<[f64; 2]>,
    grid: Grid,
}

impl Near {
    fn new(points: Vec<[f64; 2]>) -> Self {
        let mut grid = Grid::new(CELL);
        for (i, p) in points.iter().enumerate() {
            grid.insert(i, [*p, *p]);
        }
        Near { points, grid }
    }

    fn add(&mut self, p: [f64; 2]) {
        self.grid.insert(self.points.len(), [p, p]);
        self.points.push(p);
    }

    fn within(&self, p: [f64; 2], d: f64) -> bool {
        self.grid
            .near_box(grow([p, p], d), self.points.len())
            .into_iter()
            .filter_map(|i| self.points.get(i))
            .any(|q| (q[0] - p[0]).hypot(q[1] - p[1]) < d)
    }

    /// The points in the box `b`.
    fn in_box(&self, b: [[f64; 2]; 2]) -> Vec<[f64; 2]> {
        self.grid.near_box(b, self.points.len()).into_iter().filter_map(|i| self.points.get(i).copied()).collect()
    }
}

/// Where the net's copper already joins the layers of `span` (stack indexes) through the board:
/// its vias whose span holds that span, and its through-hole pads.
fn net_vias(board: &Board, nets: &BoardNets, net: usize, span: (usize, usize)) -> Vec<[f64; 2]> {
    let n = board.copper_layers;
    let vias = board.vias.iter().zip(&nets.via_nets).filter(|(v, x)| {
        let (a, b) = v.span(n);
        **x == Some(net) && a <= span.0 && span.1 <= b
    });
    let pads = nets.pads.iter().filter(|p| p.net == Some(net) && p.top && p.bottom && p.pad.kind != PadKind::Hole).map(|p| p.at());
    vias.map(|(v, _)| v.at).chain(pads).collect()
}

/// Why a stitching via can't stand at a spot whose look is `look`, by the reason's name.
fn try_spot(site: &mut Site, at: [f64; 2], look: Look) -> Result<(), &'static str> {
    if look.cuts.is_some() {
        return Err("splits_plane");
    }
    site.check(at, look.filled, Checks::STITCH).map_err(|b| b.reason())
}

/// The spots tried round `p` when it fails, `pitch` apart from its neighbours: the nearest first.
fn nudges(p: [f64; 2], pitch: f64) -> impl Iterator<Item = [f64; 2]> {
    NUDGE.into_iter().flat_map(move |k| {
        (0..NUDGE_TURNS).map(move |t| {
            let a = std::f64::consts::TAU * t as f64 / NUDGE_TURNS as f64;
            snap_point([p[0] + k * pitch * a.cos(), p[1] + k * pitch * a.sin()])
        })
    })
}

const TOO_MUCH: &str = "there is too much copper here to check every spot in time: give a smaller region";

fn grid(ecad: &Electronics, nets: &BoardNets, ask: &Ask, net: usize, mut site: Site, area: Option<Within>) -> Result<Plan, String> {
    let board = &ecad.board;
    let mut out = Plan::default();
    let area = match area {
        Some(a) => a,
        None => within(board, nets, &Area::Board, site.span, &ask.net)?,
    };
    let pitch = match ask.pitch {
        Some(p) => {
            out.why.push(format!("pitch {} mm, as asked", mm(p)));
            p
        }
        None => {
            let d = grid_pitch(ecad, ask.via.diameter);
            out.why.push(d.why);
            d.pitch
        }
    };
    out.pitch = Some(pitch);
    out.why.push("staggered rows (every other row shifted half a pitch, rows pitch × 0.866 apart), centred in the area clear of its edge: for as many vias, the largest via-free circle is smaller than a square grid's".into());
    // Only the part of the area on the board.
    let bbox = match (area.bbox(), board.outline_bbox()) {
        (Some([a0, a1]), Some([b0, b1])) => Some([[a0[0].max(b0[0]), a0[1].max(b0[1])], [a1[0].min(b1[0]), a1[1].min(b1[1])]]),
        (a, _) => a,
    };
    let Some([[x0, y0], [x1, y1]]) = bbox.filter(|[lo, hi]| lo[0] <= hi[0] && lo[1] <= hi[1]) else { return Ok(out) };
    let row = pitch * ROW;
    // The rows and columns centred in the area, kept the via's radius and the edge gap in from
    // its sides, so the outer ones aren't lost to the edge; an area narrower (or lower) than that
    // gets one column (row) on its middle.
    let inset = board.rules.copper_to_edge + ask.via.diameter / 2.0 + 0.05;
    let (w, h) = (x1 - x0 - 2.0 * inset, y1 - y0 - 2.0 * inset);
    let (columns, rows) = ((w.max(0.0) / pitch).floor(), (h.max(0.0) / row).floor());
    let count = (rows + 1.0) * (columns + 1.0);
    if !(count.is_finite() && count <= MAX_SPOTS as f64) {
        return Err(format!("a {} mm pitch over this area is more than {MAX_SPOTS} spots: give a wider pitch or a smaller region", mm(pitch)));
    }
    let left = if w > 0.0 { x0 + inset + (w - columns * pitch) / 2.0 } else { (x0 + x1) / 2.0 };
    let bottom = if h > 0.0 { y0 + inset + (h - rows * row) / 2.0 } else { (y0 + y1) / 2.0 };
    let mut points = Vec::new();
    for j in 0..=(rows as i64) {
        // Every other row half a pitch on, one spot fewer; a single column stays one.
        let (shift, last) = if j % 2 == 1 && columns >= 1.0 { (pitch / 2.0, columns as i64 - 1) } else { (0.0, columns as i64) };
        for i in 0..=last {
            points.push(snap_point([left + i as f64 * pitch + shift, bottom + j as f64 * row]));
        }
    }
    if !site.charge(area.cost(points.len())) {
        return Err(TOO_MUCH.into());
    }
    let held = area.holds(&points);
    let points: Vec<[f64; 2]> = points.into_iter().zip(held).filter(|(_, h)| *h).map(|(p, _)| p).collect();
    let looks = site.look(&points);
    // No via stands farther off its spot than this, nor does one of the net nearer to it leave it.
    let keep = NUDGE.last().copied().unwrap_or(0.0) * pitch;
    let mut near = Near::new(net_vias(board, nets, net, site.span));
    let mut failed: Vec<([f64; 2], &'static str)> = Vec::new();
    for (p, look) in points.iter().zip(looks) {
        if site.spent() {
            return Err(TOO_MUCH.into());
        }
        out.looked_at += 1;
        if near.within(*p, keep) {
            out.skip("near_via");
            continue;
        }
        match try_spot(&mut site, *p, look) {
            Ok(()) => {
                site.add(*p);
                near.add(*p);
                out.spots.push(*p);
            }
            Err(reason) => failed.push((*p, reason)),
        }
    }
    // Then each spot that failed a little off it, the nearest first, a batch at a time.
    let tries = NUDGE.len() * NUDGE_TURNS;
    for batch in failed.chunks(NUDGE_BATCH) {
        let around: Vec<[f64; 2]> = batch.iter().flat_map(|(p, _)| nudges(*p, pitch)).collect();
        if !site.charge(area.cost(around.len())) {
            return Err(TOO_MUCH.into());
        }
        let held = area.holds(&around);
        let looks = site.look(&around);
        for (k, (_, reason)) in batch.iter().enumerate() {
            let mut found = None;
            for i in k * tries..(k + 1) * tries {
                let (Some(q), Some(true), Some(look)) = (around.get(i), held.get(i).copied(), looks.get(i).copied()) else { continue };
                if !near.within(*q, keep) && try_spot(&mut site, *q, look).is_ok() {
                    found = Some(*q);
                    break;
                }
                if site.spent() {
                    return Err(TOO_MUCH.into());
                }
            }
            match found {
                Some(q) => {
                    site.add(q);
                    near.add(q);
                    out.spots.push(q);
                    out.moved += 1;
                }
                None => out.skip(reason),
            }
        }
    }
    if out.spots.len() > MAX_STITCH {
        return Err(format!("the grid would place more than {MAX_STITCH} vias: give a wider pitch or a smaller region"));
    }
    if out.moved > 0 {
        out.why.push(format!(
            "{} of the vias stand up to {} mm off their spot, where the spot itself was too close to something",
            out.moved,
            mm(keep)
        ));
    }
    Ok(out)
}

/// A pattern's pitch when none is given: the pitch, the frequency it is worked out from (none
/// for the usual pitch), and why in English.
#[derive(Clone, Debug, PartialEq)]
pub struct DefaultPitch {
    pub pitch: f64,
    pub mhz: Option<f64>,
    pub why: String,
}

/// The pitch a grid takes when none is given, for vias `via` wide (see the module notes): with
/// each via up to [`NUDGE`]'s last off its spot, staggered rows `p` apart leave no via-free
/// circle wider than `p`·(2/√3 + 2·that), which is to stay within λg/10.
pub fn grid_pitch(ecad: &Electronics, via: f64) -> DefaultPitch {
    let board = &ecad.board;
    let er = board_er(board);
    match concern(&ecad.intent) {
        Some((f, from)) => {
            let tenth = wavelength(f, er) / 10.0;
            let (aim, least) = (tidy_pitch(tenth * GRID_SPREAD), least_pitch(board, via));
            let p = aim.max(least);
            let off = mm(NUDGE.last().copied().unwrap_or(0.0) * p);
            let why = if least > aim {
                format!(
                    "pitch {} mm, the closest these vias can sit (the via and the rules' hole to hole): λg/10 at {} MHz ({from}) is {} mm in εr {}, which needs {} mm, so via-free circles can be wider than that",
                    mm(p),
                    mm(f),
                    mm(tenth),
                    mm(er),
                    mm(aim)
                )
            } else {
                format!(
                    "pitch {} mm: λg/10 at {} MHz ({from}) is {} mm in εr {}; staggered rows this far apart, each via at most {off} mm off its spot, leave no via-free circle wider than that where no spot is skipped",
                    mm(p),
                    mm(f),
                    mm(tenth),
                    mm(er)
                )
            };
            DefaultPitch { pitch: p, mhz: Some(f), why }
        }
        None => DefaultPitch {
            pitch: DEFAULT_GRID_PITCH,
            mhz: None,
            why: format!(
                "pitch {} mm: the intent gives no frequency of concern or radio band (ecad.intent.set), so the usual {} mm",
                mm(DEFAULT_GRID_PITCH),
                mm(DEFAULT_GRID_PITCH)
            ),
        },
    }
}

/// The pitch a fence along the nets `names` takes when none is given, for vias `via` wide (see
/// the module notes).
pub fn fence_pitch(ecad: &Electronics, names: &BTreeSet<String>, via: f64) -> DefaultPitch {
    let board = &ecad.board;
    let er = board_er(board);
    match fence_frequency(ecad, names) {
        Some((f, from)) => {
            let twentieth = wavelength(f, er) / 20.0;
            let (aim, least) = (tidy_pitch(twentieth), least_pitch(board, via));
            let p = aim.max(least);
            let why = if least > aim {
                format!(
                    "pitch {} mm, the closest these vias can sit (the via and the rules' hole to hole): λg/20 at {} MHz ({from}) in εr {} is {} mm, closer than they fit",
                    mm(p),
                    mm(f),
                    mm(er),
                    mm(twentieth)
                )
            } else {
                format!("pitch {} mm: λg/20 at {} MHz ({from}) in εr {} is {} mm", mm(p), mm(f), mm(er), mm(twentieth))
            };
            DefaultPitch { pitch: p, mhz: Some(f), why }
        }
        None => DefaultPitch {
            pitch: DEFAULT_FENCE_PITCH,
            mhz: None,
            why: format!(
                "pitch {} mm: the intent gives these nets no frequency and the board no radio band (ecad.intent.set), so the usual {} mm",
                mm(DEFAULT_FENCE_PITCH),
                mm(DEFAULT_FENCE_PITCH)
            ),
        },
    }
}

/// The nets a fence is for when none are named: those the intent calls RF, else those the
/// engine proposes as RF (see [`super::infer`]).
pub fn rf_nets(ecad: &Electronics) -> Vec<String> {
    let kept: Vec<String> = ecad.intent.nets.iter().filter(|r| r.role == NetRole::Rf).map(|r| r.net.clone()).collect();
    if !kept.is_empty() {
        return kept;
    }
    super::infer::infer(ecad)
        .proposals
        .into_iter()
        .filter_map(|p| match p.record {
            super::infer::Record::Net(r) if r.role == NetRole::Rf => Some(r.net),
            _ => None,
        })
        .collect()
}

/// The nets `names` by index, or why not.
fn net_indexes(nets: &BoardNets, names: &[String]) -> Result<BTreeSet<usize>, String> {
    let index: BTreeMap<&str, usize> = nets.names.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
    names.iter().map(|n| index.get(n.as_str()).copied().ok_or_else(|| format!("the schematic has no net `{n}`"))).collect()
}

/// The frequency a fence's pitch is worked out at, in MHz, and why: the intent's frequency of
/// concern as declared; else twice the carrier, the fenced nets' own frequency or else the top
/// of the board's highest radio band.
fn fence_frequency(ecad: &Electronics, names: &BTreeSet<String>) -> Option<Frequency> {
    let facts = &ecad.intent.board;
    if let Some(f) = facts.f_concern_mhz.filter(|f| f.is_finite() && *f > 0.0) {
        return Some((f, "the intent's frequency of concern".into()));
    }
    let own = ecad
        .intent
        .nets
        .iter()
        .filter(|n| names.contains(&n.net))
        .filter_map(|n| n.f_mhz.filter(|f| f.is_finite() && *f > 0.0).map(|f| (f, n.net.clone())))
        .max_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((f, n)) = own {
        return Some((2.0 * f, format!("twice {n}'s {} MHz in the intent", mm(f))));
    }
    let band = facts.bands.iter().flatten().max_by(|a, b| a.hi_mhz.total_cmp(&b.hi_mhz))?;
    Some((2.0 * band.hi_mhz, format!("twice the top of radio band {}, {} MHz", band.name, mm(band.hi_mhz))))
}

/// A fenced line's stops (see the module notes): where it bends, branches or ends, and its net's
/// pads and vias, in grids.
struct Stops {
    points: Near,
    pads: Vec<Core>,
    pad_grid: Grid,
}

impl Stops {
    fn new() -> Self {
        Stops { points: Near::new(Vec::new()), pads: Vec::new(), pad_grid: Grid::new(CELL) }
    }

    fn pad(&mut self, core: Core) {
        self.pad_grid.insert(self.pads.len(), core_bbox(&core));
        self.pads.push(core);
    }

    /// Whether `p` is within [`FENCE_KEEP`] of a stop.
    fn near(&self, p: [f64; 2]) -> bool {
        if self.points.within(p, FENCE_KEEP - 1e-9) {
            return true;
        }
        let at = Core { points: vec![p], radius: 0.0 };
        self.pad_grid
            .near_box(grow([p, p], FENCE_KEEP), self.pads.len())
            .into_iter()
            .filter_map(|i| self.pads.get(i))
            .any(|c| gap(&at, c) < FENCE_KEEP - 1e-9)
    }
}

/// The unit direction from `a` to `b`, `None` for no length.
fn unit(a: [f64; 2], b: [f64; 2]) -> Option<[f64; 2]> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l = dx.hypot(dy);
    (l > 1e-9).then(|| [dx / l, dy / l])
}

/// Whether the segments into and out of a point run on in one straight line (within half a degree).
fn straight_on(d0: Option<[f64; 2]>, d1: Option<[f64; 2]>) -> bool {
    match (d0, d1) {
        (Some(a), Some(b)) => a[0] * b[0] + a[1] * b[1] > 0.999_96,
        _ => false,
    }
}

/// A segment of a fenced line, walked along: straight, or an arc.
#[derive(Clone, Copy)]
enum Walk {
    Line { a: [f64; 2], u: [f64; 2] },
    Arc(path::Arc),
}

/// One side of one segment of a fenced line: where its vias go, `offset` from the centreline on
/// the side `side` (1 left, -1 right) of the direction it runs in.
struct Side {
    /// By index into the picked tracks.
    track: usize,
    walk: Walk,
    length: f64,
    offset: f64,
    side: f64,
    /// No via of the net nearer than this to one of its spots leaves the spot to it (less than
    /// the offset, so the far side's vias never do).
    keep: f64,
    /// Its ends are stops, and the stops along it (millimetres along).
    start: bool,
    end: bool,
    tees: Vec<f64>,
    /// Where its vias stand (or were meant to, when a stop's found no room), millimetres along.
    fixed: Vec<f64>,
}

impl Side {
    /// The spot `s` millimetres along.
    fn spot(&self, s: f64) -> [f64; 2] {
        let s = s.clamp(0.0, self.length);
        let (foot, normal) = match &self.walk {
            Walk::Line { a, u } => ([a[0] + u[0] * s, a[1] + u[1] * s], [-u[1], u[0]]),
            Walk::Arc(arc) => {
                let p = arc.point(s / self.length.max(1e-9));
                (p, unit(arc.center, p).unwrap_or([1.0, 0.0]))
            }
        };
        snap_point([foot[0] + normal[0] * self.offset * self.side, foot[1] + normal[1] * self.offset * self.side])
    }
}

/// How a fence's offset was chosen for a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Line {
    Asked,
    Coplanar,
    Stripline,
    Microstrip,
}

/// Look for a spot for one of a fence's vias on `side` at the positions `along` (millimetres
/// along, the first the one meant), placing the first that fits: where (`None` once the budget
/// is spent); why the first failed when none does (`None` when a via of the net is near enough
/// already).
fn fence_spot(
    site: &mut Site,
    near: &mut Near,
    out: &mut Plan,
    side: &Side,
    along: &[f64],
    area: Option<&Within>,
) -> Result<Option<f64>, Option<&'static str>> {
    let points: Vec<[f64; 2]> = along.iter().map(|s| side.spot(*s)).collect();
    let Some(first) = points.first().copied() else { return Err(Some("no_room")) };
    out.looked_at += 1;
    if near.within(first, side.keep) {
        return Err(None);
    }
    let held = match area {
        Some(a) => {
            if !site.charge(a.cost(points.len())) {
                return Ok(None);
            }
            a.holds(&points)
        }
        None => vec![true; points.len()],
    };
    let looks = site.look(&points);
    let mut why: Option<&'static str> = None;
    for (((p, h), look), s) in points.iter().zip(held).zip(looks).zip(along) {
        let tried = if !h {
            Err("outside")
        } else if near.within(*p, side.keep) {
            Err("near_via")
        } else {
            try_spot(site, *p, look)
        };
        match tried {
            Ok(()) => {
                site.add(*p);
                near.add(*p);
                out.spots.push(*p);
                return Ok(Some(*s));
            }
            Err(r) => {
                why.get_or_insert(r);
            }
        }
        if site.spent() {
            return Ok(None);
        }
    }
    Err(Some(why.unwrap_or("no_room")))
}

fn fence(ecad: &Electronics, nets: &BoardNets, ask: &Ask, net: usize, mut site: Site, area: Option<&Within>) -> Result<Plan, String> {
    let board = &ecad.board;
    let n = board.copper_layers;
    let mut out = Plan::default();
    let wanted = net_indexes(nets, &ask.nets)?;
    let index: BTreeMap<u64, usize> = board.tracks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
    let mut picked: Vec<usize> = Vec::new();
    for id in &ask.tracks {
        match index.get(id) {
            Some(i) => picked.push(*i),
            None => return Err(format!("the board has no track with the id {id}")),
        }
    }
    let net_of = |i: usize| nets.track_nets.get(i).copied().flatten();
    picked.extend((0..board.tracks.len()).filter(|i| net_of(*i).is_some_and(|n| wanted.contains(&n))));
    picked.sort_unstable();
    picked.dedup();
    if picked.is_empty() {
        return Err(if ask.tracks.is_empty() && ask.nets.is_empty() {
            "name the `tracks` to fence, or their `nets`".into()
        } else {
            "the nets named have no tracks to fence".into()
        });
    }
    let fenced: BTreeSet<usize> = picked.iter().filter_map(|i| net_of(*i)).collect();
    if fenced.contains(&net) {
        return Err(format!("the tracks to fence are on {} itself", ask.net));
    }
    let names: BTreeSet<String> = fenced.iter().filter_map(|n| nets.name(*n)).map(str::to_string).collect();
    let pitch = match ask.pitch {
        Some(p) => {
            out.why.push(format!("pitch {} mm, as asked", mm(p)));
            p
        }
        None => {
            let d = fence_pitch(ecad, &names, ask.via.diameter);
            out.why.push(d.why);
            d.pitch
        }
    };
    out.pitch = Some(pitch);
    // Each fenced net's stops: where its lines bend, branch or end, its pads and its vias.
    let mut stops: BTreeMap<usize, Stops> = fenced.iter().map(|n| (*n, Stops::new())).collect();
    let mut ends: BTreeMap<(usize, [i64; 2]), Vec<[f64; 2]>> = BTreeMap::new();
    for (i, t) in board.tracks.iter().enumerate() {
        let Some(tn) = net_of(i).filter(|n| fenced.contains(n)) else { continue };
        let segs: Vec<([f64; 2], [f64; 2], f64)> = path::segments(&t.points, &t.curves, false).collect();
        if picked.binary_search(&i).is_ok()
            && let Some(st) = stops.get_mut(&tn)
        {
            for k in 1..t.points.len().saturating_sub(1) {
                let (Some(before), Some(after)) = (segs.get(k - 1), segs.get(k)) else { continue };
                let arc = path::is_arc(before.2) || path::is_arc(after.2);
                if (arc || !straight_on(unit(before.0, before.1), unit(after.0, after.1)))
                    && let Some(q) = t.points.get(k)
                {
                    st.points.add(*q);
                }
            }
        }
        if let (Some(first), Some(last)) = (segs.first(), segs.last()) {
            // Each end with the direction the line leaves it in.
            let leaves = |a: [f64; 2], b: [f64; 2], s: f64| if path::is_arc(s) { [0.0, 0.0] } else { unit(b, a).unwrap_or_default() };
            ends.entry((tn, lattice(first.0))).or_default().push(leaves(first.0, first.1, first.2));
            ends.entry((tn, lattice(last.1))).or_default().push(leaves(last.1, last.0, last.2));
        }
    }
    for ((tn, key), list) in &ends {
        // Two ends that meet heading straight on are one line; anything else stops it.
        let through = list.len() == 2 && list.first().zip(list.get(1)).is_some_and(|(a, b)| a[0] * b[0] + a[1] * b[1] < -0.999_96);
        if !through && let Some(st) = stops.get_mut(tn) {
            st.points.add([key[0] as f64 / 1e4, key[1] as f64 / 1e4]);
        }
    }
    for p in nets.pads.iter().filter(|p| p.top || p.bottom) {
        if let Some(st) = p.net.and_then(|n| stops.get_mut(&n)) {
            st.pad(pad_core(p));
        }
    }
    for (v, vn) in board.vias.iter().zip(&nets.via_nets) {
        if let Some(st) = vn.and_then(|n| stops.get_mut(&n)) {
            st.points.add(v.at);
        }
    }
    // Each track's offset: on the net's copper just past the gap where it runs beside the line
    // on the line's layer; a stripline's at least 4 dielectric heights off; a microstrip with no
    // ground beside it at least 2 widths and 3 heights off, so the vias don't load it.
    let sizes = board.rules.sizes_by_net(&nets.names);
    let r = ask.via.diameter / 2.0;
    let floor = ask.clearance.unwrap_or(0.0);
    let stack = board.stackup_or_default();
    let mut gapped: Vec<f64> = Vec::with_capacity(picked.len());
    let mut samples: Vec<(usize, [f64; 2])> = Vec::new();
    for (k, i) in picked.iter().enumerate() {
        let Some(t) = board.tracks.get(*i) else { continue };
        let g = clearance_between(&board.rules, &sizes, net_of(*i), Some(net)).max(floor);
        // Rounded up, with room for the spots' rounding.
        let o = ((t.width / 2.0 + g + r + 0.0005) * 1000.0).ceil() / 1000.0;
        gapped.push(o);
        if !t.layer.is_inner() {
            for (a, b, s) in path::segments(&t.points, &t.curves, false) {
                let Some(u) = unit(a, b).filter(|_| !path::is_arc(s)) else { continue };
                let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                for side in [1.0, -1.0] {
                    samples.push((k, [m[0] - u[1] * o * side, m[1] + u[0] * o * side]));
                }
            }
        }
    }
    let at: Vec<[f64; 2]> = samples.iter().map(|(_, p)| *p).collect();
    if !site.charge(at.len().saturating_add(LOCATE_COST * nets.fills.len().max(1))) {
        return Err(TOO_MUCH.into());
    }
    let mut coplanar: BTreeSet<usize> = BTreeSet::new();
    for f in nets.fills.iter().filter(|f| f.net == Some(net)) {
        for ((k, _), hit) in samples.iter().zip(f.region.locate(&at)) {
            if hit.is_some() && picked.get(*k).and_then(|i| board.tracks.get(*i)).is_some_and(|t| t.layer == f.layer) {
                coplanar.insert(*k);
            }
        }
    }
    let mut sides: Vec<Side> = Vec::new();
    let mut lines: BTreeMap<Line, (f64, f64)> = BTreeMap::new();
    for (k, i) in picked.iter().enumerate() {
        let (Some(t), Some(tn), Some(o)) = (board.tracks.get(*i), net_of(*i), gapped.get(k).copied()) else { continue };
        let around = surroundings(&stack, t.layer, n);
        let height = |l: Option<super::stackup::StackLayer>| l.map(|l| l.thickness);
        let (line, offset) = match ask.offset {
            Some(asked) => (Line::Asked, asked),
            None if t.layer.is_inner() => {
                let h = around.and_then(|s| match (height(s.above), height(s.below)) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                });
                (Line::Stripline, o.max(h.map_or(0.0, |h| STRIPLINE_H * h)))
            }
            None if coplanar.contains(&k) => (Line::Coplanar, o),
            None => {
                let h = around.and_then(|s| height(s.below).or(height(s.above))).unwrap_or(0.0);
                (Line::Microstrip, o.max(MICROSTRIP_W * t.width).max(MICROSTRIP_H * h))
            }
        };
        let offset = ((offset * 1000.0) - 1e-6).ceil() / 1000.0;
        let slot = lines.entry(line).or_insert((offset, offset));
        *slot = (slot.0.min(offset), slot.1.max(offset));
        let Some(st) = stops.get(&tn) else { continue };
        for (a, b, sweep) in path::segments(&t.points, &t.curves, false) {
            let (walk, length) = match path::Arc::new(a, b, sweep) {
                Some(arc) => {
                    let l = arc.length();
                    (Walk::Arc(arc), l)
                }
                None => {
                    let Some(u) = unit(a, b) else { continue };
                    (Walk::Line { a, u }, (b[0] - a[0]).hypot(b[1] - a[1]))
                }
            };
            if !(length.is_finite() && length > 1e-9) {
                continue;
            }
            // A branch's end or a via of the net along it (not at its ends) is a stop too.
            let mut tees: Vec<f64> = Vec::new();
            if let Walk::Line { u, .. } = walk {
                for q in st.points.in_box(grow(path::segment_bbox(a, b, 0.0), t.width / 2.0)) {
                    let s = (q[0] - a[0]) * u[0] + (q[1] - a[1]) * u[1];
                    let off = ((q[0] - a[0]) * -u[1] + (q[1] - a[1]) * u[0]).abs();
                    if s > FENCE_KEEP && s < length - FENCE_KEEP && off <= t.width / 2.0 + 1e-6 {
                        tees.push(s);
                    }
                }
            }
            let (start, end) = (st.near(a), st.near(b));
            for side in [1.0, -1.0] {
                sides.push(Side {
                    track: k,
                    walk,
                    length,
                    offset,
                    side,
                    keep: (pitch / 2.0).min(offset * 0.9),
                    start,
                    end,
                    tees: tees.clone(),
                    fixed: Vec::new(),
                });
            }
        }
    }
    if sides.len().saturating_mul(2) > MAX_SPOTS {
        return Err(format!("these tracks have more segments than one fence takes ({}): fence fewer tracks", MAX_SPOTS / 2));
    }
    // Where each track's offset comes from.
    let range = |(lo, hi): (f64, f64)| if (hi - lo).abs() < 5e-4 { format!("{} mm", mm(lo)) } else { format!("{} to {} mm", mm(lo), mm(hi)) };
    if lines.len() == 1
        && let Some((lo, hi)) = lines.values().next()
        && (hi - lo).abs() < 5e-4
    {
        out.offset = Some(*lo);
    }
    for (line, span) in &lines {
        let o = range(*span);
        out.why.push(match line {
            Line::Asked => format!("offset {o} from the centreline, as asked"),
            Line::Coplanar => format!(
                "offset {o} from the centreline: half the track, the clearance to {} and the via's radius, so the vias stand on the {} copper beside the gap",
                ask.net, ask.net
            ),
            Line::Stripline => format!("offset {o} on inner layers: at least {STRIPLINE_H} × the dielectric height, so the fence doesn't change the stripline's impedance"),
            Line::Microstrip => format!(
                "offset {o} beside lines with no {} copper next to them on their layer: at least {MICROSTRIP_W} × the width and {MICROSTRIP_H} × the dielectric height, so the vias don't load the line (a {} pour beside it makes a coplanar line they tie down)",
                ask.net, ask.net
            ),
        });
    }
    out.why.push(format!(
        "both sides of each segment: a via beside every bend, tee, pad of the line and open end (within {} mm), then the runs between at no more than the pitch",
        mm(FENCE_KEEP)
    ));
    let mut near = Near::new(net_vias(board, nets, net, site.span));
    let mut fenced_tracks: BTreeSet<usize> = BTreeSet::new();
    let steps = (FENCE_KEEP / KEEP_STEP).round() as usize;
    // First a via beside each stop, walking in from it until one fits.
    for side in sides.iter_mut() {
        let mut anchors: Vec<Vec<f64>> = Vec::new();
        if side.start {
            anchors.push((0..=steps).map(|k| k as f64 * KEEP_STEP).filter(|s| *s <= side.length).collect());
        }
        if side.end {
            anchors.push((0..=steps).map(|k| side.length - k as f64 * KEEP_STEP).filter(|s| *s >= 0.0).collect());
        }
        for t in &side.tees {
            anchors.push(std::iter::once(*t).chain((1..=steps).flat_map(|k| [*t + k as f64 * KEEP_STEP, *t - k as f64 * KEEP_STEP])).collect());
        }
        for along in anchors {
            let Some(meant) = along.first().copied() else { continue };
            match fence_spot(&mut site, &mut near, &mut out, side, &along, area) {
                Ok(Some(s)) => {
                    fenced_tracks.insert(side.track);
                    side.fixed.push(s);
                }
                Ok(None) => return Err(TOO_MUCH.into()),
                // A via of the net is near enough already: it fences the line there.
                Err(None) => {
                    out.skip("near_via");
                    fenced_tracks.insert(side.track);
                    side.fixed.push(meant);
                }
                Err(Some(r)) => {
                    out.skip(r);
                    side.fixed.push(meant);
                }
            }
        }
        if out.spots.len() > MAX_STITCH {
            return Err(format!("the fence would place more than {MAX_STITCH} vias: give a wider pitch"));
        }
    }
    // Then the runs between, at no more than the pitch, each spot tried a little along the line
    // when it fails.
    let shift = ((pitch / 4.0) / KEEP_STEP).floor().clamp(0.0, 10.0) as usize;
    for side in sides.iter_mut() {
        let mut marks: Vec<f64> = side.fixed.clone();
        let mut regular: Vec<f64> = Vec::new();
        if !side.start {
            regular.push(0.0);
        }
        if !side.end {
            regular.push(side.length);
        }
        marks.extend([0.0, side.length]);
        marks.sort_by(f64::total_cmp);
        marks.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        for w in marks.windows(2) {
            let [a, b] = [w.first().copied().unwrap_or(0.0), w.get(1).copied().unwrap_or(0.0)];
            // Next to a bend the via beside it may be the other segment's, round the outside of
            // the corner: as the crow flies it stands up to an offset farther off, so these runs
            // close up by that much.
            let at_stop = (side.start && a <= FENCE_KEEP + 1e-9) || (side.end && b >= side.length - FENCE_KEEP - 1e-9);
            let step = if at_stop { (pitch - side.offset).max(pitch / 2.0) } else { pitch };
            let n = ((b - a) / step - 1e-9).ceil().max(1.0) as usize - 1;
            regular.extend((1..=n).map(|k| a + (b - a) * k as f64 / (n + 1) as f64));
        }
        for s in regular {
            let along: Vec<f64> = std::iter::once(s)
                .chain((1..=shift).flat_map(|k| [s + k as f64 * KEEP_STEP, s - k as f64 * KEEP_STEP]))
                .filter(|q| (0.0..=side.length).contains(q))
                .collect();
            match fence_spot(&mut site, &mut near, &mut out, side, &along, area) {
                Ok(Some(_)) => {
                    fenced_tracks.insert(side.track);
                }
                Ok(None) => return Err(TOO_MUCH.into()),
                Err(None) => {
                    out.skip("near_via");
                    fenced_tracks.insert(side.track);
                }
                Err(Some(r)) => out.skip(r),
            }
        }
        if out.spots.len() > MAX_STITCH {
            return Err(format!("the fence would place more than {MAX_STITCH} vias: give a wider pitch"));
        }
    }
    out.unfenced =
        picked.iter().enumerate().filter(|(k, _)| !fenced_tracks.contains(k)).filter_map(|(_, i)| board.tracks.get(*i)).map(|t| t.id).collect();
    if !out.unfenced.is_empty() {
        out.why.push(format!("{} of the {} tracks got no via (see `unfenced` and `skipped`)", out.unfenced.len(), picked.len()));
    }
    Ok(out)
}

/// The nets a layer change looks at by default (see the module notes), and how they were chosen.
fn signal_nets(ecad: &Electronics, nets: &BoardNets, net: usize) -> (BTreeSet<usize>, String) {
    let index: BTreeMap<&str, usize> = nets.names.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
    let index = |name: &str| index.get(name).copied();
    let fast = |r: &super::intent::NetIntent| {
        matches!(r.role, NetRole::Rf | NetRole::Clock | NetRole::Fast | NetRole::Diff | NetRole::Strobe | NetRole::SwNode)
            || r.edge_ns.is_some_and(|e| e <= FAST_EDGE_NS)
    };
    let by_intent: BTreeSet<usize> = ecad.intent.nets.iter().filter(|r| fast(r)).filter_map(|r| index(&r.net)).filter(|n| *n != net).collect();
    if !by_intent.is_empty() {
        return (by_intent, "the RF, clock and fast nets the intent names".into());
    }
    let planes: BTreeSet<usize> = nets.fills.iter().filter_map(|f| f.net).collect();
    let supplies: BTreeSet<usize> =
        ecad.intent.nets.iter().filter(|r| matches!(r.role, NetRole::Power | NetRole::Ground)).filter_map(|r| index(&r.net)).collect();
    let all = nets.via_nets.iter().flatten().copied().filter(|n| *n != net && !planes.contains(n) && !supplies.contains(n)).collect();
    (all, "every net with no pour (the intent names no RF, clock or fast nets)".into())
}

/// One signal via (or stack of them) where its net changes layer.
struct Change {
    at: [f64; 2],
    radius: f64,
    net: Option<usize>,
    /// The layers its net's copper meets it on (stack indexes).
    layers: BTreeSet<usize>,
}

fn layer_change(ecad: &Electronics, nets: &BoardNets, ask: &Ask, net: usize, mut site: Site, area: Option<&Within>) -> Result<Plan, String> {
    let board = &ecad.board;
    let n = board.copper_layers;
    let mut out = Plan::default();
    let (signals, chosen) =
        if ask.nets.is_empty() { signal_nets(ecad, nets, net) } else { (net_indexes(nets, &ask.nets)?, "the nets asked for".into()) };
    if signals.contains(&net) {
        return Err(format!("{} is the net the vias are on, not a signal", ask.net));
    }
    // The signal vias, grouped where their rings meet (a stack of micro vias is one change).
    let vias: Vec<usize> = (0..board.vias.len()).filter(|i| nets.via_nets.get(*i).copied().flatten().is_some_and(|x| signals.contains(&x))).collect();
    let mut group: Vec<usize> = (0..vias.len()).collect();
    fn root(g: &mut [usize], mut i: usize) -> usize {
        while let Some(&p) = g.get(i) {
            if p == i {
                break;
            }
            let up = g.get(p).copied().unwrap_or(p);
            if let Some(slot) = g.get_mut(i) {
                *slot = up;
            }
            i = p;
        }
        i
    }
    let mut grid = Grid::new(CELL);
    for (k, i) in vias.iter().enumerate() {
        if let Some(v) = board.vias.get(*i) {
            let r = v.diameter / 2.0;
            grid.insert(k, [[v.at[0] - r, v.at[1] - r], [v.at[0] + r, v.at[1] + r]]);
        }
    }
    for (k, i) in vias.iter().enumerate() {
        let (Some(v), vn) = (board.vias.get(*i), nets.via_nets.get(*i).copied().flatten()) else { continue };
        let r = v.diameter / 2.0;
        for m in grid.near_box([[v.at[0] - r, v.at[1] - r], [v.at[0] + r, v.at[1] + r]], vias.len()) {
            let Some(j) = vias.get(m).copied().filter(|_| m != k) else { continue };
            let Some(w) = board.vias.get(j) else { continue };
            if nets.via_nets.get(j).copied().flatten() == vn && (v.at[0] - w.at[0]).hypot(v.at[1] - w.at[1]) <= r + w.diameter / 2.0 {
                let (a, b) = (root(&mut group, k), root(&mut group, m));
                if let Some(slot) = group.get_mut(a) {
                    *slot = b;
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for k in 0..vias.len() {
        let g = root(&mut group, k);
        if let Some(i) = vias.get(k) {
            groups.entry(g).or_default().push(*i);
        }
    }
    let groups: Vec<Vec<usize>> = groups.into_values().collect();
    // The signal nets' track segments and pads, in grids.
    let segments: Vec<(usize, [f64; 2], [f64; 2], f64)> = board
        .tracks
        .iter()
        .enumerate()
        .filter(|(i, _)| nets.track_nets.get(*i).copied().flatten().is_some_and(|x| signals.contains(&x)))
        .flat_map(|(i, t)| path::segments(&t.points, &t.curves, false).map(move |(a, b, s)| (i, a, b, s)))
        .collect();
    let mut seg_grid = Grid::new(CELL);
    for (k, (i, a, b, s)) in segments.iter().enumerate() {
        let w = board.tracks.get(*i).map_or(0.0, |t| t.width / 2.0);
        seg_grid.insert(k, grow(path::segment_bbox(*a, *b, *s), w));
    }
    let pads: Vec<&BoardPad> = nets.pads.iter().filter(|p| p.net.is_some_and(|x| signals.contains(&x)) && (p.top || p.bottom)).collect();
    let mut pad_grid = Grid::new(CELL);
    for (k, p) in pads.iter().enumerate() {
        pad_grid.insert(k, p.bbox());
    }
    if !site.charge(segments.len().saturating_add(pads.len()).saturating_add(vias.len())) {
        return Err(TOO_MUCH.into());
    }
    // Where each change is, and the layers its signal runs on there.
    let mut changes: Vec<Change> = Vec::new();
    for g in &groups {
        let Some(first) = g.first().and_then(|i| board.vias.get(*i)) else { continue };
        let mut layers: BTreeSet<usize> = BTreeSet::new();
        let mut vn = None;
        for i in g {
            let Some(v) = board.vias.get(*i) else { continue };
            vn = nets.via_nets.get(*i).copied().flatten();
            let r = v.diameter / 2.0;
            let around = grow([v.at, v.at], r);
            let near = seg_grid.near_box(around, segments.len());
            if !site.charge(near.len()) {
                return Err(TOO_MUCH.into());
            }
            for k in near {
                let Some((ti, a, b, s)) = segments.get(k) else { continue };
                let Some(t) = board.tracks.get(*ti).filter(|t| nets.track_nets.get(*ti).copied().flatten() == vn && v.on(t.layer, n)) else {
                    continue;
                };
                if path::segment_distance(v.at, *a, *b, *s) <= t.width / 2.0 + r {
                    layers.insert(t.layer.index(n));
                }
            }
            for k in pad_grid.near_box(around, pads.len()) {
                let Some(p) = pads.get(k).filter(|p| p.net == vn) else { continue };
                if gap(&disc(v.at, r), &pad_core(p)) <= 1e-9 {
                    layers.extend(board.stack().into_iter().filter(|l| p.on(*l) && v.on(*l, n)).map(|l| l.index(n)));
                }
            }
        }
        if layers.len() >= 2 {
            let radius = g.iter().filter_map(|i| board.vias.get(*i)).map(|v| v.diameter / 2.0).fold(0.0, f64::max);
            changes.push(Change { at: first.at, radius, net: vn, layers });
        }
    }
    // Which plane fills (of any net) are round each change: the via clears a hole in each, so
    // they are looked for on a ring just past that hole.
    let mut points: Vec<[f64; 2]> = Vec::with_capacity(changes.len() * PLANE_PROBES);
    for c in &changes {
        let reach = c.radius + site.max_gap + PLANE_MARGIN;
        for k in 0..PLANE_PROBES {
            let a = std::f64::consts::TAU * k as f64 / PLANE_PROBES as f64;
            points.push([c.at[0] + reach * a.cos(), c.at[1] + reach * a.sin()]);
        }
    }
    let planed: Vec<&PourFill> = nets.fills.iter().filter(|f| f.net.is_some()).collect();
    if !site.charge(points.len().saturating_mul(planed.len()).saturating_add(LOCATE_COST * planed.len())) {
        return Err(TOO_MUCH.into());
    }
    let mut planes: Vec<Vec<(usize, usize)>> = vec![Vec::new(); changes.len()];
    for f in planed {
        let mut hits = vec![0usize; changes.len()];
        for (k, hit) in f.region.locate(&points).into_iter().enumerate() {
            if let (Some(_), Some(slot)) = (hit, hits.get_mut(k / PLANE_PROBES)) {
                *slot += 1;
            }
        }
        for (slot, h) in planes.iter_mut().zip(hits) {
            if let (true, Some(fnet)) = (h >= PLANE_HITS, f.net) {
                slot.push((f.layer.index(n), fnet));
            }
        }
    }
    // An RF net's change wants two vias within 1 mm (or λg/20, if less), on opposite sides.
    let rf: BTreeSet<usize> =
        ecad.intent.nets.iter().filter(|r| r.role == NetRole::Rf).filter_map(|r| nets.names.iter().position(|m| *m == r.net)).collect();
    let er = board_er(board);
    let span = site.span;
    let mut ground: Vec<([f64; 2], (usize, usize))> =
        board.vias.iter().zip(&nets.via_nets).filter(|(_, x)| **x == Some(net)).map(|(v, _)| (v.at, v.span(n))).collect();
    let last = usize::from(n).saturating_sub(1);
    ground.extend(nets.pads.iter().filter(|p| p.net == Some(net) && p.top && p.bottom).map(|p| (p.at(), (0, last))));
    let (mut same, mut had, mut held, mut rf_changes) = (0usize, 0usize, 0usize, 0usize);
    for (k, c) in changes.iter().enumerate() {
        if let Some(a) = area {
            if !site.charge(a.cost(1)) {
                return Err(TOO_MUCH.into());
            }
            if !a.holds(&[c.at]).first().copied().unwrap_or(false) {
                continue;
            }
        }
        let here = planes.get(k).cloned().unwrap_or_default();
        // Each layer's reference: the nearest other layer with a plane here, the net's first.
        let mut refs: BTreeSet<(usize, usize)> = BTreeSet::new();
        for l in &c.layers {
            for d in 1..=usize::from(n) {
                let mut found: Vec<(usize, usize)> = here.iter().copied().filter(|(m, _)| m.abs_diff(*l) == d).collect();
                if found.is_empty() {
                    continue;
                }
                found.sort_by_key(|(m, x)| (*x != net, *m));
                refs.extend(found.first().copied());
                break;
            }
        }
        let planes_at: BTreeSet<usize> = refs.iter().map(|(m, _)| *m).collect();
        if planes_at.len() < 2 {
            same += 1;
            continue;
        }
        held += 1;
        if refs.iter().any(|(_, x)| *x != net) {
            out.skip("other_reference");
            continue;
        }
        let (lo, hi) = (planes_at.first().copied().unwrap_or(0), planes_at.last().copied().unwrap_or(0));
        if !(span.0 <= lo && hi <= span.1) {
            out.skip("short_span");
            continue;
        }
        let is_rf = c.net.is_some_and(|x| rf.contains(&x));
        let (reach, need) = if is_rf {
            rf_changes += 1;
            let name: BTreeSet<String> = c.net.and_then(|x| nets.name(x)).map(str::to_string).into_iter().collect();
            let twentieth = fence_frequency(ecad, &name).map_or(f64::INFINITY, |(f, _)| wavelength(f, er) / 20.0);
            (CHANGE_NEAR.min(twentieth), RF_CHANGE_VIAS)
        } else {
            (CHANGE_REACH, 1)
        };
        let close = |q: [f64; 2]| (q[0] - c.at[0]).hypot(q[1] - c.at[1]) <= reach + 1e-9;
        let mut beside: Vec<[f64; 2]> = ground.iter().filter(|(q, (a, b))| close(*q) && *a <= lo && hi <= *b).map(|(q, _)| *q).collect();
        if beside.len() >= need {
            had += 1;
            continue;
        }
        let distances: Vec<f64> = if is_rf {
            let mut d: Vec<f64> = (0..9).map(|k| 1.0 - 0.05 * k as f64).filter(|d| *d <= reach + 1e-9).collect();
            if d.is_empty() {
                d.push(reach);
            }
            d
        } else {
            CHANGE_DISTANCES.to_vec()
        };
        let mut tried: Vec<[f64; 2]> = Vec::new();
        for d in &distances {
            for t in 0..CHANGE_TURNS {
                let a = std::f64::consts::TAU * t as f64 / CHANGE_TURNS as f64;
                tried.push(snap_point([c.at[0] + d * a.cos(), c.at[1] + d * a.sin()]));
            }
        }
        let looks = site.look(&tried);
        out.looked_at += tried.len();
        while beside.len() < need {
            // The second on the side away from the first.
            let mut order: Vec<usize> = (0..tried.len()).collect();
            if let Some(first) = beside.first().and_then(|q| unit(c.at, *q)) {
                let facing = |i: &usize| tried.get(*i).and_then(|q| unit(c.at, *q)).map_or(1.0, |u| u[0] * first[0] + u[1] * first[1]);
                order.sort_by(|a, b| facing(a).total_cmp(&facing(b)));
            }
            let mut placed = None;
            for i in order {
                let (Some(p), Some(look)) = (tried.get(i).copied(), looks.get(i).copied()) else { continue };
                if try_spot(&mut site, p, look).is_err() {
                    if site.spent() {
                        return Err(TOO_MUCH.into());
                    }
                    continue;
                }
                placed = Some(p);
                break;
            }
            let Some(p) = placed else {
                out.skip("no_room");
                break;
            };
            site.add(p);
            ground.push((p, span));
            beside.push(p);
            out.spots.push(p);
            if out.spots.len() > MAX_STITCH {
                return Err(format!("more than {MAX_STITCH} layer changes need a via: name fewer `nets`"));
            }
        }
    }
    out.why.push(format!(
        "{held} signal vias of {chosen} change reference plane ({same} keep theirs); {had} have a {} via joining both planes near enough already; the rest get one within {} mm ({} mm away preferred)",
        ask.net,
        mm(CHANGE_REACH),
        mm(CHANGE_NEAR)
    ));
    if rf_changes > 0 {
        out.why.push(format!(
            "{rf_changes} of them are on RF nets: those get {RF_CHANGE_VIAS} vias on opposite sides within {} mm (or λg/20, if less)",
            mm(CHANGE_NEAR)
        ));
    }
    out.why.push(format!("{} vias in all", out.spots.len()));
    Ok(out)
}
