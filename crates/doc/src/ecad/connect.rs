//! What is connected to what on the schematic: the nets, worked out from the sheet each time and
//! never saved.
//!
//! Points compare exactly, on the lattice of [`STEP`](super::place::STEP); a pin's point is its
//! part's `at` and the symbol point each put on the lattice, then mirrored and turned exactly (see
//! [`Placement::lattice_point`](super::Placement::lattice_point)).
//! - Everything at one point is connected: wire ends and bends, labels, markers and pins (pins that
//!   touch connect).
//! - A wire end, label or marker on a point (not on a pin), or a wire's bend, that lies inside
//!   another wire's segment connects to it: a T. Wires that only cross don't connect; wires that
//!   overlap along a line do.
//! - A pin inside a wire's segment does not connect (it is listed in
//!   [`Connectivity::pin_on_wire`]), and neither does a wire end on such a pin.
//! - Labels and power ports with the same name connect.
//!
//! A group with a wire, a label or two pins is a net. Its name: the smallest rail or ground port
//! name on it, else the smallest net label name, else `net:` and its lowest pin (`net:R1.2`), else
//! `net:wire` and its lowest wire id (`net:wire7`). Names typed by people can't hold `:`, so these
//! never clash with them.
//!
//! The work is O(N log N) in the sheet's objects: horizontal and vertical segments are kept per
//! line as sorted, merged intervals that points binary-search, and the few diagonal segments are
//! tested exactly against the points sorted by x, within a budget.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use super::schematic::{
    Anchor, LabelKind, Library, MAX_ALL_WIRE_POINTS, MAX_LABELS, MAX_MARKERS, MAX_PLACED_PINS, MAX_WIRE_POINTS, MAX_WIRES, Part, Schematic, WireEnd,
};
use super::{Electronics, Pin, PinType, place::lattice};

/// The most point-on-diagonal tests one pass makes; past it, T joins onto diagonal segments are
/// left out and [`Connectivity::truncated`] is set.
pub const MAX_DIAGONAL_TESTS: usize = 1 << 22;

/// A pin on the schematic.
#[derive(Clone, Debug, PartialEq)]
pub struct NetPin {
    pub part: u64,
    pub designator: String,
    /// The pin's number.
    pub pin: String,
    /// The pin's name, `""` when it has none.
    pub name: String,
    pub electrical: PinType,
    /// Where a wire connects, in millimetres.
    pub at: [f64; 2],
}

impl NetPin {
    /// `R1.2`: the designator, a dot and the pin's number.
    pub fn reference(&self) -> String {
        format!("{}.{}", self.designator, self.pin)
    }
}

/// One net: what is connected together.
#[derive(Clone, Debug, PartialEq)]
pub struct Net {
    /// The net's name (see the module notes).
    pub name: String,
    /// Whether a label or power port gives the name (`false` for `net:R1.2`).
    pub named: bool,
    /// Every name the labels and power ports on it give, sorted.
    pub names: Vec<String>,
    /// A rail or ground port is on it: the net is a supply (it drives power inputs).
    pub power: bool,
    /// Its pins, by designator then number (R2 before R10).
    pub pins: Vec<NetPin>,
    /// Its wires', labels' and markers' ids, ascending.
    pub wires: Vec<u64>,
    pub labels: Vec<u64>,
    pub markers: Vec<u64>,
}

/// A pin that lies inside a wire's segment without connecting to it.
#[derive(Clone, Debug, PartialEq)]
pub struct PinOnWire {
    pub pin: NetPin,
    pub wire: u64,
}

/// A wire end on a point where nothing else is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dangling {
    pub wire: u64,
    pub end: WireEnd,
    pub at: [f64; 2],
}

