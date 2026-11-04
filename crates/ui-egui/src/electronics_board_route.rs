//! Routing and reworking the board's copper (see `electronics_board`): the Route tool's bend styles
//! and options, the track it is drawing with its live preview, the drags that reshape a picked
//! track, and the Unroute, Miter and Split tools.
//!
//! A route starts on a pad, a via, a track (one of its points, or anywhere on its line) or an
//! airwire (at its nearer end, with the far end as the goal); the first click prefers copper on
//! the active layer, then through-hole pads, then the other side. Each click adds a leg bent as
//! the bend style says, and a click on copper of the same net ends the route. Copper takes the
//! pointer from within [`PAD_MAGNET`] points of it, but no more than [`MAGNET_MM`], so ends land on
//! a pad's centre; near the route's own net only, so a bend beside another net's pad stays a bend.
//! Right after a route ends, a click on bare board starts none. V goes on on the next copper layer
//! through a via (a through via, a micro via to the next layer, or one between the options' two
//! layers, as the options say), Backspace takes back a click (back across a layer change too), Cmd
//! (Ctrl) leaves the grid. The width and via are the net class's of the net being routed until the
//! options set them. While routing, the track so far and the leg to the pointer are checked
//! against other nets' copper as the design rule check measures it (`drc::clashes`): what comes
//! closer than the clearance is drawn in the error colour, and the click that would place it, or a
//! via that would, is refused with what it is too close to, unless the options allow violations.
//! The legs on each layer and the vias between them are kept with the tool until the route ends,
//! then made in one `ecad.board.track`, so the whole route is one step to undo; their corners are
//! cut as the options say. A reshape drag is one `ecad.board.track.reshape`.

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::board::{MAX_LEGS, check_track_width, check_via};
use solidcraft_engine::doc::ecad::copper;
use solidcraft_engine::doc::ecad::drc::{self, Clash, DrcItem};
use solidcraft_engine::doc::ecad::fill::PourFill;
use solidcraft_engine::doc::ecad::footprint::Core;
use solidcraft_engine::doc::ecad::path::{self, is_arc, sweep, tessellate};
use solidcraft_engine::doc::ecad::place::{lattice, snap_point};
use solidcraft_engine::doc::ecad::rework::{MiterStyle, miter_keeps};
use solidcraft_engine::doc::ecad::stitch;
use solidcraft_engine::doc::ecad::{Board, BoardNets, BoardPad, CopperLayer, Track, ViaKind};

use crate::SolveApp;
use crate::electronics_board::{
    Layer, PICK_POINTS, Tool, active_layer, arc_tolerance, board, copper_shown, layer_colour, on_track, pick_order, segment_distance,
    set_active_layer, snap, snap_tidy, stored_nets, track_width,
};
use crate::electronics_editor::tr;
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;
use crate::ui_kit;

/// A pad takes the pointer this close to its copper, in points: the point is then its centre.
const PAD_MAGNET: f32 = 8.0;
/// ... but no further than this, in millimetres, so a board seen from afar doesn't reach across
/// to the next pad.
const MAGNET_MM: f64 = 0.15;
/// The track widths the width menu offers, in millimetres.
const WIDTHS: [f64; 10] = [0.15, 0.2, 0.25, 0.3, 0.4, 0.5, 0.8, 1.0, 1.5, 2.0];
/// The most a corner is cut, in millimetres (as `ecad.board.track.miter` takes).
const MAX_MITER: f64 = 100.0;

/// How the Route tool bends between two clicked points. Right-click cycles them while routing,
/// `/` turns the current one the other way round.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BoardBend {
    /// 90°: along x first, then along y.
    HorizontalFirst,
    /// Along x or y first, then 45° into the point.
    #[default]
    StraightThen45,
    /// Straight, at any angle.
    Straight,
    /// 45° first, then along x or y into the point.
    FortyFiveThenStraight,
    /// 90°: along y first, then along x.
    VerticalFirst,
    /// A quarter arc first, then along x or y.
    ArcThenStraight,
    /// Along x or y first, then a quarter arc into the point.
    StraightThenArc,
}

/// The bend styles in the order right-click cycles them, with their names.
pub(crate) const BOARD_BENDS: [(BoardBend, &str); 7] = [
    (BoardBend::HorizontalFirst, "electronics.board_bend_horizontal"),
    (BoardBend::StraightThen45, "electronics.board_bend_straight_45"),
    (BoardBend::Straight, "electronics.board_bend_straight"),
    (BoardBend::FortyFiveThenStraight, "electronics.board_bend_45_straight"),
    (BoardBend::VerticalFirst, "electronics.board_bend_vertical"),
    (BoardBend::ArcThenStraight, "electronics.board_bend_arc_straight"),
    (BoardBend::StraightThenArc, "electronics.board_bend_straight_arc"),
];

impl BoardBend {
    pub(crate) fn next(self) -> Self {
        let i = BOARD_BENDS.iter().position(|(b, _)| *b == self).unwrap_or(0);
        BOARD_BENDS.get((i + 1) % BOARD_BENDS.len()).map_or(self, |(b, _)| *b)
    }

    /// The same style the other way round: the other run first.
    pub(crate) fn flipped(self) -> Self {
        match self {
            BoardBend::HorizontalFirst => BoardBend::VerticalFirst,
            BoardBend::VerticalFirst => BoardBend::HorizontalFirst,
            BoardBend::StraightThen45 => BoardBend::FortyFiveThenStraight,
            BoardBend::FortyFiveThenStraight => BoardBend::StraightThen45,
            BoardBend::ArcThenStraight => BoardBend::StraightThenArc,
            BoardBend::StraightThenArc => BoardBend::ArcThenStraight,
            BoardBend::Straight => BoardBend::Straight,
        }
    }

    /// Whether it keeps to 90° corners.
    pub(crate) fn square(self) -> bool {
        matches!(self, BoardBend::HorizontalFirst | BoardBend::VerticalFirst)
    }

    /// Its name's message key.
    pub(crate) fn name(self) -> &'static str {
        BOARD_BENDS.iter().find(|(b, _)| *b == self).map_or("", |(_, key)| key)
    }

    /// The leg from `a` to `b`: the points after `a`, `b` last, each with the sweep (degrees) of
    /// the segment into it. The 45° part takes the shorter of the two runs; an arc style puts a
    /// quarter arc where that 45° part would be.
    pub(crate) fn leg(self, a: [f64; 2], b: [f64; 2]) -> Vec<([f64; 2], f64)> {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let sign = |v: f64| {
            if v > 0.0 {
                1.0
            } else if v < 0.0 {
                -1.0
            } else {
                0.0
            }
        };
        let (sx, sy) = (sign(dx), sign(dy));
        let wide = dx.abs() >= dy.abs();
        let r = dx.abs().min(dy.abs());
        // The corner before a 45° part at the end of the leg, and after one at its start.
        let late = if wide { [b[0] - sx * r, a[1]] } else { [a[0], b[1] - sy * r] };
        let early = if wide { [a[0] + sx * r, b[1]] } else { [b[0], a[1] + sy * r] };
        // A quarter arc turns from one axis to the other: this way round for a wide leg's arc
        // into its end, the other way out of its start.
        let turn = sx * sy * 90.0;
        let legs = match self {
            BoardBend::HorizontalFirst => vec![([b[0], a[1]], 0.0), (b, 0.0)],
            BoardBend::VerticalFirst => vec![([a[0], b[1]], 0.0), (b, 0.0)],
            BoardBend::StraightThen45 => vec![(late, 0.0), (b, 0.0)],
            BoardBend::FortyFiveThenStraight => vec![(early, 0.0), (b, 0.0)],
            BoardBend::Straight => vec![(b, 0.0)],
            BoardBend::ArcThenStraight => vec![(early, if wide { -turn } else { turn }), (b, 0.0)],
            BoardBend::StraightThenArc => vec![(late, 0.0), (b, if wide { turn } else { -turn })],
        };
        let mut out = Vec::with_capacity(legs.len());
        let mut prev = a;
        for (p, s) in legs {
            if lattice(p) != lattice(prev) {
                out.push((p, s));
                prev = p;
            }
        }
        out
    }

    /// Its icon: the leg it makes from the lower left of `rect` to the upper right.
    fn paint_icon(self, painter: &egui::Painter, rect: Rect, stroke: Stroke) {
        let (a, b) = ([0.0, 0.0], [1.0, 0.5]);
        let (mut points, mut curves) = (vec![a], Vec::new());
        for (p, s) in self.leg(a, b) {
            points.push(p);
            curves.push(s);
        }
        // Square units, so 45° looks like 45°.
        let side = rect.width().min(rect.height() * 2.0);
        let origin = pos2(rect.center().x - side / 2.0, rect.center().y + side / 4.0);
        let to = |q: [f64; 2]| pos2(origin.x + q[0] as f32 * side, origin.y - q[1] as f32 * side);
        let line: Vec<Pos2> = tessellate(&points, &curves, false, 0.01).into_iter().map(to).collect();
        painter.add(egui::Shape::line(line, stroke));
        for q in [a, b] {
            painter.circle_filled(to(q), stroke.width * 1.3, stroke.color);
        }
    }
}

/// The layers a via spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViaSpan {
    /// From the top to the bottom.
    Through,
    /// Between two layers (blind from an outer layer, buried between inner ones): V on one of them
    /// goes on on the other, and the Via tool places one between them.
    Between(CopperLayer, CopperLayer),
}

/// The Route tool's options, shown in the Inspector while it is on (the Via and Miter tools use
/// theirs too).
#[derive(Clone, Debug, PartialEq)]
pub struct RouteOptions {
    pub bend: BoardBend,
    /// The track's width; the net class's while `None`.
    pub width: Option<f64>,
    /// How far each corner of a made track is cut (millimetres); 0 leaves it sharp.
    pub miter: f64,
    pub miter_style: MiterStyle,
    /// The diameter and drill of the via a layer change drops; the net class's while `None`.
    pub via: Option<[f64; 2]>,
    /// The layers a via spans.
    pub span: ViaSpan,
    /// V goes to the next layer through a micro via (the rules' micro via size).
    pub micro: bool,
    /// The micro via's diameter and drill; the rules' while `None`.
    pub micro_via: Option<[f64; 2]>,
    /// Copper may be placed closer to other nets' copper than the clearance.
    pub allow: bool,
}

impl Default for RouteOptions {
    fn default() -> Self {
        RouteOptions {
            bend: BoardBend::default(),
            width: None,
            miter: 0.0,
            miter_style: MiterStyle::Round,
            via: None,
            span: ViaSpan::Through,
            micro: false,
            micro_via: None,
            allow: false,
        }
    }
}

/// A via a route drops where it changes layer: where, the layers it spans and its size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegVia {
    pub at: [f64; 2],
    pub from: CopperLayer,
    pub to: CopperLayer,
    pub diameter: f64,
    pub drill: f64,
    /// Filled and capped: a via a route ends on in a pad.
    pub capped: bool,
}

impl LegVia {
    /// As `ecad.board.track`'s `via` and `ecad.board.via` take it (without `at`).
    fn params(&self) -> Value {
        let mut p = json!({"from": self.from.name(), "to": self.to.name(), "diameter": self.diameter, "drill": self.drill});
        if self.capped {
            p["protection"] = json!("filled_capped");
        }
        p
    }

    /// What kind of via it is on the board.
    fn kind(&self, app: &SolveApp) -> ViaKind {
        let b = board(app);
        let v = solidcraft_engine::doc::ecad::Via {
            id: 0,
            at: self.at,
            diameter: self.diameter,
            drill: self.drill,
            from: self.from,
            to: self.to,
            net: None,
            protection: Default::default(),
            role: Default::default(),
            pattern: None,
        };
        v.kind(b.copper_layers, &b.rules)
    }

    /// Whether it joins layers `a` and `b` at `at`.
    fn joins(&self, at: [f64; 2], a: CopperLayer, b: CopperLayer, n: u8) -> bool {
        let (lo, hi) = (self.from.index(n).min(self.to.index(n)), self.from.index(n).max(self.to.index(n)));
        dist(self.at, at) <= self.diameter / 2.0 && (lo..=hi).contains(&a.index(n)) && (lo..=hi).contains(&b.index(n))
    }
}

/// A leg of the route on a layer it has left: its track and the via it ends on, kept with the
/// route until the route is made.
#[derive(Clone, Debug, PartialEq)]
pub struct Leg {
    pub layer: CopperLayer,
    pub width: f64,
    pub points: Vec<[f64; 2]>,
    pub curves: Vec<f64>,
    clicks: Vec<usize>,
    /// None where copper there joins the layers already.
    pub via: Option<LegVia>,
}

