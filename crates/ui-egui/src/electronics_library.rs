//! An electronics library document: its symbols, footprints, 3D packages and the electronic
//! components that use them (`doc.ecad`). The Browser's Library card lists them by kind, with a
//! search and a preview; a click opens one on the page in its editor, and the toolbar shows that
//! editor's tab beside LIBRARY. The page shows the open symbol or footprint to draw on
//! (`electronics_workspace`), a component's symbol beside its chosen variant's footprint (a click
//! on a pin, then on pads, connects them), or a 3D package's model. With nothing picked the
//! Inspector shows the open item's properties.

use egui::{Align2, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};
use solidcraft_engine::doc::DocumentKind;
use solidcraft_engine::doc::ecad::{FootprintItem, PackageItem};

use crate::SolveApp;
use crate::electronics_components::{Shown, View, shown, shown_copy};
use crate::electronics_editor::{Open, tr};
use crate::electronics_workspace::{COMPONENT, FOOTPRINT, LIBRARY, PageMap, SYMBOL};
use crate::theme::Tokens;
use crate::ui_kit;

/// The Library card's preview, in points.
const PREVIEW_HEIGHT: f32 = 132.0;

/// A library item, by id (one id space for the four kinds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opened {
    Component(u64),
    Symbol(u64),
    Footprint(u64),
    Package(u64),
}

impl Opened {
    /// Its editor's toolbar tab; a 3D package has none (LIBRARY shows).
    pub(crate) fn tab(self) -> Option<&'static str> {
        match self {
            Opened::Component(_) => Some(COMPONENT),
            Opened::Symbol(_) => Some(SYMBOL),
            Opened::Footprint(_) => Some(FOOTPRINT),
            Opened::Package(_) => None,
        }
    }

    fn kind(self) -> Kind {
        match self {
            Opened::Component(_) => Kind::Components,
            Opened::Symbol(_) => Kind::Symbols,
            Opened::Footprint(_) => Kind::Footprints,
            Opened::Package(_) => Kind::Packages,
        }
    }
}

/// The Library card's kinds of item, one sub-tab each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Components,
    Symbols,
    Footprints,
    Packages,
}

/// Each kind: its key for scenarios, its sub-tab's label, what its + says and the command it starts.
const KINDS: [(Kind, &str, &str, &str, &str); 4] = [
    (Kind::Components, "components", "electronics.library_components", "electronics.new_component", "ecad.component.create"),
    (Kind::Symbols, "symbols", "electronics.library_symbols", "electronics.new_symbol", "ecad.symbol.create"),
    (Kind::Footprints, "footprints", "electronics.library_footprints", "electronics.new_footprint", "ecad.footprint.create"),
    (Kind::Packages, "packages", "electronics.library_packages", "electronics.new_package", "ecad.package.create"),
];

/// The Library card's sub-tab and search.
#[derive(Default)]
pub struct Card {
    pub kind: Kind,
    pub search: String,
}

/// Whether the library still has `item`.
fn exists(app: &SolveApp, item: Opened) -> bool {
    let ecad = &app.session.doc.ecad;
    match item {
        Opened::Component(id) => ecad.component(id).is_some(),
        Opened::Symbol(id) => ecad.symbol(id).is_some(),
        Opened::Footprint(id) => ecad.footprint(id).is_some(),
        Opened::Package(id) => ecad.package(id).is_some(),
    }
}

/// Show `item` on the page, in its editor's tab.
pub(crate) fn open(app: &mut SolveApp, item: Opened) {
    let e = &mut app.electronics;
    if e.opened != Some(item) {
        e.variant = None;
        e.page.fitted = false;
    }
    e.opened = Some(item);
    e.card.kind = item.kind();
    match item {
        Opened::Symbol(_) => e.view = View::Symbol,
        Opened::Footprint(_) => e.view = View::Footprint,
        _ => {}
    }
    app.ui.tab = item.tab().unwrap_or(LIBRARY).into();
}

/// Forget an open item or a chosen variant that is gone, and keep the editor on the open item's
/// drawing.
pub(crate) fn settle(app: &mut SolveApp) {
    if app.session.doc.kind != DocumentKind::ElectronicsLibrary {
        app.electronics.opened = None;
        return;
    }
    if app.electronics.opened.is_some_and(|item| !exists(app, item)) {
        app.electronics.opened = None;
    }
    let gone = match (app.electronics.opened, &app.electronics.variant) {
        (Some(Opened::Component(id)), Some(v)) => app.session.doc.ecad.component(id).is_none_or(|c| c.variant(v).is_none()),
        (_, v) => v.is_some(),
    };
    if gone {
        app.electronics.variant = None;
    }
    match app.electronics.opened {
        Some(Opened::Symbol(_)) => app.electronics.view = View::Symbol,
        Some(Opened::Footprint(_)) => app.electronics.view = View::Footprint,
        _ => {}
    }
}

