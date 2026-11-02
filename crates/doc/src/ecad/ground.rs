//! How well a ground net's copper is tied to its main ground: each copper layer of the board on a
//! grid of square cells, the net's copper on it (pour fills, tracks, pads, via rings) a resistive
//! sheet, its vias and through-hole pads links between the layers, and a current drawn evenly
//! from all of it into the reference. The potential each cell rises to is how weakly it is
//! tied: in millivolts when every square millimetre of the net's copper draws [`DENSITY`]
//! (1 A/cm²); 0 on the reference.
//!
//! - **Sheet.** Two neighbouring copper cells (sharing a side) are joined by one square of the
//!   layer's copper: its thickness (from the stack-up) over copper's resistivity [`RHO`]. A track
//!   narrower than a cell conducts as a cell-wide strip, so thin tracks look better tied than
//!   they are; planes, which this is for, are measured as they are.
//! - **Vias.** A via of the net, and a through-hole pad of it, joins the cell its centre is in on
//!   each layer of its span to the same cell on the next: a plated barrel of the hole's diameter,
//!   [`PLATING`] thick, as long as the board's thickness over the layers' gaps.
//! - **Reference.** The ground pads of the board's power entry (a connector the intent keeps as
//!   one, with the net and a power net on its pads), else the largest single shape of the net's
//!   pour fills on any layer, as one equipotential at 0.
//! - **Isolated.** Copper joined to the reference by nothing is isolated: no number, and listed
//!   on its own.
//! - **Solve.** The potentials solve a sparse system (a conductance matrix: symmetric, positive
//!   definite once every unknown is joined to the reference) by conjugate gradients with a
//!   diagonal preconditioner, to a relative residual of [`TOLERANCE`], at most [`MAX_ITERATIONS`]
//!   steps: a solve cut short says so (`converged`), and the checks that rest on it fall back to
//!   their older measure. A re-solve after a via is added starts from the last answer.
//! - **Bounded.** Cells are [`DEFAULT_CELL`] across (a finer one, down to [`MIN_CELL`], when
//!   asked), and larger when the board's layers would need more than [`MAX_CELLS`] of them (or
//!   the fewer a caller asks for: [`CHECK_CELLS`], [`PLAN_CELLS`], [`VIEW_CELLS`])
//!   (`coarse`), so the work grows with the board up to a cap, never past it.
//! - **Planes.** The checks and the stitching that rest on this ([`GroundMap::plane_worst`]) look
//!   at the net's fill copper only: an unrouted pad or a stray track is the design rule check's
//!   (`unrouted`), and an outer island with no via is `pi.stitching`'s own.

use std::collections::{BTreeMap, VecDeque};

use super::Electronics;
use super::airwires::BoardNets;
use super::checks::gap;
use super::copper::{disc, pad_core, pieces};
use super::footprint::PadKind;
use super::intent::{NetRole, PartRole};
use super::layers::CopperLayer;
use super::stackup::StackKind;

/// The current every square millimetre of the net's copper draws, in amperes (1 A/cm²).
pub const DENSITY: f64 = 0.01;
/// Copper's resistivity, in ohm millimetres (1.72 µΩ·cm at 20 °C).
pub const RHO: f64 = 1.72e-5;
/// A plated hole's wall, in millimetres (IPC-6012 class 2 asks for 20 µm on average; 25 µm is
/// a usual fab's figure).
pub const PLATING: f64 = 0.025;
/// A copper layer's thickness when the stack-up gives none, in millimetres (1 oz).
pub const DEFAULT_COPPER: f64 = 0.035;
/// The cell's side: the usual, the finest asked for, the widest asked for, in millimetres.
pub const DEFAULT_CELL: f64 = 0.25;
pub const MIN_CELL: f64 = 0.1;
pub const MAX_CELL: f64 = 0.25;
/// The most cells of the grid, all layers together.
pub const MAX_CELLS: usize = 400_000;
/// The solve's least relative residual, and its most steps.
pub const TOLERANCE: f64 = 1e-6;
pub const MAX_ITERATIONS: usize = 4000;
/// How weakly tied a plane may be before `pi.stitching` warns and needed stitching works on it,
/// in millivolts at [`DENSITY`].
pub const DEFAULT_TARGET_MV: f64 = 0.5;

