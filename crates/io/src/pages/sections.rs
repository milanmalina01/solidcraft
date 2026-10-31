//! The book's plan and its schematic pages: the cover first (see [`super::cover`]), a page per
//! frame (its contents, with cross-references to the pages and zones where each net goes on), and
//! the parts outside every frame (a net that goes on there refers to their page). A sheet with no
//! frames prints as tiles that stand in for them. A list of cross-references names a few pages
//! and how many more, and the net index at the end of the book has every named net's pages (ground
//! is listed only there). Pages are numbered before anything is drawn, so every page can refer to
//! every other.

use std::collections::{BTreeMap, BTreeSet};

use solidcraft_doc::ecad::connect::Connectivity;
use solidcraft_doc::ecad::frame::{frame_at, in_order, part_frames};
use solidcraft_doc::ecad::schematic::{Pins, part_bbox};
use solidcraft_doc::ecad::{Electronics, Frame, Label, LabelKind, Library, Schematic, connectivity};

use super::border::{Block, Sheet, scale_text};
use super::font::Font;
use super::page::{Align, Baseline, Item, Link, Page, Text};
use super::sheet::{self, Ctx, Detail, TEXT, View, pointing};
use super::{Book, Entry, MAX_PAGES, MIN_TEXT, Options, Orientation, PageKind, Paper, TURN_GAIN, blocks, board, cover, nets, parts};

/// The most a section is enlarged: one scale for most sections, so symbols print alike on every
/// page and a small frame does not fill its sheet with outsized symbols.
const MAX_SECTION_SCALE: f64 = 2.0;
/// A section page's scale below which its 1.27 mm text would print noticeably under 1.6 mm.
const SMALL_SCALE: f64 = MIN_TEXT / TEXT * 0.92;
/// The sheet margin round a frame's contents on its page.
const FRAME_MARGIN: f64 = 1.0;
/// The room round a section whose wires leave it, for the arrows and net names at the cuts.
const CUT_MARGIN: f64 = 8.0;
/// The scale a tile of a sheet with no frames prints at, at least: its 1.27 mm text at 1.6 mm.
const TILE_SCALE: f64 = 1.25;
/// The most tiles across and down a sheet with no frames is split into.
const MAX_TILES: usize = 8;
/// A tile with fewer parts than this gives them to its neighbour rather than a page of its own.
const FEW_PARTS: usize = 3;
/// The frame a net shows up in when it is in none, for the cross-references.
const OUTSIDE: u64 = u64::MAX;
/// The most pin names one page's warning names.
const MAX_LEFT_OUT: usize = 8;
/// The room a section page with a part not fitted keeps at its top for the key to the mark.
const KEY_ROOM: f64 = 5.0;
/// The most pages one list of cross-references names; a longer one names one fewer and how many
/// more, a link to the net's row in the net index.
const MAX_REFS: usize = 3;

/// A page planned: what it shows and how it is numbered.
#[derive(Clone, Debug)]
pub(super) struct Plan {
    pub kind: PageKind,
    pub frame: Option<u64>,
    pub title: String,
    /// The section's view, worked out before drawing so other pages can name its zones.
    pub view: Option<View>,
    pub scale: Option<f64>,
    /// The page's share of a list spread over pages (parts list rows, parts in no frame), or
    /// which inner layer it shows.
    pub chunk: usize,
    /// Its paper, turned, with its border and zones.
    pub sheet: Sheet,
}

/// Where a net shows up: in which frame ([`OUTSIDE`] for none), and the sheet point (a label of
/// it there, else a pin).
struct Occurrence {
    frame: u64,
    at: [f64; 2],
    label: bool,
}

/// What the parts of a schematic page come from: the frames (or the tiles standing in for
/// them), each part's frame, what each frame's page shows of the sheet, and its wire cuts.
struct Sections<'a> {
    frames: Vec<&'a Frame>,
    membership: BTreeMap<u64, u64>,
    extents: BTreeMap<u64, [[f64; 2]; 2]>,
    cuts: BTreeMap<u64, Vec<Cut>>,
}

impl<'a> Sections<'a> {
    fn new(ctx: &Ctx, pins: &Pins, frames: Vec<&'a Frame>, membership: BTreeMap<u64, u64>) -> Self {
        let extents = extents(ctx, pins, &frames, &membership);
        let cuts = wire_cuts(ctx.sheet, ctx.conn, pins, &frames, &extents);
        Sections { frames, membership, extents, cuts }
    }

    fn extent(&self, f: &Frame) -> [[f64; 2]; 2] {
        self.extents.get(&f.id).copied().unwrap_or(f.rect)
    }

    /// The room a frame's page keeps round it for the arrows at its wire cuts.
    fn margin(&self, f: &Frame) -> f64 {
        if self.cuts.get(&f.id).is_some_and(|c| !c.is_empty()) { CUT_MARGIN } else { 0.0 }
    }

    /// A label's frame: its part's when it sits on a pin, else the frame holding its point.
    fn label_frame(&self, pins: &Pins, l: &Label) -> u64 {
        match l.at.pin() {
            Some((part, _)) => self.membership.get(&part).copied(),
            None => pins.anchor(&l.at).and_then(|at| frame_at(&self.frames, at)),
        }
        .unwrap_or(OUTSIDE)
    }
}

