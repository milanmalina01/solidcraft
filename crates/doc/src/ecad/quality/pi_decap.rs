//! Decoupling: whether each IC power pin has its high-frequency capacitor, and whether each
//! capacitor sits, mounts and joins its pin as the planes under it ask.
//! - `pi.decap_assigned`: an IC supply pin (as the inference finds them: power inputs, an IC's
//!   own supply nodes, a charger's battery and system pins) that no kept decoupling, bulk or
//!   regulator-capacitor record names: by its number (`U1.B1`), by its name (`U1.VDD`: every pin
//!   of that name) or by its part (`U1`: one capacitor for all). A record naming a rail serves the
//!   IC supply pin on it nearest the capacitor, and a capacitor serves the pins of its pin's part
//!   beside it on the same net (within `group_gap`: a charger's two BAT pins, VDD beside VDDH),
//!   as one pin group. A pin a proposal names is not applicable until the proposal is settled;
//!   a pin whose only capacitor is placed but not mounted (`dnp`, as a datasheet's reference
//!   leaves sites empty) is an info for a look; only a pin nothing serves is an error. A module's
//!   pins are left out: it carries its own decoupling (a part proposed as one waits for that).
//! - `pi.decap_distance`: a decoupling capacitor's pad to the pin it serves, edge to edge, against
//!   the limit of its regime: 2 mm for an RF IC, `close` on close planes, else 3 mm for a fast IC
//!   (one driving a T1-or-faster net) and 5 mm for others; an error past twice the limit. An RF
//!   IC's capacitor joined to its pin only through vias (routed, but not on the surface), and a
//!   capacitor on the other side with no via of the supply within 1 mm of its pad or of the pin,
//!   are errors; on close planes, a split in either plane between the two is a warning. Bulk
//!   capacitors (4.7 µF and up), pins fed through a ferrite or filter (on its far side from the
//!   supply), and a capacitor under its IC from the other side (the pad field of a BGA) are left
//!   out.
//! - `pi.decap_mounting`: each capacitor pad's track to its first via (longer than `trace`), a via
//!   farther than `via_gap` from the pad, both vias beyond the far ends of the part, and, on
//!   close planes, one via shared by two capacitors; each finding gives the mounting's estimated
//!   inductance. A pad the net's pour joins, and a pad of a supply with no plane (it is routed
//!   to its pin), have no via to look for; a pad nothing touches is not routed yet (not
//!   applicable). Capacitors of `bulk_uf` and up are left out. No fab profile says whether it
//!   takes vias in pads, so the fix never asks for one: a short track is fine.
//! - `pi.decap_topology`: how the capacitor joins its pin, by regime. An RF IC's pin goes pin →
//!   capacitor → via (a via the pin reaches without passing the capacitor is a warning). Close
//!   planes, a digital IC (one driving fast nets): the pin gets its own via (a pin whose only way
//!   to a via is through the capacitor's pad is a warning). Wide planes (or one plane, the other
//!   net routed): the capacitor sits at the pin of the farther plane (for ground that is the IC's
//!   ground pin, measured as `pi.decap_distance` measures the supply pin). No planes: the supply
//!   reaches the pin through the capacitor's pad, and the capacitor-to-pin run is within `run`.
//!   Pins fed through a bead are left out.
//!
//! **Regime** of a capacitor: the planes (pour fills) of its supply and of its ground under the
//! run from the pin to the capacitor (a net has a plane on a layer when half the run or more is
//! over it). Close when the nearest pair of them is thinner than `pi.decap_distance.close_h`
//! (0.3 mm), wide otherwise (the transitional 0.3–0.5 mm is judged as wide, the stricter); one
//! plane when only one of the two nets has one; none when neither does.
//!
//! Copper is followed through [`Copper`]: a net's pads, vias and track ends as a graph, with its
//! pours' islands when asked. Only kept records count (see [`super::roles`]).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use serde_json::json;

use super::super::airwires::{BoardNets, BoardPad};
use super::super::checks::{Severity, gap};
use super::super::connect::Connectivity;
use super::super::copper::{disc, pad_core};
use super::super::drc::{Bound, DrcItem};
use super::super::footprint::Core;
use super::super::grid::{Grid, boxes_meet, grow};
use super::super::infer::{self, BULK_FARADS, Inference, Record, parse_value};
use super::super::intent::{PartIntent, PartRole};
use super::super::layers::CopperLayer;
use super::super::path;
use super::catalog::{Code, Family, Param};
use super::geom::{Geo, along, distance, track_pieces};
use super::{Cx, Draft, num};

const ASSIGNED: &str = "pi.decap_assigned";
const DISTANCE: &str = "pi.decap_distance";
const MOUNTING: &str = "pi.decap_mounting";
const TOPOLOGY: &str = "pi.decap_topology";

const fn param(name: &'static str, default: f64, units: &'static str, min: f64, max: f64, source: &'static str, about: &'static str) -> Param {
    Param { name, default, units, source, min, max, verify: false, about }
}

const CODES: &[Code] = &[
    Code {
        code: ASSIGNED,
        class: super::super::drc::Class::Critical,
        severity: Severity::Error,
        worst: None,
        needs: "intent: decoupling-of for each IC power pin",
        section: "power-ground#decoupling",
        about: "an IC power-input pin (or the pin group its datasheet names) has no high-frequency capacitor assigned",
        principle: "Each power pin needs a capacitor of its own close by: the plane alone can't deliver its fast current without the supply sagging.",
        params: &[param(
            "group_gap",
            0.5,
            "mm",
            0.0,
            5.0,
            "judgment: pins of one net side by side (a charger's two BAT pins, VDD beside VDDH) are one pin group",
            "the IC's pins on one net this close, edge to edge, share a capacitor assigned to one of them",
        )],
    },
    Code {
        code: DISTANCE,
        class: super::super::drc::Class::Critical,
        severity: Severity::Warning,
        worst: Some((super::super::drc::Class::Critical, Severity::Error)),
        needs: "geometry and planes; intent: decoupling-of, the IC's nets' tiers, RF ICs",
        section: "power-ground#decoupling",
        about: "a decoupling capacitor farther from its pin than its regime allows (an error past twice); an RF IC's capacitor joined through vias, a capacitor on the other side without vias at both pads (errors)",
        principle: "The capacitor's current runs to the pin and back: the farther it sits, the larger that loop and its inductance, and the less the capacitor helps at high frequencies.",
        params: &[
            param(
                "fast",
                3.0,
                "mm",
                0.1,
                100.0,
                "LearnEMC, via the guide's decoupling table",
                "no or wide planes: the most for an IC driving T1-or-faster nets",
            ),
            param("slow", 5.0, "mm", 0.1, 100.0, "LearnEMC, via the guide's decoupling table", "no or wide planes: the most for other ICs"),
            param("close", 10.0, "mm", 0.1, 200.0, "LearnEMC: close planes share their capacitance", "close planes: the most"),
            param("rf", 2.0, "mm", 0.1, 50.0, "vendor RF reference layouts (Silicon Labs), via the guide", "an RF IC's capacitor: the most"),
            param("error_ratio", 2.0, "ratio", 1.0, 20.0, "judgment", "past this many times the limit it is an error"),
            param(
                "close_h",
                0.3,
                "mm",
                0.01,
                5.0,
                "CVEL FAQ; LearnEMC: close below 0.3 mm",
                "a supply and ground plane closer than this are close planes",
            ),
            param(
                "via_reach",
                1.0,
                "mm",
                0.0,
                20.0,
                "the guide's decoupling table",
                "a capacitor on the other side needs a via of its supply this close to its pad and to the pin",
            ),
        ],
    },
    Code {
        code: MOUNTING,
        class: super::super::drc::Class::Critical,
        severity: Severity::Warning,
        worst: None,
        needs: "geometry; intent: decoupling-of",
        section: "power-ground#decoupling",
        about: "a decoupling capacitor's track to its first via longer than 0.5 mm, a via far from its pad, vias at the far ends, one via for two capacitors; with the estimated mounting inductance",
        principle: "A capacitor's pads, tracks and vias add more inductance than the part itself: short tracks and close vias keep it working at high frequencies.",
        params: &[
            param(
                "trace",
                0.5,
                "mm",
                0.0,
                50.0,
                "Zeeff, UMR TR01-1-030 (0.61–15.7 nH over six layouts)",
                "the longest track from a pad to its first via",
            ),
            param("via_gap", 0.5, "mm", 0.0, 50.0, "Zeeff, UMR TR01-1-030", "the farthest a pad's first via may sit from the pad's edge"),
            param("search", 5.0, "mm", 0.5, 50.0, "judgment", "how far along its copper a pad's first via is looked for"),
            param("bulk_uf", 10.0, "µF", 0.0, 100_000.0, "the guide: bulk ≥ 10 µF", "capacitors of this value and up are bulk, not checked"),
        ],
    },
    Code {
        code: TOPOLOGY,
        class: super::super::drc::Class::Practical,
        severity: Severity::Warning,
        worst: None,
        needs: "geometry and planes; intent: decoupling-of, RF ICs, the rail's source",
        section: "power-ground#decoupling",
        about: "a capacitor joined to its pin against its regime: through its pad before the pin's own via (close planes), not at the far plane's pin (wide planes), not between source and pin (no planes); an RF IC's pin not pin → capacitor → via",
        principle: "Where the capacitor's current loop closes depends on the planes: close planes want short vias at the pin, wide or no planes want the capacitor right in the pin's path.",
        params: &[param(
            "run",
            3.0,
            "mm",
            0.1,
            100.0,
            "the guide's decoupling table",
            "no planes: the longest copper run from the capacitor to its pin",
        )],
    },
];

pub(super) const FAMILY: Family = Family { module: "pi_decap", title: "decoupling", codes: CODES, run };

fn run(cx: &mut Cx) {
    let wanted = [ASSIGNED, DISTANCE, MOUNTING, TOPOLOGY].into_iter().any(|c| cx.wants(c));
    if !wanted {
        return;
    }
    // The inference once: the IC supply pins, and what it proposes for them.
    if !cx.spend(cx.conn.nets.iter().map(|n| n.pins.len()).fold(cx.ecad.schematic.parts.len(), usize::saturating_add)) {
        return;
    }
    let inference = infer::infer(cx.ecad);
    let pins = Pins::new(cx.nets, cx.conn);
    if cx.wants(ASSIGNED) {
        assigned(cx, &inference, &pins);
    }
    if [DISTANCE, MOUNTING, TOPOLOGY].into_iter().any(|c| cx.wants(c)) {
        let caps = served(cx, &inference, &pins);
        if cx.wants(DISTANCE) {
            distances(cx, &caps);
        }
        if cx.wants(MOUNTING) {
            mounting(cx, &caps);
        }
        if cx.wants(TOPOLOGY) {
            topology(cx, &caps);
        }
    }
}

