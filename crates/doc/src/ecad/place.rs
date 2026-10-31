//! Where a part sits on the schematic: its symbol mirrored first (x → −x) when `mirror` is set,
//! then turned `rotation` degrees counter-clockwise in exact quarter turns, then moved to `at`. A
//! quarter turn swaps coordinates instead of multiplying by a sine, so a pin on the grid stays
//! exactly on it. Text stays readable: text that would read upside down or mirrored is turned
//! half a turn and moved so it covers the same box.
//!
//! Also here: [`Motion`], the rigid move of a selection (the move and duplicate commands), and the
//! flow layout that places parts given no position.

use serde::{Deserialize, Serialize};

use super::{Graphic, GraphicType, Pin, PinDirection};

/// The finest step of a saved sheet point, in millimetres: commands round to it, so points that
/// meet compare exactly.
pub const STEP: f64 = 1e-4;
/// The schematic grid, in millimetres (0.1 in): pins of a symbol on it land on it when placed.
pub const GRID: f64 = 2.54;
/// The gap the flow layout leaves between parts and between rows, in millimetres.
pub const FLOW_GAP: f64 = 10.16;
/// How wide a row of the flow layout grows before it wraps, in millimetres.
pub const FLOW_WIDTH: f64 = 254.0;

/// A part's placement on the sheet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    /// Where the symbol's origin goes, in millimetres.
    pub at: [f64; 2],
    /// Degrees counter-clockwise: 0, 90, 180 or 270.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: u16,
    /// Mirrored left to right (x → −x) before the turn.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mirror: bool,
}

