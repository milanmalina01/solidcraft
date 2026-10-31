//! Return paths: where a fast net's return current flows, in the plane next to its tracks.
//! - `pi.return_path`: a track of a T1-or-faster net (or of a clock, RF line or switch node)
//!   whose centreline crosses a gap in its reference plane: a hole in the fill that is not one
//!   via's or pad's own antipad (a slot another track cuts, merged antipads), a split between two
//!   planes with no stitching capacitor near the crossing, or the plane's edge (an end of the
//!   track off the plane for `edge_run` or more). The reference is the neighbouring layer whose
//!   planes cover most of the track; a gap the other neighbour's plane of the same net bridges
//!   is no crossing, and the net's own vias' and pads' antipads never count. A track with no
//!   crossing whose ±3h corridor is less than `coverage_min` on the plane is an info.
//! - `pi.layer_change`: a via of such a net (RF lines excepted: `rf.vias` owns them) whose
//!   tracks leave it on layers with different reference planes: a via of the plane's net
//!   joining both within `reach` (`reach_ghz` above 1 GHz or at T2 and faster) when they are
//!   one net, a capacitor between the two nets within the same distance when not.
//! - `pi.stitching`: where a ground net's planes overlap on two or more layers and the board
//!   has fast or switching nets, the largest circle with no via joining them, against λg/10 at
//!   the frequency of concern (the intent's, else the top of its highest radio band); keepouts
//!   are left out. Also, at any frequency, an outer-layer ground island with no via to an inner
//!   ground plane.
//!
//! Planes are pours' fills (see [`super::geom`]); what is fast comes from the intent's tiers (see
//! [`super::roles`]): a net with no tier is listed as not applicable, with what it needs.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::super::board::ViaRole;
use super::super::checks::Severity;
use super::super::copper::inside;
use super::super::drc::{Bound, Class, DrcItem};
use super::super::grid::Grid;
use super::super::intent::NetRole;
use super::super::layers::CopperLayer;
use super::super::path;
use super::catalog::{Code, Family, Param};
use super::geom::{Sites, Under, Void, along, distance, track_pieces};
use super::roles::{NetInfo, Tier};
use super::{Cx, Draft, num};

const RETURN_PATH: &str = "pi.return_path";
const LAYER_CHANGE: &str = "pi.layer_change";
const STITCHING: &str = "pi.stitching";
/// The cell of the grid of a layer's tracks that slots are looked up in, in millimetres.
const CUT_CELL: f64 = 4.0;
/// The cell of the grid of a plane net's return vias, in millimetres.
const RETURN_CELL: f64 = 2.0;
/// The most crossings a finding names after its first.
const MAX_CROSSINGS: usize = 8;

const fn param(name: &'static str, default: f64, units: &'static str, min: f64, max: f64, source: &'static str, about: &'static str) -> Param {
    Param { name, default, units, source, min, max, verify: false, about }
}

const CODES: &[Code] = &[
    Code {
        code: RETURN_PATH,
        class: Class::Critical,
        severity: Severity::Error,
        worst: None,
        needs: "geometry and planes; intent: each net's edge-rate tier (driver or edge_ns) or role",
        section: "power-ground#return-paths",
        about: "a fast, RF or switching track crosses a gap, slot, split or edge of the plane it references (error); its ±3h corridor is short of plane (info)",
        principle: "A fast edge's return current flows in the plane right under its track; a gap makes it detour, which adds inductance and crosstalk and makes the gap radiate.",
        params: &[
            param("step", 0.05, "mm", 0.01, 1.0, "judgment: finer than the narrowest gap a pour leaves", "how far apart the centreline is looked at"),
            param(
                "edge_run",
                1.0,
                "mm",
                0.0,
                100.0,
                "judgment",
                "an end of a track off its plane this long or longer has left the plane (shorter: the plane's pull-back round a pad)",
            ),
            param(
                "antipad_slack",
                0.05,
                "mm",
                0.0,
                1.0,
                "judgment",
                "how much larger than one via's or pad's copper and the fill's clearance a hole may be and still be its antipad",
            ),
            param(
                "cap_reach",
                2.5,
                "mm",
                0.0,
                50.0,
                "Chen et al. 2002, via Riazi 2007",
                "a capacitor between the two planes this close to a split crossing bridges it",
            ),
            param(
                "corridor",
                3.0,
                "h",
                0.5,
                20.0,
                "Ott 2001: 80 % of the return lies within ±3h",
                "the corridor's half-width, in dielectric heights",
            ),
            param("coverage_min", 0.8, "ratio", 0.0, 1.0, "Ott 2001", "corridor coverage below this is reported as info"),
            param("min_length", 3.0, "mm", 0.0, 1000.0, "judgment", "tracks shorter than this get no corridor info"),
        ],
    },
    Code {
        code: LAYER_CHANGE,
        class: Class::Practical,
        severity: Severity::Warning,
        worst: Some((Class::Critical, Severity::Error)),
        needs: "geometry and planes; intent: each net's edge-rate tier (driver or edge_ns) or role",
        section: "power-ground#return-paths",
        about: "a via of a fast net changes reference planes with no via of the plane's net (or capacitor between two planes' nets) near it; critical and an error at T2 and faster",
        principle: "When a signal changes planes its return current has to change planes too, and without a via or capacitor right beside it, it detours through the board.",
        params: &[
            param("reach", 5.0, "mm", 0.1, 100.0, "TI SPRAAR7J: 200 mil", "a return via or capacitor this close to the signal via"),
            param("reach_ghz", 2.0, "mm", 0.1, 100.0, "TI SPRAAR7J", "the same above 1 GHz, or at T2 and faster"),
            param("probe", 1.0, "mm", 0.1, 10.0, "judgment", "how far along each track from the via its reference is looked for"),
        ],
    },
    Code {
        code: STITCHING,
        class: Class::Practical,
        severity: Severity::Warning,
        worst: None,
        needs: "geometry and planes; intent: the ground net, the board's f_concern_mhz or radio bands",
        section: "power-ground#stitching",
        about: "where a ground net's planes overlap, a via-free circle wider than λg/10 at the frequency of concern; an outer ground island with no via to an inner plane; stitching along the edge only (info)",
        principle: "Planes of one ground tied by vias closer than a tenth of a wavelength stay one conductor at the frequencies of concern instead of a cavity that resonates.",
        params: &[
            param("fraction", 0.1, "λg", 0.01, 1.0, "LearnEMC", "the largest via-free circle across, as a share of λg at the frequency of concern"),
            param("grid", 1.0, "mm", 0.2, 10.0, "judgment", "the lattice the overlap is looked at on"),
            param("island_min", 4.0, "mm²", 0.0, 10000.0, "judgment", "outer ground islands smaller than this are not checked"),
            param(
                "perimeter",
                2.0,
                "mm",
                0.0,
                50.0,
                "LearnEMC: perimeter cages are usually unnecessary",
                "a via this close to the board's edge is a perimeter via",
            ),
        ],
    },
];

