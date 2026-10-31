use egui::{Event, Key, Modifiers, PointerButton, vec2};
use serde_json::json;
use solidcraft_engine::doc::DocumentKind;

use super::*;
use crate::scenario::Harness;

fn frame(app: &mut SolveApp, ctx: &egui::Context, time: &mut f64, events: Vec<Event>) {
    *time += 1.0 / 60.0;
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
        time: Some(*time),
        focused: true,
        events: std::iter::once(Event::ModifiersChanged(Modifiers::NONE)).chain(events).collect(),
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| show(app, ui));
    out.textures_delta.clear();
}

fn run(h: &mut Harness, id: &str, p: serde_json::Value) -> serde_json::Value {
    let reply = h.call("engine.execute", json!({"command": id, "params": p}));
    assert_eq!(reply["ok"], true, "{id}: {reply}");
    reply["result"].clone()
}

/// A 3D design with a box, and an electronics library opened beside it in a second tab.
fn beside_a_design() -> Harness {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"length": 20, "width": 15, "height": 10})).unwrap();
    h.frames(2);
    crate::documents::new_electronics(&mut h.app, DocumentKind::ElectronicsLibrary);
    h.frames(2);
    h
}

/// An electronics library with the symbol `R` (pins 1 and 2 on one line) open on the page.
/// No frame is drawn: the first one, of the size the test draws, frames the page.
fn symbol_open(h: &mut Harness) {
    crate::documents::new_electronics(&mut h.app, DocumentKind::ElectronicsLibrary);
    h.app.run("ecad.symbol.create", json!({"name": "R"})).unwrap();
    let pins = json!([{"number": 1, "at": [-5.08, 0], "direction": "right"}, {"number": 2, "at": [5.08, 0], "direction": "left"}]);
    h.app.run("ecad.pin.add", json!({"symbol": "R", "pins": pins})).unwrap();
    assert!(matches!(h.app.electronics.opened, Some(crate::electronics_library::Opened::Symbol(_))), "the page follows the new symbol");
}

#[test]
fn an_electronics_document_opens_in_its_workspace_and_its_design_tab_goes_back_to_design() {
    let mut h = beside_a_design();
    assert_eq!(h.app.docs.count(), 2, "the design is not blank: the library gets its own tab");
    assert!(active(&h.app));
    assert_eq!(h.app.ui.tab, LIBRARY);
    assert!(!crate::workspace::visible_tabs(&h.app).contains(&"MODEL"), "Design's tabs are hidden");
    h.app.run("ecad.inspect", json!({})).unwrap();
    h.frames(2);
    assert!(active(&h.app), "a command run here does not close the workspace");
    crate::documents::switch(&mut h.app, 0);
    h.frames(2);
    assert!(!active(&h.app));
    assert_eq!(h.app.ui.tab, "MODEL");
    crate::documents::switch(&mut h.app, 1);
    h.frames(2);
    assert!(active(&h.app), "and back in the library's tab, its workspace");
}

#[test]
fn a_sketch_started_in_an_electronics_document_takes_design_s_view() {
    let mut h = Harness::new();
    crate::documents::new_electronics(&mut h.app, DocumentKind::ElectronicsLibrary);
    h.frames(2);
    assert!(active(&h.app));
    // A sketch started here (by a shortcut or an agent) needs Design's view: the workspace leaves.
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.frames(2);
    assert!(!active(&h.app));
    assert_eq!(h.app.ui.tab, "SKETCH");
    h.app.run("sketch.finish", json!({})).unwrap();
    h.frames(2);
    assert!(active(&h.app), "with nothing needing Design's view, the library's tab comes back");
}

#[test]
fn each_kind_of_document_refuses_the_other_s_workspaces() {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"length": 20, "width": 15, "height": 10})).unwrap();
    h.frames(2);
    assert!(crate::workspace::switch(&mut h.app, "electronics").is_err(), "a 3D design has no Electronics workspace");
    assert!(!active(&h.app));
    crate::documents::new_electronics(&mut h.app, DocumentKind::ElectronicsDesign);
    h.frames(2);
    assert!(active(&h.app));
    assert!(crate::workspace::switch(&mut h.app, "electronics").is_ok());
    for other in ["render", "design"] {
        assert!(crate::workspace::switch(&mut h.app, other).is_err(), "{other} is a 3D design's");
        assert!(active(&h.app) && !crate::render_workspace::active(&h.app));
    }
}

#[test]
fn a_new_design_opens_in_design() {
    let mut h = Harness::new();
    crate::documents::new_electronics(&mut h.app, DocumentKind::ElectronicsDesign);
    h.frames(2);
    assert!(active(&h.app));
    h.app.run("file.new", json!({})).unwrap();
    h.frames(2);
    assert!(!active(&h.app));
    assert_eq!(h.app.session.doc.kind, DocumentKind::Design);
}

