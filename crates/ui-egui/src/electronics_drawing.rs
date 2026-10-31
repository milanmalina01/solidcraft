//! Drawing on an open symbol or footprint: pins, pads, lines, rectangles, circles and text,
//! placed with clicks on the page, on the grid (Ctrl or Cmd: off it). Footprint graphics go on the
//! chosen board layer. A picked graphic can be moved, re-sized by its fields and deleted. Each
//! change is one `ecad.symbol.set_graphics` or `ecad.footprint.set_graphics`, so undo takes back one
//! shape.

use egui::{Pos2, Stroke, Ui};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::{BoardLayer, Graphic, GraphicType, PadKind, Side, SymbolItem};

use crate::SolveApp;
use crate::electronics_components::{Shown, View, shown, shown_copy};
use crate::electronics_workspace::PageMap;
use crate::ui_kit;

/// A press this close to a graphic's line, in points, picks it.
const PICK_POINTS: f32 = 5.0;
/// Symbol bodies are drawn on half the schematic grid.
const SYMBOL_STEP: f64 = 1.27;
/// The most points one drawn line takes.
const MAX_LINE_POINTS: usize = 256;

/// What the armed tool makes: a pin or a pad where you click, or a shape from clicked points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Pin,
    Pad,
    Line,
    Rect,
    Circle,
    Text,
}

/// A toolbar tool: what it makes, and a pad's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tool {
    pub shape: Shape,
    pub pad: Option<PadKind>,
}

/// Each tool and the toolbar command it stands for.
const TOOLS: [(Tool, &str); 8] = [
    (Tool { shape: Shape::Pin, pad: None }, "ecad.pin.add"),
    (Tool { shape: Shape::Pad, pad: Some(PadKind::Smd) }, "ecad.pad.add"),
    (Tool { shape: Shape::Pad, pad: Some(PadKind::ThroughHole) }, "ecad.pad.add.through_hole"),
    (Tool { shape: Shape::Pad, pad: Some(PadKind::Hole) }, "ecad.pad.add.hole"),
    (Tool { shape: Shape::Line, pad: None }, "ecad.draw.line"),
    (Tool { shape: Shape::Rect, pad: None }, "ecad.draw.rect"),
    (Tool { shape: Shape::Circle, pad: None }, "ecad.draw.circle"),
    (Tool { shape: Shape::Text, pad: None }, "ecad.draw.text"),
];

/// The tool a toolbar command arms, if it arms one.
pub(crate) fn tool_of(command: &str) -> Option<Tool> {
    TOOLS.iter().find(|(_, id)| *id == command).map(|(t, _)| *t)
}

/// The toolbar command of the armed tool, shown pressed.
pub(crate) fn armed_command(app: &SolveApp) -> Option<&'static str> {
    let draw = app.electronics.editor.draw.as_ref()?;
    TOOLS.iter().find(|(t, _)| t.shape == draw.shape && t.pad == draw.pad).map(|(_, id)| *id)
}

/// The footprint layers one can draw on, in the order the panel lists them.
const LAYERS: [(BoardLayer, &str, &str); 3] = [
    (BoardLayer::Silkscreen, "silkscreen", "electronics.layer_silkscreen"),
    (BoardLayer::Fab, "fab", "electronics.layer_fab"),
    (BoardLayer::Courtyard, "courtyard", "electronics.layer_courtyard"),
];

/// The armed draw tool, the points clicked so far, and the quarter turns R or a right-click gave
/// what it places (`None`: a pin points toward the symbol's middle).
pub struct Draw {
    pub shape: Shape,
    /// The pad tool's kind of pad.
    pub pad: Option<PadKind>,
    pub points: Vec<[f64; 2]>,
    pub turn: Option<u8>,
}

/// Symbol grid lines closer than this on screen, in points, step up to the next size.
const MIN_GRID_POINTS: f64 = 10.0;

/// The schematic grid's steps, 0.05 in and up, for the symbol page.
const INCH_STEPS: [f64; 10] = [1.27, 2.54, 5.08, 12.7, 25.4, 50.8, 127.0, 254.0, 508.0, 1270.0];

