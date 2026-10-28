use serde_json::json;

use super::*;
use crate::scenario::{Harness, handle_at};

fn run(h: &mut Harness, id: &str, p: serde_json::Value) {
    let reply = h.call("engine.execute", json!({"command": id, "params": p}));
    assert_eq!(reply["ok"], true, "{id}: {reply}");
}

/// The Electronics workspace in a tall window, with an 0603 resistor drawn by hand.
fn workspace() -> Harness {
    let mut h = Harness::new();
    h.call("ui.resize", json!({"width": 1440, "height": 1200}));
    h.call("ui.workspace", json!({"workspace": "electronics"}));
    run(&mut h, "ecad.component.create", json!({"name": "R0603", "prefix": "R"}));
    run(
        &mut h,
        "ecad.pin.add",
        json!({"component": "R0603", "pins": [{"number": 1, "name": "A", "at": [-5.08, 0], "direction": "right"}, {"number": 2, "name": "B", "at": [5.08, 0], "direction": "left"}]}),
    );
    run(
        &mut h,
        "ecad.pad.add",
        json!({"component": "R0603", "pads": [{"number": 1, "at": [-0.8, 0], "size": [0.9, 1.0]}, {"number": 2, "at": [0.8, 0], "size": [0.9, 1.0]}]}),
    );
    h.frames(3);
    h
}

fn component(h: &Harness) -> ElectronicComponent {
    shown(&h.app).cloned().expect("an electronic component is shown")
}

fn click(h: &mut Harness, handle: &str) {
    h.step(&json!({"click": {"handle": handle}})).unwrap_or_else(|e| panic!("{handle}: {e}"));
    h.frames(2);
}

#[test]
fn a_dragged_pad_moves_in_grid_steps_in_one_undo_step() {
    let mut h = workspace();
    let at = handle_at("electronics:pad:2").expect("pad 2 is drawn");
    let depth = h.app.session.undo.len();
    h.step(&json!({"drag": [[at.x, at.y], [at.x + 90.0, at.y]], "steps": 6})).unwrap();
    h.frames(2);
    let step = solidcraft_engine::view::grid_step(h.app.electronics.page.cam.half_height()).0;
    let pad = component(&h).footprint.pad("2").cloned().unwrap();
    let moved = (pad.at[0] - 0.8) / step;
    assert!(pad.at[0] > 0.8 && (moved - moved.round()).abs() < 1e-6, "on the grid of {step} mm: {:?}", pad.at);
    assert_eq!(pad.at[1], 0.0);
    assert_eq!(h.app.session.undo.len(), depth + 1, "the whole drag is one edit");
    assert_eq!(h.app.electronics.editor.picked, Some(Item::Pad("2".into())), "the dragged pad stays picked");
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(component(&h).footprint.pad("2").map(|p| p.at), Some([0.8, 0.0]));
}

#[test]
fn a_click_picks_a_pin_and_the_panel_renames_it() {
    let mut h = workspace();
    click(&mut h, "electronics:view:symbol");
    click(&mut h, "electronics:pin:1");
    assert_eq!(h.app.electronics.editor.picked, Some(Item::Pin("1".into())));
    // The panel shows the pin; a name typed there lands when Enter gives up the keyboard.
    click(&mut h, "dialog:Pin name");
    h.step(&json!({"key": "A", "cmd": true})).unwrap();
    h.step(&json!({"text": "IN"})).unwrap();
    h.step(&json!({"key": "Enter"})).unwrap();
    h.frames(2);
    assert_eq!(component(&h).symbol.pin("1").map(|p| p.name.as_str()), Some("IN"));
    // A click on the empty page drops the pick; so does Escape.
    let content = crate::shell::content_rect(&h.app, h.app.viewport.rect.unwrap());
    h.step(&json!({"click": [content.left() + 40.0, content.bottom() - 40.0]})).unwrap();
    h.frames(2);
    assert_eq!(h.app.electronics.editor.picked, None);
    click(&mut h, "electronics:pin:2");
    h.step(&json!({"key": "Escape"})).unwrap();
    h.frames(1);
    assert_eq!(h.app.electronics.editor.picked, None);
}

