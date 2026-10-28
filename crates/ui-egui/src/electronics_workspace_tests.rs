use egui::{Event, Key, Modifiers, PointerButton, vec2};
use serde_json::json;

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

#[test]
fn the_workspace_opens_stays_open_for_commands_and_goes_back_to_design() {
    let mut h = Harness::new();
    enter(&mut h.app, true).unwrap();
    assert!(active(&h.app));
    assert!(crate::workspace::visible_tabs(&h.app).is_empty(), "Design's tabs are hidden");
    h.app.run("ecad.inspect", json!({})).unwrap();
    h.frames(2);
    assert!(active(&h.app), "a command run here does not close the workspace");
    enter(&mut h.app, false).unwrap();
    assert!(!active(&h.app));
    assert_eq!(h.app.ui.tab, "MODEL");
}

#[test]
fn opening_is_refused_while_sketching_and_a_sketch_goes_back_to_design() {
    let mut h = Harness::new();
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.frames(2);
    assert!(enter(&mut h.app, true).is_err());
    assert!(!active(&h.app));
    h.app.run("sketch.finish", json!({})).unwrap();
    h.frames(2);
    enter(&mut h.app, true).unwrap();
    // A sketch started here (by a shortcut or an agent) needs Design's view: the workspace leaves.
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.frames(2);
    assert!(!active(&h.app));
    assert_eq!(h.app.ui.tab, "SKETCH");
}

#[test]
fn render_and_electronics_replace_each_other() {
    let mut h = Harness::new();
    h.app.run("solid.box", json!({"length": 20, "width": 15, "height": 10})).unwrap();
    h.frames(2);
    let design = h.app.cam;
    crate::render_workspace::enter(&mut h.app, true).unwrap();
    enter(&mut h.app, true).unwrap();
    assert!(active(&h.app) && !crate::render_workspace::active(&h.app));
    assert_eq!(h.app.cam, design, "leaving Render for Electronics restores the Design camera");
    crate::render_workspace::enter(&mut h.app, true).unwrap();
    assert!(crate::render_workspace::active(&h.app) && !active(&h.app));
}

#[test]
fn a_new_design_opens_in_design() {
    let mut h = Harness::new();
    enter(&mut h.app, true).unwrap();
    h.app.run("file.new", json!({})).unwrap();
    h.frames(2);
    assert!(!active(&h.app));
}

#[test]
fn the_page_pans_and_zooms_without_orbit_and_leaves_the_design_camera() {
    let mut h = Harness::new();
    let ctx = crate::fonts::test_context();
    h.app.preferences.smooth_wheel_zoom = false;
    h.app.preferences.zoom_reverse = false;
    h.app.ui.perspective = true;
    h.app.cam_anim = None;
    h.app.pending_fit = None;
    enter(&mut h.app, true).unwrap();
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
    // Settle the Design camera, so any change below is the page's doing.
    h.app.cam_anim = None;
    h.app.pending_fit = None;
    enter(&mut h.app, true).unwrap();
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
fn the_control_channel_opens_the_workspace() {
    let mut h = Harness::new();
    let workspace = |h: &mut Harness, params: serde_json::Value| {
        let reply = h.call("ui.workspace", params);
        assert_eq!(reply["ok"], true, "{reply}");
        reply["result"]["workspace"].clone()
    };
    assert_eq!(workspace(&mut h, json!({"workspace": "electronics"})), "electronics");
    assert!(active(&h.app));
    assert_eq!(workspace(&mut h, json!({})), "electronics");
    assert_eq!(workspace(&mut h, json!({"workspace": "render"})), "render");
    assert!(!active(&h.app));
    assert_eq!(workspace(&mut h, json!({"workspace": "design"})), "design");
    assert_eq!(h.call("ui.workspace", json!({"workspace": "drawing"}))["ok"], false);
    assert_eq!(h.call("ui.set", json!({"tab": "ELECTRONICS"}))["ok"], true);
    assert!(active(&h.app), "ui.set reaches the workspace through the same guard");
    assert_eq!(h.call("ui.set", json!({"tab": "MODEL"}))["ok"], true);
    assert!(!active(&h.app));
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    assert_eq!(h.call("ui.workspace", json!({"workspace": "electronics"}))["ok"], false, "refused while sketching");
    assert!(!active(&h.app));
}

/// An 0603 resistor over the same commands an agent uses.
fn r0603(h: &mut Harness) {
    for (id, p) in [
        ("ecad.component.create", json!({"name": "R0603", "prefix": "R", "value": "10k"})),
        (
            "ecad.pin.add",
            json!({"component": "R0603", "pins": [{"number": 1, "name": "A", "at": [-5.08, 0], "direction": "right"}, {"number": 2, "name": "B", "at": [5.08, 0], "direction": "left"}]}),
        ),
        (
            "ecad.pad.add",
            json!({"component": "R0603", "pads": [{"number": 1, "at": [-0.8, 0], "size": [0.9, 1.0]}, {"number": 2, "at": [0.8, 0], "size": [0.9, 1.0]}]}),
        ),
    ] {
        let reply = h.call("engine.execute", json!({"command": id, "params": p}));
        assert_eq!(reply["ok"], true, "{id}: {reply}");
    }
}

#[test]
fn the_page_follows_the_component_an_agent_builds() {
    let mut h = Harness::new();
    h.call("ui.resize", json!({"width": 1440, "height": 900}));
    enter(&mut h.app, true).unwrap();
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
    // Symbol: the pins' connecting ends, 10.16 mm apart.
    h.step(&json!({"click": {"handle": "electronics:view:symbol"}})).unwrap();
    h.frames(2);
    let (a, b) = (crate::scenario::handle_at("electronics:pin:1").unwrap(), crate::scenario::handle_at("electronics:pin:2").unwrap());
    assert!((a.y - b.y).abs() < 0.5 && b.x > a.x, "{a:?} {b:?}");
}

#[test]
fn the_browser_lists_electronic_components_and_picks_one() {
    let mut h = Harness::new();
    h.call("ui.resize", json!({"width": 1440, "height": 900}));
    r0603(&mut h);
    let reply = h.call("engine.execute", json!({"command": "ecad.component.create", "params": {"name": "LM358"}}));
    assert_eq!(reply["ok"], true, "{reply}");
    h.call("ui.workspace", json!({"workspace": "electronics"}));
    h.frames(2);
    let document = h.app.session.doc.clone();
    assert_eq!(crate::electronics_components::shown(&h.app).map(|c| c.name.as_str()), Some("LM358"), "the newest is shown");
    h.step(&json!({"click": {"handle": "electronics:component:R0603"}})).unwrap();
    h.frames(2);
    assert_eq!(crate::electronics_components::shown(&h.app).map(|c| c.name.as_str()), Some("R0603"));
    assert!(std::sync::Arc::ptr_eq(&document, &h.app.session.doc), "looking changes nothing in the design");
    h.step(&json!({"click": {"handle": "electronics:browser-model"}})).unwrap();
    h.frames(2);
    assert!(crate::scenario::handle_at("electronics:component:R0603").is_none() || h.app.electronics.model_tab);
    assert!(h.app.electronics.model_tab, "the Browser card shows the model again");
}
