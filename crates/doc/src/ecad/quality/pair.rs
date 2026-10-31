//! Differential pairs: whether the two nets of a kept pair record (see [`super::roles`]) run as
//! one line, and match in delay.
//! - `si.pair`: the gap between the members outside ±`gap_tol` of their net class's
//!   `diff_pair` gap on parallel runs; a member uncoupled for more than `end` at an end (its
//!   breakout, a connector's escape left out) or for more than `uncoupled` of its length away
//!   from its ends; unequal via counts (an error at T2 and faster); in-line parts (a two-pad part
//!   on one member: series resistors, AC capacitors, ESD diodes; a pull-up or bias to a supply is
//!   none) not side by side with their partner on the other, or of another package, or with none;
//!   at T2 and faster, bends with an interior angle below `bend` (info; not applicable while the
//!   pair's tier is unverified). Critical at T2 and faster. A connector's escape is no uncoupled
//!   end (its part's connector record, else a J, P, CN or CON designator).
//! - `si.skew`: the members' delay difference, as millimetres of the pair's own line, above the
//!   budget: the pair record's `skew_mm`, else 2.5 mm (USB 2.0, and any pair below T3), else 5
//!   mil at T3; never an error. Delays, not lengths, are compared (layers of other εr, via
//!   barrels). With no difference at the ends, a running mismatch along the way above
//!   max(budget, `running` × the edge's length) is an info: the compensation is far from what
//!   it compensates. Critical at T3.
//!
//! **The usb2 packs** (owner `signal-integrity#pairs`): a full-speed USB pair's edges are 4–20
//! ns, so a run shorter than `si.pair.fs_note` is lumped and every finding of both codes is a note
//! (info). A high-speed one is T2: its pair findings are critical, a member with more than
//! `max_vias` vias is a warning, and so is a capacitor in series on it. Both warn on skew at 2.5
//! mm, never an error. The rest of the packs (impedance, board delay, stubs, spacing) are other
//! codes'.
//!
//! A pair with no class gap is still checked for coupling, within 3 widths, and listed as not
//! applicable for the gap. A pair with an unrouted member, or naming a net the schematic lacks,
//! is listed as not applicable.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::super::airwires::BoardPad;
use super::super::checks::Severity;
use super::super::drc::{Bound, Class, DrcItem};
use super::super::impedance::{self, LineKind};
use super::super::intent::{Interface, PairIntent, PartRole};
use super::super::layers::CopperLayer;
use super::super::path;
use super::catalog::{Code, Family, Param};
use super::geom::{distance, track_pieces};
use super::roles::{NetInfo, Tier, USB_FS_NS, USB_HS_NS};
use super::{Cx, Draft, num};

const PAIR: &str = "si.pair";
const SKEW: &str = "si.skew";

/// How far apart the members are looked at, in millimetres.
const STEP: f64 = 0.1;
/// A gap deviation shorter than this (a corner's) is not one.
const MIN_RUN: f64 = 0.3;
/// Without a class gap, a member within this many widths of its partner is coupled.
const WIDTHS: f64 = 3.0;
/// ps per mm in a vacuum: 1/c.
const PS_PER_MM: f64 = 3.335_641;
/// Pieces this close to parallel are where the gap is measured (cos 10°).
const PARALLEL: f64 = 0.984_8;

const fn param(name: &'static str, default: f64, units: &'static str, min: f64, max: f64, source: &'static str, about: &'static str) -> Param {
    Param { name, default, units, source, min, max, verify: false, about }
}

const CODES: &[Code] = &[
    Code {
        code: PAIR,
        class: Class::Practical,
        severity: Severity::Warning,
        worst: Some((Class::Critical, Severity::Error)),
        needs: "intent: a pair record (its p and n nets), its interface or the nets' tiers; geometry: the pair's net class diff_pair gap",
        section: "signal-integrity#pairs",
        about: "a differential pair's gap off its class's, a member uncoupled at an end or along the way, unequal via counts, in-line parts not side by side, sharp bends at T2 and faster, a high-speed USB member with too many vias or a series capacitor; critical at T2 and faster",
        principle: "A pair rejects noise and keeps its impedance only while both members run together, alike, the same distance apart; every asymmetry turns part of the signal into common-mode noise.",
        params: &[
            param(
                "gap_tol",
                0.1,
                "ratio",
                0.01,
                1.0,
                "the guide's pair rule: ±10 % of the class gap",
                "the share the gap may stray from the class's on parallel runs",
            ),
            param(
                "end",
                6.35,
                "mm",
                0.0,
                100.0,
                "TI SPRAAR7J: break out within 6.35 mm (250 mil)",
                "the longest a member may run uncoupled at an end",
            ),
            param(
                "uncoupled",
                0.1,
                "ratio",
                0.0,
                1.0,
                "the guide's pair rule: 10 % of the length",
                "the share of a member's length it may run uncoupled away from its ends",
            ),
            param(
                "offset",
                0.5,
                "mm",
                0.0,
                50.0,
                "TI SPRAAR7J: in-line parts side by side",
                "how far along the pair two partner in-line parts may be apart",
            ),
            param("bend", 135.0, "°", 90.0, 180.0, "TI SPRAAR7J; USB.org", "the sharpest interior angle a bend may have at T2 and faster"),
            param("window", 2.0, "× gap", 1.0, 20.0, "judgment", "a member within this many class gaps (edge to edge) of its partner is coupled"),
            param(
                "fs_note",
                100.0,
                "mm",
                0.0,
                10000.0,
                "the usb2_fs pack",
                "a full-speed USB pair shorter than this is lumped: its findings are notes",
            ),
            param(
                "max_vias",
                4.0,
                "vias",
                0.0,
                100.0,
                "TI: at most 4 vias per track (the usb2_hs pack)",
                "the most vias a member of a high-speed USB pair may have",
            ),
        ],
    },
    Code {
        code: SKEW,
        class: Class::Practical,
        severity: Severity::Warning,
        worst: Some((Class::Critical, Severity::Warning)),
        needs: "intent: a pair record (its p and n nets), its interface or skew_mm; geometry: the stack-up",
        section: "signal-integrity#pairs",
        about: "a differential pair's members differ in delay by more than the budget (as millimetres of its line); a running mismatch far from its compensation (info); critical at T3",
        principle: "Skew inside a pair turns part of the differential edge into common-mode noise, which closes the eye and radiates from cables; compensation close to the mismatch keeps the pair balanced along its length.",
        params: &[
            param(
                "budget",
                2.5,
                "mm",
                0.01,
                100.0,
                "Renesas: USB 2.0 warns at 2.5 mm; the pair's skew_mm wins",
                "the most delay difference below T3, as millimetres of the pair's line",
            ),
            param("fast", 0.127, "mm", 0.01, 100.0, "TI: 5 mil at 5 Gb/s and up", "the same at T3"),
            param(
                "running",
                0.1,
                "× edge",
                0.0,
                10.0,
                "the SI guide: compensate within a tenth of the edge's length",
                "the running mismatch may reach this share of the edge's length (rise time × speed) or the budget, whichever is more",
            ),
        ],
    },
];