/// What to measure: the net (by name; the intent's ground, else a net named as one), and the cell
/// side (see the module notes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ask {
    pub net: Option<String>,
    pub cell: Option<f64>,
    /// The most cells, all layers together (at most [`MAX_CELLS`], that when left out): a check
    /// run after every edit or a plan solving many times asks for fewer and gets a coarser grid.
    pub max_cells: Option<usize>,
}

/// The cell budgets of the solve's users: the quality check (after every edit), needed
/// stitching's plan (solved once per round), and the board view's heat map.
pub const CHECK_CELLS: usize = 40_000;
pub const PLAN_CELLS: usize = 100_000;
pub const VIEW_CELLS: usize = 150_000;

/// What the potentials are measured from.
#[derive(Clone, Debug, PartialEq)]
pub enum Reference {
    /// The ground pads of the connector the power comes in on, by designator.
    Entry(String),
    /// The largest shape of the net's fills: its pour's id and layer, and its area in mm².
    Plane { pour: u64, layer: CopperLayer, area: f64 },
}

/// A cell's state.
const NONE: f64 = f64::NAN;
const ISOLATED: f64 = f64::INFINITY;

/// The ground net of a board: the intent's (the first net kept as ground), else the first named
/// as one (GND, AGND, PGND, DGND, VSS, 0V).
pub fn default_net(ecad: &Electronics, nets: &BoardNets) -> Option<usize> {
    let kept = ecad.intent.nets.iter().filter(|r| r.role == NetRole::Ground).find_map(|r| nets.names.iter().position(|n| *n == r.net));
    kept.or_else(|| nets.names.iter().position(|x| matches!(x.to_ascii_uppercase().as_str(), "GND" | "AGND" | "PGND" | "DGND" | "VSS" | "0V")))
}

/// The grid and its conductances, kept so vias can be added and the potentials solved again.
#[derive(Clone, Debug)]
pub struct Model {
    pub net: usize,
    pub net_name: String,
    pub cell: f64,
    /// The cell had to be wider than asked to stay within [`MAX_CELLS`].
    pub coarse: bool,
    /// The lower left corner of cell (0, 0), and the cells across and up.
    pub origin: [f64; 2],
    pub nx: usize,
    pub ny: usize,
    /// The board's copper layers, top first.
    pub layers: Vec<CopperLayer>,
    copper: Vec<bool>,
    /// The cell is in a fill of the net (a plane).
    plane: Vec<bool>,
    reference: Vec<bool>,
    /// Each layer's square conductance, in siemens.
    sheet: Vec<f64>,
    /// Links between cells of two layers (vias), with their conductance.
    links: Vec<(usize, usize, f64)>,
    pub reference_is: Reference,
    /// The last solve's potentials, in volts, by cell (NaN none, infinity isolated).
    last: Vec<f64>,
    hop: f64,
}

/// A spot that is weakly tied.
#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    pub at: [f64; 2],
    pub layer: CopperLayer,
    /// Millivolts at [`DENSITY`].
    pub mv: f64,
}

/// One layer's share of copper: cells of the net's copper, tied, isolated and weak (past the
/// target), in mm² and as shares of the copper.
#[derive(Clone, Debug, PartialEq)]
pub struct Coverage {
    pub layer: CopperLayer,
    pub copper_mm2: f64,
    pub tied: f64,
    pub isolated: f64,
    pub weak: f64,
}

/// A solve's answer (see the module notes).
#[derive(Clone, Debug)]
pub struct GroundMap {
    pub net_name: String,
    pub cell: f64,
    pub coarse: bool,
    pub origin: [f64; 2],
    pub nx: usize,
    pub ny: usize,
    pub layers: Vec<CopperLayer>,
    /// By cell (layer, then row, then column): millivolts; NaN where there is no copper of the
    /// net, infinity where it is isolated.
    pub mv: Vec<f64>,
    plane: Vec<bool>,
    pub reference: Reference,
    pub converged: bool,
    pub iterations: usize,
    pub unknowns: usize,
}

fn dist_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
}

