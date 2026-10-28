//! What can be checked about an electronic component on its own: the gaps between its pads, and
//! pins and pads that nothing connects. An agent reads these as data and corrects the component
//! from them, without looking at it.

use super::ElectronicComponent;
use super::footprint::{Core, Pad, PadKind, Side};
use super::graphics::{BoardLayer, shapes_bbox};

/// How serious a problem is: an error keeps the component from being ready to place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// One problem: a stable code, the pins or pads it is about, and what to do.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub code: &'static str,
    pub severity: Severity,
    pub items: Vec<String>,
    pub message: String,
}

/// The two pads closest together on a shared copper layer, and the gap between their copper in
/// millimetres (negative where they overlap: how deep).
#[derive(Clone, Debug, PartialEq)]
pub struct Gap {
    pub pads: [String; 2],
    pub gap: f64,
}

/// The signed distance between two convex cores grown by their radii: the gap between them, or
/// minus how far they overlap.
pub fn gap(a: &Core, b: &Core) -> f64 {
    let (pa, pb) = (&a.points, &b.points);
    let grown = a.radius + b.radius;
    match separation(pa, pb) {
        Some(_) => core_distance(pa, pb) - grown,
        None => -(penetration(pa, pb) + grown),
    }
}

/// Unit axes to test: each edge's normal and direction, and the direction between every pair of
/// points, so degenerate cores (a point, a line) are separated too.
fn axes(pa: &[[f64; 2]], pb: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut out = Vec::new();
    let mut push = |[x, y]: [f64; 2]| {
        let l = x.hypot(y);
        if l > 1e-12 {
            out.push([x / l, y / l]);
        }
    };
    for points in [pa, pb] {
        let n = points.len();
        for i in 0..n {
            let ([x0, y0], [x1, y1]) = (points[i], points[(i + 1) % n]);
            push([x1 - x0, y1 - y0]);
            push([y0 - y1, x1 - x0]);
        }
    }
    for p in pa {
        for q in pb {
            push([q[0] - p[0], q[1] - p[1]]);
        }
    }
    out
}

fn project(points: &[[f64; 2]], [ax, ay]: [f64; 2]) -> (f64, f64) {
    points.iter().map(|p| p[0] * ax + p[1] * ay).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)))
}

/// An axis the cores' projections don't overlap on, if there is one.
fn separation(pa: &[[f64; 2]], pb: &[[f64; 2]]) -> Option<[f64; 2]> {
    axes(pa, pb).into_iter().find(|&axis| {
        let ((a0, a1), (b0, b1)) = (project(pa, axis), project(pb, axis));
        a1 < b0 - 1e-12 || b1 < a0 - 1e-12
    })
}

/// How far overlapping cores would have to move apart: the least overlap over the axes.
fn penetration(pa: &[[f64; 2]], pb: &[[f64; 2]]) -> f64 {
    let least = axes(pa, pb)
        .into_iter()
        .map(|axis| {
            let ((a0, a1), (b0, b1)) = (project(pa, axis), project(pb, axis));
            a1.min(b1) - a0.max(b0)
        })
        .fold(f64::INFINITY, f64::min);
    // No axis at all: both cores are the same point, and only the radii overlap.
    if least.is_finite() { least.clamp(0.0, 1e9) } else { 0.0 }
}

/// The distance between separated convex cores: from each point of one to the other's edges and
/// points.
fn core_distance(pa: &[[f64; 2]], pb: &[[f64; 2]]) -> f64 {
    fn to_shape(p: [f64; 2], shape: &[[f64; 2]]) -> f64 {
        let n = shape.len();
        (0..n)
            .map(|i| {
                let (a, b) = (shape[i], shape[(i + 1) % n]);
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l2 = dx * dx + dy * dy;
                let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
            })
            .fold(f64::INFINITY, f64::min)
    }
    let ab = pa.iter().map(|&p| to_shape(p, pb)).fold(f64::INFINITY, f64::min);
    let ba = pb.iter().map(|&p| to_shape(p, pa)).fold(f64::INFINITY, f64::min);
    ab.min(ba)
}

/// Whether two pads have copper on a common side of the board (holes have none).
fn share_copper(a: &Pad, b: &Pad) -> bool {
    let sides = |p: &Pad| match p.kind {
        PadKind::Hole => (false, false),
        PadKind::ThroughHole => (true, true),
        PadKind::Smd => (p.side == Side::Top, p.side == Side::Bottom),
    };
    let ((at, ab), (bt, bb)) = (sides(a), sides(b));
    (at && bt) || (ab && bb)
}

