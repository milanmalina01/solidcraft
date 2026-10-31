//! The book as one PDF: a page each, text in the embedded font (subset to the glyphs used, as a
//! CID font with a Unicode map, so it selects and searches), a bookmark per page, links that go to
//! other pages, and the title and author in the document information. No dates and an id made
//! from the content, so the same book always gives the same bytes.
//!
//! A character the font has no glyph for is drawn as `?` but under a CID of its own (mapped to the
//! `?` glyph), whose Unicode entry is the character itself: copying and searching give the text as
//! written, and a real `?` stays a `?`.

use std::collections::{BTreeMap, BTreeSet};

use pdf_writer::types::{
    ActionType, AnnotationType, CidFontType, FontFlags, LineCapStyle, LineJoinStyle, SystemInfo, TextRenderingMode, UnicodeCmap,
};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};
use sha2::{Digest, Sha256};
use subsetter::GlyphRemapper;

use super::Book;
use super::font::{BOLD_STROKE, Font};
use super::page::{Item, Page, Rgb, Stroke, Text, Weight, rounded_rect};

/// Points in a millimetre.
const PT: f64 = 72.0 / 25.4;
/// The font's resource name on every page.
const FONT: Name<'static> = Name(b"F1");
/// A circle's control points sit this far along the tangents, as a share of the radius.
const KAPPA: f64 = 0.552_284_75;
/// The most characters without a glyph that get a CID of their own; more print as `?` and copy
/// as the first of them.
const MAX_MISSING: usize = 4096;

/// What the content draws each character with.
struct Cids {
    /// The glyphs used, renumbered for the subset: a glyph's CID is its new number.
    remap: GlyphRemapper,
    /// The CID of each character the font has no glyph for, after the subset's glyphs.
    missing: BTreeMap<char, u16>,
    /// The glyph the missing characters are drawn with (`?`, else the missing-glyph box), new
    /// numbering.
    fallback: u16,
}

impl Cids {
    fn of(&self, font: &Font, c: char) -> u16 {
        match font.face.glyph_index(c) {
            Some(g) => self.remap.get(g.0).unwrap_or(0),
            None => self.missing.get(&c).copied().unwrap_or(self.fallback),
        }
    }
}