/// The track the Route tool is drawing: its points so far on the active layer from where it
/// started there, the sweep of each segment, and where each click left off (so Backspace takes
/// back one click); the legs on the layers it left before.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Route {
    pub points: Vec<[f64; 2]>,
    /// One per segment.
    pub curves: Vec<f64>,
    clicks: Vec<usize>,
    /// The net of the copper it started on.
    pub net: Option<usize>,
    /// The far end of the airwire it started on.
    pub goal: Option<[f64; 2]>,
    /// The route's legs on the layers it left, first first.
    pub legs: Vec<Leg>,
    /// A route has just been made: a click on bare board starts no new one.
    pub ended: bool,
    /// The last click was on a pad of another layer (by index into the pads), which V ends the
    /// route in through a via in the pad.
    pub pad: Option<usize>,
}

impl Route {
    fn start(at: [f64; 2], net: Option<usize>, goal: Option<[f64; 2]>) -> Self {
        Route { points: vec![at], net, goal, ..Route::default() }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Whether making it now leaves any copper: a segment, or a via.
    pub(crate) fn has_copper(&self) -> bool {
        self.points.len() >= 2 || self.legs.iter().any(|l| l.points.len() >= 2 || l.via.is_some())
    }

    fn last(&self) -> Option<[f64; 2]> {
        self.points.last().copied()
    }

    /// Where it started, and on which layer (`layer` when it has no leg on another).
    fn origin(&self, layer: CopperLayer) -> Option<([f64; 2], CopperLayer)> {
        match self.legs.first() {
            Some(leg) => leg.points.first().map(|p| (*p, leg.layer)),
            None => self.points.first().map(|p| (*p, layer)),
        }
    }

    /// Its points and sweeps with the leg to `to` added as `bend` makes it.
    fn with_leg(&self, bend: BoardBend, to: [f64; 2]) -> (Vec<[f64; 2]>, Vec<f64>) {
        let (mut points, mut curves) = (self.points.clone(), self.curves.clone());
        if let Some(last) = self.last() {
            for (p, s) in bend.leg(last, to) {
                points.push(p);
                curves.push(s);
            }
        }
        (points, curves)
    }

    fn add(&mut self, bend: BoardBend, to: [f64; 2]) {
        let (points, curves) = self.with_leg(bend, to);
        if points.len() > self.points.len() {
            self.points = points;
            self.curves = curves;
            self.clicks.push(self.points.len());
        }
    }

    /// Go on on another layer from the last point: the track so far, `width` wide on `layer`, and
    /// `via` become a leg.
    fn change(&mut self, layer: CopperLayer, width: f64, via: Option<LegVia>) {
        let Some(last) = self.last() else { return };
        let (points, curves, clicks) = (std::mem::take(&mut self.points), std::mem::take(&mut self.curves), std::mem::take(&mut self.clicks));
        self.legs.push(Leg { layer, width, points, curves, clicks, via });
        self.points = vec![last];
    }

    /// Take back the last click, back across a layer change (whose layer it gives), the start
    /// last; `false` when there was nothing to take back.
    fn step_back(&mut self) -> (bool, Option<CopperLayer>) {
        if self.clicks.pop().is_none() {
            if let Some(leg) = self.legs.pop() {
                self.points = leg.points;
                self.curves = leg.curves;
                self.clicks = leg.clicks;
                return (true, Some(leg.layer));
            }
            let started = !self.points.is_empty();
            *self = Route::default();
            return (started, None);
        }
        let keep = self.clicks.last().copied().unwrap_or(1);
        self.points.truncate(keep);
        self.curves.truncate(keep.saturating_sub(1));
        (true, None)
    }
}

fn dist([ax, ay]: [f64; 2], [bx, by]: [f64; 2]) -> f64 {
    (ax - bx).hypot(ay - by)
}

fn add([ax, ay]: [f64; 2], [bx, by]: [f64; 2]) -> [f64; 2] {
    [ax + bx, ay + by]
}

fn scaled([x, y]: [f64; 2], k: f64) -> [f64; 2] {
    [x * k, y * k]
}

fn dot([ax, ay]: [f64; 2], [bx, by]: [f64; 2]) -> f64 {
    ax * bx + ay * by
}

fn page_point(map: PageMap, p: Pos2) -> [f64; 2] {
    let (x, y) = map.to_page(p);
    [x, y]
}

fn finite(p: &Pos2) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

/// The net of the track the Route tool is drawing, by name.
pub(crate) fn routed_net(app: &SolveApp) -> Option<String> {
    let Some(Tool::Track(route)) = &app.electronics.board.tool else { return None };
    route.net.and_then(|n| stored_nets(app).name(n).map(str::to_string))
}

/// The net class of the track the Route tool is drawing, by name; `None` for the default class.
pub(crate) fn routed_class(app: &SolveApp) -> Option<String> {
    board(app).rules.sizes(routed_net(app).as_deref()).class
}

/// The via the Route and Via tools place: the options' size, or the net class's of the net being
/// routed (the rules' without one).
pub(crate) fn via_size(app: &SolveApp) -> [f64; 2] {
    let sizes = board(app).rules.sizes(routed_net(app).as_deref());
    app.electronics.board.route.via.unwrap_or([sizes.via_diameter, sizes.via_drill])
}

/// The micro via the Via tool and V place: the size typed for it, else the rules' micro via.
pub(crate) fn micro_size(app: &SolveApp) -> [f64; 2] {
    let r = &board(app).rules;
    app.electronics.board.route.micro_via.unwrap_or([r.microvia_diameter, r.microvia_drill])
}

/// Whether `a` and `b` are neighbours in the stack (a micro via's span; the top and bottom of a
/// 2-layer board are not).
fn neighbours(app: &SolveApp, a: CopperLayer, b: CopperLayer) -> bool {
    let n = board(app).copper_layers;
    a.index(n).abs_diff(b.index(n)) == 1 && (a.is_inner() || b.is_inner())
}

/// The via a change from layer `a` to layer `b` at `at` drops: a micro via between neighbours
/// while Micro is on, else one between the two layers when the options' span is, else a through
/// via; with its size.
fn via_between(app: &SolveApp, a: CopperLayer, b: CopperLayer, at: [f64; 2]) -> LegVia {
    let o = &app.electronics.board.route;
    let ([from, to], [diameter, drill]) = if o.micro && neighbours(app, a, b) {
        ([a, b], micro_size(app))
    } else if matches!(o.span, ViaSpan::Between(..)) {
        ([a, b], via_size(app))
    } else {
        ([CopperLayer::Top, CopperLayer::Bottom], via_size(app))
    };
    LegVia { at, from, to, diameter, drill, capped: false }
}

/// The via the Via tool places at `at`: through, or between the options' two layers (a micro via
/// there while Micro is on and they are neighbours).
fn placed_via(app: &SolveApp, at: [f64; 2]) -> LegVia {
    match app.electronics.board.route.span {
        ViaSpan::Through => {
            let [diameter, drill] = via_size(app);
            LegVia { at, from: CopperLayer::Top, to: CopperLayer::Bottom, diameter, drill, capped: false }
        }
        ViaSpan::Between(a, b) => via_between(app, a, b, at),
    }
}

/// A click with the Via tool: a via there (on the centre of the copper in reach), spanning as the
/// options say. On no copper but a pour's fill on a layer of its span, it takes the pour's net (a
/// stitching via), so the pour joins it rather than clearing round it. One that would come closer
/// to other nets' copper than the clearance is refused, unless the options allow it.
pub(crate) fn via_click(app: &mut SolveApp, map: PageMap, p: Pos2, free: bool) {
    let (v, net, params) = via_at(app, map, p, free);
    if !app.electronics.board.route.allow
        && let Some(text) = via_blocked(app, net, &v)
    {
        app.set_status(text, true);
        return;
    }
    if app.run("ecad.board.via", params).is_ok() {
        app.set_status(crate::i18n::format("electronics.board_via_placed", &[("kind", &tr(via_kind_name(v.kind(app))))]), false);
    }
}

/// Why the via `v` of the net `net` can't go where it is: other nets' copper closer than the
/// clearance on a layer of its span (as the route checks it), its drill too close to another
/// drill (a via of its own net on top of it included), the board's edge or an unplated hole, or a
/// keepout that forbids vias. `None` when it fits.
fn via_blocked(app: &SolveApp, net: Option<usize>, v: &LegVia) -> Option<String> {
    if let Some(c) = via_clashes(app, net, v).0.first() {
        return Some(clash_text(app, c, "electronics.board_clash_via"));
    }
    let b = board(app);
    let nets = stored_nets(app);
    let spec = stitch::ViaSpec { diameter: v.diameter, drill: v.drill, from: v.from, to: v.to };
    let mut site = stitch::Site::around(b, &nets, net, spec, 0.0, v.at);
    let mm = crate::electronics_editor::mm;
    match site.check(v.at, 0, stitch::Checks::PLACED).err()? {
        stitch::Blocked::Copper { item, net, layer, gap, required } => {
            let c = Clash { piece: 0, layer, item, net, gap, required, at: v.at };
            Some(clash_text(app, &c, "electronics.board_clash_via"))
        }
        stitch::Blocked::Hole { item, gap, required } => {
            let what = item.map_or_else(|| tr("electronics.board_via_new"), |i| crate::electronics_board_rules::item_name(&i));
            Some(crate::i18n::format("electronics.board_via_hole", &[("what", &what), ("gap", &mm(gap)), ("required", &mm(required))]))
        }
        stitch::Blocked::Edge { gap, required } => {
            Some(crate::i18n::format("electronics.board_via_edge", &[("gap", &mm(gap.max(0.0))), ("required", &mm(required))]))
        }
        stitch::Blocked::Keepout(id) => Some(crate::i18n::format("electronics.board_via_keepout", &[("id", &id.to_string())])),
        stitch::Blocked::Pad(_) | stitch::Blocked::Stub { .. } | stitch::Blocked::Under(_) => None,
    }
}

/// The copper right under `at` (not near it): a pad (the active layer's first), a via, a track
/// (the way they are drawn), with its net.
fn copper_under(app: &SolveApp, at: [f64; 2]) -> Option<(On, Option<usize>)> {
    let nets = stored_nets(app);
    let b = board(app);
    let active = active_layer(app);
    let pad =
        nets.pads.iter().enumerate().filter(|(_, pad)| (pad.top || pad.bottom) && pad.contains(at)).min_by_key(|(_, pad)| side_rank(pad, active));
    if let Some((i, pad)) = pad {
        return Some((On::Pad(i), pad.net));
    }
    if let Some((v, n)) = b.vias.iter().zip(&nets.via_nets).find(|(v, _)| dist(v.at, at) <= v.diameter / 2.0) {
        return Some((On::Via(v.id), *n));
    }
    let order = pick_order(app, b);
    let on = |t: &Track| path::segments(&t.points, &t.curves, false).any(|(a, c, s)| path::segment_distance(at, a, c, s) <= t.width / 2.0);
    order.iter().find_map(|l| b.tracks.iter().zip(&nets.track_nets).find(|(t, _)| t.layer == *l && on(t)).map(|(t, n)| (On::Track(t.id), *n)))
}

/// Whether the copper `on` holds `at`.
fn holds(app: &SolveApp, on: On, at: [f64; 2]) -> bool {
    let b = board(app);
    match on {
        On::Pad(i) => stored_nets(app).pads.get(i).is_some_and(|pad| pad.contains(at)),
        On::Via(id) => b.via(id).is_some_and(|v| dist(v.at, at) <= v.diameter / 2.0),
        On::Track(id) => b
            .track(id)
            .is_some_and(|t| path::segments(&t.points, &t.curves, false).any(|(a, c, s)| path::segment_distance(at, a, c, s) <= t.width / 2.0)),
        On::Airwire(_) | On::Nothing => false,
    }
}

/// Where a click of the Via tool at `p` lands: with Cmd (`free`), right under the pointer, snapped
/// to nothing; on copper, where the pointer is (on the grid while that stays on the same copper),
/// so a via goes anywhere in a big pad; near copper, as a route's click does (a pad's centre, a
/// via's, a track's corner or the nearest point of its line); else the grid.
fn via_target(app: &SolveApp, map: PageMap, p: Pos2, free: bool) -> Target {
    let page = page_point(map, p);
    let under = copper_under(app, page);
    if free {
        let (on, net) = under.unwrap_or((On::Nothing, None));
        return Target { at: snap_tidy(page), net, on };
    }
    if let Some((on, net)) = under {
        let grid = snap(app, map, p, false);
        let at = if holds(app, on, grid) { grid } else { snap_tidy(page) };
        return Target { at, net, on };
    }
    target(app, map, p, false, None, false, None)
}

/// The via a click of the Via tool at `p` places, the net it is on, and `ecad.board.via`'s
/// parameters for it (see [`via_click`]). On no copper but a pour's fill on a layer of its span
/// it takes the pour's net; stacked on copper that no layer of its span reaches (a via or pad on
/// other layers), that copper's net, so a stack built from its loose end joins up.
fn via_at(app: &SolveApp, map: PageMap, p: Pos2, free: bool) -> (LegVia, Option<usize>, Value) {
    let t = via_target(app, map, p, free);
    let v = placed_via(app, t.at);
    let mut params = v.params();
    params["at"] = json!(t.at);
    let mut net = t.net;
    let n = board(app).copper_layers;
    let (lo, hi) = (v.from.index(n).min(v.to.index(n)), v.from.index(n).max(v.to.index(n)));
    let reaches = |l: CopperLayer| (lo..=hi).contains(&l.index(n));
    let joined = match t.on {
        On::Pad(i) => stored_nets(app).pads.get(i).is_some_and(|pad| board(app).stack().into_iter().any(|l| pad.on(l) && reaches(l))),
        On::Via(id) => board(app).via(id).is_some_and(|w| board(app).stack().into_iter().any(|l| w.on(l, n) && reaches(l))),
        _ => true,
    };
    let name = |i: usize| stored_nets(app).name(i).map(str::to_string);
    if t.on == On::Nothing {
        if let Some((k, name)) = pour_net_at(app, &v) {
            params["net"] = json!(name);
            net = Some(k);
        }
    } else if !joined && let Some(name) = t.net.and_then(name) {
        params["net"] = json!(name);
    }
    (v, net, params)
}

/// The Via tool under the pointer `hover`: the via a click places, outlined, in the error colour
/// with a ring where it would come closer to other nets' copper than the clearance.
pub(crate) fn paint_via(app: &SolveApp, painter: &egui::Painter, map: PageMap, hover: Pos2, free: bool, t: &Tokens) {
    let (v, net, _) = via_at(app, map, hover, free);
    paint_ghost_via(app, painter, map, &v, net, t.sel_edge.gamma_multiply(0.7), t);
}

/// The via `v` of the net `net` a click or V would place, outlined in `colour`, or in the error
/// colour with a ring where it would come closer to other nets' copper than the clearance.
fn paint_ghost_via(app: &SolveApp, painter: &egui::Painter, map: PageMap, v: &LegVia, net: Option<usize>, colour: Color32, t: &Tokens) {
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    let r = ((v.diameter / 2.0 * map.scale()) as f32).max(3.0);
    let allow = app.electronics.board.route.allow;
    let clashes = if allow { Vec::new() } else { via_clashes(app, net, v).0 };
    let blocked = !allow && (!clashes.is_empty() || via_blocked(app, net, v).is_some());
    let colour = if blocked { t.pcb_drc_error } else { colour };
    let c = screen(v.at);
    if !finite(&c) {
        return;
    }
    painter.circle_stroke(c, r, Stroke::new(1.5, colour));
    if blocked {
        painter.circle_stroke(c, ((v.drill / 2.0 * map.scale()) as f32).max(1.5), Stroke::new(1.5, colour));
    }
    for k in &clashes {
        let at = screen(k.at);
        if finite(&at) {
            painter.circle_stroke(at, 6.0, Stroke::new(1.5, t.pcb_drc_error));
        }
    }
}

/// The net (its index and name) of a pour whose fill holds the via `v`'s centre on a layer it
/// spans.
fn pour_net_at(app: &SolveApp, v: &LegVia) -> Option<(usize, String)> {
    let n = board(app).copper_layers;
    let (a, c) = (v.from.index(n), v.to.index(n));
    let span = a.min(c)..=a.max(c);
    let nets = stored_nets(app);
    let holds = |f: &&PourFill| f.net.is_some() && span.contains(&f.layer.index(n)) && f.region.locate(&[v.at]).first().is_some_and(Option::is_some);
    let net = nets.fills.iter().find(holds)?.net?;
    nets.name(net).map(|name| (net, name.to_string()))
}

/// What the via `v` of the net `net` would come closer to than the clearance, on each layer it
/// spans, and whether the check ran out of time first (see `drc::clashes`).
fn via_clashes(app: &SolveApp, net: Option<usize>, v: &LegVia) -> (Vec<Clash>, bool) {
    let b = board(app);
    let n = b.copper_layers;
    let (lo, hi) = (v.from.index(n).min(v.to.index(n)), v.from.index(n).max(v.to.index(n)));
    let pieces: Vec<(CopperLayer, Core)> =
        b.stack().into_iter().filter(|l| (lo..=hi).contains(&l.index(n))).map(|l| (l, copper::disc(v.at, v.diameter / 2.0))).collect();
    drc::clashes(b, &stored_nets(app), net, &pieces)
}

/// What the track `points` and `curves`, `width` wide on `layer`, of the net `net` would come
/// closer to than the clearance (see `drc::clashes`), whether the check ran out of time first,
/// and the straight pieces the check cut it into: each clash's `piece` is one of them.
#[allow(clippy::type_complexity)]
fn track_clashes(
    app: &SolveApp,
    net: Option<usize>,
    layer: CopperLayer,
    width: f64,
    points: &[[f64; 2]],
    curves: &[f64],
) -> (Vec<Clash>, bool, Vec<([f64; 2], [f64; 2])>) {
    let pieces = copper::pieces(points, curves, false);
    let cores: Vec<(CopperLayer, Core)> = pieces.iter().map(|(a, b)| (layer, copper::stroke(*a, *b, width / 2.0))).collect();
    let (clashes, truncated) = drc::clashes(board(app), &stored_nets(app), net, &cores);
    (clashes, truncated, pieces)
}

/// The width the Route tool makes a track of the net `net` (an index into the connections'
/// names): the options' width, else the net's class's (the rules' without one).
fn width_of(app: &SolveApp, net: Option<usize>) -> f64 {
    let nets = stored_nets(app);
    let name = net.and_then(|n| nets.name(n));
    app.electronics.board.route.width.unwrap_or_else(|| board(app).rules.sizes(name).track_width)
}

/// Why the clash `c` is refused, in the message `key`'s words: what the copper is too close to,
/// its net and layer, the gap and the clearance.
fn clash_text(app: &SolveApp, c: &Clash, key: &str) -> String {
    let nets = stored_nets(app);
    let net = c.net.and_then(|n| nets.name(n)).map_or_else(|| tr("electronics.board_clash_no_net"), str::to_string);
    let what = crate::electronics_board_rules::item_name(&c.item);
    let mm = crate::electronics_editor::mm;
    crate::i18n::format(key, &[("what", &what), ("net", &net), ("layer", &layer_name(c.layer)), ("gap", &mm(c.gap)), ("required", &mm(c.required))])
}

/// The kind of via's name's message key.
pub(crate) fn via_kind_name(kind: ViaKind) -> &'static str {
    match kind {
        ViaKind::Through => "electronics.via_through",
        ViaKind::Blind => "electronics.via_blind",
        ViaKind::Buried => "electronics.via_buried",
        ViaKind::Micro => "electronics.via_micro",
    }
}

