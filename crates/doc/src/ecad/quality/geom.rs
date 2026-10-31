//! Geometry the design-quality checks share, worked out once per check run and only for the
//! layers a check asks about:
//! - the planes of each copper layer: its pours' fills of a net (see [`super::super::fill`]),
//!   which net covers a point, and the void (a hole in a fill, or no fill at all) a point is in;
//! - what pierces each layer: vias whose span has it and through-hole pads, with a grid;
//! - the reference plane under a run of points: the neighbouring layer whose planes cover most
//!   of it, and its dielectric height;
//! - points along a segment, and a spatial index of points with nearest-neighbour queries.
//!
//! Planes are pour fills only: a track or a copper drawing is no plane. Every query is bounded by
//! what it is given; callers charge their work to the run's budget (see [`super::Cx::spend`]).

use std::cell::OnceCell;
use std::collections::BTreeMap;

use super::super::airwires::BoardNets;
use super::super::board::Board;
use super::super::copper::{inside, pieces};
use super::super::drc::DrcItem;
use super::super::grid::{Grid, boxes_meet};
use super::super::layers::{CopperLayer, stack};
use super::super::stackup::{StackLayer, surroundings};

/// The most points [`along`] gives for one segment.
pub const MAX_SAMPLES: usize = 100_000;
/// The grid cell of the planes' holes and the piercers, in millimetres.
const CELL: f64 = 2.0;

/// Points from `a` to `b`, both ends included, no farther apart than `step` (at most
/// [`MAX_SAMPLES`]).
pub fn along(a: [f64; 2], b: [f64; 2], step: f64) -> Vec<[f64; 2]> {
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    let step = if step.is_finite() && step > 0.0 { step } else { 0.1 };
    let n = if len.is_finite() { ((len / step).ceil() as usize).clamp(1, MAX_SAMPLES.saturating_sub(1)) } else { 1 };
    (0..=n).map(|k| k as f64 / n as f64).map(|t| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]).collect()
}

/// A track's straight pieces (arcs cut within the copper tolerance).
pub fn track_pieces(points: &[[f64; 2]], curves: &[f64]) -> Vec<([f64; 2], [f64; 2])> {
    pieces(points, curves, false)
}

pub fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// A hole in a plane's fill: copper all round it on that layer.
#[derive(Clone, Debug)]
pub struct Hole {
    /// By index into the board's fills.
    pub fill: usize,
    pub ring: Vec<[f64; 2]>,
    pub bbox: [[f64; 2]; 2],
}

impl Hole {
    /// Its longest side: how far round it a return current goes, at most.
    pub fn extent(&self) -> f64 {
        let [[x0, y0], [x1, y1]] = self.bbox;
        (x1 - x0).max(y1 - y0)
    }

    pub fn holds(&self, p: [f64; 2]) -> bool {
        p[0] >= self.bbox[0][0] && p[0] <= self.bbox[1][0] && p[1] >= self.bbox[0][1] && p[1] <= self.bbox[1][1] && inside(p, &self.ring)
    }
}

/// Where a point off a layer's planes is.
#[derive(Clone, Copy, Debug)]
pub enum Void<'p> {
    /// In a hole of a fill.
    Hole(&'p Hole),
    /// Outside every fill: past a plane's edge, or between two planes (a split).
    Outside,
}

/// A layer's planes (see the module notes).
pub struct Plane {
    pub layer: CopperLayer,
    /// The fills on it that have a net: by index into the board's fills, with the net.
    pub fills: Vec<(usize, usize)>,
    pub holes: Vec<Hole>,
    grid: Grid,
}

impl Plane {
    pub fn is_empty(&self) -> bool {
        self.fills.is_empty()
    }

    /// The net of the plane each point is on; `None` off every plane.
    pub fn nets_at(&self, nets: &BoardNets, points: &[[f64; 2]]) -> Vec<Option<usize>> {
        let mut out: Vec<Option<usize>> = vec![None; points.len()];
        for (fill, net) in &self.fills {
            let Some(f) = nets.fills.get(*fill) else { continue };
            for (slot, found) in out.iter_mut().zip(f.region.locate(points)) {
                if slot.is_none() && found.is_some() {
                    *slot = Some(*net);
                }
            }
        }
        out
    }

    /// The void a point off the planes is in.
    pub fn void_at(&self, p: [f64; 2]) -> Void<'_> {
        let probe = [p, p];
        self.grid
            .near_box(probe, self.holes.len())
            .into_iter()
            .filter_map(|i| self.holes.get(i))
            .find(|h| h.holds(p))
            .map_or(Void::Outside, Void::Hole)
    }

    /// The clearance the fill `fill` keeps (its pour's, or its net class's).
    pub fn clearance(&self, nets: &BoardNets, fill: usize) -> f64 {
        nets.fills.get(fill).map_or(0.0, |f| f.clearance)
    }
}