#[test]
fn the_page_pans_and_zooms_without_orbit_and_leaves_the_design_camera() {
    let mut h = Harness::new();
    let ctx = crate::fonts::test_context();
    h.app.preferences.smooth_wheel_zoom = false;
    h.app.preferences.zoom_reverse = false;
    h.app.ui.perspective = true;
    symbol_open(&mut h);
    h.app.cam_anim = None;
    h.app.pending_fit = None;
    let design = h.app.cam;
    let document = h.app.session.doc.clone();
    let mut time = 0.0;
    frame(&mut h.app, &ctx, &mut time, vec![]);
    assert!(h.app.electronics.page.fitted);
    let fitted = h.app.electronics.page.cam;
    let p = h.app.viewport.rect.unwrap().center() + vec2(80.0, 20.0);
    frame(&mut h.app, &ctx, &mut time, vec![Event::PointerMoved(p)]);
    let wheel =
        Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: vec2(0.0, 3.0), modifiers: Modifiers::NONE, phase: egui::TouchPhase::Move };
    frame(&mut h.app, &ctx, &mut time, vec![wheel]);
    assert!(h.app.electronics.page.cam.distance < fitted.distance, "the wheel zooms the page");
    let before = h.app.electronics.page.cam.target;
    let space = |pressed| Event::Key { key: Key::Space, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE };
    let press = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    frame(&mut h.app, &ctx, &mut time, vec![space(true), press(p, true)]);
    frame(&mut h.app, &ctx, &mut time, vec![Event::PointerMoved(p + vec2(40.0, -20.0))]);
    frame(&mut h.app, &ctx, &mut time, vec![press(p + vec2(40.0, -20.0), false), space(false)]);
    let cam = h.app.electronics.page.cam;
    assert_ne!(cam.target, before, "a pan drag moves the page");
    assert_eq!((cam.yaw, cam.pitch), (fitted.yaw, fitted.pitch), "the page never orbits");
    assert_eq!(h.app.cam, design);
    assert!(h.app.ui.perspective, "the page's own orthographic view leaves the preference alone");
    assert!(std::sync::Arc::ptr_eq(&document, &h.app.session.doc), "navigating changes nothing in the design");
}

#[test]
fn fit_home_and_view_keys_act_on_the_page() {
    let mut h = Harness::new();
    let ctx = crate::fonts::test_context();
    symbol_open(&mut h);
    // Settle the Design camera, so any change below is the page's doing.
    h.app.cam_anim = None;
    h.app.pending_fit = None;
    let design = h.app.cam;
    let mut time = 0.0;
    frame(&mut h.app, &ctx, &mut time, vec![]);
    let fitted = h.app.electronics.page.cam;
    for command in ["view.fit", "view.home"] {
        h.app.electronics.page.cam.target.x += 20.0;
        h.app.run(command, json!({})).unwrap();
        frame(&mut h.app, &ctx, &mut time, vec![]);
        assert_eq!(h.app.electronics.page.cam, fitted, "{command} frames the page");
    }
    assert!(crate::navigation::view_key(&mut h.app, &ctx, Key::ArrowRight, Modifiers::CTRL, false));
    assert_ne!(h.app.electronics.page.cam.target, fitted.target, "Ctrl+arrows pan the page");
    assert!(!crate::navigation::view_key(&mut h.app, &ctx, Key::ArrowRight, Modifiers::NONE, false), "a flat page has nothing to turn");
    h.app.start("view.front");
    h.app.run("select.set", json!({"items": [{"type": "plane", "name": "XY"}]})).unwrap();
    h.app.start("view.normal_to");
    h.frames(2);
    assert_eq!(h.app.cam, design, "view keys and standard views leave the Design camera alone");
}

#[test]
fn the_control_channel_names_the_workspace_and_keeps_the_tabs_apart() {
    let mut h = Harness::new();
    let workspace = |h: &mut Harness, params: serde_json::Value| h.call("ui.workspace", params);
    assert_eq!(workspace(&mut h, json!({}))["result"]["workspace"], "design");
    assert_eq!(workspace(&mut h, json!({"workspace": "electronics"}))["ok"], false, "a 3D design has none");
    assert_eq!(h.call("ui.set", json!({"tab": LIBRARY}))["ok"], false, "an electronics tab is an electronics document's");
    assert_eq!(workspace(&mut h, json!({"workspace": "drawing"}))["ok"], false);
    // An agent opens an electronics design with the same command File > New runs.
    run(&mut h, "file.new", json!({"kind": "electronics_design"}));
    h.frames(2);
    assert!(active(&h.app));
    assert_eq!(workspace(&mut h, json!({}))["result"]["workspace"], "electronics");
    assert_eq!(workspace(&mut h, json!({"workspace": "electronics"}))["ok"], true);
    assert_eq!(workspace(&mut h, json!({"workspace": "render"}))["ok"], false);
    assert!(active(&h.app));
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.frames(2);
    assert!(!active(&h.app), "a sketch takes Design's view");
}