/// The sheet's connections (see the module notes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Connectivity {
    /// The nets, by name (natural order: N2 before N10).
    pub nets: Vec<Net>,
    /// Each wire's net (an index into `nets`), in the order of the sheet's `wires`.
    pub wire_nets: Vec<usize>,
    /// Each label's net, in the order of the sheet's `labels`.
    pub label_nets: Vec<usize>,
    /// Each marker's net, in the order of the sheet's `markers`; `None` for a marker on a pin or
    /// point that nothing else touches.
    pub marker_nets: Vec<Option<usize>>,
    /// Where junction dots go: three or more wire ends, bends, crossings-through or pins meet, or a
    /// wire end or bend makes a T.
    pub junctions: Vec<[f64; 2]>,
    /// Pins that touch nothing (markers aside), by designator then number.
    pub unconnected: Vec<NetPin>,
    /// Pins inside a wire's segment, which don't connect to it.
    pub pin_on_wire: Vec<PinOnWire>,
    /// Wire ends on a point (not a pin) that nothing else touches.
    pub dangling: Vec<Dangling>,
    /// Some of the sheet was past the caps or the diagonal budget, so not everything is joined.
    pub truncated: bool,
    by_pin: BTreeMap<(u64, String), usize>,
    by_name: BTreeMap<String, usize>,
}

impl Connectivity {
    /// The net named `name` (as [`Net::name`] gives it).
    pub fn net(&self, name: &str) -> Option<&Net> {
        self.index_of(name).and_then(|i| self.nets.get(i))
    }

    /// The index in `nets` of the net named `name`.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }

    /// The net a part's pin is on; `None` for a pin that touches nothing.
    pub fn pin_net(&self, part: u64, pin: &str) -> Option<usize> {
        self.by_pin.get(&(part, pin.to_string())).copied()
    }

    /// The net the wire with this id is on.
    pub fn wire_net(&self, sheet: &Schematic, wire: u64) -> Option<usize> {
        let i = sheet.wires.iter().position(|w| w.id == wire)?;
        self.wire_nets.get(i).copied()
    }

    /// The net the label with this id is on.
    pub fn label_net(&self, sheet: &Schematic, label: u64) -> Option<usize> {
        let i = sheet.labels.iter().position(|l| l.id == label)?;
        self.label_nets.get(i).copied()
    }
}

impl Electronics {
    /// The schematic's nets (see [`connectivity`]).
    pub fn connectivity(&self) -> Connectivity {
        connectivity(&self.schematic, &self.library())
    }
}

/// The most pins a [`NetChange`] lists on each side.
pub const MAX_CHANGED_PINS: usize = 64;

/// How one net's pins changed between two states of the sheet: the pins it `lost` and those it
/// `gained` (as `R1.2`, by designator then number, at most [`MAX_CHANGED_PINS`] each; `more`
/// counts the ones left out).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetChange {
    pub net: String,
    pub lost: Vec<String>,
    pub gained: Vec<String>,
    pub more: usize,
}

/// Each pin's net, by part and pin number.
fn pin_nets(c: &Connectivity) -> BTreeMap<(u64, &str), usize> {
    let mut out = BTreeMap::new();
    for (i, n) in c.nets.iter().enumerate() {
        for p in &n.pins {
            out.insert((p.part, p.pin.as_str()), i);
        }
    }
    out
}

/// Each name the labels and ports give, and the net that has it.
fn name_nets(c: &Connectivity) -> BTreeMap<&str, usize> {
    let mut out = BTreeMap::new();
    for (i, n) in c.nets.iter().enumerate() {
        for name in &n.names {
            out.entry(name.as_str()).or_insert(i);
        }
    }
    out
}

/// For each net of `from`, the net of the other state that goes on with it: the one with one of
/// its names (`names`: each name the labels and ports give there, and its net), else the one
/// holding most of its pins (`pins`).
fn matched(from: &Connectivity, pins: &BTreeMap<(u64, &str), usize>, names: &BTreeMap<&str, usize>) -> Vec<Option<usize>> {
    let follow = |n: &Net| {
        if let Some(k) = n.names.iter().find_map(|name| names.get(name.as_str())) {
            return Some(*k);
        }
        let mut count: BTreeMap<usize, usize> = BTreeMap::new();
        for p in &n.pins {
            if let Some(k) = pins.get(&(p.part, p.pin.as_str())) {
                *count.entry(*k).or_insert(0) += 1;
            }
        }
        // The most pins; the lowest net where two hold as many.
        count.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(k, _)| k)
    };
    from.nets.iter().map(follow).collect()
}