#[test]
fn delete_removes_the_picked_pad_and_never_the_model() {
    let mut h = workspace();
    run(&mut h, "solid.box", json!({"length": 10, "width": 10, "height": 10, "body_name": "Cube"}));
    run(&mut h, "select.set", json!({"items": [{"type": "body", "name": "Cube"}]}));
    h.frames(2);
    let bodies = h.app.session.model.state().bodies.len();
    click(&mut h, "electronics:view:footprint");
    click(&mut h, "electronics:pad:1");
    h.step(&json!({"key": "Delete"})).unwrap();
    h.frames(2);
    assert!(component(&h).footprint.pad("1").is_none(), "the picked pad is gone");
    assert_eq!(h.app.session.model.state().bodies.len(), bodies, "the hidden model keeps its bodies");
    // With nothing picked, Delete does nothing at all.
    let document = h.app.session.doc.clone();
    h.step(&json!({"key": "Delete"})).unwrap();
    h.frames(2);
    assert!(std::sync::Arc::ptr_eq(&document, &h.app.session.doc));
}

#[test]
fn a_new_part_from_the_generators_is_ready_to_place() {
    let mut h = Harness::new();
    h.call("ui.resize", json!({"width": 1440, "height": 1200}));
    h.call("ui.workspace", json!({"workspace": "electronics"}));
    h.frames(2);
    click(&mut h, "electronics:editor:new");
    let c = component(&h);
    assert_eq!(c.name, "Part 1");
    // The footprint generator starts at an 0603 chip.
    click(&mut h, "electronics:view:footprint");
    click(&mut h, "electronics:editor:generate-footprint");
    assert_eq!(component(&h).footprint.pads.len(), 2);
    // A box symbol gets one pin for each pad.
    click(&mut h, "electronics:view:symbol");
    click(&mut h, "electronics:editor:generate-symbol");
    let c = component(&h);
    assert_eq!(c.symbol.pins.iter().map(|p| p.number.as_str()).collect::<Vec<_>>(), ["1", "2"]);
    assert!(c.ready(), "{:?}", c.problems());
    // Copy makes a second one with a free name; Delete takes the shown one away.
    click(&mut h, "electronics:editor:copy");
    assert_eq!(component(&h).name, "Part 1 copy");
    click(&mut h, "electronics:editor:delete");
    assert_eq!(h.app.session.doc.ecad.components.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Part 1"]);
}

#[test]
fn added_pads_carry_on_the_row_and_the_page_keeps_clear_of_the_panel() {
    let mut h = workspace();
    click(&mut h, "electronics:view:footprint");
    click(&mut h, "electronics:editor:add-pad");
    let c = component(&h);
    let pad = c.footprint.pad("3").expect("pad 3 added");
    assert_eq!((pad.at, pad.size), ([2.4, 0.0], [0.9, 1.0]), "1.6 mm on from pad 2, the same size");
    assert_eq!(h.app.electronics.editor.picked, Some(Item::Pad("3".into())));
    let panel = h.app.shell.workspace_panels.get("electronics:editor").map(|(r, _)| *r).expect("the panel is up");
    let content = crate::shell::content_rect(&h.app, h.app.viewport.rect.unwrap());
    assert!(content.right() <= panel.left(), "Fit frames the page left of the panel: {content:?} {panel:?}");
    h.call("ui.workspace", json!({"workspace": "design"}));
    h.frames(2);
    assert!(!h.app.shell.workspace_panels.contains_key("electronics:editor"), "Design's view isn't covered");
}

#[test]
fn pins_map_to_the_pads_typed_for_them() {
    let mut h = workspace();
    click(&mut h, "electronics:view:symbol");
    click(&mut h, "dialog:Pin 1 A");
    h.step(&json!({"key": "A", "cmd": true})).unwrap();
    h.step(&json!({"text": "1, 2"})).unwrap();
    h.step(&json!({"key": "Enter"})).unwrap();
    h.frames(2);
    let c = component(&h);
    assert_eq!(c.connections()[0], ("1", vec!["1", "2"]));
    assert_eq!(c.connections()[1], ("2", vec![]), "pad 2 left pin 2 for pin 1");
    assert!(!c.ready(), "pin 2 has no pad now");
    click(&mut h, "electronics:editor:by-number");
    assert_eq!(component(&h).connections()[0], ("1", vec!["1"]));
}

#[test]
fn numbers_read_cleanly() {
    assert_eq!(mm(0.8), "0.8");
    assert_eq!(mm(-0.00001), "0");
    assert_eq!(mm(2.0), "2");
    assert_eq!(tidy(0.1 + 0.2), 0.3);
    assert_eq!(parse_drill("1.0"), Some(json!(1.0)));
    assert_eq!(parse_drill("0.6 x 1.2"), Some(json!([0.6, 1.2])));
    assert_eq!(parse_drill("big"), None);
    assert_eq!(next_number(["1", "2", "MH"].into_iter()), "3");
    assert_eq!(next_number(["A1"].into_iter()), "1");
}
