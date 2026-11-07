//! The cover: the design's title large with its note under it, its revision, date, author and
//! company, a picture of the board's top side when it has a board, the contents (each page's
//! number and title, a link to it) and the revision history, newest first (one row from the
//! current revision, date and author while the history is empty). The title and the picture sit
//! side by side on a landscape page and one over the other on a portrait one, the contents and the
//! revisions under them, with plenty of room round each.

use solidcraft_doc::ecad::Electronics;
use solidcraft_doc::ecad::info::Revision;

use super::board::{self, Copper};
use super::border::Sheet;
use super::font::Font;
use super::page::{Align, Baseline, Item, Link, Page, Stroke, Text};
use super::sections::Plan;
use super::{Orientation, PageKind, Palette, TitleInfo};

/// Text sizes: a field's caption and value, a section's heading, a contents or revisions row, the
/// picture's caption.
const CAPTION: f64 = 2.0;
const VALUE: f64 = 3.4;
const HEADING: f64 = 3.4;
const ROW: f64 = 2.6;
const NOTE: f64 = 2.2;
/// A contents row's and a revision row's pitch, and the gap between groups of contents rows.
const ROW_PITCH: f64 = 5.0;
const GROUP_GAP: f64 = 2.0;
/// The room a section heading and its rule take over its rows.
const HEAD_ROOM: f64 = 9.0;
/// Contents longer than this many rows go in two columns, where each can be this wide.
const LONG: usize = 12;
const MIN_COLUMN: f64 = 60.0;
/// The most lines the note under the title takes.
const NOTE_LINES: usize = 3;

/// What the cover shows of the design and the book.
pub(super) struct Cover<'a> {
    pub info: &'a TitleInfo,
    /// The whole book, for the contents (links to pages not exported are dropped later).
    pub plan: &'a [Plan],
    pub copper: &'a Copper,
}

pub(super) fn draw(ecad: &Electronics, page: &mut Page, geometry: &Sheet, pal: &Palette, font: &Font, cover: &Cover) {
    let [a0, a1] = geometry.area;
    let (w, h) = (a1[0] - a0[0], a1[1] - a0[1]);
    let landscape = geometry.orientation == Orientation::Landscape;
    let pictured = board::has_picture(ecad);
    // The title and the picture, the picture the larger part of the page's top on a landscape page
    // where the contents under it still fit.
    let hero = if landscape { (h * 0.54).min(h - 4.0 - contents_height(cover.plan)).max(h * 0.44) } else { h * 0.48 };
    let (text_box, picture_box) = if landscape {
        let split = if pictured { a0[0] + w * 0.45 } else { a1[0] };
        ([a0, [split, a0[1] + hero]], [[a0[0] + w * 0.49, a0[1] + 6.0], [a1[0], a0[1] + hero - 8.0]])
    } else {
        let split = a0[1] + h * 0.2;
        ([a0, [a1[0], split]], [[a0[0], split + 4.0], [a1[0], a0[1] + hero - 8.0]])
    };
    title(page, text_box, pal, font, cover.info);
    if pictured && let Some((placed, size)) = board::picture(ecad, cover.copper, page, picture_box, pal, font) {
        // What the picture shows, then the board's facts: the parts are every side's (as the parts
        // list counts them), not the picture's.
        let b = &ecad.board;
        let caption = format!(
            "Top side  ·  board {:.1} × {:.1} mm, {} copper layers, {} parts fitted",
            size[0],
            size[1],
            b.copper_layers,
            ecad.schematic.parts.iter().filter(|p| !p.dnp).count()
        );
        let at = [(placed[0][0] + placed[1][0]) / 2.0, placed[1][1] + 4.0];
        page.add("caption", [Item::Text(Text::new(at, caption, NOTE, pal.dim).align(Align::Middle, Baseline::Top))]);
    }
    // Under them: the contents on the left, the revisions on the right.
    let top = a0[1] + hero + 4.0;
    let mid = a0[0] + w * 0.5;
    let tb = geometry.title_block;
    let clear = |x1: f64, bottom: f64| if x1 > tb[0][0] - 3.0 { bottom.min(tb[0][1] - 5.0) } else { bottom };
    let contents_box = [[a0[0], top], [mid - 6.0, clear(mid - 6.0, a1[1])]];
    contents(page, contents_box, pal, font, cover.plan);
    let revisions_box = [[mid + 4.0, top], [a1[0], clear(a1[0], a1[1])]];
    revisions(page, revisions_box, pal, font, cover.info);
}