/// The nets whose pins changed from `before` to `after`, by name in natural order. Each net of one
/// state is matched with the net of the other that goes on with it: the one with its name (a name
/// the labels and ports on it give), else the one holding most of its pins. A pin that leaves its
/// net's match is lost from that net; a pin that comes from elsewhere into a net is gained by it.
/// A pin moved from one net to another shows as both.
pub fn net_changes(before: &Connectivity, after: &Connectivity) -> Vec<NetChange> {
    let (was, now) = (pin_nets(before), pin_nets(after));
    let forward = matched(before, &now, &name_nets(after));
    let back = matched(after, &was, &name_nets(before));
    let mut out: BTreeMap<String, (Vec<&NetPin>, Vec<&NetPin>)> = BTreeMap::new();
    for (i, n) in before.nets.iter().enumerate() {
        let goes = forward.get(i).copied().flatten();
        let lost: Vec<&NetPin> = n.pins.iter().filter(|p| now.get(&(p.part, p.pin.as_str())).copied() != goes || goes.is_none()).collect();
        if !lost.is_empty() {
            out.entry(n.name.clone()).or_default().0.extend(lost);
        }
    }
    for (j, n) in after.nets.iter().enumerate() {
        let came = back.get(j).copied().flatten();
        let gained: Vec<&NetPin> = n.pins.iter().filter(|p| was.get(&(p.part, p.pin.as_str())).copied() != came || came.is_none()).collect();
        if !gained.is_empty() {
            out.entry(n.name.clone()).or_default().1.extend(gained);
        }
    }
    let mut changes: Vec<NetChange> = out
        .into_iter()
        .map(|(net, (lost, gained))| {
            let more = lost.len().saturating_sub(MAX_CHANGED_PINS).saturating_add(gained.len().saturating_sub(MAX_CHANGED_PINS));
            let refs = |v: Vec<&NetPin>| v.into_iter().take(MAX_CHANGED_PINS).map(NetPin::reference).collect();
            NetChange { net, lost: refs(lost), gained: refs(gained), more }
        })
        .collect();
    changes.sort_by(|a, b| natural_cmp(&a.net, &b.net));
    changes
}

/// `a` and `b` compared with runs of digits as numbers: `R2` before `R10`, `N1` before `N01b`.
/// Equal only when the texts are.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.as_bytes(), b.as_bytes());
    loop {
        match (x.first(), y.first()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let (n, rest_x) = digits(x);
                let (m, rest_y) = digits(y);
                let (tn, tm) = (trim_zeros(n), trim_zeros(m));
                let order = tn.len().cmp(&tm.len()).then_with(|| tn.cmp(tm));
                if order != Ordering::Equal {
                    return order;
                }
                (x, y) = (rest_x, rest_y);
            }
            (Some(c), Some(d)) => {
                if c != d {
                    return c.cmp(d);
                }
                (x, y) = (x.get(1..).unwrap_or_default(), y.get(1..).unwrap_or_default());
            }
        }
    }
}

fn digits(s: &[u8]) -> (&[u8], &[u8]) {
    let n = s.iter().take_while(|b| b.is_ascii_digit()).count();
    (s.get(..n).unwrap_or_default(), s.get(n..).unwrap_or_default())
}

fn trim_zeros(s: &[u8]) -> &[u8] {
    let n = s.iter().take_while(|b| **b == b'0').count();
    s.get(n..).unwrap_or_default()
}

/// A lattice point back in millimetres.
pub fn from_lattice([x, y]: [i64; 2]) -> [f64; 2] {
    [x as f64 / 1e4 + 0.0, y as f64 / 1e4 + 0.0]
}

/// Disjoint sets over `0..n`; a set's root is its lowest member.
pub(super) struct Sets {
    up: Vec<usize>,
}

impl Sets {
    pub(super) fn new(n: usize) -> Self {
        Sets { up: (0..n).collect() }
    }

    /// Make it `0..n` (from fewer): the new members alone.
    pub(super) fn grow(&mut self, n: usize) {
        let from = self.up.len();
        self.up.extend(from..n);
    }

    pub(super) fn root(&mut self, mut i: usize) -> usize {
        // Parents are always lower, so the walk ends; halving the path keeps it short.
        loop {
            let Some(&p) = self.up.get(i) else { return i };
            if p == i {
                return i;
            }
            let g = self.up.get(p).copied().unwrap_or(p);
            if let Some(slot) = self.up.get_mut(i) {
                *slot = g;
            }
            i = g;
        }
    }

