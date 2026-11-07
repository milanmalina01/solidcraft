//! Gerber X2 (RS-274X with file attributes) for each copper, solder mask, paste, legend layer and
//! the board's profile. Coordinates are millimetres in 4.6 format, y up as the board is seen from
//! the top (the bottom layers too, unmirrored, as fabs read them).
//!
//! - Copper: each pour's fill first, its largest shapes first, each shape dark with its holes
//!   cleared (`%LPC*%`); copper text the same; then the tracks (stroked with a round aperture of
//!   their width), the pads (each a region of its outline), the vias (flashed rings) and the
//!   copper drawings, all dark. Clearing a hole never takes away copper drawn after it.
//! - Solder mask (negative: what is drawn is opened): the pads grown by the mask margin, and the
//!   open vias.
//! - Paste: the surface-mount pads.
//! - Legend: the footprints' silkscreen, the board's silkscreen drawings and the names shown,
//!   text as filled outlines of the built-in font.
//! - Profile: the board's edge stroked 0.1 mm.

use std::fmt::Write as _;

use solidcraft_doc::ecad::Electronics;
use solidcraft_doc::ecad::airwires::{BoardNets, BoardPad};
use solidcraft_doc::ecad::board::other;
use solidcraft_doc::ecad::copper::{hull_outline, pad_core, paste_core};
use solidcraft_doc::ecad::footprint::{PadKind, Side};
use solidcraft_doc::ecad::graphics::{BoardLayer, Graphic, GraphicType};
use solidcraft_doc::ecad::layers::{CopperLayer, DrawLayer};
use solidcraft_doc::ecad::path;
use solidcraft_geom::region::Region;

use super::text::{Glyphs, Mirror};
use super::{FabFile, Options};

/// The width the profile is stroked with, in millimetres.
pub const PROFILE_WIDTH: f64 = 0.1;
/// Arcs and circles are cut into straight pieces within this, in millimetres.
const TOLERANCE: f64 = 0.002;

/// A Gerber file being written: its aperture table and body.
pub(super) struct Writer {
    head: String,
    body: String,
    apertures: Vec<(String, u32)>,
    dark: bool,
}

/// A coordinate in the file's 4.6 format.
fn c(v: f64) -> i64 {
    (v * 1e6).round() as i64
}

impl Writer {
    pub(super) fn new(function: &str, polarity: &str) -> Self {
        let mut head = String::new();
        let _ = writeln!(head, "%TF.GenerationSoftware,SolidCraft,SolidCraft,{}*%", env!("CARGO_PKG_VERSION"));
        let _ = writeln!(head, "%TF.FileFunction,{function}*%");
        let _ = writeln!(head, "%TF.FilePolarity,{polarity}*%");
        head.push_str("%FSLAX46Y46*%\n%MOMM*%\n%LPD*%\nG01*\n");
        Writer { head, body: String::new(), apertures: Vec::new(), dark: true }
    }

    /// The D code of the aperture `def` (`C,0.250000`), defined the first time it is asked for.
    fn aperture(&mut self, def: String) -> u32 {
        if let Some((_, d)) = self.apertures.iter().find(|(x, _)| *x == def) {
            return *d;
        }
        let d = 10 + self.apertures.len() as u32;
        self.apertures.push((def, d));
        d
    }

    fn polarity(&mut self, dark: bool) {
        if dark != self.dark {
            self.body.push_str(if dark { "%LPD*%\n" } else { "%LPC*%\n" });
            self.dark = dark;
        }
    }

    /// A closed polygon filled (a region), dark or clear.
    pub(super) fn region(&mut self, points: &[[f64; 2]], dark: bool) {
        let finite: Vec<[f64; 2]> = points.iter().copied().filter(|p| p[0].is_finite() && p[1].is_finite()).collect();
        if finite.len() < 3 {
            return;
        }
        self.polarity(dark);
        self.body.push_str("G36*\n");
        for (i, p) in finite.iter().chain(finite.first()).enumerate() {
            let _ = writeln!(self.body, "X{}Y{}D0{}*", c(p[0]), c(p[1]), if i == 0 { 2 } else { 1 });
        }
        self.body.push_str("G37*\n");
    }

    /// A region's shapes: each outline dark, its holes cleared, largest first.
    pub(super) fn shapes(&mut self, r: &Region) {
        let mut shapes = r.shapes();
        shapes.sort_by(|a, b| b.area().total_cmp(&a.area()));
        for s in &shapes {
            self.region(&s.outer, true);
            for h in &s.holes {
                self.region(h, false);
            }
        }
    }

