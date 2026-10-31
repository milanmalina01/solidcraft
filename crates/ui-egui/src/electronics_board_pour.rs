//! Copper pours and keepouts on the board (see `electronics_board`). The Polygon tool clicks a
//! pour's corners on the active layer (closed on its first corner, a double-click or Enter) with
//! the options' net, clearance, connection and fill; the Keepout tool draws a keepout as a
//! rectangle, a circle or a polygon (as the Outline tool's modes) on the layers and for the
//! objects its options say. A pour is drawn as its fill in its layer's colour, lighter so tracks
//! over it read, its outline dashed; a keepout hatched. Either is picked by its edge (a pour also
//! by its fill on the active layer), and a picked one shows handles at its corners and the middles
//! of its edges, whose drags reshape it. Each change is one command (`ecad.board.pour`,
//! `.pour.edit`, `.keepout`, `.keepout.edit`); the fill is the board's to work out, again after
//! every change (`BoardNets::fills`), and is triangled here once per change for drawing.

use egui::{Pos2, Rect, Stroke, Ui, vec2};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::board::MAX_OUTLINE_POINTS;
use solidcraft_engine::doc::ecad::path::{self, Arc, is_arc, tessellate};
use solidcraft_engine::doc::ecad::pour::{BadSetting, DEFAULT_MIN_WIDTH, MAX_PRIORITY};
use solidcraft_engine::doc::ecad::{Board, BoardNets, Connect, CopperLayer, Forbid, Hatch, Keepout, Pour, Thermal};
use solidcraft_engine::geom::triangulate::{MAX_TRIANGULATED_CORNERS, triangles};

use crate::SolveApp;
use crate::electronics_board::{
    Layer, PICK_POINTS, Tool, active_layer, arc_tolerance, board, copper_shown, segment_distance, snap, snap_tidy, stored_nets,
};
use crate::electronics_board_outline::{self as outline, Mode, Outline};
use crate::electronics_board_route::{change_layer, layer_name, layer_row};
use crate::electronics_editor::tr;
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;
use crate::ui_kit;

/// The most corners of pour fills the board draws filled; past it the rest are drawn as outlines,
/// and past as many again in outlines the rest are not drawn.
const MAX_FILL_CORNERS: usize = 200_000;
/// A keepout's hatch lines are this far apart on screen, in points, and at most this many.
const HATCH_POINTS: f32 = 9.0;
const MAX_HATCH_LINES: usize = 600;
/// The most edge crossings the keepouts' hatches look for each frame (lines times corners); past
/// it a keepout is drawn without its hatch.
const MAX_HATCH_WORK: usize = 1 << 21;
/// A hatch the options turn on starts with these lines and pitch, in millimetres.
const HATCH_WIDTH: f64 = 0.25;
const HATCH_PITCH: f64 = 1.0;
/// A dashed outline longer on screen than this many times round the view is drawn solid: zoomed
/// far in, its dashes (made one by one each frame) would be millions, and past 10⁸ points along
/// an edge a dash's step no longer moves a 32-bit position on.
const MAX_DASHED_ROUNDS: f32 = 16.0;

/// The Polygon tool's options, and a picked pour's settings as the Inspector shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct PourOptions {
    /// The net, by name; `None` for no net.
    pub net: Option<String>,
    /// The net was chosen (or GND taken): until then, the first click takes the net under it.
    pub net_chosen: bool,
    /// The gap to other copper; the rules' (net class aware) while `None`.
    pub clearance: Option<f64>,
    pub min_width: f64,
    pub connect: Connect,
    pub thermal: Thermal,
    pub priority: u32,
    pub remove_islands: bool,
    pub hatch: Option<Hatch>,
    /// The net dropdown's search.
    pub search: String,
}

impl Default for PourOptions {
    fn default() -> Self {
        PourOptions {
            net: None,
            net_chosen: false,
            clearance: None,
            min_width: DEFAULT_MIN_WIDTH,
            connect: Connect::Thermal,
            thermal: Thermal::default(),
            priority: 0,
            remove_islands: true,
            hatch: None,
            search: String::new(),
        }
    }
}

impl PourOptions {
    /// A pour's settings.
    fn of(p: &Pour) -> Self {
        PourOptions {
            net: p.net.clone(),
            net_chosen: true,
            clearance: p.clearance,
            min_width: p.min_width,
            connect: p.connect,
            thermal: p.thermal,
            priority: p.priority,
            remove_islands: p.remove_islands,
            hatch: p.hatch,
            search: String::new(),
        }
    }

    /// Why these settings would be refused, in the UI's words; `None` when they pass.
    fn refused(&self) -> Option<String> {
        let p = Pour {
            id: 0,
            layer: CopperLayer::Top,
            net: self.net.clone(),
            outline: Vec::new(),
            curves: Vec::new(),
            clearance: self.clearance,
            min_width: self.min_width,
            connect: self.connect,
            thermal: self.thermal,
            priority: self.priority,
            remove_islands: self.remove_islands,
            hatch: self.hatch,
        };
        p.bad_setting(0.0).map(bad_setting)
    }
}

/// A pour setting out of range, in the UI's words.
fn bad_setting(b: BadSetting) -> String {
    crate::i18n::format(b.key(), &[("max", &crate::electronics_editor::mm(b.max()))])
}

/// The Keepout tool's options.
#[derive(Clone, Debug, PartialEq)]
pub struct KeepoutOptions {
    pub mode: Mode,
    /// The copper layers it keeps clear; empty is every layer.
    pub layers: Vec<CopperLayer>,
    pub forbid: Forbid,
}

impl Default for KeepoutOptions {
    fn default() -> Self {
        KeepoutOptions { mode: Mode::Rectangle, layers: Vec::new(), forbid: Forbid::default() }
    }
}

/// One pour's fill as the board draws it, in millimetres: its triangles (none when it was too
/// big to fill on screen) and its contours.
pub struct FillMesh {
    pub pour: u64,
    pub layer: CopperLayer,
    pub triangles: Vec<[[f64; 2]; 3]>,
    pub rings: Vec<Vec<[f64; 2]>>,
}

