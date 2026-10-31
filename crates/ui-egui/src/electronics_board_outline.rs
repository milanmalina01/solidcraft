//! The board's outline (see `electronics_board`): what shape it is (a rectangle, its corners
//! maybe rounded, a circle, or any polygon with arcs), the handles a picked outline shows at its
//! corners and the middles of its edges and the drags that resize it, the Outline tool's circle
//! and polygon modes (its rectangle is dragged out on the page), and the Inspector's section for a
//! picked outline. Each change is one `ecad.board.outline`.

use egui::{Pos2, Rect, Stroke, Ui, vec2};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::board::MAX_OUTLINE_POINTS;
use solidcraft_engine::doc::ecad::path::{self, Arc, is_arc, tessellate};

use crate::SolveApp;
use crate::electronics_board::{PICK_POINTS, Tool, arc_tolerance, board, segment_distance, snap, snap_tidy};
use crate::electronics_board_layers::Layer;
use crate::electronics_editor::tr;
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;
use crate::ui_kit;

/// Points this close, in millimetres, are the same for telling the outline's shape.
const NEAR: f64 = 2e-4;

/// How the Outline tool draws: a rectangle dragged from corner to corner, a circle from its
/// centre, or a polygon clicked corner by corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Rectangle,
    Circle,
    Polygon,
}

const MODES: [(Mode, &str); 3] = [
    (Mode::Rectangle, "electronics.board_shape_rect"),
    (Mode::Circle, "electronics.board_shape_circle"),
    (Mode::Polygon, "electronics.board_shape_polygon"),
];

/// A Shape row: a dropdown of the modes; `Some` with one other than `now`.
pub(crate) fn mode_row(ui: &mut Ui, id: &str, now: Mode) -> Option<Mode> {
    let names: Vec<String> = MODES.iter().map(|(_, key)| tr(key)).collect();
    let at = MODES.iter().position(|(m, _)| *m == now).unwrap_or(0);
    let chosen = crate::electronics_editor::choice(ui, id, &tr("electronics.field_shape"), at, &names)?;
    MODES.get(chosen).map(|(m, _)| *m).filter(|m| *m != now)
}

/// What the outline is, for its handles and the Inspector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Shape {
    /// An upright rectangle from `lo` to `hi`, its corners rounded to `radius`.
    Rect {
        lo: [f64; 2],
        hi: [f64; 2],
        radius: f64,
    },
    Circle {
        center: [f64; 2],
        radius: f64,
    },
    Polygon,
}

/// An outline as `ecad.board.outline` takes it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Outline {
    Rect { lo: [f64; 2], hi: [f64; 2], radius: f64 },
    Circle { center: [f64; 2], radius: f64 },
    Polygon { points: Vec<[f64; 2]>, curves: Vec<f64> },
}

/// What a press on a picked outline holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Handle {
    /// A rectangle's corner, counter-clockwise from the lower left: the opposite one stays.
    RectCorner(usize),
    /// A rectangle's side: the bottom, the right, the top, the left.
    RectSide(usize),
    /// A circle's rim: the radius follows it.
    Rim,
    /// A polygon's corner, by index.
    Corner(usize),
    /// A polygon's edge from corner `i` to the next: it moves across itself.
    Edge(usize),
    /// The whole outline: it moves.
    Whole,
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= NEAR
}

fn dist([ax, ay]: [f64; 2], [bx, by]: [f64; 2]) -> f64 {
    (ax - bx).hypot(ay - by)
}

/// A length rounded to 0.0001 mm.
fn tidy(v: f64) -> f64 {
    (v * 1e4).round() / 1e4 + 0.0
}

fn middle([ax, ay]: [f64; 2], [bx, by]: [f64; 2]) -> [f64; 2] {
    [(ax + bx) / 2.0, (ay + by) / 2.0]
}