pub(super) fn compose(ecad: &Electronics, o: &Options, font: &Font) -> Result<Book, String> {
    let pal = o.style.palette();
    let sheet = &ecad.schematic;
    let library = ecad.library();
    let conn = connectivity(sheet, &library);
    let pins = Pins::new(sheet, &library);
    let ctx = Ctx { sheet, library: &library, conn: &conn, pal: &pal, font };
    let mut warnings: Vec<String> = Vec::new();
    let wants = |k: PageKind| o.pages.is_empty() || o.pages.contains(&k);
    let named = |k: PageKind| o.pages.contains(&k);

    let own = in_order(&sheet.frames);
    if let Some(ids) = &o.frames {
        for id in ids {
            if !own.iter().any(|f| f.id == *id) {
                return Err(format!("the schematic has no frame {id}"));
            }
        }
    }
    let has_sheet = !sheet.parts.is_empty();
    let board = &ecad.board;
    let has_board = !board.outline.is_empty() || !board.footprints.is_empty();
    if named(PageKind::BlockDiagram) && own.len() < 2 {
        warnings.push("a block diagram needs two frames or more".into());
    }
    if o.pages.iter().any(|k| k.is_board()) && !has_board {
        warnings.push("the design has no board yet, so it has no board pages".into());
    }
    for (a, b) in solidcraft_doc::ecad::frame::overlaps(&sheet.frames) {
        let title = |id: u64| sheet.frames.iter().find(|f| f.id == id).map_or(String::new(), |f| f.title.clone());
        let first = own.iter().find(|f| f.id == a || f.id == b).map_or(a, |f| f.id);
        warnings.push(format!(
            "frames `{}` and `{}` overlap: parts in both belong to `{}` (the first in print order)",
            title(a),
            title(b),
            title(first)
        ));
    }

    // One paper for the book, the same whichever frames print: A4 when every frame fits it at
    // full-size text, else A3.
    let mut sections = Sections::new(&ctx, &pins, own.clone(), part_frames(sheet, &library));
    // A section page with a part not fitted keeps room at its top for the key to the mark.
    let key_room = |s: &Sections, id: u64| {
        let dnp = sheet.parts.iter().any(|p| p.dnp && s.membership.get(&p.id) == Some(&id));
        if dnp { KEY_ROOM } else { 0.0 }
    };
    let paper = o.paper.unwrap_or_else(|| {
        let fits = !own.is_empty()
            && own.iter().all(|f| {
                section_fit(Paper::A4, o.orientation, frame_size(sections.extent(f), sections.margin(f)), key_room(&sections, f.id)).1 >= 1.25
            });
        if fits { Paper::A4 } else { Paper::A3 }
    });
    let geometry = Sheet::new(paper, o.orientation.unwrap_or(Orientation::Landscape));

    // A sheet with no frames prints as tiles of it, each at a scale its text reads at.
    let (tiles, tile_members) = if own.is_empty() && has_sheet { tiles(&ctx, paper, o.orientation) } else { (Vec::new(), BTreeMap::new()) };
    let tiled = !tiles.is_empty();
    if tiled {
        let frames: Vec<&Frame> = tiles.iter().collect();
        sections = Sections::new(&ctx, &pins, frames, tile_members);
        warnings.push(format!(
            "the schematic has no frames, so it prints as {} on its own pages: draw frames round its groups (ecad.schematic.frame) for a page per section",
            if tiles.len() == 1 { "one piece".to_string() } else { format!("{} tiles", tiles.len()) }
        ));
    }
    let frames = &sections.frames;

    // The plan: the whole book, whatever pages and frames are asked for, so a part of it prints
    // with the whole book's page numbers and cross-references (only the pages asked for are drawn).
    let mut plan: Vec<Plan> = Vec::new();
    let page = |kind, frame, title: &str| Plan { kind, frame, title: title.to_string(), view: None, scale: None, chunk: 0, sheet: geometry };
    plan.push(page(PageKind::Cover, None, "Cover"));
    let keyed: BTreeSet<u64> = frames.iter().filter(|f| key_room(&sections, f.id) > 0.0).map(|f| f.id).collect();
    for f in frames {
        let (extent, margin) = (sections.extent(f), sections.margin(f));
        let size = frame_size(extent, margin);
        let top = if keyed.contains(&f.id) { KEY_ROOM } else { 0.0 };
        let (sheet, _) = section_fit(paper, o.orientation, size, top);
        let (scale, placed) = sheet.fit_in(below(&sheet, top), size, MAX_SECTION_SCALE, true);
        let lo = [extent[0][0] - FRAME_MARGIN - margin, extent[0][1] - FRAME_MARGIN - margin];
        let hi = [extent[1][0] + FRAME_MARGIN + margin, extent[1][1] + FRAME_MARGIN + margin];
        let view = View::new([lo, hi], placed, scale);
        plan.push(Plan { view: Some(view), scale: Some(scale), sheet, ..page(PageKind::Sections, Some(f.id), &f.title) });
    }
    let outside: Vec<&solidcraft_doc::ecad::Part> = sheet.parts.iter().filter(|p| !sections.membership.contains_key(&p.id)).collect();
    if !frames.is_empty() && !outside.is_empty() {
        let mut names: Vec<&str> = outside.iter().map(|p| p.designator.as_str()).collect();
        names.sort_by(|a, b| solidcraft_doc::ecad::connect::natural_cmp(a, b));
        let shown: Vec<&str> = names.iter().take(20).copied().collect();
        let more = if names.len() > 20 { format!(" and {} more", names.len() - 20) } else { String::new() };
        let (count, verb) = if names.len() == 1 { ("1 part".to_string(), "is") } else { (format!("{} parts", names.len()), "are") };
        warnings.push(format!("{count} {verb} in no frame ({}{more}): see the “Other parts” page", shown.join(", ")));
        let n = outside.len().div_ceil(parts::other_rows(&geometry).max(1));
        for k in 0..n {
            let title = if n > 1 { format!("Other parts ({} of {n})", k + 1) } else { "Other parts".into() };
            plan.push(Plan { chunk: k, ..page(PageKind::Sections, None, &title) });
        }
    }
    if own.len() >= 2 {
        plan.push(page(PageKind::BlockDiagram, None, "Block diagram"));
    }
    let rows = parts::rows(ecad);
    let chunks = parts::paginate(&rows, &geometry, font);
    if !rows.is_empty() {
        let n = chunks.len();
        for k in 0..n {
            let title = if n > 1 { format!("Parts list ({} of {n})", k + 1) } else { "Parts list".into() };
            plan.push(Plan { chunk: k, ..page(PageKind::Parts, None, &title) });
        }
    }
    if has_board {
        let inner = usize::from(board.copper_layers).saturating_sub(2);
        for (kind, title) in [
            (PageKind::BoardTop, "Board top"),
            (PageKind::BoardBottom, "Board bottom (seen from the bottom)"),
            (PageKind::InnerLayers, ""),
            (PageKind::AssemblyTop, "Assembly top"),
            (PageKind::AssemblyBottom, "Assembly bottom (seen from the bottom)"),
            (PageKind::Stackup, "Layer stack-up"),
        ] {
            if kind == PageKind::InnerLayers {
                for k in 1..=inner {
                    plan.push(Plan { chunk: k, ..page(kind, None, &format!("Inner layer {k}")) });
                }
            } else {
                plan.push(page(kind, None, title));
            }
        }
        if named(PageKind::InnerLayers) && inner == 0 {
            warnings.push("the board has no inner copper layers".into());
        }
    }

    // Where each net shows up, by frame, for the cross-references.
    let mut seen: BTreeMap<usize, Vec<Occurrence>> = BTreeMap::new();
    let label_frames: BTreeMap<u64, u64> = sheet.labels.iter().map(|l| (l.id, sections.label_frame(&pins, l))).collect();
    for (l, net) in sheet.labels.iter().zip(&conn.label_nets) {
        if let (Some(at), Some(frame)) = (pins.anchor(&l.at), label_frames.get(&l.id)) {
            seen.entry(*net).or_default().push(Occurrence { frame: *frame, at, label: true });
        }
    }
    for (k, n) in conn.nets.iter().enumerate() {
        for p in &n.pins {
            let frame = sections.membership.get(&p.part).copied().unwrap_or(OUTSIDE);
            seen.entry(k).or_default().push(Occurrence { frame, at: p.at, label: false });
        }
    }
    let section_page: BTreeMap<u64, usize> = plan.iter().enumerate().filter_map(|(i, p)| Some((p.frame?, i))).collect();
    let others_page = plan.iter().position(|p| p.kind == PageKind::Sections && p.frame.is_none());
    let parts_page = plan.iter().position(|p| p.kind == PageKind::Parts);
    let titles: BTreeMap<u64, String> = frames.iter().map(|f| (f.id, f.title.clone())).collect();
    // Ground is on nearly every page: never cross-referenced there, listed in the net index.
    let ground: BTreeSet<usize> =
        sheet.labels.iter().zip(&conn.label_nets).filter(|(l, _)| l.kind == LabelKind::Ground).map(|(_, net)| *net).collect();

    // The net index ends the book (its pages come last, so no other page's number moves): every
    // named net with its pages, when a list of cross-references is cut short or power nets are
    // cross-referenced.
    let unindexed = BTreeMap::new();
    let index_rows: Vec<nets::Row> = {
        let refs = Refs {
            seen: &seen,
            section_page: &section_page,
            plan: &plan,
            others: others_page,
            titles: &titles,
            power: o.power_xrefs,
            ground: &ground,
            index: &unindexed,
        };
        if has_sheet && (o.power_xrefs || cuts_short(&ctx, &sections, &refs, &label_frames)) {
            conn.nets
                .iter()
                .enumerate()
                .filter(|(_, n)| n.named)
                .map(|(k, n)| nets::Row { net: k, name: n.name.clone(), power: n.power, pages: refs.targets(k, None) })
                .filter(|r| !r.pages.is_empty())
                .collect()
        } else {
            Vec::new()
        }
    };
    let mut index_pages = nets::paginate(&index_rows, &geometry, font);
    let room = MAX_PAGES.saturating_sub(plan.len());
    if index_pages.len() > room {
        warnings.push(format!("the net index is cut to {room} pages, so the book stays within {MAX_PAGES} pages"));
        index_pages.truncate(room);
    }
    // Each net's row in the index, by page and point, for the "+6 more" links.
    let mut index: BTreeMap<usize, (usize, [f64; 2])> = BTreeMap::new();
    let n = index_pages.len();
    for (k, chunk) in index_pages.iter().enumerate() {
        for p in chunk {
            if let Some(r) = index_rows.get(p.row) {
                index.insert(r.net, (plan.len(), nets::spot(&geometry, p)));
            }
        }
        let title = if n > 1 { format!("Net index ({} of {n})", k + 1) } else { "Net index".into() };
        plan.push(Plan { chunk: k, ..page(PageKind::NetIndex, None, &title) });
    }
    if plan.len() > MAX_PAGES {
        return Err(format!("that is {} pages, more than the {MAX_PAGES} one export makes: pick fewer pages or frames", plan.len()));
    }
    if !has_sheet && !has_board {
        return Err("nothing to print: the design has no schematic parts and no board".into());
    }
    let chosen: Vec<bool> = plan
        .iter()
        .map(|p| {
            wants(p.kind) && (p.kind != PageKind::Sections || tiled || p.frame.is_none_or(|f| o.frames.as_ref().is_none_or(|ids| ids.contains(&f))))
        })
        .collect();
    if !chosen.contains(&true) {
        let has: Vec<&str> = plan.iter().map(|p| p.kind.name()).collect::<BTreeSet<_>>().into_iter().collect();
        return Err(format!("nothing to print: none of the pages asked for applies to this design (it has {})", has.join(", ")));
    }
    let refs = Refs {
        seen: &seen,
        section_page: &section_page,
        plan: &plan,
        others: others_page,
        titles: &titles,
        power: o.power_xrefs,
        ground: &ground,
        index: &index,
    };
    let kinds: Vec<PageKind> = plan.iter().zip(&chosen).filter(|(_, c)| **c).map(|(p, _)| p.kind).collect();
    let copper = board::Copper::new(ecad, &conn, &kinds);

    let total = plan.len();
    let mut pages = Vec::with_capacity(total);
    let mut entries = Vec::with_capacity(total);
    let mut unprinted: BTreeSet<String> = BTreeSet::new();
    for (i, p) in plan.iter().enumerate() {
        if chosen.get(i) != Some(&true) {
            continue;
        }
        let mut out = Page { title: p.title.clone(), size: p.sheet.size, background: pal.background, layers: Vec::new(), links: Vec::new() };
        let mut scale = p.scale.map(scale_text);
        match p.kind {
            PageKind::Cover => {
                cover::draw(ecad, &mut out, &p.sheet, &pal, font, &cover::Cover { info: &o.info, plan: &plan, copper: &copper });
            }
            PageKind::Sections => match (p.frame.and_then(|id| frames.iter().find(|f| f.id == id)), p.view) {
                (Some(f), Some(view)) => {
                    let cuts = sections.cuts.get(&f.id).map(Vec::as_slice).unwrap_or(&[]);
                    let mut left_out = Vec::new();
                    let elsewhere = section(&ctx, &mut out, f, sections.extent(f), view, &refs, cuts, &label_frames, &mut left_out);
                    unprinted.extend(elsewhere);
                    if keyed.contains(&f.id) {
                        let [a0, a1] = p.sheet.area;
                        out.add("key", parts::dnp_key([a1[0], a0[1] + 0.5], &pal, font));
                    }
                    if !left_out.is_empty() {
                        let more = left_out.len().saturating_sub(MAX_LEFT_OUT);
                        let shown: Vec<&str> = left_out.iter().take(MAX_LEFT_OUT).map(String::as_str).collect();
                        let more = if more > 0 { format!(" and {more} more") } else { String::new() };
                        let (names, them) = if left_out.len() == 1 { ("name", "it is") } else { ("names", "they are") };
                        warnings.push(format!(
                            "page {} “{}”: no room for the pin {names} {}{more} (pins crowded at a corner of the symbol), so {them} left off",
                            i + 1,
                            p.title,
                            shown.join(", ")
                        ));
                    }
                    if view.scale < SMALL_SCALE {
                        let fix = paper.larger().map_or("split the frame into smaller ones".to_string(), |b| {
                            format!("{} paper, or smaller frames, print it full size", b.name())
                        });
                        warnings.push(format!(
                            "page {} “{}”: at {} its 1.27 mm text would print at {:.1} mm, so it is set at {MIN_TEXT} mm and may crowd; {fix}",
                            i + 1,
                            p.title,
                            scale_text(view.scale),
                            TEXT * view.scale
                        ));
                    }
                }
                _ => {
                    let per = parts::other_rows(&geometry).max(1);
                    let list: Vec<parts::OtherPart> = outside
                        .iter()
                        .skip(p.chunk * per)
                        .take(per)
                        .map(|q| {
                            let c = library.get(&q.component);
                            parts::OtherPart {
                                designator: q.designator.clone(),
                                value: q.value.clone().or_else(|| c.map(|c| c.component.value.clone())).unwrap_or_default(),
                                part: c.map(|c| c.component.name.clone()).unwrap_or_default(),
                                at: q.placement.at,
                            }
                        })
                        .collect();
                    let more = (p.chunk + 1) * per < outside.len();
                    parts::other_parts(&mut out, &geometry, &pal, font, &list, more);
                }
            },
            PageKind::BlockDiagram => {
                blocks::draw(&ctx, &mut out, &geometry, frames, &sections.membership, &section_page, &ecad.intent);
            }
            PageKind::Parts => {
                if let Some(chunk) = chunks.get(p.chunk) {
                    parts::draw(&mut out, &geometry, &pal, font, &rows, chunk, p.chunk + 1 < chunks.len());
                }
            }
            PageKind::NetIndex => {
                if let Some(chunk) = index_pages.get(p.chunk) {
                    nets::draw(&mut out, &geometry, &pal, font, &index_rows, chunk, p.chunk + 1 < index_pages.len());
                }
            }
            PageKind::BoardTop
            | PageKind::BoardBottom
            | PageKind::InnerLayers
            | PageKind::AssemblyTop
            | PageKind::AssemblyBottom
            | PageKind::Stackup => {
                let s = board::draw(ecad, &copper, p.kind, p.chunk, &mut out, &geometry, &pal, font, parts_page, &mut warnings);
                scale = s.map(scale_text);
            }
        }
        let block = Block { info: &o.info, page_title: &p.title, page: i + 1, pages: total, scale: scale.as_deref(), file: &o.file_name };
        p.sheet.draw(&mut out, &pal, font, &block);
        super::font::overbars(&mut out, font);
        entries.push(Entry {
            number: i + 1,
            kind: p.kind,
            frame: p.frame.filter(|_| !tiled),
            title: p.title.clone(),
            paper,
            orientation: p.sheet.orientation,
            scale,
        });
        pages.push(out);
    }
    // Links go to the pages of this export only, by their place in it.
    let index: BTreeMap<usize, usize> = chosen.iter().enumerate().filter(|(_, c)| **c).enumerate().map(|(k, (i, _))| (i, k)).collect();
    for page in &mut pages {
        page.links = std::mem::take(&mut page.links).into_iter().filter_map(|l| Some(Link { page: *index.get(&l.page)?, ..l })).collect();
    }
    if pages.len() < total {
        let these =
            if pages.len() == 1 { "this page keeps its number".to_string() } else { format!("these {} pages keep their numbers", pages.len()) };
        warnings
            .push(format!("{these} in the whole book of {total} pages, and its cross-references; references to pages not exported are not links"));
    }
    if !unprinted.is_empty() {
        let list: Vec<String> = unprinted.iter().map(|t| format!("“{t}”")).collect();
        warnings.push(format!("cross-references to frames not in this export ({}) name the frame instead of a page", list.join(", ")));
    }
    if let Some(w) = missing_glyphs(&pages, font) {
        warnings.push(w);
    }
    let author = match (o.info.author.is_empty(), o.info.company.is_empty()) {
        (false, false) => format!("{} ({})", o.info.author, o.info.company),
        (false, true) => o.info.author.clone(),
        _ => o.info.company.clone(),
    };
    let title = if o.info.title.is_empty() { o.file_name.clone() } else { o.info.title.clone() };
    Ok(Book { title, author, pages, entries, warnings, paper })
}

