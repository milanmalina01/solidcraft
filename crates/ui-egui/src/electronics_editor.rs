//! The Electronics workspace's component editor. The Component panel beside the page edits the
//! shown electronic component: its properties, the picked pin or pad, the symbol and footprint
//! generators, which pin connects to which pad, its 3D package and its checks. On the page a press
//! picks a pin or a pad and a drag moves it on the grid (Ctrl or Cmd: off it). Every change is one
//! `ecad.*` command, so undo takes back one edit and the panel does what an agent does.

use std::collections::BTreeMap;

use egui::{Pos2, Rect, Ui};
use serde_json::{Map, Value, json};
use solidcraft_engine::doc::ecad::{ElectronicComponent, Pad, PadKind, PadShape, Pin, PinDirection, PinType, Problem, Severity, Side};

use crate::SolveApp;
use crate::electronics_components::{View, shown};
use crate::electronics_workspace::PageMap;
use crate::ui_kit;

/// A press this close to a pin, or to a pad's outline, picks it, in points.
const PICK_POINTS: f32 = 6.0;
/// A press on a pin or a pad moves it once the pointer has gone this far, in points.
const DRAG_POINTS: f32 = 3.0;
/// Pins move in whole steps of the schematic grid, so their ends stay on it.
const SYMBOL_GRID: f64 = 2.54;
/// The most generated box pins or chip pins the panel asks for.
const MAX_PINS: u32 = 1024;

const PACKAGES: [(&str, &str); 6] = [
    ("chip", "electronics.package_chip"),
    ("sot23", "electronics.package_sot23"),
    ("soic", "electronics.package_soic"),
    ("qfn", "electronics.package_qfn"),
    ("dip", "electronics.package_dip"),
    ("header", "electronics.package_header"),
];
const SIZES: [&str; 8] = ["0201", "0402", "0603", "0805", "1206", "1210", "2010", "2512"];
const STYLES: [(&str, &str); 3] =
    [("box", "electronics.style_box"), ("resistor", "electronics.style_resistor"), ("capacitor", "electronics.style_capacitor")];
const PIN_TYPES: [(PinType, &str); 11] = [
    (PinType::Passive, "passive"),
    (PinType::Input, "input"),
    (PinType::Output, "output"),
    (PinType::Bidirectional, "bidirectional"),
    (PinType::TriState, "tri_state"),
    (PinType::PowerInput, "power_input"),
    (PinType::PowerOutput, "power_output"),
    (PinType::OpenCollector, "open_collector"),
    (PinType::OpenEmitter, "open_emitter"),
    (PinType::Unspecified, "unspecified"),
    (PinType::NotConnected, "not_connected"),
];
const DIRECTIONS: [(PinDirection, &str); 4] =
    [(PinDirection::Right, "right"), (PinDirection::Left, "left"), (PinDirection::Up, "up"), (PinDirection::Down, "down")];
const SHAPES: [(PadShape, &str); 4] =
    [(PadShape::Rect, "rect"), (PadShape::Round, "round"), (PadShape::Oval, "oval"), (PadShape::RoundedRect, "rounded_rect")];
const KINDS: [(PadKind, &str); 3] = [(PadKind::Smd, "smd"), (PadKind::ThroughHole, "through_hole"), (PadKind::Hole, "hole")];
const SIDES: [(Side, &str); 2] = [(Side::Top, "top"), (Side::Bottom, "bottom")];

/// A pin or a pad of the shown electronic component, by number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Pin(String),
    Pad(String),
}

impl Item {
    fn is(&self, pin: bool, number: &str) -> bool {
        match self {
            Item::Pin(n) => pin && n == number,
            Item::Pad(n) => !pin && n == number,
        }
    }
}

/// A press on a pin or a pad.
struct Drag {
    item: Item,
    /// Where the press was, on screen and on the page.
    press: Pos2,
    from: [f64; 2],
    /// How far the item has moved, in page millimetres.
    by: [f64; 2],
    moving: bool,
}

pub struct Editor {
    /// The electronic component the pick and the typing belong to.
    of: Option<u64>,
    pub picked: Option<Item>,
    drag: Option<Drag>,
    /// The text of the fields being typed in, by field: each runs its edit when it gives up the
    /// keyboard.
    typed: BTreeMap<String, String>,
    package: usize,
    size: usize,
    pins: String,
    pitch: String,
    body: [String; 2],
    thermal: [String; 2],
    rows: usize,
    style: usize,
    box_pins: String,
    width: f32,
}

impl Default for Editor {
    fn default() -> Self {
        Editor {
            of: None,
            picked: None,
            drag: None,
            typed: BTreeMap::new(),
            package: 0,
            size: 2,
            pins: String::new(),
            pitch: String::new(),
            body: ["3".into(), "3".into()],
            thermal: [String::new(), String::new()],
            rows: 0,
            style: 0,
            box_pins: "4".into(),
            width: f32::NAN,
        }
    }
}

/// Forget a pick that no longer names a pin or pad of the shown view, and the typing for another
/// electronic component.
fn sync(app: &mut SolveApp) {
    let id = shown(app).map(|c| c.id);
    let keep = match (&app.electronics.editor.picked, shown(app)) {
        (Some(Item::Pin(n)), Some(c)) => app.electronics.view == View::Symbol && c.symbol.pin(n).is_some(),
        (Some(Item::Pad(n)), Some(c)) => app.electronics.view == View::Footprint && c.footprint.pad(n).is_some(),
        _ => false,
    };
    let e = &mut app.electronics.editor;
    let other = e.of != id;
    if other {
        e.of = id;
        e.typed.clear();
    }
    if !keep || other {
        e.picked = None;
        e.drag = None;
    }
}

