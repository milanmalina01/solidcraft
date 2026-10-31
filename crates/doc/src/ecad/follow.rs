//! How wires hang on when pins move within their parts: a part shows another symbol (a redrawn
//! one, or a standard shape in place of a box), so its pins lie elsewhere though the part stays.
//! A wire's bends stay where they are, so whatever meets them still does, and an end on a pin
//! that moved reaches its old point with a short jog out of the pin. Pure functions.

use std::collections::{BTreeMap, BTreeSet};

use super::place::{GRID, Placement, lattice, snap, snap_point};
use super::route::{STUB, opposite};
use super::schematic::{Anchor, Library, MAX_WIRE_POINTS, Pins, Schematic, Wire};
use super::{PinDirection, SymbolItem};

/// What [`follow`] needs from before the change: where the ends of the wires on the changing
/// parts' pins were (`None`: on a pin that isn't there), by wire id, and where things meet wires.
pub struct Ends {
    ends: BTreeMap<u64, [Option<[f64; 2]>; 2]>,
    marks: Marks,
}

/// The lattice points where something can meet a wire (wire ends and bends, labels, markers, pin
/// ends), counted, by row and by column, so a segment's are found without walking it.
#[derive(Default)]
struct Marks {
    rows: BTreeMap<i64, BTreeMap<i64, u32>>,
    columns: BTreeMap<i64, BTreeMap<i64, u32>>,
}

impl Marks {
    fn add(&mut self, p: [f64; 2]) {
        let [x, y] = lattice(p);
        let row = self.rows.entry(y).or_default().entry(x).or_insert(0);
        *row = row.saturating_add(1);
        let column = self.columns.entry(x).or_default().entry(y).or_insert(0);
        *column = column.saturating_add(1);
    }

    /// Whether something other than `own` (points counted as this wire's) lies on the level or
    /// upright segment `a`–`b`, its ends included, `skip` left out. A slanted one is never clear.
    fn taken(&self, a: [f64; 2], b: [f64; 2], own: &BTreeMap<[i64; 2], u32>, skip: &[[i64; 2]]) -> bool {
        let (a, b) = (lattice(a), lattice(b));
        let hits: Vec<([i64; 2], u32)> = if a[1] == b[1] {
            self.rows.get(&a[1]).map(|r| r.range(a[0].min(b[0])..=a[0].max(b[0])).map(|(x, n)| ([*x, a[1]], *n)).collect()).unwrap_or_default()
        } else if a[0] == b[0] {
            self.columns.get(&a[0]).map(|c| c.range(a[1].min(b[1])..=a[1].max(b[1])).map(|(y, n)| ([a[0], *y], *n)).collect()).unwrap_or_default()
        } else {
            return true;
        };
        hits.into_iter().any(|(p, n)| !skip.contains(&p) && n > own.get(&p).copied().unwrap_or(0))
    }
}

/// Where the ends of the wires with an end on a pin of `parts` lie now (`library` saying where
/// the pins are), and what meets wires where, to hand to [`follow`] once the parts show their
/// new symbol.
pub fn wire_ends(sheet: &Schematic, library: &Library, parts: &BTreeSet<u64>) -> Ends {
    let pins = Pins::new(sheet, library);
    let on = |a: &Anchor| a.pin().is_some_and(|(part, _)| parts.contains(&part));
    // On the lattice, as saved points are: a turned part's pin ends come out of the turn a hair off it.
    let end = |a: &Anchor| pins.anchor(a).map(snap_point);
    let ends = sheet.wires.iter().filter(|w| on(&w.from) || on(&w.to)).map(|w| (w.id, [end(&w.from), end(&w.to)])).collect();
    let mut marks = Marks::default();
    if !parts.is_empty() {
        for w in &sheet.wires {
            pins.wire(w).into_iter().for_each(|p| marks.add(p));
        }
        sheet.labels.iter().filter_map(|l| pins.anchor(&l.at)).for_each(|p| marks.add(p));
        sheet.markers.iter().filter_map(|m| pins.anchor(&m.at)).for_each(|p| marks.add(p));
        for p in &sheet.parts {
            if let Some(c) = library.get(&p.component) {
                c.symbol.pins.iter().for_each(|q| marks.add(p.placement.apply(q.at)));
            }
        }
    }
    Ends { ends, marks }
}