/// A warning naming the characters the page font has no glyph for (they print as `?`), if any.
fn missing_glyphs(pages: &[Page], font: &Font) -> Option<String> {
    let mut missing: BTreeSet<char> = BTreeSet::new();
    for page in pages {
        super::pdf::each_text(page, &mut |t: &Text| {
            if missing.len() < 12 {
                missing.extend(t.text.chars().filter(|c| !c.is_control() && font.face.glyph_index(*c).is_none()));
            }
        });
    }
    if missing.is_empty() {
        return None;
    }
    let shown: String = missing.iter().take(12).collect();
    Some(format!("the page font has no glyph for {shown}: such characters print as “?” (the PDF still copies and searches them as written)"))
}

/// The sheet a section `size` mm big prints on (keeping `top` mm of its drawing area free at the
/// top), and its scale there: `orientation` when given, else landscape unless portrait prints it
/// [`TURN_GAIN`] times larger.
fn section_fit(paper: Paper, orientation: Option<Orientation>, size: [f64; 2], top: f64) -> (Sheet, f64) {
    let fit = |o: Orientation| {
        let sheet = Sheet::new(paper, o);
        let (s, _) = sheet.fit_in(below(&sheet, top), size, MAX_SECTION_SCALE, true);
        (sheet, s)
    };
    match orientation {
        Some(o) => fit(o),
        None => {
            let (wide, tall) = (fit(Orientation::Landscape), fit(Orientation::Portrait));
            if tall.1 >= wide.1 * TURN_GAIN { tall } else { wide }
        }
    }
}