/// The fills of `nets` triangled for drawing, up to [`MAX_FILL_CORNERS`] corners in all, and
/// their contours up to as many again (each frame draws all of them); `true` when some were left
/// as outlines, or out.
pub(crate) fn meshes(nets: &BoardNets) -> (Vec<FillMesh>, bool) {
    let mut left = MAX_FILL_CORNERS;
    let mut rims = MAX_FILL_CORNERS;
    let mut short = false;
    let mut out = Vec::with_capacity(nets.fills.len());
    for f in &nets.fills {
        let mut mesh = FillMesh { pour: f.pour, layer: f.layer, triangles: Vec::new(), rings: Vec::new() };
        for shape in f.shapes() {
            let corners = shape.corners();
            if corners <= left.min(MAX_TRIANGULATED_CORNERS)
                && let Some(t) = triangles(&shape, left)
            {
                mesh.triangles.extend(t);
                left -= corners;
            } else {
                short = true;
            }
            if corners <= rims {
                rims -= corners;
                mesh.rings.push(shape.outer);
                mesh.rings.extend(shape.holes);
            } else {
                short = true;
            }
        }
        out.push(mesh);
    }
    (out, short)
}

/// The net named GND (in any case), else the first whose name starts so, of `names`.
fn ground(names: &[String]) -> Option<&str> {
    let upper = |n: &&String| n.to_ascii_uppercase();
    names.iter().find(|n| upper(n) == "GND").or_else(|| names.iter().find(|n| upper(n).starts_with("GND"))).map(String::as_str)
}

/// The Polygon tool goes on: while no net was chosen, GND is when the design has one.
pub(crate) fn start_pour(app: &mut SolveApp) {
    if app.electronics.board.pour.net_chosen {
        return;
    }
    let gnd = ground(&stored_nets(app).names).map(str::to_string);
    if let Some(n) = gnd {
        let o = &mut app.electronics.board.pour;
        o.net = Some(n);
        o.net_chosen = true;
    }
}

/// A pour's or keepout's outline closed and tessellated at `map`'s scale.
fn ring(map: PageMap, points: &[[f64; 2]], curves: &[f64]) -> Vec<[f64; 2]> {
    tessellate(points, curves, true, arc_tolerance(map))
}

fn screen_ring(map: PageMap, points: &[[f64; 2]], curves: &[f64]) -> Option<Vec<Pos2>> {
    let line: Vec<Pos2> = ring(map, points, curves).into_iter().map(|q| map.to_screen(q[0], q[1])).collect();
    line.iter().all(|p| p.x.is_finite() && p.y.is_finite()).then_some(line)
}

/// Whether `p` is inside the closed ring `ring` (even–odd).
fn inside(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = ring.len();
    let mut odd = false;
    for i in 0..n {
        let (Some(a), Some(b)) = (ring.get(i), ring.get((i + 1) % n)) else { continue };
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) {
            odd = !odd;
        }
    }
    odd
}

/// How far, in points, `p` is from the screen ring `line`'s edges.
fn edge_distance(line: &[Pos2], p: Pos2) -> f32 {
    let n = line.len();
    (0..n).filter_map(|i| Some(segment_distance(p, *line.get(i)?, *line.get((i + 1) % n)?))).fold(f32::INFINITY, f32::min)
}

/// Whether the Layers tab shows the pours of `layer` (their copper layer and the pour fill).
fn pour_shown(app: &SolveApp, layer: CopperLayer) -> bool {
    copper_shown(app, layer)
}

fn keepouts_shown(app: &SolveApp) -> bool {
    !app.electronics.board.hidden.contains(&Layer::Keepouts)
}

/// The pour or keepout whose edge is under `p`: pours on shown layers (the active layer's
/// first), then keepouts.
pub(crate) fn edge_at(app: &SolveApp, map: PageMap, p: Pos2) -> Option<u64> {
    let b = board(app);
    let active = active_layer(app);
    let mut pours: Vec<&Pour> = b.pours.iter().rev().filter(|q| pour_shown(app, q.layer)).collect();
    pours.sort_by_key(|q| q.layer != active);
    let near = |points: &[[f64; 2]], curves: &[f64]| screen_ring(map, points, curves).is_some_and(|l| edge_distance(&l, p) <= PICK_POINTS);
    if let Some(q) = pours.into_iter().find(|q| near(&q.outline, &q.curves)) {
        return Some(q.id);
    }
    if !keepouts_shown(app) {
        return None;
    }
    b.keepouts.iter().rev().find(|k| near(&k.outline, &k.curves)).map(|k| k.id)
}

/// The pour of the active layer whose fill is under `p`, else the keepout `p` is inside.
pub(crate) fn area_at(app: &SolveApp, map: PageMap, p: Pos2) -> Option<u64> {
    let (x, y) = map.to_page(p);
    let layer = active_layer(app);
    if pour_shown(app, layer) && !app.electronics.board.hidden.contains(&Layer::PourFill) {
        let nets = stored_nets(app);
        if let Some(f) =
            nets.fills.iter().rev().filter(|f| f.layer == layer).find(|f| f.region.locate(&[[x, y]]).first().is_some_and(Option::is_some))
        {
            return Some(f.pour);
        }
    }
    if !keepouts_shown(app) {
        return None;
    }
    let tol = arc_tolerance(map);
    board(app).keepouts.iter().rev().find(|k| inside(&tessellate(&k.outline, &k.curves, true, tol), [x, y])).map(|k| k.id)
}

/// A handle of a picked pour or keepout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Grip {
    /// A corner, by index: it moves.
    Corner(usize),
    /// The edge from corner `i` to the next: it moves across itself.
    Edge(usize),
}