/// Copper through a layer: a via whose span has it, or a through-hole pad.
#[derive(Clone, Debug)]
pub struct Piercer {
    pub at: [f64; 2],
    /// Of its copper on the layer.
    pub radius: f64,
    pub net: Option<usize>,
    pub item: DrcItem,
    /// By index into the board's vias, for a via.
    pub via: Option<usize>,
}

/// What pierces one layer.
pub struct Piercers {
    pub list: Vec<Piercer>,
    grid: Grid,
}

impl Piercers {
    /// Those whose copper may reach into `bbox`.
    pub fn near(&self, bbox: [[f64; 2]; 2]) -> impl Iterator<Item = &Piercer> {
        self.grid
            .near_box(bbox, self.list.len())
            .into_iter()
            .filter_map(|i| self.list.get(i))
            .filter(move |p| boxes_meet([[p.at[0] - p.radius, p.at[1] - p.radius], [p.at[0] + p.radius, p.at[1] + p.radius]], bbox))
    }
}

/// The plane a run of points references.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reference {
    pub layer: CopperLayer,
    /// The dielectric between the two layers, in millimetres.
    pub h: f64,
    /// The share of the points (those counted) on the layer's planes.
    pub coverage: f64,
    /// The plane net most of the covered points are on.
    pub net: Option<usize>,
}

/// The geometry a check run shares (see the module notes).
pub struct Geo<'a> {
    pub board: &'a Board,
    pub nets: &'a BoardNets,
    /// The copper layers, top to bottom.
    pub layers: Vec<CopperLayer>,
    pub stackup: Vec<StackLayer>,
    planes: Vec<OnceCell<Plane>>,
    piercers: Vec<OnceCell<Piercers>>,
}