/// The sheet's drawing area less `top` millimetres at its top.
fn below(sheet: &Sheet, top: f64) -> [[f64; 2]; 2] {
    let [a0, a1] = sheet.area;
    [[a0[0], a0[1] + top], a1]
}

/// A section's size with `margin` round it (and the small margin every section page has).
fn frame_size([lo, hi]: [[f64; 2]; 2], margin: f64) -> [f64; 2] {
    [hi[0] - lo[0] + 2.0 * (FRAME_MARGIN + margin), hi[1] - lo[1] + 2.0 * (FRAME_MARGIN + margin)]
}

/// What each frame's page shows of the sheet: the frame, grown to take in the whole of every part
/// that belongs to it (a part that sticks out of it is printed whole, not cut at its edge) and
/// the labels on their pins.
fn extents(ctx: &Ctx, pins: &Pins, frames: &[&Frame], membership: &BTreeMap<u64, u64>) -> BTreeMap<u64, [[f64; 2]; 2]> {
    let mut out: BTreeMap<u64, [[f64; 2]; 2]> = frames.iter().map(|f| (f.id, f.rect)).collect();
    let grow = |r: &mut [[f64; 2]; 2], [a, b]: [[f64; 2]; 2]| {
        *r = [[r[0][0].min(a[0]), r[0][1].min(a[1])], [r[1][0].max(b[0]), r[1][1].max(b[1])]];
    };
    for p in &ctx.sheet.parts {
        let (Some(f), Some(b)) = (membership.get(&p.id), ctx.library.get(&p.component).and_then(|c| part_bbox(p, c.symbol))) else { continue };
        if let Some(r) = out.get_mut(f) {
            grow(r, [[b[0][0] - 1.0, b[0][1] - 1.0], [b[1][0] + 1.0, b[1][1] + 1.0]]);
        }
    }
    for l in &ctx.sheet.labels {
        let Some((part, _)) = l.at.pin() else { continue };
        let (Some(f), Some(at)) = (membership.get(&part), pins.anchor(&l.at)) else { continue };
        let Some(r) = out.get_mut(f) else { continue };
        // The tag's length on the sheet: its name at the sheet's text size, its point and ends.
        let s = pointing(l.rotation);
        let long = 2.4 + ctx.font.width(&l.name, TEXT);
        let end = [at[0] + s[0] * long, at[1] + s[1] * long];
        grow(r, [[at[0].min(end[0]) - 1.0, at[1].min(end[1]) - 1.0], [at[0].max(end[0]) + 1.0, at[1].max(end[1]) + 1.0]]);
    }
    out
}

