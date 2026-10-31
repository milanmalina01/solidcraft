//! Regions of the plane: what a set of polygons with holes covers, in millimetres, with the
//! booleans (union, difference, intersection) and offsets (grow, shrink) that the board's copper
//! pours are built from.
//!
//! A region keeps its corners on a lattice of [`UNIT`] millimetres (whole numbers), so the
//! booleans are exact and never fail on touching or overlapping edges. Its shapes are disjoint:
//! each an outer contour (counter-clockwise) with its holes (clockwise). Coordinates farther than
//! [`REACH`] from the origin are pulled in to it, and non-finite ones are dropped with their
//! polygon. The polygon work itself is the `i_overlay` crate's; no other module names it.

use std::collections::HashMap;

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{Overlay, ShapeType};
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::core::point_location::IntPointContainment;
use i_overlay::i_float::int::angle::Angle;
use i_overlay::i_float::int::point::IntPoint;
use i_overlay::i_shape::int::shape::{IntContour, IntShape, IntShapes};
use i_overlay::mesh::int::arc::ArcOptions;
use i_overlay::mesh::int::outline::offset::IntOutlineOffset;
use i_overlay::mesh::int::style::{IntLineJoin, IntOutlineStyle};

/// One lattice step, in millimetres (10 nm).
pub const UNIT: f64 = 1e-5;
/// How far from the origin a corner may be, in millimetres; farther ones are pulled in.
pub const REACH: f64 = 3000.0;
/// The largest offset, in millimetres.
pub const MAX_OFFSET: f64 = 100.0;
/// The cells each way of the grid [`Region::locate`] sorts its points into.
const LOCATE_CELLS: usize = 256;

/// A shape of a region in millimetres: its outer contour (counter-clockwise) and its holes
/// (clockwise), none of them closed by repeating the first corner.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub outer: Vec<[f64; 2]>,
    pub holes: Vec<Vec<[f64; 2]>>,
}

impl Shape {
    /// Its area in square millimetres, holes taken out.
    pub fn area(&self) -> f64 {
        (signed_area(&self.outer) + self.holes.iter().map(|h| signed_area(h)).sum::<f64>()).abs()
    }

    /// How many corners its contours have together.
    pub fn corners(&self) -> usize {
        self.outer.len() + self.holes.iter().map(Vec::len).sum::<usize>()
    }

    /// The shape with corners closer than `min_edge` millimetres to the corner kept before them
    /// dropped (the booleans leave many on rounded corners), for drawing and measuring: it strays
    /// from the shape by at most about `min_edge`. A contour left with fewer than three corners
    /// keeps them all.
    pub fn simplified(&self, min_edge: f64) -> Shape {
        let thin = |c: &Vec<[f64; 2]>| -> Vec<[f64; 2]> {
            let mut out: Vec<[f64; 2]> = Vec::with_capacity(c.len());
            for p in c {
                if out.last().is_none_or(|q| (p[0] - q[0]).hypot(p[1] - q[1]) >= min_edge) {
                    out.push(*p);
                }
            }
            while out.len() > 3 && out.first().zip(out.last()).is_some_and(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]) < min_edge) {
                out.pop();
            }
            if out.len() < 3 { c.clone() } else { out }
        };
        Shape { outer: thin(&self.outer), holes: self.holes.iter().map(thin).collect() }
    }
}

/// The signed area of a closed polygon (counter-clockwise positive), in square millimetres.
pub fn signed_area(points: &[[f64; 2]]) -> f64 {
    let n = points.len();
    let mut twice = 0.0;
    for i in 0..n {
        if let (Some([x0, y0]), Some([x1, y1])) = (points.get(i), points.get((i + 1) % n)) {
            twice += x0 * y1 - x1 * y0;
        }
    }
    twice / 2.0
}

/// What a set of polygons with holes covers (see the module notes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Region {
    shapes: IntShapes<i32>,
}

fn to_unit(v: f64) -> i32 {
    // In range by the clamp; the cast cannot overflow.
    ((v.clamp(-REACH, REACH) / UNIT).round()) as i32
}

fn from_unit(v: i32) -> f64 {
    f64::from(v) * UNIT + 0.0
}

fn int_point([x, y]: [f64; 2]) -> IntPoint<i32> {
    IntPoint::new(to_unit(x), to_unit(y))
}

