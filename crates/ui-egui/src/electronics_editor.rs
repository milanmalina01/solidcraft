//! An electronics library's editors. The Inspector beside the page edits the open item: the picked
//! pin, pad or graphic, else the item's properties (`electronics_library::properties`); the windows
//! beside it generate symbols and footprints, connect a component's pins to its pads, give it a 3D
//! package and show its checks. On a symbol's or a footprint's page a press picks a pin, a pad or a
//! graphic and a drag moves it on the grid (Ctrl or Cmd: off it). Every change is one `ecad.*`
//! command, so undo takes back one edit and the Inspector does what an agent does.

use std::collections::BTreeMap;

use egui::{Pos2, Rect, Ui};
use serde_json::{Map, Value, json};
use solidcraft_engine::doc::ecad::{GraphicType, Pad, PadKind, PadShape, Pin, PinDirection, PinType, Problem, Severity, Side, SymbolItem};

use crate::SolveApp;
use crate::electronics_components::{Shown, View, shown, shown_copy};
use crate::electronics_library::Opened;
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;
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
/// The symbol generator's styles, as `ecad.symbol.generate` names them: the box, then the standard
/// shapes by family.
const STYLES: [(&str, &str); 22] = [
    ("box", "electronics.style_box"),
    ("resistor", "electronics.style_resistor"),
    ("capacitor", "electronics.style_capacitor"),
    ("polarized_capacitor", "electronics.style_polarized_capacitor"),
    ("inductor", "electronics.style_inductor"),
    ("ferrite", "electronics.style_ferrite"),
    ("fuse", "electronics.style_fuse"),
    ("diode", "electronics.style_diode"),
    ("led", "electronics.style_led"),
    ("zener", "electronics.style_zener"),
    ("schottky", "electronics.style_schottky"),
    ("tvs", "electronics.style_tvs"),
    ("crystal", "electronics.style_crystal"),
    ("resonator", "electronics.style_resonator"),
    ("switch", "electronics.style_switch"),
    ("battery", "electronics.style_battery"),
    ("antenna", "electronics.style_antenna"),
    ("test_point", "electronics.style_test_point"),
    ("npn", "electronics.style_npn"),
    ("pnp", "electronics.style_pnp"),
    ("nmos", "electronics.style_nmos"),
    ("pmos", "electronics.style_pmos"),
];
/// A style's tile in the symbol generator: its picture's side, in points.
const STYLE_TILE: f32 = 56.0;
/// The picked style's picture in the symbol generator: its height, in points.
const STYLE_PREVIEW: f32 = 120.0;
/// The most pins a status names that a symbol change let go of.
const NAMED_PINS: usize = 6;
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

/// A pin or a pad of the shown electronic component, by number, or a graphic of the shown view, by
/// index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Pin(String),
    Pad(String),
    Graphic(usize),
}

impl Item {
    fn is(&self, pin: bool, number: &str) -> bool {
        match self {
            Item::Pin(n) => pin && n == number,
            Item::Pad(n) => !pin && n == number,
            Item::Graphic(_) => false,
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

/// The window a toolbar command opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Open {
    GenerateSymbol,
    GenerateFootprint,
    /// The open component's pins to its chosen variant's pads.
    Connect,
    /// The open component's chosen variant's 3D package.
    Package,
    Checks,
    /// The symbol the open component shows.
    Symbol,
    /// A new 3D package: its model.
    NewPackage,
}

impl Open {
    /// The toolbar command that opens it, shown pressed while it is open.
    fn command(self) -> &'static str {
        match self {
            Open::GenerateSymbol => "ecad.symbol.generate",
            Open::GenerateFootprint => "ecad.footprint.generate",
            Open::Connect => "ecad.component.map_pins",
            Open::Package | Open::NewPackage => "ecad.package.create",
            Open::Checks => "ecad.component.inspect",
            Open::Symbol => "ecad.component.edit",
        }
    }

    /// It works on the open electronic component.
    fn of_component(self) -> bool {
        matches!(self, Open::Connect | Open::Package | Open::Checks | Open::Symbol)
    }
}

pub struct Editor {
    /// The window the last toolbar command opened beside the inspector.
    pub open: Option<Open>,
    /// The open item the pick, the drawing and the typing belong to.
    of: Option<Opened>,
    pub picked: Option<Item>,
    drag: Option<Drag>,
    /// The armed draw tool and its settings.
    pub draw: Option<crate::electronics_drawing::Draw>,
    pub settings: crate::electronics_drawing::Settings,
    /// The pin whose pads are being connected: a click on a pad connects or disconnects it.
    pub connect: Option<String>,
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
    /// The symbol generator's options: how many LEDs in one (index: one to four), their shared
    /// pin (anode, cathode), a crystal's or a switch's pins (2, 4), a TVS conducting both ways.
    leds: usize,
    common: usize,
    four: usize,
    both_ways: bool,
    width: f32,
}

impl Default for Editor {
    fn default() -> Self {
        Editor {
            open: None,
            of: None,
            picked: None,
            drag: None,
            draw: None,
            settings: Default::default(),
            connect: None,
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
            leds: 0,
            common: 0,
            four: 0,
            both_ways: false,
            width: f32::NAN,
        }
    }
}

/// Forget a pick that no longer names an item of the open symbol or footprint, a connecting pin
/// that is gone, and the typing and drawing for another open item.
fn sync(app: &mut SolveApp) {
    let of = app.electronics.opened;
    let drawing = matches!(of, Some(Opened::Symbol(_) | Opened::Footprint(_)));
    let keep = drawing
        && match (&app.electronics.editor.picked, shown(app)) {
            (Some(Item::Pin(n)), Some(c)) => app.electronics.view == View::Symbol && c.symbol.pin(n).is_some(),
            (Some(Item::Pad(n)), Some(c)) => app.electronics.view == View::Footprint && c.footprint.pad(n).is_some(),
            (Some(Item::Graphic(i)), Some(_)) => *i < crate::electronics_drawing::graphics(app).len(),
            _ => false,
        };
    let connecting = match (&app.electronics.editor.connect, shown(app)) {
        (Some(pin), Some(c)) => matches!(of, Some(Opened::Component(_))) && c.symbol.pin(pin).is_some(),
        _ => false,
    };
    let e = &mut app.electronics.editor;
    let other = e.of != of;
    if other {
        // Another item keeps no typing, no connecting pin, no half-drawn shape, and no tool where
        // there is nothing to draw on.
        e.typed.clear();
        e.connect = None;
        e.of = of;
        if let Some(draw) = &mut e.draw {
            draw.points.clear();
        }
        if !drawing {
            e.draw = None;
        }
        // A window for a component goes with it.
        if e.open.is_some_and(Open::of_component) {
            e.open = None;
        }
    }
    if !keep || other {
        e.picked = None;
        e.drag = None;
    }
    if !connecting {
        e.connect = None;
    }
}

/// How far graphic `index` of the shown view has been dragged, in page millimetres.
pub(crate) fn graphic_moved_by(app: &SolveApp, index: usize) -> [f64; 2] {
    match &app.electronics.editor.drag {
        Some(d) if d.moving && d.item == Item::Graphic(index) => d.by,
        _ => [0.0; 2],
    }
}

/// Whether graphic `index` of the shown view is picked.
pub(crate) fn is_picked_graphic(app: &SolveApp, index: usize) -> bool {
    app.electronics.editor.picked == Some(Item::Graphic(index))
}

/// Whether the pin (`pin`) or pad numbered `number` is the pin being connected, or one of its pads.
pub(crate) fn is_connecting(app: &SolveApp, pin: bool, number: &str) -> bool {
    let Some(active) = &app.electronics.editor.connect else { return false };
    if pin {
        return active == number;
    }
    shown(app).is_some_and(|c| c.connections().iter().any(|(p, pads)| p == active && pads.contains(&number)))
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

/// The pin, pad or graphic under `p` on the page.
fn hit(app: &SolveApp, map: PageMap, p: Pos2) -> Option<Item> {
    let c = shown(app)?;
    match app.electronics.view {
        View::Symbol => pin_at(c.symbol, map, p).map(Item::Pin),
        View::Footprint => pad_at(c.footprint, map, p).map(Item::Pad),
    }
    .or_else(|| crate::electronics_drawing::hit(app, map, p).map(Item::Graphic))
}

/// The number of `symbol`'s pin nearest `p`, within reach, drawn with `map`.
pub(crate) fn pin_at(symbol: &solidcraft_engine::doc::ecad::SymbolItem, map: PageMap, p: Pos2) -> Option<String> {
    symbol
        .pins
        .iter()
        .map(|pin| {
            let (a, b) = (map.to_screen(pin.at[0], pin.at[1]), map.to_screen(pin.body_end()[0], pin.body_end()[1]));
            (segment_distance(p, a, b), pin)
        })
        .filter(|(d, _)| *d <= PICK_POINTS)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, pin)| pin.number.clone())
}