/// How far the pin (`pin`) or pad numbered `number` has been dragged, in page millimetres.
pub(crate) fn moved_by(app: &SolveApp, pin: bool, number: &str) -> [f64; 2] {
    match &app.electronics.editor.drag {
        Some(d) if d.moving && d.item.is(pin, number) => d.by,
        _ => [0.0; 2],
    }
}

/// Whether the pin (`pin`) or pad numbered `number` is picked.
pub(crate) fn is_picked(app: &SolveApp, pin: bool, number: &str) -> bool {
    app.electronics.editor.picked.as_ref().is_some_and(|i| i.is(pin, number))
}

/// The pin or pad under `p` on the page.
fn hit(app: &SolveApp, map: PageMap, p: Pos2) -> Option<Item> {
    let c = shown(app)?;
    match app.electronics.view {
        View::Symbol => c
            .symbol
            .pins
            .iter()
            .map(|pin| {
                let (a, b) = (map.to_screen(pin.at[0], pin.at[1]), map.to_screen(pin.body_end()[0], pin.body_end()[1]));
                (segment_distance(p, a, b), pin)
            })
            .filter(|(d, _)| *d <= PICK_POINTS)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, pin)| Item::Pin(pin.number.clone())),
        // The last drawn is on top.
        View::Footprint => c
            .footprint
            .pads
            .iter()
            .rev()
            .find(|pad| {
                let outline: Vec<Pos2> = pad.outline(6).iter().map(|q| map.to_screen(q[0], q[1])).collect();
                inside(&outline, p) || edge_distance(&outline, p) <= PICK_POINTS / 2.0
            })
            .map(|pad| Item::Pad(pad.number.clone())),
    }
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

/// Whether `p` is inside a convex outline, whichever way round it runs.
fn inside(outline: &[Pos2], p: Pos2) -> bool {
    if outline.len() < 3 {
        return false;
    }
    let mut sign = 0.0f32;
    for (i, &a) in outline.iter().enumerate() {
        let b = outline[(i + 1) % outline.len()];
        let cross = (b - a).x * (p - a).y - (b - a).y * (p - a).x;
        if !cross.is_finite() {
            return false;
        }
        if cross != 0.0 {
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return false;
            }
        }
    }
    true
}

fn edge_distance(outline: &[Pos2], p: Pos2) -> f32 {
    (0..outline.len()).map(|i| segment_distance(p, outline[i], outline[(i + 1) % outline.len()])).fold(f32::INFINITY, f32::min)
}

/// The page's pointer: a press on a pin or a pad picks it and a drag moves it; a click on the empty
/// page drops the pick. `true` while a press on a pin or pad holds the pointer, so the page doesn't
/// navigate.
pub(crate) fn page_input(app: &mut SolveApp, ui: &Ui, rect: Rect, response: &egui::Response) -> bool {
    sync(app);
    let Some(map) = PageMap::new(&app.electronics.page.cam, rect) else { return false };
    let (pressed, down, pos, free) =
        ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos(), i.modifiers.command || i.modifiers.ctrl));
    if pressed
        && response.hovered()
        && let Some(p) = pos
        && let Some(item) = hit(app, map, p)
    {
        let (x, y) = map.to_page(p);
        app.electronics.editor.picked = Some(item.clone());
        app.electronics.editor.drag = Some(Drag { item, press: p, from: [x, y], by: [0.0; 2], moving: false });
        return true;
    }
    let step = match app.electronics.view {
        View::Symbol => SYMBOL_GRID,
        View::Footprint => solidcraft_engine::view::grid_step(app.electronics.page.cam.half_height()).0,
    };
    let Some(drag) = app.electronics.editor.drag.as_mut() else {
        let over = response.hovered().then_some(pos).flatten().and_then(|p| hit(app, map, p));
        if over.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        } else if response.clicked() {
            app.electronics.editor.picked = None;
        }
        return false;
    };
    if drag.moving {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    if down {
        if let Some(p) = pos {
            drag.moving |= p.distance(drag.press) > DRAG_POINTS;
            let (x, y) = map.to_page(p);
            let raw = [x - drag.from[0], y - drag.from[1]];
            if drag.moving && raw.iter().all(|v| v.is_finite()) {
                drag.by = if free || !(step.is_finite() && step > 0.0) { raw } else { raw.map(|v| (v / step).round() * step) };
            }
        }
        return true;
    }
    if let Some(drag) = app.electronics.editor.drag.take()
        && drag.moving
        && drag.by != [0.0; 2]
    {
        move_item(app, &drag.item, drag.by);
    }
    true
}

/// A millimetre value without the float noise of adding steps: to the nearest nanometre.
fn tidy(v: f64) -> f64 {
    (v * 1e6).round() / 1e6 + 0.0
}

fn move_item(app: &mut SolveApp, item: &Item, by: [f64; 2]) {
    let Some(c) = shown(app) else { return };
    let to = |at: [f64; 2]| json!([tidy(at[0] + by[0]), tidy(at[1] + by[1])]);
    let (id, params) = match item {
        Item::Pin(n) => match c.symbol.pin(n) {
            Some(pin) => ("ecad.pin.edit", json!({"component": c.id, "pin": n, "at": to(pin.at)})),
            None => return,
        },
        Item::Pad(n) => match c.footprint.pad(n) {
            Some(pad) => ("ecad.pad.edit", json!({"component": c.id, "pad": n, "at": to(pad.at)})),
            None => return,
        },
    };
    drop(app.run(id, params));
}

/// The Delete key in the workspace: delete the picked pin or pad. `true` when there was one.
pub(crate) fn delete_picked(app: &mut SolveApp) -> bool {
    sync(app);
    let (Some(item), Some(id)) = (app.electronics.editor.picked.take(), shown(app).map(|c| c.id)) else { return false };
    app.electronics.editor.drag = None;
    let (command, key, number) = match item {
        Item::Pin(n) => ("ecad.pin.delete", "pins", n),
        Item::Pad(n) => ("ecad.pad.delete", "pads", n),
    };
    let mut params = json!({"component": id});
    params[key] = json!([number]);
    drop(app.run(command, params));
    true
}

