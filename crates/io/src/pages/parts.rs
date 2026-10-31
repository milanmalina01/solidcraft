//! The parts list: the schematic's parts grouped by value, part, footprint and part number, with
//! their count and designators (runs as ranges: `C1–C4, C7`); parts not fitted in groups of their
//! own at the end. Pages of it repeat the header, and a row longer than a page goes on over the
//! next; the CSV has the same rows with every designator listed. Also the table of parts in no
//! frame, over as many pages as it takes, and the mark and key every page gives a part not fitted
//! (struck through, a "DNP" tag, and one line saying what that means).

use std::collections::BTreeMap;

use solidcraft_doc::ecad::Electronics;
use solidcraft_doc::ecad::connect::natural_cmp;
use solidcraft_doc::ecad::schematic::split_designator;

use super::border::Sheet;
use super::font::Font;
use super::page::{Align, Baseline, Item, Page, Stroke, Text};
use super::{MIN_TEXT, Palette};

const TEXT: f64 = 2.3;
const HEAD: f64 = 2.3;
const LINE: f64 = 3.5;
const PAD: f64 = 1.6;
const HEADER_ROW: f64 = 7.0;
/// The space the page's heading takes over the table.
const HEADING: f64 = 9.0;

/// One line of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub qty: usize,
    /// Every designator, in natural order.
    pub list: Vec<String>,
    /// As printed: runs as ranges.
    pub designators: String,
    pub value: String,
    pub part: String,
    pub footprint: String,
    pub manufacturer: String,
    pub mpn: String,
    pub description: String,
    pub fitted: bool,
}

/// What a part not fitted is, as every page with one says it after the cross.
pub const DNP_KEY: &str = "DNP = not fitted: pads on the board, no part";
/// The key's text size.
const KEY: f64 = 2.2;

/// A cross from corner to corner of `[a, b]` (the mark of a part not fitted), drawn as lines so it
/// prints whatever the font has.
pub fn cross([a, b]: [[f64; 2]; 2], stroke: Stroke) -> [Item; 2] {
    [Item::Line { points: vec![a, b], closed: false, stroke }, Item::Line { points: vec![[a[0], b[1]], [b[0], a[1]]], closed: false, stroke }]
}

/// A [`dnp_tag`]'s height for text `size` high beside it.
pub fn dnp_tag_height(size: f64) -> f64 {
    (size * 0.72).max(MIN_TEXT) * 1.16
}

/// A small "DNP" tag (the page's colour on the warning colour) with the middle of its left end at
/// `at`, for text `size` high beside it (its own text never under the least size); and its width.
pub fn dnp_tag(at: [f64; 2], size: f64, pal: &Palette, font: &Font) -> (Vec<Item>, f64) {
    let text = (size * 0.72).max(MIN_TEXT);
    let pad = text * 0.3;
    let w = font.width("DNP", text) + 2.0 * pad;
    let h = dnp_tag_height(size);
    let rect = [[at[0], at[1] - h / 2.0], [at[0] + w, at[1] + h / 2.0]];
    let items = vec![
        Item::Rect { rect, radius: size * 0.18, fill: Some(pal.warn), stroke: None },
        Item::Text(Text::new([at[0] + pad, at[1]], "DNP", text, pal.background).align(Align::Start, Baseline::Middle).bold()),
    ];
    (items, w)
}

/// The key to the mark: a cross, a "DNP" tag and [`DNP_KEY`]'s meaning, on one line ending at
/// `at` (its top right), on a patch of the page.
pub fn dnp_key(at: [f64; 2], pal: &Palette, font: &Font) -> Vec<Item> {
    let meaning = DNP_KEY.trim_start_matches("DNP").trim_start();
    let y = at[1] + KEY * 0.7;
    let tw = font.width(meaning, KEY);
    let (_, tag_w) = dnp_tag([0.0, 0.0], KEY * 1.25, pal, font);
    let r = KEY * 0.42;
    let text_x = at[0] - tw;
    let tag_x = text_x - 1.0 - tag_w;
    let cross_x = tag_x - 1.4 - r;
    let mut out = vec![Item::Rect {
        rect: [[cross_x - r - 0.8, at[1] - 0.4], [at[0] + 0.6, at[1] + KEY * 1.4 + 0.4]],
        radius: 0.6,
        fill: Some(pal.background),
        stroke: None,
    }];
    let box_ = [[cross_x - r, y - r], [cross_x + r, y + r]];
    out.push(Item::Rect { rect: box_, radius: 0.0, fill: None, stroke: Some(Stroke::solid(0.2, pal.dim)) });
    out.extend(cross(box_, Stroke::solid(0.25, pal.warn)));
    out.extend(dnp_tag([tag_x, y], KEY * 1.25, pal, font).0);
    out.push(Item::Text(Text::new([text_x, y], meaning, KEY, pal.dim).align(Align::Start, Baseline::Middle)));
    out
}

