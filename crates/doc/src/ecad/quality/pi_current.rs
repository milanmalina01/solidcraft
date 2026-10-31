//! Current capacity: whether the copper a supply's current runs through is wide enough for it.
//! - `pi.current`: each track, thermal-spoke set (a pad joined to a pour through a relief) and
//!   cluster of vias on the net's current path, by the rise IPC-2152's external fit (Brooks and
//!   Adam 2015, see [`super::super::current`]) gives it at the net's current, against the rise
//!   allowed (the net class's `temp_rise`, else `rise`): a warning above it, an error above
//!   `error_factor` times it. A via cluster's barrels count as a track as wide as their
//!   circumferences, `plating` thick, at each step between two neighbouring layers the worst
//!   (stacked vias each carry all of the current; Brooks and Adam found a via follows the tracks
//!   it joins, so this is a rough bound: its findings are warnings at most), as does copper
//!   outside the thicknesses the fit was checked on.
//! - `pi.bottleneck`: the narrow places inside a pour on the current path (a neck, a slot, the
//!   webs between a row of antipads), which no track width shows: the narrowest cross-section of
//!   the pour's island between copper that joins it toward the supply and toward a load, against
//!   the width the fit asks for at the allowed rise on the pour's copper.
//!
//! **The current** is the net's intent `current_a` (the most it carries, read as the continuous
//! or RMS current), else its net class's `current`; a supply with neither is not applicable.
//!
//! **The current path** is the net's copper as a graph ([`Copper`]: pads, tracks, vias, pour
//! islands and spoke sets, joined where they touch), less the branches that lead only to pads
//! no supply current flows into: capacitors, TVS parts, test points, an IC's signal pins, and
//! two-pad parts whose other pad is on a net that is no supply (a pull-up, a divider, an LED).
//! Where the current comes from is the net's power-output pins, else its connectors, else the
//! parts in line with it (an inductor, a ferrite, a sense resistor, a diode).
//!
//! **How the current divides** ([`Flow`]): the path is solved as a network of resistances
//! (tracks by length over width and thickness, a via's barrel by its plating, spoke sets, and a
//! pour island as a sheet each contact spreads into from a quarter-millimetre ring, which
//! overstates the sheet's resistance and so the share of copper beside it), with the sources at
//! one voltage, once for each load drawing all of the current. A piece whose share is the same
//! whichever load draws (the trunk, or its part of copper running in parallel on the trunk) is
//! checked at that share: parallel paths split by conductance. A piece whose share depends on
//! which loads draw (a branch toward some of them, copper near some of them) is a finding when
//! it is too hot even at the least share it carries whichever loads draw; too hot only at a
//! larger share, it is noted, one info per net at its largest share, for a person to judge or
//! the intent to say, since how much each load draws isn't known. A connected piece of copper with more than [`MAX_FLOW`] points isn't solved: there a
//! piece all of the current passes (taking it away cuts every load off from where the current
//! comes from, or, with no source known, the net's only two current pins apart) is checked at
//! the full current, and any other is noted.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use solidcraft_geom::region::Shape;

use super::super::airwires;
use super::super::checks::{Severity, gap};
use super::super::copper::{pad_core, stroke};
use super::super::current::{FIT2152_OZ, in_range_2152, mm_to_oz, rise_2152, width_2152};
use super::super::drc::{Bound, Class, DrcItem};
use super::super::fill::Node;
use super::super::footprint::Core;
use super::super::intent::{NetRole, PartRole};
use super::super::layers::CopperLayer;
use super::super::path;
use super::super::stackup::{INNER_COPPER, OUTER_COPPER, surroundings};
use super::super::symbol::PinType;
use super::catalog::{Code, Family, Param};
use super::geom::{along, distance, track_pieces};
use super::{Cx, Draft, num};

const CURRENT: &str = "pi.current";
const BOTTLENECK: &str = "pi.bottleneck";

/// Copper's resistivity at 20 °C in Ω·mm (only how the resistances compare matters here).
const RHO: f64 = 1.724e-5;
/// The most points one connected piece of a net's copper is solved on for how its current
/// divides (a dense solve: the work grows as their cube).
const MAX_FLOW: usize = 320;
/// A piece's share of the current is the same wherever the current goes when the least and the
/// most differ by no more than this.
const SPLIT_TOL: f64 = 0.02;
/// The ring round a contact the current spreads into a pour from, in millimetres.
const CONTACT_RADIUS: f64 = 0.25;

const fn param(name: &'static str, default: f64, units: &'static str, min: f64, max: f64, source: &'static str, about: &'static str) -> Param {
    Param { name, default, units, source, min, max, verify: false, about }
}

const CODES: &[Code] = &[
    Code {
        code: CURRENT,
        class: Class::Critical,
        severity: Severity::Error,
        worst: None,
        needs: "geometry; intent: the current_a of each supply (or its net class's current)",
        section: "power-ground#current",
        about: "a track, thermal-spoke set or via cluster on a supply's current path warms by more than the rise allowed at the net's current (IPC-2152 external fit); an error above twice the rise",
        principle: "Copper too narrow for its current runs hot, drops voltage and ages the board; size it from an allowed temperature rise with IPC-2152's data.",
        params: &[
            param(
                "rise",
                20.0,
                "°C",
                1.0,
                200.0,
                "judgment: a common checker default (Brooks and Adam 2015: size from an allowed rise; 10 °C on dense or hot boards)",
                "the temperature rise allowed when the net's class gives none",
            ),
            param("error_factor", 2.0, "ratio", 1.0, 100.0, "judgment", "a rise above this many times the allowed one is an error"),
            param("plating", 0.025, "mm", 0.005, 0.2, "judgment: typical finished barrel plating, 20 to 25 µm", "a via barrel's copper thickness"),
            param("cluster", 1.0, "mm", 0.0, 20.0, "judgment", "vias of the net this close together are one transition"),
            param(
                "short",
                2.0,
                "mm",
                0.0,
                50.0,
                "judgment: the fit is for long tracks; the pads and vias at a short neck's ends draw its heat out",
                "a track shorter than this and than 4 of its widths is a neck: an info, not a rise",
            ),
        ],
    },
    Code {
        code: BOTTLENECK,
        class: Class::Practical,
        severity: Severity::Warning,
        worst: None,
        needs: "geometry and pours; intent: the current_a of each supply (or its net class's current)",
        section: "power-ground#current",
        about: "a pour on a supply's current path narrows (a neck, a slot, the webs between antipads) below the width its current needs at the allowed rise",
        principle: "A path carries what its narrowest copper carries: necks and the webs between antipads in a pour heat and drop voltage although the pour looks wide.",
        params: &[
            param("step", 0.1, "mm", 0.02, 5.0, "judgment", "how far apart the pour's cross-sections are taken"),
            param(
                "end_clear",
                1.0,
                "mm",
                0.0,
                20.0,
                "judgment",
                "no cross-section is taken this close to where the current enters or leaves the pour (a pad's own relief there is pi.current's)",
            ),
        ],
    },
];

pub(super) const FAMILY: Family = Family { module: "pi_current", title: "current capacity", codes: CODES, run };

/// A piece of one net's copper (see [`Copper`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Item {
    /// By index into the connections' pads, and the board's tracks and vias.
    Pad(usize),
    Track(usize),
    Via(usize),
    /// An island of a pour's fill: the fill's index into the connections' fills, and the
    /// island's into its shapes.
    Fill(usize, usize),
    /// The thermal spokes joining a pad (by index) to a fill (by index).
    Spokes(usize, usize),
}

/// One net's copper as a graph: its pieces, and where two of them touch (as the connections
/// join them, see [`airwires::contacts`], and as each pour's islands touch the net's copper).
pub(super) struct Copper {
    pub items: Vec<Item>,
    index: BTreeMap<Item, usize>,
    /// Two items (by index into `items`) that touch, and a point where they do.
    pub links: Vec<(usize, usize, [f64; 2])>,
    /// Each item's links, by index into `links`.
    pub at: Vec<Vec<usize>>,
}

impl Copper {
    fn new() -> Copper {
        Copper { items: Vec::new(), index: BTreeMap::new(), links: Vec::new(), at: Vec::new() }
    }

    fn add(&mut self, item: Item) -> usize {
        if let Some(i) = self.index.get(&item) {
            return *i;
        }
        let i = self.items.len();
        self.items.push(item);
        self.at.push(Vec::new());
        self.index.insert(item, i);
        i
    }

    fn link(&mut self, a: Item, b: Item, at: [f64; 2]) {
        let (a, b) = (self.add(a), self.add(b));
        if a == b {
            return;
        }
        // The connections give a contact from each side: one is enough.
        let known =
            self.at.get(a).into_iter().flatten().filter_map(|l| self.links.get(*l)).any(|(x, y, p)| (*x == b || *y == b) && distance(*p, at) < 1e-6);
        if known {
            return;
        }
        let l = self.links.len();
        self.links.push((a, b, at));
        for i in [a, b] {
            if let Some(list) = self.at.get_mut(i) {
                list.push(l);
            }
        }
    }

    pub fn find(&self, item: Item) -> Option<usize> {
        self.index.get(&item).copied()
    }

    /// The other end of link `l` from item `i`.
    pub fn other(&self, l: usize, i: usize) -> Option<usize> {
        let (a, b, _) = self.links.get(l)?;
        if *a == i { Some(*b) } else { Some(*a) }
    }

    /// The items joined to item `i`.
    pub fn neighbours(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        self.at.get(i).into_iter().flatten().filter_map(move |l| self.other(*l, i))
    }

