//! The Electronics workspace's electronic components: the Browser card lists them, and the page
//! draws the chosen one's symbol or footprint, with the Component panel's pick (`electronics_editor`).
//! Every change is an `ecad.*` command, and the page follows the electronic component the last one
//! named, so an agent's work shows as it lands.

use egui::{Align2, FontId, Pos2, Rect, Stroke, pos2, vec2};
use serde_json::Value;
use solidcraft_engine::doc::ecad::{BoardLayer, ElectronicComponent, Graphic, GraphicType, PadKind, Side};

use crate::SolveApp;
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;

/// What the page shows of the chosen electronic component.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Symbol,
    Footprint,
}

impl View {
    fn key(self) -> &'static str {
        match self {
            View::Symbol => "symbol",
            View::Footprint => "footprint",
        }
    }
}

/// Text on the page is drawn only while a millimetre is at least this many points.
const MIN_LABEL_SCALE: f64 = 4.0;
/// Graphic text is drawn only between these sizes on screen, in points.
const TEXT_POINTS: std::ops::RangeInclusive<f32> = 4.0..=256.0;

/// The electronic component the page shows: the chosen one, else the newest.
pub(crate) fn shown(app: &SolveApp) -> Option<&ElectronicComponent> {
    let ecad = &app.session.doc.ecad;
    app.electronics.selected.and_then(|id| ecad.component(id)).or_else(|| ecad.components.last())
}

/// After an `ecad.*` command: show the electronic component its reply names, in the view it
/// changed.
pub(crate) fn follow(app: &mut SolveApp, id: &str, reply: &Value) {
    if !id.starts_with("ecad.") {
        return;
    }
    if let Some(component) = reply.get("component").and_then(|c| c.get("id")).and_then(Value::as_u64) {
        app.electronics.selected = Some(component);
    }
    if id.starts_with("ecad.pin.") || id == "ecad.symbol.generate" {
        app.electronics.view = View::Symbol;
    } else if id.starts_with("ecad.pad.") || id == "ecad.footprint.generate" {
        app.electronics.view = View::Footprint;
    }
    // A generated drawing is new: frame it.
    if id.ends_with(".generate") {
        app.electronics.page.fitted = false;
    }
}

/// What Fit frames: the shown view's drawing, `[[x0, y0], [x1, y1]]` in page millimetres.
pub(crate) fn bounds(app: &SolveApp) -> Option<[[f64; 2]; 2]> {
    let c = shown(app)?;
    match app.electronics.view {
        View::Symbol => c.symbol.bbox(),
        View::Footprint => c.footprint.copper_bbox(),
    }
}

/// What the camera framed last: the electronic component, the view, and whether it had anything
/// in it (a first pin or pad is framed afresh).
pub(crate) fn framing(app: &SolveApp) -> Option<(u64, View, bool)> {
    shown(app).map(|c| (c.id, app.electronics.view, bounds(app).is_some()))
}

