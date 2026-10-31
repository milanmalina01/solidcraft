//! ESD paths: whether a strike on a connector pin goes into its TVS before it reaches anything
//! else, and whether the TVS has a short way to ground. The TVS parts (intent roles `tvs` and
//! `esd_array`) name the connector pins they protect in `of`; on each net with such a pin and a
//! pad of the TVS:
//! - `esd.path`: an error when a pad the line feeds (the protected IC's, a series part's;
//!   capacitors aside) can be reached from the connector pin without passing the TVS's pad, past
//!   a branch to it longer than `stub` (a line that runs over the pad, or a flow-through package
//!   whose pads the line enters and leaves by, passes it). Warnings for a via between the
//!   connector pin and the TVS (a via after it is fine), the TVS farther than `reach` along the
//!   copper, the protected pad nearer the TVS than `ratio` times that, and another net's track
//!   within `gap` of that unclamped run, on its layer or the next one (or farther with no plane
//!   between to shield it), for an unbroken stretch longer than `run` (ground and the lines the
//!   same TVS protects aside; a track crossing it is no stretch). An info for a turn sharper than
//!   `turn` in it.
//! - `esd.ground`: each TVS ground pad (a pad on a ground net) with fewer than `vias` vias of
//!   its ground within `reach` of its edge (a warning with one, an error with none), joined to a
//!   pour through thermal spokes, or reaching its first via through more than `track` of track
//!   (errors); an info when the protected connector's nearest shell or ground pin is farther
//!   than `chassis`. Ball-grid TVS parts are left out: their vias are in the pads.
//!
//! Lengths are along the copper (see [`Walk`]): along tracks, straight across pours, nothing
//! inside a pad or a via.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use serde_json::{Value, json};

use super::super::checks::{Severity, gap};
use super::super::copper::{disc, pad_core};
use super::super::drc::{Bound, Class, DrcItem};
use super::super::footprint::PadShape;
use super::super::grid::Grid;
use super::super::intent::{PartIntent, PartRole};
use super::super::layers::CopperLayer;
use super::catalog::{Code, Family, Param};
use super::geom::{along, distance, track_pieces};
use super::pi_current::{Copper, Item, copper, position};
use super::{Cx, Draft, num};

const PATH: &str = "esd.path";
const GROUND: &str = "esd.ground";

const fn param(name: &'static str, default: f64, units: &'static str, min: f64, max: f64, source: &'static str, about: &'static str) -> Param {
    Param { name, default, units, source, min, max, verify: false, about }
}

const CODES: &[Code] = &[
    Code {
        code: PATH,
        class: Class::Critical,
        severity: Severity::Error,
        worst: None,
        needs: "geometry; intent: tvs or esd_array parts with the connector pins they protect (of)",
        section: "rf-emc#esd",
        about: "a protected pad reachable from the connector without passing the TVS pad (error); a via before the TVS, the TVS far from the connector or the protected pad near the TVS, another net alongside the unclamped run (warnings); a sharp turn in it (info)",
        principle: "A strike's current divides by inductance: the line must run from the connector pin straight over the TVS pad, short and on one layer, so the TVS clamps before anything else sees it.",
        params: &[
            param(
                "stub",
                0.5,
                "mm",
                0.0,
                20.0,
                "TI SLVA680A (principle): a branch to the TVS adds its inductance to the clamp",
                "a branch from the line to the TVS pad no longer than this is on the line",
            ),
            param(
                "reach",
                5.0,
                "mm",
                0.1,
                200.0,
                "judgment, from TI SLVA680A's principle",
                "the TVS this far along the copper from the connector pin at most",
            ),
            param(
                "ratio",
                2.0,
                "ratio",
                0.0,
                100.0,
                "TI SLVA680A (principle); the critic's 2×",
                "the protected pad at least this many times as far from the TVS as the TVS is from the connector",
            ),
            param("gap", 0.5, "mm", 0.0, 10.0, "TI SLVA680A (principle)", "another net's track this close to the unclamped run couples into it"),
            param("run", 2.0, "mm", 0.0, 100.0, "TI SLVA680A (principle)", "for longer than this"),
            param(
                "turn",
                45.0,
                "°",
                0.0,
                180.0,
                "TI SLVA680A (principle): the critic dropped its 7 kV figure",
                "a turn sharper than this in the unclamped run is noted",
            ),
        ],
    },
    Code {
        code: GROUND,
        class: Class::Critical,
        severity: Severity::Error,
        worst: None,
        needs: "geometry; intent: tvs or esd_array parts, the ground net",
        section: "rf-emc#esd",
        about: "a TVS ground pad with fewer than 2 ground vias within 1 mm (an error with none), joined through thermal spokes, or more than 1 mm of track from its first via (errors); the connector's shell or ground pin far away (info)",
        principle: "The TVS ground connection is usually the largest inductance in the clamp path: it goes straight into the plane through vias right at the pad.",
        params: &[
            param(
                "reach",
                1.0,
                "mm",
                0.0,
                20.0,
                "TI SLVA680A (principle); the critic's 1 mm",
                "a via's centre this close to the ground pad's edge counts",
            ),
            param("vias", 2.0, "vias", 1.0, 64.0, "TI SLVA680A (principle); the critic's 2 vias", "the least number of ground vias that close"),
            param("track", 1.0, "mm", 0.0, 50.0, "TI SLVA680A (principle)", "the most track from the ground pad to its first via"),
            param(
                "chassis",
                10.0,
                "mm",
                0.0,
                500.0,
                "TI SLVA680A (principle)",
                "the connector's shell or ground pin this close to the TVS ground pad",
            ),
        ],
    },
];

pub(super) const FAMILY: Family = Family { module: "esd", title: "ESD paths", codes: CODES, run };