/// Each part's tile, by part id: the first holding the middle of its symbol's box (as frames take
/// parts).
fn members(sheet: &Schematic, library: &Library, tiles: &[&Frame]) -> BTreeMap<u64, u64> {
    let mut out = BTreeMap::new();
    for p in &sheet.parts {
        let middle = library
            .get(&p.component)
            .and_then(|c| part_bbox(p, c.symbol))
            .map_or(p.placement.at, |[a, b]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
        if let Some(f) = frame_at(tiles, middle) {
            out.insert(p.id, f);
        }
    }
    out
}

/// A sheet with no frames split into the fewest tiles that each print at [`TILE_SCALE`] or more
/// on `paper` (at most [`MAX_TILES`] across and down), as frames in reading order, with each
/// part's tile. A tile with fewer than [`FEW_PARTS`] parts gives them to the nearest other tile
/// (whose page grows to take them in) and is left out, as is a tile with none.
fn tiles(ctx: &Ctx, paper: Paper, orientation: Option<Orientation>) -> (Vec<Frame>, BTreeMap<u64, u64>) {
    let Some([lo, hi]) = ctx.sheet.bbox(ctx.library) else { return (Vec::new(), BTreeMap::new()) };
    let (lo, hi) = ([lo[0] - 2.0, lo[1] - 2.0], [hi[0] + 2.0, hi[1] + 2.0]);
    let size = [hi[0] - lo[0], hi[1] - lo[1]];
    if !(size[0] > 0.0 && size[1] > 0.0) {
        return (Vec::new(), BTreeMap::new());
    }
    let scale = |cols: usize, rows: usize| {
        section_fit(paper, orientation, frame_size([[0.0, 0.0], [size[0] / cols as f64, size[1] / rows as f64]], CUT_MARGIN), 0.0).1
    };
    let mut grid = (MAX_TILES, MAX_TILES);
    'search: for n in 1..=MAX_TILES * MAX_TILES {
        let best =
            (1..=MAX_TILES).filter(|c| n % c == 0 && n / c <= MAX_TILES).map(|c| (c, n / c, scale(c, n / c))).max_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((c, r, s)) = best
            && s >= TILE_SCALE
        {
            grid = (c, r);
            break 'search;
        }
    }
    let (cols, rows) = grid;
    let (tw, th) = (size[0] / cols as f64, size[1] / rows as f64);
    let mut all = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let rect = [[lo[0] + tw * c as f64, hi[1] - th * (r + 1) as f64], [lo[0] + tw * (c + 1) as f64, hi[1] - th * r as f64]];
            all.push(Frame { id: OUTSIDE - 1 - all.len() as u64, title: String::new(), rect, order: None });
        }
    }
    let refs: Vec<&Frame> = all.iter().collect();
    let mut membership = members(ctx.sheet, ctx.library, &refs);
    let mut count: BTreeMap<u64, usize> = BTreeMap::new();
    for f in membership.values() {
        *count.entry(*f).or_default() += 1;
    }
    let few = |id: &u64| count.get(id).copied().unwrap_or(0) < FEW_PARTS;
    if all.iter().any(|f| !few(&f.id)) {
        let middles: BTreeMap<u64, [f64; 2]> = ctx
            .sheet
            .parts
            .iter()
            .map(|p| {
                (
                    p.id,
                    ctx.library
                        .get(&p.component)
                        .and_then(|c| part_bbox(p, c.symbol))
                        .map_or(p.placement.at, |[a, b]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]),
                )
            })
            .collect();
        let away = |[x, y]: [f64; 2], [lo, hi]: [[f64; 2]; 2]| (lo[0] - x).max(x - hi[0]).max(0.0).hypot((lo[1] - y).max(y - hi[1]).max(0.0));
        for (part, tile) in membership.iter_mut() {
            if !few(tile) {
                continue;
            }
            let at = middles.get(part).copied().unwrap_or_default();
            if let Some(near) = all.iter().filter(|f| !few(&f.id)).min_by(|a, b| away(at, a.rect).total_cmp(&away(at, b.rect))) {
                *tile = near.id;
            }
        }
    }
    let used: BTreeSet<u64> = membership.values().copied().collect();
    let mut kept: Vec<Frame> = all.into_iter().filter(|f| used.contains(&f.id)).collect();
    let n = kept.len();
    for (k, f) in kept.iter_mut().enumerate() {
        f.title = if n > 1 { format!("Schematic ({} of {n})", k + 1) } else { "Schematic".into() };
    }
    (kept, membership)
}