/// An 0603 resistor over the same commands an agent uses: its symbol, then its footprint, then
/// the component that uses both.
fn r0603(h: &mut Harness) {
    for (id, p) in [
        ("ecad.symbol.create", json!({"name": "R"})),
        (
            "ecad.pin.add",
            json!({"symbol": "R", "pins": [{"number": 1, "name": "A", "at": [-5.08, 0], "direction": "right"}, {"number": 2, "name": "B", "at": [5.08, 0], "direction": "left"}]}),
        ),
        ("ecad.footprint.create", json!({"name": "R0603"})),
        (
            "ecad.pad.add",
            json!({"footprint": "R0603", "pads": [{"number": 1, "at": [-0.8, 0], "size": [0.9, 1.0]}, {"number": 2, "at": [0.8, 0], "size": [0.9, 1.0]}]}),
        ),
    ] {
        run(h, id, p);
    }
}

#[test]
fn the_page_follows_the_item_an_agent_builds() {
    let mut h = Harness::new();
    h.call("ui.resize", json!({"width": 1440, "height": 900}));
    crate::documents::new_electronics(&mut h.app, DocumentKind::ElectronicsLibrary);
    r0603(&mut h);
    h.frames(3);
    // The last command added pads, so the page shows the footprint, framed around its copper.
    assert_eq!(h.app.electronics.view, crate::electronics_components::View::Footprint);
    let (one, two) = (crate::scenario::handle_at("electronics:pad:1").unwrap(), crate::scenario::handle_at("electronics:pad:2").unwrap());
    assert!((one.y - two.y).abs() < 0.5 && two.x > one.x, "pad 2 is right of pad 1 on one line: {one:?} {two:?}");
    let content = crate::shell::content_rect(&h.app, h.app.viewport.rect.unwrap());
    assert!(((one.x + two.x) / 2.0 - content.center().x).abs() < 1.0, "Fit centres the copper in the free area");
    let points_per_mm = (two.x - one.x) / 1.6;
    assert!(points_per_mm > 20.0, "an 0603 is framed large, not left at the empty page's scale: {points_per_mm}");
    // A pin edit shows the symbol: the pins' connecting ends, 10.16 mm apart.
    run(&mut h, "ecad.pin.edit", json!({"symbol": "R", "pin": 1, "name": "IN"}));
    h.frames(2);
    assert_eq!(h.app.electronics.view, crate::electronics_components::View::Symbol);
    let (a, b) = (crate::scenario::handle_at("electronics:pin:1").unwrap(), crate::scenario::handle_at("electronics:pin:2").unwrap());
    assert!((a.y - b.y).abs() < 0.5 && b.x > a.x, "{a:?} {b:?}");
    // The new component that uses both.
    run(&mut h, "ecad.component.create", json!({"name": "R0603", "prefix": "R", "symbol": "R", "footprint": "R0603"}));
    h.frames(2);
    assert_eq!(crate::electronics_components::shown(&h.app).map(|c| c.component.name.as_str()), Some("R0603"));
}

#[test]
fn the_library_card_lists_each_kind_and_opens_one() {
    let mut h = Harness::new();
    h.call("ui.resize", json!({"width": 1440, "height": 900}));
    crate::documents::new_electronics(&mut h.app, DocumentKind::ElectronicsLibrary);
    r0603(&mut h);
    run(&mut h, "ecad.component.create", json!({"name": "R0603", "prefix": "R", "symbol": "R", "footprint": "R0603"}));
    run(&mut h, "ecad.component.create", json!({"name": "LM358"}));
    h.frames(2);
    let document = h.app.session.doc.clone();
    let name = |h: &Harness| crate::electronics_components::shown(&h.app).map(|c| c.component.name.clone());
    assert_eq!(name(&h).as_deref(), Some("LM358"), "the newest is shown");
    h.step(&json!({"click": {"handle": "electronics:library:components"}})).unwrap();
    h.frames(2);
    h.step(&json!({"click": {"handle": "electronics:item:components:R0603"}})).unwrap();
    h.frames(2);
    assert_eq!(name(&h).as_deref(), Some("R0603"));
    assert!(std::sync::Arc::ptr_eq(&document, &h.app.session.doc), "looking changes nothing in the design");
    // The Symbols sub-tab lists the symbols; a click opens one in its editor.
    h.step(&json!({"click": {"handle": "electronics:library:symbols"}})).unwrap();
    h.frames(2);
    assert_eq!(h.app.electronics.card.kind, crate::electronics_library::Kind::Symbols);
    h.step(&json!({"click": {"handle": "electronics:item:symbols:R"}})).unwrap();
    h.frames(2);
    assert!(matches!(h.app.electronics.opened, Some(crate::electronics_library::Opened::Symbol(_))));
    assert_eq!(h.app.ui.tab, SYMBOL);
    assert!(std::sync::Arc::ptr_eq(&document, &h.app.session.doc));
}