/// What a step of a walk runs through.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Through {
    /// Along a track (by index), from one distance along it to another.
    Track(usize, f64, f64),
    /// Into or out of a via (by index).
    Via(usize),
    /// Across a pad, a pour or spokes.
    Copper,
}

/// One net's copper as points (see the module notes): a point where two items touch (one per
/// link of the [`Copper`]) and each pad's and via's centre (its hub), joined along the copper.
struct Walk {
    edges: Vec<Vec<(usize, f64, Through)>>,
    /// The hub of each pad, via and spoke-set item, by item index.
    hub: BTreeMap<usize, usize>,
}

/// The most protected pads reached around the TVS that one net reports, nearest the connector
/// first.
const MAX_VICTIMS: usize = 4;

/// The most points of one pour island joined each to each; past it, each to its nearest.
const MAX_PAIRWISE: usize = 48;
const NEAREST: usize = 8;

/// The part of a track's pieces between two distances along it, as points.
fn portion(pieces: &[([f64; 2], [f64; 2])], from: f64, to: f64) -> Vec<[f64; 2]> {
    let (lo, hi) = (from.min(to), from.max(to));
    let mut out: Vec<[f64; 2]> = Vec::new();
    let mut walked = 0.0;
    for (a, b) in pieces {
        let len = distance(*a, *b);
        let (s0, s1) = (walked, walked + len);
        walked = s1;
        if s1 < lo || s0 > hi || len <= 1e-12 {
            continue;
        }
        let at = |s: f64| {
            let t = ((s - s0) / len).clamp(0.0, 1.0);
            [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
        };
        let (p, q) = (at(lo.max(s0)), at(hi.min(s1)));
        if out.last().is_none_or(|l| distance(*l, p) > 1e-9) {
            out.push(p);
        }
        out.push(q);
    }
    out
}

/// A vertex nearer the start first, in the walk's queue.
#[derive(PartialEq)]
struct Near(f64, usize);

impl Eq for Near {}

impl Ord for Near {
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.total_cmp(&self.0).then(other.1.cmp(&self.1))
    }
}

impl PartialOrd for Near {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The shortest ways from some vertices: each vertex's distance, and the step it was reached by.
struct Ways {
    dist: Vec<f64>,
    prev: Vec<Option<(usize, Through)>>,
}

impl Ways {
    fn to(&self, v: usize) -> f64 {
        self.dist.get(v).copied().unwrap_or(f64::INFINITY)
    }

    /// The steps from the start to `v`, in order.
    fn steps(&self, v: usize) -> Vec<(usize, usize, Through)> {
        let mut out = Vec::new();
        let mut at = v;
        for _ in 0..self.prev.len() {
            let Some(Some((u, through))) = self.prev.get(at) else { break };
            out.push((*u, at, *through));
            at = *u;
        }
        out.reverse();
        out
    }
}

impl Walk {
    fn new(cx: &Cx, cu: &Copper) -> Walk {
        let board = cx.board;
        let mut edges: Vec<Vec<(usize, f64, Through)>> = vec![Vec::new(); cu.links.len()];
        let mut hub = BTreeMap::new();
        fn join(edges: &mut [Vec<(usize, f64, Through)>], a: usize, b: usize, w: f64, t: Through) {
            if let Some(list) = edges.get_mut(a) {
                list.push((b, w, t));
            }
            if let Some(list) = edges.get_mut(b) {
                list.push((a, w, t));
            }
        }
        for (k, item) in cu.items.iter().enumerate() {
            let links: Vec<usize> = cu.at.get(k).cloned().unwrap_or_default();
            let point = |l: usize| cu.links.get(l).map_or([0.0, 0.0], |(_, _, at)| *at);
            match item {
                Item::Track(t) => {
                    let Some(track) = board.tracks.get(*t) else { continue };
                    let pieces = track_pieces(&track.points, &track.curves);
                    let mut along: Vec<(f64, usize)> = links.iter().map(|l| (position(&pieces, point(*l)), *l)).collect();
                    along.sort_by(|a, b| a.0.total_cmp(&b.0));
                    for w in along.windows(2) {
                        let (Some((sa, la)), Some((sb, lb))) = (w.first(), w.get(1)) else { continue };
                        join(&mut edges, *la, *lb, (sb - sa).abs(), Through::Track(*t, *sa, *sb));
                    }
                }
                Item::Pad(_) | Item::Via(_) | Item::Spokes(..) => {
                    let h = edges.len();
                    edges.push(Vec::new());
                    hub.insert(k, h);
                    let through = if let Item::Via(v) = item { Through::Via(*v) } else { Through::Copper };
                    for l in links {
                        join(&mut edges, h, l, 0.0, through);
                    }
                }
                Item::Fill(..) => {
                    let pts: Vec<(usize, [f64; 2])> = links.iter().map(|l| (*l, point(*l))).collect();
                    for (i, (la, pa)) in pts.iter().enumerate() {
                        let mut near: Vec<(f64, usize)> = pts.iter().skip(i + 1).map(|(lb, pb)| (distance(*pa, *pb), *lb)).collect();
                        if pts.len() > MAX_PAIRWISE {
                            near.sort_by(|a, b| a.0.total_cmp(&b.0));
                            near.truncate(NEAREST);
                        }
                        for (d, lb) in near {
                            join(&mut edges, *la, lb, d, Through::Copper);
                        }
                    }
                }
            }
        }
        Walk { edges, hub }
    }

