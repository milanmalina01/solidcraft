//! The numbers behind the quality findings (see `solidcraft_doc::ecad::quality::metrics`):
//! `ecad.board.net.inspect` gives nets' length by layer, delay, vias, bends, each segment's
//! reference plane, coverage, height and impedance, the neighbours they couple to and a pair's
//! skew; `ecad.board.loop.inspect` gives current loops (the intent's, and one per pin each
//! decoupling capacitor serves) with their path, length, area, vias, layers and estimated
//! inductance, ranked. Both read only.

use std::collections::BTreeSet;

use serde_json::Value;
use solidcraft_doc::ecad::intent::LoopKind;
use solidcraft_doc::ecad::quality::metrics::{self, LoopFilter, Want};

use super::CommandSpec;
use super::ecad::{closest, parse, reply};
use crate::params::bad;
use crate::schema::ecad_board_metrics::{
    BendsOut, LoopInspectParams, LoopInspectResult, LoopKindParam, LoopOut, MemberOut, MetricParam, NeighbourOut, NetInspectParams, NetInspectResult,
    NetOut, PairOut, SegmentOut,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.board.net.inspect", "Net Metrics", crate::checked!("ecad.board.net.inspect", net_inspect))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&NET_CONTRACT)
        .output(output_schema_for::<NetInspectResult>)
        .params(
            "nets? [name] | class? (a net class's nets) | pair? (an intent pair's two nets); left out, every net with copper; metrics? [length|layers|vias|bends|delay|reference|impedance|coupling|segments|pair] (length by layer, delay, vias, bends and a pair's skew always come; reference, impedance and segments list the segments, coupling the neighbours); limit? (25, at most 256) offset? → {nets: [{net, role?, tier?, regime, length_mm, by_layer: {layer: mm}, delay_ps, vias, bends: {45, 90, acute, other, any_angle}, segments?: [{track, layer, width, length_mm, delay_ps, z0?, ref?, ref_net?, ref_coverage?, h?}], segments_more?, neighbours?: [{net, layer, coupled_mm, min_gap_mm}], pair?: {name, partner, skew_mm, skew_ps}}], more?, truncated?}: the numbers behind the quality findings, to see what is long, where a net changes layer or leaves its plane, and what couples to it. Delay from the layer's effective εr (microstrip outside, stripline inside); ref is the neighbouring layer whose planes (pours) cover most of the segment. Read-only",
        ),
    CommandSpec::new("ecad.board.loop.inspect", "Loop Metrics", crate::checked!("ecad.board.loop.inspect", loop_inspect))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&LOOP_CONTRACT)
        .output(output_schema_for::<LoopInspectResult>)
        .params(
            "loops? [name] | kind? decap|hot|buck_hot|boost_hot|gate|crystal|tvs|sense | part? (designator: loops with a member on it); limit? (25, at most 512) offset? → {loops: [{name, kind, from: intent|decoupling, members: [{pin, at?, net?}], path, length_mm, area_mm2, vias, layers, h?, ref_coverage?, ground_under? (the share of the members' hull over ground on the next layer, as pi.smps_hot_loop judges it), est_nh?, rank, missing?}], more?, truncated?}: current loops, largest estimated inductance first: the intent's loops (ecad.intent.set loops) and one decoupling loop per pin each decoupling capacitor serves (pin, capacitor, back to the IC's nearest pad on the capacitor's other net). The path joins the members' pads; est_nh = μ0·h·(perimeter/2)/w (h to the reference plane, w the narrowest pad) ranks loops and shows a move helped, it is no measurement. Read-only",
        ),
];

const NET_CONTRACT: TypedContract<NetInspectParams> = TypedContract::new();
const LOOP_CONTRACT: TypedContract<LoopInspectParams> = TypedContract::new();

const DEFAULT_LIMIT: usize = 25;

fn round(v: f64) -> f64 {
    if v.is_finite() { (v * 1e4).round() / 1e4 + 0.0 } else { 0.0 }
}