/// The pour or keepout picked alone: its id, corners and sweeps.
fn picked(app: &SolveApp) -> Option<(u64, Vec<[f64; 2]>, Vec<f64>)> {
    let ids: Vec<u64> = app.electronics.board.selection.iter().copied().collect();
    let [id] = ids[..] else { return None };
    let b = board(app);
    b.pour(id).map(|p| (id, p.outline.clone(), p.curves.clone())).or_else(|| b.keepout(id).map(|k| (id, k.outline.clone(), k.curves.clone())))
}

/// The picked pour's or keepout's handles: its corners and the middles of its edges.
fn grips(points: &[[f64; 2]], curves: &[f64]) -> Vec<(Grip, [f64; 2])> {
    let mut out: Vec<(Grip, [f64; 2])> = points.iter().enumerate().map(|(i, p)| (Grip::Corner(i), *p)).collect();
    for (i, (a, c, s)) in path::segments(points, curves, true).enumerate() {
        let m = Arc::new(a, c, s).map_or([(a[0] + c[0]) / 2.0, (a[1] + c[1]) / 2.0], |arc| arc.point(0.5));
        out.push((Grip::Edge(i), m));
    }
    out
}

/// The handle of the picked pour or keepout under `p`, if any.
pub(crate) fn grip_at(app: &SolveApp, map: PageMap, p: Pos2) -> Option<(u64, Grip)> {
    let (id, points, curves) = picked(app)?;
    grips(&points, &curves)
        .into_iter()
        .map(|(g, q)| (g, map.to_screen(q[0], q[1]).distance(p)))
        .filter(|(_, d)| *d <= PICK_POINTS + 2.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(g, _)| (id, g))
}

/// The corners a drag of `grip` from `from` to `to` leaves (the sweeps stay).
fn dragged(points: &[[f64; 2]], grip: Grip, from: [f64; 2], to: [f64; 2]) -> Option<Vec<[f64; 2]>> {
    let mut out = points.to_vec();
    match grip {
        Grip::Corner(i) => *out.get_mut(i)? = to,
        Grip::Edge(i) => {
            let j = (i + 1) % out.len().max(1);
            let (a, c) = (*out.get(i)?, *out.get(j)?);
            let len = (c[0] - a[0]).hypot(c[1] - a[1]);
            if !(len > 0.0) {
                return None;
            }
            // Across the edge's chord only, so it keeps its direction.
            let normal = [-(c[1] - a[1]) / len, (c[0] - a[0]) / len];
            let across = (to[0] - from[0]) * normal[0] + (to[1] - from[1]) * normal[1];
            for k in [i, j] {
                if let Some(q) = out.get_mut(k) {
                    *q = snap_tidy([q[0] + normal[0] * across, q[1] + normal[1] * across]);
                }
            }
        }
    }
    Some(out)
}

/// `points` with `curves` as the pour and keepout edits take them.
fn outline_params(points: &[[f64; 2]], curves: &[f64]) -> Value {
    let mut p = json!({"points": points.iter().map(|q| snap_tidy(*q)).collect::<Vec<_>>()});
    if curves.iter().any(|c| is_arc(*c)) {
        p["curves"] = json!(curves);
    }
    p
}

/// The end of a drag of the handle `grip` of the pour or keepout `id`: its new outline, in one
/// step.
pub(crate) fn release(app: &mut SolveApp, id: u64, grip: Grip, from: [f64; 2], to: [f64; 2]) {
    if from == to {
        return;
    }
    let b = board(app);
    let (command, key, points, curves) = match (b.pour(id), b.keepout(id)) {
        (Some(p), _) => ("ecad.board.pour.edit", "pour", p.outline.clone(), p.curves.clone()),
        (None, Some(k)) => ("ecad.board.keepout.edit", "keepout", k.outline.clone(), k.curves.clone()),
        _ => return,
    };
    let Some(points) = dragged(&points, grip, from, to) else { return };
    let mut params = outline_params(&points, &curves);
    params[key] = json!(id);
    drop(app.run(command, params));
}

/// The picked pour's or keepout's handles, and while one is dragged the outline it leaves.
pub(crate) fn paint_grips(app: &SolveApp, painter: &egui::Painter, map: PageMap, held: Option<(u64, Grip, [f64; 2], [f64; 2])>, t: &Tokens) {
    let Some((id, points, curves)) = picked(app) else { return };
    let points = match held {
        Some((h, grip, from, to)) if h == id => {
            let moved = dragged(&points, grip, from, to).unwrap_or(points);
            if let Some(line) = screen_ring(map, &moved, &curves) {
                painter.add(egui::Shape::closed_line(line, Stroke::new(1.5, t.sel_edge.gamma_multiply(0.7))));
            }
            moved
        }
        _ => points,
    };
    let stroke = Stroke::new(1.5, t.sel_edge);
    for (g, q) in grips(&points, &curves) {
        let c = map.to_screen(q[0], q[1]);
        if !(c.x.is_finite() && c.y.is_finite()) {
            continue;
        }
        match g {
            Grip::Corner(_) => {
                painter.rect(Rect::from_center_size(c, vec2(7.0, 7.0)), 1.0, t.viewport_bottom, stroke, egui::StrokeKind::Middle);
            }
            Grip::Edge(_) => {
                painter.circle(c, 3.5, t.viewport_bottom, stroke);
            }
        }
    }
}