    /// The shortest ways from `from` through the vertices `open` lets through; lengths counted
    /// only for the steps `counts` says count.
    fn ways(&self, from: &[usize], open: &dyn Fn(usize) -> bool, counts: &dyn Fn(Through) -> bool) -> Ways {
        let n = self.edges.len();
        let mut dist = vec![f64::INFINITY; n];
        let mut prev: Vec<Option<(usize, Through)>> = vec![None; n];
        let mut queue = BinaryHeap::new();
        for v in from.iter().copied() {
            if let Some(slot) = dist.get_mut(v) {
                *slot = 0.0;
                queue.push(Near(0.0, v));
            }
        }
        while let Some(Near(d, u)) = queue.pop() {
            if d > dist.get(u).copied().unwrap_or(f64::INFINITY) {
                continue;
            }
            for (v, w, through) in self.edges.get(u).into_iter().flatten() {
                if !open(*v) {
                    continue;
                }
                let nd = d + if counts(*through) { *w } else { 0.0 };
                if let Some(slot) = dist.get_mut(*v)
                    && nd < *slot
                {
                    *slot = nd;
                    if let Some(p) = prev.get_mut(*v) {
                        *p = Some((u, *through));
                    }
                    queue.push(Near(nd, *v));
                }
            }
        }
        Ways { dist, prev }
    }
}

fn every(_: Through) -> bool {
    true
}

fn run(cx: &mut Cx) {
    let (path, ground) = (cx.wants(PATH), cx.wants(GROUND));
    if !path && !ground {
        return;
    }
    let roles = cx.roles;
    let tvs: Vec<&PartIntent> = roles.parts(PartRole::Tvs).chain(roles.parts(PartRole::EsdArray)).collect();
    if tvs.is_empty() {
        for code in [PATH, GROUND] {
            if cx.wants(code) {
                cx.not_applicable(code, "intent: tvs or esd_array parts with the connector pins they protect (of)");
            }
        }
        return;
    }
    let nets = cx.nets;
    // The nets to walk: those a TVS protects, and its ground.
    let mut which = BTreeSet::new();
    for t in &tvs {
        for p in nets.pads.iter().filter(|p| p.designator == t.part && (p.top || p.bottom)) {
            which.extend(p.net);
        }
    }
    if !cx.spend(nets.pads.len().saturating_add(1)) {
        return;
    }
    let Some(all) = copper(cx, &which) else { return };
    for t in &tvs {
        if !nets.pads.iter().any(|p| p.designator == t.part) {
            for code in [PATH, GROUND] {
                cx.not_applicable(code, format!("geometry: {} placed on the board", t.part));
            }
            continue;
        }
        if path {
            paths(cx, t, &all);
        }
        if ground {
            grounds(cx, t, &all);
        }
    }
}

/// The pads (by index) a TVS record's `of` names: `J1.A6` (a pad's number, or a pin's name or
/// number), or `J1` (all the part's pads).
fn named_pads(cx: &Cx, of: &[String]) -> Vec<usize> {
    let (nets, conn) = (cx.nets, cx.conn);
    let mut out = Vec::new();
    for name in of {
        let (part, pin) = match name.split_once('.') {
            Some((part, pin)) => (part, Some(pin)),
            None => (name.as_str(), None),
        };
        let mine: Vec<usize> = nets.pads.iter().enumerate().filter(|(_, p)| p.designator == part && (p.top || p.bottom)).map(|(i, _)| i).collect();
        let Some(pin) = pin else {
            out.extend(mine);
            continue;
        };
        let by_number = mine.iter().copied().find(|i| nets.pads.get(*i).is_some_and(|p| p.pad.number == pin));
        let by_name = || {
            let number = conn.nets.iter().flat_map(|n| n.pins.iter()).find(|q| q.designator == part && q.name == pin).map(|q| q.pin.as_str())?;
            mine.iter().copied().find(|i| nets.pads.get(*i).is_some_and(|p| p.pad.number == number))
        };
        out.extend(by_number.or_else(by_name));
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// A capacitor's pad: a shunt to ground, which a strike doesn't hurt.
fn capacitor(cx: &Cx, i: usize) -> bool {
    let Some(p) = cx.nets.pads.get(i) else { return false };
    let roles = [PartRole::Decoupling, PartRole::Bulk, PartRole::SmpsInCap, PartRole::SmpsOutCap, PartRole::LoadCap];
    if cx.roles.intent.parts.iter().any(|r| r.part == p.designator && roles.contains(&r.role)) {
        return true;
    }
    let prefix: String = p.designator.chars().take_while(char::is_ascii_alphabetic).collect::<String>().to_ascii_uppercase();
    prefix == "C" && cx.nets.pads.iter().filter(|q| q.footprint == p.footprint && (q.top || q.bottom)).count() == 2
}

fn pad_item(cx: &Cx, i: usize) -> Option<DrcItem> {
    let p = cx.nets.pads.get(i)?;
    Some(DrcItem::Pad { footprint: p.footprint, number: p.pad.number.clone(), reference: p.reference() })
}

fn pad_ref(cx: &Cx, i: usize) -> String {
    cx.nets.pads.get(i).map_or_else(|| "?".to_string(), |p| p.reference())
}

/// Whether copper on `other` at `q` couples to a run on `layer`: the same layer or the next
/// one, or farther with no plane between covering `q` (a plane between shields it).
fn couples(cx: &Cx, layer: CopperLayer, other: CopperLayer, q: [f64; 2]) -> bool {
    let geo = cx.geo;
    let (Some(i), Some(j)) = (geo.layers.iter().position(|l| *l == layer), geo.layers.iter().position(|l| *l == other)) else { return true };
    let (lo, hi) = (i.min(j), i.max(j));
    if hi <= lo + 1 {
        return true;
    }
    !geo.layers.iter().take(hi).skip(lo + 1).any(|l| geo.plane(*l).is_some_and(|pl| pl.nets_at(cx.nets, &[q]).first().copied().flatten().is_some()))
}

fn net_name(cx: &Cx, n: usize) -> String {
    cx.nets.name(n).unwrap_or("?").to_string()
}

/// The layer copper `item` is on, for a track (others are on several or found otherwise).
fn track_layer(cx: &Cx, item: Item) -> Option<CopperLayer> {
    if let Item::Track(t) = item { cx.board.tracks.get(t).map(|t| t.layer) } else { None }
}

fn paths(cx: &mut Cx, t: &PartIntent, all: &BTreeMap<usize, Copper>) {
    let (board, nets, roles) = (cx.board, cx.nets, cx.roles);
    let p = |name: &str| super::param_value(board, PATH, name);
    let (stub, reach, ratio, gap_mm, run_mm, turn) = (p("stub"), p("reach"), p("ratio"), p("gap"), p("run"), p("turn"));
    if t.of.is_empty() {
        cx.not_applicable(PATH, format!("intent: the connector pins {} protects (of)", t.part));
        return;
    }
    let named = named_pads(cx, &t.of);
    let connectors: BTreeSet<&str> = named.iter().filter_map(|i| nets.pads.get(*i)).map(|p| p.designator.as_str()).collect();
    let own: Vec<usize> = nets.pads.iter().enumerate().filter(|(_, p)| p.designator == t.part && (p.top || p.bottom)).map(|(i, _)| i).collect();
    // The nets it protects: those with a named connector pad and a pad of its own, ground aside
    // (`of` may name the whole connector).
    let protected: BTreeSet<usize> = named
        .iter()
        .filter_map(|i| nets.pads.get(*i)?.net)
        .filter(|n| !roles.grounds.contains(n) && own.iter().any(|i| nets.pads.get(*i).is_some_and(|p| p.net == Some(*n))))
        .collect();
    if protected.is_empty() {
        cx.not_applicable(PATH, format!("geometry: a pad of {} on the net of a connector pin it protects", t.part));
        return;
    }
    // Foreign tracks for the coupling test: not the protected lines, not ground.
    let mut foreign = Grid::new(2.0);
    let mut pieces: Vec<([f64; 2], [f64; 2], f64, usize, CopperLayer)> = Vec::new();
    for (ti, tr) in board.tracks.iter().enumerate() {
        let Some(n) = nets.track_nets.get(ti).copied().flatten() else { continue };
        if protected.contains(&n) || roles.grounds.contains(&n) {
            continue;
        }
        for (a, b) in track_pieces(&tr.points, &tr.curves) {
            let r = tr.width / 2.0 + gap_mm;
            foreign.insert(pieces.len(), [[a[0].min(b[0]) - r, a[1].min(b[1]) - r], [a[0].max(b[0]) + r, a[1].max(b[1]) + r]]);
            pieces.push((a, b, tr.width, n, tr.layer));
        }
    }
    if !cx.spend(pieces.len().saturating_add(1)) {
        return;
    }
    for n in protected {
        let Some(cu) = all.get(&n) else { continue };
        if !cx.spend(cu.links.len().saturating_add(cu.items.len()).saturating_mul(8)) {
            return;
        }
        let name = net_name(cx, n);
        let walk = Walk::new(cx, cu);
        let hub_of = |i: usize| cu.find(Item::Pad(i)).and_then(|k| walk.hub.get(&k).copied());
        let tvs_pads: Vec<usize> = own.iter().copied().filter(|i| nets.pads.get(*i).is_some_and(|p| p.net == Some(n))).collect();
        let tvs_hubs: Vec<usize> = tvs_pads.iter().filter_map(|i| hub_of(*i)).collect();
        let conn_pads: Vec<usize> = named.iter().copied().filter(|i| nets.pads.get(*i).is_some_and(|p| p.net == Some(n))).collect();
        let victims: Vec<usize> = nets
            .pads
            .iter()
            .enumerate()
            .filter(|(i, p)| {
                p.net == Some(n) && (p.top || p.bottom) && p.designator != t.part && !connectors.contains(p.designator.as_str()) && !capacitor(cx, *i)
            })
            .map(|(i, _)| i)
            .collect();
        // Where two other items meet inside a TVS pad, they meet through it.
        let inside: BTreeSet<usize> = cu
            .links
            .iter()
            .enumerate()
            .filter(|(_, (a, b, at))| {
                let (Some(ia), Some(ib)) = (cu.items.get(*a), cu.items.get(*b)) else { return false };
                let theirs = |x: &Item| matches!(x, Item::Pad(i) if tvs_pads.contains(i));
                if theirs(ia) || theirs(ib) {
                    return false;
                }
                let layer = track_layer(cx, *ia).or_else(|| track_layer(cx, *ib));
                tvs_pads.iter().filter_map(|i| nets.pads.get(*i)).any(|p| layer.is_none_or(|l| p.on(l)) && p.contains(*at))
            })
            .map(|(l, _)| l)
            .collect();
        let tvs_ref = tvs_pads.first().map_or_else(|| t.part.clone(), |i| pad_ref(cx, *i));
        let Some(tvs_item) = tvs_pads.first().and_then(|i| pad_item(cx, *i)) else { continue };
        // Each connector pin's way to the TVS: its length, vias and the unclamped run.
        let to_tvs = walk.ways(&tvs_hubs, &|_| true, &every);
        let mut worst: Option<(usize, f64, usize)> = None;
        let mut via_pads: Vec<(usize, Vec<u64>)> = Vec::new();
        let mut coupled: BTreeMap<usize, (f64, usize)> = BTreeMap::new();
        let mut sharp: Option<(f64, [f64; 2], usize)> = None;
        let mut unrouted = false;
        for c in &conn_pads {
            let Some(h) = hub_of(*c) else { continue };
            let d1 = to_tvs.to(h);
            if !d1.is_finite() {
                unrouted = true;
                continue;
            }
            if worst.is_none_or(|(_, d, _)| d1 > d) {
                worst = Some((*c, d1, h));
            }
            // The way from the TVS back to the pin, read from the pin's end.
            let steps = to_tvs.steps(h);
            let mut vias: Vec<u64> =
                steps.iter().filter_map(|(_, _, s)| if let Through::Via(v) = s { board.vias.get(*v).map(|v| v.id) } else { None }).collect();
            vias.dedup();
            if !vias.is_empty() {
                via_pads.push((*c, vias));
            }
            let mut runs: Vec<Vec<[f64; 2]>> = Vec::new();
            // Each other net's longest unbroken stretch alongside, and the one it is in now.
            let mut alongside: BTreeMap<usize, f64> = BTreeMap::new();
            let mut stretch: BTreeMap<usize, f64> = BTreeMap::new();
            for (_, _, s) in &steps {
                if let Through::Track(ti, a, b) = s
                    && let Some(tr) = board.tracks.get(*ti)
                {
                    // In the way's direction, so stretches carry on from one track to the next.
                    let mut pts = portion(&track_pieces(&tr.points, &tr.curves), *a, *b);
                    if a > b {
                        pts.reverse();
                    }
                    let half = tr.width / 2.0;
                    let run_layer = tr.layer;
                    let samples: Vec<[f64; 2]> =
                        pts.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?))).flat_map(|(a, b)| along(a, b, 0.1)).collect();
                    if !cx.spend(samples.len().saturating_mul(4).saturating_add(1)) {
                        return;
                    }
                    // Each sample stands for the run to the next one.
                    for w in samples.windows(2) {
                        let (Some(q), Some(r)) = (w.first(), w.get(1)) else { continue };
                        let mut hit: BTreeSet<usize> = BTreeSet::new();
                        let near: Vec<usize> = foreign.near(*q).collect();
                        if !cx.spend(near.len()) {
                            return;
                        }
                        for k in near {
                            let Some((a, b, width, net, layer)) = pieces.get(k) else { continue };
                            if super::super::path::segment_distance(*q, *a, *b, 0.0) - width / 2.0 - half <= gap_mm
                                && couples(cx, run_layer, *layer, *q)
                            {
                                hit.insert(*net);
                            }
                        }
                        stretch.retain(|x, _| hit.contains(x));
                        for x in hit {
                            let len = stretch.entry(x).or_default();
                            *len += distance(*q, *r);
                            let best = alongside.entry(x).or_default();
                            *best = best.max(*len);
                        }
                    }
                    runs.push(pts);
                }
            }
            for (x, len) in alongside {
                let slot = coupled.entry(x).or_insert((0.0, *c));
                if len > slot.0 {
                    *slot = (len, *c);
                }
            }
            for pts in runs {
                for w in pts.windows(3) {
                    let (Some(a), Some(m), Some(b)) = (w.first(), w.get(1), w.get(2)) else { continue };
                    let (u, v) = ([m[0] - a[0], m[1] - a[1]], [b[0] - m[0], b[1] - m[1]]);
                    let (lu, lv) = (u[0].hypot(u[1]), v[0].hypot(v[1]));
                    if lu <= 1e-9 || lv <= 1e-9 {
                        continue;
                    }
                    let angle = ((u[0] * v[0] + u[1] * v[1]) / (lu * lv)).clamp(-1.0, 1.0).acos().to_degrees();
                    if angle > turn + 1e-6 && sharp.is_none_or(|(s, _, _)| angle > s) {
                        sharp = Some((angle, *m, *c));
                    }
                }
            }
        }
        let regime = format!("{} protects {name} from {}", t.part, connectors.iter().copied().collect::<Vec<_>>().join(", "));
        // A protected pad reached without passing the TVS (with no copper to the TVS at all too).
        let blocked: BTreeSet<usize> = tvs_hubs.iter().copied().chain(inside.iter().copied()).collect();
        let conn_hubs: Vec<usize> = conn_pads.iter().filter_map(|i| hub_of(*i)).collect();
        let around = walk.ways(&conn_hubs, &|v| !blocked.contains(&v), &every);
        let mut reached: Vec<(usize, usize)> =
            victims.iter().filter_map(|i| Some((*i, hub_of(*i)?))).filter(|(_, h)| around.to(*h).is_finite()).collect();
        reached.sort_by(|a, b| around.to(a.1).total_cmp(&around.to(b.1)).then(a.0.cmp(&b.0)));
        if unrouted && reached.is_empty() {
            cx.not_applicable(PATH, format!("geometry: {name} routed from the connector to {}", t.part));
        }
        for (victim, vh) in reached.into_iter().take(MAX_VICTIMS) {
            let line: BTreeSet<usize> = around.steps(vh).iter().flat_map(|(u, v, _)| [*u, *v]).collect();
            let branch = line.iter().map(|v| to_tvs.to(*v)).fold(f64::INFINITY, f64::min);
            if branch > stub {
                let start = around.steps(vh).first().map(|(u, _, _)| *u);
                let Some(from) = conn_pads.iter().copied().find(|i| hub_of(*i) == start).or_else(|| conn_pads.first().copied()) else { continue };
                let (from_ref, victim_ref) = (pad_ref(cx, from), pad_ref(cx, victim));
                let items: Vec<DrcItem> = [from, victim].iter().filter_map(|i| pad_item(cx, *i)).chain([tvs_item.clone()]).collect();
                let keys: Vec<String> = items.iter().map(DrcItem::key).collect();
                let at = nets.pads.get(victim).map_or([0.0, 0.0], |p| p.at());
                let (name, regime, tvs_ref) = (name.clone(), regime.clone(), tvs_ref.clone());
                cx.add(PATH, &keys, || {
                    let says = if branch.is_finite() {
                        format!(
                            "{from_ref} reaches {victim_ref} ({name}) without passing {tvs_ref}: the TVS hangs off the line on a {} mm branch ({} mm allowed)",
                            num((branch * 100.0).round() / 100.0),
                            num(stub)
                        )
                    } else {
                        format!("{from_ref} reaches {victim_ref} ({name}), and no copper joins the line to {tvs_ref}")
                    };
                    let d = Draft::new(at, items, says)
                    .nets(vec![name.clone()])
                    .regime(regime)
                    .arg("net", name.clone())
                    .arg("tvs", tvs_ref.clone())
                    .fix(
                        "ecad.board.unroute",
                        Value::Null,
                        format!("route {name} from {from_ref} over {tvs_ref}'s pad and only then on to {victim_ref} (a branch of at most {} mm)", num(stub)),
                    );
                    if branch.is_finite() { d.measure(branch, stub, "mm", Bound::Max) } else { d.variant("unjoined") }
                });
            }
        }
        let Some((wc, d1, _)) = worst else { continue };
        let (wc_ref, wc_item) = (pad_ref(cx, wc), pad_item(cx, wc));
        // A via between the pin and the TVS.
        if let Some((c, vias)) = via_pads.first().cloned() {
            let all_vias: Vec<u64> = via_pads.iter().flat_map(|(_, v)| v.iter().copied()).collect::<BTreeSet<_>>().into_iter().collect();
            let pins: Vec<String> = via_pads.iter().map(|(i, _)| pad_ref(cx, *i)).collect();
            let items: Vec<DrcItem> =
                pad_item(cx, c).into_iter().chain([tvs_item.clone()]).chain(all_vias.iter().map(|v| DrcItem::Via(*v))).collect();
            let mut keys: Vec<String> = vec![tvs_item.key(), format!("{name}|via")];
            keys.extend(via_pads.iter().filter_map(|(i, _)| pad_item(cx, *i)).map(|x| x.key()));
            let at = board.vias.iter().find(|v| vias.first() == Some(&v.id)).map_or([0.0, 0.0], |v| v.at);
            let (name, regime, tvs_ref) = (name.clone(), regime.clone(), tvs_ref.clone());
            let list = all_vias.iter().map(u64::to_string).collect::<Vec<_>>().join(", ");
            cx.add(PATH, &keys, || {
                Draft::new(
                    at,
                    items,
                    format!("the way from {} to {tvs_ref} ({name}) changes layer through via {list} before the TVS: a via there adds its inductance to the clamp", pins.join(", ")),
                )
                .nets(vec![name.clone()])
                .variant("via")
                .severity(Severity::Warning)
                .measure(all_vias.len() as f64, 0.0, "vias", Bound::Max)
                .param("")
                .regime(regime)
                .arg("net", name.clone())
                .arg("vias", list.clone())
                .fix("ecad.board.unroute", Value::Null, format!("route {name} to {tvs_ref} on the connector's layer; change layer after the TVS"))
            });
        }
        // The TVS far along the copper from the pin.
        if d1 > reach {
            let items: Vec<DrcItem> = wc_item.clone().into_iter().chain([tvs_item.clone()]).collect();
            let keys = [tvs_item.key(), format!("{name}|length")];
            let at = nets.pads.get(wc).map_or([0.0, 0.0], |p| p.at());
            let (name, regime, wc_ref, tvs_ref) = (name.clone(), regime.clone(), wc_ref.clone(), tvs_ref.clone());
            let part = t.part.clone();
            cx.add(PATH, &keys, || {
                Draft::new(
                    at,
                    items,
                    format!(
                        "{tvs_ref} is {} mm along the copper from {wc_ref} ({name}); {} mm at most",
                        num((d1 * 100.0).round() / 100.0),
                        num(reach)
                    ),
                )
                .nets(vec![name.clone()])
                .variant("length")
                .severity(Severity::Warning)
                .measure(d1, reach, "mm", Bound::Max)
                .param("reach")
                .regime(regime)
                .arg("net", name.clone())
                .fix("ecad.board.move", Value::Null, format!("move {part} next to the connector, on the line from {wc_ref}"))
            });
        }
        // The protected pad near the TVS.
        let from_tvs = &to_tvs;
        let nearest =
            victims.iter().filter_map(|i| Some((*i, from_tvs.to(hub_of(*i)?)))).filter(|(_, d)| d.is_finite()).min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((victim, d2)) = nearest
            && d2 < ratio * d1
        {
            let items: Vec<DrcItem> = [tvs_item.clone()].into_iter().chain(pad_item(cx, victim)).collect();
            let keys: Vec<String> = items.iter().map(DrcItem::key).chain([format!("{name}|ratio")]).collect();
            let at = nets.pads.get(victim).map_or([0.0, 0.0], |p| p.at());
            let victim_ref = pad_ref(cx, victim);
            let (name, regime, wc_ref) = (name.clone(), regime.clone(), wc_ref.clone());
            let part = t.part.clone();
            cx.add(PATH, &keys, || {
                Draft::new(
                    at,
                    items,
                    format!(
                        "{victim_ref} is {} mm along the copper from the TVS ({name}), less than {}× the {} mm from {wc_ref} to the TVS: the strike splits toward it",
                        num((d2 * 100.0).round() / 100.0),
                        num(ratio),
                        num((d1 * 100.0).round() / 100.0)
                    ),
                )
                .nets(vec![name.clone()])
                .variant("ratio")
                .severity(Severity::Warning)
                .measure(d2, ratio * d1, "mm", Bound::Min)
                .param("ratio")
                .regime(regime)
                .arg("net", name.clone())
                .arg("ratio", num(ratio))
                .arg("d1", num((d1 * 100.0).round() / 100.0))
                .fix("ecad.board.move", Value::Null, format!("move {part} nearer the connector, or {victim_ref}'s part farther from it"))
            });
        }
        // Another net alongside the unclamped run.
        for (other, (len, c)) in coupled {
            if len <= run_mm {
                continue;
            }
            let other_name = net_name(cx, other);
            let items: Vec<DrcItem> = pad_item(cx, c).into_iter().chain([tvs_item.clone()]).collect();
            let keys = [tvs_item.key(), format!("{name}|{other_name}")];
            let at = nets.pads.get(c).map_or([0.0, 0.0], |p| p.at());
            let (name, regime, c_ref, tvs_ref) = (name.clone(), regime.clone(), pad_ref(cx, c), tvs_ref.clone());
            cx.add(PATH, &keys, || {
                Draft::new(
                    at,
                    items,
                    format!("{other_name} runs within {} mm of {name}'s unclamped run from {c_ref} to {tvs_ref} for {} mm: the strike couples into it before the TVS clamps", num(gap_mm), num((len * 10.0).round() / 10.0)),
                )
                .nets(vec![name.clone(), other_name.clone()])
                .variant("coupled")
                .severity(Severity::Warning)
                .measure(len, run_mm, "mm", Bound::Max)
                .param("run")
                .regime(regime)
                .arg("net", name.clone())
                .arg("other", other_name.clone())
                .arg("gap", num(gap_mm))
                .fix("ecad.board.net.inspect", json!({"nets": [other_name], "metrics": ["segments"]}), format!("move {other_name} away from the connector-to-TVS run"))
            });
        }
        // A sharp turn in the unclamped run.
        if let Some((angle, at, c)) = sharp {
            let items: Vec<DrcItem> = pad_item(cx, c).into_iter().chain([tvs_item.clone()]).collect();
            let keys = [tvs_item.key(), format!("{name}|turn")];
            let (name, regime, c_ref, tvs_ref) = (name.clone(), regime.clone(), pad_ref(cx, c), tvs_ref.clone());
            cx.add(PATH, &keys, || {
                Draft::new(
                    at,
                    items,
                    format!("{name}'s unclamped run from {c_ref} to {tvs_ref} turns by {}° at ({}, {})", num(angle.round()), num(at[0]), num(at[1])),
                )
                .nets(vec![name.clone()])
                .variant("turn")
                .severity(Severity::Info)
                .class(Class::Practical)
                .review()
                .measure(angle, turn, "°", Bound::Max)
                .param("turn")
                .regime(regime)
                .arg("net", name.clone())
                .arg("angle", num(angle.round()))
            });
        }
    }
}

