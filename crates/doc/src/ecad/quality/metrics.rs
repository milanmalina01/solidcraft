//! The numbers behind the quality findings, for an agent to see what a fix changed: per net its
//! length by layer, delay, vias, bends, each segment's reference plane and its coverage, the
//! dielectric height and impedance, the neighbours it couples to, and its pair's skew; per
//! current loop (the intent's loops, and a decoupling loop for each pin a decoupling capacitor
//! serves) its members, path, length, area, vias, layers and estimated inductance, ranked.
//!
//! Delay uses the effective εr of the layer's line (microstrip outside, stripline inside, from
//! the stack-up: see [`super::super::impedance`]): about 3.336·√εr_eff ps/mm. A loop's
//! inductance is the estimate for a run over a plane, μ0·h·(perimeter/2)/w, with h the
//! dielectric to its reference plane and w its members' narrowest pad: good to rank loops and
//! to see a change help, not a measurement. Work is bounded by the caps below and a budget.

use std::collections::{BTreeMap, BTreeSet};

use super::super::Electronics;
use super::super::airwires::{BoardNets, BoardPad};
use super::super::connect::Connectivity;
use super::super::impedance::{self, LineKind};
use super::super::intent::{LoopKind, PartRole};
use super::super::layers::CopperLayer;
use super::super::path;
use super::super::stackup::StackLayer;
use super::geom::{Geo, along, distance, track_pieces};
use super::roles::Roles;

/// The most nets one call measures.
pub const MAX_NETS: usize = 256;
/// The most segments listed for a net.
pub const MAX_SEGMENTS: usize = 256;
/// The most neighbours listed for a net.
pub const MAX_NEIGHBOURS: usize = 16;
/// The most loops one call measures.
pub const MAX_LOOPS: usize = 512;
/// The work one call may do.
pub const MAX_WORK: usize = 1 << 24;
/// μ0 in nH/mm.
const MU0: f64 = 1.256_637;
/// ps per mm in a vacuum: 1/c.
const PS_PER_MM: f64 = 3.335_641;
/// How far apart a segment's reference is looked at, in millimetres.
const REF_STEP: f64 = 0.1;

/// Which metrics to work out (the cheap ones always are).
#[derive(Clone, Copy, Debug, Default)]
pub struct Want {
    /// Each segment's reference plane, coverage and height.
    pub reference: bool,
    /// Each segment's impedance.
    pub impedance: bool,
    /// The neighbours on the same layer and how far they run alongside.
    pub coupling: bool,
    /// List the segments.
    pub segments: bool,
}