/// Escape in the workspace: a drag ends where it began, else the pick is dropped. `true` when
/// there was either.
pub(crate) fn escape(app: &mut SolveApp) -> bool {
    let e = &mut app.electronics.editor;
    e.drag.take().is_some() || e.picked.take().is_some()
}

/// The Component panel, on the right of the page.
pub(crate) fn panel(app: &mut SolveApp, ui: &mut Ui) {
    sync(app);
    let bounds = crate::shell::panel_bounds(app, ui.available_rect_before_wrap());
    // The Browser card is on the left; the panel stays on the right.
    let mut side = String::from("right");
    let mut width = app.electronics.editor.width;
    let title = app.tr("electronics.editor_title").to_string();
    let content = ui_kit::Panel::workspace(
        ui,
        ui_kit::WorkspaceOptions { id: "electronics:editor", title: &title, icon: "component", bounds, fill_height: true },
        &mut side,
        &mut width,
        |ui| {
            egui::ScrollArea::vertical().id_salt("electronics_editor").auto_shrink([false, false]).show(ui, |ui| body(app, ui));
        },
    );
    app.electronics.editor.width = width;
    let rect = content.map(|c| c.response.rect);
    crate::shell::register_panel(app, "electronics:editor", rect, Some(ui_kit::Edge::Right));
    if let Some(rect) = rect {
        crate::scenario::publish_rect("electronics:editor", rect);
    }
}

fn body(app: &mut SolveApp, ui: &mut Ui) {
    actions(app, ui);
    let Some(c) = shown(app).cloned() else {
        ui_kit::text(ui, app.tr("electronics.editor_empty"), ui_kit::Text::Note("electronics.editor_empty"));
        return;
    };
    let title = app.tr("electronics.section_properties").to_string();
    ui_kit::section(ui, "electronics_properties", &title, true, |ui| properties(app, ui, &c));
    match app.electronics.editor.picked.clone() {
        Some(Item::Pin(n)) => {
            if let Some(pin) = c.symbol.pin(&n) {
                pin_section(app, ui, &c, pin);
            }
        }
        Some(Item::Pad(n)) => {
            if let Some(pad) = c.footprint.pad(&n) {
                pad_section(app, ui, &c, pad);
            }
        }
        None => {}
    }
    // The checks next to what is being edited, so a change shows its effect at once.
    let title = app.tr("electronics.section_checks").to_string();
    ui_kit::section(ui, "electronics_checks", &title, true, |ui| checks(app, ui, &c));
    match app.electronics.view {
        View::Symbol => {
            let title = app.tr("electronics.section_symbol").to_string();
            ui_kit::section(ui, "electronics_symbol", &title, true, |ui| symbol_tools(app, ui, &c));
        }
        View::Footprint => {
            let title = app.tr("electronics.section_footprint").to_string();
            ui_kit::section(ui, "electronics_footprint", &title, true, |ui| footprint_tools(app, ui, &c));
        }
    }
    if !c.symbol.pins.is_empty() && !c.footprint.pads.is_empty() {
        let title = app.tr("electronics.section_connections").to_string();
        ui_kit::section(ui, "electronics_connections", &title, true, |ui| connections(app, ui, &c));
    }
    let title = app.tr("electronics.section_package").to_string();
    ui_kit::section(ui, "electronics_package", &title, false, |ui| package(app, ui, &c));
}

/// A button, published for scenarios as `electronics:editor:<key>`.
fn button(ui: &mut Ui, key: &str, label: &str, enabled: bool) -> bool {
    let response = ui.add_enabled_ui(enabled, |ui| ui_kit::button(ui, label, ui_kit::Button::Normal)).inner;
    crate::scenario::publish_target(&format!("electronics:editor:{key}"), response.rect);
    enabled && response.clicked()
}

/// New, Copy and Delete for the electronic components.
fn actions(app: &mut SolveApp, ui: &mut Ui) {
    let labels = [
        app.tr("electronics.editor_new").to_string(),
        app.tr("electronics.editor_copy").to_string(),
        app.tr("electronics.editor_delete").to_string(),
    ];
    let shown = shown(app).map(|c| (c.id, c.name.clone()));
    let mut clicked = None;
    ui.horizontal(|ui| {
        for (i, (key, label)) in ["new", "copy", "delete"].into_iter().zip(&labels).enumerate() {
            if button(ui, key, label, i == 0 || shown.is_some()) {
                clicked = Some(key);
            }
        }
    });
    match (clicked, shown) {
        (Some("new"), _) => {
            if let Some(name) = free_name(app, |n| crate::i18n::format("electronics.new_name", &[("n", &n.to_string())])) {
                drop(app.run("ecad.component.create", json!({"name": name})));
            }
        }
        (Some("copy"), Some((id, name))) => {
            let copy = |n: usize| {
                if n == 1 {
                    crate::i18n::format("electronics.copy_name", &[("name", &name)])
                } else {
                    crate::i18n::format("electronics.copy_name_n", &[("name", &name), ("n", &n.to_string())])
                }
            };
            if let Some(copy) = free_name(app, copy) {
                drop(app.run("ecad.component.copy", json!({"component": id, "name": copy})));
            }
        }
        (Some("delete"), Some((id, _))) => drop(app.run("ecad.component.delete", json!({"components": [id]}))),
        _ => {}
    }
}

/// The first of `name(1)`, `name(2)`… no electronic component has.
fn free_name(app: &SolveApp, name: impl Fn(usize) -> String) -> Option<String> {
    (1..=10_000).map(name).find(|n| app.session.doc.ecad.named(n).is_none())
}

