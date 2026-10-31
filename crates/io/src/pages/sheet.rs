//! The schematic drawn onto a page through a view (sheet millimetres, y up, to page millimetres,
//! y down): parts with their symbols, pins, pin names and numbers, designators and values; wires
//! and junctions; net labels as tags, rail and ground ports; markers; and frames as dashed
//! rounded rectangles with their title in a tab. A part not fitted is struck through, with a
//! "DNP" tag after its designator (or another clear spot by its texts). Line widths are fixed on
//! paper; text scales with the drawing but never prints below [`super::MIN_TEXT`] (a map at a
//! small scale may leave it out). A pin name with no room inside its symbol goes along its pin
//! outside the body where that is clear; a net label's cross-reference that would print over a
//! wire goes beside the label.

use std::cell::Cell;
use std::collections::BTreeMap;

use solidcraft_doc::ecad::connect::Connectivity;
use solidcraft_doc::ecad::graphics::shapes_bbox;
use solidcraft_doc::ecad::schematic::{Pins, part_bbox};
use solidcraft_doc::ecad::{Frame, Graphic, GraphicType, LabelKind, Library, MarkerKind, Part, Pin, PinDirection, Placeable, Placement, Schematic};

use super::font::Font;
use super::page::{Align, Baseline, Item, Rgb, Stroke, Text, Weight};
use super::{MIN_TEXT, Palette};

/// Sheet text sizes, in sheet millimetres: names and labels, pin names, pin numbers.
pub const TEXT: f64 = 1.27;
const PIN_NAME: f64 = 1.0;
const PIN_NUMBER: f64 = 0.85;
/// The most labels a page tidies (turning stacked ports); past it they are drawn as saved.
const MAX_TIDY_LABELS: usize = 2000;
/// The most boxes a page's [`Room`] holds, and the most box tests looking for room takes on one
/// page.
const MAX_ROOM_BOXES: usize = 1 << 16;
const ROOM_BUDGET: usize = 20_000_000;
/// Printed line widths, in millimetres.
const BODY: f64 = 0.25;
const PIN: f64 = 0.22;
const WIRE: f64 = 0.3;

/// Sheet to page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub scale: f64,
    /// The sheet point drawn at the page point `to`.
    pub from: [f64; 2],
    pub to: [f64; 2],
}

impl View {
    /// The view that draws the sheet box `[lo, hi]` into the page box `placed` at `scale`.
    pub fn new([lo, hi]: [[f64; 2]; 2], placed: [[f64; 2]; 2], scale: f64) -> View {
        View { scale, from: [lo[0], hi[1]], to: placed[0] }
    }

    pub fn p(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        [self.to[0] + (x - self.from[0]) * self.scale, self.to[1] - (y - self.from[1]) * self.scale]
    }

    /// A page box of a sheet box.
    pub fn rect(&self, [a, b]: [[f64; 2]; 2]) -> [[f64; 2]; 2] {
        let (p, q) = (self.p(a), self.p(b));
        [[p[0].min(q[0]), p[1].min(q[1])], [p[0].max(q[0]), p[1].max(q[1])]]
    }
}

/// How much a drawing shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detail {
    /// Texts at all (a map at a small scale may leave them out).
    pub texts: bool,
    /// The sheet box drawn; things wholly outside it are left out.
    pub within: Option<[[f64; 2]; 2]>,
}

/// Where a label's cross-reference starts on the page (where its text ends, or beside it), and
/// which way it runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LabelEnd {
    pub at: [f64; 2],
    /// The page direction it runs.
    pub dir: [f64; 2],
    pub rotation: f64,
    pub size: f64,
    /// Where the label starts by its point, and its half height across the way it runs (a net
    /// label's tag, a rail's bar or name), for a cross-reference set beside it.
    tip: [f64; 2],
    half: f64,
}

/// What a drawing gives besides its items.
pub struct Drawn {
    pub items: Vec<Item>,
    /// By label id.
    pub label_ends: BTreeMap<u64, LabelEnd>,
    /// Pin names with no room even cut short (`U1.AD2 VBUS`), by the part's centre on the sheet,
    /// for the export to warn of.
    pub left_out: Vec<([f64; 2], String)>,
    /// What the drawing takes of the page, for the cross-references to keep clear of.
    pub(super) room: Room,
}

pub struct Ctx<'a> {
    pub sheet: &'a Schematic,
    pub library: &'a Library<'a>,
    pub conn: &'a Connectivity,
    pub pal: &'a Palette,
    pub font: &'a Font<'a>,
}

impl Ctx<'_> {
    /// A text's printed size: the sheet size scaled, never below the least.
    fn size(&self, view: &View, sheet_size: f64) -> f64 {
        (sheet_size * view.scale).max(MIN_TEXT)
    }
}

fn near([lo, hi]: [[f64; 2]; 2], [a, b]: [[f64; 2]; 2], margin: f64) -> bool {
    a[0] <= hi[0] + margin && b[0] >= lo[0] - margin && a[1] <= hi[1] + margin && b[1] >= lo[1] - margin
}

/// A no-connect cross's half size on the page.
fn cross_radius(view: &View) -> f64 {
    (0.7 * view.scale).max(0.6)
}

/// A junction's dot's and a net label's dot's radius on the page.
fn junction_dot(view: &View) -> f64 {
    (0.45 * view.scale).clamp(0.4, 1.0)
}

fn label_dot(view: &View) -> f64 {
    (0.3 * view.scale).clamp(0.3, 0.7)
}

/// A sheet point as a key, to the micrometre.
fn spot([x, y]: [f64; 2]) -> [i64; 2] {
    [(x * 1000.0).round() as i64, (y * 1000.0).round() as i64]
}