/// After an `ecad.*` command in a library: open the item its reply names (a component first),
/// unless the open component uses it; and choose the variant it names.
pub(crate) fn follow(app: &mut SolveApp, id: &str, reply: &Value) {
    if !id.starts_with("ecad.") || app.session.doc.kind != DocumentKind::ElectronicsLibrary {
        return;
    }
    let item = |key: &str| reply.get(key).and_then(|c| c.get("id")).and_then(Value::as_u64);
    // What the open component shows: its symbol, and its chosen variant's footprint and package.
    let (symbol, footprint, package) = match shown(app) {
        Some(c) if c.component.id != 0 => (c.component.symbol, c.variant.and_then(|v| v.footprint), c.variant.and_then(|v| v.package)),
        _ => (None, None, None),
    };
    let target = if let Some(c) = item("component") {
        Some(Opened::Component(c))
    } else if let Some(s) = item("symbol") {
        (symbol != Some(s)).then_some(Opened::Symbol(s))
    } else if let Some(f) = item("footprint") {
        (footprint != Some(f)).then_some(Opened::Footprint(f))
    } else if let Some(p) = item("package") {
        (package != Some(p)).then_some(Opened::Package(p))
    } else {
        None
    };
    let variant = reply.get("variant").and_then(Value::as_str).map(str::to_string);
    if let Some(target) = target.filter(|t| exists(app, *t)) {
        if app.electronics.opened != Some(target) {
            open(app, target);
        }
        if let (Opened::Component(_), Some(v)) = (target, variant) {
            app.electronics.variant = Some(v);
        }
    }
    // A generated drawing is new: frame it.
    if id.ends_with(".generate") {
        app.electronics.page.fitted = false;
    }
}

/// A name no item of `kind` has yet: Part 1, Symbol 1, Footprint 1, Package 1…
pub(crate) fn new_item_name(app: &SolveApp, kind: Opened) -> String {
    let ecad = &app.session.doc.ecad;
    let (key, taken): (&str, Vec<&str>) = match kind {
        Opened::Component(_) => ("electronics.new_name", ecad.components.iter().map(|c| c.name.as_str()).collect()),
        Opened::Symbol(_) => ("electronics.new_symbol_name", ecad.symbols.iter().map(|c| c.name.as_str()).collect()),
        Opened::Footprint(_) => ("electronics.new_footprint_name", ecad.footprints.iter().map(|c| c.name.as_str()).collect()),
        Opened::Package(_) => ("electronics.new_package_name", ecad.packages.iter().map(|c| c.name.as_str()).collect()),
    };
    (1..=10_000)
        .map(|n| crate::i18n::format(key, &[("n", &n.to_string())]))
        .find(|name| !taken.contains(&name.as_str()))
        .unwrap_or_else(|| crate::i18n::format(key, &[("n", "1")]))
}

/// The library's toolbar commands not handled with the editor's: the new items. `true` when
/// taken.
pub(crate) fn start(app: &mut SolveApp, id: &str) -> bool {
    let kind = match id {
        "ecad.component.create" => Opened::Component(0),
        "ecad.symbol.create" => Opened::Symbol(0),
        "ecad.footprint.create" => Opened::Footprint(0),
        "ecad.component.variant.add" => {
            add_variant(app);
            return true;
        }
        _ => return false,
    };
    let name = new_item_name(app, kind);
    // The reply names the new item, which `follow` opens.
    drop(app.run(id, json!({"name": name})));
    true
}

/// A new variant of the open component, chosen; its footprint is picked in the Inspector.
fn add_variant(app: &mut SolveApp) {
    let Some(Opened::Component(id)) = app.electronics.opened else {
        app.set_status(tr("electronics.open_component_first"), true);
        return;
    };
    let taken: Vec<String> = app.session.doc.ecad.component(id).map(|c| c.variants.iter().map(|v| v.name.clone()).collect()).unwrap_or_default();
    let name = (1..=10_000)
        .map(|n| crate::i18n::format("electronics.new_variant_name", &[("n", &n.to_string())]))
        .find(|name| !taken.contains(name))
        .unwrap_or_default();
    if app.run("ecad.component.variant.add", json!({"component": id, "name": name})).is_ok() {
        app.electronics.variant = Some(name);
    }
}

/// One row of the Library card: the item, its name and what it has.
struct Row {
    item: Opened,
    name: String,
    detail: String,
}

