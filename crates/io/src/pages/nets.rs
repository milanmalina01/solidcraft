//! The net index at the end of the book: every named net with the pages it shows up on (a page
//! and zone for each frame it is in, each a link there), in a table of one column or two over as
//! many pages as it takes. A section page's list of cross-references cut short ("+6 more") links
//! to its net's row here, and ground nets, cross-referenced on no section page, are listed here
//! in full.

use super::Palette;
use super::border::Sheet;
use super::font::Font;
use super::page::{Align, Baseline, Item, Link, Page, Stroke, Text};
use super::sections::Target;

const TEXT: f64 = 2.3;
const HEAD: f64 = 2.3;
const LINE: f64 = 3.5;
const PAD: f64 = 1.6;
const HEADER_ROW: f64 = 7.0;
/// The space the page's heading takes over the table.
const HEADING: f64 = 9.0;
/// The net names' share of a column's width, and the room between two columns.
const NAME_SHARE: f64 = 0.3;
const GAP: f64 = 8.0;
/// The narrowest a column is: a page with less room for two has one.
const MIN_COLUMN: f64 = 160.0;
/// The most lines one net's pages take; past them the last line ends in an ellipsis.
const MAX_LINES: usize = 6;

/// One net's row.
pub(super) struct Row {
    /// Its index in the connectivity's nets.
    pub net: usize,
    pub name: String,
    /// A rail or ground net.
    pub power: bool,
    pub pages: Vec<Target>,
}

/// A row on a page: which, in which column, and how far down the column's rows it starts.
#[derive(Clone, Copy, Debug)]
pub(super) struct Placed {
    pub row: usize,
    pub column: usize,
    pub y: f64,
}

/// One line of a row's pages: its text, and each page reference on it (its index in the row's
/// list, where it starts and ends along the line).
#[derive(Default)]
struct Line {
    text: String,
    spans: Vec<(usize, f64, f64)>,
}

/// The table's columns on a page: the left of each and its width.
fn columns(geometry: &Sheet) -> Vec<(f64, f64)> {
    let [a0, a1] = geometry.area;
    let w = a1[0] - a0[0];
    let n: usize = if w >= 2.0 * MIN_COLUMN + GAP { 2 } else { 1 };
    let cw = (w - GAP * n.saturating_sub(1) as f64) / n as f64;
    (0..n).map(|k| (a0[0] + k as f64 * (cw + GAP), cw)).collect()
}

/// Where a column's rows start and end on the page (a column over the title block ends above it).
fn rows_span(geometry: &Sheet, (x, w): (f64, f64)) -> (f64, f64) {
    let [a0, a1] = geometry.area;
    let tb = geometry.title_block;
    let bottom = if x + w > tb[0][0] { tb[0][1] - 4.0 } else { a1[1] };
    (a0[1] + HEADING + HEADER_ROW, bottom)
}

/// A row's pages broken into lines no wider than `width`, at most [`MAX_LINES`].
fn lines(row: &Row, width: f64, font: &Font) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let mut line = Line::default();
    for (k, t) in row.pages.iter().enumerate() {
        if !line.text.is_empty() && font.width(&format!("{}, {}", line.text, t.text), TEXT) > width {
            line.text.push(',');
            out.push(std::mem::take(&mut line));
        }
        if !line.text.is_empty() {
            line.text.push_str(", ");
        }
        let u0 = font.width(&line.text, TEXT);
        line.text.push_str(&t.text);
        line.spans.push((k, u0, font.width(&line.text, TEXT)));
    }
    if !line.text.is_empty() {
        out.push(line);
    }
    if out.len() > MAX_LINES {
        out.truncate(MAX_LINES);
        if let Some(last) = out.last_mut() {
            last.text.push_str(" …");
        }
    }
    out
}

/// Where a column's net names and pages go: the names' width, and the pages' left and width.
fn split((x, w): (f64, f64)) -> (f64, f64, f64) {
    let name = w * NAME_SHARE;
    (name, x + name + PAD, w - name - 2.0 * PAD)
}

fn row_height(lines: usize) -> f64 {
    lines.max(1) as f64 * LINE + 2.0 * PAD - 0.4
}

