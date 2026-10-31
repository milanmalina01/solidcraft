//! The board's pages: top and bottom (copper with its pours, pads, vias and drills, silkscreen
//! and outline; the bottom mirrored, as seen from below), a page per inner copper layer, assembly
//! top and bottom (outlines and designators of the parts on that side, pad 1 and cathodes marked,
//! parts not fitted struck through and tagged "DNP"), and the layer stack-up (a table and a
//! cross-section). Each at a nice scale that fits, with the board's size dimensioned. Also the
//! cover's picture of the top side.

use std::collections::BTreeSet;

use solidcraft_doc::ecad::airwires::{board_nets, board_pads};
use solidcraft_doc::ecad::board::{footprint_box, other};
use solidcraft_doc::ecad::graphics::shapes_bbox;
use solidcraft_doc::ecad::impedance::{LineKind, impedance, medium};
use solidcraft_doc::ecad::path::tessellate;
use solidcraft_doc::ecad::stackup::StackKind;
use solidcraft_doc::ecad::{
    BoardLayer, BoardNets, BoardPad, Connectivity, CopperLayer, DrawLayer, Electronics, Graphic, GraphicType, Placement, Side, names,
};

use super::border::{Sheet, nice_scale, overlaps, scale_text};
use super::font::Font;
use super::page::{Align, Baseline, Item, Link, Page, Rgb, Stroke, Text};
use super::{MIN_TEXT, PageKind, Palette};

/// The room the dimensions take right of and below the board, in millimetres.
const DIM_ROOM: f64 = 22.0;
/// How far out from the board's box its dimension lines run.
const DIM_OUT: f64 = 11.0;
/// A "DNP" tag's gap from its part on the assembly pages.
const TAG_GAP: f64 = 0.6;

/// Board to page: mirrored left to right for a view from the bottom.
#[derive(Clone, Copy)]
struct View {
    scale: f64,
    /// The board box drawn.
    lo: [f64; 2],
    hi: [f64; 2],
    /// Its top left on the page.
    at: [f64; 2],
    mirror: bool,
}

impl View {
    fn p(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        let x = if self.mirror { self.lo[0] + self.hi[0] - x } else { x };
        [self.at[0] + (x - self.lo[0]) * self.scale, self.at[1] + (self.hi[1] - y) * self.scale]
    }

    fn ring(&self, points: &[[f64; 2]]) -> Vec<[f64; 2]> {
        points.iter().map(|p| self.p(*p)).collect()
    }
}

/// The board's copper, worked out once for all the board pages of a book: its connections with the
/// pours filled for the copper pages, else just its pads (all the assembly pages need).
pub(super) struct Copper {
    nets: Option<BoardNets>,
    pads: Vec<BoardPad>,
}

impl Copper {
    pub(super) fn new(ecad: &Electronics, conn: &Connectivity, kinds: &[PageKind]) -> Copper {
        let has = |ks: &[PageKind]| kinds.iter().any(|k| ks.contains(k));
        if has(&[PageKind::BoardTop, PageKind::BoardBottom, PageKind::InnerLayers, PageKind::Cover]) {
            Copper { nets: Some(board_nets(ecad, &ecad.board, conn)), pads: Vec::new() }
        } else if has(&[PageKind::AssemblyTop, PageKind::AssemblyBottom]) {
            Copper { nets: None, pads: board_pads(ecad, &ecad.board, conn) }
        } else {
            Copper { nets: None, pads: Vec::new() }
        }
    }

    fn pads(&self) -> &[BoardPad] {
        self.nets.as_ref().map_or(self.pads.as_slice(), |n| n.pads.as_slice())
    }
}

/// The board's box: its outline's, else its footprints'.
fn board_box(ecad: &Electronics) -> Option<[[f64; 2]; 2]> {
    let b = &ecad.board;
    if let Some(o) = b.outline_bbox() {
        return Some(o);
    }
    let boxes = b.footprints.iter().filter_map(|f| {
        let placed = ecad.placed(f)?;
        footprint_box(placed.resolved.footprint).map(|bb| f.placement().bbox(bb))
    });
    solidcraft_doc::ecad::bbox(boxes.flatten())
}

/// Draw the board page `kind` on `page` (for inner layers, inner layer `inner`); returns its
/// scale.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    ecad: &Electronics,
    copper_of: &Copper,
    kind: PageKind,
    inner: usize,
    page: &mut Page,
    geometry: &Sheet,
    pal: &Palette,
    font: &Font,
    parts_page: Option<usize>,
    warnings: &mut Vec<String>,
) -> Option<f64> {
    if kind == PageKind::Stackup {
        stackup(ecad, page, geometry, pal, font, warnings);
        return None;
    }
    let [lo, hi] = board_box(ecad)?;
    let size = [hi[0] - lo[0], hi[1] - lo[1]];
    if !(size[0] > 0.0 && size[1] > 0.0) {
        return None;
    }
    // Room for the dimensions below and to the right.
    let [a0, a1] = geometry.area;
    let room = [[a0[0] + 4.0, a0[1] + 12.0], [a1[0] - DIM_ROOM, a1[1] - DIM_ROOM]];
    let (scale, placed) = geometry.fit_clear(room, size, 20.0, true, [DIM_ROOM, DIM_ROOM]);
    let bottom = matches!(kind, PageKind::BoardBottom | PageKind::AssemblyBottom);
    let view = View { scale, lo, hi, at: placed[0], mirror: bottom };
    let side = if bottom { Side::Bottom } else { Side::Top };
    let layer = match kind {
        PageKind::InnerLayers => u8::try_from(inner).ok().map_or(CopperLayer::Top, CopperLayer::Inner),
        _ => CopperLayer::from(side),
    };
    let reach = match kind {
        PageKind::BoardTop | PageKind::BoardBottom | PageKind::InnerLayers => copper(ecad, copper_of, page, &view, layer, pal, font, warnings, false),
        _ => {
            assembly(ecad, copper_of, page, &view, side, pal, font, geometry, parts_page);
            None
        }
    };
    dimensions(page, &view, size, reach, pal);
    let inner_legend = format!("Inner layer {inner}: copper, pours, pads and vias, seen from the top · board outline");
    let legend = match kind {
        PageKind::BoardTop => "Top copper, pours, pads and vias · top silkscreen · board outline",
        PageKind::BoardBottom => "Seen from the bottom (mirrored) · bottom copper · bottom silkscreen",
        PageKind::InnerLayers => inner_legend.as_str(),
        PageKind::AssemblyTop => "Parts on the top: outlines and designators",
        _ => "Parts on the bottom, seen from the bottom (mirrored): outlines and designators",
    };
    page.add("legend", [Item::Text(Text::new([a0[0], a0[1] + 1.0], legend, 2.4, pal.dim).align(Align::Start, Baseline::Top))]);
    Some(scale)
}

