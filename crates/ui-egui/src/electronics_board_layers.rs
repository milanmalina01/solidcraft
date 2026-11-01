//! The board's layers (see `electronics_board`): the Inspector's Layers tab, which lists each layer
//! of the drawing grouped as copper, mechanical, silkscreen, documentation and rules, each with an
//! eye to show or hide it, its colour and, for copper, a radio that makes it the active (routing)
//! layer, the board's Names (how high the footprints' names and values are, and all of them shown
//! or hidden), then the Layer Stack button (the window is `electronics_board_stack`); and the
//! command bar along the top of the page, with the active layer, the grid points snap to (typed,
//! or from a menu) and where the pointer is.

use egui::{Align, Align2, Color32, FontId, Layout, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::CopperLayer;

use crate::SolveApp;
use crate::electronics_board::{Tool, active_layer, board, grid, layer_colour, snap};
use crate::electronics_board_route::{change_layer, layer_name};
use crate::electronics_editor::tr;
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;
use crate::ui_kit;

/// The grids the command bar offers, in millimetres: metric steps, then 12.5, 25, 50 and 100 mil.
pub(crate) const GRIDS: [f64; 11] = [0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 0.3175, 0.635, 1.27, 2.54];
/// The finest and the coarsest grid a typed value gives, in millimetres.
const TYPED_GRID: std::ops::RangeInclusive<f64> = 0.001..=100.0;
/// The grid a new board view snaps to: 25 mil.
pub(crate) const DEFAULT_GRID: f64 = 0.635;
/// The command bar's widest, in points.
const BAR_WIDTH: f32 = 560.0;
/// A hidden layer's eye and swatch are this faint.
const HIDDEN: f32 = 0.4;

/// A layer of the board's drawing, shown or hidden in the Layers tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    /// A copper layer's tracks, surface-mount pads and drawn copper.
    Copper(CopperLayer),
    /// Plated through-hole pads.
    Pads,
    Vias,
    Airwires,
    Outline,
    /// Unplated holes: mounting holes and footprints' bare holes.
    Holes,
    /// The holes drilled through plated pads and vias.
    Drills,
    TopSilkscreen,
    BottomSilkscreen,
    /// Footprints' name (designator) texts.
    Names,
    /// Footprints' value texts.
    Values,
    Courtyard,
    /// Footprints' fabrication drawings and the board's documentation layer.
    Fab,
    /// The copper pours' fills (their outlines show with their copper layer).
    PourFill,
    Keepouts,
    /// The design rule check's marks on the board, while its window is open.
    DrcMarkers,
    /// How weakly the ground net's copper on the active layer is tied (a heat map); hidden until
    /// shown, as it is worked out beside the window.
    Ground,
}

/// The Layers tab's groups, each with its heading's message key.
fn groups(app: &SolveApp) -> Vec<(&'static str, Vec<Layer>)> {
    let mut copper: Vec<Layer> = board(app).stack().into_iter().map(Layer::Copper).collect();
    copper.extend([Layer::Pads, Layer::Vias, Layer::PourFill, Layer::Airwires]);
    vec![
        ("electronics.layer_group_copper", copper),
        ("electronics.layer_group_mechanical", vec![Layer::Outline, Layer::Holes, Layer::Drills]),
        ("electronics.layer_group_silkscreen", vec![Layer::TopSilkscreen, Layer::BottomSilkscreen]),
        ("electronics.layer_group_documentation", vec![Layer::Names, Layer::Values, Layer::Courtyard, Layer::Fab]),
        ("electronics.layer_group_rules", vec![Layer::Keepouts, Layer::DrcMarkers, Layer::Ground]),
    ]
}