/// A text field that runs its edit when it gives up the keyboard (Enter, Tab or a click
/// elsewhere); Escape puts the old text back. `Some` with the new text when it changed.
fn field(app: &mut SolveApp, ui: &mut Ui, key: &str, label: &str, current: &str) -> Option<String> {
    let mut text = typed(app, key, current);
    let response = ui_kit::row(ui, label, |ui| ui_kit::cell_editor(ui, egui::Id::new(("electronics_field", key)), &mut text, "", false));
    settle(app, ui, key, &response, text, current)
}

/// What the field `key` shows: the text typed in it while it has the keyboard, else `current`.
fn typed(app: &SolveApp, key: &str, current: &str) -> String {
    app.electronics.editor.typed.get(key).cloned().unwrap_or_else(|| current.to_string())
}

/// After a field is drawn: keep its text while it has the keyboard, and `Some` with the text when
/// it has just given the keyboard up changed (not with Escape).
fn settle(app: &mut SolveApp, ui: &Ui, key: &str, response: &egui::Response, text: String, current: &str) -> Option<String> {
    let typed = &mut app.electronics.editor.typed;
    if response.has_focus() {
        typed.insert(key.to_string(), text);
        return None;
    }
    let was_typed = typed.remove(key).is_some();
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    (response.lost_focus() && was_typed && !escape && text != current).then_some(text)
}

/// An x and a y in millimetres side by side in one row; `Some` with both when either changed.
fn pair(app: &mut SolveApp, ui: &mut Ui, key: &str, label: &str, current: [f64; 2]) -> Option<[f64; 2]> {
    let keys = [format!("{key}:x"), format!("{key}:y")];
    let olds = current.map(mm);
    let mut texts = [typed(app, &keys[0], &olds[0]), typed(app, &keys[1], &olds[1])];
    let responses = ui_kit::row(ui, label, |ui| {
        let gap = ui.spacing().item_spacing.x;
        let width = ((ui.available_width() - gap) / 2.0).max(0.0);
        let [a, b] = &mut texts;
        ui.horizontal(|ui| {
            [(&keys[0], a), (&keys[1], b)].map(|(key, text)| {
                ui.allocate_ui(egui::vec2(width, ui_kit::row_height()), |ui| {
                    ui_kit::cell_editor(ui, egui::Id::new(("electronics_field", key)), text, "", false)
                })
                .inner
            })
        })
        .inner
    });
    let [ta, tb] = texts;
    let changed = [settle(app, ui, &keys[0], &responses[0], ta, &olds[0]), settle(app, ui, &keys[1], &responses[1], tb, &olds[1])];
    if changed.iter().all(Option::is_none) {
        return None;
    }
    let mut out = current;
    for (i, text) in changed.iter().enumerate() {
        if let Some(text) = text {
            match crate::prefs::parse_number(text) {
                Some(v) => out[i] = v,
                None => {
                    app.set_status(crate::i18n::format("electronics.not_a_number", &[("field", label), ("text", text)]), true);
                    return None;
                }
            }
        }
    }
    Some(out)
}

/// A number field in millimetres (or degrees); a text that isn't a number says so and changes
/// nothing.
fn number(app: &mut SolveApp, ui: &mut Ui, key: &str, label: &str, current: f64) -> Option<f64> {
    let text = field(app, ui, key, label, &mm(current))?;
    let value = crate::prefs::parse_number(&text);
    if value.is_none() {
        app.set_status(crate::i18n::format("electronics.not_a_number", &[("field", label), ("text", &text)]), true);
    }
    value
}

/// A dropdown row; `Some` with the new choice when it changed.
fn choice(ui: &mut Ui, id: &str, label: &str, selected: usize, options: &[String]) -> Option<usize> {
    let mut index = selected;
    let options: Vec<&str> = options.iter().map(String::as_str).collect();
    ui_kit::choice_row(ui, label, id, &mut index, &options);
    (index != selected).then_some(index)
}

