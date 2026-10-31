//! What is connected to what on the board, worked out each time and never saved: each pad's net,
//! which copper touches which, and the airwires still to route.
//!
//! A pad's net is the schematic's: the net of the part's pin that the part's variant maps to the pad
//! (see [`Resolved::connections`](super::Resolved::connections)). Copper joins on one copper layer
//! (through-hole pads are on every layer, surface-mount pads on the top or the bottom, a via on
//! the layers of its span):
//! - a track's point (an end or a bend) inside a pad's outline;
//! - a track's point on another track (within that track's half width of its line or arc);
//! - a track's point under a via (within the via's radius of its centre), or a pad's copper under
//!   its ring;
//! - a via's or a pad's centre on a track (within the track's half width);
//! - a via's ring on another via's ring, when their spans share a layer (stacked or staggered
//!   micro vias);
//! - a copper pour's filled island and the copper of its net it touches, and the islands of
//!   another pour of its net on the layer it overlaps (see [`super::fill`]).
//!
//! The copper so joined makes islands. An island takes the net of its pads and of the vias given
//! a net of their own (the lowest, where two nets touch: a short circuit, which the design rule
//! check reports); pours fill with the nets so found before they join anything. For each net,
//! the airwires are the shortest connections that join its islands into one: a minimum spanning
//! tree between the islands, each airwire between the nearest points (pad centres, track points,
//! via centres) of the two islands it joins.
//!
//! Points near a point are found through a grid of cells, so the work grows with the copper, not
//! with its square; past a budget of tests the rest is left out and [`BoardNets::truncated`] is set.
//! The airwires' tree has a budget of its own: past it, islands join in turn rather than nearest
//! first, and `truncated` is set too.
//!
//! A routed run ([`Runs`]) is copper joined through tracks and vias alone: joins at a point inside
//! a pad do not count, so the run stops at each pad. Unrouting a run brings its airwires back.

use std::collections::BTreeMap;

use super::board::{Board, BoardFootprint, Track, Via};
use super::checks::gap;
use super::connect::{Connectivity, Sets};
use super::fill::{CopperNets, Node, PourFill, pour_fills};
use super::footprint::{Core, Pad, PadKind, Side};
use super::grid::Grid;
use super::layers::CopperLayer;
use super::path::{self, segment_distance};
use super::place::Placement;
use super::schematic::Part;
use super::{Electronics, Resolved};

/// The most point tests one pass makes; past it, joins are left out and
/// [`BoardNets::truncated`] is set.
pub const MAX_TESTS: usize = 1 << 22;
/// The most points of one island the airwires start or end at (pad centres first).
const MAX_ISLAND_POINTS: usize = 32;
/// The grid's cell side, in millimetres.
const CELL: f64 = 2.54;

/// A pad as it lies on the board.
#[derive(Clone, Debug, PartialEq)]
pub struct BoardPad {
    /// The board footprint's id.
    pub footprint: u64,
    /// The schematic part's id and designator.
    pub part: u64,
    pub designator: String,
    /// The pad as the library footprint has it.
    pub pad: Pad,
    /// Where the footprint sits.
    pub placement: Placement,
    /// Copper on the top and on the bottom (none for a bare hole); a pad with both goes through
    /// the board and has copper on every inner layer too.
    pub top: bool,
    pub bottom: bool,
    /// Its net, an index into [`BoardNets::names`]; `None` when its pin is on no net (or it has
    /// none).
    pub net: Option<usize>,
}

impl BoardPad {
    /// Its centre on the board.
    pub fn at(&self) -> [f64; 2] {
        self.placement.apply(self.pad.at)
    }

    /// `R1.2`: the designator, a dot and the pad's number.
    pub fn reference(&self) -> String {
        format!("{}.{}", self.designator, self.pad.number)
    }

    /// Whether it has copper on `layer`.
    pub fn on(&self, layer: CopperLayer) -> bool {
        match layer {
            CopperLayer::Top => self.top,
            CopperLayer::Inner(_) => self.top && self.bottom,
            CopperLayer::Bottom => self.bottom,
        }
    }

