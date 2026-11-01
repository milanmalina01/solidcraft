//! Stitching vias on the test board (`ecad::tests::board`): GND planes on inner1 and inner2, the
//! SIG track 8 on top, the hand-placed GND vias 9 and 10 and the mounting hole 12.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;
use crate::cmd::ecad::tests::{board, run};

fn refused(s: &mut Session, id: &str, p: Value) -> String {
    let (before, undo) = (s.doc.clone(), s.undo.len());
    let Err(e) = s.execute(id, &p) else { panic!("{id} {p} must be refused") };
    assert!(Arc::ptr_eq(&before, &s.doc), "a refused {id} leaves the design as it was");
    assert_eq!(s.undo.len(), undo);
    e.to_string()
}

/// The quality findings of `code` on the board.
fn quality(s: &mut Session, code: &str) -> Vec<Value> {
    run(s, "ecad.quality.inspect", json!({"codes": [code]}))["findings"].as_array().cloned().unwrap_or_default()
}

/// The board's vias as `ecad.board.inspect` gives them.
fn vias(s: &mut Session) -> Vec<Value> {
    run(s, "ecad.board.inspect", json!({}))["vias"].as_array().cloned().unwrap_or_default()
}

fn drc_errors(s: &mut Session) -> Value {
    run(s, "ecad.drc.inspect", json!({"summary": true}))["counts"]["errors"].clone()
}

#[test]
fn a_grid_joins_the_planes_until_the_check_passes_and_takes_one_undo_step() {
    let mut s = board();
    // The hole in the top left corner leaves room no grid spot can take; without it the grid
    // covers the planes.
    run(&mut s, "ecad.board.delete", json!({"items": [12]}));
    let found = quality(&mut s, "pi.stitching");
    assert_eq!(found.len(), 1, "two GND planes joined by two vias only: {found:?}");
    assert_eq!(found[0]["fix"][0]["command"], "ecad.board.stitch", "the finding says how to fix it");
    // λg/10 at 2400 MHz in εr 4.3 is 6.024 mm: staggered rows 0.678 of it apart keep every
    // via-free circle within it.
    let before = s.doc.clone();
    let dry = run(&mut s, "ecad.board.stitch", json!({"net": "GND", "pattern": "grid", "under_parts": true, "dry_run": true}));
    assert!(Arc::ptr_eq(&before, &s.doc), "a dry run changes nothing");
    let n = dry["count"].as_u64().unwrap();
    assert_eq!(dry["would_create"].as_array().map(Vec::len), Some(n as usize));
    assert!(dry["created"].as_array().is_some_and(Vec::is_empty), "{dry}");
    let pitch = dry["pitch_used"].as_f64().unwrap();
    assert!(pitch <= 6.024 * 0.678 && pitch > 6.024 * 0.6, "{dry}");
    assert_eq!((dry["via"]["from"].clone(), dry["via"]["to"].clone()), (json!("top"), json!("bottom")), "the class's through via");
    let errors = drc_errors(&mut s);
    let undo = s.undo.len();
    let r = run(&mut s, "ecad.board.stitch", json!({"net": "GND", "pattern": "grid", "under_parts": true}));
    assert_eq!(s.undo.len(), undo + 1, "however many vias, one undo step");
    assert_eq!((r["count"].clone(), r["created"].as_array().map(Vec::len)), (json!(n), Some(n as usize)), "what the dry run said");
    let stitched: Vec<Value> = vias(&mut s).into_iter().filter(|v| v["role"] == "stitch").collect();
    assert_eq!(stitched.len(), n as usize, "the new vias are marked as stitching vias");
    assert!(stitched.iter().all(|v| v["net"] == "GND"));
    assert!(quality(&mut s, "pi.stitching").is_empty(), "the planes are stitched");
    assert_eq!(drc_errors(&mut s), errors, "no via breaks a design rule");
    let delta = &r["findings_delta"];
    assert!(delta["resolved"].as_u64().is_some_and(|n| n >= 1), "{delta}");
    assert!(delta["introduced"].as_array().unwrap().iter().all(|f| f["code"] != "pi.stitching" && f["code"] != "clearance"), "{delta}");
    // Again: every spot has its via, so nothing is added and no undo step either.
    let undo = s.undo.len();
    let again = run(&mut s, "ecad.board.stitch", json!({"net": "GND", "pattern": "grid", "under_parts": true}));
    assert_eq!((again["count"].clone(), s.undo.len()), (json!(0), undo));
    assert!(again["skipped"]["near_via"].as_u64().is_some_and(|k| k as usize >= stitched.len()), "{again}");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(quality(&mut s, "pi.stitching").len(), 1, "undo takes the grid back");
}