/// The board's top side (copper with its pours, pads and vias, the silkscreen with the names, and
/// the outline) as large as fits `room`, for the cover: the page box it takes and the board's
/// size, or `None` without a board.
pub(super) fn picture(
    ecad: &Electronics,
    copper_of: &Copper,
    page: &mut Page,
    room: [[f64; 2]; 2],
    pal: &Palette,
    font: &Font,
) -> Option<([[f64; 2]; 2], [f64; 2])> {
    let [lo, hi] = board_box(ecad)?;
    let size = [hi[0] - lo[0], hi[1] - lo[1]];
    let (w, h) = (room[1][0] - room[0][0], room[1][1] - room[0][1]);
    if !(size[0] > 0.0 && size[1] > 0.0 && w > 1.0 && h > 1.0) {
        return None;
    }
    let scale = (w / size[0]).min(h / size[1]).min(20.0);
    let (dw, dh) = (size[0] * scale, size[1] * scale);
    let at = [room[0][0] + (w - dw) / 2.0, room[0][1] + (h - dh) / 2.0];
    let view = View { scale, lo, hi, at, mirror: false };
    // The board top page warns of names left off; the cover need not say it again.
    let mut quiet = Vec::new();
    copper(ecad, copper_of, page, &view, CopperLayer::Top, pal, font, &mut quiet, true);
    Some(([at, [at[0] + dw, at[1] + dh]], size))
}

/// Whether the design has a board to picture.
pub(super) fn has_picture(ecad: &Electronics) -> bool {
    board_box(ecad).is_some_and(|[lo, hi]| hi[0] - lo[0] > 0.0 && hi[1] - lo[1] > 0.0)
}

fn outline_ring(ecad: &Electronics) -> Vec<[f64; 2]> {
    let b = &ecad.board;
    tessellate(&b.outline, &b.outline_curves, true, 0.02)
}

