//! Lines whose segments may be circular arcs: board tracks and the board outline. A segment from
//! `a` to `b` with a sweep of θ degrees is the arc that leaves `a` and turns θ on its way to `b`
//! (counter-clockwise when positive, 0 straight, less than a full turn either way); its radius
//! follows from the chord. A line's sweeps are a list with one per segment, or empty when all are
//! straight; a closed line's last segment joins its last point to its first.

use std::f64::consts::{FRAC_PI_2, TAU};

use super::place::{lattice, snap_point};

/// A sweep this small, in degrees, is a straight segment.
pub const MIN_SWEEP: f64 = 1e-6;
/// The most straight pieces [`tessellate`] cuts one arc into.
const MAX_PIECES: usize = 128;

/// The sweep of segment `i` in `curves`: 0 when the list is empty.
pub fn sweep(curves: &[f64], i: usize) -> f64 {
    curves.get(i).copied().unwrap_or(0.0)
}

/// Whether a sweep (degrees) bends its segment.
pub fn is_arc(sweep: f64) -> bool {
    sweep.is_finite() && sweep.abs() > MIN_SWEEP
}

/// Sweeps fit a line of `segments` segments: none, or one each, each finite and less than a full
/// turn.
pub fn check_curves(curves: &[f64], segments: usize) -> Result<(), String> {
    if !curves.is_empty() && curves.len() != segments {
        return Err(format!("`curves` has one sweep per segment ({segments}), or none"));
    }
    if curves.iter().any(|c| !(c.is_finite() && c.abs() < 360.0)) {
        return Err("each of `curves` must be more than -360 and less than 360 degrees".into());
    }
    Ok(())
}

fn distance([ax, ay]: [f64; 2], [bx, by]: [f64; 2]) -> f64 {
    (ax - bx).hypot(ay - by)
}

/// An arc segment as its circle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arc {
    pub center: [f64; 2],
    pub radius: f64,
    /// Radians from +x to the start.
    pub start: f64,
    /// Radians, counter-clockwise when positive.
    pub sweep: f64,
}

impl Arc {
    /// The arc from `a` to `b` turning `sweep` degrees; `None` when the segment is straight or its
    /// ends meet.
    pub fn new(a: [f64; 2], b: [f64; 2], sweep: f64) -> Option<Arc> {
        if !is_arc(sweep) || sweep.abs() >= 360.0 {
            return None;
        }
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let chord = dx.hypot(dy);
        if !(chord.is_finite() && chord > 0.0) {
            return None;
        }
        let theta = sweep.to_radians();
        // The centre is off the chord's middle along its left normal (right for a turn past half).
        let off = chord / 2.0 / (theta / 2.0).tan();
        let center = [(a[0] + b[0]) / 2.0 - dy / chord * off, (a[1] + b[1]) / 2.0 + dx / chord * off];
        let radius = chord / 2.0 / (theta / 2.0).sin().abs();
        let start = (a[1] - center[1]).atan2(a[0] - center[0]);
        (center[0].is_finite() && center[1].is_finite() && radius.is_finite()).then_some(Arc { center, radius, start, sweep: theta })
    }

    /// The point a share `t` (0 to 1) along it.
    pub fn point(&self, t: f64) -> [f64; 2] {
        let a = self.start + self.sweep * t;
        [self.center[0] + self.radius * a.cos(), self.center[1] + self.radius * a.sin()]
    }

    /// The share along it (0 to 1) of the direction from its centre to `p`; `None` when that
    /// direction is outside it.
    pub fn share(&self, p: [f64; 2]) -> Option<f64> {
        let phi = (p[1] - self.center[1]).atan2(p[0] - self.center[0]);
        let turned = if self.sweep >= 0.0 { (phi - self.start).rem_euclid(TAU) } else { (self.start - phi).rem_euclid(TAU) };
        let t = turned / self.sweep.abs();
        (t <= 1.0).then_some(t)
    }

    pub fn length(&self) -> f64 {
        self.radius * self.sweep.abs()
    }

    /// The smallest box round it: its ends, and the quarter points it passes.
    pub fn bbox(&self) -> [[f64; 2]; 2] {
        let mut points = vec![self.point(0.0), self.point(1.0)];
        for k in 0..4 {
            let q = f64::from(k) * FRAC_PI_2;
            let p = [self.center[0] + self.radius * q.cos(), self.center[1] + self.radius * q.sin()];
            if self.share(p).is_some() {
                points.push(p);
            }
        }
        super::bbox(points).unwrap_or([self.center; 2])
    }
}

