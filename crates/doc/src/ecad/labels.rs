//! The board's silkscreen names mode: where every footprint's name (designator) goes on the silk,
//! laid out for the whole board at once (`ecad.board.labels`).
//!
//! - **full**: every name on the silk; one that finds no legal spot stays at its usual spot and is
//!   counted as unplaceable.
//! - **fit**: the names that fit legally; the rest are hidden, left to the assembly drawing (whose
//!   pages print every name).
//! - **minimal**: only the must-keep set ([`must_keep`]: connectors, test points, switches and
//!   LEDs), where they fit; the rest are hidden.
//! - **none**: every name hidden.
//!
//! A legal spot is just outside the part's courtyard (on a side, centred or shifted along it),
//! reading at 0° (the board prints names upright, never at 180° or 270°), at the board's one name
//! height, which is at least the fab's least ([`least_height`]: `silk.size`'s height, and the
//! height whose stroke, a tenth of it, the fab prints). It keeps the legend clearance off pads
//! and open vias (`silk_to_pad`, `silk.clip`'s `open_via`), off holes (`hole_margin`) and the
//! board's edge (`edge_band`), off every courtyard on its side, off the names placed before it,
//! and nearer its own part's body than `silk.designator`'s `ratio` times any other's. Parts go
//! smallest courtyard first, as they have the fewest spots. Then the silk checks themselves
//! (`silk.size`, `silk.clip`, `silk.designator`) run on the result: in fit and minimal a name
//! they still flag is hidden, so those modes leave no silk finding on a name the mode placed; in
//! full it is counted as unplaceable.
//!
//! A name whose spot was set by hand (a `name_at` without the footprint's `name_placed`) is never
//! moved or hidden; it is kept and its box is kept clear by the rest. Values are left as they
//! are. Bounded: at most [`MAX_TESTS`] box tests, and [`ROUNDS`] runs of the checks.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::Electronics;
use super::board::{Board, ViaProtection, footprint_box};
use super::checks::gap;
use super::copper::{drill_core, inside, pad_core, pieces};
use super::footprint::{Core, FootprintItem, PadKind, Side};
use super::graphics::{BoardLayer, GraphicType, shapes_bbox};
use super::intent::PartRole;
use super::layers::CopperLayer;
use super::names::{self, Around};
use super::place::{Placement, snap_point};
use super::quality::{self, param_value};

/// How the board lays out its names (see the module notes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamesMode {
    Full,
    Fit,
    Minimal,
    None,
}

impl NamesMode {
    pub fn name(self) -> &'static str {
        match self {
            NamesMode::Full => "full",
            NamesMode::Fit => "fit",
            NamesMode::Minimal => "minimal",
            NamesMode::None => "none",
        }
    }
}

/// The most box tests one layout makes.
pub const MAX_TESTS: usize = 1 << 23;
/// How many times the silk checks run on the result, hiding what they flag.
pub const ROUNDS: usize = 4;
/// The highest name height asked for, in millimetres.
pub const MAX_HEIGHT: f64 = 5.0;
/// The gap kept between two names, in millimetres.
const NAME_GAP: f64 = 0.1;
/// How far out past the usual spot a name may be tried, as shares of its height.
const OUT: [f64; 3] = [0.0, 0.5, 1.0];
/// The checks a layout is verified by.
const CHECKS: [&str; 3] = ["silk.size", "silk.clip", "silk.designator"];

/// What a layout does: the board laid out, the height, and the names placed, hidden (left to the
/// assembly drawing), left where they were though no legal spot was found (full), and kept as set
/// by hand, by designator; and why, in English.
#[derive(Clone, Debug)]
pub struct Layout {
    pub mode: NamesMode,
    pub height: f64,
    pub board: Board,
    pub placed: Vec<String>,
    pub moved_to_assembly: Vec<String>,
    pub unplaceable: Vec<String>,
    pub kept: Vec<String>,
    pub why: Vec<String>,
    /// The footprints whose name changed, by id.
    pub changed: Vec<u64>,
}