/// The columns: heading, share of the width.
const COLUMNS: [(&str, f64); 7] =
    [("Qty", 0.05), ("Designators", 0.17), ("Value", 0.1), ("Part", 0.14), ("Footprint", 0.15), ("MPN · Manufacturer", 0.16), ("Description", 0.23)];

/// `C1–C4, C7, R2`: runs of three or more as ranges.
pub fn ranges(designators: &[String]) -> String {
    let mut sorted: Vec<&String> = designators.iter().collect();
    sorted.sort_by(|a, b| natural_cmp(a, b));
    let mut out: Vec<String> = Vec::new();
    let mut run: Vec<(&str, u64, &String)> = Vec::new();
    let flush = |run: &mut Vec<(&str, u64, &String)>, out: &mut Vec<String>| {
        if run.len() >= 3 {
            if let (Some(a), Some(b)) = (run.first(), run.last()) {
                out.push(format!("{}–{}", a.2, b.2));
            }
        } else {
            out.extend(run.iter().map(|r| r.2.clone()));
        }
        run.clear();
    };
    for d in sorted {
        match split_designator(d) {
            Some((head, n)) => {
                let follows = run.last().is_some_and(|(h, m, _)| h.eq_ignore_ascii_case(head) && *m + 1 == n);
                if !follows {
                    flush(&mut run, &mut out);
                }
                run.push((head, n, d));
            }
            None => {
                flush(&mut run, &mut out);
                out.push(d.clone());
            }
        }
    }
    flush(&mut run, &mut out);
    out.join(", ")
}

