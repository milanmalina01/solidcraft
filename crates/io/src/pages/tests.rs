//! The print pages of a small design: two frames joined by a net, a rail on both, a part not
//! fitted, and a four-layer board. The book's plan, the cover, cross-references and the net index
//! from the display lists; then each writer's output: the PDF the same bytes every time and well
//! formed, an SVG per page that parses, a PNG per page of the size its resolution gives, and the
//! parts list as CSV.

use serde_json::json;
use solidcraft_doc::ecad::Electronics;
use solidcraft_doc::ecad::info::Revision;

use super::page::Text;
use super::*;

/// R1 and R2 in the frame "Input", joined by a wire; R3, R4 and R5 (not fitted) in "Output", R3
/// and R4 joined. The net SIG is labelled on R2.2 and R3.1, so each frame refers to the other's
/// page; the rail +3V3 is on R1.1 and R4.2, GND on R5.1. The board is four layers, a footprint per
/// part.
fn design() -> Electronics {
    let pin = |number: &str, x: f64, direction: &str| json!({"number": number, "at": [x, 0.0], "direction": direction, "length": 2.54});
    let part = |id: u64, designator: &str, at: [f64; 2]| json!({"id": id, "component": 3, "designator": designator, "placement": {"at": at}});
    let on = |part: u64, pin: &str| json!({"part": part, "pin": pin});
    let mut r5 = part(5, "R5", [120.0, -15.0]);
    r5["dnp"] = json!(true);
    let footprint = |id: u64, part: u64, x: f64, y: f64| json!({"id": id, "part": part, "at": [x, y]});
    let pad = |number: &str, x: f64| json!({"number": number, "at": [x, 0.0], "size": [0.9, 0.95]});
    serde_json::from_value(json!({
        "symbols": [{"id": 1, "name": "Resistor", "pins": [pin("1", -5.08, "right"), pin("2", 5.08, "left")]}],
        "footprints": [{"id": 2, "name": "R0603", "pads": [pad("1", -0.8), pad("2", 0.8)]}],
        "components": [{
            "id": 3, "name": "R0603", "prefix": "R", "value": "10k", "mpn": "RC0603FR-0710KL", "manufacturer": "Yageo",
            "description": "Thick film resistor", "symbol": 1, "variants": [{"name": "0603", "footprint": 2}]
        }],
        "schematic": {
            "parts": [part(1, "R1", [0.0, 0.0]), part(2, "R2", [20.0, 0.0]), part(3, "R3", [100.0, 0.0]), part(4, "R4", [120.0, 0.0]), r5],
            "wires": [{"id": 6, "from": on(1, "2"), "to": on(2, "1")}, {"id": 7, "from": on(3, "2"), "to": on(4, "1")}],
            "labels": [
                {"id": 8, "name": "SIG", "kind": "net", "at": on(2, "2")},
                {"id": 9, "name": "SIG", "kind": "net", "at": on(3, "1"), "rotation": 180},
                {"id": 10, "name": "+3V3", "kind": "rail", "at": on(1, "1"), "rotation": 90},
                {"id": 11, "name": "+3V3", "kind": "rail", "at": on(4, "2"), "rotation": 90},
                {"id": 12, "name": "GND", "kind": "ground", "at": on(5, "1"), "rotation": 270}
            ],
            "frames": [
                {"id": 20, "title": "Input", "rect": [[-15.0, -15.0], [35.0, 15.0]]},
                {"id": 21, "title": "Output", "rect": [[85.0, -25.0], [135.0, 15.0]]}
            ]
        },
        "board": {
            "outline": [[0.0, 0.0], [40.0, 0.0], [40.0, 25.0], [0.0, 25.0]],
            "copper_layers": 4,
            "footprints": [
                footprint(1, 1, 5.0, 5.0), footprint(2, 2, 15.0, 5.0), footprint(3, 3, 25.0, 5.0), footprint(4, 4, 35.0, 5.0),
                footprint(5, 5, 20.0, 15.0)
            ]
        }
    }))
    .unwrap()
}