impl Model {
    /// The model of `ecad`'s board for `ask` (see the module notes).
    pub fn build(ecad: &Electronics, nets: &BoardNets, ask: &Ask) -> Result<Model, String> {
        let board = &ecad.board;
        let net = match &ask.net {
            Some(name) => nets.names.iter().position(|n| n == name).ok_or_else(|| format!("the schematic has no net `{name}`"))?,
            None => default_net(ecad, nets).ok_or("no net is kept as ground in the intent or named as one (GND): give `net`")?,
        };
        let net_name = nets.name(net).unwrap_or_default().to_string();
        if let Some(c) = ask.cell
            && !(c.is_finite() && (MIN_CELL..=MAX_CELL).contains(&c))
        {
            return Err(format!("`cell` must be {MIN_CELL} to {MAX_CELL} mm"));
        }
        let [[x0, y0], [x1, y1]] = board.outline_bbox().ok_or("the board has no outline to lay the grid over")?;
        let (w, h) = (x1 - x0, y1 - y0);
        if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
            return Err("the board's outline has no area".into());
        }
        let layers = board.stack();
        let nl = layers.len().max(1);
        let asked = ask.cell.unwrap_or(DEFAULT_CELL);
        let most = ask.max_cells.unwrap_or(MAX_CELLS).clamp(1000, MAX_CELLS);
        let least = (w * h * nl as f64 / most as f64).sqrt();
        let coarse = least > asked;
        let cell = asked.max(least);
        let nx = ((w / cell).ceil() as usize).max(1);
        let ny = ((h / cell).ceil() as usize).max(1);
        let total = nx.saturating_mul(ny).saturating_mul(nl);
        if total > MAX_CELLS.saturating_mul(2) {
            return Err("the board is too large to measure".into());
        }
        let origin = [x0, y0];
        let centre = |ix: usize, iy: usize| [x0 + (ix as f64 + 0.5) * cell, y0 + (iy as f64 + 0.5) * cell];
        let mut copper = vec![false; total];
        let mut plane = vec![false; total];
        let idx = |l: usize, ix: usize, iy: usize| (l * ny + iy) * nx + ix;
        let span_cells = |b: [[f64; 2]; 2]| {
            let ix0 = (((b[0][0] - x0) / cell).floor().max(0.0) as usize).min(nx - 1);
            let iy0 = (((b[0][1] - y0) / cell).floor().max(0.0) as usize).min(ny - 1);
            let ix1 = (((b[1][0] - x0) / cell).floor().max(0.0) as usize).min(nx - 1);
            let iy1 = (((b[1][1] - y0) / cell).floor().max(0.0) as usize).min(ny - 1);
            (ix0, iy0, ix1, iy1)
        };
        let centres: Vec<[f64; 2]> = (0..ny).flat_map(|iy| (0..nx).map(move |ix| (ix, iy))).map(|(ix, iy)| centre(ix, iy)).collect();
        let mut biggest: Option<(f64, usize, u64, Vec<usize>)> = None;
        let n = board.copper_layers;
        let half = cell / 2.0;
        for (l, layer) in layers.iter().enumerate() {
            // Fills.
            for fill in nets.fills.iter().filter(|f| f.net == Some(net) && f.layer == *layer && !f.region.is_empty()) {
                let shapes = fill.region.shapes();
                let found = fill.region.locate(&centres);
                let mut by_shape: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
                for (k, s) in found.iter().enumerate() {
                    if let Some(s) = s {
                        let (ix, iy) = (k % nx, k / nx);
                        let i = idx(l, ix, iy);
                        if let (Some(c), Some(p)) = (copper.get_mut(i), plane.get_mut(i)) {
                            *c = true;
                            *p = true;
                        }
                        by_shape.entry(*s).or_default().push(i);
                    }
                }
                for (s, cells) in by_shape {
                    let area = shapes.get(s).map_or(0.0, |x| x.area());
                    if biggest.as_ref().is_none_or(|(a, ..)| area > *a) {
                        biggest = Some((area, l, fill.pour, cells));
                    }
                }
            }
            // Tracks.
            for (t, tn) in board.tracks.iter().zip(&nets.track_nets) {
                if *tn != Some(net) || t.layer != *layer {
                    continue;
                }
                let r = t.width / 2.0;
                for (a, b) in pieces(&t.points, &t.curves, false) {
                    let bb = [[a[0].min(b[0]) - r - half, a[1].min(b[1]) - r - half], [a[0].max(b[0]) + r + half, a[1].max(b[1]) + r + half]];
                    let (ix0, iy0, ix1, iy1) = span_cells(bb);
                    for iy in iy0..=iy1 {
                        for ix in ix0..=ix1 {
                            if dist_segment(centre(ix, iy), a, b) <= r + half
                                && let Some(c) = copper.get_mut(idx(l, ix, iy))
                            {
                                *c = true;
                            }
                        }
                    }
                }
            }
            // Pads.
            for p in nets.pads.iter().filter(|p| p.net == Some(net) && p.on(*layer) && p.pad.kind != PadKind::Hole) {
                let core = pad_core(p);
                let (ix0, iy0, ix1, iy1) = span_cells(super::grid::grow(p.bbox(), half));
                for iy in iy0..=iy1 {
                    for ix in ix0..=ix1 {
                        if gap(&disc(centre(ix, iy), 0.0), &core) <= half
                            && let Some(c) = copper.get_mut(idx(l, ix, iy))
                        {
                            *c = true;
                        }
                    }
                }
            }
            // Via rings.
            for (v, vn) in board.vias.iter().zip(&nets.via_nets) {
                if *vn != Some(net) || !v.on(*layer, n) {
                    continue;
                }
                let r = v.diameter / 2.0;
                let (ix0, iy0, ix1, iy1) = span_cells([[v.at[0] - r - half, v.at[1] - r - half], [v.at[0] + r + half, v.at[1] + r + half]]);
                for iy in iy0..=iy1 {
                    for ix in ix0..=ix1 {
                        let c = centre(ix, iy);
                        if (c[0] - v.at[0]).hypot(c[1] - v.at[1]) <= r + half
                            && let Some(x) = copper.get_mut(idx(l, ix, iy))
                        {
                            *x = true;
                        }
                    }
                }
            }
        }
        // Each layer's copper thickness, from the stack-up's copper in order.
        let stack = board.stackup_or_default();
        let mut thick: Vec<f64> = stack.iter().filter(|s| s.kind == StackKind::Copper).map(|s| s.thickness).collect();
        thick.resize(nl, DEFAULT_COPPER);
        let sheet: Vec<f64> = thick.iter().map(|t| if t.is_finite() && *t > 0.0 { t / RHO } else { DEFAULT_COPPER / RHO }).collect();
        let hop = if nl > 1 { board.thickness.max(0.1) / (nl - 1) as f64 } else { board.thickness.max(0.1) };
        let mut m = Model {
            net,
            net_name,
            cell,
            coarse,
            origin,
            nx,
            ny,
            layers,
            copper,
            plane,
            reference: vec![false; total],
            sheet,
            links: Vec::new(),
            reference_is: Reference::Plane { pour: 0, layer: CopperLayer::Top, area: 0.0 },
            last: Vec::new(),
            hop,
        };
        for (v, vn) in board.vias.iter().zip(&nets.via_nets) {
            if *vn == Some(net) {
                let (a, b) = v.span(n);
                m.link(v.at, v.drill, (a, b));
            }
        }
        for p in nets.pads.iter().filter(|p| p.net == Some(net) && p.top && p.bottom && p.pad.kind == PadKind::ThroughHole) {
            let drill = p.pad.drill.map_or(0.0, |[a, b]| a.min(b));
            if drill > 0.0 {
                m.link(p.at(), drill, (0, nl - 1));
            }
        }
        // The reference: the power entry's ground pads, else the largest plane shape.
        let entry = power_entry(ecad, nets, net);
        let mut placed = false;
        if let Some(designator) = entry {
            for p in nets.pads.iter().filter(|p| p.designator == designator && p.net == Some(net)) {
                for (l, layer) in m.layers.clone().iter().enumerate() {
                    if !p.on(*layer) {
                        continue;
                    }
                    let core = pad_core(p);
                    let (ix0, iy0, ix1, iy1) = span_cells(super::grid::grow(p.bbox(), half));
                    for iy in iy0..=iy1 {
                        for ix in ix0..=ix1 {
                            let i = idx(l, ix, iy);
                            if m.copper.get(i).copied().unwrap_or(false)
                                && gap(&disc(centre(ix, iy), 0.0), &core) <= half
                                && let Some(r) = m.reference.get_mut(i)
                            {
                                *r = true;
                                placed = true;
                            }
                        }
                    }
                }
            }
            if placed {
                m.reference_is = Reference::Entry(designator);
            }
        }
        if !placed {
            let Some((area, l, pour, cells)) = biggest else {
                return Err(format!("{} has no pour fill and no power entry to measure from", m.net_name));
            };
            for i in cells {
                if let Some(r) = m.reference.get_mut(i) {
                    *r = true;
                }
            }
            m.reference_is = Reference::Plane { pour, layer: m.layers.get(l).copied().unwrap_or(CopperLayer::Top), area };
        }
        Ok(m)
    }

    /// How many cells the grid has, all layers together (what a solve's work grows with).
    pub fn cells(&self) -> usize {
        self.copper.len()
    }

    fn cell_of(&self, p: [f64; 2]) -> Option<(usize, usize)> {
        let ix = ((p[0] - self.origin[0]) / self.cell).floor();
        let iy = ((p[1] - self.origin[1]) / self.cell).floor();
        (ix >= 0.0 && iy >= 0.0 && (ix as usize) < self.nx && (iy as usize) < self.ny).then_some((ix as usize, iy as usize))
    }

    fn index(&self, l: usize, ix: usize, iy: usize) -> usize {
        (l * self.ny + iy) * self.nx + ix
    }

    /// A plated hole of `drill` at `at` across the stack indexes `span`.
    fn link(&mut self, at: [f64; 2], drill: f64, (a, b): (usize, usize)) {
        let Some((ix, iy)) = self.cell_of(at) else { return };
        if !(drill.is_finite() && drill > 0.0) {
            return;
        }
        let g = std::f64::consts::PI * drill * PLATING / (RHO * self.hop);
        for l in a..b.min(self.layers.len().saturating_sub(1)) {
            let (i, j) = (self.index(l, ix, iy), self.index(l + 1, ix, iy));
            // The via's ring is copper on each layer it joins.
            for k in [i, j] {
                if let Some(c) = self.copper.get_mut(k) {
                    *c = true;
                }
            }
            self.links.push((i, j, g));
        }
    }

    /// Add a via of the net at `at` (a stitching via about to be placed): its ring copper on each
    /// layer of `span`, as a placed via's is, and its barrel.
    pub fn add_via(&mut self, at: [f64; 2], diameter: f64, drill: f64, span: (usize, usize)) {
        let (half, r) = (self.cell / 2.0, diameter / 2.0);
        if r.is_finite() && r > 0.0 {
            let reach = r + half;
            let lo = |v: f64, o: f64| ((v - reach - o) / self.cell).floor().max(0.0) as usize;
            let hi = |v: f64, o: f64, n: usize| (((v + reach - o) / self.cell).floor().max(0.0) as usize).min(n.saturating_sub(1));
            let (ix0, ix1) = (lo(at[0], self.origin[0]), hi(at[0], self.origin[0], self.nx));
            let (iy0, iy1) = (lo(at[1], self.origin[1]), hi(at[1], self.origin[1], self.ny));
            for l in span.0..=span.1.min(self.layers.len().saturating_sub(1)) {
                for iy in iy0..=iy1 {
                    for ix in ix0..=ix1 {
                        let c = [self.origin[0] + (ix as f64 + 0.5) * self.cell, self.origin[1] + (iy as f64 + 0.5) * self.cell];
                        let i = self.index(l, ix, iy);
                        if (c[0] - at[0]).hypot(c[1] - at[1]) <= reach
                            && let Some(x) = self.copper.get_mut(i)
                        {
                            *x = true;
                        }
                    }
                }
            }
        }
        self.link(at, drill, span);
    }

    /// Solve the potentials (see the module notes).
    pub fn solve(&mut self) -> GroundMap {
        let total = self.copper.len();
        // Neighbours by cell: the sides on a layer, and the links.
        let mut extra: BTreeMap<usize, Vec<(usize, f64)>> = BTreeMap::new();
        for (i, j, g) in &self.links {
            extra.entry(*i).or_default().push((*j, *g));
            extra.entry(*j).or_default().push((*i, *g));
        }
        let neighbours = |m: &Model, i: usize, out: &mut Vec<(usize, f64)>| {
            out.clear();
            let per = m.nx * m.ny;
            let l = i / per;
            let rest = i % per;
            let (iy, ix) = (rest / m.nx, rest % m.nx);
            let g = m.sheet.get(l).copied().unwrap_or(DEFAULT_COPPER / RHO);
            let mut push = |k: usize| {
                if m.copper.get(k).copied().unwrap_or(false) {
                    out.push((k, g));
                }
            };
            if ix > 0 {
                push(i - 1);
            }
            if ix + 1 < m.nx {
                push(i + 1);
            }
            if iy > 0 {
                push(i - m.nx);
            }
            if iy + 1 < m.ny {
                push(i + m.nx);
            }
            if let Some(e) = extra.get(&i) {
                out.extend(e.iter().filter(|(k, _)| m.copper.get(*k).copied().unwrap_or(false)));
            }
        };
        // What the reference reaches.
        let mut tied = vec![false; total];
        let mut queue: VecDeque<usize> = (0..total).filter(|i| self.reference.get(*i).copied().unwrap_or(false)).collect();
        for i in &queue {
            if let Some(t) = tied.get_mut(*i) {
                *t = true;
            }
        }
        let mut nb = Vec::new();
        while let Some(i) = queue.pop_front() {
            neighbours(self, i, &mut nb);
            for (k, _) in &nb {
                if let Some(t) = tied.get_mut(*k)
                    && !*t
                {
                    *t = true;
                    queue.push_back(*k);
                }
            }
        }
        // The unknowns: tied copper off the reference.
        let mut unknown = vec![usize::MAX; total];
        let mut cells: Vec<usize> = Vec::new();
        for i in 0..total {
            if tied.get(i).copied().unwrap_or(false) && !self.reference.get(i).copied().unwrap_or(false) {
                if let Some(u) = unknown.get_mut(i) {
                    *u = cells.len();
                }
                cells.push(i);
            }
        }
        let n = cells.len();
        // The matrix in rows: the diagonal and the unknown neighbours.
        let mut diag = vec![0.0; n];
        let mut row_start = Vec::with_capacity(n + 1);
        let mut cols: Vec<usize> = Vec::with_capacity(n * 4);
        let mut vals: Vec<f64> = Vec::with_capacity(n * 4);
        for (u, i) in cells.iter().enumerate() {
            row_start.push(cols.len());
            neighbours(self, *i, &mut nb);
            for (k, g) in &nb {
                if let Some(d) = diag.get_mut(u) {
                    *d += g;
                }
                let ku = unknown.get(*k).copied().unwrap_or(usize::MAX);
                if ku != usize::MAX {
                    cols.push(ku);
                    vals.push(*g);
                }
            }
        }
        row_start.push(cols.len());
        let current = DENSITY * self.cell * self.cell;
        let b = vec![current; n];
        // Start from the last answer where there is one.
        let mut x: Vec<f64> = cells.iter().map(|i| self.last.get(*i).copied().filter(|v| v.is_finite()).unwrap_or(0.0)).collect();
        let mul = |v: &[f64], out: &mut [f64]| {
            for u in 0..n {
                let (Some(s), Some(e)) = (row_start.get(u), row_start.get(u + 1)) else { continue };
                let mut acc = diag.get(u).copied().unwrap_or(0.0) * v.get(u).copied().unwrap_or(0.0);
                for k in *s..*e {
                    if let (Some(c), Some(g)) = (cols.get(k), vals.get(k)) {
                        acc -= g * v.get(*c).copied().unwrap_or(0.0);
                    }
                }
                if let Some(o) = out.get_mut(u) {
                    *o = acc;
                }
            }
        };
        let inv: Vec<f64> = diag.iter().map(|d| if *d > 0.0 { 1.0 / d } else { 0.0 }).collect();
        let mut ax = vec![0.0; n];
        mul(&x, &mut ax);
        let mut r: Vec<f64> = b.iter().zip(&ax).map(|(b, a)| b - a).collect();
        let mut z: Vec<f64> = r.iter().zip(&inv).map(|(r, m)| r * m).collect();
        let mut p = z.clone();
        let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        let norm_b = b.iter().map(|v| v * v).sum::<f64>().sqrt().max(f64::MIN_POSITIVE);
        let mut ap = vec![0.0; n];
        let mut iterations = 0;
        let mut converged = n == 0;
        while !converged && iterations < MAX_ITERATIONS {
            if r.iter().map(|v| v * v).sum::<f64>().sqrt() / norm_b <= TOLERANCE {
                converged = true;
                break;
            }
            mul(&p, &mut ap);
            let pap: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
            if !(pap.is_finite() && pap > 0.0) {
                break;
            }
            let alpha = rz / pap;
            for u in 0..n {
                if let (Some(xu), Some(pu), Some(ru), Some(apu)) = (x.get_mut(u), p.get(u), r.get_mut(u), ap.get(u)) {
                    *xu += alpha * pu;
                    *ru -= alpha * apu;
                }
            }
            for ((zu, ru), m) in z.iter_mut().zip(&r).zip(&inv) {
                *zu = ru * m;
            }
            let rz_next: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
            let beta = if rz != 0.0 { rz_next / rz } else { 0.0 };
            rz = rz_next;
            for (pu, zu) in p.iter_mut().zip(&z) {
                *pu = zu + beta * *pu;
            }
            iterations += 1;
        }
        if !converged && r.iter().map(|v| v * v).sum::<f64>().sqrt() / norm_b <= TOLERANCE {
            converged = true;
        }
        let mut volts = vec![NONE; total];
        for i in 0..total {
            if !self.copper.get(i).copied().unwrap_or(false) {
                continue;
            }
            let v = if self.reference.get(i).copied().unwrap_or(false) {
                0.0
            } else if !tied.get(i).copied().unwrap_or(false) {
                ISOLATED
            } else {
                unknown.get(i).and_then(|u| x.get(*u)).copied().unwrap_or(0.0).max(0.0)
            };
            if let Some(slot) = volts.get_mut(i) {
                *slot = v;
            }
        }
        self.last = volts.clone();
        GroundMap {
            net_name: self.net_name.clone(),
            cell: self.cell,
            coarse: self.coarse,
            origin: self.origin,
            nx: self.nx,
            ny: self.ny,
            layers: self.layers.clone(),
            mv: volts.into_iter().map(|v| if v.is_finite() { v * 1000.0 } else { v }).collect(),
            plane: self.plane.clone(),
            reference: self.reference_is.clone(),
            converged,
            iterations,
            unknowns: n,
        }
    }
}

