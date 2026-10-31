//! The Electronics workspace: an electronics document's own UI over its electronics
//! (`doc.ecad`, the `ecad.*` commands), on a flat 2D page. An electronics library shows the item
//! the Browser's Library card opened (`electronics_library`), an electronics design its
//! schematic (`electronics_schematic`) or its board (`electronics_board`), as its SWITCH group
//! chose; a 3D design never shows it. The page has its own top-down
//! camera, so the shared navigation pans and zooms it (never orbits) and the Design camera stays
//! where it was.

use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, pos2};
use solidcraft_engine::doc::DocumentKind;
use solidcraft_engine::render::{Camera, StandardView, UpAxis};

use crate::SolveApp;
use crate::electronics_library::Opened;
use crate::theme::Tokens;

/// An electronics library's tab: new items and the generators.
pub const LIBRARY: &str = "LIBRARY";
/// The open symbol's editor tab.
pub const SYMBOL: &str = "SYMBOL";
/// The open footprint's editor tab.
pub const FOOTPRINT: &str = "FOOTPRINT";
/// The open electronic component's editor tab.
pub const COMPONENT: &str = "COMPONENT";
/// An electronics design's tab: its views, and the schematic's tools.
pub const DESIGN: &str = "DESIGN";
/// An electronics design's tab on its board: its views, and the board's tools. Shown as Design
/// (`workspace::tab_label`).
pub const BOARD: &str = "BOARD DESIGN";
/// An electronics design's tab on its board for its documentation: the layer stack and the DRAW
/// tools. Shown as Document (`workspace::tab_label`).
pub const BOARD_DOCUMENT: &str = "BOARD DOCUMENT";
/// An electronics design's tab on its board for its rules: the design rules and net classes, the
/// design rule check and keepouts. Shown as Rules (`workspace::tab_label`).
pub const BOARD_RULES: &str = "BOARD RULES";
/// An electronics design's checks: the electrical rules check, and the netlist (shown and exported).
pub const CHECK: &str = "CHECK";
/// Every toolbar tab of the electronics documents.
pub const TABS: &[&str] = &[LIBRARY, SYMBOL, FOOTPRINT, COMPONENT, DESIGN, BOARD, BOARD_DOCUMENT, BOARD_RULES, CHECK];

/// Half the width of the page Fit and Home frame while it is empty, in millimetres.
const EMPTY_HALF_EXTENT: f64 = 50.0;

/// More grid lines than this across the page are not drawn.
const MAX_GRID_LINES: i64 = 400;

/// The page's own camera: top-down and orthographic, in page millimetres (x right, y up).
pub struct PageView {
    pub cam: Camera,
    /// The camera frames the page; cleared by Fit and Home, and when another item opens.
    pub fitted: bool,
}

impl Default for PageView {
    fn default() -> Self {
        let mut cam = Camera::default();
        cam.set_view(StandardView::Top);
        cam.fov = 0.0;
        cam.up = UpAxis::Z;
        PageView { cam, fitted: false }
    }
}

/// One electronics document's view state; each tab keeps its own (`documents`).
#[derive(Default)]
pub struct State {
    pub page: PageView,
    /// The library item the page shows (in an electronics library).
    pub opened: Option<Opened>,
    /// The open electronic component's variant the page shows, by name; its first when `None`.
    pub variant: Option<String>,
    /// The open item's drawing the editor works on: a symbol's or a footprint's.
    pub view: crate::electronics_components::View,
    /// The Browser's Library card: its kind of item and search.
    pub card: crate::electronics_library::Card,
    /// The Inspector's pick, drag and typing.
    pub editor: crate::electronics_editor::Editor,
    /// The schematic's tools and pick.
    pub sheet: crate::electronics_schematic::Sheet,
    /// The board's tools, pick and layers.
    pub board: crate::electronics_board::BoardView,
    /// An electronics design's windows.
    pub design: crate::electronics_design::Design,
    /// The toolbar tab the document showed last, to come back to.
    pub tab: String,
    /// The item the camera framed last: another one is framed afresh.
    framed: Option<(Opened, bool)>,
}

/// The document is an electronics one and shows its own UI (nothing needs Design's view).
pub fn active(app: &SolveApp) -> bool {
    app.session.doc.kind.is_electronics() && TABS.contains(&app.ui.tab.as_str())
}

/// The page shows an electronics design's schematic.
pub fn schematic(app: &SolveApp) -> bool {
    active(app)
        && app.session.doc.kind == DocumentKind::ElectronicsDesign
        && app.electronics.design.view == crate::electronics_design::View::Schematic
}