/// The title large, an accent over it, the note under it, then the revision, date, author and
/// company, the whole group in the middle of its box from top to bottom.
fn title(page: &mut Page, [t0, t1]: [[f64; 2]; 2], pal: &Palette, font: &Font, info: &TitleInfo) {
    let (w, h) = (t1[0] - t0[0], t1[1] - t0[1]);
    // As large as the room allows, down to a little over half; a title longer still is cut.
    let mut size = (h * 0.13).clamp(8.0, 15.0);
    let least = size * 0.55;
    while size > least && font.width(&info.title, size) > w {
        size *= 0.92;
    }
    let note = (size * 0.36).clamp(3.2, 5.0);
    // At most three lines; a longer note ends the last in an ellipsis.
    let mut notes: Vec<String> = if info.notes.is_empty() { Vec::new() } else { font.wrap(&info.notes, note, w) };
    if notes.len() > NOTE_LINES {
        let rest = notes.split_off(NOTE_LINES - 1).join(" ");
        notes.push(font.fit(&rest, note, w));
    }
    let across = if w >= 150.0 { 4 } else { 2 };
    let gap = (h * 0.08).clamp(5.0, 12.0);
    let title_h = size * (font.ascent + font.descent) + 1.0;
    let fields_h = 10.0 + if across == 4 { 0.0 } else { 11.0 };
    let tall = 1.3 + 7.0 + title_h + notes.len() as f64 * note * 1.35 + gap + fields_h;
    let mut y = t0[1] + ((h - tall) / 2.0).max(4.0);
    let mut items = Vec::new();
    items.push(Item::Rect { rect: [[t0[0], y], [t0[0] + 18.0, y + 1.3]], radius: 0.65, fill: Some(pal.link), stroke: None });
    y += 1.3 + 7.0;
    let text = font.fit(&info.title, size, w);
    items.push(Item::Text(Text::new([t0[0], y], text, size, pal.ink).align(Align::Start, Baseline::Top).bold()));
    y += title_h;
    for line in notes {
        items.push(Item::Text(Text::new([t0[0], y], line, note, pal.dim).align(Align::Start, Baseline::Top)));
        y += note * 1.35;
    }
    y += gap;
    // The fields: four across, or two by two in a narrow column.
    let fields =
        [("REVISION", info.revision.as_str()), ("DATE", info.date.as_str()), ("AUTHOR", info.author.as_str()), ("COMPANY", info.company.as_str())];
    let cell = w / across as f64;
    for (k, (caption, value)) in fields.iter().enumerate() {
        let (col, row) = (k % across, k / across);
        let (x, top) = (t0[0] + cell * col as f64, y + row as f64 * 11.0);
        items.push(Item::Line { points: vec![[x, top], [x + cell - 5.0, top]], closed: false, stroke: Stroke::solid(0.2, pal.zone) });
        items.push(Item::Text(Text::new([x, top + 1.8], *caption, CAPTION, pal.dim).align(Align::Start, Baseline::Top)));
        let v = if value.is_empty() { "–" } else { value };
        items.push(Item::Text(Text::new([x, top + 5.0], font.fit(v, VALUE, cell - 5.0), VALUE, pal.ink).align(Align::Start, Baseline::Top)));
    }
    page.add("title", items);
}