/// A layer's name in the Layers tab, and the key scenarios find its row by.
fn name(layer: Layer) -> (String, String) {
    let (key, text) = match layer {
        Layer::Copper(l) => return (layer_name(l), l.name()),
        Layer::Pads => ("pads", "electronics.layer_pads"),
        Layer::Vias => ("vias", "electronics.layer_vias"),
        Layer::Airwires => ("airwires", "electronics.layer_airwires"),
        Layer::Outline => ("outline", "electronics.layer_outline"),
        Layer::Holes => ("holes", "electronics.layer_holes"),
        Layer::Drills => ("drills", "electronics.layer_drills"),
        Layer::TopSilkscreen => ("silkscreen_top", "electronics.side_top"),
        Layer::BottomSilkscreen => ("silkscreen_bottom", "electronics.side_bottom"),
        Layer::Names => ("names", "electronics.layer_names"),
        Layer::Values => ("values", "electronics.layer_values"),
        Layer::Courtyard => ("courtyard", "electronics.layer_courtyard"),
        Layer::Fab => ("fab", "electronics.layer_fab"),
        Layer::PourFill => ("pour_fill", "electronics.layer_pour_fill"),
        Layer::Keepouts => ("keepouts", "electronics.layer_keepouts"),
        Layer::DrcMarkers => ("drc_markers", "electronics.layer_drc_markers"),
        Layer::Ground => ("ground", "electronics.layer_ground"),
    };
    (tr(text), key.to_string())
}

/// The colour a layer is drawn in, for its swatch.
fn colour(t: &Tokens, layer: Layer) -> Color32 {
    match layer {
        Layer::Copper(l) => layer_colour(t, l),
        Layer::Pads | Layer::Vias => t.pcb_plated,
        Layer::Airwires => t.pcb_airwire,
        Layer::Outline => t.pcb_outline,
        Layer::Holes | Layer::Drills => t.text_dim,
        Layer::TopSilkscreen | Layer::Names | Layer::Values => t.pcb_silkscreen,
        Layer::BottomSilkscreen => t.pcb_silkscreen.gamma_multiply(0.6),
        Layer::Courtyard => t.grid_major,
        Layer::Fab => t.pcb_documentation,
        Layer::PourFill => layer_colour(t, CopperLayer::Top).gamma_multiply(t.pcb_pour_alpha),
        Layer::Keepouts => t.pcb_keepout,
        Layer::DrcMarkers => t.pcb_drc_error,
        Layer::Ground => t.agent_ok,
    }
}

/// A small square of `colour` in `rect`.
fn swatch(ui: &Ui, rect: Rect, colour: Color32) {
    let t = Tokens::get();
    ui.painter().rect_filled(rect, f32::from(t.control_radius) / 2.0, colour);
}

/// The Layers tab: each group's layers, the board's names, then the Layer Stack button.
pub(crate) fn tab(app: &mut SolveApp, ui: &mut Ui) {
    let t = Tokens::get();
    for (heading, layers) in groups(app) {
        ui_kit::text(ui, &tr(heading), ui_kit::Text::Heading(heading));
        for layer in layers {
            row(app, ui, layer);
            if layer == Layer::Ground
                && let Some(note) = crate::electronics_board_ground::note(app)
            {
                ui_kit::text(ui, &note, ui_kit::Text::Note("electronics.ground_map_note"));
            }
        }
        ui.add_space(t.space_6);
    }
    names(app, ui);
    ui.add_space(t.space_6);
    let label = format!("{}…", app.tr_label(command_label("ecad.board.stack")));
    if crate::electronics_editor::button(ui, "board-layer-stack", &label, true) {
        toggle_stack(app);
    }
}