/// `points` and `curves` with every corner cut as `o` says (as they are with no miter); corners
/// where other copper joins them stay sharp, as the command leaves them.
fn mitered(app: &SolveApp, points: Vec<[f64; 2]>, curves: Vec<f64>, o: &RouteOptions, layer: CopperLayer, width: f64) -> (Vec<[f64; 2]>, Vec<f64>) {
    let mut t = Track { id: 0, layer, width, points, curves };
    if o.miter > 0.0 {
        let nets = stored_nets(app);
        let original = t.clone();
        t.miter(None, o.miter, o.miter_style, miter_keeps(board(app), &nets.pads, &original));
    }
    (t.points, t.curves)
}

/// What a routing click lands on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum On {
    /// A pad, by its index in the board's pads.
    Pad(usize),
    Via(u64),
    /// A track: one of its points, or a point on its line.
    Track(u64),
    /// The nearer end of an airwire; the far end is where the route should go.
    Airwire([f64; 2]),
    Nothing,
}

/// Where a click for the Route and Via tools lands, with the net of what it is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Target {
    pub at: [f64; 2],
    pub net: Option<usize>,
    pub on: On,
}

/// How far from copper, in millimetres, the pointer at `map`'s scale is taken to it: [`PAD_MAGNET`]
/// points, at most [`MAGNET_MM`].
fn magnet(map: PageMap) -> f64 {
    (f64::from(PAD_MAGNET) / map.scale().max(1e-9)).min(MAGNET_MM)
}

/// Where a click at `p` lands: a pad's centre when the pointer is on its copper or near it (see
/// [`magnet`]), a via's centre, a track's point, the nearest point of a track's line, an airwire's
/// nearer end (with `airwires`), else the grid (anywhere with `free`). Only copper on `layer`
/// counts, when given; with `net`, copper near the pointer counts only when it is of that net
/// (copper under the pointer counts whatever its net). Without a layer, copper on the active
/// layer (through-hole pads among it) is tried first, so a pad on the other side under a track
/// on the active layer doesn't take the click.
pub(crate) fn target(app: &SolveApp, map: PageMap, p: Pos2, free: bool, layer: Option<CopperLayer>, airwires: bool, net: Option<usize>) -> Target {
    if layer.is_none() {
        let on = target_in(app, map, p, free, Some(active_layer(app)), false, net);
        if on.on != On::Nothing {
            return on;
        }
    }
    target_in(app, map, p, free, layer, airwires, net)
}

/// Where a click at `p` lands, as [`target`] says, with only copper on `layer` when given.
fn target_in(app: &SolveApp, map: PageMap, p: Pos2, free: bool, layer: Option<CopperLayer>, airwires: bool, net: Option<usize>) -> Target {
    let nets = stored_nets(app);
    let b = board(app);
    let page = page_point(map, p);
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    let reach = magnet(map);
    let near = |n: &Option<usize>| net.is_none() || *n == net;
    if let Some(i) = pad_under(&nets, map, p, layer, active_layer(app), net)
        && let Some(pad) = nets.pads.get(i)
    {
        return Target { at: snap_tidy(pad.at()), net: pad.net, on: On::Pad(i) };
    }
    for (v, n) in b.vias.iter().zip(&nets.via_nets) {
        let d = dist(v.at, page);
        if layer.is_none_or(|l| v.on(l, b.copper_layers)) && d <= v.diameter / 2.0 + if near(n) { reach } else { 0.0 } {
            return Target { at: v.at, net: *n, on: On::Via(v.id) };
        }
    }
    // A track's points first, then anywhere on its line; the layers the way they are drawn.
    let order = pick_order(app, b);
    let tracks: Vec<(&Track, &Option<usize>)> = order
        .iter()
        .filter(|l| layer.is_none_or(|on| on == **l))
        .flat_map(|l| b.tracks.iter().zip(&nets.track_nets).filter(move |(t, _)| t.layer == *l))
        .collect();
    // On a track's copper, or near it when it is of `net`.
    let takes = |t: &Track, n: &Option<usize>, q: [f64; 2]| dist(q, page) <= t.width / 2.0 + if near(n) { reach } else { 0.0 };
    for (t, n) in &tracks {
        if let Some(q) = t.points.iter().find(|q| takes(t, n, **q)) {
            return Target { at: *q, net: **n, on: On::Track(t.id) };
        }
    }
    for (t, n) in &tracks {
        if let Some((q, _)) = nearest_on(t, page)
            && takes(t, n, q)
        {
            return Target { at: snap_tidy(q), net: **n, on: On::Track(t.id) };
        }
    }
    if airwires && !app.electronics.board.hidden.contains(&Layer::Airwires) {
        for a in &nets.airwires {
            if segment_distance(p, screen(a.from), screen(a.to)) <= PICK_POINTS {
                let (near, far) = if dist(a.from, page) <= dist(a.to, page) { (a.from, a.to) } else { (a.to, a.from) };
                return Target { at: near, net: Some(a.net), on: On::Airwire(far) };
            }
        }
    }
    Target { at: snap(app, map, p, free), net: None, on: On::Nothing }
}

/// Which pad wins under the pointer: copper on the active layer `prefer`, then a through-hole
/// pad, then the other side.
fn side_rank(pad: &BoardPad, prefer: CopperLayer) -> u8 {
    if pad.top && pad.bottom {
        1
    } else if pad.on(prefer) {
        0
    } else {
        2
    }
}

