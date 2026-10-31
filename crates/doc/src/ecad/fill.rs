//! A copper pour's fill: worked out from the board each time, never saved (the pours themselves
//! are in [`super::pour`]). Pours fill highest priority first (the older first among equals), each
//! one:
//! 1. its outline, cut to the board's outline shrunk by the rules' `copper_to_edge`;
//! 2. less the copper of other nets on its layer, each piece grown by the clearance between the
//!    two (the larger of the pour's own clearance, which takes the place of its net's class
//!    clearance when set, and the other net's class clearance): pads, tracks, vias whose span has
//!    the layer, copper drawings, and the fill of each pour of another net filled before it on the
//!    layer (the larger of the two pours' clearances); less unplated holes and pad holes grown by
//!    `copper_to_edge`, and the keepouts on the layer that forbid pours. Copper on no net counts
//!    as another net's;
//! 3. with a thermal connection, less a gap round each pad of its net, with the pad's spokes
//!    added back (2 along the pad's width, or 4), each kept only where it reaches the fill (the
//!    pad joins the fill through them); vias of its net join solid whatever the connection, and
//!    with `none` its pads are cleared like another net's. A footprint's own connection, or one
//!    pad's, takes the place of the pour's for its pads (see
//!    [`BoardFootprint::connect_for`](super::board::BoardFootprint::connect_for)); a solidly joined
//!    pad's fill is looked for all round it as far out as a thermal pad's spokes reach it (past
//!    the gap), so a pad the fill can't reach is known as a thermal one is (the bit of fill over
//!    the pad itself, and right round it, doesn't count: it may be all there is);
//! 4. opened by half its thinnest width (shrunk, then grown back), so necks and slivers narrower
//!    than `min_width` go; hatched when asked (lines both ways, and a border as wide), the
//!    copper of its net's solidly joined pads and of its vias kept under the hatch;
//! 5. less the islands that touch no copper of its net (pad centres, points in thermal spokes,
//!    via centres, points along tracks, or the fill of a pour of its net filled before it that
//!    they overlap), unless the pour keeps them or has no net.
//!
//! What each island touches it joins, for the airwires (see [`super::airwires`]), overlapping
//! islands of an earlier pour of its net included.
//!
//! The work is bounded: a pour whose outline and pieces have more than [`MAX_CORNERS`] corners, or
//! overlap more than [`MAX_PAIRS`] times (pairs of pieces, or of the outline's edges, in one cell
//! of a grid: what the booleans' crossings grow with), is left unfilled and `truncated`, as is
//! every pour once the board's [`MAX_BOARD_CORNERS`] or [`MAX_BOARD_PAIRS`] are spent. The
//! earlier fills a pour keeps clear of count among its corners; what it opens, the hatch's
//! openings and the overlap tests with earlier pours of its net spend the board's corners too.
//! A hatch of more than [`MAX_HATCH_CELLS`] openings, or past the budget, is filled solid and
//! `truncated`.

use solidcraft_geom::region::{Region, Shape};

use std::collections::BTreeMap;

use super::airwires::BoardPad;
use super::board::{Board, BoardFootprint};
use super::copper::{TOLERANCE, core_bbox, disc, drill_core, graphic_cores, hull_outline, pad_core, pieces, reach, track_cores};
use super::footprint::PadKind;
use super::grid::{Grid, boxes_meet, grow};
use super::layers::{CopperLayer, DrawLayer};
use super::path;
use super::pour::{Connect, Hatch, Pour};
use super::rules::Sizes;

/// The most corners one pour's outline and pieces have.
pub const MAX_CORNERS: usize = 1 << 21;
/// The most pairs of pieces (or outline edges) of one pour that share a cell of the grid.
pub const MAX_PAIRS: usize = 1 << 20;
/// The most corners, and pairs, all the pours have together.
pub const MAX_BOARD_CORNERS: usize = 1 << 23;
pub const MAX_BOARD_PAIRS: usize = 1 << 22;
/// The most openings a hatch has.
pub const MAX_HATCH_CELLS: f64 = 250_000.0;
/// The most points along tracks a pour looks for in its islands.
const MAX_PROBES: usize = 1 << 18;
/// The arcs of the offsets, in degrees a piece.
const OFFSET_STEP: f64 = 5.0;
/// The grid's cell, in millimetres.
const CELL: f64 = 1.0;
/// How far apart, in millimetres, the points along a track an island is looked for at are.
const TRACK_STEP: f64 = 0.5;