    /// Which items the items `from` reach through the items `open` lets through (the `from` ones
    /// themselves included when open).
    pub fn reach(&self, from: &[usize], open: &dyn Fn(usize) -> bool) -> Vec<bool> {
        let mut seen = vec![false; self.items.len()];
        let mut stack: Vec<usize> = Vec::new();
        for i in from.iter().copied().filter(|i| open(*i)) {
            if let Some(slot) = seen.get_mut(i)
                && !*slot
            {
                *slot = true;
                stack.push(i);
            }
        }
        while let Some(i) = stack.pop() {
            for j in self.neighbours(i) {
                if !open(j) {
                    continue;
                }
                if let Some(slot) = seen.get_mut(j)
                    && !*slot
                {
                    *slot = true;
                    stack.push(j);
                }
            }
        }
        seen
    }
}

/// The copper of each of the nets `which` (board net indexes) on the board; `None` once the
/// family's budget is spent.
pub(super) fn copper(cx: &mut Cx, which: &BTreeSet<usize>) -> Option<BTreeMap<usize, Copper>> {
    let (board, nets) = (cx.board, cx.nets);
    let (np, nt, nv) = (nets.pads.len(), board.tracks.len(), board.vias.len());
    if !cx.spend(np.saturating_add(nt).saturating_add(nv).saturating_mul(16)) {
        return None;
    }
    let mut out: BTreeMap<usize, Copper> = which.iter().map(|n| (*n, Copper::new())).collect();
    for (i, p) in nets.pads.iter().enumerate() {
        if let Some(cu) = p.net.filter(|_| p.top || p.bottom).and_then(|n| out.get_mut(&n)) {
            cu.add(Item::Pad(i));
        }
    }
    for (t, n) in nets.track_nets.iter().enumerate() {
        if let Some(cu) = n.and_then(|n| out.get_mut(&n)) {
            cu.add(Item::Track(t));
        }
    }
    for (v, n) in nets.via_nets.iter().enumerate() {
        if let Some(cu) = n.and_then(|n| out.get_mut(&n)) {
            cu.add(Item::Via(v));
        }
    }
    // The nodes as the connections number them: the pads, then the tracks, then the vias.
    let node = |k: usize| -> Option<(usize, Item)> {
        match k.checked_sub(np) {
            None => nets.pads.get(k)?.net.map(|n| (n, Item::Pad(k))),
            Some(t) if t < nt => nets.track_nets.get(t).copied().flatten().map(|n| (n, Item::Track(t))),
            Some(t) => {
                let v = t.checked_sub(nt)?;
                nets.via_nets.get(v).copied().flatten().map(|n| (n, Item::Via(v)))
            }
        }
    };
    for (a, b, at) in airwires::contacts(board, &nets.pads) {
        let (Some((na, ia)), Some((nb, ib))) = (node(a), node(b)) else { continue };
        if na != nb {
            continue;
        }
        if let Some(cu) = out.get_mut(&na) {
            cu.link(ia, ib, at);
        }
    }
    for (f, fill) in nets.fills.iter().enumerate() {
        let Some(n) = fill.net.filter(|n| which.contains(n)) else { continue };
        let shapes = fill.region.shapes();
        if !cx.spend(shapes.iter().map(Shape::corners).fold(fill.joins.len(), usize::saturating_add)) {
            return None;
        }
        let Some(cu) = out.get_mut(&n) else { continue };
        for s in 0..shapes.len() {
            cu.add(Item::Fill(f, s));
        }
        for (s, node) in &fill.joins {
            let island = Item::Fill(f, *s);
            match node {
                Node::Pad(i) => {
                    let Some(p) = nets.pads.get(*i).filter(|p| p.net == Some(n)) else { continue };
                    let relieved = fill.reliefs.iter().any(|r| r.pad == *i && !r.solid && r.connected > 0);
                    if relieved {
                        cu.link(Item::Pad(*i), Item::Spokes(*i, f), p.at());
                        cu.link(Item::Spokes(*i, f), island, p.at());
                    } else {
                        cu.link(Item::Pad(*i), island, p.at());
                    }
                }
                Node::Track(t) => {
                    let Some(track) = board.tracks.get(*t).filter(|_| nets.track_nets.get(*t).copied().flatten() == Some(n)) else { continue };
                    // Where the track is on the island: the first of its points 0.5 mm apart that is.
                    let pts: Vec<[f64; 2]> =
                        track_pieces(&track.points, &track.curves).into_iter().flat_map(|(a, b)| along(a, b, 0.5)).take(4096).collect();
                    let on = fill.region.locate(&pts);
                    let at = pts.iter().zip(&on).find(|(_, k)| **k == Some(*s)).map(|(p, _)| *p).or_else(|| track.points.first().copied());
                    if let Some(at) = at {
                        cu.link(Item::Track(*t), island, at);
                    }
                }
                Node::Via(v) => {
                    if let Some(via) = board.vias.get(*v).filter(|_| nets.via_nets.get(*v).copied().flatten() == Some(n)) {
                        cu.link(Item::Via(*v), island, via.at);
                    }
                }
                Node::Fill(pour, k) => {
                    let other = nets.fills.iter().position(|g| g.pour == *pour && g.layer == fill.layer && g.net == Some(n));
                    let at = shapes
                        .get(*s)
                        .and_then(|sh| super::super::bbox(sh.outer.iter().copied()))
                        .map(|[[a, b], [c, d]]| [(a + c) / 2.0, (b + d) / 2.0]);
                    if let (Some(g), Some(at)) = (other, at) {
                        cu.link(island, Item::Fill(g, *k), at);
                    }
                }
            }
        }
    }
    Some(out)
}

/// Which pieces of supply nets carry current on to a load, for the checks that judge copper by
/// the current it carries (a crystal's keepout): each net's copper and current path, made the
/// first time the net is asked.
#[derive(Default)]
pub(super) struct Feeds {
    nets: BTreeMap<usize, Option<(Copper, Path)>>,
}

impl Feeds {
    /// Whether the track or via `item` (by board index) of the supply `net` carries current to a
    /// load other than the pads of footprint `except`: taking it away cuts such a load off from
    /// where the current comes from. A pour's island does (the trunk); a pad, or a branch that
    /// leads to no load, doesn't; with no source known, it may. `None` past the budget.
    pub(super) fn reach_a_load(&mut self, cx: &mut Cx, net: usize, item: Item, except: Option<u64>) -> Option<bool> {
        match item {
            Item::Fill(..) => return Some(true),
            Item::Pad(_) | Item::Spokes(..) => return Some(false),
            Item::Track(_) | Item::Via(_) => {}
        }
        if let std::collections::btree_map::Entry::Vacant(slot) = self.nets.entry(net) {
            let cu = copper(cx, &BTreeSet::from([net]))?.remove(&net);
            slot.insert(cu.map(|cu| {
                let pins = Pins::new(cx);
                let path = Path::new(cx, &pins, &cu);
                (cu, path)
            }));
        }
        let Some((cu, path)) = self.nets.get(&net).and_then(Option::as_ref) else { return Some(true) };
        let Some(k) = cu.find(item) else { return Some(true) };
        if !path.on(k) {
            return Some(false);
        }
        let terms = path.terminals_with(k);
        let sources: Vec<usize> = terms.iter().copied().filter(|t| path.sources.contains(t)).collect();
        if sources.is_empty() {
            return Some(true);
        }
        let theirs = |t: usize| match cu.items.get(t) {
            Some(Item::Pad(i)) => except.is_some() && cx.nets.pads.get(*i).map(|p| p.footprint) == except,
            _ => false,
        };
        let loads: Vec<usize> = terms.iter().copied().filter(|t| !path.sources.contains(t) && !theirs(*t)).collect();
        if !cx.spend(cu.items.len().saturating_add(cu.links.len()).saturating_mul(2)) {
            return None;
        }
        let with = cu.reach(&sources, &|j| path.on(j));
        let without = cu.reach(&sources, &|j| path.on(j) && j != k);
        Some(loads.iter().any(|l| with.get(*l).copied().unwrap_or(false) && !without.get(*l).copied().unwrap_or(true)))
    }
}

/// How far along a track's pieces `pieces` the point nearest `q` is.
pub(super) fn position(pieces: &[([f64; 2], [f64; 2])], q: [f64; 2]) -> f64 {
    let mut walked = 0.0;
    let mut best = (f64::INFINITY, 0.0);
    for (a, b) in pieces {
        let (p, t) = super::super::path::project(q, *a, *b, 0.0);
        let d = distance(p, q);
        let len = distance(*a, *b);
        if d < best.0 {
            best = (d, walked + t * len);
        }
        walked += len;
    }
    best.1
}

/// The copper's thickness of `layer`, in millimetres (the stack-up's).
fn copper_of(cx: &Cx, layer: CopperLayer) -> f64 {
    let fallback = if layer.is_inner() { INNER_COPPER } else { OUTER_COPPER };
    surroundings(&cx.geo.stackup, layer, cx.board.copper_layers).map_or(fallback, |s| s.copper.thickness)
}

/// A net's current, and where it comes from in words.
struct Supply {
    net: usize,
    name: String,
    amps: f64,
    from: String,
    /// The rise allowed, and whether the net's class gives it (else the code's `rise`).
    rise: f64,
    class_rise: bool,
}

