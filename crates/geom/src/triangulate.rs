//! Triangles covering a polygon with holes, for drawing it filled: a constrained Delaunay
//! triangulation of its contours (spade), with the triangles inside kept by a flood fill from
//! outside that flips each time it crosses a contour, so holes stay open. Linear in the corners
//! after the triangulation's n log n. A contour edge that crosses another is left out instead of
//! refused; the flood then may fill a little on the wrong side of it, which a drawing can bear.

use spade::handles::{FixedFaceHandle, InnerTag};
use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

use crate::region::Shape;

/// The most corners a shape may have to be triangulated.
pub const MAX_TRIANGULATED_CORNERS: usize = 1 << 19;

type Cdt = ConstrainedDelaunayTriangulation<Point2<f64>>;

/// The triangles covering `shape` (counter-clockwise, in its millimetres); `None` when it has
/// more than `max_corners` corners (at most [`MAX_TRIANGULATED_CORNERS`]), a corner that isn't
/// finite, or the triangulation fails.
pub fn triangles(shape: &Shape, max_corners: usize) -> Option<Vec<[[f64; 2]; 3]>> {
    let corners = shape.corners();
    if corners > max_corners.min(MAX_TRIANGULATED_CORNERS) {
        return None;
    }
    let mut vertices: Vec<Point2<f64>> = Vec::with_capacity(corners);
    let mut edges: Vec<[usize; 2]> = Vec::with_capacity(corners);
    for ring in std::iter::once(&shape.outer).chain(&shape.holes) {
        if ring.len() < 3 {
            continue;
        }
        let start = vertices.len();
        for p in ring {
            if !(p[0].is_finite() && p[1].is_finite()) {
                return None;
            }
            vertices.push(Point2::new(p[0], p[1]));
        }
        let n = ring.len();
        edges.extend((0..n).map(|i| [start + i, start + (i + 1) % n]));
    }
    if vertices.len() < 3 {
        return Some(Vec::new());
    }
    let cdt = Cdt::try_bulk_load_cdt(vertices, edges, |_| {}).ok()?;
    let inside = inside_faces(&cdt);
    let mut out = Vec::with_capacity(cdt.num_inner_faces());
    for face in cdt.inner_faces() {
        if !inside.get(face.fix().index()).copied().unwrap_or(false) {
            continue;
        }
        let [a, b, c] = face.positions();
        out.push([[a.x, a.y], [b.x, b.y], [c.x, c.y]]);
    }
    Some(out)
}

/// Per face (by index): inside the contours. A flood fill from the faces along the convex hull,
/// flipping each time it crosses a contour edge.
fn inside_faces(cdt: &Cdt) -> Vec<bool> {
    let mut parity: Vec<Option<bool>> = vec![None; cdt.num_all_faces()];
    let mut todo: Vec<(FixedFaceHandle<InnerTag>, bool)> = Vec::new();
    for e in cdt.convex_hull() {
        for side in [e, e.rev()] {
            if let Some(inner) = side.face().as_inner() {
                todo.push((inner.fix(), e.is_constraint_edge()));
            }
        }
    }
    while let Some((h, odd)) = todo.pop() {
        match parity.get_mut(h.index()) {
            Some(slot @ None) => *slot = Some(odd),
            _ => continue,
        }
        for e in cdt.face(h).adjacent_edges() {
            if let Some(next) = e.rev().face().as_inner() {
                todo.push((next.fix(), odd ^ e.is_constraint_edge()));
            }
        }
    }
    parity.into_iter().map(|p| p.unwrap_or(false)).collect()
}
