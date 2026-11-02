//! The board's Layer Stack window (see `electronics_board`): how many copper layers the board has
//! and how thick it is, its stack-up (each copper layer's thickness, each dielectric's kind,
//! thickness and εr, the total against the board's thickness) with each change one
//! `ecad.board.stack`, and an impedance calculator over that stack-up: a layer, a kind of line and
//! a target impedance give the width (or a width gives the impedance), which can become the Route
//! options' width or a new net class. The impedance is the published formulas' in
//! `ecad::impedance`, worked out here as the fields change.

use egui::{Align2, Ui};
use serde_json::json;
use solidcraft_engine::doc::ecad::impedance::{self, Impedance, ImpedanceError, LineKind};
use solidcraft_engine::doc::ecad::layers::{MAX_COPPER_LAYERS, MIN_COPPER_LAYERS};
use solidcraft_engine::doc::ecad::stackup::{THICKNESS_SLACK, total};
use solidcraft_engine::doc::ecad::{CopperLayer, StackKind, StackLayer};

use crate::SolveApp;
use crate::electronics_board::board;
use crate::electronics_board_layers::command_label;
use crate::electronics_board_route::{layer_name, span_row};
use crate::electronics_editor::{mm, tr};
use crate::ui_kit;

/// The impedance calculator's fields.
#[derive(Clone, Debug, PartialEq)]
pub struct ImpedanceState {
    pub layer: CopperLayer,
    pub kind: LineKind,
    /// Ω → width (`true`), or width → Ω.
    pub solve_width: bool,
    /// Ohms.
    pub target: f64,
    /// Millimetres: the width for width → Ω, and the gap of a coplanar line or a pair.
    pub width: f64,
    pub gap: f64,
    /// The name Make Net Class gives the class.
    pub class: String,
    /// The name was typed; until it is, it follows the impedance worked out (a free name).
    pub class_typed: bool,
}

impl Default for ImpedanceState {
    fn default() -> Self {
        ImpedanceState {
            layer: CopperLayer::Top,
            kind: LineKind::Microstrip,
            solve_width: true,
            target: 50.0,
            width: 0.2,
            gap: 0.15,
            class: String::new(),
            class_typed: false,
        }
    }
}

/// The kinds of line, with their names.
const KINDS: [(LineKind, &str); 4] = [
    (LineKind::Microstrip, "electronics.line_microstrip"),
    (LineKind::Stripline, "electronics.line_stripline"),
    (LineKind::Coplanar, "electronics.line_coplanar"),
    (LineKind::Differential, "electronics.line_differential"),
];

/// Whether a line of `kind` runs on an outer layer (`outer`) or an inner one.
fn fits(kind: LineKind, outer: bool) -> bool {
    match kind {
        LineKind::Microstrip | LineKind::Coplanar => outer,
        LineKind::Stripline => !outer,
        LineKind::Differential => true,
    }
}

/// A stack-up layer's name: a copper layer's (Top, Inner 1 …, Bottom, counted down the stack);
/// the rest go by their kind alone.
fn row_name(l: &StackLayer, copper: usize, copper_layers: u8) -> String {
    match l.kind {
        StackKind::Copper => CopperLayer::at(copper, copper_layers).map_or_else(String::new, layer_name),
        _ => String::new(),
    }
}