fn net_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.net.inspect";
    let p: NetInspectParams = parse(CMD, p)?;
    let given = usize::from(p.nets.is_some()) + usize::from(p.class.is_some()) + usize::from(p.pair.is_some());
    if given > 1 {
        return Err(bad(CMD, "give one of `nets`, `class` or `pair`"));
    }
    let limit = p.limit.unwrap_or(DEFAULT_LIMIT);
    if limit > metrics::MAX_NETS {
        return Err(bad(CMD, format!("`limit` is at most {}", metrics::MAX_NETS)));
    }
    let offset = p.offset.unwrap_or(0);
    let nets = s.board_nets();
    let ecad = &s.doc.ecad;
    let index = |name: &str| nets.names.iter().position(|n| n == name);
    let unknown = |field: &str, name: &str| {
        let mut msg = format!("`{field}`: the design has no net `{name}`");
        if let Some(near) = closest(name, nets.names.iter().map(String::as_str)) {
            msg.push_str(&format!(" (did you mean `{near}`?)"));
        }
        bad(CMD, msg)
    };
    let which: Vec<usize> = if let Some(list) = &p.nets {
        if list.len() > 4096 {
            return Err(bad(CMD, "`nets` names at most 4096 nets"));
        }
        let mut out = Vec::new();
        for (i, n) in list.iter().enumerate() {
            out.push(index(n).ok_or_else(|| unknown(&format!("nets[{i}]"), n))?);
        }
        out
    } else if let Some(name) = &p.class {
        let classes = &ecad.board.rules.net_classes;
        let Some(class) = classes.iter().find(|c| c.name == *name) else {
            let mut msg = format!("`class`: the board has no net class `{name}`");
            if let Some(near) = closest(name, classes.iter().map(|c| c.name.as_str())) {
                msg.push_str(&format!(" (did you mean `{near}`?)"));
            }
            return Err(bad(CMD, msg));
        };
        class.nets.iter().filter_map(|n| index(n)).collect()
    } else if let Some(name) = &p.pair {
        let pairs = &ecad.intent.pairs;
        let Some(pair) = pairs.iter().find(|x| x.name == *name) else {
            let mut msg = format!("`pair`: the intent has no pair `{name}`");
            if let Some(near) = closest(name, pairs.iter().map(|x| x.name.as_str())) {
                msg.push_str(&format!(" (did you mean `{near}`?)"));
            } else {
                msg.push_str(" (ecad.intent.inspect lists the pairs)");
            }
            return Err(bad(CMD, msg));
        };
        [&pair.p, &pair.n].into_iter().filter_map(|n| index(n)).collect()
    } else {
        let with: BTreeSet<usize> = nets.track_nets.iter().chain(&nets.via_nets).flatten().copied().collect();
        with.into_iter().collect()
    };
    let mut seen = BTreeSet::new();
    let which: Vec<usize> = which.into_iter().filter(|n| seen.insert(*n)).collect();
    let page: Vec<usize> = which.iter().copied().skip(offset).take(limit).collect();
    let more = which.len().saturating_sub(offset).saturating_sub(page.len());
    let asked: BTreeSet<MetricParam> = p.metrics.unwrap_or_default().into_iter().collect();
    let want = Want {
        reference: asked.contains(&MetricParam::Reference),
        impedance: asked.contains(&MetricParam::Impedance),
        coupling: asked.contains(&MetricParam::Coupling),
        segments: asked.contains(&MetricParam::Segments),
    };
    let (list, truncated) = metrics::net_metrics(ecad, &nets, &page, want);
    let out: Vec<NetOut> = list
        .into_iter()
        .map(|m| NetOut {
            net: m.net,
            role: m.role.map(str::to_string),
            tier: m.tier.map(str::to_string),
            regime: m.regime,
            length_mm: round(m.length_mm),
            by_layer: m.by_layer.into_iter().map(|(l, v)| (l.name(), round(v))).collect(),
            delay_ps: round(m.delay_ps),
            vias: m.vias,
            bends: BendsOut { deg45: m.bends.deg45, deg90: m.bends.deg90, acute: m.bends.acute, other: m.bends.other, any_angle: m.bends.any_angle },
            segments: m
                .segments
                .into_iter()
                .map(|x| SegmentOut {
                    track: x.track,
                    layer: x.layer.name(),
                    width: round(x.width),
                    length_mm: round(x.length),
                    delay_ps: round(x.delay_ps),
                    z0: x.z0.map(round),
                    reference: x.reference.map(|l| l.name()),
                    ref_net: x.ref_net,
                    ref_coverage: x.ref_coverage.map(round),
                    h: x.h.map(round),
                })
                .collect(),
            segments_more: m.segments_more,
            neighbours: m
                .neighbours
                .into_iter()
                .map(|x| NeighbourOut { net: x.net, layer: x.layer.name(), coupled_mm: round(x.coupled_mm), min_gap_mm: round(x.min_gap_mm) })
                .collect(),
            pair: m.pair.map(|x| PairOut { name: x.name, partner: x.partner, skew_mm: round(x.skew_mm), skew_ps: round(x.skew_ps) }),
        })
        .collect();
    reply(NetInspectResult { nets: out, more, truncated, param_warnings: Vec::new() })
}