// ---- Shared with the regulator checks ----

/// The board's pads by part, and the pins' numbers and names, to find the pads a pin reference
/// names: `U1.B1` by number, `U1.VDD` by name (every pin of that name), `U1` the whole part.
pub(super) struct Pins<'a> {
    by_part: BTreeMap<&'a str, Vec<usize>>,
    numbers: BTreeMap<(&'a str, &'a str), Vec<&'a str>>,
    names: BTreeMap<(&'a str, &'a str), &'a str>,
}

impl<'a> Pins<'a> {
    pub(super) fn new(nets: &'a BoardNets, conn: &'a Connectivity) -> Pins<'a> {
        let mut by_part: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (i, p) in nets.pads.iter().enumerate() {
            by_part.entry(p.designator.as_str()).or_default().push(i);
        }
        let mut numbers: BTreeMap<(&str, &str), Vec<&str>> = BTreeMap::new();
        let mut names = BTreeMap::new();
        for q in conn.nets.iter().flat_map(|n| n.pins.iter()) {
            numbers.entry((q.designator.as_str(), q.name.as_str())).or_default().push(q.pin.as_str());
            names.insert((q.designator.as_str(), q.pin.as_str()), q.name.as_str());
        }
        Pins { by_part, numbers, names }
    }

    /// The pads of `part`, by index into the board's pads.
    pub(super) fn of_part(&self, part: &str) -> &[usize] {
        self.by_part.get(part).map_or(&[], Vec::as_slice)
    }

    /// The pads `reference` names (see [`Pins`]); none when its part isn't on the board.
    pub(super) fn resolve(&self, nets: &BoardNets, reference: &str) -> Vec<usize> {
        let Some((part, pin)) = reference.split_once('.') else { return self.of_part(reference).to_vec() };
        let pads = self.of_part(part);
        let by_number: Vec<usize> = pads.iter().copied().filter(|i| nets.pads.get(*i).is_some_and(|p| p.pad.number == pin)).collect();
        if !by_number.is_empty() {
            return by_number;
        }
        let Some(numbers) = self.numbers.get(&(part, pin)) else { return Vec::new() };
        pads.iter().copied().filter(|i| nets.pads.get(*i).is_some_and(|p| numbers.contains(&p.pad.number.as_str()))).collect()
    }

    /// The name of `part`'s pin `number`.
    pub(super) fn name(&self, part: &str, number: &str) -> Option<&'a str> {
        self.names.get(&(part, number)).copied()
    }
}

/// The copper layer a pad's part sits on.
pub(super) fn side_of(p: &BoardPad) -> CopperLayer {
    if p.top { CopperLayer::Top } else { CopperLayer::Bottom }
}

pub(super) fn pad_item(p: &BoardPad) -> DrcItem {
    DrcItem::Pad { footprint: p.footprint, number: p.pad.number.clone(), reference: p.reference() }
}

/// The gap between two pads' copper, edge to edge (seen from above).
pub(super) fn pad_gap(a: &BoardPad, b: &BoardPad) -> f64 {
    gap(&pad_core(a), &pad_core(b))
}

/// A part's value in base units (`100nF` → 1e-7), when it reads as one.
pub(super) fn value_of(cx: &Cx, designator: &str) -> Option<f64> {
    let part = cx.ecad.schematic.parts.iter().find(|p| p.designator == designator)?;
    let text = part.value.clone().or_else(|| cx.ecad.component(part.component).map(|c| c.value.clone()))?;
    parse_value(&text).map(|(v, _)| v)
}

/// What a node of [`Copper`] is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Kind {
    /// By index into the board's pads.
    Pad(usize),
    /// By index into the board's vias.
    Via(usize),
    /// Where track pieces end or meet, on a layer.
    Point(CopperLayer, [f64; 2]),
    /// An island of a pour's fill: by index into the fills, and into its shapes.
    Fill(usize, usize),
}

/// Track ends this close together are one point, in millimetres.
const JOIN: f64 = 0.01;

/// A net's copper as a graph: its pads, vias and the ends of its tracks' straight pieces (ends
/// that meet are one node), joined along the pieces (their length) and where copper touches (no
/// length: an end on a pad, in a via's ring or on another piece, a via in a pad); with its pours'
/// islands when asked, joined to what each touches.
pub(super) struct Copper {
    pub(super) kinds: Vec<Kind>,
    adj: Vec<Vec<(usize, f64)>>,
    pads: BTreeMap<usize, usize>,
    vias: BTreeMap<usize, usize>,
    /// The widest track ending at each point node.
    widths: BTreeMap<usize, f64>,
}

impl Copper {
    /// The copper of `net` on `layer` (every layer when `None`), what meets `area` (all of it
    /// when `None`), with its pours' islands when `fills`; `None` when past the budget.
    pub(super) fn build(cx: &mut Cx, net: usize, layer: Option<CopperLayer>, area: Option<[[f64; 2]; 2]>, fills: bool) -> Option<Copper> {
        let (board, nets) = (cx.board, cx.nets);
        let layers = board.copper_layers;
        let on = |l: CopperLayer| layer.is_none_or(|x| x == l);
        let near = |b: [[f64; 2]; 2]| area.is_none_or(|a| boxes_meet(a, b));
        let mut g = Copper { kinds: Vec::new(), adj: Vec::new(), pads: BTreeMap::new(), vias: BTreeMap::new(), widths: BTreeMap::new() };
        let pads: Vec<(usize, Core)> = nets
            .pads
            .iter()
            .enumerate()
            .filter(|(_, p)| p.net == Some(net) && layer.map_or(p.top || p.bottom, |l| p.on(l)) && near(p.bbox()))
            .map(|(i, p)| (i, pad_core(p)))
            .collect();
        let vias: Vec<usize> = board
            .vias
            .iter()
            .enumerate()
            .filter(|(i, v)| {
                nets.via_nets.get(*i).copied().flatten() == Some(net)
                    && layer.is_none_or(|l| v.on(l, layers))
                    && near(grow([v.at, v.at], v.diameter / 2.0))
            })
            .map(|(i, _)| i)
            .collect();
        // The pieces: layer, ends, width, track.
        let mut pieces: Vec<(CopperLayer, [f64; 2], [f64; 2], f64, usize)> = Vec::new();
        for (ti, t) in board.tracks.iter().enumerate() {
            if nets.track_nets.get(ti).copied().flatten() != Some(net) || !on(t.layer) {
                continue;
            }
            if !path::bbox(&t.points, &t.curves, false).is_some_and(|b| near(grow(b, t.width / 2.0))) {
                continue;
            }
            if !cx.spend(t.points.len().saturating_add(1)) {
                return None;
            }
            pieces.extend(track_pieces(&t.points, &t.curves).into_iter().map(|(a, b)| (t.layer, a, b, t.width, ti)));
        }
        if !cx.spend(pieces.len().saturating_mul(16).saturating_add(pads.len()).saturating_add(vias.len())) {
            return None;
        }
        for (i, _) in &pads {
            let n = g.node(Kind::Pad(*i));
            g.pads.insert(*i, n);
        }
        for i in &vias {
            let n = g.node(Kind::Via(*i));
            g.vias.insert(*i, n);
        }
        // The pieces' ends, those that meet merged.
        let mut points = Grid::new(1.0);
        let mut point_w: BTreeMap<usize, f64> = BTreeMap::new();
        let mut ends: Vec<(usize, usize)> = Vec::with_capacity(pieces.len());
        let mut end_of = |g: &mut Copper, layer: CopperLayer, p: [f64; 2], w: f64| -> usize {
            let found = points.near_box(grow([p, p], JOIN), g.kinds.len()).into_iter().find(|k| match g.kinds.get(*k) {
                Some(Kind::Point(l, q)) => *l == layer && distance(*q, p) <= JOIN,
                _ => false,
            });
            let n = found.unwrap_or_else(|| {
                let n = g.node(Kind::Point(layer, p));
                points.insert(n, [p, p]);
                n
            });
            let e = point_w.entry(n).or_insert(0.0);
            *e = e.max(w);
            n
        };
        for (layer, a, b, w, _) in &pieces {
            let (na, nb) = (end_of(&mut g, *layer, *a, *w), end_of(&mut g, *layer, *b, *w));
            g.join(na, nb, distance(*a, *b));
            ends.push((na, nb));
        }
        // What each end touches: a pad, a via's ring, another piece.
        let mut pad_grid = Grid::new(2.0);
        for (k, (_, core)) in pads.iter().enumerate() {
            pad_grid.insert(k, super::super::copper::core_bbox(core));
        }
        let mut via_grid = Grid::new(2.0);
        for (k, i) in vias.iter().enumerate() {
            if let Some(v) = board.vias.get(*i) {
                via_grid.insert(k, grow([v.at, v.at], v.diameter / 2.0));
            }
        }
        // Stacked and touching vias that share a layer (microvias over a buried via) are one
        // way down.
        for (k, i) in vias.iter().enumerate() {
            let Some(v) = board.vias.get(*i) else { continue };
            let near = via_grid.near_box(grow([v.at, v.at], v.diameter / 2.0), vias.len());
            if !cx.spend(near.len().saturating_add(1)) {
                return None;
            }
            for m in near.into_iter().filter(|m| *m > k) {
                let Some(j) = vias.get(m) else { continue };
                let Some(w) = board.vias.get(*j) else { continue };
                let shared = super::super::layers::stack(layers).into_iter().any(|l| on(l) && v.on(l, layers) && w.on(l, layers));
                if shared
                    && distance(v.at, w.at) <= (v.diameter + w.diameter) / 2.0
                    && let (Some(a), Some(b)) = (g.vias.get(i).copied(), g.vias.get(j).copied())
                {
                    g.join(a, b, 0.0);
                }
            }
        }
        let mut piece_grid = Grid::new(2.0);
        for (k, (_, a, b, w, _)) in pieces.iter().enumerate() {
            piece_grid.insert(k, grow([[a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]], w / 2.0));
        }
        let point_nodes: Vec<(usize, CopperLayer, [f64; 2], f64)> = g
            .kinds
            .iter()
            .enumerate()
            .filter_map(|(n, k)| match k {
                Kind::Point(l, p) => Some((n, *l, *p, point_w.get(&n).copied().unwrap_or(0.0))),
                _ => None,
            })
            .collect();
        for (n, l, p, w) in &point_nodes {
            let probe = grow([*p, *p], w / 2.0 + JOIN);
            let near_pads = pad_grid.near_box(probe, pads.len());
            let near_vias = via_grid.near_box(probe, vias.len());
            let near_pieces = piece_grid.near_box(probe, pieces.len());
            if !cx.spend(near_pads.len().saturating_add(near_vias.len()).saturating_add(near_pieces.len()).saturating_add(1)) {
                return None;
            }
            for k in near_pads {
                let Some((i, core)) = pads.get(k) else { continue };
                if nets.pads.get(*i).is_some_and(|q| q.on(*l))
                    && gap(&disc(*p, w / 2.0), core) <= JOIN
                    && let Some(m) = g.pads.get(i).copied()
                {
                    g.join(*n, m, 0.0);
                }
            }
            for k in near_vias {
                let Some(i) = vias.get(k) else { continue };
                let Some(v) = board.vias.get(*i) else { continue };
                if v.on(*l, layers)
                    && distance(*p, v.at) <= v.diameter / 2.0 + w / 2.0 + JOIN
                    && let Some(m) = g.vias.get(i).copied()
                {
                    g.join(*n, m, 0.0);
                }
            }
            // An end on another piece (a T): joined to both of that piece's ends along it.
            for k in near_pieces {
                let (Some((ql, a, b, qw, _)), Some((na, nb))) = (pieces.get(k), ends.get(k)) else { continue };
                if ql != l || *na == *n || *nb == *n {
                    continue;
                }
                if path::segment_distance(*p, *a, *b, 0.0) <= qw / 2.0 + JOIN {
                    let (q, _) = path::project(*p, *a, *b, 0.0);
                    g.join(*n, *na, distance(q, *a));
                    g.join(*n, *nb, distance(q, *b));
                }
            }
        }
        // A via in a pad.
        for (i, core) in &pads {
            let Some(pad) = nets.pads.get(*i) else { continue };
            let Some(pn) = g.pads.get(i).copied() else { continue };
            for k in via_grid.near_box(super::super::copper::core_bbox(core), vias.len()) {
                let Some(vi) = vias.get(k) else { continue };
                let Some(v) = board.vias.get(*vi) else { continue };
                let shared = [CopperLayer::Top, CopperLayer::Bottom].into_iter().any(|l| pad.on(l) && v.on(l, layers) && on(l));
                if shared
                    && gap(core, &disc(v.at, v.diameter / 2.0)) <= 0.0
                    && let Some(m) = g.vias.get(vi).copied()
                {
                    g.join(pn, m, 0.0);
                }
            }
        }
        if fills {
            g.add_fills(cx, net, layer, &pieces, &ends)?;
        }
        g.widths = point_w;
        Some(g)
    }