/// The unit step a quarter turn points, on the sheet.
pub(super) fn pointing(rotation: u16) -> [f64; 2] {
    match rotation % 360 {
        90 => [0.0, 1.0],
        180 => [-1.0, 0.0],
        270 => [0.0, -1.0],
        _ => [1.0, 0.0],
    }
}

/// The schematic through `view`.
pub fn draw(c: &Ctx, view: &View, d: Detail) -> Drawn {
    let mut items = Vec::new();
    let mut label_ends = BTreeMap::new();
    let mut left_out = Vec::new();
    let pins = Pins::new(c.sheet, c.library);
    let keep = |b: [[f64; 2]; 2]| d.within.is_none_or(|w| near(w, b, 2.0));
    let mut room = Room::new();
    // Wires under the parts.
    for w in &c.sheet.wires {
        let points: Vec<[f64; 2]> = pins.wire(w);
        let Some(b) = solidcraft_doc::ecad::bbox(points.iter().copied()) else { continue };
        if points.len() < 2 || !keep(b) {
            continue;
        }
        let points: Vec<[f64; 2]> = points.iter().map(|p| view.p(*p)).collect();
        for pair in points.windows(2) {
            if let [a, b] = pair {
                room.add(line_box(*a, *b, WIRE));
            }
        }
        items.push(Item::Line { points, closed: false, stroke: Stroke::solid(WIRE, c.pal.wire) });
    }
    room.wires = room.boxes.len();
    // What sits on pin ends (a no-connect cross, a net label's dot, a junction), by its half size
    // on the page, for pin numbers to keep clear of.
    let mut ends: BTreeMap<[i64; 2], f64> = BTreeMap::new();
    let mut sits = |at: [f64; 2], r: f64| {
        let e = ends.entry(spot(at)).or_insert(r);
        *e = e.max(r);
    };
    for m in c.sheet.markers.iter().filter(|m| m.kind == MarkerKind::NoConnect) {
        if let Some(at) = pins.anchor(&m.at) {
            sits(at, cross_radius(view));
        }
    }
    for l in c.sheet.labels.iter().filter(|l| l.kind == LabelKind::Net) {
        if let Some(at) = pins.anchor(&l.at) {
            sits(at, label_dot(view));
        }
    }
    for j in &c.conn.junctions {
        sits(*j, junction_dot(view));
    }
    // Labels whose names would run over each other (ports stacked on neighbouring pins, all
    // pointing along the row) turn to point straight out from their pins, where that is clear.
    let shown: Vec<(&solidcraft_doc::ecad::Label, [f64; 2])> =
        c.sheet.labels.iter().filter_map(|l| pins.anchor(&l.at).map(|at| (l, at))).filter(|(_, at)| keep([*at, *at])).collect();
    let mut turns: Vec<u16> = shown.iter().map(|(l, _)| l.rotation).collect();
    if d.texts && shown.len() <= MAX_TIDY_LABELS {
        let extent = |k: usize, r: u16| shown.get(k).map(|(l, at)| label_extent(c, view, l.kind, &l.name, *at, r));
        let first: Vec<Option<[[f64; 2]; 2]>> = (0..shown.len()).map(|k| extent(k, turns.get(k).copied().unwrap_or(0))).collect();
        let meets = |b: [[f64; 2]; 2], k: usize, boxes: &[Option<[[f64; 2]; 2]>]| {
            boxes.iter().enumerate().any(|(j, o)| j != k && o.is_some_and(|o| boxes_meet(o, b)))
        };
        let crowded: Vec<bool> = first.iter().enumerate().map(|(k, b)| b.is_some_and(|b| meets(b, k, &first))).collect();
        let mut boxes = first.clone();
        for (k, (l, _)) in shown.iter().enumerate() {
            if crowded.get(k) == Some(&true)
                && l.kind != LabelKind::Ground
                && let Some(out) = outward(c, l)
                && let (Some(slot), Some(turn)) = (boxes.get_mut(k), turns.get_mut(k))
            {
                *slot = extent(k, out);
                *turn = out;
            }
        }
        // A turned label that still runs into another goes back.
        for k in 0..shown.len() {
            let (Some(Some(b)), Some(was)) = (boxes.get(k).copied(), shown.get(k).map(|(l, _)| l.rotation)) else { continue };
            if turns.get(k) != Some(&was)
                && meets(b, k, &boxes)
                && let (Some(slot), Some(turn)) = (boxes.get_mut(k), turns.get_mut(k))
            {
                *slot = first.get(k).copied().flatten();
                *turn = was;
            }
        }
    }
    // The room the labels, the symbols' bodies and their pin lines take, for texts that look for
    // a clear spot (a pin name with no room inside its symbol, a "DNP" tag, a cross-reference).
    let parts: Vec<(&Part, &Placeable)> = c
        .sheet
        .parts
        .iter()
        .filter_map(|part| Some((part, c.library.get(&part.component)?)))
        .filter(|(part, pc)| part_bbox(part, pc.symbol).is_some_and(keep))
        .collect();
    if d.texts {
        for ((l, at), rotation) in shown.iter().zip(&turns) {
            room.add(label_extent(c, view, l.kind, &l.name, *at, *rotation));
        }
        room.marks = room.boxes.len();
        for (part, pc) in &parts {
            if let Some(body) = shapes_bbox(pc.symbol.graphics.iter().filter(|g| g.kind != GraphicType::Text)).map(|b| part.placement.bbox(b)) {
                room.add(view.rect(body));
            }
            for q in &pc.symbol.pins {
                let pin = part.placement.pin(q);
                room.add(line_box(view.p(pin.at), view.p(pin.body_end()), PIN));
            }
        }
    }
    for (part, pc) in &parts {
        draw_part(c, view, d, part, pc, &ends, &mut room, &mut items, &mut left_out);
    }
    for ((l, at), rotation) in shown.iter().zip(turns) {
        if let Some(end) = draw_label(c, view, d, l.kind, &l.name, *at, rotation, &mut items) {
            label_ends.insert(l.id, end);
        }
    }
    for m in &c.sheet.markers {
        let Some(at) = pins.anchor(&m.at) else { continue };
        if !keep([at, at]) {
            continue;
        }
        let p = view.p(at);
        let r = cross_radius(view);
        let s = Stroke::solid(PIN, c.pal.dim);
        match m.kind {
            MarkerKind::NoConnect => {
                items.push(Item::Line { points: vec![[p[0] - r, p[1] - r], [p[0] + r, p[1] + r]], closed: false, stroke: s });
                items.push(Item::Line { points: vec![[p[0] - r, p[1] + r], [p[0] + r, p[1] - r]], closed: false, stroke: s });
            }
            MarkerKind::PowerFlag => {
                let h = r * 2.0;
                items.push(Item::Line {
                    points: vec![p, [p[0] - h / 2.0, p[1] - h / 2.0], [p[0], p[1] - h], [p[0] + h / 2.0, p[1] - h / 2.0]],
                    closed: true,
                    stroke: Stroke::solid(PIN, c.pal.power),
                });
            }
        }
    }
    let dot = junction_dot(view);
    for j in &c.conn.junctions {
        if keep([*j, *j]) {
            items.push(Item::Circle { center: view.p(*j), radius: dot, fill: Some(c.pal.wire), stroke: None });
        }
    }
    Drawn { items, label_ends, left_out, room }
}