/// The page shows an electronics design's board.
pub fn board(app: &SolveApp) -> bool {
    active(app) && app.session.doc.kind == DocumentKind::ElectronicsDesign && app.electronics.design.view == crate::electronics_design::View::Board
}

/// The document's toolbar tabs: LIBRARY and the open item's editor tab in a library, DESIGN (the
/// schematic's or the board's, the board's with DOCUMENT and RULES) and CHECK in a design; none in
/// a 3D design.
pub(crate) fn tabs(app: &SolveApp) -> Vec<&'static str> {
    match app.session.doc.kind {
        DocumentKind::ElectronicsLibrary => {
            let mut tabs = vec![LIBRARY];
            tabs.extend(app.electronics.opened.and_then(Opened::tab));
            tabs
        }
        DocumentKind::ElectronicsDesign if app.electronics.design.view == crate::electronics_design::View::Board => {
            vec![BOARD, BOARD_DOCUMENT, BOARD_RULES, CHECK]
        }
        DocumentKind::ElectronicsDesign => vec![DESIGN, CHECK],
        DocumentKind::Design => Vec::new(),
    }
}

/// The status line after a command `id` that worked (`reply` its answer) in an electronics
/// document, where the command writes nothing of its own: what Undo or Redo took back or did
/// again, the file a Save wrote; any other edit clears what was there, so an earlier message
/// never reads as this one's (a tool that says something says it after this).
pub(crate) fn finished(app: &mut SolveApp, id: &str, reply: &serde_json::Value) {
    if !active(app) {
        return;
    }
    let text = match id {
        "edit.undo" | "edit.redo" => {
            let (field, key, plain) = if id == "edit.undo" {
                ("undone", "electronics.undone", "electronics.undone_plain")
            } else {
                ("redone", "electronics.redone", "electronics.redone_plain")
            };
            match reply.get(field).and_then(serde_json::Value::as_str).filter(|l| !l.is_empty()) {
                Some(label) => crate::i18n::format(key, &[("what", app.tr_label(label))]),
                None => crate::i18n::tr(plain).to_string(),
            }
        }
        "file.save" | "file.save_as" => match app.session.path.clone() {
            Some(path) => crate::i18n::format("ui.saved_to", &[("path", &path)]),
            None => return,
        },
        _ => {
            if id.starts_with("ecad.") && solidcraft_engine::find_command(id).is_some_and(|c| c.undoable) {
                app.status = None;
            }
            return;
        }
    };
    app.set_status(text, false);
}

/// Whether the toolbar shows `id` pressed for what the page shows: the SWITCH group's view.
pub(crate) fn pressed(app: &SolveApp, id: &str) -> bool {
    (id == "ecad.view.schematic" && schematic(app)) || (id == "ecad.view.board" && board(app))
}

/// Each frame in an electronics document while nothing needs Design's view: forget an open item
/// that is gone, and show one of the document's tabs, the one it showed last when it can.
pub(crate) fn sync_tab(app: &mut SolveApp) {
    crate::electronics_library::settle(app);
    let tabs = tabs(app);
    if !tabs.contains(&app.ui.tab.as_str()) {
        let back = tabs.iter().find(|t| **t == app.electronics.tab).or(tabs.first()).copied().unwrap_or(LIBRARY);
        app.ui.tab = back.into();
    }
    app.electronics.tab.clone_from(&app.ui.tab);
}

/// A sketch, tool, dialog, file load or a Browser panel that works in the 3D view (Move, Redefine
/// Sketch Plane, a canvas) is open: it needs Design's view.
pub(crate) fn modelling(app: &SolveApp) -> bool {
    app.session.active_sketch.is_some()
        || app.tool.is_some()
        || app.dialog.is_some()
        || app.file_load.busy()
        || app.tree.occurrence_move.is_some()
        || app.tree.redefine.is_some()
        || app.tree.canvas_panel.is_some()
}

/// Forget the page, the pick and the open item: the document was replaced by a new or opened one.
pub fn reset(app: &mut SolveApp) {
    app.electronics = State::default();
}

/// The page, in the central area, with the status line's toast over it (what a command or a
/// tool says, a refusal in the error style), as the 3D view shows it.
pub fn show(app: &mut SolveApp, ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    page(app, ui);
    if rect.is_positive() {
        crate::viewport::page_toasts(app, ui, rect);
    }
}