    fn add_fills(
        &mut self,
        cx: &mut Cx,
        net: usize,
        layer: Option<CopperLayer>,
        pieces: &[(CopperLayer, [f64; 2], [f64; 2], f64, usize)],
        ends: &[(usize, usize)],
    ) -> Option<()> {
        let nets = cx.nets;
        let mut islands: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        for (fi, f) in nets.fills.iter().enumerate() {
            if f.net != Some(net) || layer.is_some_and(|l| l != f.layer) {
                continue;
            }
            if !cx.spend(f.joins.len().saturating_add(1)) {
                return None;
            }
            for (shape, node) in &f.joins {
                let other = match node {
                    super::super::fill::Node::Pad(i) => self.pads.get(i).copied().into_iter().collect::<Vec<_>>(),
                    super::super::fill::Node::Via(i) => self.vias.get(i).copied().into_iter().collect(),
                    super::super::fill::Node::Track(ti) => {
                        // The ends of the track's pieces inside the island.
                        let mine: Vec<(usize, [f64; 2])> = pieces
                            .iter()
                            .zip(ends)
                            .filter(|((_, _, _, _, t), _)| t == ti)
                            .flat_map(|((_, a, b, _, _), (na, nb))| [(*na, *a), (*nb, *b)])
                            .collect();
                        let at: Vec<[f64; 2]> = mine.iter().map(|(_, p)| *p).collect();
                        if !cx.spend(at.len().saturating_add(1)) {
                            return None;
                        }
                        f.region.locate(&at).into_iter().zip(mine).filter(|(s, _)| *s == Some(*shape)).map(|(_, (n, _))| n).collect()
                    }
                    super::super::fill::Node::Fill(pour, s) => nets
                        .fills
                        .iter()
                        .position(|o| o.pour == *pour && layer.is_none_or(|l| l == o.layer))
                        .map(|fj| *islands.entry((fj, *s)).or_insert_with(|| self.node(Kind::Fill(fj, *s))))
                        .into_iter()
                        .collect(),
                };
                if other.is_empty() {
                    continue;
                }
                let island = *islands.entry((fi, *shape)).or_insert_with(|| self.node(Kind::Fill(fi, *shape)));
                for m in other {
                    self.join(island, m, 0.0);
                }
            }
        }
        Some(())
    }

    fn node(&mut self, k: Kind) -> usize {
        self.kinds.push(k);
        self.adj.push(Vec::new());
        self.kinds.len() - 1
    }

    fn join(&mut self, a: usize, b: usize, len: f64) {
        if a == b || !len.is_finite() {
            return;
        }
        if let Some(l) = self.adj.get_mut(a) {
            l.push((b, len));
        }
        if let Some(l) = self.adj.get_mut(b) {
            l.push((a, len));
        }
    }

    pub(super) fn pad(&self, i: usize) -> Option<usize> {
        self.pads.get(&i).copied()
    }

    /// The pad node `n` is on: the pad itself, a track end on it, a via in it.
    pub(super) fn pad_at(&self, cx: &Cx, n: usize) -> Option<usize> {
        let layers = cx.board.copper_layers;
        let on = |i: usize| -> bool {
            let Some(p) = cx.nets.pads.get(i) else { return false };
            match self.kinds.get(n) {
                Some(Kind::Point(l, q)) => p.on(*l) && gap(&pad_core(p), &disc(*q, 0.0)) <= JOIN,
                Some(Kind::Via(v)) => cx.board.vias.get(*v).is_some_and(|v| {
                    [CopperLayer::Top, CopperLayer::Bottom].into_iter().any(|l| p.on(l) && v.on(l, layers))
                        && gap(&pad_core(p), &disc(v.at, v.diameter / 2.0)) <= 0.0
                }),
                _ => false,
            }
        };
        match self.kinds.get(n)? {
            Kind::Pad(i) => Some(*i),
            Kind::Point(..) | Kind::Via(_) => self.pads.keys().copied().find(|i| on(*i)),
            Kind::Fill(..) => None,
        }
    }

    /// The nodes on the pad `i`'s copper: the pad, the track ends on it and the vias in it. A
    /// way that passes any of them passes the pad (two tracks may meet inside it).
    pub(super) fn on_pad(&self, cx: &Cx, i: usize) -> BTreeSet<usize> {
        let (Some(p), Some(n)) = (cx.nets.pads.get(i), self.pad(i)) else { return BTreeSet::new() };
        let core = pad_core(p);
        let layers = cx.board.copper_layers;
        let mut out = BTreeSet::from([n]);
        for (k, kind) in self.kinds.iter().enumerate() {
            let on = match kind {
                Kind::Point(l, q) => p.on(*l) && gap(&core, &disc(*q, 0.0)) <= JOIN,
                Kind::Via(v) => cx.board.vias.get(*v).is_some_and(|v| {
                    [CopperLayer::Top, CopperLayer::Bottom].into_iter().any(|l| p.on(l) && v.on(l, layers))
                        && gap(&core, &disc(v.at, v.diameter / 2.0)) <= 0.0
                }),
                _ => false,
            };
            if on {
                out.insert(k);
            }
        }
        out
    }

    /// The narrowest track along `way` (node by node); `None` with no track on it.
    pub(super) fn narrowest(&self, way: &[usize]) -> Option<f64> {
        way.iter().filter_map(|n| self.widths.get(n)).copied().filter(|w| *w > 0.0).min_by(f64::total_cmp)
    }

    pub(super) fn neighbours(&self, n: usize) -> BTreeSet<usize> {
        self.adj.get(n).map(|l| l.iter().map(|(m, _)| *m).filter(|m| *m != n).collect()).unwrap_or_default()
    }

    /// Where node `n` is (a fill's island has no one place).
    pub(super) fn at(&self, cx: &Cx, n: usize) -> Option<[f64; 2]> {
        match self.kinds.get(n)? {
            Kind::Pad(i) => cx.nets.pads.get(*i).map(BoardPad::at),
            Kind::Via(i) => cx.board.vias.get(*i).map(|v| v.at),
            Kind::Point(_, p) => Some(*p),
            Kind::Fill(..) => None,
        }
    }

    /// The shortest copper distance from `from` to every node within `limit` (infinite past it),
    /// and the node each is reached from; a node `through` refuses is reached but not passed.
    pub(super) fn walk(&self, cx: &mut Cx, from: usize, through: impl Fn(usize) -> bool, limit: f64) -> (Vec<f64>, Vec<Option<usize>>) {
        let n = self.kinds.len();
        let mut dist = vec![f64::INFINITY; n];
        let mut prev: Vec<Option<usize>> = vec![None; n];
        let edges: usize = self.adj.iter().map(Vec::len).fold(n, usize::saturating_add);
        if !cx.spend(edges.saturating_mul(2)) {
            return (dist, prev);
        }
        let Some(d0) = dist.get_mut(from) else { return (dist, prev) };
        *d0 = 0.0;
        // Distances are finite and not negative, so their bits sort as they do.
        let mut heap = BinaryHeap::new();
        heap.push(Reverse((0.0_f64.to_bits(), from)));
        while let Some(Reverse((bits, u))) = heap.pop() {
            let d = f64::from_bits(bits);
            if dist.get(u).is_none_or(|x| d > *x) {
                continue;
            }
            if u != from && !through(u) {
                continue;
            }
            for (v, len) in self.adj.get(u).into_iter().flatten() {
                let nd = d + len;
                if nd > limit {
                    continue;
                }
                if let Some(slot) = dist.get_mut(*v)
                    && nd < *slot
                {
                    *slot = nd;
                    if let Some(p) = prev.get_mut(*v) {
                        *p = Some(u);
                    }
                    heap.push(Reverse((nd.to_bits(), *v)));
                }
            }
        }
        (dist, prev)
    }