    pub(super) fn join(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.root(a), self.root(b));
        if ra != rb
            && let Some(slot) = self.up.get_mut(ra.max(rb))
        {
            *slot = ra.min(rb);
        }
    }
}

/// The segments along one horizontal or vertical line: merged, sorted intervals (each with a wire
/// in it) and every segment's end coordinates.
#[derive(Default)]
struct Line {
    spans: Vec<(i64, i64, usize)>,
    ends: Vec<i64>,
}

impl Line {
    /// Sort and merge the intervals, joining the wires of intervals that overlap or touch.
    fn settle(&mut self, sets: &mut Sets) {
        self.spans.sort_unstable();
        let mut merged: Vec<(i64, i64, usize)> = Vec::with_capacity(self.spans.len());
        for &(a, b, w) in &self.spans {
            match merged.last_mut() {
                Some(last) if a <= last.1 => {
                    sets.join(last.2, w);
                    last.1 = last.1.max(b);
                }
                _ => merged.push((a, b, w)),
            }
        }
        self.spans = merged;
        self.ends.sort_unstable();
        self.ends.dedup();
    }

    /// A wire whose segment has `c` strictly inside it, `c` being no segment's end on this line.
    /// Whether `c` is strictly inside a merged interval (some segment runs through it).
    fn covers(&self, c: i64) -> bool {
        let i = self.spans.partition_point(|s| s.0 < c);
        i.checked_sub(1).and_then(|j| self.spans.get(j)).is_some_and(|s| c < s.1)
    }

    fn inside(&self, c: i64) -> Option<usize> {
        if self.ends.binary_search(&c).is_ok() {
            return None;
        }
        let i = self.spans.partition_point(|s| s.0 < c);
        let (_, b, w) = *self.spans.get(i.checked_sub(1)?)?;
        (c < b).then_some(w)
    }
}

/// The segment index: horizontal segments by their y, vertical ones by their x, and the
/// diagonal ones.
#[derive(Default)]
struct Segments {
    rows: BTreeMap<i64, Line>,
    columns: BTreeMap<i64, Line>,
    diagonals: Vec<([i64; 2], [i64; 2], usize)>,
}

impl Segments {
    fn add(&mut self, a: [i64; 2], b: [i64; 2], wire: usize) {
        if a == b {
            return;
        }
        if a[1] == b[1] {
            let line = self.rows.entry(a[1]).or_default();
            line.spans.push((a[0].min(b[0]), a[0].max(b[0]), wire));
            line.ends.extend([a[0], b[0]]);
        } else if a[0] == b[0] {
            let line = self.columns.entry(a[0]).or_default();
            line.spans.push((a[1].min(b[1]), a[1].max(b[1]), wire));
            line.ends.extend([a[1], b[1]]);
        } else {
            self.diagonals.push((a, b, wire));
        }
    }

    fn settle(&mut self, sets: &mut Sets) {
        for line in self.rows.values_mut().chain(self.columns.values_mut()) {
            line.settle(sets);
        }
    }

    /// The wires (a representative per merged run) whose horizontal or vertical segment has `q`
    /// strictly inside.
    /// Whether a horizontal or vertical segment runs through `q` (an end of one there may stop at
    /// it).
    fn covers(&self, q: [i64; 2]) -> bool {
        self.rows.get(&q[1]).is_some_and(|l| l.covers(q[0])) || self.columns.get(&q[0]).is_some_and(|l| l.covers(q[1]))
    }

    fn inside(&self, q: [i64; 2]) -> impl Iterator<Item = usize> + '_ {
        let row = self.rows.get(&q[1]).and_then(|l| l.inside(q[0]));
        let column = self.columns.get(&q[0]).and_then(|l| l.inside(q[1]));
        row.into_iter().chain(column)
    }
}

/// Whether `p` lies strictly inside the segment `a`–`b` (exact).
fn strictly_on(a: [i64; 2], b: [i64; 2], p: [i64; 2]) -> bool {
    if p == a || p == b {
        return false;
    }
    let (ax, ay, bx, by, px, py) = (i128::from(a[0]), i128::from(a[1]), i128::from(b[0]), i128::from(b[1]), i128::from(p[0]), i128::from(p[1]));
    // Collinear: the cross product is 0, so its two terms are equal (products past i128 aren't).
    let collinear = (bx - ax).checked_mul(py - ay).zip((by - ay).checked_mul(px - ax)).is_some_and(|(u, v)| u == v);
    collinear && px >= ax.min(bx) && px <= ax.max(bx) && py >= ay.min(by) && py <= ay.max(by)
}