/// What a section page needs to write its cross-references.
struct Refs<'a> {
    seen: &'a BTreeMap<usize, Vec<Occurrence>>,
    section_page: &'a BTreeMap<u64, usize>,
    plan: &'a [Plan],
    /// The first page of parts in no frame, for a net that goes on there.
    others: Option<usize>,
    /// Each frame's title, for a frame that is printed nowhere in this export.
    titles: &'a BTreeMap<u64, String>,
    power: bool,
    /// The ground nets, by index.
    ground: &'a BTreeSet<usize>,
    /// Each net's row in the net index: its page and the row's point there.
    index: &'a BTreeMap<usize, (usize, [f64; 2])>,
}

/// Where a net goes on: a page (with its zone and the page point when known) as `3·B4` or `3`,
/// or a frame not in this export by its title; or how many more a list cut short leaves out (a
/// link to the net's row in the net index).
pub(super) struct Target {
    pub page: Option<usize>,
    pub text: String,
    pub at: Option<[f64; 2]>,
    pub more: bool,
}

impl Refs<'_> {
    /// Whether net `net` gets no cross-references on section pages: ground, or a rail when power
    /// nets are not cross-referenced.
    fn skip(&self, conn: &Connectivity, net: usize) -> bool {
        self.ground.contains(&net) || conn.nets.get(net).is_none_or(|n| n.power && !self.power)
    }

    /// Just the link to net `net`'s row in the net index, for a list of `n` targets with no room
    /// on the page (`+n more`); `None` when the book has no index row for it.
    fn only_more(&self, net: usize, n: usize) -> Option<Vec<Target>> {
        let row = self.index.get(&net)?;
        Some(vec![Target { page: Some(row.0), text: format!("+{n} more"), at: Some(row.1), more: true }])
    }

    /// `list` cut to one fewer than [`MAX_REFS`] when it is longer, and how many more.
    fn cap(&self, net: usize, mut list: Vec<Target>) -> Vec<Target> {
        if list.len() <= MAX_REFS {
            return list;
        }
        let keep = MAX_REFS.saturating_sub(1);
        let more = list.len().saturating_sub(keep);
        list.truncate(keep);
        let row = self.index.get(&net);
        list.push(Target { page: row.map(|r| r.0), text: format!("+{more} more"), at: row.map(|r| r.1), more: true });
        list
    }

    /// Where net `net` goes on outside `frame` (anywhere when `None`): a target per other frame,
    /// by page.
    fn targets(&self, net: usize, frame: Option<u64>) -> Vec<Target> {
        let Some(all) = self.seen.get(&net) else { return Vec::new() };
        let mut by_frame: BTreeMap<u64, &Occurrence> = BTreeMap::new();
        for occ in all.iter().filter(|o| Some(o.frame) != frame) {
            let better = by_frame
                .get(&occ.frame)
                .is_none_or(|b| (occ.label && !b.label) || (occ.label == b.label && (occ.at[1], -occ.at[0]) > (b.at[1], -b.at[0])));
            if better {
                by_frame.insert(occ.frame, occ);
            }
        }
        let mut out: Vec<Target> = Vec::new();
        for (f, occ) in by_frame {
            let on = |n: usize, view: Option<View>| match (self.plan.get(n), view) {
                (Some(page), Some(v)) => {
                    let p = v.p(occ.at);
                    Target { page: Some(n), text: format!("{}·{}", n + 1, page.sheet.zone(p)), at: Some(p), more: false }
                }
                _ => Target { page: Some(n), text: (n + 1).to_string(), at: None, more: false },
            };
            let target = match self.section_page.get(&f) {
                Some(n) => on(*n, self.plan.get(*n).and_then(|p| p.view)),
                None if f == OUTSIDE => match self.others {
                    Some(n) => on(n, None),
                    None => continue,
                },
                None => match self.titles.get(&f) {
                    Some(t) => Target { page: None, text: t.clone(), at: None, more: false },
                    None => continue,
                },
            };
            out.push(target);
        }
        out.sort_by(|a, b| (a.page.is_none(), a.page, &a.text).cmp(&(b.page.is_none(), b.page, &b.text)));
        out.dedup_by(|a, b| a.page == b.page && a.text == b.text);
        out
    }
}

/// Whether a section page would cut a list of cross-references short (each net looked up once per
/// frame).
fn cuts_short(ctx: &Ctx, sections: &Sections, refs: &Refs, label_frames: &BTreeMap<u64, u64>) -> bool {
    let framed: BTreeSet<u64> = sections.frames.iter().map(|f| f.id).collect();
    let mut asked: BTreeSet<(usize, u64)> = BTreeSet::new();
    let labels = ctx.sheet.labels.iter().zip(&ctx.conn.label_nets).filter_map(|(l, net)| Some((*net, *label_frames.get(&l.id)?)));
    let cuts = sections.cuts.iter().flat_map(|(f, cuts)| cuts.iter().map(|c| (c.net, *f)));
    labels
        .chain(cuts)
        .filter(|(net, f)| framed.contains(f) && !refs.skip(ctx.conn, *net))
        .any(|(net, f)| asked.insert((net, f)) && refs.targets(net, Some(f)).len() > MAX_REFS)
}