/// The number of `footprint`'s pad under `p` (the last drawn is on top), drawn with `map`.
pub(crate) fn pad_at(footprint: &solidcraft_engine::doc::ecad::FootprintItem, map: PageMap, p: Pos2) -> Option<String> {
    footprint
        .pads
        .iter()
        .rev()
        .find(|pad| {
            let outline: Vec<Pos2> = pad.outline(6).iter().map(|q| map.to_screen(q[0], q[1])).collect();
            inside(&outline, p) || edge_distance(&outline, p) <= PICK_POINTS / 2.0
        })
        .map(|pad| pad.number.clone())
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
    // An armed draw tool takes the clicks; the page still pans and zooms.
    if app.electronics.editor.draw.is_some() {
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        let enter = !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if let Some(p) = pos {
            let at = crate::electronics_drawing::snap(app, map, p, free);
            if response.double_clicked() {
                crate::electronics_drawing::click(app, at, true);
            } else if response.clicked() {
                crate::electronics_drawing::click(app, at, false);
            }
        }
        if response.secondary_clicked() {
            let at = pos.map(|p| crate::electronics_drawing::snap(app, map, p, free));
            if !crate::electronics_drawing::rotate(app, at) {
                crate::electronics_drawing::finish(app);
            }
        } else if enter {
            crate::electronics_drawing::finish(app);
        }
        return false;
    }
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
            Some(pin) => ("ecad.pin.edit", json!({"symbol": c.symbol.id, "pin": n, "at": to(pin.at)})),
            None => return,
        },
        Item::Pad(n) => match c.footprint.pad(n) {
            Some(pad) => ("ecad.pad.edit", json!({"footprint": c.footprint.id, "pad": n, "at": to(pad.at)})),
            None => return,
        },
        Item::Graphic(i) => {
            let i = *i;
            crate::electronics_drawing::change(app, i, |g| Some(crate::electronics_drawing::translate(g, by)));
            return;
        }
    };
    // A pin moved under the schematic's wires may change what they join: say so.
    if let Ok(reply) = app.run(id, params) {
        detached_status(app, &reply);
    }
}

/// R in the workspace: a quarter turn of what the armed tool places, or of the picked pin or pad.
pub(crate) fn rotate(app: &mut SolveApp, ctx: &egui::Context) -> bool {
    if crate::electronics_workspace::schematic(app) {
        return crate::electronics_schematic::rotate(app);
    }
    if crate::electronics_workspace::board(app) {
        return crate::electronics_board::rotate(app);
    }
    sync(app);
    let at = ctx.input(|i| i.pointer.hover_pos()).and_then(|p| {
        let rect = app.viewport.rect?;
        let map = PageMap::new(&app.electronics.page.cam, rect)?;
        Some(crate::electronics_drawing::snap(app, map, p, false))
    });
    if crate::electronics_drawing::rotate(app, at) {
        return true;
    }
    let Some(c) = shown_copy(app) else { return false };
    match app.electronics.editor.picked.clone() {
        Some(Item::Pin(n)) => {
            let Some(pin) = c.symbol.pin(&n) else { return false };
            // Right, up, left, down: counter-clockwise.
            let turns = ["right", "up", "left", "down"];
            let now = DIRECTIONS.iter().find(|(d, _)| *d == pin.direction).and_then(|(_, name)| turns.iter().position(|t| t == name)).unwrap_or(0);
            if let Ok(reply) = app.run("ecad.pin.edit", json!({"symbol": c.symbol.id, "pin": n, "direction": turns[(now + 1) % 4]})) {
                detached_status(app, &reply);
            }
            true
        }
        Some(Item::Pad(n)) => {
            let Some(pad) = c.footprint.pad(&n) else { return false };
            drop(app.run("ecad.pad.edit", json!({"footprint": c.footprint.id, "pad": n, "rotation": (pad.rotation + 90.0).rem_euclid(360.0)})));
            true
        }
        _ => false,
    }
}

/// The Delete key in the workspace: delete the picked pin or pad. `true` when there was one.
pub(crate) fn delete_picked(app: &mut SolveApp) -> bool {
    if crate::electronics_workspace::schematic(app) {
        return crate::electronics_schematic::delete(app);
    }
    if crate::electronics_workspace::board(app) {
        return crate::electronics_board::delete(app);
    }
    sync(app);
    let (Some(item), Some((symbol, footprint))) = (app.electronics.editor.picked.take(), shown(app).map(|c| (c.symbol.id, c.footprint.id))) else {
        return false;
    };
    app.electronics.editor.drag = None;
    let (command, on, key, number) = match item {
        Item::Pin(n) => ("ecad.pin.delete", json!({"symbol": symbol}), "pins", n),
        Item::Pad(n) => ("ecad.pad.delete", json!({"footprint": footprint}), "pads", n),
        Item::Graphic(i) => {
            crate::electronics_drawing::change(app, i, |_| None);
            return true;
        }
    };
    let mut params = on;
    params[key] = json!([number]);
    drop(app.run(command, params));
    true
}

/// Escape in the workspace, one thing at a time: the shape being drawn, then the draw tool, a drag
/// (which ends where it began), the pin being connected, and last the pick. `true` when there was
/// one.
pub(crate) fn escape(app: &mut SolveApp) -> bool {
    if crate::electronics_workspace::schematic(app) {
        return crate::electronics_schematic::escape(app);
    }
    if crate::electronics_workspace::board(app) {
        return crate::electronics_board::escape(app);
    }
    let e = &mut app.electronics.editor;
    if let Some(draw) = &mut e.draw {
        if draw.points.is_empty() {
            e.draw = None;
        } else {
            draw.points.clear();
        }
        return true;
    }
    e.drag.take().is_some() || e.connect.take().is_some() || e.picked.take().is_some()
}

