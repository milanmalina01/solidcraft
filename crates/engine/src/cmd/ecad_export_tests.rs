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

#[test]
fn the_fab_files_read_back_as_the_board_and_the_assembly_lists_match_its_parts() {
    use solidcraft_io::fab;
    let mut s = board();
    run(&mut s, "ecad.component.edit", json!({"component": "R0603", "lcsc": "C25804"}));
    let r = run(&mut s, "ecad.board.cpl_rotation", json!({"footprint": "R0603", "offset": 90}));
    assert_eq!(r["offsets"], json!({"R0603": 90.0}));
    let kept = s.doc.ecad.footprints.iter().find(|f| f.name == "R0603").map(|f| f.assembly_rotation);
    assert_eq!(kept, Some(90.0), "the offset is the library footprint's, so it travels with it");
    let r = run(&mut s, "ecad.board.cpl_rotation", json!({"footprint": "R0603", "offset": 270}));
    assert_eq!(r["offsets"], json!({"R0603": -90.0}), "kept as -180 to 180");
    run(&mut s, "ecad.board.cpl_rotation", json!({"footprint": "R0603", "offset": 90}));
    let dir = temp_dir("jlc");
    let zip = dir.join("board.zip");
    let (doc, undo) = (s.doc.clone(), s.undo.len());
    let r = run(&mut s, "ecad.board.export.jlc", json!({"path": zip.to_string_lossy()}));
    assert!(Arc::ptr_eq(&doc, &s.doc) && s.undo.len() == undo, "exporting changes nothing");
    assert_eq!(r["verified"]["ok"], true, "{r}");
    // The zip, read back with the readers: four copper layers, the masks, pastes, legends and
    // profile, and every hole.
    let files = fab::unzip(&std::fs::read(&zip).unwrap(), 64, 1 << 28).unwrap();
    let mut functions = Vec::new();
    for f in files.iter().filter(|f| !f.name.ends_with(".drl")) {
        let image = fab::read::gerber(std::str::from_utf8(&f.bytes).unwrap()).unwrap_or_else(|e| panic!("{}: {e}", f.name));
        functions.push(image.function.clone());
        if image.function.starts_with("Copper") || image.function.starts_with("Profile") {
            assert!(image.objects > 0 && !image.region.is_empty(), "{} draws something", f.name);
        }
    }
    functions.sort();
    let expected = [
        "Copper,L1,Top",
        "Copper,L2,Inr",
        "Copper,L3,Inr",
        "Copper,L4,Bot",
        "Legend,Bot",
        "Legend,Top",
        "Paste,Bot",
        "Paste,Top",
        "Profile,NP",
        "Soldermask,Bot",
        "Soldermask,Top",
    ];
    assert_eq!(functions, expected);
    let nets = s.board_nets();
    let (plated, _) = fab::drill::holes(&s.doc.ecad, &nets);
    let drilled: usize = files
        .iter()
        .filter(|f| f.name.ends_with("-PTH.drl"))
        .map(|f| fab::read::excellon(std::str::from_utf8(&f.bytes).unwrap()).unwrap().len())
        .sum();
    assert!(drilled == plated.len() && drilled > 0, "{drilled} of {}", plated.len());
    // The profile is the 30 x 20 mm outline, stroked 0.1 mm.
    let profile = files.iter().find(|f| f.name.ends_with(".GKO")).unwrap();
    let [lo, hi] = fab::read::gerber(std::str::from_utf8(&profile.bytes).unwrap()).unwrap().region.bbox().unwrap();
    assert!((hi[0] - lo[0] - 30.1).abs() < 0.01 && (hi[1] - lo[1] - 20.1).abs() < 0.01, "{lo:?} {hi:?}");
    // The parts list: a row per value, footprint and LCSC part; every placed part once.
    let bom = std::fs::read_to_string(dir.join("board-bom.csv")).unwrap();
    let mut lines = bom.lines();
    assert_eq!(lines.next(), Some("Comment,Designator,Footprint,LCSC Part #"));
    let rows: Vec<&str> = lines.collect();
    assert!(rows.contains(&"10k,\"R1,R2\",R0603,C25804"), "{bom}");
    assert_eq!(r["bom_rows"].as_u64(), Some(rows.len() as u64));
    let placed: Vec<String> = s.doc.ecad.board.footprints.iter().filter_map(|f| s.doc.ecad.placed(f)).map(|p| p.part.designator.clone()).collect();
    let mut listed: Vec<String> = Vec::new();
    for row in &rows {
        let designators = if let Some(rest) = row.split_once(",\"") {
            rest.1.split('"').next().unwrap().to_string()
        } else {
            row.split(',').nth(1).unwrap().to_string()
        };
        listed.extend(designators.split(',').map(str::to_string));
    }
    listed.sort();
    let mut want = placed.clone();
    want.sort();
    assert_eq!(listed, want, "each placed part once");
    let missing: Vec<String> = serde_json::from_value(r["missing_lcsc"].clone()).unwrap();
    assert!(!missing.is_empty() && missing.iter().all(|d| !d.starts_with('R')), "{missing:?}");
    // The placement list: a row per footprint, at the middle of its pads, its rotation plus the
    // library footprint's offset.
    let cpl = std::fs::read_to_string(dir.join("board-cpl.csv")).unwrap();
    let rows: Vec<Vec<&str>> = cpl.lines().skip(1).map(|l| l.split(',').collect()).collect();
    assert_eq!(cpl.lines().next(), Some("Designator,Mid X,Mid Y,Layer,Rotation"));
    assert_eq!(rows.len(), placed.len());
    let r1 = rows.iter().find(|r| r[0] == "R1").unwrap();
    assert_eq!(r1[1..], ["-10mm", "0mm", "Top", "90"], "R1 at (-10, 0), turned 0° plus R0603's 90°");
    // A part marked do-not-populate leaves both lists.
    run(&mut s, "ecad.schematic.edit", json!({"parts": [{"part": "R2", "dnp": true}]}));
    run(&mut s, "ecad.board.export.jlc", json!({"path": dir.join("dnp.zip").to_string_lossy()}));
    let bom = std::fs::read_to_string(dir.join("dnp-bom.csv")).unwrap();
    let cpl = std::fs::read_to_string(dir.join("dnp-cpl.csv")).unwrap();
    assert!(bom.lines().any(|l| l == "10k,R1,R0603,C25804") && !bom.contains(",R2,") && !bom.contains("\"R1,R2\""), "{bom}");
    assert!(cpl.lines().all(|l| !l.starts_with("R2,")) && cpl.lines().any(|l| l.starts_with("R1,")), "{cpl}");
    run(&mut s, "ecad.schematic.edit", json!({"parts": [{"part": "R2", "dnp": false}]}));
    // A zip short of a copper layer doesn't pass.
    let short: Vec<fab::FabFile> = files.into_iter().filter(|f| !f.name.ends_with(".G1")).collect();
    std::fs::write(dir.join("short.zip"), fab::zip(&short).unwrap()).unwrap();
    let v = run(&mut s, "ecad.board.verify_outputs", json!({"path": dir.join("short.zip").to_string_lossy()}));
    assert_eq!((v["ok"].clone(), v["copper_layers"].clone(), v["expected_layers"].clone()), (json!(false), json!(3), json!(4)), "{v}");
    // A footprint whose pads sit off its origin (a module with its antenna past the pads) is placed
    // by the middle of its pads.
    run(&mut s, "ecad.pad.add", json!({"footprint": "R0603", "pads": [{"number": "T", "at": [0, 4], "size": [0.5, 0.5]}]}));
    let lo = s.doc.ecad.footprints.iter().find(|f| f.name == "R0603").and_then(|f| f.copper_bbox()).unwrap();
    run(&mut s, "ecad.board.export.jlc", json!({"path": dir.join("off.zip").to_string_lossy()}));
    let cpl = std::fs::read_to_string(dir.join("off-cpl.csv")).unwrap();
    let r1: Vec<&str> = cpl.lines().find(|l| l.starts_with("R1,")).unwrap().split(',').collect();
    let want = format!("{}mm", (lo[0][1] + lo[1][1]) / 2.0);
    assert_eq!((r1[1], r1[2]), ("-10mm", want.as_str()), "{cpl}");
    // Clearing the offset is one undo step; an unknown footprint is refused.
    let undo = s.undo.len();
    let r = run(&mut s, "ecad.board.cpl_rotation", json!({"footprint": "R0603"}));
    assert_eq!((r["offsets"].clone(), s.undo.len()), (json!({}), undo + 1));
    assert!(refused(&mut s, "ecad.board.cpl_rotation", json!({"footprint": "NOPE", "offset": 90})).contains("NOPE"));
    assert!(refused(&mut s, "ecad.board.export.jlc", json!({"path": dir.join("x.txt").to_string_lossy()})).contains(".zip"));
}

#[test]
fn copper_the_files_join_that_the_board_keeps_apart_is_a_short() {
    use solidcraft_io::fab;
    let mut s = board();
    let nets = s.board_nets();
    let o = fab::Options::default();
    let mut files = fab::gerbers(&s.doc.ecad, &nets, &o).unwrap();
    files.extend(fab::drills(&s.doc.ecad, &nets, &o).unwrap());
    assert!(fab::verify(&s.doc.ecad, &nets, &files).ok(), "as made, the files match the board");
    // A dark rectangle over the whole top copper joins every top pad.
    let top = files.iter_mut().find(|f| f.name.ends_with(".GTL")).unwrap();
    let text = String::from_utf8(top.bytes.clone()).unwrap();
    let flood = "%LPD*%\nG36*\nX-15000000Y-10000000D02*\nX15000000Y-10000000D01*\nX15000000Y10000000D01*\nX-15000000Y10000000D01*\nX-15000000Y-10000000D01*\nG37*\nM02*\n";
    top.bytes = text.replace("M02*\n", flood).into_bytes();
    let v = fab::verify(&s.doc.ecad, &nets, &files);
    assert!(!v.ok() && !v.shorts.is_empty() && v.opens.is_empty(), "{v:?}");
    let _ = &mut s;
}