pub(super) const FAMILY: Family = Family { module: "pair", title: "differential pairs", codes: CODES, run };

/// One straight piece of a member's copper.
struct Piece {
    track: u64,
    layer: CopperLayer,
    a: [f64; 2],
    b: [f64; 2],
    width: f64,
}

/// A point along a member, at the middle of a step `w` long.
struct Sample {
    at: [f64; 2],
    dir: [f64; 2],
    w: f64,
    piece: usize,
    /// The track's end it is the first or last step of, if any, and whether it is its last.
    end: Option<([f64; 2], bool)>,
}

/// One net of a pair, as routed.
struct Member<'a> {
    info: &'a NetInfo<'a>,
    pieces: Vec<Piece>,
    length: f64,
    /// Along its tracks and through its vias' barrels, in ps.
    delay: f64,
    vias: usize,
    pads: Vec<&'a BoardPad>,
}

/// What a walk along a member against its partner found.
#[derive(Default)]
struct Walk {
    /// Length off its own pads.
    length: f64,
    /// Uncoupled away from its ends.
    interior: f64,
    /// The longest uncoupled run at an end, and where it is.
    end: Option<(f64, [f64; 2], u64)>,
    /// The gap's worst stray on parallel runs: the gap, where, the two tracks; and how long it strays.
    worst: Option<(f64, [f64; 2], u64, u64)>,
    strays: f64,
}

fn run(cx: &mut Cx) {
    let pairs = cx.roles.pairs();
    if pairs.is_empty() {
        for code in [PAIR, SKEW] {
            if cx.wants(code) {
                cx.not_applicable(code, "intent: pairs (the p and n nets of each differential pair)");
            }
        }
        return;
    }
    for pair in pairs {
        if cx.spent() {
            return;
        }
        check_pair(cx, pair);
    }
}

/// ps per mm of a `width` track on `layer`: 1/c × √εr_eff by the stack-up's formula.
fn ps_per_mm(cx: &Cx, layer: CopperLayer, width: f64) -> f64 {
    let er = match impedance::medium(&cx.geo.stackup, layer, cx.board.copper_layers) {
        Ok(m) => {
            let kind = if m.outer { LineKind::Microstrip } else { LineKind::Stripline };
            impedance::impedance(&m, kind, width, None).ok().and_then(|z| z.er_eff).unwrap_or(m.er)
        }
        Err(_) => super::super::stackup::DEFAULT_ER,
    };
    PS_PER_MM * er.max(1.0).sqrt()
}

/// `info`'s copper: its tracks' pieces, length, delay (through its vias' barrels too) and vias.
fn member<'a>(cx: &mut Cx<'a>, info: &'a NetInfo<'a>) -> Option<Member<'a>> {
    let (board, nets, geo) = (cx.board, cx.nets, cx.geo);
    let mut pieces = Vec::new();
    let (mut length, mut delay) = (0.0, 0.0);
    for (ti, t) in board.tracks.iter().enumerate() {
        if nets.track_nets.get(ti).copied().flatten() != Some(info.index) {
            continue;
        }
        let parts = track_pieces(&t.points, &t.curves);
        if !cx.spend(parts.len().saturating_add(1)) {
            return None;
        }
        let v = ps_per_mm(cx, t.layer, t.width);
        for (a, b) in parts {
            let len = distance(a, b);
            length += len;
            delay += len * v;
            pieces.push(Piece { track: t.id, layer: t.layer, a, b, width: t.width });
        }
    }
    let mut vias = 0usize;
    for (vi, v) in board.vias.iter().enumerate() {
        if nets.via_nets.get(vi).copied().flatten() != Some(info.index) {
            continue;
        }
        vias += 1;
        // The barrel between the layers its tracks leave it on.
        let reach = v.diameter / 2.0 + 0.01;
        let on: BTreeSet<CopperLayer> =
            pieces.iter().filter(|p| distance(p.a, v.at) <= reach || distance(p.b, v.at) <= reach).map(|p| p.layer).collect();
        if let (Some(top), Some(bottom)) = (on.first(), on.last())
            && top != bottom
        {
            let h = geo.height(*top, *bottom);
            delay += h * PS_PER_MM * geo.er(*top, *bottom).max(1.0).sqrt();
        }
    }
    let pads = nets.pads.iter().filter(|p| p.net == Some(info.index) && (p.top || p.bottom)).collect();
    Some(Member { info, pieces, length, delay, vias, pads })
}

/// The steps along a member's pieces, track by track in order.
fn samples(pieces: &[Piece]) -> Vec<Sample> {
    let mut out = Vec::new();
    for (k, p) in pieces.iter().enumerate() {
        let len = distance(p.a, p.b);
        if len <= 1e-9 {
            continue;
        }
        let dir = [(p.b[0] - p.a[0]) / len, (p.b[1] - p.a[1]) / len];
        let n = ((len / STEP).ceil() as usize).clamp(1, super::geom::MAX_SAMPLES);
        let w = len / n as f64;
        let first = k.checked_sub(1).and_then(|j| pieces.get(j)).is_none_or(|q| q.track != p.track);
        let last = pieces.get(k + 1).is_none_or(|q| q.track != p.track);
        for i in 0..n {
            let t = (i as f64 + 0.5) / n as f64;
            let end = if i + 1 == n && last {
                Some((p.b, true))
            } else if i == 0 && first {
                Some((p.a, false))
            } else {
                None
            };
            out.push(Sample { at: [p.a[0] + (p.b[0] - p.a[0]) * t, p.a[1] + (p.b[1] - p.a[1]) * t], dir, w, piece: k, end });
        }
    }
    out
}

/// The cost of [`samples`] before it is made.
fn sample_count(pieces: &[Piece]) -> usize {
    pieces.iter().map(|p| ((distance(p.a, p.b) / STEP).ceil() as usize).clamp(1, super::geom::MAX_SAMPLES)).fold(0, usize::saturating_add)
}

