//! Reworking the board's copper: a track split at a point, its corners mitered, segments cut out
//! of it, and moves that drag along the ends of the tracks that sit on a moved pad or via. Pure
//! functions on a board, so the `ecad.board.*` commands store their result and the board view
//! previews the same thing.

use std::collections::{BTreeMap, BTreeSet};

use super::Electronics;
use super::airwires::{BoardPad, footprint_pads};
use super::board::{BOARD_GRID, Board, MAX_TRACK_POINTS, Track};
use super::footprint::PadKind;
use super::path::{self, is_arc, sweep, tidy};
use super::place::{Motion, lattice, snap_point};

/// A mitered corner's cut shorter than this, in millimetres, is left sharp.
const MIN_MITER: f64 = 1e-3;
/// Corners turning less than this, or nearly back on themselves, in degrees, are left sharp.
const MIN_TURN: f64 = 1.0;
const MAX_TURN: f64 = 179.0;

/// One corner as [`Track::miter`] would cut it: the points before and after it, the corner, where
/// the cut starts and ends, and the cut's sweep (0 straight).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cut {
    pub before: [f64; 2],
    pub from: [f64; 2],
    pub corner: [f64; 2],
    pub to: [f64; 2],
    pub after: [f64; 2],
    pub sweep: f64,
}

/// How a mitered corner is cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MiterStyle {
    /// An arc tangent to both segments.
    Round,
    /// A straight chamfer between the points the arc would touch.
    Straight,
}

fn distance([ax, ay]: [f64; 2], [bx, by]: [f64; 2]) -> f64 {
    (ax - bx).hypot(ay - by)
}

impl Track {
    /// Split segment `segment` at its point nearest `at` (an arc into two arcs); the new corner's
    /// index, or why not.
    pub fn split(&mut self, segment: usize, at: [f64; 2]) -> Result<usize, String> {
        let (Some(a), Some(b)) = (self.points.get(segment).copied(), segment.checked_add(1).and_then(|i| self.points.get(i)).copied()) else {
            return Err(format!("the track has segments 0 to {}", self.points.len().saturating_sub(2)));
        };
        if self.points.len() >= MAX_TRACK_POINTS {
            return Err(format!("a track has at most {MAX_TRACK_POINTS} points"));
        }
        let s = sweep(&self.curves, segment);
        let (q, t) = path::project(at, a, b, s);
        let q = snap_point(q);
        if lattice(q) == lattice(a) || lattice(q) == lattice(b) {
            return Err("that point is at an end of the segment: split it between its ends".into());
        }
        self.points.insert(segment + 1, q);
        if !self.curves.is_empty() {
            let first = s * t;
            if let Some(c) = self.curves.get_mut(segment) {
                *c = first;
            }
            self.curves.insert(segment + 1, s - first);
        }
        Ok(segment + 1)
    }

    /// Cut corner `corner` (the index of an inner point), or every corner, `radius` round or
    /// straight (see [`MiterStyle`]): the cut is shortened to what the segments allow (all of a
    /// segment at a track's end, half of one between two corners when every corner is cut), and
    /// corners next to an arc, nearly straight or nearly turning back are left sharp, as are those
    /// `keep` holds (see [`miter_keeps`]). How many corners were cut.
    pub fn miter(&mut self, corner: Option<usize>, radius: f64, style: MiterStyle, keep: impl Fn(&Cut) -> bool) -> usize {
        let n = self.points.len();
        if n < 3 || !(radius.is_finite() && radius > 0.0) {
            return 0;
        }
        let corners: Vec<usize> = match corner {
            Some(c) if c >= 1 && c < n - 1 => vec![c],
            Some(_) => Vec::new(),
            None => (1..n - 1).collect(),
        };
        let every = corner.is_none();
        // Each corner cut: where it starts, where it ends, and the sweep between.
        let mut cuts: BTreeMap<usize, ([f64; 2], [f64; 2], f64)> = BTreeMap::new();
        for i in corners {
            if n + cuts.len() >= MAX_TRACK_POINTS {
                break;
            }
            let (Some(a), Some(p), Some(b)) = (self.points.get(i - 1).copied(), self.points.get(i).copied(), self.points.get(i + 1).copied()) else {
                continue;
            };
            if is_arc(sweep(&self.curves, i - 1)) || is_arc(sweep(&self.curves, i)) {
                continue;
            }
            let (l1, l2) = (distance(a, p), distance(p, b));
            if !(l1 > 0.0 && l2 > 0.0) {
                continue;
            }
            let u1 = [(p[0] - a[0]) / l1, (p[1] - a[1]) / l1];
            let u2 = [(b[0] - p[0]) / l2, (b[1] - p[1]) / l2];
            let turn = (u1[0] * u2[1] - u1[1] * u2[0]).atan2(u1[0] * u2[0] + u1[1] * u2[1]).to_degrees();
            if !(MIN_TURN..=MAX_TURN).contains(&turn.abs()) {
                continue;
            }
            let room_back = if every && i >= 2 { l1 / 2.0 } else { l1 };
            let room_on = if every && i + 2 < n { l2 / 2.0 } else { l2 };
            let cut = (radius * (turn.abs() / 2.0).to_radians().tan()).min(room_back).min(room_on);
            if !(cut >= MIN_MITER) {
                continue;
            }
            let from = snap_point([p[0] - u1[0] * cut, p[1] - u1[1] * cut]);
            let to = snap_point([p[0] + u2[0] * cut, p[1] + u2[1] * cut]);
            let s = match style {
                MiterStyle::Round => turn,
                MiterStyle::Straight => 0.0,
            };
            if keep(&Cut { before: a, from, corner: p, to, after: b, sweep: s }) {
                continue;
            }
            cuts.insert(i, (from, to, s));
        }
        if cuts.is_empty() {
            return 0;
        }
        let mut points = Vec::with_capacity(n + cuts.len());
        let mut curves = Vec::with_capacity(n + cuts.len());
        for (i, p) in self.points.iter().enumerate() {
            match cuts.get(&i) {
                Some((from, to, s)) => {
                    points.extend([*from, *to]);
                    curves.extend([*s, sweep(&self.curves, i)]);
                }
                None => {
                    points.push(*p);
                    curves.push(sweep(&self.curves, i));
                }
            }
        }
        tidy(&mut points, &mut curves, false);
        self.points = points;
        self.curves = curves;
        cuts.len()
    }
}