/// A frame's page: what it shows of the sheet (`extent`: the frame and the whole of its parts),
/// the frame, and the cross-references. Returns the titles of frames not in this export that
/// its references name.
#[allow(clippy::too_many_arguments)]
fn section(
    ctx: &Ctx,
    out: &mut Page,
    f: &Frame,
    extent: [[f64; 2]; 2],
    view: View,
    refs: &Refs,
    cuts: &[Cut],
    label_frames: &BTreeMap<u64, u64>,
    left_out: &mut Vec<String>,
) -> Vec<String> {
    let lo = [extent[0][0] - FRAME_MARGIN, extent[0][1] - FRAME_MARGIN];
    let hi = [extent[1][0] + FRAME_MARGIN, extent[1][1] + FRAME_MARGIN];
    let within = [[lo[0] - 30.0, lo[1] - 30.0], [hi[0] + 30.0, hi[1] + 30.0]];
    let mut drawn = sheet::draw(ctx, &view, Detail { texts: true, within: Some(within) });
    // Pin names left out of this frame's own parts (a neighbour's, clipped here, are its page's).
    let [e0, e1] = extent;
    left_out
        .extend(drawn.left_out.iter().filter(|(c, _)| (e0[0]..=e1[0]).contains(&c[0]) && (e0[1]..=e1[1]).contains(&c[1])).map(|(_, n)| n.clone()));
    let clip = view.rect(extent);
    let clip = [[clip[0][0] - 0.6, clip[0][1] - 0.6], [clip[1][0] + 0.6, clip[1][1] + 0.6]];
    out.add("sheet", [Item::Clip { rect: clip, items: drawn.items }]);
    let mut frame_items = Vec::new();
    let tab = sheet::draw_frame(ctx, &view, f, None, 3.0, &mut frame_items);
    out.add("frame", frame_items);
    // Cross-references keep off the frame's edges and its title.
    drawn.room.add_frame(view.rect(f.rect), tab);

    let sheet = ctx.sheet;
    let mut items = Vec::new();
    let mut links = Vec::new();
    let mut named = Vec::new();
    for (l, net) in sheet.labels.iter().zip(&ctx.conn.label_nets) {
        if refs.skip(ctx.conn, *net) || label_frames.get(&l.id) != Some(&f.id) {
            continue;
        }
        let Some(end) = drawn.label_ends.get(&l.id) else { continue };
        let all = refs.targets(*net, Some(f.id));
        let total = all.len();
        let targets = refs.cap(*net, all);
        if targets.is_empty() {
            continue;
        }
        // A net label's or rail's list goes beside it when straight on it would print over a wire
        // or the frame's edge; with no room near for the list, only its link to the net index.
        let size = (end.size * 0.85).max(MIN_TEXT);
        let (end, targets) = match l.kind {
            LabelKind::Net | LabelKind::Rail => {
                let wide = |t: &[Target]| ctx.font.width(&xref_text(ctx, true, t, size).0, size);
                match sheet::beside_wire(&drawn.room, *end, wide(&targets), size) {
                    Some(at) => (at, targets),
                    None => match refs.only_more(*net, total) {
                        Some(short) => (sheet::beside_wire(&drawn.room, *end, wide(&short), size).unwrap_or(*end), short),
                        None => (*end, targets),
                    },
                }
            }
            _ => (*end, targets),
        };
        named.extend(targets.iter().filter(|t| t.page.is_none() && !t.more).map(|t| t.text.clone()));
        let taken = xref(ctx, end.at, end.dir, end.rotation, size, &targets, &mut items, &mut links);
        drawn.room.add(taken);
    }
    // Wires cut by the page's edge: an arrow out and the net's name (ground and rails too), and
    // where it goes on.
    for cut in cuts {
        let p = view.p(cut.at);
        let d = [cut.dir[0], -cut.dir[1]];
        let size = (TEXT * view.scale).max(MIN_TEXT);
        let a = 1.2 * size;
        let tip = [p[0] + d[0] * a, p[1] + d[1] * a];
        let side = [-d[1] * a * 0.45, d[0] * a * 0.45];
        items.push(Item::Area {
            rings: vec![vec![tip, [p[0] + side[0], p[1] + side[1]], [p[0] - side[0], p[1] - side[1]]]],
            fill: ctx.pal.wire,
            stroke: None,
        });
        let level = d[1] == 0.0;
        let rotation = if level { 0.0 } else { 90.0 };
        let forward = d[0] > 0.0 || d[1] < 0.0;
        let start = [tip[0] + d[0] * size * 0.4, tip[1] + d[1] * size * 0.4];
        let name = ctx.conn.nets.get(cut.net).map_or(String::new(), |n| n.name.clone());
        let t = Text::new(start, name.clone(), size, ctx.pal.wire)
            .align(if forward { Align::Start } else { Align::End }, Baseline::Middle)
            .turned(rotation);
        let w = ctx.font.width(&name, size);
        items.push(Item::Text(t));
        if refs.skip(ctx.conn, cut.net) {
            continue;
        }
        let targets = refs.cap(cut.net, refs.targets(cut.net, Some(f.id)));
        if !targets.is_empty() {
            named.extend(targets.iter().filter(|t| t.page.is_none() && !t.more).map(|t| t.text.clone()));
            let after = [start[0] + d[0] * (w + size * 0.5), start[1] + d[1] * (w + size * 0.5)];
            drawn.room.add(xref(ctx, after, d, rotation, size * 0.85, &targets, &mut items, &mut links));
        }
    }
    out.add("xrefs", items);
    out.links.extend(links);
    named
}