    /// The nearest via `from` reaches within `limit`, the distance, and the nodes on the way
    /// (`from` first); a node `through` refuses is not passed, nor taken.
    pub(super) fn first_via(&self, cx: &mut Cx, from: usize, through: impl Fn(usize) -> bool, limit: f64) -> Option<(usize, f64, Vec<usize>)> {
        let (dist, prev) = self.walk(cx, from, &through, limit);
        let (node, via, d) = self
            .kinds
            .iter()
            .enumerate()
            .filter_map(|(k, kind)| match kind {
                Kind::Via(i) if through(k) => Some((k, *i, *dist.get(k)?)),
                _ => None,
            })
            .filter(|(_, _, d)| d.is_finite())
            .min_by(|a, b| a.2.total_cmp(&b.2).then(a.1.cmp(&b.1)))?;
        let mut way = vec![node];
        let mut at = node;
        while let Some(p) = prev.get(at).copied().flatten() {
            way.push(p);
            at = p;
            if way.len() > self.kinds.len() {
                break;
            }
        }
        way.reverse();
        Some((via, d, way))
    }
}

/// How a capacitor's supply and ground planes lie (see the module notes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Pair {
    None,
    /// Only one of the two nets has a plane: the other (`routed`) is routed.
    One {
        routed: usize,
    },
    Close,
    Wide,
}

pub(super) struct Regime {
    pub(super) pair: Pair,
    /// In words, for a finding's regime.
    pub(super) words: String,
    /// The plane layers of the supply and of the ground, nearest the part's side first.
    pub(super) supply_layers: Vec<CopperLayer>,
    pub(super) ground_layers: Vec<CopperLayer>,
    /// The nearest pair of them.
    pub(super) layers: Option<(CopperLayer, CopperLayer)>,
}

/// The regime of `supply` and `ground` over the run through `points` (the pin, the capacitor),
/// seen from `side`: a net has a plane on a layer when at least half the run is over it (a via's
/// antipad under one end is no missing plane).
pub(super) fn regime(
    geo: &Geo,
    nets: &BoardNets,
    points: &[[f64; 2]],
    side: CopperLayer,
    supply: usize,
    ground: Option<usize>,
    close_h: f64,
) -> Regime {
    let name = |n: usize| nets.name(n).unwrap_or("?").to_string();
    // At most about 200 points a step, however far apart.
    let mut run: Vec<[f64; 2]> =
        points.windows(2).flat_map(|w| if let [a, b] = w { along(*a, *b, (distance(*a, *b) / 200.0).max(0.25)) } else { Vec::new() }).collect();
    if run.is_empty() {
        run = points.to_vec();
    }
    // Per layer with planes, the share of the run over each net's.
    let over: Vec<(CopperLayer, Vec<Option<usize>>)> =
        geo.layers.iter().filter_map(|l| geo.plane(*l).filter(|pl| !pl.is_empty()).map(|pl| (*l, pl.nets_at(nets, &run)))).collect();
    let planes = |n: usize| -> Vec<CopperLayer> {
        let mut out: Vec<CopperLayer> =
            over.iter().filter(|(_, at)| !at.is_empty() && at.iter().filter(|x| **x == Some(n)).count() * 2 >= at.len()).map(|(l, _)| *l).collect();
        out.sort_by(|a, b| geo.height(side, *a).total_cmp(&geo.height(side, *b)));
        out
    };
    let supply_layers = planes(supply);
    let ground_layers = ground.map(planes).unwrap_or_default();
    let best = supply_layers
        .iter()
        .flat_map(|s| ground_layers.iter().filter(move |g| *g != s).map(move |g| (*s, *g)))
        .min_by(|a, b| geo.height(a.0, a.1).total_cmp(&geo.height(b.0, b.1)));
    let g_name = ground.map(name).unwrap_or_else(|| "ground".into());
    let (pair, words) = match (supply_layers.first(), ground_layers.first(), best) {
        (_, _, Some((s, g))) => {
            let h = geo.height(s, g);
            let at = format!("{} on {}, {g_name} on {}, {} mm apart", name(supply), s.name(), g.name(), num(h));
            if h < close_h {
                (Pair::Close, format!("close planes: {at}"))
            } else if h <= 0.5 {
                (Pair::Wide, format!("transitional planes, judged as wide: {at}"))
            } else {
                (Pair::Wide, format!("wide planes: {at}"))
            }
        }
        (Some(s), None, None) => {
            (Pair::One { routed: ground.unwrap_or(supply) }, format!("one plane: {} on {}; {g_name} routed", name(supply), s.name()))
        }
        (None, Some(g), None) => (Pair::One { routed: supply }, format!("one plane: {g_name} on {}; {} routed", g.name(), name(supply))),
        // Both on one layer only: no pair.
        (Some(s), Some(_), None) => (Pair::One { routed: supply }, format!("one plane layer: {} and {g_name} share {}", name(supply), s.name())),
        (None, None, None) => (Pair::None, format!("no planes: {} and {g_name} routed", name(supply))),
    };
    Regime { pair, words, supply_layers, ground_layers, layers: best }
}

/// Whether a pad's net is fed through a ferrite or filter: such a part (kept with that role, or
/// a bead, `FB1`) joins it to a net a supply comes from (see [`source_pads`]). The supply's own
/// side of the filter is not fed through it.
pub(super) fn filtered(cx: &Cx, net: usize) -> Option<String> {
    let (nets, roles) = (cx.nets, cx.roles);
    let filter = |d: &str| {
        roles.intent.part(d).is_some_and(|r| matches!(r.role, PartRole::Ferrite | PartRole::Filter))
            || (d.starts_with("FB") && d.get(2..).is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())))
    };
    let on: BTreeSet<&str> = nets.pads.iter().filter(|p| p.net == Some(net) && filter(&p.designator)).map(|p| p.designator.as_str()).collect();
    on.into_iter()
        .find(|f| {
            let other = nets.pads.iter().filter(|p| p.designator == *f).filter_map(|p| p.net).find(|n| *n != net);
            other.is_some_and(|o| !source_pads(cx, o, "", f).is_empty())
        })
        .map(str::to_string)
}

// ---- pi.decap_assigned ----

/// The kept records that give a pin a capacitor.
fn serves(r: &PartIntent) -> bool {
    matches!(r.role, PartRole::Decoupling | PartRole::Bulk | PartRole::SmpsInCap | PartRole::SmpsOutCap)
}

/// Whether the record's `of` names `pin` (`U1.B1`) of `part` named `name`.
fn names_pin(of: &[String], part: &str, number: &str, name: Option<&str>) -> bool {
    of.iter().any(|o| match o.split_once('.') {
        Some((p, x)) => p == part && (x == number || name.is_some_and(|n| !n.is_empty() && n == x)),
        None => o == part,
    })
}

/// The board pads `records` give a capacitor, each with the capacitor: the pins their `of`
/// names, an `of` naming a net read as the IC supply pin on it nearest the capacitor, and with
/// each pin the pins of its part beside it on its net (within `group_gap`: one pin group).
fn covered<'r>(cx: &mut Cx, pins: &Pins, records: &[&'r PartIntent], power: &BTreeSet<&str>, group_gap: f64) -> BTreeMap<usize, &'r str> {
    let nets = cx.nets;
    let mut out: BTreeMap<usize, &str> = BTreeMap::new();
    for r in records {
        let own = pins.of_part(&r.part);
        for entry in &r.of {
            if !cx.spend(own.len().saturating_add(nets.pads.len()).saturating_add(1)) {
                return out;
            }
            let mut found: Vec<usize> = pins.resolve(nets, entry).into_iter().filter(|i| !own.contains(i)).collect();
            if found.is_empty()
                && let Some(n) = nets.names.iter().position(|x| x == entry)
            {
                // A rail: the IC supply pin on it nearest the capacitor.
                let caps: Vec<&BoardPad> = own.iter().filter_map(|i| nets.pads.get(*i)).collect();
                let nearest = nets
                    .pads
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.net == Some(n) && power.contains(format!("{}.{}", p.designator, p.pad.number).as_str()))
                    .filter_map(|(i, p)| Some((i, caps.iter().map(|c| pad_gap(c, p)).min_by(f64::total_cmp)?)))
                    .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
                found.extend(nearest.map(|(i, _)| i));
            }
            for i in found {
                // A pad already in is in with its whole group.
                if out.contains_key(&i) {
                    continue;
                }
                let Some(g) = group(cx, pins, i, group_gap) else { return out };
                for j in g {
                    out.entry(j).or_insert(r.part.as_str());
                }
            }
        }
    }
    out
}

/// The pad `i` and the pads of its part on its net beside it, one after another (each within
/// `gap` of the last, edge to edge); `None` when past the budget.
fn group(cx: &mut Cx, pins: &Pins, i: usize, gap: f64) -> Option<Vec<usize>> {
    let nets = cx.nets;
    let pad = nets.pads.get(i)?;
    let same: Vec<usize> = pins
        .of_part(&pad.designator)
        .iter()
        .copied()
        .filter(|j| *j != i && nets.pads.get(*j).is_some_and(|q| q.net.is_some() && q.net == pad.net))
        .collect();
    let mut seen: BTreeSet<usize> = BTreeSet::from([i]);
    let mut out = vec![i];
    let mut k = 0;
    while let Some(at) = out.get(k).and_then(|j| nets.pads.get(*j)) {
        if !cx.spend(same.len().saturating_add(1)) {
            return None;
        }
        let next: Vec<usize> =
            same.iter().copied().filter(|j| !seen.contains(j) && nets.pads.get(*j).is_some_and(|q| pad_gap(at, q) <= gap + 1e-9)).collect();
        seen.extend(next.iter().copied());
        out.extend(next);
        k += 1;
    }
    Some(out)
}