/// The page boxes things take (wires first, then labels, symbol bodies, pin lines and the texts
/// placed so far), for a text that has no room in its usual spot to look for another. Looking
/// is budgeted: once the budget is spent every spot counts as taken, so a page past it keeps
/// the usual places.
pub(super) struct Room {
    boxes: Vec<[[f64; 2]; 2]>,
    /// How many of the first boxes are wires'.
    wires: usize,
    /// How many of the first boxes are wires' and labels'.
    marks: usize,
    /// A frame's edges and title tab on its page, which nothing printed may run over either.
    edges: Vec<[[f64; 2]; 2]>,
    budget: Cell<usize>,
}

impl Room {
    fn new() -> Self {
        Room { boxes: Vec::new(), wires: 0, marks: 0, edges: Vec::new(), budget: Cell::new(ROOM_BUDGET) }
    }

    pub(super) fn add(&mut self, b: [[f64; 2]; 2]) {
        if self.boxes.len() < MAX_ROOM_BOXES && b.iter().flatten().all(|v| v.is_finite()) {
            self.boxes.push(b);
        }
    }

    /// The frame drawn round the page's sheet (`rect`, with its title `tab`): its edges count as
    /// taken, as wires do.
    pub(super) fn add_frame(&mut self, rect: [[f64; 2]; 2], tab: [[f64; 2]; 2]) {
        let [[x0, y0], [x1, y1]] = rect;
        let h = 0.4;
        for e in [
            [[x0 - h, y0 - h], [x1 + h, y0 + h]],
            [[x0 - h, y1 - h], [x1 + h, y1 + h]],
            [[x0 - h, y0 - h], [x0 + h, y1 + h]],
            [[x1 - h, y0 - h], [x1 + h, y1 + h]],
            tab,
        ] {
            if self.edges.len() < MAX_ROOM_BOXES && e.iter().flatten().all(|v| v.is_finite()) {
                self.edges.push(e);
            }
        }
    }

    /// Whether `b` meets none of the first `n` boxes nor a frame's edge (within the budget).
    fn clear_of(&self, b: [[f64; 2]; 2], n: usize) -> bool {
        let Some(left) = self.budget.get().checked_sub(n.saturating_add(self.edges.len())) else {
            self.budget.set(0);
            return false;
        };
        self.budget.set(left);
        !self.boxes.iter().take(n).chain(&self.edges).any(|o| boxes_meet(*o, b))
    }

    /// Whether nothing taken meets `b`.
    fn clear(&self, b: [[f64; 2]; 2]) -> bool {
        self.clear_of(b, self.boxes.len())
    }

    /// Whether no wire (nor a frame's edge) runs through `b`.
    fn clear_of_wires(&self, b: [[f64; 2]; 2]) -> bool {
        self.clear_of(b, self.wires)
    }

    /// Whether no wire or label (nor a frame's edge) meets `b`.
    fn clear_of_marks(&self, b: [[f64; 2]; 2]) -> bool {
        self.clear_of(b, self.marks)
    }
}

/// The page box round the line from `a` to `b` drawn `width` wide.
fn line_box(a: [f64; 2], b: [f64; 2], width: f64) -> [[f64; 2]; 2] {
    let h = width / 2.0;
    [[a[0].min(b[0]) - h, a[1].min(b[1]) - h], [a[0].max(b[0]) + h, a[1].max(b[1]) + h]]
}

/// How many rows out from a label [`beside_wire`] looks for a gap on each side.
const BESIDE_ROWS: u32 = 3;

/// A net label's or rail's cross-reference `long` millimetres long (its text `size` high) beside
/// its tag or bar, starting by its point (above it, or below when that is taken; a row further
/// out on either side when both are), when straight on it would print over a wire or the frame's
/// edge (a label in the middle of a wire, one a wire passes in front of, a rail by the frame);
/// straight on when that is clear; `None` when nothing near is.
pub(super) fn beside_wire(room: &Room, end: LabelEnd, long: f64, size: f64) -> Option<LabelEnd> {
    let [dx, dy] = end.dir;
    let from = [end.at[0] - dx * end.size * 0.5, end.at[1] - dy * end.size * 0.5];
    let to = [end.at[0] + dx * long, end.at[1] + dy * long];
    if room.clear_of_wires(line_box(from, to, size * 0.6)) {
        return Some(end);
    }
    // Up as the text reads: up the page for level text, left for upright text (it reads upwards).
    let up = if dy == 0.0 { [0.0, -1.0] } else { [-1.0, 0.0] };
    let off = end.half + size * 0.6 + end.size * 0.15;
    for row in 0..BESIDE_ROWS {
        let off = off + f64::from(row) * size * 1.4;
        for side in [1.0, -1.0] {
            let at = [end.tip[0] + up[0] * off * side, end.tip[1] + up[1] * off * side];
            let far = [at[0] + dx * long, at[1] + dy * long];
            if room.clear(line_box(at, far, size * 1.2)) {
                return Some(LabelEnd { at, ..end });
            }
        }
    }
    None
}