/// `→ 3·B4, 5·C1` from `at` running `dir` (`← 3·B4` when the run goes the other way, so the
/// arrow always points away from the label), each page reference a link to its page and zone; a
/// list cut short ends `+6 more`, a link to the net index.
/// Returns the page box it takes.
#[allow(clippy::too_many_arguments)]
fn xref(
    ctx: &Ctx,
    at: [f64; 2],
    dir: [f64; 2],
    rotation: f64,
    size: f64,
    targets: &[Target],
    items: &mut Vec<Item>,
    links: &mut Vec<Link>,
) -> [[f64; 2]; 2] {
    let size = size.max(MIN_TEXT);
    // The run's own direction on the page (level text runs right, upright text upwards).
    let (s, c) = rotation.to_radians().sin_cos();
    let along = [c, -s];
    let forward = dir[0] * along[0] + dir[1] * along[1] > 0.0;
    let (text, spans) = xref_text(ctx, forward, targets, size);
    let total = ctx.font.width(&text, size);
    let start = if forward { at } else { [at[0] - along[0] * total, at[1] - along[1] * total] };
    // On the page's own colour, so a frame's edge or a wire it crosses does not run through it.
    let end = [start[0] + along[0] * total, start[1] + along[1] * total];
    let pad = [along[1].abs() * size * 0.6 + along[0].abs() * 0.3, along[0].abs() * size * 0.6 + along[1].abs() * 0.3];
    let taken = [[start[0].min(end[0]) - pad[0], start[1].min(end[1]) - pad[1]], [start[0].max(end[0]) + pad[0], start[1].max(end[1]) + pad[1]]];
    items.push(Item::Rect { rect: taken, radius: 0.3, fill: Some(ctx.pal.background), stroke: None });
    items.push(Item::Text(Text::new(start, text, size, ctx.pal.link).align(Align::Start, Baseline::Middle).turned(rotation)));
    for ((u0, u1), t) in spans.iter().zip(targets) {
        let Some(page) = t.page else { continue };
        let a = [start[0] + along[0] * u0, start[1] + along[1] * u0];
        let b = [start[0] + along[0] * u1, start[1] + along[1] * u1];
        let h = size * 0.7;
        let across = [along[1].abs() * h, along[0].abs() * h];
        let rect = [[a[0].min(b[0]) - across[0], a[1].min(b[1]) - across[1]], [a[0].max(b[0]) + across[0], a[1].max(b[1]) + across[1]]];
        links.push(Link { rect, page, at: t.at });
    }
    taken
}

/// [`xref`]'s line for `targets` (its arrow pointing along the run when `forward`), and where each
/// page reference starts and ends along it at `size`.
fn xref_text(ctx: &Ctx, forward: bool, targets: &[Target], size: f64) -> (String, Vec<(f64, f64)>) {
    let has = |ch: char| ctx.font.face.glyph_index(ch).is_some();
    let arrow = match (forward, has('→') && has('←')) {
        (true, true) => "→ ",
        (false, true) => "← ",
        (true, false) => "> ",
        (false, false) => "< ",
    };
    let dot = if has('·') { "·" } else { "." };
    let mut text = arrow.to_string();
    let mut spans = Vec::new();
    for (k, t) in targets.iter().enumerate() {
        if k > 0 {
            text.push_str(if t.more { " " } else { ", " });
        }
        let u0 = ctx.font.width(&text, size);
        text.push_str(&t.text.replace('·', dot));
        spans.push((u0, ctx.font.width(&text, size)));
    }
    (text, spans)
}

/// A wire crossing the edge of what a frame's page shows.
pub(super) struct Cut {
    pub net: usize,
    pub at: [f64; 2],
    /// The sheet direction out of the frame.
    pub dir: [f64; 2],
}

/// Each frame's wire cuts at the edge of what its page shows, by frame id.
fn wire_cuts(
    sheet: &Schematic,
    conn: &Connectivity,
    pins: &Pins,
    frames: &[&Frame],
    extents: &BTreeMap<u64, [[f64; 2]; 2]>,
) -> BTreeMap<u64, Vec<Cut>> {
    let mut out: BTreeMap<u64, Vec<Cut>> = BTreeMap::new();
    let wires: Vec<(Vec<[f64; 2]>, usize)> = sheet.wires.iter().zip(&conn.wire_nets).map(|(w, net)| (pins.wire(w), *net)).collect();
    for f in frames {
        let rect = extents.get(&f.id).copied().unwrap_or(f.rect);
        let inside = |[x, y]: [f64; 2]| x >= rect[0][0] && x <= rect[1][0] && y >= rect[0][1] && y <= rect[1][1];
        let mut cuts = Vec::new();
        let mut done: BTreeSet<(i64, i64)> = BTreeSet::new();
        for (points, net) in &wires {
            for pair in points.windows(2) {
                let [a, b] = [pair.first().copied().unwrap_or_default(), pair.get(1).copied().unwrap_or_default()];
                let (ia, ib) = (inside(a), inside(b));
                if ia == ib {
                    continue;
                }
                let (from, to) = if ia { (a, b) } else { (b, a) };
                let Some(at) = edge_crossing(rect, from, to) else { continue };
                let len = ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt().max(1e-9);
                let dir = [(to[0] - from[0]) / len, (to[1] - from[1]) / len];
                // Level or upright: the arrow points straight out.
                let dir = if dir[0].abs() >= dir[1].abs() { [dir[0].signum(), 0.0] } else { [0.0, dir[1].signum()] };
                if done.insert(((at[0] * 100.0).round() as i64, (at[1] * 100.0).round() as i64)) {
                    cuts.push(Cut { net: *net, at, dir });
                }
            }
        }
        out.insert(f.id, cuts);
    }
    out
}

/// Where the segment from `inside` to `outside` leaves the box `[lo, hi]`.
fn edge_crossing([lo, hi]: [[f64; 2]; 2], inside: [f64; 2], outside: [f64; 2]) -> Option<[f64; 2]> {
    let d = [outside[0] - inside[0], outside[1] - inside[1]];
    let mut t = 1.0f64;
    for (k, (min, max)) in [(lo[0], hi[0]), (lo[1], hi[1])].into_iter().enumerate() {
        let (p, dv) = if k == 0 { (inside[0], d[0]) } else { (inside[1], d[1]) };
        if dv > 1e-12 {
            t = t.min((max - p) / dv);
        } else if dv < -1e-12 {
            t = t.min((min - p) / dv);
        }
    }
    (t.is_finite() && (0.0..=1.0).contains(&t)).then(|| [inside[0] + d[0] * t, inside[1] + d[1] * t])
}