/// The partner's piece nearest `s` on its layer: the edge gap, whether the two run parallel with
/// `s` across from the piece's inside (where a gap is a gap), and the piece.
fn partner(s: &Sample, own: &Piece, other: &[Piece]) -> Option<(f64, bool, usize)> {
    let mut best: Option<(f64, bool, usize)> = None;
    for (k, q) in other.iter().enumerate() {
        if q.layer != own.layer {
            continue;
        }
        let (foot, t) = path::project(s.at, q.a, q.b, 0.0);
        let gap = distance(s.at, foot) - (own.width + q.width) / 2.0;
        if best.is_none_or(|(g, _, _)| gap < g) {
            let len = distance(q.a, q.b);
            let parallel = len > 1e-9 && {
                let d = [(q.b[0] - q.a[0]) / len, (q.b[1] - q.a[1]) / len];
                (s.dir[0] * d[0] + s.dir[1] * d[1]).abs() >= PARALLEL
            };
            best = Some((gap, parallel && t > 1e-6 && t < 1.0 - 1e-6, k));
        }
    }
    best
}

/// Walks `own` against `other`: coupled where the partner is within `window` (edge to edge),
/// the gap's strays from `gap` on parallel runs when `gap` is known. Steps on the member's own
/// pads count for nothing; an uncoupled run next to a pad of a connector (`escape`) neither.
fn walk(cx: &mut Cx, own: &Member, other: &Member, window: f64, gap: Option<(f64, f64)>, escape: &BTreeSet<u64>) -> Option<Walk> {
    if !cx.spend(sample_count(&own.pieces).saturating_mul(other.pieces.len().saturating_add(own.pads.len()).saturating_add(1))) {
        return None;
    }
    let pts = samples(&own.pieces);
    let pad_at = |p: [f64; 2]| own.pads.iter().find(|q| q.contains(p));
    let mut out = Walk::default();
    // The uncoupled run going on: its length, its start, its track, and the pad it starts at.
    let mut open: Option<(f64, [f64; 2], u64, Option<u64>)> = None;
    let mut stray: Option<(f64, f64, [f64; 2], u64, u64)> = None;
    let close_run = |out: &mut Walk, open: &mut Option<(f64, [f64; 2], u64, Option<u64>)>, pad_after: Option<u64>| {
        if let Some((len, at, track, pad_before)) = open.take() {
            let pad = pad_before.or(pad_after);
            match pad {
                Some(fp) if escape.contains(&fp) => {}
                Some(_) => {
                    if out.end.is_none_or(|(l, _, _)| len > l) {
                        out.end = Some((len, at, track));
                    }
                }
                None => out.interior += len,
            }
        }
    };
    let close_stray = |out: &mut Walk, stray: &mut Option<(f64, f64, [f64; 2], u64, u64)>| {
        if let Some((len, g, at, a, b)) = stray.take()
            && len >= MIN_RUN
        {
            out.strays += len;
            let off = |x: f64| gap.map_or(0.0, |(want, _)| (x - want).abs());
            if out.worst.is_none_or(|(w, _, _, _)| off(g) > off(w)) {
                out.worst = Some((g, at, a, b));
            }
        }
    };
    let mut prev_track: Option<u64> = None;
    let mut prev_pad: Option<u64> = None;
    for s in &pts {
        let Some(p) = own.pieces.get(s.piece) else { continue };
        if prev_track != Some(p.track) {
            // A new track: what was going on ends at the old one's end.
            close_run(&mut out, &mut open, None);
            close_stray(&mut out, &mut stray);
            prev_pad = None;
            prev_track = Some(p.track);
        }
        if let Some(pad) = pad_at(s.at) {
            close_run(&mut out, &mut open, Some(pad.footprint));
            close_stray(&mut out, &mut stray);
            prev_pad = Some(pad.footprint);
            continue;
        }
        out.length += s.w;
        // A track starting or ending just off a pad (on its edge) is at that pad too.
        let end_pad = s.end.and_then(|(e, _)| own.pads.iter().find(|q| q.contains(e) || distance(q.at(), e) <= 0.05)).map(|q| q.footprint);
        let near = partner(s, p, &other.pieces);
        let coupled = near.is_some_and(|(g, _, _)| g <= window);
        if coupled {
            close_run(&mut out, &mut open, None);
        } else {
            match open.as_mut() {
                Some(run) => run.0 += s.w,
                None => open = Some((s.w, s.at, p.track, prev_pad.or(end_pad))),
            }
            if s.end.is_some_and(|(_, last)| last) && end_pad.is_some() {
                close_run(&mut out, &mut open, end_pad);
            }
        }
        prev_pad = None;
        // The gap on parallel runs.
        match (gap, near) {
            (Some((want, tol)), Some((g, true, k))) if coupled && (g - want).abs() > tol * want => {
                let q = other.pieces.get(k).map_or(0, |q| q.track);
                match stray.as_mut() {
                    Some(run) => {
                        run.0 += s.w;
                        if (g - want).abs() > (run.1 - want).abs() {
                            (run.1, run.2) = (g, s.at);
                        }
                    }
                    None => stray = Some((s.w, g, s.at, p.track, q)),
                }
            }
            _ => close_stray(&mut out, &mut stray),
        }
    }
    close_run(&mut out, &mut open, None);
    close_stray(&mut out, &mut stray);
    Some(out)
}

/// The pair's tier: its interface's, else its faster member's.
fn tier_of(interface: Option<Interface>, p: &NetInfo, n: &NetInfo) -> Option<Tier> {
    match interface {
        Some(Interface::Usb2Hs) => Some(Tier::T2),
        Some(Interface::Usb2Fs) => Some(Tier::T0),
        _ => p.tier.max(n.tier),
    }
}

