//! Board copper and drawings as shapes for the design rule check and the pour fill. Each piece is
//! a convex core grown by a radius ([`Core`]): a pad as the library gives it, a straight piece of
//! a track (arcs cut into pieces within [`TOLERANCE`]), a via's ring, a drawing's stroke. Around a
//! core, [`hull_outline`] gives a polygon that holds it (never cutting inside it), for the fill's
//! booleans.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use super::airwires::BoardPad;
use super::board::Track;
use super::footprint::{Core, Pad, PadKind, PadShape};
use super::graphics::{Graphic, GraphicType};
use super::path;

/// How far, in millimetres, a polygon round a core may stray outside it, and an arc's straight
/// pieces from the arc.
pub const TOLERANCE: f64 = 0.002;
/// The most pieces one circle or arc is cut into.
const MAX_ARC_PIECES: usize = 128;

fn distance([ax, ay]: [f64; 2], [bx, by]: [f64; 2]) -> f64 {
    (ax - bx).hypot(ay - by)
}

/// A pad's copper on the board.
pub fn pad_core(p: &BoardPad) -> Core {
    let c = p.pad.core();
    Core { points: c.points.iter().map(|q| p.placement.apply(*q)).collect(), radius: c.radius }
}

/// A pad's solder paste opening on the board: the whole pad unless the pad gives its own.
pub fn paste_core(p: &BoardPad) -> Core {
    let c = Pad { size: p.pad.paste.unwrap_or(p.pad.size), paste: None, ..p.pad.clone() }.core();
    Core { points: c.points.iter().map(|q| p.placement.apply(*q)).collect(), radius: c.radius }
}

/// A pad's drill on the board (an oval for a slot); `None` without one.
pub fn drill_core(p: &BoardPad) -> Option<Core> {
    let drill = p.pad.drill?;
    let hole = Pad { shape: PadShape::Oval, size: drill, corner: 0.0, kind: PadKind::Smd, drill: None, ..p.pad.clone() };
    let c = hole.core();
    Some(Core { points: c.points.iter().map(|q| p.placement.apply(*q)).collect(), radius: c.radius })
}

/// A circle as a core.
pub fn disc(at: [f64; 2], radius: f64) -> Core {
    Core { points: vec![at], radius }
}

/// A straight piece as a core, `radius` round it.
pub fn stroke(a: [f64; 2], b: [f64; 2], radius: f64) -> Core {
    Core { points: vec![a, b], radius }
}

/// The straight pieces of a line with sweeps, arcs cut within [`TOLERANCE`].
pub fn pieces(points: &[[f64; 2]], curves: &[f64], closed: bool) -> Vec<([f64; 2], [f64; 2])> {
    let line = path::tessellate(points, curves, closed, TOLERANCE);
    let mut out: Vec<([f64; 2], [f64; 2])> = line.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?))).collect();
    if closed
        && line.len() > 2
        && let (Some(a), Some(b)) = (line.last(), line.first())
    {
        out.push((*a, *b));
    }
    out
}

/// A track's pieces as cores, half its width round each.
pub fn track_cores(t: &Track) -> impl Iterator<Item = Core> + '_ {
    let r = t.width / 2.0;
    pieces(&t.points, &t.curves, false).into_iter().map(move |(a, b)| stroke(a, b, r))
}

/// A drawing's strokes as cores (each corner through `place`): a line's segments, a rectangle's
/// or a polygon's edges, a circle or an arc cut into pieces; a text's box (see
/// [`Graphic::text_box`]) as one.
pub fn graphic_cores(g: &Graphic, place: &dyn Fn([f64; 2]) -> [f64; 2]) -> Vec<Core> {
    let r = g.width / 2.0;
    let line = |points: Vec<[f64; 2]>, closed: bool| -> Vec<Core> {
        let pts: Vec<[f64; 2]> = points.into_iter().map(place).collect();
        pieces(&pts, &[], closed).into_iter().map(|(a, b)| stroke(a, b, r)).collect()
    };
    match g.kind {
        GraphicType::Line => line(g.points.clone(), false),
        GraphicType::Polygon => line(g.points.clone(), true),
        GraphicType::Rect => match (g.from, g.to) {
            (Some([x0, y0]), Some([x1, y1])) => line(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]], true),
            _ => Vec::new(),
        },
        GraphicType::Circle => match (g.center, g.radius) {
            (Some(c), Some(radius)) => line(arc_points(c, radius, 0.0, TAU), true),
            _ => Vec::new(),
        },
        GraphicType::Arc => match (g.center, g.radius, g.start, g.sweep) {
            (Some(c), Some(radius), Some(start), Some(sweep)) => line(arc_points(c, radius, start.to_radians(), sweep.to_radians()), false),
            _ => Vec::new(),
        },
        GraphicType::Text => {
            let corners: Vec<[f64; 2]> = g.text_box().into_iter().map(place).collect();
            if corners.is_empty() { Vec::new() } else { vec![Core { points: corners, radius: r }] }
        }
    }
}

