//! Wire shapes: the route a new wire takes between two points, how wires and what stands on them
//! hang on when what they are attached to moves (so a move never pulls a connection apart), and
//! which way a power port points. Pure functions, shared by the commands and the sheet editor's
//! previews, so a preview shows what the command will do.

use std::collections::{BTreeMap, BTreeSet};

use super::PinDirection;
use super::place::{GRID, Motion, lattice, snap, snap_point};
use super::schematic::{Anchor, Label, LabelKind, Library, MAX_WIRE_POINTS, Pins, Schematic, Wire};

/// How far an auto-routed wire runs straight out of a pin before it turns, in millimetres.
pub const STUB: f64 = GRID;

/// One end of a wire to route: where it is and, at a pin, the way a wire leaves it (away from the
/// body).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RouteEnd {
    pub at: [f64; 2],
    pub out: Option<PinDirection>,
}

/// The opposite way: a wire leaves a pin opposite to the way the pin runs toward the body (as
/// [`Placement::direction`](super::Placement::direction) gives it on the sheet).
pub fn opposite(d: PinDirection) -> PinDirection {
    match d {
        PinDirection::Right => PinDirection::Left,
        PinDirection::Left => PinDirection::Right,
        PinDirection::Up => PinDirection::Down,
        PinDirection::Down => PinDirection::Up,
    }
}

/// A direction as a label's turn: degrees counter-clockwise from +x.
pub fn angle(d: PinDirection) -> u16 {
    match d {
        PinDirection::Right => 0,
        PinDirection::Up => 90,
        PinDirection::Left => 180,
        PinDirection::Down => 270,
    }
}

fn unit(d: Option<PinDirection>) -> [f64; 2] {
    d.map_or([0.0, 0.0], PinDirection::unit)
}

fn dot([ax, ay]: [f64; 2], [bx, by]: [f64; 2]) -> f64 {
    ax * bx + ay * by
}

fn sub([ax, ay]: [f64; 2], [bx, by]: [f64; 2]) -> [f64; 2] {
    [ax - bx, ay - by]
}

fn horizontal(d: PinDirection) -> bool {
    matches!(d, PinDirection::Left | PinDirection::Right)
}

/// Between `a` and `b` on the grid, or halfway when no grid line lies strictly between them.
fn middle(a: f64, b: f64) -> f64 {
    let m = snap(((a + b) / 2.0 / GRID).round() * GRID);
    if m > a.min(b) && m < a.max(b) { m } else { snap((a + b) / 2.0) }
}

/// Whether `p` is strictly inside one of `boxes`.
fn inside_any(p: [f64; 2], boxes: &[[[f64; 2]; 2]]) -> bool {
    boxes.iter().any(|[[x0, y0], [x1, y1]]| p[0] > *x0 && p[0] < *x1 && p[1] > *y0 && p[1] < *y1)
}

/// Whether the leg `from` → `to` runs back along `u` (the way the wire was going).
fn doubles_back(from: [f64; 2], to: [f64; 2], u: [f64; 2]) -> bool {
    let d = sub(to, from);
    u != [0.0, 0.0] && dot(d, u) < 0.0 && (d[0] == 0.0 || d[1] == 0.0)
}

/// Whether `b` lies on the axis-parallel line through `a` and `c`, between them (so it is no bend).
fn between(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> bool {
    let within = |p: i64, q: i64, r: i64| q >= p.min(r) && q <= p.max(r);
    (a[1] == b[1] && b[1] == c[1] && within(a[0], b[0], c[0])) || (a[0] == b[0] && b[0] == c[0] && within(a[1], b[1], c[1]))
}

/// The points on the lattice, with repeats and points that are no bend dropped.
pub fn simplify(points: impl IntoIterator<Item = [f64; 2]>) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = Vec::new();
    for p in points {
        let p = snap_point(p);
        if out.last().is_some_and(|l| lattice(*l) == lattice(p)) {
            continue;
        }
        out.push(p);
        loop {
            let n = out.len();
            if n < 3 {
                break;
            }
            match (out.get(n - 3).copied(), out.get(n - 2).copied(), out.get(n - 1).copied()) {
                (Some(a), Some(b), Some(c)) if between(lattice(a), lattice(b), lattice(c)) => {
                    out.truncate(n - 2);
                    out.push(c);
                }
                _ => break,
            }
        }
    }
    out
}

/// The auto route from `a` to `b`, both ends included, every point on the lattice:
/// - one straight segment when they line up on an axis and neither pin faces away;
/// - for pins facing each other, a Z through the middle on the 2.54 mm grid;
/// - otherwise a [`STUB`] straight out of each pin, then an L between the stubs' ends, horizontal
///   first unless that corner falls inside one of `boxes` (the parts' boxes) or doubles back
///   along a stub.
///
/// No obstacle is routed round.
pub fn auto_route(a: RouteEnd, b: RouteEnd, boxes: &[[[f64; 2]; 2]]) -> Vec<[f64; 2]> {
    let (pa, pb) = (snap_point(a.at), snap_point(b.at));
    let (qa, qb) = (lattice(pa), lattice(pb));
    if qa == qb {
        return vec![pa];
    }
    let (ua, ub) = (unit(a.out), unit(b.out));
    let toward = |u: [f64; 2], from: [f64; 2], to: [f64; 2]| u == [0.0, 0.0] || dot(u, sub(to, from)) > 0.0;
    if (qa[0] == qb[0] || qa[1] == qb[1]) && toward(ua, pa, pb) && toward(ub, pb, pa) {
        return vec![pa, pb];
    }
    if let (Some(da), Some(db)) = (a.out, b.out)
        && db == opposite(da)
        && dot(ua, sub(pb, pa)) > 0.0
    {
        let path = if horizontal(da) {
            let x = middle(pa[0], pb[0]);
            [pa, [x, pa[1]], [x, pb[1]], pb]
        } else {
            let y = middle(pa[1], pb[1]);
            [pa, [pa[0], y], [pb[0], y], pb]
        };
        return simplify(path);
    }
    let sa = snap_point([pa[0] + ua[0] * STUB, pa[1] + ua[1] * STUB]);
    let sb = snap_point([pb[0] + ub[0] * STUB, pb[1] + ub[1] * STUB]);
    let cost =
        |c: [f64; 2]| u32::from(inside_any(c, boxes)) * 2 + u32::from(doubles_back(sa, c, ua)) + u32::from(doubles_back(c, sb, [-ub[0], -ub[1]]));
    let (h, v) = ([sb[0], sa[1]], [sa[0], sb[1]]);
    let corner = if cost(h) <= cost(v) { h } else { v };
    simplify([pa, sa, corner, sb, pb])
}

/// The auto route through `ends` in order, leg by leg (see [`auto_route`]); usually only the
/// first and last are pins.
pub fn auto_route_through(ends: &[RouteEnd], boxes: &[[[f64; 2]; 2]]) -> Vec<[f64; 2]> {
    let mut points = Vec::new();
    for pair in ends.windows(2) {
        if let [a, b] = pair {
            points.extend(auto_route(*a, *b, boxes));
        }
    }
    simplify(points)
}

/// The bend next to an end that moved (`end`, from `was` to `now`), slid so the leg between them
/// keeps its axis: across a horizontal leg it takes the end's new y, along a vertical one its x.
fn keep_axis(was: [f64; 2], next_was: [f64; 2], now: [f64; 2], next: &mut [f64; 2]) {
    let (e, n) = (lattice(was), lattice(next_was));
    if e[1] == n[1] && e[0] != n[0] {
        next[1] = now[1];
    } else if e[0] == n[0] && e[1] != n[1] {
        next[0] = now[0];
    }
}

/// The bend a straight two-point wire gains when its end `odd` moves apart from the other
/// (`rest`), so the leg at `odd` keeps its axis; `None` when it needs none.
fn elbow(odd_was: [f64; 2], rest_was: [f64; 2], odd: [f64; 2], rest: [f64; 2]) -> Option<[f64; 2]> {
    let (o, r) = (lattice(odd), lattice(rest));
    if o[0] == r[0] || o[1] == r[1] {
        return None;
    }
    let (ow, rw) = (lattice(odd_was), lattice(rest_was));
    if ow[1] == rw[1] && ow[0] != rw[0] {
        Some(snap_point([rest[0], odd[1]]))
    } else if ow[0] == rw[0] && ow[1] != rw[1] {
        Some(snap_point([odd[0], rest[1]]))
    } else {
        None
    }
}