fn info() -> TitleInfo {
    let rev = |rev: &str, date: &str, note: &str| Revision { rev: rev.into(), date: date.into(), by: "A. Designer".into(), note: note.into() };
    TitleInfo {
        title: "Sensor board".into(),
        revision: "B".into(),
        date: "2026-10-30".into(),
        author: "A. Designer".into(),
        company: "Workshop".into(),
        notes: "Two stages".into(),
        revisions: vec![rev("A", "2026-09-01", "First prototype"), rev("B", "2026-10-30", "Second stage added")],
    }
}

fn options() -> Options {
    Options {
        pages: Vec::new(),
        frames: None,
        paper: None,
        orientation: None,
        style: Style::Paper,
        power_xrefs: false,
        file_name: "sensor.solidcraft".into(),
        info: info(),
    }
}

fn book(o: &Options) -> Book {
    compose(&design(), o).unwrap()
}

/// Every text a page draws, clipped groups' too, in drawing order.
fn texts(page: &Page) -> Vec<Text> {
    fn walk(items: &[Item], out: &mut Vec<Text>) {
        for item in items {
            match item {
                Item::Text(t) => out.push(t.clone()),
                Item::Clip { items, .. } => walk(items, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    for layer in &page.layers {
        walk(&layer.items, &mut out);
    }
    out
}

fn has_text(page: &Page, wanted: &str) -> bool {
    texts(page).iter().any(|t| t.text.contains(wanted))
}

fn find_all(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    hay.windows(needle.len()).enumerate().filter(|(_, w)| *w == needle).map(|(i, _)| i).collect()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !find_all(hay, needle).is_empty()
}

#[test]
fn the_book_runs_cover_sections_block_diagram_parts_board_then_net_index() {
    let b = book(&options());
    let kinds: Vec<(PageKind, Option<u64>)> = b.entries.iter().map(|e| (e.kind, e.frame)).collect();
    use PageKind::*;
    assert_eq!(
        kinds,
        [
            (Cover, None),
            (Sections, Some(20)),
            (Sections, Some(21)),
            (BlockDiagram, None),
            (Parts, None),
            (BoardTop, None),
            (BoardBottom, None),
            (InnerLayers, None),
            (InnerLayers, None),
            (AssemblyTop, None),
            (AssemblyBottom, None),
            (Stackup, None),
        ]
    );
    assert_eq!(b.entries.iter().map(|e| e.number).collect::<Vec<_>>(), (1..=12).collect::<Vec<_>>());
    assert_eq!(b.pages.len(), 12);
    let titles: Vec<&str> = b.entries.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(&titles[..5], ["Cover", "Input", "Output", "Block diagram", "Parts list"]);
    assert_eq!(&titles[7..9], ["Inner layer 1", "Inner layer 2"]);
    // Small frames: one A4 landscape book, each section scaled for its text.
    assert_eq!(b.paper, Paper::A4);
    assert!(b.entries.iter().all(|e| e.paper == Paper::A4 && e.orientation == Orientation::Landscape));
    assert!(b.pages.iter().all(|p| p.size == [297.0, 210.0]), "{:?}", b.pages.iter().map(|p| p.size).collect::<Vec<_>>());
    assert!(b.entries[1].scale.is_some() && b.entries[2].scale.is_some());
    assert_eq!((b.title.as_str(), b.author.as_str()), ("Sensor board", "A. Designer (Workshop)"));
    // Every page has its title block: the title, the page's own number of all of them.
    for (k, page) in b.pages.iter().enumerate() {
        assert!(has_text(page, "Sensor board"), "page {}", k + 1);
        assert!(has_text(page, &format!("{} of 12", k + 1)), "page {} of 12", k + 1);
    }
    // Nothing cut short and no rails cross-referenced: no net index.
    assert!(!b.entries.iter().any(|e| e.kind == NetIndex));
}

#[test]
fn a_paper_and_orientation_given_hold_for_every_page_and_a_tall_frame_turns_upright_by_itself() {
    for (paper, orientation, size) in [
        (Paper::A3, Orientation::Portrait, [297.0, 420.0]),
        (Paper::Letter, Orientation::Landscape, [279.4, 215.9]),
        (Paper::Tabloid, Orientation::Landscape, [431.8, 279.4]),
    ] {
        let b = book(&Options { paper: Some(paper), orientation: Some(orientation), ..options() });
        assert_eq!(b.paper, paper);
        assert!(b.pages.iter().all(|p| p.size == size), "{}: {:?}", paper.name(), b.pages.iter().map(|p| p.size).collect::<Vec<_>>());
        assert!(b.entries.iter().all(|e| e.paper == paper && e.orientation == orientation), "{}", paper.name());
    }
    // A frame much taller than wide prints larger upright, so its page turns; the wide one stays.
    let mut e = design();
    e.schematic.frames[1].rect = [[85.0, -200.0], [135.0, 15.0]];
    let b = compose(&e, &Options { pages: vec![PageKind::Sections], ..options() }).unwrap();
    let orientation = |frame: u64| b.entries.iter().find(|x| x.frame == Some(frame)).map(|x| x.orientation);
    assert_eq!((orientation(20), orientation(21)), (Some(Orientation::Landscape), Some(Orientation::Portrait)));
    let tall = b.entries.iter().position(|x| x.frame == Some(21)).unwrap();
    assert!(b.pages[tall].size[1] > b.pages[tall].size[0]);
}

#[test]
fn the_cover_lists_every_page_and_the_revisions_newest_first() {
    let b = book(&options());
    let cover = &b.pages[0];
    assert_eq!(cover.title, "Cover");
    for wanted in ["Sensor board", "Two stages", "Workshop", "2026-10-30", "First prototype", "Second stage added"] {
        assert!(has_text(cover, wanted), "the cover shows {wanted}");
    }
    // The contents: a row per page, each a link to its page.
    for entry in &b.entries[1..] {
        assert!(has_text(cover, &entry.title), "the contents list {}", entry.title);
    }
    let targets: std::collections::BTreeSet<usize> = cover.links.iter().map(|l| l.page).collect();
    assert_eq!(targets, (1..b.pages.len()).collect(), "a link to every other page");
    // Newest first: B's note above A's.
    let y = |note: &str| texts(cover).iter().find(|t| t.text.contains(note)).map(|t| t.at[1]).unwrap();
    assert!(y("Second stage added") < y("First prototype"));
    // The board's picture and what it shows.
    assert!(has_text(cover, "board 40.0 × 25.0 mm, 4 copper layers, 4 parts fitted"));
}

#[test]
fn a_net_on_two_frames_refers_each_page_to_the_other() {
    let b = book(&options());
    let (input, output) = (&b.pages[1], &b.pages[2]);
    assert!(has_text(input, "SIG") && has_text(output, "SIG"));
    // The list starts with an arrow and names the other page and its zone (`3·C4`).
    let xref = |page: &Page, to: usize| texts(page).iter().any(|t| t.text.contains(&format!("{to}·")) && t.text.starts_with(['→', '←']));
    assert!(xref(input, 3), "Input refers to Output's page");
    assert!(xref(output, 2), "Output refers to Input's page");
    assert!(input.links.iter().any(|l| l.page == 2 && l.at.is_some()), "a link to the place on Output's page");
    assert!(output.links.iter().any(|l| l.page == 1 && l.at.is_some()));
    // Only SIG: rails are not cross-referenced unless asked, ground never.
    assert_eq!(texts(input).iter().filter(|t| t.text.starts_with(['→', '←'])).count(), 1);
    // The part not fitted is struck out, and its page says what the mark means.
    assert!(has_text(output, "DNP"));
    assert!(has_text(output, parts::DNP_KEY.trim_start_matches("DNP").trim_start()));
    assert!(!has_text(input, "DNP"));
}

#[test]
fn power_cross_references_add_a_net_index_at_the_end_with_ground_in_it() {
    let b = book(&Options { power_xrefs: true, ..options() });
    let last = b.entries.last().unwrap();
    assert_eq!((last.kind, last.number, last.title.as_str()), (PageKind::NetIndex, 13, "Net index"));
    assert_eq!(b.entries[..12].iter().map(|e| e.kind).collect::<Vec<_>>(), book(&options()).entries.iter().map(|e| e.kind).collect::<Vec<_>>());
    let index = b.pages.last().unwrap();
    for net in ["SIG", "+3V3", "GND"] {
        assert!(has_text(index, net), "the index lists {net}");
    }
    // Its rows link to the section pages.
    let targets: std::collections::BTreeSet<usize> = index.links.iter().map(|l| l.page).collect();
    assert!(targets.contains(&1) && targets.contains(&2), "{targets:?}");
    // The rail is cross-referenced on the section pages now (SIG and +3V3 on Input; ground is
    // still only in the index).
    let lists = |page: &Page| texts(page).iter().filter(|t| t.text.starts_with(['→', '←'])).count();
    assert_eq!(lists(&b.pages[1]), 2);
    assert_eq!(lists(&b.pages[2]), 2, "Output too: SIG and +3V3, not GND");
}

#[test]
fn part_of_the_book_keeps_the_whole_books_page_numbers_and_only_its_links() {
    let o = Options { pages: vec![PageKind::Sections], frames: Some(vec![21]), ..options() };
    let b = book(&o);
    assert_eq!(b.pages.len(), 1);
    assert_eq!((b.entries[0].number, b.entries[0].frame), (3, Some(21)));
    assert!(has_text(&b.pages[0], "3 of 12"), "page 3 of the whole book's 12");
    assert!(b.pages[0].links.is_empty(), "the Input page is not in this export, so its reference is no link");
    assert!(texts(&b.pages[0]).iter().any(|t| t.text.contains("2·")), "but it still names the page");
    assert!(b.warnings.iter().any(|w| w.contains("keeps its number in the whole book of 12 pages")), "{:?}", b.warnings);
    // The PDF shows the whole book's number as the page's label.
    let pdf = write_pdf(&b).unwrap();
    assert!(contains(&pdf, b"/PageLabels"));
    assert!(!contains(&write_pdf(&book(&options())).unwrap(), b"/PageLabels"), "the whole book counts from 1");
}

#[test]
fn what_cannot_print_is_refused_or_warned_about() {
    assert!(compose(&Electronics::default(), &options()).unwrap_err().contains("nothing to print"));
    assert!(compose(&design(), &Options { frames: Some(vec![99]), ..options() }).unwrap_err().contains("no frame 99"));
    // Asking only for a page this design has none of.
    let mut e = design();
    e.board = Default::default();
    let err = compose(&e, &Options { pages: vec![PageKind::BoardTop], ..options() }).unwrap_err();
    assert!(err.contains("none of the pages asked for applies"), "{err}");
    let b = compose(&e, &Options { pages: vec![PageKind::Parts, PageKind::BoardTop], ..options() }).unwrap();
    assert!(b.warnings.iter().any(|w| w.contains("no board yet")), "{:?}", b.warnings);
    assert_eq!(b.entries.iter().map(|e| e.kind).collect::<Vec<_>>(), [PageKind::Parts]);
    // One frame makes no block diagram.
    e.schematic.frames.truncate(1);
    let b = compose(&e, &Options { pages: vec![PageKind::BlockDiagram, PageKind::Parts], ..options() }).unwrap();
    assert!(b.warnings.iter().any(|w| w.contains("block diagram needs two frames")), "{:?}", b.warnings);
    // Parts outside every frame get a page of their own, and a warning names them.
    let b = compose(&e, &Options { pages: vec![PageKind::Sections], ..options() }).unwrap();
    assert_eq!(b.entries.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Input", "Other parts"]);
    assert!(b.warnings.iter().any(|w| w.contains("3 parts are in no frame (R3, R4, R5)")), "{:?}", b.warnings);
    // A sheet without frames prints as tiles.
    e.schematic.frames.clear();
    let b = compose(&e, &Options { pages: vec![PageKind::Sections], ..options() }).unwrap();
    assert!(!b.pages.is_empty() && b.entries.iter().all(|e| e.kind == PageKind::Sections && e.frame.is_none()));
    assert!(b.warnings.iter().any(|w| w.contains("has no frames")), "{:?}", b.warnings);
}

#[test]
fn a_book_of_more_pages_than_one_export_makes_is_refused_before_drawing() {
    let mut e = design();
    let frames: Vec<_> = (0..MAX_PAGES as u64)
        .map(|k| solidcraft_doc::ecad::Frame { id: 1000 + k, title: format!("F{k}"), rect: [[-15.0, -15.0], [35.0, 15.0]], order: None })
        .collect();
    e.schematic.frames = frames;
    let started = std::time::Instant::now();
    let err = compose(&e, &options()).unwrap_err();
    assert!(err.contains(&format!("more than the {MAX_PAGES} one export makes")), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(60));
}

#[test]
fn the_pdf_is_the_same_bytes_every_time_and_its_id_follows_the_content() {
    let o = Options { power_xrefs: true, ..options() };
    let first = write_pdf(&book(&o)).unwrap();
    assert_eq!(book(&o), book(&o), "the pages are worked out the same way each time");
    assert_eq!(first, write_pdf(&book(&o)).unwrap(), "no dates, no random id");
    assert!(!contains(&first, b"/CreationDate") && !contains(&first, b"/ModDate"));
    let trailer = |pdf: &[u8]| pdf[*find_all(pdf, b"trailer").last().unwrap()..].to_vec();
    let mut retitled = o.clone();
    retitled.info.title = "Sensor board v2".into();
    let other = write_pdf(&book(&retitled)).unwrap();
    assert_ne!(trailer(&first), trailer(&other), "another title, another file id");
    // Every style gives the same bytes again, each its own.
    for style in [Style::Monochrome, Style::Dark] {
        let o = Options { style, ..options() };
        let pdf = write_pdf(&book(&o)).unwrap();
        assert_eq!(pdf, write_pdf(&book(&o)).unwrap(), "{}", style.name());
        assert_ne!(pdf, first);
    }
}

#[test]
fn the_pdf_has_a_page_per_page_a_bookmark_each_its_links_and_a_valid_cross_reference_table() {
    let b = book(&Options { power_xrefs: true, ..options() });
    let pdf = write_pdf(&b).unwrap();
    assert!(pdf.starts_with(b"%PDF-1."));
    assert!(pdf.ends_with(b"%%EOF"));
    // `startxref` gives where the table is, and each object it lists is at its offset.
    let at = *find_all(&pdf, b"startxref").last().unwrap();
    let offset: usize = std::str::from_utf8(&pdf[at + 9..]).unwrap().split_whitespace().next().unwrap().parse().unwrap();
    assert!(pdf[offset..].starts_with(b"xref\n0 "));
    let table = std::str::from_utf8(&pdf[offset..at]).unwrap();
    let mut lines = table.lines().skip(1);
    let count: usize = lines.next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut objects = 0;
    for (id, line) in lines.take(count).enumerate() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.get(2) == Some(&"n") {
            let at: usize = fields[0].parse().unwrap();
            assert!(pdf[at..].starts_with(format!("{id} 0 obj").as_bytes()), "object {id} is where the table says");
            objects += 1;
        }
    }
    assert!(objects > b.pages.len() * 3, "pages, their contents and bookmarks, and the font");
    // A page object per page (not counting the page tree), one link annotation per link.
    let pages = find_all(&pdf, b"/Type /Page").into_iter().filter(|&i| pdf.get(i + 11) != Some(&b's')).count();
    assert_eq!(pages, b.pages.len());
    assert!(contains(&pdf, format!("/Count {}", b.pages.len()).as_bytes()));
    let links: usize = b.pages.iter().map(|p| p.links.len()).sum();
    assert!(links > b.pages.len(), "the contents and the cross-references link");
    assert_eq!(find_all(&pdf, b"/Subtype /Link").len(), links);
    // A bookmark per page, by number and title; the document's title and author.
    for (k, e) in b.entries.iter().enumerate() {
        assert!(contains(&pdf, format!("({} {})", k + 1, e.title).as_bytes()), "bookmark {} {}", k + 1, e.title);
    }
    assert!(contains(&pdf, b"/Title (Sensor board)") && contains(&pdf, b"/Author (A. Designer (Workshop))"));
    // The text is in the embedded font, cut down to the glyphs used, with a Unicode map.
    assert!(contains(&pdf, b"/FontFile2") && contains(&pdf, b"/ToUnicode") && contains(&pdf, b"/Identity-H"));
    // An A4 landscape page in points.
    assert!(contains(&pdf, b"/MediaBox [0 0 841.8"), "{}", String::from_utf8_lossy(&pdf[..pdf.len().min(4000)]));
}

#[test]
fn a_character_the_font_lacks_prints_as_a_question_mark_and_is_warned_about() {
    let mut o = options();
    o.info.title = "Sensor ⌁ board".into();
    let b = book(&o);
    assert!(b.warnings.iter().any(|w| w.contains("no glyph for ⌁")), "{:?}", b.warnings);
    // It still copies as written: the title goes in as UTF-16.
    let pdf = write_pdf(&b).unwrap();
    assert!(pdf.starts_with(b"%PDF-") && pdf.ends_with(b"%%EOF"));
    assert_eq!(pdf, write_pdf(&book(&o)).unwrap());
}

#[test]
fn each_page_is_an_svg_that_parses_with_its_size_layers_and_links_to_the_other_files() {
    let b = book(&Options { power_xrefs: true, ..options() });
    let file_of = |k: usize| format!("sensor-{:02}.svg", k + 1);
    for (k, page) in b.pages.iter().enumerate() {
        let svg = write_svg(&b, k, &file_of).unwrap();
        assert_eq!(svg, write_svg(&b, k, &file_of).unwrap(), "page {}", k + 1);
        assert!(svg.starts_with("<svg ") && svg.trim_end().ends_with("</svg>"));
        assert!(svg.contains(r#"width="297mm" height="210mm" viewBox="0 0 297 210""#), "page {}", k + 1);
        assert!(svg.contains(&format!("<title>{}</title>", page.title)));
        let mut reader = quick_xml::Reader::from_str(&svg);
        let (mut depth, mut groups, mut hrefs) = (0i32, 0usize, Vec::new());
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Start(e)) => {
                    depth += 1;
                    if e.name().as_ref() == b"g" {
                        groups += 1;
                    }
                    if e.name().as_ref() == b"a"
                        && let Some(href) = e.try_get_attribute("href").unwrap()
                    {
                        hrefs.push(String::from_utf8(href.value.to_vec()).unwrap());
                    }
                }
                Ok(quick_xml::events::Event::End(_)) => depth -= 1,
                Ok(quick_xml::events::Event::Eof) => break,
                Ok(_) => {}
                Err(e) => panic!("page {}: {e}", k + 1),
            }
        }
        assert_eq!(depth, 0, "page {}: every element closed", k + 1);
        assert!(groups >= page.layers.len(), "a group per layer");
        let expected: Vec<String> = page.links.iter().map(|l| file_of(l.page)).collect();
        assert_eq!(hrefs, expected, "page {}", k + 1);
    }
    // Text stays text, in the page font.
    let input = write_svg(&b, 1, &file_of).unwrap();
    assert!(input.contains(">SIG<") && input.contains("sensor-03.svg"));
    assert!(write_svg(&b, b.pages.len(), &file_of).unwrap_err().contains("no page 14"));
}

#[test]
fn each_page_is_a_png_of_the_size_its_resolution_gives_and_too_many_pixels_are_refused() {
    let b = book(&options());
    let png = write_png(&b, 0, 72.0).unwrap();
    assert_eq!(png, write_png(&b, 0, 72.0).unwrap(), "the same pixels every time");
    let mut reader = ::png::Decoder::new(std::io::Cursor::new(&png)).read_info().unwrap();
    let info = reader.info();
    assert_eq!((info.width, info.height), pixels(&b.pages[0], 72.0));
    assert_eq!((info.width, info.height), (842, 595), "A4 landscape at 72 dpi");
    assert_eq!((info.color_type, info.bit_depth), (::png::ColorType::Rgb, ::png::BitDepth::Eight));
    let dims = info.pixel_dims.unwrap();
    assert_eq!((dims.xppu, dims.yppu, dims.unit), (2835, 2835, ::png::Unit::Meter), "72 dpi saved in the file");
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut buf).unwrap();
    // A white page with ink on it.
    assert_eq!(&buf[..3], [255, 255, 255]);
    assert!(buf.chunks(3).any(|p| p != [255, 255, 255]));
    // The preview draws the same pixels, with an opaque alpha.
    let ([w, h], rgba) = write_rgba(&b, 0, 72.0).unwrap();
    assert_eq!(((w, h), rgba.len()), ((842, 595), 842 * 595 * 4));
    assert!(rgba.chunks(4).zip(buf.chunks(3)).all(|(a, b)| a[..3] == *b && a[3] == 255));
    // Out of range, or more pixels than a page may have.
    for dpi in [35.0, 1201.0, f64::NAN, f64::INFINITY, -72.0] {
        assert!(write_png(&b, 0, dpi).unwrap_err().contains("36 to 1200 dpi"), "{dpi}");
    }
    let (w, h) = pixels(&b.pages[0], 1200.0);
    assert!(u64::from(w) * u64::from(h) > MAX_PIXELS);
    assert!(write_png(&b, 0, 1200.0).unwrap_err().contains("choose a lower resolution"));
    assert!(write_png(&b, 99, 72.0).unwrap_err().contains("no page 100"));
}

#[test]
fn the_parts_list_csv_groups_like_parts_lists_every_designator_and_keeps_formulas_out() {
    let mut e = design();
    let csv = parts_csv(&e);
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "Qty,Designators,Value,Part,Footprint,Manufacturer,MPN,Description,Fitted");
    assert_eq!(lines[1], r#"4,"R1, R2, R3, R4",10k,R0603,R0603,Yageo,RC0603FR-0710KL,Thick film resistor,yes"#);
    assert_eq!(lines[2], "1,R5,10k,R0603,R0603,Yageo,RC0603FR-0710KL,Thick film resistor,no", "the part not fitted last, on its own");
    assert_eq!(lines.len(), 3);
    assert_eq!(parts_rows(&e), lines.len() - 1);
    assert!(csv.is_ascii(), "no byte order mark when it is all ASCII");
    // The printed list shows the run as a range.
    let b = compose(&e, &Options { pages: vec![PageKind::Parts], ..options() }).unwrap();
    assert!(has_text(&b.pages[0], "R1–R4"));
    // A spreadsheet never reads a cell as a formula; a number is a number; Ω gets the mark.
    e.components[0].mpn = "=HYPERLINK(\"x\")".into();
    e.components[0].manufacturer = "-5".into();
    e.components[0].description = "10 kΩ, 1 %".into();
    e.schematic.parts[0].value = Some("+1V".into());
    let csv = parts_csv(&e);
    assert!(csv.starts_with('\u{feff}'));
    let lines: Vec<&str> = csv.trim_start_matches('\u{feff}').lines().collect();
    assert_eq!(lines[1], "1,R1,'+1V,R0603,R0603,-5,\"'=HYPERLINK(\"\"x\"\")\",\"10 kΩ, 1 %\",yes");
    assert_eq!(lines[2], "3,\"R2, R3, R4\",10k,R0603,R0603,-5,\"'=HYPERLINK(\"\"x\"\")\",\"10 kΩ, 1 %\",yes");
    assert_eq!(parts_csv(&Electronics::default()), "Qty,Designators,Value,Part,Footprint,Manufacturer,MPN,Description,Fitted\n");
}

#[test]
fn the_board_pages_show_each_copper_layer_the_assembly_and_the_stack_up() {
    let b = book(&options());
    let page = |kind: PageKind| b.entries.iter().position(|e| e.kind == kind).map(|k| &b.pages[k]).unwrap();
    assert!(has_text(page(PageKind::BoardTop), "Top copper"));
    assert!(has_text(page(PageKind::BoardBottom), "Seen from the bottom (mirrored)"));
    let inner: Vec<&Page> = b.entries.iter().zip(&b.pages).filter(|(e, _)| e.kind == PageKind::InnerLayers).map(|(_, p)| p).collect();
    assert!(has_text(inner[0], "Inner layer 1:") && has_text(inner[1], "Inner layer 2:"));
    let top = page(PageKind::AssemblyTop);
    for d in ["R1", "R2", "R3", "R4", "R5"] {
        assert!(has_text(top, d), "the assembly names {d}");
    }
    // The board pages state their scale; the stack-up is a cross-section with no scale.
    for e in &b.entries {
        assert_eq!(e.scale.is_some(), e.kind != PageKind::Stackup && e.kind.is_board() || e.kind == PageKind::Sections, "{:?}", e.kind);
    }
    // Two layers: no inner pages, and asking for them says why.
    let mut e = design();
    e.board.copper_layers = 2;
    let b = compose(&e, &Options { pages: vec![PageKind::BoardTop, PageKind::InnerLayers], ..options() }).unwrap();
    assert_eq!(b.entries.iter().map(|e| e.kind).collect::<Vec<_>>(), [PageKind::BoardTop]);
    assert!(b.warnings.iter().any(|w| w.contains("no inner copper layers")), "{:?}", b.warnings);
}

#[test]
fn the_board_pages_print_the_names_the_silkscreen_mode_left_and_the_assembly_page_every_name() {
    use solidcraft_doc::ecad::labels::{NamesMode, layout};
    let mut e = design();
    let names = |e: &Electronics, kind: PageKind| {
        let b = compose(e, &Options { pages: vec![kind], ..options() }).unwrap();
        let page = b.pages.first().unwrap().clone();
        ["R1", "R2", "R3", "R4"].into_iter().filter(|d| texts(&page).iter().any(|t| t.text == *d)).count()
    };
    assert_eq!(names(&e, PageKind::BoardTop), 4, "every name shows before");
    e.board = layout(&e, NamesMode::None, None).unwrap().board;
    assert_eq!(names(&e, PageKind::BoardTop), 0, "none: no name on the silkscreen page");
    assert_eq!(names(&e, PageKind::AssemblyTop), 4, "the assembly page keeps every name");
    let fit = layout(&design(), NamesMode::Fit, None).unwrap();
    e.board = fit.board.clone();
    assert_eq!(names(&e, PageKind::BoardTop), fit.placed.iter().filter(|d| d.starts_with('R') && d.len() == 2 && d.as_str() != "R5").count());
}
