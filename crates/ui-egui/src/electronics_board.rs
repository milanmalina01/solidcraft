//! An electronics design's circuit board: the outline, the footprints of the schematic's parts,
//! copper tracks and vias, mounting holes, drawings, and the airwires still to route
//! (`doc.ecad.board`, the `ecad.board.*` commands), on a flat page with its own camera. The
//! toolbar's tools draw the outline (`electronics_board_outline`), bring the parts over from the
//! schematic, route tracks and place vias on the grid, unroute, miter and split tracks
//! (`electronics_board_route`), and place holes and drawings (`electronics_board_draw`); a press
//! picks the object on top, preferring the active side (Shift adds; pressed again at the same
//! spot, the next object under the pointer), a drag moves the picked objects with their tracks and
//! airwires following (Cmd or Ctrl: off the grid), R turns and F flips them to the other side,
//! Delete removes them. A press on a track reshapes it, on a footprint's name or value moves that
//! text, on the outline's edge or a picked outline's handle resizes the board, inside a picked
//! outline (or Shift on its edge) moves it whole. Copper pours and keepouts are drawn and edited in
//! `electronics_board_pour`, the design rules, net classes and the design rule check in
//! `electronics_board_rules`, the layer stack-up and impedance in `electronics_board_stack`. Every
//! change is one command, so undo takes back one edit. A command bar along the top shows the
//! active layer, the grid and the pointer's position, and the Inspector has a Layers tab that
//! shows and hides each layer (`electronics_board_layers`).

use std::collections::BTreeSet;
use std::sync::Arc;

use egui::{Pos2, Rect, Sense, Stroke, Ui, vec2};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::airwires::{Runs, board_nets_with};
use solidcraft_engine::doc::ecad::board::footprint_box;
use solidcraft_engine::doc::ecad::connect::Connectivity;
use solidcraft_engine::doc::ecad::path::{sweep, tessellate};
use solidcraft_engine::doc::ecad::{
    Board, BoardLayer, BoardNets, Connect, CopperLayer, DrawLayer, Graphic, GraphicType, Motion, Side, Via, ViaKind, ViaProtection,
};

use crate::SolveApp;
use crate::electronics_board_draw::{self as draw, Draw, DrawOptions, DrawShape, Which};
pub use crate::electronics_board_layers::Layer;
use crate::electronics_board_layers::{self as layers, DEFAULT_GRID};
use crate::electronics_board_outline::{self as outline, Handle, Mode};
use crate::electronics_board_pour::{self as pour, FillMesh, Grip, KeepoutOptions, PourOptions};
use crate::electronics_board_route::{self as route, Reshape, Route, RouteOptions};
use crate::electronics_board_rules as rules;
use crate::electronics_board_stitch::{self as stitch, StitchOptions};
use crate::electronics_editor::tr;
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;
use crate::ui_kit;

/// A press this close to a via, a track or a pad's centre, in points, picks it.
pub(crate) const PICK_POINTS: f32 = 6.0;
/// The pointer travels this far, in points, before a press on an object moves it.
const DRAG_POINTS: f32 = 3.0;
/// Half the empty board's width when Fit has nothing to frame, in millimetres.
const EMPTY_HALF: f64 = 25.0;
/// Grid lines are drawn at the finest of these multiples of the grid at least this far apart.
const GRID_MULTIPLES: [f64; 10] = [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0];
const MIN_GRID_POINTS: f64 = 8.0;
/// Pad numbers are drawn while a millimetre is at least this many points.
const PAD_NUMBER_SCALE: f64 = 12.0;

fn silkscreen(side: Side) -> Layer {
    match side {
        Side::Top => Layer::TopSilkscreen,
        Side::Bottom => Layer::BottomSilkscreen,
    }
}

/// How far, in millimetres, a drawn arc may stray from its straight pieces at `map`'s scale.
pub(crate) fn arc_tolerance(map: PageMap) -> f64 {
    0.5 / map.scale().max(1e-6)
}

/// What a toolbar tool does on the board.
#[derive(Clone, Debug, PartialEq)]
pub enum Tool {
    /// The outline, as its mode says: a rectangle dragged out, or the circle's centre or the
    /// polygon's corners clicked so far.
    Outline(Vec<[f64; 2]>),
    /// The Route tool: a track through the clicked points, on the active layer.
    Track(Route),
    Via,
    /// A click places a mounting hole.
    Hole,
    /// A DRAW tool: lines, rectangles, circles and text on the drawing layer.
    Draw(Draw),
    /// A click takes a track's or via's routed run away (Shift: all the copper of its net).
    Unroute,
    /// A click cuts a corner, or every corner of the clicked segment's track.
    Miter,
    /// A press adds a corner on a segment and drags it.
    Split,
    /// The Polygon tool: a copper pour's corners clicked so far, on the active layer.
    Pour(Vec<[f64; 2]>),
    /// The Keepout tool, as its mode says: a rectangle dragged out, or the circle's centre or the
    /// polygon's corners clicked so far.
    Keepout(Vec<[f64; 2]>),
    /// The Stitch Vias tool: its pick and preview are in [`BoardView::stitch`].
    Stitch,
    /// A click stamps a board marking; its options are in [`BoardView::marking`] and the
    /// preferences.
    Marking,
}

impl Tool {
    /// The toolbar command it stands for.
    pub(crate) fn command(&self) -> &'static str {
        match self {
            Tool::Outline(_) => "ecad.board.outline",
            Tool::Track(_) => "ecad.board.track",
            Tool::Via => "ecad.board.via",
            Tool::Hole => "ecad.board.hole",
            Tool::Draw(d) => d.shape.command(),
            Tool::Unroute => "ecad.board.unroute",
            Tool::Miter => "ecad.board.track.miter",
            Tool::Split => "ecad.board.track.split",
            Tool::Pour(_) => "ecad.board.pour",
            Tool::Keepout(_) => "ecad.board.keepout",
            Tool::Stitch => "ecad.board.stitch",
            Tool::Marking => "ecad.board.marking",
        }
    }
}

/// A press on picked objects, which moves them once the pointer has travelled, or on a track,
/// which reshapes it; with the Outline tool, the rectangle being dragged out from `from`.
struct Drag {
    press: Pos2,
    from: [f64; 2],
    by: [f64; 2],
    /// Where a held corner or end of a reshaped track, or a held outline handle, goes.
    to: [f64; 2],
    moving: bool,
    reshape: Option<Reshape>,
    /// A handle of the picked outline (from `from`, on the grid, to `to`).
    handle: Option<Handle>,
    /// A footprint's name or value text, from where it was.
    text: Option<(u64, Which, [f64; 2])>,
    /// A handle of the picked pour or keepout (from `from`, on the grid, to `to`).
    area: Option<(u64, Grip)>,
    /// Where the pressed object is (a footprint's origin, a via's or hole's centre, a drawing's or
    /// text's position): a move puts it on the grid.
    anchor: Option<[f64; 2]>,
    /// The press was again on the object the last press picked among several: a click that goes
    /// nowhere picks the next one under the pointer.
    next: bool,
    /// The press was on the outline's edge: a click that goes nowhere keeps the outline picked.
    edge: bool,
    /// The live clearance check of a move or a reshape: what the moved copper was too close to
    /// before it (worked out once a drag), and where it is too close at the offset `by` and the
    /// held point `to` were (worked out again only when they change).
    before: Option<solidcraft_engine::doc::ecad::drc::MovedBefore>,
    clashes: Option<([f64; 4], Vec<[f64; 2]>)>,
    /// Where the pointer was when Cmd went down during the drag: the drag leaves the grid once
    /// the pointer moves on from there, so Cmd pressed only to drop anyway keeps it on the grid.
    free_at: Option<Pos2>,
}

impl Drag {
    fn new(press: Pos2, from: [f64; 2]) -> Self {
        Drag {
            press,
            from,
            by: [0.0; 2],
            to: from,
            moving: false,
            reshape: None,
            handle: None,
            text: None,
            area: None,
            anchor: None,
            next: false,
            free_at: None,
            edge: false,
            before: None,
            clashes: None,
        }
    }
}

/// The board view's tools, pick, layers and the connections worked out last.
pub struct BoardView {
    pub tool: Option<Tool>,
    /// The picked board objects, by id.
    pub selection: BTreeSet<u64>,
    drag: Option<Drag>,
    /// The picked track's segment nearest the press that picked it: Delete takes that alone.
    pub segment: Option<(u64, usize)>,
    /// The outline is picked: its handles show, and the Inspector edits it.
    pub outline_picked: bool,
    /// The picked outline, a rectangle, is edited corner by corner as a polygon: kept with the
    /// outline (corners and sweeps) it was asked for, so it holds only while that is the outline.
    pub polygon: Option<(Vec<[f64; 2]>, Vec<f64>)>,
    /// The active copper layer, which the Route tool routes on (see [`active_layer`]).
    pub layer: CopperLayer,
    /// The Route tool's options (the Via and Miter tools use theirs too).
    pub route: RouteOptions,
    /// The DRAW tools' options and the Hole tool's drill.
    pub draw: DrawOptions,
    /// The grid points snap to, in millimetres (see [`grid`]).
    pub grid: f64,
    /// The layers the Layers tab hides.
    pub hidden: BTreeSet<Layer>,
    /// The Inspector shows its Layers tab.
    pub layers_tab: bool,
    /// The Layer Stack window is open, and its impedance calculator's fields.
    pub stack_open: bool,
    pub impedance: crate::electronics_board_stack::ImpedanceState,
    /// How the Outline tool draws.
    pub outline_mode: Mode,
    /// The width and height the Outline tool's fields set, and its rectangle's corner radius.
    pub outline_size: [f64; 2],
    pub outline_radius: f64,
    /// The Polygon and Keepout tools' options.
    pub pour: PourOptions,
    pub keepout: KeepoutOptions,
    /// The Stitch Vias tool's options, pick and preview.
    pub stitch: StitchOptions,
    /// The Ground connectivity layer's map, and the revision it last tried to solve.
    pub ground: crate::electronics_board_ground::GroundView,
    pub ground_tried: Option<(u64, u64)>,
    /// The Board Marking tool's options for this board.
    pub marking: crate::electronics_board_marking::MarkingOptions,
    /// A box to frame on the next frame (a finding of the design rule check), in millimetres.
    pub frame: Option<[[f64; 2]; 2]>,
    /// The design rule check worked out last, and when the board changed after it.
    pub drc: rules::DrcState,
    /// The design-quality checks as they ran last (see `electronics_board_quality`).
    pub quality: crate::electronics_board_quality::QualityState,
    /// The board's connections, for the document and revision they were worked out for (the
    /// session's, shared with the commands), with the schematic's that they follow from, and the
    /// pours' fills triangled for drawing.
    nets: Option<((u64, u64), Arc<BoardNets>, Arc<Connectivity>, Arc<Vec<FillMesh>>)>,
    /// The routed runs, worked out while the Unroute tool is on, for the document and revision.
    runs: Option<((u64, u64), Runs)>,
    /// The footprints' names and values as laid out for the document and revision.
    texts: Option<((u64, u64), Arc<Vec<draw::FootprintText>>)>,
    /// The last pick among several objects: where, what was under it, and which of them it picked
    /// (a click there again picks the next).
    cycle: Option<(Pos2, Vec<u64>, usize)>,
    /// The objects `ui.selectOther` listed, and where.
    others: Option<(Pos2, Vec<u64>)>,
    /// Cmd (Ctrl) was held on the board the last frame: turns, flips and the Inspector's place
    /// edits then go unchecked, as a drop does.
    free: bool,
    /// The pad of the picked footprint that the press picking it was on, by number: the
    /// Inspector shows how pours join it.
    pub pad: Option<(u64, String)>,
}

impl Default for BoardView {
    fn default() -> Self {
        BoardView {
            tool: None,
            selection: BTreeSet::new(),
            drag: None,
            segment: None,
            outline_picked: false,
            polygon: None,
            layer: CopperLayer::Top,
            route: RouteOptions::default(),
            draw: DrawOptions::default(),
            grid: DEFAULT_GRID,
            // The ground map is worked out only once asked for.
            hidden: BTreeSet::from([Layer::Ground]),
            layers_tab: false,
            stack_open: false,
            impedance: crate::electronics_board_stack::ImpedanceState::default(),
            outline_mode: Mode::default(),
            outline_size: [50.0, 40.0],
            outline_radius: 0.0,
            pour: PourOptions::default(),
            keepout: KeepoutOptions::default(),
            stitch: StitchOptions::default(),
            ground: Default::default(),
            ground_tried: None,
            marking: Default::default(),
            frame: None,
            drc: rules::DrcState::default(),
            quality: crate::electronics_board_quality::QualityState::default(),
            nets: None,
            runs: None,
            texts: None,
            cycle: None,
            others: None,
            free: false,
            pad: None,
        }
    }
}

pub(crate) fn board(app: &SolveApp) -> &Board {
    &app.session.doc.ecad.board
}

/// The active copper layer: the one picked, while the stack has it, else the top.
pub(crate) fn active_layer(app: &SolveApp) -> CopperLayer {
    let layer = app.electronics.board.layer;
    if board(app).has_layer(layer) { layer } else { CopperLayer::Top }
}

pub(crate) fn set_active_layer(app: &mut SolveApp, layer: CopperLayer) {
    app.electronics.board.layer = layer;
}

/// The grid points snap to, in millimetres.
pub(crate) fn grid(app: &SolveApp) -> f64 {
    let g = app.electronics.board.grid;
    if g.is_finite() && g > 0.0 { g } else { DEFAULT_GRID }
}

/// A copper layer's colour: the top's red, the bottom's blue, inner layers each their own in turn.
pub(crate) fn layer_colour(t: &Tokens, layer: CopperLayer) -> egui::Color32 {
    match layer {
        CopperLayer::Top => t.pcb_copper_top,
        CopperLayer::Bottom => t.pcb_copper_bottom,
        CopperLayer::Inner(n) => {
            let cycle = &t.pcb_copper_inner;
            cycle.get(usize::from(n.saturating_sub(1)) % cycle.len()).copied().unwrap_or(t.pcb_copper_bottom)
        }
    }
}

/// Whether the Layers tab shows the copper of `layer`.
pub(crate) fn copper_shown(app: &SolveApp, layer: CopperLayer) -> bool {
    !app.electronics.board.hidden.contains(&Layer::Copper(layer))
}