    /// Whether it has copper on the side `side`.
    pub fn on_side(&self, side: Side) -> bool {
        self.on(side.into())
    }

    /// Its outline on the board, arcs as `per_quarter` straight pieces a quarter turn.
    pub fn outline(&self, per_quarter: usize) -> Vec<[f64; 2]> {
        self.pad.outline(per_quarter).into_iter().map(|p| self.placement.apply(p)).collect()
    }

    /// Its drill's outline on the board; `None` without a drill.
    pub fn drill_outline(&self, per_quarter: usize) -> Option<Vec<[f64; 2]>> {
        Some(self.pad.drill_outline(per_quarter)?.into_iter().map(|p| self.placement.apply(p)).collect())
    }

    /// The box round it on the board.
    pub fn bbox(&self) -> [[f64; 2]; 2] {
        let [a, b] = self.pad.bbox();
        self.placement.bbox([a, b])
    }

    /// Whether `p` (on the board) lies on the pad, its edge included.
    pub fn contains(&self, p: [f64; 2]) -> bool {
        let local = self.placement.unapply(p);
        gap(&self.pad.core(), &Core { points: vec![local], radius: 0.0 }) <= 1e-9
    }
}

/// A connection still to route: between two islands of one net.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Airwire {
    /// An index into [`BoardNets::names`].
    pub net: usize,
    pub from: [f64; 2],
    pub to: [f64; 2],
}

/// The board's connections (see the module notes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoardNets {
    /// The schematic's net names, in its order.
    pub names: Vec<String>,
    /// Every pad of every footprint whose part the schematic has, holes included, footprint by
    /// footprint in board order.
    pub pads: Vec<BoardPad>,
    /// Each track's net, in the order of the board's `tracks`.
    pub track_nets: Vec<Option<usize>>,
    /// Each via's net, in the order of the board's `vias`.
    pub via_nets: Vec<Option<usize>>,
    /// The island of each pad, track and via, in the orders above, and of each pour fill's
    /// shapes (by fill, then shape): copper with the same number is joined.
    pub pad_islands: Vec<usize>,
    pub track_islands: Vec<usize>,
    pub via_islands: Vec<usize>,
    pub fill_islands: Vec<Vec<usize>>,
    /// Each via's own net (see [`super::board::Via::net`]), in board order.
    pub via_own_nets: Vec<Option<usize>>,
    /// The pours' fills, in the board's order of pours.
    pub fills: Vec<PourFill>,
    /// Ordered by net, then shortest first within the tree's order.
    pub airwires: Vec<Airwire>,
    /// Some joins were past the budget, so not everything is joined.
    pub truncated: bool,
}

impl BoardNets {
    /// The name of net `i`.
    pub fn name(&self, i: usize) -> Option<&str> {
        self.names.get(i).map(String::as_str)
    }
}

/// A footprint's part and what it is built with, when the schematic has the part.
pub struct Placed<'a> {
    pub footprint: &'a BoardFootprint,
    pub part: &'a Part,
    pub resolved: Resolved<'a>,
}

impl Electronics {
    /// The part of the board footprint `f` and its component's variant, when the schematic and
    /// the design have them.
    pub fn placed<'a>(&'a self, f: &'a BoardFootprint) -> Option<Placed<'a>> {
        let part = self.schematic.part(f.part)?;
        let c = self.component(part.component)?;
        Some(Placed { footprint: f, part, resolved: self.resolve(c, part.variant.as_deref()) })
    }

    /// The board's connections (see the module notes).
    pub fn board_nets(&self) -> BoardNets {
        board_nets(self, &self.board, &self.connectivity())
    }
}

fn distance([ax, ay]: [f64; 2], [bx, by]: [f64; 2]) -> f64 {
    (ax - bx).hypot(ay - by)
}

/// Every pad of the board's footprints, with its net from `conn`.
pub fn board_pads(ecad: &Electronics, board: &Board, conn: &Connectivity) -> Vec<BoardPad> {
    board.footprints.iter().flat_map(|f| footprint_pads(ecad, f, Some(conn))).collect()
}