/// The pad (its index) that takes the pointer at `p`: one under it, else the nearest whose copper
/// is within [`magnet`] reach and, with `net`, of that net; among several, as [`side_rank`] says.
/// Only pads with copper on `layer`, when given.
fn pad_under(nets: &BoardNets, map: PageMap, p: Pos2, layer: Option<CopperLayer>, prefer: CopperLayer, net: Option<usize>) -> Option<usize> {
    let page = page_point(map, p);
    let reach = magnet(map);
    // Under the pointer first, then by side, then the nearest.
    let mut best: Option<((u8, u8, f64), usize)> = None;
    for (i, pad) in nets.pads.iter().enumerate() {
        let copper = match layer {
            Some(l) => pad.on(l),
            None => pad.top || pad.bottom,
        };
        let [lo, hi] = pad.bbox();
        if !copper || page[0] < lo[0] - reach || page[0] > hi[0] + reach || page[1] < lo[1] - reach || page[1] > hi[1] + reach {
            continue;
        }
        let on = pad.contains(page);
        let d = if on { 0.0 } else { ring_distance(&pad.outline(6), page) };
        if !on && (d > reach || net.is_some_and(|n| pad.net != Some(n))) {
            continue;
        }
        let key = (u8::from(!on), side_rank(pad, prefer), d);
        if best.is_none_or(|(k, _)| (key.0, key.1).cmp(&(k.0, k.1)).then(key.2.total_cmp(&k.2)).is_lt()) {
            best = Some((key, i));
        }
    }
    best.map(|(_, i)| i)
}

/// How far `p` is from the closed ring `ring`'s edges.
fn ring_distance(ring: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let n = ring.len();
    (0..n).filter_map(|i| Some(path::segment_distance(p, *ring.get(i)?, *ring.get((i + 1) % n)?, 0.0))).fold(f64::INFINITY, f64::min)
}

/// The point of track `t` nearest `p`, and the segment it is on.
fn nearest_on(t: &Track, p: [f64; 2]) -> Option<([f64; 2], usize)> {
    path::segments(&t.points, &t.curves, false)
        .enumerate()
        .map(|(i, (a, b, s))| (path::project(p, a, b, s).0, i))
        .min_by(|x, y| dist(x.0, p).total_cmp(&dist(y.0, p)))
}

/// The island (see `BoardNets`) of the copper at `at` on `layer`.
fn island_at(b: &Board, nets: &BoardNets, at: [f64; 2], layer: CopperLayer) -> Option<usize> {
    if let Some(i) = nets.pads.iter().position(|pad| pad.on(layer) && pad.contains(at)) {
        return nets.pad_islands.get(i).copied();
    }
    if let Some(v) = b.vias.iter().position(|v| dist(v.at, at) <= v.diameter / 2.0) {
        return nets.via_islands.get(v).copied();
    }
    let on = |t: &Track| path::segments(&t.points, &t.curves, false).any(|(a, c, s)| path::segment_distance(at, a, c, s) <= t.width / 2.0);
    let t = b.tracks.iter().position(|t| t.layer == layer && on(t))?;
    nets.track_islands.get(t).copied()
}

/// The pads `route` still has to reach: those of its net not joined to where it started.
fn goals(app: &SolveApp, route: &Route, layer: CopperLayer) -> Vec<usize> {
    let Some(net) = route.net else { return Vec::new() };
    let nets = stored_nets(app);
    let start = route.origin(layer).and_then(|(s, l)| island_at(board(app), &nets, s, l));
    nets.pads
        .iter()
        .enumerate()
        .filter(|(i, pad)| pad.net == Some(net) && (start.is_none() || nets.pad_islands.get(*i).copied() != start))
        .map(|(i, _)| i)
        .collect()
}

/// The layer a route starting at `to` goes on when the active `layer` has none of that copper:
/// a surface-mount pad's side, or a track's own layer.
fn start_layer(app: &SolveApp, to: &Target, layer: CopperLayer) -> Option<CopperLayer> {
    let nets = stored_nets(app);
    let pad_side = |i: usize| {
        let pad = nets.pads.get(i)?;
        (!pad.on(layer) && (pad.top || pad.bottom)).then_some(if pad.top { CopperLayer::Top } else { CopperLayer::Bottom })
    };
    match to.on {
        On::Pad(i) => pad_side(i),
        On::Track(id) => board(app).track(id).map(|t| t.layer).filter(|l| *l != layer),
        On::Airwire(_) => nets
            .pads
            .iter()
            .enumerate()
            .filter(|(_, pad)| pad.contains(to.at))
            .min_by_key(|(_, pad)| side_rank(pad, layer))
            .and_then(|(i, _)| pad_side(i)),
        On::Via(_) | On::Nothing => None,
    }
}

/// A click with the Route tool: the first starts the route (on the layer of a pad or track there
/// that the active layer lacks; right after a route ended, only on copper), saying so; the next
/// ones add legs, each checked against other nets' copper; copper of the same net ends the route,
/// and so does `double` anywhere.
pub(crate) fn click(app: &mut SolveApp, map: PageMap, p: Pos2, free: bool, double: bool) {
    let Some(Tool::Track(mut route)) = app.electronics.board.tool.clone() else { return };
    let layer = active_layer(app);
    if route.is_empty() {
        // The second click of a double-click that ended a track starts none.
        if double {
            return;
        }
        let to = target(app, map, p, free, None, true, None);
        if route.ended && to.on == On::Nothing {
            app.set_status(tr("electronics.board_route_start_copper"), false);
            return;
        }
        let on = start_layer(app, &to, layer).unwrap_or(layer);
        set_active_layer(app, on);
        let goal = match to.on {
            On::Airwire(far) => Some(far),
            _ => None,
        };
        app.electronics.board.tool = Some(Tool::Track(Route::start(to.at, to.net, goal)));
        let net = to.net.and_then(|n| stored_nets(app).name(n).map(str::to_string));
        let text = match net {
            Some(net) => crate::i18n::format("electronics.board_route_new", &[("net", &net), ("layer", &layer_name(on))]),
            None => crate::i18n::format("electronics.board_route_new_free", &[("layer", &layer_name(on))]),
        };
        app.set_status(text, false);
        return;
    }
    let to = target(app, map, p, free, Some(layer), false, route.net);
    // A pad under the pointer only on another layer that the track would end on (one of its net,
    // or any for a track of no net): say so rather than end the track beside it, and V then ends
    // the route in it. Near such a pad, or on another net's, the click is a bend.
    let page = page_point(map, p);
    route.pad = None;
    if to.on == On::Nothing
        && let Some(i) = stored_nets(app)
            .pads
            .iter()
            .position(|pad| !pad.on(layer) && pad.contains(page) && (pad.top || pad.bottom) && route.net.is_none_or(|n| pad.net == Some(n)))
    {
        route.pad = Some(i);
        app.electronics.board.tool = Some(Tool::Track(route));
        app.set_status(tr("electronics.board_other_layer"), true);
        return;
    }
    app.electronics.board.tool = Some(Tool::Track(route.clone()));
    let copper = matches!(to.on, On::Pad(_) | On::Via(_) | On::Track(_));
    if let Some(text) = copper.then(|| refused(app, &to, route.net)).flatten() {
        app.set_status(text, true);
        return;
    }
    let same = route.last().is_some_and(|q| lattice(q) == lattice(to.at));
    if same && !double {
        return;
    }
    // The track with this leg as it will be made (at the width of the net it will have), against
    // other nets' copper.
    if !app.electronics.board.route.allow {
        let width = width_of(app, route.net.or(to.net));
        let (points, curves) = drawn(app, &route, to.at, layer, width);
        let (clashes, truncated, _) = track_clashes(app, route.net.or(to.net), layer, width, &points, &curves);
        // Copper of no net that the route ends on joins it and takes its net: not a clash.
        let joined = joined_copper(&to);
        let text = match clashes.iter().find(|c| joined.as_ref() != Some(&c.item)) {
            Some(c) => Some(clash_text(app, c, "electronics.board_clash")),
            None => truncated.then(|| tr("electronics.board_clash_unchecked")),
        };
        if let Some(text) = text {
            app.set_status(text, true);
            return;
        }
    }
    route.add(app.electronics.board.route.bend, to.at);
    route.net = route.net.or(to.net);
    let reached = (copper && !same) || route.goal.is_some_and(|g| lattice(g) == lattice(to.at));
    app.electronics.board.tool = Some(Tool::Track(route));
    if reached || double {
        finish(app);
    }
}

/// The copper of no net a route ends on, which the track joins (and so gives its net): a via or a
/// track with nothing connected to it yet. `None` for anything else.
fn joined_copper(to: &Target) -> Option<DrcItem> {
    if to.net.is_some() {
        return None;
    }
    match to.on {
        On::Via(id) => Some(DrcItem::Via(id)),
        On::Track(id) => Some(DrcItem::Track(id)),
        _ => None,
    }
}

/// The route's track on `layer` with the leg to `to` added, as it will be made: runs that double
/// back taken out and the corners cut as the options say.
fn drawn(app: &SolveApp, route: &Route, to: [f64; 2], layer: CopperLayer, width: f64) -> (Vec<[f64; 2]>, Vec<f64>) {
    let (points, curves) = route.with_leg(app.electronics.board.route.bend, to);
    let (points, curves) = path::unfold(&points, &curves);
    mitered(app, points, curves, &app.electronics.board.route, layer, width)
}

/// Why a track of the net `net` may not end on the copper `to`: copper of another net, or a pad
/// on no net while the track has one (an unconnected pin). `None` when it may.
fn refused(app: &SolveApp, to: &Target, net: Option<usize>) -> Option<String> {
    let nets = stored_nets(app);
    let name = |n: usize| nets.name(n).unwrap_or_default().to_string();
    let pad = matches!(to.on, On::Pad(_));
    match (to.net, net) {
        (Some(here), Some(was)) if here != was => {
            let key = if pad { "electronics.board_other_net" } else { "electronics.board_other_net_copper" };
            Some(crate::i18n::format(key, &[("net", &name(here)), ("track", &name(was))]))
        }
        (None, Some(was)) if pad => Some(crate::i18n::format("electronics.board_pad_no_net", &[("track", &name(was))])),
        _ => None,
    }
}

/// Make the route the Route tool has drawn and say so; the next click starts the next one on
/// copper. A refused route stays drawn.
pub(crate) fn finish(app: &mut SolveApp) {
    let Some(Tool::Track(route)) = app.electronics.board.tool.clone() else { return };
    if let Ok(ended) = make(app, &route) {
        if ended {
            app.set_status(tr("electronics.board_route_made"), false);
        }
        app.electronics.board.tool = Some(Tool::Track(Route { ended, ..Route::default() }));
    }
}

/// One leg as `ecad.board.track` (or its `legs`) takes it: its track with runs that double back
/// taken out and its corners cut as the options say, ending on `via`; `None` when it leaves no
/// copper.
fn leg_params(app: &SolveApp, layer: CopperLayer, width: f64, points: &[[f64; 2]], curves: &[f64], via: Option<LegVia>) -> Option<Value> {
    let (points, curves) = path::unfold(points, curves);
    let (points, curves) = mitered(app, points, curves, &app.electronics.board.route, layer, width);
    let mut params = match (points.len() >= 2, via) {
        (true, _) => json!({"layer": layer.name(), "width": width, "points": points}),
        // A via alone, where the route changed layer before it had a segment there.
        (false, Some(v)) => json!({"layer": layer.name(), "width": width, "points": [v.at]}),
        (false, None) => return None,
    };
    if curves.iter().any(|c| is_arc(*c)) {
        params["curves"] = json!(curves);
    }
    if let Some(v) = via {
        params["via"] = v.params();
    }
    Some(params)
}

/// `route` made: each leg on its layer with the via it ends on, then the track on the active
/// layer, all one `ecad.board.track`; `Ok(true)` when it made copper, `Ok(false)` when it had none
/// to make.
fn make(app: &mut SolveApp, route: &Route) -> Result<bool, String> {
    let (layer, width) = (active_layer(app), track_width(app));
    let mut legs: Vec<Value> = route.legs.iter().filter_map(|l| leg_params(app, l.layer, l.width, &l.points, &l.curves, l.via)).collect();
    legs.extend(leg_params(app, layer, width, &route.points, &route.curves, None));
    let mut legs = legs.into_iter();
    let Some(mut params) = legs.next() else { return Ok(false) };
    let rest: Vec<Value> = legs.collect();
    if !rest.is_empty() {
        params["legs"] = json!(rest);
    }
    app.run("ecad.board.track", params).map(|_| true)
}

/// Whether `at` joins layers `a` and `b` already: a via whose span has both, or a through-hole pad,
/// is there, or a via `route` dropped before.
fn joined(app: &SolveApp, route: &Route, at: [f64; 2], a: CopperLayer, b: CopperLayer) -> bool {
    let n = board(app).copper_layers;
    board(app).vias.iter().any(|v| dist(v.at, at) <= v.diameter / 2.0 && v.on(a, n) && v.on(b, n))
        || stored_nets(app).pads.iter().any(|p| p.top && p.bottom && p.contains(at))
        || route.legs.iter().filter_map(|l| l.via).any(|v| v.joins(at, a, b, n))
}