/// The list's rows: fitted groups first (by their first designator), then the parts not fitted.
/// Parts group by value, part, footprint and part number.
pub fn rows(ecad: &Electronics) -> Vec<Row> {
    type Key = (bool, String, String, String, String, String);
    let mut groups: BTreeMap<Key, (Vec<String>, String)> = BTreeMap::new();
    for p in &ecad.schematic.parts {
        let Some(c) = ecad.component(p.component) else { continue };
        let r = ecad.resolve(c, p.variant.as_deref());
        let value = p.value.clone().unwrap_or_else(|| c.value.clone());
        let footprint = if r.footprint.name.is_empty() { String::new() } else { r.footprint.name.clone() };
        let key = (p.dnp, value, c.name.clone(), footprint, c.mpn.clone(), c.manufacturer.clone());
        let entry = groups.entry(key).or_insert_with(|| (Vec::new(), c.description.clone()));
        entry.0.push(p.designator.clone());
    }
    let mut out: Vec<(bool, String, Row)> = groups
        .into_iter()
        .map(|((dnp, value, part, footprint, mpn, manufacturer), (mut ds, description))| {
            ds.sort_by(|a, b| natural_cmp(a, b));
            let first = ds.first().cloned().unwrap_or_default();
            let designators = ranges(&ds);
            (dnp, first, Row { qty: ds.len(), list: ds, designators, value, part, footprint, manufacturer, mpn, description, fitted: !dnp })
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then(natural_cmp(&a.1, &b.1)));
    out.into_iter().map(|(_, _, r)| r).collect()
}

/// The CSV of [`rows`], with a header line: every designator listed (no ranges), so a parts
/// list reader takes each. A cell a spreadsheet would read as a formula (starting `=`, `+`, `-`,
/// `@`, a tab or a return, but not a plain number) starts with `'`. The file starts with a UTF-8
/// byte order mark when it holds anything but ASCII, so spreadsheets read its µ and Ω right.
pub fn csv(ecad: &Electronics) -> String {
    let quote = |s: &str| {
        let formula = s.starts_with(['=', '+', '-', '@', '\t', '\r']) && s.trim().parse::<f64>().is_err();
        let s = if formula { format!("'{s}") } else { s.to_string() };
        if s.contains([',', '"', '\n', '\r']) || s.starts_with(' ') || s.ends_with(' ') { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
    };
    let mut out = String::from("Qty,Designators,Value,Part,Footprint,Manufacturer,MPN,Description,Fitted\n");
    for r in rows(ecad) {
        let line = [
            r.qty.to_string(),
            r.list.join(", "),
            r.value,
            r.part,
            r.footprint,
            r.manufacturer,
            r.mpn,
            r.description,
            if r.fitted { "yes".into() } else { "no".into() },
        ];
        out.push_str(&line.iter().map(|s| quote(s)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    if out.is_ascii() { out } else { format!("\u{feff}{out}") }
}

/// The table's columns on a page: left x and width of each.
fn columns(geometry: &Sheet) -> Vec<(f64, f64)> {
    let [a0, a1] = geometry.area;
    let w = a1[0] - a0[0];
    let mut x = a0[0];
    COLUMNS
        .iter()
        .map(|(_, share)| {
            let c = (x, w * share);
            x += w * share;
            c
        })
        .collect()
}

/// Each cell's lines, all of them: a long list of designators runs on over as many lines (and
/// pages) as it takes.
fn cells(row: &Row, cols: &[(f64, f64)], font: &Font) -> Vec<Vec<String>> {
    let qty = if row.fitted { row.qty.to_string() } else { format!("{} DNP", row.qty) };
    let texts: [Vec<&str>; 7] = [
        vec![qty.as_str()],
        vec![row.designators.as_str()],
        vec![row.value.as_str()],
        vec![row.part.as_str()],
        vec![row.footprint.as_str()],
        vec![row.mpn.as_str(), row.manufacturer.as_str()],
        vec![row.description.as_str()],
    ];
    texts
        .iter()
        .zip(cols)
        .map(|(paragraphs, (_, w))| {
            let lines: Vec<String> = paragraphs.iter().filter(|t| !t.is_empty()).flat_map(|t| font.wrap(t, TEXT, w - 2.0 * PAD)).collect();
            if lines.is_empty() { vec![String::new()] } else { lines }
        })
        .collect()
}

fn piece_height(lines: usize) -> f64 {
    lines.max(1) as f64 * LINE + 2.0 * PAD - 0.4
}

/// A row's lines on one page: all of them, or a run of them `from..to` when the row is longer
/// than a page holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub row: usize,
    pub from: usize,
    pub to: usize,
}

/// The rows each page holds, in pieces; a "Not fitted" heading row takes room where fitted rows
/// end.
pub fn paginate(rows: &[Row], geometry: &Sheet, font: &Font) -> Vec<Vec<Piece>> {
    let cols = columns(geometry);
    let top = geometry.area[0][1] + HEADING + HEADER_ROW;
    let bottom = geometry.title_block[0][1] - 4.0;
    // The most lines one row gets on a page, with room for the "Not fitted" heading.
    let most = (((bottom - top - HEADER_ROW - 2.0 * PAD) / LINE).floor().max(1.0)) as usize;
    let mut pages: Vec<Vec<Piece>> = vec![Vec::new()];
    let mut y = top;
    let mut fitted = true;
    for (i, r) in rows.iter().enumerate() {
        let n = cells(r, &cols, font).iter().map(Vec::len).max().unwrap_or(1).max(1);
        let mut from = 0;
        while from < n {
            let to = (from + most).min(n);
            let heading = fitted && !r.fitted;
            let h = piece_height(to - from) + if heading { HEADER_ROW } else { 0.0 };
            if y + h > bottom && pages.last().is_some_and(|p| !p.is_empty()) {
                pages.push(Vec::new());
                y = top;
            }
            if heading {
                fitted = false;
            }
            // A page's first piece of parts not fitted repeats the heading.
            let first = pages.last().is_some_and(Vec::is_empty);
            let h = piece_height(to - from) + if !r.fitted && (heading || first) { HEADER_ROW } else { 0.0 };
            if let Some(p) = pages.last_mut() {
                p.push(Piece { row: i, from, to });
            }
            y += h;
            from = to;
        }
    }
    pages
}

/// One page of the list.
pub fn draw(page: &mut Page, geometry: &Sheet, pal: &Palette, font: &Font, rows: &[Row], chunk: &[Piece], more: bool) {
    let cols = columns(geometry);
    let [a0, a1] = geometry.area;
    let mut items = Vec::new();
    let total: usize = rows.iter().filter(|r| r.fitted).map(|r| r.qty).sum();
    let dnp: usize = rows.iter().filter(|r| !r.fitted).map(|r| r.qty).sum();
    let groups = rows.iter().filter(|r| r.fitted).count();
    items.push(Item::Text(Text::new([a0[0], a0[1] + 1.0], "Parts list", 3.6, pal.ink).align(Align::Start, Baseline::Top).bold()));
    let lines = if groups == 1 { "1 line".to_string() } else { format!("{groups} lines") };
    let summary = format!("{total} parts to fit, in {lines} · {dnp} not fitted");
    items.push(Item::Text(Text::new([a1[0], a0[1] + 1.6], summary, TEXT, pal.dim).align(Align::End, Baseline::Top)));
    let mut y = a0[1] + HEADING;
    header(&mut items, &cols, y, pal, font);
    y += HEADER_ROW;
    let mut shade = false;
    let mut last_fitted = true;
    let mut last_row = None;
    for (k, piece) in chunk.iter().enumerate() {
        let Some(r) = rows.get(piece.row) else { continue };
        if !r.fitted && (last_fitted || k == 0) {
            items.push(Item::Rect {
                rect: [[a0[0], y], [a1[0], y + HEADER_ROW]],
                radius: 0.0,
                fill: Some(pal.warn.mix(pal.background, 0.85)),
                stroke: None,
            });
            // The heading says it as the marks' key does: a cross, the tag and what it means.
            let mid = y + HEADER_ROW / 2.0;
            let r = HEAD * 0.42;
            let x = a0[0] + PAD + r;
            items.push(Item::Rect { rect: [[x - r, mid - r], [x + r, mid + r]], radius: 0.0, fill: None, stroke: Some(Stroke::solid(0.2, pal.dim)) });
            items.extend(cross([[x - r, mid - r], [x + r, mid + r]], Stroke::solid(0.25, pal.warn)));
            let (tag, w) = dnp_tag([x + r + 1.4, mid], HEAD * 1.3, pal, font);
            items.extend(tag);
            let meaning = DNP_KEY.trim_start_matches("DNP").trim_start();
            items.push(Item::Text(Text::new([x + r + 2.4 + w, mid], meaning, HEAD, pal.warn).align(Align::Start, Baseline::Middle).bold()));
            y += HEADER_ROW;
        }
        last_fitted = r.fitted;
        let lines = cells(r, &cols, font);
        let h = piece_height(piece.to - piece.from);
        // A row's pieces share its shade.
        if last_row != Some(piece.row) {
            shade = !shade;
            last_row = Some(piece.row);
        }
        if !shade {
            items.push(Item::Rect { rect: [[a0[0], y], [a1[0], y + h]], radius: 0.0, fill: Some(pal.body), stroke: None });
        }
        let colour = if r.fitted { pal.ink } else { pal.dim };
        for ((x, _), cell) in cols.iter().zip(&lines) {
            for (k, line) in cell.iter().skip(piece.from).take(piece.to - piece.from).enumerate() {
                items.push(Item::Text(
                    Text::new([x + PAD, y + PAD + 0.2 + k as f64 * LINE], line.clone(), TEXT, colour).align(Align::Start, Baseline::Top),
                ));
            }
        }
        y += h;
        items.push(Item::Line { points: vec![[a0[0], y], [a1[0], y]], closed: false, stroke: Stroke::solid(0.15, pal.zone) });
    }
    if more {
        items.push(Item::Text(Text::new([a0[0], y + 4.0], "Continued on the next page", TEXT, pal.dim).align(Align::Start, Baseline::Top)));
    }
    page.add("parts", items);
}

fn header(items: &mut Vec<Item>, cols: &[(f64, f64)], y: f64, pal: &Palette, _font: &Font) {
    let (Some(first), Some(last)) = (cols.first(), cols.last()) else { return };
    let (x0, x1) = (first.0, last.0 + last.1);
    items.push(Item::Rect { rect: [[x0, y], [x1, y + HEADER_ROW]], radius: 0.0, fill: Some(pal.zone.mix(pal.background, 0.8)), stroke: None });
    for ((name, _), (x, _)) in COLUMNS.iter().zip(cols) {
        items.push(Item::Text(Text::new([x + PAD, y + HEADER_ROW / 2.0], *name, HEAD, pal.ink).align(Align::Start, Baseline::Middle).bold()));
    }
    items.push(Item::Line { points: vec![[x0, y + HEADER_ROW], [x1, y + HEADER_ROW]], closed: false, stroke: Stroke::solid(0.3, pal.ink) });
}

/// A part in no frame, for its table.
pub struct OtherPart {
    pub designator: String,
    pub value: String,
    pub part: String,
    /// Where it is on the schematic sheet, in millimetres (to find it in the app).
    pub at: [f64; 2],
}

/// The rows of parts in no frame one page holds.
pub fn other_rows(geometry: &Sheet) -> usize {
    let top = geometry.area[0][1] + HEADING + HEADER_ROW;
    let bottom = geometry.title_block[0][1] - 4.0;
    ((bottom - top) / (LINE + 2.0 * PAD)).floor().max(1.0) as usize
}

/// A page of the parts in no frame (at most [`other_rows`] of them), as a table with where each
/// is on the sheet.
pub fn other_parts(page: &mut Page, geometry: &Sheet, pal: &Palette, font: &Font, list: &[OtherPart], more: bool) {
    let [a0, a1] = geometry.area;
    let mut items = vec![Item::Text(Text::new([a0[0], a0[1] + 1.0], "Parts in no frame", 3.6, pal.ink).align(Align::Start, Baseline::Top).bold())];
    let w = (a1[0] - a0[0]).min(200.0);
    let cols = [
        (a0[0], w * 0.2, "Designator"),
        (a0[0] + w * 0.2, w * 0.25, "Value"),
        (a0[0] + w * 0.45, w * 0.35, "Part"),
        (a0[0] + w * 0.8, w * 0.2, "On the sheet at"),
    ];
    let mut y = a0[1] + HEADING;
    items.push(Item::Rect {
        rect: [[a0[0], y], [a0[0] + w, y + HEADER_ROW]],
        radius: 0.0,
        fill: Some(pal.zone.mix(pal.background, 0.8)),
        stroke: None,
    });
    for (x, _, name) in cols {
        items.push(Item::Text(Text::new([x + PAD, y + HEADER_ROW / 2.0], name, HEAD, pal.ink).align(Align::Start, Baseline::Middle).bold()));
    }
    y += HEADER_ROW;
    for p in list.iter().take(other_rows(geometry)) {
        let at = format!("{:.1}, {:.1} mm", p.at[0], p.at[1]).replace('-', "−");
        for ((x, cw, _), text) in cols.iter().zip([&p.designator, &p.value, &p.part, &at]) {
            items.push(Item::Text(
                Text::new([x + PAD, y + PAD], font.fit(text, TEXT, cw - 2.0 * PAD), TEXT, pal.ink).align(Align::Start, Baseline::Top),
            ));
        }
        y += LINE + 2.0 * PAD;
        items.push(Item::Line { points: vec![[a0[0], y], [a0[0] + w, y]], closed: false, stroke: Stroke::solid(0.15, pal.zone) });
    }
    if more {
        items.push(Item::Text(Text::new([a0[0], y + 4.0], "Continued on the next page", TEXT, pal.dim).align(Align::Start, Baseline::Top)));
    }
    page.add("parts", items);
}