/// The nets with a current, and those that need one (listed as not applicable).
fn supplies(cx: &mut Cx) -> Vec<Supply> {
    let (board, roles) = (cx.board, cx.roles);
    let rise = super::param_value(board, CURRENT, "rise");
    let mut out = Vec::new();
    for info in &roles.nets {
        let class = board.rules.class_of(info.name);
        let own = info.record.and_then(|r| r.current_a).map(|a| (a, "the intent's current_a".to_string()));
        // A switch node carries the inductor's current, at least its regulator's output current:
        // never its net class's default.
        if own.is_none() && info.has_role(NetRole::SwNode) {
            match output_current(cx, info) {
                Some((amps, from)) => {
                    let class_rise = class.and_then(|c| c.temp_rise).filter(|r| r.is_finite() && *r > 0.0);
                    out.push(Supply {
                        net: info.index,
                        name: info.name.to_string(),
                        amps,
                        from,
                        rise: class_rise.unwrap_or(rise),
                        class_rise: class_rise.is_some(),
                    });
                }
                None => {
                    let needs = format!("intent: current_a of {} (the inductor current)", info.name);
                    cx.not_applicable(CURRENT, needs.clone());
                    cx.not_applicable(BOTTLENECK, needs);
                }
            }
            continue;
        }
        let amps = own.or_else(|| class.and_then(|c| c.current.map(|a| (a, format!("net class {}'s current", c.name)))));
        let Some((amps, from)) = amps.filter(|(a, _)| a.is_finite() && *a > 0.0) else {
            if info.is_supply() && !info.has_role(NetRole::Ground) && !roles.grounds.contains(&info.index) {
                let needs = format!("intent: current_a of {}", info.name);
                cx.not_applicable(CURRENT, needs.clone());
                cx.not_applicable(BOTTLENECK, needs);
            }
            continue;
        };
        let class_rise = class.and_then(|c| c.temp_rise).filter(|r| r.is_finite() && *r > 0.0);
        out.push(Supply {
            net: info.index,
            name: info.name.to_string(),
            amps,
            from,
            rise: class_rise.unwrap_or(rise),
            class_rise: class_rise.is_some(),
        });
    }
    out
}

/// The output current of a switch node's regulator (its record's `domain`): the current_a of the
/// net its kept output capacitors serve, else of its power-output pins' nets, with where it
/// comes from in words.
fn output_current(cx: &Cx, info: &super::roles::NetInfo) -> Option<(f64, String)> {
    let (nets, roles) = (cx.nets, cx.roles);
    let ic = info.record.and_then(|r| r.domain.as_deref())?;
    let pins = super::pi_decap::Pins::new(nets, cx.conn);
    let mut outs: Vec<usize> = roles
        .parts(PartRole::SmpsOutCap)
        .flat_map(|p| p.of.iter())
        .filter(|pin| pin.split_once('.').is_some_and(|(d, _)| d == ic))
        .flat_map(|pin| pins.resolve(nets, pin))
        .filter_map(|i| nets.pads.get(i)?.net)
        .collect();
    if outs.is_empty() {
        outs = cx
            .conn
            .nets
            .iter()
            .filter(|n| n.pins.iter().any(|q| q.designator == ic && q.electrical == PinType::PowerOutput))
            .filter_map(|n| nets.names.iter().position(|x| *x == n.name))
            .collect();
    }
    let (net, amps) = outs.into_iter().filter_map(|n| Some((n, roles.net(n)?.record?.current_a?))).find(|(_, a)| a.is_finite() && *a > 0.0)?;
    let name = nets.name(net).unwrap_or("?");
    Some((amps, format!("{ic}'s output {name}'s current_a: the inductor carries at least that")))
}

/// Whether `track` widened to `width` keeps the clearance to other nets' pads and tracks on its
/// layer; `None` past the budget.
fn widens(cx: &mut Cx, track: usize, net: usize, width: f64) -> Option<bool> {
    let (board, nets) = (cx.board, cx.nets);
    let t = board.tracks.get(track)?;
    if !cx.spend(nets.pads.len().saturating_add(board.tracks.len()).saturating_add(1)) {
        return None;
    }
    let clearance = board.rules.class_of(nets.name(net).unwrap_or("")).map_or(board.rules.clearance, |c| c.clearance.max(board.rules.clearance));
    let pieces: Vec<Core> = track_pieces(&t.points, &t.curves).into_iter().map(|(a, b)| stroke(a, b, width / 2.0)).collect();
    let Some(reach) = path::bbox(&t.points, &t.curves, false).map(|b| super::super::grid::grow(b, width + clearance)) else { return Some(true) };
    let near = |b: [[f64; 2]; 2]| super::super::grid::boxes_meet(b, reach);
    for p in nets.pads.iter().filter(|p| p.net != Some(net) && p.on(t.layer) && near(p.bbox())) {
        let core = pad_core(p);
        if pieces.iter().any(|c| gap(c, &core) < clearance - 1e-6) {
            return Some(false);
        }
    }
    for (k, o) in board.tracks.iter().enumerate() {
        if o.layer != t.layer || nets.track_nets.get(k).copied().flatten() == Some(net) || !path::bbox(&o.points, &o.curves, false).is_some_and(near)
        {
            continue;
        }
        for (a, b) in track_pieces(&o.points, &o.curves) {
            let core = stroke(a, b, o.width / 2.0);
            if pieces.iter().any(|c| gap(c, &core) < clearance - 1e-6) {
                return Some(false);
            }
        }
    }
    Some(true)
}

/// What the current path knows of each pad (see the module notes).
struct Pins<'a> {
    /// Each pad's pin's electrical type, by footprint id and pad number.
    types: BTreeMap<(u64, &'a str), PinType>,
    /// The intent's roles of each part, by designator.
    parts: BTreeMap<&'a str, Vec<PartRole>>,
    /// The copper pads of each footprint, by index into the connections' pads.
    of_footprint: BTreeMap<u64, Vec<usize>>,
}

impl<'a> Pins<'a> {
    fn new(cx: &Cx<'a>) -> Pins<'a> {
        let (ecad, board, nets) = (cx.ecad, cx.board, cx.nets);
        let mut types = BTreeMap::new();
        for f in &board.footprints {
            let Some(placed) = ecad.placed(f) else { continue };
            for (pin, pads) in placed.resolved.connections() {
                let Some(ty) = placed.resolved.symbol.pin(pin).map(|p| p.electrical) else { continue };
                for pad in pads {
                    types.insert((f.id, pad), ty);
                }
            }
        }
        let mut parts: BTreeMap<&'a str, Vec<PartRole>> = BTreeMap::new();
        for p in &cx.roles.intent.parts {
            parts.entry(p.part.as_str()).or_default().push(p.role);
        }
        let mut of_footprint: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
        for (i, p) in nets.pads.iter().enumerate().filter(|(_, p)| p.top || p.bottom) {
            of_footprint.entry(p.footprint).or_default().push(i);
        }
        Pins { types, parts, of_footprint }
    }

    fn roles(&self, designator: &str) -> &[PartRole] {
        self.parts.get(designator).map_or(&[], Vec::as_slice)
    }
}

/// The letters a designator starts with, upper case.
fn prefix(designator: &str) -> String {
    designator.chars().take_while(char::is_ascii_alphabetic).collect::<String>().to_ascii_uppercase()
}

/// Whether a net carries supply current: a power or switch-node role, a current, or supply pins.
fn powered(cx: &Cx, net: usize) -> bool {
    cx.roles.net(net).is_some_and(|i| {
        !cx.roles.grounds.contains(&net)
            && (matches!(i.role(), Some(NetRole::Power | NetRole::SwNode)) || i.record.is_some_and(|r| r.current_a.is_some()) || i.supply_pins)
    })
}

/// A pad's part in the current: none, a load, or where it comes from (best first).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Role {
    None,
    Load,
    /// In line with the net: an inductor, a ferrite, a sense resistor, a diode.
    InLine,
    Connector,
    /// A power-output pin.
    Output,
}

fn pad_role(cx: &Cx, pins: &Pins, i: usize) -> Role {
    let Some(p) = cx.nets.pads.get(i) else { return Role::None };
    let roles = pins.roles(&p.designator);
    let quiet = [
        PartRole::Decoupling,
        PartRole::Bulk,
        PartRole::SmpsInCap,
        PartRole::SmpsOutCap,
        PartRole::LoadCap,
        PartRole::Tvs,
        PartRole::EsdArray,
        PartRole::TestPoint,
        PartRole::Fiducial,
        PartRole::Mounting,
    ];
    if roles.iter().any(|r| quiet.contains(r)) {
        return Role::None;
    }
    if roles.iter().any(|r| matches!(r, PartRole::SmpsInductor | PartRole::SmpsDiode | PartRole::Ferrite | PartRole::SenseResistor)) {
        return Role::InLine;
    }
    let pre = prefix(&p.designator);
    let connector = roles.contains(&PartRole::Connector) || matches!(pre.as_str(), "J" | "P" | "CN" | "CON");
    match pins.types.get(&(p.footprint, p.pad.number.as_str())) {
        Some(PinType::PowerOutput) => return Role::Output,
        Some(PinType::PowerInput) => return Role::Load,
        Some(
            PinType::Input
            | PinType::Output
            | PinType::Bidirectional
            | PinType::TriState
            | PinType::OpenCollector
            | PinType::OpenEmitter
            | PinType::NotConnected,
        ) => return Role::None,
        _ => {}
    }
    if connector {
        return Role::Connector;
    }
    let own = pins.of_footprint.get(&p.footprint).map_or(&[][..], Vec::as_slice);
    if own.len() != 2 {
        return if roles.contains(&PartRole::Module) || own.len() > 2 { Role::Load } else { Role::None };
    }
    if pre == "C" {
        return Role::None;
    }
    // A two-pad part in line with the supply, or one hanging off it (a pull-up, an LED).
    let other = own.iter().copied().find(|k| *k != i).and_then(|k| cx.nets.pads.get(k)).and_then(|q| q.net);
    if other.is_some_and(|n| powered(cx, n)) { Role::InLine } else { Role::None }
}

/// The net's current path (see the module notes).
struct Path {
    /// Each item is on it (not pruned away).
    alive: Vec<bool>,
    /// Its component, among the items on it.
    comp: Vec<usize>,
    /// The pads the current enters or leaves by, and those it comes from.
    terminals: Vec<usize>,
    sources: BTreeSet<usize>,
}