/// A rectangle's corners, counter-clockwise from the lower left.
fn corners(lo: [f64; 2], hi: [f64; 2]) -> [[f64; 2]; 4] {
    [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
}

/// The shape of the outline the board has: an upright rectangle (maybe with all four corners
/// rounded alike), a circle (two half arcs), or anything else as a polygon. `None` without one.
pub(crate) fn shape_of(points: &[[f64; 2]], curves: &[f64]) -> Option<Shape> {
    if points.is_empty() {
        return None;
    }
    if let ([a, c], [s, t]) = (points, curves)
        && near(s.abs(), 180.0)
        && near(*s, *t)
    {
        return Some(Shape::Circle { center: middle(*a, *c), radius: dist(*a, *c) / 2.0 });
    }
    let [lo, hi] = path::bbox(points, curves, true)?;
    let on_x = |x: f64| near(x, lo[0]) || near(x, hi[0]);
    let on_y = |y: f64| near(y, lo[1]) || near(y, hi[1]);
    if !points.iter().all(|p| on_x(p[0]) || on_y(p[1])) {
        return Some(Shape::Polygon);
    }
    let mut radius: Option<f64> = None;
    let mut arcs = 0;
    for (a, c, s) in path::segments(points, curves, true) {
        if !is_arc(s) {
            let upright = (near(a[0], c[0]) && on_x(a[0])) || (near(a[1], c[1]) && on_y(a[1]));
            if !upright {
                return Some(Shape::Polygon);
            }
            continue;
        }
        // A rounded corner: a quarter turn between two sides, bulging out.
        let (dx, dy) = ((c[0] - a[0]).abs(), (c[1] - a[1]).abs());
        let Some(arc) = Arc::new(a, c, s) else { return Some(Shape::Polygon) };
        let k = arc.center;
        let inside = (near(k[0] - lo[0], dx) || near(hi[0] - k[0], dx)) && (near(k[1] - lo[1], dx) || near(hi[1] - k[1], dx));
        if !near(s.abs(), 90.0) || !near(dx, dy) || !inside || radius.is_some_and(|r| !near(r, dx)) {
            return Some(Shape::Polygon);
        }
        radius = Some(dx);
        arcs += 1;
    }
    match (arcs, radius) {
        (0, _) if points.len() == 4 => Some(Shape::Rect { lo, hi, radius: 0.0 }),
        (4, Some(radius)) => Some(Shape::Rect { lo, hi, radius }),
        _ => Some(Shape::Polygon),
    }
}

/// The outline's shape as its handles edit it: a rectangle the Inspector turned into a polygon
/// is edited corner by corner.
pub(crate) fn shape(app: &SolveApp) -> Option<Shape> {
    let b = board(app);
    match shape_of(&b.outline, &b.outline_curves)? {
        Shape::Rect { .. } if app.electronics.board.polygon.as_ref().is_some_and(|(p, c)| *p == b.outline && *c == b.outline_curves) => {
            Some(Shape::Polygon)
        }
        s => Some(s),
    }
}

/// The picked outline's handles: corners and the middles of edges (the rim of a circle).
pub(crate) fn handles(app: &SolveApp) -> Vec<(Handle, [f64; 2])> {
    let b = board(app);
    match shape(app) {
        None => Vec::new(),
        Some(Shape::Rect { lo, hi, .. }) => {
            let c = corners(lo, hi);
            let mut out: Vec<(Handle, [f64; 2])> = c.iter().enumerate().map(|(i, p)| (Handle::RectCorner(i), *p)).collect();
            out.extend((0..4).map(|k| (Handle::RectSide(k), middle(c[k], c[(k + 1) % 4]))));
            out
        }
        Some(Shape::Circle { center, radius }) => {
            [[radius, 0.0], [0.0, radius], [-radius, 0.0], [0.0, -radius]].map(|[dx, dy]| (Handle::Rim, [center[0] + dx, center[1] + dy])).to_vec()
        }
        Some(Shape::Polygon) => {
            let mut out: Vec<(Handle, [f64; 2])> = b.outline.iter().enumerate().map(|(i, p)| (Handle::Corner(i), *p)).collect();
            for (i, (a, c, s)) in path::segments(&b.outline, &b.outline_curves, true).enumerate() {
                let m = Arc::new(a, c, s).map_or(middle(a, c), |arc| arc.point(0.5));
                out.push((Handle::Edge(i), m));
            }
            out
        }
    }
}

/// The handle of the picked outline under `p`, if any.
pub(crate) fn handle_at(app: &SolveApp, map: PageMap, p: Pos2) -> Option<Handle> {
    handles(app)
        .into_iter()
        .map(|(h, q)| (h, map.to_screen(q[0], q[1]).distance(p)))
        .filter(|(_, d)| *d <= PICK_POINTS + 2.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(h, _)| h)
}

/// The edge of the outline under `p`, as the handle a press there holds: a rectangle's side (its
/// corner on a rounded one), a circle's rim, a polygon's edge.
pub(crate) fn edge_at(app: &SolveApp, map: PageMap, p: Pos2) -> Option<Handle> {
    if app.electronics.board.hidden.contains(&Layer::Outline) {
        return None;
    }
    let b = board(app);
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    let (i, (a, c, s)) = path::segments(&b.outline, &b.outline_curves, true)
        .enumerate()
        .map(|(i, seg)| {
            let line = tessellate(&[seg.0, seg.1], &[seg.2], false, arc_tolerance(map));
            let d = line.windows(2).map(|w| segment_distance(p, screen(w[0]), screen(w[1]))).fold(f32::INFINITY, f32::min);
            (i, seg, d)
        })
        .filter(|(_, _, d)| *d <= PICK_POINTS)
        .min_by(|x, y| x.2.total_cmp(&y.2))
        .map(|(i, seg, _)| (i, seg))?;
    match shape(app)? {
        Shape::Rect { lo, hi, .. } => {
            if is_arc(s) {
                let m = Arc::new(a, c, s).map_or(middle(a, c), |arc| arc.point(0.5));
                let k = (0..4).min_by(|x, y| dist(corners(lo, hi)[*x], m).total_cmp(&dist(corners(lo, hi)[*y], m)))?;
                return Some(Handle::RectCorner(k));
            }
            let side = if near(a[1], lo[1]) && near(c[1], lo[1]) {
                0
            } else if near(a[0], hi[0]) && near(c[0], hi[0]) {
                1
            } else if near(a[1], hi[1]) && near(c[1], hi[1]) {
                2
            } else {
                3
            };
            Some(Handle::RectSide(side))
        }
        Shape::Circle { .. } => Some(Handle::Rim),
        Shape::Polygon => Some(Handle::Edge(i)),
    }
}

/// An upright rectangle from two corners in any order; `None` when it has no area.
fn rect_from(a: [f64; 2], b: [f64; 2], radius: f64) -> Option<Outline> {
    let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
    (hi[0] - lo[0] > 0.0 && hi[1] - lo[1] > 0.0).then_some(Outline::Rect { lo, hi, radius })
}

/// The outline a drag of `handle` from `from` to `to` (page millimetres) leaves.
pub(crate) fn dragged(app: &SolveApp, handle: Handle, from: [f64; 2], to: [f64; 2]) -> Option<Outline> {
    let b = board(app);
    match (shape(app)?, handle) {
        (Shape::Rect { lo, hi, radius }, Handle::RectCorner(i)) => rect_from(*corners(lo, hi).get((i + 2) % 4)?, to, radius),
        (Shape::Rect { mut lo, mut hi, radius }, Handle::RectSide(k)) => {
            match k {
                0 => lo[1] = to[1],
                1 => hi[0] = to[0],
                2 => hi[1] = to[1],
                _ => lo[0] = to[0],
            }
            rect_from(lo, hi, radius)
        }
        (Shape::Circle { center, .. }, Handle::Rim) => {
            let radius = dist(center, to);
            (radius > 0.0).then_some(Outline::Circle { center, radius })
        }
        (Shape::Polygon, Handle::Corner(i)) => {
            let mut points = b.outline.clone();
            *points.get_mut(i)? = to;
            Some(Outline::Polygon { points, curves: b.outline_curves.clone() })
        }
        (shape, Handle::Whole) => {
            let by = [to[0] - from[0], to[1] - from[1]];
            let moved = |q: [f64; 2]| snap_tidy([q[0] + by[0], q[1] + by[1]]);
            Some(match shape {
                Shape::Rect { lo, hi, radius } => Outline::Rect { lo: moved(lo), hi: moved(hi), radius },
                Shape::Circle { center, radius } => Outline::Circle { center: moved(center), radius },
                Shape::Polygon => Outline::Polygon { points: b.outline.iter().map(|q| moved(*q)).collect(), curves: b.outline_curves.clone() },
            })
        }
        (Shape::Polygon, Handle::Edge(i)) => {
            let mut points = b.outline.clone();
            let j = (i + 1) % points.len().max(1);
            let (a, c) = (*points.get(i)?, *points.get(j)?);
            let len = dist(a, c);
            if !(len > 0.0) {
                return None;
            }
            // Across the edge's chord only, so the edge keeps its direction.
            let normal = [-(c[1] - a[1]) / len, (c[0] - a[0]) / len];
            let across = (to[0] - from[0]) * normal[0] + (to[1] - from[1]) * normal[1];
            for k in [i, j] {
                if let Some(q) = points.get_mut(k) {
                    *q = snap_tidy([q[0] + normal[0] * across, q[1] + normal[1] * across]);
                }
            }
            Some(Outline::Polygon { points, curves: b.outline_curves.clone() })
        }
        _ => None,
    }
}

/// `o`'s corners and sweeps, for drawing it (a rounded rectangle as the command makes it).
pub(crate) fn ring(o: &Outline) -> (Vec<[f64; 2]>, Vec<f64>) {
    match o {
        Outline::Rect { lo, hi, radius } => {
            let r = radius.min((hi[0] - lo[0]) / 2.0).min((hi[1] - lo[1]) / 2.0).max(0.0);
            if r <= 0.0 {
                return (corners(*lo, *hi).to_vec(), Vec::new());
            }
            let ([x0, y0], [x1, y1]) = (*lo, *hi);
            let points = vec![[x0 + r, y0], [x1 - r, y0], [x1, y0 + r], [x1, y1 - r], [x1 - r, y1], [x0 + r, y1], [x0, y1 - r], [x0, y0 + r]];
            (points, [0.0, 90.0].repeat(4))
        }
        Outline::Circle { center, radius } => (vec![[center[0] + radius, center[1]], [center[0] - radius, center[1]]], vec![180.0, 180.0]),
        Outline::Polygon { points, curves } => (points.clone(), curves.clone()),
    }
}

/// `o` as `ecad.board.outline` takes it.
pub(crate) fn params(o: &Outline) -> Value {
    match o {
        Outline::Rect { lo, hi, radius } => {
            let mut p = json!({"rect": snap_tidy([hi[0] - lo[0], hi[1] - lo[1]]), "center": snap_tidy(middle(*lo, *hi))});
            if *radius > 0.0 {
                p["corner_radius"] = json!(tidy(*radius));
            }
            p
        }
        Outline::Circle { center, radius } => json!({"circle": {"center": snap_tidy(*center), "radius": tidy(*radius)}}),
        Outline::Polygon { points, curves } => {
            let mut p = json!({"points": points.iter().map(|q| snap_tidy(*q)).collect::<Vec<_>>()});
            if curves.iter().any(|c| is_arc(*c)) {
                p["curves"] = json!(curves);
            }
            p
        }
    }
}

/// Set the outline to `o`; `true` when it took. A polygon set so is edited as one (see
/// [`edit_as_polygon`]), even when it comes out a rectangle.
pub(crate) fn set(app: &mut SolveApp, o: &Outline) -> bool {
    if app.run("ecad.board.outline", params(o)).is_err() {
        return false;
    }
    if matches!(o, Outline::Polygon { .. }) {
        edit_as_polygon(app);
    } else {
        app.electronics.board.polygon = None;
    }
    true
}

/// The outline as it is now is edited corner by corner, as a polygon.
fn edit_as_polygon(app: &mut SolveApp) {
    let b = board(app);
    app.electronics.board.polygon = Some((b.outline.clone(), b.outline_curves.clone()));
}

/// The end of a drag of `handle`: the outline it leaves, set in one step.
pub(crate) fn release(app: &mut SolveApp, handle: Handle, from: [f64; 2], to: [f64; 2]) {
    if from == to {
        return;
    }
    if let Some(o) = dragged(app, handle, from, to) {
        set(app, &o);
    }
}

/// The picked outline's handles: squares on the corners, rounds on the edges.
pub(crate) fn paint_handles(app: &SolveApp, painter: &egui::Painter, map: PageMap, t: &Tokens) {
    let stroke = Stroke::new(1.5, t.sel_edge);
    for (h, q) in handles(app) {
        let c = map.to_screen(q[0], q[1]);
        if !(c.x.is_finite() && c.y.is_finite()) {
            continue;
        }
        match h {
            Handle::RectCorner(_) | Handle::Corner(_) => {
                let r = Rect::from_center_size(c, vec2(8.0, 8.0));
                painter.rect(r, 1.0, t.viewport_bottom, stroke, egui::StrokeKind::Middle);
            }
            Handle::RectSide(_) | Handle::Edge(_) | Handle::Rim => {
                painter.circle(c, 4.0, t.viewport_bottom, stroke);
            }
            Handle::Whole => {}
        }
    }
}

/// A click with the Outline tool's circle or polygon mode: a circle's centre then a point on its
/// rim; a polygon's corners, closed by a click on its first corner, a double-click or Enter.
pub(crate) fn click(app: &mut SolveApp, map: PageMap, p: Pos2, free: bool, double: bool) {
    let Some(Tool::Outline(mut points)) = app.electronics.board.tool.clone() else { return };
    let at = snap(app, map, p, free);
    match app.electronics.board.outline_mode {
        Mode::Rectangle => {}
        Mode::Circle => {
            if double {
                return;
            }
            match points.first() {
                None => points.push(at),
                Some(c) => {
                    let radius = dist(*c, at);
                    if radius > 0.0 {
                        done(app, &Outline::Circle { center: *c, radius });
                        return;
                    }
                    // The rim on the centre: still waiting for it.
                }
            }
        }
        Mode::Polygon => {
            let first = points.first().map(|q| map.to_screen(q[0], q[1]));
            if points.len() >= 3 && (double || first.is_some_and(|f| f.distance(p) <= PICK_POINTS + 2.0)) {
                app.electronics.board.tool = Some(Tool::Outline(points));
                close(app);
                return;
            }
            if points.last() != Some(&at) && points.len() < MAX_OUTLINE_POINTS {
                points.push(at);
            }
        }
    }
    app.electronics.board.tool = Some(Tool::Outline(points));
}

/// Enter with the Outline tool: the polygon clicked so far, closed.
pub(crate) fn close(app: &mut SolveApp) {
    let Some(Tool::Outline(points)) = app.electronics.board.tool.clone() else { return };
    if app.electronics.board.outline_mode == Mode::Polygon && points.len() >= 3 {
        done(app, &Outline::Polygon { points, curves: Vec::new() });
    }
}

/// Set the outline the tool drew; once it takes, the tool goes down and the outline is picked.
/// `true` when it took.
pub(crate) fn done(app: &mut SolveApp, o: &Outline) -> bool {
    if !set(app, o) {
        return false;
    }
    let s = &mut app.electronics.board;
    s.tool = None;
    s.selection.clear();
    s.segment = None;
    s.outline_picked = true;
    true
}

/// Backspace or Escape with the Outline tool: take back the last click. `false` with none.
pub(crate) fn step_back(app: &mut SolveApp) -> bool {
    match &mut app.electronics.board.tool {
        Some(Tool::Outline(points)) => points.pop().is_some(),
        _ => false,
    }
}

/// What the Outline tool's circle or polygon mode would draw with the pointer at `hover`.
pub(crate) fn paint_tool(app: &SolveApp, painter: &egui::Painter, map: PageMap, hover: Pos2, free: bool, t: &Tokens) {
    let Some(Tool::Outline(points)) = &app.electronics.board.tool else { return };
    let ghost = Stroke::new(1.5, t.sel_edge.gamma_multiply(0.7));
    let at = snap(app, map, hover, free);
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    painter.circle_stroke(screen(at), 4.0, ghost);
    match (app.electronics.board.outline_mode, points.first()) {
        (Mode::Circle, Some(c)) => {
            let r = (dist(*c, at) * map.scale()) as f32;
            if r.is_finite() {
                painter.circle_stroke(screen(*c), r, ghost);
            }
        }
        (Mode::Polygon, Some(_)) => {
            let mut line: Vec<Pos2> = points.iter().map(|q| screen(*q)).collect();
            line.push(screen(at));
            if line.iter().all(|q| q.x.is_finite() && q.y.is_finite()) {
                painter.add(egui::Shape::closed_line(line.clone(), Stroke::new(1.0, t.sel_edge.gamma_multiply(0.4))));
                painter.add(egui::Shape::line(line, ghost));
            }
        }
        _ => {}
    }
}

/// The Outline tool's options: its mode, and the rectangle's size and corners to set by typing.
pub(crate) fn tool_options(app: &mut SolveApp, ui: &mut Ui) -> &'static str {
    let mode = app.electronics.board.outline_mode;
    if let Some(m) = mode_row(ui, "electronics_board_outline_mode", mode) {
        app.electronics.board.outline_mode = m;
        app.electronics.board.tool = Some(Tool::Outline(Vec::new()));
    }
    match app.electronics.board.outline_mode {
        Mode::Rectangle => {
            let [w, h] = app.electronics.board.outline_size;
            if let Some(v) = crate::electronics_editor::number(app, ui, "board:outline:width", &tr("electronics.field_board_width"), w) {
                app.electronics.board.outline_size[0] = v;
            }
            if let Some(v) = crate::electronics_editor::number(app, ui, "board:outline:height", &tr("electronics.field_board_height"), h) {
                app.electronics.board.outline_size[1] = v;
            }
            let radius = app.electronics.board.outline_radius;
            if let Some(v) = crate::electronics_editor::number(app, ui, "board:outline:radius", &tr("electronics.field_corner_radius"), radius) {
                if v.is_finite() && v >= 0.0 {
                    app.electronics.board.outline_radius = v;
                } else {
                    app.set_status(tr("electronics.board_bad_radius"), true);
                }
            }
            if crate::electronics_editor::button(ui, "board-set-outline", &tr("electronics.board_set_outline"), true) {
                let ([w, h], radius) = (app.electronics.board.outline_size, app.electronics.board.outline_radius);
                // Typed, it is centred on the origin, and framed.
                match rect_from([-w / 2.0, -h / 2.0], [w / 2.0, h / 2.0], radius) {
                    Some(o) if done(app, &o) => app.electronics.page.fitted = false,
                    Some(_) => {}
                    None => app.set_status(tr("electronics.board_bad_size"), true),
                }
            }
            "electronics.board_outline_help"
        }
        Mode::Circle => "electronics.board_outline_circle_help",
        Mode::Polygon => "electronics.board_outline_polygon_help",
    }
}