/// The footprints' names and values across the board (saved with it, where the rows above only
/// show or hide them here): how high they are, empty for each in step with its part, and every
/// name or value shown or hidden; each change one `ecad.board.names`.
fn names(app: &mut SolveApp, ui: &mut Ui) {
    let heading = "electronics.board_names_heading";
    ui_kit::text(ui, &tr(heading), ui_kit::Text::Heading(heading));
    let b = board(app);
    let height = b.name_height;
    let (names, values) = (b.footprints.iter().all(|f| f.show_name), b.footprints.iter().all(|f| f.show_value));
    let shown = height.map(crate::electronics_editor::mm).unwrap_or_default();
    let label = tr("electronics.field_name_height");
    if let Some(text) = crate::electronics_editor::field(app, ui, "board:names:height", &label, &shown) {
        let value = if text.trim().is_empty() { Some(Value::Null) } else { crate::prefs::parse_number(&text).map(|v| json!(v)) };
        match value {
            Some(v) => drop(app.run("ecad.board.names", json!({"height": v}))),
            None => app.set_status(crate::i18n::format("electronics.not_a_number", &[("field", &label), ("text", &text)]), true),
        }
    }
    if height.is_none() {
        ui_kit::text(ui, &tr("electronics.board_names_auto"), ui_kit::Text::Note("electronics.board_names_auto"));
    }
    // The silkscreen names mode lays every name out at once (`ecad.board.labels`); picking the
    // mode the board has lays them out again for the board as it is now.
    let modes = ["full", "fit", "minimal", "none"];
    let shown_modes: Vec<String> = modes.iter().map(|m| tr(&format!("electronics.board_names_mode_{m}"))).collect();
    let current = board(app).names_mode.map_or(usize::MAX, |m| modes.iter().position(|x| *x == m.name()).unwrap_or(usize::MAX));
    let mut options = shown_modes.clone();
    if current == usize::MAX {
        options.insert(0, tr("electronics.board_names_mode_own"));
    }
    let at = if current == usize::MAX { 0 } else { current };
    if let Some(i) = crate::electronics_editor::choice(ui, "board_names_mode", &tr("electronics.board_names_mode"), at, &options) {
        let pick = if current == usize::MAX { i.checked_sub(1) } else { Some(i) };
        if let Some(mode) = pick.and_then(|k| modes.get(k)) {
            match app.run("ecad.board.labels", json!({"mode": mode})) {
                Ok(r) => {
                    let count = |k: &str| r.get(k).and_then(Value::as_array).map_or(0, Vec::len).to_string();
                    let text = crate::i18n::format(
                        "electronics.board_names_laid_out",
                        &[("placed", &count("placed")), ("assembly", &count("moved_to_assembly")), ("unplaceable", &count("unplaceable"))],
                    );
                    app.set_status(text, false);
                }
                Err(e) => app.set_status(e, true),
            }
        }
    }
    ui_kit::text(ui, &tr("electronics.board_names_mode_help"), ui_kit::Text::Note("electronics.board_names_mode_help"));
    if ui_kit::checkbox_row(ui, "board:names:show", &tr("electronics.board_names_show"), names) {
        drop(app.run("ecad.board.names", json!({"show_names": !names})));
    }
    if ui_kit::checkbox_row(ui, "board:names:values", &tr("electronics.board_names_values"), values) {
        drop(app.run("ecad.board.names", json!({"show_values": !values})));
    }
}

/// A command's label as the engine has it.
pub(crate) fn command_label(id: &str) -> &'static str {
    solidcraft_engine::find_command(id).map_or("", |c| c.label)
}