/// The fills of the pours on `layer` in `colour` (lighter than copper, so tracks over them
/// read), from the meshes worked out for the stored board.
pub(crate) fn paint_fills(app: &SolveApp, painter: &egui::Painter, map: PageMap, layer: CopperLayer, colour: egui::Color32, t: &Tokens) {
    if app.electronics.board.hidden.contains(&Layer::PourFill) {
        return;
    }
    let Some(meshes) = crate::electronics_board::stored_meshes(app) else { return };
    let fill = colour.gamma_multiply(t.pcb_pour_alpha);
    let rim = Stroke::new(1.0, colour.gamma_multiply((t.pcb_pour_alpha + 1.0) / 2.0));
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    for m in meshes.iter().filter(|m| m.layer == layer) {
        let mut mesh = egui::Mesh::default();
        for tri in &m.triangles {
            let ps = tri.map(screen);
            if !ps.iter().all(|p| p.x.is_finite() && p.y.is_finite()) {
                continue;
            }
            let base = u32::try_from(mesh.vertices.len()).unwrap_or(u32::MAX);
            for p in ps {
                mesh.colored_vertex(p, fill);
            }
            mesh.add_triangle(base, base.saturating_add(1), base.saturating_add(2));
        }
        if !mesh.is_empty() {
            painter.add(egui::Shape::mesh(mesh));
        }
        for r in &m.rings {
            let line: Vec<Pos2> = r.iter().map(|q| screen(*q)).collect();
            if line.iter().all(|p| p.x.is_finite() && p.y.is_finite()) {
                painter.add(egui::Shape::closed_line(line, rim));
            }
        }
    }
}

/// The outlines of the pours on `layer` of `b`, dashed: faint in `colour`, the picked ones in the
/// selection colour.
pub(crate) fn paint_outlines(
    app: &SolveApp,
    painter: &egui::Painter,
    map: PageMap,
    b: &Board,
    layer: CopperLayer,
    colour: egui::Color32,
    t: &Tokens,
) {
    let picked = &app.electronics.board.selection;
    for p in b.pours.iter().filter(|p| p.layer == layer) {
        let Some(mut line) = screen_ring(map, &p.outline, &p.curves) else { continue };
        if let Some(first) = line.first().copied() {
            line.push(first);
        }
        let stroke = if picked.contains(&p.id) { Stroke::new(2.0, t.sel_edge) } else { Stroke::new(1.0, colour.gamma_multiply(0.8)) };
        painter.extend(dashed(line, stroke, painter.clip_rect()));
    }
}

/// The screen line `line` dashed, or solid once it runs more than [`MAX_DASHED_ROUNDS`] times
/// round `view`.
fn dashed(line: Vec<Pos2>, stroke: Stroke, view: Rect) -> Vec<egui::Shape> {
    let length: f32 = line.windows(2).map(|w| w[0].distance(w[1])).sum();
    if length <= MAX_DASHED_ROUNDS * 2.0 * (view.width() + view.height()) {
        egui::Shape::dashed_line(&line, stroke, 6.0, 4.0)
    } else {
        vec![egui::Shape::line(line, stroke)]
    }
}

/// Lines across the screen ring `line` at 45°, [`HATCH_POINTS`] apart (fewer on a large one, and
/// none once they would cross more than `work` edges in all, which they spend): the pieces of
/// each inside it (even–odd).
fn hatch(line: &[Pos2], work: &mut usize) -> Vec<[Pos2; 2]> {
    let (d, n) = (vec2(1.0, -1.0) / 2f32.sqrt(), vec2(1.0, 1.0) / 2f32.sqrt());
    let across: Vec<f32> = line.iter().map(|p| p.to_vec2().dot(n)).collect();
    let (lo, hi) = across.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| (a.min(*v), b.max(*v)));
    if !(hi > lo) {
        return Vec::new();
    }
    let step = HATCH_POINTS.max((hi - lo) / MAX_HATCH_LINES as f32);
    let mut out = Vec::new();
    let count = ((hi - lo) / step).floor() as usize;
    let k = line.len();
    let cost = (count.min(MAX_HATCH_LINES) + 1).saturating_mul(k);
    if cost > *work {
        return out;
    }
    *work -= cost;
    for i in 0..=count.min(MAX_HATCH_LINES) {
        let s = lo + step * (i as f32 + 0.5);
        let mut at: Vec<f32> = Vec::new();
        for j in 0..k {
            let (Some(a), Some(b), Some(sa), Some(sb)) = (line.get(j), line.get((j + 1) % k), across.get(j), across.get((j + 1) % k)) else {
                continue;
            };
            if (*sa > s) != (*sb > s) {
                let f = (s - sa) / (sb - sa);
                at.push((*a + (*b - *a) * f).to_vec2().dot(d));
            }
        }
        at.sort_by(f32::total_cmp);
        for pair in at.chunks(2) {
            if let [u, v] = pair {
                let base = n * s;
                out.push([(base + d * *u).to_pos2(), (base + d * *v).to_pos2()]);
            }
        }
    }
    out
}

/// The keepouts of `b`, each a hatched outline (the picked ones in the selection colour); those on
/// one layer only in that layer's colour at their edge.
pub(crate) fn paint_keepouts(app: &SolveApp, painter: &egui::Painter, map: PageMap, b: &Board, t: &Tokens) {
    if !keepouts_shown(app) {
        return;
    }
    let picked = &app.electronics.board.selection;
    let clip = painter.clip_rect();
    let mut work = MAX_HATCH_WORK;
    for k in &b.keepouts {
        if !k.layers.is_empty() && !k.layers.iter().any(|l| copper_shown(app, *l)) {
            continue;
        }
        let Some(line) = screen_ring(map, &k.outline, &k.curves) else { continue };
        let ink = if picked.contains(&k.id) { t.sel_edge } else { t.pcb_keepout };
        // Off the screen, no hatch.
        let hatched = if Rect::from_points(&line).intersects(clip) { hatch(&line, &mut work) } else { Vec::new() };
        for [a, c] in hatched {
            painter.line_segment([a, c], Stroke::new(1.0, ink.gamma_multiply(0.6)));
        }
        painter.add(egui::Shape::closed_line(line, Stroke::new(if picked.contains(&k.id) { 2.0 } else { 1.5 }, ink)));
    }
}

