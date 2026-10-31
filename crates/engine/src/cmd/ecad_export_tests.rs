//! The printed pages of the test board (`ecad::tests::board`): one frame, Main, a parts list of
//! three rows and a four-layer board.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;
use crate::cmd::ecad::tests::{board, run};

fn refused(s: &mut Session, id: &str, p: Value) -> String {
    let before = s.doc.clone();
    let Err(e) = s.execute(id, &p) else { panic!("{id} {p} must be refused") };
    assert!(Arc::ptr_eq(&before, &s.doc), "a refused {id} leaves the design as it was");
    e.to_string()
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("solidcraft-ecad-export-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn kinds(v: &Value) -> Vec<String> {
    v["pages"].as_array().unwrap().iter().map(|p| p["kind"].as_str().unwrap().to_string()).collect()
}

#[test]
fn the_book_lists_every_page_that_applies_in_order() {
    let mut s = board();
    let (doc, undo) = (s.doc.clone(), s.undo.len());
    let book = run(&mut s, "ecad.export.pages.inspect", json!({}));
    assert!(Arc::ptr_eq(&doc, &s.doc) && s.undo.len() == undo, "it only reads");
    assert_eq!(
        kinds(&book),
        ["cover", "sections", "parts", "board_top", "board_bottom", "inner_layers", "inner_layers", "assembly_top", "assembly_bottom", "stackup"],
        "one page per frame, one per inner layer"
    );
    assert_eq!(book["count"], 10);
    let numbers: Vec<u64> = book["pages"].as_array().unwrap().iter().filter_map(|p| p["page"].as_u64()).collect();
    assert_eq!(numbers, (1..=10).collect::<Vec<u64>>());
    assert_eq!((book["paper"].clone(), book["style"].clone()), (json!("a4"), json!("paper")), "a small sheet fits A4");
    assert_eq!(book["pages"][1]["title"], "Main");
    assert_eq!(book["pages"][0]["size"], json!([297.0, 210.0]), "landscape A4 in mm");
    // What prints keeps its numbers in the whole book, and says so.
    let one = run(&mut s, "ecad.export.pages.inspect", json!({"pages": ["parts"], "paper": "letter", "orientation": "portrait", "style": "dark"}));
    assert_eq!((one["count"].clone(), one["pages"][0]["page"].clone()), (json!(1), json!(3)));
    assert_eq!(one["pages"][0]["size"], json!([215.9, 279.4]));
    assert!(one["warnings"][0].as_str().is_some_and(|w| w.contains("whole book of 10 pages")), "{one}");
    let framed = run(&mut s, "ecad.export.pages.inspect", json!({"pages": ["sections"], "frames": ["Main"]}));
    assert_eq!(framed["pages"][0]["frame"], 16);
    for (p, says) in [
        (json!({"pages": ["poster"]}), "`pages"),
        (json!({"frames": ["Nope"]}), "Nope"),
        (json!({"paper": "a0"}), "`paper`"),
        (json!({"format": "png", "dpi": 10}), "`dpi` must be 36 to 1200"),
    ] {
        let e = refused(&mut s, "ecad.export.pages.inspect", p.clone());
        assert!(e.contains(says), "{p}: {e}");
    }
}

#[test]
fn a_pdf_is_the_same_bytes_every_time() {
    let mut s = board();
    let dir = temp_dir("pdf");
    // Saved first: the title block prints the design's name, which a save gives it.
    let saved = dir.join("board.sc");
    run(&mut s, "file.save", json!({"path": saved}));
    let (a, b) = (dir.join("a.pdf"), dir.join("b.pdf"));
    let r = run(&mut s, "ecad.export.pages", json!({"format": "pdf", "path": a}));
    assert_eq!(r["files"][0]["pages"], json!((1..=10).collect::<Vec<u64>>()));
    run(&mut s, "ecad.export.pages", json!({"format": "pdf", "path": b}));
    let (a, b) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    assert!(a.starts_with(b"%PDF-"), "a PDF");
    assert_eq!(r["files"][0]["bytes"].as_u64(), Some(a.len() as u64));
    assert!(a == b, "the same design prints the same bytes: no clock, no random ids");
    // Opened again, it prints the same.
    let mut t = Session::default();
    run(&mut t, "doc.open", json!({"path": saved}));
    let c = dir.join("c.pdf");
    run(&mut t, "ecad.export.pages", json!({"format": "pdf", "path": c}));
    assert!(std::fs::read(&c).unwrap() == a, "the saved design prints the same");
    // The title block's fields print as set; a new revision changes the book.
    run(&mut s, "ecad.design.info", json!({"revision": "B"}));
    run(&mut s, "ecad.export.pages", json!({"format": "pdf", "path": dir.join("d.pdf")}));
    assert!(std::fs::read(dir.join("d.pdf")).unwrap() != a);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_parts_list_exports_as_csv_and_pages_as_svg() {
    let mut s = board();
    let dir = temp_dir("csv");
    let path = dir.join("parts.csv");
    let r = run(&mut s, "ecad.export.pages", json!({"format": "csv", "path": path}));
    assert_eq!(r["files"][0]["rows"], 3);
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        [
            "Qty,Designators,Value,Part,Footprint,Manufacturer,MPN,Description,Fitted",
            "1,C1,100nF,C0603,R0603,,,,yes",
            "2,\"J1, J2\",,CONN2,HDR2,,,,yes",
            "2,\"R1, R2\",10k,R0603,R0603,,,,yes"
        ]
    );
    // A part not fitted is listed apart.
    run(&mut s, "ecad.schematic.edit", json!({"parts": [{"part": "R2", "dnp": true}]}));
    run(&mut s, "ecad.export.pages", json!({"format": "csv", "path": path}));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("1,R1,10k,R0603,R0603,,,,yes") && text.contains("1,R2,10k,R0603,R0603,,,,no"), "{text}");
    // SVG pages go to files named by the stem, their numbers in the whole book and their titles.
    let r = run(&mut s, "ecad.export.pages", json!({"format": "svg", "pages": ["cover", "sections"], "path": dir.join("t")}));
    let names: Vec<String> =
        r["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().rsplit('/').next().unwrap().to_string()).collect();
    assert_eq!(names, ["t-01-cover.svg", "t-02-main.svg"]);
    assert!(
        std::fs::read_to_string(dir.join("t-02-main.svg")).unwrap().starts_with("<svg")
            || std::fs::read_to_string(dir.join("t-02-main.svg")).unwrap().contains("<svg")
    );
    let e = refused(&mut s, "ecad.export.pages", json!({"format": "pdf"}));
    assert!(e.contains("`path`"), "{e}");
    let e = refused(&mut s, "ecad.export.pages", json!({"format": "pdf", "path": dir.join("x.svg")}));
    assert!(e.contains(".pdf"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_png_past_the_pixel_cap_is_refused_and_inspect_says_what_fits() {
    let mut s = board();
    // A4 at 1200 dpi: 14031 x 9921 pixels, past the 80 million a page may have.
    let book = run(&mut s, "ecad.export.pages.inspect", json!({"format": "png", "dpi": 1200}));
    assert_eq!(book["pages"][0]["pixels"], json!([14031, 9921]));
    let warned = book["warnings"].as_array().unwrap().iter().any(|w| w.as_str().is_some_and(|w| w.contains("80000000") && w.contains("`dpi`")));
    assert!(warned, "{book}");
    let dir = temp_dir("png");
    let e = refused(&mut s, "ecad.export.pages", json!({"format": "png", "dpi": 1200, "pages": ["stackup"], "path": dir.join("p")}));
    assert!(e.contains("more than 80000000"), "{e}");
    assert!(std::fs::read_dir(&dir).unwrap().next().is_none(), "nothing written");
    let r = run(&mut s, "ecad.export.pages", json!({"format": "png", "dpi": 50, "pages": ["stackup"], "path": dir.join("p")}));
    let png = std::fs::read(r["files"][0]["path"].as_str().unwrap()).unwrap();
    assert!(png.starts_with(b"\x89PNG"), "a PNG");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_title_block_keeps_what_is_set_and_never_the_clock() {
    let mut s = board();
    let info = run(&mut s, "ecad.design.info.inspect", json!({}));
    assert_eq!(
        (info["info"]["title"].clone(), info["info"]["revision"].clone(), info["info"]["date"].clone()),
        (json!("Test board"), json!("A"), json!("2026-10-30"))
    );
    let undo = s.undo.len();
    let r = run(&mut s, "ecad.design.info", json!({"author": "", "revisions": [{"rev": "A", "date": "2026-10-30", "note": "first"}]}));
    assert_eq!(r["changed"], json!(["author", "revisions"]));
    assert_eq!(s.undo.len(), undo + 1);
    assert_eq!(r["info"]["author"], "", "an empty text clears it");
    let r = run(&mut s, "ecad.design.info", json!({"title": "Test board"}));
    assert!(r["changed"].as_array().is_none_or(Vec::is_empty) && s.undo.len() == undo + 1, "the same again changes nothing: {r}");
    let e = refused(&mut s, "ecad.design.info", json!({"title": "x".repeat(500)}));
    assert!(e.contains("title"), "{e}");
    let e = refused(&mut s, "ecad.design.info", json!({"notes": "line\u{0}break"}));
    assert!(e.contains("notes"), "{e}");
}
