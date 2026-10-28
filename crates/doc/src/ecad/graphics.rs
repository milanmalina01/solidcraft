//! Lines, shapes and text drawn on an electronic component's symbol (its body) or on one of its
//! footprint's board layers (silkscreen, fabrication, courtyard). Copper is the pads; graphics
//! never connect anything.

use serde::{Deserialize, Serialize};

use super::footprint::Side;
use super::{MAX_COORD, check_point, check_text};

/// The most graphics a symbol, or a footprint, has.
pub const MAX_GRAPHICS: usize = 4096;
/// The most points one line or polygon has.
pub const MAX_POINTS: usize = 1024;
/// The widest line, in millimetres.
pub const MAX_WIDTH: f64 = 10.0;
/// A line's width when none is given, in millimetres.
pub const DEFAULT_WIDTH: f64 = 0.15;
/// The longest text, in characters.
pub const MAX_GRAPHIC_TEXT: usize = 256;

/// What a graphic is; which other fields it has follows from this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphicType {
    /// `points`, joined in order.
    Line,
    /// The rectangle with corners `from` and `to`.
    Rect,
    /// `center` and `radius`.
    Circle,
    /// `center`, `radius`, `start` angle and `sweep` (degrees, counter-clockwise when positive).
    Arc,
    /// `points`, closed.
    Polygon,
    /// `text` from `at`, `height` tall, turned by `rotation`.
    Text,
}

/// The board layer a footprint graphic is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoardLayer {
    /// What is printed on the board.
    Silkscreen,
    /// The part's real outline, for assembly drawings.
    Fab,
    /// The keep-out other parts' courtyards must not overlap.
    Courtyard,
}

/// One graphic. `layer` (and `side`) belong to footprint graphics only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graphic {
    #[serde(rename = "type")]
    pub kind: GraphicType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<BoardLayer>,
    #[serde(default, skip_serializing_if = "is_top")]
    pub side: Side,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    /// Line width in millimetres; text uses a tenth of its height.
    #[serde(default = "default_width")]
    pub width: f64,
    /// Filled: rectangles, circles and polygons.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fill: bool,
}

