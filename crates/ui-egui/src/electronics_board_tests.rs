use serde_json::json;

use crate::scenario::{Harness, handle_at};

fn run(h: &mut Harness, id: &str, p: serde_json::Value) -> serde_json::Value {
    let reply = h.call("engine.execute", json!({"command": id, "params": p}));
    assert_eq!(reply["ok"], true, "{id}: {reply}");
    reply
}

fn click(h: &mut Harness, handle: &str) {
    h.step(&json!({"click": {"handle": handle}})).unwrap_or_else(|e| panic!("{handle}: {e}"));
    h.frames(2);
}

/// A design with one resistor on the board, shown in the Board view.
fn board_with_a_resistor() -> Harness {
    let mut h = Harness::new();
    h.call("ui.resize", json!({"width": 1440, "height": 1000}));
    crate::documents::new_electronics(&mut h.app, solidcraft_engine::doc::DocumentKind::ElectronicsDesign);
    h.frames(3);
    run(&mut h, "ecad.footprint.generate", json!({"footprint": "R0603", "package": "chip", "size": "0603"}));
    run(&mut h, "ecad.symbol.generate", json!({"symbol": "Resistor", "style": "resistor"}));
    run(&mut h, "ecad.component.create", json!({"name": "R0603", "prefix": "R", "value": "10k", "symbol": "Resistor", "footprint": "R0603"}));
    run(&mut h, "ecad.schematic.place", json!({"parts": [{"component": "R0603", "at": [0, 0]}]}));
    run(&mut h, "ecad.view.board", json!({}));
    run(&mut h, "ecad.board.sync", json!({}));
    h.frames(3);
    h
}

#[test]
fn the_inspector_types_a_footprint_rotation_and_position() {
    let mut h = board_with_a_resistor();
    click(&mut h, "electronics:footprint:R1");
    assert!(handle_at("dialog:Rotation (°)").is_some(), "the Inspector shows the footprint");
    click(&mut h, "dialog:Rotation (°)");
    h.step(&json!({"key": "A", "cmd": true})).unwrap();
    h.step(&json!({"text": "100"})).unwrap();
    h.step(&json!({"key": "Enter"})).unwrap();
    h.frames(2);
    assert_eq!(h.app.session.doc.ecad.board.footprints[0].rotation, 90, "100° goes to the nearest quarter turn");
}

/// The resistor board with GND on R1.1, a 30 by 20 mm outline, GND pours on the top and the
/// bottom tied by one via in a corner.
fn grounded_board() -> Harness {
    let mut h = board_with_a_resistor();
    run(&mut h, "ecad.schematic.power", json!({"ports": [{"name": "GND", "at": "R1.1"}]}));
    run(&mut h, "ecad.board.outline", json!({"rect": [30, 20]}));
    for layer in ["top", "bottom"] {
        run(&mut h, "ecad.board.pour", json!({"layer": layer, "net": "GND", "rect": [28, 18]}));
    }
    run(&mut h, "ecad.board.via", json!({"at": [-12, -7], "net": "GND"}));
    h.frames(2);
    h
}

/// Frames until `done`, giving the window's workers time (bounded).
fn until(h: &mut Harness, done: impl Fn(&Harness) -> bool) -> bool {
    for _ in 0..600 {
        if done(h) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.frames(1);
    }
    done(h)
}

#[test]
fn the_ribbon_tidies_names_and_shows_the_ground_map() {
    let mut h = grounded_board();
    assert!(h.app.electronics.board.hidden.contains(&crate::electronics_board_layers::Layer::Ground), "hidden until asked");
    h.app.start("ecad.view.ground");
    h.frames(2);
    assert!(!h.app.electronics.board.hidden.contains(&crate::electronics_board_layers::Layer::Ground));
    assert!(
        until(&mut h, |h| crate::electronics_board_ground::note(&h.app).is_some_and(|n| n.contains("GND"))),
        "the map is solved beside the window"
    );
    h.app.start("ecad.view.layers");
    h.frames(2);
    assert!(h.app.electronics.board.layers_tab);
    // Tidy Names: Fit unless the board has a mode, in one undo step.
    let undo = h.app.session.undo.len();
    h.app.start("ecad.board.labels");
    h.frames(2);
    assert_eq!(h.app.session.doc.ecad.board.names_mode, Some(solidcraft_engine::doc::ecad::labels::NamesMode::Fit));
    assert_eq!(h.app.session.undo.len(), undo + 1);
}