/// Millimetres for a field: up to four decimals, no trailing zeros.
fn mm(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn tr(key: &str) -> String {
    crate::i18n::tr(key).to_string()
}

fn translated<T>(prefix: &str, options: &[(T, &str)]) -> Vec<String> {
    options.iter().map(|(_, key)| tr(&format!("{prefix}{key}"))).collect()
}

fn properties(app: &mut SolveApp, ui: &mut Ui, c: &ElectronicComponent) {
    let rows = [
        ("name", "electronics.field_name", &c.name),
        ("prefix", "electronics.field_prefix", &c.prefix),
        ("value", "electronics.field_value", &c.value),
        ("mpn", "electronics.field_mpn", &c.mpn),
        ("manufacturer", "electronics.field_manufacturer", &c.manufacturer),
        ("description", "electronics.field_description", &c.description),
    ];
    for (key, label, current) in rows {
        let label = tr(label);
        if let Some(text) = field(app, ui, &format!("{}:{key}", c.id), &label, current) {
            let mut params = json!({"component": c.id});
            params[key] = json!(text);
            drop(app.run("ecad.component.edit", params));
        }
    }
}

/// Run an edit of the picked pin or pad; a new number keeps it picked.
fn edit_item(app: &mut SolveApp, command: &str, mut params: Map<String, Value>, renumbered: Option<Item>) {
    if params.len() <= 2 {
        return;
    }
    params.retain(|_, v| !v.is_null());
    if app.run(command, Value::Object(params)).is_ok()
        && let Some(item) = renumbered
    {
        app.electronics.editor.picked = Some(item);
    }
}

fn pin_section(app: &mut SolveApp, ui: &mut Ui, c: &ElectronicComponent, pin: &Pin) {
    let title = crate::i18n::format("electronics.section_pin", &[("number", &pin.number)]);
    let types = translated("electronics.pin_", &PIN_TYPES);
    let directions = translated("electronics.direction_", &DIRECTIONS);
    let delete = tr("electronics.delete_pin");
    ui_kit::section(ui, "electronics_pin", &title, true, |ui| {
        let key = |f: &str| format!("{}:pin:{}:{f}", c.id, pin.number);
        let mut edit = Map::new();
        edit.insert("component".into(), json!(c.id));
        edit.insert("pin".into(), json!(pin.number));
        let mut renumbered = None;
        if let Some(n) = field(app, ui, &key("number"), &tr("electronics.field_number"), &pin.number) {
            renumbered = Some(Item::Pin(n.clone()));
            edit.insert("number".into(), json!(n));
        }
        if let Some(name) = field(app, ui, &key("name"), &tr("electronics.field_pin_name"), &pin.name) {
            edit.insert("name".into(), json!(name));
        }
        let at = PIN_TYPES.iter().position(|(t, _)| *t == pin.electrical).unwrap_or(0);
        if let Some(i) = choice(ui, &key("electrical"), &tr("electronics.field_pin_type"), at, &types) {
            edit.insert("electrical".into(), json!(PIN_TYPES[i].1));
        }
        let at = DIRECTIONS.iter().position(|(d, _)| *d == pin.direction).unwrap_or(0);
        if let Some(i) = choice(ui, &key("direction"), &tr("electronics.field_direction"), at, &directions) {
            edit.insert("direction".into(), json!(DIRECTIONS[i].1));
        }
        if let Some(v) = number(app, ui, &key("length"), &tr("electronics.field_length"), pin.length) {
            edit.insert("length".into(), json!(v));
        }
        if let Some(at) = pair(app, ui, &key("at"), &tr("electronics.field_position"), pin.at) {
            edit.insert("at".into(), json!(at));
        }
        edit_item(app, "ecad.pin.edit", edit, renumbered);
        if button(ui, "delete-pin", &delete, true) {
            drop(app.run("ecad.pin.delete", json!({"component": c.id, "pins": [pin.number]})));
        }
    });
}

/// A drill as typed: `d`, or `w × h` for a slot.
fn parse_drill(text: &str) -> Option<Value> {
    let parts: Vec<f64> = text.split(['x', '×', ';']).map(crate::prefs::parse_number).collect::<Option<_>>()?;
    match parts[..] {
        [d] => Some(json!(d)),
        [w, h] => Some(json!([w, h])),
        _ => None,
    }
}

fn drill_text(drill: [f64; 2]) -> String {
    if drill[0] == drill[1] { mm(drill[0]) } else { format!("{} × {}", mm(drill[0]), mm(drill[1])) }
}

/// A drill for a pad that gets one: most of its shorter side.
fn default_drill(pad: &Pad) -> f64 {
    tidy(pad.size[0].min(pad.size[1]) * 0.6)
}

fn pad_section(app: &mut SolveApp, ui: &mut Ui, c: &ElectronicComponent, pad: &Pad) {
    let title = crate::i18n::format("electronics.section_pad", &[("number", &pad.number)]);
    let shapes = translated("electronics.shape_", &SHAPES);
    let kinds = translated("electronics.kind_", &KINDS);
    let sides = translated("electronics.side_", &SIDES);
    let delete = tr("electronics.delete_pad");
    ui_kit::section(ui, "electronics_pad", &title, true, |ui| {
        let key = |f: &str| format!("{}:pad:{}:{f}", c.id, pad.number);
        let mut edit = Map::new();
        edit.insert("component".into(), json!(c.id));
        edit.insert("pad".into(), json!(pad.number));
        let mut renumbered = None;
        if let Some(n) = field(app, ui, &key("number"), &tr("electronics.field_number"), &pad.number) {
            renumbered = Some(Item::Pad(n.clone()));
            edit.insert("number".into(), json!(n));
        }
        if let Some(at) = pair(app, ui, &key("at"), &tr("electronics.field_position"), pad.at) {
            edit.insert("at".into(), json!(at));
        }
        if let Some(size) = pair(app, ui, &key("size"), &tr("electronics.field_size_mm"), pad.size) {
            edit.insert("size".into(), json!(size));
        }
        if let Some(r) = number(app, ui, &key("rotation"), &tr("electronics.field_rotation"), pad.rotation) {
            edit.insert("rotation".into(), json!(r));
        }
        let at = SHAPES.iter().position(|(s, _)| *s == pad.shape).unwrap_or(0);
        if let Some(i) = choice(ui, &key("shape"), &tr("electronics.field_shape"), at, &shapes) {
            edit.insert("shape".into(), json!(SHAPES[i].1));
        }
        if pad.shape == PadShape::RoundedRect
            && let Some(v) = number(app, ui, &key("corner"), &tr("electronics.field_corner"), pad.corner)
        {
            edit.insert("corner".into(), json!(v));
        }
        let at = KINDS.iter().position(|(k, _)| *k == pad.kind).unwrap_or(0);
        if let Some(i) = choice(ui, &key("kind"), &tr("electronics.field_kind"), at, &kinds) {
            edit.insert("kind".into(), json!(KINDS[i].1));
            // A pad that gets a hole needs a drill.
            if KINDS[i].0 != PadKind::Smd && pad.drill.is_none() {
                edit.insert("drill".into(), json!(default_drill(pad)));
            }
        }
        match pad.kind {
            PadKind::Smd => {
                let at = SIDES.iter().position(|(s, _)| *s == pad.side).unwrap_or(0);
                if let Some(i) = choice(ui, &key("side"), &tr("electronics.field_side"), at, &sides) {
                    edit.insert("side".into(), json!(SIDES[i].1));
                }
            }
            PadKind::ThroughHole | PadKind::Hole => {
                let label = tr("electronics.field_drill");
                let current = pad.drill.map(drill_text).unwrap_or_default();
                if let Some(text) = field(app, ui, &key("drill"), &label, &current) {
                    match parse_drill(&text) {
                        Some(d) => {
                            edit.insert("drill".into(), d);
                        }
                        None => app.set_status(crate::i18n::format("electronics.not_a_number", &[("field", &label), ("text", &text)]), true),
                    }
                }
            }
        }
        edit_item(app, "ecad.pad.edit", edit, renumbered);
        if button(ui, "delete-pad", &delete, true) {
            drop(app.run("ecad.pad.delete", json!({"component": c.id, "pads": [pad.number]})));
        }
    });
}

/// The next free number after the highest numbered one.
fn next_number<'a>(taken: impl Iterator<Item = &'a str> + Clone) -> String {
    let start = taken.clone().filter_map(|n| n.parse::<u64>().ok()).max().map_or(1, |n| n.saturating_add(1));
    (start..start.saturating_add(10_000)).map(|n| n.to_string()).find(|n| !taken.clone().any(|t| t == n)).unwrap_or_default()
}