/// The rows of `kind` whose name or description holds `search` (any case).
fn rows(app: &SolveApp, kind: Kind, search: &str) -> Vec<Row> {
    let ecad = &app.session.doc.ecad;
    let search = search.trim().to_lowercase();
    let detail = |what: String, description: &str| if description.is_empty() { what } else { format!("{what} · {description}") };
    let count = |key: &str, n: usize| crate::i18n::format(key, &[("n", &n.to_string())]);
    let mut rows: Vec<(Row, String)> = match kind {
        Kind::Components => ecad
            .components
            .iter()
            .map(|c| {
                let r = ecad.resolve(c, None);
                let counts = crate::i18n::format(
                    "electronics.counts",
                    &[("pins", &r.symbol.pins.len().to_string()), ("pads", &r.footprint.pads.len().to_string())],
                );
                let name = if c.value.is_empty() { c.name.clone() } else { format!("{} · {}", c.name, c.value) };
                (Row { item: Opened::Component(c.id), name, detail: detail(counts, &c.description) }, c.description.clone())
            })
            .collect(),
        Kind::Symbols => ecad
            .symbols
            .iter()
            .map(|s| {
                let row = Row {
                    item: Opened::Symbol(s.id),
                    name: s.name.clone(),
                    detail: detail(count("electronics.count_pins", s.pins.len()), &s.description),
                };
                (row, s.description.clone())
            })
            .collect(),
        Kind::Footprints => ecad
            .footprints
            .iter()
            .map(|f| {
                let row = Row {
                    item: Opened::Footprint(f.id),
                    name: f.name.clone(),
                    detail: detail(count("electronics.count_pads", f.pads.len()), &f.description),
                };
                (row, f.description.clone())
            })
            .collect(),
        Kind::Packages => ecad
            .packages
            .iter()
            .map(|p| (Row { item: Opened::Package(p.id), name: p.name.clone(), detail: model_line(app, p) }, String::new()))
            .collect(),
    };
    if !search.is_empty() {
        rows.retain(|(row, description)| row.name.to_lowercase().contains(&search) || description.to_lowercase().contains(&search));
    }
    rows.into_iter().map(|(row, _)| row).collect()
}

/// "Model: …" for a 3D package, or that its model is gone.
fn model_line(app: &SolveApp, p: &PackageItem) -> String {
    match app.session.doc.components.iter().find(|m| m.id == p.model) {
        Some(m) => crate::i18n::format("electronics.package_model", &[("name", &m.name)]),
        None => tr("electronics.problem_package_missing"),
    }
}

/// The Browser card in an electronics document: an electronics library's Library card, or an
/// electronics design's tree (`electronics_design::browser`). `false` in a 3D design.
pub(crate) fn browser(app: &mut SolveApp, ui: &mut Ui) -> bool {
    if !crate::electronics_workspace::active(app) {
        return false;
    }
    // A design's Browser is the design's tree on its schematic and its board alike; the library
    // card is a library's own.
    if app.session.doc.kind == DocumentKind::ElectronicsDesign {
        ui_kit::scope(ui, |ui| crate::electronics_design::browser(app, ui));
    } else {
        ui_kit::scope(ui, |ui| card(app, ui));
    }
    true
}

/// The library's name, a sub-tab for each kind of item, a search with a + for a new item, the rows
/// and a preview of the open one.
fn card(app: &mut SolveApp, ui: &mut Ui) {
    let t = Tokens::get();
    ui.add_space(t.space_6);
    let name = crate::i18n::tr_name(&app.session.doc.name);
    ui_kit::text(ui, &name, ui_kit::Text::Heading("electronics.library_name"));
    ui.add_space(t.space_6);
    let labels: Vec<String> = KINDS.iter().map(|k| tr(k.2)).collect();
    let options: Vec<&str> = labels.iter().map(String::as_str).collect();
    let mut selected = KINDS.iter().position(|k| k.0 == app.electronics.card.kind).unwrap_or(0);
    let top = ui.cursor().min;
    let width = ui.available_width();
    ui_kit::segmented(ui, &mut selected, &options, ui_kit::Segments { dense: true, proportional: true, ..Default::default() });
    for (i, k) in KINDS.iter().enumerate() {
        let quarter = Rect::from_min_size(top + vec2(width * i as f32 / 4.0, 0.0), vec2(width / 4.0, ui_kit::row_height()));
        crate::scenario::publish_target(&format!("electronics:library:{}", k.1), quarter);
    }
    let Some(&(kind, key, _, new_label, new_command)) = KINDS.get(selected) else { return };
    app.electronics.card.kind = kind;
    ui.add_space(t.space_6);
    // The search, and a + for a new item of this kind.
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), ui_kit::row_height()), Sense::hover());
    let plus = Rect::from_min_max(pos2(row.right() - row.height(), row.top()), row.max);
    let field = Rect::from_min_max(row.min, pos2((plus.left() - t.space_6).max(row.left()), row.bottom()));
    let hint = tr("electronics.library_search");
    let mut search = ui.new_child(egui::UiBuilder::new().max_rect(field).id_salt("electronics_library_search"));
    ui_kit::search_field(&mut search, egui::Id::new("electronics_library_search"), &mut app.electronics.card.search, &hint);
    let new_label = tr(new_label);
    if ui_kit::icon_button_at(ui, plus, "plus", &new_label).clicked() {
        if kind == Kind::Packages {
            crate::electronics_editor::show_window(app, Open::NewPackage);
        } else {
            app.start(new_command);
        }
    }
    crate::scenario::publish_target(&format!("electronics:library:new:{key}"), plus);
    ui.add_space(t.space_6);
    let rows = rows(app, kind, &app.electronics.card.search);
    let opened = app.electronics.opened;
    let list_height = (ui.available_height() - PREVIEW_HEIGHT - t.space_12).max(ui_kit::row_height() * 3.0);
    let mut clicked = None;
    egui::ScrollArea::vertical().id_salt("electronics_library_rows").max_height(list_height).auto_shrink([false, true]).show(ui, |ui| {
        if rows.is_empty() {
            let note = if app.electronics.card.search.trim().is_empty() { "electronics.library_none" } else { "electronics.library_no_match" };
            ui_kit::text(ui, &tr(note), ui_kit::Text::Note(note));
        }
        for row in &rows {
            let response = ui_kit::text(ui, &row.name, ui_kit::Text::NameCell(opened == Some(row.item)));
            crate::scenario::publish_target(&format!("electronics:item:{key}:{}", row.name), response.rect);
            ui_kit::text(ui, &row.detail, ui_kit::Text::Note("electronics.library_row"));
            if response.clicked() {
                clicked = Some(row.item);
            }
        }
    });
    if let Some(item) = clicked {
        open(app, item);
    }
    // A picture of the open item, when it is of this kind.
    if let Some(item) = app.electronics.opened.filter(|o| o.kind() == kind) {
        ui.add_space(t.space_8);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), PREVIEW_HEIGHT), Sense::hover());
        preview(app, ui, item, rect);
        crate::scenario::publish_rect("electronics:library:preview", rect);
    }
}