/// The distance from `p` to the segment `a`–`b` of sweep `sweep`.
pub fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2], sweep: f64) -> f64 {
    if let Some(arc) = Arc::new(a, b, sweep) {
        return match arc.share(p) {
            Some(_) => (distance(p, arc.center) - arc.radius).abs(),
            None => distance(p, a).min(distance(p, b)),
        };
    }
    let (q, _) = project(p, a, b, 0.0);
    distance(p, q)
}

/// The point of the segment nearest `p`, and how far along it that is (0 at `a`, 1 at `b`; by
/// angle on an arc).
pub fn project(p: [f64; 2], a: [f64; 2], b: [f64; 2], sweep: f64) -> ([f64; 2], f64) {
    if let Some(arc) = Arc::new(a, b, sweep) {
        return match arc.share(p) {
            Some(t) => (arc.point(t), t),
            None if distance(p, a) <= distance(p, b) => (a, 0.0),
            None => (b, 1.0),
        };
    }
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    ([a[0] + t * dx, a[1] + t * dy], t)
}

/// The segment's length.
pub fn segment_length(a: [f64; 2], b: [f64; 2], sweep: f64) -> f64 {
    Arc::new(a, b, sweep).map_or_else(|| distance(a, b), |arc| arc.length())
}

/// The smallest box round the segment.
pub fn segment_bbox(a: [f64; 2], b: [f64; 2], sweep: f64) -> [[f64; 2]; 2] {
    Arc::new(a, b, sweep).map_or_else(|| [[a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]], |arc| arc.bbox())
}

/// Each segment's ends and sweep, the closing one last when `closed`.
pub fn segments<'a>(points: &'a [[f64; 2]], curves: &'a [f64], closed: bool) -> impl Iterator<Item = ([f64; 2], [f64; 2], f64)> + 'a {
    let n = points.len();
    let count = if closed && n >= 2 { n } else { n.saturating_sub(1) };
    (0..count).filter_map(move |i| Some((*points.get(i)?, *points.get((i + 1) % n)?, sweep(curves, i))))
}

/// The line's whole length.
pub fn length(points: &[[f64; 2]], curves: &[f64], closed: bool) -> f64 {
    segments(points, curves, closed).map(|(a, b, s)| segment_length(a, b, s)).sum()
}

/// The smallest box round the line, arcs included; `None` without points.
pub fn bbox(points: &[[f64; 2]], curves: &[f64], closed: bool) -> Option<[[f64; 2]; 2]> {
    let ends = points.iter().copied();
    super::bbox(ends.chain(segments(points, curves, closed).flat_map(|(a, b, s)| segment_bbox(a, b, s))))
}

/// The line as straight pieces for drawing: each arc cut so no piece strays more than
/// `tolerance` millimetres from it (2 to 128 pieces an arc). A closed line's first point is not
/// repeated at its end.
pub fn tessellate(points: &[[f64; 2]], curves: &[f64], closed: bool, tolerance: f64) -> Vec<[f64; 2]> {
    let mut out = Vec::with_capacity(points.len());
    out.extend(points.first().copied());
    let n = points.len();
    for (i, (a, b, s)) in segments(points, curves, closed).enumerate() {
        if let Some(arc) = Arc::new(a, b, s) {
            let pieces = pieces(&arc, tolerance);
            out.extend((1..pieces).map(|k| arc.point(k as f64 / pieces as f64)));
        }
        if !(closed && i + 1 == n) {
            out.push(b);
        }
    }
    out
}

/// How many straight pieces keep within `tolerance` of `arc`.
fn pieces(arc: &Arc, tolerance: f64) -> usize {
    let step =
        if tolerance.is_finite() && tolerance > 0.0 && tolerance < arc.radius { 2.0 * (1.0 - tolerance / arc.radius).acos() } else { FRAC_PI_2 };
    let n = (arc.sweep.abs() / step.max(1e-3)).ceil();
    if n.is_finite() { (n as usize).clamp(2, MAX_PIECES) } else { MAX_PIECES }
}