/// The rows each page holds: down the first column, then the next, then the next page.
pub(super) fn paginate(rows: &[Row], geometry: &Sheet, font: &Font) -> Vec<Vec<Placed>> {
    let cols = columns(geometry);
    let mut pages: Vec<Vec<Placed>> = Vec::new();
    let (mut column, mut y) = (0usize, 0.0f64);
    for (i, r) in rows.iter().enumerate() {
        let Some(&col) = cols.get(column).or(cols.first()) else { break };
        let (_, _, wide) = split(col);
        let h = row_height(lines(r, wide, font).len());
        if pages.is_empty() {
            pages.push(Vec::new());
        }
        let (top, bottom) = rows_span(geometry, col);
        if y > 0.0 && top + y + h > bottom {
            column += 1;
            y = 0.0;
            if column >= cols.len() {
                column = 0;
                pages.push(Vec::new());
            }
        }
        if let Some(p) = pages.last_mut() {
            p.push(Placed { row: i, column, y });
        }
        y += h;
    }
    pages
}

/// The page point of a row's top left, for a link to it.
pub(super) fn spot(geometry: &Sheet, p: &Placed) -> [f64; 2] {
    let cols = columns(geometry);
    let col = cols.get(p.column).or(cols.first()).copied().unwrap_or((geometry.area[0][0], 0.0));
    let (top, _) = rows_span(geometry, col);
    [col.0, top + p.y]
}

/// One page of the index.
pub(super) fn draw(page: &mut Page, geometry: &Sheet, pal: &Palette, font: &Font, rows: &[Row], chunk: &[Placed], more: bool) {
    let cols = columns(geometry);
    let [a0, a1] = geometry.area;
    let mut items = Vec::new();
    items.push(Item::Text(Text::new([a0[0], a0[1] + 1.0], "Net index", 3.6, pal.ink).align(Align::Start, Baseline::Top).bold()));
    let summary = format!(
        "{} named {} and the pages they are on (page·zone, a link to each){}",
        rows.len(),
        if rows.len() == 1 { "net" } else { "nets" },
        if more { " · continued on the next page" } else { "" }
    );
    items.push(Item::Text(Text::new([a1[0], a0[1] + 1.6], summary, TEXT, pal.dim).align(Align::End, Baseline::Top)));
    let used = chunk.iter().map(|p| p.column).max().map_or(0, |c| c + 1);
    for &col in cols.iter().take(used) {
        let (x, w) = col;
        let (_, pages_x, _) = split(col);
        let y = a0[1] + HEADING;
        items.push(Item::Rect { rect: [[x, y], [x + w, y + HEADER_ROW]], radius: 0.0, fill: Some(pal.zone.mix(pal.background, 0.8)), stroke: None });
        for (at, name) in [(x + PAD, "Net"), (pages_x, "Pages · zones")] {
            items.push(Item::Text(Text::new([at, y + HEADER_ROW / 2.0], name, HEAD, pal.ink).align(Align::Start, Baseline::Middle).bold()));
        }
        items.push(Item::Line { points: vec![[x, y + HEADER_ROW], [x + w, y + HEADER_ROW]], closed: false, stroke: Stroke::solid(0.3, pal.ink) });
    }
    for p in chunk {
        let (Some(r), Some(&col)) = (rows.get(p.row), cols.get(p.column)) else { continue };
        let (x, w) = col;
        let (name_w, pages_x, wide) = split(col);
        let (top, _) = rows_span(geometry, col);
        let y = top + p.y;
        let lines = lines(r, wide, font);
        let h = row_height(lines.len());
        if p.row % 2 == 1 {
            items.push(Item::Rect { rect: [[x, y], [x + w, y + h]], radius: 0.0, fill: Some(pal.body), stroke: None });
        }
        let colour = if r.power { pal.power } else { pal.ink };
        let name = font.fit(&r.name, TEXT, name_w - 2.0 * PAD);
        items.push(Item::Text(Text::new([x + PAD, y + PAD + 0.2], name, TEXT, colour).align(Align::Start, Baseline::Top).bold()));
        for (k, line) in lines.iter().enumerate() {
            let ly = y + PAD + 0.2 + k as f64 * LINE;
            items.push(Item::Text(Text::new([pages_x, ly], line.text.clone(), TEXT, pal.link).align(Align::Start, Baseline::Top)));
            for (i, u0, u1) in &line.spans {
                let Some(t) = r.pages.get(*i) else { continue };
                let Some(target) = t.page else { continue };
                page.links.push(Link { rect: [[pages_x + u0, ly - 0.4], [pages_x + u1, ly + LINE - 0.4]], page: target, at: t.at });
            }
        }
        items.push(Item::Line { points: vec![[x, y + h], [x + w, y + h]], closed: false, stroke: Stroke::solid(0.15, pal.zone) });
    }
    page.add("nets", items);
}