    /// A line stroked with a round aperture `width` wide (a dot for a single point).
    pub(super) fn stroke(&mut self, line: &[[f64; 2]], width: f64) {
        if !(width.is_finite() && width > 0.0) || line.is_empty() {
            return;
        }
        self.polarity(true);
        let d = self.aperture(format!("C,{width:.6}"));
        let _ = writeln!(self.body, "D{d}*");
        if line.len() == 1 {
            if let Some(p) = line.first() {
                let _ = writeln!(self.body, "X{}Y{}D03*", c(p[0]), c(p[1]));
            }
            return;
        }
        for (i, p) in line.iter().enumerate() {
            let _ = writeln!(self.body, "X{}Y{}D0{}*", c(p[0]), c(p[1]), if i == 0 { 2 } else { 1 });
        }
    }

    /// A round flash `diameter` across.
    pub(super) fn dot(&mut self, at: [f64; 2], diameter: f64) {
        if !(diameter.is_finite() && diameter > 0.0) {
            return;
        }
        self.polarity(true);
        let d = self.aperture(format!("C,{diameter:.6}"));
        let _ = writeln!(self.body, "D{d}*\nX{}Y{}D03*", c(at[0]), c(at[1]));
    }

    pub(super) fn finish(self) -> String {
        let mut out = self.head;
        for (def, d) in &self.apertures {
            let _ = writeln!(out, "%ADD{d}{def}*%");
        }
        out.push_str(&self.body);
        out.push_str("M02*\n");
        out
    }
}

/// The X2 file function of a copper layer, `L1` the top.
fn copper_function(layer: CopperLayer, n: u8) -> String {
    let i = layer.index(n) + 1;
    match layer {
        CopperLayer::Top => format!("Copper,L{i},Top"),
        CopperLayer::Bottom => format!("Copper,L{i},Bot"),
        CopperLayer::Inner(_) => format!("Copper,L{i},Inr"),
    }
}

/// The usual file name ending of a layer, as most fabs' upload pages recognise it.
fn extension(layer: CopperLayer, n: u8) -> String {
    match layer {
        CopperLayer::Top => "GTL".into(),
        CopperLayer::Bottom => "GBL".into(),
        CopperLayer::Inner(_) => format!("G{}", layer.index(n)),
    }
}

/// A pad's outline on the board, grown by `grow`.
fn pad_outline(p: &BoardPad, grow: f64) -> Vec<[f64; 2]> {
    hull_outline(&pad_core(p), grow)
}

/// A drawing's strokes and filled shapes, or its text as outlines.
fn graphic(w: &mut Writer, g: &Graphic, mirror: Mirror, glyphs: &Glyphs) {
    match g.kind {
        GraphicType::Text => {
            for shape in glyphs.text(g, mirror).shapes() {
                w.region(&shape.outer, true);
                for h in &shape.holes {
                    w.region(h, false);
                }
            }
        }
        _ => {
            let place = |q: [f64; 2]| q;
            if g.fill && matches!(g.kind, GraphicType::Rect | GraphicType::Polygon | GraphicType::Circle) {
                let outline: Vec<[f64; 2]> = match (g.kind, g.from, g.to) {
                    (GraphicType::Rect, Some([x0, y0]), Some([x1, y1])) => vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
                    _ => solidcraft_doc::ecad::copper::graphic_cores(g, &place).iter().flat_map(|core| core.points.clone()).collect(),
                };
                w.region(&outline, true);
            }
            for core in solidcraft_doc::ecad::copper::graphic_cores(g, &place) {
                w.stroke(&core.points, g.width);
            }
        }
    }
}

