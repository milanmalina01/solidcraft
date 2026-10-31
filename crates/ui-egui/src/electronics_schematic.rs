//! The Electronics workspace's schematic: the sheet of placed parts, wires, net labels, power
//! ports and markers (`doc.ecad.schematic`, the `ecad.schematic.*` commands), on a flat page with
//! its own camera. The toolbar's tools place parts, draw wires and put labels, power ports,
//! no-connect markers and power flags on the 0.1 in grid; a press picks, a drag moves the picked
//! objects, R turns and X mirrors them, Delete removes them. Every change is one command, so undo
//! takes back one edit. Labels and power ports point away from the pin they sit on, their names
//! running that way; the Label and Power tools take a name from a label clicked on the sheet.
//! Picking a wire or label shows its net with its net class, which can be changed there, and its
//! role in the design's intent (Design Intent opens on it); wires of a net whose class carries a
//! current are drawn heavier. The Frame tool draws section frames round groups of parts (named
//! rectangles a print splits the sheet into pages by, `ecad.schematic.frame`): a frame is picked
//! by its edges or its title tab, moved by the tab, resized by an edge or a corner, and renamed or
//! put in print order in the Inspector.
//!
//! A drag from empty sheet picks with a box: left to right, what lies wholly inside it; right to
//! left (dashed), what it touches. Moves keep connections (`route::moved`): wire ends on what
//! moves come along, and an edit that still makes or breaks a connection says so in the status
//! line. What a press would take is highlighted under the pointer.

use std::collections::{BTreeMap, BTreeSet};

use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2, pos2, vec2};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::connect::{NetChange, net_changes};
use solidcraft_engine::doc::ecad::erc::Report;
use solidcraft_engine::doc::ecad::frame::{MAX_FRAMES, in_order, normalized, overlaps, part_frames};
use solidcraft_engine::doc::ecad::rules::DEFAULT_CLASS;
use solidcraft_engine::doc::ecad::schematic::{Anchor, Label, LabelKind, Library, MarkerKind, Part, Pins, Schematic, part_bbox};
use solidcraft_engine::doc::ecad::{Frame, GraphicType, Placeable, SymbolItem};

use crate::SolveApp;
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;
use crate::ui_kit;

/// The schematic grid, 0.1 in: pins, wire bends and parts land on it.
pub const GRID: f64 = 2.54;
/// A press this close to a pin's end, a wire or a label, in points, picks it.
const PICK_POINTS: f32 = 6.0;
/// The pointer travels this far, in points, before a press on an object moves it.
const DRAG_POINTS: f32 = 3.0;
/// Half the empty sheet's width when Fit has nothing to frame, in millimetres.
const EMPTY_HALF: f64 = 20.0;

/// What a toolbar tool places on the sheet.
#[derive(Clone, Debug, PartialEq)]
pub enum Tool {
    /// A part of `component`, turned and mirrored before it is clicked down.
    Place {
        component: u64,
        rotation: u16,
        mirror: bool,
    },
    /// A wire through the clicked points.
    Wire {
        points: Vec<End>,
    },
    /// A net label with the panel's name, turned `turn` degrees (R, X), else away from the pin
    /// it goes on.
    Label {
        turn: Option<u16>,
    },
    /// A power port with the panel's name, turned `turn` degrees (R, X), else the usual way for
    /// where it goes (`PortRoom::turn`).
    Power {
        turn: Option<u16>,
    },
    NoConnect,
    /// A power flag on a pin or wire point: its net is powered from off the sheet.
    PowerFlag,
    /// A section frame with the panel's title: dragged from corner to corner, or clicked at one
    /// corner (`from`) and then at the other. `press` is where a drag began, while it lasts.
    Frame {
        from: Option<[f64; 2]>,
        press: Option<Pos2>,
    },
}

impl Tool {
    /// The toolbar command it stands for.
    pub(crate) fn command(&self) -> &'static str {
        match self {
            Tool::Place { .. } => "ecad.schematic.place",
            Tool::Wire { .. } => "ecad.schematic.wire",
            Tool::Label { .. } => "ecad.schematic.label",
            Tool::Power { .. } => "ecad.schematic.power",
            Tool::NoConnect => "ecad.schematic.marker",
            Tool::PowerFlag => "ecad.schematic.marker.power_flag",
            Tool::Frame { .. } => "ecad.schematic.frame",
        }
    }
}

/// A clicked wire point: on a part's pin, or on the grid.
#[derive(Clone, Debug, PartialEq)]
pub enum End {
    Pin { part: u64, pin: String, at: [f64; 2] },
    Point([f64; 2]),
}

impl End {
    fn at(&self) -> [f64; 2] {
        match self {
            End::Pin { at, .. } | End::Point(at) => *at,
        }
    }

    fn param(&self) -> Value {
        match self {
            End::Pin { part, pin, .. } => json!({"part": part, "pin": pin}),
            End::Point(p) => json!(p),
        }
    }
}

/// How a wire bends between two clicked points; right-click while drawing cycles them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bend {
    /// Along x first, then y.
    #[default]
    HorizontalFirst,
    /// Along y first, then x.
    VerticalFirst,
    /// Straight along x or y, then 45 degrees.
    Diagonal,
    /// Straight, at any angle.
    Straight,
}

pub(crate) const BENDS: [(Bend, &str); 4] = [
    (Bend::HorizontalFirst, "electronics.bend_horizontal"),
    (Bend::VerticalFirst, "electronics.bend_vertical"),
    (Bend::Diagonal, "electronics.bend_diagonal"),
    (Bend::Straight, "electronics.bend_straight"),
];

impl Bend {
    pub(crate) fn next(self) -> Bend {
        let i = BENDS.iter().position(|(b, _)| *b == self).unwrap_or(0);
        BENDS[(i + 1) % BENDS.len()].0
    }

    /// The bends of a leg from `a` to `b`.
    pub(crate) fn corners(self, a: [f64; 2], b: [f64; 2]) -> Vec<[f64; 2]> {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let corner = match self {
            Bend::HorizontalFirst => Some([b[0], a[1]]),
            Bend::VerticalFirst => Some([a[0], b[1]]),
            // The 45 degree part takes the shorter of the two runs.
            Bend::Diagonal if dx.abs() >= dy.abs() => Some([b[0] - dx.signum() * dy.abs(), a[1]]),
            Bend::Diagonal => Some([a[0], b[1] - dy.signum() * dx.abs()]),
            Bend::Straight => None,
        };
        corner.into_iter().filter(|c| *c != a && *c != b).collect()
    }
}

/// A press on picked objects, which moves them once the pointer has travelled; or on a wire's
/// segment or bend, which moves just that.
struct Drag {
    press: Pos2,
    from: [f64; 2],
    by: [f64; 2],
    moving: bool,
    reshape: Option<Reshape>,
    /// A part's designator or value being moved on its own, from where it was.
    text: Option<(u64, Which, [f64; 2])>,
    /// A frame's edge or corner being dragged.
    resize: Option<Resize>,
}

/// One of a placed part's two texts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Which {
    Designator,
    Value,
}

impl Which {
    fn placeholder(self) -> &'static str {
        match self {
            Which::Designator => "{designator}",
            Which::Value => "{value}",
        }
    }
}

/// Where a part's designator or value starts (its lower left) on the sheet, and how tall it is:
/// its own spot, else the symbol's `{designator}` / `{value}` text, else beside the symbol's box.
fn text_anchor(part: &Part, c: &Placeable, placement: &solidcraft_engine::doc::ecad::Placement, which: Which) -> Option<([f64; 2], f64)> {
    let placeholder = c.symbol.graphics.iter().find(|g| g.kind == GraphicType::Text && g.text == which.placeholder());
    let height = placeholder.and_then(|g| g.height).unwrap_or(1.27);
    let own = match which {
        Which::Designator => part.designator_at,
        Which::Value => part.value_at,
    };
    if let Some(o) = own {
        return Some(([placement.at[0] + o[0], placement.at[1] + o[1]], height));
    }
    let [lo, hi] = c.symbol.bbox().map(|b| placement.bbox(b))?;
    // A part turned on its side keeps its upright texts beside it, one over the other.
    if !placement.rotation.is_multiple_of(180) {
        let middle = (lo[1] + hi[1]) / 2.0;
        let y = if which == Which::Designator { middle + 0.3 } else { middle - 0.3 - height };
        return Some(([hi[0] + 0.6, y], height));
    }
    // The placeholder's anchor turns with the part; the text itself stays upright.
    if let Some(at) = placeholder.and_then(|g| g.at).map(|a| placement.apply(a)) {
        return Some((at, height));
    }
    Some(([hi[0] + 0.6, hi[1] + if which == Which::Designator { 0.4 } else { -1.4 }], height))
}

/// The text a part shows for `which`.
fn part_text(part: &Part, c: &Placeable, which: Which) -> String {
    match which {
        Which::Designator => part.designator.clone(),
        Which::Value => part.value.clone().unwrap_or_else(|| c.component.value.clone()),
    }
}

/// The screen box of a part's text from its lower left `at`.
fn text_rect(map: PageMap, at: [f64; 2], height: f64, text: &str) -> Rect {
    let h = ((height * map.scale()) as f32).clamp(6.0, 48.0);
    let p = map.to_screen(at[0], at[1]);
    Rect::from_min_size(p - vec2(0.0, h), vec2(h * 0.6 * text.chars().count().max(1) as f32, h))
}

/// Whether the sheet shows pin names and numbers at this zoom.
fn pin_texts_show(map: PageMap) -> bool {
    map.scale() >= 3.0
}

/// Each placed pin's box on screen: its line, and its number over it (left of an upright one)
/// while numbers show (`labels`).
fn pin_boxes(map: PageMap, pins: &[solidcraft_engine::doc::ecad::Pin], labels: bool) -> Vec<Rect> {
    use solidcraft_engine::doc::ecad::PinDirection;
    let scale = map.scale();
    let mm = scale as f32;
    let band = labels
        .then(|| crate::electronics_components::number_size(pins, scale))
        .flatten()
        .map_or(0.3 * mm, |n| n + crate::electronics_components::number_lift(scale));
    let pin_box = |p: &solidcraft_engine::doc::ecad::Pin| {
        let r = Rect::from_two_pos(map.to_screen(p.at[0], p.at[1]), map.to_screen(p.body_end()[0], p.body_end()[1]));
        if matches!(p.direction, PinDirection::Left | PinDirection::Right) {
            Rect::from_min_max(pos2(r.min.x - 0.2 * mm, r.min.y - band), pos2(r.max.x + 0.2 * mm, r.max.y + 0.3 * mm))
        } else {
            Rect::from_min_max(pos2(r.min.x - band, r.min.y - 0.2 * mm), pos2(r.max.x + 0.3 * mm, r.max.y + 0.2 * mm))
        }
    };
    pins.iter().map(pin_box).collect()
}

/// Whether two screen boxes overlap by more than touching.
fn overlap(a: Rect, b: Rect) -> bool {
    a.min.x < b.max.x && b.min.x < a.max.x && a.min.y < b.max.y && b.min.y < a.max.y
}

/// Where a part's text (its lower left `at`, `height` millimetres high) is drawn at this zoom:
/// slid left or right off the placed `pins` it would sit on (a symbol's text over its bottom
/// pins), whichever moves it less, as the export pages set it. A pin's number counts as part of it
/// while numbers show (`labels`).
fn clear_of_pins(map: PageMap, pins: &[solidcraft_engine::doc::ecad::Pin], at: [f64; 2], height: f64, text: &str, labels: bool) -> [f64; 2] {
    let scale = map.scale();
    let mm = scale as f32;
    let boxes = pin_boxes(map, pins, labels);
    let start = text_rect(map, at, height, text);
    let mut b = start;
    for _ in 0..4 {
        let (mut left, mut right) = (f32::INFINITY, f32::NEG_INFINITY);
        for q in boxes.iter().filter(|q| q.min.x < b.max.x && b.min.x < q.max.x && q.min.y < b.max.y && b.min.y < q.max.y) {
            left = left.min(q.min.x);
            right = right.max(q.max.x);
        }
        if left > right {
            break;
        }
        let (to_left, to_right) = (left - 0.6 * mm - start.width(), right + 0.6 * mm);
        let x = if (start.min.x - to_left).abs() < (to_right - start.min.x).abs() { to_left } else { to_right };
        b = start.translate(vec2(x - start.min.x, 0.0));
    }
    let by = f64::from(b.min.x - start.min.x) / scale;
    if by.is_finite() { [at[0] + by, at[1]] } else { at }
}

/// The side of a [`Clutter`] cell, in points.
const CLUTTER_CELL: f32 = 32.0;
/// The most cells one box of [`Clutter`] is filed under; a larger one is tried on every lookup.
const CLUTTER_CELLS: i64 = 256;
/// The most wire segments, labels and markers [`Clutter`] files; past it, part texts keep clear of
/// their pins only.
const MAX_CLUTTER: usize = 20_000;

/// Boxes on screen filed by the cells they cover (see [`Clutter`]).
#[derive(Default)]
struct Filed {
    rects: Vec<Rect>,
    cells: BTreeMap<(i32, i32), Vec<usize>>,
    wide: Vec<usize>,
}

/// What part texts keep clear of on screen besides their own pins: the wires, labels, power ports
/// and markers in view, filed by the cells they cover; and apart, the parts' bodies, which a text
/// moved off those keeps clear of too.
struct Clutter {
    area: Rect,
    things: Filed,
    bodies: Filed,
}

impl Clutter {
    /// What sheet `s` shows in `view`.
    fn new(s: &Schematic, pins: &Pins, library: &Library, map: PageMap, view: Rect) -> Self {
        let mut c = Clutter { area: view.expand(64.0), things: Filed::default(), bodies: Filed::default() };
        let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
        let mut left = MAX_CLUTTER;
        'wires: for w in &s.wires {
            let Some(points) = wire_points(pins, w) else { continue };
            for ab in points.windows(2) {
                let [a, b] = [ab[0], ab[1]];
                if left == 0 {
                    break 'wires;
                }
                left -= 1;
                c.add(Rect::from_two_pos(screen(a), screen(b)).expand(1.5));
            }
        }
        for l in s.labels.iter().take(left) {
            if let Some(a) = pins.anchor(&l.at) {
                c.add(label_rect(map, l, a));
            }
        }
        let left = left.saturating_sub(s.labels.len());
        for m in s.markers.iter().take(left) {
            if let Some(a) = pins.anchor(&m.at) {
                c.add(Rect::from_center_size(screen(a), vec2(10.0, 10.0)));
            }
        }
        for p in s.parts.iter().take(MAX_CLUTTER) {
            let Some(body) = library.get(&p.component).and_then(|c| solidcraft_engine::doc::ecad::route::body_box(&p.placement, c.symbol)) else {
                continue;
            };
            let r = Rect::from_two_pos(screen(body[0]), screen(body[1]));
            let i = c.bodies.rects.len();
            if Self::file(c.area, &mut c.bodies, i, r) {
                c.bodies.rects.push(r);
            }
        }
        c
    }

    /// The cells the part of `r` in `area` covers, when they are few enough; `Err` when there
    /// are more, `Ok(None)` when it is out of view.
    #[allow(clippy::type_complexity)]
    fn cells(area: Rect, r: Rect) -> Result<Option<(std::ops::RangeInclusive<i32>, std::ops::RangeInclusive<i32>)>, ()> {
        let r = r.intersect(area);
        if !(r.min.x.is_finite() && r.min.y.is_finite() && r.max.x.is_finite() && r.max.y.is_finite()) || r.min.x > r.max.x || r.min.y > r.max.y {
            return Ok(None);
        }
        // In view, so the casts stay small.
        let c = |v: f32| (v / CLUTTER_CELL).floor() as i32;
        let (x0, x1, y0, y1) = (c(r.min.x), c(r.max.x), c(r.min.y), c(r.max.y));
        let n = (i64::from(x1) - i64::from(x0) + 1).saturating_mul(i64::from(y1) - i64::from(y0) + 1);
        if n > CLUTTER_CELLS { Err(()) } else { Ok(Some((x0..=x1, y0..=y1))) }
    }

    /// File `r` as box `i` of `into`; `false` when it is out of view (not filed).
    fn file(area: Rect, into: &mut Filed, i: usize, r: Rect) -> bool {
        match Self::cells(area, r) {
            Ok(None) => false,
            Ok(Some((xs, ys))) => {
                for x in xs {
                    for y in ys.clone() {
                        into.cells.entry((x, y)).or_default().push(i);
                    }
                }
                true
            }
            Err(()) => {
                into.wide.push(i);
                true
            }
        }
    }

    fn add(&mut self, r: Rect) {
        let i = self.things.rects.len();
        if Self::file(self.area, &mut self.things, i, r) {
            self.things.rects.push(r);
        }
    }

    /// Whether `r` overlaps a box of `filed`.
    fn hits(area: Rect, filed: &Filed, r: Rect) -> bool {
        let hits = |i: &usize| filed.rects.get(*i).is_some_and(|o| overlap(*o, r));
        if filed.wide.iter().any(hits) {
            return true;
        }
        match Self::cells(area, r) {
            Ok(None) => false,
            Ok(Some((xs, ys))) => xs.flat_map(|x| ys.clone().map(move |y| (x, y))).filter_map(|k| filed.cells.get(&k)).flatten().any(hits),
            Err(()) => filed.rects.iter().any(|o| overlap(*o, r)),
        }
    }

    /// Whether `r` overlaps a wire, label, port or marker filed.
    fn meets(&self, r: Rect) -> bool {
        Self::hits(self.area, &self.things, r)
    }

    /// Whether `r` overlaps a part's body.
    fn on_body(&self, r: Rect) -> bool {
        Self::hits(self.area, &self.bodies, r)
    }
}