fn check_pair(cx: &mut Cx, pair: &PairIntent) {
    let roles = cx.roles;
    let (Some(pi), Some(ni)) = (roles.by_name(&pair.p), roles.by_name(&pair.n)) else {
        for code in [PAIR, SKEW] {
            if cx.wants(code) {
                cx.not_applicable(code, format!("intent: pair {} names a net the schematic lacks ({} / {})", pair.name, pair.p, pair.n));
            }
        }
        return;
    };
    let (Some(p), Some(n)) = (member(cx, pi), member(cx, ni)) else { return };
    if p.pieces.is_empty() || n.pieces.is_empty() {
        for code in [PAIR, SKEW] {
            if cx.wants(code) {
                cx.not_applicable(code, format!("geometry: both members of pair {} routed", pair.name));
            }
        }
        return;
    }
    let interface = pair.interface.or(pi.record.and_then(|r| r.interface)).or(ni.record.and_then(|r| r.interface));
    let tier = tier_of(interface, pi, ni);
    let fs_note = super::param_value(cx.board, PAIR, "fs_note");
    let longest = p.length.max(n.length);
    // The usb2_fs pack: a short full-speed pair is lumped.
    let note = interface == Some(Interface::Usb2Fs) && longest < fs_note;
    let kind = interface.map_or("differential", |i| i.name());
    let speed = match (tier, tier_from_interface(interface)) {
        (Some(t), true) => format!("{} ({kind} edges)", t.name()),
        (Some(_), false) if pi.tier >= ni.tier => pi.regime(),
        (Some(_), false) => ni.regime(),
        (None, _) => "unverified".into(),
    };
    let regime = if note {
        format!("{kind} pair, {speed}; {} mm long, under {} mm: lumped, a note", num(longest.round()), num(fs_note))
    } else {
        format!("{kind} pair, {speed}; {} mm long", num(longest.round()))
    };
    if cx.wants(PAIR) {
        pair_geometry(cx, pair, &p, &n, interface, tier, note, &regime);
    }
    if cx.wants(SKEW) {
        skew(cx, pair, &p, &n, interface, tier, note, &regime);
    }
}

fn tier_from_interface(i: Option<Interface>) -> bool {
    matches!(i, Some(Interface::Usb2Hs | Interface::Usb2Fs))
}

/// The class, and the severity in place of `usual`, of a pair finding.
fn grade(fast: bool, note: bool, usual: Severity) -> (Class, Severity) {
    let class = if fast { Class::Critical } else { Class::Practical };
    (class, if note { Severity::Info } else { usual })
}

/// The pair's class gap, if its net class has one, and how far apart (edge to edge) its members
/// count as coupled: `window` class gaps, else [`WIDTHS`] widths.
fn coupling(cx: &Cx, p: &Member, n: &Member) -> (Option<f64>, f64) {
    let rules = &cx.board.rules;
    let class_gap = rules.sizes(Some(p.info.name)).diff_pair.or(rules.sizes(Some(n.info.name)).diff_pair).map(|d| d.gap).filter(|g| *g > 0.0);
    let width = p.pieces.iter().chain(&n.pieces).map(|q| q.width).fold(0.0_f64, f64::max);
    (class_gap, class_gap.map_or(WIDTHS * width, |g| super::param_value(cx.board, PAIR, "window") * g))
}

/// Whether a designator names a connector: its part's connector record, else its prefix.
fn connector(cx: &Cx, designator: &str) -> bool {
    let prefix: String = designator.chars().take_while(char::is_ascii_alphabetic).collect::<String>().to_ascii_uppercase();
    matches!(prefix.as_str(), "J" | "P" | "CN" | "CON") || cx.roles.parts(PartRole::Connector).any(|r| r.part == designator)
}

