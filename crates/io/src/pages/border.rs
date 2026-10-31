//! Every page's border: a frame with zone references (columns numbered along the top and bottom,
//! rows lettered down the sides) and a title block in the bottom right corner, and the drawing
//! area left inside them.

use super::font::Font;
use super::page::{Align, Baseline, Item, Page, Stroke, Text};
use super::{Orientation, Palette, Paper, TitleInfo};

/// The title block's text sizes, in millimetres: the design title, the page title, a field's
/// caption and its value.
const TITLE: f64 = 4.4;
const PAGE_TITLE: f64 = 3.0;
const CAPTION: f64 = 1.7;
const VALUE: f64 = 2.5;
/// Zone letters and numbers.
const ZONE: f64 = 2.2;

/// A page's border and title block geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sheet {
    pub paper: Paper,
    pub orientation: Orientation,
    pub size: [f64; 2],
    /// The outer frame line and the inner one (inside the zone strip).
    pub outer: [[f64; 2]; 2],
    pub inner: [[f64; 2]; 2],
    /// Where drawings go: inside the inner frame, a little in.
    pub area: [[f64; 2]; 2],
    pub title_block: [[f64; 2]; 2],
    pub cols: usize,
    pub rows: usize,
}

/// What the title block says on one page.
pub struct Block<'a> {
    pub info: &'a TitleInfo,
    pub page_title: &'a str,
    pub page: usize,
    pub pages: usize,
    pub scale: Option<&'a str>,
    pub file: &'a str,
}

impl Sheet {
    pub fn new(paper: Paper, orientation: Orientation) -> Sheet {
        let [pw, ph] = paper.size();
        let size = match orientation {
            Orientation::Landscape => [pw.max(ph), pw.min(ph)],
            Orientation::Portrait => [pw.min(ph), pw.max(ph)],
        };
        let small = matches!(paper, Paper::A4 | Paper::Letter);
        let margin = if small { 8.0 } else { 10.0 };
        let strip = if small { 4.5 } else { 5.0 };
        let outer = [[margin, margin], [size[0] - margin, size[1] - margin]];
        let inner = [[margin + strip, margin + strip], [size[0] - margin - strip, size[1] - margin - strip]];
        let area = [[inner[0][0] + 4.0, inner[0][1] + 4.0], [inner[1][0] - 4.0, inner[1][1] - 4.0]];
        let (tw, th) = (if small { 112.0 } else { 130.0 }, 34.0);
        let title_block = [[inner[1][0] - tw, inner[1][1] - th], inner[1]];
        let (long, short) = if small { (6, 4) } else { (8, 6) };
        let (cols, rows) = match orientation {
            Orientation::Landscape => (long, short),
            Orientation::Portrait => (short, long),
        };
        Sheet { paper, orientation, size, outer, inner, area, title_block, cols, rows }
    }

    /// The zone a page point lies in: its row letter and column number, such as `B4`.
    pub fn zone(&self, [x, y]: [f64; 2]) -> String {
        let [lo, hi] = self.inner;
        let cw = (hi[0] - lo[0]) / self.cols as f64;
        let rh = (hi[1] - lo[1]) / self.rows as f64;
        let col = ((x - lo[0]) / cw).floor().clamp(0.0, (self.cols - 1) as f64) as usize;
        let row = ((y - lo[1]) / rh).floor().clamp(0.0, (self.rows - 1) as f64) as usize;
        format!("{}{}", row_letter(row), col + 1)
    }

    /// The largest scale a drawing `size` mm big fits `area` (the drawing area or a part of it) at
    /// (at most `most`), clear of the title block, and the page box it then fills (centred in the
    /// room it gets), rounded down to a nice scale when `nice` is set.
    pub fn fit_in(&self, area: [[f64; 2]; 2], size: [f64; 2], most: f64, nice: bool) -> (f64, [[f64; 2]; 2]) {
        self.fit_clear(area, size, most, nice, [0.0, 0.0])
    }

    /// As [`Sheet::fit_in`], keeping `clear` millimetres right of and below the drawing (where
    /// its dimensions go) clear of the title block too.
    pub fn fit_clear(&self, [a0, a1]: [[f64; 2]; 2], size: [f64; 2], most: f64, nice: bool, clear: [f64; 2]) -> (f64, [[f64; 2]; 2]) {
        let tb = self.title_block;
        let rooms = [[a0, a1], [a0, [a1[0], a1[1].min(tb[0][1] - 3.0 - clear[1])]], [a0, [a1[0].min(tb[0][0] - 3.0 - clear[0]), a1[1]]]];
        let mut best: Option<(f64, [[f64; 2]; 2])> = None;
        for (k, [lo, hi]) in rooms.into_iter().enumerate() {
            let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
            if !(w > 1.0 && h > 1.0 && size[0] > 0.0 && size[1] > 0.0) {
                continue;
            }
            let mut s = (w / size[0]).min(h / size[1]).min(most);
            if nice {
                s = nice_scale(s);
            }
            let (dw, dh) = (size[0] * s, size[1] * s);
            let at = [lo[0] + (w - dw) / 2.0, lo[1] + (h - dh) / 2.0];
            let placed = [at, [at[0] + dw, at[1] + dh]];
            // The whole area only when the centred drawing misses the title block.
            if k == 0 && overlaps([placed[0], [placed[1][0] + clear[0], placed[1][1] + clear[1]]], tb) {
                continue;
            }
            if best.is_none_or(|(b, _)| s > b * 1.0001) {
                best = Some((s, placed));
            }
        }
        best.unwrap_or((1.0, [a0, [a0[0] + size[0], a0[1] + size[1]]]))
    }