/// The pads of the board footprint `f` as they lie, with their nets from `conn` (none without
/// it); none when the schematic has no part for it.
pub fn footprint_pads(ecad: &Electronics, f: &BoardFootprint, conn: Option<&Connectivity>) -> Vec<BoardPad> {
    let Some(Placed { part, resolved, .. }) = ecad.placed(f) else { return Vec::new() };
    let mut nets: BTreeMap<&str, usize> = BTreeMap::new();
    if let Some(conn) = conn {
        for (pin, pads) in resolved.connections() {
            let Some(net) = conn.pin_net(part.id, pin) else { continue };
            for pad in pads {
                nets.entry(pad).or_insert(net);
            }
        }
    }
    let placement = f.placement();
    let mut out = Vec::with_capacity(resolved.footprint.pads.len());
    for pad in &resolved.footprint.pads {
        let (top, bottom) = match pad.kind {
            PadKind::Hole => (false, false),
            PadKind::ThroughHole => (true, true),
            // A footprint on the bottom takes its pads there.
            PadKind::Smd => {
                let top = (pad.side == Side::Top) != placement.mirror;
                (top, !top)
            }
        };
        out.push(BoardPad {
            footprint: f.id,
            part: part.id,
            designator: part.designator.clone(),
            pad: pad.clone(),
            placement,
            top,
            bottom,
            net: if pad.kind == PadKind::Hole { None } else { nets.get(pad.number.as_str()).copied() },
        });
    }
    out
}

/// Which copper touches which: pads, then tracks, then vias, as the nodes of the sets, and
/// whether the budget ran out. For `runs`, pads join nothing and nothing joins at a point inside
/// a pad (see the module notes).
fn join(board: &Board, pads: &[BoardPad], runs: bool) -> (Sets, bool) {
    join_with(board, pads, runs, &mut None)
}