/// A click with the Polygon tool, or the Keepout tool's circle or polygon mode: a polygon's
/// corners, closed by a click on its first corner or a double-click; a circle's centre, then a
/// point on its rim. The Polygon tool's first click takes the net under it while none is chosen.
pub(crate) fn click(app: &mut SolveApp, map: PageMap, p: Pos2, free: bool, double: bool) {
    let at = snap(app, map, p, free);
    let (mut points, keepout) = match app.electronics.board.tool.clone() {
        Some(Tool::Pour(points)) => (points, false),
        Some(Tool::Keepout(points)) => (points, true),
        _ => return,
    };
    if keepout && app.electronics.board.keepout.mode == Mode::Circle {
        if double {
            return;
        }
        match points.first() {
            None => points.push(at),
            Some(c) => {
                let radius = (at[0] - c[0]).hypot(at[1] - c[1]);
                if radius > 0.0 {
                    let o = Outline::Circle { center: *c, radius };
                    make_keepout(app, &o);
                    return;
                }
            }
        }
        app.electronics.board.tool = Some(Tool::Keepout(points));
        return;
    }
    if !keepout && points.is_empty() && !app.electronics.board.pour.net_chosen {
        let layer = active_layer(app);
        let net = crate::electronics_board_route::target(app, map, p, free, Some(layer), false, None).net;
        let name = net.and_then(|n| stored_nets(app).name(n).map(str::to_string));
        if let Some(name) = name {
            let o = &mut app.electronics.board.pour;
            o.net = Some(name);
            o.net_chosen = true;
        }
    }
    let first = points.first().map(|q| map.to_screen(q[0], q[1]));
    if points.len() >= 3 && (double || first.is_some_and(|f| f.distance(p) <= PICK_POINTS + 2.0)) {
        set_points(app, points);
        close(app);
        return;
    }
    if points.last() != Some(&at) && points.len() < MAX_OUTLINE_POINTS {
        points.push(at);
    }
    set_points(app, points);
}

fn set_points(app: &mut SolveApp, points: Vec<[f64; 2]>) {
    if let Some(Tool::Pour(p) | Tool::Keepout(p)) = &mut app.electronics.board.tool {
        *p = points;
    }
}

/// Enter with the Polygon or Keepout tool: the polygon clicked so far, closed and made.
pub(crate) fn close(app: &mut SolveApp) {
    match app.electronics.board.tool.clone() {
        Some(Tool::Pour(points)) if points.len() >= 3 => make_pour(app, &points),
        Some(Tool::Keepout(points)) if points.len() >= 3 && app.electronics.board.keepout.mode == Mode::Polygon => {
            make_keepout(app, &Outline::Polygon { points, curves: Vec::new() });
        }
        _ => {}
    }
}

/// Backspace or Escape with the Polygon or Keepout tool: take back the last click. `false` with
/// none.
pub(crate) fn step_back(app: &mut SolveApp) -> bool {
    match &mut app.electronics.board.tool {
        Some(Tool::Pour(points) | Tool::Keepout(points)) => points.pop().is_some(),
        _ => false,
    }
}

fn connect_name(c: Connect) -> &'static str {
    match c {
        Connect::Thermal => "thermal",
        Connect::Solid => "solid",
        Connect::None => "none",
    }
}

/// The pour on the active layer through `points`, with the options' settings; the tool stays on
/// for the next. The status line says what it fills. A refused pour stays drawn.
fn make_pour(app: &mut SolveApp, points: &[[f64; 2]]) {
    let o = app.electronics.board.pour.clone();
    let layer = active_layer(app);
    let mut params = json!({
        "layer": layer.name(),
        "points": points.iter().map(|q| snap_tidy(*q)).collect::<Vec<_>>(),
        "min_width": o.min_width,
        "connect": connect_name(o.connect),
        "thermal": {"spoke_width": o.thermal.spoke_width, "gap": o.thermal.gap, "spokes": o.thermal.spokes},
        "priority": o.priority,
        "remove_islands": o.remove_islands,
    });
    if let Some(n) = &o.net {
        params["net"] = json!(n);
    }
    if let Some(c) = o.clearance {
        params["clearance"] = json!(c);
    }
    if let Some(h) = o.hatch {
        params["hatch"] = json!({"width": h.width, "pitch": h.pitch});
    }
    let Ok(reply) = app.run("ecad.board.pour", params) else { return };
    set_points(app, Vec::new());
    let fill = &reply["pour"]["fill"];
    let n = fill["polygons"].as_u64().unwrap_or(0);
    let text = if n == 0 {
        tr("electronics.board_pour_empty")
    } else {
        let area = crate::electronics_editor::mm((fill["area"].as_f64().unwrap_or(0.0) * 100.0).round() / 100.0);
        let islands = fill["islands_removed"].as_u64().unwrap_or(0).to_string();
        crate::i18n::format("electronics.board_pour_filled", &[("n", &n.to_string()), ("area", &area), ("islands", &islands)])
    };
    app.set_status(text, n == 0);
}

/// The keepout `o` on the options' layers, keeping out what they say; the tool stays on.
pub(crate) fn make_keepout(app: &mut SolveApp, o: &Outline) {
    let k = app.electronics.board.keepout.clone();
    let mut params = outline::params(o);
    params["layers"] = json!(k.layers.iter().map(|l| l.name()).collect::<Vec<_>>());
    params["forbid"] = forbid_params(k.forbid);
    if app.run("ecad.board.keepout", params).is_ok() {
        set_points(app, Vec::new());
        app.set_status(tr("electronics.keepout_made"), false);
    }
}

fn forbid_params(f: Forbid) -> Value {
    json!({"tracks": f.tracks, "vias": f.vias, "pours": f.pours, "components": f.components})
}

