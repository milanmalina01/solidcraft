//! Section frames: named rectangles drawn on the schematic round a group of parts (Power, MCU,
//! Radio…). The sheet stays one sheet; frames only say how a print splits it into pages (an
//! overview, then a page per frame). A part belongs to the frame that holds the middle of its
//! symbol's box: where frames overlap, the first in print order (by `order`, frames without one
//! after those with one, then by id).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::schematic::{Library, Schematic, check_sheet_point, part_bbox};
use super::{MAX_TEXT, check_text};

/// The most frames a schematic holds (each prints as a page of its own).
pub const MAX_FRAMES: usize = 64;
/// A frame's shortest side, in millimetres.
pub const MIN_FRAME_SIDE: f64 = 1.0;
/// The highest `order`.
pub const MAX_ORDER: u32 = 9999;

/// A named rectangle on the sheet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    /// From the sheet's id space, like its parts and wires.
    pub id: u64,
    /// 1 to 64 characters, such as `Power` or `LoRa radio`.
    pub title: String,
    /// The lower left and upper right corners, in sheet millimetres.
    pub rect: [[f64; 2]; 2],
    /// Where it comes in the print, and which frame wins where frames overlap: lower first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
}

/// The rectangle with corners `a` and `b`, lower left first.
pub fn normalized([a, b]: [[f64; 2]; 2]) -> [[f64; 2]; 2] {
    [[a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]]
}

impl Frame {
    /// Its own rules: a title, corners on the sheet with the lower left first, and each side at
    /// least [`MIN_FRAME_SIDE`].
    pub fn validate(&self) -> Result<(), String> {
        check_title(&self.title)?;
        let [lo, hi] = self.rect;
        check_sheet_point("a frame corner", lo)?;
        check_sheet_point("a frame corner", hi)?;
        if !(hi[0] - lo[0] >= MIN_FRAME_SIDE && hi[1] - lo[1] >= MIN_FRAME_SIDE) {
            return Err(format!(
                "frame `{}`: `rect` must be [[x0, y0], [x1, y1]] with the lower left first and sides of at least {MIN_FRAME_SIDE} mm",
                self.title
            ));
        }
        if self.order.is_some_and(|o| o > MAX_ORDER) {
            return Err(format!("frame `{}`: `order` must be 0 to {MAX_ORDER}", self.title));
        }
        Ok(())
    }

    /// Whether `p` lies inside, its edge included.
    pub fn contains(&self, [x, y]: [f64; 2]) -> bool {
        let [lo, hi] = self.rect;
        x >= lo[0] && x <= hi[0] && y >= lo[1] && y <= hi[1]
    }

    /// Its middle.
    pub fn middle(&self) -> [f64; 2] {
        let [lo, hi] = self.rect;
        [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]
    }
}

/// A frame title: 1 to [`MAX_TEXT`] characters, no surrounding spaces or control characters.
pub fn check_title(title: &str) -> Result<(), String> {
    if title.trim().is_empty() || title.trim() != title {
        return Err(format!("a frame needs a title with no space at its start or end (got `{title}`)"));
    }
    check_text("title", title, MAX_TEXT)
}

/// The sheet's frames in print order: by `order` (frames without one after those with one), then
/// by id.
pub fn in_order(frames: &[Frame]) -> Vec<&Frame> {
    let mut out: Vec<&Frame> = frames.iter().collect();
    out.sort_by_key(|f| (f.order.is_none(), f.order.unwrap_or(0), f.id));
    out
}

/// The first frame in print order that holds `p`.
pub fn frame_at(frames: &[&Frame], p: [f64; 2]) -> Option<u64> {
    frames.iter().find(|f| f.contains(p)).map(|f| f.id)
}

/// Each part's frame, by part id: the first in print order holding the middle of its symbol's box
/// (its origin without one); parts in no frame are left out.
pub fn part_frames(sheet: &Schematic, library: &Library) -> BTreeMap<u64, u64> {
    let frames = in_order(&sheet.frames);
    let mut out = BTreeMap::new();
    if frames.is_empty() {
        return out;
    }
    for p in &sheet.parts {
        let middle = library
            .get(&p.component)
            .and_then(|c| part_bbox(p, c.symbol))
            .map_or(p.placement.at, |[a, b]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
        if let Some(f) = frame_at(&frames, middle) {
            out.insert(p.id, f);
        }
    }
    out
}

/// The pairs of frames that overlap, by id (lower first), for the check to say which one wins.
pub fn overlaps(frames: &[Frame]) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    for (i, a) in frames.iter().enumerate() {
        for b in frames.iter().skip(i + 1) {
            let ([a0, a1], [b0, b1]) = (a.rect, b.rect);
            if a0[0] < b1[0] && b0[0] < a1[0] && a0[1] < b1[1] && b0[1] < a1[1] {
                out.push((a.id.min(b.id), a.id.max(b.id)));
            }
        }
    }
    out
}