pub(super) const FAMILY: Family = Family { module: "pi_return", title: "return paths", codes: CODES, run };

fn run(cx: &mut Cx) {
    if cx.wants(RETURN_PATH) {
        return_paths(cx);
    }
    if cx.wants(LAYER_CHANGE) {
        layer_changes(cx);
    }
    if cx.wants(STITCHING) {
        stitching(cx);
    }
}

/// The net name, as findings give it.
fn net_name(cx: &Cx, n: usize) -> String {
    cx.nets.name(n).unwrap_or("?").to_string()
}

/// Whether a net's tracks and vias are checked for their return path: a T1-or-faster net (or a
/// clock, RF line or switch node: their tier is T1 at least) that is no supply. A net with no
/// tier is listed once as not applicable.
fn checked(cx: &mut Cx, code: &'static str, info: &NetInfo, listed: &mut BTreeSet<usize>) -> bool {
    if info.is_supply() {
        return false;
    }
    match info.tier {
        None => {
            if listed.insert(info.index) {
                cx.not_applicable(code, info.needs_edge());
            }
            false
        }
        Some(t) => t >= Tier::T1,
    }
}

/// A point along a track, and the track's direction there.
struct Sample {
    at: [f64; 2],
    dir: [f64; 2],
}

/// How many points [`samples`] gives for a track: what it costs before it is made.
fn sample_count(points: &[[f64; 2]], curves: &[f64], step: f64) -> usize {
    track_pieces(points, curves)
        .iter()
        .map(|(a, b)| ((distance(*a, *b) / step).ceil() as usize).saturating_add(1).min(super::geom::MAX_SAMPLES))
        .fold(0, usize::saturating_add)
}

/// Points along a track's pieces at most `step` apart.
fn samples(points: &[[f64; 2]], curves: &[f64], step: f64) -> Vec<Sample> {
    let mut out: Vec<Sample> = Vec::new();
    for (a, b) in track_pieces(points, curves) {
        let len = distance(a, b);
        if len <= 1e-9 {
            continue;
        }
        let dir = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        // A piece starts where the one before ended.
        let skip = usize::from(!out.is_empty());
        out.extend(along(a, b, step).into_iter().skip(skip).map(|at| Sample { at, dir }));
    }
    out
}

/// What a point over the reference is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Over {
    /// On a plane of this net.
    On(usize),
    Off,
    /// Over the net's own antipad: counts neither way.
    Own,
}

fn overs(u: &Under) -> Vec<Over> {
    u.nets
        .iter()
        .enumerate()
        .map(|(k, n)| {
            if u.skip.get(k).copied().unwrap_or(false) {
                return Over::Own;
            }
            if let Some(n) = n {
                return Over::On(*n);
            }
            // A plane of the reference's net on the other side carries the return past the gap.
            let bridged = u.reference.net.is_some_and(|r| u.others.iter().any(|(_, at)| at.get(k).copied().flatten() == Some(r)));
            if bridged { u.reference.net.map_or(Over::Off, Over::On) } else { Over::Off }
        })
        .collect()
}

/// A gap a track crosses.
struct Crossing {
    at: [f64; 2],
    /// What kind, in words.
    what: String,
    /// The other net's track that cuts it, when one does.
    cut_by: Option<(u64, usize)>,
    /// An end of the track off the plane, not a gap it crosses.
    edge: bool,
}