/// The least name height the fab prints legibly: `silk.size`'s `min_height`, and the height whose
/// stroke (a tenth of it) is the fab's least stroke (0.10 mm on the fine and HDI profiles unless
/// the board sets it), rounded up to 0.05 mm.
pub fn least_height(board: &Board) -> f64 {
    let min_height = param_value(board, "silk.size", "min_height");
    let stroke = if board.quality_params.contains_key("silk.size.stroke_min") {
        param_value(board, "silk.size", "stroke_min")
    } else {
        match board.fab_profile.as_ref().map(|p| p.id.as_str()) {
            Some("fine" | "hdi") => 0.10,
            _ => param_value(board, "silk.size", "stroke_min"),
        }
    };
    (min_height.max(stroke / 0.1) * 20.0 - 1e-9).ceil() / 20.0
}

/// Whether the part `designator` is in the must-keep set: a connector the intent keeps as one,
/// or a designator of a connector (J, P, CN, X), a test point (TP), a switch (SW, S) or an LED
/// (LED); a part whose component is named as an LED too.
pub fn must_keep(ecad: &Electronics, designator: &str, component: &str) -> bool {
    if ecad.intent.parts.iter().any(|p| p.part == designator && p.role == PartRole::Connector) {
        return true;
    }
    let prefix: String = designator.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_uppercase();
    matches!(prefix.as_str(), "J" | "P" | "CN" | "X" | "TP" | "SW" | "S" | "LED") || component.to_ascii_uppercase().contains("LED")
}

fn courtyard(fp: &FootprintItem, placement: &Placement) -> Option<[[f64; 2]; 2]> {
    let local = shapes_bbox(fp.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Courtyard))).or_else(|| footprint_box(fp))?;
    Some(placement.bbox(local))
}

fn body(fp: &FootprintItem, placement: &Placement) -> Option<[[f64; 2]; 2]> {
    match shapes_bbox(fp.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Fab) && g.kind != GraphicType::Text)) {
        Some(local) => Some(placement.bbox(local)),
        None => courtyard(fp, placement),
    }
}

fn box_core([lo, hi]: [[f64; 2]; 2]) -> Core {
    Core { points: vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]], radius: 0.0 }
}

fn meets([a0, a1]: [[f64; 2]; 2], [b0, b1]: [[f64; 2]; 2], gap: f64) -> bool {
    a0[0] < b1[0] + gap && b0[0] < a1[0] + gap && a0[1] < b1[1] + gap && b0[1] < a1[1] + gap
}

fn box_distance(p: [f64; 2], [lo, hi]: [[f64; 2]; 2]) -> f64 {
    let dx = (lo[0] - p[0]).max(0.0).max(p[0] - hi[0]);
    let dy = (lo[1] - p[1]).max(0.0).max(p[1] - hi[1]);
    dx.hypot(dy)
}

/// One footprint's name to lay out.
struct Entry {
    index: usize,
    designator: String,
    side: Side,
    court: [[f64; 2]; 2],
    body: [[f64; 2]; 2],
    keep: bool,
    /// Set by hand: kept where it is.
    pinned: bool,
}

/// What a name box must keep off on one side of the board.
struct Keepoff {
    side: Side,
    core: Core,
    /// How far off.
    margin: f64,
}