/// The Inspector, on the right of the page: the schematic's, the board's, or the library's.
pub(crate) fn panel(app: &mut SolveApp, ui: &mut Ui) {
    sync(app);
    let bounds = crate::shell::panel_bounds(app, ui.available_rect_before_wrap());
    // The Browser card is on the left; the panel stays on the right.
    let mut side = String::from("right");
    let mut width = app.electronics.editor.width;
    let schematic = crate::electronics_workspace::schematic(app);
    let board = crate::electronics_workspace::board(app);
    let title = app.tr("electronics.inspector_title").to_string();
    let content = ui_kit::Panel::workspace(
        ui,
        ui_kit::WorkspaceOptions { id: "electronics:editor", title: &title, icon: "component", bounds, fill_height: true },
        &mut side,
        &mut width,
        |ui| {
            // Each board tool (and the Layers tab) scrolls on its own, so a tool's options start
            // at their top, not where the last one was scrolled to.
            let tool = if board { app.electronics.board.tool.as_ref().map(|t| t.command()) } else { None };
            let layers = board && app.electronics.board.layers_tab;
            egui::ScrollArea::vertical().id_salt(("electronics_editor", schematic, board, tool, layers)).auto_shrink([false, false]).show(ui, |ui| {
                if schematic {
                    crate::electronics_schematic::inspector(app, ui)
                } else if board {
                    crate::electronics_board::inspector(app, ui)
                } else {
                    body(app, ui)
                }
            });
        },
    );
    app.electronics.editor.width = width;
    let rect = content.map(|c| c.response.rect);
    crate::shell::register_panel(app, "electronics:editor", rect, Some(ui_kit::Edge::Right));
    if let Some(rect) = rect {
        crate::scenario::publish_rect("electronics:editor", rect);
    }
    if !(schematic || board) {
        windows(app, ui.ctx());
    }
}

fn body(app: &mut SolveApp, ui: &mut Ui) {
    // The armed tool's options, then what is picked; with nothing picked, the open item itself.
    if let Some(command) = crate::electronics_drawing::armed_command(app) {
        let title = solidcraft_engine::find_command(command).map_or(command, |c| c.label);
        let title = app.tr_label(title).to_string();
        ui_kit::section(ui, ("electronics_tool", command), &title, true, |ui| crate::electronics_drawing::options(app, ui));
    }
    let c = shown_copy(app);
    match (app.electronics.editor.picked.clone(), &c) {
        (Some(Item::Pin(n)), Some(c)) => {
            if let Some(pin) = c.symbol.pin(&n) {
                pin_section(app, ui, c, pin);
            }
        }
        (Some(Item::Pad(n)), Some(c)) => {
            if let Some(pad) = c.footprint.pad(&n) {
                pad_section(app, ui, c, pad);
            }
        }
        (Some(Item::Graphic(i)), Some(_)) => crate::electronics_drawing::section(app, ui, i),
        _ => crate::electronics_library::properties(app, ui),
    }
}

/// The window a toolbar command opened beside the inspector: Generate, Connect, 3D Package, the
/// checks, the component's symbol or a new 3D package's model. Its close button, or Escape,
/// closes it.
fn windows(app: &mut SolveApp, ctx: &egui::Context) {
    let Some(open) = app.electronics.editor.open else { return };
    let c = shown_copy(app);
    let component = c.as_ref().filter(|c| c.component.id != 0);
    if open.of_component() && component.is_none() {
        app.electronics.editor.open = None;
        return;
    }
    let command = open.command();
    let spec = solidcraft_engine::find_command(command);
    let title = match open {
        Open::Symbol => tr("electronics.choose_symbol"),
        Open::NewPackage => tr("electronics.new_package"),
        _ => app.tr_label(spec.map_or(command, |s| s.label)).to_string(),
    };
    let icon = if open == Open::Symbol { "library" } else { spec.map_or("component", |s| s.icon) };
    // Beside the inspector, below the toolbar; it can be moved from there.
    let at = app.shell.workspace_panels.get("electronics:editor").map_or(ctx.content_rect().right_top(), |(r, _)| r.left_top());
    let at = at + egui::vec2(-crate::theme::Tokens::get().space_8, 0.0);
    let closed =
        ui_kit::Window::new(egui::Id::new(("electronics_window", command)), icon, &title).placed(egui::Align2::RIGHT_TOP, at).show_body(ctx, |ui| {
            match (open, component) {
                (Open::GenerateSymbol, _) => symbol_generator(app, ui, c.as_ref()),
                (Open::GenerateFootprint, _) => footprint_generator(app, ui, c.as_ref()),
                (Open::NewPackage, _) => crate::electronics_library::new_package(app, ui),
                (Open::Connect, Some(c)) => {
                    if c.symbol.pins.is_empty() || c.footprint.pads.is_empty() {
                        ui_kit::text(ui, app.tr("electronics.connect_needs_both"), ui_kit::Text::Note("electronics.connect_needs_both"));
                    } else {
                        connections(app, ui, c);
                    }
                }
                (Open::Package, Some(c)) => package(app, ui, c),
                (Open::Checks, Some(c)) => checks(app, ui, c),
                (Open::Symbol, Some(c)) => crate::electronics_library::choose_symbol(app, ui, c),
                _ => {}
            }
        });
    if closed {
        app.electronics.editor.open = None;
        if open == Open::Connect {
            app.electronics.editor.connect = None;
        }
    }
}

/// A button, published for scenarios as `electronics:editor:<key>`.
pub(crate) fn button(ui: &mut Ui, key: &str, label: &str, enabled: bool) -> bool {
    let response = ui.add_enabled_ui(enabled, |ui| ui_kit::button(ui, label, ui_kit::Button::Normal)).inner;
    crate::scenario::publish_target(&format!("electronics:editor:{key}"), response.rect);
    enabled && response.clicked()
}

/// A toolbar command in an electronics document: the schematic's tools, the library's new items
/// and windows (`electronics_library::start`), and the draw tools, which arm on the open symbol's
/// or footprint's page. `true` when the workspace took it.
pub(crate) fn start_interactive(app: &mut SolveApp, id: &str) -> bool {
    if !crate::electronics_workspace::active(app) || !id.starts_with("ecad.") {
        return false;
    }
    if crate::electronics_workspace::schematic(app) {
        return crate::electronics_schematic::start(app, id);
    }
    if crate::electronics_workspace::board(app) {
        return crate::electronics_board::start(app, id);
    }
    sync(app);
    if let Some(tool) = crate::electronics_drawing::tool_of(id) {
        // Pins belong to a symbol, pads to a footprint, a shape to either.
        let on = match (tool.shape, app.electronics.opened) {
            (crate::electronics_drawing::Shape::Pin, Some(Opened::Symbol(_))) => true,
            (crate::electronics_drawing::Shape::Pad, Some(Opened::Footprint(_))) => true,
            (crate::electronics_drawing::Shape::Pin | crate::electronics_drawing::Shape::Pad, _) => false,
            (_, opened) => matches!(opened, Some(Opened::Symbol(_) | Opened::Footprint(_))),
        };
        if !on {
            let key = match tool.shape {
                crate::electronics_drawing::Shape::Pad => "electronics.open_footprint_first",
                crate::electronics_drawing::Shape::Pin => "electronics.open_symbol_first",
                _ => "electronics.open_drawing_first",
            };
            app.set_status(tr(key), true);
            return true;
        }
        let e = &mut app.electronics.editor;
        // A second press puts the tool down.
        let again = e.draw.as_ref().is_some_and(|d| d.shape == tool.shape && d.pad == tool.pad);
        e.draw = (!again).then(|| crate::electronics_drawing::Draw { shape: tool.shape, pad: tool.pad, points: Vec::new(), turn: None });
        e.picked = None;
        e.connect = None;
        return true;
    }
    match id {
        // Delete takes the pick, as the Delete key does.
        "ecad.pin.delete" | "ecad.pad.delete" => {
            if !delete_picked(app) {
                app.set_status(tr("electronics.nothing_picked"), false);
            }
        }
        "ecad.footprint.courtyard" => match app.electronics.opened {
            Some(Opened::Footprint(footprint)) => drop(app.run("ecad.footprint.courtyard", json!({"footprint": footprint}))),
            _ => app.set_status(tr("electronics.open_footprint_first"), true),
        },
        "ecad.symbol.generate" => toggle(app, Open::GenerateSymbol),
        "ecad.footprint.generate" => toggle(app, Open::GenerateFootprint),
        "ecad.component.map_pins" => {
            // Connecting starts with the first pin: a click on a pin picks another, a click on a
            // pad connects it.
            let first = shown(app).and_then(|c| c.symbol.pins.first()).map(|p| p.number.clone());
            let e = &mut app.electronics.editor;
            e.draw = None;
            let closing = e.open == Some(Open::Connect);
            e.connect = if closing { None } else { e.connect.clone().or(first) };
            e.open = (!closing).then_some(Open::Connect);
        }
        "ecad.component.inspect" => toggle(app, Open::Checks),
        "ecad.component.edit" => toggle(app, Open::Symbol),
        // New 3D Package on LIBRARY; the open component's 3D package on COMPONENT.
        "ecad.package.create" if app.ui.tab == crate::electronics_workspace::COMPONENT => toggle(app, Open::Package),
        "ecad.package.create" => toggle(app, Open::NewPackage),
        _ => return crate::electronics_library::start(app, id),
    }
    // A window for a component needs one open.
    if app.electronics.editor.open.is_some_and(Open::of_component) && !matches!(app.electronics.opened, Some(Opened::Component(_))) {
        app.electronics.editor.open = None;
        app.electronics.editor.connect = None;
        app.set_status(tr("electronics.open_component_first"), true);
    }
    true
}

