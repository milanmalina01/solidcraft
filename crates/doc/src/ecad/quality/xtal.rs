//! Crystals: the two nets between an oscillator's pins and its crystal are high-impedance,
//! low-level nodes that stray capacitance pulls off frequency and nearby edges disturb.
//! - `si.xtal_loop`: the routed length from each oscillator pin to its crystal pad, against
//!   `mhz_max` (a MHz crystal) or `khz_max` (a watch crystal, below 1 MHz): a warning, an error
//!   past `error_max` (which needs no frequency); a via on a crystal net (an error); a load
//!   capacitor's pad more than `cap_max` from its crystal pad, edge to edge; the crystal or a load
//!   capacitor on the other side from its oscillator; the two nets' lengths more than `mismatch`
//!   apart (an info). An inner ball of an area-array package (BGA, aQFN) is measured from its
//!   breakout via (one within `breakout` of its pad), and that via is no crystal via. An unrouted
//!   net is measured in a straight line, an estimate no route can beat.
//! - `si.xtal_keepout`: other nets' copper near the crystal's zone (its courtyard, its load
//!   capacitors' and its nets' tracks, cut where they enter the oscillator's courtyard) on the
//!   crystal's layer and the next one in: any within `margin` (a warning); a fast (T1 or faster)
//!   or high-current (`current_min`) net's within `fast_warn` (a warning) or `fast_error` (an
//!   error), critical for a watch crystal. Ground (the can's and the load capacitors' ground) and
//!   the oscillator's own pads are never foreign. Also a test point on a crystal net (a warning),
//!   and infos: a thin ground track round the crystal with few vias (a loop that picks noise up
//!   rather than a guard), the crystal near the board's edge or an I/O connector (any not kept as
//!   cable-free).
//!
//! A crystal is a part kept with the role crystal whose `of` names its oscillator's two pins
//! (`U1.XC1` by name or `U1.B24` by number); its two nets are the crystal nets. A crystal part
//! with no two such pins (an oscillator module) follows the clock rules and is left alone. Its
//! load capacitors are the parts kept as load_cap `of` it. Its frequency is the crystal nets'
//! `f_mhz` (inferred from the crystal's value).
//!
//! The copper helpers here ([`Layered`]: a layer's copper as pieces with a grid; [`NetGraph`]: a
//! net's copper as a graph, for lengths along it) serve `rf.rs` too.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::super::airwires::BoardPad;
use super::super::board::{BoardFootprint, footprint_box};
use super::super::checks::{Severity, gap};
use super::super::copper::{TOLERANCE, core_bbox, disc, pad_core, pieces, stroke, track_cores};
use super::super::drc::{Bound, Class, DrcItem};
use super::super::footprint::{Core, PadKind, Side};
use super::super::grid::{Grid, boxes_meet, grow};
use super::super::intent::PartRole;
use super::super::layers::CopperLayer;
use super::super::path;
use super::catalog::{Code, Family, Param};
use super::geom::distance;
use super::pi_current::{Feeds, Item};
use super::{Cx, Draft, num};

const LOOP: &str = "si.xtal_loop";
const KEEPOUT: &str = "si.xtal_keepout";

/// Crystals below this frequency, in MHz, are watch crystals (32.768 kHz): tighter limits, and
/// fast copper near them is critical.
const WATCH_MHZ: f64 = 1.0;
/// The most nodes a net's graph may have; a net with more copper isn't measured.
const MAX_NODES: usize = 4096;
/// The cell of a layer's copper grid, in millimetres.
const CELL: f64 = 1.0;

const fn param(name: &'static str, default: f64, units: &'static str, min: f64, max: f64, source: &'static str, about: &'static str) -> Param {
    Param { name, default, units, source, min, max, verify: false, about }
}

const CODES: &[Code] = &[
    Code {
        code: LOOP,
        class: Class::Critical,
        severity: Severity::Error,
        worst: None,
        needs: "intent: crystal parts (crystal of the oscillator's pins), their load capacitors and the crystal nets' f_mhz",
        section: "signal-integrity#crystals",
        about: "an oscillator pin routed far from its crystal (a warning, an error past error_max), a via on a crystal net (an error), a load capacitor far from its crystal pad, the crystal or a load capacitor on the other side from its oscillator; the two nets' lengths far apart (info)",
        principle: "A crystal's pins are high-impedance, low-level nodes: every millimetre of track and every via adds stray capacitance that pulls the frequency and picks up noise.",
        params: &[
            param("mhz_max", 5.0, "mm", 0.1, 100.0, "Microchip", "a MHz crystal's pin-to-pad routed length above this is a warning"),
            param("khz_max", 3.0, "mm", 0.1, 100.0, "Microchip", "a watch crystal's (below 1 MHz) pin-to-pad routed length above this is a warning"),
            param("error_max", 12.0, "mm", 0.1, 200.0, "Microchip", "a pin-to-pad routed length above this is an error"),
            param(
                "cap_max",
                3.0,
                "mm",
                0.0,
                50.0,
                "Microchip; ST AN2867",
                "a load capacitor's pad farther than this from its crystal pad, edge to edge, is a warning",
            ),
            param(
                "mismatch",
                0.3,
                "ratio",
                0.0,
                1.0,
                "judgment",
                "the two crystal nets' routed lengths differing by more than this share are an info",
            ),
            param(
                "breakout",
                1.0,
                "mm",
                0.0,
                5.0,
                "judgment",
                "a via this close to an inner ball's pad (BGA, aQFN) is its breakout: the length starts there and it is no crystal via",
            ),
        ],
    },
    Code {
        code: KEEPOUT,
        class: Class::Practical,
        severity: Severity::Error,
        worst: Some((Class::Critical, Severity::Error)),
        needs: "intent: crystal parts (crystal of the oscillator's pins), the ground net, each net's tier (driver or edge_ns) or current_a",
        section: "signal-integrity#crystals",
        about: "another net's copper near a crystal on its layer or the next (a warning; fast or high-current copper: an error within fast_error, critical for a watch crystal), a test point on a crystal net; a thin ground loop round it, the crystal near the edge or a connector (info)",
        principle: "Edges near a crystal couple into its high-impedance nodes and pull or stop the oscillator; a stitched ground pour under and round it shields it, a long thin ground track round it is a pickup loop.",
        params: &[
            param(
                "margin",
                0.5,
                "mm",
                0.0,
                10.0,
                "judgment",
                "other nets' copper this close to the crystal's zone (its courtyard, load capacitors and tracks) is a warning",
            ),
            param("fast_warn", 2.0, "mm", 0.0, 50.0, "judgment", "fast or high-current copper this close to the zone is a warning"),
            param("fast_error", 1.0, "mm", 0.0, 50.0, "judgment", "fast or high-current copper this close to the zone is an error"),
            param(
                "current_min",
                0.5,
                "A",
                0.0,
                10_000.0,
                "judgment: the intent's high-current threshold",
                "a net carrying this much or more is high-current copper",
            ),
            param("edge", 3.0, "mm", 0.0, 50.0, "judgment", "a crystal closer than this to the board's edge is an info"),
            param("connector", 10.0, "mm", 0.0, 200.0, "judgment", "a crystal closer than this to an I/O connector is an info"),
            param("guard_width", 0.5, "mm", 0.0, 10.0, "TI SWRA640", "a ground track round the crystal narrower than this is a thin guard"),
            param("guard_length", 10.0, "mm", 0.0, 1000.0, "TI SWRA640", "a thin guard longer than this with few vias is an info"),
            param("guard_pitch", 3.0, "mm", 0.1, 100.0, "TI SWRA640", "a thin guard needs a ground via at least every this far"),
        ],
    },
];