/// What the Polygon or Keepout tool would draw with the pointer at `hover`.
pub(crate) fn paint_tool(app: &SolveApp, painter: &egui::Painter, map: PageMap, hover: Pos2, free: bool, t: &Tokens) {
    let (points, colour) = match &app.electronics.board.tool {
        Some(Tool::Pour(points)) => (points, crate::electronics_board::layer_colour(t, active_layer(app))),
        Some(Tool::Keepout(points)) => (points, t.pcb_keepout),
        _ => return,
    };
    let ghost = Stroke::new(1.5, colour);
    let at = snap(app, map, hover, free);
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    painter.circle_stroke(screen(at), 4.0, Stroke::new(1.5, t.sel_edge.gamma_multiply(0.7)));
    let circle = matches!(app.electronics.board.tool, Some(Tool::Keepout(_))) && app.electronics.board.keepout.mode == Mode::Circle;
    match points.first() {
        Some(c) if circle => {
            let r = ((at[0] - c[0]).hypot(at[1] - c[1]) * map.scale()) as f32;
            if r.is_finite() {
                painter.circle_stroke(screen(*c), r, ghost);
            }
        }
        Some(_) => {
            let mut line: Vec<Pos2> = points.iter().map(|q| screen(*q)).collect();
            line.push(screen(at));
            if line.iter().all(|q| q.x.is_finite() && q.y.is_finite()) {
                let mut closed = line.clone();
                if let Some(f) = line.first().copied() {
                    closed.push(f);
                }
                painter.extend(dashed(closed, Stroke::new(1.0, colour.gamma_multiply(0.5)), painter.clip_rect()));
                painter.add(egui::Shape::line(line, ghost));
            }
        }
        None => {}
    }
}

/// A change to a pour's settings, from its fields.
enum Change {
    Net(Option<String>),
    Clearance(Option<f64>),
    MinWidth(f64),
    Connect(Connect),
    Thermal(Thermal),
    Priority(u32),
    RemoveIslands(bool),
    Hatch(Option<Hatch>),
}

impl Change {
    fn apply(self, o: &mut PourOptions) {
        match self {
            Change::Net(n) => {
                o.net = n;
                o.net_chosen = true;
            }
            Change::Clearance(c) => o.clearance = c,
            Change::MinWidth(w) => o.min_width = w,
            Change::Connect(c) => o.connect = c,
            Change::Thermal(th) => o.thermal = th,
            Change::Priority(p) => o.priority = p,
            Change::RemoveIslands(r) => o.remove_islands = r,
            Change::Hatch(h) => o.hatch = h,
        }
    }

    /// The field of `ecad.board.pour.edit` it sets.
    fn params(&self) -> Value {
        let thermal = |th: &Thermal| json!({"spoke_width": th.spoke_width, "gap": th.gap, "spokes": th.spokes});
        match self {
            Change::Net(n) => json!({"net": n}),
            Change::Clearance(c) => json!({"clearance": c}),
            Change::MinWidth(w) => json!({"min_width": w}),
            Change::Connect(c) => json!({"connect": connect_name(*c)}),
            Change::Thermal(th) => json!({"thermal": thermal(th)}),
            Change::Priority(p) => json!({"priority": p}),
            Change::RemoveIslands(r) => json!({"remove_islands": r}),
            Change::Hatch(h) => json!({"hatch": h.map(|h| json!({"width": h.width, "pitch": h.pitch}))}),
        }
    }
}

/// A number field that may be left empty: `Some(None)` when emptied, `Some(Some(v))` with a
/// number; `hint` shows while it is empty.
pub(crate) fn optional_number(app: &mut SolveApp, ui: &mut Ui, key: &str, label: &str, current: Option<f64>, hint: &str) -> Option<Option<f64>> {
    let old = current.map(crate::electronics_editor::mm).unwrap_or_default();
    let mut text = crate::electronics_editor::typed(app, key, &old);
    let response = ui_kit::row(ui, label, |ui| ui_kit::cell_editor(ui, egui::Id::new(("electronics_field", key)), &mut text, hint, false));
    let text = crate::electronics_editor::settle(app, ui, key, &response, text, &old)?;
    if text.trim().is_empty() {
        return Some(None);
    }
    match crate::prefs::parse_number(&text) {
        Some(v) => Some(Some(v)),
        None => {
            app.set_status(crate::i18n::format("electronics.not_a_number", &[("field", label), ("text", &text)]), true);
            None
        }
    }
}

/// The net dropdown labelled `label`: no net, then the design's nets, with a search.
pub(crate) fn net_row(app: &SolveApp, ui: &mut Ui, id: &str, label: &str, current: Option<&str>, search: &mut String) -> Option<Option<String>> {
    net_row_shown(app, ui, id, label, current, None, search)
}

/// [`net_row`] showing `shown` in place of the current net (several objects' nets that differ).
pub(crate) fn net_row_shown(
    app: &SolveApp,
    ui: &mut Ui,
    id: &str,
    label: &str,
    current: Option<&str>,
    shown: Option<&str>,
    search: &mut String,
) -> Option<Option<String>> {
    let names: Vec<String> = stored_nets(app).names.clone();
    let none = tr("electronics.board_no_net_choice");
    // Each entry's control-channel handle: `dialog:<id>:<net>`, the no-net entry's `dialog:<id>:`.
    let handles: Vec<String> = std::iter::once(String::new()).chain(names.iter().cloned()).map(|n| format!("dialog:{id}:{n}")).collect();
    let handle = |i: usize| handles.get(i).map(String::as_str);
    let mut options = vec![ui_kit::SearchOption { value: "", label: &none, handle: handle(0) }];
    options.extend(names.iter().enumerate().map(|(i, n)| ui_kit::SearchOption { value: n.as_str(), label: n.as_str(), handle: handle(i + 1) }));
    let mixed = shown.is_some();
    let shown = shown.unwrap_or(current.unwrap_or(&none)).to_string();
    let chosen = ui_kit::row(ui, label, |ui| ui_kit::searchable_dropdown(ui, id, &shown, search, &options, None, None))?;
    let chosen = (!chosen.is_empty()).then_some(chosen);
    (mixed || chosen.as_deref() != current).then_some(chosen)
}