fn return_paths(cx: &mut Cx) {
    let (board, nets, geo, roles) = (cx.board, cx.nets, cx.geo, cx.roles);
    let p = |name: &str| super::param_value(board, RETURN_PATH, name);
    let (step, edge_run, slack, cap_reach) = (p("step").max(0.01), p("edge_run"), p("antipad_slack"), p("cap_reach"));
    let (corridor, coverage_min, min_length) = (p("corridor"), p("coverage_min"), p("min_length"));
    let mut listed = BTreeSet::new();
    let mut no_plane: BTreeSet<CopperLayer> = BTreeSet::new();
    let mut cuts = Cuts::default();
    for (ti, t) in board.tracks.iter().enumerate() {
        let Some(net) = nets.track_nets.get(ti).copied().flatten() else { continue };
        let Some(info) = roles.net(net) else { continue };
        if !checked(cx, RETURN_PATH, info, &mut listed) {
            continue;
        }
        // Charged before the points are made, so a hostile track costs no memory past the budget.
        if !cx.spend(sample_count(&t.points, &t.curves, step).saturating_mul(4)) {
            return;
        }
        let pts = samples(&t.points, &t.curves, step);
        if pts.len() < 2 {
            continue;
        }
        let points: Vec<[f64; 2]> = pts.iter().map(|s| s.at).collect();
        let Some(under) = geo.under(t.layer, &points, Some(net), slack) else {
            if no_plane.insert(t.layer) {
                cx.not_applicable(RETURN_PATH, format!("geometry: a plane (pour) on a layer next to {}", t.layer.name()));
            }
            continue;
        };
        let r = under.reference;
        let over = overs(&under);
        let mut crossings = crossings(cx, &mut cuts, &points, &over, net, r.layer, edge_run, slack, cap_reach);
        // Between two planes, a gap in the other one counts too, unless the reference's net
        // bridges it there (see [`stripline_crossings`]).
        crossings.extend(stripline_crossings(cx, &mut cuts, t.layer, &points, &under, net, edge_run, slack, cap_reach));
        let ref_net = r.net.map(|n| net_name(cx, n)).unwrap_or_default();
        let pours: Vec<String> = geo
            .plane(r.layer)
            .map(|pl| {
                pl.fills.iter().filter(|(_, n)| Some(*n) == r.net).filter_map(|(f, _)| nets.fills.get(*f)).map(|f| format!("z{}", f.pour)).collect()
            })
            .unwrap_or_default();
        let mut keys = vec![DrcItem::Track(t.id).key()];
        keys.extend(pours);
        let regime = format!("{}; reference {} ({ref_net}), h {} mm", info.regime(), r.layer.name(), num(r.h));
        let name = info.name.to_string();
        if let Some(first) = crossings.first() {
            let n = crossings.len();
            let mut items = vec![DrcItem::Track(t.id)];
            let mut net_list = vec![name.clone()];
            if let Some((cut, cut_net)) = first.cut_by {
                items.push(DrcItem::Track(cut));
                net_list.push(net_name(cx, cut_net));
            }
            let cutter = first.cut_by.map(|(id, n)| (id, net_name(cx, n)));
            let (at, what) = (first.at, first.what.clone());
            // The others, where and what, so each can be found and fixed.
            let others: Vec<String> =
                crossings.iter().skip(1).take(MAX_CROSSINGS).map(|c| format!("{} at ({}, {})", c.what, num(c.at[0]), num(c.at[1]))).collect();
            for c in crossings.iter().skip(1) {
                if let Some((cut, cut_net)) = c.cut_by
                    && !items.contains(&DrcItem::Track(cut))
                {
                    items.push(DrcItem::Track(cut));
                    let cn = net_name(cx, cut_net);
                    if !net_list.contains(&cn) {
                        net_list.push(cn);
                    }
                }
            }
            let more = n.saturating_sub(1).saturating_sub(others.len());
            let layer = t.layer;
            let assumed = info.assumed();
            cx.add(RETURN_PATH, &keys, || {
                let mut rest = if others.is_empty() { String::new() } else { format!("; then {}", others.join("; ")) };
                if more > 0 {
                    rest.push_str(&format!("; and {more} more"));
                }
                let mut d = Draft::new(
                    at,
                    items,
                    format!(
                        "track {} ({name}, {}) crosses {n} gap{} in its reference plane on {} ({ref_net}): {what} at ({}, {}){rest}",
                        t.id,
                        layer.name(),
                        if n == 1 { "" } else { "s" },
                        r.layer.name(),
                        num(at[0]),
                        num(at[1])
                    ),
                )
                .arg("others", others.join("; "))
                .variant(if others.is_empty() { "one" } else { "more" })
                .layer(layer)
                .nets(net_list)
                .measure(n as f64, 0.0, "crossings", Bound::Max)
                .param("")
                .regime(regime)
                .arg("what", what)
                .arg("net", name.clone())
                .arg("reference", r.layer.name())
                .fix(
                    "ecad.board.net.inspect",
                    json!({"nets": [name], "metrics": ["reference"]}),
                    format!("see where {name}'s reference breaks; route it over solid plane, or move what cuts the plane"),
                );
                if assumed {
                    // Its tier is the assumed standard drive: a warning until the net's driver
                    // or edge_ns is declared (a declared slow role exempts it).
                    d = d.severity(Severity::Warning);
                }
                if let Some((id, cut_net)) = cutter {
                    d = d.fix(
                        "ecad.board.net.inspect",
                        json!({"nets": [cut_net], "metrics": ["layers"]}),
                        format!("track {id} of {cut_net} on {} cuts the plane there: move it to a signal layer", r.layer.name()),
                    );
                }
                d
            });
            continue;
        }
        // No crossing: how much of the ±3h corridor is on the plane.
        let length = path::length(&t.points, &t.curves, false);
        if length < min_length || r.h <= 0.0 {
            continue;
        }
        let half = corridor * r.h;
        let every = ((r.h / step).round() as usize).max(1);
        let mut lateral: Vec<[f64; 2]> = Vec::new();
        for s in pts.iter().step_by(every) {
            let normal = [-s.dir[1], s.dir[0]];
            for k in [-1.0, -0.5, 0.5, 1.0] {
                lateral.push([s.at[0] + normal[0] * half * k, s.at[1] + normal[1] * half * k]);
            }
        }
        if !cx.spend(lateral.len().saturating_mul(2)) {
            return;
        }
        let Some(plane) = geo.plane(r.layer) else { continue };
        let at = plane.nets_at(nets, &lateral);
        let own = geo.own_antipads(r.layer, &lateral, Some(net), slack);
        let counted = own.iter().filter(|s| !**s).count();
        // On the reference plane's own net: another net's island beside it is no return path.
        let on = at.iter().zip(&own).filter(|(n, s)| !**s && n.is_some() && **n == r.net && **n != Some(net)).count();
        if counted == 0 {
            continue;
        }
        let coverage = on as f64 / counted as f64;
        if coverage >= coverage_min {
            continue;
        }
        let mid = pts.get(pts.len() / 2).map_or(t.points.first().copied().unwrap_or([0.0, 0.0]), |s| s.at);
        let layer = t.layer;
        // Its own key, so approving the info approves no later crossing error of the track.
        keys.push("coverage".into());
        cx.add(RETURN_PATH, &keys, || {
            Draft::new(
                mid,
                vec![DrcItem::Track(t.id)],
                format!(
                    "{} % of the ±{}h corridor under track {} ({name}, {}) is on its reference plane on {} ({ref_net}): the return spreads round voids",
                    num((coverage * 100.0).round()),
                    num(corridor),
                    t.id,
                    layer.name(),
                    r.layer.name()
                ),
            )
            .layer(layer)
            .nets(vec![name.clone()])
            .severity(Severity::Info)
            .variant("coverage")
            .arg("net", name.clone())
            .arg("percent", num((coverage * 100.0).round()))
            .arg("reference", r.layer.name())
            .measure(coverage, coverage_min, "ratio", Bound::Min)
            .param("coverage_min")
            .regime(regime)
            .fix("ecad.board.net.inspect", json!({"nets": [name], "metrics": ["reference"]}), "see which segments run over voids; keep fast tracks away from via fields and plane edges")
        });
    }
}