/// V (`down`; Shift+V the other way): the Route tool goes on on another copper layer through a via
/// (see [`change_layer`]): on the other of the options' two layers when it is on one of them, else
/// on the next layer of the stack, wrapping round. `false` with no Route tool.
pub(crate) fn switch_layer(app: &mut SolveApp, down: bool) -> bool {
    let Some(Tool::Track(route)) = &app.electronics.board.tool else { return false };
    if let Some(i) = route.pad {
        end_in_pad(app, i);
        return true;
    }
    let target = next_layer(app, down);
    change_layer(app, target);
    true
}

/// V after a click on a pad of another layer: the route goes on to the pad's centre on the
/// active layer, drops a via there to the pad's layer (filled and capped, as a via in a pad must
/// be) and ends in the pad; each checked against other nets' copper as a click is.
fn end_in_pad(app: &mut SolveApp, i: usize) {
    let Some(Tool::Track(mut route)) = app.electronics.board.tool.clone() else { return };
    route.pad = None;
    let Some(pad) = stored_nets(app).pads.get(i).cloned() else { return };
    let layer = active_layer(app);
    let side = if pad.top { CopperLayer::Top } else { CopperLayer::Bottom };
    let at = snap_tidy(pad.at());
    let net = route.net.or(pad.net);
    if route.legs.len() + 2 > MAX_LEGS {
        let text = crate::i18n::format("electronics.board_route_legs", &[("n", &(MAX_LEGS - 1).to_string())]);
        app.set_status(text, true);
        return;
    }
    let width = width_of(app, net);
    if !app.electronics.board.route.allow {
        let (points, curves) = drawn(app, &route, at, layer, width);
        let (clashes, truncated, _) = track_clashes(app, net, layer, width, &points, &curves);
        let text = match clashes.first() {
            Some(c) => Some(clash_text(app, c, "electronics.board_clash")),
            None => truncated.then(|| tr("electronics.board_clash_unchecked")),
        };
        if let Some(text) = text {
            app.set_status(text, true);
            return;
        }
    }
    if route.last().is_some_and(|q| lattice(q) != lattice(at)) {
        route.add(app.electronics.board.route.bend, at);
    }
    route.net = net;
    let via = (!joined(app, &route, at, layer, side)).then(|| LegVia { capped: true, ..via_between(app, layer, side, at) });
    if let Some(v) = &via
        && !app.electronics.board.route.allow
        && let Some(text) = via_blocked(app, net, v)
    {
        app.set_status(text, true);
        return;
    }
    route.change(layer, width, via);
    app.electronics.board.tool = Some(Tool::Track(route));
    set_active_layer(app, side);
    finish(app);
}

/// The layer V (`down`; Shift+V the other way) goes on to (see [`switch_layer`]).
fn next_layer(app: &SolveApp, down: bool) -> CopperLayer {
    let stack = board(app).stack();
    let n = stack.len().max(1);
    let active = active_layer(app);
    let i = stack.iter().position(|l| *l == active).unwrap_or(0);
    let next = stack.get(if down { (i + 1) % n } else { (i + n - 1) % n }).copied().unwrap_or(CopperLayer::Top);
    let o = &app.electronics.board.route;
    match o.span {
        ViaSpan::Between(a, b) if !o.micro && active == a && board(app).has_layer(b) => b,
        ViaSpan::Between(a, b) if !o.micro && active == b && board(app).has_layer(a) => a,
        _ => next,
    }
}

/// The active layer becomes `layer`. While the Route tool has a route started, the route goes on
/// there through a via at its last point (unless copper there joins the two layers already), as
/// the options' span says (see [`via_between`]): the track so far and the via become a leg of the
/// route, made with the rest of it in one step. A via that would come closer to other nets'
/// copper than the clearance is refused, unless the options allow it.
pub(crate) fn change_layer(app: &mut SolveApp, layer: CopperLayer) {
    let active = active_layer(app);
    if layer == active {
        return;
    }
    if let Some(Tool::Track(mut route)) = app.electronics.board.tool.clone()
        && let Some(last) = route.last()
    {
        // The command takes the legs so far, this one and the track on the new layer.
        if route.legs.len() + 2 > MAX_LEGS {
            let text = crate::i18n::format("electronics.board_route_legs", &[("n", &(MAX_LEGS - 1).to_string())]);
            app.set_status(text, true);
            return;
        }
        let via = (!joined(app, &route, last, active, layer)).then(|| via_between(app, active, layer, last));
        if let Some(v) = &via
            && !app.electronics.board.route.allow
            && let Some(text) = via_blocked(app, route.net, v)
        {
            app.set_status(text, true);
            return;
        }
        route.change(active, track_width(app), via);
        app.electronics.board.tool = Some(Tool::Track(route));
    }
    set_active_layer(app, layer);
    // "Routing on" only while the Route tool is on; with another tool it is the active layer.
    let key = if matches!(app.electronics.board.tool, Some(Tool::Track(_))) {
        "electronics.board_routing_on"
    } else {
        "electronics.board_active_layer_now"
    };
    app.set_status(crate::i18n::format(key, &[("layer", &layer_name(layer))]), false);
}

/// Backspace or Escape while routing: take back the last click, back across a layer change too
/// (the active layer goes back with it), the start last. `false` when no route is started.
pub(crate) fn step_back(app: &mut SolveApp) -> bool {
    let Some(Tool::Track(route)) = &mut app.electronics.board.tool else { return false };
    let (took, layer) = route.step_back();
    if let Some(l) = layer {
        set_active_layer(app, l);
        app.set_status(crate::i18n::format("electronics.board_routing_on", &[("layer", &layer_name(l))]), false);
    }
    took
}

/// Undo (Cmd+Z) on the board while a route is started: its legs are made only when it ends, so
/// Undo takes back its last click, as Backspace does, rather than the route made before it.
/// `false` when no route is started there.
pub(crate) fn undo_step(app: &mut SolveApp) -> bool {
    routing(app) && step_back(app)
}

/// A route is started on the board: Undo takes back its clicks (the start last, which ends it)
/// before it undoes anything made.
pub(crate) fn routing(app: &SolveApp) -> bool {
    matches!(&app.electronics.board.tool, Some(Tool::Track(route)) if !route.is_empty()) && crate::electronics_workspace::board(app)
}

/// Right-click while routing: the next bend style.
pub(crate) fn cycle_bend(app: &mut SolveApp) {
    let bend = app.electronics.board.route.bend.next();
    set_bend(app, bend);
}

/// `/`: the bend style the other way round. `false` with no Route tool.
pub(crate) fn flip_bend(app: &mut SolveApp) -> bool {
    if !matches!(app.electronics.board.tool, Some(Tool::Track(_))) {
        return false;
    }
    let bend = app.electronics.board.route.bend.flipped();
    set_bend(app, bend);
    true
}

fn set_bend(app: &mut SolveApp, bend: BoardBend) {
    app.electronics.board.route.bend = bend;
    app.set_status(crate::i18n::format("electronics.board_bend_now", &[("bend", crate::i18n::tr(bend.name()))]), false);
}

/// The Route tool under the pointer `hover`: the legs on the layers it left and their vias, the
/// track so far and the leg to the pointer as the bend style and miter make them, in each layer's
/// colour, what comes closer to other nets' copper than the clearance in the error colour with a
/// ring where, the pads still to reach outlined, and a thin airwire from the pointer to the
/// nearest of them (or to the airwire's far end).
pub(crate) fn paint(app: &SolveApp, painter: &egui::Painter, map: PageMap, hover: Pos2, free: bool, t: &Tokens) {
    let s = &app.electronics.board;
    let Some(Tool::Track(route)) = &s.tool else { return };
    let layer = active_layer(app);
    let to =
        if route.is_empty() { target(app, map, hover, free, None, true, None) } else { target(app, map, hover, free, Some(layer), false, route.net) };
    let width = width_of(app, route.net.or(to.net));
    let stroke_of = |width: f64| ((width * map.scale()) as f32).clamp(1.0, 400.0);
    let w = stroke_of(width);
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    let line = |points: &[[f64; 2]], curves: &[f64], stroke: Stroke| {
        let line: Vec<Pos2> = tessellate(points, curves, false, arc_tolerance(map)).into_iter().map(screen).collect();
        if line.iter().all(finite) {
            painter.add(egui::Shape::line(line, stroke));
            for q in points {
                painter.circle_filled(screen(*q), stroke.width / 2.0, stroke.color);
            }
        }
    };
    for leg in &route.legs {
        let (points, curves) = path::unfold(&leg.points, &leg.curves);
        let (points, curves) = mitered(app, points, curves, &s.route, leg.layer, leg.width);
        line(&points, &curves, Stroke::new(stroke_of(leg.width), layer_colour(t, leg.layer).gamma_multiply(0.85)));
        if let Some(v) = leg.via {
            let c = screen(v.at);
            if finite(&c) {
                painter.circle_filled(c, ((v.diameter / 2.0 * map.scale()) as f32).max(2.0), t.pcb_plated);
                painter.circle_filled(c, ((v.drill / 2.0 * map.scale()) as f32).max(1.0), t.viewport_bottom);
            }
        }
    }
    if !route.is_empty() {
        let (points, curves) = drawn(app, route, to.at, layer, width);
        line(&points, &curves, Stroke::new(w, layer_colour(t, layer).gamma_multiply(0.85)));
        // What breaks the clearance, over it.
        let (clashes, _, pieces) = track_clashes(app, route.net.or(to.net), layer, width, &points, &curves);
        for c in &clashes {
            if let Some((a, b)) = pieces.get(c.piece) {
                let (a, b) = (screen(*a), screen(*b));
                if finite(&a) && finite(&b) {
                    painter.line_segment([a, b], Stroke::new(w, t.pcb_drc_error));
                    painter.circle_filled(a, w / 2.0, t.pcb_drc_error);
                    painter.circle_filled(b, w / 2.0, t.pcb_drc_error);
                }
            }
            painter.circle_stroke(screen(c.at), 6.0, Stroke::new(1.5, t.pcb_drc_error));
        }
        // The via V would drop at the last click, when it would break the clearance.
        if let Some(last) = route.last() {
            let next = next_layer(app, true);
            if next != layer && !joined(app, route, last, layer, next) && !app.electronics.board.route.allow {
                let v = via_between(app, layer, next, last);
                if !via_clashes(app, route.net, &v).0.is_empty() {
                    paint_ghost_via(app, painter, map, &v, route.net, t.pcb_drc_error, t);
                }
            }
        }
        let nets = stored_nets(app);
        let goals = goals(app, route, layer);
        for pad in goals.iter().filter_map(|i| nets.pads.get(*i)) {
            let ring: Vec<Pos2> = pad.outline(6).into_iter().map(screen).collect();
            if ring.iter().all(finite) {
                painter.add(egui::Shape::closed_line(ring, Stroke::new(2.0, t.sel_edge)));
            }
        }
        let nearest = goals.iter().filter_map(|i| nets.pads.get(*i)).map(|pad| pad.at()).min_by(|a, b| dist(*a, to.at).total_cmp(&dist(*b, to.at)));
        if let Some(g) = route.goal.or(nearest) {
            painter.line_segment([screen(to.at), screen(g)], Stroke::new(1.0, t.pcb_airwire));
        }
    }
    painter.circle_stroke(screen(to.at), (w / 2.0).max(4.0), Stroke::new(1.5, t.sel_edge.gamma_multiply(0.7)));
}

/// What the Unroute, Miter and Split tools would act on under the pointer `hover`, outlined: the
/// track or via (for Unroute, all the run a click takes away, or with `net` all its net's copper),
/// and the corner the Miter tool would cut or the point the Split tool would split at.
pub(crate) fn paint_hover(app: &SolveApp, painter: &egui::Painter, map: PageMap, hover: Pos2, net: bool, t: &Tokens) {
    let tool = &app.electronics.board.tool;
    let unroute = matches!(tool, Some(Tool::Unroute));
    let Some(id) = copper_at(app, map, hover, !unroute) else { return };
    let b = board(app);
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    let mark = Stroke::new(1.5, t.sel_edge);
    let outline_via = |v: &solidcraft_engine::doc::ecad::Via| {
        painter.circle_stroke(screen(v.at), ((v.diameter / 2.0 * map.scale()) as f32).max(2.0) + 3.0, mark);
    };
    let outline_track = |track: &Track| {
        let line: Vec<Pos2> = tessellate(&track.points, &track.curves, false, arc_tolerance(map)).into_iter().map(screen).collect();
        if line.iter().all(finite) {
            painter.add(egui::Shape::line(line, mark));
        }
    };
    if unroute {
        let (tracks, vias) = unrouted_by(app, id, net);
        b.tracks.iter().zip(tracks).filter(|(_, gone)| *gone).for_each(|(track, _)| outline_track(track));
        b.vias.iter().zip(vias).filter(|(_, gone)| *gone).for_each(|(v, _)| outline_via(v));
        return;
    }
    if let Some(v) = b.via(id) {
        outline_via(v);
        return;
    }
    let Some(track) = b.track(id) else { return };
    outline_track(track);
    let last = track.points.len().saturating_sub(1);
    match tool {
        Some(Tool::Miter) => {
            // The corner in reach, or every corner a click on the segment cuts.
            let corner = corner_at(track, map, hover);
            for (i, q) in track.points.iter().enumerate() {
                if i >= 1 && i < last && corner.is_none_or(|c| c == i) {
                    painter.circle_stroke(screen(*q), 5.0, mark);
                }
            }
        }
        Some(Tool::Split) => {
            if let Some((q, _)) = nearest_on(track, page_point(map, hover)) {
                painter.circle_stroke(screen(q), 5.0, mark);
            }
        }
        _ => {}
    }
}