/// The pin directions a quarter turn apart, counter-clockwise.
const TURNS: [&str; 4] = ["right", "up", "left", "down"];

/// The draw tool's settings in the panel.
pub struct Settings {
    pub layer: usize,
    pub text: String,
    pub width: String,
    pub height: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { layer: 0, text: "{designator}".into(), width: String::new(), height: String::new() }
    }
}

/// Where a graphic lives: the symbol, or one layer and side of the footprint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Symbol,
    Footprint(BoardLayer, Side),
}

impl Target {
    fn of(g: &Graphic) -> Target {
        g.layer.map_or(Target::Symbol, |l| Target::Footprint(l, g.side))
    }

    /// The graphics on this target, in order, with their index in the symbol's or footprint's list.
    fn list(self, c: &Shown) -> Vec<(usize, &Graphic)> {
        match self {
            Target::Symbol => c.symbol.graphics.iter().enumerate().collect(),
            Target::Footprint(l, s) => c.footprint.graphics.iter().enumerate().filter(|(_, g)| g.layer == Some(l) && g.side == s).collect(),
        }
    }
}

/// The graphics of the shown view, by index in its list.
pub(crate) fn graphics(app: &SolveApp) -> &[Graphic] {
    match (shown(app), app.electronics.view) {
        (Some(c), View::Symbol) => &c.symbol.graphics,
        (Some(c), View::Footprint) => &c.footprint.graphics,
        (None, _) => &[],
    }
}

/// The page's grid step at this zoom: the schematic grid (0.05 in and up) on a symbol, millimetres
/// on a footprint. The page draws it and points snap to it, so a snapped point is on a line.
pub(crate) fn grid_step(app: &SolveApp) -> f64 {
    let half_height = app.electronics.page.cam.half_height();
    let inch = crate::electronics_workspace::schematic(app) || app.electronics.view == View::Symbol;
    match if inch { View::Symbol } else { View::Footprint } {
        // The finest 0.05 in step whose lines are at least this far apart on screen, so pins
        // (on the 0.1 in grid) sit on lines until they would crowd the page.
        View::Symbol => {
            let points_per_mm = app.viewport.rect.map_or(0.0, |r| f64::from(r.height()) / (2.0 * half_height));
            INCH_STEPS.iter().copied().find(|s| s * points_per_mm >= MIN_GRID_POINTS).unwrap_or(INCH_STEPS[INCH_STEPS.len() - 1])
        }
        View::Footprint => solidcraft_engine::view::grid_step(half_height).0,
    }
}

/// The step drawn points snap to: the grid, and for a pin's end (where a wire meets it) never
/// less than the full 0.1 in grid.
pub(crate) fn step(app: &SolveApp) -> f64 {
    let grid = grid_step(app);
    match app.electronics.editor.draw.as_ref().map(|d| d.shape) {
        Some(Shape::Pin) => (grid / (2.0 * SYMBOL_STEP)).ceil().max(1.0) * 2.0 * SYMBOL_STEP,
        _ => grid,
    }
}

/// The direction a pin placed at `at` gets: toward the symbol's middle, unless turned.
fn pin_direction(symbol: &SymbolItem, at: [f64; 2], turn: Option<u8>) -> &'static str {
    match turn {
        Some(t) => TURNS[usize::from(t % 4)],
        None => {
            let middle = symbol.bbox().map_or(0.0, |[a, b]| (a[0] + b[0]) / 2.0);
            if at[0] <= middle { "right" } else { "left" }
        }
    }
}

/// R or a right-click with a pin, pad or text tool armed: a quarter turn counter-clockwise.
/// `false` when no such tool is armed.
pub(crate) fn rotate(app: &mut SolveApp, at: Option<[f64; 2]>) -> bool {
    let auto = match (shown(app), at) {
        (Some(c), Some(at)) => TURNS.iter().position(|d| *d == pin_direction(c.symbol, at, None)).unwrap_or(0) as u8,
        _ => 0,
    };
    let Some(draw) = app.electronics.editor.draw.as_mut().filter(|d| matches!(d.shape, Shape::Pin | Shape::Pad | Shape::Text)) else {
        return false;
    };
    draw.turn = Some((draw.turn.unwrap_or(auto) + 1) % 4);
    true
}