/// Copper a pour's island touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Node {
    /// By index into the pads, tracks or vias (board order).
    Pad(usize),
    Track(usize),
    Via(usize),
    /// An island of another pour's fill of the same net it overlaps: the pour's id and the
    /// island's index into that fill's shapes.
    Fill(u64, usize),
}

/// One pour's fill (see the module notes).
#[derive(Clone, Debug, PartialEq)]
pub struct PourFill {
    /// The pour's id.
    pub pour: u64,
    pub layer: CopperLayer,
    /// The pour's net, an index into the net names; `None` without one (or for a name the
    /// schematic has no net for).
    pub net: Option<usize>,
    /// The copper, as disjoint shapes with holes.
    pub region: Region,
    /// How many islands went for touching no copper of the net.
    pub islands_removed: usize,
    /// It was past a budget (see the module notes).
    pub truncated: bool,
    /// What each island (by index into the region's shapes) touches.
    pub joins: Vec<(usize, Node)>,
    /// The gap it keeps to other nets' copper (its own, or its net class's).
    pub clearance: f64,
    /// Its connections to the pads of its net: each pad it relieves, and how many of the pad's
    /// spokes reach the fill; each pad it joins solid, and whether the fill reaches it.
    pub reliefs: Vec<Relief>,
}

/// A pad a pour joins through thermal spokes, or solid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Relief {
    /// By index into the pads.
    pub pad: usize,
    pub requested: u8,
    pub connected: u8,
    /// The pad's centre is inside the pour's outline (on the board): a pad outside it isn't
    /// meant to join.
    pub inside: bool,
    pub spoke_width: f64,
    /// The pad joins solid: `requested` points all round it, where a thermal pad's spokes would
    /// reach the fill, were looked at, and the fill holds `connected` of them.
    pub solid: bool,
}

/// How many points round a solidly joined pad the fill is looked for at.
const SOLID_PROBES: u8 = 16;

/// Fill corners closer than this to the one before, in millimetres, are left out of
/// [`PourFill::shapes`].
pub const SHAPE_MIN_EDGE: f64 = 0.001;

impl PourFill {
    /// Its copper as polygons with holes, for drawing and the checks (corners closer together
    /// than [`SHAPE_MIN_EDGE`] thinned out).
    pub fn shapes(&self) -> Vec<Shape> {
        self.region.shapes().iter().map(|s| s.simplified(SHAPE_MIN_EDGE)).collect()
    }

    /// Its copper's area, in square millimetres.
    pub fn area(&self) -> f64 {
        self.region.area()
    }
}

/// What the fill needs to know of the board's copper: the net names, the pads with their nets,
/// and each track's and via's net before the pours join anything.
pub struct CopperNets<'a> {
    pub names: &'a [String],
    pub pads: &'a [BoardPad],
    pub track_nets: &'a [Option<usize>],
    pub via_nets: &'a [Option<usize>],
}

/// The pieces a pour keeps clear of.
struct Pieces {
    polygons: Vec<Vec<[f64; 2]>>,
    grid: Grid,
    corners: usize,
}

impl Pieces {
    /// A ring's edges, counted for the work but not kept clear of.
    fn edges(&mut self, ring: &[[f64; 2]]) {
        let n = ring.len();
        for i in 0..n {
            if let (Some(a), Some(b)) = (ring.get(i), ring.get((i + 1) % n)) {
                self.grid.insert(usize::MAX - i, [[a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]]);
            }
        }
        self.corners = self.corners.saturating_add(n);
    }

    fn add(&mut self, polygon: Vec<[f64; 2]>) {
        if polygon.len() < 3 {
            return;
        }
        let bbox = super::bbox(polygon.iter().copied()).unwrap_or_default();
        self.grid.insert(self.polygons.len(), bbox);
        self.corners = self.corners.saturating_add(polygon.len());
        self.polygons.push(polygon);
    }
}