impl Path {
    fn new(cx: &Cx, pins: &Pins, cu: &Copper) -> Path {
        let n = cu.items.len();
        let mut role = vec![Role::None; n];
        for (k, item) in cu.items.iter().enumerate() {
            if let (Item::Pad(i), Some(slot)) = (item, role.get_mut(k)) {
                *slot = pad_role(cx, pins, *i);
            }
        }
        let terminal = |k: usize| role.get(k).is_some_and(|r| *r != Role::None);
        // Branches that end in no terminal carry nothing: cut them back, leaf by leaf.
        let mut alive = vec![true; n];
        let mut degree: Vec<usize> = (0..n).map(|k| cu.neighbours(k).filter(|j| *j != k).collect::<BTreeSet<_>>().len()).collect();
        let mut leaves: Vec<usize> = (0..n).filter(|k| !terminal(*k) && degree.get(*k).is_some_and(|d| *d <= 1)).collect();
        while let Some(k) = leaves.pop() {
            if !alive.get(k).copied().unwrap_or(false) {
                continue;
            }
            if let Some(slot) = alive.get_mut(k) {
                *slot = false;
            }
            let near: BTreeSet<usize> = cu.neighbours(k).filter(|j| *j != k).collect();
            for j in near {
                if !alive.get(j).copied().unwrap_or(false) {
                    continue;
                }
                if let Some(d) = degree.get_mut(j) {
                    *d = d.saturating_sub(1);
                    if *d <= 1 && !terminal(j) {
                        leaves.push(j);
                    }
                }
            }
        }
        let mut comp = vec![usize::MAX; n];
        let mut next = 0usize;
        for k in 0..n {
            if !alive.get(k).copied().unwrap_or(false) || comp.get(k).is_some_and(|c| *c != usize::MAX) {
                continue;
            }
            let seen = cu.reach(&[k], &|j| alive.get(j).copied().unwrap_or(false));
            for (slot, s) in comp.iter_mut().zip(&seen) {
                if *s {
                    *slot = next;
                }
            }
            next += 1;
        }
        let terminals: Vec<usize> = (0..n).filter(|k| terminal(*k) && alive.get(*k).copied().unwrap_or(false)).collect();
        // Where the current comes from: the best kind there is, per component.
        let mut sources = BTreeSet::new();
        let mut best: BTreeMap<usize, Role> = BTreeMap::new();
        for k in &terminals {
            let (Some(c), Some(r)) = (comp.get(*k), role.get(*k)) else { continue };
            let slot = best.entry(*c).or_insert(Role::Load);
            *slot = (*slot).max(*r);
        }
        for k in &terminals {
            let (Some(c), Some(r)) = (comp.get(*k), role.get(*k)) else { continue };
            if best.get(c).is_some_and(|b| *b > Role::Load && b == r) {
                sources.insert(*k);
            }
        }
        Path { alive, comp, terminals, sources }
    }

    fn on(&self, k: usize) -> bool {
        self.alive.get(k).copied().unwrap_or(false)
    }

    /// The terminals of item `k`'s component.
    fn terminals_with(&self, k: usize) -> Vec<usize> {
        let Some(c) = self.comp.get(k).filter(|c| **c != usize::MAX) else { return Vec::new() };
        self.terminals.iter().copied().filter(|t| self.comp.get(*t) == Some(c)).collect()
    }

    /// Whether all the current passes the items `cut` (of one component): taking them away cuts
    /// every load off from where the current comes from (with no source known, the component's
    /// only two terminals apart).
    fn carries_all(&self, cu: &Copper, cut: &[usize]) -> bool {
        let Some(first) = cut.first() else { return false };
        let terms = self.terminals_with(*first);
        let (sources, loads): (Vec<usize>, Vec<usize>) = terms.iter().copied().partition(|t| self.sources.contains(t));
        let open = |j: usize| self.on(j) && !cut.contains(&j);
        if !sources.is_empty() && !loads.is_empty() {
            let seen = cu.reach(&sources, &open);
            return !loads.iter().any(|l| seen.get(*l).copied().unwrap_or(false));
        }
        if let [a, b] = terms.as_slice() {
            let seen = cu.reach(&[*a], &open);
            return !seen.get(*b).copied().unwrap_or(false);
        }
        false
    }
}

/// How the net's current divides over its path by conductance (see the module notes): for each
/// item, its share of the current for each load of its connected piece in turn drawing all of
/// it; `None` where that wasn't solved (a piece too large, or no way to tell which way the
/// current goes).
struct Flow {
    share: Vec<Option<Vec<f64>>>,
}

/// A resistance of `ohms` as a conductance, kept finite.
fn conductance(ohms: f64) -> f64 {
    if ohms.is_finite() && ohms > 1e-9 { 1.0 / ohms } else { 1e9 }
}

/// The entry at row `i`, column `j` of the `m`-wide matrix `g` (row by row).
fn at(g: &[f64], m: usize, i: usize, j: usize) -> f64 {
    g.get(i * m + j).copied().unwrap_or(0.0)
}

fn add(g: &mut [f64], m: usize, i: usize, j: usize, v: f64) {
    if let Some(x) = g.get_mut(i * m + j) {
        *x += v;
    }
}

/// The Cholesky factor of the symmetric `m`×`m` matrix `g`, in place: its lower triangle
/// becomes L with L·Lᵀ = g. `false` when `g` isn't positive definite (as far as the numbers
/// tell).
fn cholesky(g: &mut [f64], m: usize) -> bool {
    let scale = (0..m).map(|i| at(g, m, i, i)).fold(0.0, f64::max);
    for j in 0..m {
        let mut d = at(g, m, j, j);
        for k in 0..j {
            let l = at(g, m, j, k);
            d -= l * l;
        }
        if !(d.is_finite() && d > 1e-12 * scale) {
            return false;
        }
        let d = d.sqrt();
        if let Some(x) = g.get_mut(j * m + j) {
            *x = d;
        }
        for i in j + 1..m {
            let mut s = at(g, m, i, j);
            for k in 0..j {
                s -= at(g, m, i, k) * at(g, m, j, k);
            }
            if let Some(x) = g.get_mut(i * m + j) {
                *x = s / d;
            }
        }
    }
    true
}

/// Solve L·Lᵀ·x = b in place, with L the factor [`cholesky`] left in `l`.
fn solve(l: &[f64], m: usize, b: &mut [f64]) {
    for i in 0..m {
        let mut s = b.get(i).copied().unwrap_or(0.0);
        for k in 0..i {
            s -= at(l, m, i, k) * b.get(k).copied().unwrap_or(0.0);
        }
        if let Some(x) = b.get_mut(i) {
            *x = s / at(l, m, i, i);
        }
    }
    for i in (0..m).rev() {
        let mut s = b.get(i).copied().unwrap_or(0.0);
        for k in i + 1..m {
            s -= at(l, m, k, i) * b.get(k).copied().unwrap_or(0.0);
        }
        if let Some(x) = b.get_mut(i) {
            *x = s / at(l, m, i, i);
        }
    }
}