/// Turns along a net's tracks, by angle.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bends {
    pub deg45: usize,
    pub deg90: usize,
    /// Sharper than a right angle.
    pub acute: usize,
    /// Other angles.
    pub other: usize,
    /// Straight segments longer than 0.5 mm off the 0/45/90° directions.
    pub any_angle: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SegmentMetric {
    pub track: u64,
    pub layer: CopperLayer,
    pub width: f64,
    pub length: f64,
    pub delay_ps: f64,
    pub z0: Option<f64>,
    /// The plane it references, that plane's net, the share of it over the plane, and the
    /// dielectric between.
    pub reference: Option<CopperLayer>,
    pub ref_net: Option<String>,
    pub ref_coverage: Option<f64>,
    pub h: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Neighbour {
    pub net: String,
    pub layer: CopperLayer,
    /// How far it runs alongside within the window, and the closest edge-to-edge gap.
    pub coupled_mm: f64,
    pub min_gap_mm: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PairMetric {
    pub name: String,
    pub partner: String,
    pub skew_mm: f64,
    pub skew_ps: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetMetrics {
    pub net: String,
    pub role: Option<&'static str>,
    pub tier: Option<&'static str>,
    /// The tier and where it comes from.
    pub regime: String,
    pub length_mm: f64,
    pub by_layer: Vec<(CopperLayer, f64)>,
    pub delay_ps: f64,
    pub vias: usize,
    pub bends: Bends,
    pub segments: Vec<SegmentMetric>,
    pub segments_more: usize,
    pub neighbours: Vec<Neighbour>,
    pub pair: Option<PairMetric>,
}

/// One straight piece of copper track.
struct Piece {
    track: usize,
    net: Option<usize>,
    layer: CopperLayer,
    a: [f64; 2],
    b: [f64; 2],
    width: f64,
}

/// The effective εr, impedance and dielectric height of a `width` track on `layer` of a board of
/// `copper_layers` with the stack-up `stack` (by the stack-up's formula).
fn line(stack: &[StackLayer], copper_layers: u8, layer: CopperLayer, width: f64) -> (f64, Option<f64>, Option<f64>) {
    let Ok(m) = impedance::medium(stack, layer, copper_layers) else { return (super::super::stackup::DEFAULT_ER, None, None) };
    let kind = if m.outer { LineKind::Microstrip } else { LineKind::Stripline };
    match impedance::impedance(&m, kind, width, None) {
        Ok(z) => (z.er_eff.unwrap_or(m.er), Some(z.z0), Some(m.h1)),
        Err(_) => (m.er, None, Some(m.h1)),
    }
}

/// Metrics of the nets `which` (board net indexes) on `ecad`'s board with connections `nets`;
/// `true` when past the budget or the caps.
pub fn net_metrics(ecad: &Electronics, nets: &BoardNets, which: &[usize], want: Want) -> (Vec<NetMetrics>, bool) {
    let board = &ecad.board;
    let conn: Connectivity = ecad.connectivity();
    let roles = Roles::new(ecad, nets, &conn);
    let geo = Geo::new(board, nets);
    let stack = board.stackup_or_default();
    let mut work = MAX_WORK;
    let mut truncated = which.len() > MAX_NETS;
    let mut spend = |n: usize, truncated: &mut bool| -> bool {
        if work >= n {
            work -= n;
            true
        } else {
            *truncated = true;
            false
        }
    };
    let pieces: Vec<Piece> = board
        .tracks
        .iter()
        .enumerate()
        .flat_map(|(ti, t)| {
            let net = nets.track_nets.get(ti).copied().flatten();
            track_pieces(&t.points, &t.curves).into_iter().map(move |(a, b)| Piece { track: ti, net, layer: t.layer, a, b, width: t.width })
        })
        .collect();
    let mut grid = super::super::grid::Grid::new(2.0);
    for (i, p) in pieces.iter().enumerate() {
        let bbox = [[p.a[0].min(p.b[0]), p.a[1].min(p.b[1])], [p.a[0].max(p.b[0]), p.a[1].max(p.b[1])]];
        grid.insert(i, super::super::grid::grow(bbox, p.width / 2.0));
    }
    // Length and delay of every net, for pairs' skew.
    let mut length_of: BTreeMap<usize, (f64, f64)> = BTreeMap::new();
    for p in &pieces {
        let Some(n) = p.net else { continue };
        let len = distance(p.a, p.b);
        let (er, _, _) = line(&stack, board.copper_layers, p.layer, p.width);
        let e = length_of.entry(n).or_default();
        e.0 += len;
        e.1 += len * PS_PER_MM * er.max(1.0).sqrt();
    }
    let mut out = Vec::new();
    for net in which.iter().copied().take(MAX_NETS) {
        let Some(info) = roles.net(net) else { continue };
        let mine: Vec<&Piece> = pieces.iter().filter(|p| p.net == Some(net)).collect();
        if !spend(mine.len().saturating_add(1), &mut truncated) {
            break;
        }
        let mut by_layer: BTreeMap<CopperLayer, f64> = BTreeMap::new();
        let mut segments = Vec::new();
        let mut segments_more = 0usize;
        let mut delay = 0.0;
        for p in &mine {
            let len = distance(p.a, p.b);
            *by_layer.entry(p.layer).or_default() += len;
            let (er, z0, h) = line(&stack, board.copper_layers, p.layer, p.width);
            let d = len * PS_PER_MM * er.max(1.0).sqrt();
            delay += d;
            if !want.segments && !want.reference && !want.impedance {
                continue;
            }
            if segments.len() >= MAX_SEGMENTS {
                segments_more += 1;
                continue;
            }
            let Some(t) = board.tracks.get(p.track) else { continue };
            let mut seg = SegmentMetric {
                track: t.id,
                layer: p.layer,
                width: p.width,
                length: len,
                delay_ps: d,
                z0: if want.impedance { z0 } else { None },
                reference: None,
                ref_net: None,
                ref_coverage: None,
                h,
            };
            if want.reference {
                let pts = along(p.a, p.b, REF_STEP.max(len / 200.0));
                if !spend(pts.len().saturating_mul(4), &mut truncated) {
                    break;
                }
                if let Some(u) = geo.under(p.layer, &pts, Some(net), 0.05) {
                    seg.reference = Some(u.reference.layer);
                    seg.ref_net = u.reference.net.and_then(|n| nets.name(n)).map(str::to_string);
                    seg.ref_coverage = Some(u.reference.coverage);
                    seg.h = Some(u.reference.h);
                }
            }
            segments.push(seg);
        }
        let vias = nets.via_nets.iter().filter(|n| **n == Some(net)).count();
        let bends = bends_of(board, nets, net);
        let mut neighbours: Vec<Neighbour> = Vec::new();
        if want.coupling {
            let mut by: BTreeMap<(usize, CopperLayer), (f64, f64)> = BTreeMap::new();
            for p in &mine {
                let (_, _, h) = line(&stack, board.copper_layers, p.layer, p.width);
                let window = (3.0 * h.unwrap_or(0.2)).max(1.0);
                let bbox = super::super::grid::grow([[p.a[0].min(p.b[0]), p.a[1].min(p.b[1])], [p.a[0].max(p.b[0]), p.a[1].max(p.b[1])]], window);
                let near = grid.near_box(bbox, pieces.len());
                if !spend(near.len().saturating_add(1), &mut truncated) {
                    break;
                }
                for q in near.into_iter().filter_map(|i| pieces.get(i)) {
                    let Some(other) = q.net.filter(|n| *n != net) else { continue };
                    if q.layer != p.layer {
                        continue;
                    }
                    if let Some((run, gap)) = alongside(p, q, window) {
                        let e = by.entry((other, q.layer)).or_insert((0.0, f64::INFINITY));
                        e.0 += run;
                        e.1 = e.1.min(gap);
                    }
                }
            }
            let mut list: Vec<Neighbour> = by
                .into_iter()
                .filter_map(|((n, layer), (run, gap))| {
                    Some(Neighbour { net: nets.name(n)?.to_string(), layer, coupled_mm: run, min_gap_mm: gap.max(0.0) })
                })
                .collect();
            list.sort_by(|a, b| b.coupled_mm.total_cmp(&a.coupled_mm));
            list.truncate(MAX_NEIGHBOURS);
            neighbours = list;
        }
        let pair = roles.pairs().iter().find(|x| x.p == info.name || x.n == info.name).and_then(|x| {
            let partner = if x.p == info.name { &x.n } else { &x.p };
            let other = roles.by_name(partner)?;
            let (a, b) = (length_of.get(&net).copied().unwrap_or_default(), length_of.get(&other.index).copied().unwrap_or_default());
            Some(PairMetric { name: x.name.clone(), partner: partner.clone(), skew_mm: (a.0 - b.0).abs(), skew_ps: (a.1 - b.1).abs() })
        });
        out.push(NetMetrics {
            net: info.name.to_string(),
            role: info.role().map(|r| r.name()),
            tier: info.tier.map(|t| t.name()),
            regime: info.regime(),
            length_mm: by_layer.values().sum(),
            by_layer: by_layer.into_iter().collect(),
            delay_ps: delay,
            vias,
            bends,
            segments,
            segments_more,
            neighbours,
            pair,
        });
    }
    (out, truncated)
}

/// How far `q` runs alongside `p` (parallel within 30°, within `window` of it), and the edge
/// gap between them; `None` when it doesn't.
fn alongside(p: &Piece, q: &Piece, window: f64) -> Option<(f64, f64)> {
    let len = distance(p.a, p.b);
    let lq = distance(q.a, q.b);
    if len <= 1e-9 || lq <= 1e-9 {
        return None;
    }
    let d = [(p.b[0] - p.a[0]) / len, (p.b[1] - p.a[1]) / len];
    let e = [(q.b[0] - q.a[0]) / lq, (q.b[1] - q.a[1]) / lq];
    let cos = (d[0] * e[0] + d[1] * e[1]).abs();
    if cos < 30f64.to_radians().cos() {
        return None;
    }
    let t = |x: [f64; 2]| (x[0] - p.a[0]) * d[0] + (x[1] - p.a[1]) * d[1];
    let (t0, t1) = (t(q.a).min(t(q.b)).max(0.0), t(q.a).max(t(q.b)).min(len));
    if t1 <= t0 {
        return None;
    }
    let centre = path::segment_distance(q.a, p.a, p.b, 0.0).min(path::segment_distance(q.b, p.a, p.b, 0.0));
    let gap = centre - (p.width + q.width) / 2.0;
    if gap <= window { Some((t1 - t0, gap)) } else { None }
}

/// The turns along `net`'s tracks (straight segments only).
fn bends_of(board: &super::super::board::Board, nets: &BoardNets, net: usize) -> Bends {
    let mut b = Bends::default();
    for (ti, t) in board.tracks.iter().enumerate() {
        if nets.track_nets.get(ti).copied().flatten() != Some(net) {
            continue;
        }
        let straight = |i: usize| path::sweep(&t.curves, i).abs() < 1e-9;
        for (i, w) in t.points.windows(2).enumerate() {
            let (Some(a), Some(c)) = (w.first(), w.get(1)) else { continue };
            if straight(i) && distance(*a, *c) > 0.5 {
                let ang = (c[1] - a[1]).atan2(c[0] - a[0]).to_degrees().rem_euclid(45.0);
                if ang.min(45.0 - ang) > 1.0 {
                    b.any_angle += 1;
                }
            }
        }
        for (i, w) in t.points.windows(3).enumerate() {
            let (Some(a), Some(m), Some(c)) = (w.first(), w.get(1), w.get(2)) else { continue };
            if !(straight(i) && straight(i + 1)) {
                continue;
            }
            let (u, v) = ([m[0] - a[0], m[1] - a[1]], [c[0] - m[0], c[1] - m[1]]);
            let (lu, lv) = (u[0].hypot(u[1]), v[0].hypot(v[1]));
            if lu <= 1e-9 || lv <= 1e-9 {
                continue;
            }
            let turn = ((u[0] * v[0] + u[1] * v[1]) / (lu * lv)).clamp(-1.0, 1.0).acos().to_degrees();
            if turn < 2.0 {
                continue;
            } else if (turn - 45.0).abs() <= 2.0 {
                b.deg45 += 1;
            } else if (turn - 90.0).abs() <= 2.0 {
                b.deg90 += 1;
            } else if turn > 92.0 {
                b.acute += 1;
            } else {
                b.other += 1;
            }
        }
    }
    b
}

/// A loop member: a pin, where its pad is.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    /// `C3.1`, as named.
    pub pin: String,
    pub at: Option<[f64; 2]>,
    pub net: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoopMetrics {
    pub name: String,
    pub kind: &'static str,
    /// `intent` (a loop record) or `decoupling` (made from a decoupling capacitor's record).
    pub from: &'static str,
    pub members: Vec<Member>,
    /// The members' pads in order, closed back to the first.
    pub path: Vec<[f64; 2]>,
    pub length_mm: f64,
    pub area_mm2: f64,
    /// Vias of the members' nets inside the loop (or within 0.5 mm of its path).
    pub vias: usize,
    pub layers: Vec<CopperLayer>,
    /// The dielectric to its reference plane, how much of the loop is over it, and the estimate.
    pub h: Option<f64>,
    pub ref_coverage: Option<f64>,
    /// The share of the loop's hull (its members' pads) over a ground plane on a layer next to
    /// its side: what `pi.smps_hot_loop` judges; `None` with no plane there.
    pub ground_under: Option<f64>,
    pub est_nh: Option<f64>,
    /// 1 for the largest estimate.
    pub rank: usize,
    /// Members whose pad isn't on the board.
    pub missing: Vec<String>,
}

/// Which loops to measure.
#[derive(Clone, Debug, Default)]
pub struct LoopFilter {
    pub names: Option<BTreeSet<String>>,
    /// Loops of these kinds.
    pub kinds: Option<Vec<LoopKind>>,
    /// Loops with a member on this part.
    pub part: Option<String>,
    /// How finely the hull is looked at for `ground_under` (`pi.smps_hot_loop.grid`); 0.1 mm
    /// when unset.
    pub grid: Option<f64>,
}

/// The convex hull of `points`, counter-clockwise.
pub(super) fn hull(mut points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    points.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut lower: Vec<[f64; 2]> = Vec::new();
    for p in &points {
        while let (Some(a), Some(b)) = (lower.len().checked_sub(2).and_then(|i| lower.get(i)).copied(), lower.last().copied()) {
            if cross(a, b, *p) <= 0.0 {
                lower.pop();
            } else {
                break;
            }
        }
        lower.push(*p);
    }
    let mut upper: Vec<[f64; 2]> = Vec::new();
    for p in points.iter().rev() {
        while let (Some(a), Some(b)) = (upper.len().checked_sub(2).and_then(|i| upper.get(i)).copied(), upper.last().copied()) {
            if cross(a, b, *p) <= 0.0 {
                upper.pop();
            } else {
                break;
            }
        }
        upper.push(*p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// The share of the hull of `pads` over a ground plane (a net of `grounds`) on a layer next to
/// `side`, looked at every `grid` mm (at most about 4000 points, 300 along a side, however
/// large), and the layers looked at; `None` with no point inside or no plane next to it.
pub(super) fn ground_under(
    geo: &Geo,
    nets: &BoardNets,
    grounds: &BTreeSet<usize>,
    side: CopperLayer,
    pads: &[&BoardPad],
    grid: f64,
) -> Option<(f64, Vec<CopperLayer>)> {
    let corners: Vec<[f64; 2]> = pads
        .iter()
        .flat_map(|p| {
            let [[x0, y0], [x1, y1]] = p.bbox();
            [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
        })
        .collect();
    let ring = hull(corners);
    let [[x0, y0], [x1, y1]] = super::super::bbox(ring.iter().copied())?;
    let step = grid.max(0.02).max(((x1 - x0) * (y1 - y0) / 4000.0).sqrt()).max((x1 - x0) / 300.0).max((y1 - y0) / 300.0);
    let (nx, ny) = (((x1 - x0) / step).ceil(), ((y1 - y0) / step).ceil());
    if !(nx.is_finite() && ny.is_finite()) || nx > 300.0 || ny > 300.0 {
        return None;
    }
    let mut pts = Vec::new();
    for i in 0..=(nx as usize) {
        for j in 0..=(ny as usize) {
            let q = [x0 + i as f64 * step, y0 + j as f64 * step];
            if super::super::copper::inside(q, &ring) {
                pts.push(q);
            }
        }
    }
    let next: Vec<CopperLayer> = geo.neighbours(side).into_iter().map(|(l, _)| l).collect();
    let mut covered = vec![false; pts.len()];
    let mut any = false;
    for l in &next {
        let Some(plane) = geo.plane(*l) else { continue };
        any = true;
        for (slot, n) in covered.iter_mut().zip(plane.nets_at(nets, &pts)) {
            if n.is_some_and(|n| grounds.contains(&n)) {
                *slot = true;
            }
        }
    }
    if pts.is_empty() || !any {
        return None;
    }
    Some((covered.iter().filter(|c| **c).count() as f64 / pts.len() as f64, next))
}

/// The pad a pin reference (`U1.VDD` by name, `U1.B1` by number) names on the board.
fn find_pad<'p>(pads: &'p BTreeMap<&str, Vec<&'p BoardPad>>, numbers: &BTreeMap<(&str, &str), &str>, pin: &str) -> Option<&'p BoardPad> {
    let (part, name) = pin.split_once('.')?;
    let list = pads.get(part)?;
    if let Some(p) = list.iter().find(|p| p.pad.number == name) {
        return Some(p);
    }
    let number = numbers.get(&(part, name))?;
    list.iter().find(|p| p.pad.number == *number).copied()
}

/// Metrics of the loops `filter` picks; `true` when past the caps.
pub fn loop_metrics(ecad: &Electronics, nets: &BoardNets, filter: &LoopFilter) -> (Vec<LoopMetrics>, bool) {
    let board = &ecad.board;
    let conn = ecad.connectivity();
    let geo = Geo::new(board, nets);
    let mut pads: BTreeMap<&str, Vec<&BoardPad>> = BTreeMap::new();
    for p in &nets.pads {
        pads.entry(p.designator.as_str()).or_default().push(p);
    }
    // Pins by designator and name, to their numbers.
    let numbers: BTreeMap<(&str, &str), &str> =
        conn.nets.iter().flat_map(|n| n.pins.iter()).map(|q| ((q.designator.as_str(), q.name.as_str()), q.pin.as_str())).collect();
    // The loops: the intent's, then one per pin each decoupling capacitor serves.
    let mut loops: Vec<(String, LoopKind, &'static str, Vec<String>)> =
        ecad.intent.loops.iter().map(|l| (l.name.clone(), l.kind, "intent", l.members.clone())).collect();
    for cap in ecad.intent.parts.iter().filter(|p| p.role == PartRole::Decoupling) {
        let Some(own) = pads.get(cap.part.as_str()) else { continue };
        if own.len() != 2 {
            continue;
        }
        for pin in &cap.of {
            let Some(ic) = find_pad(&pads, &numbers, pin) else {
                loops.push((format!("{}→{pin}", cap.part), LoopKind::Decap, "decoupling", vec![pin.clone()]));
                continue;
            };
            let (Some(near), Some(far)) = (own.iter().find(|p| p.net.is_some() && p.net == ic.net), own.iter().find(|p| p.net != ic.net)) else {
                continue;
            };
            // The IC's own pad on the capacitor's other net, nearest the pin.
            let ground = pads
                .get(ic.designator.as_str())
                .into_iter()
                .flatten()
                .filter(|p| p.net.is_some() && p.net == far.net)
                .min_by(|a, b| distance(a.at(), ic.at()).total_cmp(&distance(b.at(), ic.at())));
            let mut members = vec![pin.clone(), near.reference(), far.reference()];
            members.extend(ground.map(|g| g.reference()));
            loops.push((format!("{}→{pin}", cap.part), LoopKind::Decap, "decoupling", members));
        }
    }
    // The filter first, then the cap, so a loop past the cap's count can still be asked for.
    loops.retain(|(name, kind, _, members)| {
        filter.names.as_ref().is_none_or(|n| n.contains(name))
            && filter.kinds.as_ref().is_none_or(|k| k.contains(kind))
            && filter.part.as_ref().is_none_or(|part| members.iter().any(|m| m.split_once('.').is_some_and(|(p, _)| p == part)))
    });
    let truncated = loops.len() > MAX_LOOPS;
    let grounds: BTreeSet<usize> = ecad
        .intent
        .nets
        .iter()
        .filter(|r| r.role == super::super::intent::NetRole::Ground)
        .filter_map(|r| nets.names.iter().position(|n| *n == r.net))
        .collect();
    let grid = filter.grid.unwrap_or(0.1);
    let mut out: Vec<LoopMetrics> = Vec::new();
    for (name, kind, from, members) in loops.into_iter().take(MAX_LOOPS) {
        let mut list = Vec::new();
        let mut missing = Vec::new();
        let mut found: Vec<&BoardPad> = Vec::new();
        for m in &members {
            let pad = find_pad(&pads, &numbers, m);
            if pad.is_none() {
                missing.push(m.clone());
            }
            found.extend(pad);
            list.push(Member {
                pin: m.clone(),
                at: pad.map(BoardPad::at),
                net: pad.and_then(|p| p.net).and_then(|n| nets.name(n)).map(str::to_string),
            });
        }
        let path: Vec<[f64; 2]> = found.iter().map(|p| p.at()).collect();
        let n = path.len();
        let length: f64 = (0..n).filter_map(|i| Some(distance(*path.get(i)?, *path.get((i + 1) % n)?))).sum();
        let twice: f64 = (0..n).filter_map(|i| Some((*path.get(i)?, *path.get((i + 1) % n)?))).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum();
        let area = twice.abs() / 2.0;
        let member_nets: BTreeSet<usize> = found.iter().filter_map(|p| p.net).collect();
        let near = super::super::bbox(path.iter().copied()).map(|b| super::super::grid::grow(b, 0.5));
        let vias = board
            .vias
            .iter()
            .enumerate()
            .filter(|(k, v)| {
                near.is_some_and(|b| super::super::grid::boxes_meet(b, [v.at, v.at]))
                    && nets.via_nets.get(*k).copied().flatten().is_some_and(|x| member_nets.contains(&x))
                    && n >= 2
                    && (super::super::copper::inside(v.at, &path)
                        || (0..n)
                            .filter_map(|i| Some((*path.get(i)?, *path.get((i + 1) % n)?)))
                            .any(|(a, b)| path::segment_distance(v.at, a, b, 0.0) <= 0.5))
            })
            .count();
        let mut layers: BTreeSet<CopperLayer> = BTreeSet::new();
        for p in &found {
            if p.top {
                layers.insert(CopperLayer::Top);
            }
            if p.bottom {
                layers.insert(CopperLayer::Bottom);
            }
        }
        let side = found.first().map(|p| if p.top { CopperLayer::Top } else { CopperLayer::Bottom });
        // At most about 2000 points round the loop, whatever its size.
        let step = (length / 2000.0).max(0.25);
        let probe: Vec<[f64; 2]> =
            (0..n).filter_map(|i| Some((*path.get(i)?, *path.get((i + 1) % n)?))).flat_map(|(a, b)| along(a, b, step)).collect();
        let under = side.and_then(|l| geo.under(l, &probe, None, 0.0));
        let width = found.iter().map(|p| p.pad.size[0].min(p.pad.size[1])).fold(f64::INFINITY, f64::min);
        let width = if width.is_finite() { width.max(0.2) } else { 0.2 };
        let h = under.as_ref().map(|u| u.reference.h).filter(|h| *h > 0.0);
        let est = h.filter(|_| n >= 2).map(|h| MU0 * h * (length / 2.0) / width);
        out.push(LoopMetrics {
            name,
            kind: kind.name(),
            from,
            members: list,
            path,
            length_mm: length,
            area_mm2: area,
            vias,
            layers: layers.into_iter().collect(),
            h,
            ref_coverage: under.map(|u| u.reference.coverage),
            ground_under: side.and_then(|l| ground_under(&geo, nets, &grounds, l, &found, grid)).map(|(share, _)| share),
            est_nh: est,
            rank: 0,
            missing,
        });
    }
    let mut order: Vec<usize> = (0..out.len()).collect();
    order.sort_by(|a, b| {
        let key = |i: &usize| out.get(*i).map_or((0.0, 0.0), |l| (l.est_nh.unwrap_or(-1.0), l.area_mm2));
        key(b).0.total_cmp(&key(a).0).then(key(b).1.total_cmp(&key(a).1))
    });
    for (rank, i) in order.into_iter().enumerate() {
        if let Some(l) = out.get_mut(i) {
            l.rank = rank + 1;
        }
    }
    (out, truncated)
}