/// The page the document shows: its schematic, its board, or the open library item.
fn page(app: &mut SolveApp, ui: &mut egui::Ui) {
    if schematic(app) {
        return crate::electronics_schematic::show(app, ui);
    }
    if board(app) {
        return crate::electronics_board::show(app, ui);
    }
    // A component, a 3D package or nothing open has a page of its own.
    if !matches!(app.electronics.opened, Some(Opened::Symbol(_) | Opened::Footprint(_))) {
        return crate::electronics_library::show(app, ui);
    }
    let rect = ui.max_rect();
    if !rect.is_positive() {
        return;
    }
    let response = ui.interact(rect, ui.id().with("electronics_page"), Sense::click_and_drag());
    app.viewport.rect = Some(rect);
    crate::viewport::refresh_layout(app, ui.ctx().pixels_per_point());
    if !app.electronics.page.fitted || app.electronics.framed != crate::electronics_components::framing(app) {
        fit(app, rect);
    }
    // A press on a pin or a pad picks or drags it; the rest of the pointer navigates the page.
    if !crate::electronics_editor::page_input(app, ui, rect, &response) {
        with_page_camera(app, |app| {
            crate::navigation::handle(app, ui, rect, &response, crate::navigation::NavCaps { orbit: false, pick: false });
        });
    }
    paint(app, ui, rect);
    crate::scenario::publish_target("electronics:page", rect);
}

/// Run `navigate` with the page's camera as `app.cam`, and put the Design camera, its pending
/// animation, fit and view request back afterwards, so nothing the page does moves the model.
pub(crate) fn with_page_camera(app: &mut SolveApp, navigate: impl FnOnce(&mut SolveApp)) {
    let design = std::mem::replace(&mut app.cam, app.electronics.page.cam);
    let perspective = std::mem::replace(&mut app.ui.perspective, false);
    let up = app.ui.up_axis;
    let animation = app.cam_anim.take();
    let pending = app.pending_fit.take();
    let request = app.session.camera.take();
    let free = app.viewport.orbit_free.take();
    let spin = std::mem::replace(&mut app.viewport.spin, false);
    navigate(app);
    app.electronics.page.cam = std::mem::replace(&mut app.cam, design);
    app.ui.perspective = perspective;
    app.ui.up_axis = up;
    app.cam_anim = animation;
    app.pending_fit = pending;
    app.session.camera = request;
    app.viewport.orbit_free = free;
    app.viewport.spin = spin;
    // The pointer is over the page, not over the model.
    app.viewport.mouse = None;
}

/// Frame the shown drawing, or the empty page around its origin, in the part of `rect` the panels
/// leave free.
fn fit(app: &mut SolveApp, rect: Rect) {
    let content = crate::shell::content_rect(app, rect);
    let area = if content.is_positive() { content } else { rect };
    let [[x0, y0], [x1, y1]] = crate::electronics_components::bounds(app).unwrap_or([[-EMPTY_HALF_EXTENT; 2], [EMPTY_HALF_EXTENT; 2]]);
    // A margin around the drawing, and never closer than a 10 mm square.
    let (w, h) = (((x1 - x0) * 1.5).max(10.0), ((y1 - y0) * 1.5).max(10.0));
    let scale = (f64::from(area.width()) / w).min(f64::from(area.height()) / h);
    if !(scale.is_finite() && scale > 0.0) {
        return;
    }
    let cam = &mut app.electronics.page.cam;
    cam.target.x = (x0 + x1) / 2.0 - f64::from(area.center().x - rect.center().x) / scale;
    cam.target.y = (y0 + y1) / 2.0 + f64::from(area.center().y - rect.center().y) / scale;
    cam.target.z = 0.0;
    cam.distance = f64::from(rect.height()) / (2.0 * scale) / 0.4;
    app.electronics.page.fitted = true;
    app.electronics.framed = crate::electronics_components::framing(app);
}

/// Page millimetres to screen points for the page's camera over `rect`.
#[derive(Clone, Copy)]
pub(crate) struct PageMap {
    scale: f64,
    ox: f64,
    oy: f64,
}

impl PageMap {
    pub(crate) fn new(cam: &Camera, rect: Rect) -> Option<PageMap> {
        let scale = f64::from(rect.height()) / (2.0 * cam.half_height());
        let ox = f64::from(rect.center().x) - cam.target.x * scale;
        let oy = f64::from(rect.center().y) + cam.target.y * scale;
        (scale.is_finite() && scale > 0.0 && ox.is_finite() && oy.is_finite()).then_some(PageMap { scale, ox, oy })
    }

    /// The map that frames `bounds` (page millimetres) in `rect`, with a margin around it and
    /// never closer than a `least` millimetre square: for a page with no camera of its own.
    pub(crate) fn framing(bounds: Option<[[f64; 2]; 2]>, rect: Rect, least: f64) -> Option<PageMap> {
        let [[x0, y0], [x1, y1]] = bounds.unwrap_or([[-least / 2.0; 2], [least / 2.0; 2]]);
        let (w, h) = (((x1 - x0) * 1.4).max(least), ((y1 - y0) * 1.4).max(least));
        let scale = (f64::from(rect.width()) / w).min(f64::from(rect.height()) / h);
        let ox = f64::from(rect.center().x) - (x0 + x1) / 2.0 * scale;
        let oy = f64::from(rect.center().y) + (y0 + y1) / 2.0 * scale;
        (scale.is_finite() && scale > 0.0 && ox.is_finite() && oy.is_finite()).then_some(PageMap { scale, ox, oy })
    }