/// [`join`], each join also put in `contacts` (when given) with a point where the two meet.
fn join_with(board: &Board, pads: &[BoardPad], runs: bool, contacts: &mut Option<&mut Vec<Contact>>) -> (Sets, bool) {
    let (np, nt) = (pads.len(), board.tracks.len());
    let node_track = |t: usize| np + t;
    let node_via = |v: usize| np + nt + v;
    let mut sets = Sets::new(np + nt + board.vias.len());
    let layers = board.copper_layers;
    let mut pad_grid = Grid::new(CELL);
    for (i, p) in pads.iter().enumerate().filter(|(_, p)| p.top || p.bottom) {
        pad_grid.insert(i, p.bbox());
    }
    let mut via_grid = Grid::new(CELL);
    for (i, v) in board.vias.iter().enumerate() {
        let r = v.diameter / 2.0;
        via_grid.insert(i, [[v.at[0] - r, v.at[1] - r], [v.at[0] + r, v.at[1] + r]]);
    }
    let segments: Vec<(usize, [f64; 2], [f64; 2], f64)> = board
        .tracks
        .iter()
        .enumerate()
        .flat_map(|(t, track)| path::segments(&track.points, &track.curves, false).map(move |(a, b, s)| (t, a, b, s)))
        .collect();
    let mut segment_grid = Grid::new(CELL);
    for (i, (t, a, b, s)) in segments.iter().enumerate() {
        let r = board.tracks.get(*t).map_or(0.0, |track| track.width / 2.0);
        let [[x0, y0], [x1, y1]] = path::segment_bbox(*a, *b, *s);
        segment_grid.insert(i, [[x0 - r, y0 - r], [x1 + r, y1 + r]]);
    }
    // For runs, copper that meets inside a pad meets through the pad.
    let in_pad = |q: [f64; 2], layer: CopperLayer| runs && pad_grid.near(q).any(|i| pads.get(i).is_some_and(|p| p.on(layer) && p.contains(q)));
    let mut tests = 0usize;
    let mut truncated = false;
    'tracks: for (t, track) in board.tracks.iter().enumerate() {
        for q in &track.points {
            if !runs {
                for i in pad_grid.near(*q) {
                    tests += 1;
                    if let Some(p) = pads.get(i)
                        && p.on(track.layer)
                        && p.contains(*q)
                    {
                        link(&mut sets, contacts, node_track(t), i, *q);
                    }
                }
            }
            for v in via_grid.near(*q) {
                tests += 1;
                if board.vias.get(v).is_some_and(|via| via.on(track.layer, layers) && distance(*q, via.at) <= via.diameter / 2.0)
                    && !in_pad(*q, track.layer)
                {
                    link(&mut sets, contacts, node_track(t), node_via(v), *q);
                }
            }
            for s in segment_grid.near(*q) {
                tests += 1;
                let Some((other, a, b, sweep)) = segments.get(s) else { continue };
                let Some(o) = board.tracks.get(*other) else { continue };
                if *other != t && o.layer == track.layer && segment_distance(*q, *a, *b, *sweep) <= o.width / 2.0 && !in_pad(*q, track.layer) {
                    link(&mut sets, contacts, node_track(t), node_track(*other), *q);
                }
            }
            if tests > MAX_TESTS {
                truncated = true;
                break 'tracks;
            }
        }
    }
    // A track's segment under a via's or a pad's centre: their copper meets there too.
    let on_segment = |q: [f64; 2], s: usize| {
        let (t, a, b, sweep) = segments.get(s)?;
        let track = board.tracks.get(*t)?;
        (segment_distance(q, *a, *b, *sweep) <= track.width / 2.0).then_some((*t, track.layer))
    };
    for (v, via) in board.vias.iter().enumerate() {
        let r = via.diameter / 2.0;
        let around = [[via.at[0] - r, via.at[1] - r], [via.at[0] + r, via.at[1] + r]];
        // Another via's ring on its ring, on a layer both spans have: a stack of micro vias.
        let (a0, a1) = via.span(layers);
        for w in via_grid.near_box(around, board.vias.len()) {
            tests += 1;
            let meets = |o: &Via| {
                let (b0, b1) = o.span(layers);
                a0 <= b1 && b0 <= a1 && distance(via.at, o.at) <= r + o.diameter / 2.0
            };
            if w > v && board.vias.get(w).is_some_and(meets) {
                link(&mut sets, contacts, node_via(v), node_via(w), via.at);
            }
        }
        if !runs {
            // The via's ring on a pad's copper, on a layer of its span.
            for i in pad_grid.near_box(around, pads.len()) {
                tests += 1;
                let shares = |p: &BoardPad| {
                    (p.top && p.bottom) || (p.top && via.on(CopperLayer::Top, layers)) || (p.bottom && via.on(CopperLayer::Bottom, layers))
                };
                if pads
                    .get(i)
                    .is_some_and(|p| shares(p) && gap(&p.pad.core(), &Core { points: vec![p.placement.unapply(via.at)], radius: r }) <= 1e-9)
                {
                    link(&mut sets, contacts, node_via(v), i, via.at);
                }
            }
        }
        for s in segment_grid.near(via.at) {
            tests += 1;
            if let Some((t, layer)) = on_segment(via.at, s)
                && via.on(layer, layers)
                && !in_pad(via.at, layer)
            {
                link(&mut sets, contacts, node_via(v), node_track(t), via.at);
            }
        }
        if tests > MAX_TESTS {
            truncated = true;
            break;
        }
    }
    if runs {
        return (sets, truncated);
    }
    for (i, p) in pads.iter().enumerate().filter(|(_, p)| p.top || p.bottom) {
        let at = p.at();
        for s in segment_grid.near(at) {
            tests += 1;
            if let Some((t, layer)) = on_segment(at, s)
                && p.on(layer)
            {
                link(&mut sets, contacts, i, node_track(t), at);
            }
        }
        if tests > MAX_TESTS {
            truncated = true;
            break;
        }
    }
    (sets, truncated)
}

/// Two pieces of copper that touch (see [`contacts`]): nodes numbered as the islands number
/// them (the pads, then the tracks, then the vias, each in board order), and a point where they
/// meet.
pub type Contact = (usize, usize, [f64; 2]);