/// Where a part's text is drawn: where the user put it (`own`), else slid off the part's pins
/// ([`clear_of_pins`]); and where that still sits on a wire, label, power port or marker
/// (`clutter`), at the nearest spot close by that is clear of them all, of the pins and of the
/// parts' bodies.
#[allow(clippy::too_many_arguments)]
fn text_spot(
    map: PageMap,
    pins: &[solidcraft_engine::doc::ecad::Pin],
    at: [f64; 2],
    height: f64,
    text: &str,
    labels: bool,
    own: bool,
    clutter: Option<&Clutter>,
) -> [f64; 2] {
    if own {
        return at;
    }
    let slid = clear_of_pins(map, pins, at, height, text, labels);
    let Some(clutter) = clutter else { return slid };
    if !clutter.meets(text_rect(map, slid, height, text)) {
        return slid;
    }
    let boxes = pin_boxes(map, pins, labels);
    // Moved, a text keeps clear of the parts' bodies too, its own among them.
    let free = |r: Rect| !clutter.meets(r) && !clutter.on_body(r) && !boxes.iter().any(|q| overlap(*q, r));
    let start = text_rect(map, at, height, text);
    let step = (start.height() * 0.6).max(3.0);
    // Sideways first: a step up or down costs one and a half steps along.
    let mut spots: Vec<(i32, i32)> = (-8..=8).flat_map(|kx| (-4..=4).map(move |ky| (kx, ky))).filter(|k| *k != (0, 0)).collect();
    spots.sort_by_key(|(kx, ky)| 2 * kx.abs() + 3 * ky.abs());
    let scale = map.scale();
    spots
        .into_iter()
        .map(|(kx, ky)| vec2(kx as f32 * step, ky as f32 * step))
        .find(|off| free(start.translate(*off)))
        .map(|off| [at[0] + f64::from(off.x) / scale, at[1] - f64::from(off.y) / scale])
        .filter(|p| p.iter().all(|v| v.is_finite()))
        .unwrap_or(slid)
}

/// A part's designator or value as the sheet draws it: its text, where it starts (lower left)
/// and how tall it is; `None` for an empty one.
fn shown_text(
    map: PageMap,
    part: &Part,
    c: &Placeable,
    placement: &solidcraft_engine::doc::ecad::Placement,
    pins: &[solidcraft_engine::doc::ecad::Pin],
    which: Which,
    labels: bool,
    clutter: Option<&Clutter>,
) -> Option<(String, [f64; 2], f64)> {
    let text = part_text(part, c, which);
    if text.is_empty() {
        return None;
    }
    let (at, height) = text_anchor(part, c, placement, which)?;
    let own = match which {
        Which::Designator => part.designator_at.is_some(),
        Which::Value => part.value_at.is_some(),
    };
    // Zoomed out, where pin texts don't show and every text is at its least size, texts stay put
    // rather than jump about with the zoom.
    let at = text_spot(map, pins, at, height, &text, labels, own, clutter.filter(|_| labels));
    Some((text, at, height))
}

/// The part text a press at `p` takes, and where it is drawn: the one whose letters are under the
/// pointer, the one drawn on top first (later parts, and a value over its designator); with
/// `near`, else the one whose box is within reach, nearest first.
fn text_at(app: &SolveApp, map: PageMap, p: Pos2, clutter: Option<&Clutter>, near: bool) -> Option<(u64, Which, [f64; 2])> {
    let library = app.session.doc.ecad.library();
    let labels = pin_texts_show(map);
    let mut best: Option<(f32, (u64, Which, [f64; 2]))> = None;
    for part in sheet(app).parts.iter().rev() {
        let Some(c) = library.get(&part.component) else { continue };
        // Far from the part and from where its texts start, the pointer is on neither of them: a
        // text is slid off the pins by at most the part's width and its own, and moved off what
        // lies there by a few of its heights.
        let starts: Vec<Rect> = [Which::Designator, Which::Value]
            .into_iter()
            .filter_map(|which| text_anchor(part, c, &part.placement, which).map(|(at, h)| text_rect(map, at, h, &part_text(part, c, which))))
            .collect();
        let margin = starts.iter().map(|r| r.width() + 12.0 * r.height()).fold(4.0, f32::max);
        let body = part_bbox(part, c.symbol).map(|[a, b]| Rect::from_two_pos(map.to_screen(a[0], a[1]), map.to_screen(b[0], b[1])));
        if !starts.iter().copied().chain(body).any(|r| r.expand(margin).contains(p)) {
            continue;
        }
        let pins: Vec<solidcraft_engine::doc::ecad::Pin> = c.symbol.pins.iter().map(|q| part.placement.pin(q)).collect();
        for which in [Which::Value, Which::Designator] {
            let Some((text, at, height)) = shown_text(map, part, c, &part.placement, &pins, which, labels, clutter) else { continue };
            let r = text_rect(map, at, height, &text);
            if r.contains(p) {
                return Some((part.id, which, at));
            }
            let d = r.distance_to_pos(p);
            if near && d <= 2.0 && best.as_ref().is_none_or(|b| d < b.0) {
                best = Some((d, (part.id, which, at)));
            }
        }
    }
    best.map(|b| b.1)
}

/// What a press on the sheet takes: a part's designator or value (where it is drawn), or a sheet
/// object.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    Text(u64, Which, [f64; 2]),
    Item(u64),
}

/// What a press at `p` takes (with Shift held, `shift`): a part text whose letters are under the
/// pointer; else a frame tab, label, marker, wire or frame edge; else a part text within reach;
/// else a part. The sheet highlights the same under the pointer.
fn press_target(app: &SolveApp, map: PageMap, p: Pos2, clutter: Option<&Clutter>, shift: bool) -> Option<Target> {
    let texts = !shift && tab_at(sheet(app), map, p).is_none();
    if texts && let Some((part, which, at)) = text_at(app, map, p, clutter, false) {
        return Some(Target::Text(part, which, at));
    }
    let id = hit(app, map, p);
    if texts
        && id.is_none_or(|id| sheet(app).part(id).is_some())
        && let Some((part, which, at)) = text_at(app, map, p, clutter, true)
    {
        return Some(Target::Text(part, which, at));
    }
    id.map(Target::Item)
}

/// What part texts keep clear of on the sheet as it is now, in `view`.
fn clutter_now(app: &SolveApp, map: PageMap, view: Rect) -> Clutter {
    let library = app.session.doc.ecad.library();
    let s = sheet(app);
    Clutter::new(s, &Pins::new(s, &library), &library, map, view)
}

/// A drag of one wire's segment (its ends stay; neighbouring legs stretch) or one bend.
#[derive(Clone, Debug)]
struct Reshape {
    wire: u64,
    /// The wire's points as they were, ends included.
    points: Vec<[f64; 2]>,
    /// A bend's index in `points`, or the segment from `points[i]` to `points[i + 1]`.
    bend: Option<usize>,
    segment: usize,
}

impl Reshape {
    /// The wire's points moved by `by`.
    fn moved(&self, by: [f64; 2]) -> Vec<[f64; 2]> {
        let add = |p: [f64; 2], d: [f64; 2]| [p[0] + d[0], p[1] + d[1]];
        let last = self.points.len().saturating_sub(1);
        if let Some(k) = self.bend {
            return self.points.iter().enumerate().map(|(i, p)| if i == k { add(*p, by) } else { *p }).collect();
        }
        let k = self.segment;
        let (Some(a), Some(b)) = (self.points.get(k), self.points.get(k + 1)) else { return self.points.clone() };
        // A level or upright segment slides across itself, so its neighbours stay square.
        let d = if a[1] == b[1] {
            [0.0, by[1]]
        } else if a[0] == b[0] {
            [by[0], 0.0]
        } else {
            by
        };
        let mut out = Vec::with_capacity(self.points.len() + 2);
        for (i, p) in self.points.iter().enumerate() {
            match i {
                // The wire's ends stay on their pins or points: a short jog joins them.
                0 if k == 0 => out.extend([*p, add(*p, d)]),
                i if i == last && i == k + 1 => out.extend([add(*p, d), *p]),
                i if i == k || i == k + 1 => out.push(add(*p, d)),
                _ => out.push(*p),
            }
        }
        solidcraft_engine::doc::ecad::route::simplify(out)
    }
}

/// The least a frame's sides are, drawn or resized, in millimetres: one grid step.
const FRAME_LEAST: f64 = GRID;

/// A drag of one edge of a frame, or of two at a corner: `edges` are its left, bottom, right and
/// top.
#[derive(Clone, Debug)]
struct Resize {
    frame: u64,
    /// The frame's corners as they were.
    rect: [[f64; 2]; 2],
    edges: [bool; 4],
}

impl Resize {
    /// The frame's corners with the dragged edges moved by `by`, its sides kept at least
    /// [`FRAME_LEAST`] long.
    fn moved(&self, by: [f64; 2]) -> [[f64; 2]; 2] {
        let [[mut x0, mut y0], [mut x1, mut y1]] = self.rect;
        let [left, bottom, right, top] = self.edges;
        if left {
            x0 = (x0 + by[0]).min(x1 - FRAME_LEAST);
        }
        if right {
            x1 = (x1 + by[0]).max(x0 + FRAME_LEAST);
        }
        if bottom {
            y0 = (y0 + by[1]).min(y1 - FRAME_LEAST);
        }
        if top {
            y1 = (y1 + by[1]).max(y0 + FRAME_LEAST);
        }
        [[x0, y0], [x1, y1]]
    }

    /// The pointer over these edges.
    fn cursor(&self) -> egui::CursorIcon {
        match self.edges {
            [true, _, _, true] | [_, true, true, _] => egui::CursorIcon::ResizeNwSe,
            [true, true, _, _] | [_, _, true, true] => egui::CursorIcon::ResizeNeSw,
            [true, ..] | [_, _, true, _] => egui::CursorIcon::ResizeHorizontal,
            _ => egui::CursorIcon::ResizeVertical,
        }
    }
}

pub struct Sheet {
    pub tool: Option<Tool>,
    /// The picked sheet objects, by id.
    pub selection: BTreeSet<u64>,
    drag: Option<Drag>,
    /// The name the Label and Power tools put down.
    pub label: String,
    pub power: String,
    /// The title the Frame tool gives its next frame; empty: `Section 1`, `Section 2`… the first
    /// free one.
    pub frame_title: String,
    /// The Label tool counts its name on after each label it puts down (`D0`, `D1`…).
    pub count_on: bool,
    /// The designator the Place tool gives the next part (`C101`), counting on from there after
    /// each one; empty: one more than the highest of its prefix.
    pub designator: String,
    /// The Add Part window is open.
    pub choosing: bool,
    /// How the Wire tool bends.
    pub bend: Bend,
    pub fitted: bool,
    /// A box to frame on the next frame (a finding of the electrical check), in millimetres.
    pub(crate) frame: Option<[[f64; 2]; 2]>,
    /// Where a drag from empty sheet began, while it draws a box to pick with.
    boxing: Option<Pos2>,
    /// The pointer over the sheet, for R and X with the Label and Power tools.
    pointer: Option<Pos2>,
    /// The Inspector's Move by for several picked objects, in millimetres.
    move_by: [f64; 2],
}

impl Default for Sheet {
    fn default() -> Self {
        Sheet {
            tool: None,
            selection: BTreeSet::new(),
            drag: None,
            label: "NET1".into(),
            power: "GND".into(),
            frame_title: String::new(),
            count_on: false,
            designator: String::new(),
            choosing: false,
            bend: Bend::default(),
            fitted: false,
            frame: None,
            boxing: None,
            pointer: None,
            move_by: [0.0; 2],
        }
    }
}

impl Sheet {
    /// Drop the drag in hand (or the box being drawn), if any, without doing it.
    pub(crate) fn drop_drag(&mut self) {
        self.drag = None;
        self.boxing = None;
    }
}

fn sheet(app: &SolveApp) -> &Schematic {
    &app.session.doc.ecad.schematic
}

/// Points on the sheet snap to the 0.1 in grid unless `free`.
fn snap(map: PageMap, p: Pos2, free: bool) -> [f64; 2] {
    let (x, y) = map.to_page(p);
    let tidy = |v: f64| (v * 1e4).round() / 1e4 + 0.0;
    if free { [tidy(x), tidy(y)] } else { [tidy((x / GRID).round() * GRID), tidy((y / GRID).round() * GRID)] }
}

/// What the sheet holds as one box, for Fit.
fn bounds(app: &SolveApp) -> Option<[[f64; 2]; 2]> {
    let library = app.session.doc.ecad.library();
    let s = sheet(app);
    let pins = Pins::new(s, &library);
    let mut points: Vec<[f64; 2]> = Vec::new();
    for p in &s.parts {
        if let Some([a, b]) = library.get(&p.component).and_then(|c| part_bbox(p, c.symbol)) {
            points.extend([a, b]);
        }
    }
    for w in &s.wires {
        points.extend(pins.anchor(&w.from));
        points.extend(w.via.iter().copied());
        points.extend(pins.anchor(&w.to));
    }
    for l in &s.labels {
        points.extend(pins.anchor(&l.at));
    }
    for f in &s.frames {
        points.extend(f.rect);
    }
    solidcraft_engine::doc::ecad::bbox(points)
}