/// Where a part's designator or value starts (its lower left) on the sheet, and how tall it is:
/// its own spot, else the symbol's `{designator}` / `{value}` text, else beside the symbol's box
/// (as the schematic shows them).
fn text_anchor(part: &Part, c: &Placeable, placement: &Placement, designator: bool) -> Option<([f64; 2], f64)> {
    let key = if designator { "{designator}" } else { "{value}" };
    let placeholder = c.symbol.graphics.iter().find(|g| g.kind == GraphicType::Text && g.text == key);
    let height = placeholder.and_then(|g| g.height).unwrap_or(TEXT);
    let own = if designator { part.designator_at } else { part.value_at };
    if let Some(o) = own {
        return Some(([placement.at[0] + o[0], placement.at[1] + o[1]], height));
    }
    let [lo, hi] = c.symbol.bbox().map(|b| placement.bbox(b))?;
    if !placement.rotation.is_multiple_of(180) {
        let middle = (lo[1] + hi[1]) / 2.0;
        let y = if designator { middle + 0.3 } else { middle - 0.3 - height };
        return Some(([hi[0] + 0.6, y], height));
    }
    if let Some(at) = placeholder.and_then(|g| g.at).map(|a| placement.apply(a)) {
        return Some((at, height));
    }
    Some(([hi[0] + 0.6, hi[1] + if designator { 0.4 } else { -1.4 }], height))
}