/// The board's area copper may fill: its outline shrunk by `copper_to_edge`; `None` without an
/// outline.
fn board_area(board: &Board) -> Option<Region> {
    if board.outline.is_empty() {
        return None;
    }
    let ring = path::tessellate(&board.outline, &board.outline_curves, true, TOLERANCE);
    let edge = board.rules.copper_to_edge;
    Some(Region::polygon(&ring).offset(-edge, OFFSET_STEP))
}

/// Every pour's fill, in the board's order of pours (see the module notes).
pub fn pour_fills(board: &Board, c: &CopperNets) -> Vec<PourFill> {
    if board.pours.is_empty() {
        return Vec::new();
    }
    let edge = board_area(board);
    let sizes = board.rules.sizes_by_net(c.names);
    let mut order: Vec<&Pour> = board.pours.iter().collect();
    order.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.id.cmp(&b.id)));
    let mut done: Vec<PourFill> = Vec::with_capacity(order.len());
    let mut budget = Budget { corners: MAX_BOARD_CORNERS, pairs: MAX_BOARD_PAIRS };
    for pour in order {
        let fill = fill_one(board, c, &sizes, pour, &done, edge.as_ref(), &mut budget);
        done.push(fill);
    }
    let place = |f: &PourFill| board.pours.iter().position(|p| p.id == f.pour).unwrap_or(usize::MAX);
    done.sort_by_key(place);
    done
}

/// What is left of the board's work for the pours.
struct Budget {
    corners: usize,
    pairs: usize,
}

impl Budget {
    /// Spend `corners`; false (and nothing spent) past what is left.
    fn spend(&mut self, corners: usize) -> bool {
        if corners > self.corners {
            return false;
        }
        self.corners -= corners;
        true
    }
}

/// What all of `regions` cover, joined two by two in rounds (each corner is merged a few times,
/// not once for every region).
fn union_all(mut regions: Vec<Region>) -> Region {
    while regions.len() > 1 {
        let mut next = Vec::with_capacity(regions.len().div_ceil(2));
        let mut it = regions.into_iter();
        while let Some(a) = it.next() {
            next.push(match it.next() {
                Some(b) => a.union(&b),
                None => a,
            });
        }
        regions = next;
    }
    regions.pop().unwrap_or_default()
}

/// A rectangle `width` wide along `d` (a unit vector) from `a`, `length` long.
fn bar(a: [f64; 2], d: [f64; 2], length: f64, width: f64) -> Vec<[f64; 2]> {
    let (hx, hy) = (-d[1] * width / 2.0, d[0] * width / 2.0);
    let b = [a[0] + d[0] * length, a[1] + d[1] * length];
    vec![[a[0] - hx, a[1] - hy], [b[0] - hx, b[1] - hy], [b[0] + hx, b[1] + hy], [a[0] + hx, a[1] + hy]]
}