/// A pour's settings as fields (`key` keeps them apart): net, clearance, least width, the
/// connection and its thermal spokes, priority, islands and hatch. `Some` with what changed.
fn pour_fields(app: &mut SolveApp, ui: &mut Ui, key: &str, o: &PourOptions, search: &mut String) -> Option<Change> {
    let mut change = None;
    if let Some(n) = net_row(app, ui, &format!("electronics_pour_net_{key}"), &tr("electronics.field_net"), o.net.as_deref(), search) {
        change = Some(Change::Net(n));
    }
    let rule = board(app).rules.sizes(o.net.as_deref()).clearance;
    let hint = crate::i18n::format("electronics.board_pour_rule", &[("mm", &crate::electronics_editor::mm(rule))]);
    if let Some(c) = optional_number(app, ui, &format!("{key}:clearance"), &tr("electronics.field_clearance"), o.clearance, &hint) {
        change = Some(Change::Clearance(c));
    }
    if let Some(w) = crate::electronics_editor::number(app, ui, &format!("{key}:min_width"), &tr("electronics.field_min_width"), o.min_width) {
        change = Some(Change::MinWidth(w));
    }
    let connects =
        [(Connect::Thermal, "electronics.pour_thermal"), (Connect::Solid, "electronics.pour_solid"), (Connect::None, "electronics.pour_none")];
    let names: Vec<String> = connects.iter().map(|(_, name)| tr(name)).collect();
    let at = connects.iter().position(|(c, _)| *c == o.connect).unwrap_or(0);
    if let Some((c, _)) =
        crate::electronics_editor::choice(ui, &format!("electronics_pour_connect_{key}"), &tr("electronics.field_connect"), at, &names)
            .and_then(|i| connects.get(i))
    {
        change = Some(Change::Connect(*c));
    }
    if o.connect == Connect::Thermal {
        let th = o.thermal;
        if let Some(w) = crate::electronics_editor::number(app, ui, &format!("{key}:spoke"), &tr("electronics.field_spoke_width"), th.spoke_width) {
            change = Some(Change::Thermal(Thermal { spoke_width: w, ..th }));
        }
        if let Some(g) = crate::electronics_editor::number(app, ui, &format!("{key}:gap"), &tr("electronics.field_thermal_gap"), th.gap) {
            change = Some(Change::Thermal(Thermal { gap: g, ..th }));
        }
        ui_kit::wrapped_row(ui, &tr("electronics.field_spokes"), |ui| {
            for n in [2u8, 4] {
                if ui_kit::choice_chip(ui, &format!("{key}:spokes:{n}"), &n.to_string(), th.spokes == n).clicked() && th.spokes != n {
                    change = Some(Change::Thermal(Thermal { spokes: n, ..th }));
                }
            }
        });
    }
    if let Some(p) = crate::electronics_editor::number(app, ui, &format!("{key}:priority"), &tr("electronics.field_priority"), f64::from(o.priority))
    {
        if p.is_finite() && (0.0..=f64::from(MAX_PRIORITY)).contains(&p) {
            change = Some(Change::Priority(p.round() as u32));
        } else {
            app.set_status(crate::i18n::format("electronics.board_bad_priority", &[("max", &MAX_PRIORITY.to_string())]), true);
        }
    }
    if ui_kit::checkbox_row(ui, &format!("{key}:islands"), &tr("electronics.field_remove_islands"), o.remove_islands) {
        change = Some(Change::RemoveIslands(!o.remove_islands));
    }
    if ui_kit::checkbox_row(ui, &format!("{key}:hatch"), &tr("electronics.field_hatch"), o.hatch.is_some()) {
        change = Some(Change::Hatch(match o.hatch {
            Some(_) => None,
            None => Some(Hatch { width: HATCH_WIDTH, pitch: HATCH_PITCH }),
        }));
    }
    if let Some(h) = o.hatch
        && let Some([width, pitch]) =
            crate::electronics_editor::pair(app, ui, &format!("{key}:hatch_size"), &tr("electronics.field_hatch_size"), [h.width, h.pitch])
    {
        change = Some(Change::Hatch(Some(Hatch { width, pitch })));
    }
    change
}

/// The Polygon tool's options: its layer (the active one), then the pour's settings.
pub(crate) fn pour_options(app: &mut SolveApp, ui: &mut Ui) -> &'static str {
    if let Some(layer) = layer_row(app, ui, "board:pour:layer", active_layer(app)) {
        change_layer(app, layer);
    }
    let o = app.electronics.board.pour.clone();
    let mut search = o.search.clone();
    let change = pour_fields(app, ui, "board:pour", &o, &mut search);
    app.electronics.board.pour.search = search;
    if let Some(c) = change {
        let mut next = app.electronics.board.pour.clone();
        c.apply(&mut next);
        match next.refused() {
            Some(e) => app.set_status(e, true),
            None => app.electronics.board.pour = next,
        }
    }
    "electronics.board_pour_help"
}

/// The layers a keepout keeps clear, as rows: All layers, else each copper layer; `Some` with the
/// new list (empty is every layer). The last layer ticked stays (the status line says to tick All
/// Layers, or another layer first).
fn layer_rows(app: &mut SolveApp, ui: &mut Ui, key: &str, layers: &[CopperLayer]) -> Option<Vec<CopperLayer>> {
    let all = layers.is_empty();
    if ui_kit::checkbox_row(ui, &format!("{key}:all"), &tr("electronics.field_all_layers"), all) {
        return Some(if all { vec![active_layer(app)] } else { Vec::new() });
    }
    if all {
        return None;
    }
    let mut out = None;
    for l in board(app).stack() {
        let on = layers.contains(&l);
        if ui_kit::checkbox_row(ui, &format!("{key}:{}", l.name()), &layer_name(l), on) {
            let mut next: Vec<CopperLayer> = layers.iter().copied().filter(|x| *x != l).collect();
            if !on {
                next.push(l);
            }
            // An empty list would be every layer: the opposite of taking the last one off.
            if next.is_empty() {
                app.set_status(tr("electronics.board_keepout_last_layer"), true);
                continue;
            }
            out = Some(next);
        }
    }
    out
}