impl<'a> Geo<'a> {
    pub fn new(board: &'a Board, nets: &'a BoardNets) -> Geo<'a> {
        let layers = stack(board.copper_layers);
        let n = layers.len();
        Geo {
            board,
            nets,
            layers,
            stackup: board.stackup_or_default(),
            planes: (0..n).map(|_| OnceCell::new()).collect(),
            piercers: (0..n).map(|_| OnceCell::new()).collect(),
        }
    }

    fn index(&self, layer: CopperLayer) -> Option<usize> {
        self.layers.iter().position(|l| *l == layer)
    }

    /// The layers right above and below `layer`, each with the dielectric between.
    pub fn neighbours(&self, layer: CopperLayer) -> Vec<(CopperLayer, f64)> {
        let Some(i) = self.index(layer) else { return Vec::new() };
        let s = surroundings(&self.stackup, layer, self.board.copper_layers);
        let mut out = Vec::new();
        if let Some(l) = i.checked_sub(1).and_then(|j| self.layers.get(j)) {
            out.push((*l, s.and_then(|s| s.above).map_or(0.0, |d| d.thickness)));
        }
        if let Some(l) = self.layers.get(i + 1) {
            out.push((*l, s.and_then(|s| s.below).map_or(0.0, |d| d.thickness)));
        }
        out
    }

    /// The dielectric between two layers, in millimetres (each dielectric between counted).
    pub fn height(&self, a: CopperLayer, b: CopperLayer) -> f64 {
        let (Some(i), Some(j)) = (self.index(a), self.index(b)) else { return 0.0 };
        let (lo, hi) = (i.min(j), i.max(j));
        let mut copper = 0usize;
        let mut h = 0.0;
        for l in &self.stackup {
            if l.is_dielectric() {
                if copper > lo && copper <= hi {
                    h += l.thickness;
                }
            } else if l.kind == super::super::stackup::StackKind::Copper {
                copper += 1;
            }
        }
        h
    }

    /// The dielectrics' εr between two layers, weighted by thickness (the default εr when none
    /// is between).
    pub fn er(&self, a: CopperLayer, b: CopperLayer) -> f64 {
        let (Some(i), Some(j)) = (self.index(a), self.index(b)) else { return super::super::stackup::DEFAULT_ER };
        let (lo, hi) = (i.min(j), i.max(j));
        let (mut copper, mut h, mut eh) = (0usize, 0.0, 0.0);
        for l in &self.stackup {
            if l.is_dielectric() {
                if copper > lo && copper <= hi {
                    h += l.thickness;
                    eh += l.thickness * l.er();
                }
            } else if l.kind == super::super::stackup::StackKind::Copper {
                copper += 1;
            }
        }
        if h > 0.0 { eh / h } else { super::super::stackup::DEFAULT_ER }
    }

    /// The layer's planes.
    pub fn plane(&self, layer: CopperLayer) -> Option<&Plane> {
        let cell = self.planes.get(self.index(layer)?)?;
        Some(cell.get_or_init(|| self.build_plane(layer)))
    }

    fn build_plane(&self, layer: CopperLayer) -> Plane {
        let mut fills = Vec::new();
        let mut holes = Vec::new();
        let mut grid = Grid::new(CELL);
        for (i, f) in self.nets.fills.iter().enumerate() {
            let Some(net) = f.net else { continue };
            if f.layer != layer {
                continue;
            }
            fills.push((i, net));
            for shape in f.region.shapes() {
                for ring in shape.holes {
                    let Some(bbox) = super::super::bbox(ring.iter().copied()) else { continue };
                    grid.insert(holes.len(), bbox);
                    holes.push(Hole { fill: i, ring, bbox });
                }
            }
        }
        Plane { layer, fills, holes, grid }
    }

    /// What pierces the layer.
    pub fn piercers(&self, layer: CopperLayer) -> Option<&Piercers> {
        let cell = self.piercers.get(self.index(layer)?)?;
        Some(cell.get_or_init(|| self.build_piercers(layer)))
    }

    fn build_piercers(&self, layer: CopperLayer) -> Piercers {
        let mut list = Vec::new();
        let layers = self.board.copper_layers;
        for (i, v) in self.board.vias.iter().enumerate() {
            if v.on(layer, layers) {
                let net = self.nets.via_nets.get(i).copied().flatten();
                list.push(Piercer { at: v.at, radius: v.diameter / 2.0, net, item: DrcItem::Via(v.id), via: Some(i) });
            }
        }
        for p in self.nets.pads.iter().filter(|p| p.top && p.bottom && p.pad.drill.is_some()) {
            let [[x0, y0], [x1, y1]] = p.bbox();
            let item = DrcItem::Pad { footprint: p.footprint, number: p.pad.number.clone(), reference: p.reference() };
            list.push(Piercer { at: p.at(), radius: (x1 - x0).max(y1 - y0) / 2.0, net: p.net, item, via: None });
        }
        let mut grid = Grid::new(CELL);
        for (i, p) in list.iter().enumerate() {
            grid.insert(i, [[p.at[0] - p.radius, p.at[1] - p.radius], [p.at[0] + p.radius, p.at[1] + p.radius]]);
        }
        Piercers { list, grid }
    }

    /// Which of `points` (on the layer `layer`'s neighbour `plane_layer`) lie over the antipad of
    /// copper of the net `own` that pierces `plane_layer`: within its radius, the plane's widest
    /// clearance and `slack`. A track's own vias and through-hole pads cut its reference plane
    /// round them, and that is no gap it crosses.
    pub fn own_antipads(&self, plane_layer: CopperLayer, points: &[[f64; 2]], own: Option<usize>, slack: f64) -> Vec<bool> {
        let mut out = vec![false; points.len()];
        let (Some(own), Some(pierce), Some(bbox)) = (own, self.piercers(plane_layer), super::super::bbox(points.iter().copied())) else { return out };
        let clearance = self.plane(plane_layer).map_or(0.0, |pl| pl.fills.iter().map(|(f, _)| pl.clearance(self.nets, *f)).fold(0.0, f64::max));
        let reach = clearance + slack.max(0.0);
        let mine: Vec<&Piercer> = pierce.near(super::super::grid::grow(bbox, reach)).filter(|q| q.net == Some(own)).collect();
        if mine.is_empty() {
            return out;
        }
        for (slot, p) in out.iter_mut().zip(points) {
            *slot = mine.iter().any(|q| distance(*p, q.at) <= q.radius + reach);
        }
        out
    }

    /// The plane `points` (on `layer`, of the net `own`) reference (see [`Under`]): of the
    /// neighbouring layers with planes, the one whose planes of other nets cover the most of the
    /// points not over the net's own antipads, the thinner dielectric among equals; `None` when
    /// no neighbour has a plane under any of them.
    pub fn under(&self, layer: CopperLayer, points: &[[f64; 2]], own: Option<usize>, slack: f64) -> Option<Under> {
        let mut looks: Vec<(CopperLayer, f64, Vec<Option<usize>>, Vec<bool>)> = Vec::new();
        for (l, h) in self.neighbours(layer) {
            let Some(plane) = self.plane(l).filter(|p| !p.is_empty()) else { continue };
            let mut at = plane.nets_at(self.nets, points);
            for n in at.iter_mut() {
                if *n == own && own.is_some() {
                    *n = None;
                }
            }
            let skip = self.own_antipads(l, points, own, slack);
            looks.push((l, h, at, skip));
        }
        let mut best: Option<(usize, Reference)> = None;
        for (i, (l, h, at, skip)) in looks.iter().enumerate() {
            let mut counted = 0usize;
            let mut by_net: BTreeMap<usize, usize> = BTreeMap::new();
            for (n, s) in at.iter().zip(skip) {
                if *s {
                    continue;
                }
                counted += 1;
                if let Some(n) = n {
                    *by_net.entry(*n).or_default() += 1;
                }
            }
            let on: usize = by_net.values().sum();
            if on == 0 {
                continue;
            }
            let coverage = on as f64 / counted.max(1) as f64;
            let net = by_net.iter().max_by_key(|(_, c)| **c).map(|(n, _)| *n);
            let better = best.is_none_or(|(_, b)| coverage > b.coverage + 1e-9 || ((coverage - b.coverage).abs() <= 1e-9 && *h < b.h));
            if better {
                best = Some((i, Reference { layer: *l, h: *h, coverage, net }));
            }
        }
        let (i, reference) = best?;
        let mut nets = Vec::new();
        let mut skip = Vec::new();
        let mut others = Vec::new();
        for (k, (l, _, at, s)) in looks.into_iter().enumerate() {
            if k == i {
                (nets, skip) = (at, s);
            } else {
                others.push((l, at));
            }
        }
        Some(Under { reference, nets, skip, others })
    }
}