/// The PDF of `book`, set in `font` (`data` its file, which the PDF embeds a subset of).
pub fn write(book: &Book, font: &Font, data: &[u8]) -> Result<Vec<u8>, String> {
    // The glyphs every text uses, renumbered for the subset, and the character each stands for;
    // the characters the font lacks.
    let mut remap = GlyphRemapper::new();
    let mut unicode: BTreeMap<u16, char> = BTreeMap::new();
    let mut lacking: BTreeSet<char> = BTreeSet::new();
    for page in &book.pages {
        each_text(page, &mut |t: &Text| {
            for c in t.text.chars().filter(|c| !c.is_control()) {
                match font.face.glyph_index(c) {
                    Some(g) => {
                        let new = remap.remap(g.0);
                        unicode.entry(new).or_insert(c);
                    }
                    None if lacking.len() < MAX_MISSING => {
                        lacking.insert(c);
                    }
                    None => {}
                }
            }
        });
    }
    let fallback = if lacking.is_empty() { 0 } else { remap.remap(font.glyph('?').0) };
    let glyphs = remap.num_gids();
    let mut missing = BTreeMap::new();
    for (k, c) in lacking.into_iter().enumerate() {
        let Some(cid) = u16::try_from(k).ok().and_then(|k| glyphs.checked_add(k)) else { break };
        missing.insert(c, cid);
        unicode.insert(cid, c);
    }
    let cids = Cids { remap, missing, fallback };
    let remap = &cids.remap;
    let subset = subsetter::subset(data, 0, remap).map_err(|e| format!("the page font could not be cut down: {e}"))?;

    let mut alloc = Ref::new(1);
    let mut next = || alloc.bump();
    let catalog = next();
    let tree = next();
    let outline = next();
    let info = next();
    let type0 = next();
    let cid = next();
    let descriptor = next();
    let file = next();
    let cmap = next();
    let gid_map = next();
    struct Refs {
        page: Ref,
        content: Ref,
        item: Ref,
        links: Vec<Ref>,
    }
    let refs: Vec<Refs> =
        book.pages.iter().map(|p| Refs { page: next(), content: next(), item: next(), links: p.links.iter().map(|_| next()).collect() }).collect();
    // A part of the book keeps the whole book's page numbers, and the viewer shows them too: a
    // page label where the numbering jumps (by the page's place in this PDF, its number).
    let numbers: Vec<usize> = book.entries.iter().map(|e| e.number).collect();
    let labels: Vec<(i32, i32, Ref)> = if numbers.len() == book.pages.len() && numbers.iter().enumerate().any(|(k, n)| *n != k + 1) {
        numbers
            .iter()
            .enumerate()
            .filter(|(k, n)| k.checked_sub(1).and_then(|j| numbers.get(j)).is_none_or(|before| before.saturating_add(1) != **n))
            .filter_map(|(k, n)| Some((i32::try_from(k).ok()?, i32::try_from(*n).ok().filter(|n| *n >= 1)?, next())))
            .collect()
    } else {
        Vec::new()
    };

    let mut pdf = Pdf::new();
    let mut hash = Sha256::new();
    let streams: Vec<Vec<u8>> = book.pages.iter().map(|p| content(p, font, &cids)).collect();
    for s in &streams {
        hash.update(s);
    }
    hash.update(&subset);
    hash.update(book.title.as_bytes());
    let digest = hash.finalize();
    let id: Vec<u8> = digest.iter().take(16).copied().collect();
    pdf.set_file_id((id.clone(), id));

    let mut cat = pdf.catalog(catalog);
    cat.pages(tree).outlines(outline).page_mode(pdf_writer::types::PageMode::UseOutlines);
    if !labels.is_empty() {
        let mut tree = cat.page_labels();
        let mut nums = tree.nums();
        for (k, _, r) in &labels {
            nums.insert(*k, *r);
        }
    }
    cat.finish();
    for (_, n, r) in &labels {
        pdf.indirect(*r).start::<pdf_writer::writers::PageLabel>().style(pdf_writer::types::NumberingStyle::Arabic).offset(*n);
    }
    let count = i32::try_from(book.pages.len()).unwrap_or(i32::MAX);
    pdf.pages(tree).kids(refs.iter().map(|r| r.page)).count(count);
    let mut doc = pdf.document_info(info);
    if !book.title.is_empty() {
        doc.title(TextStr(&book.title));
    }
    if !book.author.is_empty() {
        doc.author(TextStr(&book.author));
    }
    doc.creator(TextStr("SolidCraft"));
    doc.producer(TextStr("SolidCraft"));
    doc.finish();

    for ((page, r), stream) in book.pages.iter().zip(&refs).zip(streams) {
        let [w, h] = page.size;
        let mut p = pdf.page(r.page);
        p.media_box(Rect::new(0.0, 0.0, pt(w), pt(h))).parent(tree).contents(r.content);
        p.resources().fonts().pair(FONT, type0);
        if !r.links.is_empty() {
            p.annotations(r.links.iter().copied());
        }
        p.finish();
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&stream, 6);
        pdf.stream(r.content, &packed).filter(Filter::FlateDecode);
        for (link, id) in page.links.iter().zip(&r.links) {
            let Some(target) = refs.get(link.page) else { continue };
            let target_h = book.pages.get(link.page).map_or(h, |p| p.size[1]);
            let [[x0, y0], [x1, y1]] = link.rect;
            let mut a = pdf.annotation(*id);
            a.subtype(AnnotationType::Link)
                .rect(Rect::new(pt(x0.min(x1)), pt(h - y0.max(y1)), pt(x0.max(x1)), pt(h - y0.min(y1))))
                .border(0.0, 0.0, 0.0, None);
            let mut action = a.action();
            action.action_type(ActionType::GoTo);
            let dest = action.destination().page(target.page);
            match link.at {
                Some([x, y]) => dest.xyz(pt((x - 40.0).max(0.0)), pt(target_h - (y - 30.0).max(0.0)), None),
                None => dest.fit(),
            }
        }
    }

    // A bookmark per page, in order.
    if let (Some(first), Some(last)) = (refs.first(), refs.last()) {
        pdf.outline(outline).first(first.item).last(last.item).count(count);
    }
    for (i, (page, r)) in book.pages.iter().zip(&refs).enumerate() {
        let mut item = pdf.outline_item(r.item);
        let title = format!("{} {}", i + 1, page.title);
        item.title(TextStr(&title)).parent(outline);
        if let Some(prev) = i.checked_sub(1).and_then(|k| refs.get(k)) {
            item.prev(prev.item);
        }
        if let Some(next) = refs.get(i + 1) {
            item.next(next.item);
        }
        item.dest().page(r.page).fit();
    }

    // The font: Type 0 over a CID font whose CIDs are the subset's glyph ids.
    let base = format!("{}+{}", subset_tag(&digest), font.postscript_name());
    let em = |v: f64| (v * 1000.0) as f32;
    pdf.type0_font(type0).base_font(Name(base.as_bytes())).encoding_predefined(Name(b"Identity-H")).descendant_font(cid).to_unicode(cmap);
    let info = SystemInfo { registry: Str(b"Adobe"), ordering: Str(b"Identity"), supplement: 0 };
    let mut c = pdf.cid_font(cid);
    c.subtype(CidFontType::Type2).base_font(Name(base.as_bytes())).system_info(info).font_descriptor(descriptor).default_width(0.0);
    if cids.missing.is_empty() {
        c.cid_to_gid_map_predefined(Name(b"Identity"));
    } else {
        c.cid_to_gid_map_stream(gid_map);
    }
    // In the subset's order: the n-th old glyph is the new glyph n; then the missing characters,
    // each as wide as the glyph drawn for it.
    let mut widths: Vec<f32> = remap.remapped_gids().map(|old| em(font.advance(ttf_parser::GlyphId(old)))).collect();
    let fallback_width = widths.get(usize::from(cids.fallback)).copied().unwrap_or(0.0);
    widths.extend(cids.missing.iter().map(|_| fallback_width));
    c.widths().consecutive(0, widths);
    c.finish();
    if !cids.missing.is_empty() {
        // CID to glyph: itself for the subset's glyphs, the fallback for the missing characters.
        let mut map: Vec<u8> = (0..glyphs).flat_map(u16::to_be_bytes).collect();
        map.extend(cids.missing.iter().flat_map(|_| cids.fallback.to_be_bytes()));
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&map, 6);
        pdf.stream(gid_map, &packed).filter(Filter::FlateDecode);
    }
    let bbox = font.face.global_bounding_box();
    let unit = |v: i16| em(f64::from(v) / font.upem);
    pdf.font_descriptor(descriptor)
        .name(Name(base.as_bytes()))
        .flags(FontFlags::NON_SYMBOLIC)
        .bbox(Rect::new(unit(bbox.x_min), unit(bbox.y_min), unit(bbox.x_max), unit(bbox.y_max)))
        .italic_angle(0.0)
        .ascent(em(font.ascent))
        .descent(em(-font.descent))
        .cap_height(em(font.cap))
        .stem_v(80.0)
        .font_file2(file);
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&subset, 6);
    pdf.stream(file, &packed).filter(Filter::FlateDecode).pair(Name(b"Length1"), i32::try_from(subset.len()).unwrap_or(i32::MAX));
    let mut map = UnicodeCmap::new(Name(b"SolidCraft-UTF16"), info);
    for (g, c) in &unicode {
        map.pair(*g, *c);
    }
    let map = map.finish();
    pdf.cmap(cmap, &map);
    Ok(pdf.finish())
}