/// The Layer Stack window: the copper layer count, the thickness, the stack-up and the impedance
/// calculator.
pub(crate) fn stack_window(app: &mut SolveApp, ctx: &egui::Context) {
    if !app.electronics.board.stack_open {
        return;
    }
    let b = board(app);
    let counts: Vec<u8> = (MIN_COPPER_LAYERS..=MAX_COPPER_LAYERS).step_by(2).collect();
    let options: Vec<String> = counts.iter().map(u8::to_string).collect();
    let now = counts.iter().position(|n| *n == b.copper_layers).unwrap_or(0);
    let thickness = b.thickness;
    let title = app.tr_label(command_label("ecad.board.stack")).to_string();
    let (mut layers, mut thick) = (None, None);
    // With another of the design's windows open beside the Inspector (Net Classes, Design Rules,
    // DRC), this one opens beside that, not on top of it (and keeps its own place there).
    let other = app.electronics.design.open.is_some();
    let (id, at) = if other {
        let t = crate::theme::Tokens::get();
        ("electronics_board_stack_beside", crate::electronics_design::beside(app, ctx) - egui::vec2(t.form_card_width * 1.5 + t.space_8, 0.0))
    } else {
        ("electronics_board_stack", crate::electronics_design::beside(app, ctx))
    };
    // Resizable, so the whole stack-up and the calculator show however many layers it has.
    let closed = ui_kit::Window::new(egui::Id::new(id), "list", &title).placed(Align2::RIGHT_TOP, at).resizable().show_body(ctx, |ui| {
        layers = crate::electronics_editor::choice(ui, "electronics_board_stack_layers", &tr("electronics.field_copper_layers"), now, &options)
            .and_then(|i| counts.get(i).copied());
        thick = crate::electronics_editor::number(app, ui, "board:stack:thickness", &tr("electronics.field_thickness"), thickness);
        ui_kit::section(ui, "electronics_board_stackup", &tr("electronics.stack_title"), true, |ui| stackup(app, ui));
        ui_kit::section(ui, "electronics_board_impedance", &tr("electronics.impedance_title"), true, |ui| calculator(app, ui));
        ui_kit::text(ui, &tr("electronics.board_stack_help"), ui_kit::Text::Note("electronics.board_stack_help"));
    });
    if let Some(n) = layers {
        drop(app.run("ecad.board.stack", json!({"copper_layers": n})));
    }
    if let Some(t) = thick {
        drop(app.run("ecad.board.stack", json!({"thickness": t})));
    }
    if closed {
        app.electronics.board.stack_open = false;
    }
}

/// The stack-up as a table, top to bottom: a copper layer's thickness, a dielectric's kind,
/// thickness and εr, the mask's; then the total against the board's thickness and a way back to
/// the default stack-up. Each change sends the whole stack-up (the board takes its total).
fn stackup(app: &mut SolveApp, ui: &mut Ui) {
    let b = board(app);
    let (layers, n, custom, thickness) = (b.stackup_or_default(), b.copper_layers, !b.stackup.is_empty(), b.thickness);
    let headers = [tr("electronics.stack_layer"), tr("electronics.stack_kind"), tr("electronics.field_thickness"), tr("electronics.stack_er")];
    let columns = [
        ui_kit::Column::new(&headers[0], 1.0).flex(),
        ui_kit::Column::new(&headers[1], 1.0).flex(),
        ui_kit::Column::new(&headers[2], 0.8).flex(),
        ui_kit::Column::new(&headers[3], 0.6).flex(),
    ];
    let mut next: Option<Vec<StackLayer>> = None;
    let mut copper = 0;
    ui_kit::table(ui, "electronics_board_stackup", &columns, "", |ui, table| {
        for (i, l) in layers.iter().enumerate() {
            let name = row_name(l, copper, n);
            if l.kind == StackKind::Copper {
                copper += 1;
            }
            table.row(ui, &format!("stack-{i}"), false, |row| {
                row.cell(|ui| ui_kit::text(ui, &name, ui_kit::Text::NameCell(false)));
                row.cell(|ui| {
                    if l.is_dielectric() {
                        let kinds = [(StackKind::Core, "electronics.stack_core"), (StackKind::Prepreg, "electronics.stack_prepreg")];
                        let shown = if l.kind == StackKind::Core { tr("electronics.stack_core") } else { tr("electronics.stack_prepreg") };
                        ui_kit::dropdown_menu(ui, ("electronics_stack_kind", i), &shown, |ui| {
                            for (k, key) in kinds {
                                if ui_kit::list_picker(ui, &format!("stack_kind_{i}_{key}"), &tr(key), l.kind == k).clicked() && l.kind != k {
                                    let mut all = layers.clone();
                                    if let Some(x) = all.get_mut(i) {
                                        x.kind = k;
                                    }
                                    next = Some(all);
                                }
                            }
                        });
                    } else {
                        let kind = if l.kind == StackKind::Copper { tr("electronics.stack_copper") } else { tr("electronics.stack_mask") };
                        ui_kit::text(ui, &kind, ui_kit::Text::Cell);
                    }
                });
                row.cell(|ui| {
                    if let Some(v) = cell_number(app, ui, &format!("board:stack:{i}:thickness"), l.thickness) {
                        let mut all = layers.clone();
                        if let Some(x) = all.get_mut(i) {
                            x.thickness = v;
                        }
                        next = Some(all);
                    }
                });
                row.cell(|ui| {
                    if l.kind == StackKind::Copper {
                        return;
                    }
                    if let Some(v) = cell_number(app, ui, &format!("board:stack:{i}:er"), l.er()) {
                        let mut all = layers.clone();
                        if let Some(x) = all.get_mut(i) {
                            x.er = Some(v);
                        }
                        next = Some(all);
                    }
                });
            });
        }
    });
    let sum = total(&layers);
    let text = crate::i18n::format("electronics.stack_total", &[("total", &mm((sum * 1e4).round() / 1e4)), ("board", &mm(thickness))]);
    let off = (sum - thickness).abs() > THICKNESS_SLACK * thickness;
    ui_kit::text(ui, &text, if off { ui_kit::Text::Error("electronics.stack_total") } else { ui_kit::Text::Note("electronics.stack_total") });
    if custom && crate::electronics_editor::button(ui, "board-stack-reset", &tr("electronics.stack_reset"), true) {
        drop(app.run("ecad.board.stack", json!({"stackup": null})));
    }
    if let Some(all) = next
        && let Ok(v) = serde_json::to_value(&all)
    {
        drop(app.run("ecad.board.stack", json!({"stackup": v})));
    }
}