    /// The frame, zones and title block, drawn on `page`.
    pub fn draw(&self, page: &mut Page, pal: &Palette, font: &Font, b: &Block) {
        let mut out = Vec::new();
        let thin = Stroke::solid(0.2, pal.zone);
        out.push(Item::Rect { rect: self.outer, radius: 0.0, fill: None, stroke: Some(Stroke::solid(0.3, pal.zone)) });
        out.push(Item::Rect { rect: self.inner, radius: 0.0, fill: None, stroke: Some(Stroke::solid(0.35, pal.ink)) });
        let [o0, o1] = self.outer;
        let [i0, i1] = self.inner;
        let cw = (i1[0] - i0[0]) / self.cols as f64;
        let rh = (i1[1] - i0[1]) / self.rows as f64;
        for c in 0..self.cols {
            let x = i0[0] + cw * c as f64;
            if c > 0 {
                out.push(Item::Line { points: vec![[x, o0[1]], [x, i0[1]]], closed: false, stroke: thin });
                out.push(Item::Line { points: vec![[x, i1[1]], [x, o1[1]]], closed: false, stroke: thin });
            }
            let label = (c + 1).to_string();
            let mx = x + cw / 2.0;
            for y in [(o0[1] + i0[1]) / 2.0, (i1[1] + o1[1]) / 2.0] {
                out.push(Item::Text(Text::new([mx, y], label.clone(), ZONE, pal.zone).align(Align::Middle, Baseline::Middle)));
            }
        }
        for r in 0..self.rows {
            let y = i0[1] + rh * r as f64;
            if r > 0 {
                out.push(Item::Line { points: vec![[o0[0], y], [i0[0], y]], closed: false, stroke: thin });
                out.push(Item::Line { points: vec![[i1[0], y], [o1[0], y]], closed: false, stroke: thin });
            }
            let label = row_letter(r).to_string();
            let my = y + rh / 2.0;
            for x in [(o0[0] + i0[0]) / 2.0, (i1[0] + o1[0]) / 2.0] {
                out.push(Item::Text(Text::new([x, my], label.clone(), ZONE, pal.zone).align(Align::Middle, Baseline::Middle)));
            }
        }
        page.add("border", out);
        page.add("title-block", self.title_block(pal, font, b));
    }