/// What sits at a point.
#[derive(Clone, Copy)]
struct At {
    q: [i64; 2],
    node: usize,
    /// How many wire legs it brings to the point, for junction dots.
    arms: u32,
    /// A wire end on a point (not a pin): the wire's index and which end.
    end: Option<(usize, WireEnd)>,
}

/// A point that joins a wire it lies inside (a T).
#[derive(Clone, Copy)]
struct Probe {
    q: [i64; 2],
    node: usize,
    /// The legs the T brings for junction dots: 2 for a wire end or bend, 0 for a label or marker.
    arms: u32,
    /// Its entry in the list of what sits at points.
    at: usize,
}

/// The joins being worked out.
struct Build {
    sets: Sets,
    at: Vec<At>,
    probes: Vec<Probe>,
    /// Which entries of `at` made a T.
    touched: Vec<bool>,
    /// The legs T joins bring to their points.
    arms: Vec<([i64; 2], u32)>,
}

impl Build {
    /// Something at `q`; with `probe`, it also joins a wire it lies inside (bringing that many legs).
    fn point(&mut self, q: [i64; 2], node: usize, arms: u32, end: Option<(usize, WireEnd)>, probe: Option<u32>) {
        if let Some(arms) = probe {
            self.probes.push(Probe { q, node, arms, at: self.at.len() });
        }
        self.at.push(At { q, node, arms, end });
    }

    /// A wire's end: joined to its pin (`pin` is the pin's node), or on its point.
    fn end(&mut self, wire: usize, q: [i64; 2], pin: Option<usize>, which: WireEnd) {
        match pin {
            Some(p) => {
                self.sets.join(wire, p);
                self.point(q, wire, 1, None, None);
            }
            None => self.point(q, wire, 1, Some((wire, which)), Some(2)),
        }
    }

    /// A label or marker: joined to its pin, or on its point (where it may make a T).
    fn anchor(&mut self, node: usize, q: [i64; 2], pin: Option<usize>) {
        match pin {
            Some(p) => {
                self.sets.join(node, p);
                self.point(q, node, 0, None, None);
            }
            None => self.point(q, node, 0, None, Some(0)),
        }
    }

    /// The probe `p` lies inside a segment of `wire`.
    fn hit(&mut self, p: Probe, wire: usize) {
        self.sets.join(p.node, wire);
        if p.arms > 0 {
            self.arms.push((p.q, p.arms));
        }
        if let Some(t) = self.touched.get_mut(p.at) {
            *t = true;
        }
    }
}

/// What one group of joined nodes holds, by index into the sheet's lists (pins: into the pin list).
#[derive(Default)]
struct Group {
    wires: Vec<usize>,
    pins: Vec<usize>,
    labels: Vec<usize>,
    markers: Vec<usize>,
}