/// A number field in a table cell; `Some` with a number typed (a text that isn't one says so).
fn cell_number(app: &mut SolveApp, ui: &mut Ui, key: &str, current: f64) -> Option<f64> {
    let old = mm(current);
    let mut text = crate::electronics_editor::typed(app, key, &old);
    let response = ui_kit::cell_editor(ui, egui::Id::new(("electronics_field", key)), &mut text, "", false);
    let text = crate::electronics_editor::settle(app, ui, key, &response, text, &old)?;
    let value = crate::prefs::parse_number(&text);
    if value.is_none() {
        app.set_status(crate::i18n::format("electronics.not_a_number", &[("field", ""), ("text", &text)]), true);
    }
    value
}

/// An impedance error in the UI's words.
fn impedance_error(e: &ImpedanceError) -> String {
    let mut args = e.args();
    if let Some(l) = e.layer() {
        args.retain(|(k, _)| *k != "layer");
        args.push(("layer", layer_name(l)));
    }
    let pairs: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
    crate::i18n::format(e.key(), &pairs)
}

/// The impedance calculator: layer, kind of line, Ω → width or width → Ω, the gap of a coplanar
/// line or a pair, what the formula gives (with its range and what it takes from the stack-up),
/// then Use as Width and Make Net Class.
fn calculator(app: &mut SolveApp, ui: &mut Ui) {
    let b = board(app);
    let (stack, n) = (b.stackup_or_default(), b.copper_layers);
    let mut s = app.electronics.board.impedance.clone();
    if !b.has_layer(s.layer) {
        s.layer = CopperLayer::Top;
    }
    if let Some(l) = span_row(app, ui, "board:impedance:layer", crate::i18n::tr("electronics.field_layer"), s.layer) {
        s.layer = l;
    }
    let outer = !s.layer.is_inner();
    // A kind that doesn't run on the layer gives way to the one that does.
    if !fits(s.kind, outer) {
        s.kind = if outer { LineKind::Microstrip } else { LineKind::Stripline };
    }
    let kinds: Vec<LineKind> = KINDS.iter().filter(|(k, ..)| fits(*k, outer)).map(|(k, ..)| *k).collect();
    let names: Vec<String> = KINDS.iter().filter(|(k, ..)| fits(*k, outer)).map(|(.., name)| tr(name)).collect();
    let at = kinds.iter().position(|k| *k == s.kind).unwrap_or(0);
    if let Some(k) = crate::electronics_editor::choice(ui, "electronics_board_impedance_line", &tr("electronics.field_line"), at, &names)
        .and_then(|i| kinds.get(i))
    {
        s.kind = *k;
    }
    ui_kit::wrapped_row(ui, crate::i18n::tr("electronics.field_solve"), |ui| {
        for (solve, key, name) in [(true, "width", "electronics.impedance_for_width"), (false, "ohms", "electronics.impedance_for_ohms")] {
            if ui_kit::choice_chip(ui, &format!("board:impedance:solve:{key}"), crate::i18n::tr(name), s.solve_width == solve).clicked() {
                s.solve_width = solve;
            }
        }
    });
    let number = |app: &mut SolveApp, ui: &mut Ui, key: &str, label: &str, v: f64| crate::electronics_editor::number(app, ui, key, &tr(label), v);
    if s.solve_width {
        if let Some(v) = number(app, ui, "board:impedance:target", "electronics.field_target_ohms", s.target) {
            s.target = v;
        }
    } else if let Some(v) = number(app, ui, "board:impedance:width", "electronics.field_track_width", s.width) {
        s.width = v;
    }
    let paired = matches!(s.kind, LineKind::Coplanar | LineKind::Differential);
    if paired && let Some(v) = number(app, ui, "board:impedance:gap", "electronics.field_gap", s.gap) {
        s.gap = v;
    }
    let gap = paired.then_some(s.gap);
    let worked: Result<(f64, Impedance, impedance::Medium), ImpedanceError> = impedance::medium(&stack, s.layer, n).and_then(|m| {
        if s.solve_width {
            impedance::width_for(&m, s.kind, s.target, gap).map(|(w, z)| (w, z, m))
        } else {
            impedance::impedance(&m, s.kind, s.width, gap).map(|z| (s.width, z, m))
        }
    });
    let mut width = None;
    match &worked {
        Ok((w, z, m)) => {
            let z0 = format!("{:.1}", z.z0);
            let w3 = mm((w * 1000.0).round() / 1000.0);
            let text = crate::i18n::format("electronics.impedance_result", &[("ohms", &z0), ("width", &w3)]);
            ui_kit::text(ui, &text, ui_kit::Text::Heading("electronics.impedance_result"));
            let er_eff = z.er_eff.map_or_else(|| "—".to_string(), |e| format!("{e:.2}"));
            let h = if m.outer {
                mm((m.h1 * 1e4).round() / 1e4)
            } else {
                format!("{} / {}", mm((m.h1 * 1e4).round() / 1e4), mm((m.h2 * 1e4).round() / 1e4))
            };
            let medium = crate::i18n::format(
                "electronics.impedance_medium",
                &[("h", &h), ("t", &mm((m.t * 1e4).round() / 1e4)), ("er", &format!("{:.2}", m.er)), ("er_eff", &er_eff)],
            );
            ui_kit::text(ui, &medium, ui_kit::Text::Note("electronics.impedance_medium"));
            let formula = crate::i18n::format("electronics.impedance_formula", &[("formula", z.formula)]);
            ui_kit::text(ui, &formula, ui_kit::Text::Note("electronics.impedance_formula"));
            if !z.valid_range {
                ui_kit::text(ui, &out_of_range(z.out_of_range), ui_kit::Text::Error("electronics.impedance_out_of_range"));
            }
            width = Some(*w);
        }
        Err(e) => {
            ui_kit::text(ui, &impedance_error(e), ui_kit::Text::Error("electronics.impedance_error"));
        }
    }
    let w = width.map(|w| (w * 1000.0).round() / 1000.0);
    if crate::electronics_editor::button(ui, "board-impedance-use", &tr("electronics.impedance_use"), w.is_some())
        && let Some(w) = w
    {
        app.electronics.board.route.width = Some(w);
        app.set_status(crate::i18n::format("electronics.impedance_used", &[("width", &mm(w))]), false);
    }
    // Until a name is typed, the class is named after the impedance worked out, free of the
    // classes the board has.
    if !s.class_typed {
        let ohms = if s.solve_width {
            format!("{:.0}", s.target)
        } else {
            worked.as_ref().map_or_else(|_| String::new(), |(_, z, _)| format!("{:.0}", z.z0))
        };
        let base = crate::i18n::format("electronics.impedance_class_name", &[("ohms", &ohms)]);
        let taken: Vec<String> = board(app).rules.net_classes.iter().map(|c| c.name.clone()).collect();
        s.class = crate::electronics_editor::free_from(&base, &taken);
    }
    let (_, typed) = ui_kit::text_row(ui, &tr("electronics.field_class_name"), &mut s.class, "");
    if typed {
        s.class_typed = !s.class.trim().is_empty();
    }
    let ready = w.is_some() && !s.class.trim().is_empty();
    if crate::electronics_editor::button(ui, "board-impedance-class", &tr("electronics.impedance_make_class"), ready)
        && let Some(w) = w
    {
        let name = s.class.trim().to_string();
        if board(app).rules.class(&name).is_some() {
            // Make Net Class makes one: a class of that name is not changed from here.
            app.set_status(crate::i18n::format("electronics.impedance_class_exists", &[("name", &name)]), true);
        } else {
            let mut params = json!({"name": name, "track_width": w});
            match s.kind {
                LineKind::Differential => params["diff_pair"] = json!({"width": w, "gap": s.gap}),
                // A coplanar line's gap is the clearance its copper keeps to the ground beside it.
                LineKind::Coplanar => params["clearance"] = json!(s.gap),
                LineKind::Microstrip | LineKind::Stripline => {}
            }
            if app.run("ecad.board.netclass", params).is_ok() {
                crate::electronics_board_rules::show_class(app, &name);
                if s.kind == LineKind::Coplanar {
                    app.set_status(crate::i18n::format("electronics.impedance_class_gap", &[("name", &name), ("gap", &mm(s.gap))]), false);
                } else {
                    app.set_status(crate::i18n::format("electronics.impedance_class_made", &[("name", &name), ("width", &mm(w))]), false);
                }
            }
        }
    }
    app.electronics.board.impedance = s;
}