/// A section's heading and the rule under it.
fn heading(items: &mut Vec<Item>, [x0, y]: [f64; 2], x1: f64, text: &str, pal: &Palette) {
    items.push(Item::Text(Text::new([x0, y], text, HEADING, pal.ink).align(Align::Start, Baseline::Top).bold()));
    items.push(Item::Line { points: vec![[x0, y + 6.0], [x1, y + 6.0]], closed: false, stroke: Stroke::solid(0.3, pal.ink) });
}

/// Which group a page's row is in: the schematic's pages, the block diagram, the parts list, the
/// board's pages, the net index; a small gap between groups.
fn group(kind: PageKind) -> u8 {
    match kind {
        PageKind::Cover | PageKind::Sections => 0,
        PageKind::BlockDiagram => 1,
        PageKind::Parts => 2,
        PageKind::NetIndex => 4,
        _ => 3,
    }
}

/// About how tall the contents are in two columns, the heading included.
fn contents_height(plan: &[Plan]) -> f64 {
    let groups: Vec<u8> = plan.iter().filter(|p| p.kind != PageKind::Cover).map(|p| group(p.kind)).collect();
    let gaps = groups.windows(2).filter(|w| w.first() != w.last()).count();
    let one = groups.len() as f64 * ROW_PITCH + gaps as f64 * GROUP_GAP;
    HEAD_ROOM + one / 2.0 + 2.0 * ROW_PITCH
}

/// The contents: a row per page but this one (its number, its title, a link to it), in one
/// column or two; past the room, the last row says how many more.
fn contents(page: &mut Page, [b0, b1]: [[f64; 2]; 2], pal: &Palette, font: &Font, plan: &[Plan]) {
    let rows: Vec<(usize, &Plan)> = plan.iter().enumerate().filter(|(_, p)| p.kind != PageKind::Cover).collect();
    if rows.is_empty() || b1[0] - b0[0] < 20.0 {
        return;
    }
    let mut items = Vec::new();
    heading(&mut items, b0, b1[0], "Contents", pal);
    let top = b0[1] + HEAD_ROOM;
    let room = (b1[1] - top).max(0.0);
    // The rows' places, column by column `height` tall at most, the gaps between groups included.
    let places = |height: f64| {
        let mut at: Vec<(usize, f64)> = Vec::with_capacity(rows.len());
        let (mut col, mut y, mut last) = (0usize, 0.0, None);
        for (_, p) in &rows {
            let g = group(p.kind);
            if last.is_some_and(|l| l != g) && y > 0.0 {
                y += GROUP_GAP;
            }
            last = Some(g);
            if y + ROW_PITCH > height && y > 0.0 {
                col += 1;
                y = 0.0;
            }
            at.push((col, y));
            y += ROW_PITCH;
        }
        at
    };
    // A long list in two even columns where they fit side by side; one column else.
    let one: f64 = places(f64::INFINITY).last().map_or(0.0, |(_, y)| y + ROW_PITCH);
    let wide = b1[0] - b0[0] >= 2.0 * MIN_COLUMN + 6.0;
    let height = if wide && rows.len() > LONG { (one / 2.0 + ROW_PITCH).min(room) } else { room };
    let at = places(height);
    let columns = if at.iter().any(|(c, _)| *c > 0) { 2 } else { 1 };
    let width = (b1[0] - b0[0] - if columns > 1 { 6.0 } else { 0.0 }) / columns as f64;
    // Rows past the second column: the last that fits says how many are left.
    let fits = at.iter().filter(|(c, y)| *c < 2 && y + ROW_PITCH <= room + 1e-9).count();
    let shown = if fits < rows.len() { fits.saturating_sub(1) } else { rows.len() };
    for ((i, p), (col, y)) in rows.iter().zip(&at).take(shown) {
        let x = b0[0] + (width + 6.0) * *col as f64;
        let y = top + y;
        let mid = y + ROW_PITCH / 2.0;
        items.push(Item::Text(Text::new([x + 6.0, mid], (i + 1).to_string(), ROW, pal.link).align(Align::End, Baseline::Middle)));
        let title = font.fit(&p.title, ROW, width - 10.0);
        items.push(Item::Text(Text::new([x + 9.0, mid], title, ROW, pal.ink).align(Align::Start, Baseline::Middle)));
        items.push(Item::Line {
            points: vec![[x, y + ROW_PITCH], [x + width, y + ROW_PITCH]],
            closed: false,
            stroke: Stroke::solid(0.12, pal.zone.mix(pal.background, 0.5)),
        });
        page.links.push(Link { rect: [[x, y], [x + width, y + ROW_PITCH]], page: *i, at: None });
    }
    if shown < rows.len()
        && let Some((col, y)) = at.get(shown)
    {
        let x = b0[0] + (width + 6.0) * *col as f64;
        let more = format!("… and {} more pages", rows.len() - shown);
        items.push(Item::Text(Text::new([x + 9.0, top + y + ROW_PITCH / 2.0], more, ROW, pal.dim).align(Align::Start, Baseline::Middle)));
    }
    page.add("contents", items);
}

