//! Where a footprint's name (designator) and value go on the board when it keeps no spot of its
//! own and its library footprint has none: just outside its courtyard, on the side of it with the
//! most room from the footprints round it on the same side of the board, the value on the side
//! across from the name; and how high they are when the board sets no height (see
//! [`super::Board::name_height`]): in step with the footprint's size, so small parts get small
//! names.

use std::ops::RangeInclusive;

use super::footprint::Side;
use super::grid::{Grid, boxes_meet};

/// The heights the board's name height may be, in millimetres.
pub const NAME_HEIGHT: RangeInclusive<f64> = 0.1..=10.0;
/// A name's height when the board sets none: this part of its courtyard's shorter side, from
/// [`MIN_AUTO_HEIGHT`] to [`MAX_AUTO_HEIGHT`] millimetres.
const SHARE: f64 = 0.8;
pub const MIN_AUTO_HEIGHT: f64 = 0.5;
pub const MAX_AUTO_HEIGHT: f64 = 1.0;
/// How far from a courtyard its room is looked for, in millimetres.
const REACH: f64 = 5.0;
/// The gap between a courtyard and its texts, as a part of their height.
const GAP: f64 = 0.2;
/// A character's width as a part of its height, as the board draws text.
pub const CHAR_WIDTH: f64 = 0.6;
/// The grid's cell, in millimetres.
const CELL: f64 = 2.0;
/// The most neighbours [`sides`] looks at, all footprints together; past it, the rest take
/// [`Around::Above`].
const MAX_VISITS: usize = 1 << 20;

/// The height of a footprint's name and value with the courtyard box `[lo, hi]` when the board
/// sets none.
pub fn auto_height([lo, hi]: [[f64; 2]; 2]) -> f64 {
    let shorter = (hi[0] - lo[0]).min(hi[1] - lo[1]);
    if shorter.is_finite() { (SHARE * shorter).clamp(MIN_AUTO_HEIGHT, MAX_AUTO_HEIGHT) } else { MAX_AUTO_HEIGHT }
}

/// A text's width at `height` with `chars` characters, as the board draws it.
pub fn text_width(height: f64, chars: usize) -> f64 {
    height * CHAR_WIDTH * chars.max(1) as f64
}

/// A side of a footprint's box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Around {
    Above,
    Below,
    Right,
    Left,
}

/// The sides in the order a tie goes.
const AROUND: [Around; 4] = [Around::Above, Around::Below, Around::Right, Around::Left];

impl Around {
    /// The side across from it.
    pub fn opposite(self) -> Around {
        match self {
            Around::Above => Around::Below,
            Around::Below => Around::Above,
            Around::Right => Around::Left,
            Around::Left => Around::Right,
        }
    }
}

/// The box beside `[lo, hi]` on side `a` that a text `[w, h]` would take, out to [`REACH`] (at
/// least the text's own size) away.
fn strip([lo, hi]: [[f64; 2]; 2], a: Around, [w, h]: [f64; 2]) -> [[f64; 2]; 2] {
    let (cx, cy) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
    // Along the side: the box's length, or the text's where it is longer.
    let (x0, x1) = (lo[0].min(cx - w / 2.0), hi[0].max(cx + w / 2.0));
    let (y0, y1) = (lo[1].min(cy - h / 2.0), hi[1].max(cy + h / 2.0));
    let (dx, dy) = (REACH.max(w * 2.0), REACH.max(h * 2.0));
    match a {
        Around::Above => [[x0, hi[1]], [x1, hi[1] + dy]],
        Around::Below => [[x0, lo[1] - dy], [x1, lo[1]]],
        Around::Right => [[hi[0], y0], [hi[0] + dx, y1]],
        Around::Left => [[lo[0] - dx, y0], [lo[0], y1]],
    }
}

/// For each footprint (its courtyard box on the board and its side), the side of its box with the
/// most room to spare for its name `sizes[i]` (`[width, height]`): the room up to the nearest
/// other footprint, or text placed before it, on the same side of the board, less what the name
/// takes there. The text `across[i]` (its value), when it has a size, goes on the opposite side
/// and takes its spot too. Bounded: past [`MAX_VISITS`] neighbours looked at, the rest take
/// [`Around::Above`].
pub fn sides(boxes: &[([[f64; 2]; 2], Side)], sizes: &[[f64; 2]], across: &[[f64; 2]]) -> Vec<Around> {
    let mut grid = Grid::new(CELL);
    for (i, (b, _)) in boxes.iter().enumerate() {
        grid.insert(i, *b);
    }
    // The footprints' boxes, then the names placed so far.
    let mut taken: Vec<([[f64; 2]; 2], Side)> = boxes.to_vec();
    let mut visits = MAX_VISITS;
    let mut out = Vec::with_capacity(boxes.len());
    for (i, (own, side)) in boxes.iter().enumerate() {
        let [w, h] = sizes.get(i).copied().unwrap_or([MAX_AUTO_HEIGHT; 2]);
        let [lo, hi] = *own;
        let mut best: Option<(Around, f64)> = None;
        for a in AROUND {
            let s = strip(*own, a, [w, h]);
            let reach = match a {
                Around::Above | Around::Below => s[1][1] - s[0][1],
                Around::Right | Around::Left => s[1][0] - s[0][0],
            };
            let near = grid.near_box(s, taken.len());
            if visits < near.len() {
                visits = 0;
                best = None;
                break;
            }
            visits -= near.len();
            let room = near
                .into_iter()
                .filter(|j| *j != i)
                .filter_map(|j| taken.get(j))
                .filter(|(b, sd)| sd == side && boxes_meet(s, *b))
                .map(|(b, _)| match a {
                    Around::Above => b[0][1] - hi[1],
                    Around::Below => lo[1] - b[1][1],
                    Around::Right => b[0][0] - hi[0],
                    Around::Left => lo[0] - b[1][0],
                })
                .fold(reach, f64::min)
                .max(0.0);
            let needs = match a {
                Around::Above | Around::Below => h * (1.0 + GAP),
                Around::Right | Around::Left => w + h * GAP,
            };
            if best.is_none_or(|(_, b)| room - needs > b + 1e-9) {
                best = Some((a, room - needs));
            }
        }
        let a = best.map_or(Around::Above, |(a, _)| a);
        // The name, and the text across from it, take their spots from those after it.
        for (a, [w, h]) in [(a, [w, h]), (a.opposite(), across.get(i).copied().unwrap_or_default())] {
            if w > 0.0 && h > 0.0 {
                let at = spot(*own, a, [w, h]);
                grid.insert(taken.len(), [at, [at[0] + w, at[1] + h]]);
                taken.push(([at, [at[0] + w, at[1] + h]], *side));
            }
        }
        out.push(a);
    }
    out
}

/// The lower left of a text `[w, h]` just outside the box `[lo, hi]` on side `a`, centred along
/// that side.
pub fn spot([lo, hi]: [[f64; 2]; 2], a: Around, [w, h]: [f64; 2]) -> [f64; 2] {
    let (cx, cy) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0);
    let gap = h * GAP;
    match a {
        Around::Above => [cx - w / 2.0, hi[1] + gap],
        Around::Below => [cx - w / 2.0, lo[1] - gap - h],
        Around::Right => [hi[0] + gap, cy - h / 2.0],
        Around::Left => [lo[0] - gap - w, cy - h / 2.0],
    }
}