/// The power entry's designator: a connector the intent keeps, with `net` and a net kept as power
/// on its pads (the first by designator).
fn power_entry(ecad: &Electronics, nets: &BoardNets, net: usize) -> Option<String> {
    let power: Vec<usize> =
        ecad.intent.nets.iter().filter(|r| r.role == NetRole::Power).filter_map(|r| nets.names.iter().position(|n| *n == r.net)).collect();
    let mut connectors: Vec<&str> = ecad.intent.parts.iter().filter(|p| p.role == PartRole::Connector).map(|p| p.part.as_str()).collect();
    connectors.sort_unstable();
    connectors
        .into_iter()
        .find(|d| {
            let on = |n: usize| nets.pads.iter().any(|p| p.designator == *d && p.net == Some(n));
            on(net) && power.iter().any(|n| on(*n))
        })
        .map(str::to_string)
}

impl GroundMap {
    fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// The cell's centre.
    pub fn centre(&self, ix: usize, iy: usize) -> [f64; 2] {
        [self.origin[0] + (ix as f64 + 0.5) * self.cell, self.origin[1] + (iy as f64 + 0.5) * self.cell]
    }

    /// The value of a cell on stack index `l`: `None` without copper; infinity isolated.
    pub fn value(&self, l: usize, ix: usize, iy: usize) -> Option<f64> {
        self.mv.get((l * self.ny + iy) * self.nx + ix).copied().filter(|v| !v.is_nan())
    }