fn pt(mm: f64) -> f32 {
    (mm * PT) as f32
}

/// Six letters from the content, as a subset font's name starts.
fn subset_tag(digest: &[u8]) -> String {
    digest.iter().take(6).map(|b| char::from(b'A' + b % 26)).collect()
}

/// Call `f` on every text of the page, clipped groups' too.
pub(super) fn each_text(page: &Page, f: &mut dyn FnMut(&Text)) {
    fn walk(items: &[Item], f: &mut dyn FnMut(&Text)) {
        for i in items {
            match i {
                Item::Text(t) => f(t),
                Item::Clip { items, .. } => walk(items, f),
                _ => {}
            }
        }
    }
    for l in &page.layers {
        walk(&l.items, f);
    }
}

/// The page's content stream: drawn in millimetres with y down (one transform at the start).
fn content(page: &Page, font: &Font, cids: &Cids) -> Vec<u8> {
    let mut c = Content::new();
    let [w, h] = page.size;
    c.transform([PT as f32, 0.0, 0.0, -PT as f32, 0.0, (h * PT) as f32]);
    c.set_line_cap(LineCapStyle::RoundCap).set_line_join(LineJoinStyle::RoundJoin);
    if page.background != Rgb(255, 255, 255) {
        fill_colour(&mut c, page.background);
        c.rect(0.0, 0.0, w as f32, h as f32).fill_nonzero();
    }
    for l in &page.layers {
        items(&mut c, &l.items, font, cids);
    }
    c.finish().to_vec()
}

fn fill_colour(c: &mut Content, rgb: Rgb) {
    let [r, g, b] = rgb.unit();
    c.set_fill_rgb(r, g, b);
}

