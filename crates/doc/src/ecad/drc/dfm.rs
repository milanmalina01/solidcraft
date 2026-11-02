//! The design rule check's manufacturability codes: an open via too close to a surface-mount pad
//! (`via_near_pad`), a thin track that leaves a pad by its corner (`pad_entry`), an empty gap that
//! opens under the least angle where copper of one net meets (`acute_angle`), and copper or a gap
//! in a pour's fill narrower than the least track or gap (`sliver`).
//!
//! Their limits are the fab profile's (`via_to_pad`, `pad_entry_angle`, `acute_angle`, `track`
//! and `gap`); without a profile, [`VIA_TO_PAD`], [`ENTRY_ANGLE`], [`ACUTE_ANGLE`] and the rules'
//! least track and clearance. The first three figures come from public summaries of fab DFM
//! guides, not from a standard or a capability page, so each finding is a warning (an info for a
//! tented, plugged or filled via) with a note that says so.
//!
//! - A pad is taken as the rectangle of its size (a rounded or oval pad's corners included) for
//!   `pad_entry`; round pads have no edge to leave and are skipped there.
//! - A track leaves a pad where its middle line first crosses the rectangle from an end inside it.
//!   It leaves by the corner when its copper reaches past the corner along that edge, and is thin
//!   there under [`THIN_SHARE`] of the edge's length. A diagonal exit (more than the profile's
//!   `pad_entry_angle` off square) is fine wherever it leaves.
//! - Pockets are measured on the merged copper, never on middle lines: a track's straight piece at
//!   an end is merged with the pad, or the other track's pieces, it ends on; each inward corner of
//!   the union that neither had alone is where their edges meet, and the empty gap there opens at
//!   the angle between those edges. Only gaps under the least angle (70° by default) count, so a
//!   track entering a pad askew is flagged on its sharp side alone, and not at all while both
//!   sides stay open. A track's own sharpest bend is measured between its pieces.
//! - Slivers: a fill opened by half the least track (shrunk, then grown back) loses its necks and
//!   fingers; closed by half the least gap it fills its narrow slots. What changes by more than
//!   [`SLIVER_AREA`] of the limit squared counts (a square corner changes by much less); one
//!   finding per fill. Tracks narrower than the least track are `track_width`'s.
//! - Bounded: the pair tests spend the check's budget, and fills past [`MAX_SLIVER_CORNERS`]
//!   corners together are left out and mark the check truncated.

use std::collections::BTreeMap;

use serde_json::json;

use super::{Bound, CELL, Ctx, Draft, DrcItem, Found, mm, pad_item};
use crate::ecad::airwires::BoardPad;
use crate::ecad::board::{ViaKind, ViaProtection};
use crate::ecad::checks::{Severity, gap};
use crate::ecad::copper::{disc, pad_core, pieces};
use crate::ecad::footprint::{PadKind, PadShape};
use crate::ecad::grid::{Grid, grow};
use crate::ecad::layers::CopperLayer;
use solidcraft_geom::region::Region;

/// Without a fab profile: the least gap from an open via's ring to a pad, in millimetres.
pub const VIA_TO_PAD: f64 = 0.25;
/// Without a fab profile: how far off square a track leaves a pad and still counts as square to
/// its edge, in degrees (more is a diagonal exit, fine from anywhere on the edge).
pub const ENTRY_ANGLE: f64 = 30.0;
/// Without a fab profile: the least opening of an empty gap where copper meets, in degrees.
pub const ACUTE_ANGLE: f64 = 70.0;
/// A track is thin beside a pad's edge under this share of the edge's length.
pub const THIN_SHARE: f64 = 0.5;
/// Corners of merged copper closer than this, in millimetres, are one corner.
const JOIN_TOLERANCE: f64 = 0.01;
/// What a fill's opening or closing must change, as a share of the limit squared, to count.
pub const SLIVER_AREA: f64 = 0.5;
/// The most fill corners the sliver test works through, all fills together.
pub const MAX_SLIVER_CORNERS: usize = 1 << 17;
/// Angles within this of their limit pass: drawing on a lattice leaves such a hair.
const ANGLE_SLACK: f64 = 0.5;
/// The arc step, in degrees, of the sliver test's offsets.
const OFFSET_STEP: f64 = 15.0;