/// Frame the sheet's contents (or the empty sheet round its origin) in the free part of `rect`.
fn fit(app: &mut SolveApp, rect: Rect) {
    let b = bounds(app).unwrap_or([[-EMPTY_HALF; 2], [EMPTY_HALF; 2]]);
    frame(app, rect, b, 40.0);
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

/// The sheet, in the central area.
pub fn show(app: &mut SolveApp, ui: &mut Ui) {
    let rect = ui.max_rect();
    if !rect.is_positive() {
        return;
    }
    let response = ui.interact(rect, ui.id().with("electronics_sheet"), Sense::click_and_drag());
    app.viewport.rect = Some(rect);
    crate::viewport::refresh_layout(app, ui.ctx().pixels_per_point());
    if !app.electronics.page.fitted {
        fit(app, rect);
    }
    if let Some(b) = app.electronics.sheet.frame.take() {
        frame(app, rect, b, 20.0);
    }
    if !input(app, ui, rect, &response) {
        crate::electronics_workspace::with_page_camera(app, |app| {
            crate::navigation::handle(app, ui, rect, &response, crate::navigation::NavCaps { orbit: false, pick: false });
        });
    }
    let checked = crate::electronics_design::checked(app);
    paint(app, ui, rect, &checked);
    crate::scenario::publish_target("electronics:sheet", rect);
    crate::electronics_design::windows(app, ui.ctx());
}

/// A part's pins as they lie on the sheet.
fn placed_pins<'a>(p: &'a Part, symbol: &'a SymbolItem) -> impl Iterator<Item = solidcraft_engine::doc::ecad::Pin> + 'a {
    symbol.pins.iter().map(|q| p.placement.pin(q))
}

/// The pin end nearest `p` on screen, within reach.
fn pin_at(app: &SolveApp, map: PageMap, p: Pos2) -> Option<End> {
    let library = app.session.doc.ecad.library();
    let mut best: Option<(f32, End)> = None;
    for part in &sheet(app).parts {
        let Some(c) = library.get(&part.component) else { continue };
        for pin in placed_pins(part, c.symbol) {
            let d = map.to_screen(pin.at[0], pin.at[1]).distance(p);
            if d <= PICK_POINTS + 2.0 && best.as_ref().is_none_or(|b| d < b.0) {
                best = Some((d, End::Pin { part: part.id, pin: pin.number.clone(), at: pin.at }));
            }
        }
    }
    best.map(|b| b.1)
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

/// A wire's points on the sheet, its ends where their pins are.
fn wire_points(pins: &Pins, w: &solidcraft_engine::doc::ecad::schematic::Wire) -> Option<Vec<[f64; 2]>> {
    let mut out = vec![pins.anchor(&w.from)?];
    out.extend(w.via.iter().copied());
    out.push(pins.anchor(&w.to)?);
    Some(out)
}

/// The sheet object under `p`: a frame by its title tab, a label or marker, a wire, a frame by
/// its edges, then a part.
fn hit(app: &SolveApp, map: PageMap, p: Pos2) -> Option<u64> {
    let library = app.session.doc.ecad.library();
    let s = sheet(app);
    let pins = Pins::new(s, &library);
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    // A frame's title tab is drawn over everything, and picks first.
    if let Some(id) = tab_at(s, map, p) {
        return Some(id);
    }
    // Labels and ports on neighbouring pins can overlap at low zoom: the one whose mark or name
    // is under the pointer wins, then the one whose line from its point runs nearest.
    let mut best: Option<((u8, f32), u64)> = None;
    for l in s.labels.iter().rev() {
        let Some(a) = pins.anchor(&l.at) else { continue };
        let r = label_rect(map, l, a);
        if !r.expand(2.0).contains(p) {
            continue;
        }
        let key = (u8::from(!r.contains(p)), label_reach(map, l, a, r, p));
        if best.as_ref().is_none_or(|(k, _)| key.0.cmp(&k.0).then(key.1.total_cmp(&k.1)).is_lt()) {
            best = Some((key, l.id));
        }
    }
    if let Some((_, id)) = best {
        return Some(id);
    }
    let marker = s
        .markers
        .iter()
        .rev()
        .filter_map(|m| pins.anchor(&m.at).map(|a| (screen(a).distance(p), m.id)))
        .filter(|(d, _)| *d <= PICK_POINTS + 2.0)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, id)) = marker {
        return Some(id);
    }
    for w in s.wires.iter().rev() {
        let Some(points) = wire_points(&pins, w) else { continue };
        if points.windows(2).any(|ab| segment_distance(p, screen(ab[0]), screen(ab[1])) <= PICK_POINTS) {
            return Some(w.id);
        }
    }
    if let Some(f) = s.frames.iter().rev().find(|f| frame_edges(map, f, section(s, f.id), p).is_some()) {
        return Some(f.id);
    }
    s.parts
        .iter()
        .rev()
        .find(|part| {
            library
                .get(&part.component)
                .and_then(|c| part_bbox(part, c.symbol))
                .is_some_and(|[a, b]| Rect::from_two_pos(screen(a), screen(b)).expand(2.0).contains(p))
        })
        .map(|part| part.id)
}

/// The pointer on the sheet: the armed tool takes clicks; otherwise a press picks (Shift adds) and
/// a drag moves what is picked, and a drag from empty sheet draws a box to pick with. `true` while
/// a drag holds the pointer, so the page doesn't navigate.
fn input(app: &mut SolveApp, ui: &Ui, rect: Rect, response: &egui::Response) -> bool {
    let Some(map) = PageMap::new(&app.electronics.page.cam, rect) else { return false };
    let (pressed, down, pos, mods) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos(), i.modifiers));
    let free = mods.command || mods.ctrl;
    app.electronics.sheet.pointer = pos.filter(|p| rect.contains(*p));
    if app.electronics.sheet.tool.is_some() {
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        // The Frame tool takes presses and drags rather than clicks.
        if matches!(app.electronics.sheet.tool, Some(Tool::Frame { .. })) {
            return frame_input(app, ui, map, response, pos, free);
        }
        let enter = !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if let Some(p) = pos {
            if response.double_clicked() {
                tool_click(app, map, p, mods, true);
            } else if response.clicked() {
                tool_click(app, map, p, mods, false);
            } else if response.secondary_clicked() && !rotate_tool(app) {
                // With the Wire tool, a right-click changes how it bends.
                let s = &mut app.electronics.sheet;
                if matches!(s.tool, Some(Tool::Wire { .. })) {
                    s.bend = s.bend.next();
                }
            }
        }
        if enter {
            finish_wire(app, None);
        }
        return false;
    }
    // A box drawn from empty sheet picks what it holds once the button is let go; a click there
    // (no drag) clears the pick.
    if let Some(from) = app.electronics.sheet.boxing {
        if down {
            return true;
        }
        app.electronics.sheet.boxing = None;
        let to = pos.unwrap_or(from);
        if from.distance(to) > DRAG_POINTS {
            // Part texts are tested where the sheet draws them, off wires and ports.
            let clutter = clutter_now(app, map, rect);
            box_pick(app, map, Rect::from_two_pos(from, to), to.x < from.x, mods, &clutter);
        } else if !(mods.shift || mods.command) {
            app.electronics.sheet.selection.clear();
        }
        return true;
    }
    if pressed
        && response.hovered()
        && let Some(p) = pos
    {
        let clutter = clutter_now(app, map, rect);
        let (x, y) = map.to_page(p);
        let start = |reshape, text, resize| Drag { press: p, from: [x, y], by: [0.0; 2], moving: false, reshape, text, resize };
        match press_target(app, map, p, Some(&clutter), mods.shift) {
            Some(Target::Text(part, which, at)) => {
                // A press on a part's designator or value moves just that text.
                app.electronics.sheet.selection = BTreeSet::from([part]);
                app.electronics.sheet.drag = Some(start(None, Some((part, which, at)), None));
                return true;
            }
            // A frame's edge or a wire's segment is taken alone unless it is one of several
            // picked objects: then a press on it drags them all.
            Some(Target::Item(id))
                if !mods.shift
                    && !in_group(app, id)
                    && let Some(resize) = resize_at(app, map, id, p) =>
            {
                // A press on a frame's edge or corner takes that edge or corner.
                app.electronics.sheet.selection = BTreeSet::from([id]);
                app.electronics.sheet.drag = Some(start(None, None, Some(resize)));
                return true;
            }
            Some(Target::Item(id))
                if !mods.shift
                    && !in_group(app, id)
                    && let Some(reshape) = reshape_at(app, map, id, p) =>
            {
                // A press on a wire takes that wire's segment or bend.
                app.electronics.sheet.selection = BTreeSet::from([id]);
                app.electronics.sheet.drag = Some(start(Some(reshape), None, None));
                return true;
            }
            Some(Target::Item(id)) => {
                let sel = &mut app.electronics.sheet.selection;
                if mods.shift {
                    if !sel.remove(&id) {
                        sel.insert(id);
                    }
                } else if !sel.contains(&id) {
                    *sel = BTreeSet::from([id]);
                }
                app.electronics.sheet.drag = Some(start(None, None, None));
                return true;
            }
            // Panning or zooming keeps the pick; only a click on empty sheet clears it.
            None if navigates(app, ui, mods) => {}
            None => {
                app.electronics.sheet.boxing = Some(p);
                return true;
            }
        }
    }
    let Some(drag) = app.electronics.sheet.drag.as_mut() else {
        if response.hovered()
            && let Some(p) = pos
            && let Some(id) = hit(app, map, p)
        {
            // A frame's edge or corner resizes it; anything else moves.
            let icon = resize_at(app, map, id, p).map_or(egui::CursorIcon::Grab, |r| r.cursor());
            ui.ctx().set_cursor_icon(icon);
        }
        return false;
    };
    if down {
        if let Some(p) = pos {
            drag.moving |= p.distance(drag.press) > DRAG_POINTS;
            let (x, y) = map.to_page(p);
            let raw = [x - drag.from[0], y - drag.from[1]];
            if drag.moving && raw.iter().all(|v| v.is_finite()) {
                drag.by = if free { raw } else { raw.map(|v| (v / GRID).round() * GRID) };
                ui.ctx().set_cursor_icon(drag.resize.as_ref().map_or(egui::CursorIcon::Grabbing, Resize::cursor));
            }
        }
        return true;
    }
    if let Some(drag) = app.electronics.sheet.drag.take()
        && drag.moving
        && drag.by != [0.0; 2]
    {
        match (drag.resize, drag.reshape, drag.text) {
            (Some(r), _, _) => {
                // Resized: what it holds now.
                if let Ok(reply) = app.run("ecad.schematic.frame.edit", json!({"frame": r.frame, "rect": r.moved(drag.by)})) {
                    let title = reply.get("frame").and_then(|f| f.get("title")).and_then(Value::as_str).unwrap_or_default().to_string();
                    app.set_status(frame_holds(&reply, &title), false);
                }
            }
            (None, _, Some((part, which, at))) => {
                // Kept as an offset from the part's origin, so it follows the part.
                if let Some(origin) = sheet(app).part(part).map(|p| p.placement.at) {
                    let tidy = |v: f64| (v * 1e4).round() / 1e4 + 0.0;
                    let offset = [tidy(at[0] + drag.by[0] - origin[0]), tidy(at[1] + drag.by[1] - origin[1])];
                    let key = if which == Which::Designator { "designator_at" } else { "value_at" };
                    let mut row = json!({"part": part});
                    row[key] = json!(offset);
                    drop(app.run("ecad.schematic.edit", json!({"parts": [row]})));
                }
            }
            (None, Some(r), None) => {
                let points = r.moved(drag.by);
                let via: Vec<[f64; 2]> = points.get(1..points.len().saturating_sub(1)).map(<[_]>::to_vec).unwrap_or_default();
                keeping_nets(app, "ecad.schematic.wire.edit", json!({"wire": r.wire, "via": via}));
            }
            (None, None, None) => {
                let items: Vec<u64> = app.electronics.sheet.selection.iter().copied().collect();
                moved_by(app, &items, drag.by);
            }
        }
    }
    true
}

/// Move the sheet objects `items` by `by` (one step), and say so; or say which connections the
/// move changed anyway. `true` when it moved and kept every connection.
fn moved_by(app: &mut SolveApp, items: &[u64], by: [f64; 2]) -> bool {
    let what = sheet_name(sheet(app), items);
    let kept = matches!(keeping_nets(app, "ecad.schematic.move", json!({"items": items, "by": by})), Some((_, false)));
    if kept {
        app.set_status(crate::i18n::format("electronics.sheet_moved", &[("what", &what)]), false);
    }
    kept
}

/// Whether a left press on empty sheet navigates (Space held, the navigation bar's fixed mode, or
/// the preset's left-button pan or zoom with these keys) rather than drawing a box to pick with.
fn navigates(app: &SolveApp, ui: &Ui, mods: egui::Modifiers) -> bool {
    use crate::viewport::NavMode;
    let space = !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_down(egui::Key::Space));
    let flat = |m: Option<NavMode>| m.is_some_and(|m| m != NavMode::Orbit);
    space || flat(app.viewport.nav) || flat(crate::prefs::nav_mode(app, crate::prefs::NavButton::Left, mods))
}

/// Run `cmd`, a sheet edit meant to leave every connection as it is (a move, turn, mirror or
/// drag). `Some` with its reply when it ran, and whether it changed the nets anyway, in which case
/// the status line says how, as an error (Undo takes the edit back).
fn keeping_nets(app: &mut SolveApp, cmd: &str, params: Value) -> Option<(Value, bool)> {
    let before = crate::electronics_design::checked(app);
    let reply = app.run(cmd, params).ok()?;
    let after = crate::electronics_design::checked(app);
    let changes = net_changes(&before.nets, &after.nets);
    let text = net_change_text(app, &changes);
    let changed = text.is_some();
    if let Some(text) = text {
        app.set_status(text, true);
    }
    Some((reply, changed))
}

/// `pins` as one phrase: `A`, `A and B`, `A, B and C`, `A, B, C and 4 more` (`more` past those
/// listed).
fn pin_list(pins: &[String], more: usize) -> String {
    const SHOWN: usize = 3;
    let rest = pins.len().saturating_sub(SHOWN).saturating_add(more);
    let shown: Vec<&str> = pins.iter().take(SHOWN).map(String::as_str).collect();
    if rest > 0 {
        return crate::i18n::format("electronics.sheet_list_more", &[("list", &shown.join(", ")), ("n", &rest.to_string())]);
    }
    match shown.split_last() {
        Some((last, head)) if !head.is_empty() => crate::i18n::format("electronics.sheet_list_two", &[("a", &head.join(", ")), ("b", last)]),
        Some((last, _)) => (*last).to_string(),
        None => String::new(),
    }
}

/// What the status line says of `changes` (the nets a sheet edit changed): the first net's pins
/// lost and gained, how many more nets changed, and how to take it back; `None` when nothing
/// changed.
fn net_change_text(app: &SolveApp, changes: &[NetChange]) -> Option<String> {
    nets_changed_text(app, changes, "electronics.sheet_net")
}

/// [`net_change_text`] for an edit of the kind `prefix` names: its `_lost`, `_gained` and
/// `_changed` texts (`electronics.sheet_net` for a move, `electronics.symbol_net` for a symbol
/// change).
pub(crate) fn nets_changed_text(app: &SolveApp, changes: &[NetChange], prefix: &str) -> Option<String> {
    let first = changes.first()?;
    let net = first.net.as_str();
    // `more` counts the pins left out of both lists; it is told with whichever list shows.
    let key = |what: &str| format!("{prefix}_{what}");
    let change = match (first.lost.is_empty(), first.gained.is_empty()) {
        (false, true) => crate::i18n::format(&key("lost"), &[("pins", &pin_list(&first.lost, first.more)), ("net", net)]),
        (true, false) => crate::i18n::format(&key("gained"), &[("pins", &pin_list(&first.gained, first.more)), ("net", net)]),
        _ => crate::i18n::format(
            &key("changed"),
            &[("lost", &pin_list(&first.lost, first.more)), ("gained", &pin_list(&first.gained, 0)), ("net", net)],
        ),
    };
    let change = match changes.len().saturating_sub(1) {
        0 => change,
        1 => crate::i18n::format("electronics.sheet_net_more_one", &[("change", &change)]),
        n => crate::i18n::format("electronics.sheet_net_more", &[("change", &change), ("n", &n.to_string())]),
    };
    let undo = crate::keymap::display_for(app, "edit.undo").unwrap_or_else(|| crate::i18n::tr("ui.undo").to_string());
    Some(crate::i18n::format("electronics.sheet_net_undo", &[("change", &change), ("undo", &undo)]))
}

/// Pick with the box `r` on screen: drawn right to left (`crossing`), what it touches; else what
/// lies wholly inside it. Shift adds to the pick, Cmd (Ctrl) toggles, otherwise it replaces it.
fn box_pick(app: &mut SolveApp, map: PageMap, r: Rect, crossing: bool, mods: egui::Modifiers, clutter: &Clutter) {
    let found = boxed(app, map, r, crossing, clutter);
    let sel = &mut app.electronics.sheet.selection;
    if mods.command {
        for id in found {
            if !sel.remove(&id) {
                sel.insert(id);
            }
        }
    } else if mods.shift {
        sel.extend(found);
    } else {
        *sel = found;
    }
}

/// Whether the segment `a`–`b` touches the box `r`.
fn segment_meets(a: Pos2, b: Pos2, r: Rect) -> bool {
    if r.contains(a) || r.contains(b) {
        return true;
    }
    let side = |o: Pos2, u: Pos2, v: Pos2| (u - o).x * (v - o).y - (u - o).y * (v - o).x;
    let crosses = |c: Pos2, d: Pos2| (side(c, d, a) > 0.0) != (side(c, d, b) > 0.0) && (side(a, b, c) > 0.0) != (side(a, b, d) > 0.0);
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
    corners.iter().zip(corners.iter().cycle().skip(1)).any(|(c, d)| crosses(*c, *d))
}