fn is_top(side: &Side) -> bool {
    *side == Side::Top
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn default_width() -> f64 {
    DEFAULT_WIDTH
}

impl Graphic {
    /// The fields its type needs and only those, finite and within the page.
    pub fn validate(&self) -> Result<(), String> {
        let what = format!("{:?} graphic", self.kind).to_lowercase();
        let has = |set: bool, name: &str| if set { Ok(()) } else { Err(format!("a {what} needs `{name}`")) };
        let extra = |set: bool, name: &str| if set { Err(format!("a {what} has no `{name}`")) } else { Ok(()) };
        use GraphicType::*;
        let k = self.kind;
        has(!self.points.is_empty() || !matches!(k, Line | Polygon), "points")?;
        extra(!self.points.is_empty() && !matches!(k, Line | Polygon), "points")?;
        has(self.from.is_some() || k != Rect, "from")?;
        has(self.to.is_some() || k != Rect, "to")?;
        extra((self.from.is_some() || self.to.is_some()) && k != Rect, "from")?;
        has(self.center.is_some() || !matches!(k, Circle | Arc), "center")?;
        has(self.radius.is_some() || !matches!(k, Circle | Arc), "radius")?;
        extra((self.center.is_some() || self.radius.is_some()) && !matches!(k, Circle | Arc), "center")?;
        has(self.start.is_some() || k != Arc, "start")?;
        has(self.sweep.is_some() || k != Arc, "sweep")?;
        extra((self.start.is_some() || self.sweep.is_some()) && k != Arc, "start")?;
        has(self.at.is_some() || k != Text, "at")?;
        has(!self.text.is_empty() || k != Text, "text")?;
        has(self.height.is_some() || k != Text, "height")?;
        extra((self.at.is_some() || !self.text.is_empty() || self.height.is_some()) && k != Text, "text")?;
        extra(self.rotation != 0.0 && k != Text, "rotation")?;
        extra(self.fill && !matches!(k, Rect | Circle | Polygon), "fill")?;
        if (k == Line && self.points.len() < 2) || (k == Polygon && self.points.len() < 3) {
            return Err(format!("a {what} needs at least {} points", if k == Line { 2 } else { 3 }));
        }
        if self.points.len() > MAX_POINTS {
            return Err(format!("a {what} has at most {MAX_POINTS} points"));
        }
        for p in self.points.iter().chain(&self.from).chain(&self.to).chain(&self.center).chain(&self.at) {
            check_point(&what, *p)?;
        }
        if let Some(r) = self.radius
            && !(r.is_finite() && r > 0.0 && r <= MAX_COORD)
        {
            return Err(format!("a {what}'s `radius` must be more than 0 and at most {MAX_COORD} mm"));
        }
        for (name, v) in [("start", self.start), ("sweep", self.sweep)] {
            if let Some(v) = v
                && !(v.is_finite() && v.abs() <= 360.0)
            {
                return Err(format!("a {what}'s `{name}` must be -360 to 360 degrees"));
            }
        }
        if self.sweep == Some(0.0) {
            return Err(format!("a {what}'s `sweep` must not be 0"));
        }
        if let Some(h) = self.height
            && !(h.is_finite() && h > 0.0 && h <= 100.0)
        {
            return Err(format!("a {what}'s `height` must be more than 0 and at most 100 mm"));
        }
        if !(self.rotation.is_finite() && self.rotation.abs() <= 360.0) {
            return Err(format!("a {what}'s `rotation` must be -360 to 360 degrees"));
        }
        if !(self.width.is_finite() && self.width >= 0.0 && self.width <= MAX_WIDTH) {
            return Err(format!("a {what}'s `width` must be 0 to {MAX_WIDTH} mm"));
        }
        check_text("text", &self.text, MAX_GRAPHIC_TEXT)?;
        Ok(())
    }

    /// The arc's points from its start to its end, `steps` pieces.
    pub fn arc_points(&self, steps: usize) -> Vec<[f64; 2]> {
        let (Some([cx, cy]), Some(r), Some(start), Some(sweep)) = (self.center, self.radius, self.start, self.sweep) else {
            return Vec::new();
        };
        let n = steps.clamp(2, 256);
        (0..=n)
            .map(|i| {
                let a = (start + sweep * i as f64 / n as f64).to_radians();
                [cx + r * a.cos(), cy + r * a.sin()]
            })
            .collect()
    }

    /// The text's box corners, counter-clockwise from `at` (its lower left before rotation), taking a
    /// character as 0.6 of the height wide.
    pub fn text_box(&self) -> Vec<[f64; 2]> {
        let (Some([x, y]), Some(h)) = (self.at, self.height) else { return Vec::new() };
        let w = h * 0.6 * self.text.chars().count() as f64;
        let (s, c) = self.rotation.to_radians().sin_cos();
        [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]].iter().map(|[u, v]| [x + u * c - v * s, y + u * s + v * c]).collect()
    }

    /// The smallest box around the graphic, its line width included.
    pub fn bbox(&self) -> Option<[[f64; 2]; 2]> {
        let half = self.width / 2.0;
        let [[x0, y0], [x1, y1]] = self.shape_bbox()?;
        Some([[x0 - half, y0 - half], [x1 + half, y1 + half]])
    }

    /// The smallest box around the graphic's geometry, its line width left out (a courtyard's box
    /// is its centre line).
    pub fn shape_bbox(&self) -> Option<[[f64; 2]; 2]> {
        let points: Vec<[f64; 2]> = match self.kind {
            GraphicType::Line | GraphicType::Polygon => self.points.clone(),
            GraphicType::Rect => self.from.into_iter().chain(self.to).collect(),
            GraphicType::Circle => {
                let ([cx, cy], r) = (self.center?, self.radius?);
                vec![[cx - r, cy - r], [cx + r, cy + r]]
            }
            GraphicType::Arc => self.arc_points(128),
            GraphicType::Text => self.text_box(),
        };
        super::bbox(points)
    }
}

/// The box around several graphics, line widths included.
pub fn graphics_bbox<'a>(graphics: impl IntoIterator<Item = &'a Graphic>) -> Option<[[f64; 2]; 2]> {
    super::bbox(graphics.into_iter().filter_map(Graphic::bbox).flatten())
}

/// The box around several graphics' geometry, line widths left out.
pub fn shapes_bbox<'a>(graphics: impl IntoIterator<Item = &'a Graphic>) -> Option<[[f64; 2]; 2]> {
    super::bbox(graphics.into_iter().filter_map(Graphic::shape_bbox).flatten())
}

/// The caps, each graphic's rules, and whether it belongs on a symbol (`footprint` false: no
/// layer) or a footprint (a layer).
pub fn validate_all(graphics: &[Graphic], footprint: bool) -> Result<(), String> {
    if graphics.len() > MAX_GRAPHICS {
        return Err(format!("at most {MAX_GRAPHICS} graphics"));
    }
    for (i, g) in graphics.iter().enumerate() {
        g.validate().map_err(|e| format!("graphics[{i}]: {e}"))?;
        match (footprint, g.layer) {
            (false, Some(_)) => return Err(format!("graphics[{i}]: a symbol graphic has no `layer`")),
            (false, None) if g.side != Side::Top => return Err(format!("graphics[{i}]: a symbol graphic has no `side`")),
            (true, None) => return Err(format!("graphics[{i}]: a footprint graphic needs a `layer`")),
            _ => {}
        }
    }
    Ok(())
}
