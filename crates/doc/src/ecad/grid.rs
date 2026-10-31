//! A grid of square cells over the board, each holding the shapes (by index) whose box reaches
//! into it, so the checks find what is near a point or a box without testing everything. A shape
//! over too many cells is kept apart and handed out with every query.

use std::collections::HashMap;

/// A shape over more cells than this is tested against everything instead.
const MAX_CELLS: i64 = 64;

pub(crate) struct Grid {
    /// The cell side, in millimetres.
    side: f64,
    cells: HashMap<(i64, i64), Vec<usize>>,
    /// Shapes too big for the cells, or with boxes that aren't finite.
    wide: Vec<usize>,
}

impl Grid {
    pub(crate) fn new(side: f64) -> Self {
        Grid { side: if side.is_finite() && side > 0.0 { side } else { 1.0 }, cells: HashMap::new(), wide: Vec::new() }
    }

    fn cell(&self, v: f64) -> i64 {
        // Out-of-range values saturate; they come only from points the board refuses.
        (v / self.side).floor() as i64
    }

    /// The cells a box covers, or `None` past [`MAX_CELLS`] (or not finite).
    fn span(&self, [[x0, y0], [x1, y1]]: [[f64; 2]; 2]) -> Option<(i64, i64, i64, i64)> {
        if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
            return None;
        }
        let (cx0, cx1, cy0, cy1) = (self.cell(x0), self.cell(x1), self.cell(y0), self.cell(y1));
        let many = cx1.saturating_sub(cx0).saturating_add(1).saturating_mul(cy1.saturating_sub(cy0).saturating_add(1));
        (many > 0 && many <= MAX_CELLS).then_some((cx0, cx1, cy0, cy1))
    }

    pub(crate) fn insert(&mut self, i: usize, bbox: [[f64; 2]; 2]) {
        let Some((cx0, cx1, cy0, cy1)) = self.span(bbox) else {
            self.wide.push(i);
            return;
        };
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                self.cells.entry((cx, cy)).or_default().push(i);
            }
        }
    }

    /// The shapes whose box may hold `p`.
    pub(crate) fn near(&self, [x, y]: [f64; 2]) -> impl Iterator<Item = usize> + '_ {
        self.cells.get(&(self.cell(x), self.cell(y))).into_iter().flatten().chain(&self.wide).copied()
    }

    /// The shapes whose box may meet `bbox`, each once (every one of `0..all` past
    /// [`MAX_CELLS`] cells).
    pub(crate) fn near_box(&self, bbox: [[f64; 2]; 2], all: usize) -> Vec<usize> {
        let Some((cx0, cx1, cy0, cy1)) = self.span(bbox) else { return (0..all).collect() };
        let mut out: Vec<usize> = self.wide.clone();
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                out.extend(self.cells.get(&(cx, cy)).into_iter().flatten());
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The cell `p` is in.
    pub(crate) fn cell_of(&self, [x, y]: [f64; 2]) -> (i64, i64) {
        (self.cell(x), self.cell(y))
    }

    /// The shapes in one cell, then the ones too big for the cells (with every cell asked).
    pub(crate) fn in_cell(&self, c: (i64, i64)) -> &[usize] {
        self.cells.get(&c).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn wide(&self) -> &[usize] {
        &self.wide
    }

    /// The cell side, in millimetres.
    pub(crate) fn side(&self) -> f64 {
        self.side
    }

    /// An upper bound on the pairs of shapes that share a cell: the work a pass over every pair
    /// of neighbours does.
    pub(crate) fn pairs(&self) -> usize {
        let wide = self.wide.len();
        let cells: usize = self.cells.values().map(|v| v.len().saturating_mul(v.len()) / 2).fold(0, usize::saturating_add);
        let all: usize = self.cells.values().map(Vec::len).fold(0, usize::saturating_add);
        cells.saturating_add(wide.saturating_mul(all.saturating_add(wide)))
    }
}

/// Whether two boxes meet (edges touching count).
pub(crate) fn boxes_meet([[ax0, ay0], [ax1, ay1]]: [[f64; 2]; 2], [[bx0, by0], [bx1, by1]]: [[f64; 2]; 2]) -> bool {
    ax0 <= bx1 && bx0 <= ax1 && ay0 <= by1 && by0 <= ay1
}

/// A box grown by `r` each way.
pub(crate) fn grow([[x0, y0], [x1, y1]]: [[f64; 2]; 2], r: f64) -> [[f64; 2]; 2] {
    [[x0 - r, y0 - r], [x1 + r, y1 + r]]
}