/// The via or track under `p`, vias first, then the tracks the way they are drawn (the active
/// layer's first); only a track with `tracks`.
pub(crate) fn copper_at(app: &SolveApp, map: PageMap, p: Pos2, tracks: bool) -> Option<u64> {
    let b = board(app);
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    if !tracks && let Some(v) = b.vias.iter().rev().find(|v| screen(v.at).distance(p) <= ((v.diameter / 2.0 * map.scale()) as f32).max(PICK_POINTS)) {
        return Some(v.id);
    }
    pick_order(app, b)
        .into_iter()
        .filter(|l| copper_shown(app, *l))
        .find_map(|layer| b.tracks.iter().rev().filter(|t| t.layer == layer).find(|t| on_track(map, t, p)))
        .map(|t| t.id)
}

/// The inner point of `t` within reach of `p`, if any.
fn corner_at(t: &Track, map: PageMap, p: Pos2) -> Option<usize> {
    let last = t.points.len().saturating_sub(1);
    (1..last).find(|i| t.points.get(*i).is_some_and(|q| map.to_screen(q[0], q[1]).distance(p) <= PICK_POINTS + 2.0))
}

/// What a press on a picked track holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Grab {
    /// An inner point, by index: it moves, and its two segments follow.
    Corner(usize),
    /// The first or the last point: it moves, onto a pad's or via's centre in reach.
    End(usize),
    /// A segment, by index: it slides across itself (see [`Reshape::apply`]).
    Segment(usize),
}

/// A drag that reshapes one track.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Reshape {
    pub track: u64,
    layer: CopperLayer,
    /// The track as it was (split already, for the Split tool).
    points: Vec<[f64; 2]>,
    curves: Vec<f64>,
    pub grab: Grab,
    /// The segment nearest the press.
    pub segment: usize,
    /// The ends that sit on copper (a pad, a via, another track of the layer), which a sliding
    /// segment leaves where they are.
    fixed: [bool; 2],
    /// With the Split tool: the segment and point split, sent as `ecad.board.track.split` when
    /// the press goes nowhere.
    split: Option<(usize, [f64; 2])>,
    /// The track's net, which a held end may join.
    net: Option<usize>,
}

/// Whether the point `q` of track `t` sits on copper that stays: a pad or a via, or another track
/// of its layer.
fn anchored(app: &SolveApp, t: &Track, q: [f64; 2]) -> bool {
    let b = board(app);
    let on = |o: &Track| path::segments(&o.points, &o.curves, false).any(|(a, c, s)| path::segment_distance(q, a, c, s) <= o.width / 2.0);
    stored_nets(app).pads.iter().any(|pad| pad.on(t.layer) && pad.contains(q))
        || b.vias.iter().any(|v| dist(v.at, q) <= v.diameter / 2.0)
        || b.tracks.iter().any(|o| o.id != t.id && o.layer == t.layer && on(o))
}

/// What a press at `p` on track `id` holds: an end or a corner within reach, else the nearest
/// segment.
pub(crate) fn reshape_at(app: &SolveApp, map: PageMap, id: u64, p: Pos2) -> Option<Reshape> {
    let t = board(app).track(id)?;
    let last = t.points.len().checked_sub(1)?;
    let (_, segment) = nearest_on(t, page_point(map, p))?;
    let near = |i: usize| t.points.get(i).is_some_and(|q| map.to_screen(q[0], q[1]).distance(p) <= PICK_POINTS + 2.0);
    let grab = if near(0) {
        Grab::End(0)
    } else if near(last) {
        Grab::End(last)
    } else {
        corner_at(t, map, p).map_or(Grab::Segment(segment), Grab::Corner)
    };
    Some(Reshape::of(app, t, grab, segment, None))
}

/// A press with the Split tool at `p` on a track: the track split at the point of its segment
/// nearest `p` (not sent yet), with the new corner held.
pub(crate) fn split_at(app: &SolveApp, map: PageMap, p: Pos2) -> Option<Reshape> {
    let id = copper_at(app, map, p, true)?;
    let t = board(app).track(id)?;
    let (_, segment) = nearest_on(t, page_point(map, p))?;
    let mut split = t.clone();
    let corner = split.split(segment, page_point(map, p)).ok()?;
    let at = split.points.get(corner).copied()?;
    Some(Reshape::of(app, &split, Grab::Corner(corner), segment, Some((segment, at))))
}

/// Whether copper of the net `copper` may join a track of the net `track`: the same net, or a
/// track of none.
fn joins(track: Option<usize>, copper: Option<usize>) -> bool {
    track.is_none() || copper == track
}

/// Where a held corner or end goes with the pointer at `p`: an end onto the centre of a pad or
/// via in reach (on the track's layer) that the track may join, else the grid.
pub(crate) fn held_point(app: &SolveApp, map: PageMap, r: &Reshape, p: Pos2, free: bool) -> [f64; 2] {
    if matches!(r.grab, Grab::End(_)) {
        let nets = stored_nets(app);
        if let Some(pad) = pad_under(&nets, map, p, Some(r.layer), r.layer, r.net).and_then(|i| nets.pads.get(i)).filter(|pad| joins(r.net, pad.net))
        {
            return snap_tidy(pad.at());
        }
        let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
        let reach = |v: &solidcraft_engine::doc::ecad::Via| ((v.diameter / 2.0 * map.scale()) as f32).max(PICK_POINTS);
        if let Some((v, _)) =
            board(app).vias.iter().zip(&nets.via_nets).find(|(v, net)| screen(v.at).distance(p) <= reach(v) && (net.is_none() || joins(r.net, **net)))
        {
            return v.at;
        }
    }
    snap(app, map, p, free)
}

/// Why the held end of `r` may not go to `at`: a pad or via there of another net (or a pad of no
/// net for a track of one). `None` when it may.
fn end_refused(app: &SolveApp, r: &Reshape, at: [f64; 2]) -> Option<String> {
    let nets = stored_nets(app);
    let pad = nets.pads.iter().position(|pad| pad.on(r.layer) && pad.contains(at));
    let to = match pad {
        Some(i) => Target { at, net: nets.pads.get(i).and_then(|pad| pad.net), on: On::Pad(i) },
        None => {
            let (v, net) = board(app).vias.iter().zip(&nets.via_nets).find(|(v, _)| dist(v.at, at) <= v.diameter / 2.0)?;
            Target { at, net: *net, on: On::Via(v.id) }
        }
    };
    refused(app, &to, r.net)
}

impl Reshape {
    fn of(app: &SolveApp, t: &Track, grab: Grab, segment: usize, split: Option<(usize, [f64; 2])>) -> Reshape {
        let last = t.points.len().saturating_sub(1);
        let fixed = [0, last].map(|i| t.points.get(i).is_some_and(|q| anchored(app, t, *q)));
        let b = board(app);
        let net = b.tracks.iter().position(|o| o.id == t.id).and_then(|i| stored_nets(app).track_nets.get(i).copied().flatten());
        Reshape { track: t.id, layer: t.layer, points: t.points.clone(), curves: t.curves.clone(), grab, segment, fixed, split, net }
    }

    /// The track as the drag leaves it: the held corner or end at `to`, or the held segment moved
    /// by `by` (see [`Reshape::slide`]); jogs at 90° with `square`, else at 45°.
    pub(crate) fn apply(&self, by: [f64; 2], to: [f64; 2], square: bool) -> (Vec<[f64; 2]>, Vec<f64>) {
        let mut nodes = self.nodes();
        match self.grab {
            Grab::Corner(i) | Grab::End(i) => {
                if let Some(n) = nodes.get_mut(i) {
                    n.0 = to;
                }
            }
            Grab::Segment(k) => nodes = self.slide(k, by, square),
        }
        settle(nodes)
    }

    /// Each point with the sweep of the segment into it (0 for the first).
    fn nodes(&self) -> Vec<([f64; 2], f64)> {
        self.points.iter().enumerate().map(|(i, p)| (*p, if i == 0 { 0.0 } else { sweep(&self.curves, i - 1) })).collect()
    }

    /// Segment `k` moved across itself by `by` (an arc by all of `by`): each straight neighbour
    /// keeps its direction and stretches to meet it, so the corners keep their angles. Where one
    /// cannot (it would turn back, it runs parallel or is an arc, or the end there sits on copper)
    /// a short jog joins them instead, at 45° (90° with `square`, or where the segment is too short
    /// for two 45° jogs). A free end moves with the segment.
    fn slide(&self, k: usize, by: [f64; 2], square: bool) -> Vec<([f64; 2], f64)> {
        let nodes = self.nodes();
        let (Some(&(a, _)), Some(&(b, s))) = (nodes.get(k), nodes.get(k + 1)) else { return nodes };
        let length = dist(a, b);
        if !(length > 0.0) {
            return nodes;
        }
        let arc = is_arc(s);
        let u = [(b[0] - a[0]) / length, (b[1] - a[1]) / length];
        let normal = [-u[1], u[0]];
        let d = if arc { by } else { scaled(normal, dot(by, normal)) };
        let off = d[0].hypot(d[1]);
        if !(off > 1e-9) {
            return nodes;
        }
        let first = if square || arc { 0.0 } else { off };
        // The last try never fails: both corners keep their neighbours and take a square jog.
        for (meet, jog) in [(true, first), (true, 0.0), (false, 0.0)] {
            if let Some(out) = self.try_slide(&nodes, k, d, meet, jog) {
                return out;
            }
        }
        nodes
    }

    /// One way of [`Reshape::slide`] for segment `k` moved by `d`: neighbours that `meet` the moved
    /// segment where they can; a neighbour the moved segment has passed is dropped and its far point
    /// joined to the segment by a jog; elsewhere a jog `jog` long along the segment joins it to its
    /// old corner. `None` when the moved segment would turn back.
    fn try_slide(&self, nodes: &[([f64; 2], f64)], k: usize, d: [f64; 2], meet: bool, jog: f64) -> Option<Vec<([f64; 2], f64)>> {
        let (a, into) = *nodes.get(k)?;
        let (b, s) = *nodes.get(k + 1)?;
        let (a2, b2) = (add(a, d), add(b, d));
        let length = dist(a, b);
        let u = [(b[0] - a[0]) / length, (b[1] - a[1]) / length];
        let arc = is_arc(s);
        let along = |q: [f64; 2]| dot([q[0] - a2[0], q[1] - a2[1]], u);
        // Where a jog from `q` (a point left behind) reaches the moved segment's line: straight
        // across it, or at 45° `forward` along it (back with -1).
        let landing = |q: [f64; 2], forward: f64| {
            let t = along(q);
            let off = if jog > 0.0 { (u[0] * (q[1] - a2[1]) - u[1] * (q[0] - a2[0])).abs() } else { 0.0 };
            add(a2, scaled(u, t + forward * off))
        };
        let straight = meet && !arc;
        let mut out: Vec<([f64; 2], f64)> = nodes.get(..k)?.to_vec();
        // The segment's start: where the neighbour before meets it, or a jog.
        let before = k.checked_sub(1).and_then(|i| nodes.get(i)).map(|n| n.0);
        let met = before.filter(|_| straight && !is_arc(into)).map(|far| meet_point(far, a, a2, u));
        let from = match (before, met) {
            (None, _) if !self.fixed[0] => {
                out.push((a2, 0.0));
                0.0
            }
            (Some(_), Some(Meet::At(x))) => {
                out.push((x, into));
                along(x)
            }
            (Some(far), Some(Meet::Passed)) => {
                // `out` ends on the far point already.
                let j = landing(far, 1.0);
                out.push((j, 0.0));
                along(j)
            }
            _ => {
                out.push((a, into));
                out.push((add(a2, scaled(u, jog)), 0.0));
                jog
            }
        };
        // Its end, the same way round.
        let after = nodes.get(k + 2).copied();
        let met = after.filter(|n| straight && !is_arc(n.1)).map(|n| meet_point(n.0, b, b2, u));
        let (to, rest) = match (after, met) {
            (None, _) if !self.fixed[1] => {
                out.push((b2, s));
                (length, k + 2)
            }
            (Some(_), Some(Meet::At(x))) => {
                out.push((x, s));
                (along(x), k + 2)
            }
            (Some((far, _)), Some(Meet::Passed)) => {
                let j = landing(far, -1.0);
                out.push((j, s));
                out.push((far, 0.0));
                (along(j), k + 3)
            }
            _ => {
                out.push((add(b2, scaled(u, -jog)), s));
                out.push((b, 0.0));
                (length - jog, k + 2)
            }
        };
        if !arc && !(to - from > 1e-6) {
            return None;
        }
        out.extend_from_slice(nodes.get(rest..).unwrap_or(&[]));
        Some(out)
    }
}

