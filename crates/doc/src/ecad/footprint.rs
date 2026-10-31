//! A footprint in the design's library: the copper pads a part is soldered to, as seen from the top
//! of the board, and its silkscreen, fabrication and courtyard graphics. Electronic components use
//! a footprint by id, so one footprint (SOT-23) serves many components. A pad is a rectangle, a circle, an oval or a rounded rectangle; a surface-mount pad
//! is on one side, a plated through-hole pad has a drill and copper on both sides, and an unplated
//! hole is a drill with no copper.
//!
//! Every pad shape is a convex core (a point, a line or a rectangle) grown by a radius, so areas,
//! boxes and the gaps between pads are exact.

use serde::{Deserialize, Serialize};

use super::{MAX_DESCRIPTION, MAX_ID, check_item_name, check_number, check_point, check_text};

/// The most pads a footprint has.
pub const MAX_PADS: usize = 4096;
/// The smallest pad side, in millimetres.
pub const MIN_PAD_SIZE: f64 = 0.001;
/// The largest pad side, in millimetres.
pub const MAX_PAD_SIZE: f64 = 100.0;
/// The smallest drill, in millimetres.
pub const MIN_DRILL: f64 = 0.05;

/// A footprint.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FootprintItem {
    /// Never 0; symbols, footprints, 3D packages and electronic components share one id space.
    pub id: u64,
    /// Unique among the footprints, such as `SOT-23` or `R0603`.
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pads: Vec<Pad>,
    /// Silkscreen, fabrication and courtyard graphics, each with its `layer` and `side`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphics: Vec<super::Graphic>,
}

/// A pad: copper (or a bare hole) at one place in the footprint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pad {
    /// Unique in the footprint, such as `1` or `A3`; the symbol's pin with the same number connects
    /// to it.
    pub number: String,
    /// The pad's centre, in millimetres from the footprint's origin.
    pub at: [f64; 2],
    /// Width along x and height along y before the rotation, in millimetres.
    pub size: [f64; 2],
    /// Degrees counter-clockwise about the centre.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    /// The side of a surface-mount pad; through-hole pads and holes are on both.
    #[serde(default, skip_serializing_if = "Side::is_top")]
    pub side: Side,
    #[serde(default, skip_serializing_if = "PadShape::is_rect")]
    pub shape: PadShape,
    /// A rounded rectangle's corner radius, as a share of its shorter side: more than 0, at most 0.5.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub corner: f64,
    #[serde(default, skip_serializing_if = "PadKind::is_smd")]
    pub kind: PadKind,
    /// The drill of a through-hole pad or a hole: `[d, d]` for a round hole, `[w, h]` for a slot
    /// (an oval hole), before the pad's rotation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drill: Option<[f64; 2]>,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// A pad's outline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadShape {
    #[default]
    Rect,
    /// A circle: both sides equal.
    Round,
    /// A rectangle with half-round ends across its shorter side.
    Oval,
    RoundedRect,
}

impl PadShape {
    fn is_rect(&self) -> bool {
        *self == PadShape::Rect
    }
}

/// What a pad is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PadKind {
    /// Surface-mount: copper on one side, no drill.
    #[default]
    Smd,
    /// A plated hole with copper around it on both sides.
    ThroughHole,
    /// An unplated hole: no copper.
    Hole,
}

impl PadKind {
    fn is_smd(&self) -> bool {
        *self == PadKind::Smd
    }
}

/// A convex core (1, 2 or 4 points, in the board frame) grown by `radius`.
#[derive(Clone, Debug, PartialEq)]
pub struct Core {
    pub points: Vec<[f64; 2]>,
    pub radius: f64,
}

impl Core {
    /// The core's own area and perimeter (0 for a point, twice the length for a line).
    fn area_and_perimeter(&self) -> (f64, f64) {
        let n = self.points.len();
        let mut area = 0.0;
        let mut perimeter = 0.0;
        for i in 0..n {
            let ([x0, y0], [x1, y1]) = (self.points[i], self.points[(i + 1) % n]);
            area += x0 * y1 - x1 * y0;
            perimeter += (x1 - x0).hypot(y1 - y0);
        }
        // A line goes there and back: its perimeter is twice its length, as a grown line needs.
        ((area / 2.0).abs(), perimeter)
    }

    /// The area of the core grown by the radius: area + perimeter × r + π r².
    pub fn area(&self) -> f64 {
        let (a, p) = self.area_and_perimeter();
        a + p * self.radius + std::f64::consts::PI * self.radius * self.radius
    }