/// The gaps the points cross (see the module notes).
#[allow(clippy::too_many_arguments)]
fn crossings(
    cx: &mut Cx,
    cuts: &mut Cuts,
    points: &[[f64; 2]],
    over: &[Over],
    own: usize,
    layer: CopperLayer,
    edge_run: f64,
    slack: f64,
    cap_reach: f64,
) -> Vec<Crossing> {
    let (nets, geo) = (cx.nets, cx.geo);
    let mut out = Vec::new();
    let Some(plane) = geo.plane(layer) else { return out };
    let n = over.len();
    let mut i = 0usize;
    while i < n {
        if over.get(i) != Some(&Over::Off) {
            i += 1;
            continue;
        }
        let mut j = i;
        while over.get(j + 1) == Some(&Over::Off) {
            j += 1;
        }
        let before = over.get(..i).and_then(|s| s.iter().rev().find_map(|o| if let Over::On(n) = o { Some(*n) } else { None }));
        let after = over.get(j + 1..).and_then(|s| s.iter().find_map(|o| if let Over::On(n) = o { Some(*n) } else { None }));
        let run: f64 = points.get(i..=j).map_or(0.0, |s| s.windows(2).map(|w| w.first().zip(w.get(1)).map_or(0.0, |(a, b)| distance(*a, *b))).sum());
        let mid = points.get((i + j) / 2).copied().unwrap_or([0.0, 0.0]);
        let next = j + 1;
        if cx.spent() {
            break;
        }
        match (before, after) {
            (Some(a), Some(b)) => {
                let crossing = match plane.void_at(mid) {
                    Void::Hole(h) => {
                        let clearance = plane.clearance(nets, h.fill);
                        let foreign: Vec<_> = geo
                            .piercers(layer)
                            .map(|p| p.near(h.bbox).filter(|q| q.net != Some(own) && h.holds(q.at)).collect::<Vec<_>>())
                            .unwrap_or_default();
                        // One via's or pad's own antipad: the return goes round it.
                        let lone = foreign.len() == 1 && foreign.first().is_some_and(|q| h.extent() <= 2.0 * (q.radius + clearance + slack));
                        let cut = if lone { None } else { cut_by(cx, cuts, h, layer, own, a) };
                        if lone {
                            None
                        } else if foreign.len() >= 2 && cut.is_none() {
                            Some(Crossing { at: mid, what: format!("merged antipads of {} vias or pads", foreign.len()), cut_by: None, edge: false })
                        } else {
                            let what = match cut {
                                Some((id, net)) => format!("a slot that track {id} ({}) cuts", net_name(cx, net)),
                                None => format!("a void {} mm across", num(h.extent())),
                            };
                            Some(Crossing { at: mid, what, cut_by: cut, edge: false })
                        }
                    }
                    Void::Outside if a != b => {
                        if bridging_cap(cx, a, b, mid, cap_reach) {
                            None
                        } else {
                            Some(Crossing {
                                at: mid,
                                what: format!("a split between {} and {}", net_name(cx, a), net_name(cx, b)),
                                cut_by: None,
                                edge: false,
                            })
                        }
                    }
                    Void::Outside => Some(Crossing { at: mid, what: "a gap between two parts of the plane".into(), cut_by: None, edge: false }),
                };
                out.extend(crossing);
            }
            // An end of the track off the plane: past its edge when that long.
            _ if run >= edge_run && (before.is_some() || after.is_some()) => {
                out.push(Crossing { at: mid, what: format!("the plane's edge ({} mm of track off it)", num(run)), cut_by: None, edge: true });
            }
            _ => {}
        }
        i = next;
    }
    out
}

/// The gaps a track between two planes (a stripline) crosses in the plane that isn't its
/// reference, when that plane is within twice the nearer one's dielectric and under most of the
/// track: part of the return flows there too, so a gap in it makes that part detour, unless a
/// plane of the same net on the reference layer bridges it.
#[allow(clippy::too_many_arguments)]
fn stripline_crossings(
    cx: &mut Cx,
    cuts: &mut Cuts,
    layer: CopperLayer,
    points: &[[f64; 2]],
    under: &Under,
    own: usize,
    edge_run: f64,
    slack: f64,
    cap_reach: f64,
) -> Vec<Crossing> {
    let geo = cx.geo;
    let r = under.reference;
    let mut out = Vec::new();
    for (l, at) in &under.others {
        let Some(h) = geo.neighbours(layer).into_iter().find(|(x, _)| x == l).map(|(_, h)| h) else { continue };
        if !(h > 0.0 && r.h > 0.0 && h.max(r.h) <= 2.0 * h.min(r.h) + 1e-9) {
            continue;
        }
        let skip = geo.own_antipads(*l, points, Some(own), slack);
        let mut by_net: BTreeMap<usize, usize> = BTreeMap::new();
        let mut counted = 0usize;
        for (n, s) in at.iter().zip(&skip) {
            if *s {
                continue;
            }
            counted += 1;
            if let Some(n) = n {
                *by_net.entry(*n).or_default() += 1;
            }
        }
        let on: usize = by_net.values().sum();
        let Some(plane_net) = by_net.iter().max_by_key(|(_, c)| **c).map(|(n, _)| *n) else { continue };
        if counted == 0 || (on as f64) < 0.5 * counted as f64 {
            continue;
        }
        let over: Vec<Over> = at
            .iter()
            .enumerate()
            .map(|(k, n)| match n {
                _ if skip.get(k).copied().unwrap_or(false) => Over::Own,
                Some(n) => Over::On(*n),
                None if under.nets.get(k).copied().flatten() == Some(plane_net) => Over::On(plane_net),
                None => Over::Off,
            })
            .collect();
        let name = net_name(cx, plane_net);
        // Where the track runs past the other plane's edge, its reference still carries the
        // return: only gaps the other plane has under the track count.
        for mut c in crossings(cx, cuts, points, &over, own, *l, edge_run, slack, cap_reach).into_iter().filter(|c| !c.edge) {
            c.what = format!("in the other plane, {name} on {}: {}", l.name(), c.what);
            out.push(c);
        }
    }
    out
}

/// The part of the segment `a`–`b` inside `bbox`, if any.
fn clip(a: [f64; 2], b: [f64; 2], [[x0, y0], [x1, y1]]: [[f64; 2]; 2]) -> Option<([f64; 2], [f64; 2])> {
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    let d = [b[0] - a[0], b[1] - a[1]];
    for (p, q) in [(-d[0], a[0] - x0), (d[0], x1 - a[0]), (-d[1], a[1] - y0), (d[1], y1 - a[1])] {
        if p.abs() <= 1e-12 {
            if q < 0.0 {
                return None;
            }
        } else if p < 0.0 {
            t0 = t0.max(q / p);
        } else {
            t1 = t1.min(q / p);
        }
    }
    if t0 > t1 {
        return None;
    }
    Some(([a[0] + d[0] * t0, a[1] + d[1] * t0], [a[0] + d[0] * t1, a[1] + d[1] * t1]))
}

/// The tracks that run through the holes looked at (each hole once), with a grid of each
/// layer's tracks made the first time the layer is asked.
#[derive(Default)]
struct Cuts {
    grids: BTreeMap<CopperLayer, Grid>,
    /// By the hole's fill and box: the tracks through it, with their nets.
    holes: BTreeMap<(usize, [u64; 4]), Vec<(u64, usize)>>,
}