/// The nets of `sheet` (see the module notes); parts whose component `library` lacks have no pins.
pub fn connectivity(sheet: &Schematic, library: &Library) -> Connectivity {
    let mut out = Connectivity::default();
    let wires: Vec<_> = sheet.wires.iter().take(MAX_WIRES).collect();
    let labels: Vec<_> = sheet.labels.iter().take(MAX_LABELS).collect();
    let markers: Vec<_> = sheet.markers.iter().take(MAX_MARKERS).collect();
    out.truncated = wires.len() < sheet.wires.len() || labels.len() < sheet.labels.len() || markers.len() < sheet.markers.len();

    // The pins, each with its point.
    let mut pins: Vec<(&Part, &Pin, [i64; 2])> = Vec::new();
    'parts: for part in &sheet.parts {
        let Some(c) = library.get(&part.component) else { continue };
        for pin in &c.symbol.pins {
            if pins.len() >= MAX_PLACED_PINS {
                out.truncated = true;
                break 'parts;
            }
            pins.push((part, pin, part.placement.lattice_point(pin.at)));
        }
    }
    let pin_index: BTreeMap<(u64, &str), usize> = pins.iter().enumerate().map(|(i, (p, q, _))| ((p.id, q.number.as_str()), i)).collect();

    // Nodes: wires, then pins, then labels, then markers.
    let (nw, np, nl, nm) = (wires.len(), pins.len(), labels.len(), markers.len());
    let pin_node = |i: usize| nw + i;
    let label_node = |i: usize| nw + np + i;
    let marker_node = |i: usize| nw + np + nl + i;
    let total = nw + np + nl + nm;

    // An anchor's point, and its pin's node when it is on one.
    let resolve = |a: &Anchor| -> Option<([i64; 2], Option<usize>)> {
        match a {
            Anchor::Point(p) => Some((lattice(*p), None)),
            Anchor::Pin { part, pin } => {
                let i = *pin_index.get(&(*part, pin.as_str()))?;
                pins.get(i).map(|(_, _, q)| (*q, Some(pin_node(i))))
            }
        }
    };

    let mut b = Build { sets: Sets::new(total), at: Vec::new(), probes: Vec::new(), touched: Vec::new(), arms: Vec::new() };
    let mut segments = Segments::default();
    let mut points = 0usize;
    for (i, w) in wires.iter().enumerate() {
        let via: Vec<[i64; 2]> = w.via.iter().take(MAX_WIRE_POINTS.saturating_sub(2)).map(|v| lattice(*v)).collect();
        if via.len() < w.via.len() {
            out.truncated = true;
        }
        let mut line: Vec<[i64; 2]> = Vec::with_capacity(via.len().saturating_add(2));
        if let Some((q, pin)) = resolve(&w.from) {
            line.push(q);
            b.end(i, q, pin, WireEnd::From);
        }
        for q in via {
            line.push(q);
            b.point(q, i, 2, None, Some(2));
        }
        if let Some((q, pin)) = resolve(&w.to) {
            line.push(q);
            b.end(i, q, pin, WireEnd::To);
        }
        points = points.saturating_add(line.len());
        for pair in line.windows(2) {
            if let [p, q] = pair {
                segments.add(*p, *q, i);
            }
        }
    }
    if points > MAX_ALL_WIRE_POINTS {
        out.truncated = true;
    }
    for (i, (_, _, q)) in pins.iter().enumerate() {
        b.point(*q, pin_node(i), 1, None, None);
    }
    for (i, l) in labels.iter().enumerate() {
        if let Some((q, pin)) = resolve(&l.at) {
            b.anchor(label_node(i), q, pin);
        }
    }
    for (i, m) in markers.iter().enumerate() {
        if let Some((q, pin)) = resolve(&m.at) {
            b.anchor(marker_node(i), q, pin);
        }
    }
    b.touched = vec![false; b.at.len()];

    // Everything at one point joins; `crowd` counts what shares each entry's point.
    let mut order: Vec<usize> = (0..b.at.len()).collect();
    order.sort_unstable_by_key(|&k| b.at.get(k).map(|a| a.q));
    let mut crowd = vec![1usize; b.at.len()];
    for run in order.chunk_by(|&x, &y| b.at.get(x).map(|a| a.q) == b.at.get(y).map(|a| a.q)) {
        let nodes: Vec<usize> = run.iter().filter_map(|&k| b.at.get(k).map(|a| a.node)).collect();
        if let Some(&first) = nodes.first() {
            for &n in &nodes {
                b.sets.join(first, n);
            }
        }
        for &k in run {
            if let Some(c) = crowd.get_mut(k) {
                *c = run.len();
            }
        }
    }

    // Wires overlapping along a line join; then the T joins.
    segments.settle(&mut b.sets);
    for k in 0..b.probes.len() {
        let Some(p) = b.probes.get(k).copied() else { continue };
        let inside: Vec<usize> = segments.inside(p.q).collect();
        for wire in inside {
            b.hit(p, wire);
        }
    }
    // A pin inside a segment doesn't join it; it is reported, unless a wire's bend or an end on a
    // point is on it too (that joins both the pin and the wire it lies inside).
    let wire_points: Vec<[i64; 2]> = {
        let mut v: Vec<[i64; 2]> = b.at.iter().filter(|a| a.node < nw && (a.end.is_some() || a.arms == 2)).map(|a| a.q).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let mut on_wire: Vec<(usize, usize)> = Vec::new();
    for (i, (_, _, q)) in pins.iter().enumerate() {
        if wire_points.binary_search(q).is_err()
            && let Some(w) = segments.inside(*q).next()
        {
            on_wire.push((i, w));
        }
    }
    // Diagonal segments, tested against the points sorted by x.
    if !segments.diagonals.is_empty() {
        let mut xs: Vec<([i64; 2], Option<usize>, Option<usize>)> = b.probes.iter().enumerate().map(|(k, p)| (p.q, Some(k), None)).collect();
        xs.extend(pins.iter().enumerate().filter(|(_, (_, _, q))| wire_points.binary_search(q).is_err()).map(|(i, (_, _, q))| (*q, None, Some(i))));
        xs.sort_unstable_by_key(|(q, _, _)| *q);
        let mut budget = MAX_DIAGONAL_TESTS;
        'diagonals: for &(p0, p1, wire) in &segments.diagonals {
            let (x0, x1) = (p0[0].min(p1[0]), p0[0].max(p1[0]));
            let start = xs.partition_point(|(q, _, _)| q[0] <= x0);
            for (q, probe, pin) in xs.iter().skip(start).take_while(|(q, _, _)| q[0] < x1) {
                if budget == 0 {
                    out.truncated = true;
                    break 'diagonals;
                }
                budget -= 1;
                if !strictly_on(p0, p1, *q) {
                    continue;
                }
                if let Some(p) = probe.and_then(|k| b.probes.get(k).copied()) {
                    b.hit(p, wire);
                }
                if let Some(i) = *pin
                    && !on_wire.iter().any(|(j, _)| *j == i)
                {
                    on_wire.push((i, wire));
                }
            }
        }
    }

    // Equal names join.
    let mut first_named: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, l) in labels.iter().enumerate() {
        let first = *first_named.entry(l.name.as_str()).or_insert(label_node(i));
        b.sets.join(first, label_node(i));
    }

    // The groups, by root.
    let mut group_of = vec![usize::MAX; total];
    let mut groups: Vec<Group> = Vec::new();
    for node in 0..total {
        let root = b.sets.root(node);
        let g = match group_of.get(root).copied() {
            Some(g) if g != usize::MAX => g,
            _ => {
                groups.push(Group::default());
                let g = groups.len() - 1;
                if let Some(slot) = group_of.get_mut(root) {
                    *slot = g;
                }
                g
            }
        };
        if let Some(slot) = group_of.get_mut(node) {
            *slot = g;
        }
        let Some(group) = groups.get_mut(g) else { continue };
        if node < nw {
            group.wires.push(node);
        } else if node < nw + np {
            group.pins.push(node - nw);
        } else if node < nw + np + nl {
            group.labels.push(node - nw - np);
        } else {
            group.markers.push(node - nw - np - nl);
        }
    }

    let net_pin = |i: usize| -> Option<NetPin> {
        let (part, pin, q) = pins.get(i)?;
        Some(NetPin {
            part: part.id,
            designator: part.designator.clone(),
            pin: pin.number.clone(),
            name: pin.name.clone(),
            electrical: pin.electrical,
            at: from_lattice(*q),
        })
    };
    let pin_order = |a: &NetPin, b: &NetPin| natural_cmp(&a.designator, &b.designator).then_with(|| natural_cmp(&a.pin, &b.pin));

    // The nets, each with the group it came from.
    let mut nets: Vec<(Net, usize)> = Vec::new();
    for (g, group) in groups.iter().enumerate() {
        if group.wires.is_empty() && group.labels.is_empty() && group.pins.len() < 2 {
            out.unconnected.extend(group.pins.iter().filter_map(|&i| net_pin(i)));
            continue;
        }
        nets.push((net(group, &wires, &labels, &markers, &net_pin, &pin_order), g));
    }
    nets.sort_by(|(x, _), (y, _)| natural_cmp(&x.name, &y.name));
    let mut net_of_group: Vec<Option<usize>> = vec![None; groups.len()];
    for (k, (_, g)) in nets.iter().enumerate() {
        if let Some(slot) = net_of_group.get_mut(*g) {
            *slot = Some(k);
        }
    }
    let net_of = |node: usize| group_of.get(node).and_then(|g| net_of_group.get(*g).copied().flatten());
    out.wire_nets = (0..nw).map(|i| net_of(i).unwrap_or_default()).collect();
    out.label_nets = (0..nl).map(|i| net_of(label_node(i)).unwrap_or_default()).collect();
    out.marker_nets = (0..nm).map(|i| net_of(marker_node(i))).collect();
    for (i, (part, pin, _)) in pins.iter().enumerate() {
        if let Some(k) = net_of(pin_node(i)) {
            out.by_pin.insert((part.id, pin.number.clone()), k);
        }
    }
    out.nets = nets.into_iter().map(|(n, _)| n).collect();
    out.by_name = out.nets.iter().enumerate().map(|(k, n)| (n.name.clone(), k)).collect();
    out.unconnected.sort_by(pin_order);

    // Junction dots: three or more legs at a point, or a T.
    let mut arms: Vec<([i64; 2], u32)> = b.at.iter().filter(|a| a.arms > 0).map(|a| (a.q, a.arms)).collect();
    arms.append(&mut b.arms);
    arms.sort_unstable_by_key(|(q, _)| *q);
    for run in arms.chunk_by(|x, y| x.0 == y.0) {
        let n = run.iter().map(|(_, k)| *k).fold(0u32, u32::saturating_add);
        if n >= 3
            && let Some((q, _)) = run.first()
        {
            out.junctions.push(from_lattice(*q));
        }
    }

    // Wire ends on a point nothing else is at.
    for (k, a) in b.at.iter().enumerate() {
        let Some((i, end)) = a.end else { continue };
        if crowd.get(k).copied().unwrap_or(1) == 1
            && !b.touched.get(k).copied().unwrap_or(false)
            && !segments.covers(a.q)
            && let Some(w) = wires.get(i)
        {
            out.dangling.push(Dangling { wire: w.id, end, at: from_lattice(a.q) });
        }
    }

    on_wire.sort_unstable();
    out.pin_on_wire = on_wire.into_iter().filter_map(|(i, w)| Some(PinOnWire { pin: net_pin(i)?, wire: wires.get(w)?.id })).collect();
    out
}

