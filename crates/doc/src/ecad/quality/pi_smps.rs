//! Switching regulators: the hot loop, the switch node and the feedback divider, from the
//! intent's loop records and roles (see [`super::super::intent`]); what a regulator lacks of them
//! is listed as not applicable. The regulators are the kept switching regulators and the ICs a
//! kept switch node belongs to (a radio's own DC/DC).
//! - `pi.smps_hot_loop`: each kept hot loop (`buck_hot`, `boost_hot`). Errors: its current runs
//!   through a via (two members of one net next to each other in the loop, joined only through
//!   vias; not joined at all is not routed yet, the connection check's), or a capacitor of the
//!   loop on the other side from the regulator (a layout with a reason for it, a vendor's, is
//!   declared by approving the finding with that reason). Warnings: a capacitor pad farther than
//!   `cap_distance` from the regulator's pin of its net; less than `ground_min` of the loop's
//!   hull over ground on the next layer; a loop more than `ratio_max` times the vendor's when
//!   the record gives `reference_ratio`. Otherwise an info ranks it: area, length and estimated
//!   nH (μ0·h·length/width, see [`super::metrics`]). A regulator kept as a module (its
//!   capacitors inside) is left out; switch-node vias under a capacitor (a WCSP layout's) are no
//!   member of the loop, so never counted.
//! - `pi.smps_switch_node`: each kept switch node. Warnings: its copper on more than one layer
//!   (a vendor layout's switch-node vias are approved with that reason); a ground void on the next
//!   layer under it (past `void_max` of it, its own vias' antipads left out); other nets' copper
//!   inside its inductor's body (the fab outline, else the courtyard) on the inductor's layer
//!   (ground excepted: a shield), once per inductor; feedback, analog, sensitive or clock copper
//!   within `near_w` of it on the same layer (an info within `near_i`). Error: such copper
//!   overlapping it, or its inductor, on another layer with no ground plane between. Within the
//!   pad field of an IC that both nets come from, the copper is its pins' breakout, set by the
//!   pinout: left out of both.
//! - `pi.smps_feedback`: each kept feedback net. Warning: the divider's pad on it farther than
//!   `distance` from the regulator's feedback pin (along the copper when routed); the divider's
//!   top tapping the output anywhere but an output capacitor's or the regulator's own pad (a
//!   junction partway along a track, another part's pad, the output's pour: copper that carries
//!   the load current). A regulator with no divider kept is a fixed-output part: left out.
//!
//! The ground under the loop and the switch node is judged once the board has a ground pour;
//! before, it is not applicable.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use super::super::airwires::BoardPad;
use super::super::checks::{Severity, gap};
use super::super::copper::{disc, pad_core, stroke};
use super::super::drc::{Bound, Class, DrcItem};
use super::super::footprint::{Core, Side};
use super::super::graphics::BoardLayer;
use super::super::grid::{Grid, grow};
use super::super::intent::{LoopIntent, LoopKind, NetRole, PartRole};
use super::super::layers::CopperLayer;
use super::catalog::{Code, Family, Param};
use super::geom::{along, track_pieces};
use super::metrics::{LoopFilter, ground_under, loop_metrics};
use super::pi_decap::{Copper, Kind, Pins, joined_on, pad_gap, pad_item, side_of};
use super::{Cx, Draft, num};

const HOT_LOOP: &str = "pi.smps_hot_loop";
const SWITCH_NODE: &str = "pi.smps_switch_node";
const FEEDBACK: &str = "pi.smps_feedback";

const fn param(name: &'static str, default: f64, units: &'static str, min: f64, max: f64, source: &'static str, about: &'static str) -> Param {
    Param { name, default, units, source, min, max, verify: false, about }
}

const CODES: &[Code] = &[
    Code {
        code: HOT_LOOP,
        class: Class::Critical,
        severity: Severity::Error,
        worst: None,
        needs: "geometry and planes; intent: the regulator's hot loops (loop records) and its capacitors",
        section: "power-ground#switchers",
        about: "a via in a regulator's hot loop, or its capacitor on the other side without a reason (errors); a capacitor far from its pins, less than solid ground under the loop, a loop larger than the vendor's (warnings); the loops ranked by estimated nH (info)",
        principle: "The hot loop carries current that switches in nanoseconds: every nanohenry in it rings and radiates, so it stays small, on one layer, over solid ground.",
        params: &[
            param(
                "cap_distance",
                2.0,
                "mm",
                0.1,
                50.0,
                "the guide's Switchers step 2 (Hegarty, TI SLYT682)",
                "the farthest a hot-loop capacitor's pad sits from the regulator's pin of its net",
            ),
            param(
                "ground_min",
                0.95,
                "ratio",
                0.0,
                1.0,
                "judgment, as the guide's Switchers step 2",
                "the least share of the loop's hull over ground on the next layer",
            ),
            param(
                "ratio_max",
                1.5,
                "ratio",
                1.0,
                100.0,
                "judgment, as the guide's Switchers step 7",
                "the most a loop may exceed the vendor's when its record gives reference_ratio",
            ),
            param("grid", 0.1, "mm", 0.02, 2.0, "judgment", "how finely the loop's hull is looked at"),
        ],
    },
    Code {
        code: SWITCH_NODE,
        class: Class::Critical,
        severity: Severity::Warning,
        worst: Some((Class::Critical, Severity::Error)),
        needs: "geometry and planes; intent: switch-node nets, the regulator's inductor, feedback, analog, sensitive and clock nets",
        section: "power-ground#switchers",
        about: "switch-node copper on more than one layer, over a ground void, or with other nets inside its inductor's outline; sensitive copper near it on its layer (warning, info), or across it on another layer with no ground between (error)",
        principle: "The switch node swings the full input voltage in nanoseconds: kept small, on one layer and over solid ground, its field stays out of the feedback, analog and clock copper.",
        params: &[
            param("near_w", 1.0, "mm", 0.0, 50.0, "the guide's Switchers step 3", "sensitive copper closer than this on the same layer is a warning"),
            param("near_i", 3.0, "mm", 0.0, 100.0, "the guide's Switchers step 3", "sensitive copper closer than this on the same layer is an info"),
            param(
                "void_max",
                0.02,
                "ratio",
                0.0,
                1.0,
                "judgment: specks where the plane's clearance meets the copper's edge",
                "the share of the switch node over no ground on the next layer that passes",
            ),
            param("step", 0.1, "mm", 0.02, 2.0, "judgment", "how finely the switch node's copper is looked at"),
        ],
    },
    Code {
        code: FEEDBACK,
        class: Class::Critical,
        severity: Severity::Warning,
        worst: None,
        needs: "geometry; intent: the feedback net and its divider",
        section: "power-ground#switchers",
        about: "a regulator's feedback divider farther than 3 mm from its FB pin, or its top tapping the output on copper that carries the load current",
        principle: "The feedback node is high-impedance and sets the output: a long FB track picks up noise, and a tap on loaded copper regulates the wrong voltage.",
        params: &[param(
            "distance",
            3.0,
            "mm",
            0.1,
            100.0,
            "the guide's Switchers step 4",
            "the farthest the divider's pad on the FB net sits from the FB pin",
        )],
    },
];