const NOTE: &str = "the limit comes from public summaries of fab DFM guides, not from a standard: verify it with your fab";

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn len(a: [f64; 2]) -> f64 {
    a[0].hypot(a[1])
}

/// The angle between two directions, in degrees (0 to 180); `None` for one of no length.
fn angle(a: [f64; 2], b: [f64; 2]) -> Option<f64> {
    let (la, lb) = (len(a), len(b));
    if la <= 1e-9 || lb <= 1e-9 {
        return None;
    }
    Some((dot(a, b) / (la * lb)).clamp(-1.0, 1.0).acos().to_degrees())
}

/// The smaller angle between two lines, in degrees (0 to 90).
fn line_angle(a: [f64; 2], b: [f64; 2]) -> Option<f64> {
    angle(a, b).map(|t| t.min(180.0 - t))
}

/// The limits the checks use (see the module notes).
struct Limits {
    via_to_pad: f64,
    entry: f64,
    acute: f64,
}

fn limits(c: &Ctx) -> Limits {
    match c.profile {
        Some(p) => Limits { via_to_pad: p.via_to_pad.value, entry: p.pad_entry_angle.value, acute: p.acute_angle.value },
        None => Limits { via_to_pad: VIA_TO_PAD, entry: ENTRY_ANGLE, acute: ACUTE_ANGLE },
    }
}

/// A pad's rectangle on the board: its centre, its unit axes and half sizes along them.
struct Frame {
    o: [f64; 2],
    u: [f64; 2],
    v: [f64; 2],
    half: [f64; 2],
}

impl Frame {
    fn of(p: &BoardPad) -> Option<Frame> {
        if p.pad.shape == PadShape::Round || p.pad.kind != PadKind::Smd {
            return None;
        }
        let (s, co) = p.pad.rotation.to_radians().sin_cos();
        let local = |x: f64, y: f64| p.placement.apply([p.pad.at[0] + x * co - y * s, p.pad.at[1] + x * s + y * co]);
        let o = local(0.0, 0.0);
        let (u, v) = (sub(local(1.0, 0.0), o), sub(local(0.0, 1.0), o));
        let [w, h] = p.pad.size;
        (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0).then_some(Frame { o, u, v, half: [w / 2.0, h / 2.0] })
    }

    fn local(&self, q: [f64; 2]) -> [f64; 2] {
        let d = sub(q, self.o);
        [dot(d, self.u), dot(d, self.v)]
    }

    fn inside(&self, q: [f64; 2]) -> bool {
        let [s, t] = self.local(q);
        s.abs() <= self.half[0] + 1e-9 && t.abs() <= self.half[1] + 1e-9
    }
}

/// Where a track leaves a pad: the point, how far off square to the edge it goes (degrees), and
/// how far from the edge's middle it crosses (a share of the way to the corner).
struct Exit {
    at: [f64; 2],
    off_square: f64,
    toward_corner: f64,
    /// How far, in millimetres, the crossing is from the corner it is nearer, along the edge.
    to_corner: f64,
    /// The length of the edge it crosses, in millimetres.
    edge: f64,
}

/// Where the line `line` (from an end inside the pad) first leaves it; `None` if it stays inside.
fn exit(f: &Frame, line: &[[f64; 2]]) -> Option<Exit> {
    for w in line.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
        if f.inside(*b) {
            continue;
        }
        let (la, lb) = (f.local(*a), f.local(*b));
        let d = sub(lb, la);
        // The first of the four sides it crosses: 0 the x sides, 1 the y sides.
        let mut best: Option<(f64, usize)> = None;
        for axis in 0..2 {
            let (from, delta, half) = (la[axis], d[axis], f.half[axis]);
            for side in [half, -half] {
                if delta.abs() <= 1e-12 {
                    continue;
                }
                let t = (side - from) / delta;
                if (0.0..=1.0 + 1e-12).contains(&t) && best.is_none_or(|(bt, _)| t < bt) {
                    let other = 1 - axis;
                    let at = la[other] + d[other] * t;
                    if at.abs() <= f.half[other] + 1e-9 {
                        best = Some((t, axis));
                    }
                }
            }
        }
        let (t, axis) = best?;
        let other = 1 - axis;
        let local_at = [la[0] + d[0] * t, la[1] + d[1] * t];
        let normal = if axis == 0 { [1.0, 0.0] } else { [0.0, 1.0] };
        let off_square = line_angle(d, normal)?;
        let half = f.half[other];
        let toward_corner = if half > 0.0 { local_at[other].abs() / half } else { 0.0 };
        let at = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        let (to_corner, edge) = ((half - local_at[other].abs()).max(0.0), 2.0 * half);
        return Some(Exit { at, off_square, toward_corner, to_corner, edge });
    }
    None
}