/// Copper `layer` (an outer one with its silkscreen and names). Returns how far right and down
/// the names reach on the page, past the board. For the cover's picture (`picture`) the pours are
/// lighter and the names left off, so the board reads at a glance.
#[allow(clippy::too_many_arguments)]
fn copper(
    ecad: &Electronics,
    copper_of: &Copper,
    page: &mut Page,
    view: &View,
    layer: CopperLayer,
    pal: &Palette,
    font: &Font,
    warnings: &mut Vec<String>,
    picture: bool,
) -> Option<[f64; 2]> {
    let b = &ecad.board;
    let colour = match layer {
        CopperLayer::Top => pal.copper_top,
        CopperLayer::Bottom => pal.copper_bottom,
        // Inner layers between the two, alternately nearer each.
        CopperLayer::Inner(k) => pal.copper_top.mix(pal.copper_bottom, if k % 2 == 1 { 0.35 } else { 0.65 }),
    };
    let pour = colour.mix(pal.background, if picture { 0.84 } else { 0.72 });
    let outline = outline_ring(ecad);
    let mut items = Vec::new();
    if outline.len() >= 3 {
        items.push(Item::Area { rings: vec![view.ring(&outline)], fill: pal.board, stroke: None });
    }
    let Some(nets) = &copper_of.nets else { return None };
    let late = "the board's connections were past their budget; the pour fills drawn may be incomplete";
    if nets.truncated && !warnings.iter().any(|w| w == late) {
        warnings.push(late.into());
    }
    for f in nets.fills.iter().filter(|f| f.layer == layer) {
        for s in f.shapes() {
            let mut rings = vec![view.ring(&s.outer)];
            rings.extend(s.holes.iter().map(|h| view.ring(h)));
            items.push(Item::Area { rings, fill: pour, stroke: None });
        }
    }
    for t in b.tracks.iter().filter(|t| t.layer == layer) {
        let points = tessellate(&t.points, &t.curves, false, 0.01);
        if points.len() >= 2 {
            items.push(Item::Line { points: view.ring(&points), closed: false, stroke: Stroke::solid(t.width * view.scale, colour) });
        }
    }
    let pad_colour = colour.mix(Rgb(0, 0, 0), 0.15);
    for p in nets.pads.iter().filter(|p| p.on(layer)) {
        let ring = p.outline(4);
        if ring.len() >= 3 {
            items.push(Item::Area { rings: vec![view.ring(&ring)], fill: pad_colour, stroke: None });
        }
    }
    for v in b.vias.iter().filter(|v| v.on(layer, b.copper_layers)) {
        items.push(Item::Circle { center: view.p(v.at), radius: v.diameter / 2.0 * view.scale, fill: Some(pad_colour), stroke: None });
    }
    // Drills on top of the copper.
    for p in &nets.pads {
        if let Some(ring) = p.drill_outline(4).filter(|r| r.len() >= 3) {
            items.push(Item::Area { rings: vec![view.ring(&ring)], fill: pal.background, stroke: None });
        }
    }
    for v in b.vias.iter().filter(|v| v.on(layer, b.copper_layers)) {
        items.push(Item::Circle { center: view.p(v.at), radius: (v.drill / 2.0 * view.scale).max(0.08), fill: Some(pal.background), stroke: None });
    }
    for h in &b.holes {
        items.push(Item::Circle {
            center: view.p(h.at),
            radius: h.drill / 2.0 * view.scale,
            fill: Some(pal.background),
            stroke: Some(Stroke::solid(0.15, pal.outline)),
        });
    }
    page.add("copper", items);
    let Some(side) = layer.side() else {
        if outline.len() >= 3 {
            page.add("outline", [Item::Line { points: view.ring(&outline), closed: true, stroke: Stroke::solid(0.35, pal.outline) }]);
        }
        return None;
    };

    // Silkscreen: footprints' and the board's drawings on this side, and the names. The cover's
    // picture leaves out a footprint's silkscreen that is only dots (a pin 1 mark with no outline
    // reads as a speck there).
    let mut silk = Vec::new();
    for f in &b.footprints {
        let Some(placed) = ecad.placed(f) else { continue };
        let placement = f.placement();
        let graphics = &placed.resolved.footprint.graphics;
        let drawn = |g: &&Graphic| g.layer == Some(BoardLayer::Silkscreen) && !is_placeholder(g);
        if picture && graphics.iter().filter(drawn).all(|g| g.kind == GraphicType::Circle) {
            continue;
        }
        for g in graphics.iter().filter(drawn) {
            let on = if placement.mirror { other(g.side) } else { g.side };
            if on == side {
                shape(&mut silk, view, &placement.graphic(g), pal.silk, font, false);
            }
        }
    }
    let layer_draw = if side == Side::Top { DrawLayer::SilkscreenTop } else { DrawLayer::SilkscreenBottom };
    for g in b.graphics.iter().filter(|g| g.layer == layer_draw) {
        shape(&mut silk, view, &g.graphic, pal.silk, font, g.mirror);
    }
    // The names, where the board has them; one that would cover another drawn before it is left
    // to the assembly pages.
    let mut placed: Vec<[[f64; 2]; 2]> = Vec::new();
    let mut hidden = 0usize;
    let names = if picture { Vec::new() } else { footprint_names(ecad) };
    for t in names.iter().filter(|t| t.side == side) {
        // Names too small to read at this scale are left off.
        let size = t.height * view.scale;
        if size < 0.9 {
            continue;
        }
        let size = size.max(MIN_TEXT);
        // The text's box on the board, then upright on the page at its lower left as seen.
        let w = names::text_width(t.height, t.text.chars().count());
        let corner = view.p(if view.mirror { [t.at[0] + w, t.at[1]] } else { t.at });
        let tw = font.width(&t.text, size);
        let b = [[corner[0], corner[1] - size * 0.95], [corner[0] + tw, corner[1]]];
        if placed.iter().any(|o| o[0][0] < b[1][0] && b[0][0] < o[1][0] && o[0][1] < b[1][1] && b[0][1] < o[1][1]) {
            hidden += 1;
            continue;
        }
        placed.push(b);
        silk.push(Item::Text(Text::new(corner, t.text.clone(), size, pal.silk).align(Align::Start, Baseline::Bottom)));
    }
    if hidden > 0 {
        let (names, verb, pronoun) =
            if hidden == 1 { ("1 name".to_string(), "covers", "it is") } else { (format!("{hidden} names"), "cover", "they are") };
        warnings.push(format!(
            "{names} on the {} silkscreen {verb} others, so {pronoun} left off its page (the assembly page shows every part)",
            if side == Side::Top { "top" } else { "bottom" }
        ));
    }
    if outline.len() >= 3 {
        silk.push(Item::Line { points: view.ring(&outline), closed: true, stroke: Stroke::solid(0.35, pal.outline) });
    }
    page.add("silkscreen", silk);
    placed.iter().map(|b| b[1]).reduce(|a, b| [a[0].max(b[0]), a[1].max(b[1])])
}

/// The box round `points`; `None` for none.
fn bounds(points: &[[f64; 2]]) -> Option<[[f64; 2]; 2]> {
    let first = *points.first()?;
    Some(points.iter().fold([first, first], |[lo, hi], p| [[lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]]))
}

fn is_placeholder(g: &Graphic) -> bool {
    g.kind == GraphicType::Text && (g.text == "{designator}" || g.text == "{value}")
}