pub(super) const FAMILY: Family = Family { module: "pi_smps", title: "regulator loops", codes: CODES, run };

fn run(cx: &mut Cx) {
    let pins = Pins::new(cx.nets, cx.conn);
    let switch_nodes_kept = cx.roles.nets.iter().filter(|n| n.has_role(NetRole::SwNode)).count();
    if !cx.spend(switch_nodes_kept.saturating_add(1).saturating_mul(cx.nets.pads.len().saturating_add(1))) {
        return;
    }
    let regs = regulators(cx);
    if cx.wants(HOT_LOOP) {
        hot_loops(cx, &pins, &regs);
    }
    if cx.wants(SWITCH_NODE) {
        switch_nodes(cx, &pins, &regs);
    }
    if cx.wants(FEEDBACK) {
        feedback(cx, &pins, &regs);
    }
}

/// The regulators kept, by designator: the switching regulators, and the ICs a kept switch node
/// belongs to (a radio's own DC/DC).
fn regulators(cx: &Cx) -> Vec<String> {
    let mut out: Vec<String> = cx.roles.parts(PartRole::SmpsIc).map(|p| p.part.clone()).collect();
    let mut pads: BTreeMap<&str, usize> = BTreeMap::new();
    for p in &cx.nets.pads {
        *pads.entry(p.designator.as_str()).or_default() += 1;
    }
    for n in cx.roles.nets.iter().filter(|n| n.has_role(NetRole::SwNode)) {
        // Its record's domain, else the part on it with the most pads.
        let ic = n.record.and_then(|r| r.domain.clone()).or_else(|| {
            cx.nets
                .pads
                .iter()
                .filter(|p| p.net == Some(n.index))
                .map(|p| p.designator.as_str())
                .max_by_key(|d| (pads.get(d).copied().unwrap_or(0), std::cmp::Reverse(*d)))
                .map(str::to_string)
        });
        if let Some(ic) = ic.filter(|d| !out.contains(d)) {
            out.push(ic);
        }
    }
    out
}

/// The regulator a net belongs to: its record's `domain`, else a kept regulator with a pad on it.
fn regulator_of(cx: &Cx, regs: &[String], net: usize) -> Option<String> {
    if let Some(d) =
        cx.roles.net(net).and_then(|n| n.record).and_then(|r| r.domain.clone()).filter(|d| cx.nets.pads.iter().any(|p| p.designator == *d))
    {
        return Some(d);
    }
    cx.nets.pads.iter().filter(|p| p.net == Some(net)).map(|p| &p.designator).find(|d| regs.contains(d)).cloned()
}

/// Whether the board has a ground pour anywhere: before it does, there is no plane to judge.
fn poured(cx: &Cx) -> bool {
    cx.nets.fills.iter().any(|f| f.net.is_some_and(|n| cx.roles.grounds.contains(&n)))
}

fn net_name(cx: &Cx, n: usize) -> String {
    cx.nets.name(n).unwrap_or("?").to_string()
}

// ---- pi.smps_hot_loop ----

/// The share of `points` over a ground plane on a layer next to `layer` (any of them).
fn ground_share(cx: &Cx, layer: CopperLayer, points: &[[f64; 2]], skip: &[bool]) -> Option<(f64, Vec<CopperLayer>)> {
    let (nets, geo, roles) = (cx.nets, cx.geo, cx.roles);
    let next: Vec<CopperLayer> = geo.neighbours(layer).into_iter().map(|(l, _)| l).collect();
    let mut covered = vec![false; points.len()];
    for l in &next {
        let Some(plane) = geo.plane(*l) else { continue };
        for (slot, n) in covered.iter_mut().zip(plane.nets_at(nets, points)) {
            if n.is_some_and(|n| roles.grounds.contains(&n)) {
                *slot = true;
            }
        }
    }
    let counted: Vec<bool> = covered.iter().zip(skip.iter().chain(std::iter::repeat(&false))).filter(|(_, s)| !**s).map(|(c, _)| *c).collect();
    if counted.is_empty() {
        return None;
    }
    Some((counted.iter().filter(|c| **c).count() as f64 / counted.len() as f64, next))
}