/// Vias whose ring comes closer to a surface-mount pad on a layer they share than the profile's
/// `via_to_pad`, without being in it (`via_in_pad`'s): solder can wick into an open via there.
/// Micro vias are skipped (they are filled or small enough); a tented, plugged or filled via is
/// only an info.
pub(super) fn vias_near_pads(c: &mut Ctx, f: &mut Found) {
    let board = c.board;
    let n = board.copper_layers;
    let limit = limits(c).via_to_pad;
    let pads: Vec<&BoardPad> = c.nets.pads.iter().filter(|p| p.pad.kind == PadKind::Smd).collect();
    if pads.is_empty() || board.vias.is_empty() {
        return;
    }
    let mut grid = Grid::new(CELL);
    for (k, p) in pads.iter().enumerate() {
        grid.insert(k, p.bbox());
    }
    for via in &board.vias {
        if via.kind(n, &board.rules) == ViaKind::Micro {
            continue;
        }
        let r = via.diameter / 2.0;
        let land = disc(via.at, r);
        let near = grid.near_box(grow([[via.at[0] - r, via.at[1] - r], [via.at[0] + r, via.at[1] + r]], limit), pads.len());
        if !c.visit(near.len()) {
            return;
        }
        for k in near {
            let Some(p) = pads.get(k) else { continue };
            if !c.spend(1) {
                return;
            }
            let on = (p.top && via.on(CopperLayer::Top, n)) || (p.bottom && via.on(CopperLayer::Bottom, n));
            if !on {
                continue;
            }
            let g = gap(&land, &pad_core(p));
            if g < -1e-9 || g >= limit - 1e-9 {
                continue;
            }
            let (item, pad) = (DrcItem::Via(via.id), pad_item(p));
            let open = via.protection == ViaProtection::Open;
            let how = via.protection.name();
            let message = format!(
                "via {} ({how}) is {} mm from pad {}; keep {} mm, so a solder mask web stays between them and the solder doesn't wick into the via",
                via.id,
                mm(g.max(0.0)),
                p.reference(),
                mm(limit)
            );
            f.add("via_near_pad", &[item.key(), pad.key()], || {
                let d = Draft::new(via.at, vec![item, pad], message)
                    .layer(Some(if p.top { CopperLayer::Top } else { CopperLayer::Bottom }))
                    .gap(g, limit)
                    .param("profile.via_to_pad")
                    .arg("protection", how.to_string())
                    .note(NOTE)
                    .fix(
                        "ecad.board.move",
                        serde_json::Value::Null,
                        format!("move via {} {} mm further from the pad, joined to it by a short track", via.id, mm(super::step(limit - g))),
                    );
                if open {
                    d.severity(Severity::Warning).fix(
                        "ecad.board.via.edit",
                        json!({"vias": [via.id], "protection": "tented"}),
                        "or tent or plug it, if your fab does: then solder can't reach the hole",
                    )
                } else {
                    d.severity(Severity::Info)
                }
            });
        }
    }
}

/// Each track's straight pieces as one line, and as it runs from its last end back.
fn lines(t: &crate::ecad::board::Track) -> (Vec<[f64; 2]>, Vec<[f64; 2]>) {
    let p = pieces(&t.points, &t.curves, false);
    let mut line: Vec<[f64; 2]> = p.iter().map(|(a, _)| *a).collect();
    if let Some((_, b)) = p.last() {
        line.push(*b);
    }
    let back = line.iter().rev().copied().collect();
    (line, back)
}