impl Flow {
    fn new(cx: &mut Cx, cu: &Copper, path: &Path) -> Flow {
        let (board, nets) = (cx.board, cx.nets);
        let n = cu.items.len();
        let mut share: Vec<Option<Vec<f64>>> = vec![None; n];
        let live = |l: usize| cu.links.get(l).is_some_and(|(a, b, _)| path.on(*a) && path.on(*b));
        let point = |l: usize| cu.links.get(l).map_or([0.0, 0.0], |(_, _, at)| *at);
        // The points: one where two items touch (by link), then a hub for each pad, via and
        // spoke set and a centre for each pour island on the path.
        let mut hub: Vec<Option<usize>> = vec![None; n];
        let mut count = cu.links.len();
        for (k, item) in cu.items.iter().enumerate() {
            if path.on(k)
                && !matches!(item, Item::Track(_))
                && let Some(slot) = hub.get_mut(k)
            {
                *slot = Some(count);
                count += 1;
            }
        }
        // The resistances between two points, as conductances, with the item each is of; points
        // at one place along a track are one point (where three tracks meet).
        let mut edges: Vec<(usize, usize, f64, usize)> = Vec::new();
        let mut same: Vec<usize> = (0..count).collect();
        let outer = copper_of(cx, CopperLayer::Top).min(copper_of(cx, CopperLayer::Bottom));
        let plating = super::param_value(board, CURRENT, "plating").max(0.001);
        for (k, item) in cu.items.iter().enumerate() {
            if !path.on(k) {
                continue;
            }
            let links: Vec<usize> = cu.at.get(k).into_iter().flatten().copied().filter(|l| live(*l)).collect();
            // A hub's or centre's resistance to each place the item touches another.
            let ohms = match item {
                Item::Track(t) => {
                    let Some(tr) = board.tracks.get(*t) else { continue };
                    let th = copper_of(cx, tr.layer);
                    let pieces = track_pieces(&tr.points, &tr.curves);
                    let mut along: Vec<(f64, usize)> = links.iter().map(|l| (position(&pieces, point(*l)), *l)).collect();
                    along.sort_by(|a, b| a.0.total_cmp(&b.0));
                    for w in along.windows(2) {
                        let (Some((sa, la)), Some((sb, lb))) = (w.first(), w.get(1)) else { continue };
                        if (sb - sa).abs() <= 1e-6 {
                            let (ra, rb) = (root(&mut same, *la), root(&mut same, *lb));
                            if let Some(slot) = same.get_mut(ra.max(rb)) {
                                *slot = ra.min(rb);
                            }
                            continue;
                        }
                        let r = RHO * (sb - sa).abs().max(0.01) / (tr.width.max(0.01) * th);
                        edges.push((*la, *lb, conductance(r), k));
                    }
                    continue;
                }
                // Half a square of the outer copper.
                Item::Pad(_) => RHO * 0.5 / outer,
                // Half the barrel (as long as the dielectric it spans) to each side.
                Item::Via(v) => {
                    let Some(via) = board.vias.get(*v) else { continue };
                    RHO * cx.geo.height(via.from, via.to).max(0.05) / (std::f64::consts::PI * via.drill.max(0.05) * plating) / 2.0
                }
                Item::Spokes(pi, fi) => {
                    let Some(fill) = nets.fills.get(*fi) else { continue };
                    let Some(r) = fill.reliefs.iter().find(|r| r.pad == *pi) else { continue };
                    let gap = board.pours.iter().find(|p| p.id == fill.pour).map_or(0.3, |p| p.thermal.gap);
                    let wide = f64::from(r.connected.max(1)) * r.spoke_width.max(0.01);
                    RHO * gap.max(0.05) / (wide * copper_of(cx, fill.layer)) / 2.0
                }
                // A contact spreading into a sheet as wide as the island: (R□ / 2π)·ln(D / a).
                Item::Fill(fi, si) => {
                    let Some(fill) = nets.fills.get(*fi) else { continue };
                    let extent = fill
                        .region
                        .shapes()
                        .get(*si)
                        .and_then(|s| super::super::bbox(s.outer.iter().copied()))
                        .map_or(1.0, |[[a, b], [c, d]]| (c - a).hypot(d - b) / 2.0);
                    let sheet = RHO / copper_of(cx, fill.layer);
                    sheet / std::f64::consts::TAU * (extent / CONTACT_RADIUS).max(std::f64::consts::E).ln()
                }
            };
            if let Some(h) = hub.get(k).copied().flatten() {
                edges.extend(links.iter().map(|l| (h, *l, conductance(ohms), k)));
            }
        }
        for e in edges.iter_mut() {
            (e.0, e.1) = (root(&mut same, e.0), root(&mut same, e.1));
        }
        edges.retain(|e| e.0 != e.1);
        let hub: Vec<Option<usize>> = hub.iter().map(|h| h.map(|h| root(&mut same, h))).collect();
        // Each point's piece of copper.
        let mut piece = vec![usize::MAX; count];
        for (l, (a, _, _)) in cu.links.iter().enumerate() {
            if live(l)
                && let (Some(slot), Some(c)) = (piece.get_mut(l), path.comp.get(*a))
            {
                *slot = *c;
            }
        }
        for (k, h) in hub.iter().enumerate() {
            if let (Some(h), Some(c)) = (h, path.comp.get(k))
                && let Some(slot) = piece.get_mut(*h)
            {
                *slot = *c;
            }
        }
        // Where the current comes from and goes, as points, by piece.
        let mut ends: BTreeMap<usize, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
        for t in &path.terminals {
            let (Some(c), Some(h)) = (path.comp.get(*t), hub.get(*t).copied().flatten()) else { continue };
            let e = ends.entry(*c).or_default();
            if path.sources.contains(t) { e.0.push(h) } else { e.1.push(h) }
        }
        for (c, (mut sources, mut loads)) in ends {
            if sources.is_empty() || loads.is_empty() {
                // Only a piece's two current pins tell which way the current goes.
                let mut both: Vec<usize> = sources.into_iter().chain(loads).collect();
                if both.len() != 2 {
                    continue;
                }
                loads = both.split_off(1);
                sources = both;
            }
            let free: Vec<usize> = (0..count).filter(|v| piece.get(*v) == Some(&c) && !sources.contains(v) && same.get(*v) == Some(v)).collect();
            let m = free.len();
            if m == 0 || m > MAX_FLOW {
                continue;
            }
            let mine: Vec<(usize, usize, f64, usize)> = edges.iter().copied().filter(|(a, _, _, _)| piece.get(*a) == Some(&c)).collect();
            let work = (m * m * m / 6).saturating_add(loads.len().saturating_mul(m * m + mine.len()).saturating_mul(2));
            if !cx.spend(work) {
                return Flow { share };
            }
            let index: BTreeMap<usize, usize> = free.iter().enumerate().map(|(i, v)| (*v, i)).collect();
            let mut g = vec![0.0; m * m];
            for (a, b, y, _) in &mine {
                match (index.get(a), index.get(b)) {
                    (Some(i), Some(j)) => {
                        add(&mut g, m, *i, *i, *y);
                        add(&mut g, m, *j, *j, *y);
                        add(&mut g, m, *i, *j, -*y);
                        add(&mut g, m, *j, *i, -*y);
                    }
                    (Some(i), None) | (None, Some(i)) => add(&mut g, m, *i, *i, *y),
                    (None, None) => {}
                }
            }
            if !cholesky(&mut g, m) {
                continue;
            }
            let mut shares: BTreeMap<usize, Vec<f64>> = BTreeMap::new();
            for (li, load) in loads.iter().enumerate() {
                let Some(i) = index.get(load) else { continue };
                // One ampere drawn at the load, the sources at 0 V.
                let mut v = vec![0.0; m];
                if let Some(x) = v.get_mut(*i) {
                    *x = 1.0;
                }
                solve(&g, m, &mut v);
                let volts = |p: usize| index.get(&p).and_then(|i| v.get(*i)).copied().unwrap_or(0.0);
                // Along a track, the most any stretch of it carries; through a hub, half of what
                // flows in and out of it.
                let mut through: BTreeMap<usize, f64> = BTreeMap::new();
                for (a, b, y, k) in &mine {
                    let amps = (y * (volts(*a) - volts(*b))).abs();
                    let amps = if amps.is_finite() { amps } else { 1.0 };
                    let slot = through.entry(*k).or_default();
                    if matches!(cu.items.get(*k), Some(Item::Track(_))) {
                        *slot = slot.max(amps);
                    } else {
                        *slot += amps / 2.0;
                    }
                }
                for (k, amps) in through {
                    let slot = shares.entry(k).or_insert_with(|| vec![0.0; loads.len()]);
                    if let Some(s) = slot.get_mut(li) {
                        *s = amps.min(1.0);
                    }
                }
            }
            for (k, s) in shares {
                if let Some(slot) = share.get_mut(k) {
                    *slot = Some(s);
                }
            }
        }
        Flow { share }
    }
}

/// How much of a net's current a piece of its copper carries: the least and the most share of
/// it, over which of the loads draws (the same for a piece of the trunk: 1 for all of it, less
/// for its part of copper running in parallel).
#[derive(Clone, Copy, Debug)]
struct Part {
    least: f64,
    most: f64,
}

impl Part {
    const ALL: Part = Part { least: 1.0, most: 1.0 };

    /// All of the current passes it.
    fn all(&self) -> bool {
        self.least >= 1.0 - SPLIT_TOL
    }

    /// Its share is the same wherever the current goes.
    fn known(&self) -> bool {
        self.most - self.least <= SPLIT_TOL
    }
}

/// The share of the current the items `items` (of one piece of copper) carry together.
fn part(flow: &Flow, path: &Path, cu: &Copper, items: &[usize]) -> Part {
    let mut sum: Option<Vec<f64>> = None;
    let mut solved = !items.is_empty();
    let piece = items.first().and_then(|k| path.comp.get(*k));
    for k in items {
        let s = flow.share.get(*k).and_then(Option::as_ref).filter(|_| path.comp.get(*k) == piece);
        match (s, sum.as_mut()) {
            (Some(s), None) => sum = Some(s.clone()),
            (Some(s), Some(acc)) if acc.len() == s.len() => {
                for (a, x) in acc.iter_mut().zip(s) {
                    *a = (*a + x).min(1.0);
                }
            }
            _ => solved = false,
        }
    }
    if let Some(s) = sum.filter(|_| solved) {
        let lo = s.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = s.iter().copied().fold(0.0, f64::max);
        return if s.is_empty() { Part { least: 0.0, most: 0.0 } } else { Part { least: lo.min(hi), most: hi } };
    }
    if path.carries_all(cu, items) { Part::ALL } else { Part { least: 0.0, most: 1.0 } }
}

fn run(cx: &mut Cx) {
    let (current, bottleneck) = (cx.wants(CURRENT), cx.wants(BOTTLENECK));
    if !current && !bottleneck {
        return;
    }
    let supplies = supplies(cx);
    if supplies.is_empty() {
        return;
    }
    let which: BTreeSet<usize> = supplies.iter().map(|s| s.net).collect();
    let Some(all) = copper(cx, &which) else { return };
    let pins = Pins::new(cx);
    // Each supply's capacity first (the critical code), then the pours' necks.
    let mut paths: Vec<(&Supply, &Copper, Path)> = Vec::new();
    for s in &supplies {
        let Some(cu) = all.get(&s.net) else { continue };
        if !cx.spend(cu.items.len().saturating_add(cu.links.len()).saturating_mul(4)) {
            return;
        }
        let path = Path::new(cx, &pins, cu);
        if !path.terminals.iter().any(|t| path.terminals_with(*t).len() >= 2) {
            let needs = format!("geometry: copper joining two of {}'s power pins", s.name);
            cx.not_applicable(CURRENT, needs.clone());
            cx.not_applicable(BOTTLENECK, needs);
            continue;
        }
        if current {
            let flow = Flow::new(cx, cu, &path);
            capacity(cx, s, cu, &path, &flow);
        }
        paths.push((s, cu, path));
    }
    if bottleneck {
        for (s, cu, path) in &paths {
            if cx.spent() {
                return;
            }
            necks(cx, s, cu, path);
        }
    }
}