/// The board's connections as stored, its pours filled (and the routed runs, for the Unroute
/// tool), once per change of the design (the session's, which the command that made the change
/// worked out already); the status line warns when a fill was cut short by its budget.
fn refresh(app: &mut SolveApp) {
    let key = (app.session.uid, app.session.revision);
    if app.electronics.board.nets.as_ref().is_none_or(|(k, ..)| *k != key) {
        let conn = Arc::new(app.session.doc.ecad.connectivity());
        let nets = app.session.board_nets();
        let (meshes, short) = pour::meshes(&nets);
        let truncated = short || nets.fills.iter().any(|f| f.truncated);
        app.electronics.board.nets = Some((key, nets, conn, Arc::new(meshes)));
        if truncated {
            app.set_status(tr("electronics.board_pour_truncated"), true);
        }
    }
    if app.electronics.board.texts.as_ref().is_none_or(|(k, _)| *k != key) {
        let texts = Arc::new(draw::footprint_texts(app, board(app)));
        app.electronics.board.texts = Some((key, texts));
    }
    if matches!(app.electronics.board.tool, Some(Tool::Unroute)) && app.electronics.board.runs.as_ref().is_none_or(|(k, _)| *k != key) {
        let ecad = &app.session.doc.ecad;
        let runs = ecad.board_runs(&ecad.board);
        app.electronics.board.runs = Some((key, runs));
    }
    // A pick that undo or another command took away is forgotten.
    let b = &app.session.doc.ecad.board;
    let gone: Vec<u64> = app.electronics.board.selection.iter().copied().filter(|id| b.kind_of(*id).is_none()).collect();
    for id in gone {
        app.electronics.board.selection.remove(&id);
    }
    // The picked segment, while its track is the pick and has it.
    let s = &app.electronics.board;
    let kept =
        s.segment.is_some_and(|(id, k)| s.selection.len() == 1 && s.selection.contains(&id) && b.track(id).is_some_and(|t| k + 1 < t.points.len()));
    if !kept {
        app.electronics.board.segment = None;
    }
    // An outline that undo took away is no longer picked; one edited as a polygon is so only
    // while picked and the same.
    if app.session.doc.ecad.board.outline.is_empty() {
        app.electronics.board.outline_picked = false;
    }
    let b = &app.session.doc.ecad.board;
    let s = &app.electronics.board;
    if s.polygon.as_ref().is_some_and(|(points, curves)| !s.outline_picked || *points != b.outline || *curves != b.outline_curves) {
        app.electronics.board.polygon = None;
    }
    stitch::refresh(app);
}

/// The connections as stored (worked out now if [`refresh`] has not run yet).
pub(crate) fn stored_nets(app: &SolveApp) -> std::borrow::Cow<'_, BoardNets> {
    match &app.electronics.board.nets {
        Some((key, nets, ..)) if *key == (app.session.uid, app.session.revision) => std::borrow::Cow::Borrowed(nets.as_ref()),
        _ => std::borrow::Cow::Owned(Arc::unwrap_or_clone(app.session.board_nets())),
    }
}

/// The pours' fills triangled for drawing, as [`refresh`] worked them out last (for the board as
/// it was before a drag, or an undo not yet refreshed).
pub(crate) fn stored_meshes(app: &SolveApp) -> Option<&[FillMesh]> {
    app.electronics.board.nets.as_ref().map(|(.., m)| m.as_slice())
}

/// The schematic's connections the stored board's follow from (worked out now if [`refresh`] has
/// not run yet).
fn stored_connectivity(app: &SolveApp) -> Arc<Connectivity> {
    match &app.electronics.board.nets {
        Some((key, _, conn, _)) if *key == (app.session.uid, app.session.revision) => conn.clone(),
        _ => Arc::new(app.session.doc.ecad.connectivity()),
    }
}

/// The footprints' names and values as [`refresh`] laid them out for `key` (the document and
/// revision), while it has them.
pub(crate) fn texts_for(app: &SolveApp, key: (u64, u64)) -> Option<Arc<Vec<draw::FootprintText>>> {
    app.electronics.board.texts.as_ref().filter(|(k, _)| *k == key).map(|(_, t)| t.clone())
}

/// The routed runs as stored, while [`refresh`] has them (the Unroute tool is on).
pub(crate) fn stored_runs(app: &SolveApp) -> Option<&Runs> {
    match &app.electronics.board.runs {
        Some((key, runs)) if *key == (app.session.uid, app.session.revision) => Some(runs),
        _ => None,
    }
}

/// Points on the board snap to the [`grid`] unless `free`.
pub(crate) fn snap(app: &SolveApp, map: PageMap, p: Pos2, free: bool) -> [f64; 2] {
    let (x, y) = map.to_page(p);
    let g = grid(app);
    let tidy = |v: f64| (v * 1e4).round() / 1e4 + 0.0;
    if free { [tidy(x), tidy(y)] } else { [tidy((x / g).round() * g), tidy((y / g).round() * g)] }
}

/// The finest multiple of the grid whose lines are far enough apart on screen.
fn grid_step(app: &SolveApp, map: PageMap) -> f64 {
    let g = grid(app);
    GRID_MULTIPLES.iter().map(|k| g * k).find(|s| s * map.scale() >= MIN_GRID_POINTS).unwrap_or(g * 1000.0)
}

/// The box round a footprint on the board, from its library footprint.
pub(crate) fn placed_box(app: &SolveApp, f: &solidcraft_engine::doc::ecad::BoardFootprint) -> Option<[[f64; 2]; 2]> {
    let placed = app.session.doc.ecad.placed(f)?;
    footprint_box(placed.resolved.footprint).map(|b| f.placement().bbox(b))
}

/// What the board holds as one box, for Fit.
fn bounds(app: &SolveApp) -> Option<[[f64; 2]; 2]> {
    let b = board(app);
    let mut points: Vec<[f64; 2]> = b.outline_bbox().into_iter().flatten().collect();
    for f in &b.footprints {
        points.extend(placed_box(app, f).into_iter().flatten());
    }
    points.extend(b.tracks.iter().flat_map(|t| t.points.iter().copied()));
    points.extend(b.vias.iter().map(|v| v.at));
    points.extend(b.holes.iter().map(|h| h.at));
    points.extend(b.graphics.iter().filter_map(|g| g.graphic.bbox()).flatten());
    points.extend(b.pours.iter().flat_map(|p| p.outline.iter().copied()));
    points.extend(b.keepouts.iter().flat_map(|k| k.outline.iter().copied()));
    solidcraft_engine::doc::ecad::bbox(points)
}

/// Frame the board (or the empty page round its origin) in the free part of `rect`.
fn fit(app: &mut SolveApp, rect: Rect) {
    let b = bounds(app).unwrap_or([[-EMPTY_HALF; 2], [EMPTY_HALF; 2]]);
    frame(app, rect, b, 20.0);
    app.electronics.page.fitted = true;
}

/// Frame the box `[[x0, y0], [x1, y1]]` with a margin, at least `least` millimetres across, in the
/// free part of `rect`.
fn frame(app: &mut SolveApp, rect: Rect, [[x0, y0], [x1, y1]]: [[f64; 2]; 2], least: f64) {
    let content = crate::shell::content_rect(app, rect);
    let area = if content.is_positive() { content } else { rect };
    let (w, h) = (((x1 - x0) * 1.3).max(least), ((y1 - y0) * 1.3).max(least));
    let scale = (f64::from(area.width()) / w).min(f64::from(area.height()) / h);
    if !(scale.is_finite() && scale > 0.0) {
        return;
    }
    let cam = &mut app.electronics.page.cam;
    cam.target.x = (x0 + x1) / 2.0 - f64::from(area.center().x - rect.center().x) / scale;
    cam.target.y = (y0 + y1) / 2.0 + f64::from(area.center().y - rect.center().y) / scale;
    cam.target.z = 0.0;
    cam.distance = f64::from(rect.height()) / (2.0 * scale) / 0.4;
}

/// The board, in the central area.
pub fn show(app: &mut SolveApp, ui: &mut Ui) {
    let rect = ui.max_rect();
    if !rect.is_positive() {
        return;
    }
    let response = ui.interact(rect, ui.id().with("electronics_board"), Sense::click_and_drag());
    // The command bar takes the pointer over it.
    let bar = layers::bar_rect(app, rect);
    if bar.is_positive() {
        ui.interact(bar, ui.id().with("electronics_board_bar"), Sense::click_and_drag());
    }
    app.viewport.rect = Some(rect);
    crate::viewport::refresh_layout(app, ui.ctx().pixels_per_point());
    if !app.electronics.page.fitted {
        fit(app, rect);
    }
    if let Some(b) = app.electronics.board.frame.take() {
        frame(app, rect, b, rules::FRAME_MM);
    }
    refresh(app);
    crate::electronics_board_ground::refresh(app, ui.ctx());
    if !input(app, ui, rect, &response) {
        crate::electronics_workspace::with_page_camera(app, |app| {
            crate::navigation::handle(app, ui, rect, &response, crate::navigation::NavCaps { orbit: false, pick: false });
        });
    }
    drag_clashes(app);
    paint(app, ui, rect, bar);
    layers::command_bar(app, ui, rect, bar);
    crate::scenario::publish_target("electronics:board", rect);
    crate::electronics_design::windows(app, ui.ctx());
    crate::electronics_board_stack::stack_window(app, ui.ctx());
}

pub(crate) fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

/// A point rounded to 0.0001 mm.
pub(crate) fn snap_tidy([x, y]: [f64; 2]) -> [f64; 2] {
    let tidy = |v: f64| (v * 1e4).round() / 1e4 + 0.0;
    [tidy(x), tidy(y)]
}

/// The side whose objects a press prefers: the active layer's (the top for an inner layer).
fn active_side(app: &SolveApp) -> Side {
    if active_layer(app) == CopperLayer::Bottom { Side::Bottom } else { Side::Top }
}

/// Every board object under `p` on a shown layer, the one on top first: vias, footprints by their
/// through-hole pads, then layer by layer the way they are drawn (the active one first) footprints
/// by their surface-mount pads and tracks, a mounting hole or drawing, a pour or keepout by its
/// edge, then footprints by their boxes, those on the active side first and the smaller first (a
/// pour by its fill, or a keepout by its inside, is [`pour::area_at`]'s).
fn hits(app: &SolveApp, map: PageMap, p: Pos2) -> Vec<u64> {
    let b = board(app);
    let nets = stored_nets(app);
    let hidden = &app.electronics.board.hidden;
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    let (x, y) = map.to_page(p);
    let mut out: Vec<u64> = Vec::new();
    let mut add = |id: u64| {
        if !out.contains(&id) {
            out.push(id);
        }
    };
    if !hidden.contains(&Layer::Vias) {
        // Stacked vias: the one on the active layer first, then layer by layer as tracks are.
        let order = pick_order(app, b);
        let rank = |v: &Via| order.iter().position(|l| v.on(*l, b.copper_layers)).unwrap_or(order.len());
        let mut under: Vec<(usize, u64)> = b
            .vias
            .iter()
            .rev()
            .filter(|v| screen(v.at).distance(p) <= ((v.diameter / 2.0 * map.scale()) as f32).max(PICK_POINTS))
            .map(|v| (rank(v), v.id))
            .collect();
        under.sort_by_key(|(rank, _)| *rank);
        under.into_iter().for_each(|(_, id)| add(id));
    }
    let through = |pad: &&solidcraft_engine::doc::ecad::BoardPad| pad.top && pad.bottom;
    if !hidden.contains(&Layer::Pads) {
        nets.pads.iter().rev().filter(through).filter(|pad| pad.contains([x, y])).for_each(|pad| add(pad.footprint));
    }
    for layer in pick_order(app, b) {
        if !copper_shown(app, layer) {
            continue;
        }
        nets.pads.iter().rev().filter(|pad| !through(pad) && pad.on(layer) && pad.contains([x, y])).for_each(|pad| add(pad.footprint));
        b.tracks.iter().rev().filter(|t| t.layer == layer && on_track(map, t, p)).for_each(|t| add(t.id));
    }
    if let Some(id) = draw::hit(app, map, p) {
        add(id);
    }
    if let Some(id) = pour::edge_at(app, map, p) {
        add(id);
    }
    let side = active_side(app);
    let mut boxes: Vec<(bool, f64, u64)> = b
        .footprints
        .iter()
        .rev()
        .filter_map(|f| {
            let [lo, hi] = placed_box(app, f)?;
            Rect::from_two_pos(screen(lo), screen(hi)).expand(2.0).contains(p).then_some((f.side != side, (hi[0] - lo[0]) * (hi[1] - lo[1]), f.id))
        })
        .collect();
    boxes.sort_by(|a, c| a.0.cmp(&c.0).then(a.1.total_cmp(&c.1)));
    boxes.into_iter().for_each(|(_, _, id)| add(id));
    out
}

/// Whether copper (a via, a pad, a track) on a shown layer is under `p`.
fn copper_under(app: &SolveApp, map: PageMap, p: Pos2) -> bool {
    let b = board(app);
    let hidden = &app.electronics.board.hidden;
    let (x, y) = map.to_page(p);
    let via = !hidden.contains(&Layer::Vias)
        && b.vias.iter().any(|v| map.to_screen(v.at[0], v.at[1]).distance(p) <= ((v.diameter / 2.0 * map.scale()) as f32).max(PICK_POINTS));
    let pad = !hidden.contains(&Layer::Pads)
        && stored_nets(app).pads.iter().any(|pad| pad.contains([x, y]) && b.stack().into_iter().any(|l| pad.on(l) && copper_shown(app, l)));
    let track = b.tracks.iter().any(|t| copper_shown(app, t.layer) && on_track(map, t, p));
    via || pad || track
}

/// The board object under `p` on a shown layer, the one on top (see [`hits`]).
fn hit(app: &SolveApp, map: PageMap, p: Pos2) -> Option<u64> {
    hits(app, map, p).into_iter().next()
}

/// The footprint under `p` (by a pad or its box), the one on top (see [`hits`]).
pub(crate) fn footprint_at(app: &SolveApp, map: PageMap, p: Pos2) -> Option<u64> {
    let b = board(app);
    hits(app, map, p).into_iter().find(|id| b.footprint(*id).is_some())
}

/// How the status line and Select Other name the board object `id`: a footprint by its part's
/// designator, the rest by kind and id.
fn object_name(app: &SolveApp, id: u64) -> String {
    use solidcraft_engine::doc::ecad::BoardKind;
    use solidcraft_engine::doc::ecad::drc::DrcItem;
    let ecad = &app.session.doc.ecad;
    let b = &ecad.board;
    let item = match b.kind_of(id) {
        Some(BoardKind::Footprint) => {
            return b.footprint(id).and_then(|f| ecad.schematic.part(f.part)).map_or_else(|| id.to_string(), |p| p.designator.clone());
        }
        Some(BoardKind::Track) => DrcItem::Track(id),
        Some(BoardKind::Via) => DrcItem::Via(id),
        Some(BoardKind::Hole) => DrcItem::Hole(id),
        Some(BoardKind::Graphic) => DrcItem::Graphic(id),
        Some(BoardKind::Pour) => DrcItem::Pour(id),
        Some(BoardKind::Keepout) => DrcItem::Keepout(id),
        None => return id.to_string(),
    };
    rules::item_name(&item)
}