/// Add a pin under the last one, the same way round, and pick it.
fn add_pin(app: &mut SolveApp, c: &ElectronicComponent) {
    let number = next_number(c.symbol.pins.iter().map(|p| p.number.as_str()));
    let (at, direction, length) = match c.symbol.pins.last() {
        Some(last) => {
            let dir = DIRECTIONS.iter().find(|(d, _)| *d == last.direction).map_or("right", |(_, s)| *s);
            ([tidy(last.at[0]), tidy(last.at[1] - SYMBOL_GRID)], dir, last.length)
        }
        None => ([-3.0 * SYMBOL_GRID, 0.0], "right", SYMBOL_GRID),
    };
    let pin = json!({"number": number, "at": at, "direction": direction, "length": length});
    if app.run("ecad.pin.add", json!({"component": c.id, "pins": [pin]})).is_ok() {
        app.electronics.editor.picked = Some(Item::Pin(number));
    }
}

/// Add a pad like the last one, carrying on its row (or beside it), and pick it.
fn add_pad(app: &mut SolveApp, c: &ElectronicComponent) {
    let pads = &c.footprint.pads;
    let number = next_number(pads.iter().map(|p| p.number.as_str()));
    let mut pad = match pads.last() {
        Some(last) => {
            let step = match pads.len() {
                n if n >= 2 => [last.at[0] - pads[n - 2].at[0], last.at[1] - pads[n - 2].at[1]],
                _ => [last.size[0].max(last.size[1]) * 1.5, 0.0],
            };
            let mut pad = json!({
                "at": [tidy(last.at[0] + step[0]), tidy(last.at[1] + step[1])],
                "size": last.size,
                "rotation": last.rotation,
                "shape": SHAPES.iter().find(|(s, _)| *s == last.shape).map_or("rect", |(_, s)| *s),
                "kind": KINDS.iter().find(|(k, _)| *k == last.kind).map_or("smd", |(_, s)| *s),
            });
            if last.shape == PadShape::RoundedRect {
                pad["corner"] = json!(last.corner);
            }
            if let Some(d) = last.drill {
                pad["drill"] = json!(d);
            }
            if last.kind == PadKind::Smd {
                pad["side"] = json!(SIDES.iter().find(|(s, _)| *s == last.side).map_or("top", |(_, s)| *s));
            }
            pad
        }
        None => json!({"at": [0, 0], "size": [1, 1]}),
    };
    pad["number"] = json!(number);
    if app.run("ecad.pad.add", json!({"component": c.id, "pads": [pad]})).is_ok() {
        app.electronics.editor.picked = Some(Item::Pad(number));
    }
}

/// Add Pin, and the symbol generator.
fn symbol_tools(app: &mut SolveApp, ui: &mut Ui, c: &ElectronicComponent) {
    if button(ui, "add-pin", &tr("electronics.add_pin"), true) {
        add_pin(app, c);
    }
    let styles = translated("", &STYLES);
    let label = tr("electronics.field_style");
    if let Some(i) = choice(ui, "electronics_style", &label, app.electronics.editor.style, &styles) {
        app.electronics.editor.style = i;
    }
    let style = STYLES.get(app.electronics.editor.style).map_or("box", |s| s.0);
    let copper: Vec<&str> = c.footprint.pads.iter().filter(|p| p.kind != PadKind::Hole).map(|p| p.number.as_str()).collect();
    if style == "box" {
        if copper.is_empty() {
            let label = tr("electronics.field_pins");
            ui_kit::text_row(ui, &label, &mut app.electronics.editor.box_pins, "4");
        } else {
            let note = crate::i18n::format("electronics.box_from_pads", &[("n", &copper.len().to_string())]);
            ui_kit::text(ui, &note, ui_kit::Text::Note("electronics.box_from_pads"));
        }
    }
    if !c.symbol.is_empty() {
        ui_kit::text(ui, app.tr("electronics.symbol_replaces"), ui_kit::Text::Note("electronics.symbol_replaces"));
    }
    if !button(ui, "generate-symbol", &tr("electronics.generate_symbol"), true) {
        return;
    }
    let mut params = json!({"component": c.id, "style": style});
    if style == "box" {
        let pins: Vec<Value> = if copper.is_empty() {
            let text = app.electronics.editor.box_pins.clone();
            let Some(n) = count(app, &text, "electronics.field_pins") else { return };
            (1..=n).map(|n| json!({"number": n.to_string()})).collect()
        } else {
            copper.iter().map(|n| json!({"number": n})).collect()
        };
        params["pins"] = json!(pins);
    }
    if app.run("ecad.symbol.generate", params).is_ok() {
        app.electronics.editor.picked = None;
    }
}

/// A whole number from 1 to [`MAX_PINS`] typed in the field labelled `label`.
fn count(app: &mut SolveApp, text: &str, label: &str) -> Option<u32> {
    let n = text.trim().parse::<u32>().ok().filter(|n| (1..=MAX_PINS).contains(n));
    if n.is_none() {
        let field = tr(label);
        app.set_status(crate::i18n::format("electronics.not_a_count", &[("field", &field), ("max", &MAX_PINS.to_string())]), true);
    }
    n
}

