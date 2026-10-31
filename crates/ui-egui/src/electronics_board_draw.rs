//! What the board shows besides its copper and outline (see `electronics_board`): mounting holes
//! (the Hole tool), lines, rectangles, circles and text drawn on its silkscreen, documentation and
//! copper layers (the DRAW tools of the DOCUMENT tab), and each footprint's name and value text,
//! sized and set round its courtyard where it has no spot of its own (`ecad::names`), which a drag
//! moves on its own (`ecad.board.edit`). Picked holes and drawings show in the
//! Inspector, where a drawing's layer, size and text change (`ecad.board.draw.edit`) and a hole's
//! drill (`ecad.board.hole.edit`).

use std::sync::Arc;

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, vec2};
use serde_json::json;
use solidcraft_engine::doc::ecad::board::{DEFAULT_HOLE_DRILL, check_hole, footprint_box};
use solidcraft_engine::doc::ecad::graphics::{DEFAULT_WIDTH, shapes_bbox};
use solidcraft_engine::doc::ecad::names;
use solidcraft_engine::doc::ecad::{Board, BoardFootprint, BoardGraphic, BoardLayer, DrawLayer, Graphic, GraphicType, Side};

use crate::SolveApp;
use crate::electronics_board::{PICK_POINTS, Tool, board, layer_colour, snap};
use crate::electronics_board_layers::Layer;
use crate::electronics_board_route::layer_name;
use crate::electronics_editor::tr;
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;
use crate::ui_kit;

/// Names and values on the board are this tall, in millimetres, when nothing else says (a
/// footprint without a courtyard or copper); drawn text starts at it too.
pub(crate) const TEXT_HEIGHT: f64 = 1.0;
/// Text this small on screen, in points, is not drawn, and none is drawn bigger than the most.
const MIN_TEXT_POINTS: f32 = 4.0;
const MAX_TEXT_POINTS: f32 = 256.0;
/// The most points one drawn line takes.
const MAX_LINE_POINTS: usize = 256;

/// What a DRAW tool makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawShape {
    Line,
    Rect,
    Circle,
    Text,
}

/// Each DRAW tool and the toolbar command it stands for.
const SHAPES: [(DrawShape, &str); 4] = [
    (DrawShape::Line, "ecad.board.draw.line"),
    (DrawShape::Rect, "ecad.board.draw.rect"),
    (DrawShape::Circle, "ecad.board.draw.circle"),
    (DrawShape::Text, "ecad.board.draw.text"),
];

impl DrawShape {
    /// The DRAW tool a toolbar command arms, if any.
    pub(crate) fn of(command: &str) -> Option<Self> {
        SHAPES.iter().find(|(_, id)| *id == command).map(|(s, _)| *s)
    }

    pub(crate) fn command(self) -> &'static str {
        SHAPES.iter().find(|(s, _)| *s == self).map_or("ecad.board.draw.line", |(_, id)| id)
    }
}

/// A DRAW tool: its shape and the points clicked so far.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    pub shape: DrawShape,
    pub points: Vec<[f64; 2]>,
}

/// The DRAW tools' and the Hole tool's options.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawOptions {
    pub layer: DrawLayer,
    /// A line's width, in millimetres.
    pub width: f64,
    /// A text's height, in millimetres.
    pub height: f64,
    pub text: String,
    /// The Hole tool's drill, in millimetres.
    pub drill: f64,
}

impl Default for DrawOptions {
    fn default() -> Self {
        DrawOptions { layer: DrawLayer::SilkscreenTop, width: DEFAULT_WIDTH, height: TEXT_HEIGHT, text: String::new(), drill: DEFAULT_HOLE_DRILL }
    }
}

/// The layers one can draw on: the silkscreens, the documentation layer, then the stack's copper.
fn draw_layers(app: &SolveApp) -> Vec<DrawLayer> {
    let mut out = vec![DrawLayer::SilkscreenTop, DrawLayer::SilkscreenBottom, DrawLayer::Documentation];
    out.extend(board(app).stack().into_iter().map(DrawLayer::Copper));
    out
}