fn fill_one(board: &Board, c: &CopperNets, sizes: &[Sizes], pour: &Pour, done: &[PourFill], edge: Option<&Region>, budget: &mut Budget) -> PourFill {
    let rules = &board.rules;
    let class_gap = |n: Option<usize>| n.and_then(|n| sizes.get(n)).map_or(rules.clearance, |s| s.clearance);
    let net = pour.net.as_deref().and_then(|n| c.names.iter().position(|m| m == n));
    let layer = pour.layer;
    let layers = board.copper_layers;
    let own = pour.clearance.unwrap_or_else(|| class_gap(net));
    let mut out = PourFill {
        pour: pour.id,
        layer,
        net,
        region: Region::new(),
        islands_removed: 0,
        truncated: false,
        joins: Vec::new(),
        clearance: own,
        reliefs: Vec::new(),
    };
    let ring = path::tessellate(&pour.outline, &pour.curves, true, TOLERANCE);
    // An outline that crosses itself a lot costs as much as many pieces: it is counted first.
    let mut outline_work = Pieces { polygons: Vec::new(), grid: Grid::new(CELL), corners: 0 };
    outline_work.edges(&ring);
    let outline_pairs = outline_work.grid.pairs();
    if outline_pairs > MAX_PAIRS.min(budget.pairs) || ring.len() > MAX_CORNERS.min(budget.corners) {
        out.truncated = true;
        return out;
    }
    let mut area = Region::polygon(&ring);
    if let Some(edge) = edge {
        area = area.intersection(edge);
    }
    let Some(bbox) = area.bbox() else { return out };
    let gap_to = |other: Option<usize>| own.max(class_gap(other));
    let same = |n: Option<usize>| net.is_some() && n == net;
    let near = |core_box: [[f64; 2]; 2], g: f64| boxes_meet(grow(core_box, g), bbox);
    // Each pad's connection: its own or its footprint's, else the pour's.
    let overrides: BTreeMap<u64, &BoardFootprint> = board.footprints.iter().filter(|f| f.overrides_pours()).map(|f| (f.id, f)).collect();
    let connect_of = |p: &BoardPad| overrides.get(&p.footprint).map_or(pour.connect, |f| f.connect_for(&p.pad.number, pour.connect));
    let t = pour.thermal;
    let mut blockers = Pieces { polygons: Vec::new(), grid: Grid::new(CELL), corners: 0 };
    // The pads of its net it joins, near it: through thermal spokes or solid.
    let mut own_pads: Vec<(usize, Connect)> = Vec::new();
    for (i, p) in c.pads.iter().enumerate() {
        if p.pad.kind == PadKind::Hole {
            if let Some(d) = drill_core(p)
                && near(core_bbox(&d), rules.copper_to_edge)
            {
                blockers.add(hull_outline(&d, rules.copper_to_edge));
            }
            continue;
        }
        if !p.on(layer) {
            continue;
        }
        let core = pad_core(p);
        let joins = connect_of(p);
        if same(p.net) && joins != Connect::None {
            if near(core_bbox(&core), t.gap) {
                own_pads.push((i, joins));
            }
            continue;
        }
        let g = gap_to(p.net);
        if near(core_bbox(&core), g) {
            blockers.add(hull_outline(&core, g));
        }
    }
    for (i, track) in board.tracks.iter().enumerate().filter(|(_, t)| t.layer == layer) {
        let n = c.track_nets.get(i).copied().flatten();
        if same(n) {
            continue;
        }
        let g = gap_to(n);
        for core in track_cores(track) {
            if near(core_bbox(&core), g) {
                blockers.add(hull_outline(&core, g));
            }
        }
    }
    for (i, v) in board.vias.iter().enumerate().filter(|(_, v)| v.on(layer, layers)) {
        let n = c.via_nets.get(i).copied().flatten();
        if same(n) {
            continue;
        }
        let (core, g) = (disc(v.at, v.diameter / 2.0), gap_to(n));
        if near(core_bbox(&core), g) {
            blockers.add(hull_outline(&core, g));
        }
    }
    for h in &board.holes {
        let (core, g) = (disc(h.at, h.drill / 2.0), rules.copper_to_edge);
        if near(core_bbox(&core), g) {
            blockers.add(hull_outline(&core, g));
        }
    }
    for g in board.graphics.iter().filter(|g| g.layer == DrawLayer::Copper(layer)) {
        let gap = gap_to(None);
        for core in graphic_cores(&g.graphic, &|p| p) {
            if near(core_bbox(&core), gap) {
                blockers.add(hull_outline(&core, gap));
            }
        }
    }
    for k in board.keepouts.iter().filter(|k| k.forbid.pours && k.on(layer)) {
        blockers.add(path::tessellate(&k.outline, &k.curves, true, TOLERANCE));
    }
    // The fills of other nets before it on the layer, kept clear by the larger of the two pours'
    // clearances; each counts in full (cutting it to this pour's box walks all of it).
    let mut earlier: Vec<(&PourFill, f64)> = Vec::new();
    let mut corners = blockers.corners.saturating_add(ring.len());
    for f in done.iter().filter(|f| f.layer == layer && !(net.is_some() && f.net == net)) {
        let g = own.max(f.clearance);
        if f.region.bbox().is_some_and(|b| boxes_meet(grow(b, g), bbox)) {
            corners = corners.saturating_add(f.region.corners());
            earlier.push((f, g));
        }
    }
    let pairs = blockers.grid.pairs().saturating_add(outline_pairs);
    if corners > MAX_CORNERS.min(budget.corners) || pairs > MAX_PAIRS.min(budget.pairs) {
        out.truncated = true;
        return out;
    }
    budget.corners -= corners;
    budget.pairs -= pairs;
    // Of an earlier fill, only what its gap can carry into this pour's box: the cut's own edges
    // then grow away from the box.
    let mut regions = vec![Region::polygons(blockers.polygons.iter().map(Vec::as_slice))];
    for (f, g) in earlier {
        let [lo, hi] = grow(bbox, g + 0.1);
        regions.push(f.region.intersection(&Region::rect(lo, hi)).offset(g, OFFSET_STEP));
    }
    let blocked = union_all(regions);
    let base = area.difference(&blocked);
    let joined = |how: Connect| -> Vec<_> {
        own_pads.iter().filter(|(_, j)| *j == how).filter_map(|(i, _)| Some((*i, c.pads.get(*i)?))).map(|(i, p)| (i, p, pad_core(p))).collect()
    };
    let cores = if net.is_some() { joined(Connect::Thermal) } else { Vec::new() };
    let solid_cores = if net.is_some() { joined(Connect::Solid) } else { Vec::new() };
    let cut = if !cores.is_empty() {
        let rings: Vec<Vec<[f64; 2]>> = cores.iter().map(|(.., core)| hull_outline(core, t.gap)).collect();
        base.difference(&Region::polygons(rings.iter().map(Vec::as_slice)))
    } else {
        base.clone()
    };
    // Opening it and hatching it walk it a few times.
    if !budget.spend(cut.corners().saturating_mul(3)) {
        out.truncated = true;
        return out;
    }
    let half = pour.min_width / 2.0;
    let opened = if half > 0.0 { cut.offset(-half, OFFSET_STEP).offset(half, OFFSET_STEP).intersection(&cut) } else { cut };
    // A point inside each kept spoke, with its pad (an index into the pads): the pad's centre is
    // on the end of its spokes, the edge of the copper there, so the pad joins through these.
    let mut spoke_at: Vec<(usize, [f64; 2])> = Vec::new();
    let spokes = if !cores.is_empty() {
        let mut bars = Vec::new();
        let mut probes = Vec::new();
        let mut owners = Vec::new();
        let turn = 360.0 / f64::from(t.spokes.max(1));
        for (i, p, core) in &cores {
            let centre = p.at();
            for k in 0..t.spokes {
                let a = (p.pad.rotation + turn * f64::from(k)).to_radians();
                let d = p.placement.vector([a.cos(), a.sin()]);
                let r = reach(core, centre, d);
                probes.push([centre[0] + d[0] * (r + t.gap + half.max(5.0 * TOLERANCE)), centre[1] + d[1] * (r + t.gap + half.max(5.0 * TOLERANCE))]);
                owners.push(*i);
                bars.push(bar(centre, d, r + t.gap + pour.min_width.max(t.spoke_width) + t.spoke_width, t.spoke_width));
            }
        }
        let reached = opened.locate(&probes);
        for ((at, i), hit) in probes.iter().zip(&owners).zip(&reached) {
            if hit.is_some() {
                spoke_at.push((*i, *at));
            }
        }
        let mut connected: std::collections::BTreeMap<usize, u8> = std::collections::BTreeMap::new();
        for (i, hit) in owners.iter().zip(&reached) {
            if hit.is_some() {
                let n = connected.entry(*i).or_default();
                *n = n.saturating_add(1);
            }
        }
        let centres: Vec<[f64; 2]> = cores.iter().map(|(_, p, _)| p.at()).collect();
        for ((i, ..), inside) in cores.iter().zip(area.locate(&centres)) {
            let connected = connected.get(i).copied().unwrap_or(0);
            out.reliefs.push(Relief { pad: *i, requested: t.spokes, connected, inside: inside.is_some(), spoke_width: t.spoke_width, solid: false });
        }
        let kept = bars.iter().zip(&reached).filter(|(_, at)| at.is_some()).map(|(b, _)| b.as_slice());
        Region::polygons(kept).intersection(&base)
    } else {
        Region::new()
    };
    // Whether the fill reaches each solidly joined pad: points all round it, as far out as a
    // thermal pad's spokes meet the fill.
    if !solid_cores.is_empty() {
        let mut probes = Vec::new();
        let mut owners = Vec::new();
        let turn = 360.0 / f64::from(SOLID_PROBES);
        for (i, p, core) in &solid_cores {
            let centre = p.at();
            for k in 0..SOLID_PROBES {
                let a = (p.pad.rotation + turn * f64::from(k)).to_radians();
                let d = p.placement.vector([a.cos(), a.sin()]);
                let r = reach(core, centre, d) + t.gap + half.max(5.0 * TOLERANCE);
                probes.push([centre[0] + d[0] * r, centre[1] + d[1] * r]);
                owners.push(*i);
            }
        }
        let reached = opened.locate(&probes);
        let centres: Vec<[f64; 2]> = solid_cores.iter().map(|(_, p, _)| p.at()).collect();
        for ((i, ..), inside) in solid_cores.iter().zip(area.locate(&centres)) {
            let hits = owners.iter().zip(&reached).filter(|(o, hit)| **o == *i && hit.is_some()).count();
            let connected = u8::try_from(hits).unwrap_or(u8::MAX);
            out.reliefs.push(Relief { pad: *i, requested: SOLID_PROBES, connected, inside: inside.is_some(), spoke_width: 0.0, solid: true });
        }
    }
    let fill = match pour.hatch {
        Some(h) => {
            let hatched = hatch(&opened, h, budget, &mut out.truncated);
            // An opening may fall on a pad or a via of its net: their copper, where the fill is
            // solid, stays, so the lines that reach them join them.
            let mut solid: Vec<Vec<[f64; 2]>> = Vec::new();
            for (_, p, core) in &solid_cores {
                if p.on(layer) && near(core_bbox(core), 0.0) {
                    solid.push(hull_outline(core, 0.0));
                }
            }
            for (i, v) in board.vias.iter().enumerate().filter(|(_, v)| v.on(layer, layers)) {
                let core = disc(v.at, v.diameter / 2.0);
                if same(c.via_nets.get(i).copied().flatten()) && near(core_bbox(&core), 0.0) {
                    solid.push(hull_outline(&core, 0.0));
                }
            }
            let n = solid.iter().map(Vec::len).fold(0, usize::saturating_add);
            if solid.is_empty() {
                hatched
            } else if budget.spend(n) {
                hatched.union(&Region::polygons(solid.iter().map(Vec::as_slice)).intersection(&opened))
            } else {
                out.truncated = true;
                hatched
            }
        }
        None => opened,
    }
    .union(&spokes);
    // Islands: which copper of the net each one touches.
    let mut probes: Vec<[f64; 2]> = Vec::new();
    let mut nodes: Vec<Node> = Vec::new();
    if net.is_some() {
        let mut spoked = vec![false; c.pads.len()];
        for (i, ..) in &cores {
            if let Some(slot) = spoked.get_mut(*i) {
                *slot = true;
            }
        }
        for (i, p) in c.pads.iter().enumerate() {
            if same(p.net) && p.on(layer) && connect_of(p) != Connect::None && !spoked.get(i).copied().unwrap_or(false) {
                probes.push(p.at());
                nodes.push(Node::Pad(i));
            }
        }
        for (i, at) in &spoke_at {
            probes.push(*at);
            nodes.push(Node::Pad(*i));
        }
        for (i, v) in board.vias.iter().enumerate() {
            if same(c.via_nets.get(i).copied().flatten()) && v.on(layer, layers) {
                probes.push(v.at);
                nodes.push(Node::Via(i));
            }
        }
        'tracks: for (i, track) in board.tracks.iter().enumerate().filter(|(_, t)| t.layer == layer) {
            if !same(c.track_nets.get(i).copied().flatten()) {
                continue;
            }
            for (a, b) in pieces(&track.points, &track.curves, false) {
                let length = (b[0] - a[0]).hypot(b[1] - a[1]);
                let n = ((length / TRACK_STEP).ceil() as usize).clamp(1, MAX_PROBES);
                for k in 0..=n {
                    if probes.len() >= MAX_PROBES {
                        out.truncated = true;
                        break 'tracks;
                    }
                    let s = k as f64 / n as f64;
                    probes.push([a[0] + (b[0] - a[0]) * s, a[1] + (b[1] - a[1]) * s]);
                    nodes.push(Node::Track(i));
                }
            }
        }
    }
    let at = fill.locate(&probes);
    let mut touched = vec![false; fill.len()];
    for s in at.iter().flatten() {
        if let Some(slot) = touched.get_mut(*s) {
            *slot = true;
        }
    }
    // Islands that overlap the fill of a pour of its net made before it on the layer: joined to
    // it, and touching its net.
    let mut overlaps: Vec<(usize, Node)> = Vec::new();
    let others: Vec<&PourFill> =
        done.iter().filter(|f| net.is_some() && f.layer == layer && f.net == net && f.region.bbox().is_some_and(|b| boxes_meet(b, bbox))).collect();
    if !others.is_empty() {
        let boxed = |r: Region| {
            let b = r.bbox();
            (r, b)
        };
        let mine: Vec<(Region, Option<[[f64; 2]; 2]>)> = fill.split().into_iter().map(boxed).collect();
        'others: for f in others {
            let theirs: Vec<(Region, Option<[[f64; 2]; 2]>)> = f.region.split().into_iter().map(boxed).collect();
            let mut grid = Grid::new(CELL);
            for (k, (_, b)) in theirs.iter().enumerate() {
                if let Some(b) = b {
                    grid.insert(k, *b);
                }
            }
            for (s, (a, ab)) in mine.iter().enumerate() {
                let Some(ab) = ab else { continue };
                let near = grid.near_box(*ab, theirs.len());
                if !budget.spend(near.len()) {
                    out.truncated = true;
                    break 'others;
                }
                for k in near {
                    let Some((b, _)) = theirs.get(k).filter(|(_, bb)| bb.is_some_and(|bb| boxes_meet(*ab, bb))) else { continue };
                    if !budget.spend(a.corners().saturating_add(b.corners())) {
                        out.truncated = true;
                        break 'others;
                    }
                    if !a.intersection(b).is_empty() {
                        if let Some(slot) = touched.get_mut(s) {
                            *slot = true;
                        }
                        overlaps.push((s, Node::Fill(f.pour, k)));
                    }
                }
            }
        }
    }
    let keep = |s: usize| !(pour.remove_islands && net.is_some()) || touched.get(s).copied().unwrap_or(false);
    let mut renumber = vec![None; fill.len()];
    let mut next = 0;
    for (s, slot) in renumber.iter_mut().enumerate() {
        if keep(s) {
            *slot = Some(next);
            next += 1;
        } else {
            out.islands_removed += 1;
        }
    }
    let found = at.iter().zip(&nodes).filter_map(|(s, node)| Some(((*s)?, *node))).chain(overlaps);
    let mut joins: Vec<(usize, Node)> = found.filter_map(|(s, node)| Some((renumber.get(s).copied().flatten()?, node))).collect();
    joins.sort_unstable();
    joins.dedup();
    out.joins = joins;
    out.region = fill.select(keep);
    out
}