/// What a press at `p` picks: the object on top; pressed again where the last pick among several
/// was, on the object it picked (alone), that object again, so a drag moves it, with `true`: a
/// click there that goes nowhere picks the next one (see [`pick_next`]); with several picked, the
/// one of them under the pointer, so they move together. With more than one there, the status
/// line says which of how many it is.
fn pick_at(app: &mut SolveApp, map: PageMap, p: Pos2, shift: bool) -> Option<(u64, bool)> {
    let list = hits(app, map, p);
    let s = &app.electronics.board;
    if s.selection.len() > 1
        && let Some(id) = list.iter().find(|id| s.selection.contains(id))
    {
        return Some((*id, false));
    }
    if !shift
        && let Some((at, was, i)) = &s.cycle
        && at.distance(p) <= PICK_POINTS
        && *was == list
        && s.selection.len() == 1
        && let Some(id) = list.get(*i).filter(|id| s.selection.contains(id))
    {
        return Some((*id, true));
    }
    let id = list.first().copied();
    if list.len() > 1 && !shift {
        if let Some(id) = id {
            cycle_status(app, 0, list.len(), id);
        }
        app.electronics.board.cycle = Some((p, list, 0));
    } else {
        app.electronics.board.cycle = None;
    }
    id.map(|id| (id, false))
}

/// The status line's "2 of 3: C20" for the pick among several objects.
fn cycle_status(app: &mut SolveApp, n: usize, total: usize, id: u64) {
    let text = crate::i18n::format(
        "electronics.board_pick_cycle",
        &[("n", &(n + 1).to_string()), ("total", &total.to_string()), ("name", &object_name(app, id))],
    );
    app.set_status(text, false);
}

/// A click that went nowhere, again on the object the last pick among several picked: the next
/// object under the pointer is picked instead, round and round.
fn pick_next(app: &mut SolveApp, map: PageMap) {
    let Some((at, list, i)) = app.electronics.board.cycle.clone() else { return };
    let next = (i + 1) % list.len().max(1);
    let Some(id) = list.get(next).copied() else { return };
    cycle_status(app, next, list.len(), id);
    let segment = route::reshape_at(app, map, id, at).map(|r| (r.track, r.segment));
    // A footprint picked under a via in its pad takes that pad, as a press on the pad does.
    let (x, y) = map.to_page(at);
    let pad = stored_nets(app).pads.iter().find(|pad| pad.footprint == id && pad.contains([x, y])).map(|pad| (id, pad.pad.number.clone()));
    let s = &mut app.electronics.board;
    s.selection = BTreeSet::from([id]);
    s.segment = segment;
    s.outline_picked = false;
    s.pad = pad;
    s.cycle = Some((at, list, next));
}

/// The control channel's `ui.selectOther` on the board: `{x, y}` lists every object under that
/// screen point (the one on top first), `{pick: i, add?}` picks row `i` of that list (`add`: into
/// the pick, or out of it), `{close: true}` forgets it; it answers with the list.
pub(crate) fn select_other(app: &mut SolveApp, p: &Value) -> Result<Value, String> {
    if p.get("close").and_then(Value::as_bool) == Some(true) {
        app.electronics.board.others = None;
        return Ok(others(app));
    }
    if let (Some(x), Some(y)) = (p.get("x").and_then(Value::as_f64), p.get("y").and_then(Value::as_f64)) {
        if !(x.is_finite() && y.is_finite() && x.abs() <= 1e6 && y.abs() <= 1e6) {
            return Err("x and y must be screen points within ±1000000".into());
        }
        let rect = app.viewport.rect.ok_or("the board is not on screen")?;
        let map = PageMap::new(&app.electronics.page.cam, rect).ok_or("the board is not on screen")?;
        let at = egui::pos2(x as f32, y as f32);
        let list = hits(app, map, at);
        app.electronics.board.others = Some((at, list));
        return Ok(others(app));
    }
    if let Some(i) = p.get("pick").and_then(Value::as_u64) {
        let Some((_, list)) = &app.electronics.board.others else { return Err("Select Other is not open: give x and y first".into()) };
        let Some(id) = usize::try_from(i).ok().and_then(|i| list.get(i)).copied() else {
            return Err(format!("Select Other has {} rows", list.len()));
        };
        let add = p.get("add").and_then(Value::as_bool).unwrap_or(false);
        let s = &mut app.electronics.board;
        if add {
            if !s.selection.remove(&id) {
                s.selection.insert(id);
            }
        } else {
            s.selection = BTreeSet::from([id]);
        }
        s.segment = None;
        s.outline_picked = false;
        s.others = None;
        let selection: Vec<u64> = s.selection.iter().copied().collect();
        return Ok(json!({"picked": object_name(app, id), "id": id, "selection": selection}));
    }
    Ok(others(app))
}

/// The list `ui.selectOther` opened on the board, as it answers.
fn others(app: &SolveApp) -> Value {
    let s = &app.electronics.board;
    let Some((at, list)) = &s.others else { return json!({"open": false}) };
    let b = board(app);
    let rows: Vec<Value> = list
        .iter()
        .map(|id| {
            let kind = b.kind_of(*id).map(|k| format!("{k:?}").to_lowercase());
            json!({"label": object_name(app, *id), "id": id, "kind": kind, "sel": s.selection.contains(id)})
        })
        .collect();
    json!({"open": true, "at": [at.x, at.y], "total": list.len(), "rows": rows})
}

/// The pointer on the board: the armed tool takes clicks (the Outline tool's rectangle and the
/// Split tool a drag); otherwise a press picks (Shift adds), a drag moves what is picked, reshapes
/// the pressed track, moves a footprint's name or value, or resizes the outline by a handle or an
/// edge, and a double-click picks a track's or via's whole routed run. `true` while a drag holds
/// the pointer, so the page doesn't navigate.
fn input(app: &mut SolveApp, ui: &Ui, rect: Rect, response: &egui::Response) -> bool {
    let Some(map) = PageMap::new(&app.electronics.page.cam, rect) else { return false };
    let (pressed, down, pos, mods) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos(), i.modifiers));
    let free = mods.command || mods.ctrl;
    app.electronics.board.free = free;
    // A press on the command bar is the bar's.
    let pressed = pressed && response.hovered();
    match app.electronics.board.tool.clone() {
        Some(Tool::Outline(_)) if app.electronics.board.outline_mode == Mode::Rectangle => {
            return outline_input(app, ui, map, response, pressed, down, pos, free);
        }
        Some(Tool::Keepout(_)) if app.electronics.board.keepout.mode == Mode::Rectangle => {
            return outline_input(app, ui, map, response, pressed, down, pos, free);
        }
        Some(Tool::Split) => {
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if pressed
                && let Some(p) = pos
                && let Some(r) = route::split_at(app, map, p)
            {
                let (x, y) = map.to_page(p);
                app.electronics.board.drag = Some(Drag { reshape: Some(r), ..Drag::new(p, [x, y]) });
                return true;
            }
            return drag_input(app, ui, map, down, pos, free);
        }
        Some(tool) => {
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            let enter = !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let Some(p) = pos {
                if response.double_clicked() {
                    tool_click(app, map, p, free, true, mods.shift);
                } else if response.clicked() {
                    tool_click(app, map, p, free, false, mods.shift);
                } else if response.secondary_clicked() && matches!(tool, Tool::Track(_)) {
                    // With the Route tool, a right-click changes how it bends.
                    route::cycle_bend(app);
                }
            }
            if enter {
                match tool {
                    Tool::Track(_) => route::finish(app),
                    Tool::Draw(_) => draw::finish(app),
                    Tool::Outline(_) => outline::close(app),
                    Tool::Pour(_) | Tool::Keepout(_) => pour::close(app),
                    Tool::Stitch => stitch::place(app),
                    _ => {}
                }
            }
            return false;
        }
        None => {}
    }
    if response.double_clicked()
        && let Some(p) = pos
        && let Some(id) = route::copper_at(app, map, p, false)
    {
        pick_run(app, id);
        return true;
    }
    if pressed && let Some(p) = pos {
        let (x, y) = map.to_page(p);
        // The picked outline's handles first, then a footprint's name or value text.
        if app.electronics.board.outline_picked
            && let Some(h) = outline::handle_at(app, map, p)
        {
            let from = snap(app, map, p, free);
            app.electronics.board.drag = Some(Drag { handle: Some(h), ..Drag::new(p, from) });
            return true;
        }
        // The picked pour's or keepout's handles.
        if let Some(grip) = pour::grip_at(app, map, p) {
            let from = snap(app, map, p, free);
            app.electronics.board.drag = Some(Drag { area: Some(grip), ..Drag::new(p, from) });
            return true;
        }
        // A footprint's name or value is taken only where no copper is under the pointer: a name
        // drawn over a via or a track never hides it (names come last, as in Select Other).
        if !mods.shift
            && !copper_under(app, map, p)
            && let Some(text) = draw::text_at(app, map, p)
        {
            let s = &mut app.electronics.board;
            s.selection = BTreeSet::from([text.0]);
            s.segment = None;
            s.outline_picked = false;
            s.drag = Some(Drag { text: Some(text), anchor: Some(text.2), ..Drag::new(p, [x, y]) });
            return true;
        }
        match pick_at(app, map, p, mods.shift) {
            Some((id, next)) => {
                // A press on a track takes hold of its segment, corner or end, also with other
                // objects picked (a finding's two items, a run): that track alone is reshaped.
                let reshape = if mods.shift { None } else { route::reshape_at(app, map, id, p) };
                let s = &mut app.electronics.board;
                s.outline_picked = false;
                if mods.shift {
                    if !s.selection.remove(&id) {
                        s.selection.insert(id);
                    }
                } else if reshape.is_some() || !s.selection.contains(&id) {
                    s.selection = BTreeSet::from([id]);
                }
                s.segment = reshape.as_ref().map(|r| (r.track, r.segment));
                let pad =
                    stored_nets(app).pads.iter().find(|pad| pad.footprint == id && pad.contains([x, y])).map(|pad| (id, pad.pad.number.clone()));
                app.electronics.board.pad = pad;
                let anchor = board(app).position(id);
                app.electronics.board.drag = Some(Drag { reshape, anchor, next, ..Drag::new(p, [x, y]) });
                return true;
            }
            // A press on the outline's edge picks it and takes hold of that edge (with Shift, of
            // the whole outline).
            None if let Some(h) = outline::edge_at(app, map, p) => {
                let from = snap(app, map, p, free);
                let s = &mut app.electronics.board;
                s.selection.clear();
                s.segment = None;
                s.outline_picked = true;
                let h = if mods.shift { Handle::Whole } else { h };
                s.drag = Some(Drag { handle: Some(h), edge: true, ..Drag::new(p, from) });
                return true;
            }
            // Inside a picked outline, on bare board, a drag moves it whole; a click lets it go.
            None if !mods.shift
                && app.electronics.board.outline_picked
                && outline::inside_at(app, map, p)
                && pour::area_at(app, map, p).is_none() =>
            {
                let from = snap(app, map, p, free);
                app.electronics.board.drag = Some(Drag { handle: Some(Handle::Whole), ..Drag::new(p, from) });
                return true;
            }
            // A press on a pour's bare fill or inside a keepout picks it but takes no hold, so a
            // plane under everything doesn't move with a stray drag; with something else picked,
            // it lets go of that first.
            None if let Some(id) = pour::area_at(app, map, p) => {
                let s = &mut app.electronics.board;
                s.segment = None;
                s.outline_picked = false;
                if mods.shift {
                    if !s.selection.remove(&id) {
                        s.selection.insert(id);
                    }
                } else if s.selection.is_empty() || s.selection.contains(&id) {
                    s.selection = BTreeSet::from([id]);
                } else {
                    s.selection.clear();
                }
            }
            None if !mods.shift => {
                let s = &mut app.electronics.board;
                s.selection.clear();
                s.segment = None;
                s.outline_picked = false;
            }
            None => {}
        }
    }
    if app.electronics.board.drag.is_none() {
        let over = pos.filter(|_| response.hovered());
        let grab = over.is_some_and(|p| {
            hit(app, map, p).is_some()
                || draw::text_at(app, map, p).is_some()
                || (app.electronics.board.outline_picked && outline::handle_at(app, map, p).is_some())
                || pour::grip_at(app, map, p).is_some()
                || outline::edge_at(app, map, p).is_some()
        });
        if grab {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        return false;
    }
    drag_input(app, ui, map, down, pos, free)
}

