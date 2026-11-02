//! The Board Marking tool (see `electronics_board`, the board's Place group): a click stamps the
//! board's identity on the silkscreen of the active layer's side as `ecad.board.marking` lays it
//! out (`solidcraft_doc::ecad::marking`), its top left where the click is. What the block holds is
//! the app's template (the preferences' `board_marking`, the same for every design), edited in
//! the tool's options; the part number's `{number}` and the copper layer marks are this board's.
//! While the tool is armed the block is laid out once (again only when the design, the template
//! or the options change) and its ghost, the very text and shapes a click stamps, follows the
//! pointer; the click commits it.

use egui::{Color32, Pos2, Rect, Stroke, Ui};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::CopperLayer;
use solidcraft_engine::doc::ecad::board::BoardGraphic;
use solidcraft_engine::doc::ecad::footprint::Side;
use solidcraft_engine::doc::ecad::graphics::Graphic;
use solidcraft_engine::doc::ecad::marking::{self, Marking};

use crate::SolveApp;
use crate::electronics_board::{active_layer, snap};
use crate::electronics_editor::tr;
use crate::electronics_workspace::PageMap;
use crate::ui_kit;

/// The tool's own options for this board.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarkingOptions {
    /// The part number's `{number}`.
    pub number: String,
    pub layer_marks: bool,
    /// The block laid out with its top left at the origin, and what it was laid out for.
    pub(crate) ghost: Option<(String, Vec<BoardGraphic>)>,
}

/// The side the active layer is on.
fn side(app: &SolveApp) -> Side {
    if active_layer(app) == CopperLayer::Bottom { Side::Bottom } else { Side::Top }
}

/// Lay the block out again when the design, the template or the options changed (looked at each
/// frame the board view draws while the tool is armed); nothing otherwise.
pub(crate) fn refresh(app: &mut SolveApp) {
    if !matches!(app.electronics.board.tool, Some(crate::electronics_board::Tool::Marking)) {
        app.electronics.board.marking.ghost = None;
        return;
    }
    let o = &app.electronics.board.marking;
    let template = app.preferences.board_marking.clone();
    let side = side(app);
    let key = format!(
        "{}/{}/{side:?}/{}/{}/{}",
        app.session.uid,
        app.session.revision,
        serde_json::to_string(&template).unwrap_or_default(),
        o.number,
        o.layer_marks
    );
    if o.ghost.as_ref().is_some_and(|(k, _)| *k == key) {
        return;
    }
    let ecad = &app.session.doc.ecad;
    let fields = solidcraft_engine::cmd::marking_fields(&app.session, ecad);
    let m = Marking { id: 0, side, at: [0.0, 0.0], number: o.number.clone(), template, layer_marks: o.layer_marks };
    let (graphics, _) = marking::shapes(&m, &fields, ecad.board.copper_layers);
    app.electronics.board.marking.ghost = Some((key, graphics));
}

/// `g` moved by `d`.
fn moved(g: &Graphic, d: [f64; 2]) -> Graphic {
    let by = |p: [f64; 2]| [p[0] + d[0], p[1] + d[1]];
    Graphic {
        points: g.points.iter().copied().map(by).collect(),
        from: g.from.map(by),
        to: g.to.map(by),
        center: g.center.map(by),
        at: g.at.map(by),
        ..g.clone()
    }
}

/// The block's ghost with its top left at `at`: each shape in its own layer's colour, a little
/// faded, and a box in the ghost colour round it all.
pub(crate) fn paint(app: &SolveApp, painter: &egui::Painter, map: PageMap, at: [f64; 2], ghost: Color32, t: &crate::theme::Tokens) {
    let Some((_, graphics)) = &app.electronics.board.marking.ghost else { return };
    let font = egui::FontId::proportional(crate::theme::Tokens::view().font_small);
    let mut b: Option<[[f64; 2]; 2]> = None;
    for g in graphics {
        let shape = moved(&g.graphic, at);
        let mirror = if g.mirror { crate::electronics_board_draw::Mirror::AboutStart } else { crate::electronics_board_draw::Mirror::No };
        let ink = crate::electronics_board_draw::draw_colour(t, g.layer).gamma_multiply(0.8);
        crate::electronics_board_draw::paint_shape(painter, map, &shape, ink, &font, mirror);
        if let Some([lo, hi]) = shape.bbox() {
            b = Some(b.map_or([lo, hi], |[a, c]| [[a[0].min(lo[0]), a[1].min(lo[1])], [c[0].max(hi[0]), c[1].max(hi[1])]]));
        }
    }
    if let Some([lo, hi]) = b {
        let r = Rect::from_two_pos(map.to_screen(lo[0], lo[1]), map.to_screen(hi[0], hi[1])).expand(3.0);
        if r.is_finite() {
            painter.rect_stroke(r, 2.0, Stroke::new(1.0, ghost.gamma_multiply(0.6)), egui::StrokeKind::Outside);
        }
    }
}