/// Add Pad, Make Courtyard, and the footprint generator.
fn footprint_tools(app: &mut SolveApp, ui: &mut Ui, c: &ElectronicComponent) {
    ui.horizontal(|ui| {
        if button(ui, "add-pad", &tr("electronics.add_pad"), true) {
            add_pad(app, c);
        }
        if button(ui, "courtyard", &tr("electronics.make_courtyard"), !c.footprint.pads.is_empty()) {
            drop(app.run("ecad.footprint.courtyard", json!({"component": c.id})));
        }
    });
    let packages = translated("", &PACKAGES);
    let labels = [
        "electronics.field_package",
        "electronics.field_size",
        "electronics.field_pins",
        "electronics.field_pitch",
        "electronics.field_body_x",
        "electronics.field_body_y",
        "electronics.field_thermal_x",
        "electronics.field_thermal_y",
        "electronics.field_rows",
    ]
    .map(tr);
    let default = tr("electronics.default_hint");
    let e = &mut app.electronics.editor;
    let mut package = e.package;
    let options: Vec<&str> = packages.iter().map(String::as_str).collect();
    ui_kit::choice_row(ui, &labels[0], "electronics_package", &mut package, &options);
    e.package = package.min(PACKAGES.len() - 1);
    let kind = PACKAGES[e.package].0;
    match kind {
        "chip" => {
            ui_kit::choice_row(ui, &labels[1], "electronics_size", &mut e.size, &SIZES);
        }
        "sot23" => {}
        _ => {
            let hint = match kind {
                "qfn" => "16",
                "header" => "4",
                _ => "8",
            };
            ui_kit::text_row(ui, &labels[2], &mut e.pins, hint);
            ui_kit::text_row(ui, &labels[3], &mut e.pitch, &default);
        }
    }
    if kind == "qfn" {
        ui_kit::text_row(ui, &labels[4], &mut e.body[0], "");
        ui_kit::text_row(ui, &labels[5], &mut e.body[1], "");
        ui_kit::text_row(ui, &labels[6], &mut e.thermal[0], &default);
        ui_kit::text_row(ui, &labels[7], &mut e.thermal[1], &default);
    }
    if kind == "header" {
        ui_kit::choice_row(ui, &labels[8], "electronics_rows", &mut e.rows, &["1", "2"]);
    }
    if !c.footprint.is_empty() {
        ui_kit::text(ui, app.tr("electronics.footprint_replaces"), ui_kit::Text::Note("electronics.footprint_replaces"));
    }
    if !button(ui, "generate-footprint", &tr("electronics.generate_footprint"), true) {
        return;
    }
    let Some(params) = footprint_params(app, c.id, &labels) else { return };
    if app.run("ecad.footprint.generate", params).is_ok() {
        app.electronics.editor.picked = None;
    }
}

/// The generator's form as `ecad.footprint.generate` parameters; `None` after saying what isn't a
/// number.
fn footprint_params(app: &mut SolveApp, id: u64, labels: &[String; 9]) -> Option<Value> {
    let e = &app.electronics.editor;
    let kind = PACKAGES.get(e.package).map_or("chip", |p| p.0);
    let mut params = json!({"component": id, "package": kind});
    let mut bad: Option<(String, String)> = None;
    // An empty field is left to the package's default.
    let optional = |bad: &mut Option<(String, String)>, text: &str, label: &str| -> Option<f64> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let v = crate::prefs::parse_number(text);
        if v.is_none() && bad.is_none() {
            *bad = Some((label.to_string(), text.to_string()));
        }
        v
    };
    match kind {
        "chip" => params["size"] = json!(SIZES.get(e.size).copied().unwrap_or("0603")),
        "sot23" => {}
        _ => {
            let pins = e.pins.trim();
            let pins = if pins.is_empty() {
                match kind {
                    "qfn" => "16",
                    "header" => "4",
                    _ => "8",
                }
            } else {
                pins
            };
            match pins.parse::<u32>() {
                Ok(n) => params["pins"] = json!(n),
                Err(_) => bad = Some((labels[2].clone(), pins.to_string())),
            }
            if let Some(p) = optional(&mut bad, &e.pitch, &labels[3]) {
                params["pitch"] = json!(p);
            }
        }
    }
    if kind == "qfn" {
        if let (Some(x), Some(y)) = (optional(&mut bad, &e.body[0], &labels[4]), optional(&mut bad, &e.body[1], &labels[5])) {
            params["body"] = json!([x, y]);
        }
        if let (Some(x), Some(y)) = (optional(&mut bad, &e.thermal[0], &labels[6]), optional(&mut bad, &e.thermal[1], &labels[7])) {
            params["thermal"] = json!([x, y]);
        }
    }
    if kind == "header" {
        params["rows"] = json!(e.rows.min(1) + 1);
    }
    if let Some((field, text)) = bad {
        app.set_status(crate::i18n::format("electronics.not_a_number", &[("field", &field), ("text", &text)]), true);
        return None;
    }
    Some(params)
}