    fn title_block(&self, pal: &Palette, font: &Font, b: &Block) -> Vec<Item> {
        let [[x0, y0], [x1, y1]] = self.title_block;
        let rule = Stroke::solid(0.2, pal.zone);
        let mut out =
            vec![Item::Rect { rect: self.title_block, radius: 0.0, fill: Some(pal.background), stroke: Some(Stroke::solid(0.35, pal.ink)) }];
        let w = x1 - x0;
        let pad = 2.2;
        // Rows from the top: titles (11 + 7), then two rows of fields (8 each).
        let r1 = y0 + 18.0;
        let r2 = r1 + 8.0;
        for y in [r1, r2] {
            out.push(Item::Line { points: vec![[x0, y], [x1, y]], closed: false, stroke: rule });
        }
        let title = if b.info.title.is_empty() { b.file } else { b.info.title.as_str() };
        let tool = "SolidCraft";
        let tool_w = font.width(tool, CAPTION + 0.4);
        out.push(Item::Text(
            Text::new([x0 + pad, y0 + 2.4], font.fit(title, TITLE, w - 2.0 * pad - tool_w - 3.0), TITLE, pal.ink)
                .align(Align::Start, Baseline::Top)
                .bold(),
        ));
        out.push(Item::Text(Text::new([x1 - pad, y0 + 2.6], tool, CAPTION + 0.4, pal.dim).align(Align::End, Baseline::Top).bold()));
        let page_title_w = if b.info.notes.is_empty() { w - 2.0 * pad } else { w * 0.55 - pad };
        out.push(Item::Text(
            Text::new([x0 + pad, y0 + 15.6], font.fit(b.page_title, PAGE_TITLE, page_title_w), PAGE_TITLE, pal.ink)
                .align(Align::Start, Baseline::Baseline),
        ));
        if !b.info.notes.is_empty() {
            out.push(Item::Text(
                Text::new([x1 - pad, y0 + 15.6], font.fit(&b.info.notes, CAPTION + 0.2, w * 0.45 - 2.0 * pad), CAPTION + 0.2, pal.dim)
                    .align(Align::End, Baseline::Baseline),
            ));
        }
        let cell = |out: &mut Vec<Item>, cx0: f64, cx1: f64, top: f64, caption: &str, value: &str| {
            out.push(Item::Text(Text::new([cx0 + pad, top + 1.6], caption, CAPTION, pal.dim).align(Align::Start, Baseline::Top)));
            let v = if value.is_empty() { "–" } else { value };
            out.push(Item::Text(Text::new([cx0 + pad, top + 6.6], font.fit(v, VALUE, cx1 - cx0 - 2.0 * pad), VALUE, pal.ink)));
        };
        let page_text = format!("{} of {}", b.page, b.pages);
        let scale = b.scale.unwrap_or("–");
        let cols = [x0, x0 + w * 0.22, x0 + w * 0.40, x0 + w * 0.70, x1];
        for x in cols.iter().take(4).skip(1) {
            out.push(Item::Line { points: vec![[*x, r1], [*x, r2]], closed: false, stroke: rule });
        }
        let fields = [("PAGE", page_text.as_str()), ("REVISION", b.info.revision.as_str()), ("DATE", b.info.date.as_str()), ("SCALE", scale)];
        for (k, (caption, value)) in fields.iter().enumerate() {
            if let (Some(a), Some(z)) = (cols.get(k), cols.get(k + 1)) {
                cell(&mut out, *a, *z, r1, caption, value);
            }
        }
        let by = match (b.info.author.is_empty(), b.info.company.is_empty()) {
            (false, false) => format!("{} · {}", b.info.author, b.info.company),
            (false, true) => b.info.author.clone(),
            (true, false) => b.info.company.clone(),
            (true, true) => String::new(),
        };
        let paper = format!("{} {}", self.paper.name(), self.orientation.name());
        let cols = [x0, x0 + w * 0.40, x0 + w * 0.62, x1];
        for x in cols.iter().take(3).skip(1) {
            out.push(Item::Line { points: vec![[*x, r2], [*x, y1]], closed: false, stroke: rule });
        }
        let fields = [("AUTHOR · COMPANY", by.as_str()), ("PAPER", paper.as_str()), ("FILE", b.file)];
        for (k, (caption, value)) in fields.iter().enumerate() {
            if let (Some(a), Some(z)) = (cols.get(k), cols.get(k + 1)) {
                cell(&mut out, *a, *z, r2, caption, value);
            }
        }
        out
    }
}

/// `A` for row 0, …, `Z`, then `AA`.
pub fn row_letter(row: usize) -> String {
    let letters = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    let n = letters.len();
    let one = |k: usize| letters.get(k % n).map_or('?', |b| char::from(*b));
    if row < n { one(row).to_string() } else { format!("{}{}", one(row / n - 1), one(row)) }
}

/// Whether two boxes overlap (touching is not).
pub(super) fn overlaps([a0, a1]: [[f64; 2]; 2], [b0, b1]: [[f64; 2]; 2]) -> bool {
    a0[0] < b1[0] && b0[0] < a1[0] && a0[1] < b1[1] && b0[1] < a1[1]
}

/// The largest nice scale at most `s`: an enlargement 1, 1.25, 1.5, 1.75, 2, 2.5, 3, 4, 5, 6 or 8
/// times a power of ten (`2.5:1`), a reduction one over 1.25, 1.5, 2, 2.5, 3, 4, 5, 6 or 8 times a
/// power of ten (`1:2.5`), so the title block always states a clean ratio.
pub fn nice_scale(s: f64) -> f64 {
    if !(s.is_finite() && s > 0.0) {
        return 1.0;
    }
    if s >= 1.0 {
        let decade = 10f64.powf(s.log10().floor());
        let steps = [10.0, 8.0, 6.0, 5.0, 4.0, 3.0, 2.5, 2.0, 1.75, 1.5, 1.25, 1.0];
        return steps.iter().map(|k| k * decade).find(|v| *v <= s * (1.0 + 1e-9)).unwrap_or(decade);
    }
    // The smallest nice reduction 1:r with r at least 1 / s.
    let r = 1.0 / s;
    let decade = 10f64.powf(r.log10().floor());
    let steps = [1.0, 1.25, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0];
    let r = steps.iter().map(|k| k * decade).find(|v| *v >= r * (1.0 - 1e-9)).unwrap_or(10.0 * decade);
    1.0 / r
}

/// A scale as a title block states it: `2:1`, `1:1`, `1:2.5`.
pub fn scale_text(s: f64) -> String {
    let trim = |v: f64| {
        let t = format!("{v:.2}");
        t.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    if s >= 1.0 { format!("{}:1", trim(s)) } else { format!("1:{}", trim(1.0 / s)) }
}