#[allow(clippy::too_many_arguments)]
fn draw_part(
    c: &Ctx,
    view: &View,
    d: Detail,
    part: &Part,
    pc: &Placeable,
    ends: &BTreeMap<[i64; 2], f64>,
    room: &mut Room,
    out: &mut Vec<Item>,
    left_out: &mut Vec<([f64; 2], String)>,
) {
    let placement = part.placement;
    let ink = c.pal.ink;
    let value = part.value.clone().unwrap_or_else(|| pc.component.value.clone());
    for g in pc.symbol.graphics.iter().filter(|g| !(g.kind == GraphicType::Text && (g.text == "{designator}" || g.text == "{value}"))) {
        let g = placement.graphic(g);
        graphic(c, view, d, &g, &part.designator, &value, out);
    }
    let pins: Vec<_> = pc.symbol.pins.iter().map(|q| placement.pin(q)).collect();
    for pin in &pins {
        out.push(Item::Line { points: vec![view.p(pin.at), view.p(pin.body_end())], closed: false, stroke: Stroke::solid(PIN, ink) });
    }
    let body = shapes_bbox(pc.symbol.graphics.iter().filter(|g| g.kind != GraphicType::Text)).map(|b| view.rect(placement.bbox(b)));
    // Not fitted: struck through, corner to corner of its body.
    if part.dnp
        && let Some([a, b]) = body
    {
        let pad = 0.4 * view.scale;
        out.extend(super::parts::cross([[a[0] - pad, a[1] - pad], [b[0] + pad, b[1] + pad]], Stroke::solid(0.3, c.pal.warn)));
    }
    if !d.texts {
        return;
    }
    let named = pins.iter().any(|p| p.shows_name());
    let numbers = pins.len() > 2 || named;
    // Sheet boxes taken by pin names, so names that would run into each other (pins near a
    // corner) are set smaller or cut short.
    let mut taken: Vec<[[f64; 2]; 2]> = Vec::new();
    let mut names: Vec<(bool, &Pin, Text)> = Vec::new();
    for pin in &pins {
        let b = pin.body_end();
        let level = matches!(pin.direction, PinDirection::Left | PinDirection::Right);
        if numbers {
            // Over the pin, from the body outwards, clear of what sits on the pin's end: a number
            // too long for the room a no-connect cross or a dot leaves there rises over it.
            let size = c.size(view, PIN_NUMBER);
            let lift = 0.35 * view.scale;
            let lift = match ends.get(&spot(pin.at)) {
                Some(r) if c.font.width(&pin.number, size) > (pin.length - 0.3) * view.scale - r - 0.2 => lift.max(r + 0.25),
                _ => lift,
            };
            let [ux, uy] = pin.direction.unit();
            let near = view.p([b[0] - ux * 0.3, b[1] - uy * 0.3]);
            let t = match pin.direction {
                PinDirection::Right => {
                    Text::new([near[0], near[1] - lift], pin.number.clone(), size, c.pal.dim).align(Align::End, Baseline::Baseline)
                }
                PinDirection::Left => {
                    Text::new([near[0], near[1] - lift], pin.number.clone(), size, c.pal.dim).align(Align::Start, Baseline::Baseline)
                }
                PinDirection::Down => {
                    Text::new([near[0] - lift, near[1]], pin.number.clone(), size, c.pal.dim).align(Align::Start, Baseline::Baseline).turned(90.0)
                }
                PinDirection::Up => {
                    Text::new([near[0] - lift, near[1]], pin.number.clone(), size, c.pal.dim).align(Align::End, Baseline::Baseline).turned(90.0)
                }
            };
            out.push(Item::Text(t));
        }
        if pin.shows_name() {
            let size = c.size(view, PIN_NAME);
            let [ux, uy] = pin.direction.unit();
            let start = view.p([b[0] + ux * 0.6, b[1] + uy * 0.6]);
            let (align, rotation) = match pin.direction {
                PinDirection::Right => (Align::Start, 0.0),
                PinDirection::Left => (Align::End, 0.0),
                PinDirection::Up => (Align::Start, 90.0),
                PinDirection::Down => (Align::End, 90.0),
            };
            names.push((level, pin, Text::new(start, pin.name.clone(), size, ink).align(align, Baseline::Middle).turned(rotation)));
        }
    }
    // Pin lines and their numbers (over a level pin, left of an upright one), for the
    // designator, the value and names set outside the body to keep clear of.
    let number = if numbers { (PIN_NUMBER * view.scale).max(MIN_TEXT) + 0.35 * view.scale } else { 0.3 };
    let pin_boxes: Vec<[[f64; 2]; 2]> = pins
        .iter()
        .map(|p| {
            let (a, b) = (view.p(p.at), view.p(p.body_end()));
            let (x0, x1, y0, y1) = (a[0].min(b[0]), a[0].max(b[0]), a[1].min(b[1]), a[1].max(b[1]));
            if matches!(p.direction, PinDirection::Left | PinDirection::Right) {
                [[x0 - 0.2, y0 - number], [x1 + 0.2, y1 + 0.3]]
            } else {
                [[x0 - number, y0 - 0.2], [x1 + 0.3, y1 + 0.2]]
            }
        })
        .collect();
    // Level names first: they are most of a symbol's; upright ones give way.
    names.sort_by_key(|(level, _, _)| !*level);
    let mut shown: Vec<String> = Vec::new();
    let mut missing: Vec<(&str, String)> = Vec::new();
    for (_, pin, mut t) in names {
        let number = pin.number.as_str();
        let mut fits = false;
        for size in [t.size, (t.size * 0.8).max(MIN_TEXT), MIN_TEXT] {
            t.size = size;
            let b = text_box(c, &t);
            if !taken.iter().any(|o| boxes_meet(*o, b)) {
                taken.push(b);
                fits = true;
                break;
            }
        }
        // No room along the pin inside, and no other pin shows the name: an upright pin's name
        // level by the body's edge, inside it; else along the pin outside the body, on the side
        // away from its number, where that is clear of the symbol's other names and pins and of
        // what the page has.
        if !fits
            && !shown.contains(&t.text)
            && let Some(o) =
                body.and_then(|b| name_across(c, view, pin, &t, &taken, b)).or_else(|| name_outside(c, view, pin, &t, &taken, &pin_boxes, room))
        {
            taken.push(text_box(c, &o));
            t = o;
            fits = true;
        }
        // No room at any size: cut short to the room there is (an ellipsis says so), keeping
        // half its letters at least (two at least), or a cut would say nothing.
        let full = t.text.clone();
        let wide = c.font.width(&full, t.size);
        let letters = solidcraft_doc::ecad::overbar::plain(&full).chars().count();
        let least = letters.div_ceil(2).max(2);
        for k in (1..NAME_CUTS).rev() {
            if fits {
                break;
            }
            let cut = c.font.fit(&full, t.size, wide * k as f64 / NAME_CUTS as f64);
            if solidcraft_doc::ecad::overbar::plain(&cut).chars().count() < least.saturating_add(1) {
                break;
            }
            t.text = cut;
            let b = text_box(c, &t);
            if !taken.iter().any(|o| boxes_meet(*o, b)) {
                taken.push(b);
                fits = true;
            }
        }
        if fits {
            shown.push(full);
            out.push(Item::Text(t));
        } else {
            missing.push((number, full));
        }
    }
    // Left out: said in the export's warnings, unless another pin of the part shows that name
    // (a second GND).
    let centre = part_bbox(part, pc.symbol).map_or(part.placement.at, |[a, b]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
    for (number, name) in missing.into_iter().filter(|(_, n)| !shown.contains(n)) {
        left_out.push((centre, format!("{}.{number} {}", part.designator, solidcraft_doc::ecad::overbar::plain(&name))));
    }
    for b in pin_boxes.iter().chain(&taken) {
        room.add(*b);
    }
    // The designator and the value; a part not fitted has a "DNP" tag after its designator,
    // which counts as the designator's while it slides off the pins.
    let mut placed: Vec<(bool, Text)> = Vec::new();
    for designator in [true, false] {
        let text = if designator { part.designator.clone() } else { value.clone() };
        if text.is_empty() {
            continue;
        }
        let Some((at, height)) = text_anchor(part, pc, &placement, designator) else { continue };
        let colour = if designator { ink } else { c.pal.dim };
        let mut t = Text::new(view.p(at), text, c.size(view, height), colour).align(Align::Start, Baseline::Bottom);
        if designator {
            t.weight = Weight::Bold;
        }
        let tag = if designator && part.dnp { super::parts::dnp_tag([0.0, 0.0], t.size, c.pal, c.font).1 + t.size * 0.35 } else { 0.0 };
        // Slide it off pins it would sit on (a symbol's text over its bottom pins): left or
        // right of them, whichever moves it less.
        let start = t.at[0];
        for _ in 0..4 {
            let mut b = text_box(c, &t);
            b[1][0] += tag;
            let hit: Vec<&[[f64; 2]; 2]> = pin_boxes.iter().filter(|p| boxes_meet(**p, b)).collect();
            let (Some(left), Some(right)) = (hit.iter().map(|p| p[0][0]).reduce(f64::min), hit.iter().map(|p| p[1][0]).reduce(f64::max)) else {
                break;
            };
            let width = b[1][0] - b[0][0];
            let (to_left, to_right) = (left - 0.6 - width, right + 0.6);
            t.at[0] = if (start - to_left).abs() < (to_right - start).abs() { to_left } else { to_right };
        }
        // A text the user did not place that still sits on a wire or a label goes to the nearest
        // spot close by that is clear of what the page has (as the schematic canvas does): a
        // step or two along costs less than one up or down.
        let own = if designator { part.designator_at.is_some() } else { part.value_at.is_some() };
        let mut b = text_box(c, &t);
        b[1][0] += tag;
        if !own && !room.clear_of_marks(b) {
            let step = (t.size * 0.6).max(0.5);
            let mut spots: Vec<(i32, i32)> = (-6..=6).flat_map(|kx| (-3..=3).map(move |ky| (kx, ky))).filter(|k| *k != (0, 0)).collect();
            spots.sort_by_key(|(kx, ky)| 2 * kx.abs() + 3 * ky.abs());
            let clear = |[dx, dy]: [f64; 2]| {
                let moved = [[b[0][0] + dx, b[0][1] + dy], [b[1][0] + dx, b[1][1] + dy]];
                !pin_boxes.iter().any(|p| boxes_meet(*p, moved)) && room.clear(moved)
            };
            if let Some([dx, dy]) = spots.into_iter().map(|(kx, ky)| [f64::from(kx) * step, f64::from(ky) * step]).find(|d| clear(*d)) {
                t.at = [t.at[0] + dx, t.at[1] + dy];
            }
        }
        room.add(text_box(c, &t));
        placed.push((designator, t));
    }
    // Not fitted: the tag after the designator, else after the value, over the designator or
    // before it, whichever is clear first (of the page's wires, labels, symbols and texts).
    let designator = placed.iter().find(|(d, _)| *d).map(|(_, t)| text_box(c, t));
    if part.dnp
        && let Some(db) = designator
    {
        let size = placed.iter().find(|(d, _)| *d).map_or(MIN_TEXT, |(_, t)| t.size);
        let (_, w) = super::parts::dnp_tag([0.0, 0.0], size, c.pal, c.font);
        let h = super::parts::dnp_tag_height(size);
        let middle = |b: [[f64; 2]; 2]| (b[0][1] + b[1][1]) / 2.0;
        let gap = size * 0.35;
        let mut spots = vec![[db[1][0] + gap, middle(db)]];
        if let Some(vb) = placed.iter().find(|(d, _)| !*d).map(|(_, t)| text_box(c, t)) {
            spots.push([vb[1][0] + gap, middle(vb)]);
        }
        spots.push([db[0][0], db[0][1] - gap * 0.5 - h / 2.0]);
        spots.push([db[0][0] - gap - w, middle(db)]);
        let tag_box = |at: [f64; 2]| [[at[0], at[1] - h / 2.0], [at[0] + w, at[1] + h / 2.0]];
        // Clear by a little, so the tag does not touch a neighbour's text (less than the gap over
        // the designator, which it may sit that close to).
        let margin = gap * 0.4;
        let clear = |at: [f64; 2]| {
            let [a, b] = tag_box(at);
            room.clear([[a[0] - margin, a[1] - margin], [b[0] + margin, b[1] + margin]])
        };
        let at = spots.iter().copied().find(|at| clear(*at)).or(spots.first().copied()).unwrap_or(db[1]);
        room.add(tag_box(at));
        out.extend(super::parts::dnp_tag(at, size, c.pal, c.font).0);
    }
    out.extend(placed.into_iter().map(|(_, t)| Item::Text(t)));
}

/// An upright pin's name with no room along the pin inside its symbol, set level instead (at its
/// size, else the least), centred on the pin just inside the body's edge; `None` for a level pin,
/// or when that meets the symbol's other names (`taken`) or leaves the body (`body`, on the page).
fn name_across(c: &Ctx, view: &View, pin: &Pin, t: &Text, taken: &[[[f64; 2]; 2]], body: [[f64; 2]; 2]) -> Option<Text> {
    let edge = view.p(pin.body_end());
    let (at, baseline) = match pin.direction {
        // Below the body (it runs up to it): over the bottom edge.
        PinDirection::Up => ([edge[0], edge[1] - 0.5], Baseline::Bottom),
        PinDirection::Down => ([edge[0], edge[1] + 0.5], Baseline::Top),
        PinDirection::Left | PinDirection::Right => return None,
    };
    let mut sizes = vec![t.size];
    if t.size > MIN_TEXT {
        sizes.push(MIN_TEXT);
    }
    let [lo, hi] = body;
    sizes.into_iter().find_map(|size| {
        let o = Text::new(at, t.text.clone(), size, t.color).align(Align::Middle, baseline);
        let [a, b] = text_box(c, &o);
        let within = a[0] >= lo[0] + 0.3 && b[0] <= hi[0] - 0.3 && a[1] >= lo[1] && b[1] <= hi[1];
        let free = within && !taken.iter().any(|q| boxes_meet(*q, [a, b]));
        free.then_some(o)
    })
}

/// A pin name with no room inside its symbol, set along its pin outside the body instead (at its
/// size, else the least): beside the pin line on the side away from its number (under a level
/// pin, right of an upright one), from the body outwards; `None` when that meets the symbol's
/// other names (`taken`), its pins and their numbers, or anything else on the page.
fn name_outside(c: &Ctx, view: &View, pin: &Pin, t: &Text, taken: &[[[f64; 2]; 2]], pin_boxes: &[[[f64; 2]; 2]], room: &Room) -> Option<Text> {
    let body = view.p(pin.body_end());
    let gap = 0.5;
    let side = 0.45;
    let (at, align, rotation) = match pin.direction {
        // A pin left of the body (it runs right to it): the name ends by the body.
        PinDirection::Right => ([body[0] - gap, body[1] + side], Align::End, 0.0),
        PinDirection::Left => ([body[0] + gap, body[1] + side], Align::Start, 0.0),
        // Below the body (it runs up to it): the name reads upwards and ends by the body.
        PinDirection::Up => ([body[0] + side, body[1] + gap], Align::End, 90.0),
        PinDirection::Down => ([body[0] + side, body[1] - gap], Align::Start, 90.0),
    };
    let mut sizes = vec![t.size];
    if t.size > MIN_TEXT {
        sizes.push(MIN_TEXT);
    }
    sizes.into_iter().find_map(|size| {
        let o = Text::new(at, t.text.clone(), size, t.color).align(align, Baseline::Top).turned(rotation);
        let b = text_box(c, &o);
        let free = !taken.iter().chain(pin_boxes).any(|q| boxes_meet(*q, b)) && room.clear(b);
        free.then_some(o)
    })
}

/// The page box round a text (level, or upright reading upwards).
fn text_box(c: &Ctx, t: &Text) -> [[f64; 2]; 2] {
    let (o, along, down) = c.font.placement(t);
    let w = c.font.width(&t.text, t.size);
    let (up, dn) = (c.font.ascent * t.size * 0.85, c.font.descent * t.size);
    let corners = [(0.0, -up), (w, -up), (w, dn), (0.0, dn)].map(|(u, v)| [o[0] + along[0] * u + down[0] * v, o[1] + along[1] * u + down[1] * v]);
    let xs = corners.map(|p| p[0]);
    let ys = corners.map(|p| p[1]);
    [
        [xs.iter().copied().fold(f64::INFINITY, f64::min), ys.iter().copied().fold(f64::INFINITY, f64::min)],
        [xs.iter().copied().fold(f64::NEG_INFINITY, f64::max), ys.iter().copied().fold(f64::NEG_INFINITY, f64::max)],
    ]
}

fn boxes_meet([a0, a1]: [[f64; 2]; 2], [b0, b1]: [[f64; 2]; 2]) -> bool {
    a0[0] < b1[0] && b0[0] < a1[0] && a0[1] < b1[1] && b0[1] < a1[1]
}

/// How finely a pin name with no room is cut short: to 7/8 of its length, 6/8, and so on.
const NAME_CUTS: usize = 8;

/// One symbol graphic, placed on the sheet.
fn graphic(c: &Ctx, view: &View, d: Detail, g: &Graphic, designator: &str, value: &str, out: &mut Vec<Item>) {
    let ink = c.pal.ink;
    let width = (g.width * view.scale).clamp(BODY, 0.45);
    let stroke = Stroke::solid(width, ink);
    let fill = g.fill.then(|| ink.mix(c.pal.background, 0.65));
    match g.kind {
        GraphicType::Line => {
            if g.points.len() >= 2 {
                out.push(Item::Line { points: g.points.iter().map(|p| view.p(*p)).collect(), closed: false, stroke });
            }
        }
        GraphicType::Polygon => {
            let points: Vec<[f64; 2]> = g.points.iter().map(|p| view.p(*p)).collect();
            match fill {
                Some(f) => out.push(Item::Area { rings: vec![points], fill: f, stroke: Some(stroke) }),
                None => out.push(Item::Line { points, closed: true, stroke }),
            }
        }
        GraphicType::Rect => {
            let (Some(a), Some(b)) = (g.from, g.to) else { return };
            // Symbol bodies get the page's light body fill.
            out.push(Item::Rect { rect: view.rect([a, b]), radius: 0.0, fill: Some(fill.unwrap_or(c.pal.body)), stroke: Some(stroke) });
        }
        GraphicType::Circle => {
            let (Some(center), Some(r)) = (g.center, g.radius) else { return };
            out.push(Item::Circle { center: view.p(center), radius: r * view.scale, fill, stroke: Some(stroke) });
        }
        GraphicType::Arc => {
            let points: Vec<[f64; 2]> = g.arc_points(32).iter().map(|p| view.p(*p)).collect();
            if points.len() >= 2 {
                out.push(Item::Line { points, closed: false, stroke });
            }
        }
        GraphicType::Text => {
            if !d.texts {
                return;
            }
            let (Some(at), Some(h)) = (g.at, g.height) else { return };
            let text = g.text.replace("{designator}", designator).replace("{value}", value);
            out.push(Item::Text(Text::new(view.p(at), text, c.size(view, h), ink).align(Align::Start, Baseline::Bottom).turned(g.rotation)));
        }
    }
}

/// A label at `at` pointing `rotation`: a net label as a tag round its name, a rail as a bar on
/// a stem with its name beyond, ground as three shrinking bars. Returns where its name ends.
#[allow(clippy::too_many_arguments)]
pub fn draw_label(
    c: &Ctx,
    view: &View,
    d: Detail,
    kind: LabelKind,
    name: &str,
    at: [f64; 2],
    rotation: u16,
    out: &mut Vec<Item>,
) -> Option<LabelEnd> {
    let s = pointing(rotation);
    let p = view.p(at);
    // On the page (y down).
    let dir = [s[0], -s[1]];
    let across = [-dir[1], dir[0]];
    let level = dir[1] == 0.0;
    let size = c.size(view, TEXT);
    let colour = if kind == LabelKind::Net { c.pal.label } else { c.pal.power };
    let off = |q: [f64; 2], along: f64, side: f64| [q[0] + dir[0] * along + across[0] * side, q[1] + dir[1] * along + across[1] * side];
    // Text along `dir` from `start`: level text runs right, upright text reads upwards, so a run
    // pointing left or down ends at `start` instead.
    let run = |start: [f64; 2], text: &str, size: f64, colour: Rgb| {
        let forward = dir[0] > 0.0 || dir[1] < 0.0;
        let align = if forward { Align::Start } else { Align::End };
        Text::new(start, text, size, colour).align(align, Baseline::Middle).turned(if level { 0.0 } else { 90.0 })
    };
    match kind {
        LabelKind::Net => {
            out.push(Item::Circle { center: p, radius: label_dot(view), fill: Some(colour), stroke: None });
            if !d.texts {
                return None;
            }
            let gap = 0.5 * view.scale;
            let w = c.font.width(name, size);
            let h = size * 1.35;
            // The tag: a box round the name, its pin end pointed.
            let tip = off(p, gap, 0.0);
            let base = off(tip, h / 2.0, 0.0);
            let start = off(base, size * 0.15, 0.0);
            let end = off(start, w + size * 0.4, 0.0);
            let ring = vec![tip, off(base, 0.0, -h / 2.0), off(end, 0.0, -h / 2.0), off(end, 0.0, h / 2.0), off(base, 0.0, h / 2.0)];
            out.push(Item::Area { rings: vec![ring], fill: c.pal.background, stroke: Some(Stroke::solid(0.2, colour)) });
            out.push(Item::Text(run(start, name, size, colour)));
            Some(LabelEnd { at: off(end, size * 0.5, 0.0), dir, rotation: if level { 0.0 } else { 90.0 }, size, tip, half: h / 2.0 })
        }
        LabelKind::Rail => {
            let stem = 1.0 * view.scale;
            let top = off(p, stem, 0.0);
            let bar = 0.9 * view.scale;
            let st = Stroke::solid(PIN + 0.05, colour);
            out.push(Item::Line { points: vec![p, top], closed: false, stroke: st });
            out.push(Item::Line { points: vec![off(top, 0.0, -bar), off(top, 0.0, bar)], closed: false, stroke: Stroke::solid(0.35, colour) });
            if !d.texts {
                return None;
            }
            let start = off(top, 0.5 * view.scale, 0.0);
            out.push(Item::Text(run(start, name, size, colour).bold()));
            let w = c.font.width(name, size);
            Some(LabelEnd {
                at: off(start, w + size * 0.5, 0.0),
                dir,
                rotation: if level { 0.0 } else { 90.0 },
                size,
                tip: p,
                half: bar.max(size * 0.6),
            })
        }
        LabelKind::Ground => {
            let stem = 0.7 * view.scale;
            let base = off(p, stem, 0.0);
            let st = Stroke::solid(PIN + 0.05, colour);
            out.push(Item::Line { points: vec![p, base], closed: false, stroke: st });
            for (i, w) in [1.0f64, 0.62, 0.25].into_iter().enumerate() {
                let q = off(base, i as f64 * 0.38 * view.scale, 0.0);
                let half = w * view.scale;
                out.push(Item::Line { points: vec![off(q, 0.0, -half), off(q, 0.0, half)], closed: false, stroke: Stroke::solid(0.3, colour) });
            }
            // A cross-reference (when ground is cross-referenced) goes on past the last bar.
            let end = off(base, 2.0 * 0.38 * view.scale + size * 0.6, 0.0);
            d.texts.then_some(LabelEnd { at: end, dir, rotation: if level { 0.0 } else { 90.0 }, size, tip: p, half: view.scale })
        }
    }
}

/// The way straight out from the pin a label sits on (away from its part's body).
fn outward(c: &Ctx, l: &solidcraft_doc::ecad::Label) -> Option<u16> {
    let (part, number) = l.at.pin()?;
    let p = c.sheet.part(part)?;
    let pc = c.library.get(&p.component)?;
    let pin = pc.symbol.pins.iter().find(|q| q.number == number).map(|q| p.placement.pin(q))?;
    Some(match pin.direction {
        PinDirection::Right => 180,
        PinDirection::Left => 0,
        PinDirection::Up => 270,
        PinDirection::Down => 90,
    })
}

/// The page box a label takes pointing `rotation`, its name included (as [`draw_label`] draws it).
fn label_extent(c: &Ctx, view: &View, kind: LabelKind, name: &str, at: [f64; 2], rotation: u16) -> [[f64; 2]; 2] {
    let s = pointing(rotation);
    let p = view.p(at);
    let dir = [s[0], -s[1]];
    let across = [-dir[1], dir[0]];
    let size = c.size(view, TEXT);
    let w = c.font.width(name, size);
    let k = view.scale;
    let (along, half) = match kind {
        LabelKind::Net => (0.5 * k + size * 1.35 / 2.0 + size * 0.55 + w, size * 0.7),
        LabelKind::Rail => (1.5 * k + w, (0.9 * k).max(size * 0.6)),
        LabelKind::Ground => (1.6 * k, k),
    };
    let a = [p[0] - across[0] * half, p[1] - across[1] * half];
    let b = [p[0] + dir[0] * along + across[0] * half, p[1] + dir[1] * along + across[1] * half];
    [[a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]]
}

/// A frame as the sheet shows it: a dashed rounded rectangle, its title in a tab at the top left,
/// and the page it prints on beside it. Returns the tab's page box.
pub fn draw_frame(c: &Ctx, view: &View, f: &Frame, page_no: Option<usize>, title_size: f64, out: &mut Vec<Item>) -> [[f64; 2]; 2] {
    let rect = view.rect(f.rect);
    let line = Stroke::dashed(0.35, c.pal.zone, [2.2, 1.2]);
    out.push(Item::Rect { rect, radius: 1.5, fill: None, stroke: Some(line) });
    let label = match page_no {
        Some(n) => format!("{}  ·  p. {n}", f.title),
        None => f.title.clone(),
    };
    let pad = title_size * 0.45;
    let w = c.font.width(&label, title_size) + 2.0 * pad;
    let h = title_size * 1.55;
    let tab = [[rect[0][0], rect[0][1] - h * 0.5], [rect[0][0] + w, rect[0][1] + h * 0.5]];
    out.push(Item::Rect {
        rect: tab,
        radius: 0.8,
        fill: Some(c.pal.zone.mix(c.pal.background, 0.82)),
        stroke: Some(Stroke::solid(0.25, c.pal.zone)),
    });
    let mut t = Text::new([tab[0][0] + pad, rect[0][1]], label, title_size, c.pal.ink).align(Align::Start, Baseline::Middle);
    t.weight = Weight::Bold;
    out.push(Item::Text(t));
    tab
}