/// A drawing layer's name.
pub(crate) fn draw_layer_name(layer: DrawLayer) -> String {
    match layer {
        DrawLayer::SilkscreenTop => tr("electronics.layer_top_silkscreen"),
        DrawLayer::SilkscreenBottom => tr("electronics.layer_bottom_silkscreen"),
        DrawLayer::Documentation => tr("electronics.layer_documentation"),
        DrawLayer::Copper(l) => crate::i18n::format("electronics.board_layer_copper", &[("layer", &layer_name(l))]),
    }
}

/// The colour drawings on `layer` are drawn in.
pub(crate) fn draw_colour(t: &Tokens, layer: DrawLayer) -> Color32 {
    match layer {
        DrawLayer::SilkscreenTop => t.pcb_silkscreen,
        DrawLayer::SilkscreenBottom => t.pcb_silkscreen.gamma_multiply(0.6),
        DrawLayer::Documentation => t.pcb_documentation,
        DrawLayer::Copper(l) => layer_colour(t, l),
    }
}

/// The Layers tab's layer that shows drawings on `layer`.
pub(crate) fn shown_on(layer: DrawLayer) -> Layer {
    match layer {
        DrawLayer::SilkscreenTop => Layer::TopSilkscreen,
        DrawLayer::SilkscreenBottom => Layer::BottomSilkscreen,
        DrawLayer::Documentation => Layer::Fab,
        DrawLayer::Copper(l) => Layer::Copper(l),
    }
}

/// A dropdown of the drawing layers, each with its colour; `Some` with one other than `current`.
pub(crate) fn layer_menu(app: &SolveApp, ui: &mut Ui, id: &str, current: DrawLayer) -> Option<DrawLayer> {
    let t = Tokens::get();
    let mut chosen = None;
    ui_kit::dropdown_menu(ui, id, &draw_layer_name(current), |ui| {
        for layer in draw_layers(app) {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(t.space_12, t.space_12), Sense::hover());
                ui.painter().rect_filled(r.shrink(1.0), f32::from(t.control_radius) / 2.0, draw_colour(&t, layer));
                let response = ui_kit::list_picker(ui, &format!("{id}_{}", layer.name()), &draw_layer_name(layer), layer == current);
                crate::scenario::publish_handle(&format!("dialog:{id}:{}", layer.name()), response.rect.center());
                if response.clicked() {
                    chosen = Some(layer);
                }
            });
        }
    });
    chosen.filter(|l| *l != current)
}

/// How [`paint_glyphs`] mirrors text left to right, as text on the bottom reads seen from the top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mirror {
    No,
    /// About its start: it runs to the left of it (a drawing's text on the bottom).
    AboutStart,
    /// Within the box it fills (a bottom footprint's text, placed to cover its box).
    InPlace,
}

