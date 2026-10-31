use serde_json::json;
use solidcraft_engine::doc::DocumentKind;

use super::*;
use crate::electronics_components::{Shown, shown_copy};
use crate::scenario::{Harness, handle_at};

fn run(h: &mut Harness, id: &str, p: serde_json::Value) {
    let reply = h.call("engine.execute", json!({"command": id, "params": p}));
    assert_eq!(reply["ok"], true, "{id}: {reply}");
}

/// A new electronics library in a tall window.
fn library() -> Harness {
    let mut h = Harness::new();
    h.call("ui.resize", json!({"width": 1440, "height": 1200}));
    crate::documents::new_electronics(&mut h.app, DocumentKind::ElectronicsLibrary);
    h.frames(2);
    h
}

/// An electronics library with an 0603 resistor drawn by hand: the symbol `R` with pins 1 and 2,
/// and the footprint `R0603` with two pads, which the page shows (the last command named it).
fn workspace() -> Harness {
    let mut h = library();
    run(&mut h, "ecad.symbol.create", json!({"name": "R"}));
    run(
        &mut h,
        "ecad.pin.add",
        json!({"symbol": "R", "pins": [{"number": 1, "name": "A", "at": [-5.08, 0], "direction": "right"}, {"number": 2, "name": "B", "at": [5.08, 0], "direction": "left"}]}),
    );
    run(&mut h, "ecad.footprint.create", json!({"name": "R0603"}));
    run(
        &mut h,
        "ecad.pad.add",
        json!({"footprint": "R0603", "pads": [{"number": 1, "at": [-0.8, 0], "size": [0.9, 1.0]}, {"number": 2, "at": [0.8, 0], "size": [0.9, 1.0]}]}),
    );
    h.frames(3);
    h
}

/// What the page shows: the open item as the editor works on it.
fn shown(h: &Harness) -> Shown {
    shown_copy(&h.app).expect("a library item is shown")
}

fn click(h: &mut Harness, handle: &str) {
    h.step(&json!({"click": {"handle": handle}})).unwrap_or_else(|e| panic!("{handle}: {e}"));
    h.frames(2);
}

/// Open the library item `name` of `kind` (`symbols`, `footprints`, `components`) from the
/// Browser's Library card.
fn open(h: &mut Harness, kind: &str, name: &str) {
    click(h, &format!("electronics:library:{kind}"));
    click(h, &format!("electronics:item:{kind}:{name}"));
}

#[test]
fn a_dragged_pad_moves_in_grid_steps_in_one_undo_step() {
    let mut h = workspace();
    open(&mut h, "footprints", "R0603");
    let at = handle_at("electronics:pad:2").expect("pad 2 is drawn");
    let depth = h.app.session.undo.len();
    h.step(&json!({"drag": [[at.x, at.y], [at.x + 90.0, at.y]], "steps": 6})).unwrap();
    h.frames(2);
    let step = solidcraft_engine::view::grid_step(h.app.electronics.page.cam.half_height()).0;
    let pad = shown(&h).footprint.pad("2").cloned().unwrap();
    let moved = (pad.at[0] - 0.8) / step;
    assert!(pad.at[0] > 0.8 && (moved - moved.round()).abs() < 1e-6, "on the grid of {step} mm: {:?}", pad.at);
    assert_eq!(pad.at[1], 0.0);
    assert_eq!(h.app.session.undo.len(), depth + 1, "the whole drag is one edit");
    assert_eq!(h.app.electronics.editor.picked, Some(Item::Pad("2".into())), "the dragged pad stays picked");
    h.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(shown(&h).footprint.pad("2").map(|p| p.at), Some([0.8, 0.0]));
}

#[test]
fn a_click_picks_a_pin_and_the_inspector_renames_it() {
    let mut h = workspace();
    open(&mut h, "symbols", "R");
    click(&mut h, "electronics:pin:1");
    assert_eq!(h.app.electronics.editor.picked, Some(Item::Pin("1".into())));
    // The Inspector shows the pin; a name typed there lands when Enter gives up the keyboard.
    click(&mut h, "dialog:Pin name");
    h.step(&json!({"key": "A", "cmd": true})).unwrap();
    h.step(&json!({"text": "IN"})).unwrap();
    h.step(&json!({"key": "Enter"})).unwrap();
    h.frames(2);
    assert_eq!(shown(&h).symbol.pin("1").map(|p| p.name.as_str()), Some("IN"));
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
    open(&mut h, "footprints", "R0603");
    click(&mut h, "electronics:pad:1");
    h.step(&json!({"key": "Delete"})).unwrap();
    h.frames(2);
    assert!(shown(&h).footprint.pad("1").is_none(), "the picked pad is gone");
    assert_eq!(h.app.session.model.state().bodies.len(), bodies, "the hidden model keeps its bodies");
    // With nothing picked, Delete does nothing at all.
    let document = h.app.session.doc.clone();
    h.step(&json!({"key": "Delete"})).unwrap();
    h.frames(2);
    assert!(std::sync::Arc::ptr_eq(&document, &h.app.session.doc));
}