/// The sheet objects the box `r` (on screen) picks: with `crossing`, those it touches (a part by
/// its symbol or its texts, where they are drawn clear of `clutter`, a frame by its edges); else
/// those wholly inside it.
fn boxed(app: &SolveApp, map: PageMap, r: Rect, crossing: bool, clutter: &Clutter) -> BTreeSet<u64> {
    let library = app.session.doc.ecad.library();
    let s = sheet(app);
    let pins = Pins::new(s, &library);
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    let takes = |b: Rect| if crossing { r.intersects(b) } else { r.contains_rect(b) };
    let labels = pin_texts_show(map);
    let mut out = BTreeSet::new();
    for part in &s.parts {
        let Some(c) = library.get(&part.component) else { continue };
        let Some([a, b]) = part_bbox(part, c.symbol) else { continue };
        let mut hit = takes(Rect::from_two_pos(screen(a), screen(b)));
        if crossing && !hit {
            let placed: Vec<solidcraft_engine::doc::ecad::Pin> = c.symbol.pins.iter().map(|q| part.placement.pin(q)).collect();
            hit = [Which::Designator, Which::Value].into_iter().any(|which| {
                shown_text(map, part, c, &part.placement, &placed, which, labels, Some(clutter))
                    .is_some_and(|(text, at, height)| r.intersects(text_rect(map, at, height, &text)))
            });
        }
        if hit {
            out.insert(part.id);
        }
    }
    for w in &s.wires {
        let Some(points) = wire_points(&pins, w) else { continue };
        let points: Vec<Pos2> = points.iter().map(|q| screen(*q)).collect();
        let hit = if crossing { points.windows(2).any(|ab| segment_meets(ab[0], ab[1], r)) } else { points.iter().all(|q| r.contains(*q)) };
        if hit {
            out.insert(w.id);
        }
    }
    for l in &s.labels {
        if pins.anchor(&l.at).is_some_and(|a| takes(label_rect(map, l, a))) {
            out.insert(l.id);
        }
    }
    for m in &s.markers {
        if pins.anchor(&m.at).is_some_and(|a| takes(Rect::from_center_size(screen(a), vec2(10.0, 10.0)))) {
            out.insert(m.id);
        }
    }
    for f in &s.frames {
        let b = frame_screen(map, f);
        // A frame is taken by its edges: wholly inside the box, or the box crossing one.
        let hit = if crossing { r.intersects(b) && !b.contains_rect(r) } else { r.contains_rect(b) };
        if hit {
            out.insert(f.id);
        }
    }
    out
}

/// What a press at `p` on wire `id` takes: a bend within reach, else the nearest segment.
fn reshape_at(app: &SolveApp, map: PageMap, id: u64, p: Pos2) -> Option<Reshape> {
    let library = app.session.doc.ecad.library();
    let s = sheet(app);
    let w = s.wires.iter().find(|w| w.id == id)?;
    let points = wire_points(&Pins::new(s, &library), w)?;
    let screen: Vec<Pos2> = points.iter().map(|q| map.to_screen(q[0], q[1])).collect();
    let last = points.len().saturating_sub(1);
    let bend = (1..last).find(|i| screen[*i].distance(p) <= PICK_POINTS + 2.0);
    let segment = (0..last).map(|i| (i, segment_distance(p, screen[i], screen[i + 1]))).min_by(|a, b| a.1.total_cmp(&b.1)).map_or(0, |(i, _)| i);
    Some(Reshape { wire: id, points, bend, segment })
}

/// The sheet as the drag in hand leaves it: picked parts moved with their wires following, one
/// wire reshaped (what stands on a slid segment going along), or one frame resized. `None` while
/// nothing is being dragged.
fn dragged_sheet(app: &SolveApp) -> Option<Schematic> {
    let drag = app.electronics.sheet.drag.as_ref().filter(|d| d.moving && d.by != [0.0; 2] && d.text.is_none())?;
    let s = sheet(app);
    if let Some(r) = &drag.resize {
        let mut out = s.clone();
        if let Some(f) = out.frames.iter_mut().find(|f| f.id == r.frame) {
            f.rect = r.moved(drag.by);
        }
        return Some(out);
    }
    let library = app.session.doc.ecad.library();
    Some(match &drag.reshape {
        Some(r) => {
            let points = r.moved(drag.by);
            let via: Vec<[f64; 2]> = points.get(1..points.len().saturating_sub(1)).map(<[_]>::to_vec).unwrap_or_default();
            solidcraft_engine::doc::ecad::route::reshaped(s, &library, r.wire, &via)
        }
        None => {
            // As the move will leave it: what touches the picked objects hangs on.
            let motion = solidcraft_engine::doc::ecad::Motion { about: [0.0; 2], mirror: false, rotation: 0, by: drag.by };
            solidcraft_engine::doc::ecad::route::moved(s, &library, &app.electronics.sheet.selection, &motion)
        }
    })
}

/// Whether `id` is one of several picked objects (a press on it drags them all).
fn in_group(app: &SolveApp, id: u64) -> bool {
    let picked = &app.electronics.sheet.selection;
    picked.len() > 1 && picked.contains(&id)
}

fn tool_click(app: &mut SolveApp, map: PageMap, p: Pos2, mods: egui::Modifiers, double: bool) {
    let at = snap(map, p, mods.command || mods.ctrl);
    let on_pin = pin_at(app, map, p);
    let Some(tool) = app.electronics.sheet.tool.clone() else { return };
    match tool {
        Tool::Place { component, rotation, mirror } => {
            let mut row = json!({"component": component, "at": at, "rotation": rotation, "mirror": mirror});
            // A designator typed that a part has taken steps on to the next free one, as the ghost
            // shows.
            let given = free_designator(sheet(app), app.electronics.sheet.designator.trim());
            if !given.is_empty() {
                row["designator"] = json!(given);
            }
            // A designator typed in the options counts on for the next part.
            if app.run("ecad.schematic.place", json!({"parts": [row]})).is_ok() && !given.is_empty() {
                app.electronics.sheet.designator = next_designator(sheet(app), &given).unwrap_or_default();
            }
            // The tool stays for the next one.
            app.electronics.sheet.selection.clear();
        }
        Tool::Wire { mut points } => {
            let end = on_pin.unwrap_or(End::Point(at));
            let first = points.is_empty();
            // The bends of the leg to this point, as the tool bends.
            if let Some(last) = points.last().map(End::at) {
                points.extend(app.electronics.sheet.bend.corners(last, end.at()).into_iter().map(End::Point));
            }
            let ends_here = !first && (matches!(end, End::Pin { .. }) || double);
            points.push(end);
            if ends_here {
                finish_wire(app, Some(points));
            } else {
                app.electronics.sheet.tool = Some(Tool::Wire { points });
            }
        }
        Tool::Label { turn } | Tool::Power { turn } => {
            let is_label = matches!(tool, Tool::Label { .. });
            // A click on a label or power port off a pin (or with Alt, on a wire) takes its name.
            if on_pin.is_none()
                && let Some(name) = name_at(app, map, p, mods.alt)
            {
                let s = &mut app.electronics.sheet;
                if is_label {
                    s.label.clone_from(&name)
                } else {
                    s.power.clone_from(&name)
                }
                app.set_status(crate::i18n::format("electronics.sheet_took_name", &[("name", &name)]), false);
                return;
            }
            // Alt only takes a name: where there is none to take, nothing is placed.
            if mods.alt {
                app.set_status(crate::i18n::tr("electronics.sheet_no_name_here").to_string(), false);
                return;
            }
            let name = if is_label { app.electronics.sheet.label.trim().to_string() } else { app.electronics.sheet.power.trim().to_string() };
            if name.is_empty() {
                return;
            }
            let anchor = on_pin.map_or(json!(at), |e| e.param());
            // Turned with R or X: that way; else the command's way, as the ghost shows.
            let mut row = json!({"name": name, "at": anchor});
            if let Some(turn) = turn {
                row["rotation"] = json!(turn);
            }
            if is_label {
                let done = app.run("ecad.schematic.label", json!({"labels": [row]})).is_ok();
                let s = &mut app.electronics.sheet;
                if done
                    && s.count_on
                    && let Some(next) = count_on(&name)
                {
                    s.label = next;
                }
            } else {
                drop(app.run("ecad.schematic.power", json!({"ports": [row]})));
            }
        }
        Tool::NoConnect => {
            if let Some(end) = on_pin {
                drop(app.run("ecad.schematic.marker", json!({"markers": [{"kind": "no_connect", "at": end.param()}]})));
            }
        }
        Tool::PowerFlag => {
            let anchor = on_pin.map_or(json!(at), |e| e.param());
            drop(app.run("ecad.schematic.marker.power_flag", json!({"markers": [{"at": anchor}]})));
        }
        // Presses and drags, in `frame_input`.
        Tool::Frame { .. } => {}
    }
}

/// The name a click at `p` takes for the Label and Power tools: a label's or power port's under
/// it, or with `wires`, the named net of a wire under it.
fn name_at(app: &mut SolveApp, map: PageMap, p: Pos2, wires: bool) -> Option<String> {
    let id = hit(app, map, p)?;
    let s = sheet(app);
    if let Some(l) = s.labels.iter().find(|l| l.id == id) {
        return Some(l.name.clone());
    }
    if !wires || !s.wires.iter().any(|w| w.id == id) {
        return None;
    }
    let checked = crate::electronics_design::checked(app);
    let net = checked.nets.wire_net(sheet(app), id).and_then(|k| checked.nets.nets.get(k))?;
    net.named.then(|| net.name.clone())
}

/// `name` with its closing number one more, keeping its width (`D0` → `D1`, `A09` → `A10`); `None`
/// when it ends in no number.
fn count_on(name: &str) -> Option<String> {
    let head = name.trim_end_matches(|c: char| c.is_ascii_digit());
    let digits = name.get(head.len()..).filter(|d| !d.is_empty())?;
    let next = digits.parse::<u64>().ok()?.checked_add(1)?;
    Some(format!("{head}{next:0width$}", width = digits.len()))
}

/// The designator after `given` that no part has yet (`C101` → `C102`, past any taken); `None`
/// when it ends in no number, or none is free close by.
fn next_designator(s: &Schematic, given: &str) -> Option<String> {
    let (head, n) = solidcraft_engine::doc::ecad::schematic::split_designator(given)?;
    let taken: BTreeSet<String> = s.parts.iter().map(|p| p.designator.to_ascii_lowercase()).collect();
    (1..=MAX_SKIP).filter_map(|k| n.checked_add(k)).map(|m| format!("{head}{m}")).find(|d| !taken.contains(&d.to_ascii_lowercase()))
}

/// The most taken designators the Place tool's count steps over.
const MAX_SKIP: u64 = 10_000;

/// The designator the Place tool gives its next part of `prefix`: the one typed in its options,
/// else one more than the highest of that prefix on the sheet (as `ecad.schematic.place` does).
fn placing_designator(app: &SolveApp, prefix: &str) -> String {
    let given = app.electronics.sheet.designator.trim();
    if given.is_empty() { auto_designator(sheet(app), prefix) } else { free_designator(sheet(app), given) }
}

/// The designator `given`, or while a part has it, the next free one of its prefix (`given` as it
/// is when it has no number to count on).
fn free_designator(s: &Schematic, given: &str) -> String {
    let taken = s.parts.iter().any(|p| p.designator.eq_ignore_ascii_case(given));
    match taken.then(|| next_designator(s, given)).flatten() {
        Some(next) => next,
        None => given.to_string(),
    }
}

/// One more than the highest designator of `prefix` on the sheet.
fn auto_designator(s: &Schematic, prefix: &str) -> String {
    let top = s.numbers().get(&prefix.to_ascii_uppercase()).copied().unwrap_or(0);
    format!("{prefix}{}", top.saturating_add(1))
}

/// Make the wire through `points` (or the tool's), and start the next one.
fn finish_wire(app: &mut SolveApp, points: Option<Vec<End>>) {
    let points = match (points, &app.electronics.sheet.tool) {
        (Some(p), _) => p,
        (None, Some(Tool::Wire { points })) => points.clone(),
        _ => return,
    };
    app.electronics.sheet.tool = Some(Tool::Wire { points: Vec::new() });
    let [first, .., last] = points.as_slice() else { return };
    let via: Vec<[f64; 2]> = points[1..points.len() - 1].iter().map(End::at).collect();
    drop(app.run("ecad.schematic.wire", json!({"wires": [{"from": first.param(), "via": via, "to": last.param(), "route": "straight"}]})));
}

/// A quarter turn of what the Place, Label or Power tool puts down. `false` with another tool.
fn rotate_tool(app: &mut SolveApp) -> bool {
    let shown = tool_turn(app);
    match &mut app.electronics.sheet.tool {
        Some(Tool::Place { rotation, .. }) => {
            *rotation = (*rotation + 90) % 360;
            true
        }
        Some(Tool::Label { turn } | Tool::Power { turn }) => {
            *turn = Some((shown.unwrap_or(0) + 90) % 360);
            true
        }
        _ => false,
    }
}

/// The label the Label or Power tool puts down on `on` (the pin under the pointer, if any), else
/// at the point `at`, turned as it will be: as R and X turned it, else as the command turns it.
fn ghost_label(app: &SolveApp, on: Option<&End>, at: [f64; 2]) -> Option<Label> {
    let s = &app.electronics.sheet;
    let anchor = match on {
        Some(End::Pin { part, pin, .. }) => Anchor::Pin { part: *part, pin: pin.clone() },
        Some(End::Point(p)) => Anchor::Point(*p),
        None => Anchor::Point(at),
    };
    match &s.tool {
        Some(Tool::Label { turn }) => {
            Some(Label { id: 0, name: s.label.clone(), kind: LabelKind::Net, at: anchor, rotation: turn.unwrap_or_else(|| away(app, on, 0)) })
        }
        Some(Tool::Power { turn }) => {
            let (kind, usual) = if is_ground(&s.power) { (LabelKind::Ground, 270) } else { (LabelKind::Rail, 90) };
            let mut l = Label { id: 0, name: s.power.clone(), kind, at: anchor, rotation: usual };
            l.rotation = match turn {
                Some(t) => *t,
                // Off a pin, a port points the usual way, with nothing to work out.
                None if l.at.pin().is_none() => usual,
                None => port_turn(app, &l),
            };
            Some(l)
        }
        _ => None,
    }
}

thread_local! {
    /// The way the Power tool's ghost points on a pin, for the document and port it was worked out
    /// for, so a pointer resting on a pin doesn't redo it every frame.
    static PORT_TURN: std::cell::RefCell<Option<(std::sync::Weak<solidcraft_engine::doc::Document>, Anchor, String, LabelKind, u16)>> =
        const { std::cell::RefCell::new(None) };
}

/// The way the power port `l` points by default where it stands ([`PortRoom::turn`]), worked out
/// again only when the document, the pin or the port changes.
///
/// [`PortRoom::turn`]: solidcraft_engine::doc::ecad::route::PortRoom::turn
fn port_turn(app: &SolveApp, l: &Label) -> u16 {
    let doc = &app.session.doc;
    let cached = PORT_TURN.with_borrow(|c| {
        c.as_ref()
            .filter(|(d, at, name, kind, _)| {
                std::ptr::eq(d.as_ptr(), std::sync::Arc::as_ptr(doc)) && *at == l.at && *name == l.name && *kind == l.kind
            })
            .map(|c| c.4)
    });
    if let Some(t) = cached {
        return t;
    }
    let t = solidcraft_engine::doc::ecad::route::PortRoom::new(sheet(app), &doc.ecad.library()).turn(l);
    PORT_TURN.set(Some((std::sync::Arc::downgrade(doc), l.at.clone(), l.name.clone(), l.kind, t)));
    t
}