#[allow(clippy::too_many_arguments)]
fn pair_geometry(cx: &mut Cx, pair: &PairIntent, p: &Member, n: &Member, interface: Option<Interface>, tier: Option<Tier>, note: bool, regime: &str) {
    let board = cx.board;
    let prm = |name: &str| super::param_value(board, PAIR, name);
    let (gap_tol, end_max, uncoupled, offset_max, bend_min, max_vias) =
        (prm("gap_tol"), prm("end"), prm("uncoupled"), prm("offset"), prm("bend"), prm("max_vias"));
    let fast = tier.is_some_and(|t| t >= Tier::T2);
    let hs = interface == Some(Interface::Usb2Hs);
    let (class_gap, reach) = coupling(cx, p, n);
    if class_gap.is_none() {
        cx.not_applicable(PAIR, format!("geometry: a net class with a diff_pair gap for {} and {}", p.info.name, n.info.name));
    }
    let escape: BTreeSet<u64> = p.pads.iter().chain(&n.pads).filter(|q| connector(cx, &q.designator)).map(|q| q.footprint).collect();
    let gap = class_gap.map(|g| (g, gap_tol));
    let Some(wp) = walk(cx, p, n, reach, gap, &escape) else { return };
    let Some(wn) = walk(cx, n, p, reach, None, &escape) else { return };
    let name = pair.name.clone();
    let both = vec![p.info.name.to_string(), n.info.name.to_string()];
    // The gap.
    if let (Some(want), Some((g, at, ta, tb))) = (class_gap, wp.worst) {
        let (class, severity) = grade(fast, note, Severity::Warning);
        let bound = if g < want { Bound::Min } else { Bound::Max };
        let limit = if g < want { want * (1.0 - gap_tol) } else { want * (1.0 + gap_tol) };
        let (strays, name, both, regime) = (wp.strays, name.clone(), both.clone(), regime.to_string());
        cx.add(PAIR, &[format!("pair:{name}"), "gap".into()], || {
            Draft::new(
                at,
                vec![DrcItem::Track(ta), DrcItem::Track(tb)],
                format!(
                    "pair {name}: tracks {ta} and {tb} run {} mm apart, against the class gap of {} mm ±{} %, over {} mm",
                    num(g),
                    num(want),
                    num(gap_tol * 100.0),
                    num(strays)
                ),
            )
            .nets(both)
            .class(class)
            .severity(severity)
            .measure(g, limit, "mm", bound)
            .param("gap_tol")
            .regime(regime)
            .arg("pair", name.clone())
            .arg("gap", num(want))
            .arg("length", num(strays))
            .fix(
                "ecad.board.net.inspect",
                json!({"pair": name, "metrics": ["coupling", "segments"]}),
                format!("see where pair {name}'s gap strays; reroute both members together at the class gap"),
            )
        });
    }
    // Uncoupled: at an end, and along the way.
    for (m, w, other) in [(p, &wp, n), (n, &wn, p)] {
        if let Some((len, at, track)) = w.end
            && len > end_max
        {
            let (class, severity) = grade(fast, note, Severity::Warning);
            let (name, member, partner, regime) = (name.clone(), m.info.name.to_string(), other.info.name.to_string(), regime.to_string());
            cx.add(PAIR, &[format!("pair:{name}"), format!("end:{member}")], || {
                Draft::new(
                    at,
                    vec![DrcItem::Track(track)],
                    format!("pair {name}: {member} runs {} mm uncoupled from {partner} at an end, more than {} mm", num(len), num(end_max)),
                )
                .nets(vec![member.clone(), partner.clone()])
                .class(class)
                .severity(severity)
                .variant("end")
                .measure(len, end_max, "mm", Bound::Max)
                .param("end")
                .regime(regime)
                .arg("pair", name.clone())
                .arg("member", member.clone())
                .arg("partner", partner.clone())
                .fix(
                    "ecad.board.net.inspect",
                    json!({"pair": name, "metrics": ["coupling", "segments"]}),
                    format!("bring {member} and {partner} together closer to their pads"),
                )
            });
        }
        let limit = uncoupled * w.length;
        if w.length > 0.0 && w.interior > limit + 1e-9 {
            let (class, severity) = grade(fast, note, Severity::Warning);
            let (name, member, partner, regime) = (name.clone(), m.info.name.to_string(), other.info.name.to_string(), regime.to_string());
            let at = m.pieces.first().map_or([0.0, 0.0], |q| q.a);
            let track = m.pieces.first().map_or(0, |q| q.track);
            let (interior, length) = (w.interior, w.length);
            cx.add(PAIR, &[format!("pair:{name}"), format!("uncoupled:{member}")], || {
                Draft::new(
                    at,
                    vec![DrcItem::Track(track)],
                    format!(
                        "pair {name}: {member} runs {} mm uncoupled from {partner} away from its ends, {} % of its {} mm (at most {} %)",
                        num(interior),
                        num((interior / length * 100.0).round()),
                        num(length),
                        num(uncoupled * 100.0)
                    ),
                )
                .nets(vec![member.clone(), partner.clone()])
                .class(class)
                .severity(severity)
                .variant("uncoupled")
                .measure(interior, limit, "mm", Bound::Max)
                .param("uncoupled")
                .regime(regime)
                .arg("pair", name.clone())
                .arg("member", member.clone())
                .arg("partner", partner.clone())
                .arg("percent", num((interior / length * 100.0).round()))
                .fix(
                    "ecad.board.net.inspect",
                    json!({"pair": name, "metrics": ["coupling", "segments"]}),
                    format!("route {member} beside {partner} the whole way"),
                )
            });
        }
    }
    // Via counts.
    if p.vias != n.vias {
        let (class, severity) = grade(fast, note, if fast { Severity::Error } else { Severity::Warning });
        let (name, pn, nn, regime) = (name.clone(), p.info.name.to_string(), n.info.name.to_string(), regime.to_string());
        let (pv, nv) = (p.vias, n.vias);
        let at = p.pieces.first().map_or([0.0, 0.0], |q| q.a);
        cx.add(PAIR, &[format!("pair:{name}"), "vias".into()], || {
            Draft::new(at, Vec::new(), format!("pair {name}: {pn} has {pv} via{} and {nn} has {nv}", if pv == 1 { "" } else { "s" }))
                .nets(vec![pn.clone(), nn.clone()])
                .class(class)
                .severity(severity)
                .variant("vias")
                .measure(pv.abs_diff(nv) as f64, 0.0, "vias", Bound::Max)
                .param("")
                .regime(regime)
                .arg("pair", name.clone())
                .arg("p", pn.clone())
                .arg("n", nn.clone())
                .arg("p_vias", pv.to_string())
                .arg("n_vias", nv.to_string())
                .fix(
                    "ecad.board.net.inspect",
                    json!({"pair": name, "metrics": ["vias", "layers"]}),
                    "give both members the same layer changes, with their vias side by side",
                )
        });
    }
    // The usb2_hs pack: a member's via budget.
    if hs {
        for (m, other) in [(p, n), (n, p)] {
            if (m.vias as f64) <= max_vias + 1e-9 {
                continue;
            }
            let (class, severity) = grade(fast, note, Severity::Warning);
            let (name, member, partner, regime) = (name.clone(), m.info.name.to_string(), other.info.name.to_string(), regime.to_string());
            let (count, at) = (m.vias, m.pieces.first().map_or([0.0, 0.0], |q| q.a));
            cx.add(PAIR, &[format!("pair:{name}"), format!("via_budget:{member}")], || {
                Draft::new(
                    at,
                    Vec::new(),
                    format!("pair {name}: {member} has {count} vias, more than the {} a high-speed USB track may have", num(max_vias)),
                )
                .nets(vec![member.clone(), partner.clone()])
                .class(class)
                .severity(severity)
                .variant("via_budget")
                .measure(count as f64, max_vias, "vias", Bound::Max)
                .param("max_vias")
                .regime(regime)
                .arg("pair", name.clone())
                .arg("member", member.clone())
                .fix(
                    "ecad.board.net.inspect",
                    json!({"pair": name, "metrics": ["vias", "layers"]}),
                    format!("route {member} and {partner} with fewer layer changes"),
                )
            });
        }
    }
    in_line(cx, pair, p, n, (fast, hs, note), offset_max, regime);
    // Bends, at T2 and faster; a tier-gated rule, so not applicable while the tier is unknown.
    if tier.is_none() {
        cx.not_applicable(PAIR, format!("intent: driver, edge_ns or interface of pair {name} (for its T2 bend rule)"));
    }
    if fast {
        let mut count = 0usize;
        let mut sharpest: Option<(f64, [f64; 2], u64)> = None;
        for m in [p, n] {
            for w in m.pieces.windows(2) {
                let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
                if a.track != b.track {
                    continue;
                }
                let (la, lb) = (distance(a.a, a.b), distance(b.a, b.b));
                if la <= 1e-9 || lb <= 1e-9 {
                    continue;
                }
                let cos = ((a.b[0] - a.a[0]) * (b.b[0] - b.a[0]) + (a.b[1] - a.a[1]) * (b.b[1] - b.a[1])) / (la * lb);
                let interior = 180.0 - cos.clamp(-1.0, 1.0).acos().to_degrees();
                if interior < bend_min - 0.5 {
                    count += 1;
                    if sharpest.is_none_or(|(s, _, _)| interior < s) {
                        sharpest = Some((interior, a.b, a.track));
                    }
                }
            }
        }
        if let Some((angle, at, track)) = sharpest {
            let (name, both, regime) = (name.clone(), both.clone(), regime.to_string());
            cx.add(PAIR, &[format!("pair:{name}"), "bends".into()], || {
                Draft::new(
                    at,
                    vec![DrcItem::Track(track)],
                    format!(
                        "pair {name}: {count} bend{} sharper than {}° inside, the sharpest {}°",
                        if count == 1 { "" } else { "s" },
                        num(bend_min),
                        num(angle.round())
                    ),
                )
                .nets(both)
                .class(Class::Critical)
                .severity(Severity::Info)
                .variant("bends")
                .measure(angle, bend_min, "°", Bound::Min)
                .param("bend")
                .regime(regime)
                .arg("pair", name.clone())
                .arg("count", count.to_string())
                .fix(
                    "ecad.board.track.miter",
                    Value::Null,
                    "bend the pair in two 45° steps or an arc, turning left and right in turn",
                )
            });
        }
    }
}