/// A wire's points (`was`, ends included) when its ends go to `ends` (`moving` says which of them
/// moved with the move `m`) and, with `rigid`, its bends move by `m` too: the bend next to an end
/// that moved apart from the rest slides so its leg keeps its axis ([`keep_axis`]). Returns the
/// points one for one with `was`, and the elbow a straight wire gains ([`elbow`]).
fn slide(was: &[[f64; 2]], [nf, nt]: [[f64; 2]; 2], [mf, mt]: [bool; 2], rigid: bool, m: &Motion) -> (Vec<[f64; 2]>, Option<[f64; 2]>) {
    let (Some(&of), Some(&ot)) = (was.first(), was.last()) else { return (Vec::new(), None) };
    let n = was.len();
    let mut now: Vec<[f64; 2]> = was.iter().map(|p| if rigid { m.point(*p) } else { *p }).collect();
    if let Some(p) = now.first_mut() {
        *p = nf;
    }
    if let Some(p) = now.last_mut() {
        *p = nt;
    }
    let (odd_from, odd_to) = (mf != rigid, mt != rigid);
    if n <= 2 {
        let bend = match (odd_from, odd_to) {
            (true, _) => elbow(of, ot, nf, nt),
            (false, true) => elbow(ot, of, nt, nf),
            (false, false) => None,
        };
        return (now, bend);
    }
    if odd_from && let (Some(&next), Some(v)) = (was.get(1), now.get_mut(1)) {
        keep_axis(of, next, nf, v);
    }
    let before_last = n.saturating_sub(2);
    if odd_to && let (Some(&prev), Some(v)) = (was.get(before_last), now.get_mut(before_last)) {
        keep_axis(ot, prev, nt, v);
    }
    (now, None)
}

/// `now` (as [`slide`] gives it) with the elbow put in after the first point, tidied.
fn joined(now: &[[f64; 2]], bend: Option<[f64; 2]>) -> Vec<[f64; 2]> {
    let (head, tail) = now.split_at(now.len().min(1));
    simplify(head.iter().copied().chain(bend).chain(tail.iter().copied()))
}

/// Segments on the lattice, for asking whether a point lies on one: per horizontal and vertical
/// line, sorted by their low end with the furthest high end so far (a lookup is a binary search),
/// and the few diagonal ones.
#[derive(Default)]
struct Lines {
    level: BTreeMap<i64, Vec<(i64, i64)>>,
    upright: BTreeMap<i64, Vec<(i64, i64)>>,
    slanted: Vec<([i64; 2], [i64; 2])>,
}

/// The most diagonal segments [`Lines`] keeps; past it, points on further ones are not found.
const MAX_SLANTED: usize = 1024;

impl Lines {
    /// The segments of a wire through `points`.
    fn add(&mut self, points: &[[i64; 2]]) {
        for ab in points.windows(2) {
            let [a, b] = [ab[0], ab[1]];
            if a == b {
                continue;
            }
            if a[1] == b[1] {
                self.level.entry(a[1]).or_default().push((a[0].min(b[0]), a[0].max(b[0])));
            } else if a[0] == b[0] {
                self.upright.entry(a[0]).or_default().push((a[1].min(b[1]), a[1].max(b[1])));
            } else if self.slanted.len() < MAX_SLANTED {
                self.slanted.push((a, b));
            }
        }
    }

    /// Ready for lookups: each line's spans sorted, their high ends turned into the furthest so far.
    /// Spans added since are taken in; those already turned stay right.
    fn finish(&mut self) {
        for spans in self.level.values_mut().chain(self.upright.values_mut()) {
            spans.sort_unstable();
            let mut reach = i64::MIN;
            for s in spans.iter_mut() {
                reach = reach.max(s.1);
                s.1 = reach;
            }
        }
    }

    /// Whether a span on the line starts below `below` and reaches past `past` (with both `v`:
    /// it holds `v` strictly inside; with a stretch's high and low ends: it runs along it).
    fn reaches(spans: Option<&Vec<(i64, i64)>>, below: i64, past: i64) -> bool {
        spans
            .is_some_and(|s| s.partition_point(|(low, _)| *low < below).checked_sub(1).and_then(|i| s.get(i)).is_some_and(|(_, reach)| *reach > past))
    }

    /// Whether `p` lies strictly inside one of the segments (not on an end).
    fn inside(&self, p: [i64; 2]) -> bool {
        Self::reaches(self.level.get(&p[1]), p[0], p[0])
            || Self::reaches(self.upright.get(&p[0]), p[1], p[1])
            || self.slanted.iter().any(|(a, b)| {
                let (ab, ap) = (
                    [i128::from(b[0]) - i128::from(a[0]), i128::from(b[1]) - i128::from(a[1])],
                    [i128::from(p[0]) - i128::from(a[0]), i128::from(p[1]) - i128::from(a[1])],
                );
                let along = ab[0].saturating_mul(ap[0]).saturating_add(ab[1].saturating_mul(ap[1]));
                let length = ab[0].saturating_mul(ab[0]).saturating_add(ab[1].saturating_mul(ab[1]));
                ab[0].saturating_mul(ap[1]) == ab[1].saturating_mul(ap[0]) && along > 0 && along < length
            })
    }

    /// Whether the level or upright segment `a`–`b` runs along one of the segments for more than
    /// a point (wires that overlap along a line connect).
    fn overlaps(&self, a: [i64; 2], b: [i64; 2]) -> bool {
        if a[1] == b[1] && a[0] != b[0] {
            Self::reaches(self.level.get(&a[1]), a[0].max(b[0]), a[0].min(b[0]))
        } else if a[0] == b[0] && a[1] != b[1] {
            Self::reaches(self.upright.get(&a[0]), a[1].max(b[1]), a[1].min(b[1]))
        } else {
            false
        }
    }
}

/// Points on the lattice, by row and by column, so the ones on a level or upright segment are
/// found without walking it.
#[derive(Default)]
struct Spots {
    rows: BTreeSet<(i64, i64)>,
    columns: BTreeSet<(i64, i64)>,
}

impl Spots {
    fn add(&mut self, [x, y]: [i64; 2]) {
        self.rows.insert((y, x));
        self.columns.insert((x, y));
    }

    fn has(&self, [x, y]: [i64; 2]) -> bool {
        self.rows.contains(&(y, x))
    }

    fn remove(&mut self, [x, y]: [i64; 2]) {
        self.rows.remove(&(y, x));
        self.columns.remove(&(x, y));
    }

    /// The points strictly inside the level or upright segment `a`–`b`; none for a slanted one.
    fn inside(&self, a: [i64; 2], b: [i64; 2]) -> Vec<[i64; 2]> {
        // Each range starts at or before its end: `lo < hi`, so `lo + 1 <= hi`.
        if a[1] == b[1] && a[0] != b[0] {
            let (lo, hi) = (a[0].min(b[0]), a[0].max(b[0]));
            self.rows.range((a[1], lo.saturating_add(1))..(a[1], hi)).map(|(y, x)| [*x, *y]).collect()
        } else if a[0] == b[0] && a[1] != b[1] {
            let (lo, hi) = (a[1].min(b[1]), a[1].max(b[1]));
            self.columns.range((a[0], lo.saturating_add(1))..(a[0], hi)).map(|(x, y)| [*x, *y]).collect()
        } else {
            Vec::new()
        }
    }
}