/// Whether `n` is a ground net: kept as ground, or named as one.
fn ground_net(cx: &Cx, n: usize) -> bool {
    cx.roles.grounds.contains(&n)
        || cx.nets.name(n).is_some_and(|x| matches!(x.to_ascii_uppercase().as_str(), "GND" | "AGND" | "PGND" | "DGND" | "VSS" | "0V"))
}

/// The capacitors on `net` (to ground) the design places but doesn't mount (`dnp`): a site its
/// designer left empty, as a datasheet's reference does for pins that need no capacitor.
fn unmounted(cx: &Cx, net: usize) -> Vec<String> {
    let (nets, sheet) = (cx.nets, &cx.ecad.schematic);
    let mut out: Vec<String> = nets
        .pads
        .iter()
        .filter(|p| p.net == Some(net) && p.designator.starts_with('C'))
        .filter(|p| sheet.parts.iter().any(|s| s.designator == p.designator && s.dnp))
        .filter(|p| nets.pads.iter().any(|q| q.designator == p.designator && q.net.is_some_and(|g| g != net && ground_net(cx, g))))
        .map(|p| p.designator.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn assigned(cx: &mut Cx, inference: &Inference, pins: &Pins) {
    let (nets, roles, conn) = (cx.nets, cx.roles, cx.conn);
    let group_gap = cx.param(ASSIGNED, "group_gap");
    let kept: Vec<&PartIntent> = roles.intent.parts.iter().filter(|r| serves(r)).collect();
    // A part's kept record stands in place of what is proposed for it.
    let proposed: Vec<&PartIntent> = inference
        .proposals
        .iter()
        .filter_map(|p| if let Record::Part(r) = &p.record { Some(r) } else { None })
        .filter(|r| serves(r) && roles.intent.part(&r.part).is_none())
        .collect();
    let modules: BTreeSet<&str> = inference
        .proposals
        .iter()
        .filter_map(|p| if let Record::Part(r) = &p.record { Some(r) } else { None })
        .filter(|r| r.role == PartRole::Module)
        .map(|r| r.part.as_str())
        .collect();
    let power: BTreeSet<&str> = inference.power_pins.iter().map(String::as_str).collect();
    let kept_pads = covered(cx, pins, &kept, &power, group_gap);
    let proposed_pads = covered(cx, pins, &proposed, &power, group_gap);
    for pin in &inference.power_pins {
        let Some((part, number)) = pin.split_once('.') else { continue };
        // A module carries its own decoupling: what it asks for outside is in its datasheet.
        if roles.intent.part(part).is_some_and(|r| r.role == PartRole::Module) {
            continue;
        }
        if roles.intent.part(part).is_none() && modules.contains(part) {
            cx.not_applicable(ASSIGNED, format!("intent: the role of {part} (a module proposed: accept or correct it)"));
            continue;
        }
        let name = pins.name(part, number);
        let own: Vec<usize> = pins.of_part(part).iter().copied().filter(|i| nets.pads.get(*i).is_some_and(|p| p.pad.number == number)).collect();
        if kept.iter().any(|r| names_pin(&r.of, part, number, name)) || own.iter().any(|i| kept_pads.contains_key(i)) {
            continue;
        }
        let by_proposal = proposed.iter().find(|r| names_pin(&r.of, part, number, name)).map(|r| r.part.as_str());
        if let Some(cap) = by_proposal.or_else(|| own.iter().find_map(|i| proposed_pads.get(i).copied())) {
            cx.not_applicable(ASSIGNED, format!("intent: decoupling-of {pin} ({cap} proposed: accept or correct it)"));
            continue;
        }
        let net =
            conn.nets.iter().find(|n| n.pins.iter().any(|q| q.designator == part && q.pin == number)).map(|n| n.name.clone()).unwrap_or_default();
        let pad = own.first().and_then(|i| nets.pads.get(*i));
        let at = pad.map_or([0.0, 0.0], BoardPad::at);
        let items: Vec<DrcItem> = pad.map(pad_item).into_iter().collect();
        let key = pad.map_or(format!("pin.{pin}"), |p| pad_item(p).key());
        let empty = pad.and_then(|p| p.net).map(|n| unmounted(cx, n)).unwrap_or_default();
        let (pin, part, name) = (pin.clone(), part.to_string(), name.unwrap_or("").to_string());
        if !empty.is_empty() {
            // A site left empty is the designer's decision: a look settles it.
            let caps = empty.join(", ");
            cx.add(ASSIGNED, &[key, "dnp".into()], || {
                Draft::new(
                    at,
                    items,
                    format!("{pin} ({name}) on {net} has only {caps}, placed but not mounted: confirm its datasheet asks for no capacitor there"),
                )
                .nets(vec![net.clone()])
                .severity(Severity::Info)
                .review()
                .variant("dnp")
                .param("")
                .arg("pin", pin.clone())
                .arg("name", name.clone())
                .arg("net", net.clone())
                .arg("parts", caps.clone())
                .fix(
                    "ecad.intent.inspect",
                    json!({"parts": [part], "propose": true}),
                    format!(
                        "check {part}'s datasheet for {pin}: mount {caps} if it asks for a capacitor, else approve this with the datasheet's words"
                    ),
                )
            });
            continue;
        }
        cx.add(ASSIGNED, &[key], || {
            Draft::new(at, items, format!("{pin} ({name}) on {net} has no high-frequency capacitor assigned, and none is proposed"))
                .nets(vec![net.clone()])
                .param("")
                .arg("pin", pin.clone())
                .arg("name", name.clone())
                .arg("net", net.clone())
                .fix(
                    "ecad.intent.inspect",
                    json!({"parts": [part], "propose": true}),
                    format!("see what serves {part}'s supply pins; assign a capacitor on {net} to {pin} with ecad.intent.set (role decoupling, of [{pin}]), or add one at the pin"),
                )
        });
    }
}

// ---- The capacitors on the board ----

/// A decoupling capacitor and the pin it serves, both on the board.
struct Served<'a> {
    record: &'a PartIntent,
    /// The capacitor's pad on the pin's net and its other pad, the pin's pad: by index into the
    /// board's pads.
    supply_pad: usize,
    ground_pad: usize,
    pin: usize,
    /// The `of` entry, as decoupling loops are named after it.
    entry: String,
    ic: String,
    net: usize,
    ground: Option<usize>,
    farads: Option<f64>,
}

impl Served<'_> {
    fn pads<'n>(&self, nets: &'n BoardNets) -> Option<(&'n BoardPad, &'n BoardPad, &'n BoardPad)> {
        Some((nets.pads.get(self.supply_pad)?, nets.pads.get(self.ground_pad)?, nets.pads.get(self.pin)?))
    }

    fn bulk(&self) -> bool {
        self.farads.is_some_and(|f| f >= BULK_FARADS - 1e-12)
    }
}

/// Each kept decoupling record with each pin it serves, on the board; what can't be placed is
/// listed as not applicable.
fn served<'a>(cx: &mut Cx<'a>, inference: &Inference, pins: &Pins) -> Vec<Served<'a>> {
    let (nets, roles) = (cx.nets, cx.roles);
    let codes: Vec<&'static str> = [DISTANCE, MOUNTING, TOPOLOGY].into_iter().filter(|c| cx.wants(c)).collect();
    let mut out = Vec::new();
    let records: Vec<&PartIntent> = roles.parts(PartRole::Decoupling).collect();
    if records.is_empty() {
        if !inference.power_pins.is_empty() {
            for c in &codes {
                cx.not_applicable(c, "intent: decoupling-of records for the IC power pins (ecad.intent.inspect proposes them)");
            }
        }
        return out;
    }
    for r in records {
        let own = pins.of_part(&r.part);
        let cap: Vec<usize> = own.iter().copied().filter(|i| nets.pads.get(*i).is_some_and(|p| p.net.is_some())).collect();
        let [a, b] = cap.as_slice() else {
            for c in &codes {
                cx.not_applicable(c, format!("geometry: {} on the board with its two pads on nets", r.part));
            }
            continue;
        };
        let farads = value_of(cx, &r.part);
        for entry in &r.of {
            let candidates: Vec<usize> = pins.resolve(nets, entry).into_iter().filter(|i| !own.contains(i)).collect();
            let best = candidates
                .iter()
                .filter_map(|i| {
                    let p = nets.pads.get(*i)?;
                    let (s, g) =
                        [(*a, *b), (*b, *a)].into_iter().find(|(s, _)| nets.pads.get(*s).is_some_and(|q| q.net.is_some() && q.net == p.net))?;
                    let d = pad_gap(nets.pads.get(s)?, p);
                    Some((*i, s, g, d))
                })
                .min_by(|x, y| x.3.total_cmp(&y.3).then(x.0.cmp(&y.0)));
            let Some((pin, supply_pad, ground_pad, _)) = best else {
                let why = if candidates.is_empty() && !entry.contains('.') && nets.names.iter().any(|n| n == entry) {
                    format!("intent: the pin {} serves (its of names the net {entry})", r.part)
                } else if candidates.is_empty() {
                    format!("geometry: {entry} on the board")
                } else {
                    format!("intent: {}'s of names {entry}, on neither of its nets", r.part)
                };
                for c in &codes {
                    cx.not_applicable(c, why.clone());
                }
                continue;
            };
            let (Some(net), ground) = (nets.pads.get(pin).and_then(|p| p.net), nets.pads.get(ground_pad).and_then(|p| p.net)) else {
                continue;
            };
            let ic = nets.pads.get(pin).map(|p| p.designator.clone()).unwrap_or_default();
            out.push(Served { record: r, supply_pad, ground_pad, pin, entry: entry.clone(), ic, net, ground, farads });
        }
    }
    out
}

/// Whether the IC is kept as an RF IC.
fn rf_ic(cx: &Cx, ic: &str) -> bool {
    cx.roles.intent.part(ic).is_some_and(|r| r.role == PartRole::RfIc)
}

/// A net the IC drives at T1 or faster (no supply), with its regime: a fast IC.
fn fast_ic(cx: &Cx, pins: &[usize]) -> Option<String> {
    let mut seen = BTreeSet::new();
    for i in pins {
        let Some(n) = cx.nets.pads.get(*i).and_then(|p| p.net) else { continue };
        if !seen.insert(n) {
            continue;
        }
        if let Some(info) = cx.roles.net(n).filter(|x| x.fast() && !x.is_supply()) {
            return Some(format!("{}: {}", info.name, info.regime()));
        }
    }
    None
}