#[test]
fn stitching_where_needed_previews_ghost_vias_and_numbers_then_places_them() {
    use solidcraft_engine::doc::ecad::stitch::Pattern;
    let mut h = grounded_board();
    h.app.start("ecad.board.stitch");
    h.frames(2);
    let s = &mut h.app.electronics.board.stitch;
    s.pattern = Pattern::Needed;
    s.pick = Some(crate::electronics_board_stitch::StitchPick::Board);
    h.frames(1);
    assert!(until(&mut h, |h| h.app.electronics.board.stitch.preview.is_some()), "the dry run comes back");
    let preview = h.app.electronics.board.stitch.preview.clone().unwrap();
    assert!(preview.error.is_none() && !preview.spots.is_empty(), "{preview:?}");
    let (before, after, _) = preview.ground.unwrap();
    assert!(after.unwrap() < before.unwrap(), "{preview:?}");
    assert!(crate::electronics_board_ground::note(&h.app).is_some(), "the heat map shows while stitching where needed");
    let undo = h.app.session.undo.len();
    crate::electronics_board_stitch::place(&mut h.app);
    h.frames(2);
    let placed = h.app.session.doc.ecad.board.vias.iter().filter(|v| v.pattern == Some(Pattern::Needed)).count();
    assert_eq!((placed, h.app.session.undo.len()), (preview.spots.len(), undo + 1));
}

#[test]
fn a_drag_on_a_track_over_a_pad_reshapes_the_track_and_never_moves_the_part() {
    let mut h = board_with_a_resistor();
    let (f_at, pad) = {
        let nets = h.app.session.doc.ecad.board_nets();
        let p = nets.pads.iter().find(|p| p.pad.number == "2").unwrap();
        (h.app.session.doc.ecad.board.footprints[0].at, p.at())
    };
    run(&mut h, "ecad.board.track", json!({"layer": "top", "points": [pad, [pad[0] + 6.0, pad[1]]], "width": 0.25}));
    h.frames(2);
    let before = h.app.session.doc.ecad.board.tracks[0].points.clone();
    let rect = crate::scenario::rect_of("electronics:board").unwrap();
    let map = crate::electronics_workspace::PageMap::new(&h.app.electronics.page.cam, rect).unwrap();
    // On the pad, where the track runs into it, then 2 mm up.
    let from = map.to_screen(pad[0] + 0.15, pad[1]);
    let to = map.to_screen(pad[0] + 0.15, pad[1] + 2.0);
    h.step(&json!({"drag": [[from.x, from.y], [to.x, to.y]], "steps": 8})).unwrap();
    h.frames(2);
    let b = &h.app.session.doc.ecad.board;
    assert_eq!(b.footprints[0].at, f_at, "the part stays");
    assert_ne!(b.tracks[0].points, before, "the track is reshaped");
}

#[test]
fn the_drc_panel_refreshes_after_an_edit_beside_the_window_and_marks_the_picked_gap() {
    use crate::electronics_board_rules::{cached, current};
    use crate::electronics_design::Open;
    let mut h = board_with_a_resistor();
    run(&mut h, "ecad.schematic.power", json!({"ports": [{"name": "GND", "at": "R1.1"}]}));
    run(&mut h, "ecad.board.outline", json!({"rect": [30, 20]}));
    h.app.start("ecad.drc.inspect");
    h.frames(3);
    assert!(matches!(h.app.electronics.design.open, Some(Open::Drc(_))), "the panel is open");
    let clearances = |h: &Harness| cached(&h.app).map_or(0, |r| r.findings.iter().filter(|f| f.code == "clearance").count());
    assert_eq!(clearances(&h), 0);
    // A GND via 0.1 mm off R1.2's outer edge, where the clearance is 0.2 mm.
    let nets = h.app.session.board_nets();
    let pad = |n: &str| nets.pads.iter().find(|p| p.pad.number == n).unwrap();
    let (one, two) = (pad("1").at(), pad("2"));
    let out = if two.at()[0] >= one[0] { 1.0 } else { -1.0 };
    let at = [two.at()[0] + out * (two.pad.size[0] / 2.0 + 0.3 + 0.1), two.at()[1]];
    run(&mut h, "ecad.board.via", json!({"at": at, "net": "GND"}));
    let via = h.app.session.doc.ecad.board.vias.last().unwrap().id;
    h.frames(1);
    assert!(!current(&h.app), "the edit never waited for the check");
    assert!(until(&mut h, |h| current(&h.app)), "worked out beside the window");
    assert_eq!(clearances(&h), 1);
    let f = cached(&h.app).unwrap().findings.iter().find(|f| f.code == "clearance").unwrap().clone();
    let [a, b] = f.span.expect("a gap has its two ends");
    assert!(((a[0] - b[0]).hypot(a[1] - b[1]) - 0.1).abs() < 0.01, "{a:?} {b:?}");
    // Next picks it: framed, its items picked, its one marker.
    click(&mut h, "electronics:editor:drc-next");
    let Some(Open::Drc(w)) = &h.app.electronics.design.open else { panic!("closed") };
    assert_eq!(w.picked.as_deref(), Some(f.key.as_str()));
    assert!(h.app.electronics.board.selection.contains(&via), "{:?}", h.app.electronics.board.selection);
    // Moved away (the fix), the finding goes once the check has run again.
    run(&mut h, "ecad.board.move", json!({"items": [via], "by": [out * 2.0, 0.0]}));
    assert!(until(&mut h, |h| current(&h.app)));
    assert_eq!(clearances(&h), 0, "{:?}", cached(&h.app).map(|r| r.findings.iter().map(|f| f.message.clone()).collect::<Vec<_>>()));
}