fn link(sets: &mut Sets, contacts: &mut Option<&mut Vec<Contact>>, a: usize, b: usize, at: [f64; 2]) {
    sets.join(a, b);
    if let Some(list) = contacts.as_mut() {
        list.push((a, b, at));
    }
}

/// Where the board's copper (with `pads`, as [`BoardNets::pads`] has them) touches, as the
/// islands are joined, pours left out: for finding where two nets meet.
pub fn contacts(board: &Board, pads: &[BoardPad]) -> Vec<Contact> {
    let mut list = Vec::new();
    join_with(board, pads, false, &mut Some(&mut list));
    list
}

/// The routed runs (see the module notes): the run of each track and each via, in board order;
/// copper with the same number is one run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Runs {
    pub tracks: Vec<usize>,
    pub vias: Vec<usize>,
    /// Some joins were past the budget, so a run may be cut short.
    pub truncated: bool,
}

impl Electronics {
    /// The routed runs of `board` (the stored one, or one being edited).
    pub fn board_runs(&self, board: &Board) -> Runs {
        let pads: Vec<BoardPad> = board.footprints.iter().flat_map(|f| footprint_pads(self, f, None)).collect();
        let (mut sets, truncated) = join(board, &pads, true);
        let (np, nt) = (pads.len(), board.tracks.len());
        Runs {
            tracks: (0..nt).map(|t| sets.root(np + t)).collect(),
            vias: (0..board.vias.len()).map(|v| sets.root(np + nt + v)).collect(),
            truncated,
        }
    }
}

/// Each island's net: the lowest net of its pads, its vias' own nets and its pour fills.
fn island_nets(
    sets: &mut Sets,
    pads: &[BoardPad],
    via_own: &[Option<usize>],
    np: usize,
    nt: usize,
    fills: &[(usize, Option<usize>)],
) -> BTreeMap<usize, usize> {
    let mut island_net: BTreeMap<usize, usize> = BTreeMap::new();
    let nodes = pads.iter().enumerate().map(|(i, p)| (i, p.net));
    let vias = via_own.iter().enumerate().map(|(v, n)| (np + nt + v, *n));
    for (node, net) in nodes.chain(vias).chain(fills.iter().copied()) {
        if let Some(net) = net {
            let root = sets.root(node);
            let slot = island_net.entry(root).or_insert(net);
            *slot = (*slot).min(net);
        }
    }
    island_net
}

/// The board's connections as `board` has it (the stored one, or one being dragged), its pads'
/// nets from `conn`, its pours filled (see the module notes).
pub fn board_nets(ecad: &Electronics, board: &Board, conn: &Connectivity) -> BoardNets {
    board_nets_with(ecad, board, conn, true)
}