/// Whether a cut of a corner of `track` (see [`Cut`]) would part it from other copper of `board`
/// that joins it there and nowhere along what is left: another track of its layer (a point of
/// either on the other's line), a via (a point of the track within it, or its centre on the
/// track's line) or a pad of `pads` on its layer (a point of the track inside it, or its centre on
/// the track's line). Such a corner stays sharp.
pub fn miter_keeps<'a>(board: &'a Board, pads: &'a [BoardPad], track: &'a Track) -> impl Fn(&Cut) -> bool + 'a {
    let half = track.width / 2.0;
    move |c: &Cut| {
        // On the track's line round the corner (within its half width), before the cut and after.
        let was =
            |q: [f64; 2]| path::segment_distance(q, c.before, c.corner, 0.0) <= half || path::segment_distance(q, c.corner, c.after, 0.0) <= half;
        let is = |q: [f64; 2]| {
            path::segment_distance(q, c.before, c.from, 0.0) <= half
                || path::segment_distance(q, c.from, c.to, c.sweep) <= half
                || path::segment_distance(q, c.to, c.after, 0.0) <= half
        };
        let tracks = board.tracks.iter().filter(|o| o.id != track.id && o.layer == track.layer).any(|o| {
            let on = |q: [f64; 2]| path::segments(&o.points, &o.curves, false).any(|(a, b, s)| path::segment_distance(q, a, b, s) <= o.width / 2.0);
            let before = on(c.corner) || o.points.iter().any(|q| was(*q));
            before && !(on(c.from) || on(c.to) || o.points.iter().any(|q| is(*q)))
        });
        let vias = board.vias.iter().any(|v| {
            let r = v.diameter / 2.0;
            let before = distance(c.corner, v.at) <= r || was(v.at);
            before && !(distance(c.from, v.at) <= r || distance(c.to, v.at) <= r || is(v.at))
        });
        let pads = pads.iter().filter(|p| p.on(track.layer)).any(|p| {
            let before = p.contains(c.corner) || was(p.at());
            before && !(p.contains(c.from) || p.contains(c.to) || is(p.at()))
        });
        tracks || vias || pads
    }
}

impl Board {
    /// Cut the segments `cuts` names (by track id, the segments' indexes) out of their tracks: what
    /// is left of a track before a cut keeps its id, each piece after gets a new one and follows
    /// it in the board's order; a piece of no segment goes. The new tracks' ids, or why not (a
    /// track or segment that isn't there).
    pub fn cut_segments(&mut self, cuts: &BTreeMap<u64, BTreeSet<usize>>) -> Result<Vec<u64>, String> {
        let counts: BTreeMap<u64, usize> = self.tracks.iter().map(|t| (t.id, t.points.len().saturating_sub(1))).collect();
        // Each cut may leave one more piece.
        let mut pieces = 0usize;
        for (id, segments) in cuts {
            let Some(count) = counts.get(id).copied() else { return Err(format!("the board has no track with the id {id}")) };
            if let Some(s) = segments.iter().find(|s| **s >= count) {
                return Err(format!("track {id} has segments 0 to {}, not {s}", count.saturating_sub(1)));
            }
            pieces = pieces.saturating_add(segments.len());
        }
        let mut ids = self.free_ids(pieces)?;
        let mut created = Vec::new();
        let mut out = Vec::with_capacity(self.tracks.len());
        // A refusal part way leaves the tracks as they were.
        for t in self.tracks.clone() {
            let Some(gone) = cuts.get(&t.id) else {
                out.push(t);
                continue;
            };
            let mut pieces: Vec<(Vec<[f64; 2]>, Vec<f64>)> = Vec::new();
            let mut points: Vec<[f64; 2]> = t.points.first().copied().into_iter().collect();
            let mut curves = Vec::new();
            for (i, (_, b, s)) in path::segments(&t.points, &t.curves, false).enumerate() {
                if gone.contains(&i) {
                    pieces.push((std::mem::take(&mut points), std::mem::take(&mut curves)));
                    points.push(b);
                } else {
                    points.push(b);
                    curves.push(s);
                }
            }
            pieces.push((points, curves));
            let mut first = true;
            for (points, mut curves) in pieces.into_iter().filter(|(p, _)| p.len() >= 2) {
                if curves.iter().all(|c| !is_arc(*c)) {
                    curves.clear();
                }
                let id = if first { t.id } else { ids.next().ok_or("no board ids are left in this design")? };
                if !first {
                    created.push(id);
                }
                first = false;
                out.push(Track { id, points, curves, ..t.clone() });
            }
        }
        self.tracks = out;
        if let Some(last) = created.last() {
            self.next_id = self.next_id.max(last.saturating_add(1));
        }
        Ok(created)
    }
}