/// The way the Label or Power tool's label points now, at the pointer; `None` with another tool.
fn tool_turn(app: &SolveApp) -> Option<u16> {
    if !matches!(app.electronics.sheet.tool, Some(Tool::Label { .. } | Tool::Power { .. })) {
        return None;
    }
    let map = app.viewport.rect.and_then(|r| PageMap::new(&app.electronics.page.cam, r));
    let pointer = app.electronics.sheet.pointer;
    let on = map.zip(pointer).and_then(|(map, p)| pin_at(app, map, p));
    let at = map.zip(pointer).map_or([0.0; 2], |(map, p)| snap(map, p, false));
    ghost_label(app, on.as_ref(), at).map(|l| l.rotation)
}

/// R: turn the part being placed, or the picked objects about their middle.
pub(crate) fn rotate(app: &mut SolveApp) -> bool {
    if rotate_tool(app) {
        return true;
    }
    let items: Vec<u64> = app.electronics.sheet.selection.iter().copied().collect();
    if items.is_empty() {
        return false;
    }
    // One part turns about its origin cross; several about their middle.
    let mut params = json!({"items": items, "rotation": 90});
    if let [id] = items.as_slice()
        && let Some(part) = sheet(app).part(*id)
    {
        params["about"] = json!(part.placement.at);
    }
    keeping_nets(app, "ecad.schematic.move", params);
    true
}

/// X: mirror the part being placed, or the picked objects.
pub(crate) fn mirror(app: &mut SolveApp) -> bool {
    if let Some(Tool::Place { mirror, .. }) = &mut app.electronics.sheet.tool {
        *mirror = !*mirror;
        return true;
    }
    let shown = tool_turn(app);
    if let Some(Tool::Label { turn } | Tool::Power { turn }) = &mut app.electronics.sheet.tool {
        let flip = solidcraft_engine::doc::ecad::Motion { mirror: true, ..Default::default() };
        *turn = Some(flip.angle(shown.unwrap_or(0)));
        return true;
    }
    let items: Vec<u64> = app.electronics.sheet.selection.iter().copied().collect();
    if items.is_empty() {
        return false;
    }
    keeping_nets(app, "ecad.schematic.move", json!({"items": items, "mirror": true}));
    true
}

/// Delete: remove the picked objects (wires on a deleted part's pins stay), and say what went.
pub(crate) fn delete(app: &mut SolveApp) -> bool {
    let items: Vec<u64> = std::mem::take(&mut app.electronics.sheet.selection).into_iter().collect();
    if items.is_empty() {
        return false;
    }
    let what = sheet_name(sheet(app), &items);
    if app.run("ecad.schematic.delete", json!({"items": items})).is_ok() {
        app.set_status(crate::i18n::format("electronics.sheet_deleted", &[("what", &what)]), false);
    }
    true
}

/// How the status line names the sheet objects `items`: a part by its designator, a frame by its
/// title, a label or power port by its name, a wire or marker by what it is, else how many.
fn sheet_name(s: &Schematic, items: &[u64]) -> String {
    let one = match items {
        [id] => s
            .part(*id)
            .map(|p| p.designator.clone())
            .or_else(|| s.frames.iter().find(|f| f.id == *id).map(|f| crate::i18n::format("electronics.sheet_frame_named", &[("title", &f.title)])))
            .or_else(|| s.labels.iter().find(|l| l.id == *id).map(|l| l.name.clone()))
            .or_else(|| s.wires.iter().any(|w| w.id == *id).then(|| crate::i18n::tr("electronics.sheet_wire").to_string()))
            .or_else(|| {
                s.markers.iter().find(|m| m.id == *id).map(|m| {
                    let key = match m.kind {
                        MarkerKind::NoConnect => "electronics.sheet_no_connect",
                        MarkerKind::PowerFlag => "electronics.sheet_power_flag",
                    };
                    crate::i18n::tr(key).to_string()
                })
            }),
        _ => None,
    };
    one.unwrap_or_else(|| crate::i18n::format("electronics.sheet_objects", &[("n", &items.len().to_string())]))
}

/// Escape, one thing at a time: a window, the wire or frame being drawn, the tool, a drag, the
/// pick.
pub(crate) fn escape(app: &mut SolveApp) -> bool {
    if std::mem::take(&mut app.electronics.sheet.choosing) || crate::electronics_design::escape(app) {
        return true;
    }
    let s = &mut app.electronics.sheet;
    match &mut s.tool {
        Some(Tool::Wire { points }) if !points.is_empty() => points.clear(),
        Some(Tool::Frame { from, press }) if from.is_some() => (*from, *press) = (None, None),
        Some(_) => s.tool = None,
        None => return s.drag.take().is_some() | s.boxing.take().is_some() || !std::mem::take(&mut s.selection).is_empty(),
    }
    true
}

/// A toolbar command in an electronics design: the tools arm on the sheet, Add Part opens the
/// components to choose from, and the rest are the design's (`electronics_design::start`). `true`
/// when taken.
pub(crate) fn start(app: &mut SolveApp, id: &str) -> bool {
    if id == "ecad.schematic.place" && !app.electronics.sheet.choosing {
        app.electronics.design.open = None;
    }
    let s = &mut app.electronics.sheet;
    let again = s.tool.as_ref().is_some_and(|t| t.command() == id);
    let tool = match id {
        "ecad.schematic.place" => {
            s.choosing = !s.choosing;
            s.tool = None;
            return true;
        }
        "ecad.schematic.wire" => Tool::Wire { points: Vec::new() },
        "ecad.schematic.label" => Tool::Label { turn: None },
        "ecad.schematic.power" => Tool::Power { turn: None },
        "ecad.schematic.marker" => Tool::NoConnect,
        "ecad.schematic.marker.power_flag" => Tool::PowerFlag,
        "ecad.schematic.frame" => Tool::Frame { from: None, press: None },
        "ecad.schematic.delete" => return delete(app),
        "ecad.schematic.annotate" => {
            drop(app.run("ecad.schematic.annotate", json!({"order": "position", "keep_existing": false})));
            return true;
        }
        _ => return crate::electronics_design::start(app, id),
    };
    // A second press puts the tool down.
    s.tool = (!again).then_some(tool);
    s.choosing = false;
    s.selection.clear();
    true
}

/// Arm the Place tool with `component` (a pick in the Add Part window).
pub(crate) fn place(app: &mut SolveApp, component: u64) {
    // A designator typed for parts of another prefix doesn't carry over.
    let prefix = app.session.doc.ecad.component(component).map(|c| c.prefix.to_ascii_uppercase()).unwrap_or_default();
    let s = &mut app.electronics.sheet;
    let head = solidcraft_engine::doc::ecad::schematic::split_designator(&s.designator).map(|(h, _)| h.to_ascii_uppercase());
    if head.is_none_or(|h| h != prefix) {
        s.designator.clear();
    }
    s.tool = Some(Tool::Place { component, rotation: 0, mirror: false });
    s.choosing = false;
    s.selection.clear();
    zoom_out_for(app, component);
}

/// Zoom out (never in) round the middle of the page when the part about to be placed would cover
/// more than half of it, so its ghost and the place it goes both show.
fn zoom_out_for(app: &mut SolveApp, component: u64) {
    let library = app.session.doc.ecad.library();
    let Some([lo, hi]) = library.get(&component).and_then(|c| c.symbol.bbox()) else { return };
    let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
    let cam = app.electronics.page.cam;
    let Some((rect, map)) = app.viewport.rect.and_then(|r| Some((r, PageMap::new(&cam, r)?))) else { return };
    let scale = map.scale();
    if w * scale <= 0.5 * f64::from(rect.width()) && h * scale <= 0.5 * f64::from(rect.height()) {
        return;
    }
    // The middle of the page's free part stays put.
    let content = crate::shell::content_rect(app, rect);
    let (x, y) = map.to_page(if content.is_positive() { content.center() } else { rect.center() });
    app.electronics.sheet.frame = Some([[x - w, y - h], [x + w, y + h]]);
}

/// The toolbar command to show pressed.
pub(crate) fn running(app: &SolveApp) -> Option<&'static str> {
    let s = &app.electronics.sheet;
    if s.choosing {
        return Some("ecad.schematic.place");
    }
    s.tool.as_ref().map(Tool::command).or_else(|| crate::electronics_design::running(app))
}

/// A label's text height on screen, in points.
fn label_height(map: PageMap) -> f32 {
    ((1.27 * map.scale()) as f32).clamp(8.0, 40.0)
}

/// The way a label turned `rotation` degrees points, on screen (y down).
fn pointing(rotation: u16) -> Vec2 {
    match rotation % 360 {
        90 => vec2(0.0, -1.0),
        180 => vec2(-1.0, 0.0),
        270 => vec2(0.0, 1.0),
        _ => vec2(1.0, 0.0),
    }
}

/// Where a net label's name starts from its point `p`, and which corner of the text sits there:
/// it runs the way the label points, over a level wire and left of an upright one.
fn net_text(p: Pos2, rotation: u16) -> (Pos2, Align2) {
    match rotation % 360 {
        90 => (p + vec2(-1.0, -2.0), Align2::LEFT_BOTTOM),
        180 => (p + vec2(-2.0, -1.0), Align2::RIGHT_BOTTOM),
        270 => (p + vec2(-1.0, 2.0), Align2::RIGHT_BOTTOM),
        _ => (p + vec2(2.0, -1.0), Align2::LEFT_BOTTOM),
    }
}

/// `text` with its `anchor` corner at `p`: level, or upright reading upwards (never upside down),
/// its overbar markup drawn as bars.
fn text_along(painter: &egui::Painter, p: Pos2, anchor: Align2, text: &str, font: FontId, colour: egui::Color32, upright: bool) {
    let angle = if upright { -std::f32::consts::FRAC_PI_2 } else { 0.0 };
    crate::electronics_components::barred_text(painter, p, anchor, text, font, colour, angle);
}

/// The screen box of a label's text and flag, which point the way it is turned.
fn label_rect(map: PageMap, l: &Label, at: [f64; 2]) -> Rect {
    let p = map.to_screen(at[0], at[1]);
    let h = label_height(map);
    let w = h * 0.6 * solidcraft_engine::doc::ecad::overbar::plain(&l.name).chars().count() as f32;
    let d = pointing(l.rotation);
    let across = vec2(-d.y, d.x);
    match l.kind {
        LabelKind::Net => match l.rotation % 360 {
            90 => Rect::from_min_size(p - vec2(h, w + h), vec2(h, w + h)),
            180 => Rect::from_min_size(p - vec2(w + h, h), vec2(w + h, h)),
            270 => Rect::from_min_size(p - vec2(h, 0.0), vec2(h, w + h)),
            _ => Rect::from_min_size(p - vec2(0.0, h), vec2(w + h, h)),
        },
        LabelKind::Rail => {
            let (top, end) = (p + d * h, p + d * (h + 2.0 + w));
            Rect::from_points(&[p, top + across * h * 0.6, top - across * h * 0.6, end + across * h * 0.5, end - across * h * 0.5])
        }
        LabelKind::Ground => {
            let end = p + d * h * 1.2;
            Rect::from_points(&[p, end + across * h * 0.8, end - across * h * 0.8])
        }
    }
}

/// How far `p` is from the line a label runs along: from its point `at` the way it points, as far
/// as its box `r` reaches.
fn label_reach(map: PageMap, l: &Label, at: [f64; 2], r: Rect, p: Pos2) -> f32 {
    let a = map.to_screen(at[0], at[1]);
    let d = pointing(l.rotation);
    let along = if d.x == 0.0 { r.height() } else { r.width() };
    segment_distance(p, a, a + d * along)
}

/// The way a label or power port on the end `end` points by default: away from the pin it is on
/// (as the commands turn it), else `free`.
fn away(app: &SolveApp, end: Option<&End>, free: u16) -> u16 {
    let Some(End::Pin { part, pin, .. }) = end else { return free };
    let library = app.session.doc.ecad.library();
    let Some(part) = sheet(app).part(*part) else { return free };
    let Some(c) = library.get(&part.component) else { return free };
    placed_pins(part, c.symbol)
        .find(|q| q.number == *pin)
        .map_or(free, |q| solidcraft_engine::doc::ecad::route::angle(solidcraft_engine::doc::ecad::route::opposite(q.direction)))
}