/// Open `window` beside the inspector.
pub(crate) fn show_window(app: &mut SolveApp, window: Open) {
    app.electronics.editor.open = Some(window);
}

/// Open `window`, or close it when it is already open.
fn toggle(app: &mut SolveApp, window: Open) {
    let e = &mut app.electronics.editor;
    e.open = (e.open != Some(window)).then_some(window);
}

/// The toolbar command to show pressed: the armed tool, Connect while a pin is being connected,
/// or the command whose window is open.
pub(crate) fn running(app: &SolveApp) -> Option<&'static str> {
    if crate::electronics_workspace::schematic(app) {
        return crate::electronics_schematic::running(app);
    }
    if crate::electronics_workspace::board(app) {
        return crate::electronics_board::running(app);
    }
    let e = &app.electronics.editor;
    crate::electronics_drawing::armed_command(app)
        .or_else(|| e.connect.is_some().then_some("ecad.component.map_pins"))
        .or_else(|| e.open.map(Open::command))
}

/// `base`, or `base 2`, `base 3`… the first that is not in `taken`.
pub(crate) fn free_from(base: &str, taken: &[String]) -> String {
    let free = |n: &String| !taken.contains(n);
    (1..=10_000).map(|n| if n == 1 { base.to_string() } else { format!("{base} {n}") }).find(free).unwrap_or_else(|| base.to_string())
}

/// A name no library symbol (`symbol`) or footprint has, after `base`.
pub(crate) fn free_item_name(app: &SolveApp, base: &str, symbol: bool) -> String {
    let ecad = &app.session.doc.ecad;
    let taken: Vec<String> =
        if symbol { ecad.symbols.iter().map(|x| x.name.clone()).collect() } else { ecad.footprints.iter().map(|x| x.name.clone()).collect() };
    free_from(base, &taken)
}

/// The open symbol, which the pin and draw tools put their pins and shapes on.
pub(crate) fn editable_symbol(app: &SolveApp) -> Option<u64> {
    match app.electronics.opened {
        Some(Opened::Symbol(id)) => Some(id),
        _ => None,
    }
}

/// The open footprint, which the pad and draw tools put their pads and shapes on.
pub(crate) fn editable_footprint(app: &SolveApp) -> Option<u64> {
    match app.electronics.opened {
        Some(Opened::Footprint(id)) => Some(id),
        _ => None,
    }
}

/// A text field that runs its edit when it gives up the keyboard (Enter, Tab or a click
/// elsewhere); Escape puts the old text back. `Some` with the new text when it changed.
pub(crate) fn field(app: &mut SolveApp, ui: &mut Ui, key: &str, label: &str, current: &str) -> Option<String> {
    let mut text = typed(app, key, current);
    let response = ui_kit::row(ui, label, |ui| ui_kit::cell_editor(ui, egui::Id::new(("electronics_field", key)), &mut text, "", false));
    settle(app, ui, key, &response, text, current)
}

/// What the field `key` shows: the text typed in it while it has the keyboard, else `current`.
pub(crate) fn typed(app: &SolveApp, key: &str, current: &str) -> String {
    app.electronics.editor.typed.get(key).cloned().unwrap_or_else(|| current.to_string())
}