/// The board's names laid out in `mode`, `height` high (at least [`least_height`]; that when left
/// out); why not for a height out of range.
pub fn layout(ecad: &Electronics, mode: NamesMode, height: Option<f64>) -> Result<Layout, String> {
    let board = &ecad.board;
    let least = least_height(board);
    let mut why = Vec::new();
    let h = match height {
        Some(v) if !(v.is_finite() && v > 0.0 && v <= MAX_HEIGHT) => return Err(format!("`height` must be more than 0 and at most {MAX_HEIGHT} mm")),
        Some(v) if v < least - 1e-9 => {
            why.push(format!("names are {} mm high, not {} mm: the least the fab prints legibly on this board", fmt(least), fmt(v)));
            least
        }
        Some(v) => v,
        None => {
            why.push(format!("names are {} mm high: the least the fab prints legibly on this board", fmt(least)));
            least
        }
    };
    let mut out = board.clone();
    out.name_height = Some(h);
    out.names_mode = Some(mode);
    let nets = ecad.board_nets();
    let silk_to_pad = board.rules.silk_to_pad;
    let (open_via, hole_margin, edge_band) =
        (param_value(board, "silk.clip", "open_via"), param_value(board, "silk.clip", "hole_margin"), param_value(board, "silk.clip", "edge_band"));
    let ratio = param_value(board, "silk.designator", "ratio");
    // What names keep off, by side.
    let mut keepoff: Vec<Keepoff> = Vec::new();
    for p in &nets.pads {
        let core = match p.pad.kind {
            PadKind::Hole => match drill_core(p) {
                Some(c) => c,
                None => continue,
            },
            _ => pad_core(p),
        };
        let margin = if p.pad.kind == PadKind::Hole { hole_margin } else { silk_to_pad };
        for side in [Side::Top, Side::Bottom] {
            let on = p.pad.kind == PadKind::Hole || if side == Side::Top { p.top } else { p.bottom };
            if on {
                keepoff.push(Keepoff { side, core: core.clone(), margin });
            }
        }
    }
    let n = board.copper_layers;
    for v in &board.vias {
        let open = v.protection == ViaProtection::Open && v.diameter >= open_via - 1e-9;
        for (side, layer) in [(Side::Top, CopperLayer::Top), (Side::Bottom, CopperLayer::Bottom)] {
            if v.on(layer, n) {
                let (core, margin) = if open {
                    (Core { points: vec![v.at], radius: v.diameter / 2.0 }, silk_to_pad)
                } else {
                    (Core { points: vec![v.at], radius: v.drill / 2.0 }, hole_margin)
                };
                keepoff.push(Keepoff { side, core, margin });
            }
        }
    }
    for hole in &board.holes {
        for side in [Side::Top, Side::Bottom] {
            keepoff.push(Keepoff { side, core: Core { points: vec![hole.at], radius: hole.drill / 2.0 }, margin: hole_margin });
        }
    }
    let ring = if board.outline.is_empty() { Vec::new() } else { super::path::tessellate(&board.outline, &board.outline_curves, true, 0.01) };
    let edges: Vec<Core> = if board.outline.is_empty() {
        Vec::new()
    } else {
        pieces(&board.outline, &board.outline_curves, true).into_iter().map(|(a, b)| Core { points: vec![a, b], radius: 0.0 }).collect()
    };
    // The names to lay out.
    let mut entries: Vec<Entry> = Vec::new();
    for (index, f) in board.footprints.iter().enumerate() {
        let Some(placed) = ecad.placed(f) else { continue };
        let placement = f.placement();
        let fp = placed.resolved.footprint;
        let Some(court) = courtyard(fp, &placement) else { continue };
        let side = if placement.mirror { Side::Bottom } else { Side::Top };
        let designator = placed.part.designator.clone();
        if designator.is_empty() {
            continue;
        }
        let keep = must_keep(ecad, &designator, &placed.resolved.component.name);
        let pinned = f.name_at.is_some() && !f.name_placed;
        entries.push(Entry { index, designator, side, court, body: body(fp, &placement).unwrap_or(court), keep, pinned });
    }
    let area = |b: &[[f64; 2]; 2]| (b[1][0] - b[0][0]) * (b[1][1] - b[0][1]);
    entries.sort_by(|a, b| area(&a.court).total_cmp(&area(&b.court)).then_with(|| super::connect::natural_cmp(&a.designator, &b.designator)));
    let mut budget = MAX_TESTS;
    let mut taken: Vec<(Side, [[f64; 2]; 2])> = Vec::new();
    // Names set by hand keep their spot, and the rest keep off them.
    for e in entries.iter().filter(|e| e.pinned) {
        if let Some(f) = board.footprints.get(e.index)
            && let Some(o) = f.name_at
            && f.show_name
        {
            let at = [f.at[0] + o[0], f.at[1] + o[1]];
            let w = names::text_width(h, e.designator.chars().count());
            taken.push((e.side, [at, [at[0] + w, at[1] + h]]));
        }
    }
    let courts: Vec<(Side, [[f64; 2]; 2])> = entries.iter().map(|e| (e.side, e.court)).collect();
    let bodies: Vec<(usize, Side, [[f64; 2]; 2])> = entries.iter().map(|e| (e.index, e.side, e.body)).collect();
    let mut result = Layout {
        mode,
        height: h,
        board: out,
        placed: Vec::new(),
        moved_to_assembly: Vec::new(),
        unplaceable: Vec::new(),
        kept: Vec::new(),
        why,
        changed: Vec::new(),
    };
    let mut ours: Vec<usize> = Vec::new();
    for e in &entries {
        if e.pinned {
            result.kept.push(e.designator.clone());
            continue;
        }
        let wanted = match mode {
            NamesMode::None => false,
            NamesMode::Minimal => e.keep,
            NamesMode::Fit | NamesMode::Full => true,
        };
        let w = names::text_width(h, e.designator.chars().count());
        let spot = if wanted { find_spot(e, [w, h], &keepoff, &courts, &bodies, &taken, &ring, &edges, edge_band, ratio, &mut budget) } else { None };
        let Some(f) = result.board.footprints.get_mut(e.index) else { continue };
        let before = (f.name_at, f.show_name, f.name_placed);
        match (wanted, spot) {
            (true, Some(at)) => {
                taken.push((e.side, [at, [at[0] + w, at[1] + h]]));
                f.name_at = Some(snap_point([at[0] - f.at[0], at[1] - f.at[1]]));
                f.show_name = true;
                result.placed.push(e.designator.clone());
                ours.push(e.index);
            }
            (true, None) if mode == NamesMode::Full => {
                // Every name stays on the silk: at its usual spot, counted as unplaceable.
                let at = names::spot(e.court, Around::Above, [w, h]);
                f.name_at = Some(snap_point([at[0] - f.at[0], at[1] - f.at[1]]));
                f.show_name = true;
                result.unplaceable.push(e.designator.clone());
            }
            _ => {
                f.show_name = false;
                result.moved_to_assembly.push(e.designator.clone());
            }
        }
        f.name_placed = true;
        if before != (f.name_at, f.show_name, f.name_placed) {
            result.changed.push(f.id);
        }
    }
    if budget == 0 {
        result.why.push("the board was too busy to try every spot: the names left over went to the assembly drawing".into());
    }
    // The silk checks on what was laid out: a name of ours they still flag goes (fit, minimal) or
    // counts as unplaceable (full).
    let only: BTreeSet<&str> = CHECKS.into_iter().collect();
    for _ in 0..ROUNDS {
        if ours.is_empty() || mode == NamesMode::None {
            break;
        }
        let mut probe = ecad.clone();
        probe.board = result.board.clone();
        let report = quality::check(&probe, &probe.board_nets(), Some(&only));
        let flagged: BTreeSet<String> = report
            .findings
            .iter()
            .filter(|q| q.finding.items.iter().any(|i| matches!(i, super::drc::DrcItem::Footprint { .. })))
            .filter_map(|q| q.finding.message.split("'s name").next().filter(|_| q.finding.message.contains("'s name")).map(str::to_string))
            .collect();
        let bad: Vec<usize> = ours.iter().copied().filter(|i| entries.iter().any(|e| e.index == *i && flagged.contains(&e.designator))).collect();
        if bad.is_empty() {
            break;
        }
        for i in &bad {
            let Some(e) = entries.iter().find(|e| e.index == *i) else { continue };
            result.placed.retain(|d| *d != e.designator);
            if mode == NamesMode::Full {
                result.unplaceable.push(e.designator.clone());
            } else if let Some(f) = result.board.footprints.get_mut(*i) {
                f.show_name = false;
                result.moved_to_assembly.push(e.designator.clone());
            }
        }
        ours.retain(|i| !bad.contains(i));
    }
    for list in [&mut result.placed, &mut result.moved_to_assembly, &mut result.unplaceable, &mut result.kept] {
        list.sort_by(|a, b| super::connect::natural_cmp(a, b));
        list.dedup();
    }
    result.changed.sort_unstable();
    result.changed.dedup();
    Ok(result)
}