/// Everything on the sheet, then what the armed tool would place under the pointer.
fn paint(app: &SolveApp, ui: &Ui, rect: Rect, checked: &crate::electronics_design::Checked) {
    let t = Tokens::get();
    let view = Tokens::view();
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 0.0, t.viewport_bottom);
    let Some(map) = PageMap::new(&app.electronics.page.cam, rect) else { return };
    if app.ui.show_grid {
        crate::electronics_workspace::paint_grid(&painter, map, rect, crate::electronics_drawing::grid_step(app), &t);
    }
    let library = app.session.doc.ecad.library();
    // While dragging, the sheet as the drag will leave it: wires stretch with the parts they meet.
    let preview = dragged_sheet(app);
    let s = preview.as_ref().unwrap_or(sheet(app));
    let pins = Pins::new(s, &library);
    let selection = &app.electronics.sheet.selection;
    let font = FontId::proportional(view.font_small);
    let labels = pin_texts_show(map);
    // Part texts keep clear of the wires, labels, ports and markers in view.
    let clutter = Clutter::new(s, &pins, &library, map, rect);
    let wire_colour = t.accent;
    // Wires of a net whose class carries a current are drawn heavier (while the sheet's wires are
    // the ones the nets were worked out for).
    let rules = &app.session.doc.ecad.board.rules;
    let powered: BTreeSet<&str> = rules.net_classes.iter().filter(|c| c.current.is_some()).flat_map(|c| c.nets.iter().map(String::as_str)).collect();
    let heavy: BTreeSet<usize> = checked.nets.nets.iter().enumerate().filter(|(_, n)| powered.contains(n.name.as_str())).map(|(i, _)| i).collect();
    let same = s.wires.len() == checked.nets.wire_nets.len();
    // Wires under the parts.
    for (i, w) in s.wires.iter().enumerate() {
        let Some(points) = wire_points(&pins, w) else { continue };
        let points: Vec<Pos2> = points.iter().map(|p| map.to_screen(p[0], p[1])).collect();
        if !points.iter().all(|p| p.x.is_finite() && p.y.is_finite()) {
            continue;
        }
        if selection.contains(&w.id) {
            painter.add(egui::Shape::line(points.clone(), Stroke::new(6.0, t.sel_edge_rim)));
        }
        let width = if same && checked.nets.wire_nets.get(i).is_some_and(|n| heavy.contains(n)) { 3.5 } else { 2.0 };
        painter.add(egui::Shape::line(points, Stroke::new(width, wire_colour)));
    }
    for part in &s.parts {
        let Some(c) = library.get(&part.component) else { continue };
        let placement = part.placement;
        let picked = selection.contains(&part.id);
        let moving = app.electronics.sheet.drag.as_ref().filter(|d| d.moving).and_then(|d| d.text.filter(|t| t.0 == part.id).map(|t| (t.1, d.by)));
        paint_part(&painter, map, part, c, &placement, picked, labels, moving, Some(&clutter), &font, &t);
    }
    for l in &s.labels {
        let Some(at) = pins.anchor(&l.at) else { continue };
        paint_label(&painter, map, l, at, selection.contains(&l.id), &font, &t);
    }
    for m in &s.markers {
        let Some(at) = pins.anchor(&m.at) else { continue };
        let p = map.to_screen(at[0], at[1]);
        let colour = if selection.contains(&m.id) { t.sel_edge } else { t.warning };
        match m.kind {
            MarkerKind::NoConnect => {
                let r = 5.0;
                painter.line_segment([p - vec2(r, r), p + vec2(r, r)], Stroke::new(2.0, colour));
                painter.line_segment([p - vec2(r, -r), p + vec2(r, -r)], Stroke::new(2.0, colour));
            }
            MarkerKind::PowerFlag => paint_flag(&painter, p, colour),
        }
    }
    // Ends of wires that meet one pin or point show as dots where three or more meet.
    for p in &checked.nets.junctions {
        painter.circle_filled(map.to_screen(p[0], p[1]), 4.0, wire_colour);
    }
    // Frames' edges and title tabs over the wires and labels (a frame has no fill), so a title
    // reads where a port stands on its edge.
    for f in &s.frames {
        paint_frame(&painter, map, rect, f, section(s, f.id), selection.contains(&f.id), &t);
    }
    // The electrical check's pins that touch nothing (a ring) and wire ends that touch nothing (a
    // square), dim; not while a drag moves things away from them.
    if preview.is_none() {
        paint_open_ends(&painter, map, &checked.report, &t);
    }
    // The armed tool's ghost; without one, the box being drawn, or what a press would take.
    let (hover, free, shift) = ui.ctx().input(|i| (i.pointer.hover_pos(), i.modifiers.command || i.modifiers.ctrl, i.modifiers.shift));
    let Some(hover) = hover.filter(|p| rect.contains(*p)) else { return };
    if let Some(from) = app.electronics.sheet.boxing {
        paint_box(&painter, Rect::from_two_pos(from, hover), hover.x < from.x, &t);
        return;
    }
    if app.electronics.sheet.tool.is_none() && app.electronics.sheet.drag.is_none() {
        paint_hover(app, &painter, map, hover, &clutter, shift, &t);
        return;
    }
    let at = snap(map, hover, free);
    let ghost = t.sel_edge.gamma_multiply(0.7);
    match &app.electronics.sheet.tool {
        Some(Tool::Place { component, rotation, mirror }) => {
            if let Some(c) = library.get(component) {
                let placement = solidcraft_engine::doc::ecad::Placement { at, rotation: *rotation, mirror: *mirror };
                let part = Part {
                    id: 0,
                    component: c.component.id,
                    variant: None,
                    designator: placing_designator(app, &c.component.prefix),
                    value: None,
                    placement,
                    designator_at: None,
                    value_at: None,
                    dnp: false,
                };
                paint_part(&painter, map, &part, c, &placement, true, labels, None, Some(&clutter), &font, &t);
            }
        }
        Some(Tool::Wire { points }) => {
            let target = pin_at(app, map, hover).map_or(at, |e| e.at());
            if let Some(last) = points.last().map(End::at) {
                let corners = app.electronics.sheet.bend.corners(last, target);
                let mut line: Vec<Pos2> = points.iter().map(End::at).chain(corners).chain([target]).map(|p| map.to_screen(p[0], p[1])).collect();
                line.dedup();
                painter.add(egui::Shape::line(line, Stroke::new(2.0, ghost)));
            }
            let p = map.to_screen(target[0], target[1]);
            painter.circle_stroke(p, 5.0, Stroke::new(1.5, ghost));
        }
        Some(Tool::Label { .. } | Tool::Power { .. }) => {
            // Turned as it will be put down: as R and X turned it, else the command's way.
            let on = pin_at(app, map, hover);
            let target = on.as_ref().map_or(at, End::at);
            if let Some(l) = ghost_label(app, on.as_ref(), target) {
                paint_label(&painter, map, &l, target, true, &font, &t);
            }
        }
        Some(Tool::NoConnect) => {
            let target = pin_at(app, map, hover).map_or(at, |e| e.at());
            let p = map.to_screen(target[0], target[1]);
            painter.line_segment([p - vec2(5.0, 5.0), p + vec2(5.0, 5.0)], Stroke::new(2.0, ghost));
            painter.line_segment([p - vec2(5.0, -5.0), p + vec2(5.0, -5.0)], Stroke::new(2.0, ghost));
        }
        Some(Tool::PowerFlag) => {
            let target = pin_at(app, map, hover).map_or(at, |e| e.at());
            paint_flag(&painter, map.to_screen(target[0], target[1]), ghost);
        }
        // The frame from its first corner to the pointer, titled as it will be; before the first
        // corner, a corner mark.
        Some(Tool::Frame { from: Some(from), .. }) => {
            let title = app.electronics.sheet.frame_title.trim();
            let title = if title.is_empty() { free_frame_title(sheet(app)) } else { title.to_string() };
            let f = Frame { id: 0, title, rect: normalized([*from, at]), order: None };
            paint_frame(&painter, map, rect, &f, None, true, &t);
        }
        Some(Tool::Frame { from: None, .. }) => {
            let p = map.to_screen(at[0], at[1]);
            painter.line_segment([p, p + vec2(10.0, 0.0)], Stroke::new(1.5, ghost));
            painter.line_segment([p, p + vec2(0.0, 10.0)], Stroke::new(1.5, ghost));
        }
        None => {}
    }
}

/// A frame's box on screen.
fn frame_screen(map: PageMap, f: &Frame) -> Rect {
    let [lo, hi] = f.rect;
    Rect::from_two_pos(map.to_screen(lo[0], lo[1]), map.to_screen(hi[0], hi[1]))
}

/// A frame's title's height on screen, in points: larger than a label's, so it reads at Fit.
fn frame_title_height(map: PageMap) -> f32 {
    ((2.0 * map.scale()) as f32).clamp(10.0, 40.0)
}

/// A frame's place in print order (1 for the first section), as the Inspector gives it.
fn section(s: &Schematic, id: u64) -> Option<usize> {
    in_order(&s.frames).iter().position(|f| f.id == id).map(|i| i + 1)
}

/// What a frame's tab says: its section number in print order (`number`) and its title.
fn tab_text(f: &Frame, number: Option<usize>) -> String {
    match number {
        Some(n) => crate::i18n::format("electronics.frame_tab", &[("n", &n.to_string()), ("title", &f.title)]),
        None => f.title.clone(),
    }
}

/// A frame's title tab on screen: standing on its top edge at the left, as wide as its section
/// number and title (`number` as for [`tab_text`]) but no wider than the frame (the title is cut
/// short to fit).
fn frame_tab(map: PageMap, f: &Frame, number: Option<usize>) -> Rect {
    let r = frame_screen(map, f);
    let h = frame_title_height(map);
    let w = (h * 0.6 * tab_text(f, number).chars().count().max(1) as f32 + h).min(r.width().max(2.0 * h));
    Rect::from_min_size(r.left_top() - vec2(0.0, h + 4.0), vec2(w, h + 4.0))
}

/// The frame whose title tab is under `p`, if any (tabs are drawn over everything else).
fn tab_at(s: &Schematic, map: PageMap, p: Pos2) -> Option<u64> {
    s.frames.iter().rev().find(|f| frame_tab(map, f, section(s, f.id)).expand(2.0).contains(p)).map(|f| f.id)
}

/// The edges of frame `f` (its tab as for [`frame_tab`]) a press at `p` takes (left, bottom,
/// right, top; two at a corner), or `None` off its edges and on its title tab, which moves it.
fn frame_edges(map: PageMap, f: &Frame, number: Option<usize>, p: Pos2) -> Option<[bool; 4]> {
    let r = frame_screen(map, f);
    if frame_tab(map, f, number).contains(p) || !r.expand(PICK_POINTS).contains(p) {
        return None;
    }
    let (l, rr, t, b) = ((p.x - r.left()).abs(), (p.x - r.right()).abs(), (p.y - r.top()).abs(), (p.y - r.bottom()).abs());
    // On a frame drawn small, the nearer of two facing edges.
    let left = l <= PICK_POINTS && l <= rr;
    let right = rr <= PICK_POINTS && rr < l;
    let top = t <= PICK_POINTS && t <= b;
    let bottom = b <= PICK_POINTS && b < t;
    (left || right || top || bottom).then_some([left, bottom, right, top])
}

/// What a press at `p` on the sheet object `id` takes when it is a frame: an edge or a corner
/// (`None` on its title tab, which moves it).
fn resize_at(app: &SolveApp, map: PageMap, id: u64, p: Pos2) -> Option<Resize> {
    let s = sheet(app);
    let f = s.frames.iter().find(|f| f.id == id)?;
    let edges = frame_edges(map, f, section(s, id), p)?;
    Some(Resize { frame: id, rect: f.rect, edges })
}

/// The Frame tool: a press puts down the first corner, and a drag to the opposite one, or a
/// second click there, makes the frame. `true` while a drag draws it, so the page doesn't pan.
fn frame_input(app: &mut SolveApp, ui: &Ui, map: PageMap, response: &egui::Response, pos: Option<Pos2>, free: bool) -> bool {
    let (pressed, released, down) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.pointer.primary_down()));
    let Some(Tool::Frame { from, press }) = app.electronics.sheet.tool.clone() else { return false };
    let Some(p) = pos else { return press.is_some() && down };
    let at = snap(map, p, free);
    if pressed && response.hovered() {
        match from {
            // The second click: the opposite corner.
            Some(a) if press.is_none() => make_frame(app, a, at),
            _ => app.electronics.sheet.tool = Some(Tool::Frame { from: Some(at), press: Some(p) }),
        }
        return true;
    }
    if released && let (Some(a), Some(q)) = (from, press) {
        if p.distance(q) > DRAG_POINTS {
            make_frame(app, a, at);
        } else {
            // A click: the corner stays for a click at the opposite one.
            app.electronics.sheet.tool = Some(Tool::Frame { from: Some(a), press: None });
        }
        return true;
    }
    press.is_some() && down
}

/// Make the frame with corners `a` and `b` (one too small to be a frame starts again), titled as
/// the tool's options say, else `Section 1`, `Section 2`… the first free one; then pick it, and
/// say how many parts it holds and how many are in no frame yet.
fn make_frame(app: &mut SolveApp, a: [f64; 2], b: [f64; 2]) {
    app.electronics.sheet.tool = Some(Tool::Frame { from: None, press: None });
    if (a[0] - b[0]).abs() < FRAME_LEAST || (a[1] - b[1]).abs() < FRAME_LEAST {
        app.set_status(crate::i18n::tr("electronics.frame_too_small").to_string(), false);
        return;
    }
    let typed = app.electronics.sheet.frame_title.trim().to_string();
    let title = if typed.is_empty() { free_frame_title(sheet(app)) } else { typed };
    // A refusal (a title another frame has) says why on the status line.
    let Ok(reply) = app.run("ecad.schematic.frame", json!({"title": title, "rect": [a, b]})) else { return };
    let id = reply.get("frame").and_then(|f| f.get("id")).and_then(Value::as_u64);
    let s = &mut app.electronics.sheet;
    s.frame_title.clear();
    s.selection = id.into_iter().collect();
    app.set_status(frame_holds(&reply, &title), false);
}

/// What a frame command's `reply` says of frame `title` for the status line: how many parts it
/// holds, and how many are in no frame yet.
fn frame_holds(reply: &Value, title: &str) -> String {
    let parts = counted("electronics.count_parts", reply.get("frame").and_then(|f| f.get("parts")).and_then(Value::as_array).map_or(0, Vec::len));
    let outside = reply.get("outside").and_then(Value::as_array).map_or(0, Vec::len);
    if outside == 0 {
        crate::i18n::format("electronics.frame_holds_all", &[("title", title), ("parts", &parts)])
    } else {
        crate::i18n::format(
            "electronics.frame_holds",
            &[("title", title), ("parts", &parts), ("outside", &counted("electronics.count_parts", outside))],
        )
    }
}

/// `Section 1`, `Section 2`… the first title no frame has (frame titles differ in more than case).
fn free_frame_title(s: &Schematic) -> String {
    let base = crate::i18n::tr("electronics.frame_default_title").to_string();
    (1..=MAX_FRAMES + 1).map(|n| format!("{base} {n}")).find(|t| !s.frames.iter().any(|f| f.title.eq_ignore_ascii_case(t))).unwrap_or(base)
}

/// A section frame: a thin dashed rounded rectangle (solid when it is far larger than the page
/// shows), its title in a tab standing on its top edge at the left.
fn paint_frame(painter: &egui::Painter, map: PageMap, page: Rect, f: &Frame, number: Option<usize>, picked: bool, t: &Tokens) {
    let view = Tokens::view();
    let r = frame_screen(map, f);
    let tab = frame_tab(map, f, number);
    if !(r.min.is_finite() && r.max.is_finite()) || !r.union(tab).expand(8.0).intersects(page) {
        return;
    }
    let colour = if picked { t.sel_edge } else { view.text_dim };
    let stroke = Stroke::new(if picked { 2.0 } else { 1.2 }, colour);
    let outline = rounded(r, (1.5 * map.scale()) as f32);
    if r.width() + r.height() > 8.0 * (page.width() + page.height()) {
        painter.add(egui::Shape::line(outline, stroke));
    } else {
        painter.add(egui::Shape::dashed_line(&outline, stroke, 6.0, 4.0));
    }
    // The tab mostly on the page's own colour first, so what stands under it doesn't cross its
    // title but still shows faintly.
    let corners = egui::CornerRadius { nw: t.radius_sm, ne: t.radius_sm, sw: 0, se: 0 };
    painter.rect_filled(tab, corners, t.viewport_bottom.gamma_multiply(0.8));
    painter.rect(tab, corners, colour.gamma_multiply(0.18), Stroke::new(1.0, colour), egui::StrokeKind::Inside);
    let h = frame_title_height(map);
    let ink = if picked { t.sel_edge } else { view.text };
    let title = crate::electronics_components::fit(painter, &tab_text(f, number), h, tab.width() - h, ink);
    painter.text(tab.left_center() + vec2(h * 0.5, 0.0), Align2::LEFT_CENTER, title, FontId::proportional(h), ink);
    crate::scenario::publish_handle(&format!("electronics:frame:{}", f.title), tab.center());
}

/// The outline of `r` with its corners rounded by `radius` points (at most a third of a side),
/// closed.
fn rounded(r: Rect, radius: f32) -> Vec<Pos2> {
    let rad = radius.min(r.width() / 3.0).min(r.height() / 3.0).clamp(0.0, 12.0);
    // Each corner's centre and the quarter turn its arc starts at (y runs down on screen).
    let corners = [
        (pos2(r.left() + rad, r.top() + rad), 2.0),
        (pos2(r.right() - rad, r.top() + rad), 3.0),
        (pos2(r.right() - rad, r.bottom() - rad), 0.0),
        (pos2(r.left() + rad, r.bottom() - rad), 1.0),
    ];
    let mut out = Vec::with_capacity(21);
    for (c, quarter) in corners {
        for k in 0..=4u8 {
            let a = (quarter + f32::from(k) / 4.0) * std::f32::consts::FRAC_PI_2;
            out.push(c + rad * vec2(a.cos(), a.sin()));
        }
    }
    out.extend(out.first().copied());
    out
}

/// A power flag at `p`: a diamond standing on its point.
fn paint_flag(painter: &egui::Painter, p: Pos2, colour: egui::Color32) {
    painter.add(egui::Shape::convex_polygon(
        vec![p - vec2(0.0, 12.0), p - vec2(6.0, 6.0), p, p - vec2(-6.0, 6.0)],
        egui::Color32::TRANSPARENT,
        Stroke::new(1.5, colour),
    ));
}

/// The box being drawn to pick with: solid left to right (what it holds), dashed right to left
/// (what it touches), as the 3D view draws its box.
fn paint_box(painter: &egui::Painter, r: Rect, crossing: bool, t: &Tokens) {
    let colour = if crossing { t.box_crossing } else { t.box_window };
    painter.rect_filled(r, 0.0, colour.gamma_multiply(0.12));
    if crossing {
        let c = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        painter.add(egui::Shape::dashed_line(&c, Stroke::new(1.2, colour), 6.0, 4.0));
    } else {
        painter.rect_stroke(r, 0.0, Stroke::new(1.2, colour), egui::StrokeKind::Inside);
    }
}