/// After a field is drawn: keep its text while it has the keyboard, and `Some` with the text when
/// it has just given the keyboard up changed (not with Escape).
pub(crate) fn settle(app: &mut SolveApp, ui: &Ui, key: &str, response: &egui::Response, text: String, current: &str) -> Option<String> {
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
pub(crate) fn pair(app: &mut SolveApp, ui: &mut Ui, key: &str, label: &str, current: [f64; 2]) -> Option<[f64; 2]> {
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
pub(crate) fn number(app: &mut SolveApp, ui: &mut Ui, key: &str, label: &str, current: f64) -> Option<f64> {
    let text = field(app, ui, key, label, &mm(current))?;
    let value = crate::prefs::parse_number(&text);
    if value.is_none() {
        app.set_status(crate::i18n::format("electronics.not_a_number", &[("field", label), ("text", &text)]), true);
    }
    value
}

/// A dropdown row; `Some` with the new choice when it changed.
pub(crate) fn choice(ui: &mut Ui, id: &str, label: &str, selected: usize, options: &[String]) -> Option<usize> {
    let mut index = selected;
    let options: Vec<&str> = options.iter().map(String::as_str).collect();
    ui_kit::choice_row(ui, label, id, &mut index, &options);
    (index != selected).then_some(index)
}

/// Millimetres for a field: up to four decimals, no trailing zeros.
pub(crate) fn mm(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

pub(crate) fn tr(key: &str) -> String {
    crate::i18n::tr(key).to_string()
}

fn translated<T>(prefix: &str, options: &[(T, &str)]) -> Vec<String> {
    options.iter().map(|(_, key)| tr(&format!("{prefix}{key}"))).collect()
}

/// The open component's fields.
pub(crate) fn component_properties(app: &mut SolveApp, ui: &mut Ui, c: &Shown) {
    let rows = [
        ("name", "electronics.field_name", &c.component.name),
        ("prefix", "electronics.field_prefix", &c.component.prefix),
        ("value", "electronics.field_value", &c.component.value),
        ("mpn", "electronics.field_mpn", &c.component.mpn),
        ("manufacturer", "electronics.field_manufacturer", &c.component.manufacturer),
        ("description", "electronics.field_description", &c.component.description),
    ];
    for (key, label, current) in rows {
        let label = tr(label);
        if let Some(text) = field(app, ui, &format!("{}:{key}", c.component.id), &label, current) {
            let mut params = json!({"component": c.component.id});
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
    let Ok(reply) = app.run(command, Value::Object(params)) else { return };
    detached_status(app, &reply);
    if let Some(item) = renumbered {
        app.electronics.editor.picked = Some(item);
    }
}

fn pin_section(app: &mut SolveApp, ui: &mut Ui, c: &Shown, pin: &Pin) {
    let title = crate::i18n::format("electronics.section_pin", &[("number", &pin.number)]);
    let types = translated("electronics.pin_", &PIN_TYPES);
    let directions = translated("electronics.direction_", &DIRECTIONS);
    let delete = tr("electronics.delete_pin");
    ui_kit::section(ui, "electronics_pin", &title, true, |ui| {
        let key = |f: &str| format!("{}:pin:{}:{f}", c.symbol.id, pin.number);
        let mut edit = Map::new();
        edit.insert("symbol".into(), json!(c.symbol.id));
        edit.insert("pin".into(), json!(pin.number));
        let mut renumbered = None;
        if let Some(n) = field(app, ui, &key("number"), &tr("electronics.field_number"), &pin.number) {
            renumbered = Some(Item::Pin(n.clone()));
            edit.insert("number".into(), json!(n));
        }
        // The name's tooltip tells of the overbar markup (`~{CE}`).
        let mut name = typed(app, &key("name"), &pin.name);
        let response = ui_kit::row(ui, &tr("electronics.field_pin_name"), |ui| {
            ui_kit::cell_editor(ui, egui::Id::new(("electronics_field", key("name"))), &mut name, "", false)
        })
        .on_hover_text(tr("electronics.pin_name_help"));
        if let Some(name) = settle(app, ui, &key("name"), &response, name, &pin.name) {
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
            drop(app.run("ecad.pin.delete", json!({"symbol": c.symbol.id, "pins": [pin.number]})));
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

fn pad_section(app: &mut SolveApp, ui: &mut Ui, c: &Shown, pad: &Pad) {
    let title = crate::i18n::format("electronics.section_pad", &[("number", &pad.number)]);
    let shapes = translated("electronics.shape_", &SHAPES);
    let kinds = translated("electronics.kind_", &KINDS);
    let sides = translated("electronics.side_", &SIDES);
    let delete = tr("electronics.delete_pad");
    ui_kit::section(ui, "electronics_pad", &title, true, |ui| {
        let key = |f: &str| format!("{}:pad:{}:{f}", c.footprint.id, pad.number);
        let mut edit = Map::new();
        edit.insert("footprint".into(), json!(c.footprint.id));
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
            drop(app.run("ecad.pad.delete", json!({"footprint": c.footprint.id, "pads": [pad.number]})));
        }
    });
}

/// The next free number after the highest numbered one.
fn next_number<'a>(taken: impl Iterator<Item = &'a str> + Clone) -> String {
    let start = taken.clone().filter_map(|n| n.parse::<u64>().ok()).max().map_or(1, |n| n.saturating_add(1));
    (start..start.saturating_add(10_000)).map(|n| n.to_string()).find(|n| !taken.clone().any(|t| t == n)).unwrap_or_default()
}

/// Add a pin under the last one, the same way round, and pick it.
/// The next free pin or pad number after the highest one.
pub(crate) fn next_free<'a>(taken: impl Iterator<Item = &'a str> + Clone) -> String {
    next_number(taken)
}

/// Add a pin, at a click with its direction, or under the last one, and pick it.
pub(crate) fn add_pin(app: &mut SolveApp, c: &Shown, at: Option<([f64; 2], &str)>) {
    let number = next_number(c.symbol.pins.iter().map(|p| p.number.as_str()));
    let (at, direction, length) = match (at, c.symbol.pins.last()) {
        (Some((at, direction)), last) => (at, direction, last.map_or(SYMBOL_GRID, |p| p.length)),
        (None, Some(last)) => {
            let dir = DIRECTIONS.iter().find(|(d, _)| *d == last.direction).map_or("right", |(_, s)| *s);
            ([tidy(last.at[0]), tidy(last.at[1] - SYMBOL_GRID)], dir, last.length)
        }
        (None, None) => ([-3.0 * SYMBOL_GRID, 0.0], "right", SYMBOL_GRID),
    };
    let pin = json!({"number": number, "at": at, "direction": direction, "length": length});
    let Some(symbol) = editable_symbol(app) else { return };
    if app.run("ecad.pin.add", json!({"symbol": symbol, "pins": [pin]})).is_ok() {
        app.electronics.editor.picked = Some(Item::Pin(number));
    }
}

/// The pad the pad tool of `kind` puts at `at`, turned `turn` degrees more: like the footprint's
/// last pad of that kind, else a usual one (a 1 mm square surface-mount pad, a 1.7 mm round
/// through-hole pad with a 1 mm drill, a 3.2 mm hole for an M3 screw). Numbered after the highest.
pub(crate) fn new_pad(footprint: &solidcraft_engine::doc::ecad::FootprintItem, kind: PadKind, at: [f64; 2], turn: f64) -> Pad {
    let number = next_number(footprint.pads.iter().map(|p| p.number.as_str()));
    let rotate = |r: f64| (r + turn).rem_euclid(360.0);
    match footprint.pads.iter().rev().find(|p| p.kind == kind) {
        Some(last) => Pad { number, at, rotation: rotate(last.rotation), ..last.clone() },
        None => {
            let (size, shape, drill) = match kind {
                PadKind::Smd => ([1.0, 1.0], PadShape::Rect, None),
                PadKind::ThroughHole => ([1.7, 1.7], PadShape::Round, Some([1.0, 1.0])),
                PadKind::Hole => ([3.2, 3.2], PadShape::Round, Some([3.2, 3.2])),
            };
            Pad { number, at, size, rotation: rotate(0.0), side: Side::Top, shape, corner: 0.0, kind, drill }
        }
    }
}

/// A pad in the shape `ecad.pad.add` takes.
fn pad_spec(pad: &Pad) -> Value {
    let mut spec = json!({
        "number": pad.number,
        "at": [tidy(pad.at[0]), tidy(pad.at[1])],
        "size": pad.size,
        "rotation": pad.rotation,
        "shape": SHAPES.iter().find(|(s, _)| *s == pad.shape).map_or("rect", |(_, s)| *s),
        "kind": KINDS.iter().find(|(k, _)| *k == pad.kind).map_or("smd", |(_, s)| *s),
    });
    if pad.shape == PadShape::RoundedRect {
        spec["corner"] = json!(pad.corner);
    }
    if let Some([w, h]) = pad.drill {
        spec["drill"] = if w == h { json!(w) } else { json!([w, h]) };
    }
    if pad.kind == PadKind::Smd {
        spec["side"] = json!(SIDES.iter().find(|(s, _)| *s == pad.side).map_or("top", |(_, s)| *s));
    }
    spec
}

/// Add a pad of `kind` at `at`, turned `turn` degrees, to the open footprint, and pick it.
pub(crate) fn add_pad(app: &mut SolveApp, c: &Shown, kind: PadKind, at: [f64; 2], turn: f64) {
    let pad = new_pad(&c.footprint, kind, at, turn);
    let Some(footprint) = editable_footprint(app) else { return };
    if app.run("ecad.pad.add", json!({"footprint": footprint, "pads": [pad_spec(&pad)]})).is_ok() {
        app.electronics.editor.picked = Some(Item::Pad(pad.number));
    }
}

/// What a generator makes or replaces, by what is open: the open symbol or footprint; the open
/// component's symbol or its chosen variant's footprint (made, named after it, when it has none);
/// else a new one, named like New Symbol's or New Footprint's. The parameters naming it (and the
/// component and variant it goes to), and the drawing it replaces.
fn generate_target(app: &SolveApp, c: Option<&Shown>, symbol: bool) -> (Map<String, Value>, bool) {
    let key = if symbol { "symbol" } else { "footprint" };
    let mut params = Map::new();
    let mut replaces = false;
    match (app.electronics.opened, c) {
        (Some(Opened::Symbol(id)), Some(c)) if symbol => {
            params.insert(key.into(), json!(id));
            replaces = !c.symbol.is_empty();
        }
        (Some(Opened::Footprint(id)), Some(c)) if !symbol => {
            params.insert(key.into(), json!(id));
            replaces = !c.footprint.is_empty();
        }
        (Some(Opened::Component(id)), Some(c)) => {
            let has = if symbol { c.component.symbol.is_some() } else { c.variant.as_ref().is_some_and(|v| v.footprint.is_some()) };
            let target = match (has, symbol) {
                (true, true) => json!(c.symbol.id),
                (true, false) => json!(c.footprint.id),
                (false, _) => json!(free_item_name(app, &c.component.name, symbol)),
            };
            replaces = has && if symbol { !c.symbol.is_empty() } else { !c.footprint.is_empty() };
            params.insert(key.into(), target);
            params.insert("component".into(), json!(id));
            if let (false, Some(v)) = (symbol, &c.variant) {
                params.insert("variant".into(), json!(v.name));
            }
        }
        _ => {
            let name = crate::electronics_library::new_item_name(app, if symbol { Opened::Symbol(0) } else { Opened::Footprint(0) });
            params.insert(key.into(), json!(name));
        }
    }
    (params, replaces)
}

/// The symbol generator: a style picked from small pictures of them all, its options, a picture
/// of what it makes, and for a box its pins (one for each of the open footprint's or the
/// component's pads, when it has some). A shape that replaces a symbol of as many pins keeps
/// their numbers, names and types (see [`carried_pins`]), so what hangs on them stays.
fn symbol_generator(app: &mut SolveApp, ui: &mut Ui, c: Option<&Shown>) {
    let (mut params, replaces) = generate_target(app, c, true);
    let old: &[Pin] = match c {
        Some(c) if replaces => &c.symbol.pins,
        _ => &[],
    };
    let label = tr("electronics.field_style");
    let picked = app.electronics.editor.style.min(STYLES.len() - 1);
    let name = translated("", &STYLES).get(picked).cloned().unwrap_or_default();
    ui_kit::row(ui, &label, |ui| ui_kit::text(ui, &name, ui_kit::Text::Inline));
    let options = style_options(app, STYLES.get(picked).map_or("box", |s| s.0));
    if let Some(i) = style_tiles(ui, picked, &options) {
        // A shape of variants starts as many pins as the symbol it replaces has.
        let e = &mut app.electronics.editor;
        e.style = i;
        if !old.is_empty() {
            (e.leds, e.four) = (old.len().saturating_sub(2).min(3), usize::from(old.len() == 4));
        }
    }
    let style = STYLES.get(app.electronics.editor.style).map_or("box", |s| s.0);
    style_option_rows(app, ui, style);
    let mut shape = style_options(app, style);
    shape.insert("style".into(), json!(style));
    let copper: Vec<&str> =
        c.map(|c| c.footprint.pads.iter().filter(|p| p.kind != PadKind::Hole).map(|p| p.number.as_str()).collect()).unwrap_or_default();
    if style == "box" {
        let pins: Vec<Value> = if copper.is_empty() {
            let label = tr("electronics.field_pins");
            ui_kit::text_row(ui, &label, &mut app.electronics.editor.box_pins, "4");
            let n = app.electronics.editor.box_pins.trim().parse::<u32>().ok().filter(|n| (1..=MAX_PINS).contains(n));
            (1..=n.unwrap_or(0)).map(|n| json!({"number": n.to_string()})).collect()
        } else {
            let note = crate::i18n::format("electronics.box_from_pads", &[("n", &copper.len().to_string())]);
            ui_kit::text(ui, &note, ui_kit::Text::Note("electronics.box_from_pads"));
            copper.iter().map(|n| json!({"number": n})).collect()
        };
        // The picture shows the pins it makes; the button works them out again, saying what is wrong.
        if !pins.is_empty() {
            shape.insert("pins".into(), json!(pins));
        }
    } else if let Some(usual) = solidcraft_engine::cmd::symbol_style_preview(&Value::Object(shape.clone())) {
        match carried_pins(old, &usual.pins) {
            Some(pins) => {
                shape.insert("pins".into(), json!(pins));
                ui_kit::text(ui, app.tr("electronics.symbol_keeps_pins"), ui_kit::Text::Note("electronics.symbol_keeps_pins"));
            }
            None if !old.is_empty() => {
                let note = crate::i18n::format("electronics.symbol_other_pins", &[("n", &usual.pins.len().to_string())]);
                ui_kit::text(ui, &note, ui_kit::Text::Note("electronics.symbol_other_pins"));
            }
            None => {}
        }
    }
    style_preview(app, ui, &shape);
    if replaces {
        ui_kit::text(ui, app.tr("electronics.symbol_replaces"), ui_kit::Text::Note("electronics.symbol_replaces"));
    }
    if !button(ui, "generate-symbol", &tr("electronics.generate_symbol"), true) {
        return;
    }
    params.extend(shape);
    let mut params = Value::Object(params);
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
    if let Ok(reply) = app.run("ecad.symbol.generate", params) {
        app.electronics.editor.picked = None;
        detached_status(app, &reply);
    }
}

/// The generator's options for `style` (`count`, `common`, `bidirectional`), as its parameters.
fn style_options(app: &SolveApp, style: &str) -> Map<String, Value> {
    let e = &app.electronics.editor;
    let mut out = Map::new();
    match style {
        "led" => {
            let leds = e.leds.min(3);
            out.insert("count".into(), json!(leds.saturating_add(1)));
            if leds > 0 {
                out.insert("common".into(), json!(if e.common == 1 { "cathode" } else { "anode" }));
            }
        }
        "crystal" | "switch" => {
            out.insert("count".into(), json!(if e.four == 1 { 4 } else { 2 }));
        }
        "tvs" => {
            out.insert("bidirectional".into(), json!(e.both_ways));
        }
        _ => {}
    }
    out
}

/// The rows that set [`style_options`] for `style`.
fn style_option_rows(app: &mut SolveApp, ui: &mut Ui, style: &str) {
    let e = &mut app.electronics.editor;
    match style {
        "led" => {
            ui_kit::choice_row(ui, &tr("electronics.field_leds"), "electronics_leds", &mut e.leds, &["1", "2", "3", "4"]);
            e.leds = e.leds.min(3);
            if e.leds > 0 {
                let [anode, cathode] = ["electronics.common_anode", "electronics.common_cathode"].map(tr);
                ui_kit::choice_row(ui, &tr("electronics.field_common"), "electronics_common", &mut e.common, &[&anode, &cathode]);
            }
        }
        "crystal" | "switch" => {
            ui_kit::choice_row(ui, &tr("electronics.field_pins"), "electronics_shape_pins", &mut e.four, &["2", "4"]);
        }
        "tvs" if ui_kit::checkbox_row(ui, "electronics_bidirectional", &tr("electronics.field_bidirectional"), e.both_ways) => {
            e.both_ways = !e.both_ways;
        }
        _ => {}
    }
}

/// Every style as a small picture (the body and the pins, no texts) with its name under it, in
/// rows as the width allows; the one at `picked` drawn with `options` and outlined. `Some` with
/// the one clicked. Each is published for scenarios as `electronics:editor:style:<style>`.
fn style_tiles(ui: &mut Ui, picked: usize, options: &Map<String, Value>) -> Option<usize> {
    let t = Tokens::get();
    let ink = Tokens::view().text;
    let names = translated("", &STYLES);
    let gap = t.space_4;
    let width = ui.available_width();
    let columns = if width.is_finite() { ((width + gap) / (STYLE_TILE + gap)).floor().clamp(1.0, 16.0) as usize } else { 1 };
    let name_height = t.font_small + t.space_6;
    let font = egui::FontId::proportional(t.font_small);
    let styles: Vec<(usize, &str)> = STYLES.iter().map(|s| s.0).enumerate().collect();
    let mut clicked = None;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
        for row in styles.chunks(columns) {
            ui.horizontal(|ui| {
                for &(i, style) in row {
                    let (rect, response) = ui.allocate_exact_size(egui::vec2(STYLE_TILE, STYLE_TILE + name_height), egui::Sense::click());
                    let picture = Rect::from_min_size(rect.min, egui::vec2(STYLE_TILE, STYLE_TILE));
                    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                    painter.rect_filled(picture, t.radius_sm, t.viewport_bottom);
                    let mut params = if i == picked { options.clone() } else { Map::new() };
                    params.insert("style".into(), json!(style));
                    if let Some(symbol) = solidcraft_engine::cmd::symbol_style_preview(&Value::Object(params)) {
                        let graphics: Vec<_> = symbol.graphics.iter().filter(|g| g.kind != GraphicType::Text).cloned().collect();
                        let bare = SymbolItem { graphics, ..symbol };
                        if let Some(map) = PageMap::framing(bare.bbox(), picture.shrink(t.space_4), 6.0) {
                            for g in &bare.graphics {
                                crate::electronics_components::paint_graphic(&painter, map, g, ink, &font);
                            }
                            for pin in &bare.pins {
                                let (a, b) = (map.to_screen(pin.at[0], pin.at[1]), map.to_screen(pin.body_end()[0], pin.body_end()[1]));
                                painter.line_segment([a, b], egui::Stroke::new(1.0, ink));
                            }
                        }
                    }
                    let outline = if i == picked {
                        egui::Stroke::new(t.space_2, t.form_primary)
                    } else if response.hovered() {
                        egui::Stroke::new(1.0, t.border)
                    } else {
                        egui::Stroke::NONE
                    };
                    painter.rect_stroke(picture, t.radius_sm, outline, egui::StrokeKind::Inside);
                    let name = names.get(i).cloned().unwrap_or_default();
                    let text = if i == picked { t.form_ink } else { t.form_dim };
                    let shown = crate::electronics_components::fit(&painter, &name, t.font_small, STYLE_TILE, text);
                    painter.text(egui::pos2(rect.center().x, picture.bottom() + t.space_2), egui::Align2::CENTER_TOP, shown, font.clone(), text);
                    crate::scenario::publish_target(&format!("electronics:editor:style:{style}"), rect);
                    if response.on_hover_text(name).clicked() {
                        clicked = Some(i);
                    }
                }
            });
        }
    });
    clicked
}

/// What the generator makes with `params` (a style, its options and pins), drawn as the Library
/// shows a symbol: its pins numbered and named.
fn style_preview(app: &SolveApp, ui: &mut Ui, params: &Map<String, Value>) {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), STYLE_PREVIEW), egui::Sense::hover());
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, t.radius_sm, t.viewport_bottom);
    if let Some(symbol) = solidcraft_engine::cmd::symbol_style_preview(&Value::Object(params.clone()))
        && let Some(map) = PageMap::framing(symbol.bbox(), rect.shrink(t.space_8), 10.0)
    {
        crate::electronics_components::paint_symbol(app, &painter, map, &symbol, false);
    }
    crate::scenario::publish_rect("electronics:editor:style_preview", rect);
}