    fn is_plane(&self, l: usize, ix: usize, iy: usize) -> bool {
        self.plane.get((l * self.ny + iy) * self.nx + ix).copied().unwrap_or(false)
    }

    /// The planes' isolated copper by layer, in mm² (layers with none left out).
    pub fn plane_isolated(&self) -> Vec<(CopperLayer, f64)> {
        let area = self.cell * self.cell;
        let mut out = Vec::new();
        for (l, layer) in self.layers.iter().enumerate() {
            let mut k = 0usize;
            for iy in 0..self.ny {
                for ix in 0..self.nx {
                    if self.is_plane(l, ix, iy) && self.value(l, ix, iy).is_some_and(f64::is_infinite) {
                        k += 1;
                    }
                }
            }
            if k > 0 {
                out.push((*layer, k as f64 * area));
            }
        }
        out
    }

    /// The most weakly tied plane cell that is tied (millivolts), with where; `None` with no plane.
    pub fn plane_worst(&self, layers: &[CopperLayer]) -> Option<Spot> {
        self.spots(layers, true, 1, 0.0).into_iter().next()
    }

    /// The `count` most weakly tied cells (of the planes only, when `planes`), at least `apart`
    /// millimetres from each other, worst first; isolated cells left out.
    pub fn spots(&self, layers: &[CopperLayer], planes: bool, count: usize, apart: f64) -> Vec<Spot> {
        let mut all: Vec<(f64, usize, usize, usize)> = Vec::new();
        for (l, layer) in self.layers.iter().enumerate() {
            if !layers.is_empty() && !layers.contains(layer) {
                continue;
            }
            for iy in 0..self.ny {
                for ix in 0..self.nx {
                    if planes && !self.is_plane(l, ix, iy) {
                        continue;
                    }
                    if let Some(v) = self.value(l, ix, iy).filter(|v| v.is_finite()) {
                        all.push((v, l, ix, iy));
                    }
                }
            }
        }
        all.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut out: Vec<Spot> = Vec::new();
        for (v, l, ix, iy) in all {
            if out.len() >= count {
                break;
            }
            let at = self.centre(ix, iy);
            if out.iter().any(|s| (s.at[0] - at[0]).hypot(s.at[1] - at[1]) < apart) {
                continue;
            }
            out.push(Spot { at, layer: self.layers.get(l).copied().unwrap_or(CopperLayer::Top), mv: v });
        }
        out
    }