    pub(crate) fn to_screen(self, x: f64, y: f64) -> Pos2 {
        pos2((self.ox + x * self.scale) as f32, (self.oy - y * self.scale) as f32)
    }

    /// Points per millimetre.
    pub(crate) fn scale(self) -> f64 {
        self.scale
    }

    pub(crate) fn to_page(self, p: Pos2) -> (f64, f64) {
        ((f64::from(p.x) - self.ox) / self.scale, (self.oy - f64::from(p.y)) / self.scale)
    }
}

fn paint(app: &SolveApp, ui: &egui::Ui, rect: Rect) {
    let t = Tokens::get();
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 0.0, t.viewport_bottom);
    let cam = app.electronics.page.cam;
    if let Some(map) = PageMap::new(&cam, rect) {
        if app.ui.show_grid {
            // The grid points snap to: 0.05 in steps on a symbol (every 0.1 in strong), millimetres
            // on a footprint (every fifth strong).
            let step = crate::electronics_drawing::grid_step(app);
            let major = if app.electronics.view == crate::electronics_components::View::Symbol && step < 2.0 { 2 } else { 5 };
            grid(&painter, map, rect, step, major, &t);
        }
        let origin = map.to_screen(0.0, 0.0);
        let [x_axis, y_axis, _] = t.axes();
        if origin.y.is_finite() && rect.y_range().contains(origin.y) {
            painter.hline(rect.x_range(), origin.y, Stroke::new(1.4, x_axis));
        }
        if origin.x.is_finite() && rect.x_range().contains(origin.x) {
            painter.vline(origin.x, rect.y_range(), Stroke::new(1.4, y_axis));
        }
        crate::electronics_components::paint(app, &painter, map);
    }
    if let Some(note) = crate::electronics_components::empty_note(app) {
        page_note(app, &painter, rect, note);
    }
}

/// A short note at the top of the page's free area, on the page's own colour so it reads over the
/// grid and the axes.
pub(crate) fn page_note(app: &SolveApp, painter: &egui::Painter, rect: Rect, note: String) {
    let t = Tokens::get();
    let view = Tokens::view();
    let content = crate::shell::content_rect(app, rect);
    let galley = painter.layout_no_wrap(note, FontId::proportional(view.font_body), view.text_dim);
    let note = Align2::CENTER_TOP.anchor_size(pos2(content.center().x, content.top() + view.space_24), galley.size());
    painter.rect_filled(note.expand2(egui::vec2(view.space_8, view.space_4)), view.radius_sm, t.viewport_bottom);
    painter.galley(note.min, galley, view.text_dim);
}

/// The page's grid at `step`: on the 0.05 in grid every 0.1 in line strong, otherwise every fifth.
pub(crate) fn paint_grid(painter: &egui::Painter, map: PageMap, rect: Rect, step: f64, t: &Tokens) {
    grid(painter, map, rect, step, if step < 2.0 { 2 } else { 5 }, t);
}

/// Minor lines every `step` page millimetres, every `major`th one major.
fn grid(painter: &egui::Painter, map: PageMap, rect: Rect, step: f64, major: i64, t: &Tokens) {
    if !(step.is_finite() && step > 0.0) {
        return;
    }
    let (left, top) = map.to_page(rect.left_top());
    let (right, bottom) = map.to_page(rect.right_bottom());
    let lines = |from: f64, to: f64| {
        let (first, last) = ((from.min(to) / step).floor(), (from.max(to) / step).ceil());
        (first.is_finite() && last.is_finite() && last - first <= MAX_GRID_LINES as f64).then_some(first as i64..=last as i64)
    };
    let (Some(columns), Some(rows)) = (lines(left, right), lines(bottom, top)) else { return };
    let stroke = |i: i64| if i.rem_euclid(major.max(1)) == 0 { Stroke::new(1.0, t.grid_major) } else { Stroke::new(0.7, t.grid) };
    for i in columns {
        painter.vline(map.to_screen(i as f64 * step, 0.0).x, rect.y_range(), stroke(i));
    }
    for i in rows {
        painter.hline(rect.x_range(), map.to_screen(0.0, i as f64 * step).y, stroke(i));
    }
}

#[cfg(test)]
#[path = "electronics_workspace_tests.rs"]
mod tests;