#[test]
fn needed_stitching_says_before_and_after_and_takes_one_undo_step() {
    let mut s = board();
    run(&mut s, "ecad.board.delete", json!({"items": [12]}));
    let map = run(&mut s, "ecad.board.ground.inspect", json!({"map": 8}));
    assert_eq!(map["net"], "GND");
    assert_eq!(map["reference"]["kind"], "plane");
    assert!(map["converged"] == true && map["worst"].as_array().is_some_and(|w| !w.is_empty()), "{map}");
    let worst = map["worst"][0]["mv"].as_f64().unwrap();
    assert!(map["layers"].as_array().unwrap().iter().all(|l| l["map"].as_array().is_some_and(|r| r.len() <= 8)), "{map}");
    let before = s.doc.clone();
    let dry = run(&mut s, "ecad.board.stitch", json!({"net": "GND", "pattern": "needed", "budget": 30, "dry_run": true}));
    assert!(Arc::ptr_eq(&before, &s.doc), "a dry run changes nothing");
    let g = &dry["ground"];
    assert!((g["before"]["worst_mv"].as_f64().unwrap() - worst).abs() < 1e-3, "the inspect's number: {dry}");
    assert!(g["after"]["worst_mv"].as_f64().unwrap() < worst && dry["count"].as_u64().unwrap() >= 1, "{dry}");
    let n = dry["count"].as_u64().unwrap();
    let (errors, undo) = (drc_errors(&mut s), s.undo.len());
    let r = run(&mut s, "ecad.board.stitch", json!({"net": "GND", "pattern": "needed", "budget": 30}));
    assert_eq!((r["count"].as_u64(), s.undo.len()), (Some(n), undo + 1), "what the dry run said, in one undo step");
    assert_eq!(drc_errors(&mut s), errors, "no via breaks a design rule");
    let placed: Vec<Value> = vias(&mut s).into_iter().filter(|v| v["pattern"] == "needed").collect();
    assert_eq!(placed.len(), n as usize, "{placed:?}");
    let after = run(&mut s, "ecad.board.ground.inspect", json!({"map": 0}));
    assert!((after["worst"][0]["mv"].as_f64().unwrap() - g["after"]["worst_mv"].as_f64().unwrap()).abs() < 1e-3, "{after}");
    // They come off with their pattern.
    let gone = run(&mut s, "ecad.board.stitch.remove", json!({"pattern": "needed"}));
    assert_eq!(gone["count"].as_u64(), Some(n));
    refused(&mut s, "ecad.board.ground.inspect", json!({"cell": 1.0}));
    refused(&mut s, "ecad.board.stitch", json!({"net": "GND", "pattern": "needed", "budget": 0}));
}

#[test]
fn removing_stitching_takes_only_the_stitching_vias() {
    let mut s = board();
    let r = run(&mut s, "ecad.board.stitch", json!({"net": "GND", "pattern": "grid", "under_parts": true}));
    let made = r["count"].as_u64().unwrap();
    assert!(made > 10, "{r}");
    let fence = run(&mut s, "ecad.board.stitch", json!({"net": "GND", "pattern": "fence", "tracks": [8]}));
    let fenced = fence["count"].as_u64().unwrap();
    // Only those its pattern placed; a region; then the rest of the net's.
    let r = run(&mut s, "ecad.board.stitch.remove", json!({"net": "GND", "pattern": "fence"}));
    assert_eq!(r["count"], json!(fenced));
    let r = run(&mut s, "ecad.board.stitch.remove", json!({"region": {"rect": [[0, 0], [15, 10]]}}));
    let quarter = r["count"].as_u64().unwrap();
    assert!(quarter > 0 && quarter < made, "only those in the region: {r}");
    let r = run(&mut s, "ecad.board.stitch.remove", json!({"net": "GND"}));
    assert_eq!(r["count"], json!(made - quarter));
    let left: Vec<u64> = vias(&mut s).iter().filter_map(|v| v["id"].as_u64()).collect();
    assert_eq!(left, [9, 10], "the vias placed by hand stay");
    let undo = s.undo.len();
    let r = run(&mut s, "ecad.board.stitch.remove", json!({"net": "GND"}));
    assert_eq!((r["count"].clone(), s.undo.len()), (json!(0), undo), "nothing left to take: no undo step");
    let e = refused(&mut s, "ecad.board.stitch.remove", json!({"ids": [9]}));
    assert!(e.contains('9'), "a via placed by hand is no stitching via: {e}");
}

