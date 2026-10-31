//! One page as a PNG: drawn with tiny-skia at a chosen resolution, text from the page font's
//! glyph outlines (so it matches the PDF), the resolution saved in the file.

use tiny_skia::{Color, FillRule, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, StrokeDash, Transform};

use super::font::{BOLD_STROKE, Font, PathSink};
use super::page::{Item, Page, Rgb, Stroke, Text, Weight, rounded_rect};

/// The fewest and most dots an inch.
pub const DPI: std::ops::RangeInclusive<f64> = 36.0..=1200.0;
/// The most pixels one page may have (an A3 page at 600 dpi has 70 million).
pub const MAX_PIXELS: u64 = 80_000_000;

/// The size in pixels `page` comes to at `dpi`.
pub fn pixels(page: &Page, dpi: f64) -> (u32, u32) {
    let px = |mm: f64| (mm / 25.4 * dpi).round().clamp(1.0, f64::from(u32::MAX)) as u32;
    (px(page.size[0]), px(page.size[1]))
}

/// The PNG of `page` at `dpi`.
pub fn write(page: &Page, font: &Font, dpi: f64) -> Result<Vec<u8>, String> {
    encode(raster(page, font, dpi)?, dpi)
}

/// `page` drawn at `dpi` as opaque RGBA rows, top first, with its width and height: what a
/// preview shows without encoding a file.
pub fn rgba(page: &Page, font: &Font, dpi: f64) -> Result<([u32; 2], Vec<u8>), String> {
    let pixmap = raster(page, font, dpi)?;
    let size = [pixmap.width(), pixmap.height()];
    // Every page is opaque, so the premultiplied pixels are the plain colours.
    Ok((size, pixmap.take()))
}

fn raster(page: &Page, font: &Font, dpi: f64) -> Result<Pixmap, String> {
    if !DPI.contains(&dpi) {
        return Err(format!("the resolution must be {} to {} dpi", DPI.start(), DPI.end()));
    }
    let (w, h) = pixels(page, dpi);
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(format!("a {w} × {h} pixel page is more than the {MAX_PIXELS} pixels a page may have: choose a lower resolution"));
    }
    let mut pixmap = Pixmap::new(w, h).ok_or("the page has no pixels")?;
    let Rgb(r, g, b) = page.background;
    pixmap.fill(Color::from_rgba8(r, g, b, 255));
    let scale = (dpi / 25.4) as f32;
    let mut canvas = Canvas { pixmap, transform: Transform::from_scale(scale, scale), font };
    for l in &page.layers {
        canvas.items(&l.items, None);
    }
    Ok(canvas.pixmap)
}

fn encode(pixmap: Pixmap, dpi: f64) -> Result<Vec<u8>, String> {
    let (w, h) = (pixmap.width(), pixmap.height());
    // Every page is opaque, so the premultiplied pixels are the plain colours.
    let rgb: Vec<u8> = pixmap.data().as_chunks::<4>().0.iter().flat_map(|[r, g, b, _]| [*r, *g, *b]).collect();
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let per_metre = (dpi / 0.0254).round() as u32;
        enc.set_pixel_dims(Some(png::PixelDimensions { xppu: per_metre, yppu: per_metre, unit: png::Unit::Meter }));
        let mut writer = enc.write_header().map_err(|e| format!("PNG: {e}"))?;
        writer.write_image_data(&rgb).map_err(|e| format!("PNG: {e}"))?;
    }
    Ok(out)
}

struct Canvas<'f> {
    pixmap: Pixmap,
    transform: Transform,
    font: &'f Font<'f>,
}

fn paint(c: Rgb) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.0, c.1, c.2, 255);
    p.anti_alias = true;
    p
}

fn stroke_of(s: &Stroke) -> tiny_skia::Stroke {
    tiny_skia::Stroke {
        width: s.width.max(0.0) as f32,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        dash: s.dash.and_then(|[on, off]| StrokeDash::new(vec![on as f32, off as f32], 0.0)),
        ..Default::default()
    }
}

fn polyline(points: &[[f64; 2]], closed: bool) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let mut it = points.iter();
    let first = it.next()?;
    pb.move_to(first[0] as f32, first[1] as f32);
    for p in it {
        pb.line_to(p[0] as f32, p[1] as f32);
    }
    if closed {
        pb.close();
    }
    pb.finish()
}

/// The intersection of two clip rectangles.
fn meet(a: [[f64; 2]; 2], b: Option<[[f64; 2]; 2]>) -> [[f64; 2]; 2] {
    let a = [[a[0][0].min(a[1][0]), a[0][1].min(a[1][1])], [a[0][0].max(a[1][0]), a[0][1].max(a[1][1])]];
    match b {
        None => a,
        Some(b) => [
            [a[0][0].max(b[0][0]), a[0][1].max(b[0][1])],
            [a[1][0].min(b[1][0]).max(a[0][0].max(b[0][0])), a[1][1].min(b[1][1]).max(a[0][1].max(b[0][1]))],
        ],
    }
}

