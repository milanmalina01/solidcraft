//! The Electronics workspace: the design's electronics (`doc.ecad`, the `ecad.*` commands) on a
//! flat 2D page. The page has its own top-down camera, so the shared navigation pans and zooms it
//! (never orbits) and the Design camera stays where it was. Empty for now: the page shows its grid
//! and origin until the component library, schematic and board arrive.

use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, pos2};
use solidcraft_engine::render::{Camera, StandardView, UpAxis};

use crate::SolveApp;
use crate::theme::Tokens;

/// The ribbon tab the workspace shows (it has no tabs of its own yet).
pub const TAB: &str = "ELECTRONICS";

/// Half the width of the page Fit and Home frame while it is empty, in millimetres.
const EMPTY_HALF_EXTENT: f64 = 50.0;

/// More grid lines than this across the page are not drawn.
const MAX_GRID_LINES: i64 = 400;

/// The page's own camera: top-down and orthographic, in page millimetres (x right, y up).
pub struct PageView {
    pub cam: Camera,
    /// The camera frames the page; cleared by Fit and Home, and when the workspace opens.
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

#[derive(Default)]
pub struct State {
    pub page: PageView,
}

pub fn active(app: &SolveApp) -> bool {
    app.ui.tab == TAB
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

/// Open the workspace (`on`) or go back to Design. Opening it leaves Render first, and is refused
/// while a sketch, tool, dialog or file load is open, as Render's is.
pub fn enter(app: &mut SolveApp, on: bool) -> Result<(), String> {
    if !on {
        if active(app) {
            app.ui.tab = "MODEL".into();
        }
        return Ok(());
    }
    if modelling(app) {
        return Err(app.tr("electronics.finish_operation").to_string());
    }
    if crate::render_workspace::active(app) {
        crate::render_workspace::enter(app, false)?;
    }
    if !active(app) {
        app.ui.tab = TAB.into();
        app.electronics.page.fitted = false;
    }
    Ok(())
}

/// The page, in the central area.
pub fn show(app: &mut SolveApp, ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    if !rect.is_positive() {
        return;
    }
    let response = ui.interact(rect, ui.id().with("electronics_page"), Sense::click_and_drag());
    app.viewport.rect = Some(rect);
    crate::viewport::refresh_layout(app, ui.ctx().pixels_per_point());
    if !app.electronics.page.fitted {
        fit(app, rect);
    }
    with_page_camera(app, |app| {
        crate::navigation::handle(app, ui, rect, &response, crate::navigation::NavCaps { orbit: false, pick: false });
    });
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

/// Frame the empty page around its origin in the part of `rect` the panels leave free.
fn fit(app: &mut SolveApp, rect: Rect) {
    let content = crate::shell::content_rect(app, rect);
    let area = if content.is_positive() { content } else { rect };
    let scale = f64::from(area.width().min(area.height())) / (2.0 * EMPTY_HALF_EXTENT);
    if !(scale.is_finite() && scale > 0.0) {
        return;
    }
    let cam = &mut app.electronics.page.cam;
    cam.target.x = -f64::from(area.center().x - rect.center().x) / scale;
    cam.target.y = f64::from(area.center().y - rect.center().y) / scale;
    cam.target.z = 0.0;
    cam.distance = f64::from(rect.height()) / (2.0 * scale) / 0.4;
    app.electronics.page.fitted = true;
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

    pub(crate) fn to_screen(self, x: f64, y: f64) -> Pos2 {
        pos2((self.ox + x * self.scale) as f32, (self.oy - y * self.scale) as f32)
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
            grid(&painter, map, rect, solidcraft_engine::view::grid_step(cam.half_height()).0, &t);
        }
        let origin = map.to_screen(0.0, 0.0);
        let [x_axis, y_axis, _] = t.axes();
        if origin.y.is_finite() && rect.y_range().contains(origin.y) {
            painter.hline(rect.x_range(), origin.y, Stroke::new(1.4, x_axis));
        }
        if origin.x.is_finite() && rect.x_range().contains(origin.x) {
            painter.vline(origin.x, rect.y_range(), Stroke::new(1.4, y_axis));
        }
    }
    if !app.session.doc.ecad.has_content() {
        let content = crate::shell::content_rect(app, rect);
        let view = Tokens::view();
        let galley = painter.layout_no_wrap(app.tr("electronics.page_empty").to_string(), FontId::proportional(view.font_body), view.text_dim);
        let note = Align2::CENTER_TOP.anchor_size(pos2(content.center().x, content.top() + view.space_24), galley.size());
        // The page's own colour behind the note keeps it readable over the grid and the axes.
        painter.rect_filled(note.expand2(egui::vec2(view.space_8, view.space_4)), view.radius_sm, t.viewport_bottom);
        painter.galley(note.min, galley, view.text_dim);
    }
}

/// Minor lines every `step` page millimetres, every fifth one major.
fn grid(painter: &egui::Painter, map: PageMap, rect: Rect, step: f64, t: &Tokens) {
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
    let stroke = |i: i64| if i.rem_euclid(5) == 0 { Stroke::new(1.0, t.grid_major) } else { Stroke::new(0.7, t.grid) };
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