/// Whether `p` is inside the outline, where a press on bare board takes hold of a picked outline
/// to move it whole.
pub(crate) fn inside_at(app: &SolveApp, map: PageMap, p: Pos2) -> bool {
    let b = board(app);
    if b.outline.len() < 2 || app.electronics.board.hidden.contains(&Layer::Outline) {
        return false;
    }
    let (x, y) = map.to_page(p);
    solidcraft_engine::doc::ecad::copper::inside([x, y], &tessellate(&b.outline, &b.outline_curves, true, arc_tolerance(map)))
}

/// The picked outline in the Inspector: its shape (a rectangle, a circle or a polygon, each turned
/// into the others), its size (about its centre) and its centre, a rectangle's corner radius, and
/// the board's thickness.
pub(crate) fn section(app: &mut SolveApp, ui: &mut Ui) {
    let Some(shape) = shape(app) else { return };
    let b = board(app);
    let (thickness, bbox, corners_n) = (b.thickness, b.outline_bbox(), b.outline.len());
    let title = tr("electronics.board_outline");
    ui_kit::section(ui, "electronics_board_outline", &title, true, |ui| {
        let now = match shape {
            Shape::Rect { .. } => Mode::Rectangle,
            Shape::Circle { .. } => Mode::Circle,
            Shape::Polygon => Mode::Polygon,
        };
        if let (Some(m), Some([lo, hi])) = (mode_row(ui, "electronics_board_outline_shape", now), bbox) {
            convert(app, m, lo, hi);
            return;
        }
        let number = |app: &mut SolveApp, ui: &mut Ui, key: &str, label: &str, v: f64| crate::electronics_editor::number(app, ui, key, &tr(label), v);
        // `Some(None)`: a size typed that makes no outline.
        let mut to: Option<Option<Outline>> = None;
        match shape {
            Shape::Rect { lo, hi, radius } => {
                // A new width or height keeps the centre where it is.
                let c = middle(lo, hi);
                let ok = |v: f64| v.is_finite() && v > 0.0;
                if let Some(w) = number(app, ui, "board:outline:picked:width", "electronics.field_board_width", hi[0] - lo[0]) {
                    to = Some(ok(w).then(|| rect_from(snap_tidy([c[0] - w / 2.0, lo[1]]), snap_tidy([c[0] + w / 2.0, hi[1]]), radius)).flatten());
                }
                if let Some(h) = number(app, ui, "board:outline:picked:height", "electronics.field_board_height", hi[1] - lo[1]) {
                    to = Some(ok(h).then(|| rect_from(snap_tidy([lo[0], c[1] - h / 2.0]), snap_tidy([hi[0], c[1] + h / 2.0]), radius)).flatten());
                }
                if let Some(at) = crate::electronics_editor::pair(app, ui, "board:outline:picked:center", &tr("electronics.field_center"), c) {
                    let by = [at[0] - c[0], at[1] - c[1]];
                    to = Some(rect_from(snap_tidy([lo[0] + by[0], lo[1] + by[1]]), snap_tidy([hi[0] + by[0], hi[1] + by[1]]), radius));
                }
                if let Some(r) = number(app, ui, "board:outline:picked:radius", "electronics.field_corner_radius", radius) {
                    to = Some((r.is_finite() && r >= 0.0).then(|| rect_from(lo, hi, r)).flatten());
                }
            }
            Shape::Circle { center, radius } => {
                if let Some(d) = number(app, ui, "board:outline:picked:diameter", "electronics.field_diameter", radius * 2.0) {
                    to = Some((d.is_finite() && d > 0.0).then_some(Outline::Circle { center, radius: d / 2.0 }));
                }
                if let Some(c) = crate::electronics_editor::pair(app, ui, "board:outline:picked:center", &tr("electronics.field_center"), center) {
                    to = Some(Some(Outline::Circle { center: c, radius }));
                }
            }
            Shape::Polygon => {
                if let Some([[x0, y0], [x1, y1]]) = bbox {
                    let mm = crate::electronics_editor::mm;
                    let text = crate::i18n::format("electronics.board_outline_size", &[("w", &mm(x1 - x0)), ("h", &mm(y1 - y0))]);
                    ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_outline_size"));
                    // Its box's centre: a new one moves every corner.
                    let c = middle([x0, y0], [x1, y1]);
                    if let Some(at) = crate::electronics_editor::pair(app, ui, "board:outline:picked:center", &tr("electronics.field_center"), c) {
                        let b = board(app);
                        let points = b.outline.iter().map(|q| snap_tidy([q[0] + at[0] - c[0], q[1] + at[1] - c[1]])).collect();
                        to = Some(Some(Outline::Polygon { points, curves: b.outline_curves.clone() }));
                    }
                }
                let text = crate::i18n::format("electronics.board_outline_corners", &[("n", &corners_n.to_string())]);
                ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_outline_corners"));
            }
        }
        match to {
            Some(Some(o)) => {
                set(app, &o);
            }
            Some(None) => app.set_status(tr("electronics.board_bad_size"), true),
            None => {}
        }
        if let Some(t) = number(app, ui, "board:outline:thickness", "electronics.field_thickness", thickness) {
            drop(app.run("ecad.board.outline", json!({"thickness": t})));
        }
        ui_kit::text(ui, &tr("electronics.board_outline_picked_help"), ui_kit::Text::Note("electronics.board_outline_picked_help"));
    });
}

/// The picked outline made into `mode`, within its box `lo`–`hi`: a rectangle filling it, the
/// largest circle in it, or a polygon (a rectangle's own corners, edited one by one).
fn convert(app: &mut SolveApp, mode: Mode, lo: [f64; 2], hi: [f64; 2]) {
    let c = middle(lo, hi);
    let radius = ((hi[0] - lo[0]).min(hi[1] - lo[1]) / 2.0).max(0.0);
    match mode {
        Mode::Rectangle => {
            app.electronics.board.polygon = None;
            if let Some(o) = rect_from(lo, hi, 0.0) {
                set(app, &o);
            }
        }
        Mode::Circle => {
            app.electronics.board.polygon = None;
            if radius > 0.0 {
                set(app, &Outline::Circle { center: c, radius });
            }
        }
        Mode::Polygon => {
            // A circle becomes the square round it; a rectangle stays as it is.
            let b = board(app);
            if matches!(shape_of(&b.outline, &b.outline_curves), Some(Shape::Circle { .. })) {
                set(app, &Outline::Polygon { points: corners(lo, hi).to_vec(), curves: Vec::new() });
            } else {
                edit_as_polygon(app);
            }
        }
    }
}