/// The net a group makes: its members in order, and its name (see the module notes).
fn net(
    group: &Group,
    wires: &[&super::Wire],
    labels: &[&super::Label],
    markers: &[&super::Marker],
    net_pin: &dyn Fn(usize) -> Option<NetPin>,
    pin_order: &dyn Fn(&NetPin, &NetPin) -> Ordering,
) -> Net {
    let mut pins: Vec<NetPin> = group.pins.iter().filter_map(|&i| net_pin(i)).collect();
    pins.sort_by(|a, b| pin_order(a, b));
    let ids = |v: &[usize], id: &dyn Fn(usize) -> Option<u64>| {
        let mut v: Vec<u64> = v.iter().filter_map(|&i| id(i)).collect();
        v.sort_unstable();
        v
    };
    let wire_ids = ids(&group.wires, &|i| wires.get(i).map(|w| w.id));
    let label_ids = ids(&group.labels, &|i| labels.get(i).map(|l| l.id));
    let marker_ids = ids(&group.markers, &|i| markers.get(i).map(|m| m.id));
    let on: Vec<&super::Label> = group.labels.iter().filter_map(|&i| labels.get(i).copied()).collect();
    let smallest = |power: bool| {
        on.iter().filter(|l| matches!(l.kind, LabelKind::Rail | LabelKind::Ground) == power).map(|l| l.name.as_str()).min_by(|a, b| natural_cmp(a, b))
    };
    let mut names: Vec<String> = on.iter().map(|l| l.name.clone()).collect();
    names.sort_by(|a, b| natural_cmp(a, b));
    names.dedup();
    let power = smallest(true);
    let (name, named) = match power.or_else(|| smallest(false)) {
        Some(n) => (n.to_string(), true),
        None => match (pins.first(), wire_ids.first()) {
            (Some(p), _) => (format!("net:{}", p.reference()), false),
            (None, Some(w)) => (format!("net:wire{w}"), false),
            (None, None) => (String::from("net:"), false),
        },
    };
    Net { name, named, names, power: power.is_some(), pins, wires: wire_ids, labels: label_ids, markers: marker_ids }
}