/// The pins a shape gets in place of `old`, a symbol of as many pins: for each of the shape's
/// (`shape`, in its order and numbered as usual), the old pin named as its role (a diode's `A`),
/// else the one numbered as it is, else the next one left, with its number, name and type. So the
/// pins keep their pads and what hangs on them, and a symbol whose anode was 1 keeps it on the
/// anode. `None` when the counts differ.
fn carried_pins(old: &[Pin], shape: &[Pin]) -> Option<Vec<Value>> {
    if old.is_empty() || old.len() != shape.len() {
        return None;
    }
    // A shape's pin is named as its role, or has no name and its number for a role.
    let role = |q: &Pin| if q.name.is_empty() { q.number.clone() } else { q.name.clone() };
    let by_name = |o: &Pin, q: &Pin| o.name == role(q);
    let by_number = |o: &Pin, q: &Pin| o.number == q.number;
    let any = |_: &Pin, _: &Pin| true;
    let tests: [&dyn Fn(&Pin, &Pin) -> bool; 3] = [&by_name, &by_number, &any];
    let mut used = vec![false; old.len()];
    let mut picks: Vec<Option<usize>> = vec![None; shape.len()];
    for test in tests {
        for (pick, q) in picks.iter_mut().zip(shape).filter(|(pick, _)| pick.is_none()) {
            if let Some((i, u)) = old.iter().zip(used.iter_mut()).enumerate().find_map(|(i, (o, u))| (!*u && test(o, q)).then_some((i, u))) {
                (*pick, *u) = (Some(i), true);
            }
        }
    }
    picks
        .into_iter()
        .map(|i| {
            let o = old.get(i?)?;
            let mut pin = json!({"number": o.number});
            if !o.name.is_empty() {
                pin["name"] = json!(o.name);
            }
            if let Some((_, electrical)) = PIN_TYPES.iter().find(|(k, _)| *k == o.electrical) {
                pin["electrical"] = json!(electrical);
            }
            Some(pin)
        })
        .collect()
}

