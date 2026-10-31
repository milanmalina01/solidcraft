//! The one font every page is set in: the built-in Liberation Sans (SIL Open Font License, shipped
//! with the sketch text), read with `ttf-parser`. Widths come from its advances, so the PDF, SVG
//! and PNG writers place text alike; bold is the regular face drawn with a thin outline of its
//! own colour (the font has no bold face of its own here).

use solidcraft_doc::ecad::overbar;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

use super::page::{Align, Baseline, Item, Page, Stroke, Text, Weight};

/// The family SVG pages ask for, with fallbacks after it.
pub const SVG_FAMILY: &str = "'Liberation Sans', Arial, Helvetica, sans-serif";
/// The outline bold text gets, as a share of its size.
pub const BOLD_STROKE: f64 = 0.04;

/// The parsed face and its metrics.
pub struct Font<'a> {
    pub face: Face<'a>,
    /// Font units in an em.
    pub upem: f64,
    /// Above and below the baseline, and the capitals' height, in ems.
    pub ascent: f64,
    pub descent: f64,
    pub cap: f64,
}

impl<'a> Font<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, String> {
        let face = Face::parse(data, 0).map_err(|e| format!("the page font does not read: {e}"))?;
        let upem = f64::from(face.units_per_em().max(1));
        let ascent = f64::from(face.ascender()) / upem;
        let descent = -f64::from(face.descender()) / upem;
        let cap = face.capital_height().map_or(0.72, |c| f64::from(c) / upem);
        Ok(Font { face, upem, ascent, descent, cap })
    }

    /// The glyph for `c`: its own, else `?`, else the font's missing-glyph box.
    pub fn glyph(&self, c: char) -> GlyphId {
        self.face.glyph_index(c).or_else(|| self.face.glyph_index('?')).unwrap_or(GlyphId(0))
    }

    /// A glyph's advance, in ems.
    pub fn advance(&self, g: GlyphId) -> f64 {
        f64::from(self.face.glyph_hor_advance(g).unwrap_or(0)) / self.upem
    }

    /// The width of `text` at `size`, in millimetres, its overbar markup left out.
    pub fn width(&self, text: &str, size: f64) -> f64 {
        overbar::plain(text).chars().map(|c| self.advance(self.glyph(c))).sum::<f64>() * size
    }

    /// The longest start of `text` that fits `width` at `size`, with an ellipsis when cut; the
    /// letters kept keep their overbar markup.
    pub fn fit(&self, text: &str, size: f64, width: f64) -> String {
        // A hair of slack, so a width measured on this very text fits it.
        if self.width(text, size) <= width + 1e-9 {
            return text.to_string();
        }
        let dots = self.width("…", size);
        let barred = overbar::parse(text);
        let mut kept = 0;
        let mut used = 0.0;
        for c in barred.text.chars() {
            let w = self.advance(self.glyph(c)) * size;
            if used + w + dots > width {
                break;
            }
            used += w;
            kept += 1;
        }
        let mut out = overbar::cut(&barred, kept);
        out.push('…');
        out
    }

    /// `text` broken at spaces (and after commas) into lines no wider than `width` at `size`;
    /// a word longer than the width is cut.
    pub fn wrap(&self, text: &str, size: f64, width: f64) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut line = String::new();
        for word in text.split_inclusive(' ') {
            let candidate = format!("{line}{word}");
            if line.is_empty() || self.width(candidate.trim_end(), size) <= width {
                line = candidate;
            } else {
                lines.push(line.trim_end().to_string());
                line = word.to_string();
            }
        }
        if !line.trim().is_empty() {
            lines.push(line.trim_end().to_string());
        }
        lines.into_iter().map(|l| self.fit(&l, size, width)).collect()
    }

    /// Where a text's baseline starts on the page, and the page directions of its run and of its
    /// down (y down), its turn and mirror applied.
    pub fn placement(&self, t: &Text) -> ([f64; 2], [f64; 2], [f64; 2]) {
        let w = self.width(&t.text, t.size);
        let (s, c) = t.rotation.to_radians().sin_cos();
        let mut along = [c, -s];
        let down = [s, c];
        if t.mirror {
            along = [-along[0], -along[1]];
        }
        let shift = match t.align {
            Align::Start => 0.0,
            Align::Middle => -w / 2.0,
            Align::End => -w,
        };
        let drop = match t.baseline {
            Baseline::Top => self.ascent * t.size,
            Baseline::Middle => self.cap * t.size / 2.0,
            Baseline::Baseline => 0.0,
            Baseline::Bottom => -self.descent * t.size,
        };
        let origin = [t.at[0] + along[0] * shift + down[0] * drop, t.at[1] + along[1] * shift + down[1] * drop];
        (origin, along, down)
    }

    /// The glyphs of `text` and where each starts along the run, in ems.
    pub fn layout(&self, text: &str) -> Vec<(GlyphId, f64)> {
        let mut x = 0.0;
        text.chars()
            .filter(|c| !c.is_control())
            .map(|c| {
                let g = self.glyph(c);
                let at = x;
                x += self.advance(g);
                (g, at)
            })
            .collect()
    }

    /// A glyph's outline in ems (y up), fed to `out`.
    pub fn outline(&self, g: GlyphId, out: &mut dyn PathSink) {
        let mut b = Builder { out, scale: 1.0 / self.upem };
        let _ = self.face.outline_glyph(g, &mut b);
    }

    /// The PostScript name the PDF names the font by.
    pub fn postscript_name(&self) -> String {
        self.face
            .names()
            .into_iter()
            .find(|n| n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME)
            .and_then(|n| n.to_string())
            .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_graphic()))
            .unwrap_or_else(|| "LiberationSans".into())
    }
}