/// A faint outline round what a press at `p` would take (`press_target`), so overlapping
/// objects show which one a press picks.
fn paint_hover(app: &SolveApp, painter: &egui::Painter, map: PageMap, p: Pos2, clutter: &Clutter, shift: bool, t: &Tokens) {
    let stroke = Stroke::new(1.2, t.sel_edge.gamma_multiply(0.6));
    let library = app.session.doc.ecad.library();
    let s = sheet(app);
    let pins = Pins::new(s, &library);
    let screen = |q: [f64; 2]| map.to_screen(q[0], q[1]);
    let outline = |r: Rect| painter.rect_stroke(r, 2.0, stroke, egui::StrokeKind::Outside);
    match press_target(app, map, p, Some(clutter), shift) {
        Some(Target::Text(id, which, at)) => {
            let Some(part) = s.part(id) else { return };
            let Some((text, height)) =
                library.get(&part.component).and_then(|c| text_anchor(part, c, &part.placement, which).map(|(_, h)| (part_text(part, c, which), h)))
            else {
                return;
            };
            outline(text_rect(map, at, height, &text).expand(1.5));
        }
        Some(Target::Item(id)) => {
            if let Some(l) = s.labels.iter().find(|l| l.id == id) {
                if let Some(a) = pins.anchor(&l.at) {
                    outline(label_rect(map, l, a).expand(1.5));
                }
            } else if let Some(m) = s.markers.iter().find(|m| m.id == id) {
                if let Some(a) = pins.anchor(&m.at) {
                    painter.circle_stroke(screen(a), PICK_POINTS + 1.0, stroke);
                }
            } else if let Some(w) = s.wires.iter().find(|w| w.id == id) {
                if let Some(points) = wire_points(&pins, w) {
                    painter.add(egui::Shape::line(points.iter().map(|q| screen(*q)).collect(), Stroke::new(5.0, t.sel_edge_rim.gamma_multiply(0.5))));
                }
            } else if let Some(f) = s.frames.iter().find(|f| f.id == id) {
                outline(frame_tab(map, f, section(s, f.id)));
            } else if let Some([a, b]) = s.part(id).and_then(|part| library.get(&part.component).and_then(|c| part_bbox(part, c.symbol))) {
                outline(Rect::from_two_pos(screen(a), screen(b)).expand(4.0));
            }
        }
        None => {}
    }
}

/// Whether a power name is drawn as ground.
fn is_ground(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.starts_with("GND") || n.starts_with("VSS") || n == "0V"
}