/// The revision history, newest first, as a table; one row from the current revision, date and
/// author while it is empty. Past the room, the last row says how many older ones are left out.
fn revisions(page: &mut Page, [b0, b1]: [[f64; 2]; 2], pal: &Palette, font: &Font, info: &TitleInfo) {
    let w = b1[0] - b0[0];
    if w < 30.0 {
        return;
    }
    let mut items = Vec::new();
    heading(&mut items, b0, b1[0], "Revisions", pal);
    let current = Revision { rev: info.revision.clone(), date: info.date.clone(), by: info.author.clone(), note: String::new() };
    let list: Vec<&Revision> = if info.revisions.is_empty() { vec![&current] } else { info.revisions.iter().rev().collect() };
    let cols = [("REV", 0.12), ("DATE", 0.2), ("BY", 0.24), ("NOTE", 0.44)];
    let mut y = b0[1] + HEAD_ROOM;
    let band = ROW_PITCH * 0.9;
    let mut x = b0[0];
    items.push(Item::Rect { rect: [[b0[0], y], [b1[0], y + band]], radius: 0.0, fill: Some(pal.zone.mix(pal.background, 0.85)), stroke: None });
    for (name, share) in cols {
        items.push(Item::Text(Text::new([x + 1.4, y + band / 2.0], name, CAPTION, pal.dim).align(Align::Start, Baseline::Middle).bold()));
        x += w * share;
    }
    y += band;
    let room = ((b1[1] - y) / ROW_PITCH).floor().max(0.0) as usize;
    let shown = if list.len() > room { room.saturating_sub(1) } else { list.len() };
    for (k, r) in list.iter().take(shown).enumerate() {
        if k % 2 == 1 {
            items.push(Item::Rect { rect: [[b0[0], y], [b1[0], y + ROW_PITCH]], radius: 0.0, fill: Some(pal.body), stroke: None });
        }
        let mut x = b0[0];
        for ((_, share), text) in cols.iter().zip([&r.rev, &r.date, &r.by, &r.note]) {
            let cell = w * share;
            let v = if text.is_empty() { "–" } else { text.as_str() };
            items.push(Item::Text(
                Text::new([x + 1.4, y + ROW_PITCH / 2.0], font.fit(v, ROW, cell - 2.8), ROW, pal.ink).align(Align::Start, Baseline::Middle),
            ));
            x += cell;
        }
        y += ROW_PITCH;
    }
    if shown < list.len() && room > 0 {
        let more = list.len() - shown;
        let text = if more == 1 { "… and 1 older revision".to_string() } else { format!("… and {more} older revisions") };
        items.push(Item::Text(Text::new([b0[0] + 1.4, y + ROW_PITCH / 2.0], text, ROW, pal.dim).align(Align::Start, Baseline::Middle)));
        y += ROW_PITCH;
    }
    items.push(Item::Line { points: vec![[b0[0], y], [b1[0], y]], closed: false, stroke: Stroke::solid(0.2, pal.zone) });
    page.add("revisions", items);
}