    /// Each layer's copper: tied, isolated and weak (past `target` millivolts) shares.
    pub fn coverage(&self, layers: &[CopperLayer], target: f64) -> Vec<Coverage> {
        let area = self.cell * self.cell;
        let mut out = Vec::new();
        for (l, layer) in self.layers.iter().enumerate() {
            if !layers.is_empty() && !layers.contains(layer) {
                continue;
            }
            let (mut copper, mut tied, mut isolated, mut weak) = (0usize, 0usize, 0usize, 0usize);
            for iy in 0..self.ny {
                for ix in 0..self.nx {
                    let Some(v) = self.value(l, ix, iy) else { continue };
                    copper += 1;
                    if v.is_infinite() {
                        isolated += 1;
                    } else {
                        tied += 1;
                        if v > target {
                            weak += 1;
                        }
                    }
                }
            }
            let share = |k: usize| if copper > 0 { k as f64 / copper as f64 } else { 0.0 };
            out.push(Coverage { layer: *layer, copper_mm2: copper as f64 * area, tied: share(tied), isolated: share(isolated), weak: share(weak) });
        }
        out
    }

    /// A coarse map of stack index `l`: at most `most` cells each way, each the worst value of the
    /// cells it holds (`None` without copper, infinity isolated); with the side of its cells.
    pub fn coarse_map(&self, l: usize, most: usize) -> (f64, Vec<Vec<Option<f64>>>) {
        let k = self.nx.max(self.ny).div_ceil(most.max(1)).max(1);
        let (cx, cy) = (self.nx.div_ceil(k), self.ny.div_ceil(k));
        let mut rows = vec![vec![None; cx]; cy];
        if l >= self.layer_count() {
            return (self.cell * k as f64, rows);
        }
        for iy in 0..self.ny {
            for ix in 0..self.nx {
                let Some(v) = self.value(l, ix, iy) else { continue };
                if let Some(slot) = rows.get_mut(iy / k).and_then(|r| r.get_mut(ix / k)) {
                    *slot = Some(slot.map_or(v, |w: f64| w.max(v)));
                }
            }
        }
        (self.cell * k as f64, rows)
    }
}