/// A footprint or board graphic through the view.
fn shape(out: &mut Vec<Item>, view: &View, g: &Graphic, colour: Rgb, _font: &Font, mirror: bool) {
    let width = (g.width * view.scale).max(0.12);
    let stroke = Stroke::solid(width, colour);
    match g.kind {
        GraphicType::Line => {
            if g.points.len() >= 2 {
                out.push(Item::Line { points: view.ring(&g.points), closed: false, stroke });
            }
        }
        GraphicType::Polygon => {
            let points = view.ring(&g.points);
            if g.fill {
                out.push(Item::Area { rings: vec![points], fill: colour, stroke: Some(stroke) });
            } else {
                out.push(Item::Line { points, closed: true, stroke });
            }
        }
        GraphicType::Rect => {
            let (Some(a), Some(b)) = (g.from, g.to) else { return };
            let ring = vec![view.p(a), view.p([b[0], a[1]]), view.p(b), view.p([a[0], b[1]])];
            if g.fill {
                out.push(Item::Area { rings: vec![ring], fill: colour, stroke: Some(stroke) });
            } else {
                out.push(Item::Line { points: ring, closed: true, stroke });
            }
        }
        GraphicType::Circle => {
            let (Some(c), Some(r)) = (g.center, g.radius) else { return };
            out.push(Item::Circle { center: view.p(c), radius: r * view.scale, fill: g.fill.then_some(colour), stroke: Some(stroke) });
        }
        GraphicType::Arc => {
            let points = g.arc_points(24);
            if points.len() >= 2 {
                out.push(Item::Line { points: view.ring(&points), closed: false, stroke });
            }
        }
        GraphicType::Text => {
            let (Some(at), Some(h)) = (g.at, g.height) else { return };
            let size = h * view.scale;
            if size < 0.9 {
                return;
            }
            // Its box on the board (a mirrored text runs to the left of its start), then set
            // readable on the page: its run turned as the view turns it, from the corner of the
            // box that is then its lower left.
            let start = if mirror { [at[0] - h * 0.6 * g.text.chars().count() as f64, at[1]] } else { at };
            let corners: Vec<[f64; 2]> = Graphic { at: Some(start), ..g.clone() }.text_box().iter().map(|p| view.p(*p)).collect();
            let turn = readable(if view.mirror { 180.0 - g.rotation } else { g.rotation });
            let (s, c) = turn.to_radians().sin_cos();
            // The run is [c, -s] on the page (y down) and its up [-s, -c]: the lower left is the
            // corner least along both.
            let key = |p: &[f64; 2]| p[0] * (c - s) - p[1] * (s + c);
            let Some(corner) = corners.into_iter().min_by(|a, b| key(a).total_cmp(&key(b))) else { return };
            out.push(Item::Text(Text::new(corner, g.text.clone(), size.max(MIN_TEXT), colour).align(Align::Start, Baseline::Bottom).turned(turn)));
        }
    }
}

/// A turn in degrees brought to (−90, 90], so text reads left to right or bottom to top.
fn readable(turn: f64) -> f64 {
    let t = turn.rem_euclid(360.0);
    if t > 270.0 {
        t - 360.0
    } else if t > 90.0 {
        t - 180.0
    } else {
        t
    }
}

/// A footprint's name as the board shows it.
struct Name {
    side: Side,
    /// Its lower left on the board, before any mirror.
    at: [f64; 2],
    height: f64,
    text: String,
}

/// The footprints' names (designators) where the board shows them: their own spot, else the
/// footprint's placeholder, else outside the courtyard on the side with the most room.
fn footprint_names(ecad: &Electronics) -> Vec<Name> {
    let b = &ecad.board;
    let mut out: Vec<(Name, Option<usize>)> = Vec::new();
    let mut boxes = Vec::new();
    let mut sizes = Vec::new();
    for f in b.footprints.iter().filter(|f| f.show_name) {
        let Some(placed) = ecad.placed(f) else { continue };
        let placement = f.placement();
        let side = if placement.mirror { Side::Bottom } else { Side::Top };
        let fp = placed.resolved.footprint;
        let court = shapes_bbox(fp.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Courtyard)))
            .or_else(|| footprint_box(fp))
            .map(|c| placement.bbox(c));
        let text = placed.part.designator.clone();
        let placeholder =
            fp.graphics.iter().find(|g| g.layer == Some(BoardLayer::Silkscreen) && g.kind == GraphicType::Text && g.text == "{designator}");
        let height = b.name_height.or_else(|| placeholder.and_then(|g| g.height)).unwrap_or_else(|| court.map_or(1.0, names::auto_height));
        let spot = match (f.name_at, placeholder.and_then(|g| placement.graphic(g).at)) {
            (Some(o), _) => Some([f.at[0] + o[0], f.at[1] + o[1]]),
            (None, at) => at,
        };
        let k = match (spot, court) {
            (None, Some(c)) => {
                boxes.push((c, side));
                sizes.push([names::text_width(height, text.chars().count()), height]);
                Some(boxes.len() - 1)
            }
            _ => None,
        };
        out.push((Name { side, at: spot.unwrap_or(f.at), height, text }, k));
    }
    if !boxes.is_empty() {
        let across = vec![[0.0, 0.0]; boxes.len()];
        let around = names::sides(&boxes, &sizes, &across);
        for (name, k) in &mut out {
            if let Some(k) = k
                && let (Some((court, _)), Some(a), Some(s)) = (boxes.get(*k), around.get(*k), sizes.get(*k))
            {
                name.at = names::spot(*court, *a, *s);
            }
        }
    }
    out.into_iter().map(|(n, _)| n).collect()
}

/// Whether a pin name says it is a diode's cathode.
fn is_cathode(name: &str) -> bool {
    ["k", "cathode", "kathode"].iter().any(|n| name.trim().eq_ignore_ascii_case(n))
}