/// A two-pad part on one member: its footprint, designator, package, centre, the pad on the
/// member, and whether its other pad goes on to a signal (in series, not a shunt to ground).
struct InLine {
    footprint: u64,
    designator: String,
    package: String,
    at: [f64; 2],
    pad: [f64; 2],
    series: bool,
}

/// Whether a net is a ground: kept as one, else named like one.
fn ground_like(cx: &Cx, info: &NetInfo) -> bool {
    let name = info.name.to_ascii_uppercase();
    cx.roles.grounds.contains(&info.index) || name.starts_with("GND") || name.ends_with("GND") || name == "VSS" || name == "0V"
}

/// The capacitors in series on either member, with the member and its partner.
fn ps_ns_caps<'s, 'm>(sides: &'s [Vec<InLine>; 2], p: &'m Member<'m>, n: &'m Member<'m>) -> Vec<(&'s InLine, &'m Member<'m>, &'m Member<'m>)> {
    let cap = |a: &InLine| a.series && a.designator.chars().take_while(char::is_ascii_alphabetic).collect::<String>().eq_ignore_ascii_case("C");
    let [ps, ns] = sides;
    ps.iter().filter(|a| cap(a)).map(|a| (a, p, n)).chain(ns.iter().filter(|a| cap(a)).map(|a| (a, n, p))).collect()
}

/// `grade_by` is (T2 or faster, the usb2_hs pack, the usb2_fs note).
fn in_line(cx: &mut Cx, pair: &PairIntent, p: &Member, n: &Member, grade_by: (bool, bool, bool), offset_max: f64, regime: &str) {
    let (fast, hs, note) = grade_by;
    let (board, nets, ecad, roles) = (cx.board, cx.nets, cx.ecad, cx.roles);
    if !cx.spend(nets.pads.len().saturating_add(board.footprints.len())) {
        return;
    }
    let (pn, nn) = (p.info.index, n.info.index);
    let mut sides: [Vec<InLine>; 2] = [Vec::new(), Vec::new()];
    for f in &board.footprints {
        let pads: Vec<&BoardPad> = nets.pads.iter().filter(|q| q.footprint == f.id && (q.top || q.bottom)).collect();
        if pads.len() != 2 {
            continue;
        }
        let on = |net: usize| pads.iter().find(|q| q.net == Some(net));
        let side = match (on(pn), on(nn)) {
            (Some(q), None) => Some((0usize, q.at(), pn)),
            (None, Some(q)) => Some((1usize, q.at(), nn)),
            _ => None,
        };
        let Some((k, pad, own)) = side else { continue };
        // The other pad's net: a supply's (not ground's) makes it a pull-up or bias, no in-line part.
        let other = pads.iter().find(|q| q.net != Some(own)).and_then(|q| q.net).and_then(|i| roles.net(i));
        if other.is_some_and(|i| i.is_supply() && !ground_like(cx, i)) {
            continue;
        }
        let Some(placed) = ecad.placed(f) else { continue };
        let series = other.is_some_and(|i| !i.is_supply() && !ground_like(cx, i));
        let item = InLine {
            footprint: f.id,
            designator: placed.part.designator.clone(),
            package: placed.resolved.footprint.name.clone(),
            at: f.at,
            pad,
            series,
        };
        if let Some(list) = sides.get_mut(k) {
            list.push(item);
        }
    }
    // The usb2_hs pack: no series capacitor.
    if hs {
        for (a, m, other) in ps_ns_caps(&sides, p, n) {
            let (class, severity) = grade(fast, note, Severity::Warning);
            let (name, regime, member, partner) = (pair.name.clone(), regime.to_string(), m.info.name.to_string(), other.info.name.to_string());
            let item = DrcItem::Footprint { id: a.footprint, designator: a.designator.clone() };
            cx.add(PAIR, &[format!("pair:{}", pair.name), item.key(), "capacitor".into()], || {
                Draft::new(a.at, vec![item], format!("pair {name}: {} is a capacitor in series on {member}; high-speed USB takes none", a.designator))
                    .nets(vec![member.clone(), partner.clone()])
                    .class(class)
                    .severity(severity)
                    .variant("capacitor")
                    .param("")
                    .regime(regime)
                    .arg("pair", name.clone())
                    .arg("member", member.clone())
                    .fix(
                        "ecad.board.net.inspect",
                        json!({"pair": name, "metrics": ["segments"]}),
                        format!("take {} out of {member} in the schematic and join the line through", a.designator),
                    )
            });
        }
    }
    let [ps, ns] = sides;
    let mut used = vec![false; ns.len()];
    let name = pair.name.clone();
    // The pair's direction at a point: its nearest piece of `m`'s.
    let dir_at = |m: &Member, at: [f64; 2]| {
        m.pieces.iter().min_by(|a, b| path::segment_distance(at, a.a, a.b, 0.0).total_cmp(&path::segment_distance(at, b.a, b.b, 0.0))).and_then(|q| {
            let len = distance(q.a, q.b);
            (len > 1e-9).then(|| [(q.b[0] - q.a[0]) / len, (q.b[1] - q.a[1]) / len])
        })
    };
    let mut unmatched: Vec<(InLine, &Member, &Member)> = Vec::new();
    for a in ps {
        let best = ns
            .iter()
            .enumerate()
            .filter(|(k, _)| !used.get(*k).copied().unwrap_or(true))
            .min_by(|(_, x), (_, y)| distance(x.at, a.at).total_cmp(&distance(y.at, a.at)));
        let Some((k, b)) = best else {
            unmatched.push((a, p, n));
            continue;
        };
        if let Some(u) = used.get_mut(k) {
            *u = true;
        }
        let along = dir_at(p, a.pad).map_or(0.0, |d| ((a.at[0] - b.at[0]) * d[0] + (a.at[1] - b.at[1]) * d[1]).abs());
        let other_package = a.package != b.package;
        if !other_package && along <= offset_max {
            continue;
        }
        let (class, severity) = grade(fast, note, Severity::Warning);
        let (name, regime) = (name.clone(), regime.to_string());
        let both = vec![p.info.name.to_string(), n.info.name.to_string()];
        let keys = [
            format!("pair:{name}"),
            DrcItem::Footprint { id: a.footprint, designator: a.designator.clone() }.key(),
            DrcItem::Footprint { id: b.footprint, designator: b.designator.clone() }.key(),
        ];
        cx.add(PAIR, &keys, || {
            let why = if other_package {
                format!("are different packages ({} and {})", a.package, b.package)
            } else {
                format!("sit {} mm apart along the pair", num(along))
            };
            let mut d = Draft::new(
                a.at,
                vec![
                    DrcItem::Footprint { id: a.footprint, designator: a.designator.clone() },
                    DrcItem::Footprint { id: b.footprint, designator: b.designator.clone() },
                ],
                format!("pair {name}: in-line parts {} and {} {why}", a.designator, b.designator),
            )
            .nets(both)
            .class(class)
            .severity(severity)
            .variant(if other_package { "package" } else { "inline" })
            .regime(regime)
            .arg("pair", name.clone())
            .fix(
                "ecad.board.move",
                Value::Null,
                format!("place {} and {} side by side across the pair, in the same package", a.designator, b.designator),
            );
            if !other_package {
                d = d.measure(along, offset_max, "mm", Bound::Max).param("offset");
            } else {
                d = d.param("");
            }
            d
        });
    }
    for (b, u) in ns.into_iter().zip(used) {
        if !u {
            unmatched.push((b, n, p));
        }
    }
    for (a, m, other) in unmatched {
        let (class, severity) = grade(fast, note, Severity::Warning);
        let (name, regime, member, partner) = (name.clone(), regime.to_string(), m.info.name.to_string(), other.info.name.to_string());
        let item = DrcItem::Footprint { id: a.footprint, designator: a.designator.clone() };
        cx.add(PAIR, &[format!("pair:{name}"), item.key()], || {
            Draft::new(a.at, vec![item], format!("pair {name}: in-line part {} on {member} has no partner on {partner}", a.designator))
                .nets(vec![member.clone(), partner.clone()])
                .class(class)
                .severity(severity)
                .variant("unmatched")
                .param("")
                .regime(regime)
                .arg("pair", name.clone())
                .arg("member", member.clone())
                .arg("partner", partner.clone())
                .review()
                .fix(
                    "ecad.board.move",
                    Value::Null,
                    format!("give {partner} the same part beside {}, or move {} off the pair's run", a.designator, a.designator),
                )
        });
    }
}