/// The track of another net than `own` and the plane's `plane` on `layer` that runs through the
/// hole `h` (cuts the slot), if one does; every track looked at is charged to the budget, and a
/// hole is looked at once.
fn cut_by(cx: &mut Cx, cuts: &mut Cuts, h: &super::geom::Hole, layer: CopperLayer, own: usize, plane: usize) -> Option<(u64, usize)> {
    let (board, nets) = (cx.board, cx.nets);
    let [[x0, y0], [x1, y1]] = h.bbox;
    let key = (h.fill, [x0.to_bits(), y0.to_bits(), x1.to_bits(), y1.to_bits()]);
    if !cuts.holes.contains_key(&key) {
        if let std::collections::btree_map::Entry::Vacant(slot) = cuts.grids.entry(layer) {
            if !cx.spend(board.tracks.len().saturating_add(1)) {
                return None;
            }
            let mut grid = Grid::new(CUT_CELL);
            for (ti, t) in board.tracks.iter().enumerate().filter(|(_, t)| t.layer == layer) {
                if let Some(bbox) = path::bbox(&t.points, &t.curves, false) {
                    grid.insert(ti, bbox);
                }
            }
            slot.insert(grid);
        }
        let near = cuts.grids.get(&layer).map(|g| g.near_box(h.bbox, board.tracks.len())).unwrap_or_default();
        if !cx.spend(near.len().saturating_add(1)) {
            return None;
        }
        let mut through = Vec::new();
        for ti in near {
            let Some(t) = board.tracks.get(ti).filter(|t| t.layer == layer) else { continue };
            let Some(net) = nets.track_nets.get(ti).copied().flatten() else { continue };
            let Some(bbox) = path::bbox(&t.points, &t.curves, false) else { continue };
            if !super::super::grid::boxes_meet(bbox, h.bbox) {
                continue;
            }
            // Only the pieces' parts over the hole's box are looked at, 0.05 mm apart.
            let parts: Vec<([f64; 2], [f64; 2])> = track_pieces(&t.points, &t.curves).into_iter().filter_map(|(a, b)| clip(a, b, h.bbox)).collect();
            let cost = parts.iter().map(|(a, b)| ((distance(*a, *b) / 0.05).ceil() as usize).saturating_add(1)).fold(1, usize::saturating_add);
            if !cx.spend(cost) {
                return None;
            }
            if parts.iter().any(|(a, b)| along(*a, *b, 0.05).iter().any(|p| h.holds(*p))) {
                through.push((t.id, net));
            }
        }
        cuts.holes.insert(key, through);
    }
    cuts.holes.get(&key)?.iter().copied().find(|(_, n)| *n != own && *n != plane)
}

/// Whether a two-pad part (a stitching capacitor) between the nets `a` and `b` has both pads
/// within `reach` of `at`.
fn bridging_cap(cx: &mut Cx, a: usize, b: usize, at: [f64; 2], reach: f64) -> bool {
    if !cx.spend(cx.nets.pads.len().saturating_add(1)) {
        return false;
    }
    let mut by_part: BTreeMap<u64, Vec<(Option<usize>, [f64; 2])>> = BTreeMap::new();
    for p in &cx.nets.pads {
        if (p.net == Some(a) || p.net == Some(b)) && distance(p.at(), at) <= reach + 5.0 {
            by_part.entry(p.footprint).or_default().push((p.net, p.at()));
        }
    }
    by_part.values().any(|pads| {
        pads.len() == 2
            && pads.iter().any(|(n, _)| *n == Some(a))
            && pads.iter().any(|(n, _)| *n == Some(b))
            && pads.iter().all(|(_, q)| distance(*q, at) <= reach)
    })
}

/// A track's point `probe` along it from its end at `at` (`None` when no end of it is there).
fn leg(points: &[[f64; 2]], curves: &[f64], at: [f64; 2], reach: f64, probe: f64) -> Option<Vec<[f64; 2]>> {
    let mut line = path::tessellate(points, curves, false, 0.01);
    let (first, last) = (line.first().copied()?, line.last().copied()?);
    if distance(last, at) <= reach && distance(first, at) > reach {
        line.reverse();
    } else if distance(first, at) > reach {
        return None;
    }
    // Points from just past the via's copper out to `probe`.
    let mut out = Vec::new();
    let mut walked = 0.0;
    for w in line.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
        let len = distance(*a, *b);
        for p in along(*a, *b, 0.1) {
            let d = walked + distance(*a, p);
            if d > reach && d <= probe.max(reach) + 1e-9 {
                out.push(p);
            }
        }
        walked += len;
        if walked > probe {
            break;
        }
    }
    if out.is_empty() {
        out.push(line.get(1).copied().unwrap_or(first));
    }
    Some(out)
}