/// How a turn (`rotation`, degrees counter-clockwise) or a flip moves the board objects `ids`:
/// footprints alone each about its own origin; anything else with them, all as one about the
/// middle of them on the board's grid (so picked tracks stay on picked pads). For
/// [`Electronics::move_on_board`].
pub fn turn_groups(board: &Board, ids: &[u64], rotation: u16, flip: bool) -> Vec<(BTreeSet<u64>, Motion)> {
    let turn = |about: [f64; 2]| Motion { about, mirror: flip, rotation, by: [0.0, 0.0] };
    if ids.iter().all(|id| board.footprint(*id).is_some()) {
        return ids.iter().filter_map(|id| board.footprint(*id)).map(|f| (BTreeSet::from([f.id]), turn(f.at))).collect();
    }
    let places: Vec<[f64; 2]> = ids.iter().filter_map(|id| board.position(*id)).collect();
    let about = match places.as_slice() {
        [one] => *one,
        _ => super::bbox(places.iter().copied()).map_or([0.0, 0.0], |[[x0, y0], [x1, y1]]| {
            let on_grid = |v: f64| (v / BOARD_GRID).round() * BOARD_GRID;
            snap_point([on_grid((x0 + x1) / 2.0), on_grid((y0 + y1) / 2.0)])
        }),
    };
    vec![(ids.iter().copied().collect(), turn(about))]
}

impl Electronics {
    /// Move the board objects of each group by the group's motion (see [`Board::move_items`]),
    /// dragging along the ends of the tracks that stay: an end on a moved footprint's pad (on the
    /// track's layer) or on a moved via takes the motion, so the segment to it stretches. A flip
    /// leaves an end on a surface-mount pad where it was, since the pad goes to the other side. A
    /// dragged track that shrinks to one point goes.
    pub fn move_on_board(&self, board: &mut Board, groups: &[(BTreeSet<u64>, Motion)]) {
        let moved: BTreeSet<u64> = groups.iter().flat_map(|(ids, _)| ids.iter().copied()).collect();
        let mut pads: Vec<(BoardPad, Motion)> = Vec::new();
        let mut vias: Vec<([f64; 2], f64, Motion)> = Vec::new();
        for (ids, m) in groups {
            for f in board.footprints.iter().filter(|f| ids.contains(&f.id)) {
                pads.extend(footprint_pads(self, f, None).into_iter().map(|p| (p, *m)));
            }
            vias.extend(board.vias.iter().filter(|v| ids.contains(&v.id)).map(|v| (v.at, v.diameter / 2.0, *m)));
        }
        let mut drags: Vec<(usize, usize, [f64; 2])> = Vec::new();
        for (t, track) in board.tracks.iter().enumerate().filter(|(_, t)| !moved.contains(&t.id)) {
            let last = track.points.len().saturating_sub(1);
            for end in [0, last] {
                let Some(q) = track.points.get(end).copied() else { continue };
                let to = match pads.iter().find(|(p, _)| p.on(track.layer) && p.contains(q)) {
                    Some((p, m)) if m.mirror && p.pad.kind == PadKind::Smd => None,
                    Some((_, m)) => Some(m.point(q)),
                    None => vias.iter().find(|(at, r, _)| distance(q, *at) <= *r).map(|(_, _, m)| m.point(q)),
                };
                if let Some(to) = to {
                    drags.push((t, end, to));
                }
            }
        }
        for (ids, m) in groups {
            board.move_items(ids, m);
        }
        let mut dragged = BTreeSet::new();
        for (t, end, to) in drags {
            if let Some(p) = board.tracks.get_mut(t).and_then(|track| track.points.get_mut(end)) {
                *p = to;
                dragged.insert(t);
            }
        }
        for t in dragged {
            if let Some(track) = board.tracks.get_mut(t) {
                tidy(&mut track.points, &mut track.curves, false);
            }
        }
        board.tracks.retain(|t| t.points.len() >= 2);
    }
}