/// Text from its lower left `at` on screen, mirrored as `mirror` says: the glyphs are laid out as
/// egui lays out text, then drawn with their x turned round.
pub(crate) fn paint_glyphs(painter: &egui::Painter, at: Pos2, text: &str, font: FontId, colour: Color32, mirror: Mirror) {
    if mirror == Mirror::No {
        painter.text(at, Align2::LEFT_BOTTOM, text, font, colour);
        return;
    }
    let galley = painter.layout_no_wrap(text.to_string(), font, colour);
    let [w, h] = painter.ctx().fonts(|f| f.font_image_size());
    if w == 0 || h == 0 {
        return;
    }
    let uv = vec2(1.0 / w as f32, 1.0 / h as f32);
    let origin = at - vec2(0.0, galley.size().y);
    let axis = if mirror == Mirror::InPlace { at.x + galley.size().x / 2.0 } else { at.x };
    let mut mesh = egui::Mesh::default();
    for row in &galley.rows {
        let base = u32::try_from(mesh.vertices.len()).unwrap_or(u32::MAX);
        let offset = origin + row.pos.to_vec2();
        mesh.indices.extend(row.visuals.mesh.indices.iter().map(|i| i.saturating_add(base)));
        mesh.vertices.extend(row.visuals.mesh.vertices.iter().map(|v| {
            let p = offset + v.pos.to_vec2();
            let color = if v.color == Color32::PLACEHOLDER { colour } else { v.color };
            egui::epaint::Vertex { pos: egui::pos2(2.0 * axis - p.x, p.y), uv: (v.uv.to_vec2() * uv).to_pos2(), color }
        }));
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// A graphic in `colour`; a text mirrored as `mirror` says (the rest as the library draws them).
pub(crate) fn paint_shape(painter: &egui::Painter, map: PageMap, g: &Graphic, colour: Color32, font: &FontId, mirror: Mirror) {
    let (GraphicType::Text, Some(at), Some(height)) = (g.kind, g.at, g.height) else {
        crate::electronics_components::paint_graphic(painter, map, g, colour, font);
        return;
    };
    if mirror == Mirror::No {
        crate::electronics_components::paint_graphic(painter, map, g, colour, font);
        return;
    }
    let size = (height * map.scale()) as f32;
    let p = map.to_screen(at[0], at[1]);
    if (MIN_TEXT_POINTS..=MAX_TEXT_POINTS).contains(&size) && p.x.is_finite() && p.y.is_finite() {
        paint_glyphs(painter, p, &g.text, FontId::new(size, font.family.clone()), colour, mirror);
    }
}

/// A board drawing's graphic as the board shows it: a mirrored text's box runs to the left of
/// its `at`, for picking it.
fn laid_out(g: &BoardGraphic) -> std::borrow::Cow<'_, Graphic> {
    match (g.mirror, g.graphic.kind, g.graphic.at, g.graphic.height) {
        (true, GraphicType::Text, Some(at), Some(h)) => {
            let width = h * 0.6 * g.graphic.text.chars().count() as f64;
            std::borrow::Cow::Owned(Graphic { at: Some([at[0] - width, at[1]]), ..g.graphic.clone() })
        }
        _ => std::borrow::Cow::Borrowed(&g.graphic),
    }
}

/// The board's drawings on `layer`, the picked ones in the selection colour; a text made mirrored
/// (on a bottom layer) runs to the left of where it starts.
pub(crate) fn paint_drawings(app: &SolveApp, painter: &egui::Painter, map: PageMap, b: &Board, layer: DrawLayer, colour: Color32, t: &Tokens) {
    let font = FontId::proportional(Tokens::view().font_small);
    let picked = &app.electronics.board.selection;
    for g in b.graphics.iter().filter(|g| g.layer == layer) {
        let ink = if picked.contains(&g.id) { t.sel_edge } else { colour };
        paint_shape(painter, map, &g.graphic, ink, &font, if g.mirror { Mirror::AboutStart } else { Mirror::No });
    }
}

/// The board's mounting holes: a ring round the hole (the selection colour when picked).
pub(crate) fn paint_holes(app: &SolveApp, painter: &egui::Painter, map: PageMap, b: &Board, t: &Tokens) {
    let ring = Tokens::view().text_dim;
    for h in &b.holes {
        let c = map.to_screen(h.at[0], h.at[1]);
        if !(c.x.is_finite() && c.y.is_finite()) {
            continue;
        }
        let r = ((h.drill / 2.0 * map.scale()) as f32).max(2.0);
        let picked = app.electronics.board.selection.contains(&h.id);
        if picked {
            painter.circle_filled(c, r + 3.0, t.sel_edge_rim);
        }
        painter.circle(c, r, t.viewport_bottom, Stroke::new(1.5, if picked { t.sel_edge } else { ring }));
    }
}

/// The mounting hole or drawing under `p`, the last made on top; only those on shown layers.
pub(crate) fn hit(app: &SolveApp, map: PageMap, p: Pos2) -> Option<u64> {
    let b = board(app);
    let hidden = &app.electronics.board.hidden;
    if !hidden.contains(&Layer::Holes) {
        for h in b.holes.iter().rev() {
            let reach = ((h.drill / 2.0 * map.scale()) as f32).max(PICK_POINTS);
            if map.to_screen(h.at[0], h.at[1]).distance(p) <= reach {
                return Some(h.id);
            }
        }
    }
    b.graphics
        .iter()
        .rev()
        .filter(|g| !hidden.contains(&shown_on(g.layer)))
        .map(|g| (g.id, crate::electronics_drawing::distance(&laid_out(g), map, p)))
        .filter(|(_, d)| *d <= PICK_POINTS)
        .min_by(|a, c| a.1.total_cmp(&c.1))
        .map(|(id, _)| id)
}

/// A click with the Hole tool: a mounting hole there, its drill the options'.
pub(crate) fn hole_click(app: &mut SolveApp, map: PageMap, p: Pos2, free: bool) {
    let at = snap(app, map, p, free);
    let drill = app.electronics.board.draw.drill;
    drop(app.run("ecad.board.hole", json!({"at": at, "drill": drill})));
}

/// A click with a DRAW tool: a line's points (a double-click or Enter ends it), a rectangle's two
/// corners, a circle's centre then a point on it, a text's lower left.
pub(crate) fn click(app: &mut SolveApp, map: PageMap, p: Pos2, free: bool, double: bool) {
    let at = snap(app, map, p, free);
    let Some(Tool::Draw(draw)) = &mut app.electronics.board.tool else { return };
    if draw.points.last() != Some(&at) && draw.points.len() < MAX_LINE_POINTS {
        draw.points.push(at);
    }
    let done = match draw.shape {
        DrawShape::Line => double && draw.points.len() >= 2,
        DrawShape::Rect | DrawShape::Circle => draw.points.len() >= 2,
        DrawShape::Text => !double && !draw.points.is_empty(),
    };
    if done {
        finish(app);
    } else if double && draw.shape == DrawShape::Text {
        draw.points.clear();
    }
}

/// Make the shape clicked so far on the options' layer; the tool stays on for the next one.
pub(crate) fn finish(app: &mut SolveApp) {
    let Some(Tool::Draw(draw)) = &mut app.electronics.board.tool else { return };
    let (shape, points) = (draw.shape, std::mem::take(&mut draw.points));
    let o = app.electronics.board.draw.clone();
    let mut params = json!({"layer": o.layer.name()});
    match (shape, points.as_slice()) {
        (DrawShape::Line, [_, _, ..]) => {
            params["points"] = json!(points);
            params["width"] = json!(o.width);
        }
        (DrawShape::Rect, [a, b]) if a[0] != b[0] && a[1] != b[1] => {
            params["from"] = json!(a);
            params["to"] = json!(b);
            params["width"] = json!(o.width);
        }
        (DrawShape::Circle, [c, r]) => {
            let radius = crate::electronics_board::snap_tidy([(r[0] - c[0]).hypot(r[1] - c[1]), 0.0])[0];
            if radius <= 0.0 {
                return;
            }
            params["center"] = json!(c);
            params["radius"] = json!(radius);
            params["width"] = json!(o.width);
        }
        (DrawShape::Text, [at, ..]) => {
            if o.text.trim().is_empty() {
                app.set_status(tr("electronics.board_text_first"), true);
                return;
            }
            params["at"] = json!(at);
            params["text"] = json!(o.text);
            params["height"] = json!(o.height);
        }
        _ => return,
    }
    drop(app.run(shape.command(), params));
}

/// Backspace or Escape with a DRAW tool: take back the last click. `false` with none.
pub(crate) fn step_back(app: &mut SolveApp) -> bool {
    match &mut app.electronics.board.tool {
        Some(Tool::Draw(draw)) => draw.points.pop().is_some(),
        _ => false,
    }
}

/// What the Hole or DRAW tool would make with the pointer at `hover`.
pub(crate) fn paint_tool(app: &SolveApp, painter: &egui::Painter, map: PageMap, hover: Pos2, free: bool, t: &Tokens) {
    let s = &app.electronics.board;
    let at = snap(app, map, hover, free);
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    let ghost = t.sel_edge.gamma_multiply(0.7);
    match &s.tool {
        Some(Tool::Hole) => {
            let r = ((s.draw.drill / 2.0 * map.scale()) as f32).max(3.0);
            painter.circle_stroke(screen(at), r, Stroke::new(1.5, ghost));
        }
        Some(Tool::Draw(draw)) => {
            let colour = draw_colour(t, s.draw.layer).gamma_multiply(0.85);
            let w = ((s.draw.width * map.scale()) as f32).clamp(1.0, 64.0);
            painter.circle_stroke(screen(at), 4.0, Stroke::new(1.5, ghost));
            match (draw.shape, draw.points.as_slice()) {
                (DrawShape::Line, [_, ..]) => {
                    let line: Vec<Pos2> = draw.points.iter().chain([&at]).map(|q| screen(*q)).collect();
                    painter.add(egui::Shape::line(line, Stroke::new(w, colour)));
                }
                (DrawShape::Rect, [a]) => {
                    painter.rect_stroke(Rect::from_two_pos(screen(*a), screen(at)), 0.0, Stroke::new(w, colour), egui::StrokeKind::Middle);
                }
                (DrawShape::Circle, [c]) => {
                    let r = ((c[0] - at[0]).hypot(c[1] - at[1]) * map.scale()) as f32;
                    if r.is_finite() {
                        painter.circle_stroke(screen(*c), r, Stroke::new(w, colour));
                    }
                }
                (DrawShape::Text, []) => {
                    let size = (s.draw.height * map.scale()) as f32;
                    if size >= MIN_TEXT_POINTS && !s.draw.text.is_empty() {
                        let mirror = if s.draw.layer.is_bottom() { Mirror::AboutStart } else { Mirror::No };
                        paint_glyphs(painter, screen(at), &s.draw.text, FontId::proportional(size.min(MAX_TEXT_POINTS)), colour, mirror);
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

/// The DRAW tools' options: the layer, a line's width, a text's words and height.
pub(crate) fn options(app: &mut SolveApp, ui: &mut Ui, shape: DrawShape) -> &'static str {
    let layer = app.electronics.board.draw.layer;
    let mut chosen = None;
    ui_kit::row(ui, crate::i18n::tr("electronics.field_layer"), |ui| chosen = layer_menu(app, ui, "electronics_board_draw_layer", layer));
    if let Some(l) = chosen {
        app.electronics.board.draw.layer = l;
    }
    let o = app.electronics.board.draw.clone();
    if shape == DrawShape::Text {
        if let Some(text) = crate::electronics_editor::field(app, ui, "board:draw:text", &tr("electronics.field_text"), &o.text) {
            app.electronics.board.draw.text = text;
        }
        if let Some(h) = crate::electronics_editor::number(app, ui, "board:draw:height", &tr("electronics.field_text_height"), o.height) {
            if h.is_finite() && h > 0.0 && h <= 100.0 {
                app.electronics.board.draw.height = h;
            } else {
                app.set_status(tr("electronics.board_bad_text_height"), true);
            }
        }
    } else if let Some(w) = crate::electronics_editor::number(app, ui, "board:draw:width", &tr("electronics.field_line_width"), o.width) {
        if w.is_finite() && w > 0.0 && w <= solidcraft_engine::doc::ecad::graphics::MAX_WIDTH {
            app.electronics.board.draw.width = w;
        } else {
            app.set_status(tr("electronics.board_bad_line_width"), true);
        }
    }
    match shape {
        DrawShape::Line => "electronics.board_draw_line_help",
        DrawShape::Rect => "electronics.board_draw_rect_help",
        DrawShape::Circle => "electronics.board_draw_circle_help",
        DrawShape::Text => "electronics.board_draw_text_help",
    }
}

/// The Hole tool's options: its drill.
pub(crate) fn hole_options(app: &mut SolveApp, ui: &mut Ui) {
    let drill = app.electronics.board.draw.drill;
    if let Some(d) = crate::electronics_editor::number(app, ui, "board:hole:drill", &tr("electronics.field_drill"), drill) {
        if check_hole(d).is_ok() {
            app.electronics.board.draw.drill = d;
        } else {
            app.set_status(tr("electronics.board_bad_hole"), true);
        }
    }
}

/// The picked mounting hole: where it is and its drill.
pub(crate) fn hole_section(app: &mut SolveApp, ui: &mut Ui, id: u64) {
    let Some(hole) = board(app).hole(id).cloned() else { return };
    let title = app.tr_label("Hole").to_string();
    ui_kit::section(ui, ("electronics_board_hole", id), &title, true, |ui| {
        if let Some(to) = crate::electronics_editor::pair(app, ui, &format!("board:{id}:at"), crate::i18n::tr("electronics.field_position"), hole.at)
        {
            drop(app.run("ecad.board.move", json!({"items": [id], "to": to})));
        }
        if let Some(d) = crate::electronics_editor::number(app, ui, &format!("board:{id}:drill"), &tr("electronics.field_drill"), hole.drill) {
            drop(app.run("ecad.board.hole.edit", json!({"holes": [id], "drill": d})));
        }
    });
}

/// The picked drawing: its layer, where it is, a text's words and height or a shape's line width.
pub(crate) fn graphic_section(app: &mut SolveApp, ui: &mut Ui, id: u64) {
    let Some(g) = board(app).graphic(id).cloned() else { return };
    let title = match g.graphic.kind {
        GraphicType::Text => tr("electronics.board_drawing_text"),
        _ => tr("electronics.board_drawing"),
    };
    let edit = |app: &mut SolveApp, mut params: serde_json::Value| {
        params["graphic"] = json!(id);
        drop(app.run("ecad.board.draw.edit", params));
    };
    ui_kit::section(ui, ("electronics_board_graphic", id), &title, true, |ui| {
        let mut chosen = None;
        ui_kit::row(ui, crate::i18n::tr("electronics.field_layer"), |ui| chosen = layer_menu(app, ui, &format!("board:{id}:layer"), g.layer));
        if let Some(l) = chosen {
            edit(app, json!({"layer": l.name()}));
        }
        if let Some(at) = g.position()
            && let Some(to) = crate::electronics_editor::pair(app, ui, &format!("board:{id}:at"), crate::i18n::tr("electronics.field_position"), at)
        {
            drop(app.run("ecad.board.move", json!({"items": [id], "to": to})));
        }
        if g.graphic.kind == GraphicType::Text {
            if let Some(text) = crate::electronics_editor::field(app, ui, &format!("board:{id}:text"), &tr("electronics.field_text"), &g.graphic.text)
            {
                edit(app, json!({"text": text}));
            }
            let height = g.graphic.height.unwrap_or(TEXT_HEIGHT);
            if let Some(h) = crate::electronics_editor::number(app, ui, &format!("board:{id}:height"), &tr("electronics.field_text_height"), height) {
                edit(app, json!({"height": h}));
            }
            if g.mirror {
                ui_kit::text(ui, &tr("electronics.board_text_mirrored"), ui_kit::Text::Note("electronics.board_text_mirrored"));
            }
        } else if let Some(w) =
            crate::electronics_editor::number(app, ui, &format!("board:{id}:width"), &tr("electronics.field_line_width"), g.graphic.width)
        {
            edit(app, json!({"width": w}));
        }
    });
}

/// One of a footprint's two texts on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Which {
    Name,
    Value,
}

impl Which {
    /// The footprint text that stands for it.
    fn placeholder(self) -> &'static str {
        match self {
            Which::Name => "{designator}",
            Which::Value => "{value}",
        }
    }

    /// Its key in `ecad.board.edit`.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Which::Name => "name_at",
            Which::Value => "value_at",
        }
    }
}

/// A footprint's name or value text as the board shows it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FootprintText {
    pub footprint: u64,
    pub which: Which,
    /// Its lower left.
    pub at: [f64; 2],
    pub height: f64,
    pub text: String,
    /// The silkscreen it is on: its footprint's side.
    pub side: Side,
}

/// Whether a footprint graphic is the placeholder for its name or value (drawn where the
/// footprint keeps that text instead).
pub(crate) fn is_placeholder(g: &solidcraft_engine::doc::ecad::Graphic) -> bool {
    g.kind == GraphicType::Text && [Which::Name, Which::Value].iter().any(|w| g.text == w.placeholder())
}

/// The box round a footprint's courtyard on the board (round all of it without one).
pub(crate) fn courtyard_box(app: &SolveApp, f: &BoardFootprint) -> Option<[[f64; 2]; 2]> {
    let placed = app.session.doc.ecad.placed(f)?;
    let fp = placed.resolved.footprint;
    let local = shapes_bbox(fp.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Courtyard))).or_else(|| footprint_box(fp))?;
    Some(f.placement().bbox(local))
}