/// Twice the signed area a closed line encloses (counter-clockwise positive), arcs included.
pub fn twice_area(points: &[[f64; 2]], curves: &[f64]) -> f64 {
    let ring = tessellate(points, curves, true, 1e-3);
    let n = ring.len();
    (0..n)
        .filter_map(|i| {
            let ([x0, y0], [x1, y1]) = (*ring.get(i)?, *ring.get((i + 1) % n)?);
            Some(x0 * y1 - x1 * y0)
        })
        .sum()
}

/// Drop segments whose ends meet (on the lattice of saved points), with their sweeps; a closed
/// line also loses a last point equal to its first. Sweeps that are all straight become none.
pub fn tidy(points: &mut Vec<[f64; 2]>, curves: &mut Vec<f64>, closed: bool) {
    let has_curves = !curves.is_empty();
    let mut out_points: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    let mut out_curves = Vec::with_capacity(curves.len());
    for (i, p) in points.iter().enumerate() {
        if out_points.last().is_some_and(|last| lattice(*last) == lattice(*p)) {
            // The zero segment into `p` goes; the one out of it is kept on the point before.
            if has_curves {
                out_curves.pop();
                out_curves.push(sweep(curves, i));
            }
            continue;
        }
        out_points.push(*p);
        if has_curves {
            out_curves.push(sweep(curves, i));
        }
    }
    if closed {
        while out_points.len() > 1 && out_points.first().map(|p| lattice(*p)) == out_points.last().map(|p| lattice(*p)) {
            // The zero closing segment goes; the one into the last point now closes the line.
            out_points.pop();
            out_curves.pop();
        }
    } else if has_curves {
        // An open line has no segment out of its last point.
        out_curves.truncate(out_points.len().saturating_sub(1));
    }
    if out_curves.iter().all(|c| !is_arc(*c)) {
        out_curves.clear();
    }
    *points = out_points.into_iter().map(snap_point).collect();
    *curves = out_curves;
}

/// Whether the straight run `a` → `b` → `c` turns straight back at `b` (lattice points; far-off
/// ones saturate rather than overflow).
fn turns_back(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> bool {
    let (abx, aby) = (i128::from(b[0]) - i128::from(a[0]), i128::from(b[1]) - i128::from(a[1]));
    let (bcx, bcy) = (i128::from(c[0]) - i128::from(b[0]), i128::from(c[1]) - i128::from(b[1]));
    let cross = abx.saturating_mul(bcy).saturating_sub(aby.saturating_mul(bcx));
    let along = abx.saturating_mul(bcx).saturating_add(aby.saturating_mul(bcy));
    cross == 0 && along < 0
}

/// The first point of an open line where a straight segment runs straight back along the straight
/// one before it (a track doubled on itself); `None` when there is none.
pub fn doubles_back(points: &[[f64; 2]], curves: &[f64]) -> Option<usize> {
    (1..points.len().saturating_sub(1)).find(|&i| {
        let (Some(a), Some(b), Some(c)) = (points.get(i - 1), points.get(i), points.get(i + 1)) else { return false };
        !is_arc(sweep(curves, i - 1)) && !is_arc(sweep(curves, i)) && turns_back(lattice(*a), lattice(*b), lattice(*c))
    })
}

/// An open line with the runs that double back taken out: where a straight segment runs straight
/// back along the one before, the point it turns at goes (so the line runs on from the point
/// before), and a segment of no length left by that goes too. The rest is kept as it is.
pub fn unfold(points: &[[f64; 2]], curves: &[f64]) -> (Vec<[f64; 2]>, Vec<f64>) {
    // Each point kept with the sweep of the segment into it (0 for the first).
    let mut kept: Vec<([f64; 2], f64)> = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        let s = if i == 0 { 0.0 } else { sweep(curves, i - 1) };
        loop {
            let n = kept.len();
            if kept.last().is_some_and(|(q, _)| lattice(*q) == lattice(*p)) {
                break;
            }
            if n >= 2
                && let (Some((a, _)), Some((b, into))) = (kept.get(n - 2), kept.get(n - 1))
                && !is_arc(*into)
                && !is_arc(s)
                && turns_back(lattice(*a), lattice(*b), lattice(*p))
            {
                kept.pop();
                continue;
            }
            kept.push((*p, s));
            break;
        }
    }
    let points: Vec<[f64; 2]> = kept.iter().map(|(p, _)| *p).collect();
    let mut curves: Vec<f64> = kept.iter().skip(1).map(|(_, s)| *s).collect();
    if curves.iter().all(|c| !is_arc(*c)) {
        curves.clear();
    }
    (points, curves)
}
