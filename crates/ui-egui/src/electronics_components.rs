//! The open library item as the editor works on it, and how symbols and footprints are drawn: on
//! the page (with the Inspector's pick, `electronics_editor`), on a component's page side by side,
//! and in the Library card's preview. Every change is an `ecad.*` command, and the page follows the
//! item the last one named (`electronics_library::follow`), so an agent's work shows as it lands.

use egui::{Align2, FontId, Pos2, Stroke, pos2, vec2};
use solidcraft_engine::doc::ecad::overbar;
use solidcraft_engine::doc::ecad::{
    BoardLayer, ElectronicComponent, FootprintItem, Graphic, GraphicType, NO_FOOTPRINT, NO_SYMBOL, PackageItem, PadKind, Resolved, Side, SymbolItem,
    Variant,
};

use crate::SolveApp;
use crate::electronics_library::Opened;
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;

/// Which drawing of the open item the editor works on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Symbol,
    Footprint,
}

/// Text on the page is drawn only while a millimetre is at least this many points.
const MIN_LABEL_SCALE: f64 = 4.0;
/// Graphic text is drawn only between these sizes on screen, in points.
const TEXT_POINTS: std::ops::RangeInclusive<f32> = 4.0..=256.0;

/// Stands in for the component of a symbol or a footprint opened alone: it has none.
static NO_COMPONENT: ElectronicComponent = ElectronicComponent {
    id: 0,
    name: String::new(),
    prefix: String::new(),
    value: String::new(),
    mpn: String::new(),
    manufacturer: String::new(),
    description: String::new(),
    symbol: None,
    variants: Vec::new(),
};

/// The open item as the editor works on it: an electronic component with its symbol and its
/// chosen variant's footprint, or a symbol or a footprint alone (with an empty other drawing and
/// a component of id 0). `None` with nothing open, or a 3D package.
pub(crate) fn shown(app: &SolveApp) -> Option<Resolved<'_>> {
    let ecad = &app.session.doc.ecad;
    match app.electronics.opened? {
        Opened::Component(id) => {
            let c = ecad.component(id)?;
            let variant = app.electronics.variant.as_deref().and_then(|name| c.variant(name)).or(c.variants.first());
            Some(ecad.resolve_variant(c, variant))
        }
        Opened::Symbol(id) => {
            let symbol = ecad.symbol(id)?;
            Some(Resolved { component: &NO_COMPONENT, symbol, variant: None, footprint: &NO_FOOTPRINT, package: None })
        }
        Opened::Footprint(id) => {
            let footprint = ecad.footprint(id)?;
            Some(Resolved { component: &NO_COMPONENT, symbol: &NO_SYMBOL, variant: None, footprint, package: None })
        }
        Opened::Package(_) => None,
    }
}

/// What [`shown`] gives, copied, to keep while commands run.
#[derive(Clone, Debug)]
pub(crate) struct Shown {
    pub component: ElectronicComponent,
    pub symbol: SymbolItem,
    pub variant: Option<Variant>,
    pub footprint: FootprintItem,
    pub package: Option<PackageItem>,
}

impl Shown {
    /// Looked up again, to work out what connects and what is ready.
    pub(crate) fn resolved(&self) -> Resolved<'_> {
        Resolved {
            component: &self.component,
            symbol: &self.symbol,
            variant: self.variant.as_ref(),
            footprint: &self.footprint,
            package: self.package.as_ref(),
        }
    }
}

/// [`shown`], copied.
pub(crate) fn shown_copy(app: &SolveApp) -> Option<Shown> {
    shown(app).map(|r| Shown {
        component: r.component.clone(),
        symbol: r.symbol.clone(),
        variant: r.variant.cloned(),
        footprint: r.footprint.clone(),
        package: r.package.cloned(),
    })
}

/// What Fit frames: the shown view's drawing, `[[x0, y0], [x1, y1]]` in page millimetres.
pub(crate) fn bounds(app: &SolveApp) -> Option<[[f64; 2]; 2]> {
    let c = shown(app)?;
    match app.electronics.view {
        View::Symbol => c.symbol.bbox(),
        View::Footprint => c.footprint.copper_bbox(),
    }
}