/// `item` drawn small in `rect`: a symbol, a footprint, a component's symbol, a 3D package's model.
fn preview(app: &SolveApp, ui: &Ui, item: Opened, rect: Rect) {
    let t = Tokens::get();
    let view = Tokens::view();
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, t.radius_sm, t.viewport_bottom);
    let inner = rect.shrink(t.space_8);
    let ecad = &app.session.doc.ecad;
    match item {
        Opened::Component(id) => {
            if let Some(c) = ecad.component(id) {
                let symbol = ecad.symbol_of(c);
                if let Some(map) = PageMap::framing(symbol.bbox(), inner, 10.0) {
                    crate::electronics_components::paint_symbol(app, &painter, map, symbol, false);
                }
            }
        }
        Opened::Symbol(id) => {
            if let Some(symbol) = ecad.symbol(id)
                && let Some(map) = PageMap::framing(symbol.bbox(), inner, 10.0)
            {
                crate::electronics_components::paint_symbol(app, &painter, map, symbol, false);
            }
        }
        Opened::Footprint(id) => {
            if let Some(footprint) = ecad.footprint(id)
                && let Some(map) = PageMap::framing(footprint_extent(footprint), inner, 3.0)
            {
                crate::electronics_components::paint_footprint(app, &painter, map, footprint, false);
            }
        }
        Opened::Package(id) => {
            if let Some(p) = ecad.package(id) {
                painter.text(rect.center(), Align2::CENTER_CENTER, model_line(app, p), FontId::proportional(view.font_small), view.text_dim);
            }
        }
    }
}

/// A footprint's pads and graphics as one box.
fn footprint_extent(footprint: &FootprintItem) -> Option<[[f64; 2]; 2]> {
    let boxes = [footprint.copper_bbox(), solidcraft_engine::doc::ecad::graphics::graphics_bbox(&footprint.graphics)];
    solidcraft_engine::doc::ecad::bbox(boxes.into_iter().flatten().flatten())
}

/// The page for a component, a 3D package, or nothing open.
pub(crate) fn show(app: &mut SolveApp, ui: &mut Ui) {
    let rect = ui.max_rect();
    if !rect.is_positive() {
        return;
    }
    let response = ui.interact(rect, ui.id().with("electronics_item_page"), Sense::click());
    app.viewport.rect = Some(rect);
    crate::viewport::refresh_layout(app, ui.ctx().pixels_per_point());
    let t = Tokens::get();
    ui.painter().with_clip_rect(rect.intersect(ui.clip_rect())).rect_filled(rect, 0.0, t.viewport_bottom);
    match app.electronics.opened {
        Some(Opened::Component(_)) => component_page(app, ui, rect, &response),
        Some(Opened::Package(id)) => package_page(app, ui, rect, id),
        _ => empty_page(app, ui, rect),
    }
    crate::scenario::publish_target("electronics:page", rect);
}