/// Every Gerber file of the board.
pub fn files(ecad: &Electronics, nets: &BoardNets, o: &Options, glyphs: &Glyphs) -> Vec<FabFile> {
    let b = &ecad.board;
    let n = b.copper_layers;
    let mut out = Vec::new();
    for layer in b.stack() {
        let mut w = Writer::new(&copper_function(layer, n), "Positive");
        // Fills first: clearing their holes must not take copper drawn after.
        let mut fills: Vec<&Region> = nets.fills.iter().filter(|f| f.layer == layer).map(|f| &f.region).collect();
        fills.sort_by(|a, b| b.area().total_cmp(&a.area()));
        for r in fills {
            w.shapes(r);
        }
        for g in b.graphics.iter().filter(|g| g.layer == DrawLayer::Copper(layer) && g.graphic.kind == GraphicType::Text) {
            graphic(&mut w, &g.graphic, if g.mirror { Mirror::AboutStart } else { Mirror::No }, glyphs);
        }
        for t in b.tracks.iter().filter(|t| t.layer == layer) {
            let line = path::tessellate(&t.points, &t.curves, false, TOLERANCE);
            w.stroke(&line, t.width);
        }
        for p in nets.pads.iter().filter(|p| p.on(layer)) {
            w.region(&pad_outline(p, 0.0), true);
        }
        for v in b.vias.iter().filter(|v| v.on(layer, n)) {
            w.dot(v.at, v.diameter);
        }
        for g in b.graphics.iter().filter(|g| g.layer == DrawLayer::Copper(layer) && g.graphic.kind != GraphicType::Text) {
            graphic(&mut w, &g.graphic, if g.mirror { Mirror::AboutStart } else { Mirror::No }, glyphs);
        }
        out.push(FabFile::text(format!("{}.{}", o.stem, extension(layer, n)), copper_function(layer, n), w.finish()));
    }
    for side in [Side::Top, Side::Bottom] {
        let (tb, letter) = if side == Side::Top { ("Top", 'T') } else { ("Bot", 'B') };
        let copper = if side == Side::Top { CopperLayer::Top } else { CopperLayer::Bottom };
        // Solder mask openings.
        let mut w = Writer::new(&format!("Soldermask,{tb}"), "Negative");
        for p in nets.pads.iter().filter(|p| p.on(copper) || (p.pad.kind == PadKind::Hole)) {
            w.region(&pad_outline(p, o.mask_margin), true);
        }
        for h in &b.holes {
            w.dot(h.at, h.drill + 2.0 * o.mask_margin);
        }
        for v in b.vias.iter().filter(|v| v.on(copper, n) && v.protection == solidcraft_doc::ecad::board::ViaProtection::Open) {
            w.dot(v.at, v.diameter + 2.0 * o.mask_margin);
        }
        out.push(FabFile::text(format!("{}.G{letter}S", o.stem), format!("Soldermask,{tb}"), w.finish()));
        // Paste.
        let mut w = Writer::new(&format!("Paste,{tb}"), "Positive");
        // A part that isn't fitted gets no paste: nothing would sit on it, and paste across close
        // pads (a solder bridge left open) would join them.
        let fitted = |p: &BoardPad| !b.footprints.iter().any(|f| f.id == p.footprint && ecad.placed(f).is_some_and(|q| q.part.dnp));
        for p in nets.pads.iter().filter(|p| p.pad.kind == PadKind::Smd && p.on(copper) && fitted(p)) {
            w.region(&hull_outline(&paste_core(p), 0.0), true);
        }
        out.push(FabFile::text(format!("{}.G{letter}P", o.stem), format!("Paste,{tb}"), w.finish()));
        // Legend.
        let mut w = Writer::new(&format!("Legend,{tb}"), "Positive");
        legend(&mut w, ecad, side, glyphs);
        out.push(FabFile::text(format!("{}.G{letter}O", o.stem), format!("Legend,{tb}"), w.finish()));
    }
    let mut w = Writer::new("Profile,NP", "Positive");
    if !b.outline.is_empty() {
        let mut ring = path::tessellate(&b.outline, &b.outline_curves, true, TOLERANCE);
        if let Some(first) = ring.first().copied() {
            ring.push(first);
        }
        w.stroke(&ring, PROFILE_WIDTH);
    }
    out.push(FabFile::text(format!("{}.GKO", o.stem), "Profile,NP".into(), w.finish()));
    out
}

/// A text graphic with its lower left at `at`.
fn text_graphic(at: [f64; 2], text: String, height: f64) -> Graphic {
    Graphic {
        kind: GraphicType::Text,
        layer: None,
        side: Side::Top,
        points: Vec::new(),
        from: None,
        to: None,
        center: None,
        radius: None,
        start: None,
        sweep: None,
        at: Some(at),
        text,
        height: Some(height),
        rotation: 0.0,
        width: height / 10.0,
        fill: false,
    }
}

/// One side's silkscreen: the footprints' drawings (their name placeholders left out), the
/// board's drawings, and the names the board shows.
fn legend(w: &mut Writer, ecad: &Electronics, side: Side, glyphs: &Glyphs) {
    let b = &ecad.board;
    let mut texts: Vec<(Graphic, Mirror)> = Vec::new();
    let flip = if side == Side::Bottom { Mirror::InPlace } else { Mirror::No };
    for f in &b.footprints {
        let Some(placed) = ecad.placed(f) else { continue };
        let placement = f.placement();
        for g in placed.resolved.footprint.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Silkscreen)) {
            let on = if placement.mirror { other(g.side) } else { g.side };
            if on != side || (g.kind == GraphicType::Text && g.text.contains('{')) {
                continue;
            }
            let g = placement.graphic(g);
            if g.kind == GraphicType::Text {
                texts.push((g, flip));
            } else {
                graphic(w, &g, Mirror::No, glyphs);
            }
        }
    }
    let drawn = if side == Side::Bottom { DrawLayer::SilkscreenBottom } else { DrawLayer::SilkscreenTop };
    for g in b.graphics.iter().filter(|g| g.layer == drawn) {
        if g.graphic.kind == GraphicType::Text {
            texts.push((g.graphic.clone(), if g.mirror { Mirror::AboutStart } else { Mirror::No }));
        } else {
            graphic(w, &g.graphic, Mirror::No, glyphs);
        }
    }
    for (name, s, [lo, hi], _) in solidcraft_doc::ecad::labels::shown_names(ecad) {
        if s != side {
            continue;
        }
        texts.push((text_graphic(lo, name, hi[1] - lo[1]), flip));
    }
    for (g, mirror) in texts {
        graphic(w, &g, mirror, glyphs);
    }
}