/// Every text on `page` with overbar markup (`~{CE}`) as its plain text and a line over each
/// barred run, drawn just after it (in its clipped group, if any), so the writers see only plain
/// text and lines.
pub fn overbars(page: &mut Page, font: &Font) {
    for layer in &mut page.layers {
        bars_in(&mut layer.items, font);
    }
}

fn bars_in(items: &mut Vec<Item>, font: &Font) {
    let marked = |i: &Item| match i {
        Item::Text(t) => overbar::has_markup(&t.text),
        Item::Clip { .. } => true,
        _ => false,
    };
    if !items.iter().any(marked) {
        return;
    }
    for item in std::mem::take(items) {
        match item {
            Item::Text(mut t) if overbar::has_markup(&t.text) => {
                let b = overbar::parse(&t.text);
                t.text = b.text;
                let (origin, along, down) = font.placement(&t);
                // Along the run to the start of each character, in millimetres.
                let mut x = vec![0.0];
                for c in t.text.chars() {
                    let last = x.last().copied().unwrap_or(0.0);
                    x.push(last + font.advance(font.glyph(c)) * t.size);
                }
                let lift = (font.cap + 0.12) * t.size;
                let width = t.size * if t.weight == Weight::Bold { 0.085 } else { 0.065 };
                let at = |u: f64| [origin[0] + along[0] * u - down[0] * lift, origin[1] + along[1] * u - down[1] * lift];
                let lines: Vec<Item> = b
                    .bars
                    .iter()
                    .filter_map(|r| Some((*x.get(r.start)?, *x.get(r.end)?)))
                    .map(|(a, b)| Item::Line { points: vec![at(a + width), at(b - width)], closed: false, stroke: Stroke::solid(width, t.color) })
                    .collect();
                items.push(Item::Text(t));
                items.extend(lines);
            }
            Item::Clip { rect, items: mut inner } => {
                bars_in(&mut inner, font);
                items.push(Item::Clip { rect, items: inner });
            }
            other => items.push(other),
        }
    }
}

/// Where an outline goes, in ems with y up.
pub trait PathSink {
    fn move_to(&mut self, p: [f64; 2]);
    fn line_to(&mut self, p: [f64; 2]);
    fn quad_to(&mut self, c: [f64; 2], p: [f64; 2]);
    fn cubic_to(&mut self, c1: [f64; 2], c2: [f64; 2], p: [f64; 2]);
    fn close(&mut self);
}

struct Builder<'b> {
    out: &'b mut dyn PathSink,
    scale: f64,
}

impl Builder<'_> {
    fn p(&self, x: f32, y: f32) -> [f64; 2] {
        [f64::from(x) * self.scale, f64::from(y) * self.scale]
    }
}

impl OutlineBuilder for Builder<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.out.move_to(p);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.out.line_to(p);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (c, p) = (self.p(x1, y1), self.p(x, y));
        self.out.quad_to(c, p);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (c1, c2, p) = (self.p(x1, y1), self.p(x2, y2), self.p(x, y));
        self.out.cubic_to(c1, c2, p);
    }
    fn close(&mut self) {
        self.out.close();
    }
}