/// Tracks that leave a non-round surface-mount pad they end in from its corner while thin beside
/// the edge they leave by (`pad_entry`); a diagonal exit, more than the profile's
/// `pad_entry_angle` off square, is fine wherever it leaves. Then the empty gaps where copper
/// meets, against the least angle (`acute_angle`): each track's sharpest bend, and every inward
/// corner of a track merged with the pad or other track of its net it ends on (see
/// [`junction_gaps`]).
pub(super) fn pad_entries_and_pockets(c: &mut Ctx, f: &mut Found) {
    let lim = limits(c);
    let (board, nets) = (c.board, c.nets);
    let frames: Vec<(usize, &BoardPad, Frame)> = nets.pads.iter().enumerate().filter_map(|(i, p)| Frame::of(p).map(|fr| (i, p, fr))).collect();
    let mut grid = Grid::new(CELL);
    for (k, (_, p, _)) in frames.iter().enumerate() {
        grid.insert(k, p.bbox());
    }
    let mut tracks: Vec<(usize, Vec<[f64; 2]>, Vec<[f64; 2]>)> = Vec::with_capacity(board.tracks.len());
    for (ti, t) in board.tracks.iter().enumerate() {
        let (line, back) = lines(t);
        if line.len() >= 2 {
            tracks.push((ti, line, back));
        }
    }
    // Pads: a thin track leaving by the corner.
    for (ti, line, back) in &tracks {
        let Some(t) = board.tracks.get(*ti) else { continue };
        let root = nets.track_islands.get(*ti).copied();
        for (end, l) in [(0u8, line), (1u8, back)] {
            let Some(at) = l.first().copied() else { continue };
            let near = grid.near_box([at, at], frames.len());
            if !c.visit(near.len()) {
                return;
            }
            for k in near {
                let Some((i, p, fr)) = frames.get(k) else { continue };
                if !c.spend(1) {
                    return;
                }
                if !p.on(t.layer) || nets.pad_islands.get(*i).copied() != root || !fr.inside(at) {
                    continue;
                }
                let Some(x) = exit(fr, l) else { continue };
                let diagonal = x.off_square > lim.entry + ANGLE_SLACK;
                // Its copper reaches past the corner, and it is thin beside the edge.
                let spill = t.width / 2.0 / x.off_square.to_radians().cos().max(0.2);
                let cornered = x.to_corner <= spill + 1e-9;
                let thin = t.width < THIN_SHARE * x.edge - 1e-9;
                if diagonal || !cornered || !thin {
                    continue;
                }
                let (item, pad) = (DrcItem::TrackEnd(t.id, end), pad_item(p));
                let share = x.toward_corner;
                let message = format!(
                    "track {} ({} mm) leaves pad {} at its corner, {} % of the way from the edge's middle: enter a pad near the middle of its edge",
                    t.id,
                    mm(t.width),
                    p.reference(),
                    mm((share * 100.0).round())
                );
                f.add("pad_entry", &[item.key(), pad.key()], || {
                    Draft::new(x.at, vec![item, pad], message)
                        .layer(Some(t.layer))
                        .note(NOTE)
                        .count(share, 1.0, "ratio", Bound::Max)
                        .param("pad_entry.corner")
                        .arg("share", mm((share * 100.0).round()))
                        .fix(
                            "ecad.board.track.reshape",
                            serde_json::Value::Null,
                            format!("reshape track {} so it leaves the pad from the middle of its edge", t.id),
                        )
                });
            }
        }
    }
    // Bends: each track's sharpest.
    for (ti, line, _) in &tracks {
        let Some(t) = board.tracks.get(*ti) else { continue };
        let mut worst: Option<(f64, [f64; 2])> = None;
        for w in line.windows(3) {
            let (Some(a), Some(v), Some(b)) = (w.first(), w.get(1), w.get(2)) else { continue };
            if let Some(th) = angle(sub(*a, *v), sub(*b, *v))
                && worst.is_none_or(|(x, _)| th < x)
            {
                worst = Some((th, *v));
            }
        }
        if let Some((th, at)) = worst.filter(|(th, _)| *th < lim.acute - ANGLE_SLACK) {
            let item = DrcItem::Track(t.id);
            let message = format!(
                "track {} bends at {}°: a pocket under {}° can trap etchant; bend at 90° or more (two 45° bends)",
                t.id,
                mm(th),
                mm(lim.acute)
            );
            f.add("acute_angle", &[item.key()], || {
                Draft::new(at, vec![item], message)
                    .layer(Some(t.layer))
                    .count(th, lim.acute, "degrees", Bound::Min)
                    .arg("angle", mm(th))
                    .arg("limit", mm(lim.acute))
                    .note(NOTE)
                    .fix(
                        "ecad.board.track.miter",
                        json!({"track": t.id, "radius": 0.5}),
                        "round the bend, or add a segment so no corner is under 90°",
                    )
            });
        }
    }
    // Junctions: a track end merged with what of its net it ends on.
    let mut seg_grid: BTreeMap<CopperLayer, Grid> = BTreeMap::new();
    let mut segs: Vec<(usize, [f64; 2], [f64; 2])> = Vec::new();
    for (k, line, _) in &tracks {
        let Some(t) = board.tracks.get(*k) else { continue };
        for w in line.windows(2) {
            let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
            let s = segs.len();
            segs.push((*k, *a, *b));
            let r = t.width / 2.0;
            seg_grid.entry(t.layer).or_insert_with(|| Grid::new(CELL)).insert(s, grow(crate::ecad::path::segment_bbox(*a, *b, 0.0), r));
        }
    }
    let mut pad_grid = Grid::new(CELL);
    for (k, p) in nets.pads.iter().enumerate() {
        pad_grid.insert(k, p.bbox());
    }
    let mut done: std::collections::BTreeSet<(u64, u64, i64, i64)> = std::collections::BTreeSet::new();
    for (ti, line, back) in &tracks {
        let Some(t) = board.tracks.get(*ti) else { continue };
        let root = nets.track_islands.get(*ti).copied();
        let r = t.width / 2.0;
        for (end, l) in [(0u8, line), (1u8, back)] {
            let (Some(at), Some(next)) = (l.first().copied(), l.get(1).copied()) else { continue };
            let mine = stroke_region(at, next, r);
            // Pads it ends in or on.
            let reach = [[at[0] - r, at[1] - r], [at[0] + r, at[1] + r]];
            let near = pad_grid.near_box(reach, nets.pads.len());
            if !c.visit(near.len()) {
                return;
            }
            for k in near {
                let Some(p) = nets.pads.get(k) else { continue };
                if !c.spend(8) {
                    return;
                }
                if !p.on(t.layer) || nets.pad_islands.get(k).copied() != root {
                    continue;
                }
                let core = pad_core(p);
                if gap(&disc(at, r), &core) > 1e-9 {
                    continue;
                }
                let theirs = Region::polygon(&crate::ecad::copper::hull_outline(&core, 0.0));
                for (corner, opening) in junction_gaps(&mine, &theirs, at, near_reach(&core, r)) {
                    if opening >= lim.acute - ANGLE_SLACK || !done.insert((t.id, p.footprint.wrapping_add(1 << 40), key(corner[0]), key(corner[1]))) {
                        continue;
                    }
                    let (a, b) = (DrcItem::TrackEnd(t.id, end), pad_item(p));
                    let message = format!(
                        "the gap between track {} and pad {} opens at {}°: a pocket under {}° can trap etchant; turn the track so both sides open wider",
                        t.id,
                        p.reference(),
                        mm(opening.round()),
                        mm(lim.acute)
                    );
                    f.add("acute_angle", &[a.key(), b.key()], || {
                        Draft::new(corner, vec![a, b], message)
                            .layer(Some(t.layer))
                            .count(opening, lim.acute, "degrees", Bound::Min)
                            .arg("angle", mm(opening.round()))
                            .arg("limit", mm(lim.acute))
                            .arg("meets", "pad".into())
                            .note(NOTE)
                            .fix(
                                "ecad.board.track.reshape",
                                serde_json::Value::Null,
                                format!("reshape track {} so it leaves the pad square to its edge", t.id),
                            )
                    });
                }
            }
            // Other tracks of its net it ends on (their end, bend or side).
            let Some(grid) = seg_grid.get(&t.layer) else { continue };
            let near = grid.near_box(reach, segs.len());
            if !c.visit(near.len()) {
                return;
            }
            let mut others: BTreeMap<usize, Vec<([f64; 2], [f64; 2])>> = BTreeMap::new();
            for s in near {
                let Some((oi, a, b)) = segs.get(s) else { continue };
                if oi == ti || nets.track_islands.get(*oi).copied() != root || !c.spend(1) {
                    continue;
                }
                let Some(o) = board.tracks.get(*oi) else { continue };
                if gap(&disc(at, r), &crate::ecad::copper::stroke(*a, *b, o.width / 2.0)) > 1e-9 {
                    continue;
                }
                others.entry(*oi).or_default().push((*a, *b));
            }
            for (oi, pieces) in others {
                let Some(o) = board.tracks.get(oi) else { continue };
                if !c.spend(8 * pieces.len()) {
                    return;
                }
                let mut theirs = Region::new();
                for (a, b) in &pieces {
                    theirs = theirs.union(&stroke_region(*a, *b, o.width / 2.0));
                }
                for (corner, opening) in junction_gaps(&mine, &theirs, at, r + o.width) {
                    if opening >= lim.acute - ANGLE_SLACK || !done.insert((t.id.min(o.id), t.id.max(o.id), key(corner[0]), key(corner[1]))) {
                        continue;
                    }
                    let (a, b) = (DrcItem::TrackEnd(t.id, end), DrcItem::Track(o.id));
                    let message = format!(
                        "the gap between track {} and track {} opens at {}°: a pocket under {}° can trap etchant; join them at 90° or more",
                        t.id,
                        o.id,
                        mm(opening.round()),
                        mm(lim.acute)
                    );
                    f.add("acute_angle", &[a.key(), b.key()], || {
                        Draft::new(corner, vec![a, b], message)
                            .layer(Some(t.layer))
                            .count(opening, lim.acute, "degrees", Bound::Min)
                            .arg("angle", mm(opening.round()))
                            .arg("limit", mm(lim.acute))
                            .arg("meets", "track".into())
                            .note(NOTE)
                            .fix(
                                "ecad.board.track.reshape",
                                serde_json::Value::Null,
                                format!("reshape track {} so it meets track {} at 90° or more", t.id, o.id),
                            )
                    });
                }
            }
        }
    }
}