/// A piece of the current path too hot at its largest share of the current only, which depends
/// on which loads draw.
struct Hot {
    at: [f64; 2],
    item: DrcItem,
    what: String,
    rise: f64,
}

/// The current a piece carries in words (at the least share): all of the net's, its share by
/// conductance, or the least it carries whichever loads draw.
fn carried(s: &Supply, p: Part) -> String {
    let (amps, percent) = (num((s.amps * p.least * 1000.0).round() / 1000.0), num((p.least * 100.0).round()));
    if p.all() {
        format!("{}'s {} A, all of which passes it", s.name, num(s.amps))
    } else if p.known() {
        format!("{amps} A, its share by conductance ({percent} %) of {}'s {} A where copper runs in parallel", s.name, num(s.amps))
    } else {
        format!("{amps} A, the least share by conductance ({percent} %) of {}'s {} A it carries whichever loads draw", s.name, num(s.amps))
    }
}

/// How the share is said in a finding's regime.
fn split(p: Part) -> String {
    let percent = num((p.least * 100.0).round());
    if p.all() {
        String::new()
    } else if p.known() {
        format!(", {percent} % of it here by conductance")
    } else {
        format!(", at least {percent} % of it here by conductance whichever loads draw")
    }
}

/// The track message's variant for a share.
fn variant(p: Part) -> Option<&'static str> {
    if p.all() {
        None
    } else if p.known() {
        Some("parallel")
    } else {
        Some("least")
    }
}

/// The current, the fit and the rise allowed, as a finding's regime words them.
fn regime_of(s: &Supply, p: Part, th: f64, layer: &str) -> String {
    let allowed = if s.class_rise { format!("its net class's {} °C", num(s.rise)) } else { format!("{} °C", num(s.rise)) };
    let split = split(p);
    format!(
        "{} A ({}){split}; IPC-2152 external fit on {} mm ({} oz) copper on {layer}; allowed rise {allowed}",
        num(s.amps),
        s.from,
        num(th),
        num((mm_to_oz(th) * 100.0).round() / 100.0)
    )
}