/// A ball-grid package: four or more pads, all round.
fn ball_grid(cx: &Cx, part: &str) -> bool {
    let pads: Vec<_> = cx.nets.pads.iter().filter(|p| p.designator == part && (p.top || p.bottom)).collect();
    pads.len() >= 4 && pads.iter().all(|p| p.pad.shape == PadShape::Round && p.pad.drill.is_none())
}

fn grounds(cx: &mut Cx, t: &PartIntent, all: &BTreeMap<usize, Copper>) {
    let (board, nets, roles) = (cx.board, cx.nets, cx.roles);
    let p = |name: &str| super::param_value(board, GROUND, name);
    let (reach, want, track_max, chassis) = (p("reach"), p("vias").max(1.0), p("track"), p("chassis"));
    if roles.grounds.is_empty() {
        cx.not_applicable(GROUND, "intent: the ground net (a net record with role ground)");
        return;
    }
    if ball_grid(cx, &t.part) {
        return;
    }
    let layers = board.copper_layers;
    let named = named_pads(cx, &t.of);
    let connectors: BTreeSet<&str> = named.iter().filter_map(|i| nets.pads.get(*i)).map(|p| p.designator.as_str()).collect();
    let pads: Vec<usize> = nets
        .pads
        .iter()
        .enumerate()
        .filter(|(_, p)| p.designator == t.part && p.net.is_some_and(|n| roles.grounds.contains(&n)))
        .map(|(i, _)| i)
        .collect();
    if pads.is_empty() {
        cx.not_applicable(GROUND, format!("intent: the ground role of the net on {}'s ground pin", t.part));
        return;
    }
    for gi in pads {
        let Some(g) = nets.pads.get(gi) else { continue };
        let Some(n) = g.net else { continue };
        let Some(gp) = pad_item(cx, gi) else { continue };
        let name = net_name(cx, n);
        let layer = if g.top { CopperLayer::Top } else { CopperLayer::Bottom };
        let core = pad_core(g);
        if !cx.spend(board.vias.len().saturating_add(1)) {
            return;
        }
        let near: Vec<(u64, f64)> = board
            .vias
            .iter()
            .enumerate()
            .filter(|(k, v)| nets.via_nets.get(*k).copied().flatten() == Some(n) && v.on(layer, layers))
            .map(|(_, v)| (v.id, gap(&core, &disc(v.at, 0.0)).max(0.0)))
            .filter(|(_, d)| *d <= reach)
            .collect();
        let count = near.len();
        // The track from the pad to its first via.
        if let Some(cu) = all.get(&n)
            && !cx.spend(cu.links.len().saturating_add(cu.items.len()).saturating_mul(8))
        {
            return;
        }
        let first = all.get(&n).and_then(|cu| {
            let walk = Walk::new(cx, cu);
            let start = cu.find(Item::Pad(gi)).and_then(|k| walk.hub.get(&k).copied())?;
            let ways = walk.ways(&[start], &|_| true, &|s| matches!(s, Through::Track(..)));
            cu.items.iter().enumerate().filter(|(_, it)| matches!(it, Item::Via(_))).filter_map(|(k, _)| walk.hub.get(&k)).map(|h| ways.to(*h)).fold(
                None,
                |m: Option<f64>, d| {
                    if d.is_finite() { Some(m.map_or(d, |m| m.min(d))) } else { m }
                },
            )
        });
        let reference = g.reference();
        let regime = format!("{} is a TVS; {reference} is its ground pad on {}", t.part, layer.name());
        let fix_via = format!("put {} {name} vias within {} mm of {reference}'s edge, straight into the plane", num(want), num(reach));
        if count == 0 {
            let (name, regime, gp2, fix_via) = (name.clone(), regime.clone(), gp.clone(), fix_via.clone());
            cx.add(GROUND, &[gp.key(), "none".into()], || {
                let far = first
                    .map_or("none is joined to it".to_string(), |d| format!("the first is {} mm of track away", num((d * 100.0).round() / 100.0)));
                Draft::new(g.at(), vec![gp2], format!("TVS ground pad {reference} ({name}) has no {name} via within {} mm: {far}", num(reach)))
                    .layer(layer)
                    .nets(vec![name.clone()])
                    .variant("none")
                    .measure(0.0, want, "vias", Bound::Min)
                    .param("vias")
                    .regime(regime)
                    .arg("net", name.clone())
                    .arg("reach", num(reach))
                    .fix("ecad.board.via", Value::Null, fix_via)
            });
        } else if first.is_none_or(|d| d > track_max) {
            let (name, regime, gp2) = (name.clone(), regime.clone(), gp.clone());
            let d = first.unwrap_or(f64::INFINITY);
            cx.add(GROUND, &[gp.key(), "track".into()], || {
                let far = if d.is_finite() { format!("{} mm of track", num((d * 100.0).round() / 100.0)) } else { "no copper".to_string() };
                let x = Draft::new(
                    g.at(),
                    vec![gp2],
                    format!("TVS ground pad {reference} ({name}) reaches its first {name} via through {far}; {} mm at most", num(track_max)),
                )
                .layer(layer)
                .nets(vec![name.clone()])
                .param("track")
                .regime(regime)
                .arg("net", name.clone())
                .fix("ecad.board.track", Value::Null, format!("join {reference} straight to a {name} via at its edge"));
                if d.is_finite() { x.variant("track").measure(d, track_max, "mm", Bound::Max) } else { x.variant("unjoined") }
            });
        } else if (count as f64) < want {
            let (name, regime, gp2, fix_via) = (name.clone(), regime.clone(), gp.clone(), fix_via.clone());
            let ids: Vec<DrcItem> = near.iter().map(|(id, _)| DrcItem::Via(*id)).collect();
            cx.add(GROUND, &[gp.key(), "few".into()], || {
                Draft::new(
                    g.at(),
                    [gp2].into_iter().chain(ids).collect(),
                    format!(
                        "TVS ground pad {reference} ({name}) has {count} {name} via{} within {} mm of its edge; {} or more",
                        if count == 1 { "" } else { "s" },
                        num(reach),
                        num(want)
                    ),
                )
                .layer(layer)
                .nets(vec![name.clone()])
                .severity(Severity::Warning)
                .measure(count as f64, want, "vias", Bound::Min)
                .param("vias")
                .regime(regime)
                .arg("net", name.clone())
                .arg("reach", num(reach))
                .fix("ecad.board.via", Value::Null, fix_via)
            });
        }
        // Spokes between the pad and its pour.
        for fill in nets.fills.iter().filter(|f| f.net == Some(n) && f.layer == layer) {
            if !fill.reliefs.iter().any(|r| r.pad == gi && !r.solid && r.connected > 0) {
                continue;
            }
            let (name, regime, gp2, pour) = (name.clone(), regime.clone(), gp.clone(), fill.pour);
            let (designator, number) = (g.designator.clone(), g.pad.number.clone());
            cx.add(GROUND, &[gp.key(), DrcItem::Pour(pour).key()], || {
                Draft::new(
                    g.at(),
                    vec![gp2, DrcItem::Pour(pour)],
                    format!("TVS ground pad {reference} joins pour {pour} ({name}) through thermal spokes: their inductance is in the clamp path"),
                )
                .layer(layer)
                .nets(vec![name.clone()])
                .variant("relief")
                .regime(regime)
                .arg("net", name.clone())
                .arg("pour", pour.to_string())
                .fix(
                    "ecad.board.pour.connect",
                    json!({"footprint": designator, "pads": [number], "connect": "solid"}),
                    format!("join {reference} to the pour solid"),
                )
            });
        }
        // The protected connector's shell or ground pin.
        let shell = nets
            .pads
            .iter()
            .filter(|q| connectors.contains(q.designator.as_str()) && q.net.is_some_and(|x| roles.grounds.contains(&x)))
            .map(|q| {
                (
                    q.reference(),
                    distance(q.at(), g.at()),
                    DrcItem::Pad { footprint: q.footprint, number: q.pad.number.clone(), reference: q.reference() },
                )
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((shell_ref, d, shell_item)) = shell
            && d > chassis
        {
            let (name, regime, gp2) = (name.clone(), regime.clone(), gp.clone());
            cx.add(GROUND, &[gp.key(), shell_item.key()], || {
                Draft::new(
                    g.at(),
                    vec![gp2, shell_item],
                    format!(
                        "TVS ground pad {reference} is {} mm from {shell_ref}, the nearest shell or ground pin of the connector it protects",
                        num((d * 10.0).round() / 10.0)
                    ),
                )
                .layer(layer)
                .nets(vec![name.clone()])
                .variant("chassis")
                .severity(Severity::Info)
                .class(Class::Practical)
                .review()
                .measure(d, chassis, "mm", Bound::Max)
                .param("chassis")
                .regime(regime)
                .arg("net", name.clone())
            });
        }
    }
}