/// One layer's row: a press on it shows or hides the layer; a copper layer's radio at its end
/// makes that the active layer.
fn row(app: &mut SolveApp, ui: &mut Ui, layer: Layer) {
    let t = Tokens::get();
    let h = ui_kit::row_height();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let (text, key) = name(layer);
    let copper = match layer {
        Layer::Copper(l) => Some(l),
        _ => None,
    };
    let radio = Rect::from_min_max(pos2(rect.right() - h, rect.top()), rect.max);
    let body = if copper.is_some() { Rect::from_min_max(rect.min, pos2(radio.left(), rect.bottom())) } else { rect };
    let response = ui.interact(body, ui.id().with(("electronics_board_layer", &key)), Sense::click());
    crate::scenario::publish_target(&format!("electronics:board:layer:{key}"), body);
    let shown = !app.electronics.board.hidden.contains(&layer);
    if response.hovered() {
        ui.painter().rect_filled(body, t.radius_sm, t.hover);
    }
    let eye = Rect::from_center_size(pos2(rect.left() + h / 2.0, rect.center().y), vec2(15.0, 15.0));
    crate::icons::paint_icon(ui.painter(), eye, if shown { "eye" } else { "suppress" }, if shown { 0.85 } else { HIDDEN });
    let square = Rect::from_center_size(pos2(eye.right() + t.space_8 + t.space_6, rect.center().y), vec2(t.space_12, t.space_12));
    let fill = colour(&t, layer);
    swatch(ui, square, if shown { fill } else { fill.gamma_multiply(HIDDEN) });
    let ink = if shown { t.text } else { t.text_dim };
    ui.painter().text(pos2(square.right() + t.space_8, rect.center().y), Align2::LEFT_CENTER, text, FontId::proportional(t.font_label), ink);
    // The Hide/Show tip only over the eye, so it doesn't cover the next row as the pointer runs
    // down the list.
    let tip = if shown { crate::i18n::tr("ui.hide") } else { crate::i18n::tr("ui.show") };
    let over_eye = response.hover_pos().is_some_and(|q| eye.expand(2.0).contains(q));
    let response = if over_eye { response.on_hover_text(tip) } else { response };
    if response.clicked() {
        let hidden = &mut app.electronics.board.hidden;
        if shown {
            hidden.insert(layer);
        } else {
            hidden.remove(&layer);
        }
    }
    let Some(l) = copper else { return };
    let active = active_layer(app) == l;
    let picked = ui.interact(radio, ui.id().with(("electronics_board_layer_radio", &key)), Sense::click());
    crate::scenario::publish_target(&format!("electronics:board:layer:{key}:active"), radio);
    let c = radio.center();
    ui.painter().circle_stroke(c, 5.0, Stroke::new(1.2, if active { t.accent } else { t.text_dim }));
    if active {
        ui.painter().circle_filled(c, 2.8, t.accent);
    }
    if picked.on_hover_text(tr("electronics.board_active_layer")).clicked() {
        change_layer(app, l);
    }
}

/// Open the Layer Stack window, or close it.
pub(crate) fn toggle_stack(app: &mut SolveApp) {
    let s = &mut app.electronics.board;
    s.stack_open = !s.stack_open;
}

/// Where the command bar goes on the page `rect`: along the top of the part the panels leave free.
pub(crate) fn bar_rect(app: &SolveApp, rect: Rect) -> Rect {
    let t = Tokens::get();
    let content = crate::shell::content_rect(app, rect);
    let area = if content.is_positive() { content } else { rect };
    let height = ui_kit::row_height() + 2.0 * t.space_4;
    let width = BAR_WIDTH.min(area.width() - 2.0 * t.space_8).max(0.0);
    Rect::from_min_size(area.left_top() + vec2(t.space_8, t.space_8), vec2(width, height))
}

/// A grid's name in the bar's menu: millimetres, with mils for the inch steps (whole half-mils).
fn grid_label(g: f64) -> String {
    let mm = crate::electronics_editor::mm(g);
    let half_mils = g / 0.0254 * 2.0;
    if (half_mils - half_mils.round()).abs() < 1e-6 {
        let mil = crate::electronics_editor::mm(half_mils.round() / 2.0);
        crate::i18n::format("electronics.board_grid_mil", &[("mil", &mil), ("mm", &mm)])
    } else {
        crate::i18n::format("electronics.board_grid_mm", &[("mm", &mm)])
    }
}

