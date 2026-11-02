//! Text as filled outlines of the built-in font (Liberation Sans, as the print pages use), for
//! the legend and copper text of the manufacturing files.

use solidcraft_doc::ecad::graphics::{Graphic, GraphicType};
use solidcraft_doc::ecad::overbar;
use solidcraft_geom::region::Region;
use ttf_parser::{Face, OutlineBuilder};

/// Pieces each quadratic or cubic curve of a glyph is cut into.
const CURVE_STEPS: usize = 6;

/// How a text is mirrored, as text on the bottom reads seen from the top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mirror {
    No,
    /// About its start: it runs to the left of its `at` (a board drawing made mirrored).
    AboutStart,
    /// Within the box it covers (a bottom footprint's text, placed to cover its box).
    InPlace,
}

/// The font, ready to draw text with.
pub struct Glyphs {
    data: solidcraft_sketch::FontData,
}

struct Contours {
    all: Vec<Vec<[f64; 2]>>,
    now: Vec<[f64; 2]>,
}

impl Contours {
    fn last(&self) -> [f64; 2] {
        self.now.last().copied().unwrap_or([0.0, 0.0])
    }
}

impl OutlineBuilder for Contours {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.now.len() >= 3 {
            self.all.push(std::mem::take(&mut self.now));
        }
        self.now = vec![[f64::from(x), f64::from(y)]];
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.now.push([f64::from(x), f64::from(y)]);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (p0, p1, p2) = (self.last(), [f64::from(x1), f64::from(y1)], [f64::from(x), f64::from(y)]);
        for k in 1..=CURVE_STEPS {
            let t = k as f64 / CURVE_STEPS as f64;
            let u = 1.0 - t;
            self.now.push([u * u * p0[0] + 2.0 * u * t * p1[0] + t * t * p2[0], u * u * p0[1] + 2.0 * u * t * p1[1] + t * t * p2[1]]);
        }
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (p0, p1, p2, p3) = (self.last(), [f64::from(x1), f64::from(y1)], [f64::from(x2), f64::from(y2)], [f64::from(x), f64::from(y)]);
        for k in 1..=CURVE_STEPS {
            let t = k as f64 / CURVE_STEPS as f64;
            let u = 1.0 - t;
            let b = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            self.now.push([b[0] * p0[0] + b[1] * p1[0] + b[2] * p2[0] + b[3] * p3[0], b[0] * p0[1] + b[1] * p1[1] + b[2] * p2[1] + b[3] * p3[1]]);
        }
    }
    fn close(&mut self) {
        if self.now.len() >= 3 {
            self.all.push(std::mem::take(&mut self.now));
        }
        self.now.clear();
    }
}

impl Glyphs {
    pub fn new() -> Result<Self, String> {
        let data = solidcraft_sketch::font_data(None).map_err(|e| format!("the built-in font: {e}"))?;
        Face::parse(data.bytes(), 0).map_err(|e| format!("the built-in font does not read: {e}"))?;
        Ok(Glyphs { data })
    }

    /// `g`'s text filled: its em the text's height, its baseline above the box's bottom by the
    /// font's descent, turned by its rotation about `at`, mirrored as `mirror` says. Empty for
    /// anything but a text with a place and a height.
    pub fn text(&self, g: &Graphic, mirror: Mirror) -> Region {
        let (GraphicType::Text, Some(at), Some(h)) = (g.kind, g.at, g.height) else { return Region::new() };
        let Ok(face) = Face::parse(self.data.bytes(), 0) else { return Region::new() };
        if !(h.is_finite() && h > 0.0) {
            return Region::new();
        }
        let upem = f64::from(face.units_per_em().max(1));
        let descent = -f64::from(face.descender()) / upem;
        let scale = h / upem;
        let mut contours: Vec<Vec<[f64; 2]>> = Vec::new();
        let mut x = 0.0;
        for ch in overbar::plain(&g.text).chars().filter(|c| !c.is_control()) {
            let id = face.glyph_index(ch).or_else(|| face.glyph_index('?')).unwrap_or(ttf_parser::GlyphId(0));
            let mut b = Contours { all: Vec::new(), now: Vec::new() };
            let _ = face.outline_glyph(id, &mut b);
            b.close();
            for c in b.all {
                contours.push(c.into_iter().map(|[u, v]| [x + u * scale, v * scale + descent * h]).collect());
            }
            x += f64::from(face.glyph_hor_advance(id).unwrap_or(0)) * scale;
        }
        let width = x;
        let (s, c) = g.rotation.to_radians().sin_cos();
        let place = |[u, v]: [f64; 2]| {
            let u = match mirror {
                Mirror::No => u,
                Mirror::AboutStart => -u,
                Mirror::InPlace => width - u,
            };
            [at[0] + u * c - v * s, at[1] + u * s + v * c]
        };
        let placed: Vec<Vec<[f64; 2]>> = contours.into_iter().map(|c| c.into_iter().map(place).collect()).collect();
        Region::even_odd(placed.iter().map(Vec::as_slice))
    }
}