/// A polygon on the lattice, counter-clockwise; `None` when a corner isn't finite or fewer than
/// three corners are left.
fn contour(points: &[[f64; 2]]) -> Option<IntContour<i32>> {
    if points.iter().any(|[x, y]| !(x.is_finite() && y.is_finite())) {
        return None;
    }
    let mut out: IntContour<i32> = Vec::with_capacity(points.len());
    for p in points {
        let q = int_point(*p);
        if out.last() != Some(&q) {
            out.push(q);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    if out.len() < 3 {
        return None;
    }
    if twice_area(&out) < 0 {
        out.reverse();
    }
    Some(out)
}

fn twice_area(c: &[IntPoint<i32>]) -> i128 {
    let n = c.len();
    let mut s: i128 = 0;
    for i in 0..n {
        if let (Some(a), Some(b)) = (c.get(i), c.get((i + 1) % n)) {
            s += i128::from(a.x) * i128::from(b.y) - i128::from(b.x) * i128::from(a.y);
        }
    }
    s
}

fn mm_contour(c: &[IntPoint<i32>]) -> Vec<[f64; 2]> {
    c.iter().map(|p| [from_unit(p.x), from_unit(p.y)]).collect()
}

impl Region {
    /// The empty region.
    pub fn new() -> Self {
        Region::default()
    }

    /// What the polygons cover together: each one filled whichever way it runs (one that crosses
    /// itself, where it winds), overlaps joined.
    pub fn polygons<'a>(polygons: impl IntoIterator<Item = &'a [[f64; 2]]>) -> Self {
        let contours: Vec<IntContour<i32>> = polygons.into_iter().filter_map(contour).collect();
        if contours.is_empty() {
            return Region::new();
        }
        let capacity = contours.iter().map(Vec::len).sum();
        let mut overlay = Overlay::new(capacity);
        overlay.add_source(&contours, ShapeType::Subject);
        Region { shapes: overlay.overlay(OverlayRule::Subject, FillRule::NonZero) }
    }

    /// One polygon, filled (see [`Region::polygons`]).
    pub fn polygon(points: &[[f64; 2]]) -> Self {
        Region::polygons([points])
    }

    /// The axis-aligned rectangle between two corners.
    pub fn rect([x0, y0]: [f64; 2], [x1, y1]: [f64; 2]) -> Self {
        Region::polygon(&[[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// How many disjoint shapes it has.
    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    /// How many corners all its contours have together: the size of the work done with it.
    pub fn corners(&self) -> usize {
        self.shapes.iter().flatten().map(Vec::len).sum()
    }

    fn boolean(&self, other: &Region, rule: OverlayRule) -> Region {
        if self.is_empty() && other.is_empty() {
            return Region::new();
        }
        let mut overlay = Overlay::from_subj_and_clip(&self.shapes, &other.shapes);
        Region { shapes: overlay.overlay(rule, FillRule::NonZero) }
    }

    /// What either covers.
    pub fn union(&self, other: &Region) -> Region {
        match (self.is_empty(), other.is_empty()) {
            (true, _) => other.clone(),
            (_, true) => self.clone(),
            _ => self.boolean(other, OverlayRule::Union),
        }
    }

    /// What this covers and `other` doesn't.
    pub fn difference(&self, other: &Region) -> Region {
        if self.is_empty() || other.is_empty() {
            return self.clone();
        }
        self.boolean(other, OverlayRule::Difference)
    }

    /// What both cover.
    pub fn intersection(&self, other: &Region) -> Region {
        if self.is_empty() || other.is_empty() {
            return Region::new();
        }
        self.boolean(other, OverlayRule::Intersect)
    }

    /// The region grown by `distance` millimetres (shrunk when it is negative), its corners
    /// rounded with arcs of at most `step` degrees a piece; nothing changes for a distance under
    /// one lattice step or not finite. At most [`MAX_OFFSET`] either way.
    pub fn offset(&self, distance: f64, step: f64) -> Region {
        if self.is_empty() || !distance.is_finite() {
            return self.clone();
        }
        let d = to_unit(distance.clamp(-MAX_OFFSET, MAX_OFFSET));
        if d == 0 {
            return self.clone();
        }
        let turn = if step.is_finite() { (step / 360.0).clamp(1.0 / 1024.0, 0.125) } else { 0.125 };
        // A whole turn is 2^32 angle units; in range by the clamp.
        let max_step = Angle::from_bits((turn * 4_294_967_296.0) as u32);
        let style = IntOutlineStyle::new(d).line_join(IntLineJoin::Round(ArcOptions { max_step, rotation_precision: 5 }));
        match self.shapes.outline(&style) {
            Ok(shapes) => Region { shapes },
            Err(_) => self.clone(),
        }
    }

    /// Its shapes, in millimetres.
    pub fn shapes(&self) -> Vec<Shape> {
        self.shapes.iter().map(shape_mm).collect()
    }

    /// Only the shapes `keep` says yes to (by index, as [`Region::shapes`] lists them).
    pub fn select(&self, keep: impl Fn(usize) -> bool) -> Region {
        Region { shapes: self.shapes.iter().enumerate().filter(|(i, _)| keep(*i)).map(|(_, s)| s.clone()).collect() }
    }

    /// Each of its shapes as a region of its own.
    pub fn split(&self) -> Vec<Region> {
        self.shapes.iter().map(|s| Region { shapes: vec![s.clone()] }).collect()
    }

    /// Its area in square millimetres.
    pub fn area(&self) -> f64 {
        self.shapes.iter().map(|s| shape_mm(s).area()).sum()
    }

    /// The box round it; `None` while empty.
    pub fn bbox(&self) -> Option<[[f64; 2]; 2]> {
        let mut out: Option<[[i32; 2]; 2]> = None;
        for p in self.shapes.iter().flat_map(|s| s.first()).flatten() {
            out = Some(match out {
                None => [[p.x, p.y], [p.x, p.y]],
                Some([[x0, y0], [x1, y1]]) => [[x0.min(p.x), y0.min(p.y)], [x1.max(p.x), y1.max(p.y)]],
            });
        }
        out.map(|[[x0, y0], [x1, y1]]| [[from_unit(x0), from_unit(y0)], [from_unit(x1), from_unit(y1)]])
    }

    /// Which of its shapes (by index, as [`Region::shapes`] lists them) holds each point; `None`
    /// for a point outside every shape. A point on an edge may go either way. The points are
    /// sorted into cells first, so each shape is asked only about the points in its box.
    pub fn locate(&self, points: &[[f64; 2]]) -> Vec<Option<usize>> {
        let mut out = vec![None; points.len()];
        let Some([[bx0, by0], [bx1, by1]]) = self.bbox() else { return out };
        if points.is_empty() {
            return out;
        }
        let queries: Vec<IntPoint<i32>> = points.iter().map(|p| int_point(*p)).collect();
        // A grid of up to LOCATE_CELLS × LOCATE_CELLS cells over the region's box.
        let side = ((bx1 - bx0).max(by1 - by0) / LOCATE_CELLS as f64).max(UNIT);
        let cell = |x: f64, y: f64| (((x - bx0) / side).floor() as i64, ((y - by0) / side).floor() as i64);
        let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, p) in points.iter().enumerate() {
            if p[0] >= bx0 && p[0] <= bx1 && p[1] >= by0 && p[1] <= by1 {
                cells.entry(cell(p[0], p[1])).or_default().push(i);
            }
        }
        for (k, shape) in self.shapes.iter().enumerate() {
            let Some(outer) = shape.first() else { continue };
            let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
            for p in outer {
                (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
            }
            let (c0, c1) = (cell(from_unit(x0), from_unit(y0)), cell(from_unit(x1), from_unit(y1)));
            let span = (c1.0 - c0.0 + 1).saturating_mul(c1.1 - c0.1 + 1);
            let mut near: Vec<usize> = Vec::new();
            let inside_box = |q: &IntPoint<i32>| q.x >= x0 && q.x <= x1 && q.y >= y0 && q.y <= y1;
            if span > 0 && span as usize <= cells.len() {
                for cx in c0.0..=c1.0 {
                    for cy in c0.1..=c1.1 {
                        near.extend(cells.get(&(cx, cy)).into_iter().flatten().copied());
                    }
                }
            } else {
                near.extend(cells.values().flatten().copied());
            }
            near.retain(|i| out.get(*i).is_some_and(Option::is_none) && queries.get(*i).is_some_and(inside_box));
            if near.is_empty() {
                continue;
            }
            let asked: Vec<IntPoint<i32>> = near.iter().filter_map(|i| queries.get(*i).copied()).collect();
            let inside = shape.as_slice().contains_points(&asked);
            for (i, yes) in near.iter().zip(inside) {
                if yes && let Some(slot) = out.get_mut(*i) {
                    *slot = Some(k);
                }
            }
        }
        out
    }
}

fn shape_mm(s: &IntShape<i32>) -> Shape {
    let mut contours = s.iter();
    Shape { outer: contours.next().map(|c| mm_contour(c)).unwrap_or_default(), holes: contours.map(|c| mm_contour(c)).collect() }
}