/// How a neighbouring segment's line meets a moved segment's line.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Meet {
    /// Here, beyond the neighbour's far point: the neighbour keeps its direction.
    At([f64; 2]),
    /// Behind the far point: the moved segment has passed it.
    Passed,
    /// Not at all: they run parallel.
    Parallel,
}

/// Where the line from `far` through `corner` meets the line through `on` along the unit `u`.
fn meet_point(far: [f64; 2], corner: [f64; 2], on: [f64; 2], u: [f64; 2]) -> Meet {
    let len = dist(far, corner);
    if !(len > 0.0) {
        return Meet::Parallel;
    }
    let v = [(corner[0] - far[0]) / len, (corner[1] - far[1]) / len];
    let cross = v[0] * u[1] - v[1] * u[0];
    if cross.abs() < 1e-6 {
        return Meet::Parallel;
    }
    let w = [on[0] - far[0], on[1] - far[1]];
    let t = (w[0] * u[1] - w[1] * u[0]) / cross;
    if !t.is_finite() {
        Meet::Parallel
    } else if t > 1e-4 {
        Meet::At(add(far, scaled(v, t)))
    } else {
        Meet::Passed
    }
}

/// Points and sweeps from `nodes` (each point with the sweep into it), rounded to the lattice,
/// with segments of no length dropped, a straight run through a point that is no bend joined, and
/// runs that double back taken out (as the command refuses them: an end dragged back along its
/// segment shortens it).
fn settle(nodes: Vec<([f64; 2], f64)>) -> (Vec<[f64; 2]>, Vec<f64>) {
    let mut points: Vec<[f64; 2]> = Vec::with_capacity(nodes.len());
    let mut curves: Vec<f64> = Vec::with_capacity(nodes.len());
    for (p, s) in nodes {
        let p = snap_point(p);
        if points.last().is_some_and(|q| lattice(*q) == lattice(p)) {
            continue;
        }
        let n = points.len();
        if n >= 2
            && !is_arc(s)
            && curves.last().is_some_and(|c| !is_arc(*c))
            && let (Some(a), Some(b)) = (points.get(n - 2), points.get(n - 1))
            && straight_through(lattice(*a), lattice(*b), lattice(p))
        {
            points.pop();
            curves.pop();
        }
        if !points.is_empty() {
            curves.push(s);
        }
        points.push(p);
    }
    if curves.iter().all(|c| !is_arc(*c)) {
        curves.clear();
    }
    path::unfold(&points, &curves)
}