/// The name and value texts the footprints of `b` show, as high as the board says, else as the
/// footprint's placeholder for them says, else in step with the footprint (see `ecad::names`): at
/// their own spot, else where the placeholder is, else the name just outside the courtyard on the
/// side with the most room and the value across from it (a value whose name has a spot of its own
/// takes the side with the most room itself). The stored board's are kept by [`stored_texts`].
pub(crate) fn footprint_texts(app: &SolveApp, b: &Board) -> Vec<FootprintText> {
    let ecad = &app.session.doc.ecad;
    // Each text, with its spot when it has one; then those without one go round their courtyard.
    let mut out: Vec<(FootprintText, Option<usize>)> = Vec::new();
    let mut boxes: Vec<([[f64; 2]; 2], Side)> = Vec::new();
    // The text each box's room is looked for (its name, else its value), and the value that goes
    // across from its name, of those placed round it.
    let mut sizes: Vec<[f64; 2]> = Vec::new();
    let mut across: Vec<[f64; 2]> = Vec::new();
    let mut unplaced: Vec<bool> = Vec::new();
    for f in &b.footprints {
        let Some(placed) = ecad.placed(f) else { continue };
        let placement = f.placement();
        let side = if placement.mirror { Side::Bottom } else { Side::Top };
        let court = courtyard_box(app, f);
        let k = court.map(|c| {
            boxes.push((c, side));
            sizes.push([0.0; 2]);
            across.push([0.0; 2]);
            boxes.len() - 1
        });
        for (which, shown, own) in [(Which::Name, f.show_name, f.name_at), (Which::Value, f.show_value, f.value_at)] {
            if !shown {
                continue;
            }
            let text = match which {
                Which::Name => placed.part.designator.clone(),
                Which::Value => placed.part.value.clone().unwrap_or_else(|| placed.resolved.component.value.clone()),
            };
            if text.is_empty() {
                continue;
            }
            let placeholder = placed
                .resolved
                .footprint
                .graphics
                .iter()
                .find(|g| g.layer == Some(BoardLayer::Silkscreen) && g.kind == GraphicType::Text && g.text == which.placeholder());
            let height =
                b.name_height.or_else(|| placeholder.and_then(|g| g.height)).unwrap_or_else(|| court.map_or(TEXT_HEIGHT, names::auto_height));
            let spot = match (own, placeholder.and_then(|g| placement.graphic(g).at)) {
                (Some(o), _) => Some([f.at[0] + o[0], f.at[1] + o[1]]),
                (None, at) => at,
            };
            // Of the texts placed round the courtyard, its room is looked for the name (the value
            // without one), and the value goes across from the name.
            if spot.is_none()
                && let Some(k) = k
            {
                let size = [names::text_width(height, text.chars().count()), height];
                match sizes.get_mut(k) {
                    Some(main) if main[1] == 0.0 => *main = size,
                    Some(_) => {
                        if let Some(a) = across.get_mut(k) {
                            *a = size;
                        }
                    }
                    None => {}
                }
            }
            unplaced.push(spot.is_none());
            out.push((FootprintText { footprint: f.id, which, at: spot.unwrap_or(f.at), height, text, side }, k));
        }
    }
    if unplaced.iter().any(|u| *u) {
        let around = names::sides(&boxes, &sizes, &across);
        for ((text, k), free) in out.iter_mut().zip(unplaced) {
            if let (true, Some(k)) = (free, *k)
                && let (Some((court, _)), Some(a)) = (boxes.get(k), around.get(k))
            {
                let goes_across = across.get(k).is_some_and(|s| s[1] > 0.0);
                let a = if text.which == Which::Value && goes_across { a.opposite() } else { *a };
                text.at = names::spot(*court, a, [names::text_width(text.height, text.text.chars().count()), text.height]);
            }
        }
    }
    out.into_iter().map(|(text, _)| text).collect()
}