fn hot_loops(cx: &mut Cx, pins: &Pins, regs: &[String]) {
    let (board, nets, roles) = (cx.board, cx.nets, cx.roles);
    let p = |name: &str| super::param_value(board, HOT_LOOP, name);
    let (cap_distance, ground_min, ratio_max, grid) = (p("cap_distance"), p("ground_min"), p("ratio_max"), p("grid").max(0.02));
    let loops: Vec<&LoopIntent> = roles.intent.loops.iter().filter(|l| matches!(l.kind, LoopKind::BuckHot | LoopKind::BoostHot)).collect();
    // A regulator no hot loop has a member on.
    for r in regs {
        let prefix = format!("{r}.");
        if !loops.iter().any(|l| l.members.iter().any(|m| m.starts_with(&prefix) || m == r)) {
            cx.not_applicable(HOT_LOOP, format!("intent: the hot loops of {r} (loop records)"));
        }
    }
    if loops.is_empty() {
        return;
    }
    if !cx.spend(loops.len().saturating_mul(4096)) {
        return;
    }
    let filter = LoopFilter { kinds: Some(vec![LoopKind::BuckHot, LoopKind::BoostHot]), grid: Some(grid), ..LoopFilter::default() };
    let (metrics, _) = loop_metrics(cx.ecad, nets, &filter);
    for l in loops {
        let mut members: Vec<(String, usize)> = Vec::new();
        let mut missing: Vec<&str> = Vec::new();
        for m in &l.members {
            match pins.resolve(nets, m).first() {
                Some(i) => members.push((m.clone(), *i)),
                None => missing.push(m),
            }
        }
        if !missing.is_empty() {
            cx.not_applicable(HOT_LOOP, format!("geometry: {} on the board (loop {})", missing.join(", "), l.name));
        }
        if members.len() < 2 {
            continue;
        }
        let parts: BTreeSet<&str> = members.iter().filter_map(|(_, i)| nets.pads.get(*i)).map(|p| p.designator.as_str()).collect();
        let ic = parts
            .iter()
            .copied()
            .find(|d| roles.intent.part(d).is_some_and(|r| r.role == PartRole::SmpsIc))
            .or_else(|| parts.iter().copied().max_by_key(|d| pins.of_part(d).len()));
        let Some(ic) = ic else { continue };
        if roles.intent.part(ic).is_some_and(|r| r.role == PartRole::Module) {
            continue;
        }
        let Some(ic_pad) = members.iter().filter_map(|(_, i)| nets.pads.get(*i)).find(|p| p.designator == ic) else { continue };
        let side = side_of(ic_pad);
        let name = l.name.clone();
        // Its capacitors: the two-pad members.
        for cap in parts.iter().copied().filter(|d| *d != ic && pins.of_part(d).len() == 2) {
            let pads: Vec<&BoardPad> = pins.of_part(cap).iter().filter_map(|i| nets.pads.get(*i)).collect();
            let Some(first) = pads.first() else { continue };
            if side_of(first) != side {
                let (name, cap, ic) = (name.clone(), cap.to_string(), ic.to_string());
                let fp = DrcItem::Footprint { id: first.footprint, designator: cap.clone() };
                cx.add(HOT_LOOP, &[fp.key(), name.clone(), "side".into()], || {
                    Draft::new(
                        first.at(),
                        vec![fp],
                        format!("{cap} in {name} is on the other side from {ic}: the hot loop goes through vias and the board"),
                    )
                    .variant("side")
                    .param("")
                    .arg("loop", name.clone())
                    .arg("ic", ic.clone())
                    .fix(
                        "ecad.board.flip",
                        json!({"items": [cap.clone()]}),
                        format!(
                            "put {cap} on {ic}'s side, right at its pins; if a vendor's layout puts it there, approve this finding with that reason"
                        ),
                    )
                });
                continue;
            }
            for pad in &pads {
                let Some(n) = pad.net else { continue };
                // The regulator's nearest pad on the same net: its pulsed current takes the
                // shortest way in, whichever pin the record lists.
                let pin =
                    nets.pads.iter().filter(|q| q.designator == ic && q.net == Some(n)).min_by(|a, b| pad_gap(a, pad).total_cmp(&pad_gap(b, pad)));
                let Some(pin) = pin else { continue };
                let d = pad_gap(pad, pin);
                if d <= cap_distance + 1e-9 {
                    continue;
                }
                let (pad_ref, pin_ref, net, name) = (pad.reference(), pin.reference(), net_name(cx, n), name.clone());
                let items = vec![pad_item(pad), pad_item(pin)];
                cx.add(HOT_LOOP, &[pad_item(pad).key(), name.clone(), "distance".into()], || {
                    Draft::new(
                        pad.at(),
                        items,
                        format!(
                            "{pad_ref} is {} mm from {pin_ref} ({net}) in {name}; keep the loop's capacitor within {} mm",
                            num(d),
                            num(cap_distance)
                        ),
                    )
                    .layer(side)
                    .nets(vec![net.clone()])
                    .measure(d, cap_distance, "mm", Bound::Max)
                    .param("cap_distance")
                    .severity(Severity::Warning)
                    .variant("distance")
                    .arg("loop", name.clone())
                    .fix("ecad.board.move", serde_json::Value::Null, format!("move {} right up to {pin_ref}", pad.designator))
                });
            }
        }
        // Each capacitor pad and the regulator's nearest pad of its net (as the distance takes
        // it), whatever order the record lists the members in, joined only through vias.
        let mut joins: Vec<(usize, usize)> = Vec::new();
        for cap in parts.iter().copied().filter(|d| *d != ic && pins.of_part(d).len() == 2) {
            for a in pins.of_part(cap).iter().copied() {
                let Some(pa) = nets.pads.get(a) else { continue };
                let Some(n) = pa.net else { continue };
                let on_net = |i: &usize| nets.pads.get(*i).is_some_and(|q| q.designator == ic && q.net == Some(n));
                let gap_to = |i: &usize| nets.pads.get(*i).map_or(f64::INFINITY, |q| pad_gap(q, pa));
                if let Some(b) = (0..nets.pads.len()).filter(on_net).min_by(|x, y| gap_to(x).total_cmp(&gap_to(y))) {
                    joins.push((a, b));
                }
            }
        }
        for (a, b) in &joins {
            let (Some(pa), Some(pb)) = (nets.pads.get(*a), nets.pads.get(*b)) else { continue };
            if pa.net.is_none() || pa.net != pb.net || side_of(pa) != side_of(pb) {
                continue;
            }
            let Some(net) = pa.net else { continue };
            let [[ax0, ay0], [ax1, ay1]] = pa.bbox();
            let [[bx0, by0], [bx1, by1]] = pb.bbox();
            let area = grow([[ax0.min(bx0), ay0.min(by0)], [ax1.max(bx1), ay1.max(by1)]], 5.0);
            let Some(surface) = joined_on(cx, net, Some(side_of(pa)), *a, *b, area) else { return };
            if surface {
                continue;
            }
            // Not joined on any layer nearby: not routed yet, the connection check's.
            let Some(through) = joined_on(cx, net, None, *a, *b, area) else { return };
            if !through {
                continue;
            }
            let (ra, rb, net_n, name) = (pa.reference(), pb.reference(), net_name(cx, net), name.clone());
            let items = vec![pad_item(pa), pad_item(pb)];
            cx.add(HOT_LOOP, &[pad_item(pa).key(), pad_item(pb).key(), name.clone(), "via".into()], || {
                Draft::new(
                    pa.at(),
                    items,
                    format!("{name}'s current runs through vias: {ra} reaches {rb} ({net_n}) on {} only through another layer", side_of(pa).name()),
                )
                .layer(side_of(pa))
                .nets(vec![net_n.clone()])
                .variant("via")
                .param("")
                .arg("loop", name.clone())
                .fix(
                    "ecad.board.track",
                    serde_json::Value::Null,
                    format!("join {ra} to {rb} with short, wide copper on {}", side_of(pa).name()),
                )
            });
        }
        // Ground on the next layer under the loop's hull, worked out as ecad.board.loop.inspect
        // gives it (its ground_under).
        let member_pads: Vec<&BoardPad> = members.iter().filter_map(|(_, i)| nets.pads.get(*i)).collect();
        if !poured(cx) {
            cx.not_applicable(HOT_LOOP, "geometry: a ground plane (none poured yet)");
        } else if let Some((share, next)) = ground_under(cx.geo, nets, &roles.grounds, side, &member_pads, grid)
            && share < ground_min
        {
            let below = next.iter().map(|l| l.name()).collect::<Vec<_>>().join(" or ");
            let at = members.first().and_then(|(_, i)| nets.pads.get(*i)).map_or([0.0, 0.0], BoardPad::at);
            let name = name.clone();
            let fp = DrcItem::Footprint { id: ic_pad.footprint, designator: ic.to_string() };
            cx.add(HOT_LOOP, &[fp.key(), name.clone(), "ground".into()], || {
                Draft::new(at, vec![fp], format!("{} % of {name}'s area has ground under it on {below}; keep a solid ground on the next layer", num((share * 100.0).round())))
                    .layer(side)
                    .measure(share, ground_min, "ratio", Bound::Min)
                    .param("ground_min")
                    .severity(Severity::Warning)
                    .variant("ground")
                    .arg("loop", name.clone())
                    .arg("percent", num((share * 100.0).round()))
                    .arg("below", below.clone())
                    .fix("ecad.board.loop.inspect", json!({"loops": [name.clone()]}), format!("keep the ground on {below} solid under the loop (the loop's ground_under): move other nets' vias and tracks out from under it"))
            });
        }
        // Rank, or compare with the vendor's loop.
        let Some(m) = metrics.iter().find(|m| m.name == l.name) else { continue };
        let fp = DrcItem::Footprint { id: ic_pad.footprint, designator: ic.to_string() };
        let est = m.est_nh.map_or("no estimate (no plane under it)".to_string(), |v| format!("about {} nH", num((v * 100.0).round() / 100.0)));
        let words = format!(
            "{} mm², {} mm round, {est}; rank {} of {}",
            num((m.area_mm2 * 100.0).round() / 100.0),
            num((m.length_mm * 100.0).round() / 100.0),
            m.rank,
            metrics.len()
        );
        let at = m.path.first().copied().unwrap_or_else(|| ic_pad.at());
        if let Some(ratio) = l.reference_ratio.filter(|r| *r > ratio_max) {
            let name = name.clone();
            cx.add(HOT_LOOP, &[fp.key(), name.clone(), "reference".into()], || {
                Draft::new(at, vec![fp], format!("{name} is {}× the vendor's loop, more than {}×: {words}", num(ratio), num(ratio_max)))
                    .measure(ratio, ratio_max, "ratio", Bound::Max)
                    .param("ratio_max")
                    .severity(Severity::Warning)
                    .variant("reference")
                    .arg("loop", name.clone())
                    .fix(
                        "ecad.board.loop.inspect",
                        json!({"loops": [name.clone()]}),
                        "see the loop's members and path; place them as the vendor's layout does",
                    )
            });
        } else {
            let name = name.clone();
            let nh = m.est_nh.unwrap_or(0.0);
            cx.add(HOT_LOOP, &[fp.key(), name.clone(), "rank".into()], || {
                Draft::new(at, vec![fp], format!("{name}: {words}"))
                    .severity(Severity::Info)
                    .class(Class::Practical)
                    .variant("rank")
                    .param("")
                    .arg("loop", name.clone())
                    .arg("nh", num((nh * 100.0).round() / 100.0))
                    .arg("area", num((m.area_mm2 * 100.0).round() / 100.0))
                    .arg("rank", m.rank.to_string())
                    .review()
                    .fix(
                        "ecad.board.loop.inspect",
                        json!({"loops": [name.clone()]}),
                        "compare the loop with the vendor's layout; give its record reference_ratio to check it",
                    )
            });
        }
    }
}