/// `p` on the page, snapped to the grid unless `free`.
pub(crate) fn snap(app: &SolveApp, map: PageMap, p: Pos2, free: bool) -> [f64; 2] {
    let (x, y) = map.to_page(p);
    let step = step(app);
    if free || !(step.is_finite() && step > 0.0) {
        return [tidy(x), tidy(y)];
    }
    [tidy((x / step).round() * step), tidy((y / step).round() * step)]
}

fn tidy(v: f64) -> f64 {
    (v * 1e6).round() / 1e6 + 0.0
}

/// A copy of `g` moved by `by`.
pub(crate) fn translate(g: &Graphic, [dx, dy]: [f64; 2]) -> Graphic {
    let mv = |p: [f64; 2]| [tidy(p[0] + dx), tidy(p[1] + dy)];
    Graphic {
        points: g.points.iter().copied().map(mv).collect(),
        from: g.from.map(mv),
        to: g.to.map(mv),
        center: g.center.map(mv),
        at: g.at.map(mv),
        ..g.clone()
    }
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

fn polyline_distance(points: &[Pos2], p: Pos2, closed: bool) -> f32 {
    let n = points.len();
    let segments = if closed { n } else { n.saturating_sub(1) };
    (0..segments).map(|i| segment_distance(p, points[i], points[(i + 1) % n])).fold(f32::INFINITY, f32::min)
}

/// How far `p` is from `g` on screen, in points (0 inside a filled shape or a text's box).
pub(crate) fn distance(g: &Graphic, map: PageMap, p: Pos2) -> f32 {
    let screen = |q: &[f64; 2]| map.to_screen(q[0], q[1]);
    match g.kind {
        GraphicType::Line => polyline_distance(&g.points.iter().map(screen).collect::<Vec<_>>(), p, false),
        GraphicType::Polygon => polyline_distance(&g.points.iter().map(screen).collect::<Vec<_>>(), p, true),
        GraphicType::Arc => polyline_distance(&g.arc_points(48).iter().map(screen).collect::<Vec<_>>(), p, false),
        GraphicType::Rect => {
            let (Some(a), Some(b)) = (g.from, g.to) else { return f32::INFINITY };
            let corners: Vec<Pos2> = [a, [b[0], a[1]], b, [a[0], b[1]]].iter().map(screen).collect();
            let rect = egui::Rect::from_two_pos(corners[0], corners[2]);
            if g.fill && rect.contains(p) { 0.0 } else { polyline_distance(&corners, p, true) }
        }
        GraphicType::Circle => {
            let (Some(c), Some(r)) = (g.center, g.radius) else { return f32::INFINITY };
            let d = screen(&c).distance(p);
            let r = (r * map.scale()) as f32;
            if g.fill && d <= r { 0.0 } else { (d - r).abs() }
        }
        GraphicType::Text => {
            let (Some(at), Some(h)) = (g.at, g.height) else { return f32::INFINITY };
            // A box as wide as the text's characters at about 0.6 of its height each (drawn with
            // its bars: their markup takes no room).
            let width = solidcraft_engine::doc::ecad::overbar::plain(&g.text).chars().count() as f64 * h * 0.6;
            let a = screen(&at);
            let b = map.to_screen(at[0] + width, at[1] + h);
            if egui::Rect::from_two_pos(a, b).contains(p) { 0.0 } else { f32::INFINITY }
        }
    }
}

/// The graphic of the shown view under `p` (the last drawn on top).
pub(crate) fn hit(app: &SolveApp, map: PageMap, p: Pos2) -> Option<usize> {
    graphics(app)
        .iter()
        .enumerate()
        .rev()
        .map(|(i, g)| (i, distance(g, map, p)))
        .filter(|(_, d)| *d <= PICK_POINTS)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// A graphic in the shape the graphics commands take: its layer and side are the command's.
fn spec(g: &Graphic) -> Value {
    let mut v = serde_json::to_value(g).unwrap_or(Value::Null);
    if let Some(map) = v.as_object_mut() {
        map.remove("layer");
        map.remove("side");
    }
    v
}

/// Replace `target`'s graphics (on the shown symbol or footprint `c`) with `list`. `true` when it
/// took.
fn set(app: &mut SolveApp, c: &Shown, target: Target, list: &[Graphic]) -> bool {
    let graphics: Vec<Value> = list.iter().map(spec).collect();
    let (command, params) = match target {
        Target::Symbol => ("ecad.symbol.set_graphics", json!({"symbol": c.symbol.id, "graphics": graphics})),
        Target::Footprint(l, s) => {
            let layer = LAYERS.iter().find(|(x, _, _)| *x == l).map_or("silkscreen", |(_, k, _)| *k);
            let side = if s == Side::Bottom { "bottom" } else { "top" };
            ("ecad.footprint.set_graphics", json!({"footprint": c.footprint.id, "layer": layer, "side": side, "graphics": graphics}))
        }
    };
    app.run(command, params).is_ok()
}

/// Change graphic `index` of the shown view with `change` (`None`: delete it), keeping it picked
/// where it lands.
pub(crate) fn change(app: &mut SolveApp, index: usize, change: impl FnOnce(&Graphic) -> Option<Graphic>) {
    let Some(c) = shown_copy(app) else { return };
    let Some(g) = graphics(app).get(index).cloned() else { return };
    let target = Target::of(&g);
    let replaced = change(&g);
    let on = target.list(&c);
    let k = on.iter().position(|(i, _)| *i == index);
    let list: Vec<Graphic> = on.iter().filter_map(|(i, x)| if *i == index { replaced.clone() } else { Some((*x).clone()) }).collect();
    let count = list.len();
    if !set(app, &c, target, &list) {
        return;
    }
    // The footprint keeps its other layers in place and puts this one after them.
    app.electronics.editor.picked = match (replaced, k) {
        (Some(_), Some(k)) => Some(crate::electronics_editor::Item::Graphic(match target {
            Target::Symbol => k,
            Target::Footprint(..) => graphics(app).len().saturating_sub(count) + k,
        })),
        _ => None,
    };
}

/// The armed tool's options and how to use it, at the top of the panel.
pub(crate) fn options(app: &mut SolveApp, ui: &mut Ui) {
    let Some(shape) = app.electronics.editor.draw.as_ref().map(|d| d.shape) else { return };
    let footprint = app.electronics.view == View::Footprint;
    let s = &mut app.electronics.editor.settings;
    if footprint && !matches!(shape, Shape::Pin | Shape::Pad) {
        let layers: Vec<String> = LAYERS.iter().map(|(_, _, k)| crate::i18n::tr(k).to_string()).collect();
        let options: Vec<&str> = layers.iter().map(String::as_str).collect();
        ui_kit::choice_row(ui, crate::i18n::tr("electronics.field_layer"), "electronics_draw_layer", &mut s.layer, &options);
        s.layer = s.layer.min(LAYERS.len() - 1);
    }
    match shape {
        Shape::Text => {
            ui_kit::text_row(ui, crate::i18n::tr("electronics.field_text"), &mut s.text, "{designator}");
            let hint = mm(default_height(footprint));
            ui_kit::text_row(ui, crate::i18n::tr("electronics.field_text_height"), &mut s.height, &hint);
            ui_kit::text(ui, crate::i18n::tr("electronics.placeholders_help"), ui_kit::Text::Note("electronics.placeholders_help"));
        }
        Shape::Line | Shape::Rect | Shape::Circle => {
            let layer = LAYERS.get(s.layer).map_or(BoardLayer::Silkscreen, |l| l.0);
            let hint = mm(default_width(footprint.then_some(layer)));
            ui_kit::text_row(ui, crate::i18n::tr("electronics.field_line_width"), &mut s.width, &hint);
        }
        Shape::Pin | Shape::Pad => {}
    }
    let help = match shape {
        Shape::Pin => "electronics.place_pin_help",
        Shape::Pad => "electronics.place_pad_help",
        Shape::Line => "electronics.draw_line_help",
        Shape::Rect => "electronics.draw_rect_help",
        Shape::Circle => "electronics.draw_circle_help",
        Shape::Text => "electronics.draw_text_help",
    };
    ui_kit::text(ui, crate::i18n::tr(help), ui_kit::Text::Note(help));
}

fn mm(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A line's width when the panel leaves it empty: thick enough to read on a symbol, the usual
/// widths on each footprint layer.
fn default_width(layer: Option<BoardLayer>) -> f64 {
    match layer {
        None => 0.254,
        Some(BoardLayer::Silkscreen) => 0.12,
        Some(BoardLayer::Fab) => 0.1,
        Some(BoardLayer::Courtyard) => 0.05,
    }
}

fn default_height(footprint: bool) -> f64 {
    if footprint { 1.0 } else { 1.27 }
}

/// A number typed in a settings field, or `default` when it is empty; `None` (after saying so)
/// when it isn't a positive number.
fn setting(app: &mut SolveApp, text: &str, default: f64, label: &str) -> Option<f64> {
    if text.trim().is_empty() {
        return Some(default);
    }
    let v = crate::prefs::parse_number(text).filter(|v| *v > 0.0);
    if v.is_none() {
        app.set_status(crate::i18n::format("electronics.not_a_number", &[("field", crate::i18n::tr(label)), ("text", text)]), true);
    }
    v
}

/// A click on the page with a draw tool armed, at `at` (snapped). `double` finishes a line.
pub(crate) fn click(app: &mut SolveApp, at: [f64; 2], double: bool) {
    let Some(draw) = app.electronics.editor.draw.as_mut() else { return };
    if draw.points.last() != Some(&at) && draw.points.len() < MAX_LINE_POINTS {
        draw.points.push(at);
    }
    let done = match draw.shape {
        Shape::Line => double && draw.points.len() >= 2,
        Shape::Rect | Shape::Circle => draw.points.len() >= 2,
        Shape::Pin | Shape::Pad | Shape::Text => !draw.points.is_empty(),
    };
    if done {
        finish(app);
    }
}

/// Make the pin, pad or shape from the clicked points; the tool stays armed for the next one.
pub(crate) fn finish(app: &mut SolveApp) {
    let Some(draw) = app.electronics.editor.draw.as_mut() else { return };
    let (shape, points) = (draw.shape, std::mem::take(&mut draw.points));
    let Some(c) = shown_copy(app) else { return };
    let turn = app.electronics.editor.draw.as_ref().and_then(|d| d.turn);
    match (shape, points.first()) {
        (Shape::Pin, Some(at)) => return crate::electronics_editor::add_pin(app, &c, Some((*at, pin_direction(&c.symbol, *at, turn)))),
        (Shape::Pad, Some(at)) => {
            let kind = app.electronics.editor.draw.as_ref().and_then(|d| d.pad).unwrap_or_default();
            return crate::electronics_editor::add_pad(app, &c, kind, *at, f64::from(turn.unwrap_or(0)) * 90.0);
        }
        _ => {}
    }
    let footprint = app.electronics.view == View::Footprint;
    let layer = LAYERS.get(app.electronics.editor.settings.layer).map_or(BoardLayer::Silkscreen, |l| l.0);
    let mut params = json!({});
    if footprint {
        params["layer"] = json!(LAYERS.iter().find(|(l, _, _)| *l == layer).map_or("silkscreen", |(_, k, _)| *k));
    }
    let s = &app.electronics.editor.settings;
    let (width_text, height_text, text) = (s.width.clone(), s.height.clone(), s.text.clone());
    let command = match shape {
        Shape::Text => {
            let Some(height) = setting(app, &height_text, default_height(footprint), "electronics.field_text_height") else { return };
            let Some(at) = points.first() else { return };
            if text.trim().is_empty() {
                return;
            }
            params["at"] = json!(at);
            params["text"] = json!(text);
            params["height"] = json!(height);
            if let Some(t) = turn {
                params["rotation"] = json!(f64::from(t % 4) * 90.0);
            }
            "ecad.draw.text"
        }
        _ => {
            let Some(width) = setting(app, &width_text, default_width(footprint.then_some(layer)), "electronics.field_line_width") else { return };
            params["width"] = json!(width);
            match (shape, points.as_slice()) {
                (Shape::Line, [_, _, ..]) => {
                    params["points"] = json!(points);
                    "ecad.draw.line"
                }
                (Shape::Rect, [a, b]) if a[0] != b[0] && a[1] != b[1] => {
                    params["from"] = json!(a);
                    params["to"] = json!(b);
                    "ecad.draw.rect"
                }
                (Shape::Circle, [c, r]) => {
                    let radius = tidy((r[0] - c[0]).hypot(r[1] - c[1]));
                    if radius <= 0.0 {
                        return;
                    }
                    params["center"] = json!(c);
                    params["radius"] = json!(radius);
                    "ecad.draw.circle"
                }
                _ => return,
            }
        }
    };
    // The shape goes on the shown symbol or footprint, made (named after the component) when it
    // has none.
    let on = if footprint { crate::electronics_editor::editable_footprint(app) } else { crate::electronics_editor::editable_symbol(app) };
    let Some(on) = on else { return };
    params[if footprint { "footprint" } else { "symbol" }] = json!(on);
    drop(app.run(command, params));
}

/// What the pin, pad or text tool would place at `at`, see-through.
fn ghost(app: &SolveApp, painter: &egui::Painter, map: PageMap, at: [f64; 2]) {
    let (Some(draw), Some(c)) = (&app.electronics.editor.draw, shown(app)) else { return };
    let t = crate::theme::Tokens::get();
    let colour = t.sel_edge.gamma_multiply(0.7);
    let font = egui::FontId::proportional(crate::theme::Tokens::view().font_small);
    let on_page = |p: Pos2| p.x.is_finite() && p.y.is_finite();
    match draw.shape {
        Shape::Pin => {
            let length = c.symbol.pins.last().map_or(2.0 * SYMBOL_STEP, |p| p.length);
            let [dx, dy] = match pin_direction(c.symbol, at, draw.turn) {
                "up" => [0.0, 1.0],
                "left" => [-1.0, 0.0],
                "down" => [0.0, -1.0],
                _ => [1.0, 0.0],
            };
            let (a, b) = (map.to_screen(at[0], at[1]), map.to_screen(at[0] + dx * length, at[1] + dy * length));
            if on_page(a) && on_page(b) {
                painter.line_segment([a, b], Stroke::new(2.0, colour));
                painter.circle_stroke(a, 3.0, Stroke::new(1.4, colour));
                let number = crate::electronics_editor::next_free(c.symbol.pins.iter().map(|p| p.number.as_str()));
                let middle = egui::pos2((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
                painter.text(middle - egui::vec2(0.0, 3.0), egui::Align2::CENTER_BOTTOM, number, font, colour);
            }
        }
        Shape::Pad => {
            let turn = f64::from(draw.turn.unwrap_or(0)) * 90.0;
            let pad = crate::electronics_editor::new_pad(c.footprint, draw.pad.unwrap_or_default(), at, turn);
            let points: Vec<Pos2> = pad.outline(6).iter().map(|p| map.to_screen(p[0], p[1])).collect();
            if points.iter().copied().all(on_page) {
                let fill = if pad.kind == PadKind::Hole { egui::Color32::TRANSPARENT } else { colour.gamma_multiply(0.35) };
                painter.add(egui::Shape::convex_polygon(points, fill, Stroke::new(1.2, colour)));
                painter.text(map.to_screen(at[0], at[1]), egui::Align2::CENTER_CENTER, pad.number, font, colour);
            }
        }
        Shape::Text => {
            let s = &app.electronics.editor.settings;
            let footprint = app.electronics.view == View::Footprint;
            let height = crate::prefs::parse_number(&s.height).filter(|h| *h > 0.0).unwrap_or(default_height(footprint));
            let size = (height * map.scale()) as f32;
            let p = map.to_screen(at[0], at[1]);
            if on_page(p) && (4.0..=256.0).contains(&size) {
                let font = egui::FontId::new(size, font.family);
                crate::electronics_components::barred_text(painter, p, egui::Align2::LEFT_BOTTOM, &s.text, font, colour, 0.0);
            }
        }
        _ => {}
    }
}

/// The shape being drawn, from the clicked points to the pointer, or a ghost of what the tool
/// places.
pub(crate) fn preview(app: &SolveApp, painter: &egui::Painter, map: PageMap, pointer: Option<[f64; 2]>) {
    let Some(draw) = &app.electronics.editor.draw else { return };
    if let Some(at) = pointer.filter(|_| matches!(draw.shape, Shape::Pin | Shape::Pad | Shape::Text)) {
        ghost(app, painter, map, at);
    }
    let t = crate::theme::Tokens::get();
    let stroke = Stroke::new(1.5, t.sel_edge);
    let mut points: Vec<[f64; 2]> = draw.points.clone();
    points.extend(pointer);
    let screen: Vec<Pos2> = points.iter().map(|p| map.to_screen(p[0], p[1])).collect();
    if !screen.iter().all(|p| p.x.is_finite() && p.y.is_finite()) {
        return;
    }
    match (draw.shape, screen.as_slice()) {
        (Shape::Line, [_, _, ..]) => {
            painter.add(egui::Shape::line(screen.clone(), stroke));
        }
        (Shape::Rect, [a, b, ..]) => {
            painter.rect_stroke(egui::Rect::from_two_pos(*a, *b), 0.0, stroke, egui::StrokeKind::Middle);
        }
        (Shape::Circle, [c, r, ..]) => {
            painter.circle_stroke(*c, c.distance(*r), stroke);
        }
        _ => {}
    }
    for p in &screen {
        painter.circle_filled(*p, 2.5, t.sel_edge);
    }
}

/// The picked graphic's section in the panel: what it is, its line width or text, and Delete.
pub(crate) fn section(app: &mut SolveApp, ui: &mut Ui, index: usize) {
    let Some(g) = graphics(app).get(index).cloned() else { return };
    let kind = match g.kind {
        GraphicType::Line | GraphicType::Polygon => "electronics.draw_line",
        GraphicType::Rect => "electronics.draw_rect",
        GraphicType::Circle | GraphicType::Arc => "electronics.draw_circle",
        GraphicType::Text => "electronics.draw_text",
    };
    let title = crate::i18n::tr(kind).to_string();
    ui_kit::section(ui, "electronics_graphic", &title, true, |ui| {
        if let Some(layer) = g.layer {
            let name = LAYERS.iter().find(|(l, _, _)| *l == layer).map_or("", |(_, _, k)| crate::i18n::tr(k));
            ui_kit::row(ui, crate::i18n::tr("electronics.field_layer"), |ui| ui_kit::text(ui, name, ui_kit::Text::Cell));
        }
        let key = |f: &str| format!("graphic:{index}:{f}");
        if g.kind == GraphicType::Text {
            if let Some(text) = crate::electronics_editor::field(app, ui, &key("text"), crate::i18n::tr("electronics.field_text"), &g.text)
                && !text.trim().is_empty()
            {
                change(app, index, |g| Some(Graphic { text, ..g.clone() }));
            }
            let height = g.height.unwrap_or(1.0);
            if let Some(h) = crate::electronics_editor::number(app, ui, &key("height"), crate::i18n::tr("electronics.field_text_height"), height) {
                change(app, index, |g| Some(Graphic { height: Some(h), width: tidy(h / 10.0), ..g.clone() }));
            }
        } else if let Some(w) = crate::electronics_editor::number(app, ui, &key("width"), crate::i18n::tr("electronics.field_line_width"), g.width) {
            change(app, index, |g| Some(Graphic { width: w, ..g.clone() }));
        }
        let label = crate::i18n::tr("electronics.delete_graphic").to_string();
        let response = ui_kit::button(ui, label.as_str(), ui_kit::Button::Normal);
        crate::scenario::publish_target("electronics:editor:delete-graphic", response.rect);
        if response.clicked() {
            change(app, index, |_| None);
        }
    });
}