/// Points along an arc, within [`TOLERANCE`] of it (a full turn without its last point).
fn arc_points(c: [f64; 2], r: f64, start: f64, sweep: f64) -> Vec<[f64; 2]> {
    if !(r.is_finite() && r > 0.0 && start.is_finite() && sweep.is_finite()) {
        return Vec::new();
    }
    let n = ((sweep.abs() / step(r)).ceil() as usize).clamp(2, MAX_ARC_PIECES);
    let full = sweep.abs() >= TAU - 1e-9;
    let last = if full { n - 1 } else { n };
    (0..=last).map(|k| start + sweep * k as f64 / n as f64).map(|a| [c[0] + r * a.cos(), c[1] + r * a.sin()]).collect()
}

/// The turn, in radians, between corners of a polygon that keeps within [`TOLERANCE`] of a
/// circle of radius `r`: 3° to 45°.
fn step(r: f64) -> f64 {
    let s = if r > TOLERANCE { 2.0 * (1.0 - TOLERANCE / r).acos() } else { FRAC_PI_2 };
    s.clamp(PI / 60.0, PI / 4.0)
}

/// The convex hull of `points`, counter-clockwise, without repeated points.
fn hull(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut pts: Vec<[f64; 2]> = points.iter().copied().filter(|[x, y]| x.is_finite() && y.is_finite()).collect();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup_by(|a, b| distance(*a, *b) < 1e-9);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(2 * pts.len());
    for pass in 0..2 {
        let start = out.len();
        let ordered: Box<dyn Iterator<Item = &[f64; 2]>> = if pass == 0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
        for p in ordered {
            while out.len() >= start + 2 {
                let (Some(a), Some(b)) = (out.get(out.len() - 2), out.last()) else { break };
                if cross(*a, *b, *p) <= 0.0 {
                    out.pop();
                } else {
                    break;
                }
            }
            out.push(*p);
        }
        out.pop();
    }
    out
}

/// A polygon, counter-clockwise, that holds `core` grown by `grow` more (its radius plus
/// `grow` round its convex core), straying outside by at most [`TOLERANCE`]: arcs round each
/// corner of the core, straight edges between. Empty for a core with no area.
pub fn hull_outline(core: &Core, grow: f64) -> Vec<[f64; 2]> {
    let h = hull(&core.points);
    let r = core.radius + grow;
    if !(r.is_finite() && r > 0.0) {
        return if h.len() >= 3 { h } else { Vec::new() };
    }
    let Some(first) = h.first().copied() else { return Vec::new() };
    if h.len() == 1 {
        let n = ((TAU / step(r)).ceil() as usize).clamp(8, MAX_ARC_PIECES);
        let out_r = r / (PI / n as f64).cos();
        return (0..n).map(|k| TAU * k as f64 / n as f64).map(|a| [first[0] + out_r * a.cos(), first[1] + out_r * a.sin()]).collect();
    }
    let n = h.len();
    let normal = |a: [f64; 2], b: [f64; 2]| {
        // Outward of a counter-clockwise polygon: the edge's right.
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        dy.atan2(dx) - FRAC_PI_2
    };
    let mut out = Vec::with_capacity(n * 8);
    for i in 0..n {
        let (Some(prev), Some(v), Some(next)) = (h.get((i + n - 1) % n), h.get(i), h.get((i + 1) % n)) else { continue };
        let a0 = normal(*prev, *v);
        let sweep = (normal(*v, *next) - a0).rem_euclid(TAU);
        let k = ((sweep / step(r)).ceil() as usize).clamp(1, MAX_ARC_PIECES);
        let out_r = r / (sweep / (2.0 * k as f64)).cos();
        for j in 0..=k {
            let a = a0 + sweep * j as f64 / k as f64;
            out.push([v[0] + out_r * a.cos(), v[1] + out_r * a.sin()]);
        }
    }
    out
}

/// How far the core reaches from `from` along the unit direction `d` (its radius included).
pub fn reach(core: &Core, from: [f64; 2], d: [f64; 2]) -> f64 {
    core.points.iter().map(|p| (p[0] - from[0]) * d[0] + (p[1] - from[1]) * d[1]).fold(0.0_f64, f64::max) + core.radius
}

/// The box round a core.
pub fn core_bbox(core: &Core) -> [[f64; 2]; 2] {
    let r = core.radius;
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for [x, y] in &core.points {
        (x0, y0, x1, y1) = (x0.min(*x), y0.min(*y), x1.max(*x), y1.max(*y));
    }
    [[x0 - r, y0 - r], [x1 + r, y1 + r]]
}

/// Whether `p` lies inside the closed polygon `ring` (even-odd).
pub fn inside(p: [f64; 2], ring: &[[f64; 2]]) -> bool {
    let n = ring.len();
    let mut yes = false;
    for i in 0..n {
        let (Some(a), Some(b)) = (ring.get(i), ring.get((i + 1) % n)) else { continue };
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                yes = !yes;
            }
        }
    }
    yes
}