/// The drag in hand, once the pointer has travelled: the picked objects (or a footprint's text)
/// move on the grid, the held corner or end of a track or the held outline handle follows the
/// pointer (a held segment slides). Released, one command does it. `true` while there is a drag.
fn drag_input(app: &mut SolveApp, ui: &Ui, map: PageMap, down: bool, pos: Option<Pos2>, free: bool) -> bool {
    let g = grid(app);
    // Off the grid once the pointer moves with Cmd held: Cmd pressed at the drop alone (as a
    // refusal says, to drop anyway) leaves the drag where the grid put it.
    let off_grid = match (app.electronics.board.drag.as_mut(), pos) {
        (Some(d), Some(p)) if free => p.distance(*d.free_at.get_or_insert(p)) > DRAG_POINTS,
        (Some(d), _) => {
            d.free_at = d.free_at.filter(|_| free);
            false
        }
        (None, _) => false,
    };
    let held = match (&app.electronics.board.drag, pos) {
        (Some(Drag { reshape: Some(r), .. }), Some(p)) if down => Some(route::held_point(app, map, r, p, off_grid)),
        (Some(Drag { handle: Some(_), .. } | Drag { area: Some(_), .. }), Some(p)) if down => Some(snap(app, map, p, off_grid)),
        _ => None,
    };
    let Some(drag) = app.electronics.board.drag.as_mut() else { return false };
    if down {
        if let Some(p) = pos {
            drag.moving |= p.distance(drag.press) > DRAG_POINTS;
            let (x, y) = map.to_page(p);
            let raw = [x - drag.from[0], y - drag.from[1]];
            if drag.moving && raw.iter().all(|v| v.is_finite()) {
                // On the grid: the pressed object's position, else the distance moved.
                let on_grid = |v: f64| (v / g).round() * g;
                drag.by = match drag.anchor {
                    _ if off_grid => raw,
                    Some(a) => [on_grid(a[0] + raw[0]) - a[0], on_grid(a[1] + raw[1]) - a[1]],
                    None => raw.map(on_grid),
                };
                drag.to = held.unwrap_or(drag.to);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
        }
        return true;
    }
    let Some(drag) = app.electronics.board.drag.take() else { return true };
    if drag.next && !drag.moving {
        pick_next(app, map);
        return true;
    }
    if let Some((id, grip)) = drag.area {
        if drag.moving {
            pour::release(app, id, grip, drag.from, drag.to);
        }
        return true;
    }
    match (&drag.reshape, drag.handle, drag.text) {
        (Some(r), _, _) => route::release(app, r, drag.moving, drag.by, drag.to),
        (None, Some(h), _) if drag.moving => outline::release(app, h, drag.from, drag.to),
        // A click inside the picked outline that went nowhere lets go of it.
        (None, Some(Handle::Whole), _) if !drag.edge => app.electronics.board.outline_picked = false,
        (None, None, Some((id, which, at))) if drag.moving && drag.by != [0.0; 2] => {
            // Kept as an offset from the footprint's origin, so it follows the footprint.
            if let Some(origin) = board(app).footprint(id).map(|f| f.at) {
                let offset = snap_tidy([at[0] + drag.by[0] - origin[0], at[1] + drag.by[1] - origin[1]]);
                let mut row = json!({"footprint": id});
                row[which.key()] = json!(offset);
                drop(app.run("ecad.board.edit", json!({"footprints": [row]})));
            }
        }
        (None, None, None) if drag.moving && drag.by != [0.0; 2] => {
            let items: Vec<u64> = app.electronics.board.selection.iter().copied().collect();
            // Copper brought too close to another net's is refused, unless Cmd is held at the
            // drop or the Route options allow violations.
            let check = !(free || app.electronics.board.route.allow);
            let by = snap_tidy(drag.by);
            let before = before_forced(app, check);
            let moved: BTreeSet<u64> = items.iter().copied().collect();
            match app.run("ecad.board.move", json!({"items": items, "by": by, "check": check})) {
                // Made anyway: what it broke, if anything.
                Ok(_) => say_forced(app, before.as_ref(), &moved, None),
                Err(_) if check => {
                    let mut after = board(app).clone();
                    let motion = Motion { about: [0.0; 2], mirror: false, rotation: 0, by };
                    app.session.doc.ecad.move_on_board(&mut after, &[(moved.clone(), motion)]);
                    clash_refused(app, &after, &moved, "electronics.board_refused_move");
                }
                Err(_) => {}
            }
        }
        _ => {}
    }
    true
}

/// A double-click on a track or via: the whole routed run it is part of (tracks and vias joined
/// through tracks and vias) is picked.
fn pick_run(app: &mut SolveApp, id: u64) {
    let b = board(app);
    let runs = app.session.doc.ecad.board_runs(b);
    let run = b
        .tracks
        .iter()
        .position(|t| t.id == id)
        .and_then(|i| runs.tracks.get(i))
        .or_else(|| b.vias.iter().position(|v| v.id == id).and_then(|i| runs.vias.get(i)))
        .copied();
    let Some(run) = run else { return };
    let tracks = b.tracks.iter().zip(&runs.tracks).filter(|(_, r)| **r == run).map(|(t, _)| t.id);
    let vias = b.vias.iter().zip(&runs.vias).filter(|(_, r)| **r == run).map(|(v, _)| v.id);
    let picked: BTreeSet<u64> = tracks.chain(vias).collect();
    let s = &mut app.electronics.board;
    s.selection = picked;
    s.segment = None;
    s.outline_picked = false;
    s.drag = None;
}

/// The Outline tool's rectangle: a press starts a corner on the grid, the release sets the outline
/// to the rectangle dragged out (its corners rounded as the options say), and the tool goes down.
/// The Keepout tool's rectangle the same way makes a keepout, and the tool stays on.
#[allow(clippy::too_many_arguments)]
fn outline_input(
    app: &mut SolveApp,
    ui: &Ui,
    map: PageMap,
    response: &egui::Response,
    pressed: bool,
    down: bool,
    pos: Option<Pos2>,
    free: bool,
) -> bool {
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    if pressed && let Some(p) = pos {
        let from = snap(app, map, p, free);
        app.electronics.board.drag = Some(Drag::new(p, from));
        return true;
    }
    let to = pos.map(|p| snap(app, map, p, free));
    let Some(drag) = app.electronics.board.drag.as_mut() else { return false };
    if down {
        if let (Some(p), Some(to)) = (pos, to) {
            drag.moving |= p.distance(drag.press) > DRAG_POINTS;
            drag.by = [to[0] - drag.from[0], to[1] - drag.from[1]];
        }
        return true;
    }
    if let Some(drag) = app.electronics.board.drag.take()
        && drag.moving
        && drag.by[0] != 0.0
        && drag.by[1] != 0.0
    {
        let [x0, y0] = drag.from;
        let (lo, hi) = ([x0.min(x0 + drag.by[0]), y0.min(y0 + drag.by[1])], [x0.max(x0 + drag.by[0]), y0.max(y0 + drag.by[1])]);
        if matches!(app.electronics.board.tool, Some(Tool::Keepout(_))) {
            pour::make_keepout(app, &outline::Outline::Rect { lo: snap_tidy(lo), hi: snap_tidy(hi), radius: 0.0 });
        } else {
            let radius = app.electronics.board.outline_radius;
            outline::done(app, &outline::Outline::Rect { lo: snap_tidy(lo), hi: snap_tidy(hi), radius });
        }
    }
    true
}

/// The board as the drag in hand leaves it: the picked objects moved (the ends of the tracks on
/// them following), or one track reshaped. `None` while nothing is being dragged.
fn dragged_board(app: &SolveApp) -> Option<Board> {
    let s = &app.electronics.board;
    let drag = s.drag.as_ref().filter(|d| d.moving)?;
    if let Some(r) = &drag.reshape {
        let mut out = board(app).clone();
        let (points, curves) = r.apply(drag.by, drag.to, s.route.bend.square());
        if points.len() >= 2
            && let Some(t) = out.tracks.iter_mut().find(|t| t.id == r.track)
        {
            t.points = points;
            t.curves = curves;
        }
        return Some(out);
    }
    if s.tool.is_some() || drag.by == [0.0; 2] || drag.handle.is_some() || drag.text.is_some() || drag.area.is_some() {
        return None;
    }
    let mut out = board(app).clone();
    // As the move command will: the ends of tracks on moved pads and vias follow.
    let moved = (s.selection.clone(), Motion { about: [0.0; 2], mirror: false, rotation: 0, by: drag.by });
    app.session.doc.ecad.move_on_board(&mut out, &[moved]);
    Some(out)
}

/// The live clearance check of a move of picked objects or of a track's reshape (see
/// [`Drag::clashes`]): what the drag brings too close to another net's copper, worked out when
/// the offset changes. The moved copper keeps the nets it had as stored where the dragged board,
/// its pours not filled again, finds none for it, as the drop will.
fn drag_clashes(app: &mut SolveApp) {
    let s = &app.electronics.board;
    let Some(d) = s.drag.as_ref().filter(|d| d.moving && d.handle.is_none() && d.text.is_none() && d.area.is_none()) else {
        return;
    };
    let by = [d.by[0], d.by[1], d.to[0], d.to[1]];
    if s.route.allow || d.clashes.as_ref().is_some_and(|(at, _)| *at == by) {
        return;
    }
    let mut before = d.before.clone();
    // A reshape moves its track alone, whatever else is picked.
    let ids = match &d.reshape {
        Some(r) => BTreeSet::from([r.track]),
        None => s.selection.clone(),
    };
    let Some(moved) = dragged_board(app) else {
        if let Some(d) = app.electronics.board.drag.as_mut() {
            d.clashes = Some((by, Vec::new()));
        }
        return;
    };
    let stored = stored_nets(app);
    let mut worked = board_nets_with(&app.session.doc.ecad, &moved, &stored_connectivity(app), false);
    for (slot, was) in worked.track_nets.iter_mut().zip(&stored.track_nets) {
        if slot.is_none() {
            *slot = *was;
        }
    }
    for (slot, was) in worked.via_nets.iter_mut().zip(&stored.via_nets) {
        if slot.is_none() {
            *slot = *was;
        }
    }
    let (found, _) = solidcraft_engine::doc::ecad::drc::moved_clashes_kept(&app.session.doc.ecad.board, &stored, &moved, &worked, &ids, &mut before);
    let points = found.into_iter().map(|m| m.clash.at).collect();
    drop(stored);
    if let Some(d) = app.electronics.board.drag.as_mut() {
        d.before = before;
        d.clashes = Some((by, points));
    }
}

/// A click with the armed tool; `double` for the second click of a double-click, `shift` with
/// Shift held.
fn tool_click(app: &mut SolveApp, map: PageMap, p: Pos2, free: bool, double: bool, shift: bool) {
    let Some(tool) = app.electronics.board.tool.clone() else { return };
    match tool {
        Tool::Track(_) => route::click(app, map, p, free, double),
        Tool::Outline(_) => outline::click(app, map, p, free, double),
        Tool::Draw(_) => draw::click(app, map, p, free, double),
        Tool::Pour(_) | Tool::Keepout(_) => pour::click(app, map, p, free, double),
        Tool::Stitch => stitch::click(app, map, p, double, shift),
        // The second click of a double-click would only do it again.
        _ if double => {}
        Tool::Via => route::via_click(app, map, p, free),
        Tool::Hole => draw::hole_click(app, map, p, free),
        Tool::Marking => crate::electronics_board_marking::click(app, map, p, free),
        Tool::Unroute => route::unroute_click(app, map, p, shift),
        Tool::Miter => route::miter_click(app, map, p),
        Tool::Split => {}
    }
}

/// The Route tool's width: its own, or the net class's of the net being routed (the rules'
/// without one).
pub(crate) fn track_width(app: &SolveApp) -> f64 {
    app.electronics.board.route.width.unwrap_or_else(|| board(app).rules.sizes(route::routed_net(app).as_deref()).track_width)
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Top => "top",
        Side::Bottom => "bottom",
    }
}

/// R: turn the picked objects a quarter turn: footprints alone each about its origin, a mix as one.
pub(crate) fn rotate(app: &mut SolveApp) -> bool {
    turn(app, 90, false, checked(app))
}

/// Cmd+R: [`rotate`] even where it brings copper too close to another net's.
pub(crate) fn rotate_anyway(app: &mut SolveApp) -> bool {
    turn(app, 90, false, false)
}

/// Whether moves, turns and flips are refused when they bring copper too close to another net's:
/// unless the Route options allow violations or Cmd is held.
pub(crate) fn checked(app: &SolveApp) -> bool {
    !(app.electronics.board.route.allow || app.electronics.board.free)
}

/// F: flip the picked objects to the other side of the board.
pub(crate) fn flip(app: &mut SolveApp) -> bool {
    turn(app, 0, true, checked(app))
}

/// Cmd+F: [`flip`] even where it brings copper too close to another net's.
pub(crate) fn flip_anyway(app: &mut SolveApp) -> bool {
    turn(app, 0, true, false)
}

/// Turn the picked objects by `rotation` (degrees, a multiple of 90), or with `flip` flip them;
/// with `check`, refused where that brings copper too close to another net's, the status line
/// naming the two that clash.
fn turn(app: &mut SolveApp, rotation: u16, flip: bool, check: bool) -> bool {
    let items: Vec<u64> = app.electronics.board.selection.iter().copied().collect();
    if items.is_empty() {
        return false;
    }
    let (id, done, anyway) = if flip {
        ("ecad.board.flip", "electronics.board_flipped", "electronics.board_refused_flip")
    } else {
        ("ecad.board.rotate", "electronics.board_rotated", "electronics.board_refused_rotate")
    };
    let mut params = json!({"items": items, "check": check});
    if !flip {
        params["rotation"] = json!(rotation);
    }
    let before = before_forced(app, check);
    match app.run(id, params) {
        Ok(_) => {
            let text = crate::i18n::format(done, &[("what", &picked_name(app, &items))]);
            let moved: BTreeSet<u64> = items.iter().copied().collect();
            say_forced(app, before.as_ref(), &moved, Some(text));
        }
        Err(_) if check => {
            let mut after = board(app).clone();
            let groups = solidcraft_engine::doc::ecad::rework::turn_groups(board(app), &items, rotation, flip);
            app.session.doc.ecad.move_on_board(&mut after, &groups);
            clash_refused(app, &after, &items.into_iter().collect(), anyway);
        }
        Err(_) => {}
    }
    true
}

/// How the status line names the board objects `items`: the one by its name, else how many.
fn picked_name(app: &SolveApp, items: &[u64]) -> String {
    match items {
        [one] => object_name(app, *one),
        _ => crate::i18n::format("electronics.board_objects", &[("n", &items.len().to_string())]),
    }
}

/// After the clearance check refused a move, turn, flip or reshape that would leave the board as
/// `after` (the objects `ids` moved, the ends of tracks on them following): the status line names
/// the two copper objects of different nets that come too close, and how to do it anyway (the
/// message key `anyway`). The command's own message stays where this finds nothing.
pub(crate) fn clash_refused(app: &mut SolveApp, after: &Board, ids: &BTreeSet<u64>, anyway: &str) {
    let stored = stored_nets(app);
    let worked = solidcraft_engine::doc::ecad::airwires::board_nets(&app.session.doc.ecad, after, &stored_connectivity(app));
    let (found, _) = solidcraft_engine::doc::ecad::drc::moved_clashes(board(app), &stored, after, &worked, ids);
    drop(stored);
    let Some(m) = found.first() else { return };
    let net = |n: Option<usize>| n.and_then(|n| worked.name(n)).map_or_else(|| tr("electronics.board_clash_no_net"), str::to_string);
    let mm = crate::electronics_editor::mm;
    let mut text = crate::i18n::format(
        "electronics.board_refused",
        &[
            ("what", &rules::item_name(&m.moved)),
            ("net", &net(m.net)),
            ("other", &rules::item_name(&m.clash.item)),
            ("other_net", &net(m.clash.net)),
            ("layer", &route::layer_name(m.clash.layer)),
            ("gap", &mm(m.clash.gap)),
            ("required", &mm(m.clash.required)),
        ],
    );
    if found.len() > 1 {
        text.push(' ');
        text.push_str(&crate::i18n::format("electronics.board_refused_more", &[("n", &found.len().to_string())]));
    }
    text.push(' ');
    text.push_str(&tr(anyway));
    app.set_status(text, true);
}