/// Nothing open: what to do, and a New Component button.
fn empty_page(app: &mut SolveApp, ui: &mut Ui, rect: Rect) {
    let view = Tokens::view();
    let empty = app.session.doc.ecad.components.is_empty()
        && app.session.doc.ecad.symbols.is_empty()
        && app.session.doc.ecad.footprints.is_empty()
        && app.session.doc.ecad.packages.is_empty();
    let note = tr(if empty { "electronics.library_empty" } else { "electronics.library_pick" });
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    crate::electronics_workspace::page_note(app, &painter, rect, note);
    let content = crate::shell::content_rect(app, rect);
    let width = Tokens::get().form_card_width * 0.6;
    let at = Rect::from_center_size(pos2(content.center().x, content.top() + view.space_24 * 3.0), vec2(width, ui_kit::row_height()));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(at).id_salt("electronics_library_start"));
    let label = tr("electronics.new_component");
    let response = ui_kit::button(&mut child, &label, ui_kit::Button::Primary);
    crate::scenario::publish_target("electronics:library:start", response.rect);
    if response.clicked() {
        app.start("ecad.component.create");
    }
}

/// A 3D package: its name and its model, which the Inspector changes.
fn package_page(app: &SolveApp, ui: &Ui, rect: Rect, id: u64) {
    let Some(p) = app.session.doc.ecad.package(id) else { return };
    let view = Tokens::view();
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let centre = crate::shell::content_rect(app, rect).center();
    painter.text(centre - vec2(0.0, view.space_12), Align2::CENTER_BOTTOM, &p.name, FontId::proportional(view.font_heading), view.text);
    painter.text(centre + vec2(0.0, view.space_4), Align2::CENTER_TOP, model_line(app, p), FontId::proportional(view.font_body), view.text_dim);
}

/// A component: its symbol on the left half, its chosen variant's footprint on the right, a choice
/// of variant over the footprint. A click on a pin starts connecting it; then a click on a pad
/// connects or disconnects it (the Connect window lists the same).
fn component_page(app: &mut SolveApp, ui: &mut Ui, rect: Rect, response: &egui::Response) {
    let Some(c) = shown_copy(app) else { return };
    let t = Tokens::get();
    let view = Tokens::view();
    let content = crate::shell::content_rect(app, rect);
    let content = if content.is_positive() { content } else { rect };
    let head = content.top() + ui_kit::row_height() + view.space_12 * 2.0;
    let middle = content.center().x;
    let left = Rect::from_min_max(pos2(content.left() + view.space_16, head), pos2(middle - view.space_8, content.bottom() - view.space_16));
    let right = Rect::from_min_max(pos2(middle + view.space_8, head), pos2(content.right() - view.space_16, content.bottom() - view.space_16));
    if !(left.is_positive() && right.is_positive()) {
        return;
    }
    let symbol_map = PageMap::framing(c.symbol.bbox(), left, 10.0);
    let footprint_map = PageMap::framing(footprint_extent(&c.footprint), right, 3.0);
    // The pointer: a pin starts connecting, a pad connects to the pin being connected.
    let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| response.hovered() && !crate::shell::covered(app, *p));
    let pin = pointer.zip(symbol_map).filter(|(p, _)| left.contains(*p)).and_then(|(p, map)| crate::electronics_editor::pin_at(&c.symbol, map, p));
    let pad =
        pointer.zip(footprint_map).filter(|(p, _)| right.contains(*p)).and_then(|(p, map)| crate::electronics_editor::pad_at(&c.footprint, map, p));
    if pin.is_some() || (pad.is_some() && app.electronics.editor.connect.is_some()) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if response.clicked() {
        if let Some(pin) = pin {
            app.electronics.editor.connect = Some(pin);
            crate::electronics_editor::show_window(app, Open::Connect);
        } else if let (Some(pad), Some(pin)) = (pad, app.electronics.editor.connect.clone()) {
            crate::electronics_editor::toggle_pad(app, &c, &pin, &pad);
        }
    }
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.vline(middle, egui::Rangef::new(content.top() + view.space_16, content.bottom() - view.space_16), Stroke::new(1.0, t.border));
    let half = |r: Rect| painter.with_clip_rect(r.expand(view.space_8).intersect(ui.clip_rect()));
    // The latest copy, so a connection just made shows.
    let Some(now) = shown_copy(app) else { return };
    match symbol_map {
        Some(map) if !now.symbol.is_empty() => {
            if app.ui.show_grid {
                crate::electronics_workspace::paint_grid(&half(left), map, left, 2.54, &t);
            }
            crate::electronics_components::paint_symbol(app, &half(left), map, &now.symbol, true);
        }
        _ => centred_note(
            &painter,
            left,
            tr(if now.component.symbol.is_some() { "electronics.symbol_empty" } else { "electronics.component_no_symbol" }),
        ),
    }
    match footprint_map {
        Some(map) if !now.footprint.is_empty() => {
            if app.ui.show_grid {
                crate::electronics_workspace::paint_grid(&half(right), map, right, 1.0, &t);
            }
            crate::electronics_components::paint_footprint(app, &half(right), map, &now.footprint, true);
        }
        _ => {
            let has = now.variant.as_ref().is_some_and(|v| v.footprint.is_some());
            centred_note(&painter, right, tr(if has { "electronics.footprint_empty" } else { "electronics.variant_no_footprint" }));
        }
    }
    // Over the symbol its name; over the footprint the variant chooser.
    let caption = |r: Rect| Rect::from_min_max(pos2(r.left(), content.top() + view.space_12), pos2(r.right(), head - view.space_12));
    let name = if now.component.symbol.is_some() { now.symbol.name.clone() } else { tr("electronics.no_symbol") };
    let symbol_caption = crate::i18n::format("electronics.component_symbol", &[("name", &name)]);
    painter.text(caption(left).left_center(), Align2::LEFT_CENTER, symbol_caption, FontId::proportional(view.font_body), view.text_dim);
    variant_chooser(app, ui, &now, caption(right));
}