impl Canvas<'_> {
    fn fill(&mut self, path: &Path, colour: Rgb, rule: FillRule, mask: Option<&Mask>) {
        self.pixmap.fill_path(path, &paint(colour), rule, self.transform, mask);
    }

    fn stroke(&mut self, path: &Path, s: &Stroke, mask: Option<&Mask>) {
        self.pixmap.stroke_path(path, &paint(s.color), &stroke_of(s), self.transform, mask);
    }

    fn shape(&mut self, path: Option<Path>, fill: Option<Rgb>, stroke: Option<&Stroke>, rule: FillRule, mask: Option<&Mask>) {
        let Some(path) = path else { return };
        if let Some(f) = fill {
            self.fill(&path, f, rule, mask);
        }
        if let Some(s) = stroke {
            self.stroke(&path, s, mask);
        }
    }

    fn items(&mut self, list: &[Item], clip: Option<[[f64; 2]; 2]>) {
        let mask = clip.and_then(|r| self.mask(r));
        let m = mask.as_ref();
        for i in list {
            match i {
                Item::Line { points, closed, stroke } => {
                    if points.len() >= 2 {
                        self.shape(polyline(points, *closed), None, Some(stroke), FillRule::Winding, m);
                    }
                }
                Item::Area { rings, fill, stroke } => {
                    let mut pb = PathBuilder::new();
                    for r in rings.iter().filter(|r| r.len() >= 3) {
                        let mut it = r.iter();
                        if let Some(first) = it.next() {
                            pb.move_to(first[0] as f32, first[1] as f32);
                            for p in it {
                                pb.line_to(p[0] as f32, p[1] as f32);
                            }
                            pb.close();
                        }
                    }
                    self.shape(pb.finish(), Some(*fill), stroke.as_ref(), FillRule::EvenOdd, m);
                }
                Item::Rect { rect, radius, fill, stroke } => {
                    self.shape(polyline(&rounded_rect(*rect, *radius), true), *fill, stroke.as_ref(), FillRule::Winding, m);
                }
                Item::Circle { center, radius, fill, stroke } => {
                    let path = PathBuilder::from_circle(center[0] as f32, center[1] as f32, radius.max(0.0) as f32);
                    self.shape(path, *fill, stroke.as_ref(), FillRule::Winding, m);
                }
                Item::Text(t) => self.text(t, m),
                Item::Clip { rect, items } => self.items(items, Some(meet(*rect, clip))),
            }
        }
    }

    fn mask(&self, [a, b]: [[f64; 2]; 2]) -> Option<Mask> {
        let mut mask = Mask::new(self.pixmap.width(), self.pixmap.height())?;
        let rect = tiny_skia::Rect::from_ltrb(a[0] as f32, a[1] as f32, b[0] as f32, b[1] as f32)?;
        let path = PathBuilder::from_rect(rect);
        mask.fill_path(&path, FillRule::Winding, true, self.transform);
        Some(mask)
    }

    fn text(&mut self, t: &Text, mask: Option<&Mask>) {
        if t.text.is_empty() || !(t.size > 0.0) {
            return;
        }
        let (o, along, down) = self.font.placement(t);
        let mut sink = Sink { pb: PathBuilder::new(), origin: o, along, down, size: t.size, x: 0.0 };
        for (g, x) in self.font.layout(&t.text) {
            sink.x = x;
            self.font.outline(g, &mut sink);
        }
        let Some(path) = sink.pb.finish() else { return };
        self.fill(&path, t.color, FillRule::Winding, mask);
        if t.weight == Weight::Bold {
            self.stroke(&path, &Stroke::solid(t.size * BOLD_STROKE, t.color), mask);
        }
    }
}

/// Glyph outlines (ems, y up) placed on the page.
struct Sink {
    pb: PathBuilder,
    origin: [f64; 2],
    along: [f64; 2],
    down: [f64; 2],
    size: f64,
    /// The glyph's start along the run, in ems.
    x: f64,
}

impl Sink {
    fn at(&self, [u, v]: [f64; 2]) -> (f32, f32) {
        let (u, v) = ((u + self.x) * self.size, v * self.size);
        let x = self.origin[0] + self.along[0] * u - self.down[0] * v;
        let y = self.origin[1] + self.along[1] * u - self.down[1] * v;
        (x as f32, y as f32)
    }
}

impl PathSink for Sink {
    fn move_to(&mut self, p: [f64; 2]) {
        let (x, y) = self.at(p);
        self.pb.move_to(x, y);
    }
    fn line_to(&mut self, p: [f64; 2]) {
        let (x, y) = self.at(p);
        self.pb.line_to(x, y);
    }
    fn quad_to(&mut self, c: [f64; 2], p: [f64; 2]) {
        let ((cx, cy), (x, y)) = (self.at(c), self.at(p));
        self.pb.quad_to(cx, cy, x, y);
    }
    fn cubic_to(&mut self, c1: [f64; 2], c2: [f64; 2], p: [f64; 2]) {
        let ((ax, ay), (bx, by), (x, y)) = (self.at(c1), self.at(c2), self.at(p));
        self.pb.cubic_to(ax, ay, bx, by, x, y);
    }
    fn close(&mut self) {
        self.pb.close();
    }
}