/// A point rounded to a micrometre, for telling corners apart.
fn key(v: f64) -> i64 {
    (v * 1000.0).round() as i64
}

/// How far from a track's end the corners of its junction with `core` can be.
fn near_reach(core: &crate::ecad::footprint::Core, r: f64) -> f64 {
    let bbox = super::super::bbox(core.points.iter().copied()).unwrap_or([[0.0, 0.0], [0.0, 0.0]]);
    let diag = (bbox[1][0] - bbox[0][0]).hypot(bbox[1][1] - bbox[0][1]);
    diag + 2.0 * (core.radius + r)
}

/// A straight piece of track as a region.
fn stroke_region(a: [f64; 2], b: [f64; 2], r: f64) -> Region {
    Region::polygon(&crate::ecad::copper::hull_outline(&crate::ecad::copper::stroke(a, b, r), 0.0))
}

/// The inward corners of `mine` merged with `theirs` that neither has alone, within `reach` of
/// `at`, each with the opening of the empty gap there in degrees: where a track's side meets a
/// pad's edge or another track's side, the gap between their copper opens at the angle of their
/// edges on the empty side, whatever angle the track's middle line makes.
pub(super) fn junction_gaps(mine: &Region, theirs: &Region, at: [f64; 2], reach: f64) -> Vec<([f64; 2], f64)> {
    let own: Vec<[f64; 2]> = inward_corners(mine).into_iter().chain(inward_corners(theirs)).map(|(p, _)| p).collect();
    inward_corners(&mine.union(theirs))
        .into_iter()
        .filter(|(p, _)| len(sub(*p, at)) <= reach && !own.iter().any(|q| len(sub(*p, *q)) <= JOIN_TOLERANCE))
        .collect()
}