pub(super) const FAMILY: Family = Family { module: "xtal", title: "crystals", codes: CODES, run };

fn run(cx: &mut Cx) {
    let crystals = crystals(cx);
    if cx.wants(LOOP) {
        for c in &crystals {
            if cx.spent() {
                return;
            }
            loops(cx, c);
        }
    }
    if cx.wants(KEEPOUT) {
        let mut layers: BTreeMap<CopperLayer, Option<Layered>> = BTreeMap::new();
        for c in &crystals {
            if cx.spent() {
                return;
            }
            keepout(cx, c, &mut layers);
        }
    }
}

/// One piece of copper on a layer (see [`Layered`]).
pub(super) struct Piece {
    pub core: Core,
    pub net: Option<usize>,
    pub item: DrcItem,
    /// The footprint's id, for a pad.
    pub footprint: Option<u64>,
    /// An edge of a pour's fill: the fill's copper lies on one side of it.
    pub edge: bool,
}

/// A layer's copper: tracks' straight pieces, vias' rings, pads, and the edges of the pours'
/// fills, with a grid; and the fills (by index into the board's fills), for what lies inside them.
pub(super) struct Layered {
    pub pieces: Vec<Piece>,
    pub fills: Vec<usize>,
    grid: Grid,
}

impl Layered {
    /// The copper on `layer`; `None` once the family's budget is spent.
    pub(super) fn new(cx: &mut Cx, layer: CopperLayer) -> Option<Layered> {
        let (board, nets) = (cx.board, cx.nets);
        let rough = board.tracks.len().saturating_add(board.vias.len()).saturating_add(nets.pads.len());
        if !cx.spend(rough.saturating_add(1)) {
            return None;
        }
        let mut pieces = Vec::new();
        for (ti, t) in board.tracks.iter().enumerate().filter(|(_, t)| t.layer == layer) {
            let net = nets.track_nets.get(ti).copied().flatten();
            pieces.extend(track_cores(t).map(|core| Piece { core, net, item: DrcItem::Track(t.id), footprint: None, edge: false }));
        }
        for (vi, v) in board.vias.iter().enumerate().filter(|(_, v)| v.on(layer, board.copper_layers)) {
            let net = nets.via_nets.get(vi).copied().flatten();
            pieces.push(Piece { core: disc(v.at, v.diameter / 2.0), net, item: DrcItem::Via(v.id), footprint: None, edge: false });
        }
        for p in nets.pads.iter().filter(|p| p.on(layer) && p.pad.kind != PadKind::Hole) {
            pieces.push(Piece { core: pad_core(p), net: p.net, item: pad_item(p), footprint: Some(p.footprint), edge: false });
        }
        let mut fills = Vec::new();
        for (i, f) in nets.fills.iter().enumerate().filter(|(_, f)| f.layer == layer) {
            fills.push(i);
            let shapes = f.shapes();
            let corners: usize = shapes.iter().map(|s| s.corners()).fold(0, usize::saturating_add);
            if !cx.spend(corners.saturating_add(1)) {
                return None;
            }
            for s in &shapes {
                for ring in std::iter::once(&s.outer).chain(&s.holes) {
                    let n = ring.len();
                    for k in 0..n {
                        let (Some(a), Some(b)) = (ring.get(k), ring.get((k + 1) % n)) else { continue };
                        pieces.push(Piece { core: stroke(*a, *b, 0.0), net: f.net, item: DrcItem::Pour(f.pour), footprint: None, edge: true });
                    }
                }
            }
        }
        let mut grid = Grid::new(CELL);
        for (i, p) in pieces.iter().enumerate() {
            grid.insert(i, core_bbox(&p.core));
        }
        Some(Layered { pieces, fills, grid })
    }

    /// The pieces whose copper may reach into `bbox`.
    pub(super) fn near(&self, bbox: [[f64; 2]; 2]) -> impl Iterator<Item = &Piece> {
        self.grid
            .near_box(bbox, self.pieces.len())
            .into_iter()
            .filter_map(|i| self.pieces.get(i))
            .filter(move |p| boxes_meet(core_bbox(&p.core), bbox))
    }
}

/// The copper on `layer` from `cache`, built the first time it is asked for; `None` once the
/// family's budget is spent.
pub(super) fn copper_on<'c>(cx: &mut Cx, cache: &'c mut BTreeMap<CopperLayer, Option<Layered>>, layer: CopperLayer) -> Option<&'c Layered> {
    cache.entry(layer).or_insert_with(|| Layered::new(cx, layer)).as_ref()
}

/// A pad as findings name it.
pub(super) fn pad_item(p: &BoardPad) -> DrcItem {
    DrcItem::Pad { footprint: p.footprint, number: p.pad.number.clone(), reference: p.reference() }
}

/// The board's footprints by their parts' designators, each with its courtyard on the board (its
/// box when the footprint has none).
pub(super) fn footprints<'a>(cx: &Cx<'a>) -> BTreeMap<&'a str, (&'a BoardFootprint, [[f64; 2]; 2])> {
    let ecad = cx.ecad;
    let mut out = BTreeMap::new();
    for fp in &cx.board.footprints {
        let Some(placed) = ecad.placed(fp) else { continue };
        let item = placed.resolved.footprint;
        let Some(local) = item.courtyard(Side::Top).or_else(|| footprint_box(item)) else { continue };
        out.insert(placed.part.designator.as_str(), (fp, fp.placement().bbox(local)));
    }
    out
}