/// Small marks where the electrical check finds a pin that touches nothing (a ring) or a wire end
/// that touches nothing (a square); approved findings get none.
fn paint_open_ends(painter: &egui::Painter, map: PageMap, report: &Report, t: &Tokens) {
    let stroke = Stroke::new(1.2, t.warning.gamma_multiply(0.55));
    for f in report.findings.iter().filter(|f| !f.approved) {
        let Some(at) = f.at else { continue };
        let p = map.to_screen(at[0], at[1]);
        if !(p.x.is_finite() && p.y.is_finite()) {
            continue;
        }
        match f.code {
            "pin_unconnected" => {
                painter.circle_stroke(p, 4.0, stroke);
            }
            "wire_dangling" => {
                painter.rect_stroke(Rect::from_center_size(p, vec2(6.0, 6.0)), 0.0, stroke, egui::StrokeKind::Middle);
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_part(
    painter: &egui::Painter,
    map: PageMap,
    part: &Part,
    c: &Placeable,
    placement: &solidcraft_engine::doc::ecad::Placement,
    picked: bool,
    labels: bool,
    moving: Option<(Which, [f64; 2])>,
    clutter: Option<&Clutter>,
    font: &FontId,
    t: &Tokens,
) {
    let view = Tokens::view();
    let ink = if picked { t.sel_edge } else { view.text };
    let on_page = |p: Pos2| p.x.is_finite() && p.y.is_finite();
    if picked && let Some([a, b]) = c.symbol.bbox().map(|b| placement.bbox(b)) {
        let r = Rect::from_two_pos(map.to_screen(a[0], a[1]), map.to_screen(b[0], b[1])).expand(4.0);
        painter.rect_filled(r, 2.0, t.sel_edge_rim.gamma_multiply(0.25));
    }
    // The designator and value placeholders are drawn below, where the part keeps its texts.
    for g in c
        .symbol
        .graphics
        .iter()
        .filter(|g| !(g.kind == GraphicType::Text && [Which::Designator, Which::Value].iter().any(|w| g.text == w.placeholder())))
    {
        let g = placement.graphic(g);
        // Text standing for the part's own designator and value.
        let g = if g.kind == GraphicType::Text {
            let value = part.value.clone().unwrap_or_else(|| c.component.value.clone());
            solidcraft_engine::doc::ecad::Graphic { text: g.text.replace("{designator}", &part.designator).replace("{value}", &value), ..g }
        } else {
            g
        };
        crate::electronics_components::paint_graphic(painter, map, &g, ink, font);
    }
    let pins: Vec<solidcraft_engine::doc::ecad::Pin> = c.symbol.pins.iter().map(|q| placement.pin(q)).collect();
    for pin in &pins {
        let (a, b) = (map.to_screen(pin.at[0], pin.at[1]), map.to_screen(pin.body_end()[0], pin.body_end()[1]));
        if !(on_page(a) && on_page(b)) {
            continue;
        }
        painter.line_segment([a, b], Stroke::new(1.6, ink));
        crate::scenario::publish_handle(&format!("electronics:part-pin:{}.{}", part.designator, pin.number), a);
    }
    // Pin numbers and names (as the export pages set them); only the pins near the view are set.
    if labels {
        crate::electronics_components::pin_texts(painter, map, &pins, ink, view.text_dim);
    }
    // The origin: where the part is held and what it turns about.
    let o = map.to_screen(placement.at[0], placement.at[1]);
    if on_page(o) {
        let r = 4.0;
        let cross = Stroke::new(1.2, if picked { t.sel_edge } else { view.text_dim });
        painter.line_segment([o - vec2(r, 0.0), o + vec2(r, 0.0)], cross);
        painter.line_segment([o - vec2(0.0, r), o + vec2(0.0, r)], cross);
    }
    // The designator and value, each from its own small cross; a dragged one where it is going.
    for which in [Which::Designator, Which::Value] {
        // Clear of the pins and their numbers, wires and ports, as a press finds it (`text_at`).
        let Some((text, at, height)) = shown_text(map, part, c, placement, &pins, which, labels, clutter) else { continue };
        let at = match moving {
            Some((w, by)) if w == which => [at[0] + by[0], at[1] + by[1]],
            _ => at,
        };
        let p = map.to_screen(at[0], at[1]);
        if !on_page(p) {
            continue;
        }
        let size = ((height * map.scale()) as f32).clamp(6.0, 48.0);
        let colour = if which == Which::Designator { ink } else { view.text_dim };
        painter.text(p, Align2::LEFT_BOTTOM, &text, FontId::proportional(size), colour);
        let r = 3.0;
        let cross = Stroke::new(1.0, if picked { t.sel_edge } else { view.text_dim });
        painter.line_segment([p - vec2(r, 0.0), p + vec2(r, 0.0)], cross);
        painter.line_segment([p - vec2(0.0, r), p + vec2(0.0, r)], cross);
        let key = if which == Which::Designator { "designator" } else { "value" };
        crate::scenario::publish_handle(&format!("electronics:part-{key}:{}", part.designator), p);
    }
    crate::scenario::publish_handle(&format!("electronics:part:{}", part.designator), o);
}

/// A label drawn the way it is turned, away from the pin it sits on: a net label's name runs that
/// way from its dot; a power port's stem, bar and name point that way (a name level or reading
/// upwards, so ports on neighbouring pins stand side by side); ground's bars too.
fn paint_label(painter: &egui::Painter, map: PageMap, l: &Label, at: [f64; 2], picked: bool, font: &FontId, t: &Tokens) {
    let p = map.to_screen(at[0], at[1]);
    if !(p.x.is_finite() && p.y.is_finite()) {
        return;
    }
    let colour = if picked { t.sel_edge } else { t.form_success };
    let h = label_height(map);
    let text_font = FontId::new(h, font.family.clone());
    let d = pointing(l.rotation);
    let across = vec2(-d.y, d.x);
    let upright = d.x == 0.0;
    match l.kind {
        LabelKind::Net => {
            painter.circle_filled(p, 2.5, colour);
            let (start, anchor) = net_text(p, l.rotation);
            text_along(painter, start, anchor, &l.name, text_font, colour, upright);
        }
        LabelKind::Rail => {
            // A bar on a short stem, the name beyond it.
            let top = p + d * h;
            painter.line_segment([p, top], Stroke::new(1.6, colour));
            painter.line_segment([top - across * h * 0.6, top + across * h * 0.6], Stroke::new(2.0, colour));
            let anchor = if d.x < 0.0 || d.y > 0.0 { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER };
            text_along(painter, top + d * 2.0, anchor, &l.name, text_font, colour, upright);
        }
        LabelKind::Ground => {
            // A stem out to three shrinking bars.
            let base = p + d * h * 0.6;
            painter.line_segment([p, base], Stroke::new(1.6, colour));
            for (i, w) in [0.8f32, 0.5, 0.2].into_iter().enumerate() {
                let c = base + d * (i as f32 * h * 0.3);
                painter.line_segment([c - across * h * w, c + across * h * w], Stroke::new(1.8, colour));
            }
        }
    }
    crate::scenario::publish_handle(&format!("electronics:label:{}", l.id), p);
}

/// The inspector on the schematic: the armed tool's options, the picked part's fields, or what
/// the sheet holds.
pub(crate) fn inspector(app: &mut SolveApp, ui: &mut Ui) {
    if let Some(tool) = app.electronics.sheet.tool.clone() {
        let title = solidcraft_engine::find_command(tool.command()).map_or(tool.command(), |c| c.label);
        let title = app.tr_label(title).to_string();
        // What the options offer: the designator the next part gets, the design's net names.
        let auto = match &tool {
            Tool::Place { component, .. } => {
                let prefix = app.session.doc.ecad.component(*component).map(|c| c.prefix.clone()).unwrap_or_default();
                auto_designator(sheet(app), &prefix)
            }
            Tool::Frame { .. } => free_frame_title(sheet(app)),
            _ => String::new(),
        };
        let power = matches!(tool, Tool::Power { .. });
        let names = if matches!(tool, Tool::Label { .. } | Tool::Power { .. }) { net_names(app, power) } else { Vec::new() };
        ui_kit::section(ui, ("electronics_sheet_tool", tool.command()), &title, true, |ui| {
            let s = &mut app.electronics.sheet;
            let help = match tool {
                Tool::Place { .. } => {
                    ui_kit::text_row(ui, crate::i18n::tr("electronics.field_designator"), &mut s.designator, &auto);
                    "electronics.sheet_place_help"
                }
                Tool::Wire { .. } => {
                    ui.horizontal_wrapped(|ui| {
                        for (bend, key) in BENDS {
                            if ui_kit::choice_chip(ui, &format!("bend:{key}"), crate::i18n::tr(key), s.bend == bend).clicked() {
                                s.bend = bend;
                            }
                        }
                    });
                    "electronics.sheet_wire_help"
                }
                Tool::Label { .. } => {
                    ui_kit::text_row(ui, crate::i18n::tr("electronics.field_net_name"), &mut s.label, "NET1");
                    net_menu(ui, "electronics_sheet_label_nets", &names, &mut s.label);
                    if ui_kit::checkbox_row(ui, "electronics_sheet_count_on", crate::i18n::tr("electronics.sheet_count_on"), s.count_on) {
                        s.count_on = !s.count_on;
                    }
                    "electronics.sheet_label_help"
                }
                Tool::Power { .. } => {
                    ui_kit::text_row(ui, crate::i18n::tr("electronics.field_net_name"), &mut s.power, "GND");
                    net_menu(ui, "electronics_sheet_power_nets", &names, &mut s.power);
                    ui.horizontal_wrapped(|ui| {
                        for name in ["GND", "+3V3", "+5V", "VCC", "VIN"] {
                            if ui_kit::choice_chip(ui, &format!("power:{name}"), name, s.power == name).clicked() {
                                s.power = name.into();
                            }
                        }
                    });
                    "electronics.sheet_power_help"
                }
                Tool::NoConnect => "electronics.sheet_marker_help",
                Tool::PowerFlag => "electronics.sheet_power_flag_help",
                Tool::Frame { .. } => {
                    ui_kit::text_row(ui, crate::i18n::tr("electronics.frame_field_title"), &mut s.frame_title, &auto);
                    "electronics.frame_tool_help"
                }
            };
            ui_kit::text(ui, crate::i18n::tr(help), ui_kit::Text::Note(help));
            if matches!(tool, Tool::Label { .. } | Tool::Power { .. }) {
                ui_kit::text(ui, crate::i18n::tr("electronics.sheet_turn_label_help"), ui_kit::Text::Note("electronics.sheet_turn_label_help"));
                ui_kit::text(ui, crate::i18n::tr("electronics.sheet_take_name_help"), ui_kit::Text::Note("electronics.sheet_take_name_help"));
            }
        });
    }
    let selection: Vec<u64> = app.electronics.sheet.selection.iter().copied().collect();
    let net = one_net(app, &selection);
    let s = sheet(app);
    match selection.as_slice() {
        [id] if s.part(*id).is_some() => part_section(app, ui, *id),
        [id] if s.frames.iter().any(|f| f.id == *id) => frame_section(app, ui, *id),
        _ if let Some(net) = net => net_section(app, ui, &net),
        [] => {
            let text = crate::i18n::format(
                "electronics.sheet_summary_counted",
                &[
                    ("parts", &counted("electronics.count_parts", s.parts.len())),
                    ("wires", &counted("electronics.count_wires", s.wires.len())),
                    ("labels", &counted("electronics.count_labels", s.labels.len())),
                ],
            );
            ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.sheet_summary"));
            // The section frames, and the parts no frame holds yet (Export Pages lists them on a
            // page of their own).
            if !s.frames.is_empty() {
                let outside = s.parts.len().saturating_sub(part_frames(s, &app.session.doc.ecad.library()).len());
                let text = crate::i18n::format(
                    "electronics.frame_summary_counted",
                    &[("frames", &counted("electronics.count_frames", s.frames.len())), ("outside", &counted("electronics.count_parts", outside))],
                );
                ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.frame_summary"));
            }
            check_summary(app, ui);
            ui_kit::text(ui, crate::i18n::tr("electronics.design_help"), ui_kit::Text::Note("electronics.design_help"));
        }
        many => {
            let text = crate::i18n::format("electronics.sheet_picked", &[("n", &many.len().to_string())]);
            ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.sheet_picked"));
        }
    }
    if selection.len() > 1 {
        move_by_section(app, ui, &selection);
    }
}

/// `n` things as a phrase from `key` (`1 part`, `3 parts`): `<key>_one` for one.
fn counted(key: &str, n: usize) -> String {
    let one = format!("{key}_one");
    crate::i18n::format(if n == 1 { &one } else { key }, &[("n", &n.to_string())])
}

/// Move by, for several picked objects: a distance typed in x and y, and Move, one step (for a
/// long move, where a drag would need both places on one screen).
fn move_by_section(app: &mut SolveApp, ui: &mut Ui, items: &[u64]) {
    let label = crate::i18n::tr("electronics.sheet_move_by");
    if let Some(by) = crate::electronics_editor::pair(app, ui, "sheet:move_by", label, app.electronics.sheet.move_by)
        && by.iter().all(|v| v.is_finite())
    {
        app.electronics.sheet.move_by = by;
    }
    let by = app.electronics.sheet.move_by;
    let go = crate::electronics_editor::button(ui, "sheet-move-by", &crate::electronics_editor::tr("electronics.sheet_move"), by != [0.0; 2]);
    if go {
        // Whole grid steps, as a drag goes, so pins stay where wires on the grid meet them;
        // Cmd (Ctrl) moves by exactly what is typed.
        let free = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        let step = if free { by } else { by.map(|v| ((v / GRID).round() * GRID * 1e4).round() / 1e4 + 0.0) };
        app.electronics.sheet.move_by = step;
        if step == [0.0; 2] {
            app.set_status(crate::i18n::tr("electronics.sheet_move_too_small").to_string(), false);
        } else if moved_by(app, items, step) && step != by {
            let mm = |v: f64| format!("{v:.2}");
            app.set_status(crate::i18n::format("electronics.sheet_move_snapped", &[("dx", &mm(step[0])), ("dy", &mm(step[1]))]), false);
        }
    }
}

/// The most net names the Label and Power tools' menu lists.
const MAX_NAMES: usize = 500;

/// The design's net names given by labels and power ports, sorted; the supplies first for
/// `power`.
fn net_names(app: &mut SolveApp, power: bool) -> Vec<String> {
    let checked = crate::electronics_design::checked(app);
    let mut nets: Vec<(bool, &str)> = checked.nets.nets.iter().filter(|n| n.named).map(|n| (!(power && n.power), n.name.as_str())).collect();
    nets.sort_unstable();
    nets.dedup();
    nets.into_iter().take(MAX_NAMES).map(|(_, n)| n.to_string()).collect()
}

/// A menu of the design's net names under the Name field; a pick fills it. Each entry is the
/// handle `dialog:<id>:<net>`.
fn net_menu(ui: &mut Ui, id: &str, names: &[String], name: &mut String) {
    if names.is_empty() {
        return;
    }
    ui_kit::row(ui, crate::i18n::tr("electronics.sheet_existing_nets"), |ui| {
        ui_kit::dropdown_menu(ui, id, crate::i18n::tr("electronics.sheet_pick_net"), |ui| {
            for n in names {
                let r = ui_kit::list_picker(ui, &format!("{id}_{n}"), n, n == name);
                crate::scenario::publish_handle(&format!("dialog:{id}:{n}"), r.rect.center());
                if r.clicked() {
                    name.clone_from(n);
                }
            }
        });
    });
}

/// The net the picked wires and labels are all on, by name; `None` when anything else is picked
/// or they are on more than one net. One walk over the sheet's wires and labels, however many are
/// picked.
fn one_net(app: &mut SolveApp, selection: &[u64]) -> Option<String> {
    if selection.is_empty() {
        return None;
    }
    let checked = crate::electronics_design::checked(app);
    let s = &app.session.doc.ecad.schematic;
    let picked: BTreeSet<u64> = selection.iter().copied().collect();
    let wires = s.wires.iter().map(|w| w.id).zip(checked.nets.wire_nets.iter().copied());
    let labels = s.labels.iter().map(|l| l.id).zip(checked.nets.label_nets.iter().copied());
    let (mut net, mut found) = (None, 0usize);
    for (_, n) in wires.chain(labels).filter(|(id, _)| picked.contains(id)) {
        if net.is_some_and(|m| m != n) {
            return None;
        }
        net = Some(n);
        found = found.saturating_add(1);
    }
    // Anything else picked (a part, a marker, a frame) has no one net.
    if found != picked.len() {
        return None;
    }
    checked.nets.nets.get(net?).map(|n| n.name.clone())
}

/// What the net's section was asked to do: put the net in the default class or a class, in a
/// new class named after it, open Net Classes, or open Design Intent on the net.
enum ClassPick {
    Default,
    Class(String),
    New,
    Open,
    Intent,
}

/// The picked net: its name, its net class (Default, a class, or a new one named after the net,
/// which opens Net Classes on it) and that class's sizes and current, then its role in the
/// design's intent. A change is one `ecad.board.netclass`.
fn net_section(app: &mut SolveApp, ui: &mut Ui, net: &str) {
    let role = crate::electronics_intent::net_role(app, net);
    let b = &app.session.doc.ecad.board;
    let (rules, stack) = (b.rules.clone(), b.stackup_or_default());
    let class = rules.class_of(net).cloned();
    let title = crate::i18n::format("electronics.sheet_net", &[("net", net)]);
    let default_name = crate::i18n::tr("electronics.class_default").to_string();
    let mut pick: Option<ClassPick> = None;
    ui_kit::section(ui, ("electronics_sheet_net", net), &title, true, |ui| {
        let shown = class.as_ref().map_or_else(|| default_name.clone(), |c| c.name.clone());
        ui_kit::row(ui, crate::i18n::tr("electronics.field_net_class"), |ui| {
            // Each entry is the handle `dialog:electronics_sheet_net_class:<class>` (`:Default`,
            // and `:new` for a new class from the net).
            ui_kit::dropdown_menu(ui, "electronics_sheet_net_class", &shown, |ui| {
                let mut entry = |ui: &mut Ui, on: bool, text: &str, key: &str, chosen: ClassPick| {
                    let r = ui_kit::list_picker(ui, &format!("sheet_net_class_{key}"), text, on);
                    crate::scenario::publish_handle(&format!("dialog:electronics_sheet_net_class:{key}"), r.rect.center());
                    if r.clicked() {
                        pick = Some(chosen);
                    }
                };
                entry(ui, class.is_none(), &default_name, DEFAULT_CLASS, ClassPick::Default);
                for c in &rules.net_classes {
                    let on = class.as_ref().is_some_and(|k| k.name == c.name);
                    entry(ui, on, &c.name, &c.name, ClassPick::Class(c.name.clone()));
                }
                ui.separator();
                entry(ui, false, crate::i18n::tr("electronics.class_from_net"), "new", ClassPick::New);
            });
        });
        let (width, clearance) = class.as_ref().map_or((rules.track_width, rules.clearance), |c| (c.track_width, c.clearance));
        let sizes = crate::i18n::format(
            "electronics.sheet_net_sizes",
            &[("width", &crate::electronics_editor::mm(width)), ("clearance", &crate::electronics_editor::mm(clearance))],
        );
        ui_kit::text(ui, &sizes, ui_kit::Text::Note("electronics.sheet_net_sizes"));
        if let Some(c) = class.as_ref().filter(|c| c.current.is_some()) {
            let text = crate::electronics_board_rules::current_text(c, &stack);
            ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.sheet_net_current"));
        }
        // The net's role beside its class: kept, proposed, or none yet.
        let r = ui_kit::row(ui, crate::i18n::tr("electronics.field_role"), |ui| ui_kit::text(ui, &role, ui_kit::Text::Inline));
        crate::scenario::publish_target(&format!("electronics:sheet:net_role:{net}"), r.rect);
        // Net Classes opens on the net's class.
        if crate::electronics_editor::button(ui, "sheet-net-classes", &crate::electronics_editor::tr("electronics.sheet_net_classes"), true) {
            pick = Some(ClassPick::Open);
        }
        if crate::electronics_editor::button(ui, "sheet-net-intent", &crate::electronics_editor::tr("electronics.sheet_net_intent"), true) {
            pick = Some(ClassPick::Intent);
        }
    });
    match pick {
        Some(ClassPick::Open) => match &class {
            Some(c) => crate::electronics_board_rules::show_class(app, &c.name),
            None => {
                app.electronics.design.open = Some(crate::electronics_design::Open::NetClasses(Default::default()));
                app.electronics.sheet.choosing = false;
            }
        },
        Some(ClassPick::Intent) => crate::electronics_intent::show(app, &format!("net|{net}")),
        Some(ClassPick::Default) => {
            if let Some(c) = &class
                && app.run("ecad.board.netclass", json!({"name": c.name, "remove_nets": [net]})).is_ok()
            {
                app.set_status(crate::i18n::format("electronics.class_net_default", &[("net", net)]), false);
            }
        }
        Some(ClassPick::Class(name)) if class.as_ref().is_none_or(|c| c.name != name) => {
            if app.run("ecad.board.netclass", json!({"name": name, "add_nets": [net]})).is_ok() {
                app.set_status(crate::i18n::format("electronics.class_net_moved", &[("net", net), ("class", &name)]), false);
            }
        }
        Some(ClassPick::New) => {
            let name = crate::electronics_board_rules::class_name_for(net, &rules);
            if app.run("ecad.board.netclass", json!({"name": name, "add_nets": [net]})).is_ok() {
                crate::electronics_board_rules::show_class(app, &name);
                app.set_status(crate::i18n::format("electronics.class_made_for", &[("net", net), ("class", &name)]), false);
            }
        }
        _ => {}
    }
}

/// The electrical check's counts, live; Electrical Check on the CHECK tab lists them. Pins that
/// touch nothing yet are counted on their own, not as errors, while the sheet is being drawn.
fn check_summary(app: &mut SolveApp, ui: &mut Ui) {
    let r = &crate::electronics_design::checked(app).report;
    let open: Vec<_> = r.findings.iter().filter(|f| !f.approved && f.code == "pin_unconnected").collect();
    let open_errors = open.iter().filter(|f| f.severity == solidcraft_engine::doc::ecad::Severity::Error).count();
    let errors = r.errors.saturating_sub(open_errors);
    let warnings = r.warnings.saturating_sub(open.len() - open_errors);
    let open = open.len();
    if errors + warnings + open == 0 {
        ui_kit::text(ui, crate::i18n::tr("electronics.sheet_checks_clean"), ui_kit::Text::Note("electronics.sheet_checks"));
        return;
    }
    if errors + warnings == 0 {
        let text = crate::i18n::format("electronics.sheet_checks_open", &[("n", &open.to_string())]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.sheet_checks_open"));
        return;
    }
    let text = crate::i18n::format("electronics.sheet_checks", &[("errors", &errors.to_string()), ("warnings", &warnings.to_string())]);
    let style = if errors > 0 { ui_kit::Text::Error("electronics.sheet_checks") } else { ui_kit::Text::Note("electronics.sheet_checks") };
    ui_kit::text(ui, &text, style);
    if open > 0 {
        let text = crate::i18n::format("electronics.sheet_checks_open_more", &[("n", &open.to_string())]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.sheet_checks_open_more"));
    }
}

/// The picked part: its designator, value, component and do-not-populate.
fn part_section(app: &mut SolveApp, ui: &mut Ui, id: u64) {
    let Some(part) = sheet(app).part(id).cloned() else { return };
    let library_value = app.session.doc.ecad.component(part.component).map(|c| c.value.clone()).unwrap_or_default();
    let title = part.designator.clone();
    ui_kit::section(ui, ("electronics_part", id), &title, true, |ui| {
        let key = |f: &str| format!("part:{id}:{f}");
        if let Some(d) =
            crate::electronics_editor::field(app, ui, &key("designator"), crate::i18n::tr("electronics.field_designator"), &part.designator)
        {
            drop(app.run("ecad.schematic.edit", json!({"parts": [{"part": id, "designator": d}]})));
        }
        let shown = part.value.clone().unwrap_or_else(|| library_value.clone());
        if let Some(v) = crate::electronics_editor::field(app, ui, &key("value"), crate::i18n::tr("electronics.field_value"), &shown) {
            // Back to the component's value when it is typed again, or left empty.
            let value = if v.trim().is_empty() || v == library_value { Value::Null } else { json!(v) };
            drop(app.run("ecad.schematic.edit", json!({"parts": [{"part": id, "value": value}]})));
        }
        let components: Vec<(u64, String)> = app.session.doc.ecad.components.iter().map(|c| (c.id, c.name.clone())).collect();
        let at = components.iter().position(|(c, _)| *c == part.component).unwrap_or(0);
        let names: Vec<&str> = components.iter().map(|(_, n)| n.as_str()).collect();
        let mut chosen = at;
        ui_kit::choice_row(ui, crate::i18n::tr("electronics.field_component"), &key("component"), &mut chosen, &names);
        if chosen != at
            && let Some((c, _)) = components.get(chosen)
        {
            drop(app.run("ecad.schematic.edit", json!({"parts": [{"part": id, "component": c}]})));
        }
        let mut dnp = part.dnp;
        if ui_kit::checkbox_row(ui, &key("dnp"), crate::i18n::tr("electronics.field_dnp"), dnp) {
            dnp = !dnp;
            drop(app.run("ecad.schematic.edit", json!({"parts": [{"part": id, "dnp": dnp}]})));
        }
        // The turn and the mirror, about the part's origin cross (as R and X turn and mirror it).
        let turns = ["0", "90", "180", "270"];
        let at = usize::from(part.placement.rotation / 90).min(turns.len() - 1);
        let mut chosen = at;
        ui_kit::choice_row(ui, crate::i18n::tr("electronics.field_rotation"), &key("rotation"), &mut chosen, &turns);
        if chosen != at {
            let by = (chosen + turns.len() - at) % turns.len() * 90;
            keeping_nets(app, "ecad.schematic.move", json!({"items": [id], "rotation": by, "about": part.placement.at}));
        }
        if ui_kit::checkbox_row(ui, &key("mirror"), crate::i18n::tr("electronics.field_mirrored"), part.placement.mirror) {
            keeping_nets(app, "ecad.schematic.move", json!({"items": [id], "mirror": true, "about": part.placement.at}));
        }
    });
}

/// The most designators the frame's section lists.
const MAX_LISTED: usize = 60;

/// The picked frame: its title, its corners and its place in the print order, then the parts it
/// holds and the frames it overlaps. A change is one `ecad.schematic.frame.edit`.
fn frame_section(app: &mut SolveApp, ui: &mut Ui, id: u64) {
    let s = sheet(app);
    let Some(f) = s.frames.iter().find(|f| f.id == id).cloned() else { return };
    let members = part_frames(s, &app.session.doc.ecad.library());
    let mut parts: Vec<String> = s.parts.iter().filter(|p| members.get(&p.id) == Some(&id)).map(|p| p.designator.clone()).collect();
    parts.sort_by(|a, b| solidcraft_engine::doc::ecad::connect::natural_cmp(a, b));
    let page = in_order(&s.frames).iter().position(|x| x.id == id).map_or(0, |i| i + 1);
    // Where frames overlap, the parts in both belong to the first in print order.
    let order = in_order(&s.frames);
    let title_of = |k: u64| s.frames.iter().find(|x| x.id == k).map(|x| x.title.clone()).unwrap_or_default();
    let shared: Vec<(String, String)> = overlaps(&s.frames)
        .into_iter()
        .filter_map(|(a, b)| {
            let other = if a == id {
                b
            } else if b == id {
                a
            } else {
                return None;
            };
            let first = order.iter().find(|x| x.id == a || x.id == b).map_or(a, |x| x.id);
            Some((title_of(other), title_of(first)))
        })
        .collect();
    let heading = crate::i18n::format("electronics.frame_heading", &[("title", &f.title)]);
    let mut edit: Option<Value> = None;
    ui_kit::section(ui, ("electronics_frame", id), &heading, true, |ui| {
        let key = |k: &str| format!("frame:{id}:{k}");
        if let Some(t) = crate::electronics_editor::field(app, ui, &key("title"), crate::i18n::tr("electronics.frame_field_title"), &f.title) {
            edit = Some(json!({"title": t.trim()}));
        }
        let [lo, hi] = f.rect;
        if let Some(c) = crate::electronics_editor::pair(app, ui, &key("from"), crate::i18n::tr("electronics.frame_field_lower_left"), lo) {
            edit = Some(json!({"rect": [c, hi]}));
        }
        if let Some(c) = crate::electronics_editor::pair(app, ui, &key("to"), crate::i18n::tr("electronics.frame_field_upper_right"), hi) {
            edit = Some(json!({"rect": [lo, c]}));
        }
        // The print order: a number, or nothing (after the numbered ones, oldest first).
        let order = f.order.map(|o| o.to_string()).unwrap_or_default();
        let label = crate::i18n::tr("electronics.frame_field_order");
        if let Some(text) = crate::electronics_editor::field(app, ui, &key("order"), label, &order) {
            match text.trim() {
                "" => edit = Some(json!({"order": null})),
                t => match t.parse::<u32>() {
                    Ok(n) => edit = Some(json!({"order": n})),
                    Err(_) => app.set_status(crate::i18n::format("electronics.not_a_number", &[("field", label), ("text", t)]), true),
                },
            }
        }
        let size = crate::i18n::format(
            "electronics.frame_size",
            &[
                ("w", &crate::electronics_editor::mm(hi[0] - lo[0])),
                ("h", &crate::electronics_editor::mm(hi[1] - lo[1])),
                ("page", &page.to_string()),
            ],
        );
        ui_kit::text(ui, &size, ui_kit::Text::Note("electronics.frame_size"));
        let listed = if parts.is_empty() {
            crate::i18n::tr("electronics.frame_no_parts").to_string()
        } else {
            let mut names = parts.iter().take(MAX_LISTED).cloned().collect::<Vec<_>>().join(", ");
            if parts.len() > MAX_LISTED {
                names.push_str(", …");
            }
            crate::i18n::format("electronics.frame_parts_counted", &[("n", &counted("electronics.count_parts", parts.len())), ("parts", &names)])
        };
        ui_kit::text(ui, &listed, ui_kit::Text::Note("electronics.frame_parts"));
        for (other, first) in &shared {
            let text = crate::i18n::format("electronics.frame_overlap", &[("other", other), ("first", first)]);
            ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.frame_overlap"));
        }
        ui_kit::text(ui, crate::i18n::tr("electronics.frame_help"), ui_kit::Text::Note("electronics.frame_help"));
    });
    if let Some(mut p) = edit {
        p["frame"] = json!(id);
        drop(app.run("ecad.schematic.frame.edit", p));
    }
}