/// Whether `b` lies on the way from `a` to `c`, on one straight line (lattice points; far-off ones
/// saturate rather than overflow).
fn straight_through(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> bool {
    let (abx, aby) = (i128::from(b[0]) - i128::from(a[0]), i128::from(b[1]) - i128::from(a[1]));
    let (bcx, bcy) = (i128::from(c[0]) - i128::from(b[0]), i128::from(c[1]) - i128::from(b[1]));
    let cross = abx.saturating_mul(bcy).saturating_sub(aby.saturating_mul(bcx));
    let along = abx.saturating_mul(bcx).saturating_add(aby.saturating_mul(bcy));
    cross == 0 && along > 0
}

/// The end of a reshape drag: one `ecad.board.track.reshape` with the track as it leaves it (an
/// end on copper of another net is refused); with the Split tool, a press that went nowhere splits
/// the segment where it was pressed. The picked segment is then the one nearest `to`, as the
/// reshape may have added or joined segments before it.
pub(crate) fn release(app: &mut SolveApp, r: &Reshape, moving: bool, by: [f64; 2], to: [f64; 2]) {
    if !moving {
        if let Some((segment, at)) = r.split {
            drop(app.run("ecad.board.track.split", json!({"track": r.track, "segment": segment, "at": at})));
        }
        return;
    }
    let (points, curves) = r.apply(by, to, app.electronics.board.route.bend.square());
    let Some(t) = board(app).track(r.track) else { return };
    if points.len() < 2 || (points == t.points && curves == t.curves) {
        return;
    }
    let end = match r.grab {
        Grab::End(0) => points.first().copied(),
        Grab::End(_) => points.last().copied(),
        _ => None,
    };
    if let Some(text) = end.and_then(|at| end_refused(app, r, at)) {
        app.set_status(text, true);
        return;
    }
    // Copper brought too close to another net's is refused, as a move is (Cmd at the drop, or
    // Allow violations, reshapes it anyway).
    let check = crate::electronics_board::checked(app);
    let mut params = json!({"track": r.track, "points": points, "check": check});
    if !curves.is_empty() {
        params["curves"] = json!(curves);
    }
    let before = crate::electronics_board::before_forced(app, check);
    match app.run("ecad.board.track.reshape", params) {
        Ok(_) => {
            if app.electronics.board.segment.is_some_and(|(id, _)| id == r.track) {
                let nearest = board(app).track(r.track).and_then(|t| nearest_on(t, to)).map(|(_, k)| k);
                app.electronics.board.segment = nearest.map(|k| (r.track, k));
            }
            // Made anyway: what it broke, if anything.
            crate::electronics_board::say_forced(app, before.as_ref(), &std::collections::BTreeSet::from([r.track]), None);
        }
        Err(_) if check => {
            let mut after = board(app).clone();
            if let Some(t) = after.tracks.iter_mut().find(|t| t.id == r.track) {
                t.points = points;
                t.curves = curves;
            }
            crate::electronics_board::clash_refused(app, &after, &std::collections::BTreeSet::from([r.track]), "electronics.board_refused_reshape");
        }
        Err(_) => {}
    }
}

/// Which tracks and vias (in board order) a click of the Unroute tool on the track or via `id`
/// takes away: its routed run, or with `net` all the copper of its net (the run while it has
/// none).
fn unrouted_by(app: &SolveApp, id: u64, net: bool) -> (Vec<bool>, Vec<bool>) {
    let b = board(app);
    let nets = stored_nets(app);
    let track = b.tracks.iter().position(|t| t.id == id);
    let via = b.vias.iter().position(|v| v.id == id);
    let of_net = track.and_then(|i| nets.track_nets.get(i).copied().flatten()).or_else(|| via.and_then(|i| nets.via_nets.get(i).copied().flatten()));
    if net && let Some(n) = of_net {
        let on = |m: &Option<usize>| *m == Some(n);
        return (nets.track_nets.iter().map(on).collect(), nets.via_nets.iter().map(on).collect());
    }
    let Some(runs) = crate::electronics_board::stored_runs(app) else {
        return (b.tracks.iter().map(|t| t.id == id).collect(), b.vias.iter().map(|v| v.id == id).collect());
    };
    let run = track.and_then(|i| runs.tracks.get(i)).or_else(|| via.and_then(|i| runs.vias.get(i))).copied();
    let of = |r: &usize| Some(*r) == run;
    (runs.tracks.iter().map(of).collect(), runs.vias.iter().map(of).collect())
}

/// A click with the Unroute tool: the routed run of the track or via under `p` goes; with `net`
/// (Shift) all the copper of its net.
pub(crate) fn unroute_click(app: &mut SolveApp, map: PageMap, p: Pos2, net: bool) {
    let Some(id) = copper_at(app, map, p, false) else { return };
    let name = if net { net_name(app, id) } else { None };
    let params = match name {
        Some(name) => json!({"nets": [name]}),
        None => json!({"items": [id]}),
    };
    drop(app.run("ecad.board.unroute", params));
}

/// The net of the track or via `id`, by name.
fn net_name(app: &SolveApp, id: u64) -> Option<String> {
    let b = board(app);
    let nets = stored_nets(app);
    let net = b
        .tracks
        .iter()
        .position(|t| t.id == id)
        .and_then(|i| nets.track_nets.get(i).copied().flatten())
        .or_else(|| b.vias.iter().position(|v| v.id == id).and_then(|i| nets.via_nets.get(i).copied().flatten()))?;
    nets.name(net).map(str::to_string)
}

/// A click with the Miter tool: the corner under `p` cut, or on a segment every corner of its
/// track, by the options' radius and style.
pub(crate) fn miter_click(app: &mut SolveApp, map: PageMap, p: Pos2) {
    let o = app.electronics.board.route.clone();
    if !(o.miter > 0.0) {
        app.set_status(tr("electronics.board_miter_radius"), true);
        return;
    }
    let Some(t) = copper_at(app, map, p, true).and_then(|id| board(app).track(id)).cloned() else { return };
    let corner = corner_at(&t, map, p);
    // Tried here first, so a click that cuts nothing is no step to undo.
    let cuts = t.clone().miter(corner, o.miter, o.miter_style, miter_keeps(board(app), &stored_nets(app).pads, &t));
    if cuts == 0 {
        app.set_status(tr("electronics.board_miter_none"), true);
        return;
    }
    let style = match o.miter_style {
        MiterStyle::Round => "round",
        MiterStyle::Straight => "straight",
    };
    let mut params = json!({"track": t.id, "radius": o.miter, "style": style});
    if let Some(c) = corner {
        params["corner"] = json!(c);
    }
    drop(app.run("ecad.board.track.miter", params));
}

/// A copper layer's name as the UI shows it: Top, Inner 1 …, Bottom.
pub(crate) fn layer_name(layer: CopperLayer) -> String {
    match layer {
        CopperLayer::Top => tr("electronics.side_top"),
        CopperLayer::Inner(n) => crate::i18n::format("electronics.board_layer_inner", &[("n", &n.to_string())]),
        CopperLayer::Bottom => tr("electronics.side_bottom"),
    }
}

/// A small square of `colour`.
fn swatch(ui: &mut Ui, colour: Color32) {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(vec2(t.space_12, t.space_12), Sense::hover());
    ui.painter().rect_filled(rect.shrink(1.0), f32::from(t.control_radius) / 2.0, colour);
}

/// A dropdown of the stack's copper layers, each with its colour; `Some` with a layer chosen
/// other than `current`.
pub(crate) fn layer_row(app: &SolveApp, ui: &mut Ui, key: &str, current: CopperLayer) -> Option<CopperLayer> {
    span_row(app, ui, key, crate::i18n::tr("electronics.field_layer"), current)
}

/// [`layer_row`] under the label `label`.
pub(crate) fn span_row(app: &SolveApp, ui: &mut Ui, key: &str, label: &str, current: CopperLayer) -> Option<CopperLayer> {
    shared_layer_row(app, ui, key, label, Some(current))
}

/// [`span_row`] for several objects: `None` shows "Mixed" (their layers differ).
pub(crate) fn shared_layer_row(app: &SolveApp, ui: &mut Ui, key: &str, label: &str, shared: Option<CopperLayer>) -> Option<CopperLayer> {
    let t = Tokens::get();
    let stack = board(app).stack();
    let mut chosen = None;
    let shown = shared.map_or_else(|| tr("electronics.board_mixed"), layer_name);
    ui_kit::row(ui, label, |ui| {
        ui.horizontal(|ui| {
            if let Some(current) = shared {
                swatch(ui, layer_colour(&t, current));
            }
            ui_kit::dropdown_menu(ui, ("electronics_layer", key), &shown, |ui| {
                for layer in &stack {
                    ui.horizontal(|ui| {
                        swatch(ui, layer_colour(&t, *layer));
                        let r = ui_kit::list_picker(ui, &format!("{key}_{}", layer.name()), &layer_name(*layer), Some(*layer) == shared);
                        crate::scenario::publish_handle(&format!("dialog:{key}:{}", layer.name()), r.rect.center());
                        if r.clicked() {
                            chosen = Some(*layer);
                        }
                    });
                }
            });
        });
    });
    chosen.filter(|l| Some(*l) != shared)
}

/// A bend style's chip, `width` wide: its icon, with its name as the tooltip.
fn bend_chip(ui: &mut Ui, bend: BoardBend, selected: bool, width: f32) -> egui::Response {
    let h = ui_kit::row_height();
    let name = crate::i18n::tr(bend.name());
    // The kit's list entry at the chip's width (it publishes `dialog:list:<bend>`), the icon on it.
    let response = ui.allocate_ui(vec2(width, h), |ui| ui_kit::list_picker(ui, bend.name(), "", selected)).inner.on_hover_text(name);
    let colour = ui.style().interact_selectable(&response, selected).fg_stroke.color;
    bend.paint_icon(ui.painter(), response.rect.shrink2(vec2(h * 0.3, h * 0.25)), Stroke::new(1.5, colour));
    response
}

/// The bend styles' chips, as many rows of them as the Inspector's width takes.
fn bend_rows(app: &mut SolveApp, ui: &mut Ui) {
    let bend = &mut app.electronics.board.route.bend;
    let chip = ui_kit::row_height() * 1.3;
    let mut label = crate::i18n::tr("electronics.field_bend");
    let mut shown = 0;
    while shown < BOARD_BENDS.len() {
        let before = shown;
        ui_kit::row(ui, label, |ui| {
            let gap = ui.spacing().item_spacing.x;
            let fit = (((ui.available_width() + gap) / (chip + gap)).floor() as usize).max(1);
            for (b, _) in BOARD_BENDS.iter().skip(shown).take(fit) {
                if bend_chip(ui, *b, *bend == *b, chip).clicked() {
                    *bend = *b;
                }
                shown += 1;
            }
        });
        if shown == before {
            break;
        }
        label = "";
    }
}

/// A width field in millimetres with a menu of [`WIDTHS`] beside it; `Some` with a new width.
pub(crate) fn width_row(app: &mut SolveApp, ui: &mut Ui, key: &str, label: &str, current: f64) -> Option<f64> {
    width_field(app, ui, key, label, current, None).flatten()
}

/// As [`width_row`]; with `class` (whether the net class's width is in use), the menu starts with
/// the net class's width, and it or an emptied field gives `Some(None)`.
fn width_field(app: &mut SolveApp, ui: &mut Ui, key: &str, label: &str, current: f64, class: Option<bool>) -> Option<Option<f64>> {
    let old = crate::electronics_editor::mm(current);
    let mut text = crate::electronics_editor::typed(app, key, &old);
    let mut picked = None;
    let response = ui_kit::row(ui, label, |ui| {
        let h = ui_kit::row_height();
        let menu = h * 1.4;
        let width = (ui.available_width() - menu - ui.spacing().item_spacing.x).max(0.0);
        ui.horizontal(|ui| {
            let field =
                ui.allocate_ui(vec2(width, h), |ui| ui_kit::cell_editor(ui, egui::Id::new(("electronics_field", key)), &mut text, "", false)).inner;
            ui.allocate_ui(vec2(menu, h), |ui| {
                ui_kit::dropdown_menu(ui, ("electronics_widths", key), "", |ui| {
                    if let Some(on) = class {
                        let r = ui_kit::list_picker(ui, &format!("{key}_class"), crate::i18n::tr("electronics.board_width_net_class"), on);
                        crate::scenario::publish_handle(&format!("dialog:{key}:class"), r.rect.center());
                        if r.clicked() {
                            picked = Some(None);
                        }
                    }
                    for w in WIDTHS {
                        let mm = crate::electronics_editor::mm(w);
                        let label = crate::i18n::format("electronics.board_width_mm", &[("width", &mm)]);
                        let r = ui_kit::list_picker(ui, &format!("{key}_{mm}"), &label, class != Some(true) && (w - current).abs() < 1e-9);
                        crate::scenario::publish_handle(&format!("dialog:{key}:{mm}"), r.rect.center());
                        if r.clicked() {
                            picked = Some(Some(w));
                        }
                    }
                });
            });
            field
        })
        .inner
    });
    let typed = crate::electronics_editor::settle(app, ui, key, &response, text, &old);
    if picked.is_some() {
        return picked;
    }
    let text = typed?;
    if class.is_some() && text.trim().is_empty() {
        return Some(None);
    }
    let value = crate::prefs::parse_number(&text);
    if value.is_none() {
        app.set_status(crate::i18n::format("electronics.not_a_number", &[("field", label), ("text", &text)]), true);
    }
    value.map(Some)
}

/// The Route tool's options: the layer, how it bends, its width, the miter of its corners and the
/// via a layer change drops.
pub(crate) fn options(app: &mut SolveApp, ui: &mut Ui) {
    if let Some(layer) = layer_row(app, ui, "board:route:layer", active_layer(app)) {
        change_layer(app, layer);
    }
    bend_rows(app, ui);
    let label = tr("electronics.field_track_width");
    // The menu's Net Class entry, or an emptied field, goes back to the net class's width.
    let class = Some(app.electronics.board.route.width.is_none());
    match width_field(app, ui, "board:route:width", &label, track_width(app), class) {
        Some(Some(v)) if check_track_width(v).is_ok() => app.electronics.board.route.width = Some(v),
        Some(Some(_)) => app.set_status(tr("electronics.board_bad_width"), true),
        Some(None) => app.electronics.board.route.width = None,
        None => {}
    }
    // A width not typed is the net class's: say which.
    if app.electronics.board.route.width.is_none()
        && let Some(class) = routed_class(app)
    {
        let text = crate::i18n::format("electronics.board_width_class", &[("class", &class)]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.board_width_class"));
    }
    allow_row(app, ui);
    miter_options(app, ui);
    via_options(app, ui);
}

/// Allow violations: the Route and Via tools place copper closer to other nets' than the
/// clearance (the design rule check still reports it).
pub(crate) fn allow_row(app: &mut SolveApp, ui: &mut Ui) {
    let allow = app.electronics.board.route.allow;
    if ui_kit::checkbox_row(ui, "board:route:allow", &tr("electronics.field_allow_violations"), allow) {
        app.electronics.board.route.allow = !allow;
    }
}

/// The miter's radius and style (the Route and Miter tools').
pub(crate) fn miter_options(app: &mut SolveApp, ui: &mut Ui) {
    let radius = app.electronics.board.route.miter;
    if let Some(v) = crate::electronics_editor::number(app, ui, "board:route:miter", &tr("electronics.field_miter"), radius) {
        if v.is_finite() && (0.0..=MAX_MITER).contains(&v) {
            app.electronics.board.route.miter = v;
        } else {
            app.set_status(tr("electronics.board_bad_miter"), true);
        }
    }
    let style = &mut app.electronics.board.route.miter_style;
    ui_kit::wrapped_row(ui, crate::i18n::tr("electronics.field_miter_style"), |ui| {
        for (s, key, name) in
            [(MiterStyle::Round, "round", "electronics.miter_round"), (MiterStyle::Straight, "straight", "electronics.miter_straight")]
        {
            if ui_kit::choice_chip(ui, &format!("board:miter:{key}"), crate::i18n::tr(name), *style == s).clicked() {
                *style = s;
            }
        }
    });
}

/// The via's span (Through, or Blind/Buried between two layers), Micro, and its diameter and drill
/// (the Route and Via tools').
pub(crate) fn via_options(app: &mut SolveApp, ui: &mut Ui) {
    let span = app.electronics.board.route.span;
    app.electronics.board.route.span = span_rows(app, ui, "board:via", span);
    let micro = app.electronics.board.route.micro;
    if ui_kit::checkbox_row(ui, "board:via:micro", &tr("electronics.field_micro_via"), micro) {
        app.electronics.board.route.micro = !micro;
        // A micro via joins two neighbouring layers: the Via tool's span becomes the active layer
        // and the next one when it isn't such a pair.
        if !micro && !matches!(app.electronics.board.route.span, ViaSpan::Between(a, b) if neighbours(app, a, b)) {
            let n = board(app).copper_layers;
            let a = active_layer(app);
            let below = CopperLayer::at(a.index(n) + 1, n).filter(|b| neighbours(app, a, *b));
            let above = a.index(n).checked_sub(1).and_then(|i| CopperLayer::at(i, n)).filter(|b| neighbours(app, *b, a));
            if let Some((x, y)) = below.map(|b| (a, b)).or_else(|| above.map(|b| (b, a))) {
                app.electronics.board.route.span = ViaSpan::Between(x, y);
                app.set_status(span_text(ViaSpan::Between(x, y)), false);
            }
        }
    }
    // The size shown is the one a click places: the micro via's while Micro is on.
    let micro = app.electronics.board.route.micro && matches!(app.electronics.board.route.span, ViaSpan::Between(a, b) if neighbours(app, a, b));
    let size = if micro { micro_size(app) } else { via_size(app) };
    if let Some([diameter, drill]) = crate::electronics_editor::pair(app, ui, "board:route:via", crate::i18n::tr("electronics.field_via"), size) {
        if check_via(diameter, drill).is_ok() {
            let o = &mut app.electronics.board.route;
            if micro { o.micro_via = Some([diameter, drill]) } else { o.via = Some([diameter, drill]) }
        } else {
            app.set_status(tr("electronics.board_bad_via"), true);
        }
    }
    // A size typed holds until this gives it back to the net class (the rules' micro via).
    let typed = if micro { app.electronics.board.route.micro_via.is_some() } else { app.electronics.board.route.via.is_some() };
    if typed && crate::electronics_editor::button(ui, "board-via-class", &tr("electronics.board_via_net_class"), true) {
        let o = &mut app.electronics.board.route;
        if micro { o.micro_via = None } else { o.via = None }
    }
}

/// How the status line words a via span: "Via span: Inner 2 to Inner 3", or through.
pub(crate) fn span_text(span: ViaSpan) -> String {
    match span {
        ViaSpan::Through => tr("electronics.board_via_span_through"),
        ViaSpan::Between(a, b) => crate::i18n::format("electronics.board_via_span_now", &[("from", &layer_name(a)), ("to", &layer_name(b))]),
    }
}

/// A via span's rows (`key` keeps them apart): Through or Blind/Buried, and for the latter its
/// From and To layers. Picking a From at the To (or a To at the From) moves the other one layer
/// on, the way it can go, so no pick is refused; the status line says the span it now is.
pub(crate) fn span_rows(app: &mut SolveApp, ui: &mut Ui, key: &str, span: ViaSpan) -> ViaSpan {
    let stack = board(app).stack();
    let n = board(app).copper_layers;
    let (top, bottom) = (stack.first().copied().unwrap_or(CopperLayer::Top), stack.get(1).copied().unwrap_or(CopperLayer::Bottom));
    let mut next = span;
    let names = [tr("electronics.via_through"), tr("electronics.via_blind_buried")];
    let at = usize::from(matches!(span, ViaSpan::Between(..)));
    match crate::electronics_editor::choice(ui, &format!("electronics_{key}_span"), &tr("electronics.field_via_span"), at, &names) {
        Some(0) => next = ViaSpan::Through,
        Some(_) => next = ViaSpan::Between(top, bottom),
        None => {}
    }
    let ViaSpan::Between(a, b) = next else { return next };
    // A layer the stack lost is the nearest end of it.
    let fit = |l: CopperLayer| if board(app).has_layer(l) { l } else { CopperLayer::Bottom };
    let (mut a, mut b) = (fit(a), fit(b));
    let mut moved = None;
    if let Some(l) = span_row(app, ui, &format!("{key}:from"), &tr("electronics.field_via_from"), a) {
        a = l;
        moved = Some(true);
    }
    if let Some(l) = span_row(app, ui, &format!("{key}:to"), &tr("electronics.field_via_to"), b) {
        b = l;
        moved = Some(false);
    }
    if a == b {
        // The other end moves on: down from a From, up from a To, the other way at the stack's end.
        let i = a.index(n);
        let down = CopperLayer::at(i + 1, n);
        let up = i.checked_sub(1).and_then(|k| CopperLayer::at(k, n));
        let other = if moved == Some(false) { up.or(down) } else { down.or(up) };
        match other {
            Some(o) if moved == Some(false) => a = o,
            Some(o) => b = o,
            None => return ViaSpan::Through,
        }
    }
    let out = if a.index(n) < b.index(n) { ViaSpan::Between(a, b) } else { ViaSpan::Between(b, a) };
    let out = match out {
        ViaSpan::Between(x, y) if x == CopperLayer::Top && y == CopperLayer::Bottom => ViaSpan::Through,
        o => o,
    };
    if moved.is_some() {
        app.set_status(span_text(out), false);
    }
    out
}

/// The Unroute tool's options: Unroute All.
pub(crate) fn unroute_options(app: &mut SolveApp, ui: &mut Ui) {
    let b = board(app);
    let any = !(b.tracks.is_empty() && b.vias.is_empty());
    if crate::electronics_editor::button(ui, "board-unroute-all", &tr("electronics.board_unroute_all"), any) {
        drop(app.run("ecad.board.unroute", json!({"all": true})));
    }
}