/// A box as a core.
pub(super) fn box_core([[x0, y0], [x1, y1]]: [[f64; 2]; 2]) -> Core {
    Core { points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]], radius: 0.0 }
}

/// The pad a pin reference names (`U1.B24` by number, `U1.XC1` by the pin's name), by index into
/// the board's pads.
pub(super) fn find_pad(cx: &Cx, pin: &str) -> Option<usize> {
    let (part, name) = pin.split_once('.')?;
    let pads = &cx.nets.pads;
    if let Some(i) = pads.iter().position(|p| p.designator == part && p.pad.number == name) {
        return Some(i);
    }
    let number = cx.conn.nets.iter().flat_map(|n| n.pins.iter()).find(|q| q.designator == part && q.name == name).map(|q| q.pin.as_str())?;
    pads.iter().position(|p| p.designator == part && p.pad.number == number)
}

/// A net's copper as a graph: track corners, vias and pads as nodes, tracks' pieces as edges with
/// their lengths, and copper that touches joined at no length (a track's end on another track
/// joins it where it lands).
pub(super) struct NetGraph {
    edges: Vec<Vec<(usize, f64)>>,
    /// The node of each of the net's vias (by index into the board's vias) and pads (by index into
    /// the pads).
    pub vias: BTreeMap<usize, usize>,
    pub pads: BTreeMap<usize, usize>,
}

impl NetGraph {
    /// The graph of net `net`'s tracks, vias and pads; `None` past [`MAX_NODES`] or the budget.
    pub(super) fn new(cx: &mut Cx, net: usize) -> Option<NetGraph> {
        let (board, nets) = (cx.board, cx.nets);
        let layers = board.copper_layers;
        let lines: Vec<(CopperLayer, f64, Vec<[f64; 2]>)> = board
            .tracks
            .iter()
            .enumerate()
            .filter(|(ti, _)| nets.track_nets.get(*ti).copied().flatten() == Some(net))
            .map(|(_, t)| (t.layer, t.width, path::tessellate(&t.points, &t.curves, false, 0.01)))
            .collect();
        let via_list: Vec<usize> = (0..board.vias.len()).filter(|i| nets.via_nets.get(*i).copied().flatten() == Some(net)).collect();
        let pad_list: Vec<usize> = (0..nets.pads.len()).filter(|i| nets.pads.get(*i).is_some_and(|p| p.net == Some(net))).collect();
        let n = lines.iter().map(|l| l.2.len()).fold(0, usize::saturating_add).saturating_add(via_list.len()).saturating_add(pad_list.len());
        if n > MAX_NODES || !cx.spend(n.saturating_mul(n).saturating_add(1)) {
            return None;
        }
        let mut nodes: Vec<[f64; 2]> = Vec::with_capacity(n);
        let mut edges: Vec<Vec<(usize, f64)>> = Vec::with_capacity(n);
        let join = |edges: &mut Vec<Vec<(usize, f64)>>, a: usize, b: usize, len: f64| {
            if let Some(e) = edges.get_mut(a) {
                e.push((b, len));
            }
            if let Some(e) = edges.get_mut(b) {
                e.push((a, len));
            }
        };
        // Each line's nodes, by its first node.
        let mut starts = Vec::with_capacity(lines.len());
        for (_, _, line) in &lines {
            starts.push(nodes.len());
            for (k, p) in line.iter().enumerate() {
                let i = nodes.len();
                nodes.push(*p);
                edges.push(Vec::new());
                if let Some(prev) = k.checked_sub(1).and_then(|j| line.get(j)) {
                    join(&mut edges, i.saturating_sub(1), i, distance(*prev, *p));
                }
            }
        }
        let mut vias = BTreeMap::new();
        for vi in &via_list {
            let Some(v) = board.vias.get(*vi) else { continue };
            vias.insert(*vi, nodes.len());
            nodes.push(v.at);
            edges.push(Vec::new());
        }
        let mut pads = BTreeMap::new();
        for pi in &pad_list {
            let Some(p) = nets.pads.get(*pi) else { continue };
            pads.insert(*pi, nodes.len());
            nodes.push(p.at());
            edges.push(Vec::new());
        }
        for (li, (layer, width, line)) in lines.iter().enumerate() {
            let start = starts.get(li).copied().unwrap_or(0);
            for (k, p) in line.iter().enumerate() {
                let node = start + k;
                for (vi, vn) in &vias {
                    if let Some(v) = board.vias.get(*vi)
                        && v.on(*layer, layers)
                        && distance(*p, v.at) <= v.diameter / 2.0 + width / 2.0
                    {
                        join(&mut edges, node, *vn, 0.0);
                    }
                }
                for (pi, pn) in &pads {
                    if let Some(pad) = nets.pads.get(*pi)
                        && pad.on(*layer)
                        && gap(&pad_core(pad), &disc(*p, 0.0)) <= TOLERANCE
                    {
                        join(&mut edges, node, *pn, 0.0);
                    }
                }
            }
            // The line's ends on another line of the layer: joined where they land.
            for (k, end) in [(0, line.first()), (line.len().saturating_sub(1), line.last())] {
                let Some(end) = end.copied() else { continue };
                let end_node = start + k;
                for (lj, (other_layer, other_width, other)) in lines.iter().enumerate() {
                    if lj == li || other_layer != layer {
                        continue;
                    }
                    let other_start = starts.get(lj).copied().unwrap_or(0);
                    for (k, w) in other.windows(2).enumerate() {
                        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
                        let (q, _) = path::project(end, *a, *b, 0.0);
                        if distance(end, q) <= (other_width / 2.0).max(width / 2.0) {
                            join(&mut edges, end_node, other_start + k, distance(q, *a));
                            join(&mut edges, end_node, other_start + k + 1, distance(q, *b));
                        }
                    }
                }
            }
        }
        // A via in a pad.
        for (vi, vn) in &vias {
            let Some(v) = board.vias.get(*vi) else { continue };
            for (pi, pn) in &pads {
                if let Some(pad) = nets.pads.get(*pi)
                    && [CopperLayer::Top, CopperLayer::Bottom].into_iter().any(|l| pad.on(l) && v.on(l, layers))
                    && pad.contains(v.at)
                {
                    join(&mut edges, *vn, *pn, 0.0);
                }
            }
        }
        Some(NetGraph { edges, vias, pads })
    }