fn capacity(cx: &mut Cx, s: &Supply, cu: &Copper, path: &Path, flow: &Flow) {
    let (board, nets) = (cx.board, cx.nets);
    let p = |name: &str| super::param_value(board, CURRENT, name);
    let (factor, plating, cluster) = (p("error_factor").max(1.0), p("plating"), p("cluster"));
    let mut shares: Vec<Hot> = Vec::new();
    let checked = |k: usize| path.on(k) && path.terminals_with(k).len() >= 2;
    let fit_note = |th: f64| {
        let oz = mm_to_oz(th);
        (!in_range_2152(0.0, th)).then(|| {
            format!(
                "the fit is checked on {} to {} oz copper; {} oz is outside it: an estimate",
                num(*FIT2152_OZ.start()),
                num(*FIT2152_OZ.end()),
                num(oz)
            )
        })
    };
    let severity = |dt: f64| if dt > factor * s.rise { Severity::Error } else { Severity::Warning };
    let short = p("short");
    let cost = cu.items.len().saturating_add(cu.links.len());
    // Tracks: what isn't too hot at all of the current isn't at its share either.
    for (k, item) in cu.items.iter().enumerate() {
        let Item::Track(ti) = item else { continue };
        let Some(t) = board.tracks.get(*ti).filter(|_| checked(k)) else { continue };
        let th = copper_of(cx, t.layer);
        if rise_2152(s.amps, t.width, th).is_none_or(|dt| dt <= s.rise) {
            continue;
        }
        if !cx.spend(cost) {
            return;
        }
        let share = part(flow, path, cu, &[k]);
        let mid = track_pieces(&t.points, &t.curves)
            .first()
            .map_or(t.points.first().copied().unwrap_or([0.0, 0.0]), |(a, b)| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
        // Too hot at the least it carries: a finding; only at the most: noted.
        let hot = |f: f64| rise_2152(s.amps * f, t.width, th).filter(|dt| *dt > s.rise);
        let dt = match (hot(share.least), hot(share.most)) {
            (Some(dt), _) => dt,
            (None, Some(dt)) => {
                shares.push(Hot {
                    at: mid,
                    item: DrcItem::Track(t.id),
                    what: format!("track {} ({} mm on {})", t.id, num(t.width), t.layer.name()),
                    rise: dt,
                });
                continue;
            }
            (None, None) => continue,
        };
        let amps = s.amps * share.least;
        let need = width_2152(amps, s.rise, th).map_or(t.width, |w| (w * 100.0).ceil() / 100.0);
        let (name, layer, note) = (s.name.clone(), t.layer, fit_note(th));
        let (regime, what) = (regime_of(s, share, th, &layer.name()), carried(s, share));
        let length = path::length(&t.points, &t.curves, false);
        if length < short && length < 4.0 * t.width {
            // A neck: its ends' pads and vias cool it, so the long-track fit says little.
            let id = t.id;
            cx.add(CURRENT, &[DrcItem::Track(t.id).key(), "neck".into()], || {
                Draft::new(
                    mid,
                    vec![DrcItem::Track(id)],
                    format!(
                        "track {id} ({name}, {}, {} mm) is a {} mm neck at {what}: as a long track it would warm by {} °C; the copper at its ends draws the heat out, so judge its drop and the copper round it",
                        layer.name(),
                        num(t.width),
                        num((length * 100.0).round() / 100.0),
                        num(dt.round())
                    ),
                )
                .layer(layer)
                .nets(vec![name.clone()])
                .severity(Severity::Info)
                .class(Class::Practical)
                .variant("neck")
                .param("short")
                .regime(regime)
                .arg("net", name.clone())
                .arg("length", num((length * 100.0).round() / 100.0))
                .arg("width", num(t.width))
                .review()
                .fix("ecad.board.net.inspect", json!({"nets": [name.clone()], "metrics": ["segments"]}), "see the copper round the neck: a pour or more vias at its ends carry the current better than a wider neck")
            });
            continue;
        }
        let Some(fits) = widens(cx, *ti, s.net, need) else { return };
        cx.add(CURRENT, &[DrcItem::Track(t.id).key()], || {
            let mut d = Draft::new(
                mid,
                vec![DrcItem::Track(t.id)],
                format!(
                    "track {} ({name}, {}, {} mm) warms by {} °C at {what}; {} °C is allowed: it needs {} mm",
                    t.id,
                    layer.name(),
                    num(t.width),
                    num(dt.round()),
                    num(s.rise),
                    num(need)
                ),
            )
            .layer(layer)
            .nets(vec![name.clone()])
            .measure(dt, s.rise, "°C", Bound::Max)
            .param(if s.class_rise { "" } else { "rise" })
            .severity(severity(dt))
            .regime(regime)
            .arg("net", name.clone())
            .arg("amps", num((amps * 1000.0).round() / 1000.0))
            .arg("width", num(t.width))
            .arg("need", num(need))
            .fix(
                "ecad.board.track.edit",
                if fits { json!({"tracks": [t.id], "width": need}) } else { Value::Null },
                if fits {
                    format!("widen track {} to {} mm (what the fit asks for at {} A)", t.id, num(need), num((amps * 1000.0).round() / 1000.0))
                } else {
                    format!("track {} can't widen to {} mm without coming too close to other nets' copper: add parallel copper or a pour on {name}, or carry it on another layer through more vias", t.id, num(need))
                },
            )
            .fix("ecad.intent.set", Value::Null, format!("if {name} carries less than {} A at most, declare its current_a", num(s.amps)));
            if let Some(v) = variant(share) {
                d = d.variant(v).arg("percent", num((share.least * 100.0).round()));
            }
            if let Some(n) = note {
                d = d.verify(n);
            }
            d
        });
    }
    // Spoke sets.
    for (k, item) in cu.items.iter().enumerate() {
        let Item::Spokes(pi, fi) = item else { continue };
        if !checked(k) {
            continue;
        }
        let (Some(pad), Some(fill)) = (nets.pads.get(*pi), nets.fills.get(*fi)) else { continue };
        let Some(relief) = fill.reliefs.iter().find(|r| r.pad == *pi) else { continue };
        let width = f64::from(relief.connected) * relief.spoke_width;
        let th = copper_of(cx, fill.layer);
        if rise_2152(s.amps, width, th).is_none_or(|dt| dt <= s.rise) {
            continue;
        }
        if !cx.spend(cost) {
            return;
        }
        let share = part(flow, path, cu, &[k]);
        let pad_item = DrcItem::Pad { footprint: pad.footprint, number: pad.pad.number.clone(), reference: pad.reference() };
        let hot = |f: f64| rise_2152(s.amps * f, width, th).filter(|dt| *dt > s.rise);
        let dt = match (hot(share.least), hot(share.most)) {
            (Some(dt), _) => dt,
            (None, Some(dt)) => {
                shares.push(Hot { at: pad.at(), item: pad_item, what: format!("the spokes of {} to pour {}", pad.reference(), fill.pour), rise: dt });
                continue;
            }
            (None, None) => continue,
        };
        let (name, layer, note, reference) = (s.name.clone(), fill.layer, fit_note(th), pad.reference());
        let (regime, what) = (regime_of(s, share, th, &layer.name()), carried(s, share));
        let keys = [pad_item.key(), DrcItem::Pour(fill.pour).key()];
        let (designator, number, pour, connected) = (pad.designator.clone(), pad.pad.number.clone(), fill.pour, relief.connected);
        cx.add(CURRENT, &keys, || {
            let mut d = Draft::new(
                pad.at(),
                vec![pad_item, DrcItem::Pour(pour)],
                format!(
                    "the {connected} thermal spokes ({} mm together) joining {reference} to pour {pour} ({name}, {}) warm by {} °C at {what}; {} °C is allowed",
                    num(width),
                    layer.name(),
                    num(dt.round()),
                    num(s.rise)
                ),
            )
            .layer(layer)
            .nets(vec![name.clone()])
            .variant("spokes")
            .measure(dt, s.rise, "°C", Bound::Max)
            .param(if s.class_rise { "" } else { "rise" })
            .severity(severity(dt))
            .regime(regime)
            .arg("net", name.clone())
            .arg("amps", num((s.amps * share.least * 1000.0).round() / 1000.0))
            .arg("width", num(width))
            .arg("count", connected.to_string())
            .fix(
                "ecad.board.pour.connect",
                json!({"footprint": designator, "pads": [number], "connect": "solid"}),
                format!("join {reference} to the pour solid (a high-current pad needs no relief), or give it wider spokes"),
            );
            if let Some(n) = note {
                d = d.verify(n);
            }
            d
        });
    }
    // Via clusters: vias of the net on the path within `cluster` of one another.
    let vias: Vec<(usize, usize)> =
        cu.items.iter().enumerate().filter_map(|(k, item)| if let Item::Via(v) = item { checked(k).then_some((k, *v)) } else { None }).collect();
    if !cx.spend(vias.len().saturating_mul(vias.len()).saturating_add(1)) {
        return;
    }
    let mut group: Vec<usize> = (0..vias.len()).collect();
    for a in 0..vias.len() {
        for b in a + 1..vias.len() {
            let (Some((_, va)), Some((_, vb))) = (vias.get(a), vias.get(b)) else { continue };
            let (Some(x), Some(y)) = (board.vias.get(*va), board.vias.get(*vb)) else { continue };
            if distance(x.at, y.at) <= cluster + (x.diameter + y.diameter) / 2.0 {
                let (ga, gb) = (root(&mut group, a), root(&mut group, b));
                if let Some(slot) = group.get_mut(ga.max(gb)) {
                    *slot = ga.min(gb);
                }
            }
        }
    }
    let mut clusters: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for i in 0..vias.len() {
        let g = root(&mut group, i);
        if let Some(v) = vias.get(i) {
            clusters.entry(g).or_default().push(*v);
        }
    }
    for members in clusters.values() {
        let list: Vec<(usize, &super::super::board::Via)> = members.iter().filter_map(|(k, v)| Some((*k, board.vias.get(*v)?))).collect();
        // The current crosses a cluster a layer at a time: between two neighbouring layers, the
        // vias spanning the step share it (each of a stack of vias carries all of it). The worst
        // step counts.
        let steps: BTreeSet<usize> = list
            .iter()
            .flat_map(|(_, v)| {
                let (a, b) = v.span(board.copper_layers);
                a..b
            })
            .collect();
        // The worst step too hot at the least it carries; else the worst too hot at the most.
        let (mut worst, mut noted): (Option<(f64, Part, usize, f64)>, Option<(f64, usize)>) = (None, None);
        for step in steps {
            let across = |v: &super::super::board::Via| {
                let (a, b) = v.span(board.copper_layers);
                a <= step && step < b
            };
            let width: f64 = list.iter().filter(|(_, v)| across(v)).map(|(_, v)| std::f64::consts::PI * v.drill).sum();
            if rise_2152(s.amps, width, plating).is_none_or(|dt| dt <= s.rise) {
                continue;
            }
            if !cx.spend(cost) {
                return;
            }
            let cut: Vec<usize> = list.iter().filter(|(_, v)| across(v)).map(|(k, _)| *k).collect();
            let share = part(flow, path, cu, &cut);
            let hot = |f: f64| rise_2152(s.amps * f, width, plating).filter(|dt| *dt > s.rise);
            match (hot(share.least), hot(share.most)) {
                (Some(dt), _) if worst.is_none_or(|w| dt > w.0) => worst = Some((dt, share, step, width)),
                (None, Some(dt)) if noted.is_none_or(|w| dt > w.0) => noted = Some((dt, step)),
                _ => {}
            }
        }
        let step_of = |step: usize| -> Vec<&super::super::board::Via> {
            list.iter()
                .map(|(_, v)| *v)
                .filter(|v| {
                    let (a, b) = v.span(board.copper_layers);
                    a <= step && step < b
                })
                .collect()
        };
        // The layers the vias at a step span (the step's own when they differ).
        let between = |step: usize, vias: &[&super::super::board::Via]| {
            let spans: BTreeSet<(usize, usize)> = vias.iter().map(|v| v.span(board.copper_layers)).collect();
            let (a, b) = match spans.iter().next() {
                Some(span) if spans.len() == 1 => *span,
                _ => (step, step + 1),
            };
            match (cx.geo.layers.get(a), cx.geo.layers.get(b)) {
                (Some(a), Some(b)) => format!("{} to {}", a.name(), b.name()),
                _ => String::new(),
            }
        };
        let Some((dt, share, step, width)) = worst else {
            if let Some((dt, step)) = noted {
                let vias = step_of(step);
                if let Some(first) = vias.first() {
                    let ids: Vec<String> = vias.iter().map(|v| v.id.to_string()).collect();
                    shares.push(Hot {
                        at: first.at,
                        item: DrcItem::Via(first.id),
                        what: format!("via {} ({})", ids.join(", "), between(step, &vias)),
                        rise: dt,
                    });
                }
            }
            continue;
        };
        let at_step: Vec<&super::super::board::Via> = list
            .iter()
            .map(|(_, v)| *v)
            .filter(|v| {
                let (a, b) = v.span(board.copper_layers);
                a <= step && step < b
            })
            .collect();
        let Some(first) = at_step.first() else { continue };
        let ids: Vec<String> = at_step.iter().map(|v| v.id.to_string()).collect();
        let between = between(step, &at_step);
        let amps = s.amps * share.least;
        let drill = at_step.iter().map(|v| v.drill).fold(f64::INFINITY, f64::min);
        let need = width_2152(amps, s.rise, plating).map(|w| (w / (std::f64::consts::PI * drill.max(0.01))).ceil());
        let count = at_step.len();
        let keys: Vec<String> = list.iter().map(|(_, v)| DrcItem::Via(v.id).key()).collect();
        let items: Vec<DrcItem> = at_step.iter().map(|v| DrcItem::Via(v.id)).collect();
        let (name, at, what, split) = (s.name.clone(), first.at, carried(s, share), split(share));
        let regime = format!(
            "{} A ({}){split}; IPC-2152 external fit on the barrels from {between} ({} mm plating); allowed rise {} °C",
            num(s.amps),
            s.from,
            num(plating),
            num(s.rise)
        );
        let says = match need {
            Some(n) if n.is_finite() && n > count as f64 => {
                format!(
                    "use {} vias of {} mm drill or larger from {between} here, side by side where the current enters and leaves",
                    num(n),
                    num(drill)
                )
            }
            _ => "use more or larger vias here".to_string(),
        };
        cx.add(CURRENT, &keys, || {
            Draft::new(
                at,
                items,
                format!(
                    "{} {} ({name}, {between}) at {what}: barrels {} mm round together warm by {} °C by the fit; {} °C is allowed",
                    if count == 1 { "via" } else { "vias" },
                    ids.join(", "),
                    num((width * 100.0).round() / 100.0),
                    num(dt.round()),
                    num(s.rise)
                ),
            )
            .nets(vec![name.clone()])
            .variant(if count == 1 { "via" } else { "vias" })
            .measure(dt, s.rise, "°C", Bound::Max)
            .param(if s.class_rise { "" } else { "rise" })
            .severity(severity(dt))
            .regime(regime)
            .arg("net", name.clone())
            .arg("amps", num((amps * 1000.0).round() / 1000.0))
            .arg("count", count.to_string())
            .arg("between", between.clone())
            .verify("a via's barrel taken as a track as wide as its circumference: a via follows the tracks it joins (Brooks and Adam), so this is a rough bound")
            .fix("ecad.board.via", Value::Null, says)
        });
    }
    // Where the current's share depends on which loads draw: one info for the net.
    let Some(worst) = shares.iter().max_by(|a, b| a.rise.total_cmp(&b.rise)) else { return };
    let count = shares.len();
    let (name, item, at, what, dt) = (s.name.clone(), worst.item.clone(), worst.at, worst.what.clone(), worst.rise);
    cx.add(CURRENT, &[format!("{name}|share")], || {
        Draft::new(
            at,
            vec![item],
            format!(
                "{count} piece{} of {name}'s copper would warm by more than {} °C at the largest share of its {} A they carry, should the loads nearest them draw all of it (the worst: {what}, {} °C); how much each load draws isn't known",
                if count == 1 { "" } else { "s" },
                num(s.rise),
                num(s.amps),
                num(dt.round())
            ),
        )
        .nets(vec![name.clone()])
        .variant("share")
        .severity(Severity::Info)
        .class(Class::Practical)
        .review()
        .measure(dt, s.rise, "°C", Bound::Max)
        .param(if s.class_rise { "" } else { "rise" })
        .regime(format!("{} A ({}), the most {name} carries; a branch carries its loads' share", num(s.amps), s.from))
        .arg("net", name.clone())
        .arg("amps", num(s.amps))
        .arg("count", count.to_string())
        .arg("what", what)
        .fix("ecad.board.net.inspect", json!({"nets": [name], "metrics": ["segments"]}), "see the net's segments: widen those that carry most of the current")
        .fix("ecad.intent.set", Value::Null, format!("if {name}'s branches carry less, declare the current where it is known (a smaller current_a), or approve"))
    });
}

/// The representative of `i`'s group.
fn root(group: &mut [usize], i: usize) -> usize {
    let mut r = i;
    for _ in 0..group.len() {
        match group.get(r) {
            Some(p) if *p != r => r = *p,
            _ => break,
        }
    }
    // Shorten the way for the next look.
    if let Some(slot) = group.get_mut(i) {
        *slot = r;
    }
    r
}

/// The most cross-sections taken along one way across a pour island.
const MAX_CUTS: usize = 4096;
/// The most sides of one island the current is followed between, places kept on each side, and
/// ways across it looked at.
const MAX_SIDES: usize = 64;
const MAX_ENDS: usize = 16;
const MAX_PAIRS: usize = 48;

/// The least copper of `shape` across the way from `a` to `b`, and where: on lines square to the
/// way, every `step` along it and at least `clear` from either end, the length of each line
/// inside the copper (outline less holes). All current from `a` to `b` crosses each such line,
/// so each length is a cut the current can't avoid. `None` for a way too short, or once the
/// budget is spent.
fn least_across(cx: &mut Cx, shape: &Shape, a: [f64; 2], b: [f64; 2], step: f64, clear: f64) -> Option<(f64, [f64; 2])> {
    let len = distance(a, b);
    let (lo, hi) = (clear, len - clear);
    if !(len > 1e-9 && hi > lo) {
        return None;
    }
    if !cx.spend(shape.corners().saturating_add(1)) {
        return None;
    }
    let d = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
    let n = [-d[1], d[0]];
    let local = |p: [f64; 2]| ((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1], (p[0] - a[0]) * n[0] + (p[1] - a[1]) * n[1]);
    // The edges reaching into the band between the ends: where each corner lies along the way
    // and across it, sorted by where they start along it.
    let mut edges: Vec<((f64, f64), (f64, f64))> = Vec::new();
    for ring in std::iter::once(&shape.outer).chain(shape.holes.iter()) {
        let k = ring.len();
        for i in 0..k {
            let (Some(p), Some(q)) = (ring.get(i), ring.get((i + 1) % k)) else { continue };
            let (lp, lq) = (local(*p), local(*q));
            if lp.0.max(lq.0) < lo || lp.0.min(lq.0) > hi {
                continue;
            }
            edges.push((lp, lq));
        }
    }
    edges.sort_by(|x, y| x.0.0.min(x.1.0).total_cmp(&y.0.0.min(y.1.0)));
    let cuts = ((hi - lo) / step).ceil().clamp(1.0, MAX_CUTS as f64) as usize;
    let (mut active, mut next, mut ts) = (Vec::new(), 0usize, Vec::new());
    let mut best: Option<(f64, f64)> = None;
    for c in 0..=cuts {
        let s = lo + (hi - lo) * c as f64 / cuts as f64;
        while let Some(e) = edges.get(next)
            && e.0.0.min(e.1.0) <= s
        {
            active.push(next);
            next += 1;
        }
        active.retain(|i| edges.get(*i).is_some_and(|e| e.0.0.max(e.1.0) >= s));
        if !cx.spend(active.len().saturating_add(1)) {
            return None;
        }
        ts.clear();
        for ((sp, tp), (sq, tq)) in active.iter().filter_map(|i| edges.get(*i)) {
            if (*sp > s) != (*sq > s) {
                ts.push(tp + (tq - tp) * (s - sp) / (sq - sp));
            }
        }
        ts.sort_by(f64::total_cmp);
        let w: f64 = ts.chunks(2).filter_map(|c| Some(c.get(1)? - c.first()?)).filter(|v| v.is_finite()).sum();
        if best.is_none_or(|(v, _)| w < v) {
            best = Some((w, s));
        }
    }
    best.map(|(w, s)| (w, [a[0] + d[0] * s, a[1] + d[1] * s]))
}

fn necks(cx: &mut Cx, s: &Supply, cu: &Copper, path: &Path) {
    let (board, nets) = (cx.board, cx.nets);
    let step = super::param_value(board, BOTTLENECK, "step").max(0.02);
    let clear = super::param_value(board, BOTTLENECK, "end_clear");
    for (k, item) in cu.items.iter().enumerate() {
        let Item::Fill(fi, si) = item else { continue };
        if !path.on(k) || path.terminals_with(k).len() < 2 {
            continue;
        }
        let Some(fill) = nets.fills.get(*fi) else { continue };
        let shapes = fill.shapes();
        let Some(shape) = shapes.get(*si) else { continue };
        let th = copper_of(cx, fill.layer);
        let Some(need) = width_2152(s.amps, s.rise, th) else { continue };
        // Where the island meets the rest of the path, by side: the items past a place without
        // the island (places joined some other way share a side), and its places.
        let mut sides: Vec<(Vec<bool>, Vec<[f64; 2]>)> = Vec::new();
        let mut side_of: BTreeMap<usize, usize> = BTreeMap::new();
        for l in cu.at.get(k).into_iter().flatten() {
            let (Some(j), Some((_, _, at))) = (cu.other(*l, k), cu.links.get(*l)) else { continue };
            if !path.on(j) {
                continue;
            }
            let g = if let Some(g) = side_of.get(&j) {
                *g
            } else if let Some(g) = sides.iter().position(|(seen, _)| seen.get(j).copied().unwrap_or(false)) {
                g
            } else {
                if sides.len() >= MAX_SIDES {
                    continue;
                }
                if !cx.spend(cu.items.len().saturating_add(cu.links.len())) {
                    return;
                }
                sides.push((cu.reach(&[j], &|x| x != k && path.on(x)), Vec::new()));
                sides.len() - 1
            };
            side_of.insert(j, g);
            if let Some((_, ends)) = sides.get_mut(g)
                && ends.len() < MAX_ENDS
                && !ends.iter().any(|p| distance(*p, *at) < 0.5)
            {
                ends.push(*at);
            }
        }
        let terms = path.terminals_with(k);
        let holds = |g: usize, t: usize| sides.get(g).and_then(|(seen, _)| seen.get(t)).copied().unwrap_or(false);
        let sources: Vec<usize> = terms.iter().copied().filter(|t| path.sources.contains(t)).collect();
        let loads: Vec<usize> = terms.iter().copied().filter(|t| !path.sources.contains(t)).collect();
        let has = |g: usize, list: &[usize]| list.iter().any(|t| holds(g, *t));
        let all_on = |g: usize, list: &[usize]| !list.is_empty() && list.iter().all(|t| holds(g, *t));
        let directed = !sources.is_empty() && !loads.is_empty();
        // The ways the current crosses the island: from a side with a source to one with a load
        // (with no source known, between any two sides with a terminal), between their nearest
        // places. All of it crosses where one side holds every source and the other every load
        // (with no source known, the island's only two terminals).
        let mut pairs: Vec<([f64; 2], [f64; 2], bool)> = Vec::new();
        for (ga, (_, ea)) in sides.iter().enumerate() {
            for (gb, (_, eb)) in sides.iter().enumerate() {
                if ga == gb || pairs.len() >= MAX_PAIRS {
                    continue;
                }
                let way = if directed {
                    has(ga, &sources) && has(gb, &loads) && !(gb < ga && has(gb, &sources) && has(ga, &loads))
                } else {
                    ga < gb && has(ga, &terms) && has(gb, &terms)
                };
                if !way {
                    continue;
                }
                let full = if directed { all_on(ga, &sources) && all_on(gb, &loads) } else { terms.len() == 2 };
                let nearest = ea.iter().flat_map(|p| eb.iter().map(move |q| (distance(*p, *q), *p, *q))).min_by(|x, y| x.0.total_cmp(&y.0));
                if let Some((_, p, q)) = nearest {
                    pairs.push((p, q, full));
                }
            }
        }
        // The narrowest, a way all of the current takes first.
        let mut worst: Option<(f64, [f64; 2], bool)> = None;
        for (a, b, full) in pairs {
            let Some((w, at)) = least_across(cx, shape, a, b, step, clear) else {
                if cx.spent() {
                    return;
                }
                continue;
            };
            if w < need && worst.is_none_or(|(v, _, f)| (full && !f) || (full == f && w < v)) {
                worst = Some((w, at, full));
            }
        }
        let Some((w, at, full)) = worst else { continue };
        let (name, layer, pour) = (s.name.clone(), fill.layer, fill.pour);
        let keys = [DrcItem::Pour(pour).key(), format!("i{si}")];
        let regime = regime_of(s, Part::ALL, th, &layer.name());
        cx.add(BOTTLENECK, &keys, || {
            let mut d = Draft::new(
                at,
                vec![DrcItem::Pour(pour)],
                format!(
                    "pour {pour} ({name}, {}) narrows to {} mm across at ({}, {}) on {name}'s current path; {} A needs {} mm there{}",
                    layer.name(),
                    num((w * 100.0).round() / 100.0),
                    num((at[0] * 100.0).round() / 100.0),
                    num((at[1] * 100.0).round() / 100.0),
                    num(s.amps),
                    num((need * 100.0).round() / 100.0),
                    if full { "" } else { " if all of it ran there (the island feeds some of the loads)" }
                ),
            )
            .layer(layer)
            .nets(vec![name.clone()])
            .measure(w, need, "mm", Bound::Min)
            .param("")
            .regime(regime)
            .arg("net", name.clone())
            .arg("amps", num(s.amps))
            .arg("x", num((at[0] * 100.0).round() / 100.0))
            .arg("y", num((at[1] * 100.0).round() / 100.0))
            .fix(
                "ecad.board.pour.inspect",
                json!({"pour": pour, "polygons": true}),
                format!(
                    "see the pour's outline and what cuts it at ({}, {}): widen the neck, move the vias or tracks that cut it, or add a track alongside",
                    num((at[0] * 100.0).round() / 100.0),
                    num((at[1] * 100.0).round() / 100.0)
                ),
            );
            if !full {
                d = d.variant("branch").severity(Severity::Info).review();
            }
            d
        });
    }
}