fn layer_changes(cx: &mut Cx) {
    let (board, nets, geo, roles) = (cx.board, cx.nets, cx.geo, cx.roles);
    let p = |name: &str| super::param_value(board, LAYER_CHANGE, name);
    let (reach, reach_ghz, probe) = (p("reach"), p("reach_ghz"), p("probe"));
    let slack = super::param_value(board, RETURN_PATH, "antipad_slack");
    let layers = board.copper_layers;
    let mut listed = BTreeSet::new();
    // Each net's tracks, by board index.
    let mut tracks_of: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (ti, n) in nets.track_nets.iter().enumerate() {
        if let Some(n) = n {
            tracks_of.entry(*n).or_default().push(ti);
        }
    }
    // Each net's pads that are on one side only (a pad a via sits in), by board index.
    let mut pads_of: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, p) in nets.pads.iter().enumerate() {
        if let Some(n) = p.net.filter(|_| !(p.top && p.bottom)) {
            pads_of.entry(n).or_default().push(i);
        }
    }
    // Each plane net's return paths between layers: its vias (by index) and its through-hole
    // pads, in a grid, made the first time the net is asked.
    let mut returns: BTreeMap<usize, (Vec<([f64; 2], Option<usize>)>, Grid)> = BTreeMap::new();
    // Two-pad parts by their pads' nets, for stitching capacitors.
    let mut two_pad: BTreeMap<u64, Vec<(Option<usize>, [f64; 2])>> = BTreeMap::new();
    for p in nets.pads.iter().filter(|p| p.top || p.bottom) {
        two_pad.entry(p.footprint).or_default().push((p.net, p.at()));
    }
    two_pad.retain(|_, v| v.len() == 2);
    for (vi, v) in board.vias.iter().enumerate() {
        if v.role == ViaRole::Stitch {
            continue;
        }
        let Some(net) = nets.via_nets.get(vi).copied().flatten() else { continue };
        let Some(info) = roles.net(net) else { continue };
        if info.has_role(NetRole::Rf) || !checked(cx, LAYER_CHANGE, info, &mut listed) {
            continue;
        }
        if !cx.spend(64) {
            return;
        }
        let r = v.diameter / 2.0 + 0.01;
        // The layers the net leaves the via on, each with points along its track there: only
        // tracks whose box reaches the via are followed.
        let mine = tracks_of.get(&net).map_or(&[][..], Vec::as_slice);
        let own_pads = pads_of.get(&net).map_or(&[][..], Vec::as_slice);
        if !cx.spend(mine.len().saturating_add(own_pads.len())) {
            return;
        }
        let mut legs: Vec<(CopperLayer, Vec<[f64; 2]>)> = Vec::new();
        for ti in mine {
            let Some(t) = board.tracks.get(*ti) else { continue };
            if !v.on(t.layer, layers) {
                continue;
            }
            let near =
                path::bbox(&t.points, &t.curves, false).is_some_and(|b| super::super::grid::boxes_meet(super::super::grid::grow(b, r), [v.at, v.at]));
            if !near {
                continue;
            }
            if !cx.spend(t.points.len().saturating_mul(16).saturating_add(probe.max(0.0).ceil() as usize * 10)) {
                return;
            }
            if let Some(pts) = leg(&t.points, &t.curves, v.at, r, probe) {
                legs.push((t.layer, pts));
            }
        }
        for p in own_pads.iter().filter_map(|i| nets.pads.get(*i)).filter(|p| p.contains(v.at)) {
            let layer = if p.top { CopperLayer::Top } else { CopperLayer::Bottom };
            if v.on(layer, layers) {
                legs.push((layer, vec![p.at(), v.at]));
            }
        }
        let distinct: BTreeSet<CopperLayer> = legs.iter().map(|(l, _)| *l).collect();
        if distinct.len() < 2 {
            continue;
        }
        let mut refs: BTreeMap<CopperLayer, (usize, CopperLayer)> = BTreeMap::new();
        for (l, pts) in &legs {
            if refs.contains_key(l) {
                continue;
            }
            if !cx.spend(pts.len().saturating_mul(8).saturating_add(1)) {
                return;
            }
            if let Some(u) = geo.under(*l, pts, Some(net), slack)
                && let Some(n) = u.reference.net
            {
                refs.insert(*l, (n, u.reference.layer));
            }
        }
        let planes: BTreeSet<(CopperLayer, usize)> = refs.values().map(|(n, l)| (*l, *n)).collect();
        if planes.len() < 2 {
            continue;
        }
        let fast = info.tier.is_some_and(|t| t >= Tier::T2);
        let ghz = fast || info.f_mhz().is_some_and(|f| f > 1000.0);
        let limit = if ghz { reach_ghz } else { reach };
        let plane_nets: BTreeSet<usize> = planes.iter().map(|(_, n)| *n).collect();
        let route: Vec<String> = refs.iter().map(|(l, (n, rl))| format!("{} over {} ({})", l.name(), rl.name(), net_name(cx, *n))).collect();
        let name = info.name.to_string();
        let regime = format!("{}; {}", info.regime(), route.join(" → "));
        let (class, severity) = if fast { (Class::Critical, Severity::Error) } else { (Class::Practical, Severity::Warning) };
        if plane_nets.len() == 1 {
            let Some(g) = plane_nets.first().copied() else { continue };
            let want: Vec<CopperLayer> = planes.iter().map(|(l, _)| *l).collect();
            // A via of the plane's net joining both planes, or a through-hole pad of it: looked
            // up in a grid of the net's, so a via costs what is near it.
            if let std::collections::btree_map::Entry::Vacant(slot) = returns.entry(g) {
                if !cx.spend(board.vias.len().saturating_add(nets.pads.len())) {
                    return;
                }
                let mut list: Vec<([f64; 2], Option<usize>)> = Vec::new();
                for (k, w) in board.vias.iter().enumerate() {
                    if nets.via_nets.get(k).copied().flatten() == Some(g) {
                        list.push((w.at, Some(k)));
                    }
                }
                list.extend(nets.pads.iter().filter(|p| p.net == Some(g) && p.top && p.bottom && p.pad.drill.is_some()).map(|p| (p.at(), None)));
                let mut grid = Grid::new(RETURN_CELL);
                for (i, (at, _)) in list.iter().enumerate() {
                    grid.insert(i, [*at, *at]);
                }
                slot.insert((list, grid));
            }
            let Some((list, grid)) = returns.get(&g) else { continue };
            // Within the limit first; past it, the nearest anywhere for the message.
            let near = grid.near_box(super::super::grid::grow([v.at, v.at], limit), list.len());
            if !cx.spend(near.len().saturating_add(1)) {
                return;
            }
            let joins = |(at, k): &([f64; 2], Option<usize>)| match k {
                Some(k) => board.vias.get(*k).is_some_and(|w| want.iter().all(|l| w.on(*l, layers))),
                None => at[0].is_finite(),
            };
            let mut nearest = near.iter().filter_map(|i| list.get(*i)).filter(|x| joins(x)).map(|(q, _)| distance(*q, v.at)).min_by(f64::total_cmp);
            if nearest.is_none_or(|d| d > limit) {
                if !cx.spend(list.len().saturating_add(1)) {
                    return;
                }
                nearest = list.iter().filter(|x| joins(x)).map(|(q, _)| distance(*q, v.at)).min_by(f64::total_cmp);
            }
            if nearest.is_some_and(|d| d <= limit) {
                continue;
            }
            let g_name = net_name(cx, g);
            let joins = want.iter().map(|l| l.name()).collect::<Vec<_>>().join(" and ");
            cx.add(LAYER_CHANGE, &[DrcItem::Via(v.id).key()], || {
                let far = nearest.map_or("none is on the board".to_string(), |d| format!("the nearest is {} mm away", num(d)));
                let mut d = Draft::new(
                    v.at,
                    vec![DrcItem::Via(v.id)],
                    format!("via {} ({name}) changes its reference between {g_name} planes on {joins}, and no {g_name} via joins them within {} mm: {far}", v.id, num(limit)),
                )
                .nets(vec![name.clone(), g_name.clone()])
                .arg("plane", g_name.clone())
                .arg("layers", joins.clone())
                .arg("reach", num(limit))
                .class(class)
                .severity(severity)
                .regime(regime)
                .param(if ghz { "reach_ghz" } else { "reach" })
                .fix(
                    "ecad.board.stitch",
                    json!({"net": g_name, "pattern": "layer_change", "nets": [name]}),
                    format!("place a {g_name} via joining {joins} beside via {} (the layer-change pattern puts one within 2 mm)", v.id),
                );
                if let Some(dist) = nearest {
                    d = d.measure(dist, limit, "mm", Bound::Max);
                }
                d
            });
        } else {
            let list: Vec<usize> = plane_nets.iter().copied().collect();
            for (k, a) in list.iter().enumerate() {
                for b in list.iter().skip(k + 1) {
                    let nearest = two_pad
                        .values()
                        .filter(|pads| pads.iter().any(|(n, _)| *n == Some(*a)) && pads.iter().any(|(n, _)| *n == Some(*b)))
                        .map(|pads| pads.iter().map(|(_, q)| distance(*q, v.at)).fold(0.0, f64::max))
                        .min_by(f64::total_cmp);
                    if nearest.is_some_and(|d| d <= limit) {
                        continue;
                    }
                    let (an, bn) = (net_name(cx, *a), net_name(cx, *b));
                    let keys = [DrcItem::Via(v.id).key(), format!("{an}/{bn}")];
                    let regime = regime.clone();
                    let name = name.clone();
                    cx.add(LAYER_CHANGE, &keys, || {
                        let far = nearest.map_or("none is on the board".to_string(), |d| format!("the nearest is {} mm away", num(d)));
                        let mut d = Draft::new(
                            v.at,
                            vec![DrcItem::Via(v.id)],
                            format!("via {} ({name}) moves its return from the {an} plane to the {bn} plane, and no capacitor between them is within {} mm: {far}", v.id, num(limit)),
                        )
                        .nets(vec![name.clone(), an.clone(), bn.clone()])
                        .variant("capacitor")
                        .arg("from", an.clone())
                        .arg("to", bn.clone())
                        .arg("reach", num(limit))
                        .class(class)
                        .severity(severity)
                        .regime(regime)
                        .param(if ghz { "reach_ghz" } else { "reach" })
                        .fix(
                            "ecad.board.move",
                            Value::Null,
                            format!("move a capacitor between {an} and {bn} (1 µF or less) to within {} mm of via {}, or route {name} without changing reference planes", num(limit), v.id),
                        );
                        if let Some(dist) = nearest {
                            d = d.measure(dist, limit, "mm", Bound::Max);
                        }
                        d
                    });
                }
            }
        }
    }
}