#[allow(clippy::too_many_arguments)]
fn assembly(
    ecad: &Electronics,
    copper_of: &Copper,
    page: &mut Page,
    view: &View,
    side: Side,
    pal: &Palette,
    font: &Font,
    geometry: &Sheet,
    parts_page: Option<usize>,
) {
    let b = &ecad.board;
    let outline = outline_ring(ecad);
    let mut items = Vec::new();
    if outline.len() >= 3 {
        items.push(Item::Area { rings: vec![view.ring(&outline)], fill: pal.background, stroke: Some(Stroke::solid(0.35, pal.outline)) });
    }
    for h in &b.holes {
        items.push(Item::Circle { center: view.p(h.at), radius: h.drill / 2.0 * view.scale, fill: None, stroke: Some(Stroke::solid(0.2, pal.zone)) });
    }
    // Which way a part goes in: a diode's cathode pad, else pad 1 (A1 on a grid of balls) of a part
    // with more pads than two, darker than the rest.
    let mut marked: BTreeSet<(u64, &str)> = BTreeSet::new();
    for f in &b.footprints {
        let Some(placed) = ecad.placed(f) else { continue };
        let r = &placed.resolved;
        let cathode = |pin: &str| r.symbol.pins.iter().any(|p| p.number == pin && is_cathode(&p.name));
        let cathodes: Vec<&str> = r.connections().into_iter().filter(|(pin, _)| cathode(pin)).flat_map(|(_, pads)| pads).collect();
        if !cathodes.is_empty() {
            marked.extend(cathodes.into_iter().map(|n| (f.id, n)));
        } else if r.footprint.pads.iter().filter(|p| p.kind != solidcraft_doc::ecad::PadKind::Hole).count() > 2
            && let Some(one) = ["1", "A1"].iter().find_map(|n| r.footprint.pads.iter().find(|p| p.number == *n))
        {
            marked.insert((f.id, one.number.as_str()));
        }
    }
    let pad = pal.zone.mix(pal.background, 0.6);
    let mark = pal.ink.mix(pal.background, 0.45);
    // What a "DNP" tag must keep clear of: each part's pads, outline and name, by footprint.
    let mut taken: Vec<(u64, [[f64; 2]; 2])> = Vec::new();
    for p in copper_of.pads().iter().filter(|p| p.on_side(side)) {
        let ring = p.outline(3);
        if ring.len() >= 3 {
            let fill = if marked.contains(&(p.footprint, p.pad.number.as_str())) { mark } else { pad };
            let ring = view.ring(&ring);
            if let Some(bb) = bounds(&ring) {
                taken.push((p.footprint, bb));
            }
            items.push(Item::Area { rings: vec![ring], fill, stroke: None });
        }
    }
    let mut names = Vec::new();
    // The parts not fitted: footprint, outline, middle, name width and size, whether upright.
    let mut unfitted: Vec<(u64, [[f64; 2]; 2], [f64; 2], f64, f64, bool)> = Vec::new();
    let (mut count, mut dnp) = (0usize, 0usize);
    for f in &b.footprints {
        let Some(placed) = ecad.placed(f) else { continue };
        let placement: Placement = f.placement();
        let on = if placement.mirror { Side::Bottom } else { Side::Top };
        if on != side {
            continue;
        }
        count += 1;
        if placed.part.dnp {
            dnp += 1;
        }
        let fp = placed.resolved.footprint;
        let fab: Vec<&Graphic> = fp.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Fab) && g.kind != GraphicType::Text).collect();
        let ink = if placed.part.dnp { pal.dim } else { pal.ink };
        if fab.is_empty() {
            if let Some(bb) = shapes_bbox(fp.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Courtyard))).or_else(|| footprint_box(fp)) {
                let [a, c] = placement.bbox(bb);
                let ring = vec![view.p(a), view.p([c[0], a[1]]), view.p(c), view.p([a[0], c[1]])];
                items.push(Item::Line { points: ring, closed: true, stroke: Stroke::solid(0.2, ink) });
            }
        } else {
            for g in fab {
                shape(&mut items, view, &placement.graphic(g), ink, font, false);
            }
        }
        // The designator in the middle of the part, along its longer side.
        let Some(bb) = shapes_bbox(fp.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Fab))).or_else(|| footprint_box(fp)) else { continue };
        let [a, c] = placement.bbox(bb);
        let (pa, pc) = (view.p(a), view.p(c));
        // A part not fitted is struck through (so it shows in black and white too) and tagged.
        if placed.part.dnp {
            items.extend(super::parts::cross([pa, pc], Stroke::solid(0.25, pal.warn.mix(pal.background, 0.3))));
        }
        let (w, h) = ((pc[0] - pa[0]).abs(), (pc[1] - pa[1]).abs());
        let mid = [(pa[0] + pc[0]) / 2.0, (pa[1] + pc[1]) / 2.0];
        let upright = h > w * 1.2;
        let (long, short) = if upright { (h, w) } else { (w, h) };
        let text = placed.part.designator.clone();
        let em = font.width(&text, 1.0).max(0.1);
        let size = (long * 0.82 / em).min(short * 0.72).clamp(MIN_TEXT, 3.2);
        let tw = font.width(&text, size);
        let t = Text::new(mid, text, size, pal.ink).align(Align::Middle, Baseline::Middle);
        names.push(Item::Text(if upright { t.turned(90.0) } else { t }));
        let outline = [[pa[0].min(pc[0]), pa[1].min(pc[1])], [pa[0].max(pc[0]), pa[1].max(pc[1])]];
        let (half_w, half_h) = if upright { (size / 2.0, tw / 2.0) } else { (tw / 2.0, size / 2.0) };
        taken.push((f.id, outline));
        taken.push((f.id, [[mid[0] - half_w, mid[1] - half_h], [mid[0] + half_w, mid[1] + half_h]]));
        if placed.part.dnp {
            unfitted.push((f.id, outline, mid, tw, size, upright));
        }
    }
    // Each tag beside its part, outside the name so it stays readable on a small part: right of it
    // (above an upright name), else on another side, the first spot with no other part nearer than
    // twice its gap to its own (so it reads as that part's), else none fitted that near (beside
    // another part not fitted it misleads no one), else the first just clear of every other part
    // and tag; right (above) when none is.
    let not_fitted: BTreeSet<u64> = unfitted.iter().map(|u| u.0).collect();
    let mut tags = Vec::new();
    let mut tagged: Vec<[[f64; 2]; 2]> = Vec::new();
    for (id, outline, mid, tw, size, upright) in unfitted {
        // Past the part's own pads too.
        let [lo, hi] = taken
            .iter()
            .filter(|(o, _)| *o == id)
            .fold(outline, |a, (_, b)| [[a[0][0].min(b[0][0]), a[0][1].min(b[0][1])], [a[1][0].max(b[1][0]), a[1][1].max(b[1][1])]]);
        let size = size.max(MIN_TEXT);
        let (_, tag_w) = super::parts::dnp_tag([0.0, 0.0], size, pal, font);
        let tag_h = super::parts::dnp_tag_height(size);
        let right = [hi[0].max(mid[0] + tw / 2.0) + TAG_GAP, mid[1]];
        let left = [lo[0].min(mid[0] - tw / 2.0) - TAG_GAP - tag_w, mid[1]];
        let above = [mid[0] - tag_w / 2.0, lo[1] - TAG_GAP - tag_h / 2.0];
        let below = [mid[0] - tag_w / 2.0, hi[1] + TAG_GAP + tag_h / 2.0];
        let spots = if upright { [above, below, right, left] } else { [right, left, above, below] };
        let rect = |at: [f64; 2]| [[at[0] - 0.3, at[1] - tag_h / 2.0 - 0.3], [at[0] + tag_w + 0.3, at[1] + tag_h / 2.0 + 0.3]];
        // Clear of every other part and tag, and of the fitted ones (all of them, unless
        // `fitted_only`) by `keep` besides.
        let clear = |at: &[f64; 2], keep: f64, fitted_only: bool| {
            let r = rect(*at);
            let wide = [[r[0][0] - keep, r[0][1] - keep], [r[1][0] + keep, r[1][1] + keep]];
            let near = |other: u64, o: [[f64; 2]; 2]| overlaps(r, o) || (overlaps(wide, o) && !(fitted_only && not_fitted.contains(&other)));
            !taken.iter().any(|(other, o)| *other != id && near(*other, *o)) && !tagged.iter().any(|o| overlaps(r, *o))
        };
        let at = [(TAG_GAP * 2.0, false), (TAG_GAP * 2.0, true), (0.0, false)]
            .iter()
            .find_map(|(keep, fitted_only)| spots.iter().find(|a| clear(a, *keep, *fitted_only)))
            .copied()
            .unwrap_or(if upright { above } else { right });
        tagged.push(rect(at));
        tags.extend(super::parts::dnp_tag(at, size, pal, font).0);
    }
    page.add("assembly", items);
    page.add("designators", names);
    page.add("dnp", tags);
    let [a0, a1] = geometry.area;
    if dnp > 0 {
        page.add("key", super::parts::dnp_key([a1[0], a0[1] + 1.0], pal, font));
    }
    let dnp = if dnp == 0 { String::new() } else { format!(" ({dnp} not fitted, marked DNP)") };
    let parts = if count == 1 { "1 part".to_string() } else { format!("{count} parts") };
    let marks = if marked.is_empty() { "" } else { " · pad 1 and cathodes darker" };
    let note = match parts_page {
        Some(n) => format!("{parts} on this side{dnp}{marks} · values and part numbers: parts list, page {}", n + 1),
        None => format!("{parts} on this side{dnp}{marks}"),
    };
    let at = [a0[0], a0[1] + 5.2];
    let width = font.width(&note, 2.4);
    page.add("legend", [Item::Text(Text::new(at, note, 2.4, pal.link).align(Align::Start, Baseline::Top))]);
    if let Some(n) = parts_page {
        page.links.push(Link { rect: [[at[0], at[1] - 0.5], [at[0] + width, at[1] + 3.2]], page: n, at: None });
    }
}