/// The IC's pads, by index.
fn ic_pads(nets: &BoardNets, ic: &str) -> Vec<usize> {
    nets.pads.iter().enumerate().filter(|(_, p)| p.designator == ic).map(|(i, _)| i).collect()
}

/// The box round the IC's pads: what a capacitor under it from the other side sits in.
fn pad_field(nets: &BoardNets, pads: &[usize]) -> Option<[[f64; 2]; 2]> {
    super::super::bbox(pads.iter().filter_map(|i| nets.pads.get(*i)).flat_map(|p| {
        let [lo, hi] = p.bbox();
        [lo, hi]
    }))
}

/// Whether a via of `net` is within `reach` of the pad's copper, on the pad's layer.
fn via_near(cx: &Cx, net: usize, pad: &BoardPad, reach: f64) -> bool {
    let (board, nets) = (cx.board, cx.nets);
    let core = pad_core(pad);
    let layer = side_of(pad);
    board.vias.iter().enumerate().any(|(i, v)| {
        nets.via_nets.get(i).copied().flatten() == Some(net) && v.on(layer, board.copper_layers) && gap(&core, &disc(v.at, v.diameter / 2.0)) <= reach
    })
}

/// Whether two pads of `net` are joined by its copper in `area` on `layer` (tracks, a via's ring
/// passed through, the net's pour there), or on any layers when `None`; `None` past the budget.
pub(super) fn joined_on(cx: &mut Cx, net: usize, layer: Option<CopperLayer>, a: usize, b: usize, area: [[f64; 2]; 2]) -> Option<bool> {
    let g = Copper::build(cx, net, layer, Some(area), true)?;
    let (Some(na), Some(nb)) = (g.pad(a), g.pad(b)) else { return Some(false) };
    let (dist, _) = g.walk(cx, na, |_| true, f64::INFINITY);
    Some(dist.get(nb).is_some_and(|d| d.is_finite()))
}

/// A box round two pads, grown by `r`.
fn around(a: &BoardPad, b: &BoardPad, r: f64) -> [[f64; 2]; 2] {
    let [[ax0, ay0], [ax1, ay1]] = a.bbox();
    let [[bx0, by0], [bx1, by1]] = b.bbox();
    grow([[ax0.min(bx0), ay0.min(by0)], [ax1.max(bx1), ay1.max(by1)]], r)
}

// ---- pi.decap_distance ----

fn distances(cx: &mut Cx, caps: &[Served]) {
    let (board, nets, geo) = (cx.board, cx.nets, cx.geo);
    let p = |name: &str| super::param_value(board, DISTANCE, name);
    let (close_h, ratio, via_reach) = (p("close_h"), p("error_ratio").max(1.0), p("via_reach"));
    for s in caps {
        if s.bulk() {
            continue;
        }
        let Some((cap, _, pin)) = s.pads(nets) else { continue };
        if !cx.spend(64) {
            return;
        }
        // A pin fed through a bead or filter: its capacitor is the filter's (pi.ferrite_rail).
        if filtered(cx, s.net).is_some() {
            continue;
        }
        let ic_pads = ic_pads(nets, &s.ic);
        let rf = rf_ic(cx, &s.ic);
        let side = side_of(pin);
        let reg = regime(geo, nets, &[pin.at(), cap.at()], side, s.net, s.ground, close_h);
        let fast = fast_ic(cx, &ic_pads);
        let (limit, param, why) = if rf {
            (p("rf"), "rf", format!("RF IC {}", s.ic))
        } else if reg.pair == Pair::Close {
            (p("close"), "close", reg.words.clone())
        } else if let Some(f) = &fast {
            (p("fast"), "fast", format!("{}; fast IC ({f})", reg.words))
        } else {
            (p("slow"), "slow", format!("{}; IC with no fast nets", reg.words))
        };
        let net_name = nets.name(s.net).unwrap_or("?").to_string();
        let (cap_ref, pin_ref, part) = (cap.reference(), pin.reference(), s.record.part.clone());
        let items = vec![pad_item(cap), pad_item(pin)];
        let keys = vec![pad_item(cap).key(), pad_item(pin).key()];
        let loop_name = format!("{}→{}", s.record.part, s.entry);
        let opposite = side_of(cap) != side;
        let under = opposite && pad_field(nets, &ic_pads).is_some_and(|b| boxes_meet(b, [cap.at(), cap.at()]));
        if opposite && !(via_near(cx, s.net, cap, via_reach) && via_near(cx, s.net, pin, via_reach)) {
            let mut k = keys.clone();
            k.push("side".into());
            let (items, regime, net_name, cap_ref, pin_ref) = (items.clone(), reg.words.clone(), net_name.clone(), cap_ref.clone(), pin_ref.clone());
            cx.add(DISTANCE, &k, || {
                Draft::new(
                    cap.at(),
                    items,
                    format!(
                        "{cap_ref} is on the other side from {pin_ref} ({net_name}) with no {net_name} via within {} mm of both pads",
                        num(via_reach)
                    ),
                )
                .nets(vec![net_name.clone()])
                .severity(Severity::Error)
                .variant("side")
                .arg("reach", num(via_reach))
                .param("via_reach")
                .regime(regime)
                .fix(
                    "ecad.board.via",
                    serde_json::Value::Null,
                    format!(
                        "put a {net_name} via within {} mm of {cap_ref} and of {pin_ref}, or move the capacitor to the pin's side",
                        num(via_reach)
                    ),
                )
            });
        }
        let d = pad_gap(cap, pin);
        if !under && d > limit + 1e-9 {
            let severity = if d > ratio * limit { Severity::Error } else { Severity::Warning };
            let (items, net_name, cap_ref, pin_ref, part) = (items.clone(), net_name.clone(), cap_ref.clone(), pin_ref.clone(), part.clone());
            let loop_name = loop_name.clone();
            cx.add(DISTANCE, &keys, || {
                Draft::new(
                    cap.at(),
                    items,
                    format!("{cap_ref} is {} mm from {pin_ref} ({net_name}), edge to edge; {why} allows {} mm", num(d), num(limit)),
                )
                .nets(vec![net_name.clone()])
                .measure(d, limit, "mm", Bound::Max)
                .param(param)
                .severity(severity)
                .regime(why.clone())
                .fix("ecad.board.move", serde_json::Value::Null, format!("move {part} within {} mm of {pin_ref}", num(limit)))
                .fix("ecad.board.loop.inspect", json!({"loops": [loop_name]}), "see the decoupling loop's estimated inductance")
            });
        }
        if rf && !opposite {
            let area = around(cap, pin, 2.0);
            let Some(surface) = joined_on(cx, s.net, Some(side), s.supply_pad, s.pin, area) else { return };
            // Not joined at all nearby: not routed yet, the connection check's.
            let Some(at_all) = (if surface { Some(true) } else { joined_on(cx, s.net, None, s.supply_pad, s.pin, grow(area, 5.0)) }) else { return };
            if !surface && at_all {
                let mut k = keys.clone();
                k.push("via".into());
                let (items, net_name, cap_ref, pin_ref) = (items.clone(), net_name.clone(), cap_ref.clone(), pin_ref.clone());
                cx.add(DISTANCE, &k, || {
                    Draft::new(
                        cap.at(),
                        items,
                        format!(
                            "{cap_ref} reaches RF IC pin {pin_ref} ({net_name}) only through vias: the vendor reference joins them on the surface"
                        ),
                    )
                    .nets(vec![net_name.clone()])
                    .severity(Severity::Error)
                    .variant("via")
                    .param("rf")
                    .regime(format!("RF IC {}", s.ic))
                    .fix(
                        "ecad.board.track",
                        serde_json::Value::Null,
                        format!("route {pin_ref} to {cap_ref} on {} with no via between", side.name()),
                    )
                });
            }
        }
        if reg.pair == Pair::Close
            && let Some((ls, lg)) = reg.layers
        {
            let pts = along(pin.at(), cap.at(), 0.1);
            if !cx.spend(pts.len().saturating_mul(4)) {
                return;
            }
            let split = [(ls, s.net), (lg, s.ground.unwrap_or(usize::MAX))].into_iter().find_map(|(l, n)| {
                let plane = geo.plane(l)?;
                let on = plane.nets_at(nets, &pts);
                pts.iter().zip(on).find(|(q, x)| *x != Some(n) && matches!(plane.void_at(**q), super::geom::Void::Outside)).map(|(q, _)| (l, *q))
            });
            if let Some((l, at)) = split {
                let mut k = keys.clone();
                k.push("split".into());
                let (items, net_name, cap_ref, pin_ref, regime) =
                    (items.clone(), net_name.clone(), cap_ref.clone(), pin_ref.clone(), reg.words.clone());
                cx.add(DISTANCE, &k, || {
                    Draft::new(at, items, format!("the plane on {} is split between {cap_ref} and {pin_ref} ({net_name})", l.name()))
                        .nets(vec![net_name.clone()])
                        .layer(l)
                        .variant("split")
                        .param("close")
                        .regime(regime)
                        .fix("ecad.board.move", serde_json::Value::Null, format!("move {cap_ref} to {pin_ref}'s side of the split"))
                });
            }
        }
    }
}

// ---- pi.decap_mounting ----

/// How a capacitor pad reaches the plane.
enum Mount {
    /// Its net's pour on its layer joins it, or its net has no plane: no via to look for.
    NoVia,
    /// Its first via: by index, the track run outside the pad (and the via's ring), the via's
    /// gap from the pad, the narrowest track on the way.
    Via { via: usize, run: f64, gap: f64, width: f64 },
    /// No via within reach.
    Missing,
    /// No copper of its net touches it: not routed yet.
    Unrouted,
}