// ---- pi.smps_switch_node ----

/// A piece of copper: its layer, shape, the part it belongs to (a pad's), and the object.
#[derive(Clone)]
struct Piece {
    layer: CopperLayer,
    core: Core,
    part: Option<u64>,
    item: DrcItem,
}

/// The copper of `net`: its tracks' pieces, pads and vias, layer by layer.
fn copper_of(cx: &Cx, net: usize) -> Vec<Piece> {
    let (board, nets) = (cx.board, cx.nets);
    let mut out = Vec::new();
    for (ti, t) in board.tracks.iter().enumerate() {
        if nets.track_nets.get(ti).copied().flatten() == Some(net) {
            out.extend(track_pieces(&t.points, &t.curves).into_iter().map(|(a, b)| Piece {
                layer: t.layer,
                core: stroke(a, b, t.width / 2.0),
                part: None,
                item: DrcItem::Track(t.id),
            }));
        }
    }
    for p in nets.pads.iter().filter(|p| p.net == Some(net)) {
        for l in pad_layers(cx, p) {
            out.push(Piece { layer: l, core: pad_core(p), part: Some(p.footprint), item: pad_item(p) });
        }
    }
    out
}

/// How far round an IC's pads its pins' breakout reaches, in millimetres.
const BREAKOUT: f64 = 0.2;

/// `pieces` without what lies in `fields`: the pads of the parts `parts` dropped, tracks cut to
/// their runs outside.
fn clip(pieces: &[Piece], fields: &[[[f64; 2]; 2]], parts: &BTreeSet<u64>) -> Vec<Piece> {
    if fields.is_empty() {
        return pieces.to_vec();
    }
    let within = |q: [f64; 2]| fields.iter().any(|[lo, hi]| q[0] >= lo[0] && q[0] <= hi[0] && q[1] >= lo[1] && q[1] <= hi[1]);
    let mut out = Vec::new();
    for p in pieces {
        if p.part.is_some_and(|f| parts.contains(&f)) {
            continue;
        }
        let (DrcItem::Track(_), [a, b]) = (&p.item, p.core.points.as_slice()) else {
            out.push(p.clone());
            continue;
        };
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        let pts = along(*a, *b, (len / 400.0).max(0.05));
        let mut run: Vec<[f64; 2]> = Vec::new();
        for q in pts.iter().copied().chain(std::iter::once([f64::NAN, f64::NAN])) {
            if q[0].is_finite() && !within(q) {
                run.push(q);
                continue;
            }
            if let (Some(first), Some(last)) = (run.first().copied(), run.last().copied()) {
                out.push(Piece { layer: p.layer, core: stroke(first, last, p.core.radius), part: None, item: p.item.clone() });
            }
            run.clear();
        }
    }
    out
}

/// The copper layers a pad is on.
fn pad_layers(cx: &Cx, p: &BoardPad) -> Vec<CopperLayer> {
    cx.geo.layers.iter().copied().filter(|l| p.on(*l)).collect()
}