/// As [`board_nets`], without filling the pours when `pours` is false (cheaper, for a board
/// being dragged: its pours then join nothing).
pub fn board_nets_with(ecad: &Electronics, board: &Board, conn: &Connectivity, pours: bool) -> BoardNets {
    let pads = board_pads(ecad, board, conn);
    let names: Vec<String> = conn.nets.iter().map(|n| n.name.clone()).collect();
    let (np, nt, nv) = (pads.len(), board.tracks.len(), board.vias.len());
    let node_track = |t: usize| np + t;
    let node_via = |v: usize| np + nt + v;
    let (mut sets, truncated) = join(board, &pads, false);
    let via_own_nets: Vec<Option<usize>> = board.vias.iter().map(|v| v.net.as_deref().and_then(|n| names.iter().position(|m| m == n))).collect();
    let mut island_net = island_nets(&mut sets, &pads, &via_own_nets, np, nt, &[]);
    let mut fills = Vec::new();
    let mut fill_nodes: Vec<Vec<usize>> = Vec::new();
    if pours && !board.pours.is_empty() {
        let net_of = |sets: &mut Sets, node: usize| island_net.get(&sets.root(node)).copied();
        let track_nets: Vec<Option<usize>> = (0..nt).map(|t| net_of(&mut sets, node_track(t))).collect();
        let via_nets: Vec<Option<usize>> = (0..nv).map(|v| net_of(&mut sets, node_via(v))).collect();
        fills = pour_fills(board, &CopperNets { names: &names, pads: &pads, track_nets: &track_nets, via_nets: &via_nets });
        // Each fill's shapes become nodes, joined to the copper they touch.
        let mut next = np + nt + nv;
        for f in &fills {
            let count = f.region.len();
            fill_nodes.push((next..next + count).collect());
            next += count;
        }
        sets.grow(next);
        let mut with_nets: Vec<(usize, Option<usize>)> = Vec::new();
        let fill_at: BTreeMap<u64, usize> = fills.iter().enumerate().map(|(i, f)| (f.pour, i)).collect();
        for (f, nodes) in fills.iter().zip(&fill_nodes) {
            for (shape, node) in &f.joins {
                let Some(&n) = nodes.get(*shape) else { continue };
                let other = match node {
                    Node::Pad(i) => *i,
                    Node::Track(t) => node_track(*t),
                    Node::Via(v) => node_via(*v),
                    Node::Fill(pour, k) => {
                        let Some(&o) = fill_at.get(pour).and_then(|i| fill_nodes.get(*i)).and_then(|o| o.get(*k)) else { continue };
                        o
                    }
                };
                sets.join(n, other);
            }
            with_nets.extend(nodes.iter().map(|n| (*n, f.net)));
        }
        island_net = island_nets(&mut sets, &pads, &via_own_nets, np, nt, &with_nets);
    }
    let pad_islands: Vec<usize> = (0..np).map(|i| sets.root(i)).collect();
    let track_islands: Vec<usize> = (0..nt).map(|t| sets.root(node_track(t))).collect();
    let via_islands: Vec<usize> = (0..nv).map(|v| sets.root(node_via(v))).collect();
    let fill_islands: Vec<Vec<usize>> = fill_nodes.iter().map(|nodes| nodes.iter().map(|n| sets.root(*n)).collect()).collect();
    let track_nets = track_islands.iter().map(|root| island_net.get(root).copied()).collect();
    let via_nets = via_islands.iter().map(|root| island_net.get(root).copied()).collect();
    // Each net's islands, with the points airwires may start or end at: its pads' centres first.
    let mut islands: BTreeMap<usize, BTreeMap<usize, Vec<[f64; 2]>>> = BTreeMap::new();
    for (i, p) in pads.iter().enumerate() {
        if let Some(net) = p.net {
            let points = islands.entry(net).or_default().entry(sets.root(i)).or_default();
            if points.len() < MAX_ISLAND_POINTS {
                points.push(p.at());
            }
        }
    }
    let mut extra = |node: usize, points: &mut dyn Iterator<Item = [f64; 2]>| {
        let root = sets.root(node);
        let Some(net) = island_net.get(&root) else { return };
        if let Some(island) = islands.get_mut(net).and_then(|n| n.get_mut(&root)) {
            for p in points.take(MAX_ISLAND_POINTS.saturating_sub(island.len())) {
                island.push(p);
            }
        }
    };
    for (t, track) in board.tracks.iter().enumerate() {
        extra(node_track(t), &mut track.points.iter().copied());
    }
    for (v, via) in board.vias.iter().enumerate() {
        extra(node_via(v), &mut std::iter::once(via.at));
    }
    let mut airwires = Vec::new();
    let mut budget = MAX_TESTS;
    let mut cut_short = false;
    for (net, by_root) in islands {
        let groups: Vec<Vec<[f64; 2]>> = by_root.into_values().filter(|g| !g.is_empty()).collect();
        for (from, to) in spanning(&groups, &mut budget, &mut cut_short) {
            airwires.push(Airwire { net, from, to });
        }
    }
    let truncated = truncated || cut_short || fills.iter().any(|f| f.truncated);
    BoardNets { names, pads, track_nets, via_nets, pad_islands, track_islands, via_islands, fill_islands, via_own_nets, fills, airwires, truncated }
}