/// A member's tracks as one line from end to end, if they make one (no branches): its points.
fn chain(pieces: &[Piece]) -> Option<Vec<[f64; 2]>> {
    // Each track's line, its pieces in order.
    let mut lines: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut widths: Vec<f64> = Vec::new();
    let mut last: Option<u64> = None;
    for q in pieces {
        if last == Some(q.track)
            && let Some(l) = lines.last_mut()
        {
            l.push(q.b);
        } else {
            lines.push(vec![q.a, q.b]);
            widths.push(q.width);
        }
        last = Some(q.track);
    }
    let ends = |l: &Vec<[f64; 2]>| Some((*l.first()?, *l.last()?));
    let meets = |i: usize, p: [f64; 2], j: usize, q: [f64; 2]| {
        let tol = widths.get(i).copied().unwrap_or(0.0).max(widths.get(j).copied().unwrap_or(0.0)) / 2.0 + 0.01;
        distance(p, q) <= tol
    };
    // Each end's partner end (track, which end); two partners is a branch.
    let n = lines.len();
    let mut next: Vec<[Option<(usize, usize)>; 2]> = vec![[None, None]; n];
    for i in 0..n {
        let (a0, a1) = ends(lines.get(i)?)?;
        for j in 0..n {
            if i == j {
                continue;
            }
            let (b0, b1) = ends(lines.get(j)?)?;
            for (ei, pa) in [(0usize, a0), (1usize, a1)] {
                for (ej, pb) in [(0usize, b0), (1usize, b1)] {
                    if meets(i, pa, j, pb) {
                        let slot = next.get_mut(i)?.get_mut(ei)?;
                        if slot.is_some_and(|s| s != (j, ej)) {
                            return None;
                        }
                        *slot = Some((j, ej));
                    }
                }
            }
        }
    }
    let starts: Vec<usize> = (0..n).filter(|i| next.get(*i).is_some_and(|e| e[0].is_none() || e[1].is_none())).collect();
    if starts.len() != 2 && n > 1 {
        return None;
    }
    let first = starts.first().copied().unwrap_or(0);
    let mut out: Vec<[f64; 2]> = Vec::new();
    let mut seen = vec![false; n];
    let mut at = first;
    // Enter the first track at its free end.
    let mut from_end = if next.get(first)?.first()?.is_none() { 0 } else { 1 };
    loop {
        *seen.get_mut(at)? = true;
        let mut l = lines.get(at)?.clone();
        if from_end == 1 {
            l.reverse();
        }
        out.extend(l);
        let leave = if from_end == 0 { 1 } else { 0 };
        match *next.get(at)?.get(leave)? {
            Some((j, ej)) if !seen.get(j).copied().unwrap_or(true) => {
                at = j;
                from_end = ej;
            }
            _ => break,
        }
    }
    (seen.iter().all(|s| *s)).then_some(out)
}

/// Each point's distance along the line from its start.
fn arclength(line: &[[f64; 2]]) -> Vec<f64> {
    let mut out = Vec::with_capacity(line.len());
    let mut s = 0.0;
    let mut prev: Option<[f64; 2]> = None;
    for p in line {
        if let Some(q) = prev {
            s += distance(q, *p);
        }
        out.push(s);
        prev = Some(*p);
    }
    out
}

/// How far along `line` (with arclengths `s`) the point nearest `p` is, and how far `p` is from it.
fn along_line(line: &[[f64; 2]], s: &[f64], p: [f64; 2]) -> Option<(f64, f64)> {
    let mut best: Option<(f64, f64)> = None;
    for (k, w) in line.windows(2).enumerate() {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
        let (foot, t) = path::project(p, *a, *b, 0.0);
        let d = distance(p, foot);
        if best.is_none_or(|(_, bd)| d < bd) {
            let base = s.get(k).copied().unwrap_or(0.0);
            best = Some((base + t * distance(*a, *b), d));
        }
    }
    best
}