/// The most lattice points the stitching check looks at, per ground net.
const MAX_LATTICE: usize = 250_000;

fn stitching(cx: &mut Cx) {
    let (board, nets, geo, roles) = (cx.board, cx.nets, cx.geo, cx.roles);
    let p = |name: &str| super::param_value(board, STITCHING, name);
    let (fraction, grid, island_min, perimeter) = (p("fraction"), p("grid").max(0.2), p("island_min"), p("perimeter"));
    if roles.grounds.is_empty() {
        cx.not_applicable(STITCHING, "intent: the ground net (a net record with role ground)");
        return;
    }
    let layers = board.copper_layers;
    let fast = roles.nets.iter().any(|n| !n.is_supply() && n.fast());
    let keepouts: Vec<Vec<[f64; 2]>> = board.keepouts.iter().map(|k| path::tessellate(&k.outline, &k.curves, true, 0.05)).collect();
    let edge: Vec<[f64; 2]> = if board.outline.is_empty() { Vec::new() } else { path::tessellate(&board.outline, &board.outline_curves, true, 0.05) };
    for g in roles.grounds.iter().copied() {
        let g_name = net_name(cx, g);
        let ground_layers: Vec<CopperLayer> =
            geo.layers.iter().copied().filter(|l| geo.plane(*l).is_some_and(|pl| pl.fills.iter().any(|(_, n)| *n == g))).collect();
        let inner: Vec<CopperLayer> = ground_layers.iter().copied().filter(|l| l.is_inner()).collect();
        // Vias of the ground (and its through-hole pads) that join two of its plane layers.
        let mut sites: Vec<([f64; 2], Vec<CopperLayer>)> = Vec::new();
        for (k, v) in board.vias.iter().enumerate() {
            if nets.via_nets.get(k).copied().flatten() != Some(g) {
                continue;
            }
            let on: Vec<CopperLayer> = ground_layers.iter().copied().filter(|l| v.on(*l, layers)).collect();
            sites.push((v.at, on));
        }
        for pad in nets.pads.iter().filter(|q| q.net == Some(g) && q.top && q.bottom && q.pad.drill.is_some()) {
            sites.push((pad.at(), ground_layers.clone()));
        }
        // Outer islands with no via to an inner plane, at any frequency.
        if !inner.is_empty() {
            outer_islands(cx, g, &g_name, &ground_layers, &sites, island_min);
        }
        if ground_layers.len() < 2 || !fast {
            continue;
        }
        let Some((f_mhz, why)) = roles.f_concern() else {
            cx.not_applicable(STITCHING, "intent: the board's f_concern_mhz or radio bands");
            continue;
        };
        let (Some(top), Some(bottom)) = (ground_layers.first(), ground_layers.last()) else { continue };
        let er = geo.er(*top, *bottom);
        let lambda = 299_792.458 / f_mhz.max(1e-3) / er.max(1.0).sqrt();
        let limit = fraction * lambda;
        let joining: Vec<[f64; 2]> = sites.iter().filter(|(_, on)| on.len() >= 2).map(|(at, _)| *at).collect();
        let index = Sites::new(joining.clone(), 2.0);
        // The lattice over the ground's fills.
        let Some(bbox) = super::super::bbox(
            geo.layers
                .iter()
                .filter_map(|l| geo.plane(*l))
                .flat_map(|pl| pl.fills.iter().filter(|(_, n)| *n == g))
                .filter_map(|(f, _)| nets.fills.get(*f))
                .filter_map(|f| f.region.bbox())
                .flatten(),
        ) else {
            continue;
        };
        let [[x0, y0], [x1, y1]] = bbox;
        let area = ((x1 - x0) * (y1 - y0)).max(0.0);
        let step = grid.max((area / MAX_LATTICE as f64).sqrt());
        let (nx, ny) = (((x1 - x0) / step).ceil() as usize, ((y1 - y0) / step).ceil() as usize);
        let mut lattice: Vec<[f64; 2]> = Vec::new();
        for i in 0..=nx.min(MAX_LATTICE) {
            for j in 0..=ny.min(MAX_LATTICE) {
                if lattice.len() >= MAX_LATTICE {
                    break;
                }
                let q = [x0 + i as f64 * step, y0 + j as f64 * step];
                if !keepouts.iter().any(|k| inside(q, k)) {
                    lattice.push(q);
                }
            }
        }
        if !cx.spend(lattice.len().saturating_mul(ground_layers.len() + 4)) {
            return;
        }
        let mut covers = vec![0usize; lattice.len()];
        for l in &ground_layers {
            let Some(pl) = geo.plane(*l) else { continue };
            for (slot, n) in covers.iter_mut().zip(pl.nets_at(nets, &lattice)) {
                if n == Some(g) {
                    *slot += 1;
                }
            }
        }
        let far = (x1 - x0).hypot(y1 - y0);
        let mut worst: Option<([f64; 2], f64)> = None;
        for (q, c) in lattice.iter().zip(&covers) {
            if *c < 2 {
                continue;
            }
            let (near, looked) = index.nearest_counted(*q, far);
            if !cx.spend(looked) {
                return;
            }
            let r = near.map_or(far, |(_, d)| d);
            if worst.is_none_or(|(_, w)| r > w) {
                worst = Some((*q, r));
            }
        }
        let Some((at, radius)) = worst else { continue };
        let across = 2.0 * radius;
        let pours: Vec<String> =
            board.pours.iter().filter(|z| nets.fills.iter().any(|f| f.pour == z.id && f.net == Some(g))).map(|z| format!("z{}", z.id)).collect();
        let on: String = ground_layers.iter().map(|l| l.name()).collect::<Vec<_>>().join(", ");
        let regime = format!("{why}; εr {}, λg {} mm", num((er * 100.0).round() / 100.0), num(lambda.round()));
        if across > limit {
            let g_name = g_name.clone();
            let none = joining.is_empty();
            cx.add(STITCHING, &pours, || {
                let circle = if none {
                    format!("no via joins them anywhere (the overlap reaches {} mm across)", num(across.round()))
                } else {
                    format!("the largest circle with no via joining them is {} mm across at ({}, {})", num(across.round()), num(at[0]), num(at[1]))
                };
                Draft::new(
                    at,
                    Vec::new(),
                    format!(
                        "where {g_name}'s planes overlap ({on}), {circle}; λg/10 at {} MHz is {} mm",
                        num(f_mhz),
                        num((limit * 10.0).round() / 10.0)
                    ),
                )
                .nets(vec![g_name.clone()])
                .arg("layers", on.clone())
                .arg("f_mhz", num(f_mhz))
                .measure(across, limit, "mm", Bound::Max)
                .param("fraction")
                .regime(regime)
                .fix(
                    "ecad.board.stitch",
                    json!({"net": g_name, "pattern": "grid"}),
                    "add a grid of stitching vias: by default its pitch keeps every via-free circle within λg/10",
                )
            });
            continue;
        }
        // Stitching along the edge only.
        if joining.len() >= 8 && !edge.is_empty() {
            let near_edge = joining.iter().filter(|q| edge_distance(**q, &edge) <= perimeter).count();
            if near_edge as f64 >= 0.8 * joining.len() as f64 {
                let mut keys = pours.clone();
                keys.push("perimeter".into());
                cx.add(STITCHING, &keys, || {
                    Draft::new(at, Vec::new(), format!("{near_edge} of {g_name}'s {} stitching vias run along the board's edge: a perimeter cage is usually unnecessary; spread them where fast circuits are", joining.len()))
                        .nets(vec![g_name.clone()])
                        .severity(Severity::Info)
                        .variant("perimeter")
                        .arg("count", near_edge.to_string())
                        .arg("total", joining.len().to_string())
                        .review()
                        .regime(regime)
                });
            }
        }
    }
}