    /// The shortest length along the copper from node `from` to node `to`, and the nodes on the
    /// way; `None` when they aren't joined.
    pub(super) fn shortest(&self, from: usize, to: usize) -> Option<(f64, Vec<usize>)> {
        let n = self.edges.len();
        let mut dist = vec![f64::INFINITY; n];
        let mut prev: Vec<Option<usize>> = vec![None; n];
        let mut done = vec![false; n];
        *dist.get_mut(from)? = 0.0;
        loop {
            let next = (0..n)
                .filter(|i| !done.get(*i).copied().unwrap_or(true))
                .filter_map(|i| dist.get(i).map(|d| (i, *d)))
                .filter(|(_, d)| d.is_finite())
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let Some((u, du)) = next else { break };
            if let Some(d) = done.get_mut(u) {
                *d = true;
            }
            if u == to {
                break;
            }
            for (v, len) in self.edges.get(u).into_iter().flatten() {
                let alt = du + len;
                if let Some(dv) = dist.get_mut(*v)
                    && alt < *dv
                {
                    *dv = alt;
                    if let Some(p) = prev.get_mut(*v) {
                        *p = Some(u);
                    }
                }
            }
        }
        let total = dist.get(to).copied().filter(|d| d.is_finite())?;
        let mut way = vec![to];
        let mut at = to;
        while let Some(p) = prev.get(at).copied().flatten() {
            way.push(p);
            at = p;
            if way.len() > n {
                break;
            }
        }
        way.reverse();
        Some((total, way))
    }
}

/// The closest copper of each net on a layer to the cores `zone`, within `reach` of them: the gap
/// (below 0 where they overlap) and the piece; a fill counts from inside too. Pieces `skip` says
/// yes to are left out.
pub(super) fn closest(cx: &mut Cx, copper: &Layered, zone: &[Core], reach: f64, skip: impl Fn(&Piece) -> bool) -> BTreeMap<usize, (f64, DrcItem)> {
    let nets = cx.nets;
    let mut out: BTreeMap<usize, (f64, DrcItem)> = BTreeMap::new();
    let Some(bbox) = super::super::bbox(zone.iter().flat_map(|z| core_bbox(z).into_iter())) else { return out };
    let near: Vec<&Piece> = copper.near(grow(bbox, reach.max(0.0))).filter(|p| !skip(p)).collect();
    if !cx.spend(near.len().saturating_mul(zone.len()).saturating_add(1)) {
        return out;
    }
    let mut keep = |net: usize, g: f64, item: &DrcItem| {
        if out.get(&net).is_none_or(|(old, _)| g < *old) {
            out.insert(net, (g, item.clone()));
        }
    };
    for p in near {
        let Some(net) = p.net else { continue };
        let g = zone.iter().map(|z| gap(z, &p.core)).fold(f64::INFINITY, f64::min);
        if g < reach {
            keep(net, g, &p.item);
        }
    }
    // A zone inside a fill touches none of its edges.
    let probe: Vec<[f64; 2]> = zone.iter().flat_map(|z| z.points.iter().copied()).collect();
    for f in &copper.fills {
        let Some(fill) = nets.fills.get(*f) else { continue };
        let Some(net) = fill.net else { continue };
        if fill.region.locate(&probe).iter().any(Option::is_some) {
            keep(net, -TOLERANCE, &DrcItem::Pour(fill.pour));
        }
    }
    out
}

/// One of a crystal's two nets: the oscillator's pin and the crystal's pad on it, by index into
/// the board's pads.
struct Leg {
    net: usize,
    pin: usize,
    pad: usize,
}

/// A crystal on the board (see the module notes).
struct Crystal<'a> {
    designator: &'a str,
    footprint: &'a BoardFootprint,
    yard: [[f64; 2]; 2],
    legs: Vec<Leg>,
    /// The oscillator's footprint and courtyard.
    ic: Option<(&'a BoardFootprint, [[f64; 2]; 2])>,
    /// Its load capacitors: designator, footprint and courtyard.
    caps: Vec<(&'a str, &'a BoardFootprint, [[f64; 2]; 2])>,
    f_mhz: Option<f64>,
}

impl Crystal<'_> {
    fn watch(&self) -> bool {
        self.f_mhz.is_some_and(|f| f < WATCH_MHZ)
    }

    fn item(&self) -> DrcItem {
        DrcItem::Footprint { id: self.footprint.id, designator: self.designator.to_string() }
    }

    fn nets(&self) -> BTreeSet<usize> {
        self.legs.iter().map(|l| l.net).collect()
    }
}

/// The crystals of the board (see the module notes); what keeps a crystal part from being
/// checked is listed as not applicable.
fn crystals<'a>(cx: &mut Cx<'a>) -> Vec<Crystal<'a>> {
    let (roles, nets) = (cx.roles, cx.nets);
    let fps = footprints(cx);
    let mut out = Vec::new();
    let records: Vec<_> = roles.parts(PartRole::Crystal).collect();
    if records.is_empty() {
        // Parts that look like crystals with no record: the checks need one.
        let looks = cx.ecad.schematic.parts.iter().find(|p| {
            let prefix: String = p.designator.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_uppercase();
            prefix == "Y" || prefix == "XTAL"
        });
        if let Some(p) = looks {
            for code in [LOOP, KEEPOUT] {
                if cx.wants(code) {
                    cx.not_applicable(code, format!("intent: the crystal role of {} (crystal of its oscillator's pins)", p.designator));
                }
            }
        }
        return out;
    }
    for r in records {
        let Some((fp, yard)) = fps.get(r.part.as_str()).copied() else { continue };
        if r.of.is_empty() {
            for code in [LOOP, KEEPOUT] {
                if cx.wants(code) {
                    cx.not_applicable(code, format!("intent: the oscillator pins {} serves (of)", r.part));
                }
            }
            continue;
        }
        let mut legs = Vec::new();
        for pin in &r.of {
            let Some(pi) = find_pad(cx, pin) else { continue };
            let Some(net) = nets.pads.get(pi).and_then(|p| p.net) else { continue };
            let pad = (0..nets.pads.len()).find(|i| nets.pads.get(*i).is_some_and(|p| p.footprint == fp.id && p.net == Some(net)));
            if let Some(pad) = pad
                && !legs.iter().any(|l: &Leg| l.net == net)
            {
                legs.push(Leg { net, pin: pi, pad });
            }
        }
        // An oscillator module, or a crystal not wired to its oscillator yet.
        if legs.len() != 2 {
            continue;
        }
        let ic = legs.first().and_then(|l| nets.pads.get(l.pin)).and_then(|p| fps.values().find(|(f, _)| f.id == p.footprint).copied());
        let caps = roles
            .parts(PartRole::LoadCap)
            .filter(|c| c.of.contains(&r.part))
            .filter_map(|c| fps.get_key_value(c.part.as_str()).map(|(d, (f, y))| (*d, *f, *y)))
            .collect();
        let f_mhz = legs.iter().find_map(|l| roles.net(l.net).and_then(|n| n.f_mhz())).filter(|f| f.is_finite() && *f > 0.0);
        let designator = fps.get_key_value(r.part.as_str()).map_or("", |(d, _)| *d);
        out.push(Crystal { designator, footprint: fp, yard, legs, ic, caps, f_mhz });
    }
    out
}

