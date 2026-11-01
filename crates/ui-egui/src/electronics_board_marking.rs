//! The Board Marking tool (see `electronics_board`, the board's Place group): a click stamps the
//! board's identity on the silkscreen of the active layer's side as `ecad.board.marking` lays it
//! out (`solidcraft_doc::ecad::marking`), its top left where the click is. What the block holds is
//! the app's template (the preferences' `board_marking`, the same for every design), edited in
//! the tool's options; the part number's `{number}` and the copper layer marks are this board's.

use egui::{Pos2, Ui};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::CopperLayer;

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
}

/// A click: the marking, its top left at the click, on the active layer's side.
pub(crate) fn click(app: &mut SolveApp, map: PageMap, p: Pos2, free: bool) {
    let at = snap(app, map, p, free);
    let side = if active_layer(app) == CopperLayer::Bottom { "bottom" } else { "top" };
    let template = serde_json::to_value(&app.preferences.board_marking).unwrap_or(Value::Null);
    let o = app.electronics.board.marking.clone();
    let mut params = json!({"side": side, "at": at, "template": template, "layer_marks": o.layer_marks});
    if !o.number.is_empty() {
        params["number"] = json!(o.number);
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