/// A click: the marking, its top left at the click, on the active layer's side.
pub(crate) fn click(app: &mut SolveApp, map: PageMap, p: Pos2, free: bool) {
    let at = snap(app, map, p, free);
    let side = if side(app) == Side::Bottom { "bottom" } else { "top" };
    let template = serde_json::to_value(&app.preferences.board_marking).unwrap_or(Value::Null);
    let o = &app.electronics.board.marking;
    let mut params = json!({"side": side, "at": at, "template": template, "layer_marks": o.layer_marks});
    if !o.number.is_empty() {
        params["number"] = json!(o.number.clone());
    }
    match app.run("ecad.board.marking", params) {
        Ok(r) => {
            let warnings = r.get("warnings").and_then(Value::as_array).map_or(0, Vec::len);
            let key = if warnings > 0 { "electronics.marking_placed_warnings" } else { "electronics.marking_placed" };
            let text = crate::i18n::format(key, &[("n", &warnings.to_string())]);
            app.set_status(text, false);
        }
        Err(e) => app.set_status(e, true),
    }
}

/// The tool's options: the template's parts (saved in the preferences) and this board's number
/// and layer marks; the help's key.
pub(crate) fn options(app: &mut SolveApp, ui: &mut Ui) -> &'static str {
    ui_kit::text(ui, crate::i18n::tr("electronics.marking_template_note"), ui_kit::Text::Note("electronics.marking_template_note"));
    let t = app.preferences.board_marking.clone();
    let mut next = t.clone();
    for (key, label, on) in [
        ("marking:title", "electronics.marking_title", t.title),
        ("marking:revision", "electronics.marking_revision", t.revision),
        ("marking:date", "electronics.marking_date", t.date),
        ("marking:serial", "electronics.marking_serial_box", t.serial_box),
        ("marking:qr", "electronics.marking_qr", t.qr),
    ] {
        if ui_kit::checkbox_row(ui, key, &tr(label), on) {
            match key {
                "marking:title" => next.title = !on,
                "marking:revision" => next.revision = !on,
                "marking:date" => next.date = !on,
                "marking:serial" => next.serial_box = !on,
                _ => next.qr = !on,
            }
        }
    }
    if let Some(v) = crate::electronics_editor::field(app, ui, "marking:part_number", &tr("electronics.marking_part_number"), &t.part_number) {
        next.part_number = v;
    }
    ui_kit::text(ui, crate::i18n::tr("electronics.marking_part_number_note"), ui_kit::Text::Note("electronics.marking_part_number_note"));
    if let Some(v) = crate::electronics_editor::field(app, ui, "marking:made_by", &tr("electronics.marking_made_by"), &t.made_by) {
        next.made_by = v;
    }
    if let Some(v) = crate::electronics_editor::field(app, ui, "marking:order", &tr("electronics.marking_order"), &t.order_placeholder) {
        next.order_placeholder = v;
    }
    let shown = crate::electronics_editor::mm(t.height);
    if let Some(v) = crate::electronics_editor::field(app, ui, "marking:height", &tr("electronics.marking_height"), &shown) {
        match v.trim().parse::<f64>() {
            Ok(h) if h.is_finite() && (0.5..=5.0).contains(&h) => next.height = h,
            _ => app.set_status(tr("electronics.marking_bad_height"), true),
        }
    }
    if next != t {
        match next.validate() {
            Ok(()) => app.preferences.board_marking = next,
            Err(e) => app.set_status(e, true),
        }
    }
    let number = app.electronics.board.marking.number.clone();
    if let Some(v) = crate::electronics_editor::field(app, ui, "marking:number", &tr("electronics.marking_number"), &number) {
        app.electronics.board.marking.number = v;
    }
    let on = app.electronics.board.marking.layer_marks;
    if ui_kit::checkbox_row(ui, "marking:layer_marks", &tr("electronics.marking_layer_marks"), on) {
        app.electronics.board.marking.layer_marks = !on;
    }
    "electronics.marking_help"
}