/// Points over a piece's copper, `step` apart (at most 100 along a side of its box).
fn sample(core: &Core, step: f64) -> Vec<[f64; 2]> {
    let [[x0, y0], [x1, y1]] = super::super::copper::core_bbox(core);
    let (nx, ny) = (((x1 - x0) / step).ceil().clamp(0.0, 100.0) as usize, ((y1 - y0) / step).ceil().clamp(0.0, 100.0) as usize);
    let mut out = Vec::new();
    for i in 0..=nx {
        for j in 0..=ny {
            let q = [x0 + (x1 - x0) * i as f64 / nx.max(1) as f64, y0 + (y1 - y0) * j as f64 / ny.max(1) as f64];
            if gap(core, &disc(q, 0.0)) <= 0.0 {
                out.push(q);
            }
        }
    }
    out
}

/// The outline of the footprint of `designator` on the board (its body as the fab layer draws
/// it, else its courtyard, else its box), and its side.
fn outline_of(cx: &Cx, designator: &str) -> Option<(Core, CopperLayer, u64)> {
    let id = cx.nets.pads.iter().find(|p| p.designator == designator)?.footprint;
    let fp = cx.board.footprints.iter().find(|f| f.id == id)?;
    let placed = cx.ecad.placed(fp)?;
    let item = placed.resolved.footprint;
    let [[x0, y0], [x1, y1]] = super::super::graphics::shapes_bbox(item.layer(BoardLayer::Fab, Side::Top))
        .or_else(|| item.courtyard(Side::Top))
        .or_else(|| super::super::board::footprint_box(item))?;
    let pl = fp.placement();
    let points = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(|p| pl.apply(p)).to_vec();
    let layer = if fp.side == Side::Bottom { CopperLayer::Bottom } else { CopperLayer::Top };
    Some((Core { points, radius: 0.0 }, layer, fp.id))
}

/// Whether a ground plane covers every one of `points` on some layer strictly between `a` and
/// `b`.
fn ground_between(cx: &Cx, a: CopperLayer, b: CopperLayer, points: &[[f64; 2]]) -> bool {
    let (nets, geo, roles) = (cx.nets, cx.geo, cx.roles);
    let layers = cx.board.copper_layers;
    let (ia, ib) = (a.index(layers), b.index(layers));
    let (lo, hi) = (ia.min(ib), ia.max(ib));
    geo.layers
        .iter()
        .filter(|l| l.index(layers) > lo && l.index(layers) < hi)
        .any(|l| geo.plane(*l).is_some_and(|pl| pl.nets_at(nets, points).iter().all(|n| n.is_some_and(|n| roles.grounds.contains(&n)))))
}