fn mount(cx: &mut Cx, pad_index: usize, search: f64) -> Option<Mount> {
    let (board, nets, geo) = (cx.board, cx.nets, cx.geo);
    let pad = nets.pads.get(pad_index)?;
    let net = pad.net?;
    let layer = side_of(pad);
    // The pour of its net on its layer joins it.
    let in_pour = nets
        .fills
        .iter()
        .any(|f| f.net == Some(net) && f.layer == layer && f.joins.iter().any(|(_, n)| *n == super::super::fill::Node::Pad(pad_index)));
    let planes = geo.layers.iter().any(|l| geo.plane(*l).is_some_and(|pl| pl.fills.iter().any(|(_, n)| *n == net)));
    if in_pour || !planes {
        return Some(Mount::NoVia);
    }
    let area = grow(pad.bbox(), search);
    let g = Copper::build(cx, net, Some(layer), Some(area), false)?;
    let start = g.pad(pad_index)?;
    if g.neighbours(start).is_empty() {
        return Some(Mount::Unrouted);
    }
    let Some((via, _, way)) = g.first_via(cx, start, |_| true, search) else { return Some(Mount::Missing) };
    let v = board.vias.get(via)?;
    let core = pad_core(pad);
    let r = v.diameter / 2.0;
    let mut run = 0.0;
    for w in way.windows(2) {
        let (Some(a), Some(b)) = (w.first().and_then(|n| g.at(cx, *n)), w.get(1).and_then(|n| g.at(cx, *n))) else { continue };
        let len = distance(a, b);
        if len <= 1e-9 {
            continue;
        }
        let pts = along(a, b, 0.02);
        let out = pts.iter().filter(|p| gap(&core, &disc(**p, 0.0)) > 0.0 && distance(**p, v.at) > r).count();
        run += len * out as f64 / pts.len().max(1) as f64;
    }
    let via_gap = gap(&core, &disc(v.at, r)).max(0.0);
    Some(Mount::Via { via, run, gap: via_gap, width: g.narrowest(&way).unwrap_or(0.2) })
}

/// μ0/2π in nH/mm: a via's partial inductance is about 0.2·h·(ln(4h/d) + 1) nH (h, d in mm).
const MU0_2PI: f64 = 0.2;
/// μ0 in nH/mm.
const MU0: f64 = 1.256_637;

fn mounting(cx: &mut Cx, caps: &[Served]) {
    let (board, nets, geo) = (cx.board, cx.nets, cx.geo);
    let p = |name: &str| super::param_value(board, MOUNTING, name);
    let (trace, via_gap, search, bulk_uf) = (p("trace"), p("via_gap"), p("search"), p("bulk_uf"));
    let close_h = super::param_value(board, DISTANCE, "close_h");
    // Each capacitor once, with the regime of its first pin.
    let mut done: BTreeSet<&str> = BTreeSet::new();
    let mut first_vias: BTreeMap<usize, Vec<(String, u64, String)>> = BTreeMap::new();
    for s in caps {
        if !done.insert(s.record.part.as_str()) {
            continue;
        }
        if s.farads.is_some_and(|f| f >= bulk_uf * 1e-6 - 1e-12) {
            continue;
        }
        let Some((cap, gnd, pin)) = s.pads(nets) else { continue };
        let side = side_of(cap);
        let reg = regime(geo, nets, &[pin.at(), cap.at()], side_of(pin), s.net, s.ground, close_h);
        let mut mounts = Vec::new();
        for (i, pad) in [(s.supply_pad, cap), (s.ground_pad, gnd)] {
            match mount(cx, i, search) {
                Some(m) => mounts.push((i, pad, m)),
                None if cx.spent() => return,
                None => {}
            }
        }
        // The estimate: each pad's track over its nearest plane, each via down to its net's plane.
        let plane_depth = |n: Option<usize>| -> Option<f64> {
            let n = n?;
            geo.layers
                .iter()
                .filter(|l| geo.plane(**l).is_some_and(|pl| pl.fills.iter().any(|(_, x)| *x == n)))
                .map(|l| geo.height(side, *l))
                .filter(|h| *h > 0.0)
                .min_by(f64::total_cmp)
        };
        let h_ref = geo.neighbours(side).iter().map(|(_, h)| *h).filter(|h| *h > 0.0).min_by(f64::total_cmp).unwrap_or(0.0);
        let mut nh = 0.0;
        for (_, pad, m) in &mounts {
            if let Mount::Via { via, run, width, .. } = m {
                nh += MU0 * h_ref * run / width.max(0.05);
                if let (Some(v), Some(h)) = (board.vias.get(*via), plane_depth(pad.net)) {
                    nh += MU0_2PI * h * ((4.0 * h / v.diameter.max(0.05)).ln() + 1.0).max(0.0);
                }
            }
        }
        let net_names = vec![nets.name(s.net).unwrap_or("?").to_string(), s.ground.and_then(|g| nets.name(g)).unwrap_or("?").to_string()];
        let part = s.record.part.clone();
        let says_nh = format!("about {} nH of mounting (Zeeff measured 0.61–15.7 nH)", num((nh * 100.0).round() / 100.0));
        for (_, pad, m) in &mounts {
            match m {
                Mount::Via { via, run, gap, .. } => {
                    let Some(v) = board.vias.get(*via) else { continue };
                    first_vias.entry(*via).or_default().push((part.clone(), pad.footprint, pad.reference()));
                    let (variant, measured, limit, param) = if *run > trace + 1e-9 {
                        ("trace", *run, trace, "trace")
                    } else if *gap > via_gap + 1e-9 {
                        ("far", *gap, via_gap, "via_gap")
                    } else {
                        continue;
                    };
                    let pad_ref = pad.reference();
                    let items = vec![pad_item(pad), DrcItem::Via(v.id)];
                    let keys = vec![pad_item(pad).key(), variant.to_string()];
                    let what = if variant == "trace" {
                        format!("{pad_ref} runs {} mm of track to its first via (via {})", num(*run), v.id)
                    } else {
                        format!("{pad_ref}'s first via (via {}) is {} mm from the pad", v.id, num(*gap))
                    };
                    let (says_nh, nets_v, regime) = (says_nh.clone(), net_names.clone(), reg.words.clone());
                    cx.add(MOUNTING, &keys, || {
                        Draft::new(pad.at(), items, format!("{what}; {says_nh}"))
                            .layer(side)
                            .nets(nets_v)
                            .measure(measured, limit, "mm", Bound::Max)
                            .param(param)
                            .variant(variant)
                            .arg("nh", num((nh * 100.0).round() / 100.0))
                            .regime(regime)
                            .fix(
                                "ecad.board.via",
                                serde_json::Value::Null,
                                format!("put a via right beside {pad_ref}, within {} mm, with no longer track", num(trace)),
                            )
                    });
                }
                Mount::Missing => {
                    let pad_ref = pad.reference();
                    let net = pad.net.and_then(|n| nets.name(n)).unwrap_or("?").to_string();
                    let regime = reg.words.clone();
                    cx.add(MOUNTING, &[pad_item(pad).key(), "none".to_string()], || {
                        Draft::new(
                            pad.at(),
                            vec![pad_item(pad)],
                            format!("{pad_ref} reaches no {net} via within {} mm along its copper", num(search)),
                        )
                        .layer(side)
                        .nets(vec![net.clone()])
                        .variant("none")
                        .param("search")
                        .arg("reach", num(search))
                        .regime(regime)
                        .fix("ecad.board.via", serde_json::Value::Null, format!("put a {net} via right beside {pad_ref}"))
                    });
                }
                Mount::Unrouted => cx.not_applicable(MOUNTING, format!("geometry: {} routed", pad.reference())),
                Mount::NoVia => {}
            }
        }
        // Both vias beyond the part's far ends.
        if let [(_, a, Mount::Via { via: va, .. }), (_, b, Mount::Via { via: vb, .. })] = mounts.as_slice()
            && let (Some(x), Some(y)) = (board.vias.get(*va), board.vias.get(*vb))
        {
            let (pa, pb) = (a.at(), b.at());
            let len = distance(pa, pb);
            if len > 1e-9 {
                let u = [(pb[0] - pa[0]) / len, (pb[1] - pa[1]) / len];
                let half = |p: &BoardPad| {
                    let [[x0, y0], [x1, y1]] = p.bbox();
                    (u[0].abs() * (x1 - x0) + u[1].abs() * (y1 - y0)) / 2.0
                };
                let t = |q: [f64; 2]| (q[0] - pa[0]) * u[0] + (q[1] - pa[1]) * u[1];
                if t(x.at) < -half(a) && t(y.at) > len + half(b) {
                    let items = vec![pad_item(a), pad_item(b), DrcItem::Via(x.id), DrcItem::Via(y.id)];
                    let (says_nh, nets_v, regime, part) = (says_nh.clone(), net_names.clone(), reg.words.clone(), part.clone());
                    cx.add(MOUNTING, &[pad_item(a).key(), pad_item(b).key(), "ends".into()], || {
                        Draft::new(
                            a.at(),
                            items,
                            format!(
                                "{part}'s vias sit beyond both ends of the part (vias {} and {}): the loop is as long as it can be; {says_nh}",
                                x.id, y.id
                            ),
                        )
                        .layer(side)
                        .nets(nets_v)
                        .variant("ends")
                        .param("")
                        .arg("part", part.clone())
                        .regime(regime)
                        .fix(
                            "ecad.board.via",
                            serde_json::Value::Null,
                            format!("put {part}'s vias beside its pads, on the long sides, close together"),
                        )
                    });
                }
            }
        }
        if reg.pair != Pair::Close {
            // Wide or no planes: a via two capacitors share is fine (pi.decap_topology).
            for (_, _, m) in &mounts {
                if let Mount::Via { via, .. } = m
                    && let Some(list) = first_vias.get_mut(via)
                {
                    list.retain(|(p, _, _)| *p != part);
                }
            }
        }
    }
    for (via, users) in first_vias {
        let parts: BTreeSet<&str> = users.iter().map(|(p, _, _)| p.as_str()).collect();
        if parts.len() < 2 {
            continue;
        }
        let Some(v) = board.vias.get(via) else { continue };
        let names: Vec<&str> = parts.iter().copied().collect();
        let mut keys = vec![DrcItem::Via(v.id).key(), "shared".into()];
        keys.extend(names.iter().map(|n| n.to_string()));
        let list = names.join(", ");
        let net = nets.via_nets.get(via).copied().flatten().and_then(|n| nets.name(n)).unwrap_or("?").to_string();
        cx.add(MOUNTING, &keys, || {
            Draft::new(v.at, vec![DrcItem::Via(v.id)], format!("via {} ({net}) is the first via of {} capacitors: {list}", v.id, parts.len()))
                .nets(vec![net.clone()])
                .variant("shared")
                .param("")
                .arg("parts", list.clone())
                .arg("count", parts.len().to_string())
                .regime("close planes: each capacitor's own vias count")
                .fix("ecad.board.via", serde_json::Value::Null, "give each capacitor its own via beside its pad")
        });
    }
}