/// "t/s (the copper's thickness over the gap) is 0.233, and the formula holds up to 0.2: take it
/// as a rough guess.", or the plain note when no input is named.
fn out_of_range(o: Option<impedance::OutOfRange>) -> String {
    let Some(o) = o else { return tr("electronics.impedance_out_of_range") };
    let key = match o.input {
        "w/h" => "electronics.impedance_input_w_h",
        "er" => "electronics.impedance_input_er",
        "w/(b-t)" => "electronics.impedance_input_w_b",
        "t/b" => "electronics.impedance_input_t_b",
        _ => "electronics.impedance_input_t_s",
    };
    let value = format!("{:.3}", o.value);
    let (range_key, args): (&str, Vec<(&str, String)>) = match (o.low, o.high) {
        (Some(l), Some(h)) => ("electronics.impedance_range_between", vec![("low", mm(l)), ("high", mm(h))]),
        (Some(l), None) => ("electronics.impedance_range_above", vec![("low", mm(l))]),
        (None, Some(h)) => ("electronics.impedance_range_below", vec![("high", mm(h))]),
        (None, None) => return tr("electronics.impedance_out_of_range"),
    };
    let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let range = crate::i18n::format(range_key, &args);
    crate::i18n::format("electronics.impedance_out_of_range_input", &[("input", &tr(key)), ("value", &value), ("range", &range)])
}