/// `fill` hatched: less square openings between lines `width` wide every `pitch` both ways, with
/// a border `width` wide kept round it; solid (and `truncated` set) past [`MAX_HATCH_CELLS`], or
/// when the openings' corners are past the `budget`.
fn hatch(fill: &Region, h: Hatch, budget: &mut Budget, truncated: &mut bool) -> Region {
    let Some([[x0, y0], [x1, y1]]) = fill.bbox() else { return fill.clone() };
    let (nx, ny) = (((x1 - x0) / h.pitch).ceil() + 1.0, ((y1 - y0) / h.pitch).ceil() + 1.0);
    if !(nx.is_finite() && ny.is_finite() && nx * ny <= MAX_HATCH_CELLS) || !budget.spend((nx * ny) as usize * 4) {
        *truncated = true;
        return fill.clone();
    }
    let side = h.pitch - h.width;
    let mut squares = Vec::new();
    for i in 0..nx as usize {
        for j in 0..ny as usize {
            let (x, y) = (x0 + h.width + i as f64 * h.pitch, y0 + h.width + j as f64 * h.pitch);
            squares.push(vec![[x, y], [x + side, y], [x + side, y + side], [x, y + side]]);
        }
    }
    let openings = Region::polygons(squares.iter().map(Vec::as_slice));
    let border = fill.difference(&fill.offset(-h.width, OFFSET_STEP));
    fill.difference(&openings).union(&border)
}