/// The closest pair of pads on a shared copper layer, and every overlapping pair.
pub fn pad_gaps(pads: &[Pad]) -> (Option<Gap>, Vec<Gap>) {
    let mut order: Vec<(usize, [[f64; 2]; 2], Core)> = pads.iter().enumerate().map(|(i, p)| (i, p.bbox(), p.core())).collect();
    // Sweep along the way the pads spread most, so a single column is pruned as well as a row.
    let spread = |k: usize| {
        let (lo, hi) = order.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (_, b, _)| (lo.min(b[0][k]), hi.max(b[1][k])));
        hi - lo
    };
    let axis = usize::from(spread(1) > spread(0));
    order.sort_by(|a, b| a.1[0][axis].total_cmp(&b.1[0][axis]));
    let mut closest: Option<Gap> = None;
    let mut overlaps = Vec::new();
    for (k, (i, bi, ci)) in order.iter().enumerate() {
        for (j, bj, cj) in &order[k + 1..] {
            // Sorted by leading edge: once a pad starts further on than the best gap so far (and
            // past touching), none after it is closer.
            let ahead = bj[0][axis] - bi[1][axis];
            if ahead > closest.as_ref().map_or(f64::INFINITY, |g| g.gap) && ahead > 0.0 {
                break;
            }
            let (a, b) = (&pads[*i], &pads[*j]);
            if !share_copper(a, b) {
                continue;
            }
            let g = gap(ci, cj);
            let pair = if a.number <= b.number { [a.number.clone(), b.number.clone()] } else { [b.number.clone(), a.number.clone()] };
            if g < 0.0 {
                overlaps.push(Gap { pads: pair.clone(), gap: g });
            }
            if closest.as_ref().is_none_or(|c| g < c.gap) {
                closest = Some(Gap { pads: pair, gap: g });
            }
        }
    }
    overlaps.sort_by(|a, b| a.pads.cmp(&b.pads));
    (closest, overlaps)
}

impl ElectronicComponent {
    /// The closest pair of pads on a shared copper layer.
    pub fn min_pad_gap(&self) -> Option<Gap> {
        pad_gaps(&self.footprint.pads).0
    }

    /// Everything wrong or missing, errors first.
    pub fn problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        for o in pad_gaps(&self.footprint.pads).1 {
            out.push(Problem {
                code: "pads_overlap",
                severity: Severity::Error,
                message: format!("pads {} and {} overlap by {:.4} mm: move them apart or make them smaller", o.pads[0], o.pads[1], -o.gap),
                items: o.pads.to_vec(),
            });
        }
        if !self.footprint.pads.is_empty() {
            for pin in self.unconnected_pins() {
                out.push(Problem {
                    code: "pin_unconnected",
                    severity: Severity::Error,
                    items: vec![pin.to_string()],
                    message: format!("pin {pin} connects to no pad: give it a pad with its number, or map it with ecad.component.map_pins"),
                });
            }
        }
        out.extend(self.courtyard_problems());
        if self.symbol.pins.is_empty() {
            out.push(Problem {
                code: "no_pins",
                severity: Severity::Warning,
                items: Vec::new(),
                message: "the symbol has no pins yet: add them with ecad.pin.add".into(),
            });
        }
        if self.footprint.pads.is_empty() {
            out.push(Problem {
                code: "no_pads",
                severity: Severity::Warning,
                items: Vec::new(),
                message: "the footprint has no pads yet: add them with ecad.pad.add".into(),
            });
        } else if !self.symbol.pins.is_empty() {
            for pad in self.unconnected_pads() {
                out.push(Problem {
                    code: "pad_unconnected",
                    severity: Severity::Warning,
                    items: vec![pad.to_string()],
                    message: format!("no pin connects to pad {pad} (fine for a mounting pad)"),
                });
            }
        }
        out.sort_by_key(|p| p.severity != Severity::Error);
        out
    }

    /// Ready to place: a symbol with pins, a footprint with pads, and no errors.
    pub fn ready(&self) -> bool {
        self.ready_with(&self.problems())
    }

    /// [`Self::ready`] from problems already found.
    pub fn ready_with(&self, problems: &[Problem]) -> bool {
        !self.symbol.pins.is_empty() && !self.footprint.pads.is_empty() && problems.iter().all(|p| p.severity != Severity::Error)
    }

    /// The box the courtyard on `side` outlines (its centre line), if there is one.
    pub fn courtyard(&self, side: Side) -> Option<[[f64; 2]; 2]> {
        shapes_bbox(self.footprint.layer(BoardLayer::Courtyard, side))
    }

    /// A footprint with pads needs a courtyard on top, and every pad inside the courtyard of its
    /// side (a through-hole pad on top).
    fn courtyard_problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        if self.footprint.pads.is_empty() {
            return out;
        }
        if self.courtyard(Side::Top).is_none() {
            out.push(Problem {
                code: "courtyard_missing",
                severity: Severity::Warning,
                items: Vec::new(),
                message: "the footprint has no courtyard: draw one, or make one with ecad.footprint.courtyard".into(),
            });
        }
        let courtyards = [self.courtyard(Side::Top), self.courtyard(Side::Bottom)];
        for pad in &self.footprint.pads {
            let side = if pad.kind == PadKind::Smd { pad.side } else { Side::Top };
            let Some([[x0, y0], [x1, y1]]) = courtyards[usize::from(side == Side::Bottom)] else { continue };
            let [[a0, b0], [a1, b1]] = pad.bbox();
            if a0 < x0 - 1e-9 || b0 < y0 - 1e-9 || a1 > x1 + 1e-9 || b1 > y1 + 1e-9 {
                out.push(Problem {
                    code: "pad_outside_courtyard",
                    severity: Severity::Error,
                    items: vec![pad.number.clone()],
                    message: format!(
                        "pad {} reaches outside the courtyard: make the courtyard bigger, or run ecad.footprint.courtyard again",
                        pad.number
                    ),
                });
            }
        }
        out
    }
}