/// A dim note in the middle of `rect`.
fn centred_note(painter: &egui::Painter, rect: Rect, note: String) {
    let view = Tokens::view();
    painter.text(rect.center(), Align2::CENTER_CENTER, note, FontId::proportional(view.font_body), view.text_dim);
}

/// The open component's variants, to choose the one the page and the windows show.
fn variant_chooser(app: &mut SolveApp, ui: &mut Ui, c: &Shown, rect: Rect) {
    if c.component.variants.is_empty() {
        let view = Tokens::view();
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.text(rect.left_center(), Align2::LEFT_CENTER, tr("electronics.no_variants"), FontId::proportional(view.font_body), view.text_dim);
        return;
    }
    let names: Vec<String> = c.component.variants.iter().map(|v| v.name.clone()).collect();
    let at = c.variant.as_ref().and_then(|v| names.iter().position(|n| *n == v.name)).unwrap_or(0);
    let width = rect.width().min(Tokens::get().form_card_width);
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(rect.min, vec2(width, rect.height()))).id_salt("electronics_variant"));
    let label = tr("electronics.field_variant");
    if let Some(i) = crate::electronics_editor::choice(&mut child, "electronics_variant", &label, at, &names) {
        app.electronics.variant = names.get(i).cloned();
    }
}

/// The Inspector with nothing picked: the open item's properties, with a button to delete it.
pub(crate) fn properties(app: &mut SolveApp, ui: &mut Ui) {
    let Some(item) = app.electronics.opened else {
        ui_kit::text(ui, &tr("electronics.inspector_nothing"), ui_kit::Text::Note("electronics.inspector_nothing"));
        return;
    };
    let title = tr("electronics.section_properties");
    match item {
        Opened::Component(_) => {
            let Some(c) = shown_copy(app) else { return };
            ui_kit::section(ui, "electronics_properties", &title, true, |ui| {
                crate::electronics_editor::component_properties(app, ui, &c);
                symbol_row(app, ui, &c);
            });
            let variants = tr("electronics.section_variants");
            ui_kit::section(ui, "electronics_variants", &variants, true, |ui| variant_rows(app, ui, &c));
        }
        Opened::Symbol(id) => {
            let Some(s) = app.session.doc.ecad.symbol(id).cloned() else { return };
            let users = app.session.doc.ecad.symbol_users(id).count();
            ui_kit::section(ui, "electronics_properties", &title, true, |ui| {
                name_and_description(app, ui, "ecad.symbol.edit", json!({"symbol": id}), &s.name, &s.description);
                used_by(ui, users);
            });
        }
        Opened::Footprint(id) => {
            let Some(f) = app.session.doc.ecad.footprint(id).cloned() else { return };
            let users = app.session.doc.ecad.footprint_users(id).count();
            ui_kit::section(ui, "electronics_properties", &title, true, |ui| {
                name_and_description(app, ui, "ecad.footprint.edit", json!({"footprint": id}), &f.name, &f.description);
                used_by(ui, users);
            });
        }
        Opened::Package(id) => {
            let Some(p) = app.session.doc.ecad.package(id).cloned() else { return };
            let users = app.session.doc.ecad.package_users(id).count();
            ui_kit::section(ui, "electronics_properties", &title, true, |ui| {
                package_fields(app, ui, &p);
                used_by(ui, users);
            });
        }
    }
    let delete = tr("electronics.delete_item");
    if crate::electronics_editor::button(ui, "delete-item", &delete, true) {
        let (command, params) = match item {
            Opened::Component(id) => ("ecad.component.delete", json!({"components": [id]})),
            Opened::Symbol(id) => ("ecad.symbol.delete", json!({"symbol": id})),
            Opened::Footprint(id) => ("ecad.footprint.delete", json!({"footprint": id})),
            Opened::Package(id) => ("ecad.package.delete", json!({"package": id})),
        };
        // An item a component uses is refused: the status line says which.
        drop(app.run(command, params));
    }
}