/// The board's width under it and its height to its right, with arrows and the size in mm, past
/// what is drawn beside the board (`reach`: the right and bottom of it on the page).
fn dimensions(page: &mut Page, view: &View, size: [f64; 2], reach: Option<[f64; 2]>, pal: &Palette) {
    let (p0, p1) = (view.p(view.lo), view.p(view.hi));
    let (x0, x1) = (p0[0].min(p1[0]), p0[0].max(p1[0]));
    let (y0, y1) = (p1[1].min(p0[1]), p1[1].max(p0[1]));
    // Out past the names that stick out of the board, as far as the room the fit kept allows.
    let [rx, ry] = reach.unwrap_or([x1, y1]);
    let out_x = (rx - x1 + 3.0).clamp(DIM_OUT, DIM_ROOM - 5.0);
    let out_y = (ry - y1 + 3.0).clamp(DIM_OUT, DIM_ROOM - 5.0);
    let s = Stroke::solid(0.18, pal.dim);
    let mut items = Vec::new();
    let y = y1 + out_y;
    items.push(Item::Line { points: vec![[x0, y1 + 1.5], [x0, y + 1.5]], closed: false, stroke: s });
    items.push(Item::Line { points: vec![[x1, y1 + 1.5], [x1, y + 1.5]], closed: false, stroke: s });
    items.push(Item::Line { points: vec![[x0, y], [x1, y]], closed: false, stroke: s });
    items.push(head([x0, y], [1.0, 0.0], pal.dim));
    items.push(head([x1, y], [-1.0, 0.0], pal.dim));
    items.push(Item::Text(Text::new([(x0 + x1) / 2.0, y + 1.2], format!("{:.2} mm", size[0]), 2.5, pal.ink).align(Align::Middle, Baseline::Top)));
    let x = x1 + out_x;
    items.push(Item::Line { points: vec![[x1 + 1.5, y0], [x + 1.5, y0]], closed: false, stroke: s });
    items.push(Item::Line { points: vec![[x1 + 1.5, y1], [x + 1.5, y1]], closed: false, stroke: s });
    items.push(Item::Line { points: vec![[x, y0], [x, y1]], closed: false, stroke: s });
    items.push(head([x, y0], [0.0, 1.0], pal.dim));
    items.push(head([x, y1], [0.0, -1.0], pal.dim));
    items.push(Item::Text(
        Text::new([x + 1.2, (y0 + y1) / 2.0], format!("{:.2} mm", size[1]), 2.5, pal.ink).align(Align::Middle, Baseline::Top).turned(90.0),
    ));
    page.add("dimensions", items);
}