/// The command bar along the top of the page, in `bar`: the active layer (the drawing layer while
/// a DRAW tool is on) with its colour and a menu of the others, the grid, and where the pointer is
/// on it.
pub(crate) fn command_bar(app: &mut SolveApp, ui: &mut Ui, rect: Rect, bar: Rect) {
    if !bar.is_positive() {
        return;
    }
    let t = Tokens::get();
    ui.painter().rect(bar, t.radius_md, t.overlay, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    crate::scenario::publish_target("electronics:board:bar", bar);
    let pointer = ui.input(|i| (i.pointer.hover_pos(), i.modifiers.command || i.modifiers.ctrl));
    let at = match (pointer, PageMap::new(&app.electronics.page.cam, rect)) {
        ((Some(p), free), Some(map)) if rect.contains(p) && !bar.contains(p) => Some(snap(app, map, p, free)),
        _ => None,
    };
    let inner = bar.shrink2(vec2(t.space_8, t.space_4));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
    ui_kit::scope(&mut child, |ui| {
        ui.spacing_mut().item_spacing.x = t.space_8;
        let h = ui_kit::row_height();
        let square = |ui: &mut Ui, colour: Color32| {
            let (r, _) = ui.allocate_exact_size(vec2(t.space_12, t.space_12), Sense::hover());
            swatch(ui, r, colour);
        };
        if let Some(Tool::Draw(_)) = &app.electronics.board.tool {
            let layer = app.electronics.board.draw.layer;
            square(ui, crate::electronics_board_draw::draw_colour(&t, layer));
            ui.allocate_ui(vec2(150.0, h), |ui| {
                if let Some(l) = crate::electronics_board_draw::layer_menu(app, ui, "electronics_board_bar_draw_layer", layer) {
                    app.electronics.board.draw.layer = l;
                }
            });
        } else {
            let layer = active_layer(app);
            square(ui, layer_colour(&t, layer));
            let stack = board(app).stack();
            let mut chosen = None;
            ui.allocate_ui(vec2(120.0, h), |ui| {
                ui_kit::dropdown_menu(ui, "electronics_board_bar_layer", &layer_name(layer), |ui| {
                    for l in &stack {
                        ui.horizontal(|ui| {
                            square(ui, layer_colour(&t, *l));
                            let r = ui_kit::list_picker(ui, &format!("board_bar_layer_{}", l.name()), &layer_name(*l), *l == layer);
                            crate::scenario::publish_handle(&format!("dialog:board-bar-layer:{}", l.name()), r.rect.center());
                            if r.clicked() {
                                chosen = Some(*l);
                            }
                        });
                    }
                });
            });
            if let Some(l) = chosen {
                change_layer(app, l);
            }
        }
        ui.label(egui::RichText::new(tr("electronics.board_grid")).color(t.text_dim));
        // The grid in millimetres, typed, with a menu of the usual steps beside it.
        let g = grid(app);
        let mut picked = None;
        let key = "board:grid";
        let old = crate::electronics_editor::mm(g);
        let mut text = crate::electronics_editor::typed(app, key, &old);
        let field =
            ui.allocate_ui(vec2(64.0, h), |ui| ui_kit::cell_editor(ui, egui::Id::new(("electronics_field", key)), &mut text, "", false)).inner;
        crate::scenario::publish_handle("dialog:board-bar-grid:field", field.rect.center());
        ui.allocate_ui(vec2(h * 1.4, h), |ui| {
            ui_kit::dropdown_menu(ui, "electronics_board_bar_grid", "", |ui| {
                for step in GRIDS {
                    let r = ui_kit::list_picker(ui, &format!("board_bar_grid_{step}"), &grid_label(step), (step - g).abs() < 1e-9);
                    crate::scenario::publish_handle(&format!("dialog:board-bar-grid:{}", crate::electronics_editor::mm(step)), r.rect.center());
                    if r.clicked() {
                        picked = Some(step);
                    }
                }
            });
        });
        if let Some(typed) = crate::electronics_editor::settle(app, ui, key, &field, text, &old) {
            match crate::prefs::parse_number(&typed).filter(|v| TYPED_GRID.contains(v)) {
                Some(v) => picked = Some(v),
                None => app.set_status(tr("electronics.board_bad_grid"), true),
            }
        }
        if let Some(step) = picked {
            app.electronics.board.grid = step;
        }
        if let Some([x, y]) = at {
            let text = crate::i18n::format(
                "electronics.board_cursor",
                &[("x", &crate::electronics_editor::mm(x)), ("y", &crate::electronics_editor::mm(y))],
            );
            ui.label(egui::RichText::new(text).color(t.text_dim).size(t.font_small));
        }
    });
}