    pub fn bbox(&self) -> [[f64; 2]; 2] {
        let [[x0, y0], [x1, y1]] = super::bbox(self.points.iter().copied()).unwrap_or([[0.0; 2]; 2]);
        let r = self.radius;
        [[x0 - r, y0 - r], [x1 + r, y1 + r]]
    }
}

/// The core of a `w` × `h` shape centred on the origin, before rotation: a rectangle shrunk by the
/// radius, or a line or point when it shrinks to one.
fn local_core(shape: PadShape, [w, h]: [f64; 2], corner: f64) -> (Vec<[f64; 2]>, f64) {
    let r = match shape {
        PadShape::Rect => 0.0,
        PadShape::Round | PadShape::Oval => w.min(h) / 2.0,
        PadShape::RoundedRect => corner * w.min(h),
    };
    let (hx, hy) = ((w / 2.0 - r).max(0.0), (h / 2.0 - r).max(0.0));
    let points = match (hx > 0.0, hy > 0.0) {
        (true, true) => vec![[-hx, -hy], [hx, -hy], [hx, hy], [-hx, hy]],
        (true, false) => vec![[-hx, 0.0], [hx, 0.0]],
        (false, true) => vec![[0.0, -hy], [0.0, hy]],
        (false, false) => vec![[0.0, 0.0]],
    };
    (points, r)
}

/// Which side of the board a pad is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    #[default]
    Top,
    Bottom,
}

impl Side {
    fn is_top(&self) -> bool {
        *self == Side::Top
    }
}

impl Pad {
    pub fn validate(&self) -> Result<(), String> {
        check_number("pad", &self.number)?;
        check_point(&format!("pad {} `at`", self.number), self.at)?;
        for side in self.size {
            if !(side.is_finite() && (MIN_PAD_SIZE..=MAX_PAD_SIZE).contains(&side)) {
                return Err(format!("pad {}: each `size` side must be {MIN_PAD_SIZE} to {MAX_PAD_SIZE} mm", self.number));
            }
        }
        if !(self.rotation.is_finite() && self.rotation.abs() <= 360.0) {
            return Err(format!("pad {}: `rotation` must be -360 to 360 degrees", self.number));
        }
        let n = &self.number;
        let [w, h] = self.size;
        if self.shape == PadShape::Round && (w - h).abs() > 1e-9 {
            return Err(format!("pad {n}: a round pad's `size` sides must be equal (an oval pad has two)"));
        }
        match self.shape {
            PadShape::RoundedRect if !(self.corner.is_finite() && self.corner > 0.0 && self.corner <= 0.5) => {
                return Err(format!("pad {n}: a rounded rectangle's `corner` must be more than 0 and at most 0.5 of its shorter side"));
            }
            PadShape::Rect | PadShape::Round | PadShape::Oval if self.corner != 0.0 => {
                return Err(format!("pad {n}: only a rounded rectangle has a `corner`"));
            }
            _ => {}
        }
        match (self.kind, self.drill) {
            (PadKind::Smd, Some(_)) => return Err(format!("pad {n}: a surface-mount pad has no `drill`; make it a through-hole pad")),
            (PadKind::ThroughHole | PadKind::Hole, None) => return Err(format!("pad {n}: a through-hole pad or a hole needs a `drill`")),
            (PadKind::ThroughHole | PadKind::Hole, Some([dw, dh])) => {
                if !(dw.is_finite() && dh.is_finite() && dw >= MIN_DRILL && dh >= MIN_DRILL) {
                    return Err(format!("pad {n}: each `drill` side must be at least {MIN_DRILL} mm"));
                }
                if self.kind == PadKind::ThroughHole && !(dw < w && dh < h) {
                    return Err(format!("pad {n}: the drill must be smaller than the pad, leaving copper around it"));
                }
                if self.kind == PadKind::Hole && !(dw <= w && dh <= h) {
                    return Err(format!("pad {n}: the drill must fit inside the hole's `size`"));
                }
                if self.side != Side::Top {
                    return Err(format!("pad {n}: a through-hole pad or a hole goes through the board: leave out `side`"));
                }
            }
            (PadKind::Smd, None) => {}
        }
        Ok(())
    }

    /// The pad's shape as a convex core grown by a radius, in the footprint's frame.
    pub fn core(&self) -> Core {
        let (points, radius) = local_core(self.shape, self.size, self.corner);
        Core { points: points.into_iter().map(|p| self.place(p)).collect(), radius }
    }

    /// A point given relative to the pad's centre, before rotation, in the footprint's frame.
    fn place(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        let (s, c) = self.rotation.to_radians().sin_cos();
        [self.at[0] + x * c - y * s, self.at[1] + x * s + y * c]
    }

