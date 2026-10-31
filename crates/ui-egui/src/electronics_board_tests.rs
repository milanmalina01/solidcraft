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