// ---- pi.decap_topology ----

fn topology(cx: &mut Cx, caps: &[Served]) {
    let (board, nets, geo) = (cx.board, cx.nets, cx.geo);
    let close_h = super::param_value(board, DISTANCE, "close_h");
    let (run_max, search) = (super::param_value(board, TOPOLOGY, "run"), super::param_value(board, MOUNTING, "search"));
    for s in caps {
        if s.bulk() {
            continue;
        }
        let Some((cap, gnd, pin)) = s.pads(nets) else { continue };
        if !cx.spend(64) {
            return;
        }
        if filtered(cx, s.net).is_some() {
            continue;
        }
        let side = side_of(pin);
        let same_side = side_of(cap) == side;
        let reg = regime(geo, nets, &[pin.at(), cap.at()], side, s.net, s.ground, close_h);
        let net_name = nets.name(s.net).unwrap_or("?").to_string();
        let (cap_ref, pin_ref, part) = (cap.reference(), pin.reference(), s.record.part.clone());
        let items = vec![pad_item(cap), pad_item(pin)];
        let keys = vec![pad_item(cap).key(), pad_item(pin).key()];
        let rf = rf_ic(cx, &s.ic);
        if rf || reg.pair == Pair::Close {
            // Close planes: a digital IC's (one driving fast nets) pins want their own vias.
            let digital = if rf { None } else { fast_ic(cx, &ic_pads(nets, &s.ic)) };
            if !same_side || (!rf && digital.is_none()) {
                continue;
            }
            // The pin's way to a via, with and without the capacitor's pad.
            let area = grow(around(cap, pin, 0.0), search);
            let Some(g) = Copper::build(cx, s.net, Some(side), Some(area), true) else { return };
            let (Some(np), Some(_)) = (g.pad(s.pin), g.pad(s.supply_pad)) else { continue };
            let cap_copper = g.on_pad(cx, s.supply_pad);
            let own = g.first_via(cx, np, |n| !cap_copper.contains(&n), search);
            let any = g.first_via(cx, np, |_| true, search);
            let (variant, message, says, regime) = if rf {
                let Some((via, _, _)) = own else { continue };
                let id = board.vias.get(via).map_or(0, |v| v.id);
                (
                    "rf",
                    format!(
                        "RF IC pin {pin_ref} ({net_name}) reaches via {id} without passing {cap_ref}: the vendor reference goes pin → capacitor → via"
                    ),
                    format!("route {pin_ref} into {cap_ref}'s pad and put the via on the capacitor's far side"),
                    format!("RF IC {}", s.ic),
                )
            } else {
                if own.is_some() || any.is_none() {
                    continue;
                }
                (
                    "through",
                    format!("{pin_ref} ({net_name}) reaches its plane only through {cap_ref}'s pad: on close planes the pin gets its own via"),
                    format!("put a {net_name} via right at {pin_ref}, and give {cap_ref} its own"),
                    format!("{}; digital IC ({})", reg.words, digital.unwrap_or_default()),
                )
            };
            let mut k = keys.clone();
            k.push(variant.into());
            let items = items.clone();
            cx.add(TOPOLOGY, &k, || {
                Draft::new(pin.at(), items, message).layer(side).nets(vec![net_name.clone()]).variant(variant).param("").regime(regime).fix(
                    "ecad.board.via",
                    serde_json::Value::Null,
                    says,
                )
            });
            continue;
        }
        match reg.pair {
            Pair::Wide | Pair::One { .. } => {
                // The farther plane's net: its pin is where the capacitor belongs.
                let depth = |layers: &[CopperLayer]| layers.iter().map(|l| geo.height(side, *l)).min_by(f64::total_cmp);
                let far_ground = match reg.pair {
                    Pair::One { routed } => Some(routed) == s.ground && s.ground != Some(s.net),
                    _ => depth(&reg.ground_layers).zip(depth(&reg.supply_layers)).is_some_and(|(g, p)| g > p + 1e-9),
                };
                // A far supply plane's pin is the one pi.decap_distance measures.
                let Some(ground) = s.ground.filter(|_| far_ground) else { continue };
                let ic = ic_pads(nets, &s.ic);
                let Some(gpin) = ic
                    .iter()
                    .filter_map(|i| nets.pads.get(*i))
                    .filter(|q| q.net == Some(ground))
                    .min_by(|a, b| distance(a.at(), pin.at()).total_cmp(&distance(b.at(), pin.at())))
                else {
                    continue;
                };
                let p = |name: &str| super::param_value(board, DISTANCE, name);
                let fast = fast_ic(cx, &ic);
                let (limit, which) = if fast.is_some() { (p("fast"), "fast IC") } else { (p("slow"), "IC with no fast nets") };
                let d = pad_gap(gnd, gpin);
                if d <= limit + 1e-9 {
                    continue;
                }
                let (g_ref, gp_ref) = (gnd.reference(), gpin.reference());
                let g_name = nets.name(ground).unwrap_or("?").to_string();
                let items = vec![pad_item(gnd), pad_item(gpin)];
                let regime = format!("{}; {which}", reg.words);
                cx.add(TOPOLOGY, &[pad_item(gnd).key(), pad_item(gpin).key(), "far".into()], || {
                    Draft::new(
                        gnd.at(),
                        items,
                        format!(
                            "{g_ref} is {} mm from {gp_ref}, the IC's pin on the farther plane ({g_name}); {which}s allow {} mm",
                            num(d),
                            num(limit)
                        ),
                    )
                    .nets(vec![g_name.clone()])
                    .measure(d, limit, "mm", Bound::Max)
                    .param("")
                    .variant("far")
                    .regime(regime)
                    .fix("ecad.board.move", serde_json::Value::Null, format!("move {part} so its {g_name} pad sits at {gp_ref}"))
                });
            }
            Pair::None => {
                // The supply's copper on every layer: the capacitor between its source and the pin.
                let Some(g) = Copper::build(cx, s.net, None, None, true) else { return };
                let (Some(np), Some(nc)) = (g.pad(s.pin), g.pad(s.supply_pad)) else { continue };
                let cap_copper = g.on_pad(cx, s.supply_pad);
                let (dist, _) = g.walk(cx, nc, |_| true, f64::INFINITY);
                let run = dist.get(np).copied().unwrap_or(f64::INFINITY);
                if !run.is_finite() {
                    continue;
                }
                let sources: Vec<usize> = source_pads(cx, s.net, &s.ic, &s.record.part);
                if sources.is_empty() {
                    cx.not_applicable(TOPOLOGY, format!("intent: the source of {net_name} (a power output, regulator or connector on it)"));
                } else {
                    let (around_cap, _) = g.walk(cx, np, |n| !cap_copper.contains(&n), f64::INFINITY);
                    let bypass = sources.iter().filter_map(|i| g.pad(*i)).find(|n| around_cap.get(*n).is_some_and(|d| d.is_finite()));
                    if let Some(n) = bypass {
                        let src = match g.kinds.get(n) {
                            Some(Kind::Pad(i)) => nets.pads.get(*i).map(BoardPad::reference).unwrap_or_default(),
                            _ => String::new(),
                        };
                        let mut k = keys.clone();
                        k.push("between".into());
                        let (items, net_name, regime) = (items.clone(), net_name.clone(), reg.words.clone());
                        let (cap_ref, pin_ref) = (cap_ref.clone(), pin_ref.clone());
                        cx.add(TOPOLOGY, &k, || {
                            Draft::new(pin.at(), items, format!("{net_name} reaches {pin_ref} from {src} without passing {cap_ref}: with no planes, route the supply into the capacitor's pad, then to the pin"))
                                .nets(vec![net_name.clone()])
                                .variant("between")
                                .param("")
                                .arg("source", src.clone())
                                .regime(regime)
                                .fix("ecad.board.track", serde_json::Value::Null, format!("route {net_name} from {src} to {cap_ref}, then on to {pin_ref}"))
                        });
                    }
                }
                if run > run_max + 1e-9 {
                    let mut k = keys.clone();
                    k.push("run".into());
                    let (items, net_name, regime) = (items.clone(), net_name.clone(), reg.words.clone());
                    cx.add(TOPOLOGY, &k, || {
                        Draft::new(
                            cap.at(),
                            items,
                            format!(
                                "{cap_ref} runs {} mm of copper to {pin_ref} ({net_name}); with no planes keep it within {} mm",
                                num(run),
                                num(run_max)
                            ),
                        )
                        .nets(vec![net_name.clone()])
                        .measure(run, run_max, "mm", Bound::Max)
                        .param("run")
                        .variant("run")
                        .regime(regime)
                        .fix(
                            "ecad.board.move",
                            serde_json::Value::Null,
                            format!("move {part} next to {pin_ref} and shorten the track"),
                        )
                    });
                }
            }
            Pair::Close => {}
        }
    }
}

/// The pads a supply comes from: a power output, or a connector's, regulator's, its inductor's
/// or other power IC's pin on it (not the IC served, nor the capacitor).
fn source_pads(cx: &Cx, net: usize, ic: &str, cap: &str) -> Vec<usize> {
    let (nets, roles, conn) = (cx.nets, cx.roles, cx.conn);
    let name = nets.name(net).unwrap_or("");
    let outputs: BTreeSet<(&str, &str)> = conn
        .net(name)
        .map(|n| {
            n.pins
                .iter()
                .filter(|q| q.electrical == super::super::symbol::PinType::PowerOutput)
                .map(|q| (q.designator.as_str(), q.pin.as_str()))
                .collect()
        })
        .unwrap_or_default();
    nets.pads
        .iter()
        .enumerate()
        .filter(|(_, p)| p.net == Some(net) && p.designator != ic && p.designator != cap)
        .filter(|(_, p)| {
            outputs.contains(&(p.designator.as_str(), p.pad.number.as_str()))
                || roles.intent.part(&p.designator).is_some_and(|r| {
                    matches!(r.role, PartRole::Connector | PartRole::SmpsIc | PartRole::SmpsInductor | PartRole::PowerIc | PartRole::Module)
                })
                || p.designator.starts_with('J')
        })
        .map(|(i, _)| i)
        .collect()
}