    /// The drill's area, in square millimetres (0 without one).
    pub fn drill_area(&self) -> f64 {
        self.drill.map_or(0.0, |d| {
            let (points, radius) = local_core(PadShape::Oval, d, 0.0);
            Core { points, radius }.area()
        })
    }

    /// Copper area, in square millimetres: the pad less its drill; none for a hole.
    pub fn area(&self) -> f64 {
        match self.kind {
            PadKind::Hole => 0.0,
            _ => self.core().area() - self.drill_area(),
        }
    }

    /// The narrowest copper between the drill and the pad's edge, for a through-hole pad.
    pub fn annular_ring(&self) -> Option<f64> {
        match (self.kind, self.drill) {
            (PadKind::ThroughHole, Some([dw, dh])) => Some(((self.size[0] - dw) / 2.0).min((self.size[1] - dh) / 2.0)),
            _ => None,
        }
    }

    /// The outline as a polygon, for drawing: arcs as `per_quarter` straight pieces a quarter turn.
    pub fn outline(&self, per_quarter: usize) -> Vec<[f64; 2]> {
        let (points, r) = local_core(self.shape, self.size, self.corner);
        if r <= 0.0 {
            return points.into_iter().map(|p| self.place(p)).collect();
        }
        // Each core corner, from its centre outward, sweeps the quarter turn facing away from the
        // core's middle; a line or a point stands for two or four such corners.
        let [hx, hy] = [(self.size[0] / 2.0 - r).max(0.0), (self.size[1] / 2.0 - r).max(0.0)];
        let corners = [[hx, hy], [-hx, hy], [-hx, -hy], [hx, -hy]];
        let steps = per_quarter.clamp(1, 32);
        let mut out = Vec::with_capacity(4 * (steps + 1));
        for (q, [cx, cy]) in corners.into_iter().enumerate() {
            for i in 0..=steps {
                let a = (q as f64 + i as f64 / steps as f64) * std::f64::consts::FRAC_PI_2;
                out.push(self.place([cx + r * a.cos(), cy + r * a.sin()]));
            }
        }
        out
    }

    /// The smallest box around the pad, rotation included.
    pub fn bbox(&self) -> [[f64; 2]; 2] {
        self.core().bbox()
    }

    /// The drill's outline as a polygon, for drawing; `None` without a drill.
    pub fn drill_outline(&self, per_quarter: usize) -> Option<Vec<[f64; 2]>> {
        let drill = self.drill?;
        let hole = Pad { shape: PadShape::Oval, size: drill, corner: 0.0, kind: PadKind::Smd, drill: None, ..self.clone() };
        Some(hole.outline(per_quarter))
    }
}

/// What a variant without a footprint has: nothing.
pub static NO_FOOTPRINT: FootprintItem =
    FootprintItem { id: 0, name: String::new(), description: String::new(), pads: Vec::new(), graphics: Vec::new() };

impl FootprintItem {
    /// Whether it has nothing to draw: no pads and no graphics.
    pub fn is_empty(&self) -> bool {
        self.pads.is_empty() && self.graphics.is_empty()
    }

    /// The graphics on one layer and side.
    pub fn layer(&self, layer: super::BoardLayer, side: Side) -> impl Iterator<Item = &super::Graphic> {
        self.graphics.iter().filter(move |g| g.layer == Some(layer) && g.side == side)
    }

    pub fn pad(&self, number: &str) -> Option<&Pad> {
        self.pads.iter().find(|p| p.number == number)
    }

    /// The record's own rules (an id, a name, a description within its length), the caps, each
    /// pad's rules, and unique numbers.
    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0 || self.id >= MAX_ID {
            return Err(format!("footprint `{}` has the id {}, which is out of range", self.name, self.id));
        }
        check_item_name("footprint", &self.name)?;
        check_text("description", &self.description, MAX_DESCRIPTION)?;
        self.validate_drawing().map_err(|e| format!("footprint `{}`: {e}", self.name))
    }

    /// The pads and graphics alone: the caps, each pad's rules, and unique numbers.
    pub fn validate_drawing(&self) -> Result<(), String> {
        if self.pads.len() > MAX_PADS {
            return Err(format!("a footprint has at most {MAX_PADS} pads"));
        }
        let mut numbers = std::collections::BTreeSet::new();
        for p in &self.pads {
            p.validate()?;
            if !numbers.insert(p.number.as_str()) {
                return Err(format!("two pads are numbered `{}`", p.number));
            }
        }
        super::graphics::validate_all(&self.graphics, true).map_err(|e| format!("footprint {e}"))
    }

    /// The smallest box around all the copper; `None` with no pads.
    pub fn copper_bbox(&self) -> Option<[[f64; 2]; 2]> {
        super::bbox(self.pads.iter().flat_map(|p| p.bbox()))
    }
}