/// When a symbol change moved a connection on the schematic (`reply`'s `netlist_changed`: a pin
/// that moved landed on another wire or left its own), say how, as an error; else when it let go
/// of anything (`detached`), say as an error which pins it was on: the new symbol has none of
/// them, so it stays where they were.
pub(crate) fn detached_status(app: &mut SolveApp, reply: &Value) {
    let changes: Vec<solidcraft_engine::doc::ecad::connect::NetChange> = reply
        .get("netlist_changed")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let texts = |key: &str| -> Vec<String> {
                c.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
            };
            let more = c.get("more").and_then(Value::as_u64).and_then(|n| usize::try_from(n).ok()).unwrap_or(0);
            Some(solidcraft_engine::doc::ecad::connect::NetChange {
                net: c.get("net")?.as_str()?.to_string(),
                lost: texts("lost"),
                gained: texts("gained"),
                more,
            })
        })
        .collect();
    if let Some(text) = crate::electronics_schematic::nets_changed_text(app, &changes, "electronics.symbol_net") {
        app.set_status(text, true);
        return;
    }
    let Some(list) = reply.get("detached").and_then(Value::as_array).filter(|l| !l.is_empty()) else { return };
    let mut pins: Vec<String> = list.iter().filter_map(|d| Some(format!("{}.{}", d.get("designator")?.as_str()?, d.get("pin")?.as_str()?))).collect();
    pins.sort();
    pins.dedup();
    let more = pins.len().saturating_sub(NAMED_PINS);
    pins.truncate(NAMED_PINS);
    let mut named = pins.join(", ");
    if more > 0 {
        named = crate::i18n::format("electronics.pins_more", &[("pins", &named), ("n", &more.to_string())]);
    }
    app.set_status(crate::i18n::format("electronics.pins_let_go", &[("pins", &named)]), true);
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