fn loop_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.loop.inspect";
    let p: LoopInspectParams = parse(CMD, p)?;
    let limit = p.limit.unwrap_or(DEFAULT_LIMIT);
    if limit > metrics::MAX_LOOPS {
        return Err(bad(CMD, format!("`limit` is at most {}", metrics::MAX_LOOPS)));
    }
    let offset = p.offset.unwrap_or(0);
    if p.loops.as_ref().is_some_and(|l| l.len() > metrics::MAX_LOOPS) {
        return Err(bad(CMD, format!("`loops` names at most {} loops", metrics::MAX_LOOPS)));
    }
    let ecad = &s.doc.ecad;
    if let Some(part) = &p.part
        && !ecad.schematic.parts.iter().any(|x| x.designator == *part)
    {
        let mut msg = format!("`part`: the design has no part `{part}`");
        if let Some(near) = closest(part, ecad.schematic.parts.iter().map(|x| x.designator.as_str())) {
            msg.push_str(&format!(" (did you mean `{near}`?)"));
        }
        return Err(bad(CMD, msg));
    }
    let kinds = p.kind.map(|k| match k {
        LoopKindParam::Decap => vec![LoopKind::Decap],
        LoopKindParam::Hot => vec![LoopKind::BuckHot, LoopKind::BoostHot],
        LoopKindParam::BuckHot => vec![LoopKind::BuckHot],
        LoopKindParam::BoostHot => vec![LoopKind::BoostHot],
        LoopKindParam::Gate => vec![LoopKind::Gate],
        LoopKindParam::Crystal => vec![LoopKind::Crystal],
        LoopKindParam::Tvs => vec![LoopKind::Tvs],
        LoopKindParam::Sense => vec![LoopKind::Sense],
    });
    // The hull's ground looked at as finely as pi.smps_hot_loop looks, so the two agree.
    let grid = solidcraft_doc::ecad::quality::param_value(&ecad.board, "pi.smps_hot_loop", "grid");
    let filter = LoopFilter { names: p.loops.map(|l| l.into_iter().collect()), kinds, part: p.part, grid: Some(grid) };
    let nets = s.board_nets();
    let (mut list, truncated) = metrics::loop_metrics(ecad, &nets, &filter);
    list.sort_by_key(|l| l.rank);
    let total = list.len();
    let loops: Vec<LoopOut> = list
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|l| LoopOut {
            name: l.name,
            kind: l.kind.to_string(),
            from: l.from.to_string(),
            members: l.members.into_iter().map(|m| MemberOut { pin: m.pin, at: m.at.map(|a| [round(a[0]), round(a[1])]), net: m.net }).collect(),
            path: l.path.into_iter().map(|a| [round(a[0]), round(a[1])]).collect(),
            length_mm: round(l.length_mm),
            area_mm2: round(l.area_mm2),
            vias: l.vias,
            layers: l.layers.into_iter().map(|x| x.name()).collect(),
            h: l.h.map(round),
            ref_coverage: l.ref_coverage.map(round),
            ground_under: l.ground_under.map(round),
            est_nh: l.est_nh.map(round),
            rank: l.rank,
            missing: l.missing,
        })
        .collect();
    let more = total.saturating_sub(offset).saturating_sub(loops.len());
    reply(LoopInspectResult { loops, more, truncated, param_warnings: Vec::new() })
}