/// The design and its board's connections before a move, turn, flip or reshape made anyway, to
/// say afterwards what it broke (see [`forced_clash`]).
pub(crate) struct Before {
    doc: Arc<solidcraft_engine::doc::Document>,
    nets: Arc<BoardNets>,
}

/// [`Before`], when the clearance check is off for a change (`check` false: Cmd held, or Allow
/// violations in the Route options).
pub(crate) fn before_forced(app: &SolveApp, check: bool) -> Option<Before> {
    (!check).then(|| Before { doc: app.session.doc.clone(), nets: app.session.board_nets() })
}

/// What a change made anyway brought too close, the objects `ids` moved since `before`, for the
/// status line: a short where its copper now touches another net's (naming that net), else the
/// clearance it breaks; with the count when there is more. `None` when it broke none.
pub(crate) fn forced_clash(app: &SolveApp, before: &Before, ids: &BTreeSet<u64>) -> Option<String> {
    let now = app.session.board_nets();
    let (found, _) = solidcraft_engine::doc::ecad::drc::moved_clashes(&before.doc.ecad.board, &before.nets, board(app), &now, ids);
    let m = found.first()?;
    let net = |n: Option<usize>| n.and_then(|n| now.name(n)).map_or_else(|| tr("electronics.board_clash_no_net"), str::to_string);
    let mm = crate::electronics_editor::mm;
    let key = if m.clash.gap <= 0.0 { "electronics.board_forced_short" } else { "electronics.board_forced_close" };
    let mut text = crate::i18n::format(
        key,
        &[
            ("what", &rules::item_name(&m.moved)),
            ("net", &net(m.net)),
            ("other", &rules::item_name(&m.clash.item)),
            ("other_net", &net(m.clash.net)),
            ("layer", &route::layer_name(m.clash.layer)),
            ("gap", &mm(m.clash.gap)),
            ("required", &mm(m.clash.required)),
        ],
    );
    if found.len() > 1 {
        text.push(' ');
        text.push_str(&crate::i18n::format("electronics.board_refused_more", &[("n", &found.len().to_string())]));
    }
    Some(text)
}

/// After a change made anyway (see [`before_forced`]; `None` for a checked one): `done` (what the
/// change says of itself, if anything), and what it broke as an error.
pub(crate) fn say_forced(app: &mut SolveApp, before: Option<&Before>, ids: &BTreeSet<u64>, done: Option<String>) {
    let clash = before.and_then(|b| forced_clash(app, b, ids));
    match (done, clash) {
        (Some(done), Some(clash)) => app.set_status(format!("{done} {clash}"), true),
        (None, Some(clash)) => app.set_status(clash, true),
        (Some(done), None) => app.set_status(done, false),
        (None, None) => {}
    }
}

/// A position typed in the Inspector: object `id` moves there, and a move made anyway (Allow
/// violations) says what it broke.
fn moved_to(app: &mut SolveApp, id: u64, to: [f64; 2]) {
    let check = checked(app);
    let before = before_forced(app, check);
    if app.run("ecad.board.move", json!({"items": [id], "to": to, "check": check})).is_ok() {
        say_forced(app, before.as_ref(), &BTreeSet::from([id]), None);
    }
}

/// Delete on the toolbar (and the workspace's Delete): the picked objects whole (tracks on a
/// deleted footprint's pads stay), or the picked outline. The Delete key takes a picked segment
/// alone (see [`delete_key`]).
pub(crate) fn delete(app: &mut SolveApp) -> bool {
    delete_picked(app, true)
}

/// The Delete and Backspace keys on the board: Backspace first takes back the armed tool's last
/// click; then the picked segment goes alone, or with `whole` (Shift) the whole pick.
pub(crate) fn delete_key(app: &mut SolveApp, backspace: bool, whole: bool) -> bool {
    if backspace && step_back(app) {
        return true;
    }
    delete_picked(app, whole)
}

/// The armed tool's last click taken back (a route's start last). `false` when there was none.
fn step_back(app: &mut SolveApp) -> bool {
    route::step_back(app) || outline::step_back(app) || draw::step_back(app) || pour::step_back(app) || stitch::step_back(app)
}

fn delete_picked(app: &mut SolveApp, whole: bool) -> bool {
    if std::mem::take(&mut app.electronics.board.outline_picked) {
        drop(app.run("ecad.board.outline", json!({"points": []})));
        return true;
    }
    let s = &mut app.electronics.board;
    if let Some((track, segment)) = s.segment.take()
        && !whole
        && s.selection.len() == 1
        && s.selection.contains(&track)
    {
        s.selection.clear();
        drop(app.run("ecad.board.delete", json!({"segments": [{"track": track, "segment": segment}]})));
        return true;
    }
    let items: Vec<u64> = std::mem::take(&mut s.selection).into_iter().collect();
    if items.is_empty() {
        return false;
    }
    let what = picked_name(app, &items);
    if app.run("ecad.board.delete", json!({"items": items})).is_ok() {
        app.set_status(crate::i18n::format("electronics.board_deleted", &[("what", &what)]), false);
    }
    true
}

/// Escape, one thing at a time: a window, the tool's last click (a route's start last), the tool,
/// a drag, the pick.
pub(crate) fn escape(app: &mut SolveApp) -> bool {
    if std::mem::take(&mut app.electronics.sheet.choosing)
        || crate::electronics_design::escape(app)
        || std::mem::take(&mut app.electronics.board.stack_open)
        || step_back(app)
    {
        return true;
    }
    let s = &mut app.electronics.board;
    if s.tool.is_some() {
        s.tool = None;
        s.drag = None;
        return true;
    }
    s.segment = None;
    s.drag.take().is_some() || std::mem::take(&mut s.outline_picked) || !std::mem::take(&mut s.selection).is_empty()
}

/// A toolbar command on the board: its tools arm on the page, Sync, Rotate, Flip and Delete run on
/// the board or the pick, and the rest are the design's (`electronics_design::start`). `true` when
/// taken.
pub(crate) fn start(app: &mut SolveApp, id: &str) -> bool {
    let picked = |app: &mut SolveApp, done: bool| {
        if !done {
            app.set_status(tr("electronics.board_pick_first"), false);
        }
    };
    let tool = match id {
        "ecad.board.outline" => Tool::Outline(Vec::new()),
        "ecad.board.track" => Tool::Track(Route::default()),
        "ecad.board.via" => Tool::Via,
        "ecad.board.hole" => Tool::Hole,
        "ecad.board.stack" => {
            layers::toggle_stack(app);
            return true;
        }
        _ if let Some(shape) = DrawShape::of(id) => {
            // Text starts with a word to place, which the options change.
            if shape == DrawShape::Text && app.electronics.board.draw.text.trim().is_empty() {
                app.electronics.board.draw.text = tr("electronics.board_text_default");
            }
            Tool::Draw(Draw { shape, points: Vec::new() })
        }
        "ecad.board.unroute" => Tool::Unroute,
        "ecad.board.track.miter" => Tool::Miter,
        "ecad.board.track.split" => Tool::Split,
        "ecad.board.pour" => Tool::Pour(Vec::new()),
        "ecad.board.keepout" => Tool::Keepout(Vec::new()),
        "ecad.board.stitch" => Tool::Stitch,
        "ecad.board.marking" => Tool::Marking,
        "ecad.board.rules" | "ecad.drc.inspect" => {
            rules::toggle(app, id);
            return true;
        }
        "ecad.quality.inspect" => {
            crate::electronics_board_quality::toggle(app);
            return true;
        }
        "ecad.board.sync" => {
            sync(app);
            return true;
        }
        "ecad.board.rotate" => {
            let done = rotate(app);
            picked(app, done);
            return true;
        }
        "ecad.board.flip" => {
            let done = flip(app);
            picked(app, done);
            return true;
        }
        "ecad.board.delete" => {
            let done = delete(app);
            picked(app, done);
            return true;
        }
        _ => return crate::electronics_design::start(app, id),
    };
    // A route drawn so far is made before its tool goes.
    finish_route(app);
    // Stitch Vias takes the picked parts, for Remove Stitching under them.
    let parts: Vec<u64> = app.electronics.board.selection.iter().copied().filter(|id| board(app).footprint(*id).is_some()).collect();
    let s = &mut app.electronics.board;
    // A second press puts the tool down.
    let again = s.tool.as_ref().is_some_and(|t| t.command() == tool.command());
    s.tool = (!again).then_some(tool);
    if matches!(s.tool, Some(Tool::Stitch)) && !parts.is_empty() {
        s.stitch.pick = Some(stitch::StitchPick::Parts(parts));
        s.stitch.preview = None;
    }
    s.drag = None;
    s.selection.clear();
    s.segment = None;
    s.outline_picked = false;
    s.layers_tab = false;
    if matches!(s.tool, Some(Tool::Pour(_))) {
        pour::start_pour(app);
    }
    true
}

/// A route with a leg drawn, made into its track (see `route::finish`), as its tool goes down.
fn finish_route(app: &mut SolveApp) {
    if matches!(&app.electronics.board.tool, Some(Tool::Track(r)) if r.has_copper()) {
        route::finish(app);
    }
}

/// The board's tool and any drag put down, as when the view changes; a route drawn so far is made
/// first, so none of its clicks is lost.
pub(crate) fn put_down(app: &mut SolveApp) {
    finish_route(app);
    let s = &mut app.electronics.board;
    s.tool = None;
    s.drag = None;
}

/// Sync from Schematic, saying what it placed, removed and could not place.
fn sync(app: &mut SolveApp) {
    let Ok(reply) = app.run("ecad.board.sync", json!({})) else { return };
    let count = |key: &str| reply.get(key).and_then(Value::as_array).map_or(0, Vec::len);
    let (added, removed) = (count("added"), count("removed"));
    let missing: Vec<&str> = reply.get("no_footprint").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    if missing.is_empty() {
        let text = crate::i18n::format("electronics.board_synced", &[("added", &added.to_string()), ("removed", &removed.to_string())]);
        app.set_status(text, false);
    } else {
        app.set_status(crate::i18n::format("electronics.board_no_footprint", &[("parts", &missing.join(", "))]), true);
    }
    if added > 0 {
        // Frame the board with what came over.
        app.electronics.page.fitted = false;
    }
}

/// The toolbar command to show pressed.
pub(crate) fn running(app: &SolveApp) -> Option<&'static str> {
    if app.electronics.sheet.choosing {
        return Some("ecad.schematic.place");
    }
    let s = &app.electronics.board;
    s.tool.as_ref().map(Tool::command).or_else(|| crate::electronics_design::running(app)).or_else(|| s.stack_open.then_some("ecad.board.stack"))
}

/// A closed polygon with no turn the other way, for a fill egui can draw.
fn convex(points: &[[f64; 2]]) -> bool {
    let n = points.len();
    let mut sign = 0.0f64;
    for i in 0..n {
        let (a, b, c) = (points[i], points[(i + 1) % n], points[(i + 2) % n]);
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if cross.abs() < 1e-12 {
            continue;
        }
        if sign != 0.0 && cross.signum() != sign {
            return false;
        }
        sign = cross.signum();
    }
    true
}

/// A via as its kind shows (stitching vias as any other): a through via a plated disc; a blind or
/// buried one split in two halves in the colours of the layers it ends on, round a plated rim; a
/// micro via a thinner ring with a dot in the middle.
fn paint_via_kind(painter: &egui::Painter, c: Pos2, r: f32, v: &Via, b: &Board, t: &Tokens) {
    match v.kind(b.copper_layers, &b.rules) {
        ViaKind::Through => {
            painter.circle_filled(c, r, t.pcb_plated);
        }
        ViaKind::Blind | ViaKind::Buried => {
            painter.circle_filled(c, r, t.pcb_plated);
            let inner = (r * 0.78).max(1.5);
            for (from, to, layer) in [(90.0_f32, 270.0_f32, v.from), (270.0, 450.0, v.to)] {
                let mut points = vec![c];
                for k in 0..=12 {
                    let a = (from + (to - from) * k as f32 / 12.0).to_radians();
                    points.push(c + egui::vec2(a.cos(), -a.sin()) * inner);
                }
                painter.add(egui::Shape::convex_polygon(points, layer_colour(t, layer), Stroke::NONE));
            }
        }
        ViaKind::Micro => {
            painter.circle_stroke(c, (r * 0.8).max(1.5), Stroke::new((r * 0.35).max(1.0), t.pcb_plated));
            painter.circle_filled(c, (r * 0.28).max(1.0), t.pcb_plated);
        }
    }
}

/// A colour for `layer`'s copper: the layers other than the active one dimmer, and dimmer still
/// while the Route tool is on.
fn copper_colour(app: &SolveApp, t: &Tokens, layer: CopperLayer) -> egui::Color32 {
    let colour = layer_colour(t, layer);
    if active_layer(app) == layer {
        colour
    } else if matches!(app.electronics.board.tool, Some(Tool::Track(_))) {
        colour.gamma_multiply(0.45)
    } else {
        colour.gamma_multiply(0.7)
    }
}

/// The copper layers in the order they are drawn: the bottom first, up through the inner layers
/// to the top, with the active layer last, over the others.
fn draw_order(app: &SolveApp, b: &Board) -> Vec<CopperLayer> {
    let active = active_layer(app);
    let mut order: Vec<CopperLayer> = b.stack().into_iter().rev().filter(|l| *l != active).collect();
    order.push(active);
    order
}

/// The copper layers in the order a press tries them: the one drawn on top first.
pub(crate) fn pick_order(app: &SolveApp, b: &Board) -> Vec<CopperLayer> {
    let mut order = draw_order(app, b);
    order.reverse();
    order
}

/// Whether the press `p` is on track `t`: within its half width or [`PICK_POINTS`] of its line.
pub(crate) fn on_track(map: PageMap, t: &solidcraft_engine::doc::ecad::Track, p: Pos2) -> bool {
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    let reach = ((t.width / 2.0 * map.scale()) as f32).max(PICK_POINTS);
    let line = tessellate(&t.points, &t.curves, false, arc_tolerance(map));
    line.windows(2).any(|w| segment_distance(p, screen(w[0]), screen(w[1])) <= reach)
}