/// How many use the open symbol, footprint or 3D package.
fn used_by(ui: &mut Ui, n: usize) {
    let text = crate::i18n::format("electronics.used_by", &[("n", &n.to_string())]);
    ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.used_by"));
}

/// A symbol's or a footprint's name and description, each changed with `command` on `on`.
fn name_and_description(app: &mut SolveApp, ui: &mut Ui, command: &str, on: Value, name: &str, description: &str) {
    let id = on.as_object().and_then(|o| o.values().next()).and_then(Value::as_u64).unwrap_or(0);
    for (key, label, current) in [("name", "electronics.field_name", name), ("description", "electronics.field_description", description)] {
        if let Some(text) = crate::electronics_editor::field(app, ui, &format!("{id}:{key}"), &tr(label), current) {
            let mut params = on.clone();
            params[key] = json!(text);
            drop(app.run(command, params));
        }
    }
}

/// The symbol the open component shows, from the library's.
fn symbol_row(app: &mut SolveApp, ui: &mut Ui, c: &Shown) {
    let symbols: Vec<(u64, String)> = app.session.doc.ecad.symbols.iter().map(|s| (s.id, s.name.clone())).collect();
    let mut options = vec![tr("electronics.package_none")];
    options.extend(symbols.iter().map(|(_, name)| name.clone()));
    let at = c.component.symbol.and_then(|id| symbols.iter().position(|(s, _)| *s == id)).map_or(0, |i| i + 1);
    let id = c.component.id;
    if let Some(i) = crate::electronics_editor::choice(ui, &format!("{id}:symbol"), &tr("electronics.field_symbol"), at, &options) {
        let symbol = symbols.get(i.wrapping_sub(1)).map_or(Value::Null, |(s, _)| json!(s));
        drop(app.run("ecad.component.edit", json!({"component": id, "symbol": symbol})));
    }
}

/// The open component's variants: a click chooses one; the chosen one's name, footprint and 3D
/// package can change, and it can go.
fn variant_rows(app: &mut SolveApp, ui: &mut Ui, c: &Shown) {
    let id = c.component.id;
    let chosen = c.variant.as_ref().map(|v| v.name.clone());
    for v in &c.component.variants {
        if ui_kit::list_picker(ui, &format!("variant:{}", v.name), &v.name, chosen.as_deref() == Some(v.name.as_str())).clicked() {
            app.electronics.variant = Some(v.name.clone());
        }
    }
    if let Some(v) = &c.variant {
        let ecad = &app.session.doc.ecad;
        let footprints: Vec<(u64, String)> = ecad.footprints.iter().map(|f| (f.id, f.name.clone())).collect();
        let packages: Vec<(u64, String)> = ecad.packages.iter().map(|p| (p.id, p.name.clone())).collect();
        let none = tr("electronics.package_none");
        let key = |f: &str| format!("{id}:variant:{}:{f}", v.name);
        if let Some(name) = crate::electronics_editor::field(app, ui, &key("name"), &tr("electronics.field_variant_name"), &v.name)
            && app.run("ecad.component.variant.edit", json!({"component": id, "variant": v.name, "name": name})).is_ok()
        {
            app.electronics.variant = Some(name);
        }
        for (field, label, items, current) in [
            ("footprint", "electronics.section_footprint", &footprints, v.footprint),
            ("package", "electronics.section_package", &packages, v.package),
        ] {
            let mut options = vec![none.clone()];
            options.extend(items.iter().map(|(_, name)| name.clone()));
            let at = current.and_then(|id| items.iter().position(|(x, _)| *x == id)).map_or(0, |i| i + 1);
            if let Some(i) = crate::electronics_editor::choice(ui, &key(field), &tr(label), at, &options) {
                let chosen = items.get(i.wrapping_sub(1)).map_or(Value::Null, |(x, _)| json!(x));
                let mut params = json!({"component": id, "variant": v.name});
                params[field] = chosen;
                drop(app.run("ecad.component.variant.edit", params));
            }
        }
        if crate::electronics_editor::button(ui, "delete-variant", &tr("electronics.delete_variant"), true) {
            drop(app.run("ecad.component.variant.delete", json!({"component": id, "variant": v.name})));
        }
    }
    if crate::electronics_editor::button(ui, "add-variant", &tr("electronics.add_variant"), true) {
        add_variant(app);
    }
}