/// The footprint generator: a package and its numbers.
fn footprint_generator(app: &mut SolveApp, ui: &mut Ui, c: Option<&Shown>) {
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
    if generate_target(app, c, false).1 {
        ui_kit::text(ui, app.tr("electronics.footprint_replaces"), ui_kit::Text::Note("electronics.footprint_replaces"));
    }
    if !button(ui, "generate-footprint", &tr("electronics.generate_footprint"), true) {
        return;
    }
    let Some(params) = footprint_params(app, c, &labels) else { return };
    if app.run("ecad.footprint.generate", params).is_ok() {
        app.electronics.editor.picked = None;
    }
}

/// The generator's form as `ecad.footprint.generate` parameters; `None` after saying what isn't a
/// number.
fn footprint_params(app: &mut SolveApp, c: Option<&Shown>, labels: &[String; 9]) -> Option<Value> {
    let (mut target, _) = generate_target(app, c, false);
    let e = &app.electronics.editor;
    let kind = PACKAGES.get(e.package).map_or("chip", |p| p.0);
    target.insert("package".into(), json!(kind));
    let mut params = Value::Object(target);
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

/// Which pins connect to which pads: pick a pin, then click its pads, here or on the footprint.
fn connections(app: &mut SolveApp, ui: &mut Ui, c: &Shown) {
    ui_kit::text(ui, app.tr("electronics.connect_help"), ui_kit::Text::Note("electronics.connect_help"));
    let rows = c.resolved().connections();
    let active = app.electronics.editor.connect.clone();
    let none = tr("electronics.connect_no_pad");
    for (pin, pads) in &rows {
        let label = match c.symbol.pin(pin).map(|p| p.name.as_str()) {
            Some(name) if !name.is_empty() => crate::i18n::format("electronics.connection_row", &[("pin", pin), ("name", name)]),
            _ => crate::i18n::format("electronics.connection_row_unnamed", &[("pin", pin)]),
        };
        let text = format!("{label}  →  {}", if pads.is_empty() { none.clone() } else { pads.join(", ") });
        if ui_kit::list_picker(ui, &format!("connect:{pin}"), &text, active.as_deref() == Some(pin)).clicked() {
            // A second click on the pin being connected lets it go.
            app.electronics.editor.connect = (active.as_deref() != Some(pin)).then(|| pin.to_string());
        }
    }
    if let Some(pin) = app.electronics.editor.connect.clone()
        && let Some((_, mine)) = rows.iter().find(|(p, _)| *p == pin)
    {
        let title = crate::i18n::format("electronics.connect_pads_of", &[("pin", &pin)]);
        ui_kit::text(ui, &title, ui_kit::Text::Note("electronics.connect_pads_of"));
        let mut toggled = None;
        ui.horizontal_wrapped(|ui| {
            for pad in c.footprint.pads.iter().filter(|p| p.kind != PadKind::Hole) {
                let owner = rows.iter().find(|(p, pads)| *p != pin && pads.contains(&pad.number.as_str())).map(|(p, _)| *p);
                let response = ui_kit::choice_chip(ui, &format!("connect-pad:{}", pad.number), &pad.number, mine.contains(&pad.number.as_str()));
                let response = match owner {
                    Some(owner) => response.on_hover_text(crate::i18n::format("electronics.connect_owned", &[("pin", owner)])),
                    None => response,
                };
                if response.clicked() {
                    toggled = Some(pad.number.clone());
                }
            }
        });
        if let Some(pad) = toggled {
            toggle_pad(app, c, &pin, &pad);
        }
    }
    if button(ui, "by-number", &tr("electronics.connect_by_number"), true) {
        drop(app.run("ecad.component.map_pins", of_variant(c, json!({"component": c.component.id, "by_number": true}))));
    }
}

/// `params` for the open component's chosen variant (its first when it has none chosen).
fn of_variant(c: &Shown, mut params: Value) -> Value {
    if let Some(v) = &c.variant {
        params["variant"] = json!(v.name);
    }
    params
}

/// Connect `pad` to `pin`, or disconnect it when it already is. A pad connects to one pin, so it
/// leaves the pin that had it.
pub(crate) fn toggle_pad(app: &mut SolveApp, c: &Shown, pin: &str, pad: &str) {
    let rows = c.resolved().connections();
    let had = rows.iter().any(|(p, pads)| *p == pin && pads.contains(&pad));
    let mut map = Map::new();
    for (other, pads) in &rows {
        let mut pads: Vec<&str> = pads.iter().copied().filter(|p| *p != pad).collect();
        if *other == pin && !had {
            pads.push(pad);
        }
        map.insert(other.to_string(), json!(pads));
    }
    drop(app.run("ecad.component.map_pins", of_variant(c, json!({"component": c.component.id, "map": map}))));
}

/// Which SolidCraft component is the 3D package of the shown variant, and where it sits: a new
/// library 3D package (named after the component) when the variant has none, else that package,
/// which every variant using it follows.
pub(crate) fn package(app: &mut SolveApp, ui: &mut Ui, c: &Shown) {
    let models: Vec<(u64, String)> = app.session.doc.components.iter().map(|m| (m.id, m.name.clone())).collect();
    if models.is_empty() {
        crate::electronics_library::import_model(app, ui);
        return;
    }
    let mut options = vec![tr("electronics.package_none")];
    options.extend(models.iter().map(|(_, name)| name.clone()));
    let at = c.package.as_ref().and_then(|p| models.iter().position(|(id, _)| *id == p.model)).map_or(0, |i| i + 1);
    let label = tr("electronics.field_model");
    let (translate, rotation) = c.package.as_ref().map_or(([0.0; 3], 0.0), |p| (p.translate, p.rotation));
    let id = c.component.id;
    match choice(ui, &format!("{id}:package"), &label, at, &options) {
        Some(0) => {
            if let Some(v) = &c.variant {
                drop(app.run("ecad.component.variant.edit", json!({"component": id, "variant": v.name, "package": null})));
            }
        }
        Some(i) => {
            if let Some((model, _)) = models.get(i - 1) {
                match &c.package {
                    Some(p) => drop(app.run("ecad.package.edit", json!({"package": p.id, "model": model}))),
                    None => {
                        let taken: Vec<String> = app.session.doc.ecad.packages.iter().map(|p| p.name.clone()).collect();
                        let name = free_from(&c.component.name, &taken);
                        let made = json!({"name": name, "model": model, "translate": translate, "rotation": rotation, "component": id});
                        drop(app.run("ecad.package.create", of_variant(c, made)));
                    }
                }
            }
        }
        None => {}
    }
    let Some(p) = &c.package else { return };
    let key = |f: &str| format!("{id}:package:{f}");
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
        drop(app.run("ecad.package.edit", json!({"package": p.id, "translate": t, "rotation": r})));
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

fn checks(app: &mut SolveApp, ui: &mut Ui, c: &Shown) {
    let r = c.resolved();
    let mut problems = r.problems();
    // A 3D package whose model is gone keeps the part from being ready, as `ecad.component.inspect` says.
    let missing = c.package.as_ref().is_some_and(|p| !app.session.doc.components.iter().any(|m| m.id == p.model));
    let ready = r.ready_with(&problems) && !missing;
    problems.extend(r.symbol_problems());
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
    if let Some(gap) = c.footprint.min_pad_gap() {
        let text = crate::i18n::format("electronics.min_pad_gap", &[("gap", &mm(gap.gap)), ("a", &gap.pads[0]), ("b", &gap.pads[1])]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.min_pad_gap"));
    }
    if let Some([[x0, y0], [x1, y1]]) = c.footprint.courtyard(Side::Top) {
        let text = crate::i18n::format("electronics.courtyard_size", &[("w", &mm(x1 - x0)), ("h", &mm(y1 - y0))]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.courtyard_size"));
    }
}

#[cfg(test)]
#[path = "electronics_editor_tests.rs"]
mod tests;
