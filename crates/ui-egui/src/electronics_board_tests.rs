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