fn is_zero(v: &u16) -> bool {
    *v == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// `v` rounded to [`STEP`].
pub fn snap(v: f64) -> f64 {
    // Adding 0.0 turns -0.0 into 0.0, so a point never saves as `-0.0`.
    (v * 1e4).round() / 1e4 + 0.0
}

/// A point rounded to [`STEP`].
pub fn snap_point([x, y]: [f64; 2]) -> [f64; 2] {
    [snap(x), snap(y)]
}

/// A point's place on the lattice of [`STEP`]: equal for points that meet. Values beyond i64
/// saturate, which only out-of-range points reach.
pub fn lattice([x, y]: [f64; 2]) -> [i64; 2] {
    [(x * 1e4).round() as i64, (y * 1e4).round() as i64]
}

/// `v` moved to the grid of `step`, up (`up`) or down; a value already on it (to within rounding)
/// stays.
fn to_grid(v: f64, up: bool, step: f64) -> f64 {
    let n = v / step;
    snap(if up { (n - 1e-9).ceil() } else { (n + 1e-9).floor() } * step)
}

/// Whether `rotation` is a quarter turn this module places: 0, 90, 180 or 270.
pub fn is_quarter(rotation: u16) -> bool {
    matches!(rotation, 0 | 90 | 180 | 270)
}

/// A vector turned `rotation` degrees counter-clockwise, `rotation` a quarter turn (anything else
/// is taken as the quarter turn below it).
fn turn([x, y]: [f64; 2], rotation: u16) -> [f64; 2] {
    match rotation % 360 / 90 {
        0 => [x, y],
        1 => [-y, x],
        2 => [-x, -y],
        _ => [y, -x],
    }
}

/// An angle in degrees, in [0, 360).
fn degrees(a: f64) -> f64 {
    a.rem_euclid(360.0) + 0.0
}

fn dot([ax, ay]: [f64; 2], [bx, by]: [f64; 2]) -> f64 {
    ax * bx + ay * by
}

/// The direction closest to a unit vector.
fn direction_of([x, y]: [f64; 2]) -> PinDirection {
    if x > 0.5 {
        PinDirection::Right
    } else if x < -0.5 {
        PinDirection::Left
    } else if y > 0.5 {
        PinDirection::Up
    } else {
        PinDirection::Down
    }
}

impl Placement {
    /// A vector of the symbol on the sheet: mirrored, then turned (no move).
    pub fn vector(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        turn([if self.mirror { -x } else { x }, y], self.rotation)
    }

    /// A point of the symbol on the sheet.
    pub fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        let [x, y] = self.vector(p);
        [self.at[0] + x, self.at[1] + y]
    }

    /// The point of the symbol that [`Placement::apply`] puts at `q`.
    pub fn unapply(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        let [u, v] = turn([x - self.at[0], y - self.at[1]], (360 - self.rotation % 360) % 360);
        [if self.mirror { -u } else { u }, v]
    }

    /// A point of the symbol on the sheet, on the lattice of [`STEP`] (see [`lattice`]): `at` and
    /// the symbol point each put on the lattice first, then mirrored and turned exactly, so the
    /// pins of parts that meet compare equal whatever their coordinates.
    pub fn lattice_point(&self, p: [f64; 2]) -> [i64; 2] {
        let [x, y] = lattice(p);
        let x = if self.mirror { x.saturating_neg() } else { x };
        let [dx, dy] = match self.rotation % 360 / 90 {
            0 => [x, y],
            1 => [y.saturating_neg(), x],
            2 => [x.saturating_neg(), y.saturating_neg()],
            _ => [y, x.saturating_neg()],
        };
        let [ax, ay] = lattice(self.at);
        [ax.saturating_add(dx), ay.saturating_add(dy)]
    }

    /// A pin direction on the sheet.
    pub fn direction(&self, d: PinDirection) -> PinDirection {
        direction_of(self.vector(d.unit()))
    }

    /// An angle of the symbol (degrees counter-clockwise from +x) on the sheet, in [0, 360).
    pub fn angle(&self, a: f64) -> f64 {
        degrees(if self.mirror { 180.0 - a } else { a } + f64::from(self.rotation))
    }

    /// A pin as it lies on the sheet: its end where a wire connects, and its direction.
    pub fn pin(&self, p: &Pin) -> Pin {
        Pin { at: self.apply(p.at), direction: self.direction(p.direction), ..p.clone() }
    }

    /// A box of the symbol (`[[x0, y0], [x1, y1]]`) on the sheet.
    pub fn bbox(&self, [a, b]: [[f64; 2]; 2]) -> [[f64; 2]; 2] {
        let (p, q) = (self.apply(a), self.apply(b));
        [[p[0].min(q[0]), p[1].min(q[1])], [p[0].max(q[0]), p[1].max(q[1])]]
    }

    /// A symbol graphic as it lies on the sheet. A mirrored arc runs the other way (start
    /// 180 − s, sweep −w); text is kept readable (see [`Placement::text`]).
    pub fn graphic(&self, g: &Graphic) -> Graphic {
        let mut out = g.clone();
        out.points = g.points.iter().map(|p| self.apply(*p)).collect();
        out.from = g.from.map(|p| self.apply(p));
        out.to = g.to.map(|p| self.apply(p));
        out.center = g.center.map(|p| self.apply(p));
        if g.kind == GraphicType::Arc {
            out.start = g.start.map(|s| self.angle(s));
            out.sweep = g.sweep.map(|w| if self.mirror { -w } else { w });
        }
        if g.kind == GraphicType::Text {
            let (at, rotation) = self.text(g);
            out.at = at;
            out.rotation = rotation;
        }
        out
    }

    /// Where a text's lower left goes and its turn, so it reads left to right or bottom to top
    /// (a turn in (−90, 90]) and covers the box the placed text would.
    pub fn text(&self, g: &Graphic) -> (Option<[f64; 2]>, f64) {
        let mut phi = self.angle(g.rotation);
        if phi > 270.0 {
            phi -= 360.0;
        } else if phi > 90.0 {
            phi -= 180.0;
        }
        let corners = g.text_box();
        if corners.is_empty() {
            return (g.at.map(|p| self.apply(p)), phi);
        }
        // The box's corner lowest along the text's new baseline and its new up direction: the
        // one lowest along their sum, which no other corner ties.
        let (s, c) = phi.to_radians().sin_cos();
        let diagonal = [c - s, s + c];
        let at = corners.iter().map(|p| self.apply(*p)).min_by(|a, b| dot(*a, diagonal).total_cmp(&dot(*b, diagonal)));
        (at, phi)
    }
}