/// One row per pin: the pads it connects to, as typed numbers.
fn connections(app: &mut SolveApp, ui: &mut Ui, c: &ElectronicComponent) {
    ui_kit::text(ui, app.tr("electronics.connections_help"), ui_kit::Text::Note("electronics.connections_help"));
    let rows = c.connections();
    for (pin, pads) in &rows {
        let label = match c.symbol.pin(pin).map(|p| p.name.as_str()) {
            Some(name) if !name.is_empty() => crate::i18n::format("electronics.connection_row", &[("pin", pin), ("name", name)]),
            _ => crate::i18n::format("electronics.connection_row_unnamed", &[("pin", pin)]),
        };
        let current = pads.join(", ");
        if let Some(text) = field(app, ui, &format!("{}:map:{pin}", c.id), &label, &current) {
            let typed: Vec<&str> = text.split([',', ' ', ';']).map(str::trim).filter(|s| !s.is_empty()).collect();
            let mut map = Map::new();
            for (other, pads) in &rows {
                // A pad connects to one pin: one typed here leaves the pin that had it.
                let pads: Vec<&str> = if other == pin { typed.clone() } else { pads.iter().copied().filter(|p| !typed.contains(p)).collect() };
                map.insert(other.to_string(), json!(pads));
            }
            drop(app.run("ecad.component.map_pins", json!({"component": c.id, "map": map})));
        }
    }
    if button(ui, "by-number", &tr("electronics.connect_by_number"), true) {
        drop(app.run("ecad.component.map_pins", json!({"component": c.id, "by_number": true})));
    }
}

/// Which SolidCraft component is the 3D package, and where it sits.
fn package(app: &mut SolveApp, ui: &mut Ui, c: &ElectronicComponent) {
    let models: Vec<(u64, String)> = app.session.doc.components.iter().map(|m| (m.id, m.name.clone())).collect();
    if models.is_empty() {
        ui_kit::text(ui, app.tr("electronics.package_help"), ui_kit::Text::Note("electronics.package_help"));
        return;
    }
    let mut options = vec![tr("electronics.package_none")];
    options.extend(models.iter().map(|(_, name)| name.clone()));
    let at = c.package.as_ref().and_then(|p| models.iter().position(|(id, _)| *id == p.model)).map_or(0, |i| i + 1);
    let label = tr("electronics.field_model");
    let (translate, rotation) = c.package.as_ref().map_or(([0.0; 3], 0.0), |p| (p.translate, p.rotation));
    let set =
        |model: u64, translate: [f64; 3], rotation: f64| json!({"component": c.id, "model": model, "translate": translate, "rotation": rotation});
    match choice(ui, &format!("{}:package", c.id), &label, at, &options) {
        Some(0) => drop(app.run("ecad.package.clear", json!({"component": c.id}))),
        Some(i) => {
            if let Some((model, _)) = models.get(i - 1) {
                drop(app.run("ecad.package.set", set(*model, translate, rotation)));
            }
        }
        None => {}
    }
    let Some(p) = &c.package else { return };
    let key = |f: &str| format!("{}:package:{f}", c.id);
    let mut moved = None;
    for (i, (f, label)) in [("x", "electronics.field_x"), ("y", "electronics.field_y"), ("z", "electronics.field_lift")].into_iter().enumerate() {
        if let Some(v) = number(app, ui, &key(f), &tr(label), p.translate[i]) {
            let mut t = p.translate;
            t[i] = v;
            moved = Some((t, p.rotation));
        }
    }
    if let Some(r) = number(app, ui, &key("rotation"), &tr("electronics.field_rotation"), p.rotation) {
        moved = Some((p.translate, r));
    }
    if let Some((t, r)) = moved {
        drop(app.run("ecad.package.set", set(p.model, t, r)));
    }
}

/// What a problem says, in the panel's words.
fn problem_text(p: &Problem) -> String {
    let item = |i: usize| p.items.get(i).map(String::as_str).unwrap_or_default();
    match p.code {
        "pads_overlap" => crate::i18n::format("electronics.problem_pads_overlap", &[("a", item(0)), ("b", item(1))]),
        "pin_unconnected" => crate::i18n::format("electronics.problem_pin_unconnected", &[("pin", item(0))]),
        "pad_unconnected" => crate::i18n::format("electronics.problem_pad_unconnected", &[("pad", item(0))]),
        "pad_outside_courtyard" => crate::i18n::format("electronics.problem_pad_outside_courtyard", &[("pad", item(0))]),
        "courtyard_missing" => crate::i18n::tr("electronics.problem_courtyard_missing").to_string(),
        "no_pins" => crate::i18n::tr("electronics.problem_no_pins").to_string(),
        "no_pads" => crate::i18n::tr("electronics.problem_no_pads").to_string(),
        _ => crate::i18n::tr_diagnostic(&p.message),
    }
}

fn checks(app: &mut SolveApp, ui: &mut Ui, c: &ElectronicComponent) {
    let problems = c.problems();
    // A 3D package whose model is gone keeps the part from being ready, as `ecad.component.inspect` says.
    let missing = c.package.as_ref().is_some_and(|p| !app.session.doc.components.iter().any(|m| m.id == p.model));
    let ready = c.ready_with(&problems) && !missing;
    let key = if ready { "electronics.ready" } else { "electronics.not_ready" };
    ui_kit::text(ui, app.tr(key), ui_kit::Text::Report(key, !ready));
    if missing {
        ui_kit::text(ui, app.tr("electronics.problem_package_missing"), ui_kit::Text::Error("electronics.problem"));
    }
    for p in problems {
        let text = problem_text(&p);
        let style =
            if p.severity == Severity::Error { ui_kit::Text::Error("electronics.problem") } else { ui_kit::Text::Note("electronics.problem") };
        ui_kit::text(ui, &text, style);
    }
    if let Some(gap) = c.min_pad_gap() {
        let text = crate::i18n::format("electronics.min_pad_gap", &[("gap", &mm(gap.gap)), ("a", &gap.pads[0]), ("b", &gap.pads[1])]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.min_pad_gap"));
    }
    if let Some([[x0, y0], [x1, y1]]) = c.courtyard(Side::Top) {
        let text = crate::i18n::format("electronics.courtyard_size", &[("w", &mm(x1 - x0)), ("h", &mm(y1 - y0))]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.courtyard_size"));
    }
}

#[cfg(test)]
#[path = "electronics_editor_tests.rs"]
mod tests;