/// Whether the pad `pin` is an inner ball of its footprint's area array: farther in from the box
/// round the footprint's pads than half their pitch.
fn inner_ball(cx: &Cx, pin: &BoardPad) -> bool {
    let mine: Vec<[f64; 2]> = cx.nets.pads.iter().filter(|p| p.footprint == pin.footprint).map(BoardPad::at).collect();
    if mine.len() < 9 {
        return false;
    }
    let Some([[x0, y0], [x1, y1]]) = super::super::bbox(mine.iter().copied()) else { return false };
    let at = pin.at();
    let pitch = mine.iter().filter(|q| distance(**q, at) > 1e-6).map(|q| distance(*q, at)).fold(f64::INFINITY, f64::min);
    if !pitch.is_finite() {
        return false;
    }
    let inward = (at[0] - x0).min(x1 - at[0]).min(at[1] - y0).min(y1 - at[1]);
    inward > pitch / 2.0
}

/// The routed length of a leg from its oscillator pin (or that pin's breakout via) to the crystal
/// pad, whether it was routed (else the straight line), and the breakout via (by index into the
/// board's vias).
fn leg_length(cx: &Cx, leg: &Leg, graph: Option<&NetGraph>, breakout: f64) -> (f64, bool, Option<usize>) {
    let (board, nets) = (cx.board, cx.nets);
    let (Some(pin), Some(pad)) = (nets.pads.get(leg.pin), nets.pads.get(leg.pad)) else { return (0.0, false, None) };
    let straight = distance(pin.at(), pad.at());
    let Some(g) = graph else { return (straight, false, None) };
    let mut via = None;
    if inner_ball(cx, pin) {
        let core = pad_core(pin);
        via = g
            .vias
            .keys()
            .filter_map(|vi| board.vias.get(*vi).map(|v| (*vi, gap(&core, &disc(v.at, v.diameter / 2.0)))))
            .filter(|(_, d)| *d <= breakout)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(vi, _)| vi);
    }
    let from = via.and_then(|v| g.vias.get(&v)).or_else(|| g.pads.get(&leg.pin)).copied();
    let to = g.pads.get(&leg.pad).copied();
    match from.zip(to).and_then(|(a, b)| g.shortest(a, b)) {
        Some((len, _)) => (len, true, via),
        None => (straight, false, via),
    }
}