/// A dimension arrowhead at `at` pointing along `u` into the line.
fn head(at: [f64; 2], u: [f64; 2], colour: Rgb) -> Item {
    let (l, w) = (1.8, 0.5);
    let base = [at[0] + u[0] * l, at[1] + u[1] * l];
    let n = [-u[1], u[0]];
    Item::Area {
        rings: vec![vec![at, [base[0] + n[0] * w, base[1] + n[1] * w], [base[0] - n[0] * w, base[1] - n[1] * w]]],
        fill: colour,
        stroke: None,
    }
}

/// The stack-up: a table of its layers, the net classes' impedances, and a cross-section. Rows
/// get shorter (to the least text size) so many layers and classes still fit the page; classes
/// past that are left out, with a warning.
fn stackup(ecad: &Electronics, page: &mut Page, geometry: &Sheet, pal: &Palette, font: &Font, warnings: &mut Vec<String>) {
    let b = &ecad.board;
    let layers = b.stackup_or_default();
    let n = b.copper_layers;
    let [a0, a1] = geometry.area;
    let mut items = vec![Item::Text(Text::new([a0[0], a0[1] + 1.0], "Layer stack-up", 3.6, pal.ink).align(Align::Start, Baseline::Top).bold())];
    let total: f64 = solidcraft_doc::ecad::stackup::total(&layers);
    items.push(Item::Text(
        Text::new([a0[0], a0[1] + 7.0], format!("{n} copper layers · {:.3} mm finished ({:.2} mm board)", total, b.thickness), 2.4, pal.dim)
            .align(Align::Start, Baseline::Top),
    ));
    // The table, on the left.
    let cols = [("Layer", 34.0), ("Kind", 24.0), ("Thickness", 24.0), ("Copper", 20.0), ("εr", 14.0), ("tan δ", 16.0)];
    let x_start = a0[0];
    let mut y = a0[1] + 14.0;
    let table_w: f64 = cols.iter().map(|c| c.1).sum();
    // Down to the inner frame where the tables stay left of the title block, else to its top.
    let bottom = if x_start + 140.0 < geometry.title_block[0][0] { a1[1] } else { geometry.title_block[0][1] - 4.0 };
    let classes = &b.rules.net_classes;
    // Room: a header and a row per layer, and for the classes a heading, a header, a row each and
    // the note under them.
    let fixed = if classes.is_empty() { 0.0 } else { 8.0 + 6.0 + 6.0 };
    let units = (1 + layers.len() + if classes.is_empty() { 0 } else { 1 + classes.len() }) as f64;
    let row = ((bottom - y - fixed) / units).clamp(3.2, 5.6);
    let size = (row * 0.41).clamp(MIN_TEXT, 2.3);
    items.push(Item::Rect {
        rect: [[x_start, y], [x_start + table_w, y + row]],
        radius: 0.0,
        fill: Some(pal.zone.mix(pal.background, 0.8)),
        stroke: None,
    });
    let mut x = x_start;
    for (name, w) in cols {
        items.push(Item::Text(Text::new([x + 1.6, y + row / 2.0], name, size, pal.ink).align(Align::Start, Baseline::Middle).bold()));
        x += w;
    }
    y += row;
    let mut copper = 0usize;
    let mut cores = 0usize;
    let mut bands: Vec<(StackKind, f64, String)> = Vec::new();
    for (i, l) in layers.iter().enumerate() {
        let (name, kind) = match l.kind {
            StackKind::Copper => {
                let layer = CopperLayer::at(copper, n);
                copper += 1;
                let name = match layer {
                    Some(CopperLayer::Top) => "Top".to_string(),
                    Some(CopperLayer::Bottom) => "Bottom".to_string(),
                    Some(CopperLayer::Inner(k)) => format!("Inner {k}"),
                    None => "Copper".into(),
                };
                (name, "Copper")
            }
            StackKind::Core => {
                cores += 1;
                (format!("Core {cores}"), "Core")
            }
            StackKind::Prepreg => ("Prepreg".to_string(), "Prepreg"),
            StackKind::Mask => ("Solder mask".to_string(), "Mask"),
        };
        if i % 2 == 1 {
            items.push(Item::Rect { rect: [[x_start, y], [x_start + table_w, y + row]], radius: 0.0, fill: Some(pal.body), stroke: None });
        }
        let oz = if l.kind == StackKind::Copper { format!("{:.1} oz", l.thickness / 0.0347) } else { String::new() };
        let er = if l.kind == StackKind::Copper { String::new() } else { format!("{:.2}", l.er()) };
        let tan = l.loss_tangent.map_or(String::new(), |t| format!("{t:.3}"));
        let cells = [name.clone(), kind.to_string(), format!("{:.3} mm", l.thickness), oz, er, tan];
        let mut x = x_start;
        for ((_, w), text) in cols.iter().zip(cells) {
            items.push(Item::Text(
                Text::new([x + 1.6, y + row / 2.0], font.fit(&text, size, w - 3.0), size, pal.ink).align(Align::Start, Baseline::Middle),
            ));
            x += w;
        }
        y += row;
        bands.push((l.kind, l.thickness, name));
    }
    items.push(Item::Line { points: vec![[x_start, y], [x_start + table_w, y]], closed: false, stroke: Stroke::solid(0.25, pal.zone) });

    // Net classes and the impedance their width gives on the top layer.
    if !classes.is_empty() {
        y += 8.0;
        items.push(Item::Text(Text::new([x_start, y], "Net classes on the top layer", 2.8, pal.ink).align(Align::Start, Baseline::Top).bold()));
        y += 6.0;
        let ccols = [("Class", 40.0), ("Width", 22.0), ("Gap", 20.0), ("Clearance", 24.0), ("Impedance", 26.0)];
        let cw: f64 = ccols.iter().map(|c| c.1).sum();
        items.push(Item::Rect {
            rect: [[x_start, y], [x_start + cw, y + row]],
            radius: 0.0,
            fill: Some(pal.zone.mix(pal.background, 0.8)),
            stroke: None,
        });
        let mut x = x_start;
        for (name, w) in ccols {
            items.push(Item::Text(Text::new([x + 1.6, y + row / 2.0], name, size, pal.ink).align(Align::Start, Baseline::Middle).bold()));
            x += w;
        }
        y += row;
        let room = ((bottom - 6.0 - y) / row).floor().max(0.0) as usize;
        let shown = classes.len().min(room);
        if shown < classes.len() {
            warnings.push(format!(
                "the stack-up page lists {shown} of the board's {} net classes: the rest do not fit (see ecad.board.rules.inspect)",
                classes.len()
            ));
        }
        let m = medium(&layers, CopperLayer::Top, n).ok();
        for c in classes.iter().take(shown) {
            let (width, gap, z) = match c.diff_pair {
                Some(d) => (
                    d.width,
                    Some(d.gap),
                    m.and_then(|m| impedance(&m, LineKind::Differential, d.width, Some(d.gap)).ok()).map(|z| format!("{:.0} Ω diff.", z.z0)),
                ),
                None => (
                    c.track_width,
                    None,
                    m.and_then(|m| impedance(&m, LineKind::Microstrip, c.track_width, None).ok()).map(|z| format!("{:.0} Ω", z.z0)),
                ),
            };
            let cells = [
                c.name.clone(),
                format!("{width:.3} mm"),
                gap.map_or("–".into(), |g| format!("{g:.3} mm")),
                format!("{:.3} mm", c.clearance),
                z.unwrap_or_else(|| "–".into()),
            ];
            let mut x = x_start;
            for ((_, w), text) in ccols.iter().zip(cells) {
                items.push(Item::Text(
                    Text::new([x + 1.6, y + row / 2.0], font.fit(&text, size, w - 3.0), size, pal.ink).align(Align::Start, Baseline::Middle),
                ));
                x += w;
            }
            y += row;
        }
        items.push(Item::Text(
            Text::new([x_start, y + 2.0], "Impedance: IPC-2141 microstrip (an edge-coupled pair for a class with one) over the nearest plane. A guide: build to the fab's own figures.", 2.0, pal.dim)
                .align(Align::Start, Baseline::Top),
        ));
    }

    // The cross-section, on the right: bands not to scale (copper drawn thicker to show).
    let sx0 = a0[0] + table_w + 20.0;
    let sx1 = (a1[0] - 45.0).min(sx0 + 150.0).max(sx0 + 40.0);
    let top = a0[1] + 16.0;
    let bottom = geometry.title_block[0][1] - 8.0;
    let room = bottom - top;
    let weight = |k: StackKind, t: f64| match k {
        StackKind::Copper => 1.0,
        StackKind::Mask => 0.8,
        _ => 1.4 + 6.0 * (t / total.max(0.01)),
    };
    let sum: f64 = bands.iter().map(|(k, t, _)| weight(*k, *t)).sum::<f64>().max(1.0);
    let unit = (room / sum).min(9.0);
    let label = (unit * 0.9).clamp(MIN_TEXT, 2.1);
    let mut y = top;
    let core = pal.core.mix(pal.background, 0.25);
    let prepreg = pal.prepreg.mix(pal.background, 0.3);
    let mask = pal.mask.mix(pal.background, 0.35);
    for (k, t, name) in &bands {
        let h = weight(*k, *t) * unit;
        let fill = match k {
            StackKind::Copper => pal.foil,
            StackKind::Core => core,
            StackKind::Prepreg => prepreg,
            StackKind::Mask => mask,
        };
        items.push(Item::Rect { rect: [[sx0, y], [sx1, y + h]], radius: 0.0, fill: Some(fill), stroke: Some(Stroke::solid(0.12, pal.zone)) });
        let text = format!("{name}  {:.3} mm", t);
        items.push(Item::Text(Text::new([sx1 + 2.0, y + h / 2.0], text, label, pal.ink).align(Align::Start, Baseline::Middle)));
        y += h;
    }
    items.push(Item::Text(Text::new([sx0, top - 2.0], "Cross-section (not to scale)", 2.2, pal.dim).align(Align::Start, Baseline::Baseline)));
    page.add("stackup", items);
    let _ = (nice_scale, scale_text);
}