/// How far `p` is from the closed outline `ring`.
fn edge_distance(p: [f64; 2], ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    (0..n)
        .filter_map(|i| Some((*ring.get(i)?, *ring.get((i + 1) % n)?)))
        .map(|(a, b)| path::segment_distance(p, a, b, 0.0))
        .fold(f64::INFINITY, f64::min)
}

/// Outer-layer islands of the ground `g` with no via (or through-hole pad) to one of its inner
/// planes.
fn outer_islands(cx: &mut Cx, g: usize, g_name: &str, ground_layers: &[CopperLayer], sites: &[([f64; 2], Vec<CopperLayer>)], island_min: f64) {
    let (nets, geo) = (cx.nets, cx.geo);
    for outer in ground_layers.iter().copied().filter(|l| !l.is_inner()) {
        let Some(pl) = geo.plane(outer) else { continue };
        // Sites on this layer that reach an inner plane.
        let tied: Vec<[f64; 2]> = sites.iter().filter(|(_, on)| on.contains(&outer) && on.iter().any(|l| l.is_inner())).map(|(at, _)| *at).collect();
        for (f, n) in pl.fills.iter().copied() {
            if n != g {
                continue;
            }
            let Some(fill) = nets.fills.get(f) else { continue };
            if !cx.spend(tied.len().saturating_add(16)) {
                return;
            }
            let mut held: BTreeSet<usize> = BTreeSet::new();
            held.extend(fill.region.locate(&tied).into_iter().flatten());
            for (k, shape) in fill.region.shapes().iter().enumerate() {
                if held.contains(&k) || shape.area() < island_min {
                    continue;
                }
                let Some([[a, b], [c, d]]) = super::super::bbox(shape.outer.iter().copied()) else { continue };
                let centre = [(a + c) / 2.0, (b + d) / 2.0];
                let at = if inside(centre, &shape.outer) { centre } else { shape.outer.first().copied().unwrap_or(centre) };
                let keys =
                    [format!("z{}", fill.pour), format!("@{},{}", num((centre[0] * 10.0).round() / 10.0), num((centre[1] * 10.0).round() / 10.0))];
                let area = shape.area();
                let g_name = g_name.to_string();
                cx.add(STITCHING, &keys, || {
                    Draft::new(
                        at,
                        vec![DrcItem::Pour(fill.pour)],
                        format!(
                            "the {g_name} pour on {} has an island of {} mm² near ({}, {}) with no via to an inner {g_name} plane",
                            outer.name(),
                            num(area.round()),
                            num(at[0]),
                            num(at[1])
                        ),
                    )
                    .layer(outer)
                    .nets(vec![g_name.clone()])
                    .variant("island")
                    .arg("area", num(area.round()))
                    .measure(0.0, 1.0, "vias", Bound::Min)
                    .param("island_min")
                    .fix(
                        "ecad.board.stitch",
                        json!({"net": g_name, "pattern": "grid", "region": {"pour": fill.pour}}),
                        "stitch the island to the inner plane, or let the pour drop it",
                    )
                });
            }
        }
    }
}