#[allow(clippy::too_many_arguments)]
fn skew(cx: &mut Cx, pair: &PairIntent, p: &Member, n: &Member, interface: Option<Interface>, tier: Option<Tier>, note: bool, regime: &str) {
    let board = cx.board;
    let prm = |name: &str| super::param_value(board, SKEW, name);
    let t3 = tier == Some(Tier::T3);
    let (budget, param) = match pair.skew_mm.filter(|v| *v > 0.0) {
        Some(v) => (v, ""),
        None if t3 => (prm("fast"), "fast"),
        None => (prm("budget"), "budget"),
    };
    let total = p.length + n.length;
    if total <= 0.0 {
        return;
    }
    // The pair's own line: ps per mm on average.
    let v = (p.delay + n.delay) / total;
    if !(v.is_finite() && v > 0.0) {
        return;
    }
    let ps = (p.delay - n.delay).abs();
    let mm = ps / v;
    let name = pair.name.clone();
    let both = vec![p.info.name.to_string(), n.info.name.to_string()];
    let class = if t3 { Class::Critical } else { Class::Practical };
    let severity = if note { Severity::Info } else { Severity::Warning };
    let usb = matches!(interface, Some(Interface::Usb2Hs | Interface::Usb2Fs));
    let longer = if p.delay >= n.delay { p } else { n };
    if mm > budget + 1e-9 {
        let at = longer.pieces.first().map_or([0.0, 0.0], |q| q.a);
        let (long_name, regime) = (longer.info.name.to_string(), regime.to_string());
        let source = if param.is_empty() {
            format!("pair {name}'s skew_mm")
        } else if usb {
            "USB 2.0 (Renesas)".into()
        } else {
            "the budget".into()
        };
        cx.add(SKEW, &[format!("pair:{name}")], || {
            Draft::new(
                at,
                Vec::new(),
                format!(
                    "pair {name}: {} and {} differ by {} ps, {} mm of their line, against {} mm ({source}); {long_name} is the longer",
                    both.first().map_or("", String::as_str),
                    both.get(1).map_or("", String::as_str),
                    num(ps.round()),
                    num(mm),
                    num(budget)
                ),
            )
            .nets(both.clone())
            .class(class)
            .severity(severity)
            .measure(mm, budget, "mm", Bound::Max)
            .param(param)
            .regime(regime)
            .arg("pair", name.clone())
            .arg("ps", num(ps.round()))
            .arg("longer", long_name.clone())
            .fix(
                "ecad.board.net.inspect",
                json!({"pair": name, "metrics": ["length", "delay"]}),
                format!(
                    "lengthen the shorter member near where the mismatch arises (a small bump at the bend that makes it), or shorten {long_name}"
                ),
            )
        });
        return;
    }
    // The running mismatch, when both members make one line each.
    let tracks = |m: &Member| m.pieces.iter().map(|q| q.track).collect::<BTreeSet<u64>>().len();
    if !cx.spend(tracks(p).saturating_mul(tracks(p)).saturating_add(tracks(n).saturating_mul(tracks(n)))) {
        return;
    }
    let (Some(lp), Some(mut ln)) = (chain(&p.pieces), chain(&n.pieces)) else { return };
    let (Some(p0), Some(n0), Some(n1)) = (lp.first().copied(), ln.first().copied(), ln.last().copied()) else { return };
    if distance(p0, n1) < distance(p0, n0) {
        ln.reverse();
    }
    let sp = arclength(&lp);
    let sn = arclength(&ln);
    let by_interface = match interface {
        Some(Interface::Usb2Fs) => Some(USB_FS_NS),
        Some(Interface::Usb2Hs) => Some(USB_HS_NS),
        _ => None,
    };
    let edge = p.info.rt_ns.or(n.info.rt_ns).or(by_interface).map(|rt| rt * 1000.0 / v);
    let limit = edge.map_or(budget, |dx| budget.max(prm("running") * dx));
    let step = 0.25_f64;
    let count = (sp.last().copied().unwrap_or(0.0) / step).ceil() as usize;
    if !cx.spend(count.saturating_add(1).saturating_mul(ln.len().saturating_add(1))) {
        return;
    }
    let width = p.pieces.iter().chain(&n.pieces).map(|q| q.width).fold(0.0_f64, f64::max);
    let centres = coupling(cx, p, n).1 + width + 1e-6;
    let mut worst: Option<(f64, [f64; 2])> = None;
    let mut base: Option<f64> = None;
    for k in 0..=count.min(super::geom::MAX_SAMPLES) {
        let s = (k as f64 * step).min(sp.last().copied().unwrap_or(0.0));
        let Some(at) = point_at(&lp, &sp, s) else { continue };
        let Some((s_n, d)) = along_line(&ln, &sn, at) else { continue };
        // Only where the members run together: at a breakout the nearest point isn't the partner's.
        if d > centres {
            continue;
        }
        let m = s - s_n;
        let b = *base.get_or_insert(m);
        let off = m - b;
        if worst.is_none_or(|(w, _)| off.abs() > w.abs()) {
            worst = Some((off, at));
        }
    }
    let Some((off, at)) = worst else { return };
    if off.abs() <= limit + 1e-9 {
        return;
    }
    let regime = format!("{regime}; edge {} mm", edge.map_or("unknown".to_string(), |e| num(e.round())));
    cx.add(SKEW, &[format!("pair:{name}"), "running".into()], || {
        Draft::new(
            at,
            Vec::new(),
            format!(
                "pair {name}: the running mismatch reaches {} mm at ({}, {}) though the ends match within {} mm: compensate near where it arises",
                num(off.abs()),
                num(at[0]),
                num(at[1]),
                num(mm)
            ),
        )
        .nets(both)
        .class(class)
        .severity(Severity::Info)
        .variant("running")
        .measure(off.abs(), limit, "mm", Bound::Max)
        .param("running")
        .regime(regime)
        .arg("pair", name.clone())
        .fix(
            "ecad.board.net.inspect",
            json!({"pair": name, "metrics": ["length", "bends"]}),
            "turn left and right in turn, so the inner and outer lengths cancel bend by bend",
        )
    });
}

/// The point `s` along `line` (with arclengths `sl`).
fn point_at(line: &[[f64; 2]], sl: &[f64], s: f64) -> Option<[f64; 2]> {
    let k = sl.iter().position(|v| *v >= s)?;
    let Some(j) = k.checked_sub(1) else { return line.first().copied() };
    let (a, b) = (*line.get(j)?, *line.get(k)?);
    let (s0, s1) = (*sl.get(j)?, *sl.get(k)?);
    let t = if s1 > s0 { (s - s0) / (s1 - s0) } else { 0.0 };
    Some([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t])
}