/// Draw the shown view over the page's grid.
pub(crate) fn paint(app: &SolveApp, painter: &egui::Painter, map: PageMap) {
    let Some(c) = shown(app) else { return };
    let t = Tokens::get();
    let view = Tokens::view();
    let labels = map.scale() >= MIN_LABEL_SCALE;
    let font = FontId::proportional(view.font_small);
    let on_page = |p: Pos2| p.x.is_finite() && p.y.is_finite();
    match app.electronics.view {
        View::Symbol => {
            for g in &c.symbol.graphics {
                graphic(painter, map, g, view.text, &font);
            }
            for pin in &c.symbol.pins {
                // A pin being dragged is drawn where it is going.
                let [dx, dy] = crate::electronics_editor::moved_by(app, true, &pin.number);
                let a = map.to_screen(pin.at[0] + dx, pin.at[1] + dy);
                let b = map.to_screen(pin.body_end()[0] + dx, pin.body_end()[1] + dy);
                if !(on_page(a) && on_page(b)) {
                    continue;
                }
                if crate::electronics_editor::is_picked(app, true, &pin.number) {
                    painter.line_segment([a, b], Stroke::new(6.0, t.sel_edge_rim));
                    painter.circle_filled(a, 5.0, t.sel_edge);
                }
                painter.line_segment([a, b], Stroke::new(2.0, view.text));
                painter.circle_stroke(a, 3.0, Stroke::new(1.4, t.accent));
                crate::scenario::publish_handle(&format!("electronics:pin:{}", pin.number), a);
                if labels {
                    let middle = pos2((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
                    painter.text(middle - vec2(0.0, 3.0), Align2::CENTER_BOTTOM, &pin.number, font.clone(), view.text_dim);
                    if !pin.name.is_empty() {
                        // The name sits just inside the body, past the pin's inner end.
                        let [dx, dy] = pin.direction.unit();
                        let inside = b + vec2(dx as f32, -dy as f32) * 4.0;
                        let align = match pin.direction {
                            solidcraft_engine::doc::ecad::PinDirection::Right => Align2::LEFT_CENTER,
                            solidcraft_engine::doc::ecad::PinDirection::Left => Align2::RIGHT_CENTER,
                            solidcraft_engine::doc::ecad::PinDirection::Up => Align2::CENTER_BOTTOM,
                            solidcraft_engine::doc::ecad::PinDirection::Down => Align2::CENTER_TOP,
                        };
                        painter.text(inside, align, &pin.name, font.clone(), view.text);
                    }
                }
            }
        }
        View::Footprint => {
            // Under the copper: the courtyard and the fabrication outline; over it: silkscreen.
            for (layer, colour) in [(BoardLayer::Courtyard, t.grid_major), (BoardLayer::Fab, view.text_dim)] {
                for side in [Side::Bottom, Side::Top] {
                    for g in c.footprint.layer(layer, side) {
                        graphic(painter, map, g, colour, &font);
                    }
                }
            }
            for pad in &c.footprint.pads {
                let [dx, dy] = crate::electronics_editor::moved_by(app, false, &pad.number);
                let screen = |outline: Vec<[f64; 2]>| -> Vec<Pos2> { outline.iter().map(|p| map.to_screen(p[0] + dx, p[1] + dy)).collect() };
                let points = screen(pad.outline(6));
                if !points.iter().copied().all(on_page) {
                    continue;
                }
                let colour = match (pad.kind, pad.side) {
                    (PadKind::Hole, _) => view.text_dim,
                    (PadKind::Smd, Side::Bottom) => t.warning,
                    _ => t.accent,
                };
                let fill = if pad.kind == PadKind::Hole { egui::Color32::TRANSPARENT } else { colour.gamma_multiply(0.55) };
                let picked = crate::electronics_editor::is_picked(app, false, &pad.number);
                if picked {
                    painter.add(egui::Shape::closed_line(points.clone(), Stroke::new(5.0, t.sel_edge_rim)));
                }
                painter.add(egui::Shape::convex_polygon(
                    points,
                    fill,
                    Stroke::new(if picked { 2.0 } else { 1.2 }, if picked { t.sel_edge } else { colour }),
                ));
                if let Some(hole) = pad.drill_outline(6).map(screen).filter(|h| h.iter().copied().all(on_page)) {
                    painter.add(egui::Shape::convex_polygon(hole, t.viewport_bottom, Stroke::new(1.0, colour)));
                }
                let centre = map.to_screen(pad.at[0] + dx, pad.at[1] + dy);
                crate::scenario::publish_handle(&format!("electronics:pad:{}", pad.number), centre);
                if labels {
                    painter.text(centre, Align2::CENTER_CENTER, &pad.number, font.clone(), view.text);
                }
            }
            for side in [Side::Bottom, Side::Top] {
                for g in c.footprint.layer(BoardLayer::Silkscreen, side) {
                    graphic(painter, map, g, view.text, &font);
                }
            }
        }
    }
}

/// One graphic, its line width to scale.
fn graphic(painter: &egui::Painter, map: PageMap, g: &Graphic, colour: egui::Color32, font: &FontId) {
    let screen = |points: &[[f64; 2]]| -> Option<Vec<Pos2>> {
        let out: Vec<Pos2> = points.iter().map(|p| map.to_screen(p[0], p[1])).collect();
        out.iter().all(|p| p.x.is_finite() && p.y.is_finite()).then_some(out)
    };
    let width = ((g.width * map.scale()) as f32).clamp(1.0, 64.0);
    let stroke = Stroke::new(width, colour);
    let fill = if g.fill { colour.gamma_multiply(0.35) } else { egui::Color32::TRANSPARENT };
    match g.kind {
        GraphicType::Line => {
            if let Some(points) = screen(&g.points) {
                painter.add(egui::Shape::line(points, stroke));
            }
        }
        GraphicType::Polygon => {
            if let Some(points) = screen(&g.points) {
                painter.add(egui::Shape::closed_line(points.clone(), stroke));
                if g.fill && points.len() >= 3 {
                    painter.add(egui::Shape::convex_polygon(points, fill, Stroke::NONE));
                }
            }
        }
        GraphicType::Rect => {
            let (Some(a), Some(b)) = (g.from, g.to) else { return };
            if let Some(points) = screen(&[a, [b[0], a[1]], b, [a[0], b[1]]]) {
                painter.add(egui::Shape::convex_polygon(points, fill, stroke));
            }
        }
        GraphicType::Circle => {
            let (Some(c), Some(r)) = (g.center, g.radius) else { return };
            let centre = map.to_screen(c[0], c[1]);
            let r = (r * map.scale()) as f32;
            if centre.x.is_finite() && centre.y.is_finite() && r.is_finite() {
                painter.add(egui::Shape::circle_filled(centre, r, fill));
                painter.circle_stroke(centre, r, stroke);
            }
        }
        GraphicType::Arc => {
            if let Some(points) = screen(&g.arc_points(48)) {
                painter.add(egui::Shape::line(points, stroke));
            }
        }
        GraphicType::Text => {
            let (Some(at), Some(h)) = (g.at, g.height) else { return };
            let size = (h * map.scale()) as f32;
            if !TEXT_POINTS.contains(&size) {
                return;
            }
            let p = map.to_screen(at[0], at[1]);
            if p.x.is_finite() && p.y.is_finite() {
                // Upright on screen; a turned text keeps its anchor.
                painter.text(p, Align2::LEFT_BOTTOM, &g.text, FontId::new(size, font.family.clone()), colour);
            }
        }
    }
}

/// The note for a shown view with nothing in it yet, if any.
pub(crate) fn empty_note(app: &SolveApp) -> Option<String> {
    let c = shown(app)?;
    match app.electronics.view {
        View::Symbol if c.symbol.is_empty() => Some(crate::i18n::format("electronics.no_pins", &[("name", &c.name)])),
        View::Footprint if c.footprint.is_empty() => Some(crate::i18n::format("electronics.no_pads", &[("name", &c.name)])),
        _ => None,
    }
}

/// Symbol and Footprint, over the top of the page's free area.
pub(crate) fn view_switch(app: &mut SolveApp, ui: &mut egui::Ui, content: Rect) {
    if shown(app).is_none() {
        return;
    }
    let t = Tokens::get();
    let width = (t.form_card_width * 0.9).min(content.width() - 2.0 * t.space_16);
    if width < t.control_height * 3.0 {
        return;
    }
    let rect = Rect::from_min_size(pos2(content.center().x - width / 2.0, content.top() + t.space_12), vec2(width, crate::ui_kit::row_height()));
    let labels = [app.tr("electronics.view_symbol").to_string(), app.tr("electronics.view_footprint").to_string()];
    let mut selected = usize::from(app.electronics.view == View::Footprint);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("electronics_view"));
    crate::ui_kit::segmented(&mut child, &mut selected, &[labels[0].as_str(), labels[1].as_str()], Default::default());
    for (i, view) in [View::Symbol, View::Footprint].into_iter().enumerate() {
        let half = Rect::from_min_size(rect.min + vec2(width * i as f32 / 2.0, 0.0), vec2(width / 2.0, rect.height()));
        crate::scenario::publish_target(&format!("electronics:view:{}", view.key()), half);
    }
    app.electronics.view = if selected == 1 { View::Footprint } else { View::Symbol };
}

/// The Browser card's content in the Electronics workspace: the model as in Design, or the
/// electronic components. `true` when it drew the components (the model rows are then left out).
pub(crate) fn browser_mode(app: &mut SolveApp, ui: &mut egui::Ui) -> bool {
    if !crate::electronics_workspace::active(app) {
        return false;
    }
    let labels = [app.tr("electronics.browser_model").to_string(), app.tr("electronics.browser_components").to_string()];
    let mut selected = usize::from(!app.electronics.model_tab);
    let top = ui.cursor().min;
    let width = ui.available_width();
    crate::ui_kit::segmented(ui, &mut selected, &[labels[0].as_str(), labels[1].as_str()], Default::default());
    for (i, name) in ["electronics:browser-model", "electronics:browser-components"].iter().enumerate() {
        crate::scenario::publish_target(
            name,
            Rect::from_min_size(top + vec2(width * i as f32 / 2.0, 0.0), vec2(width / 2.0, crate::ui_kit::row_height())),
        );
    }
    app.electronics.model_tab = selected == 0;
    if app.electronics.model_tab {
        return false;
    }
    crate::ui_kit::scope(ui, |ui| list(app, ui));
    true
}

/// One row per electronic component; a click shows it on the page.
fn list(app: &mut SolveApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    ui.add_space(t.space_6);
    if app.session.doc.ecad.components.is_empty() {
        crate::ui_kit::text(ui, app.tr("electronics.no_components"), crate::ui_kit::Text::Note("electronics.no_components"));
        return;
    }
    let shown = shown(app).map(|c| c.id);
    let rows: Vec<(u64, String, String)> = app
        .session
        .doc
        .ecad
        .components
        .iter()
        .map(|c| {
            let title = if c.value.is_empty() { c.name.clone() } else { format!("{} · {}", c.name, c.value) };
            let counts = crate::i18n::format(
                "electronics.counts",
                &[("pins", &c.symbol.pins.len().to_string()), ("pads", &c.footprint.pads.len().to_string())],
            );
            (c.id, title, counts)
        })
        .collect();
    let names: Vec<String> = app.session.doc.ecad.components.iter().map(|c| c.name.clone()).collect();
    for ((id, title, counts), name) in rows.into_iter().zip(names) {
        let response = crate::ui_kit::text(ui, &title, crate::ui_kit::Text::NameCell(shown == Some(id)));
        crate::scenario::publish_target(&format!("electronics:component:{name}"), response.rect);
        crate::ui_kit::text(ui, &counts, crate::ui_kit::Text::Note("electronics.counts"));
        if response.clicked() {
            app.electronics.selected = Some(id);
        }
    }
}