/// The nearest points of two groups and their distance.
fn nearest(a: &[[f64; 2]], b: &[[f64; 2]]) -> (f64, [f64; 2], [f64; 2]) {
    let mut best = (f64::INFINITY, [0.0; 2], [0.0; 2]);
    for p in a {
        for q in b {
            let d = distance(*p, *q);
            if d < best.0 {
                best = (d, *p, *q);
            }
        }
    }
    best
}

/// The nearest points of two groups, each pair of points tried spending one of `budget`; past it
/// only their first points (one more), and `None` once the budget is gone.
fn nearest_within(a: &[[f64; 2]], b: &[[f64; 2]], budget: &mut usize) -> Option<(f64, [f64; 2], [f64; 2])> {
    let cost = a.len().saturating_mul(b.len());
    if cost <= *budget {
        *budget -= cost;
        return Some(nearest(a, b));
    }
    let (Some(p), Some(q)) = (a.first(), b.first()) else { return None };
    *budget = budget.checked_sub(1)?;
    Some((distance(*p, *q), *p, *q))
}

/// The shortest connections that join `groups` into one (Prim's minimum spanning tree), each
/// between the nearest points of the two groups it joins. Every point pair tried, and every group
/// looked at to choose the next, spends one of `budget` (shared by every net): past it groups are
/// compared by their first points, and once it is gone the groups left are joined in turn, first
/// point to first point, and `truncated` is set.
fn spanning(groups: &[Vec<[f64; 2]>], budget: &mut usize, truncated: &mut bool) -> Vec<([f64; 2], [f64; 2])> {
    let k = groups.len();
    let Some(first) = groups.first().filter(|_| k >= 2) else { return Vec::new() };
    let mut joined = vec![false; k];
    if let Some(j) = joined.first_mut() {
        *j = true;
    }
    let mut best: Vec<Option<(f64, [f64; 2], [f64; 2])>> = groups.iter().map(|g| nearest_within(first, g, budget)).collect();
    let mut out = Vec::with_capacity(k - 1);
    let mut last = 0;
    let free = |joined: &[bool], j: usize| !joined.get(j).copied().unwrap_or(true);
    for _ in 1..k {
        *budget = budget.saturating_sub(k);
        if *budget == 0 {
            // Out of budget: the groups left join in turn, from the one joined last.
            *truncated = true;
            let mut from = groups.get(last).and_then(|g| g.first()).copied();
            for (_, g) in groups.iter().enumerate().filter(|(m, _)| free(&joined, *m)) {
                if let (Some(p), Some(q)) = (from, g.first()) {
                    out.push((p, *q));
                }
                from = g.first().copied().or(from);
            }
            break;
        }
        let cost = |j: &usize| best.get(*j).copied().flatten().map_or(f64::INFINITY, |b| b.0);
        let Some(j) = (0..k).filter(|j| free(&joined, *j)).min_by(|a, b| cost(a).total_cmp(&cost(b))) else { break };
        if let Some(slot) = joined.get_mut(j) {
            *slot = true;
        }
        // Without a nearest pair worked out, from the group joined last.
        let link = best.get(j).copied().flatten().or_else(|| {
            *truncated = true;
            let (p, q) = (groups.get(last)?.first()?, groups.get(j)?.first()?);
            Some((distance(*p, *q), *p, *q))
        });
        if let Some((_, from, to)) = link {
            out.push((from, to));
        }
        last = j;
        let Some(group) = groups.get(j) else { continue };
        for m in (0..k).filter(|m| free(&joined, *m)) {
            let (Some(other), Some(slot)) = (groups.get(m), best.get_mut(m)) else { continue };
            if let Some(c) = nearest_within(group, other, budget)
                && slot.is_none_or(|b| c.0 < b.0)
            {
                *slot = Some(c);
            }
        }
    }
    out
}

/// Whether the track `t` has a point on the pad `p` (on the track's layer).
pub fn track_on_pad(t: &Track, p: &BoardPad) -> bool {
    p.on(t.layer) && t.points.iter().any(|q| p.contains(*q))
}