/// A rigid move of sheet objects: mirrored left to right about the vertical line through `about`
/// when `mirror` is set, then turned `rotation` degrees (a quarter turn) about `about`, then moved
/// `by`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motion {
    pub about: [f64; 2],
    pub mirror: bool,
    pub rotation: u16,
    pub by: [f64; 2],
}

impl Motion {
    /// Whether it leaves everything where it was.
    pub fn is_identity(&self) -> bool {
        !self.mirror && self.rotation.is_multiple_of(360) && self.by == [0.0, 0.0]
    }

    /// A vector moved (mirrored and turned; no move).
    pub fn vector(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        turn([if self.mirror { -x } else { x }, y], self.rotation)
    }

    /// A point moved, rounded to [`STEP`].
    pub fn point(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        let [dx, dy] = self.vector([x - self.about[0], y - self.about[1]]);
        snap_point([self.about[0] + dx + self.by[0], self.about[1] + dy + self.by[1]])
    }

    /// A placement moved with what it places: mirroring after a turn θ is mirroring before a turn
    /// −θ, so a mirror flips the part's own turn as well as its `mirror`.
    pub fn placement(&self, p: &Placement) -> Placement {
        let (r, theta) = (i32::from(self.rotation % 360), i32::from(p.rotation % 360));
        let rotation = if self.mirror { r - theta } else { r + theta };
        Placement { at: self.point(p.at), rotation: rotation.rem_euclid(360) as u16, mirror: p.mirror != self.mirror }
    }

    /// A label's turn (the way it points) moved.
    pub fn angle(&self, a: u16) -> u16 {
        let a = i32::from(a % 360);
        let a = if self.mirror { 180 - a } else { a };
        (a + i32::from(self.rotation % 360)).rem_euclid(360) as u16
    }
}

/// Origins for parts placed with no position: each box (the part's symbol box on the sheet,
/// relative to its origin) goes in rows under `content` (what the sheet already holds; rows start
/// at the origin on an empty sheet), left to right with [`FLOW_GAP`] between them, a row wrapping
/// once it would grow past [`FLOW_WIDTH`]. Origins are on the grid.
pub fn flow(content: Option<[[f64; 2]; 2]>, boxes: &[[[f64; 2]; 2]]) -> Vec<[f64; 2]> {
    flow_with(content, boxes, Flow { gap: FLOW_GAP, width: FLOW_WIDTH, grid: GRID })
}

/// The spacing of a [`flow_with`] layout, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flow {
    /// Between boxes, and between rows.
    pub gap: f64,
    /// How wide a row grows before it wraps.
    pub width: f64,
    /// The grid origins land on.
    pub grid: f64,
}

/// [`flow`] with its own spacing: the board lays footprints out closer, on its finer grid.
pub fn flow_with(content: Option<[[f64; 2]; 2]>, boxes: &[[[f64; 2]; 2]], spacing: Flow) -> Vec<[f64; 2]> {
    let Flow { gap, width: wrap, grid } = spacing;
    let to_grid = |v: f64, up: bool| to_grid(v, up, grid);
    let x0 = content.map_or(0.0, |[[x, _], _]| to_grid(x, false));
    let mut top = content.map_or(0.0, |[[_, y], _]| y - gap);
    let (mut cursor, mut bottom) = (x0, top);
    let mut out = Vec::with_capacity(boxes.len());
    for [[bx0, by0], [bx1, by1]] in boxes {
        let width = bx1 - bx0;
        if cursor > x0 && cursor + width > x0 + wrap {
            top = bottom - gap;
            (cursor, bottom) = (x0, top);
        }
        let origin = [to_grid(cursor - bx0, true), to_grid(top - by1, false)];
        cursor = origin[0] + bx1 + gap;
        bottom = bottom.min(origin[1] + by0);
        out.push(origin);
    }
    out
}

#[cfg(test)]
#[path = "place_tests.rs"]
mod tests;