/// What the camera framed last: the open item, and whether it had anything in it (a first pin or
/// pad is framed afresh).
pub(crate) fn framing(app: &SolveApp) -> Option<(Opened, bool)> {
    app.electronics.opened.map(|o| (o, bounds(app).is_some()))
}

/// Draw the open symbol or footprint over the page's grid, with the shape being drawn.
pub(crate) fn paint(app: &SolveApp, painter: &egui::Painter, map: PageMap) {
    let Some(c) = shown(app) else { return };
    match app.electronics.view {
        View::Symbol => paint_symbol(app, painter, map, c.symbol, true),
        View::Footprint => paint_footprint(app, painter, map, c.footprint, true),
    }
    // The shape being drawn, from its clicked points to the pointer.
    let (hover, free) = painter.ctx().input(|i| (i.pointer.hover_pos(), i.modifiers.command || i.modifiers.ctrl));
    let pointer = hover.filter(|p| painter.clip_rect().contains(*p)).map(|p| crate::electronics_drawing::snap(app, map, p, free));
    crate::electronics_drawing::preview(app, painter, map, pointer);
}

/// A symbol's body and pins. `live`: on a page, where the pick, a drag and the pin being connected
/// show, and the pins are published for scenarios; else a still picture (the Library's preview).
pub(crate) fn paint_symbol(app: &SolveApp, painter: &egui::Painter, map: PageMap, symbol: &SymbolItem, live: bool) {
    let t = Tokens::get();
    let view = Tokens::view();
    let labels = map.scale() >= MIN_LABEL_SCALE;
    let font = FontId::proportional(view.font_small);
    let on_page = |p: Pos2| p.x.is_finite() && p.y.is_finite();
    for (i, g) in symbol.graphics.iter().enumerate() {
        if live {
            shown_graphic(app, painter, map, i, g, view.text, &font);
        } else {
            paint_graphic(painter, map, g, view.text, &font);
        }
    }
    for pin in &symbol.pins {
        // A pin being dragged is drawn where it is going.
        let [dx, dy] = if live { crate::electronics_editor::moved_by(app, true, &pin.number) } else { [0.0; 2] };
        let a = map.to_screen(pin.at[0] + dx, pin.at[1] + dy);
        let b = map.to_screen(pin.body_end()[0] + dx, pin.body_end()[1] + dy);
        if !(on_page(a) && on_page(b)) {
            continue;
        }
        if live && (crate::electronics_editor::is_picked(app, true, &pin.number) || crate::electronics_editor::is_connecting(app, true, &pin.number))
        {
            painter.line_segment([a, b], Stroke::new(6.0, t.sel_edge_rim));
            painter.circle_filled(a, 5.0, t.sel_edge);
        }
        painter.line_segment([a, b], Stroke::new(2.0, view.text));
        painter.circle_stroke(a, 3.0, Stroke::new(1.4, t.accent));
        if live {
            crate::scenario::publish_handle(&format!("electronics:pin:{}", pin.number), a);
        }
        if labels {
            let middle = pos2((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
            painter.text(middle - vec2(0.0, 3.0), Align2::CENTER_BOTTOM, &pin.number, font.clone(), view.text_dim);
            if !pin.name.is_empty() && !pin.name_hidden {
                // The name sits just inside the body, past the pin's inner end.
                let [dx, dy] = pin.direction.unit();
                let inside = b + vec2(dx as f32, -dy as f32) * 4.0;
                let align = match pin.direction {
                    solidcraft_engine::doc::ecad::PinDirection::Right => Align2::LEFT_CENTER,
                    solidcraft_engine::doc::ecad::PinDirection::Left => Align2::RIGHT_CENTER,
                    solidcraft_engine::doc::ecad::PinDirection::Up => Align2::CENTER_BOTTOM,
                    solidcraft_engine::doc::ecad::PinDirection::Down => Align2::CENTER_TOP,
                };
                barred_text(painter, inside, align, &pin.name, font.clone(), view.text, 0.0);
            }
        }
    }
}

/// A footprint's pads and its graphics on each layer. `live` as for [`paint_symbol`].
pub(crate) fn paint_footprint(app: &SolveApp, painter: &egui::Painter, map: PageMap, footprint: &FootprintItem, live: bool) {
    let t = Tokens::get();
    let view = Tokens::view();
    let labels = map.scale() >= MIN_LABEL_SCALE;
    let font = FontId::proportional(view.font_small);
    let on_page = |p: Pos2| p.x.is_finite() && p.y.is_finite();
    let layer = |layer: BoardLayer, colour: egui::Color32| {
        for side in [Side::Bottom, Side::Top] {
            for (i, g) in footprint.graphics.iter().enumerate().filter(|(_, g)| g.layer == Some(layer) && g.side == side) {
                if live {
                    shown_graphic(app, painter, map, i, g, colour, &font);
                } else {
                    paint_graphic(painter, map, g, colour, &font);
                }
            }
        }
    };
    // Under the copper: the courtyard and the fabrication outline; over it: silkscreen.
    layer(BoardLayer::Courtyard, t.grid_major);
    layer(BoardLayer::Fab, view.text_dim);
    for pad in &footprint.pads {
        let [dx, dy] = if live { crate::electronics_editor::moved_by(app, false, &pad.number) } else { [0.0; 2] };
        let screen = |outline: Vec<[f64; 2]>| -> Vec<Pos2> { outline.iter().map(|p| map.to_screen(p[0] + dx, p[1] + dy)).collect() };
        let points = screen(pad.outline(6));
        if !points.iter().copied().all(on_page) {
            continue;
        }
        let colour = match (pad.kind, pad.side) {
            (PadKind::Hole, _) => view.text_dim,
            (PadKind::Smd, Side::Bottom) => t.warning,
            _ => t.accent,
        };
        let fill = if pad.kind == PadKind::Hole { egui::Color32::TRANSPARENT } else { colour.gamma_multiply(0.55) };
        let picked = live
            && (crate::electronics_editor::is_picked(app, false, &pad.number) || crate::electronics_editor::is_connecting(app, false, &pad.number));
        if picked {
            painter.add(egui::Shape::closed_line(points.clone(), Stroke::new(5.0, t.sel_edge_rim)));
        }
        painter.add(egui::Shape::convex_polygon(points, fill, Stroke::new(if picked { 2.0 } else { 1.2 }, if picked { t.sel_edge } else { colour })));
        if let Some(hole) = pad.drill_outline(6).map(screen).filter(|h| h.iter().copied().all(on_page)) {
            painter.add(egui::Shape::convex_polygon(hole, t.viewport_bottom, Stroke::new(1.0, colour)));
        }
        let centre = map.to_screen(pad.at[0] + dx, pad.at[1] + dy);
        if live {
            crate::scenario::publish_handle(&format!("electronics:pad:{}", pad.number), centre);
        }
        if labels {
            painter.text(centre, Align2::CENTER_CENTER, &pad.number, font.clone(), view.text);
        }
    }
    layer(BoardLayer::Silkscreen, view.text);
}

/// Graphic `index` of the shown view where it is (or is being dragged to), with a halo when picked.
fn shown_graphic(app: &SolveApp, painter: &egui::Painter, map: PageMap, index: usize, g: &Graphic, colour: egui::Color32, font: &FontId) {
    let by = crate::electronics_editor::graphic_moved_by(app, index);
    let moved;
    let g = if by == [0.0; 2] {
        g
    } else {
        moved = crate::electronics_drawing::translate(g, by);
        &moved
    };
    if crate::electronics_editor::is_picked_graphic(app, index) {
        let halo = Graphic { width: g.width + 4.0 / map.scale(), fill: false, ..g.clone() };
        paint_graphic(painter, map, &halo, Tokens::get().sel_edge_rim, font);
    }
    paint_graphic(painter, map, g, colour, font);
}

/// One graphic, its line width to scale.
pub(crate) fn paint_graphic(painter: &egui::Painter, map: PageMap, g: &Graphic, colour: egui::Color32, font: &FontId) {
    let screen = |points: &[[f64; 2]]| -> Option<Vec<Pos2>> {
        let out: Vec<Pos2> = points.iter().map(|p| map.to_screen(p[0], p[1])).collect();
        out.iter().all(|p| p.x.is_finite() && p.y.is_finite()).then_some(out)
    };
    let width = ((g.width * map.scale()) as f32).clamp(1.0, 64.0);
    let stroke = Stroke::new(width, colour);
    let fill = if g.fill { colour.gamma_multiply(0.35) } else { egui::Color32::TRANSPARENT };
    match g.kind {
        GraphicType::Line => {
            if let Some(points) = screen(&g.points) {
                painter.add(egui::Shape::line(points, stroke));
            }
        }
        GraphicType::Polygon => {
            if let Some(points) = screen(&g.points) {
                painter.add(egui::Shape::closed_line(points.clone(), stroke));
                if g.fill && points.len() >= 3 {
                    painter.add(egui::Shape::convex_polygon(points, fill, Stroke::NONE));
                }
            }
        }
        GraphicType::Rect => {
            let (Some(a), Some(b)) = (g.from, g.to) else { return };
            if let Some(points) = screen(&[a, [b[0], a[1]], b, [a[0], b[1]]]) {
                painter.add(egui::Shape::convex_polygon(points, fill, stroke));
            }
        }
        GraphicType::Circle => {
            let (Some(c), Some(r)) = (g.center, g.radius) else { return };
            let centre = map.to_screen(c[0], c[1]);
            let r = (r * map.scale()) as f32;
            if centre.x.is_finite() && centre.y.is_finite() && r.is_finite() {
                painter.add(egui::Shape::circle_filled(centre, r, fill));
                painter.circle_stroke(centre, r, stroke);
            }
        }
        GraphicType::Arc => {
            if let Some(points) = screen(&g.arc_points(48)) {
                painter.add(egui::Shape::line(points, stroke));
            }
        }
        GraphicType::Text => {
            let (Some(at), Some(h)) = (g.at, g.height) else { return };
            let size = (h * map.scale()) as f32;
            if !TEXT_POINTS.contains(&size) {
                return;
            }
            let p = map.to_screen(at[0], at[1]);
            if p.x.is_finite() && p.y.is_finite() {
                // Upright on screen; a turned text keeps its anchor.
                barred_text(painter, p, Align2::LEFT_BOTTOM, &g.text, FontId::new(size, font.family.clone()), colour, 0.0);
            }
        }
    }
}

/// `text` with its `anchor` corner at `p`, turned `angle` radians about `p` (clockwise on screen),
/// its overbar markup (`~{CE}`) drawn as a bar over the barred letters. Returns the unturned
/// screen box of the text.
pub(crate) fn barred_text(
    painter: &egui::Painter,
    p: Pos2,
    anchor: Align2,
    text: &str,
    font: FontId,
    colour: egui::Color32,
    angle: f32,
) -> egui::Rect {
    let barred = overbar::parse(text);
    let galley = painter.layout_no_wrap(barred.text, font, colour);
    let corner = anchor.pos_in_rect(&galley.rect).to_vec2();
    let turn = egui::emath::Rot2::from_angle(angle);
    let rect = egui::Rect::from_min_size(p - corner, galley.size());
    if let Some(row) = galley.rows.first() {
        // Each letter's start, end and top of its capitals (about four fifths of its ascent), in
        // the galley.
        let letters: Vec<(f32, f32, f32)> =
            row.row.glyphs.iter().map(|g| (row.pos.x + g.pos.x, row.pos.x + g.max_x(), row.pos.y + g.pos.y - g.font_ascent * 0.8)).collect();
        let width = (galley.size().y * 0.06).max(1.0);
        for bar in &barred.bars {
            let (Some(first), Some(last)) = (letters.get(bar.start), bar.end.checked_sub(1).and_then(|k| letters.get(k))) else { continue };
            let screen = |x: f32| p + turn * (vec2(x, first.2) - corner);
            painter.line_segment([screen(first.0 + width * 0.5), screen(last.1 - width * 0.5)], Stroke::new(width, colour));
        }
    }
    painter.add(egui::epaint::TextShape::new(p - corner, galley, colour).with_angle_and_anchor(angle, anchor));
    rect
}

/// Pin names and numbers on the sheet, in millimetres, as the export pages set them.
const PIN_NAME: f64 = 1.0;
const PIN_NUMBER: f64 = 0.85;
/// Pin texts smaller than this on screen, in points, are left out (zoom in to read them).
const MIN_PIN_POINTS: f32 = 4.0;
const MAX_PIN_POINTS: f32 = 64.0;
/// How finely a pin name with no room is cut short: to 7/8 of its length, 6/8, and so on.
const NAME_CUTS: usize = 8;
/// The turn of a text reading upwards, as an upright pin's.
const UPRIGHT: f32 = -std::f32::consts::FRAC_PI_2;
/// How far off the screen, in points, a pin text's anchor may be and still be set.
const SET_MARGIN: f32 = 400.0;

/// A placed part's pin numbers and names, as the export pages set them: each number over its pin
/// (left of an upright one) just outside the body, none on a part of two pins showing no names (a
/// resistor's, a diode's); each name inside the body past its pin's end, its overbar drawn. Level names go
/// first; a name that would run into one already set is set smaller, then cut short (an ellipsis
/// says so, half its letters kept at least), else left out. Texts that would be smaller than
/// [`MIN_PIN_POINTS`] are not drawn.
pub(crate) fn pin_texts(painter: &egui::Painter, map: PageMap, pins: &[solidcraft_engine::doc::ecad::Pin], ink: egui::Color32, dim: egui::Color32) {
    use solidcraft_engine::doc::ecad::PinDirection;
    let scale = map.scale();
    // Only what can reach the screen is set, so a symbol of many pins costs what the view shows.
    let view = painter.clip_rect().expand(SET_MARGIN);
    let finite = |p: Pos2| p.x.is_finite() && p.y.is_finite() && view.contains(p);
    if let Some(number_size) = number_size(pins, scale) {
        let font = FontId::proportional(number_size);
        let lift = number_lift(scale);
        for pin in pins {
            let [ux, uy] = pin.direction.unit();
            let b = pin.body_end();
            let near = map.to_screen(b[0] - ux * 0.3, b[1] - uy * 0.3);
            if !finite(near) {
                continue;
            }
            // From the body outwards.
            let (at, anchor, angle) = match pin.direction {
                PinDirection::Right => (near - vec2(0.0, lift), Align2::RIGHT_BOTTOM, 0.0),
                PinDirection::Left => (near - vec2(0.0, lift), Align2::LEFT_BOTTOM, 0.0),
                PinDirection::Down => (near - vec2(lift, 0.0), Align2::LEFT_BOTTOM, UPRIGHT),
                PinDirection::Up => (near - vec2(lift, 0.0), Align2::RIGHT_BOTTOM, UPRIGHT),
            };
            barred_text(painter, at, anchor, &pin.number, font.clone(), dim, angle);
        }
    }
    let size = ((PIN_NAME * scale) as f32).min(MAX_PIN_POINTS);
    if size < MIN_PIN_POINTS {
        return;
    }
    let least = (size * 0.6).max(MIN_PIN_POINTS);
    let mut names: Vec<(bool, Pos2, Align2, f32, &str)> = pins
        .iter()
        .filter(|p| p.shows_name())
        .filter_map(|pin| {
            let [ux, uy] = pin.direction.unit();
            let b = pin.body_end();
            let at = map.to_screen(b[0] + ux * 0.6, b[1] + uy * 0.6);
            let (level, anchor, angle) = match pin.direction {
                PinDirection::Right => (true, Align2::LEFT_CENTER, 0.0),
                PinDirection::Left => (true, Align2::RIGHT_CENTER, 0.0),
                PinDirection::Up => (false, Align2::LEFT_CENTER, UPRIGHT),
                PinDirection::Down => (false, Align2::RIGHT_CENTER, UPRIGHT),
            };
            finite(at).then_some((level, at, anchor, angle, pin.name.as_str()))
        })
        .collect();
    names.sort_by_key(|n| !n.0);
    let mut taken: Vec<egui::Rect> = Vec::new();
    for (_, at, anchor, angle, name) in names {
        let free = |text: &str, size: f32, taken: &[egui::Rect]| {
            let b = text_box(painter, at, anchor, angle, text, size, ink);
            (!taken.iter().any(|o| o.min.x < b.max.x && b.min.x < o.max.x && o.min.y < b.max.y && b.min.y < o.max.y)).then_some(b)
        };
        let mut set = [size, (size * 0.8).max(least), least].into_iter().find_map(|s| free(name, s, &taken).map(|b| (name.to_string(), s, b)));
        if set.is_none() {
            // No room at any size: cut short to the room there is, keeping half its letters at
            // least (two at least), or a cut would say nothing.
            let room = text_box(painter, at, anchor, 0.0, name, least, ink).width();
            let letters = overbar::plain(name).chars().count();
            let fewest = letters.div_ceil(2).max(2);
            for k in (1..NAME_CUTS).rev() {
                let cut = fit(painter, name, least, room * k as f32 / NAME_CUTS as f32, ink);
                if overbar::plain(&cut).chars().count() < fewest.saturating_add(1) {
                    break;
                }
                if let Some(b) = free(&cut, least, &taken) {
                    set = Some((cut, least, b));
                    break;
                }
            }
        }
        if let Some((text, size, b)) = set {
            taken.push(b);
            barred_text(painter, at, anchor, &text, FontId::proportional(size), ink, angle);
        }
    }
}

/// The size, in points, [`pin_texts`] sets the numbers of `pins` at on a sheet of `scale` points a
/// millimetre: `None` when it sets none (a part of two pins showing no names, or too small to read).
pub(crate) fn number_size(pins: &[solidcraft_engine::doc::ecad::Pin], scale: f64) -> Option<f32> {
    let named = pins.iter().any(|p| p.shows_name());
    let size = ((PIN_NUMBER * scale) as f32).min(MAX_PIN_POINTS);
    ((pins.len() > 2 || named) && size >= MIN_PIN_POINTS).then_some(size)
}

/// How far a pin's number stands off its pin, in points.
pub(crate) fn number_lift(scale: f64) -> f32 {
    (0.35 * scale) as f32
}

/// The screen box `text` (overbar markup left out) takes at `size` points, its `anchor` corner at
/// `p`, turned `angle` radians about it. Laid out in the `colour` it is drawn in, so drawing it
/// reuses the same layout.
fn text_box(painter: &egui::Painter, p: Pos2, anchor: Align2, angle: f32, text: &str, size: f32, colour: egui::Color32) -> egui::Rect {
    let galley = painter.layout_no_wrap(overbar::plain(text).into_owned(), FontId::proportional(size), colour);
    let s = galley.size();
    // Lines take more height than their letters: the box keeps to the letters, so neighbouring
    // pins' names don't count as meeting.
    let inset = s.y * 0.1;
    let corner = anchor.pos_in_rect(&egui::Rect::from_min_size(Pos2::ZERO, s)).to_vec2();
    let turn = egui::emath::Rot2::from_angle(angle);
    let corners = [vec2(0.0, inset), vec2(s.x, inset), vec2(s.x, s.y - inset), vec2(0.0, s.y - inset)].map(|c| p + turn * (c - corner));
    egui::Rect::from_points(&corners)
}

/// `text` cut short with an ellipsis to fit `width` points at `size` (its overbar markup kept for
/// the letters left); the whole of it when it fits. Laid out in the `colour` it is drawn in.
pub(crate) fn fit(painter: &egui::Painter, text: &str, size: f32, width: f32, colour: egui::Color32) -> String {
    let barred = overbar::parse(text);
    let font = FontId::proportional(size);
    let galley = painter.layout_no_wrap(barred.text.clone(), font.clone(), colour);
    if galley.size().x <= width + 1e-3 {
        return text.to_string();
    }
    let dots = painter.layout_no_wrap("…".to_string(), font, colour).size().x;
    let kept = galley.rows.first().map_or(0, |row| row.row.glyphs.iter().take_while(|g| g.max_x() + dots <= width).count());
    let mut out = overbar::cut(&barred, kept);
    out.push('…');
    out
}

/// The note for an open symbol or footprint with nothing in it yet, if any.
pub(crate) fn empty_note(app: &SolveApp) -> Option<String> {
    let c = shown(app)?;
    match app.electronics.view {
        View::Symbol if c.symbol.is_empty() => Some(crate::i18n::format("electronics.no_pins", &[("name", &c.symbol.name)])),
        View::Footprint if c.footprint.is_empty() => Some(crate::i18n::format("electronics.no_pads", &[("name", &c.footprint.name)])),
        _ => None,
    }
}