/// What a keepout forbids, as checkboxes; `Some` with the new set.
fn forbid_rows(ui: &mut Ui, key: &str, f: Forbid) -> Option<Forbid> {
    let mut out = None;
    for (field, label, on) in [
        ("tracks", "electronics.forbid_tracks", f.tracks),
        ("vias", "electronics.forbid_vias", f.vias),
        ("pours", "electronics.forbid_pours", f.pours),
        ("components", "electronics.forbid_components", f.components),
    ] {
        if ui_kit::checkbox_row(ui, &format!("{key}:forbid:{field}"), &tr(label), on) {
            let mut next = f;
            match field {
                "tracks" => next.tracks = !on,
                "vias" => next.vias = !on,
                "pours" => next.pours = !on,
                _ => next.components = !on,
            }
            out = Some(next);
        }
    }
    out
}

/// The Keepout tool's options: its shape (as the Outline tool's), its layers and what it forbids.
pub(crate) fn keepout_options(app: &mut SolveApp, ui: &mut Ui) -> &'static str {
    let k = app.electronics.board.keepout.clone();
    if let Some(m) = outline::mode_row(ui, "electronics_board_keepout_mode", k.mode) {
        app.electronics.board.keepout.mode = m;
        app.electronics.board.tool = Some(Tool::Keepout(Vec::new()));
    }
    ui_kit::text(ui, &tr("electronics.board_keepout_layers"), ui_kit::Text::Heading("electronics.board_keepout_layers"));
    if let Some(layers) = layer_rows(app, ui, "board:keepout:layers", &k.layers) {
        app.electronics.board.keepout.layers = layers;
    }
    ui_kit::text(ui, &tr("electronics.board_keepout_forbid"), ui_kit::Text::Heading("electronics.board_keepout_forbid"));
    if let Some(f) = forbid_rows(ui, "board:keepout", k.forbid) {
        app.electronics.board.keepout.forbid = f;
    }
    match app.electronics.board.keepout.mode {
        Mode::Rectangle => "electronics.board_keepout_rect_help",
        Mode::Circle => "electronics.board_keepout_circle_help",
        Mode::Polygon => "electronics.board_keepout_polygon_help",
    }
}

/// The picked pour: its layer and settings, each change one `ecad.board.pour.edit`, and what it
/// fills.
pub(crate) fn pour_section(app: &mut SolveApp, ui: &mut Ui, id: u64) {
    let Some(p) = board(app).pour(id).cloned() else { return };
    let fill = stored_nets(app).fills.iter().find(|f| f.pour == id).map(|f| (f.shapes().len(), f.area(), f.islands_removed, f.truncated));
    let title = app.tr_label(crate::electronics_board_layers::command_label("ecad.board.pour")).to_string();
    let edit = |app: &mut SolveApp, mut params: Value| {
        params["pour"] = json!(id);
        drop(app.run("ecad.board.pour.edit", params));
    };
    ui_kit::section(ui, ("electronics_board_pour", id), &title, true, |ui| {
        if let Some(layer) = layer_row(app, ui, &format!("board:{id}:pour_layer"), p.layer) {
            edit(app, json!({"layer": layer.name()}));
        }
        let o = PourOptions::of(&p);
        let mut search = app.electronics.board.pour.search.clone();
        let change = pour_fields(app, ui, &format!("board:{id}:pour"), &o, &mut search);
        app.electronics.board.pour.search = search;
        if let Some(c) = change {
            edit(app, c.params());
        }
        let text = match fill {
            Some((0, ..)) | None => tr("electronics.board_pour_empty"),
            Some((n, area, islands, _)) => crate::i18n::format(
                "electronics.board_pour_fill",
                &[("n", &n.to_string()), ("area", &crate::electronics_editor::mm((area * 100.0).round() / 100.0)), ("islands", &islands.to_string())],
            ),
        };
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_pour_fill"));
        if fill.is_some_and(|f| f.3) {
            ui_kit::text(ui, &tr("electronics.board_pour_truncated"), ui_kit::Text::Error("electronics.board_pour_truncated"));
        }
        let corners = crate::i18n::format("electronics.board_outline_corners", &[("n", &p.outline.len().to_string())]);
        ui_kit::text(ui, &corners, ui_kit::Text::Note("electronics.board_outline_corners"));
        ui_kit::text(ui, &tr("electronics.board_area_picked_help"), ui_kit::Text::Note("electronics.board_area_picked_help"));
    });
}

/// The picked keepout: the layers it keeps clear and what it forbids, each change one
/// `ecad.board.keepout.edit`.
pub(crate) fn keepout_section(app: &mut SolveApp, ui: &mut Ui, id: u64) {
    let Some(k): Option<Keepout> = board(app).keepout(id).cloned() else { return };
    let title = app.tr_label(crate::electronics_board_layers::command_label("ecad.board.keepout")).to_string();
    ui_kit::section(ui, ("electronics_board_keepout", id), &title, true, |ui| {
        ui_kit::text(ui, &tr("electronics.board_keepout_layers"), ui_kit::Text::Heading("electronics.board_keepout_layers"));
        if let Some(layers) = layer_rows(app, ui, &format!("board:{id}:keepout_layers"), &k.layers) {
            let names: Vec<String> = layers.iter().map(|l| l.name()).collect();
            drop(app.run("ecad.board.keepout.edit", json!({"keepout": id, "layers": names})));
        }
        ui_kit::text(ui, &tr("electronics.board_keepout_forbid"), ui_kit::Text::Heading("electronics.board_keepout_forbid"));
        if let Some(f) = forbid_rows(ui, &format!("board:{id}:keepout"), k.forbid) {
            drop(app.run("ecad.board.keepout.edit", json!({"keepout": id, "forbid": forbid_params(f)})));
        }
        let corners = crate::i18n::format("electronics.board_outline_corners", &[("n", &k.outline.len().to_string())]);
        ui_kit::text(ui, &corners, ui_kit::Text::Note("electronics.board_outline_corners"));
        ui_kit::text(ui, &tr("electronics.board_area_picked_help"), ui_kit::Text::Note("electronics.board_area_picked_help"));
    });
}