#[test]
fn a_fence_lines_both_sides_of_a_track() {
    let mut s = board();
    let r = run(&mut s, "ecad.board.stitch", json!({"net": "GND", "pattern": "fence", "tracks": [8], "pitch": 2}));
    let n = r["count"].as_u64().unwrap() as usize;
    let offset = r["offset_used"].as_f64().unwrap();
    let skipped: u64 = r["skipped"].as_object().map_or(0, |o| o.values().filter_map(Value::as_u64).sum());
    assert!(offset > 0.1, "{r}");
    let ids: Vec<u64> = r["created"].as_array().unwrap().iter().filter_map(|c| c["id"].as_u64()).collect();
    let fence: Vec<[f64; 2]> = vias(&mut s)
        .iter()
        .filter(|v| v["id"].as_u64().is_some_and(|id| ids.contains(&id)))
        .map(|v| [v["at"][0].as_f64().unwrap(), v["at"][1].as_f64().unwrap()])
        .collect();
    assert_eq!(fence.len(), n);
    // Track 8 runs along y = 0 from x −9.2 to 9.2: vias stand on both sides, `offset` from it,
    // along its length, at most the pitch apart along a side (the ends sit beside the pads) but
    // where a spot was skipped (too near a pad of another net).
    let (above, below): (Vec<[f64; 2]>, Vec<[f64; 2]>) = fence.iter().copied().partition(|p| p[1] > 0.0);
    assert!(!above.is_empty() && !below.is_empty(), "{fence:?}");
    let mut wide = 0;
    for side in [&above, &below] {
        assert!(side.iter().all(|p| (p[1].abs() - offset).abs() < 1e-6 && p[0].abs() <= 9.2 + 1.0 + 1e-6), "{side:?}");
        let mut xs: Vec<f64> = side.iter().map(|p| p[0]).collect();
        xs.sort_by(f64::total_cmp);
        assert!(xs.first().is_some_and(|x| *x <= -9.2 + 1e-6) && xs.last().is_some_and(|x| *x >= 9.2 - 1e-6), "a via beside each end: {xs:?}");
        wide += xs.windows(2).filter(|w| w[1] - w[0] > 2.0 + 1e-6).count();
    }
    assert!(wide as u64 <= skipped, "{wide} gaps wider than the pitch, {skipped} spots skipped: {r}");
    let e = refused(&mut s, "ecad.board.stitch", json!({"net": "GND", "pattern": "fence", "tracks": [9]}));
    assert!(e.contains('9'), "a via is no track: {e}");
}

#[test]
fn a_layer_change_gets_its_return_via() {
    let mut s = board();
    // SIG goes down to the bottom and back up: its reference changes from the inner1 plane to the
    // inner2 plane at each via, with no GND via near.
    run(&mut s, "ecad.board.delete", json!({"items": [8]}));
    run(&mut s, "ecad.board.track", json!({"layer": "top", "points": [[-9.2, 0], [-3, 0]], "width": 0.2, "via": {}}));
    run(&mut s, "ecad.board.track", json!({"layer": "bottom", "points": [[-3, 0], [3, 0]], "width": 0.2, "via": {}}));
    run(&mut s, "ecad.board.track", json!({"layer": "top", "points": [[3, 0], [9.2, 0]], "width": 0.2}));
    // The via at (3, 0) has J1's through-hole GND pad within reach; the one at (−3, 0) nothing.
    let found = quality(&mut s, "pi.layer_change");
    assert_eq!(found.len(), 1, "{found:?}");
    let r = run(&mut s, "ecad.board.stitch", json!({"net": "GND", "pattern": "layer_change", "nets": ["SIG"]}));
    assert!(r["count"].as_u64().is_some_and(|n| n >= 1), "a GND via beside it: {r}");
    assert!(quality(&mut s, "pi.layer_change").is_empty());
    for v in vias(&mut s).iter().filter(|v| v["role"] == "stitch") {
        let [x, y] = [v["at"][0].as_f64().unwrap(), v["at"][1].as_f64().unwrap()];
        assert!((x.abs() - 3.0).hypot(y) <= 2.0 + 1e-6, "within 2 mm of a signal via: {v}");
    }
}

#[test]
fn stitching_refuses_what_it_cannot_place_well() {
    let mut s = board();
    for (p, says) in [
        (json!({"net": "NOPE", "pattern": "grid"}), "NOPE"),
        (json!({"net": "GND", "pattern": "spiral"}), "`pattern`"),
        (json!({"net": "GND", "pattern": "grid", "via": {"drill": 0.05}}), "least drill"),
        (json!({"net": "GND", "pattern": "grid", "pitch": -1}), "`pitch`"),
        (json!({"net": "GND", "pattern": "grid", "region": {"pour": 99}}), "99"),
        (json!({"net": "GND", "pattern": "grid", "region": {"rect": [[0, 0], [1e9, 1]]}}), "region"),
    ] {
        let e = refused(&mut s, "ecad.board.stitch", p.clone());
        assert!(e.contains(says), "{p}: {e}");
    }
    let e = refused(&mut s, "ecad.board.stitch.remove", json!({"region": {"pour": 99}}));
    assert!(e.contains("99"), "{e}");
}