/// Inward (reflex) corners of a region's outlines, with the empty gap's opening there in degrees.
fn inward_corners(r: &Region) -> Vec<([f64; 2], f64)> {
    let mut out = Vec::new();
    for s in r.shapes() {
        let s = s.simplified(JOIN_TOLERANCE / 2.0);
        // Outer contours run counter-clockwise and holes clockwise: either way the copper is on
        // the left, so a right turn is an inward corner.
        for ring in std::iter::once(&s.outer).chain(s.holes.iter()) {
            let n = ring.len();
            if n < 3 {
                continue;
            }
            for i in 0..n {
                let (Some(prev), Some(v), Some(next)) = (ring.get((i + n - 1) % n), ring.get(i), ring.get((i + 1) % n)) else { continue };
                let (a, b) = (sub(*v, *prev), sub(*next, *v));
                let cross = a[0] * b[1] - a[1] * b[0];
                if cross >= -1e-12 * len(a) * len(b) {
                    continue;
                }
                if let Some(opening) = angle(sub(*prev, *v), sub(*next, *v)) {
                    out.push((*v, opening));
                }
            }
        }
    }
    out
}

/// Copper or gaps in each pour's fill narrower than the least track or gap (see the module
/// notes): one finding per fill, at its largest sliver.
pub(super) fn slivers(c: &mut Ctx, f: &mut Found) {
    let (copper, room) = match c.profile {
        Some(p) => (p.track.value, p.gap.value),
        None => (c.board.rules.min_track_width, c.board.rules.clearance),
    };
    let mut corners = 0usize;
    for fill in &c.nets.fills {
        if fill.region.is_empty() {
            continue;
        }
        corners = corners.saturating_add(fill.region.corners());
        if corners > MAX_SLIVER_CORNERS {
            f.truncated = true;
            return;
        }
        let measure = |thin: &solidcraft_geom::region::Region, limit: f64| -> Option<(usize, f64, [f64; 2])> {
            let floor = SLIVER_AREA * limit * limit;
            let mut count = 0;
            let mut best: Option<(f64, f64, [f64; 2])> = None;
            for s in thin.shapes() {
                let area = s.area();
                if area <= floor {
                    continue;
                }
                count += 1;
                let perimeter: f64 = s.outer.iter().zip(s.outer.iter().cycle().skip(1)).map(|(a, b)| len(sub(*b, *a))).sum();
                // A thin piece's width is about twice its area over its perimeter.
                let width = if perimeter > 0.0 { (2.0 * area / perimeter).min(limit) } else { 0.0 };
                let n = s.outer.len().max(1) as f64;
                let mid = s.outer.iter().fold([0.0, 0.0], |m, p| [m[0] + p[0] / n, m[1] + p[1] / n]);
                if best.is_none_or(|(a, ..)| area > a) {
                    best = Some((area, width, mid));
                }
            }
            best.map(|(_, w, at)| (count, w, at))
        };
        let r = &fill.region;
        let narrow_copper = r.difference(&r.offset(-copper / 2.0, OFFSET_STEP).offset(copper / 2.0, OFFSET_STEP));
        let narrow_gap = r.offset(room / 2.0, OFFSET_STEP).offset(-room / 2.0, OFFSET_STEP).difference(r);
        let (a, b) = (measure(&narrow_copper, copper), measure(&narrow_gap, room));
        if a.is_none() && b.is_none() {
            continue;
        }
        let item = DrcItem::Pour(fill.pour);
        let (pieces_c, pieces_g) = (a.map_or(0, |x| x.0), b.map_or(0, |x| x.0));
        let message = format!(
            "pour {}'s fill has {pieces_c} piece(s) of copper narrower than {} mm and {pieces_g} gap(s) narrower than {} mm: the fab can lose them or leave them ragged",
            fill.pour,
            mm(copper),
            mm(room)
        );
        // The copper sliver speaks for the finding when there is one.
        let (width, limit, at, param) = match (a, b) {
            (Some((_, w, at)), _) => (w, copper, at, "profile.track"),
            (None, Some((_, w, at))) => (w, room, at, "profile.gap"),
            (None, None) => continue,
        };
        f.add("sliver", &[item.key()], || {
            let d = Draft::new(at, vec![item], message)
                .layer(Some(fill.layer))
                .measure(width, limit)
                .param(param)
                .arg("copper", pieces_c.to_string())
                .arg("gaps", pieces_g.to_string())
                .arg("track", mm(copper))
                .arg("gap", mm(room));
            let d = if pieces_c > 0 {
                d.fix(
                    "ecad.board.pour.edit",
                    json!({"pour": fill.pour, "min_width": copper}),
                    format!("fill no copper narrower than {} mm", mm(copper)),
                )
            } else {
                d
            };
            if pieces_g > 0 {
                d.fix(
                    "ecad.board.pour.edit",
                    serde_json::Value::Null,
                    "close or widen the narrow gaps: move the copper or the outline that makes them",
                )
            } else {
                d
            }
        });
    }
}