/// Everything on the board, then what the armed tool would place under the pointer (not over the
/// command bar `bar`).
fn paint(app: &SolveApp, ui: &Ui, rect: Rect, bar: Rect) {
    let t = Tokens::get();
    let view = Tokens::view();
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 0.0, t.viewport_bottom);
    let Some(map) = PageMap::new(&app.electronics.page.cam, rect) else { return };
    if app.ui.show_grid {
        crate::electronics_workspace::paint_grid(&painter, map, rect, grid_step(app, map), &t);
    }
    let ecad = &app.session.doc.ecad;
    // While dragging, the board as the drag will leave it: the airwires follow the footprints.
    let dragged = dragged_board(app);
    let stored = stored_nets(app);
    let worked;
    let (b, nets): (&Board, &BoardNets) = match &dragged {
        Some(d) => {
            // The schematic doesn't change during a drag: its connections are the stored ones.
            // The pours are not filled again until the drop (see the airwires below).
            worked = board_nets_with(ecad, d, &stored_connectivity(app), false);
            (d, &worked)
        }
        None => (&ecad.board, &stored),
    };
    let s = &app.electronics.board;
    let shown = |l: Layer| !s.hidden.contains(&l);
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    let finite = |p: &Pos2| p.x.is_finite() && p.y.is_finite();
    let font = egui::FontId::proportional(view.font_small);
    let moving = s.drag.as_ref().filter(|d| d.moving);
    // The outline (as a drag of its handle leaves it), with the board faintly filled where egui can.
    let edited = moving.and_then(|d| Some(outline::ring(&outline::dragged(app, d.handle?, d.from, d.to)?)));
    let editing = edited.is_some();
    let (corners, curves) = edited.unwrap_or_else(|| (b.outline.clone(), b.outline_curves.clone()));
    if shown(Layer::Outline) && corners.len() >= 2 {
        let ring = tessellate(&corners, &curves, true, arc_tolerance(map));
        let points: Vec<Pos2> = ring.iter().map(|q| screen(*q)).collect();
        if points.iter().all(finite) {
            if convex(&ring) {
                painter.add(egui::Shape::convex_polygon(points.clone(), t.pcb_outline.gamma_multiply(0.06), Stroke::NONE));
            }
            if s.outline_picked {
                painter.add(egui::Shape::closed_line(points.clone(), Stroke::new(5.0, t.sel_edge_rim)));
            }
            painter.add(egui::Shape::closed_line(points, Stroke::new(2.0, t.pcb_outline)));
        }
    }
    // Courtyards under everything.
    if shown(Layer::Courtyard) {
        for f in &b.footprints {
            let Some(placed) = ecad.placed(f) else { continue };
            let placement = f.placement();
            for g in placed.resolved.footprint.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Courtyard)) {
                crate::electronics_components::paint_graphic(&painter, map, &placement.graphic(g), t.grid_major, &font);
            }
        }
    }
    // Copper, layer by layer: pour fills and outlines, tracks, surface-mount pads and drawn copper.
    for layer in draw_order(app, b) {
        if !shown(Layer::Copper(layer)) {
            continue;
        }
        let colour = copper_colour(app, &t, layer);
        pour::paint_fills(app, &painter, map, layer, colour, &t);
        pour::paint_outlines(app, &painter, map, b, layer, colour, &t);
        for track in b.tracks.iter().filter(|track| track.layer == layer) {
            let line = tessellate(&track.points, &track.curves, false, arc_tolerance(map));
            let points: Vec<Pos2> = line.iter().map(|q| screen(*q)).collect();
            if !points.iter().all(finite) {
                continue;
            }
            let w = ((track.width * map.scale()) as f32).clamp(1.0, 400.0);
            if s.selection.contains(&track.id) {
                painter.add(egui::Shape::line(points.clone(), Stroke::new(w + 5.0, t.sel_edge_rim)));
            }
            painter.add(egui::Shape::line(points.clone(), Stroke::new(w, colour)));
            for p in points {
                painter.circle_filled(p, w / 2.0, colour);
            }
            // The picked segment, marked along its middle.
            if let Some((_, k)) = s.segment.filter(|(id, _)| *id == track.id && s.drag.is_none())
                && let (Some(a), Some(c)) = (track.points.get(k), track.points.get(k + 1))
            {
                let piece = tessellate(&[*a, *c], &[sweep(&track.curves, k)], false, arc_tolerance(map));
                painter.add(egui::Shape::line(piece.into_iter().map(screen).collect(), Stroke::new(1.5, t.sel_edge)));
            }
        }
        for pad in nets.pads.iter().filter(|pad| pad.on(layer) && !(pad.top && pad.bottom)) {
            let points: Vec<Pos2> = pad.outline(6).into_iter().map(screen).collect();
            if points.iter().all(finite) {
                painter.add(egui::Shape::convex_polygon(points, colour, Stroke::NONE));
            }
        }
        draw::paint_drawings(app, &painter, map, b, DrawLayer::Copper(layer), colour, &t);
    }
    // Plated through-hole pads and vias go through every layer: green, over the copper.
    if shown(Layer::Pads) {
        for pad in nets.pads.iter().filter(|pad| pad.top && pad.bottom) {
            let points: Vec<Pos2> = pad.outline(6).into_iter().map(screen).collect();
            if points.iter().all(finite) {
                painter.add(egui::Shape::convex_polygon(points, t.pcb_plated, Stroke::NONE));
            }
        }
    }
    if shown(Layer::Vias) {
        for v in &b.vias {
            let c = screen(v.at);
            if !finite(&c) {
                continue;
            }
            let r = ((v.diameter / 2.0 * map.scale()) as f32).max(2.0);
            if s.selection.contains(&v.id) {
                painter.circle_filled(c, r + 3.0, t.sel_edge_rim);
            }
            paint_via_kind(&painter, c, r, v, b, &t);
        }
    }
    // The holes drilled through them, in the page's colour.
    if shown(Layer::Drills) {
        for pad in nets.pads.iter().filter(|pad| (pad.top || pad.bottom) && shown(Layer::Pads)) {
            let Some(hole) = pad.drill_outline(6) else { continue };
            let points: Vec<Pos2> = hole.into_iter().map(screen).collect();
            if points.iter().all(finite) {
                painter.add(egui::Shape::convex_polygon(points, t.viewport_bottom, Stroke::NONE));
            }
        }
        // A micro via is laser-drilled and filled: its dot is drawn with it, not a hole.
        for v in b.vias.iter().filter(|v| shown(Layer::Vias) && v.kind(b.copper_layers, &b.rules) != ViaKind::Micro) {
            let c = screen(v.at);
            if finite(&c) {
                painter.circle_filled(c, ((v.drill / 2.0 * map.scale()) as f32).max(1.0), t.viewport_bottom);
            }
        }
    }
    // How weakly the ground is tied on the active layer, over the copper.
    crate::electronics_board_ground::paint(app, &painter, map, &t);
    // Keepouts, hatched over the copper.
    pour::paint_keepouts(app, &painter, map, b, &t);
    // Unplated holes: a footprint's bare holes and the mounting holes, as rings.
    if shown(Layer::Holes) {
        for pad in nets.pads.iter().filter(|pad| !(pad.top || pad.bottom)) {
            let Some(hole) = pad.drill_outline(6) else { continue };
            let points: Vec<Pos2> = hole.into_iter().map(screen).collect();
            if points.iter().all(finite) {
                painter.add(egui::Shape::convex_polygon(points, t.viewport_bottom, Stroke::new(1.0, view.text_dim)));
            }
        }
        draw::paint_holes(app, &painter, map, b, &t);
    }
    // Silkscreen, the bottom's under the top's: the footprints', the board's drawings, and each
    // footprint's name and value.
    // As laid out for the stored board; while picked footprints are dragged, theirs go with them
    // (the rest stay put until the drop).
    let mut texts = Arc::unwrap_or_clone(draw::stored_texts(app));
    if dragged.is_some()
        && let Some(d) = moving.filter(|d| d.reshape.is_none())
    {
        for text in texts.iter_mut().filter(|text| s.selection.contains(&text.footprint)) {
            text.at = [text.at[0] + d.by[0], text.at[1] + d.by[1]];
        }
    }
    let moved_text = moving.and_then(|d| d.text.map(|(id, which, _)| (id, which, d.by)));
    for side in [Side::Bottom, Side::Top] {
        if !shown(silkscreen(side)) {
            continue;
        }
        let colour = if side == Side::Bottom { t.pcb_silkscreen.gamma_multiply(0.6) } else { t.pcb_silkscreen };
        for f in &b.footprints {
            let Some(placed) = ecad.placed(f) else { continue };
            let placement = f.placement();
            let value = placed.part.value.clone().unwrap_or_else(|| placed.resolved.component.value.clone());
            for g in placed.resolved.footprint.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Silkscreen) && !draw::is_placeholder(g)) {
                let on = if placement.mirror { solidcraft_engine::doc::ecad::board::other(g.side) } else { g.side };
                if on != side {
                    continue;
                }
                let mut g = placement.graphic(g);
                if g.kind == GraphicType::Text {
                    g = Graphic { text: g.text.replace("{designator}", &placed.part.designator).replace("{value}", &value), ..g };
                }
                // Text on the bottom reads mirrored, in the box it covers.
                let mirror = if side == Side::Bottom { draw::Mirror::InPlace } else { draw::Mirror::No };
                draw::paint_shape(&painter, map, &g, colour, &font, mirror);
            }
        }
        let drawn = if side == Side::Bottom { DrawLayer::SilkscreenBottom } else { DrawLayer::SilkscreenTop };
        draw::paint_drawings(app, &painter, map, b, drawn, colour, &t);
        for text in texts.iter().filter(|text| text.side == side && draw::text_shown(app, text)) {
            let picked = s.selection.contains(&text.footprint);
            let ink = if picked { t.sel_edge } else { colour };
            match moved_text {
                Some((id, which, by)) if id == text.footprint && which == text.which => {
                    let moved = draw::FootprintText { at: [text.at[0] + by[0], text.at[1] + by[1]], ..text.clone() };
                    draw::paint_text(&painter, map, &moved, ink);
                }
                _ => draw::paint_text(&painter, map, text, ink),
            }
        }
    }
    // The documentation layer: the footprints' fabrication drawings and the board's notes.
    if shown(Layer::Fab) {
        for f in &b.footprints {
            let Some(placed) = ecad.placed(f) else { continue };
            let placement = f.placement();
            for g in placed.resolved.footprint.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Fab) && !draw::is_placeholder(g)) {
                crate::electronics_components::paint_graphic(&painter, map, &placement.graphic(g), t.pcb_documentation, &font);
            }
        }
        draw::paint_drawings(app, &painter, map, b, DrawLayer::Documentation, t.pcb_documentation, &t);
    }
    // Pad numbers, close up.
    if map.scale() >= PAD_NUMBER_SCALE {
        let seen = |pad: &&solidcraft_engine::doc::ecad::BoardPad| {
            if pad.top && pad.bottom {
                shown(Layer::Pads)
            } else {
                (pad.top && shown(Layer::Copper(CopperLayer::Top))) || (pad.bottom && shown(Layer::Copper(CopperLayer::Bottom)))
            }
        };
        for pad in nets.pads.iter().filter(seen) {
            let c = screen(pad.at());
            if finite(&c) {
                painter.text(c, egui::Align2::CENTER_CENTER, &pad.pad.number, font.clone(), t.viewport_bottom);
            }
        }
    }
    // Picked footprints, and each footprint's origin: where it is held and what it turns about.
    for f in &b.footprints {
        let picked = s.selection.contains(&f.id);
        if picked && let Some([lo, hi]) = placed_box(app, f) {
            let r = Rect::from_two_pos(screen(lo), screen(hi)).expand(3.0);
            painter.rect(r, 2.0, t.sel_edge_rim.gamma_multiply(0.2), Stroke::new(1.5, t.sel_edge), egui::StrokeKind::Outside);
        }
        let o = screen(f.at);
        if finite(&o) {
            let r = 4.0;
            let cross = Stroke::new(1.2, if picked { t.sel_edge } else { view.text_dim });
            painter.line_segment([o - vec2(r, 0.0), o + vec2(r, 0.0)], cross);
            painter.line_segment([o - vec2(0.0, r), o + vec2(0.0, r)], cross);
        }
        if let Some(placed) = ecad.placed(f) {
            crate::scenario::publish_handle(&format!("electronics:footprint:{}", placed.part.designator), o);
        }
    }
    // The board's origin.
    let o = screen([0.0, 0.0]);
    if finite(&o) {
        let r = 8.0;
        let [x_axis, y_axis, _] = t.axes();
        painter.line_segment([o - vec2(r, 0.0), o + vec2(r, 0.0)], Stroke::new(1.4, x_axis));
        painter.line_segment([o - vec2(0.0, r), o + vec2(0.0, r)], Stroke::new(1.4, y_axis));
    }
    if shown(Layer::Airwires) {
        // While dragging, the pours are not filled again: the airwires of their nets, which the
        // fill would join, are left out until the drop.
        let poured: BTreeSet<usize> = if dragged.is_some() { stored.fills.iter().filter_map(|f| f.net).collect() } else { BTreeSet::new() };
        for a in nets.airwires.iter().filter(|a| !poured.contains(&a.net)) {
            let (p, q) = (screen(a.from), screen(a.to));
            if finite(&p) && finite(&q) {
                painter.line_segment([p, q], Stroke::new(1.0, t.pcb_airwire));
                // One of no length (copper of the net meeting on two layers that nothing joins
                // there) gets a ring, so it can be found.
                if (a.from[0] - a.to[0]).hypot(a.from[1] - a.to[1]) < 1e-6 {
                    painter.circle_stroke(p, 6.0, Stroke::new(1.5, t.pcb_airwire));
                }
            }
        }
    }
    // While picked objects are dragged, what they would bring too close to another net's copper,
    // ringed in the error colour (the drop is refused unless allowed), on a ring of the page's
    // colour so it reads over copper of the same colour.
    if let (Some(_), Some((_, clashes))) = (&dragged, moving.and_then(|m| m.clashes.as_ref()))
        && !s.route.allow
    {
        for c in clashes {
            let at = screen(*c);
            if finite(&at) {
                painter.circle_stroke(at, 9.0, Stroke::new(5.0, t.viewport_bottom));
                painter.circle_stroke(at, 9.0, Stroke::new(2.5, t.pcb_drc_error));
                painter.circle_filled(at, 3.5, t.viewport_bottom);
                painter.circle_filled(at, 2.5, t.pcb_drc_error);
            }
        }
    }
    if s.outline_picked && !editing {
        outline::paint_handles(app, &painter, map, &t);
    }
    let held = moving.and_then(|d| d.area.map(|(id, grip)| (id, grip, d.from, d.to)));
    pour::paint_grips(app, &painter, map, held, &t);
    rules::paint_markers(app, &painter, map, &t);
    crate::electronics_board_quality::paint_markers(app, &painter, map, &t);
    if b.footprints.is_empty() && s.tool.is_none() {
        let below = rect.with_min_y(bar.bottom().max(rect.top()));
        crate::electronics_workspace::page_note(app, &painter, below, tr("electronics.board_empty"));
    }
    paint_tool(app, ui, &painter, map, rect, bar, &t);
}