fn loops(cx: &mut Cx, c: &Crystal) {
    let (board, nets) = (cx.board, cx.nets);
    let p = |name: &str| super::param_value(board, LOOP, name);
    let (mhz_max, khz_max, error_max, cap_max, mismatch, breakout) =
        (p("mhz_max"), p("khz_max"), p("error_max"), p("cap_max"), p("mismatch"), p("breakout"));
    let xtal = c.item();
    let fkey = xtal.key();
    let regime = match c.f_mhz {
        Some(f) if c.watch() => format!("watch crystal, {} kHz", num(f * 1000.0)),
        Some(f) => format!("{} MHz crystal", num(f)),
        None => "frequency not known".to_string(),
    };
    let mut routed: Vec<(usize, f64)> = Vec::new();
    for leg in &c.legs {
        let graph = NetGraph::new(cx, leg.net);
        if graph.is_none() && cx.spent() {
            return;
        }
        let (len, was_routed, breakout_via) = leg_length(cx, leg, graph.as_ref(), breakout);
        let name = net_name(cx, leg.net);
        let (Some(pin), Some(pad)) = (nets.pads.get(leg.pin), nets.pads.get(leg.pad)) else { continue };
        let (pin_item, pad_item_) = (pad_item(pin), pad_item(pad));
        if was_routed {
            routed.push((leg.net, len));
        }
        // The length, against the crystal's limit: the warning's needs the frequency (MHz or
        // watch crystal), the error's doesn't.
        let warn_at = match c.f_mhz {
            None => {
                cx.not_applicable(LOOP, format!("intent: f_mhz of {name} (the crystal's frequency sets its length limit)"));
                None
            }
            Some(_) if c.watch() => Some((khz_max, "khz_max")),
            Some(_) => Some((mhz_max, "mhz_max")),
        };
        let error = len > error_max;
        let over = if error { Some((error_max, "error_max")) } else { warn_at.filter(|(l, _)| len > *l) };
        if let Some((limit, which)) = over {
            let how = if was_routed { "of copper" } else { "in a straight line (not routed yet)" };
            let from = match breakout_via.and_then(|v| board.vias.get(v)) {
                Some(v) => format!("{}'s breakout via {}", pin.reference(), v.id),
                None => pin.reference(),
            };
            let regime = regime.clone();
            cx.add(LOOP, &[fkey.clone(), name.clone(), "length".into()], || {
                Draft::new(
                    pad.at(),
                    vec![pin_item.clone(), pad_item_.clone()],
                    format!("crystal net {name} runs {} mm {how} from {from} to {}; keep it within {} mm", num(len), pad.reference(), num(limit)),
                )
                .nets(vec![name.clone()])
                .measure(len, limit, "mm", Bound::Max)
                .param(which)
                .severity(if error { Severity::Error } else { Severity::Warning })
                .regime(regime)
                .arg("net", name.clone())
                .fix(
                    "ecad.board.move",
                    Value::Null,
                    format!("move {} next to {} and route {name} straight on one layer", c.designator, pin.designator),
                )
            });
        }
        // Vias on the crystal net, the breakout of an inner ball aside.
        let vias: Vec<usize> =
            (0..board.vias.len()).filter(|vi| nets.via_nets.get(*vi).copied().flatten() == Some(leg.net) && Some(*vi) != breakout_via).collect();
        if !vias.is_empty() {
            let items: Vec<DrcItem> = vias.iter().filter_map(|vi| board.vias.get(*vi)).map(|v| DrcItem::Via(v.id)).collect();
            let ids: Vec<String> = vias.iter().filter_map(|vi| board.vias.get(*vi)).map(|v| v.id.to_string()).collect();
            let at = vias.first().and_then(|vi| board.vias.get(*vi)).map_or(pad.at(), |v| v.at);
            let count = vias.len();
            let regime = regime.clone();
            cx.add(LOOP, &[fkey.clone(), name.clone(), "vias".into()], || {
                Draft::new(
                    at,
                    items,
                    format!(
                        "crystal net {name} goes through {count} via{} ({}): route it on the crystal's layer with none",
                        if count == 1 { "" } else { "s" },
                        ids.join(", ")
                    ),
                )
                .nets(vec![name.clone()])
                .variant("via")
                .measure(count as f64, 0.0, "vias", Bound::Max)
                .param("")
                .regime(regime)
                .arg("net", name.clone())
                .arg("count", count.to_string())
                .fix(
                    "ecad.board.net.inspect",
                    json!({"nets": [name], "metrics": ["layers", "vias"]}),
                    "see where it changes layer; reroute it on the crystal's layer",
                )
            });
        }
        if cx.spent() {
            return;
        }
    }
    // The load capacitors, each by its pad on a crystal net.
    for (designator, fp, _) in &c.caps {
        let Some(cap_pad) = nets.pads.iter().find(|q| q.footprint == fp.id && c.legs.iter().any(|l| q.net == Some(l.net))) else { continue };
        let Some(xpad) = c.legs.iter().find(|l| Some(l.net) == cap_pad.net).and_then(|l| nets.pads.get(l.pad)) else { continue };
        let d = gap(&pad_core(cap_pad), &pad_core(xpad)).max(0.0);
        if d <= cap_max {
            continue;
        }
        let cap_item = DrcItem::Footprint { id: fp.id, designator: designator.to_string() };
        let keys = [fkey.clone(), cap_item.key(), "cap".into()];
        let (a, b) = (pad_item(cap_pad), pad_item(xpad));
        let at = cap_pad.at();
        let regime = regime.clone();
        cx.add(LOOP, &keys, || {
            Draft::new(
                at,
                vec![a, b],
                format!("load capacitor {designator} is {} mm from crystal pad {}; keep it within {} mm", num(d), xpad.reference(), num(cap_max)),
            )
            .variant("cap")
            .measure(d, cap_max, "mm", Bound::Max)
            .param("cap_max")
            .severity(Severity::Warning)
            .regime(regime)
            .fix("ecad.board.move", Value::Null, format!("move {designator} next to {}", xpad.reference()))
        });
    }
    // The crystal, or a load capacitor, on the other side from the oscillator.
    if let Some((ic, _)) = c.ic {
        let ic_name = nets.pads.iter().find(|q| q.footprint == ic.id).map_or(String::new(), |q| q.designator.clone());
        let ic_item = DrcItem::Footprint { id: ic.id, designator: ic_name.clone() };
        let mut far: Vec<(&str, &BoardFootprint, &'static str)> = Vec::new();
        if ic.side != c.footprint.side {
            far.push((c.designator, c.footprint, "side"));
        }
        far.extend(c.caps.iter().filter(|(_, f, _)| f.side != ic.side).map(|(d, f, _)| (*d, *f, "cap_side")));
        for (designator, fp, variant) in far {
            let item = DrcItem::Footprint { id: fp.id, designator: designator.to_string() };
            let what = if variant == "side" { "crystal" } else { "load capacitor" };
            let regime = regime.clone();
            cx.add(LOOP, &[fkey.clone(), item.key(), "side".into()], || {
                Draft::new(
                    fp.at,
                    vec![item.clone(), ic_item.clone()],
                    format!("{what} {designator} is on the other side of the board from oscillator {ic_name}: put it on {ic_name}'s side"),
                )
                .variant(variant)
                .severity(Severity::Warning)
                .regime(regime)
                .fix("ecad.board.flip", json!({"items": [designator]}), format!("flip {designator} to {ic_name}'s side"))
            });
        }
    }
    // The two nets' lengths.
    if let (Some((na, la)), Some((nb, lb))) = (routed.first().copied(), routed.get(1).copied()) {
        let longer = la.max(lb);
        let off = if longer > 0.0 { (la - lb).abs() / longer } else { 0.0 };
        if off > mismatch {
            let (an, bn) = (net_name(cx, na), net_name(cx, nb));
            cx.add(LOOP, &[fkey.clone(), "mismatch".into()], || {
                Draft::new(
                    c.footprint.at,
                    vec![xtal.clone()],
                    format!(
                        "crystal {}'s nets differ in length: {an} {} mm against {bn} {} mm ({} %)",
                        c.designator,
                        num(la),
                        num(lb),
                        num((off * 100.0).round())
                    ),
                )
                .nets(vec![an.clone(), bn.clone()])
                .variant("mismatch")
                .measure(off, mismatch, "ratio", Bound::Max)
                .param("mismatch")
                .severity(Severity::Info)
                .regime(regime)
                .arg("net", an.clone())
                .arg("other", bn.clone())
                .arg("length", num(la))
                .arg("other_length", num(lb))
                .arg("percent", num((off * 100.0).round()))
                .fix(
                    "ecad.board.net.inspect",
                    json!({"nets": [an, bn], "metrics": ["length"]}),
                    "route both crystal nets alike, as short as the shorter",
                )
            });
        }
    }
}

/// The name of net `n`, as findings give it.
pub(super) fn net_name(cx: &Cx, n: usize) -> String {
    cx.nets.name(n).unwrap_or("?").to_string()
}

/// The parts of the segment `a`–`b` outside the box (none, one or two).
fn outside_box(a: [f64; 2], b: [f64; 2], [[x0, y0], [x1, y1]]: [[f64; 2]; 2]) -> Vec<([f64; 2], [f64; 2])> {
    // The part inside, as a share of the way from a to b (Liang–Barsky).
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [(-d[0], a[0] - x0), (d[0], x1 - a[0]), (-d[1], a[1] - y0), (d[1], y1 - a[1])] {
        if p.abs() <= 1e-12 {
            if q < 0.0 {
                return vec![(a, b)];
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            t0 = t0.max(r);
        } else {
            t1 = t1.min(r);
        }
    }
    if t0 >= t1 {
        return vec![(a, b)];
    }
    let at = |t: f64| [a[0] + d[0] * t, a[1] + d[1] * t];
    let mut out = Vec::new();
    if t0 > 1e-9 {
        out.push((a, at(t0)));
    }
    if t1 < 1.0 - 1e-9 {
        out.push((at(t1), b));
    }
    out
}

/// The layer a crystal sits on and the next one in.
fn crystal_layers(cx: &Cx, c: &Crystal) -> Vec<CopperLayer> {
    let layers = &cx.geo.layers;
    let mut out = Vec::new();
    if c.footprint.side == Side::Top {
        out.extend(layers.first().copied());
        out.extend(layers.get(1).copied());
    } else {
        out.extend(layers.last().copied());
        out.extend(layers.len().checked_sub(2).and_then(|i| layers.get(i)).copied());
    }
    out.dedup();
    out
}

fn keepout(cx: &mut Cx, c: &Crystal, cache: &mut BTreeMap<CopperLayer, Option<Layered>>) {
    let (board, nets, roles) = (cx.board, cx.nets, cx.roles);
    let p = |name: &str| super::param_value(board, KEEPOUT, name);
    let (margin, fast_warn, fast_error, current_min) = (p("margin"), p("fast_warn"), p("fast_error"), p("current_min"));
    let (edge_min, connector_min) = (p("edge"), p("connector"));
    let (guard_width, guard_length, guard_pitch) = (p("guard_width"), p("guard_length"), p("guard_pitch").max(0.1));
    let xtal = c.item();
    let fkey = xtal.key();
    let own = c.nets();
    let ic_id = c.ic.map(|(f, _)| f.id);
    let ic_yard = c.ic.map(|(_, y)| y);
    let layers = crystal_layers(cx, c);
    // The zone: the crystal's and its load capacitors' courtyards, and its nets' tracks outside
    // the oscillator's courtyard on the layers looked at.
    let mut zone: Vec<Core> = vec![box_core(c.yard)];
    zone.extend(c.caps.iter().map(|(_, _, y)| box_core(*y)));
    for (ti, t) in board.tracks.iter().enumerate() {
        if !nets.track_nets.get(ti).copied().flatten().is_some_and(|n| own.contains(&n)) || !layers.contains(&t.layer) {
            continue;
        }
        for (a, b) in pieces(&t.points, &t.curves, false) {
            let outside = match ic_yard {
                Some(yard) => outside_box(a, b, yard),
                None => vec![(a, b)],
            };
            zone.extend(outside.into_iter().map(|(p, q)| stroke(p, q, t.width / 2.0)));
        }
    }
    let reach = fast_warn.max(margin);
    let class_of = |hot: bool| if c.watch() && hot { Class::Critical } else { Class::Practical };
    let regime = match c.f_mhz {
        Some(f) if c.watch() => format!("watch crystal, {} kHz", num(f * 1000.0)),
        Some(f) => format!("{} MHz crystal", num(f)),
        None => "frequency not known".to_string(),
    };
    // Each other net's closest copper to the zone over the layers looked at: how near, what,
    // where, and every layer it is near on.
    let mut by_net: BTreeMap<usize, (f64, DrcItem, CopperLayer, Vec<CopperLayer>)> = BTreeMap::new();
    for layer in &layers {
        let Some(copper) = copper_on(cx, cache, *layer) else { return };
        let near = closest(cx, copper, &zone, reach, |q| q.footprint.is_some() && q.footprint == ic_id);
        for (net, (g, item)) in near {
            if own.contains(&net) || roles.grounds.contains(&net) {
                continue;
            }
            let e = by_net.entry(net).or_insert_with(|| (g, item.clone(), *layer, Vec::new()));
            if g < e.0 {
                (e.0, e.1, e.2) = (g, item, *layer);
            }
            e.3.push(*layer);
        }
        if cx.spent() {
            return;
        }
    }
    let mut feeds = Feeds::default();
    for (net, (g, item, layer, on)) in by_net {
        let Some(info) = roles.net(net) else { continue };
        let fast = info.fast() && !info.is_supply();
        let mut current = info.record.and_then(|r| r.current_a).filter(|a| *a >= current_min);
        // A rail's branch that feeds only the oscillator's own pins (and capacitors) carries its
        // few milliamps, not the rail's current.
        if current.is_some() {
            let piece = match &item {
                DrcItem::Track(id) => board.tracks.iter().position(|t| t.id == *id).map(Item::Track),
                DrcItem::Via(id) => board.vias.iter().position(|v| v.id == *id).map(Item::Via),
                DrcItem::Pour(_) => Some(Item::Fill(0, 0)),
                _ => None,
            };
            let feeds_load = match piece {
                Some(piece) => match feeds.reach_a_load(cx, net, piece, ic_id) {
                    Some(f) => f,
                    None => return,
                },
                None => false,
            };
            if !feeds_load {
                current = None;
            }
        }
        let hot = fast || current.is_some();
        // Fast only by the assumed standard drive: a warning until the net's driver says so.
        let assumed = fast && info.assumed() && current.is_none();
        let (severity, limit, which) = if hot && g < fast_error {
            (if assumed { Severity::Warning } else { Severity::Error }, fast_error, "fast_error")
        } else if hot && g < fast_warn {
            (Severity::Warning, fast_warn, "fast_warn")
        } else if g < margin {
            (Severity::Warning, margin, "margin")
        } else {
            // Unverified copper this near might be fast.
            if info.tier.is_none() && !info.is_supply() && g < fast_warn {
                cx.not_applicable(KEEPOUT, info.needs_edge());
            }
            continue;
        };
        let other = info.name.to_string();
        let why = match (fast, current) {
            (true, _) => format!("fast: {}", info.regime()),
            (false, Some(a)) => format!("{} A", num(a)),
            _ => "another net".to_string(),
        };
        let layers_on = on.iter().map(|l| l.name()).collect::<Vec<_>>().join(", ");
        let at = c.footprint.at;
        let keys = [fkey.clone(), other.clone()];
        let regime = format!("{regime}; {why}");
        cx.add(KEEPOUT, &keys, || {
            let mut d = Draft::new(
                at,
                vec![xtal.clone(), item.clone()],
                format!(
                    "{} of {other} ({why}) on {} is {} mm from crystal {}'s zone (its courtyard, load capacitors and tracks; {other} is near it on {layers_on}); keep {} mm clear",
                    item.reference(),
                    layer.name(),
                    num(g.max(0.0)),
                    c.designator,
                    num(limit)
                ),
            )
            .layer(layer)
            .nets(vec![other.clone()])
            .measure(g.max(0.0), limit, "mm", Bound::Min)
            .param(which)
            .severity(severity)
            .class(class_of(hot))
            .regime(regime)
            .arg("other", other.clone())
            .arg("on", layers_on.clone())
            .fix("ecad.board.net.inspect", json!({"nets": [other], "metrics": ["layers"]}), format!("move {other}'s copper away from {} on {layers_on}, or put ground between", c.designator));
            if hot {
                d = d.variant("fast");
            }
            d
        });
    }
    // Test points on the crystal nets.
    for tp in roles.parts(PartRole::TestPoint) {
        let Some(q) = nets.pads.iter().find(|q| q.designator == tp.part && q.net.is_some_and(|n| own.contains(&n))) else { continue };
        let name = q.net.map(|n| net_name(cx, n)).unwrap_or_default();
        let item = DrcItem::Footprint { id: q.footprint, designator: q.designator.clone() };
        let at = q.at();
        cx.add(KEEPOUT, &[fkey.clone(), item.key()], || {
            Draft::new(
                at,
                vec![xtal.clone(), item.clone()],
                format!("test point {} is on crystal net {name}: its pad loads the oscillator; leave crystal nets without test points", q.designator),
            )
            .nets(vec![name.clone()])
            .variant("test_point")
            .severity(Severity::Warning)
            .arg("net", name.clone())
            .fix("ecad.board.delete", Value::Null, format!("remove test point {} from {name}", q.designator))
        });
    }
    // A thin ground track round the crystal with few vias.
    for (ti, t) in board.tracks.iter().enumerate() {
        let Some(g_net) = nets.track_nets.get(ti).copied().flatten().filter(|n| roles.grounds.contains(n)) else { continue };
        if t.width >= guard_width || !layers.contains(&t.layer) {
            continue;
        }
        let length = path::length(&t.points, &t.curves, false);
        if length <= guard_length {
            continue;
        }
        let cores: Vec<Core> = track_cores(t).collect();
        if !cx.spend(cores.len().saturating_mul(zone.len()).saturating_add(board.vias.len())) {
            return;
        }
        let close = cores.iter().any(|tc| zone.iter().any(|z| gap(z, tc) < fast_warn));
        if !close {
            continue;
        }
        let vias = board
            .vias
            .iter()
            .enumerate()
            .filter(|(k, v)| {
                nets.via_nets.get(*k).copied().flatten() == Some(g_net) && cores.iter().any(|tc| gap(tc, &disc(v.at, v.diameter / 2.0)) <= 0.0)
            })
            .count();
        let want = (length / guard_pitch).floor();
        if vias as f64 >= want {
            continue;
        }
        let at = t.points.first().copied().unwrap_or(c.footprint.at);
        let name = net_name(cx, g_net);
        cx.add(KEEPOUT, &[fkey.clone(), DrcItem::Track(t.id).key()], || {
            Draft::new(
                at,
                vec![xtal.clone(), DrcItem::Track(t.id)],
                format!(
                    "track {} of {name} round crystal {} is {} mm wide, {} mm long with {vias} vias: a thin guard is a pickup loop; use a ground pour stitched every {} mm",
                    t.id,
                    c.designator,
                    num(t.width),
                    num(length.round()),
                    num(guard_pitch)
                ),
            )
            .layer(t.layer)
            .nets(vec![name.clone()])
            .variant("guard")
            .severity(Severity::Info)
            .review()
            .arg("length", num(length.round()))
            .arg("count", vias.to_string())
            .arg("width", num(t.width))
            .arg("pitch", num(guard_pitch))
            .fix("ecad.board.pour", Value::Null, format!("replace track {} with a {name} pour under and round the crystal, stitched to the plane", t.id))
        });
    }
    // Near the board's edge.
    if !board.outline.is_empty() {
        let ring = path::tessellate(&board.outline, &board.outline_curves, true, 0.05);
        let yard = box_core(c.yard);
        let n = ring.len();
        if cx.spend(n.saturating_add(1)) {
            let d = (0..n)
                .filter_map(|i| Some((*ring.get(i)?, *ring.get((i + 1) % n)?)))
                .map(|(a, b)| gap(&yard, &stroke(a, b, 0.0)))
                .fold(f64::INFINITY, f64::min)
                .max(0.0);
            if d < edge_min {
                cx.add(KEEPOUT, &[fkey.clone(), "edge".into()], || {
                    Draft::new(
                        c.footprint.at,
                        vec![xtal.clone()],
                        format!(
                            "crystal {} is {} mm from the board's edge: keep it {} mm in, away from what the edge picks up",
                            c.designator,
                            num(d),
                            num(edge_min)
                        ),
                    )
                    .variant("edge")
                    .measure(d, edge_min, "mm", Bound::Min)
                    .param("edge")
                    .severity(Severity::Info)
                    .fix("ecad.board.move", Value::Null, format!("move {} further in", c.designator))
                });
            }
        }
    }
    // Near an I/O connector: one a cable may plug into.
    let fps = footprints(cx);
    for conn in roles.parts(PartRole::Connector).filter(|p| p.cable != Some(false)) {
        let Some((fp, yard)) = fps.get(conn.part.as_str()).copied() else { continue };
        let d = gap(&box_core(c.yard), &box_core(yard)).max(0.0);
        if d >= connector_min {
            continue;
        }
        let item = DrcItem::Footprint { id: fp.id, designator: conn.part.clone() };
        cx.add(KEEPOUT, &[fkey.clone(), item.key()], || {
            Draft::new(
                c.footprint.at,
                vec![xtal.clone(), item.clone()],
                format!(
                    "crystal {} is {} mm from connector {}: keep it {} mm away from what comes in on a cable",
                    c.designator,
                    num(d),
                    conn.part,
                    num(connector_min)
                ),
            )
            .variant("connector")
            .measure(d, connector_min, "mm", Bound::Min)
            .param("connector")
            .severity(Severity::Info)
            .fix("ecad.board.move", Value::Null, format!("move {} away from {}", c.designator, conn.part))
        });
    }
}