/// Hang the wires `was` lists (as [`wire_ends`] found them) on the pins of `parts`, which show
/// `new` now (anchors already renumbered to it). An end on a pin that moved slides the bend next
/// to it when nothing else meets the legs that change (a T, a label, another pin), so the leg out
/// of the pin keeps its axis; else it reaches its old point with a jog (see [`follow_pins`]). A
/// wire that would grow past its cap keeps its bends, its end leg aslant.
pub fn follow(sheet: &mut Schematic, parts: &BTreeSet<u64>, new: &SymbolItem, was: &Ends) {
    let placed: BTreeMap<u64, Placement> = sheet.parts.iter().filter(|p| parts.contains(&p.id)).map(|p| (p.id, p.placement)).collect();
    let mut marks_now = Marks::default();
    for placement in placed.values() {
        new.pins.iter().for_each(|q| marks_now.add(placement.apply(q.at)));
    }
    // Where an end is now, and the way out of its pin when that pin is one of the parts'.
    let now = |a: &Anchor, was: [f64; 2]| -> ([f64; 2], Option<PinDirection>) {
        let Some((part, number)) = a.pin() else { return (was, None) };
        match (placed.get(&part), new.pin(number)) {
            (Some(placement), Some(q)) => {
                let q = placement.pin(q);
                (snap_point(q.at), Some(opposite(q.direction)))
            }
            _ => (was, None),
        }
    };
    for w in &mut sheet.wires {
        let Some([Some(of), Some(ot)]) = was.ends.get(&w.id).copied() else { continue };
        let ((nf, df), (nt, dt)) = (now(&w.from, of), now(&w.to, ot));
        if lattice(nf) == lattice(of) && lattice(nt) == lattice(ot) {
            continue;
        }
        // This wire's own points, as the marks counted them.
        let mut own: BTreeMap<[i64; 2], u32> = BTreeMap::new();
        for p in std::iter::once(of).chain(w.via.iter().copied()).chain(std::iter::once(ot)) {
            let n = own.entry(lattice(p)).or_insert(0);
            *n = n.saturating_add(1);
        }
        let skip = [lattice(of), lattice(ot), lattice(nf), lattice(nt)];
        // Whether the segment a–b meets nothing but this wire (`also`: a point left out too).
        let free = |a: [f64; 2], b: [f64; 2], also: Option<[f64; 2]>| {
            let mut skip = skip.to_vec();
            skip.extend(also.map(lattice));
            !was.marks.taken(a, b, &own, &skip) && !marks_now.taken(a, b, &BTreeMap::new(), &skip)
        };
        let hung = slid(w, [of, ot], [nf, nt], [df, dt], &free).unwrap_or_else(|| follow_pins(w, [of, ot], [nf, nt], [df, dt]));
        if hung.via.len().saturating_add(2) <= MAX_WIRE_POINTS {
            *w = hung;
        }
    }
}

/// Between `a` and `b` on the grid, or `None` when no grid line lies strictly between them.
fn grid_between(a: f64, b: f64) -> Option<f64> {
    let m = snap(((a + b) / 2.0 / GRID).round() * GRID);
    (m > a.min(b) && m < a.max(b)).then_some(m)
}

/// The wire with its one moved end hung on by moving what joins it: the bend next to it slid so
/// the leg out of the pin keeps its axis, or a straight wire stepped through the grid line
/// between its ends (or kept straight when the pin moved along it). Only when `free` (a segment,
/// and a point of it left out) says what changes meets nothing but the wire, and a leg that left
/// the pin outward still does; `None` otherwise.
fn slid(
    w: &Wire,
    was: [[f64; 2]; 2],
    now: [[f64; 2]; 2],
    out: [Option<PinDirection>; 2],
    free: &dyn Fn([f64; 2], [f64; 2], Option<[f64; 2]>) -> bool,
) -> Option<Wire> {
    let moved = [lattice(was[0]) != lattice(now[0]), lattice(was[1]) != lattice(now[1])];
    // One end at a time: the from end, or the to end with the wire turned round.
    let (end, reversed) = match moved {
        [true, false] => (0, false),
        [false, true] => (1, true),
        _ => return None,
    };
    let mut points: Vec<[f64; 2]> = std::iter::once(was[0]).chain(w.via.iter().copied()).chain(std::iter::once(was[1])).collect();
    if reversed {
        points.reverse();
    }
    let (Some(&of), Some(&next)) = (points.first(), points.get(1)) else { return None };
    let nf = if end == 0 { now[0] } else { now[1] };
    let [ux, uy] = out.get(end).copied().flatten().map_or([0.0, 0.0], PinDirection::unit);
    let outward = |from: [f64; 2], to: [f64; 2]| (to[0] - from[0]) * ux + (to[1] - from[1]) * uy > 0.0;
    let (a, b) = (lattice(of), lattice(next));
    let level = a[1] == b[1] && a[0] != b[0];
    let upright = a[0] == b[0] && a[1] != b[1];
    if !(level || upright) || !free(of, next, None) {
        return None;
    }
    let keeps_out = |first: [f64; 2]| !outward(of, next) || outward(nf, first);
    let new: Vec<[f64; 2]> = if points.len() == 2 {
        let n = lattice(nf);
        if (level && n[1] == b[1]) || (upright && n[0] == b[0]) {
            // Moved along the wire: it stays straight.
            if !(keeps_out(next) && free(nf, next, Some(next))) {
                return None;
            }
            vec![nf, next]
        } else {
            // Moved across it: a step through the grid line between the ends.
            let (m1, m2) = if level {
                let x = grid_between(nf[0], next[0])?;
                ([x, nf[1]], [x, next[1]])
            } else {
                let y = grid_between(nf[1], next[1])?;
                ([nf[0], y], [next[0], y])
            };
            if !(keeps_out(m1) && free(nf, m1, None) && free(m1, m2, None) && free(m2, next, Some(next))) {
                return None;
            }
            vec![nf, m1, m2, next]
        }
    } else {
        let after = *points.get(2)?;
        let c = lattice(after);
        // The bend must turn (the next leg runs across the first) for sliding it to keep both
        // legs on their axes.
        let slide = if level && b[0] == c[0] {
            snap_point([next[0], nf[1]])
        } else if upright && b[1] == c[1] {
            snap_point([nf[0], next[1]])
        } else {
            return None;
        };
        // The next leg shrinks (the slid bend lies on it) or grows past the old bend.
        let s = lattice(slide);
        let shrinks = (s[0] - b[0]).signum() * (c[0] - b[0]).signum() >= 0 && (s[1] - b[1]).signum() * (c[1] - b[1]).signum() >= 0;
        let next_leg = if shrinks { free(next, slide, Some(slide)) } else { free(slide, next, Some(next)) };
        if !(keeps_out(slide) && next_leg && free(nf, slide, Some(slide))) {
            return None;
        }
        let mut new = vec![nf, slide];
        new.extend(points.iter().skip(2).copied());
        new
    };
    let mut new = super::route::simplify(new);
    if reversed {
        new.reverse();
    }
    let n = new.len();
    let via = new.get(1..n.saturating_sub(1)).map(<[_]>::to_vec).unwrap_or_default();
    Some(Wire { id: w.id, from: w.from.clone(), via, to: w.to.clone() })
}