#[test]
fn a_new_part_from_the_generators_is_ready_to_place() {
    let mut h = library();
    // The Library card's + makes a new component and opens it.
    click(&mut h, "electronics:library:new:components");
    assert_eq!(shown(&h).component.name, "Part 1");
    // The footprint generator starts at an 0603 chip, and makes the part's first footprint.
    h.app.start("ecad.footprint.generate");
    h.frames(2);
    click(&mut h, "electronics:editor:generate-footprint");
    let c = shown(&h);
    assert_eq!(c.component.name, "Part 1", "the part stays open");
    assert_eq!(c.footprint.pads.len(), 2);
    // A box symbol gets one pin for each pad.
    h.app.start("ecad.symbol.generate");
    h.frames(2);
    // Its Generate button sits below the style pictures: scroll the window down to it.
    let tiles = handle_at("electronics:editor:style:box").expect("the style pictures are drawn");
    h.call("ui.scroll", json!({"x": tiles.x, "y": tiles.y, "delta": -600}));
    h.frames(3);
    click(&mut h, "electronics:editor:generate-symbol");
    let c = shown(&h);
    assert_eq!(c.symbol.pins.iter().map(|p| p.number.as_str()).collect::<Vec<_>>(), ["1", "2"]);
    assert!(c.resolved().ready(), "{:?}", c.resolved().problems());
    // Delete takes the open part away; its symbol and footprint stay in the library.
    click(&mut h, "electronics:editor:delete-item");
    let ecad = &h.app.session.doc.ecad;
    assert!(ecad.components.is_empty());
    assert_eq!((ecad.symbols.len(), ecad.footprints.len()), (1, 1));
}

#[test]
fn the_pad_tool_adds_pads_like_the_last_and_the_page_keeps_clear_of_the_panel() {
    let mut h = workspace();
    open(&mut h, "footprints", "R0603");
    let two = handle_at("electronics:pad:2").expect("pad 2 is drawn");
    h.app.start("ecad.pad.add");
    h.frames(1);
    h.step(&json!({"click": [two.x + 90.0, two.y]})).unwrap();
    h.frames(2);
    let pad = shown(&h).footprint.pad("3").cloned().expect("pad 3 added");
    assert!(pad.at[0] > 0.8 && pad.at[1] == 0.0, "where the click was, on pad 2's row: {:?}", pad.at);
    assert_eq!((pad.size, pad.kind), ([0.9, 1.0], PadKind::Smd), "the same as the last pad");
    assert_eq!(h.app.electronics.editor.picked, Some(Item::Pad("3".into())));
    assert!(h.app.electronics.editor.draw.is_some(), "the tool stays armed for the next one");
    let panel = h.app.shell.workspace_panels.get("electronics:editor").map(|(r, _)| *r).expect("the panel is up");
    let content = crate::shell::content_rect(&h.app, h.app.viewport.rect.unwrap());
    assert!(content.right() <= panel.left(), "Fit frames the page left of the panel: {content:?} {panel:?}");
    // A 3D design in another tab has no Inspector over its view.
    crate::documents::new_design(&mut h.app);
    h.frames(2);
    assert!(!h.app.shell.workspace_panels.contains_key("electronics:editor"), "Design's view isn't covered");
}

#[test]
fn pins_connect_to_the_pads_picked_for_them() {
    let mut h = workspace();
    run(&mut h, "ecad.component.create", json!({"name": "R0603", "prefix": "R", "symbol": "R", "footprint": "R0603"}));
    h.frames(2);
    assert_eq!(shown(&h).component.name, "R0603", "the page follows the new component");
    assert_eq!(shown(&h).resolved().connections()[0], ("1", vec!["1"]), "pins connect to pads by number at first");
    // Connect starts with the first pin; a click on pad 2 gives it to pin 1 as well.
    h.app.start("ecad.component.map_pins");
    h.frames(2);
    click(&mut h, "dialog:list:connect-pad:2");
    let c = shown(&h);
    assert_eq!(c.resolved().connections()[0], ("1", vec!["1", "2"]));
    assert_eq!(c.resolved().connections()[1], ("2", vec![]), "pad 2 left pin 2 for pin 1");
    assert!(!c.resolved().ready(), "pin 2 has no pad now");
    click(&mut h, "electronics:editor:by-number");
    assert_eq!(shown(&h).resolved().connections()[0], ("1", vec!["1"]));
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