/// A run of points over its reference plane (see [`Geo::under`]).
#[derive(Clone, Debug)]
pub struct Under {
    pub reference: Reference,
    /// The plane net under each point on the reference layer (`None` off it).
    pub nets: Vec<Option<usize>>,
    /// The points over the net's own antipads, which count neither way.
    pub skip: Vec<bool>,
    /// The other neighbouring layers with planes, and the plane net under each point there.
    pub others: Vec<(CopperLayer, Vec<Option<usize>>)>,
}

/// Points with a grid, for the nearest one to a place.
pub struct Sites {
    pub points: Vec<[f64; 2]>,
    grid: Grid,
}

impl Sites {
    pub fn new(points: Vec<[f64; 2]>, side: f64) -> Sites {
        let mut grid = Grid::new(side);
        for (i, p) in points.iter().enumerate() {
            grid.insert(i, [*p, *p]);
        }
        Sites { points, grid }
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The nearest point to `p` within `reach`, and how far; `None` with none that near (see
    /// [`Sites::nearest_counted`]).
    pub fn nearest(&self, p: [f64; 2], reach: f64) -> Option<(usize, f64)> {
        self.nearest_counted(p, reach).0
    }

    /// The nearest point to `p` within `reach`, and how many points and cells were looked at
    /// (for the caller's budget). Looks ring by ring of cells out from `p`'s, stopping once no
    /// farther ring can hold a nearer point; past as many cells as there are points, it looks at
    /// every point instead, so a query costs at most about that.
    pub fn nearest_counted(&self, p: [f64; 2], reach: f64) -> (Option<(usize, f64)>, usize) {
        if self.points.is_empty() || !(reach.is_finite() && reach >= 0.0 && p[0].is_finite() && p[1].is_finite()) {
            return (None, 1);
        }
        let side = self.grid.side();
        let (cx, cy) = self.grid.cell_of(p);
        let mut best: Option<(usize, f64)> = None;
        let mut looked = 0usize;
        let test = |i: usize, best: &mut Option<(usize, f64)>| {
            if let Some(q) = self.points.get(i) {
                let d = distance(p, *q);
                if d <= reach && best.is_none_or(|(_, b)| d < b) {
                    *best = Some((i, d));
                }
            }
        };
        for i in self.grid.wide() {
            test(*i, &mut best);
        }
        looked = looked.saturating_add(self.grid.wide().len());
        let rings = (reach / side).ceil().clamp(0.0, 1e9) as i64 + 1;
        for k in 0..=rings {
            // Every point in a ring past k is at least k cells' side away.
            if best.is_some_and(|(_, d)| d <= (k as f64 - 1.0).max(0.0) * side) {
                break;
            }
            if looked > self.points.len() {
                // Cheaper to look at them all.
                for i in 0..self.points.len() {
                    test(i, &mut best);
                }
                return (best, looked.saturating_add(self.points.len()));
            }
            let ring: Vec<(i64, i64)> = if k == 0 {
                vec![(cx, cy)]
            } else {
                let (x0, x1, y0, y1) = (cx.saturating_sub(k), cx.saturating_add(k), cy.saturating_sub(k), cy.saturating_add(k));
                let mut r: Vec<(i64, i64)> = Vec::new();
                for x in x0..=x1 {
                    r.push((x, y0));
                    r.push((x, y1));
                }
                for y in (y0 + 1)..y1 {
                    r.push((x0, y));
                    r.push((x1, y));
                }
                r
            };
            for c in ring {
                looked = looked.saturating_add(1);
                for i in self.grid.in_cell(c) {
                    looked = looked.saturating_add(1);
                    test(*i, &mut best);
                }
            }
        }
        (best, looked)
    }
}