/// What the armed tool would do under the pointer.
#[allow(clippy::too_many_arguments)]
fn paint_tool(app: &SolveApp, ui: &Ui, painter: &egui::Painter, map: PageMap, rect: Rect, bar: Rect, t: &Tokens) {
    let s = &app.electronics.board;
    let ghost = t.sel_edge.gamma_multiply(0.7);
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    if let (Some(Tool::Outline(_) | Tool::Keepout(_)), Some(d)) = (&s.tool, &s.drag) {
        let (a, b) = (screen(d.from), screen([d.from[0] + d.by[0], d.from[1] + d.by[1]]));
        painter.rect_stroke(Rect::from_two_pos(a, b), 0.0, Stroke::new(1.5, ghost), egui::StrokeKind::Middle);
        return;
    }
    if matches!(s.tool, Some(Tool::Stitch)) {
        stitch::paint(app, painter, map, t);
    }
    let (hover, free, shift) = ui.ctx().input(|i| (i.pointer.hover_pos(), i.modifiers.command || i.modifiers.ctrl, i.modifiers.shift));
    let Some(hover) = hover.filter(|p| rect.contains(*p) && !bar.contains(*p)) else { return };
    match &s.tool {
        Some(Tool::Outline(_)) if s.outline_mode == Mode::Rectangle => {
            let at = screen(snap(app, map, hover, free));
            painter.circle_stroke(at, 4.0, Stroke::new(1.5, ghost));
        }
        Some(Tool::Outline(_)) => outline::paint_tool(app, painter, map, hover, free, t),
        Some(Tool::Via) => route::paint_via(app, painter, map, hover, free, t),
        Some(Tool::Hole | Tool::Draw(_)) => draw::paint_tool(app, painter, map, hover, free, t),
        Some(Tool::Keepout(_)) if s.keepout.mode == Mode::Rectangle => {
            let at = screen(snap(app, map, hover, free));
            painter.circle_stroke(at, 4.0, Stroke::new(1.5, ghost));
        }
        Some(Tool::Pour(_) | Tool::Keepout(_)) => pour::paint_tool(app, painter, map, hover, free, t),
        Some(Tool::Track(_)) => route::paint(app, painter, map, hover, free, t),
        Some(Tool::Unroute | Tool::Miter | Tool::Split) => route::paint_hover(app, painter, map, hover, shift, t),
        Some(Tool::Stitch | Tool::Marking) | None => {}
    }
}

/// The Inspector on the board: an Inspector tab (the armed tool's options, the picked footprint,
/// track or via, or the board's summary) and a Layers tab.
pub(crate) fn inspector(app: &mut SolveApp, ui: &mut Ui) {
    let t = Tokens::get();
    let labels = [tr("electronics.inspector_title"), tr("electronics.layers_title")];
    let options: Vec<&str> = labels.iter().map(String::as_str).collect();
    let mut selected = usize::from(app.electronics.board.layers_tab);
    let top = ui.cursor().min;
    let width = ui.available_width();
    ui_kit::segmented(ui, &mut selected, &options, ui_kit::Segments { dense: true, proportional: true, ..Default::default() });
    for (i, key) in ["inspector", "layers"].iter().enumerate() {
        let half = Rect::from_min_size(top + vec2(width * i as f32 / 2.0, 0.0), vec2(width / 2.0, ui_kit::row_height()));
        crate::scenario::publish_target(&format!("electronics:board:tab:{key}"), half);
    }
    app.electronics.board.layers_tab = selected == 1;
    ui.add_space(t.space_6);
    if app.electronics.board.layers_tab {
        layers::tab(app, ui);
        return;
    }
    if let Some(tool) = app.electronics.board.tool.clone() {
        let title = solidcraft_engine::find_command(tool.command()).map_or(tool.command(), |c| c.label);
        let title = app.tr_label(title).to_string();
        ui_kit::section(ui, ("electronics_board_tool", tool.command()), &title, true, |ui| tool_options(app, ui, &tool));
    }
    if app.electronics.board.outline_picked {
        outline::section(app, ui);
        return;
    }
    let selection: Vec<u64> = app.electronics.board.selection.iter().copied().collect();
    let b = board(app);
    match selection.as_slice() {
        [id] if b.footprint(*id).is_some() => footprint_section(app, ui, *id),
        [id] if b.track(*id).is_some() => track_section(app, ui, *id),
        [id] if b.via(*id).is_some() => via_section(app, ui, *id),
        [id] if b.hole(*id).is_some() => draw::hole_section(app, ui, *id),
        [id] if b.graphic(*id).is_some() => draw::graphic_section(app, ui, *id),
        [id] if b.pour(*id).is_some() => pour::pour_section(app, ui, *id),
        [id] if b.keepout(*id).is_some() => pour::keepout_section(app, ui, *id),
        [] => summary(app, ui),
        many if many.iter().all(|id| b.via(*id).is_some()) => vias_section(app, ui, many),
        many if many.iter().all(|id| b.track(*id).is_some()) => tracks_section(app, ui, many),
        many => {
            let text = crate::i18n::format("electronics.board_picked", &[("n", &many.len().to_string())]);
            ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_picked"));
        }
    }
}

/// The armed tool's options: the outline's (`electronics_board_outline`), the route's
/// (`electronics_board_route`), the via's size, the miter's, the hole's drill and the drawing's.
fn tool_options(app: &mut SolveApp, ui: &mut Ui, tool: &Tool) {
    let help = match tool {
        Tool::Outline(_) => outline::tool_options(app, ui),
        Tool::Hole => {
            draw::hole_options(app, ui);
            "electronics.board_hole_help"
        }
        Tool::Draw(d) => draw::options(app, ui, d.shape),
        Tool::Track(_) => {
            route::options(app, ui);
            "electronics.board_track_help"
        }
        Tool::Via => {
            route::via_options(app, ui);
            route::allow_row(app, ui);
            "electronics.board_via_help"
        }
        Tool::Unroute => {
            route::unroute_options(app, ui);
            "electronics.board_unroute_help"
        }
        Tool::Miter => {
            route::miter_options(app, ui);
            "electronics.board_miter_help"
        }
        Tool::Split => "electronics.board_split_help",
        Tool::Pour(_) => pour::pour_options(app, ui),
        Tool::Keepout(_) => pour::keepout_options(app, ui),
        Tool::Stitch => stitch::options(app, ui),
        Tool::Marking => crate::electronics_board_marking::options(app, ui),
    };
    ui_kit::text(ui, crate::i18n::tr(help), ui_kit::Text::Note(help));
}

/// The picked footprint: its part's designator and value, the library footprint, its side, turn
/// and position.
fn footprint_section(app: &mut SolveApp, ui: &mut Ui, id: u64) {
    let ecad = &app.session.doc.ecad;
    let Some(f) = ecad.board.footprint(id).cloned() else { return };
    let Some((part, designator, value, library_value, name)) = ecad
        .placed(&f)
        .map(|p| (p.part.id, p.part.designator.clone(), p.part.value.clone(), p.resolved.component.value.clone(), p.resolved.footprint.name.clone()))
    else {
        return;
    };
    ui_kit::section(ui, ("electronics_board_footprint", id), &designator, true, |ui| {
        let key = |field: &str| format!("board:{id}:{field}");
        // The designator and value are the schematic part's.
        if let Some(d) = crate::electronics_editor::field(app, ui, &key("designator"), crate::i18n::tr("electronics.field_designator"), &designator) {
            drop(app.run("ecad.schematic.edit", json!({"parts": [{"part": part, "designator": d}]})));
        }
        let shown = value.clone().unwrap_or_else(|| library_value.clone());
        if let Some(v) = crate::electronics_editor::field(app, ui, &key("value"), crate::i18n::tr("electronics.field_value"), &shown) {
            let value = if v.trim().is_empty() || v == library_value { Value::Null } else { json!(v) };
            drop(app.run("ecad.schematic.edit", json!({"parts": [{"part": part, "value": value}]})));
        }
        let mut side = f.side;
        ui_kit::wrapped_row(ui, crate::i18n::tr("electronics.field_side"), |ui| {
            for (s, label) in [(Side::Top, "electronics.side_top"), (Side::Bottom, "electronics.side_bottom")] {
                if ui_kit::choice_chip(ui, &format!("board:{id}:side:{}", side_name(s)), crate::i18n::tr(label), side == s).clicked() {
                    side = s;
                }
            }
        });
        // The footprint is the pick, so a flip or turn here is the board's own (refused, and
        // said so, as R and F are).
        if side != f.side {
            turn(app, 0, true, checked(app));
        }
        // A typed angle goes to the nearest quarter turn.
        if let Some(r) = crate::electronics_editor::number(app, ui, &key("rotation"), &tr("electronics.field_rotation"), f64::from(f.rotation))
            && r.is_finite()
        {
            let turn = ((r / 90.0).round() as i64).rem_euclid(4);
            let by = (turn - i64::from(f.rotation / 90)).rem_euclid(4) * 90;
            if let Ok(by) = u16::try_from(by)
                && by != 0
            {
                self::turn(app, by, false, checked(app));
            }
        }
        if let Some(to) = crate::electronics_editor::pair(app, ui, &key("at"), crate::i18n::tr("electronics.field_position"), f.at) {
            moved_to(app, id, to);
        }
        // The name and value texts on the silkscreen; a drag on the board moves each.
        for (field, label, on) in
            [("show_name", "electronics.board_show_name", f.show_name), ("show_value", "electronics.board_show_value", f.show_value)]
        {
            if ui_kit::checkbox_row(ui, &key(field), crate::i18n::tr(label), on) {
                let mut row = json!({"footprint": id});
                row[field] = json!(!on);
                drop(app.run("ecad.board.edit", json!({"footprints": [row]})));
            }
        }
        if (f.name_at.is_some() || f.value_at.is_some())
            && crate::electronics_editor::button(ui, &format!("board-{id}-text-home"), &tr("electronics.board_text_home"), true)
        {
            drop(app.run("ecad.board.edit", json!({"footprints": [{"footprint": id, "name_at": null, "value_at": null}]})));
        }
        // How pours join its pads (each pour's own setting unless set here), and the pressed pad.
        if let Some((how, words)) =
            connect_row(ui, &key("pour_connect"), &tr("electronics.board_pour_connect"), f.pour_connect, "electronics.board_connect_pour")
            && app.run("ecad.board.pour.connect", json!({"footprint": id, "connect": how})).is_ok()
        {
            app.set_status(crate::i18n::format("electronics.board_connect_set", &[("what", &designator), ("how", &tr(words))]), false);
        }
        if let Some((_, number)) = app.electronics.board.pad.clone().filter(|(f, _)| *f == id) {
            let label = crate::i18n::format("electronics.board_pad_connect", &[("pad", &number)]);
            let own = f.pad_connect.get(&number).copied();
            if let Some((how, words)) = connect_row(ui, &key("pad_connect"), &label, own, "electronics.board_connect_footprint")
                && app.run("ecad.board.pour.connect", json!({"footprint": id, "pads": [number.clone()], "connect": how})).is_ok()
            {
                let what = crate::i18n::format("electronics.board_pad_of", &[("part", &designator), ("pad", &number)]);
                app.set_status(crate::i18n::format("electronics.board_connect_set", &[("what", &what), ("how", &tr(words))]), false);
            }
            if let Some(pins) = pad_pins(app, &f, &number) {
                let text = crate::i18n::format("electronics.board_pad_pins", &[("pad", &number), ("pins", &pins)]);
                ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_pad_pins"));
            }
        }
        // Pad overrides left for pads the footprint no longer has (its part's footprint changed).
        let stale: Vec<String> = {
            let nets = stored_nets(app);
            f.pad_connect.keys().filter(|n| !nets.pads.iter().any(|p| p.footprint == id && p.pad.number == **n)).cloned().collect()
        };
        if !stale.is_empty() {
            let text = crate::i18n::format("electronics.board_stale_connect", &[("pads", &stale.join(", "))]);
            ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_stale_connect"));
            if crate::electronics_editor::button(ui, &format!("board-{id}-stale-connect"), &tr("electronics.board_stale_connect_clear"), true) {
                drop(app.run("ecad.board.pour.connect", json!({"footprint": id, "pads": stale, "connect": null})));
            }
        }
        let footprint = if name.is_empty() { tr("electronics.board_no_library_footprint") } else { name };
        let text = crate::i18n::format("electronics.board_footprint", &[("name", &footprint)]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_footprint"));
    });
}

/// The names of the pins on the pad `number` of `f` as plain text, a bar over letters (`~{CE}`)
/// as a mark after them (`CE¯`); `None` when no pin there has a name of its own.
fn pad_pins(app: &SolveApp, f: &solidcraft_engine::doc::ecad::BoardFootprint, number: &str) -> Option<String> {
    let placed = app.session.doc.ecad.placed(f)?;
    let names: Vec<String> = placed
        .resolved
        .connections()
        .into_iter()
        .filter(|(_, pads)| pads.contains(&number))
        .filter_map(|(pin, _)| placed.resolved.symbol.pin(pin))
        .filter(|p| !p.name.is_empty() && p.name != p.number)
        .map(|p| {
            // A name barred whole (an active-low pin's) says so in words; one barred in part
            // keeps the mark after its barred letters.
            let b = solidcraft_engine::doc::ecad::overbar::parse(&p.name);
            match b.bars.as_slice() {
                [bar] if bar.start == 0 && bar.end == b.text.chars().count() => {
                    crate::i18n::format("electronics.pin_active_low", &[("name", &b.text)])
                }
                _ => solidcraft_engine::doc::ecad::overbar::marked(&p.name).into_owned(),
            }
        })
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// A row of chips for how pours join pads: as set further up (`inherit`, a message key), thermal,
/// solid or none. The connection picked (`null` for the inherited one) and its chip's message key,
/// when one is.
fn connect_row<'a>(ui: &mut Ui, key: &str, label: &str, now: Option<Connect>, inherit: &'a str) -> Option<(Value, &'a str)> {
    let mut picked = None;
    // The chips wrap onto more lines at the Inspector's width, each in reach.
    ui_kit::wrapped_row(ui, label, |ui| {
        let options = [
            (None, inherit),
            (Some(Connect::Thermal), "electronics.pour_thermal"),
            (Some(Connect::Solid), "electronics.pour_solid"),
            (Some(Connect::None), "electronics.pour_none"),
        ];
        for (how, words) in options {
            let name = how.map_or("inherit", connect_name);
            if ui_kit::choice_chip(ui, &format!("{key}:{name}"), crate::i18n::tr(words), now == how).clicked() && now != how {
                picked = Some((how.map_or(Value::Null, |c| json!(connect_name(c))), words));
            }
        }
    });
    picked
}

fn connect_name(c: Connect) -> &'static str {
    match c {
        Connect::Thermal => "thermal",
        Connect::Solid => "solid",
        Connect::None => "none",
    }
}