/// `(b − a) × (c − a)`: zero when the three points are on a line (saturating, so points the
/// lattice saturated give a wrong answer, never a panic).
fn cross(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> i128 {
    let d = |p: [i64; 2], q: [i64; 2]| [i128::from(q[0]) - i128::from(p[0]), i128::from(q[1]) - i128::from(p[1])];
    let (u, v) = (d(a, b), d(a, c));
    u[0].saturating_mul(v[1]).saturating_sub(u[1].saturating_mul(v[0]))
}

/// Whether `x` lies on the segment `p`–`q`, its ends included.
fn covers(p: [i64; 2], q: [i64; 2], x: [i64; 2]) -> bool {
    cross(p, q, x) == 0 && x[0] >= p[0].min(q[0]) && x[0] <= p[0].max(q[0]) && x[1] >= p[1].min(q[1]) && x[1] <= p[1].max(q[1])
}

/// Whether the path `a` → `b` → `c` turns back on itself along one line.
fn turns_back(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> bool {
    let d = |p: [i64; 2], q: [i64; 2]| [i128::from(q[0]) - i128::from(p[0]), i128::from(q[1]) - i128::from(p[1])];
    let (u, v) = (d(a, b), d(b, c));
    let dot = u[0].saturating_mul(v[0]).saturating_add(u[1].saturating_mul(v[1]));
    cross(a, b, c) == 0 && dot < 0
}

/// The way from `a` to `b` when they lie on a level or upright line.
fn toward(a: [i64; 2], b: [i64; 2]) -> Option<PinDirection> {
    match (a[0].cmp(&b[0]), a[1].cmp(&b[1])) {
        (std::cmp::Ordering::Less, std::cmp::Ordering::Equal) => Some(PinDirection::Right),
        (std::cmp::Ordering::Greater, std::cmp::Ordering::Equal) => Some(PinDirection::Left),
        (std::cmp::Ordering::Equal, std::cmp::Ordering::Less) => Some(PinDirection::Up),
        (std::cmp::Ordering::Equal, std::cmp::Ordering::Greater) => Some(PinDirection::Down),
        _ => None,
    }
}

/// Where the end `a` goes when what it is on moves by `m`.
fn end_now(pins: &Pins, a: &Anchor, m: &Motion) -> Option<[f64; 2]> {
    match a {
        Anchor::Point(p) => Some(m.point(*p)),
        Anchor::Pin { part, pin } => {
            let placed = m.placement(&pins.part(*part)?.placement);
            pins.pin(*part, pin).map(|q| snap_point(placed.apply(q.at)))
        }
    }
}

/// The way a wire leaves the end `a` when it is on a pin (its part moved by `m`, if given).
fn way_out(pins: &Pins, a: &Anchor, m: Option<&Motion>) -> Option<PinDirection> {
    let (part, number) = a.pin()?;
    let placement = pins.part(part)?.placement;
    let placement = m.map_or(placement, |m| m.placement(&placement));
    pins.pin(part, number).map(|q| opposite(placement.direction(q.direction)))
}

/// What goes along rigidly with a move: the moved parts, the points that move with it, and the
/// segments that slide whole (a point inside one goes along).
struct Carry {
    parts: BTreeSet<u64>,
    points: BTreeSet<[i64; 2]>,
    lines: Lines,
}

impl Carry {
    /// Whether a wire end, label or marker on the point `p` goes along.
    fn carries(&self, p: [f64; 2]) -> bool {
        let q = lattice(p);
        self.points.contains(&q) || self.lines.inside(q)
    }

    /// Whether the wire end `a` goes along: on a moved part's pin, or on a point that does.
    fn moves(&self, a: &Anchor) -> bool {
        match a {
            Anchor::Point(p) => self.carries(*p),
            Anchor::Pin { part, .. } => self.parts.contains(part),
        }
    }

    /// A wire whose points `was` go to `now` (one for one): its points that move by `m` go
    /// along, and so do its segments with both ends doing so (call [`Lines::finish`] after).
    fn add(&mut self, was: &[[f64; 2]], now: &[[f64; 2]], m: &Motion) {
        let goes: Vec<bool> = was.iter().zip(now).map(|(a, b)| lattice(m.point(*a)) == lattice(*b)).collect();
        for (p, g) in was.iter().zip(&goes) {
            if *g {
                self.points.insert(lattice(*p));
            }
        }
        for (ab, g) in was.windows(2).zip(goes.windows(2)) {
            if g.iter().all(|g| *g) {
                self.lines.add(&[lattice(ab[0]), lattice(ab[1])]);
            }
        }
    }
}

/// How far inside a part's body box a new wire segment must reach to count as crossing it, in
/// millimetres (running along its edge is fine).
const BODY_EDGE: f64 = 0.05;

/// A part's body on the sheet: its symbol's shapes (texts left out) and its pins' inner ends.
pub fn body_box(placement: &super::Placement, symbol: &super::SymbolItem) -> Option<[[f64; 2]; 2]> {
    let shapes = super::graphics::shapes_bbox(symbol.graphics.iter().filter(|g| g.kind != super::GraphicType::Text));
    let inner = symbol.pins.iter().map(|p| p.body_end());
    super::bbox(inner.chain(shapes.into_iter().flatten())).map(|b| placement.bbox(b))
}

/// What [`Room::faults`] counts for a wire that meets something it didn't (or leaves off what
/// stood on it): far worse than an ugly route, since the nets may change.
const MEETS: u32 = 16;

/// What a wire that hangs on is checked against, as the sheet is after the move: what stays
/// where it is, the pins, and the wires hung on so far.
struct Room {
    /// Labels, markers and wire ends that stay where they are, and the bends of wires that don't
    /// change.
    spots: Spots,
    /// The pins' ends, moved parts' where they go.
    pins: Spots,
    /// The wires that don't change.
    still: Lines,
    /// The wires hung on so far, and their bends and the labels and markers they took along.
    fresh: Lines,
    bends: Spots,
    /// The parts' bodies, moved parts' where they go.
    bodies: Grid,
    /// The points among `spots` where only labels and markers stand: a leg that slides across
    /// itself may take them along.
    marks: Spots,
}

/// A wire hung on by [`Room::hang`]: its points (ends included), and the label and marker points
/// it takes along with a leg that slides (from, to).
struct Hung {
    points: Vec<[f64; 2]>,
    took: Vec<([i64; 2], [i64; 2])>,
}

impl Room {
    /// What stands on the old wire through `old` (strictly inside a segment, or on a bend): it
    /// must stay on the wire.
    fn standing(&self, old: &[[i64; 2]]) -> BTreeSet<[i64; 2]> {
        let mut out: BTreeSet<[i64; 2]> = old.windows(2).flat_map(|ab| self.spots.inside(ab[0], ab[1])).collect();
        let bends = old.get(1..old.len().saturating_sub(1)).unwrap_or_default();
        out.extend(bends.iter().filter(|v| self.spots.has(**v) || self.pins.has(**v)));
        out
    }

    /// Whether the segment `a`–`b` crosses a part's body.
    fn crosses_body(&self, a: [i64; 2], b: [i64; 2]) -> bool {
        let mm = |v: i64| v as f64 * super::place::STEP;
        let span = [[mm(a[0].min(b[0])), mm(a[1].min(b[1]))], [mm(a[0].max(b[0])), mm(a[1].max(b[1]))]];
        self.bodies.near(span).into_iter().filter_map(|i| self.bodies.boxes.get(i)).any(|[lo, hi]| {
            span[0][0] < hi[0] - BODY_EDGE && span[1][0] > lo[0] + BODY_EDGE && span[0][1] < hi[1] - BODY_EDGE && span[1][1] > lo[1] + BODY_EDGE
        })
    }

    /// What is wrong with the wire through `q` (its ends where they go), counted up to `cap`
    /// (0: it fits): something in `keep` left off it, or where it leaves its old path `old` (new
    /// legs and bends) it meets a label, marker, wire end or bend, pin or other wire (each counts
    /// [`MEETS`]: the nets may change), crosses a part's body or turns back on itself (each 1).
    fn faults(&self, q: &[[f64; 2]], old: &[[i64; 2]], keep: &BTreeSet<[i64; 2]>, cap: u32) -> u32 {
        let q: Vec<[i64; 2]> = q.iter().map(|p| lattice(*p)).collect();
        let (Some(&e0), Some(&e1)) = (q.first(), q.last()) else { return cap };
        let mut n = 0u32;
        let mut count = |k: u32| {
            n = n.saturating_add(k);
            n >= cap
        };
        let on = |x: [i64; 2]| q.windows(2).any(|ab| covers(ab[0], ab[1], x)) || (q.len() == 1 && x == e0);
        for _ in keep.iter().filter(|k| !on(**k)) {
            if count(MEETS) {
                return cap;
            }
        }
        for _ in q.windows(3).filter(|t| turns_back(t[0], t[1], t[2])) {
            if count(1) {
                return cap;
            }
        }
        let own = |p: &[i64; 2]| *p == e0 || *p == e1 || keep.contains(p);
        let along_old = |a: [i64; 2], b: [i64; 2]| old.windows(2).any(|s| covers(s[0], s[1], a) && covers(s[0], s[1], b));
        for ab in q.windows(2) {
            let (a, b) = (ab[0], ab[1]);
            if along_old(a, b) {
                // Kept as it was, but a wire hung on before may have come onto it since.
                if (self.fresh.overlaps(a, b) || self.bends.inside(a, b).into_iter().any(|p| !own(&p))) && count(MEETS) {
                    return cap;
                }
                continue;
            }
            let met = self.spots.inside(a, b).into_iter().chain(self.bends.inside(a, b)).any(|p| !own(&p))
                || self.pins.inside(a, b).into_iter().any(|p| p != e0 && p != e1)
                || self.still.overlaps(a, b)
                || self.fresh.overlaps(a, b);
            if (met && count(MEETS)) || (self.crosses_body(a, b) && count(1)) {
                return cap;
            }
        }
        let bends = q.get(1..q.len().saturating_sub(1)).unwrap_or_default();
        for v in bends {
            let met = !old.contains(v)
                && !keep.contains(v)
                && (self.spots.has(*v) || self.bends.has(*v) || self.pins.has(*v) || self.still.inside(*v) || self.fresh.inside(*v));
            if met && count(MEETS) {
                return cap;
            }
        }
        n
    }

    /// The parts' bodies near the box round `a` and `b`, for the auto route's corners.
    fn bodies_near(&self, a: [f64; 2], b: [f64; 2]) -> Vec<[[f64; 2]; 2]> {
        let span = [[a[0].min(b[0]) - STUB, a[1].min(b[1]) - STUB], [a[0].max(b[0]) + STUB, a[1].max(b[1]) + STUB]];
        let mut near = self.bodies.near(span);
        near.sort_unstable();
        near.dedup();
        near.into_iter().filter_map(|i| self.bodies.boxes.get(i).copied()).collect()
    }

    /// The wire through `was` (ends included) with its first end gone to `ne`, leaving it
    /// `out` (`turned`: a pin that points another way now); its last end, `last_out` from its
    /// pin, stays. Tried in order, the first with no [`faults`](Room::faults):
    /// - unless turned, the bend next to the end slides so its leg keeps its axis (a straight
    ///   wire gains an elbow), taking along the labels and markers on that leg;
    /// - unless turned, a jog [`STUB`] out of the end onto the old first leg, ahead of whatever
    ///   stands on it, the rest of the wire as it was;
    /// - the auto route from the end to the first point on the old wire that stands (something
    ///   on the first leg, or a bend, or the far end), and on from there as before;
    /// - turned, the same points reached round the part's `body` (out of the pin, past the body's
    ///   top or bottom, or left or right side, a grid step or a few clear of it, then across).
    ///
    /// `false` with the least faulty of these when none fits (the reply's `netlist_changed`
    /// tells what that does).
    fn hang(
        &self,
        was: &[[f64; 2]],
        ne: [f64; 2],
        out: Option<PinDirection>,
        turned: bool,
        last_out: Option<PinDirection>,
        body: Option<[[f64; 2]; 2]>,
    ) -> (Hung, bool) {
        let old: Vec<[i64; 2]> = was.iter().map(|p| lattice(*p)).collect();
        let (Some(&p0), Some(&p1), Some(&last)) = (was.first(), was.get(1), was.last()) else {
            return (Hung { points: was.to_vec(), took: Vec::new() }, true);
        };
        let keep = self.standing(&old);
        // The faults of a way (none: it fits), counted up to `cap`; one past the cap is no way.
        let faults =
            |q: &[[f64; 2]], keep: &BTreeSet<[i64; 2]>, cap: u32| if q.len() > MAX_WIRE_POINTS { cap } else { self.faults(q, &old, keep, cap) };
        let (a0, a1) = (lattice(p0), lattice(p1));
        let (now, bend) = slide(was, [ne, last], [true, false], false, &Motion::default());
        let slid = joined(&now, bend);
        // The first leg slid across itself by `d` (to the elbow, for a straight wire): the labels
        // and markers alone on it go along, where they land on the slid wire and on nothing else.
        let d = match (bend, now.get(1)) {
            (Some(e), _) => sub(e, last),
            (None, Some(n1)) if was.len() > 2 => sub(*n1, p1),
            _ => [0.0, 0.0],
        };
        let shift = |k: [i64; 2]| lattice([k[0] as f64 * super::place::STEP + d[0], k[1] as f64 * super::place::STEP + d[1]]);
        let on_slid: Vec<[i64; 2]> = slid.iter().map(|p| lattice(*p)).collect();
        let lands = |k: [i64; 2]| {
            on_slid.windows(2).any(|ab| covers(ab[0], ab[1], k))
                && !(self.spots.has(k) || self.bends.has(k) || self.pins.has(k) || self.still.inside(k) || self.fresh.inside(k))
        };
        let took: Vec<([i64; 2], [i64; 2])> = if d == [0.0, 0.0] {
            Vec::new()
        } else {
            keep.iter()
                .filter(|k| self.marks.has(**k) && covers(a0, a1, **k) && **k != a0 && **k != a1)
                .map(|k| (*k, shift(*k)))
                .filter(|(_, to)| lands(*to))
                .collect()
        };
        let rest: BTreeSet<[i64; 2]> = keep.iter().filter(|k| !took.iter().any(|(from, _)| from == *k)).copied().collect();
        // The least faulty way so far, should none fit; sliding unless the pin turned.
        let mut best: Option<(u32, Hung)> = None;
        if !turned {
            match faults(&slid, &rest, u32::MAX) {
                0 => return (Hung { points: slid, took }, true),
                n => best = Some((n, Hung { points: slid.clone(), took: took.clone() })),
            }
        }
        // Where the wire may join its old path: a point, the index of the old point after it,
        // and the way the old path leaves it toward the moved end.
        let mut joins: Vec<([f64; 2], usize, Option<PinDirection>)> = Vec::new();
        let first_leg = toward(a0, a1);
        let from_start = |k: &[i64; 2]| k[0].abs_diff(a0[0]).saturating_add(k[1].abs_diff(a0[1]));
        let length = |p: [i64; 2]| from_start(&p) as f64 * super::place::STEP;
        // The first thing standing on the first leg, nearest the moved end.
        let first_stand =
            keep.iter().filter(|k| first_leg.is_some() && covers(a0, a1, **k) && **k != a0 && **k != a1).min_by_key(|k| from_start(k)).copied();
        if let Some(dir) = first_leg.filter(|_| !turned) {
            let u = dir.unit();
            let ahead = ((ne[0] - p0[0]) * u[0] + (ne[1] - p0[1]) * u[1]).max(0.0) + STUB;
            let room = first_stand.map_or_else(|| length(a1), length);
            if ahead < room - super::place::STEP {
                joins.push((snap_point([p0[0] + u[0] * ahead, p0[1] + u[1] * ahead]), 1, None));
            }
        }
        if let (Some(f), Some(dir)) = (first_stand, first_leg) {
            joins.push(([f[0] as f64 * super::place::STEP, f[1] as f64 * super::place::STEP], 1, Some(opposite(dir))));
        }
        let n = was.len();
        for j in 1..n.min(5) {
            let (Some(&at), Some(&here), Some(&prev)) = (was.get(j), old.get(j), old.get(j.saturating_sub(1))) else { continue };
            // Into a bend along the old leg; at the far end, out of its pin (a point takes any way).
            let way = if j.saturating_add(1) == n { last_out } else { toward(here, prev) };
            joins.push((at, j.saturating_add(1), way));
        }
        let mut ways: Vec<Vec<[f64; 2]>> = Vec::new();
        for (at, next, way) in &joins {
            let route = auto_route(RouteEnd { at: ne, out }, RouteEnd { at: *at, out: *way }, &self.bodies_near(ne, *at));
            ways.push(simplify(route.into_iter().chain(was.iter().skip(*next).copied())));
        }
        if let (true, Some([lo, hi]), Some(o)) = (turned, body, out) {
            let u = o.unit();
            let sa = snap_point([ne[0] + u[0] * STUB, ne[1] + u[1] * STUB]);
            let level = horizontal(o);
            for (at, next, _) in &joins {
                for k in 1..=AROUND_STEPS {
                    let step = GRID * f64::from(k);
                    let (high, low) = if level { (hi[1] + step, lo[1] - step) } else { (hi[0] + step, lo[0] - step) };
                    let toward_at = if level { at[1] } else { at[0] };
                    let sides = if (high - toward_at).abs() <= (low - toward_at).abs() { [high, low] } else { [low, high] };
                    for c in sides {
                        let path = if level { [ne, sa, [sa[0], c], [at[0], c], *at] } else { [ne, sa, [c, sa[1]], [c, at[1]], *at] };
                        ways.push(simplify(path.into_iter().chain(was.iter().skip(*next).copied())));
                    }
                }
            }
        }
        for q in ways {
            let cap = best.as_ref().map_or(u32::MAX, |(n, _)| *n);
            match faults(&q, &keep, cap) {
                0 => return (Hung { points: q, took: Vec::new() }, true),
                n if n < cap => best = Some((n, Hung { points: q, took: Vec::new() })),
                _ => {}
            }
        }
        match best {
            Some((_, hung)) => (hung, false),
            None => (Hung { points: slid, took }, false),
        }
    }

    /// A wire hung on: later ones keep clear of it and of what it took along.
    fn add(&mut self, hung: &Hung) {
        let at: Vec<[i64; 2]> = hung.points.iter().map(|p| lattice(*p)).collect();
        self.fresh.add(&at);
        self.fresh.finish();
        for v in at.get(1..at.len().saturating_sub(1)).unwrap_or_default() {
            self.bends.add(*v);
        }
        for (from, to) in &hung.took {
            self.spots.remove(*from);
            self.marks.remove(*from);
            self.bends.add(*to);
        }
    }
}

/// The most passes [`hang_on`] makes looking for wires that go whole because what they join
/// went whole the pass before; past it, the rest hang on by one end.
const MAX_PASSES: usize = 64;

/// How many more times [`hang_on`] tries the wires that fit nowhere yet.
const MAX_ROUNDS: usize = 4;

/// How many grid steps clear of a turned part's body [`Room::hang`] tries going round it.
const AROUND_STEPS: u8 = 6;

/// The wires as they hang on when what `carry` holds moves by `m`, `set` holding the wires
/// already placed (their points, ends included) and `ids` the labels and markers moved with them:
/// - a wire with both ends going along (on a moved pin, a point that moves, or inside a segment
///   that slides whole) goes whole, bends and all, and what is on it goes along: repeated, so a
///   chain of wires between moved things goes whole;
/// - a wire with one end going along hangs on ([`Room::hang`]): it keeps whatever stands on it
///   and runs into nothing new where it can.
///
/// Returns every wire placed, `set` included, and the labels and markers that slid legs took
/// along (where to), and fills `carry` with what goes along.
fn hang_on(
    sheet: &Schematic,
    library: &Library,
    pins: &Pins,
    carry: &mut Carry,
    mut set: BTreeMap<u64, Vec<[f64; 2]>>,
    ids: &BTreeSet<u64>,
    m: &Motion,
) -> (BTreeMap<u64, Vec<[f64; 2]>>, BTreeMap<u64, [f64; 2]>) {
    for _ in 0..MAX_PASSES {
        carry.lines.finish();
        // What a pass finds goes along from the next pass on (lookups need the lines finished).
        let mut whole = Vec::new();
        for w in sheet.wires.iter().filter(|w| !set.contains_key(&w.id)) {
            if !(carry.moves(&w.from) && carry.moves(&w.to)) {
                continue;
            }
            let (Some(of), Some(ot), Some(nf), Some(nt)) =
                (pins.anchor(&w.from), pins.anchor(&w.to), end_now(pins, &w.from, m), end_now(pins, &w.to, m))
            else {
                continue;
            };
            let was: Vec<[f64; 2]> = std::iter::once(of).chain(w.via.iter().copied()).chain(std::iter::once(ot)).collect();
            let (now, _) = slide(&was, [nf, nt], [true, true], true, m);
            whole.push((w.id, was, now));
        }
        if whole.is_empty() {
            break;
        }
        for (id, was, now) in whole {
            carry.add(&was, &now, m);
            set.insert(id, simplify(now));
        }
    }
    carry.lines.finish();
    let mut moving: Vec<(&Wire, bool)> = sheet
        .wires
        .iter()
        .filter(|w| !set.contains_key(&w.id))
        .map(|w| (w, carry.moves(&w.from)))
        .filter(|(w, mf)| *mf != carry.moves(&w.to))
        .collect();
    let mut took = BTreeMap::new();
    if moving.is_empty() {
        return (set, took);
    }
    let hanging: BTreeSet<u64> = moving.iter().map(|(w, _)| w.id).collect();
    let mut room = Room {
        spots: Spots::default(),
        pins: Spots::default(),
        still: Lines::default(),
        fresh: Lines::default(),
        bends: Spots::default(),
        bodies: Grid::default(),
        marks: Spots::default(),
    };
    // The labels and markers that stay, by point, and the other points where something stays.
    let stays = |id: u64, a: &Anchor| !ids.contains(&id) && a.point().is_some_and(|p| !carry.carries(p));
    let mut marks: BTreeMap<[i64; 2], Vec<u64>> = BTreeMap::new();
    for (id, p) in sheet.labels.iter().filter(|l| stays(l.id, &l.at)).filter_map(|l| Some((l.id, l.at.point()?))) {
        marks.entry(lattice(p)).or_default().push(id);
    }
    for (id, p) in sheet.markers.iter().filter(|k| stays(k.id, &k.at)).filter_map(|k| Some((k.id, k.at.point()?))) {
        marks.entry(lattice(p)).or_default().push(id);
    }
    let mut others = BTreeSet::new();
    for w in sheet.wires.iter().filter(|w| !set.contains_key(&w.id)) {
        others.extend([&w.from, &w.to].into_iter().filter(|a| !carry.moves(a)).filter_map(Anchor::point).map(lattice));
        if !hanging.contains(&w.id) {
            others.extend(w.via.iter().map(|v| lattice(*v)));
            room.still.add(&pins.wire(w).into_iter().map(lattice).collect::<Vec<_>>());
        }
    }
    room.still.finish();
    for p in &sheet.parts {
        let Some(c) = library.get(&p.component) else { continue };
        let placement = if carry.parts.contains(&p.id) { m.placement(&p.placement) } else { p.placement };
        c.symbol.pins.iter().for_each(|q| room.pins.add(placement.lattice_point(q.at)));
        if let Some(b) = body_box(&placement, c.symbol) {
            room.bodies.add(b);
        }
    }
    for p in marks.keys() {
        room.spots.add(*p);
        if !others.contains(p) && !room.pins.has(*p) {
            room.marks.add(*p);
        }
    }
    others.iter().for_each(|p| room.spots.add(*p));
    // Ends that move furthest along the move first, so a column of wires slides like a column.
    let lead = |w: &Wire, mf: bool| {
        let at = pins.anchor(if mf { &w.from } else { &w.to }).unwrap_or_default();
        -(at[0] * m.by[0] + at[1] * m.by[1])
    };
    moving.sort_by(|(a, fa), (b, fb)| lead(a, *fa).total_cmp(&lead(b, *fb)));
    // A wire that fits nowhere yet is tried again once those after it have hung on (they may
    // have taken what was in its way along); what still fits nowhere takes its first way.
    let mut force = false;
    for round in 0..=MAX_ROUNDS {
        force |= round == MAX_ROUNDS;
        let before = moving.len();
        let mut left = Vec::new();
        for (w, mf) in std::mem::take(&mut moving) {
            let (odd, rest) = if mf { (&w.from, &w.to) } else { (&w.to, &w.from) };
            let (Some(of), Some(ot), Some(ne)) = (pins.anchor(&w.from), pins.anchor(&w.to), end_now(pins, odd, m)) else { continue };
            let mut was: Vec<[f64; 2]> = std::iter::once(of).chain(w.via.iter().copied()).chain(std::iter::once(ot)).collect();
            if !mf {
                was.reverse();
            }
            let out = way_out(pins, odd, Some(m));
            let turned = out != way_out(pins, odd, None);
            // The body of the part the end is on, where it goes, for going round it.
            let body = odd.pin().and_then(|(part, _)| {
                let p = pins.part(part)?;
                body_box(&m.placement(&p.placement), library.get(&p.component)?.symbol)
            });
            let (mut hung, fits) = room.hang(&was, ne, out, turned, way_out(pins, rest, None), body);
            if !fits && !force {
                left.push((w, mf));
                continue;
            }
            room.add(&hung);
            for (from, to) in &hung.took {
                for id in marks.get(from).into_iter().flatten() {
                    took.insert(*id, [to[0] as f64 * super::place::STEP, to[1] as f64 * super::place::STEP]);
                }
            }
            if !mf {
                hung.points.reverse();
            }
            set.insert(w.id, hung.points);
        }
        moving = left;
        if moving.is_empty() {
            break;
        }
        // Nothing hung on this round: the rest take their first ways in the next.
        force |= moving.len() == before;
    }
    (set, took)
}

/// The wire `w` through `points` (its ends included): an end on a point takes the new point, an
/// end on a pin stays on it.
fn rewired(w: &Wire, points: &[[f64; 2]]) -> Wire {
    let end = |a: &Anchor, p: Option<&[f64; 2]>| match (a, p) {
        (Anchor::Point(_), Some(p)) => Anchor::Point(*p),
        _ => a.clone(),
    };
    Wire { id: w.id, from: end(&w.from, points.first()), via: inner(points), to: end(&w.to, points.last()) }
}

/// A wire's bends: its points but the first and last.
fn inner(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    match points.len() {
        0..=2 => Vec::new(),
        n => points.get(1..n - 1).map(<[_]>::to_vec).unwrap_or_default(),
    }
}

/// How wires hang on (as [`hang_on`] gives it): each wire placed, by id, with its points (ends
/// included), and the labels and markers a slid leg took along, with where they go.
type Hanging = (BTreeMap<u64, Vec<[f64; 2]>>, BTreeMap<u64, [f64; 2]>);

/// `sheet` with the labels and markers `items` moved by `m` (with the parts among them), the
/// wires and what they took along as `hung` says, and, when `m` turns or mirrors, the
/// power ports on the pins of `parts` (and those among `items`, with `turn_moved`) pointing the
/// usual way again.
fn settled(
    sheet: &Schematic,
    library: &Library,
    items: &BTreeSet<u64>,
    hung: &Hanging,
    m: &Motion,
    parts: &BTreeSet<u64>,
    turn_moved: bool,
) -> Schematic {
    let (wires, took) = hung;
    let mut out = sheet.clone();
    out.move_items(items, m);
    for w in &mut out.wires {
        if let Some(points) = wires.get(&w.id) {
            *w = rewired(w, points);
        }
    }
    for l in &mut out.labels {
        if let Some(p) = took.get(&l.id) {
            l.at = Anchor::Point(*p);
        }
    }
    for k in &mut out.markers {
        if let Some(p) = took.get(&k.id) {
            k.at = Anchor::Point(*p);
        }
    }
    if m.mirror || !m.rotation.is_multiple_of(360) {
        upright(&mut out, library, parts, turn_moved.then_some(items));
    }
    out
}

/// The sheet after the sheet objects `ids` move by `m`, with what touches them hanging on so that
/// no connection comes apart where it can be kept:
/// - a picked wire moves, but an end of it that is the foot of a T into a wire left behind (on
///   its bend or inside a segment, with nothing moved standing there) stays, and so does an end
///   on a pin that doesn't move; the bend next to an end that stays slides so its leg keeps its
///   axis (a straight wire gains an elbow), unless that leaves something standing on it off the
///   wire: then it moves with the rest and a new bend beside it keeps the leg's axis;
/// - what stands on a moved pin, label, marker, wire end or bend, or inside a segment that slides
///   whole, goes along: labels and markers move, and other wires hang on: a wire with both ends
///   going along goes whole, one with one end going along keeps what stands on it and runs into
///   nothing new where it can (a slid leg, a jog onto its old path, or a new route);
/// - turned or mirrored, the power ports on the moved parts' pins (and the moved and carried
///   ones, unless only labels and markers move) point the usual way again ([`PortRoom::turn`]).
pub fn moved(sheet: &Schematic, library: &Library, ids: &BTreeSet<u64>, m: &Motion) -> Schematic {
    let pins = Pins::new(sheet, library);
    let parts: BTreeSet<u64> = sheet.parts.iter().filter(|p| ids.contains(&p.id)).map(|p| p.id).collect();
    let mut points = BTreeSet::new();
    for p in sheet.parts.iter().filter(|p| parts.contains(&p.id)) {
        if let Some(c) = library.get(&p.component) {
            points.extend(c.symbol.pins.iter().map(|q| p.placement.lattice_point(q.at)));
        }
    }
    let mut marks: BTreeSet<[i64; 2]> = sheet.labels.iter().filter(|l| ids.contains(&l.id)).filter_map(|l| l.at.point()).map(lattice).collect();
    marks.extend(sheet.markers.iter().filter(|k| ids.contains(&k.id)).filter_map(|k| k.at.point()).map(lattice));
    // A picked wire's end stays where it is the foot of a T into a wire left behind (on its bend
    // or inside a segment) with nothing moved standing there.
    let (mut still, mut bends, mut moved_bends) = (Lines::default(), BTreeSet::new(), BTreeSet::new());
    for w in &sheet.wires {
        if ids.contains(&w.id) {
            moved_bends.extend(w.via.iter().map(|v| lattice(*v)));
        } else {
            still.add(&pins.wire(w).into_iter().map(lattice).collect::<Vec<_>>());
            bends.extend(w.via.iter().map(|v| lattice(*v)));
        }
    }
    still.finish();
    let mut held = BTreeSet::new();
    for w in sheet.wires.iter().filter(|w| ids.contains(&w.id)) {
        for q in [&w.from, &w.to].into_iter().filter_map(Anchor::point).map(lattice) {
            let alone = !marks.contains(&q) && !points.contains(&q) && !moved_bends.contains(&q);
            if alone && (bends.contains(&q) || still.inside(q)) {
                held.insert(q);
            }
        }
    }
    points.extend(marks);
    let mut carry = Carry { parts, points, lines: Lines::default() };
    // Where something stands that a picked wire's bend may carry: other wires' point ends and
    // bends (by wire), labels and markers on points.
    let mut feet: BTreeMap<[i64; 2], BTreeSet<u64>> = BTreeMap::new();
    for w in &sheet.wires {
        for p in [&w.from, &w.to].into_iter().filter_map(Anchor::point).chain(w.via.iter().copied()) {
            feet.entry(lattice(p)).or_default().insert(w.id);
        }
    }
    let marked: BTreeSet<[i64; 2]> =
        sheet.labels.iter().filter_map(|l| l.at.point()).chain(sheet.markers.iter().filter_map(|k| k.at.point())).map(lattice).collect();
    // The picked wires first: what goes along with them is what of them moves by `m` (a leg next
    // to an end that stays slides across, so what is on it stays).
    let mut set = BTreeMap::new();
    for w in sheet.wires.iter().filter(|w| ids.contains(&w.id)) {
        let moves = |a: &Anchor| match a {
            Anchor::Point(p) => !held.contains(&lattice(*p)),
            Anchor::Pin { part, .. } => carry.parts.contains(part),
        };
        let (mf, mt) = (moves(&w.from), moves(&w.to));
        let (Some(of), Some(ot)) = (pins.anchor(&w.from), pins.anchor(&w.to)) else { continue };
        let (Some(nf), Some(nt)) = (if mf { end_now(&pins, &w.from, m) } else { Some(of) }, if mt { end_now(&pins, &w.to, m) } else { Some(ot) })
        else {
            continue;
        };
        let was: Vec<[f64; 2]> = std::iter::once(of).chain(w.via.iter().copied()).chain(std::iter::once(ot)).collect();
        let (mut now, bend) = slide(&was, [nf, nt], [mf, mt], true, m);
        // A bend that slid off something else on it (a tee's foot, a label, a marker: the slid wire
        // no longer runs through the point) moves with the rest instead, so what is on it goes
        // along; new bends beside it keep the legs to the ends that stay on their axes.
        let slid: Vec<[i64; 2]> = joined(&now, bend).into_iter().map(lattice).collect();
        let left = |q: [i64; 2]| {
            (marked.contains(&q) || feet.get(&q).is_some_and(|ids| ids.iter().any(|id| *id != w.id)))
                && !slid.windows(2).any(|ab| covers(ab[0], ab[1], q))
        };
        let last = was.len().saturating_sub(1);
        let mut path = Vec::with_capacity(now.len().saturating_add(2));
        for (i, (p, q)) in was.iter().zip(now.iter_mut()).enumerate() {
            let rigid = m.point(*p);
            if i == 0 || i == last || lattice(*q) == lattice(rigid) || !left(lattice(*p)) {
                path.push(*q);
                continue;
            }
            let (mut a, mut b) = (rigid, rigid);
            if i == 1 && !mf {
                keep_axis(of, *p, nf, &mut a);
            }
            if i.saturating_add(1) == last && !mt {
                keep_axis(ot, *p, nt, &mut b);
            }
            path.extend([a, rigid, b]);
            *q = rigid;
        }
        carry.add(&was, &now, m);
        set.insert(w.id, joined(&path, bend));
    }
    let wires = hang_on(sheet, library, &pins, &mut carry, set, ids, m);
    let wire_ids: BTreeSet<u64> = sheet.wires.iter().map(|w| w.id).collect();
    let along = |id: u64, at: &Anchor| !ids.contains(&id) && at.point().is_some_and(|p| carry.carries(p));
    let mut items: BTreeSet<u64> = ids.iter().copied().filter(|id| !wire_ids.contains(id)).collect();
    items.extend(sheet.labels.iter().filter(|l| along(l.id, &l.at)).map(|l| l.id));
    items.extend(sheet.markers.iter().filter(|k| along(k.id, &k.at)).map(|k| k.id));
    let label_ids: BTreeSet<u64> = sheet.labels.iter().map(|l| l.id).chain(sheet.markers.iter().map(|k| k.id)).collect();
    let only_marks = ids.iter().all(|id| label_ids.contains(id));
    settled(sheet, library, &items, &wires, m, &carry.parts, !only_marks)
}

/// The sheet after the wire `id` takes the bends `via` (in order from its first end; its ends
/// stay): a level or upright segment that slid straight across itself (as dragging a segment
/// slides it) takes along what stood on it and is no longer on the wire: the feet of T joins,
/// labels and markers, and the wires whose feet came along hang on as a move hangs them
/// (as [`moved`] hangs them). Anything else on the wire stays where it is.
pub fn reshaped(sheet: &Schematic, library: &Library, id: u64, via: &[[f64; 2]]) -> Schematic {
    let pins = Pins::new(sheet, library);
    let via: Vec<[f64; 2]> = via.iter().map(|p| snap_point(*p)).collect();
    let Some(w) = sheet.wires.iter().find(|w| w.id == id) else { return sheet.clone() };
    let (Some(of), Some(ot)) = (pins.anchor(&w.from), pins.anchor(&w.to)) else {
        let mut out = sheet.clone();
        if let Some(w) = out.wires.iter_mut().find(|w| w.id == id) {
            w.via = via;
        }
        return out;
    };
    let was: Vec<[i64; 2]> = std::iter::once(of).chain(w.via.iter().copied()).chain(std::iter::once(ot)).map(lattice).collect();
    let now: Vec<[f64; 2]> = std::iter::once(snap_point(of)).chain(via.iter().copied()).chain(std::iter::once(snap_point(ot))).collect();
    let new: Vec<[i64; 2]> = now.iter().map(|p| lattice(*p)).collect();
    let on_new = |x: [i64; 2]| new.windows(2).any(|ab| covers(ab[0], ab[1], x));
    // Where each old segment slid to: the nearest new segment on a parallel line that spans it.
    let slid: Vec<Option<[i64; 2]>> = was
        .windows(2)
        .map(|ab| {
            let (a, b) = (ab[0], ab[1]);
            let level = a[1] == b[1] && a[0] != b[0];
            let upright = a[0] == b[0] && a[1] != b[1];
            new.windows(2)
                .filter_map(|cd| {
                    let (c, d) = (cd[0], cd[1]);
                    let spans = |i: usize| {
                        let (lo, hi) = (c[i].min(d[i]), c[i].max(d[i]));
                        lo <= a[i].min(b[i]) && hi >= a[i].max(b[i])
                    };
                    if level && c[1] == d[1] && c[1] != a[1] && spans(0) {
                        Some([0, c[1].saturating_sub(a[1])])
                    } else if upright && c[0] == d[0] && c[0] != a[0] && spans(1) {
                        Some([c[0].saturating_sub(a[0]), 0])
                    } else {
                        None
                    }
                })
                .min_by_key(|d| d[0].unsigned_abs().saturating_add(d[1].unsigned_abs()))
        })
        .collect();
    // How far what stands on the old point `x` goes: inside a segment that slid, or on a bend
    // next to one; nothing when it is still on the wire.
    let offset = |x: [i64; 2]| -> Option<[i64; 2]> {
        if on_new(x) {
            return None;
        }
        let end = Some(&x) == was.first() || Some(&x) == was.last();
        was.windows(2).zip(&slid).find_map(|(ab, d)| d.filter(|_| !end && covers(ab[0], ab[1], x)))
    };
    let mut groups: BTreeMap<[i64; 2], (BTreeSet<[i64; 2]>, BTreeSet<u64>)> = BTreeMap::new();
    for l in &sheet.labels {
        if let Some(d) = l.at.point().and_then(|p| offset(lattice(p))) {
            groups.entry(d).or_default().1.insert(l.id);
        }
    }
    for k in &sheet.markers {
        if let Some(d) = k.at.point().and_then(|p| offset(lattice(p))) {
            groups.entry(d).or_default().1.insert(k.id);
        }
    }
    for o in sheet.wires.iter().filter(|o| o.id != id) {
        for p in [&o.from, &o.to].into_iter().filter_map(Anchor::point).map(lattice) {
            if let Some(d) = offset(p) {
                groups.entry(d).or_default().0.insert(p);
            }
        }
    }
    let mut out = sheet.clone();
    if let Some(w) = out.wires.iter_mut().find(|w| w.id == id) {
        w.via = inner(&simplify(now.iter().copied()));
    }
    let none = BTreeSet::new();
    for (d, (points, items)) in groups {
        let m = Motion { by: [d[0] as f64 * super::place::STEP, d[1] as f64 * super::place::STEP], ..Motion::default() };
        let pins = Pins::new(&out, library);
        let mut carry = Carry { parts: BTreeSet::new(), points, lines: Lines::default() };
        let set = out.wires.iter().filter(|o| o.id == id).map(|o| (o.id, pins.wire(o))).collect();
        let wires = hang_on(&out, library, &pins, &mut carry, set, &items, &m);
        out = settled(&out, library, &items, &wires, &m, &none, false);
    }
    out
}

/// Turn the power ports on the pins of `parts`, and those among `moved`, the usual way for where
/// they stand ([`PortRoom::turn`]).
fn upright(sheet: &mut Schematic, library: &Library, parts: &BTreeSet<u64>, moved: Option<&BTreeSet<u64>>) {
    let fix = |l: &Label| {
        l.kind != LabelKind::Net && (l.at.pin().is_some_and(|(part, _)| parts.contains(&part)) || moved.is_some_and(|ids| ids.contains(&l.id)))
    };
    let turns: Vec<(u64, u16)> = {
        let room = PortRoom::new(sheet, library);
        sheet.labels.iter().filter(|l| fix(l)).map(|l| (l.id, room.turn(l))).collect()
    };
    let turns: BTreeMap<u64, u16> = turns.into_iter().collect();
    for l in &mut sheet.labels {
        if let Some(t) = turns.get(&l.id) {
            l.rotation = *t;
        }
    }
}

/// The height a label's name is reckoned at, in millimetres (as the sheet draws it).
const LABEL_TEXT: f64 = 1.27;

/// The box a label takes on the sheet at `at` pointing `rotation` degrees: its mark and its name,
/// roughly as the sheet and the printed pages draw them.
pub fn label_box(kind: LabelKind, name: &str, at: [f64; 2], rotation: u16) -> [[f64; 2]; 2] {
    let chars = super::overbar::plain(name).chars().count() as f64;
    let text = 0.6 * LABEL_TEXT * chars;
    let (along, half) = match kind {
        LabelKind::Net => (0.4 + text + 0.6, 0.6 * LABEL_TEXT),
        LabelKind::Rail => (LABEL_TEXT + 0.4 + text, 0.6 * LABEL_TEXT),
        LabelKind::Ground => (1.2 * LABEL_TEXT, 0.8 * LABEL_TEXT),
    };
    let d = match rotation % 360 {
        90 => [0.0, 1.0],
        180 => [-1.0, 0.0],
        270 => [0.0, -1.0],
        _ => [1.0, 0.0],
    };
    let across = [-d[1] * half, d[0] * half];
    let (a, b) = ([at[0] - across[0], at[1] - across[1]], [at[0] + d[0] * along + across[0], at[1] + d[1] * along + across[1]]);
    [[a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]]
}

/// Whether two boxes overlap by more than touching.
fn meet([a0, a1]: [[f64; 2]; 2], [b0, b1]: [[f64; 2]; 2]) -> bool {
    a0[0] < b1[0] && b0[0] < a1[0] && a0[1] < b1[1] && b0[1] < a1[1]
}

/// The side of a [`Grid`] cell, in millimetres.
const CELL: f64 = 10.0;
/// The most cells one box is filed under; a larger one is tried for every lookup.
const MAX_CELLS: i64 = 64;

/// Boxes on the sheet filed by the cells they cover, to find the ones near a box quickly.
#[derive(Default)]
struct Grid {
    boxes: Vec<[[f64; 2]; 2]>,
    cells: BTreeMap<[i64; 2], Vec<usize>>,
    wide: Vec<usize>,
}

/// The cells a box covers, when they are few enough.
fn cells([lo, hi]: [[f64; 2]; 2]) -> Option<(std::ops::RangeInclusive<i64>, std::ops::RangeInclusive<i64>)> {
    // Casts saturate (and NaN gives 0), so a hostile box covers too many cells, never panics.
    let c = |v: f64| (v / CELL).floor() as i64;
    let (x0, x1, y0, y1) = (c(lo[0]), c(hi[0]), c(lo[1]), c(hi[1]));
    let n = x1.saturating_sub(x0).saturating_add(1).saturating_mul(y1.saturating_sub(y0).saturating_add(1));
    (x0 <= x1 && y0 <= y1 && n <= MAX_CELLS).then_some((x0..=x1, y0..=y1))
}

impl Grid {
    fn add(&mut self, b: [[f64; 2]; 2]) {
        let i = self.boxes.len();
        self.boxes.push(b);
        match cells(b) {
            Some((xs, ys)) => {
                for x in xs {
                    for y in ys.clone() {
                        self.cells.entry([x, y]).or_default().push(i);
                    }
                }
            }
            None => self.wide.push(i),
        }
    }

    /// The boxes that may meet `b`, by index (some more than once).
    fn near(&self, b: [[f64; 2]; 2]) -> Vec<usize> {
        let Some((xs, ys)) = cells(b) else { return (0..self.boxes.len()).collect() };
        let mut out = self.wide.clone();
        for x in xs {
            for y in ys.clone() {
                out.extend(self.cells.get(&[x, y]).into_iter().flatten());
            }
        }
        out
    }
}

/// A placed pin, as [`PortRoom`] files it.
struct RoomPin {
    part: u64,
    number: String,
    /// Where a wire meets it.
    end: [f64; 2],
}

/// What a power port's way is worked out against: the pins on the sheet and the labels' boxes.
/// Ports put down in one go are added as they are made.
pub struct PortRoom<'a> {
    pins: Pins<'a>,
    placed: Vec<RoomPin>,
    /// The pins' lines, in the order of `placed`.
    lines: Grid,
    /// The labels' boxes, with the label's id (0 for one added).
    labels: Grid,
    label_ids: Vec<u64>,
}

impl<'a> PortRoom<'a> {
    pub fn new(sheet: &'a Schematic, library: &Library<'a>) -> Self {
        let pins = Pins::new(sheet, library);
        let (mut placed, mut lines) = (Vec::new(), Grid::default());
        for p in &sheet.parts {
            let Some(c) = library.get(&p.component) else { continue };
            for q in &c.symbol.pins {
                let q = p.placement.pin(q);
                let (a, b) = (q.at, q.body_end());
                lines.add([[a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]]);
                placed.push(RoomPin { part: p.id, number: q.number, end: q.at });
            }
        }
        let mut room = PortRoom { pins, placed, lines, labels: Grid::default(), label_ids: Vec::new() };
        for l in &sheet.labels {
            if let Some(at) = room.pins.anchor(&l.at) {
                room.labels.add(label_box(l.kind, &l.name, at, l.rotation));
                room.label_ids.push(l.id);
            }
        }
        room
    }

    /// A label put down since: later ones keep clear of it.
    pub fn add(&mut self, l: &Label) {
        if let Some(at) = self.pins.anchor(&l.at) {
            self.labels.add(label_box(l.kind, &l.name, at, l.rotation));
            self.label_ids.push(0);
        }
    }

    /// The way a power port points by default where it stands: a supply up and ground down, as
    /// schematics draw them; but straight out from its pin where that would point into the part,
    /// where the pin is one of a column of pins on the part's side (so the ports on a column
    /// stand side by side), or where it would run over another pin or label. A net label points
    /// straight out from its pin, and on a point to the right.
    pub fn turn(&self, l: &Label) -> u16 {
        let usual = match l.kind {
            LabelKind::Ground => 270,
            LabelKind::Rail => 90,
            LabelKind::Net => 0,
        };
        let Some(at) = self.pins.anchor(&l.at) else { return usual };
        let Some((part, number)) = l.at.pin() else { return usual };
        let Some(pin) = self.pins.part(part).zip(self.pins.pin(part, number)).map(|(p, q)| p.placement.pin(q)) else { return usual };
        let out = angle(opposite(pin.direction));
        if l.kind == LabelKind::Net {
            return out;
        }
        let own = (part, number);
        let side = horizontal(pin.direction);
        let fits = usual != angle(pin.direction) && !(side && self.in_column(at, own)) && self.clear(l, at, usual, own);
        if fits { usual } else { out }
    }

    /// Whether another pin's end lies within a grid step above or below `at`.
    fn in_column(&self, at: [f64; 2], own: (u64, &str)) -> bool {
        let reach = [[at[0] - 0.01, at[1] - GRID - 0.01], [at[0] + 0.01, at[1] + GRID + 0.01]];
        self.lines.near(reach).into_iter().filter_map(|i| self.placed.get(i)).any(|q| {
            (q.part, q.number.as_str()) != own
                && q.end[0] >= reach[0][0]
                && q.end[0] <= reach[1][0]
                && q.end[1] >= reach[0][1]
                && q.end[1] <= reach[1][1]
        })
    }

    /// Whether the port `l` at `at` pointing `rotation` runs over no pin but its own, and no
    /// other label.
    fn clear(&self, l: &Label, at: [f64; 2], rotation: u16, own: (u64, &str)) -> bool {
        let [lo, hi] = label_box(l.kind, &l.name, at, rotation);
        let b = [[lo[0] + 0.1, lo[1] + 0.1], [hi[0] - 0.1, hi[1] - 0.1]];
        let on_pin = self.lines.near(b).into_iter().any(|i| {
            self.placed.get(i).is_some_and(|q| (q.part, q.number.as_str()) != own) && self.lines.boxes.get(i).is_some_and(|line| meet(*line, b))
        });
        let skip = (l.id != 0).then_some(l.id);
        let on_label = self
            .labels
            .near(b)
            .into_iter()
            .any(|i| self.label_ids.get(i).is_some_and(|id| Some(*id) != skip) && self.labels.boxes.get(i).is_some_and(|o| meet(*o, b)));
        !on_pin && !on_label
    }
}