fn switch_nodes(cx: &mut Cx, pins: &Pins, regs: &[String]) {
    let (board, nets, geo, roles) = (cx.board, cx.nets, cx.geo, cx.roles);
    let p = |name: &str| super::param_value(board, SWITCH_NODE, name);
    let (near_w, near_i, void_max, step) = (p("near_w"), p("near_i"), p("void_max"), p("step").max(0.02));
    let slack = super::param_value(board, "pi.return_path", "antipad_slack");
    let sw: Vec<usize> = roles.nets.iter().filter(|n| n.has_role(NetRole::SwNode)).map(|n| n.index).collect();
    if sw.is_empty() {
        for r in regs {
            cx.not_applicable(SWITCH_NODE, format!("intent: the switch node of {r}"));
        }
        return;
    }
    let sensitive: Vec<usize> = roles
        .nets
        .iter()
        .filter(|n| matches!(n.role(), Some(NetRole::Fb | NetRole::Analog | NetRole::Sensitive | NetRole::Clock)))
        .map(|n| n.index)
        .collect();
    for n in sw {
        let name = net_name(cx, n);
        let copper = copper_of(cx, n);
        if copper.is_empty() {
            cx.not_applicable(SWITCH_NODE, format!("geometry: {name} on the board"));
            continue;
        }
        if !cx.spend(copper.len().saturating_mul(64)) {
            return;
        }
        let at = copper.first().map_or([0.0, 0.0], |c| c.core.points.first().copied().unwrap_or([0.0, 0.0]));
        // On more than one layer (pads of a through-hole part count on their side only).
        let mut layers: BTreeSet<CopperLayer> = copper.iter().filter(|c| matches!(c.item, DrcItem::Track(_))).map(|c| c.layer).collect();
        layers.extend(nets.pads.iter().filter(|q| q.net == Some(n)).map(side_of));
        layers.extend(nets.fills.iter().filter(|f| f.net == Some(n)).map(|f| f.layer));
        if layers.len() > 1 {
            let list = layers.iter().map(|l| l.name()).collect::<Vec<_>>().join(", ");
            let name = name.clone();
            cx.add(SWITCH_NODE, &[name.clone(), "layers".into()], || {
                Draft::new(at, Vec::new(), format!("switch node {name} has copper on {} layers ({list}): keep it on one", layers.len()))
                    .nets(vec![name.clone()])
                    .variant("layers")
                    .param("")
                    .arg("net", name.clone())
                    .arg("layers", list.clone())
                    .fix("ecad.board.net.inspect", json!({"nets": [name.clone()]}), "route the switch node on the regulator's layer only; if a vendor's layout has the via, approve this finding with that reason")
            });
        }
        // A ground void under it (once a ground plane is poured).
        let ground = poured(cx);
        if !ground {
            cx.not_applicable(SWITCH_NODE, "geometry: a ground plane (none poured yet)");
        }
        for layer in layers.iter().filter(|_| ground) {
            let mut pts: Vec<[f64; 2]> = Vec::new();
            for c in copper.iter().filter(|c| c.layer == *layer) {
                if let [a, b] = c.core.points.as_slice()
                    && matches!(c.item, DrcItem::Track(_))
                {
                    pts.extend(along(*a, *b, step));
                } else {
                    pts.extend(sample(&c.core, step));
                }
                if pts.len() > super::geom::MAX_SAMPLES {
                    break;
                }
            }
            if !cx.spend(pts.len().saturating_mul(4)) {
                return;
            }
            let next: Vec<CopperLayer> = geo.neighbours(*layer).into_iter().map(|(l, _)| l).collect();
            let mut skip = vec![false; pts.len()];
            for l in &next {
                for (s, own) in skip.iter_mut().zip(geo.own_antipads(*l, &pts, Some(n), slack)) {
                    *s |= own;
                }
            }
            let Some((share, next)) = ground_share(cx, *layer, &pts, &skip) else { continue };
            let off = 1.0 - share;
            if off <= void_max + 1e-9 {
                continue;
            }
            let below = next.iter().map(|l| l.name()).collect::<Vec<_>>().join(" or ");
            let name = name.clone();
            let layer = *layer;
            cx.add(SWITCH_NODE, &[name.clone(), layer.name(), "void".into()], || {
                Draft::new(
                    at,
                    Vec::new(),
                    format!("{} % of switch node {name} on {} has no ground under it on {below}", num((off * 100.0).round()), layer.name()),
                )
                .layer(layer)
                .nets(vec![name.clone()])
                .measure(off, void_max, "ratio", Bound::Max)
                .param("void_max")
                .variant("void")
                .arg("net", name.clone())
                .arg("percent", num((off * 100.0).round()))
                .arg("below", below.clone())
                .fix(
                    "ecad.board.net.inspect",
                    json!({"nets": [name.clone()], "metrics": ["reference"]}),
                    format!("keep the ground plane on {below} solid under the switch node: move what cuts it"),
                )
            });
        }
        // Its inductors' outlines.
        let inductors: Vec<String> = nets
            .pads
            .iter()
            .filter(|q| q.net == Some(n))
            .map(|q| q.designator.clone())
            .filter(|d| roles.intent.part(d).is_some_and(|r| r.role == PartRole::SmpsInductor))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut outlines: Vec<(String, Core, CopperLayer)> = Vec::new();
        for d in &inductors {
            let Some((outline, layer, _)) = outline_of(cx, d) else { continue };
            let own: BTreeSet<usize> = pins.of_part(d).iter().filter_map(|i| nets.pads.get(*i)).filter_map(|q| q.net).collect();
            let foreign = |net: Option<usize>| net.is_none_or(|x| !own.contains(&x) && !roles.grounds.contains(&x));
            let mut found: Vec<DrcItem> = Vec::new();
            for (ti, t) in board.tracks.iter().enumerate() {
                if t.layer != layer || !foreign(nets.track_nets.get(ti).copied().flatten()) {
                    continue;
                }
                if track_pieces(&t.points, &t.curves).iter().any(|(a, b)| gap(&stroke(*a, *b, t.width / 2.0), &outline) < 0.0) {
                    found.push(DrcItem::Track(t.id));
                }
            }
            for (vi, v) in board.vias.iter().enumerate() {
                if v.on(layer, board.copper_layers)
                    && foreign(nets.via_nets.get(vi).copied().flatten())
                    && gap(&disc(v.at, v.diameter / 2.0), &outline) < 0.0
                {
                    found.push(DrcItem::Via(v.id));
                }
            }
            for q in nets.pads.iter().filter(|q| q.designator != *d && q.on(layer) && foreign(q.net)) {
                if gap(&pad_core(q), &outline) < 0.0 {
                    found.push(pad_item(q));
                }
            }
            if !cx.spend(board.tracks.len().saturating_add(board.vias.len()).saturating_add(nets.pads.len())) {
                return;
            }
            if let Some(first) = found.first().cloned() {
                let count = found.len();
                let (d, name) = (d.clone(), name.clone());
                let mut items = vec![first.clone()];
                items.extend(found.iter().skip(1).take(3).cloned());
                let at = outline.points.first().copied().unwrap_or(at);
                // Once per inductor, though it is on two switch nodes (a buck-boost's).
                cx.add(SWITCH_NODE, &[d.clone(), "inductor".into()], || {
                    Draft::new(
                        at,
                        items,
                        format!(
                            "{count} {} of other nets' copper {} inside {d}'s outline on {} ({}…)",
                            if count == 1 { "piece" } else { "pieces" },
                            if count == 1 { "lies" } else { "lie" },
                            layer.name(),
                            first.reference()
                        ),
                    )
                    .layer(layer)
                    .nets(vec![name.clone()])
                    .variant("inductor")
                    .param("")
                    .arg("part", d.clone())
                    .arg("count", count.to_string())
                    .fix(
                        "ecad.board.net.inspect",
                        json!({"nets": [name.clone()]}),
                        format!("move other nets' tracks and vias out from under {d} on {}", layer.name()),
                    )
                });
            }
            outlines.push((d.clone(), outline, layer));
        }
        // Sensitive copper near it on its layer, or across it on another; within the pad field
        // of an IC both nets come from, its pins' breakout is the pinout's, left out.
        for s in &sensitive {
            let shared: BTreeSet<u64> = nets
                .pads
                .iter()
                .filter(|q| q.net == Some(n))
                .map(|q| q.footprint)
                .filter(|f| nets.pads.iter().any(|q| q.net == Some(*s) && q.footprint == *f))
                .collect();
            let fields: Vec<[[f64; 2]; 2]> = shared
                .iter()
                .filter_map(|f| super::super::bbox(nets.pads.iter().filter(|q| q.footprint == *f).flat_map(|q| q.bbox())).map(|b| grow(b, BREAKOUT)))
                .collect();
            let theirs = clip(&copper_of(cx, *s), &fields, &shared);
            let copper = clip(&copper, &fields, &shared);
            if !cx.spend(theirs.len().saturating_add(copper.len()).saturating_mul(16)) {
                return;
            }
            let mut grid = Grid::new(2.0);
            for (k, c) in copper.iter().enumerate() {
                grid.insert(k, super::super::copper::core_bbox(&c.core));
            }
            // Per layer, the closest piece on that layer; and the overlaps on other layers.
            let mut closest: BTreeMap<CopperLayer, (f64, DrcItem, DrcItem, [f64; 2])> = BTreeMap::new();
            let mut across: Option<(DrcItem, String, CopperLayer, CopperLayer, [f64; 2])> = None;
            for t in &theirs {
                let bbox = super::super::copper::core_bbox(&t.core);
                let near = grid.near_box(grow(bbox, near_i), copper.len());
                if !cx.spend(near.len().saturating_add(1)) {
                    return;
                }
                for c in near.into_iter().filter_map(|k| copper.get(k)) {
                    if t.part.is_some() && t.part == c.part {
                        continue;
                    }
                    let d = gap(&t.core, &c.core);
                    if c.layer == t.layer {
                        let at = t.core.points.first().copied().unwrap_or(at);
                        let e = closest.entry(t.layer).or_insert((f64::INFINITY, t.item.clone(), c.item.clone(), at));
                        if d < e.0 {
                            *e = (d, t.item.clone(), c.item.clone(), at);
                        }
                    } else if d < 0.0 && across.is_none() {
                        let over: Vec<[f64; 2]> =
                            sample(&t.core, step).into_iter().filter(|q| gap(&c.core, &disc(*q, 0.0)) <= 0.0).take(64).collect();
                        if !over.is_empty() && !ground_between(cx, t.layer, c.layer, &over) {
                            across = Some((t.item.clone(), c.item.reference(), t.layer, c.layer, over.first().copied().unwrap_or(at)));
                        }
                    }
                }
                for (d, outline, layer) in &outlines {
                    if *layer == t.layer || across.is_some() || gap(&t.core, outline) >= 0.0 {
                        continue;
                    }
                    let over: Vec<[f64; 2]> = sample(&t.core, step).into_iter().filter(|q| gap(outline, &disc(*q, 0.0)) <= 0.0).take(64).collect();
                    if !over.is_empty() && !ground_between(cx, t.layer, *layer, &over) {
                        across = Some((t.item.clone(), format!("{d}'s outline"), t.layer, *layer, over.first().copied().unwrap_or(at)));
                    }
                }
            }
            let s_name = net_name(cx, *s);
            let role = roles.net(*s).and_then(|x| x.role()).map_or("sensitive", |r| r.name());
            if let Some((item, what, ls, lw, at)) = across {
                let (name, s_name) = (name.clone(), s_name.clone());
                cx.add(SWITCH_NODE, &[name.clone(), s_name.clone(), "overlap".into()], || {
                    Draft::new(
                        at,
                        vec![item],
                        format!(
                            "{s_name} ({role}) on {} crosses {what} of switch node {name} on {} with no ground plane between",
                            ls.name(),
                            lw.name()
                        ),
                    )
                    .layer(ls)
                    .nets(vec![s_name.clone(), name.clone()])
                    .severity(Severity::Error)
                    .variant("overlap")
                    .param("")
                    .arg("net", s_name.clone())
                    .arg("sw", name.clone())
                    .arg("what", what.clone())
                    .fix(
                        "ecad.board.net.inspect",
                        json!({"nets": [s_name.clone()]}),
                        format!("route {s_name} away from {name}, or keep a ground plane between them"),
                    )
                });
            }
            for (layer, (d, mine, other, at)) in closest {
                if d >= near_i {
                    continue;
                }
                let (severity, limit, param) = if d < near_w { (Severity::Warning, near_w, "near_w") } else { (Severity::Info, near_i, "near_i") };
                let (name, s_name) = (name.clone(), s_name.clone());
                let d = d.max(0.0);
                cx.add(SWITCH_NODE, &[name.clone(), s_name.clone(), layer.name(), "near".into()], || {
                    Draft::new(
                        at,
                        vec![mine, other],
                        format!("{s_name} ({role}) comes within {} mm of switch node {name} on {}", num(d), layer.name()),
                    )
                    .layer(layer)
                    .nets(vec![s_name.clone(), name.clone()])
                    .measure(d, limit, "mm", Bound::Min)
                    .param(param)
                    .severity(severity)
                    .variant("near")
                    .arg("net", s_name.clone())
                    .arg("sw", name.clone())
                    .fix(
                        "ecad.board.net.inspect",
                        json!({"nets": [s_name.clone()], "metrics": ["coupling"]}),
                        format!("keep {s_name} at least {} mm from {name}", num(near_w)),
                    )
                });
            }
        }
    }
}