/// The picked track: its layer and width, its net, its length and segments, and the segment
/// picked with it.
fn track_section(app: &mut SolveApp, ui: &mut Ui, id: u64) {
    let Some(track) = board(app).track(id).cloned() else { return };
    let net = net_of(app, |b, nets| b.tracks.iter().position(|t| t.id == id).and_then(|i| nets.track_nets.get(i).copied().flatten()));
    let title = tr("electronics.board_track");
    ui_kit::section(ui, ("electronics_board_track", id), &title, true, |ui| {
        if let Some(layer) = route::layer_row(app, ui, &format!("board:{id}:layer"), track.layer) {
            drop(app.run("ecad.board.track.edit", json!({"tracks": [id], "layer": layer.name()})));
        }
        if let Some(w) = route::width_row(app, ui, &format!("board:{id}:width"), &tr("electronics.field_track_width"), track.width) {
            drop(app.run("ecad.board.track.edit", json!({"tracks": [id], "width": w})));
        }
        net_note(ui, net);
        let length = solidcraft_engine::doc::ecad::path::length(&track.points, &track.curves, false);
        let segments = track.points.len().saturating_sub(1).to_string();
        let text = crate::i18n::format("electronics.board_track_length", &[("length", &mm(length)), ("segments", &segments)]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_track_length"));
        if let Some((_, k)) = app.electronics.board.segment.filter(|(t, _)| *t == id) {
            let text = crate::i18n::format("electronics.board_segment_picked", &[("n", &(k + 1).to_string())]);
            ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_segment_picked"));
        }
    });
}

/// The picked via: where it is, the layers it spans (and so its kind), its size, how its hole is
/// protected (open, tented, plugged, filled, filled and capped: a via in a pad needs the last) and
/// its net, each change one command.
fn via_section(app: &mut SolveApp, ui: &mut Ui, id: u64) {
    let Some(via) = board(app).via(id).cloned() else { return };
    let net = net_of(app, |b, nets| b.vias.iter().position(|v| v.id == id).and_then(|i| nets.via_nets.get(i).copied().flatten()));
    let b = board(app);
    let kind = route::via_kind_name(via.kind(b.copper_layers, &b.rules));
    let title = app.tr_label("Via").to_string();
    let edit = |app: &mut SolveApp, mut params: Value| {
        params["vias"] = json!([id]);
        drop(app.run("ecad.board.via.edit", params));
    };
    ui_kit::section(ui, ("electronics_board_via", id), &title, true, |ui| {
        if let Some(to) = crate::electronics_editor::pair(app, ui, &format!("board:{id}:at"), crate::i18n::tr("electronics.field_position"), via.at) {
            moved_to(app, id, to);
        }
        if let Some(l) = route::span_row(app, ui, &format!("board:{id}:via_from"), &tr("electronics.field_via_from"), via.from) {
            edit(app, json!({"from": l.name()}));
        }
        if let Some(l) = route::span_row(app, ui, &format!("board:{id}:via_to"), &tr("electronics.field_via_to"), via.to) {
            edit(app, json!({"to": l.name()}));
        }
        ui_kit::text(ui, &tr(kind), ui_kit::Text::Note("electronics.board_via_kind"));
        if let Some([diameter, drill]) = crate::electronics_editor::pair(
            app,
            ui,
            &format!("board:{id}:via_size"),
            crate::i18n::tr("electronics.field_via"),
            [via.diameter, via.drill],
        ) {
            edit(app, json!({"diameter": diameter, "drill": drill}));
        }
        let shown = tr(&format!("electronics.via_protection_{}", via.protection.name()));
        let mut protection = None;
        ui_kit::row(ui, crate::i18n::tr("electronics.field_via_protection"), |ui| {
            // Each entry is the handle `dialog:electronics_via_protection:<name>`.
            ui_kit::dropdown_menu(ui, ("electronics_via_protection", id), &shown, |ui| {
                for p in VIA_PROTECTIONS {
                    let r = ui_kit::list_picker(
                        ui,
                        &format!("via_protection_{}", p.name()),
                        &tr(&format!("electronics.via_protection_{}", p.name())),
                        via.protection == p,
                    );
                    crate::scenario::publish_handle(&format!("dialog:electronics_via_protection:{}", p.name()), r.rect.center());
                    if r.clicked() {
                        protection = Some(p);
                    }
                }
            });
        });
        if let Some(p) = protection.filter(|p| *p != via.protection) {
            edit(app, json!({"protection": p.name()}));
        }
        net_note(ui, net);
        // A net of its own: a stitching via, joined to the pours of that net.
        let mut search = app.electronics.board.pour.search.clone();
        let own = pour::net_row(app, ui, &format!("electronics_via_net_{id}"), &tr("electronics.field_own_net"), via.net.as_deref(), &mut search);
        app.electronics.board.pour.search = search;
        if let Some(n) = own {
            edit(app, json!({"net": n}));
        }
    });
}

/// The one value `values` share, if they do.
fn shared<T: PartialEq + Copy>(values: impl IntoIterator<Item = T>) -> Option<T> {
    let mut it = values.into_iter();
    let first = it.next()?;
    it.all(|v| v == first).then_some(first)
}

/// A number field for several objects: their shared value, or empty with "Mixed" where they
/// differ. `Some` with a number typed.
fn shared_number(app: &mut SolveApp, ui: &mut Ui, key: &str, label: &str, values: &[f64]) -> Option<f64> {
    match shared(values.iter().map(|v| (v * 1e4).round() as i64)) {
        Some(_) => values.first().and_then(|v| crate::electronics_editor::number(app, ui, key, label, *v)),
        None => pour::optional_number(app, ui, key, label, None, &tr("electronics.board_mixed")).flatten(),
    }
}

/// Several vias picked (and nothing else): the fields they share, "Mixed" where they differ; an
/// edit changes them all in one `ecad.board.via.edit`.
fn vias_section(app: &mut SolveApp, ui: &mut Ui, ids: &[u64]) {
    let b = board(app);
    let vias: Vec<solidcraft_engine::doc::ecad::Via> = ids.iter().filter_map(|id| b.via(*id).cloned()).collect();
    let title = crate::i18n::format("electronics.board_vias_picked", &[("n", &vias.len().to_string())]);
    let edit = |app: &mut SolveApp, mut params: Value| {
        params["vias"] = json!(ids);
        if app.run("ecad.board.via.edit", params).is_ok() {
            app.set_status(crate::i18n::format("electronics.board_vias_changed", &[("n", &ids.len().to_string())]), false);
        }
    };
    ui_kit::section(ui, ("electronics_board_vias", ids.len()), &title, true, |ui| {
        let diameters: Vec<f64> = vias.iter().map(|v| v.diameter).collect();
        let drills: Vec<f64> = vias.iter().map(|v| v.drill).collect();
        if let Some(d) = shared_number(app, ui, "board:vias:diameter", &tr("electronics.field_via_diameter"), &diameters) {
            edit(app, json!({"diameter": d}));
        }
        if let Some(d) = shared_number(app, ui, "board:vias:drill", &tr("electronics.field_via_drill"), &drills) {
            edit(app, json!({"drill": d}));
        }
        if let Some(l) =
            route::shared_layer_row(app, ui, "board:vias:via_from", &tr("electronics.field_via_from"), shared(vias.iter().map(|v| v.from)))
        {
            edit(app, json!({"from": l.name()}));
        }
        if let Some(l) = route::shared_layer_row(app, ui, "board:vias:via_to", &tr("electronics.field_via_to"), shared(vias.iter().map(|v| v.to))) {
            edit(app, json!({"to": l.name()}));
        }
        let protection = shared(vias.iter().map(|v| v.protection));
        let shown = protection.map_or_else(|| tr("electronics.board_mixed"), |p| tr(&format!("electronics.via_protection_{}", p.name())));
        let mut picked = None;
        ui_kit::row(ui, crate::i18n::tr("electronics.field_via_protection"), |ui| {
            ui_kit::dropdown_menu(ui, ("electronics_vias_protection", ids.len()), &shown, |ui| {
                for p in VIA_PROTECTIONS {
                    let r = ui_kit::list_picker(
                        ui,
                        &format!("vias_protection_{}", p.name()),
                        &tr(&format!("electronics.via_protection_{}", p.name())),
                        protection == Some(p),
                    );
                    crate::scenario::publish_handle(&format!("dialog:electronics_vias_protection:{}", p.name()), r.rect.center());
                    if r.clicked() {
                        picked = Some(p);
                    }
                }
            });
        });
        if let Some(p) = picked.filter(|p| Some(*p) != protection) {
            edit(app, json!({"protection": p.name()}));
        }
        let own: Vec<Option<String>> = vias.iter().map(|v| v.net.clone()).collect();
        let same = own.first().filter(|n| own.iter().all(|m| m == *n)).cloned();
        let mixed = tr("electronics.board_mixed");
        let mut search = app.electronics.board.pour.search.clone();
        let current = same.clone().flatten();
        let shown = same.is_none().then_some(mixed.as_str());
        let chosen = pour::net_row_shown(app, ui, "electronics_vias_net", &tr("electronics.field_own_net"), current.as_deref(), shown, &mut search);
        app.electronics.board.pour.search = search;
        if let Some(n) = chosen {
            edit(app, json!({"net": n}));
        }
        let text = crate::i18n::format("electronics.board_picked", &[("n", &ids.len().to_string())]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_picked"));
    });
}

/// Several tracks picked (and nothing else): their shared layer and width, "Mixed" where they
/// differ; an edit changes them all in one `ecad.board.track.edit`.
fn tracks_section(app: &mut SolveApp, ui: &mut Ui, ids: &[u64]) {
    let b = board(app);
    let tracks: Vec<solidcraft_engine::doc::ecad::Track> = ids.iter().filter_map(|id| b.track(*id).cloned()).collect();
    let title = crate::i18n::format("electronics.board_tracks_picked", &[("n", &tracks.len().to_string())]);
    ui_kit::section(ui, ("electronics_board_tracks", ids.len()), &title, true, |ui| {
        if let Some(l) =
            route::shared_layer_row(app, ui, "board:tracks:layer", &tr("electronics.field_layer"), shared(tracks.iter().map(|t| t.layer)))
        {
            drop(app.run("ecad.board.track.edit", json!({"tracks": ids, "layer": l.name()})));
        }
        let widths: Vec<f64> = tracks.iter().map(|t| t.width).collect();
        if let Some(w) = shared_number(app, ui, "board:tracks:width", &tr("electronics.field_track_width"), &widths) {
            drop(app.run("ecad.board.track.edit", json!({"tracks": ids, "width": w})));
        }
        let length: f64 = tracks.iter().map(|t| solidcraft_engine::doc::ecad::path::length(&t.points, &t.curves, false)).sum();
        let text = crate::i18n::format("electronics.board_tracks_length", &[("length", &mm(length)), ("n", &tracks.len().to_string())]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_tracks_length"));
    });
}

/// How a via's hole can be protected, in the order the Inspector lists them.
pub(crate) const VIA_PROTECTIONS: [ViaProtection; 5] =
    [ViaProtection::Open, ViaProtection::Tented, ViaProtection::Plugged, ViaProtection::Filled, ViaProtection::FilledCapped];

/// The name of the net `find` picks from the board's connections.
fn net_of(app: &SolveApp, find: impl Fn(&Board, &BoardNets) -> Option<usize>) -> Option<String> {
    let nets = stored_nets(app);
    find(board(app), &nets).and_then(|n| nets.name(n)).map(str::to_string)
}

fn net_note(ui: &mut Ui, net: Option<String>) {
    let text = match net {
        Some(net) => crate::i18n::format("electronics.board_net", &[("net", &net)]),
        None => tr("electronics.board_no_net"),
    };
    ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_net"));
}

/// Millimetres for a note: up to three decimals, no trailing zeros.
fn mm(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Nothing picked: the stack, the outline's size, what the board holds and what is left to route.
fn summary(app: &mut SolveApp, ui: &mut Ui) {
    let b = board(app);
    let stack = crate::i18n::format("electronics.board_stack_summary", &[("n", &b.copper_layers.to_string()), ("thickness", &mm(b.thickness))]);
    let outline = match b.outline_bbox() {
        Some([[x0, y0], [x1, y1]]) => crate::i18n::format("electronics.board_outline_size", &[("w", &mm(x1 - x0)), ("h", &mm(y1 - y0))]),
        None => tr("electronics.board_no_outline"),
    };
    let counts = crate::i18n::format(
        "electronics.board_counts",
        &[
            ("footprints", &b.footprints.len().to_string()),
            ("tracks", &b.tracks.len().to_string()),
            ("vias", &b.vias.len().to_string()),
            ("holes", &b.holes.len().to_string()),
            ("drawings", &b.graphics.len().to_string()),
        ],
    );
    let unrouted = match stored_nets(app).airwires.len() {
        0 => tr("electronics.board_unrouted_none"),
        1 => tr("electronics.board_unrouted_one"),
        n => crate::i18n::format("electronics.board_unrouted", &[("n", &n.to_string())]),
    };
    for (text, key) in [
        (stack, "electronics.board_stack_summary"),
        (outline, "electronics.board_outline_size"),
        (counts, "electronics.board_counts"),
        (unrouted, "electronics.board_unrouted"),
    ] {
        ui_kit::text(ui, &text, ui_kit::Text::Note(key));
    }
    let b = board(app);
    if !(b.pours.is_empty() && b.keepouts.is_empty()) {
        let text = crate::i18n::format(
            "electronics.board_area_counts",
            &[("pours", &b.pours.len().to_string()), ("keepouts", &b.keepouts.len().to_string())],
        );
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_area_counts"));
    }
    if let Some(text) = rules::summary(app) {
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.drc_summary"));
    }
    if let Some(text) = crate::electronics_board_quality::summary(app) {
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.quality_summary"));
    }
    ui_kit::text(ui, crate::i18n::tr("electronics.board_help"), ui_kit::Text::Note("electronics.board_help"));
}

#[cfg(test)]
#[path = "electronics_board_tests.rs"]
mod tests;