/// The stored board's footprint texts (see [`footprint_texts`]), as the board view's refresh laid
/// them out for this revision, else laid out now.
pub(crate) fn stored_texts(app: &SolveApp) -> Arc<Vec<FootprintText>> {
    match crate::electronics_board::texts_for(app, (app.session.uid, app.session.revision)) {
        Some(texts) => texts,
        None => Arc::new(footprint_texts(app, board(app))),
    }
}

/// Whether the Layers tab shows `text`: its silkscreen, and names or values.
pub(crate) fn text_shown(app: &SolveApp, text: &FootprintText) -> bool {
    let hidden = &app.electronics.board.hidden;
    let silkscreen = if text.side == Side::Bottom { Layer::BottomSilkscreen } else { Layer::TopSilkscreen };
    let kind = if text.which == Which::Name { Layer::Names } else { Layer::Values };
    !hidden.contains(&silkscreen) && !hidden.contains(&kind)
}

/// The screen box of a text from its lower left `at`.
fn text_rect(map: PageMap, at: [f64; 2], height: f64, text: &str) -> Rect {
    let h = (height * map.scale()) as f32;
    let p = map.to_screen(at[0], at[1]);
    Rect::from_min_size(p - vec2(0.0, h), vec2(h * 0.6 * text.chars().count().max(1) as f32, h))
}