fn stroke_style(c: &mut Content, s: &Stroke) {
    let [r, g, b] = s.color.unit();
    c.set_stroke_rgb(r, g, b).set_line_width(s.width as f32);
    match s.dash {
        Some([on, off]) => c.set_dash_pattern([on as f32, off as f32], 0.0),
        None => c.set_dash_pattern([], 0.0),
    };
}

fn path(c: &mut Content, points: &[[f64; 2]], closed: bool) {
    let mut it = points.iter();
    let Some(first) = it.next() else { return };
    c.move_to(first[0] as f32, first[1] as f32);
    for p in it {
        c.line_to(p[0] as f32, p[1] as f32);
    }
    if closed {
        c.close_path();
    }
}

fn circle(c: &mut Content, [x, y]: [f64; 2], r: f64) {
    let k = r * KAPPA;
    let f = |v: f64| v as f32;
    c.move_to(f(x + r), f(y));
    c.cubic_to(f(x + r), f(y + k), f(x + k), f(y + r), f(x), f(y + r));
    c.cubic_to(f(x - k), f(y + r), f(x - r), f(y + k), f(x - r), f(y));
    c.cubic_to(f(x - r), f(y - k), f(x - k), f(y - r), f(x), f(y - r));
    c.cubic_to(f(x + k), f(y - r), f(x + r), f(y - k), f(x + r), f(y));
    c.close_path();
}

/// Fill and/or stroke the path just built.
fn paint(c: &mut Content, fill: Option<Rgb>, stroke: Option<&Stroke>, even_odd: bool) {
    if let Some(f) = fill {
        fill_colour(c, f);
    }
    if let Some(s) = stroke {
        stroke_style(c, s);
    }
    match (fill.is_some(), stroke.is_some(), even_odd) {
        (true, true, true) => c.fill_even_odd_and_stroke(),
        (true, true, false) => c.fill_nonzero_and_stroke(),
        (true, false, true) => c.fill_even_odd(),
        (true, false, false) => c.fill_nonzero(),
        (false, true, _) => c.stroke(),
        (false, false, _) => c.end_path(),
    };
}

fn items(c: &mut Content, list: &[Item], font: &Font, cids: &Cids) {
    for i in list {
        match i {
            Item::Line { points, closed, stroke } => {
                if points.len() < 2 {
                    continue;
                }
                path(c, points, *closed);
                paint(c, None, Some(stroke), false);
            }
            Item::Area { rings, fill, stroke } => {
                for r in rings.iter().filter(|r| r.len() >= 3) {
                    path(c, r, true);
                }
                paint(c, Some(*fill), stroke.as_ref(), true);
            }
            Item::Rect { rect, radius, fill, stroke } => {
                path(c, &rounded_rect(*rect, *radius), true);
                paint(c, *fill, stroke.as_ref(), false);
            }
            Item::Circle { center, radius, fill, stroke } => {
                circle(c, *center, *radius);
                paint(c, *fill, stroke.as_ref(), false);
            }
            Item::Text(t) => text(c, t, font, cids),
            Item::Clip { rect: [a, b], items: inner } => {
                c.save_state();
                c.rect(a[0].min(b[0]) as f32, a[1].min(b[1]) as f32, (b[0] - a[0]).abs() as f32, (b[1] - a[1]).abs() as f32);
                c.clip_nonzero().end_path();
                items(c, inner, font, cids);
                c.restore_state();
            }
        }
    }
}

fn text(c: &mut Content, t: &Text, font: &Font, cids: &Cids) {
    if t.text.is_empty() || !(t.size > 0.0) {
        return;
    }
    let mut bytes = Vec::with_capacity(t.text.len() * 2);
    for ch in t.text.chars().filter(|c| !c.is_control()) {
        bytes.extend_from_slice(&cids.of(font, ch).to_be_bytes());
    }
    let (o, along, down) = font.placement(t);
    fill_colour(c, t.color);
    c.begin_text();
    if t.weight == Weight::Bold {
        let [r, g, b] = t.color.unit();
        c.set_stroke_rgb(r, g, b).set_line_width((t.size * BOLD_STROKE) as f32).set_dash_pattern([], 0.0);
        c.set_text_rendering_mode(TextRenderingMode::FillStroke);
    } else {
        c.set_text_rendering_mode(TextRenderingMode::Fill);
    }
    c.set_font(FONT, t.size as f32);
    // Glyph space is y up; the page is drawn y down, so the glyphs' up is the run's "up".
    c.set_text_matrix([along[0] as f32, along[1] as f32, -down[0] as f32, -down[1] as f32, o[0] as f32, o[1] as f32]);
    c.show(Str(&bytes));
    c.end_text();
}