/// A 3D package's name, model, offset and turn.
fn package_fields(app: &mut SolveApp, ui: &mut Ui, p: &PackageItem) {
    let id = p.id;
    let key = |f: &str| format!("{id}:package:{f}");
    if let Some(name) = crate::electronics_editor::field(app, ui, &key("name"), &tr("electronics.field_name"), &p.name) {
        drop(app.run("ecad.package.edit", json!({"package": id, "name": name})));
    }
    let models: Vec<(u64, String)> = app.session.doc.components.iter().map(|m| (m.id, m.name.clone())).collect();
    let options: Vec<String> = models.iter().map(|(_, name)| name.clone()).collect();
    match models.iter().position(|(m, _)| *m == p.model) {
        Some(at) => {
            if let Some(i) = crate::electronics_editor::choice(ui, &key("model"), &tr("electronics.field_model"), at, &options)
                && let Some((model, _)) = models.get(i)
            {
                drop(app.run("ecad.package.edit", json!({"package": id, "model": model})));
            }
        }
        None => {
            ui_kit::text(ui, &tr("electronics.problem_package_missing"), ui_kit::Text::Error("electronics.problem"));
        }
    }
    let mut moved = None;
    for (i, (f, label)) in [("x", "electronics.field_x"), ("y", "electronics.field_y"), ("z", "electronics.field_lift")].into_iter().enumerate() {
        if let Some(v) = crate::electronics_editor::number(app, ui, &key(f), &tr(label), p.translate[i]) {
            let mut t = p.translate;
            t[i] = v;
            moved = Some((t, p.rotation));
        }
    }
    if let Some(r) = crate::electronics_editor::number(app, ui, &key("rotation"), &tr("electronics.field_rotation"), p.rotation) {
        moved = Some((p.translate, r));
    }
    if let Some((t, r)) = moved {
        drop(app.run("ecad.package.edit", json!({"package": id, "translate": t, "rotation": r})));
    }
}

/// The Symbol window: the symbol the open component shows, or a new one named after it.
pub(crate) fn choose_symbol(app: &mut SolveApp, ui: &mut Ui, c: &Shown) {
    let symbols: Vec<(u64, String, usize)> = app.session.doc.ecad.symbols.iter().map(|s| (s.id, s.name.clone(), s.pins.len())).collect();
    let id = c.component.id;
    if symbols.is_empty() {
        ui_kit::text(ui, &tr("electronics.no_symbols"), ui_kit::Text::Note("electronics.no_symbols"));
    }
    let mut chosen = None;
    for (symbol, name, pins) in &symbols {
        let text = format!("{name}  ({})", crate::i18n::format("electronics.count_pins", &[("n", &pins.to_string())]));
        if ui_kit::list_picker(ui, &format!("symbol:{name}"), &text, c.component.symbol == Some(*symbol)).clicked() {
            chosen = Some(*symbol);
        }
    }
    if let Some(symbol) = chosen {
        if let Ok(reply) = app.run("ecad.component.edit", json!({"component": id, "symbol": symbol})) {
            app.electronics.editor.open = None;
            crate::electronics_editor::detached_status(app, &reply);
        }
        return;
    }
    if crate::electronics_editor::button(ui, "new-symbol", &tr("electronics.new_symbol_for"), true) {
        let name = crate::electronics_editor::free_item_name(app, &c.component.name, true);
        drop(app.run("ecad.symbol.create", json!({"name": name, "component": id})));
    }
}

/// The New 3D Package window: a model from this library's components makes the package.
pub(crate) fn new_package(app: &mut SolveApp, ui: &mut Ui) {
    let models: Vec<(u64, String)> = app.session.doc.components.iter().map(|m| (m.id, m.name.clone())).collect();
    if !models.is_empty() {
        ui_kit::text(ui, &tr("electronics.new_package_help"), ui_kit::Text::Note("electronics.new_package_help"));
    }
    let mut chosen = None;
    for (model, name) in &models {
        if ui_kit::list_picker(ui, &format!("model:{name}"), name, false).clicked() {
            chosen = Some((*model, name.clone()));
        }
    }
    if let Some((model, name)) = chosen {
        let taken: Vec<String> = app.session.doc.ecad.packages.iter().map(|p| p.name.clone()).collect();
        let name = crate::electronics_editor::free_from(&name, &taken);
        if app.run("ecad.package.create", json!({"name": name, "model": model})).is_ok() {
            app.electronics.editor.open = None;
        }
        return;
    }
    import_model(app, ui);
}

/// What a 3D package's model is, and a button to import one from a STEP file into the library.
pub(crate) fn import_model(app: &mut SolveApp, ui: &mut Ui) {
    if app.session.doc.components.is_empty() {
        ui_kit::text(ui, &tr("electronics.package_import_help"), ui_kit::Text::Note("electronics.package_import_help"));
    }
    if crate::electronics_editor::button(ui, "import-model", &tr("electronics.import_model"), true) {
        app.start("file.insert_step");
    }
}