/// The shown footprint text under `p` (one too small to be drawn is not): its footprint, which
/// text, and where it starts.
pub(crate) fn text_at(app: &SolveApp, map: PageMap, p: Pos2) -> Option<(u64, Which, [f64; 2])> {
    stored_texts(app)
        .iter()
        .rev()
        .filter(|t| text_shown(app, t) && (t.height * map.scale()) as f32 >= MIN_TEXT_POINTS)
        .find(|t| text_rect(map, t.at, t.height, &t.text).expand(2.0).contains(p))
        .map(|t| (t.footprint, t.which, t.at))
}

/// Draw a footprint's text in `colour`, unless it is too small to read; on the bottom, mirrored in
/// its box.
pub(crate) fn paint_text(painter: &egui::Painter, map: PageMap, text: &FootprintText, colour: Color32) {
    let size = (text.height * map.scale()) as f32;
    let p = map.to_screen(text.at[0], text.at[1]);
    if size >= MIN_TEXT_POINTS && p.x.is_finite() && p.y.is_finite() {
        let mirror = if text.side == Side::Bottom { Mirror::InPlace } else { Mirror::No };
        paint_glyphs(painter, p, &text.text, FontId::proportional(size.min(MAX_TEXT_POINTS)), colour, mirror);
    }
}