/// Whether `b` lies on the axis-parallel line through `a` and `c`, between them or not (a bend
/// that turns back on itself is no bend either).
fn in_line(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> bool {
    (a[0] == b[0] && b[0] == c[0]) || (a[1] == b[1] && b[1] == c[1])
}

/// A wire whose ends were at `was` (from, to) and are now at `now`, each leaving its pin toward
/// `out`. Its bends stay where they are; an end that moved reaches its old point with a jog:
/// straight out of the pin by [`STUB`], across onto the old line, and on along it. A jog's points
/// (and the old end) left on a straight line, or turning back on themselves, are dropped.
pub fn follow_pins(w: &Wire, was: [[f64; 2]; 2], now: [[f64; 2]; 2], out: [Option<PinDirection>; 2]) -> Wire {
    // The jog from `now` to just before `was`, `now` first; none when the end didn't move.
    let jog = |was: [f64; 2], now: [f64; 2], out: Option<PinDirection>| -> Vec<[f64; 2]> {
        if lattice(was) == lattice(now) {
            return Vec::new();
        }
        let [ux, uy] = out.map_or([0.0, 0.0], PinDirection::unit);
        let s = snap_point([now[0] + ux * STUB, now[1] + uy * STUB]);
        // Across to the old line: along x out of a level pin, along y out of an upright one.
        let c = if uy == 0.0 { [s[0], was[1]] } else { [was[0], s[1]] };
        vec![now, s, snap_point(c)]
    };
    // Each point, and whether the tidying may drop it: the ends and the old bends stay.
    let mut points: Vec<([f64; 2], bool)> = Vec::new();
    let head = jog(was[0], now[0], out[0]);
    points.push((now[0], false));
    if !head.is_empty() {
        points.extend(head.iter().skip(1).map(|p| (*p, true)));
        points.push((was[0], true));
    }
    points.extend(w.via.iter().map(|p| (*p, false)));
    let tail = jog(was[1], now[1], out[1]);
    if !tail.is_empty() {
        points.push((was[1], true));
        points.extend(tail.iter().skip(1).rev().map(|p| (*p, true)));
    }
    points.push((now[1], false));
    // Drop one point at a time until none can go: each pass drops one, so this ends.
    let limit = points.len();
    for _ in 0..limit {
        let n = points.len();
        let drop = (1..n.saturating_sub(1)).find(|&i| {
            let (Some(a), Some(b), Some(c)) = (points.get(i - 1), points.get(i), points.get(i + 1)) else { return false };
            let (pa, pb, pc) = (lattice(a.0), lattice(b.0), lattice(c.0));
            b.1 && (pa == pb || pb == pc || in_line(pa, pb, pc))
        });
        match drop {
            Some(i) => {
                points.remove(i);
            }
            None => break,
        }
    }
    let n = points.len();
    let via = points.get(1..n.saturating_sub(1)).map(|p| p.iter().map(|(q, _)| *q).collect()).unwrap_or_default();
    Wire { id: w.id, from: w.from.clone(), via, to: w.to.clone() }
}