// ---- pi.smps_feedback ----

fn feedback(cx: &mut Cx, pins: &Pins, regs: &[String]) {
    let (board, nets, roles) = (cx.board, cx.nets, cx.roles);
    let limit = super::param_value(board, FEEDBACK, "distance");
    let fb: Vec<usize> = roles.nets.iter().filter(|n| n.has_role(NetRole::Fb)).map(|n| n.index).collect();
    if fb.is_empty() {
        for r in regs {
            cx.not_applicable(FEEDBACK, format!("intent: the feedback net of {r}"));
        }
        return;
    }
    for f in fb {
        let name = net_name(cx, f);
        let Some(ic) = regulator_of(cx, regs, f) else {
            cx.not_applicable(FEEDBACK, format!("intent: the regulator {name} feeds back to (its record's domain)"));
            continue;
        };
        let dividers: BTreeSet<String> = nets
            .pads
            .iter()
            .filter(|p| p.net == Some(f))
            .map(|p| p.designator.clone())
            .filter(|d| roles.intent.part(d).is_some_and(|r| r.role == PartRole::FbDivider))
            .collect();
        // No divider: a fixed-output part.
        if dividers.is_empty() {
            continue;
        }
        let Some((pin_i, pin)) = nets.pads.iter().enumerate().find(|(_, p)| p.designator == ic && p.net == Some(f)) else {
            cx.not_applicable(FEEDBACK, format!("geometry: {ic}'s feedback pin on the board"));
            continue;
        };
        let centre: Vec<(usize, &BoardPad)> =
            nets.pads.iter().enumerate().filter(|(_, p)| p.net == Some(f) && dividers.contains(&p.designator)).collect();
        // Along the FB net's copper when routed, else straight.
        let Some(g) = Copper::build(cx, f, None, None, true) else { return };
        let walked = g.pad(pin_i).map(|n| g.walk(cx, n, |_| true, f64::INFINITY).0);
        let (d, pad, routed) = centre
            .iter()
            .map(|(i, p)| {
                let along = walked.as_ref().and_then(|w| g.pad(*i).and_then(|n| w.get(n).copied())).filter(|d| d.is_finite());
                match along {
                    Some(d) => (d, *p, true),
                    None => (pad_gap(pin, p), *p, false),
                }
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap_or((0.0, pin, false));
        if d > limit + 1e-9 {
            let (pin_ref, pad_ref, name) = (pin.reference(), pad.reference(), name.clone());
            let how = if routed { "along its copper" } else { "edge to edge" };
            let items = vec![pad_item(pad), pad_item(pin)];
            cx.add(FEEDBACK, &[pad_item(pad).key(), pad_item(pin).key(), "distance".into()], || {
                Draft::new(
                    pad.at(),
                    items,
                    format!("the feedback divider's {pad_ref} is {} mm from {pin_ref} ({name}), {how}; keep it within {} mm", num(d), num(limit)),
                )
                .nets(vec![name.clone()])
                .measure(d, limit, "mm", Bound::Max)
                .param("distance")
                .variant("distance")
                .arg("how", how)
                .fix("ecad.board.move", serde_json::Value::Null, format!("move {} next to {pin_ref}", pad.designator))
            });
        }
        // The divider's top: where it taps the output.
        for d in &dividers {
            let own: Vec<(usize, &BoardPad)> = pins.of_part(d).iter().filter_map(|i| nets.pads.get(*i).map(|p| (*i, p))).collect();
            let Some((top_i, top)) = own.iter().find(|(_, p)| p.net.is_some_and(|n| n != f && !roles.grounds.contains(&n))).copied() else {
                continue;
            };
            let Some(out) = top.net else { continue };
            let Some(tap) = tap_of(cx, out, top_i, &ic) else {
                if cx.spent() {
                    return;
                }
                continue;
            };
            let Some(what) = tap else { continue };
            let (top_ref, out_name) = (top.reference(), net_name(cx, out));
            cx.add(FEEDBACK, &[pad_item(top).key(), "tap".into()], || {
                Draft::new(
                    top.at(),
                    vec![pad_item(top)],
                    format!("the feedback divider's top {top_ref} taps {out_name} at {what}, copper that carries the load current"),
                )
                .nets(vec![out_name.clone()])
                .variant("tap")
                .param("")
                .arg("net", out_name.clone())
                .arg("what", what.clone())
                .fix(
                    "ecad.board.track",
                    serde_json::Value::Null,
                    format!("run {top_ref}'s own track to the output capacitor's {out_name} pad"),
                )
            });
        }
    }
}

/// Where the divider's top pad (by index) taps the output `out`: `Some(None)` at an output
/// capacitor's pad or the regulator's own, or on a pour those reach; `Some(Some(what))` on
/// copper that carries the load current (the pad itself when the output reaches a load only
/// through it); `None` past the budget or when nothing is reached.
fn tap_of(cx: &mut Cx, out: usize, top: usize, ic: &str) -> Option<Option<String>> {
    let (nets, roles) = (cx.nets, cx.roles);
    let g = Copper::build(cx, out, None, None, true)?;
    g.pad(top)?;
    let is_cap = |p: &BoardPad| {
        roles.intent.part(&p.designator).is_some_and(|r| r.role == PartRole::SmpsOutCap)
            || (p.designator.starts_with('C')
                && nets.pads.iter().filter(|q| q.designator == p.designator).count() == 2
                && nets.pads.iter().any(|q| q.designator == p.designator && q.net.is_some_and(|n| roles.grounds.contains(&n))))
    };
    let divider = nets.pads.get(top).map_or("", |p| p.designator.as_str());
    let source = |i: usize| nets.pads.get(i).is_some_and(|p| is_cap(p) || p.designator == ic);
    // The divider's pad with the track ends and vias on it, and the copper leaving it.
    let own = g.on_pad(cx, top);
    let exits: BTreeSet<usize> = own.iter().flat_map(|n| g.neighbours(*n)).filter(|m| !own.contains(m)).collect();
    if exits.len() > 1 {
        // Load current passes the pad when, without it, the output no longer reaches a load it
        // reaches with it.
        if !cx.spend(g.kinds.len().saturating_mul(8)) {
            return None;
        }
        let starts: Vec<usize> = (0..nets.pads.len()).filter(|i| source(*i)).filter_map(|i| g.pad(i)).filter(|n| !own.contains(n)).collect();
        let reach = |skip: &BTreeSet<usize>| {
            let mut reached = vec![false; g.kinds.len()];
            let mut todo = starts.clone();
            while let Some(n) = todo.pop() {
                match reached.get_mut(n) {
                    Some(r) if !*r => *r = true,
                    _ => continue,
                }
                todo.extend(g.neighbours(n).into_iter().filter(|m| !skip.contains(m)));
            }
            reached
        };
        let (with, without) = (reach(&BTreeSet::new()), reach(&own));
        let cut = (0..nets.pads.len()).any(|i| {
            nets.pads.get(i).is_some_and(|p| p.net == Some(out) && p.designator != divider && !source(i))
                && g.pad(i).is_some_and(|n| with.get(n) == Some(&true) && without.get(n) == Some(&false))
        });
        if cut {
            return Some(Some(format!("its own pad, which {} other pieces of copper meet on the way from the output to its loads", exits.len())));
        }
    }
    // Each way out of the pad, followed to where it meets other copper: the tap is fine when
    // every way reaches the output's own copper.
    let mut found: Option<String> = None;
    for start in exits {
        match follow(cx, &g, out, &own, start, ic, &is_cap)? {
            None => {}
            Some(what) => {
                found.get_or_insert(what);
            }
        }
    }
    Some(found)
}

/// Where a way out of the divider's pad (`own`), starting at node `at`, meets other copper:
/// `Some(None)` at an output capacitor's or the regulator's pad, or a pour those reach;
/// `Some(Some(what))` at other copper; `None` when it ends nowhere.
fn follow(
    cx: &Cx,
    g: &Copper,
    out: usize,
    own: &BTreeSet<usize>,
    mut at: usize,
    ic: &str,
    is_cap: &dyn Fn(&BoardPad) -> bool,
) -> Option<Option<String>> {
    let nets = cx.nets;
    let mut seen = own.clone();
    for _ in 0..g.kinds.len() {
        seen.insert(at);
        // A track end or via on a pad has reached that pad.
        if let Some(i) = g.pad_at(cx, at) {
            let p = nets.pads.get(i)?;
            return Some(if is_cap(p) || p.designator == ic { None } else { Some(format!("{}'s pad", p.reference())) });
        }
        let next: Vec<usize> = g.neighbours(at).into_iter().filter(|n| !seen.contains(n)).collect();
        match g.kinds.get(at)? {
            Kind::Fill(fi, _) => {
                // A pour the output's capacitor or the regulator's pad joins (through its vias)
                // is the output's own copper; a plane carries the load at no drop to speak of.
                let mut todo = vec![at];
                let mut been = BTreeSet::new();
                while let Some(n) = todo.pop() {
                    if !been.insert(n) || been.len() > g.kinds.len() {
                        continue;
                    }
                    if let Some(i) = g.pad_at(cx, n).or(match g.kinds.get(n) {
                        Some(Kind::Pad(i)) => Some(*i),
                        _ => None,
                    }) {
                        if nets.pads.get(i).is_some_and(|p| p.net == Some(out) && (is_cap(p) || p.designator == ic)) {
                            return Some(None);
                        }
                        continue;
                    }
                    todo.extend(g.neighbours(n).into_iter().filter(|m| !own.contains(m)));
                }
                let layer = nets.fills.get(*fi).map_or(String::new(), |f| f.layer.name());
                return Some(Some(format!("its pour on {layer}")));
            }
            Kind::Point(..) | Kind::Via(_) if next.len() == 1 => at = next.into_iter().next()?,
            Kind::Point(_, q) if next.len() > 1 => {
                return Some(Some(format!("a junction partway along its track at ({}, {})", num(q[0]), num(q[1]))));
            }
            Kind::Via(i) if next.len() > 1 => {
                // Vias stacked down to a pour reach it: follow on to the pour.
                if let Some(fill) = next.iter().copied().find(|n| matches!(g.kinds.get(*n), Some(Kind::Fill(..)))) {
                    at = fill;
                    continue;
                }
                let id = cx.board.vias.get(*i).map_or(0, |v| v.id);
                return Some(Some(format!("via {id}, where its copper branches")));
            }
            _ => return None,
        }
    }
    None
}