fn fmt(v: f64) -> String {
    let s = format!("{:.3}", (v * 1000.0).round() / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The first legal spot (a name's lower left) round `e`'s courtyard, by the sides in the order
/// tried, centred first, then shifted along the side, then further out.
#[allow(clippy::too_many_arguments)]
fn find_spot(
    e: &Entry,
    [w, h]: [f64; 2],
    keepoff: &[Keepoff],
    courts: &[(Side, [[f64; 2]; 2])],
    bodies: &[(usize, Side, [[f64; 2]; 2])],
    taken: &[(Side, [[f64; 2]; 2])],
    ring: &[[f64; 2]],
    edges: &[Core],
    edge_band: f64,
    ratio: f64,
    budget: &mut usize,
) -> Option<[f64; 2]> {
    for a in [Around::Above, Around::Below, Around::Right, Around::Left] {
        let base = names::spot(e.court, a, [w, h]);
        let along: [f64; 5] = match a {
            Around::Above | Around::Below => [0.0, -w / 2.0, w / 2.0, -w, w],
            Around::Right | Around::Left => [0.0, -h, h, -2.0 * h, 2.0 * h],
        };
        for out in OUT {
            let push = out * h;
            let (ox, oy) = match a {
                Around::Above => (0.0, push),
                Around::Below => (0.0, -push),
                Around::Right => (push, 0.0),
                Around::Left => (-push, 0.0),
            };
            for s in along {
                let at = match a {
                    Around::Above | Around::Below => [base[0] + s + ox, base[1] + oy],
                    Around::Right | Around::Left => [base[0] + ox, base[1] + s + oy],
                };
                let at = snap_point(at);
                let b = [at, [at[0] + w, at[1] + h]];
                if legal(e, b, keepoff, courts, bodies, taken, ring, edges, edge_band, ratio, budget) {
                    return Some(at);
                }
                if *budget == 0 {
                    return None;
                }
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn legal(
    e: &Entry,
    b: [[f64; 2]; 2],
    keepoff: &[Keepoff],
    courts: &[(Side, [[f64; 2]; 2])],
    bodies: &[(usize, Side, [[f64; 2]; 2])],
    taken: &[(Side, [[f64; 2]; 2])],
    ring: &[[f64; 2]],
    edges: &[Core],
    edge_band: f64,
    ratio: f64,
    budget: &mut usize,
) -> bool {
    let cost = keepoff.len() + courts.len() + bodies.len() + taken.len() + edges.len() + ring.len();
    if *budget < cost {
        *budget = 0;
        return false;
    }
    *budget -= cost;
    let core = box_core(b);
    // On the board, clear of its edge.
    if !ring.is_empty() && !core.points.iter().all(|p| inside(*p, ring)) {
        return false;
    }
    if edges.iter().any(|edge| gap(&core, edge) < edge_band) {
        return false;
    }
    if courts.iter().any(|(s, c)| *s == e.side && meets(b, *c, 0.0)) {
        return false;
    }
    if taken.iter().any(|(s, t)| *s == e.side && meets(b, *t, NAME_GAP)) {
        return false;
    }
    if keepoff.iter().any(|k| k.side == e.side && gap(&core, &k.core) < k.margin) {
        return false;
    }
    // Nearer its own body than `ratio` times any other's on its side.
    let c = [(b[0][0] + b[1][0]) / 2.0, (b[0][1] + b[1][1]) / 2.0];
    let own = box_distance(c, e.body);
    !bodies.iter().any(|(i, s, other)| *i != e.index && *s == e.side && box_distance(c, *other) < own * ratio)
}
