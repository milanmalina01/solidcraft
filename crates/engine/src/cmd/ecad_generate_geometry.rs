//! Footprints and symbols worked out from a package's drawing, as plain geometry: no design, no
//! command. The land rules are SolidCraft's own and simple: a pad reaches `toe` past a lead's tip,
//! `heel` inside its inner end, and `side` past each of its edges; the defaults are moderate and
//! every one can be given. Package sizes are the nominal values of public EIA and JEDEC tables.

use solidcraft_doc::ecad::{BoardLayer, Graphic, GraphicType, Pad, PadKind, PadShape, Pin, PinDirection, PinType, Side};

/// How far pads reach beyond a lead, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Land {
    pub toe: f64,
    pub heel: f64,
    pub side: f64,
}

/// A footprint's pads and its fabrication and silkscreen graphics (the courtyard is made after).
pub(super) struct Drawn {
    pub pads: Vec<Pad>,
    pub graphics: Vec<Graphic>,
}

/// A two-terminal chip (resistor, capacitor) by its EIA size code in inches, as `0603`: body
/// length, width and terminal length.
pub(super) fn chip_size(code: &str) -> Option<[f64; 3]> {
    Some(match code {
        "0201" => [0.6, 0.3, 0.15],
        "0402" => [1.0, 0.5, 0.25],
        "0603" => [1.6, 0.8, 0.3],
        "0805" => [2.0, 1.25, 0.4],
        "1206" => [3.2, 1.6, 0.5],
        "1210" => [3.2, 2.5, 0.5],
        "2010" => [5.0, 2.5, 0.6],
        "2512" => [6.4, 3.2, 0.6],
        _ => return None,
    })
}

pub(super) fn pad(number: impl ToString, at: [f64; 2], size: [f64; 2]) -> Pad {
    Pad { number: number.to_string(), at, size, rotation: 0.0, side: Side::Top, shape: PadShape::Rect, corner: 0.0, kind: PadKind::Smd, drill: None }
}

fn graphic(kind: GraphicType, layer: Option<BoardLayer>, width: f64) -> Graphic {
    Graphic {
        kind,
        layer,
        side: Side::Top,
        points: Vec::new(),
        from: None,
        to: None,
        center: None,
        radius: None,
        start: None,
        sweep: None,
        at: None,
        text: String::new(),
        height: None,
        rotation: 0.0,
        width,
        fill: false,
    }
}

pub(super) fn rect(layer: Option<BoardLayer>, from: [f64; 2], to: [f64; 2], width: f64) -> Graphic {
    Graphic { from: Some(from), to: Some(to), ..graphic(GraphicType::Rect, layer, width) }
}

pub(super) fn line(layer: Option<BoardLayer>, points: Vec<[f64; 2]>, width: f64) -> Graphic {
    Graphic { points, ..graphic(GraphicType::Line, layer, width) }
}

pub(super) fn dot(layer: Option<BoardLayer>, center: [f64; 2], radius: f64) -> Graphic {
    Graphic { center: Some(center), radius: Some(radius), fill: true, ..graphic(GraphicType::Circle, layer, 0.0) }
}

const FAB: Option<BoardLayer> = Some(BoardLayer::Fab);
const SILK: Option<BoardLayer> = Some(BoardLayer::Silkscreen);
const FAB_WIDTH: f64 = 0.1;
const SILK_WIDTH: f64 = 0.12;

/// Which corner of the body pin 1 is at.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pin1 {
    TopLeft,
    BottomLeft,
}

/// The fabrication outline: the body, with its pin-1 corner cut by `cut` (`None`: no cut).
fn body(half: [f64; 2], cut: Option<f64>, pin1: Pin1) -> Graphic {
    let [hx, hy] = half;
    match cut {
        None => rect(FAB, [-hx, -hy], [hx, hy], FAB_WIDTH),
        Some(c) => {
            let c = c.min(hx).min(hy);
            let mut points = vec![[-hx + c, hy], [hx, hy], [hx, -hy], [-hx, -hy], [-hx, hy - c], [-hx + c, hy]];
            if pin1 == Pin1::BottomLeft {
                for p in &mut points {
                    p[1] = -p[1];
                }
            }
            line(FAB, points, FAB_WIDTH)
        }
    }
}

/// A lead's pad across one row: from `inner` to `outer` along the row's axis, `width` wide.
fn span(inner: f64, outer: f64) -> (f64, f64) {
    ((inner + outer) / 2.0, outer - inner)
}

/// A two-terminal chip `[length, width, terminal]`: pads 1 (left) and 2 (right).
pub(super) fn chip([l, w, t]: [f64; 3], land: Land) -> Drawn {
    let (centre, length) = span(l / 2.0 - t - land.heel, l / 2.0 + land.toe);
    let size = [length, w + 2.0 * land.side];
    Drawn { pads: vec![pad(1, [-centre, 0.0], size), pad(2, [centre, 0.0], size)], graphics: vec![body([l / 2.0, w / 2.0], None, Pin1::TopLeft)] }
}

/// Gull-wing leads in two rows along x (SOIC, SOT-23 style), `per_row` in each row `pitch` apart.
/// `span` is the lead span tip to tip, `lead` the lead's foot length and `lead_width` its width.
/// Pin 1 is top left; numbers run down the left and up the right.
pub(super) fn dual(per_row: [usize; 2], pitch: f64, span_: f64, lead: f64, lead_width: f64, body_: [f64; 2], land: Land) -> Drawn {
    let (centre, length) = span(span_ / 2.0 - lead - land.heel, span_ / 2.0 + land.toe);
    let size = [length, lead_width + 2.0 * land.side];
    let mut pads = Vec::new();
    let mut n = 1;
    for (row, count) in per_row.into_iter().enumerate() {
        let x = if row == 0 { -centre } else { centre };
        let top = (count.max(1) - 1) as f64 * pitch / 2.0;
        for i in 0..count {
            // Left row from the top down, right row from the bottom up.
            let y = if row == 0 { top - i as f64 * pitch } else { -top + i as f64 * pitch };
            pads.push(pad(n, [x, y], size));
            n += 1;
        }
    }
    let marker = [-(centre + length / 2.0 + 0.3), pads.first().map_or(0.0, |p| p.at[1])];
    Drawn { pads, graphics: vec![body([body_[0] / 2.0, body_[1] / 2.0], Some(0.6_f64.min(body_[0] / 4.0)), Pin1::TopLeft), dot(SILK, marker, 0.15)] }
}

/// Three leads, SOT-23 style: pins 1 and 2 on the bottom row `pitch` either side of the middle, pin 3
/// alone on top. The rows are along y.
pub(super) fn sot23(pitch: f64, span_: f64, lead: f64, lead_width: f64, body_: [f64; 2], land: Land) -> Drawn {
    let (centre, length) = span(span_ / 2.0 - lead - land.heel, span_ / 2.0 + land.toe);
    let size = [lead_width + 2.0 * land.side, length];
    let pads = vec![pad(1, [-pitch, -centre], size), pad(2, [pitch, -centre], size), pad(3, [0.0, centre], size)];
    let marker = [-pitch - size[0] / 2.0 - 0.3, -centre - length / 2.0];
    Drawn { pads, graphics: vec![body([body_[0] / 2.0, body_[1] / 2.0], Some(0.3), Pin1::BottomLeft), dot(SILK, marker, 0.15)] }
}

/// A quad flat no-lead package: `per_side` terminals on each side of a `body` square-ish package,
/// `pitch` apart, `lead` long and `lead_width` wide, and an exposed pad (`thermal`, numbered after the
/// last terminal) when given. Pin 1 is at the top of the left side; numbers run counter-clockwise.
pub(super) fn qfn(per_side: usize, pitch: f64, body_: [f64; 2], lead: f64, lead_width: f64, thermal: Option<[f64; 2]>, land: Land) -> Drawn {
    let mut pads = Vec::new();
    let mut n = 1;
    let along = (per_side.max(1) - 1) as f64 * pitch / 2.0;
    for side in 0..4 {
        let half = if side % 2 == 0 { body_[0] / 2.0 } else { body_[1] / 2.0 };
        let (centre, length) = span(half - lead - land.heel, half + land.toe);
        let across = lead_width + 2.0 * land.side;
        for i in 0..per_side {
            let t = -along + i as f64 * pitch;
            let (at, size) = match side {
                // Left, top to bottom.
                0 => ([-centre, -t], [length, across]),
                // Bottom, left to right.
                1 => ([t, -centre], [across, length]),
                // Right, bottom to top.
                2 => ([centre, t], [length, across]),
                // Top, right to left.
                _ => ([-t, centre], [across, length]),
            };
            pads.push(pad(n, at, size));
            n += 1;
        }
    }
    if let Some(ep) = thermal {
        pads.push(pad(n, [0.0, 0.0], ep));
    }
    let marker = [-body_[0] / 2.0 - land.toe - 0.3, along];
    Drawn { pads, graphics: vec![body([body_[0] / 2.0, body_[1] / 2.0], Some(0.5), Pin1::TopLeft), dot(SILK, marker, 0.15)] }
}

/// How far silkscreen keeps from copper, beyond half its line width.
const SILK_CLEARANCE: f64 = 0.15;

/// Through-hole pins in `rows` rows (1 or 2) of `per_row`, `pitch` apart, the rows `row_spacing` apart:
/// round pads `pad_size` across with a `drill` (`[pad_size, drill]`), pin 1's pad square, pin 1 at the top left. A DIP
/// (`dip`) numbers down the left row and up the right; a two-row header numbers across the rows (1
/// and 2 side by side, then 3 and 4), and a one-row header along its row.
pub(super) fn through_hole(
    dip: bool,
    rows: usize,
    per_row: usize,
    pitch: f64,
    row_spacing: f64,
    [pad_size, drill]: [f64; 2],
    body_: [f64; 2],
) -> Drawn {
    let rows = rows.clamp(1, 2);
    let mut pads = Vec::new();
    let top = (per_row.max(1) - 1) as f64 * pitch / 2.0;
    for row in 0..rows {
        let x = if rows == 1 {
            0.0
        } else if row == 0 {
            -row_spacing / 2.0
        } else {
            row_spacing / 2.0
        };
        for i in 0..per_row {
            let (n, y) = if dip && row == 1 {
                (per_row + i + 1, -top + i as f64 * pitch)
            } else if dip {
                (i + 1, top - i as f64 * pitch)
            } else {
                (i * rows + row + 1, top - i as f64 * pitch)
            };
            let shape = if n == 1 { PadShape::Rect } else { PadShape::Round };
            pads.push(Pad { shape, kind: PadKind::ThroughHole, drill: Some([drill, drill]), ..pad(n, [x, y], [pad_size, pad_size]) });
        }
    }
    pads.sort_by_key(|p| p.number.parse::<usize>().unwrap_or(usize::MAX));
    // A DIP's outline runs between its rows, clear of the pads; a header's goes round them.
    let clear = SILK_WIDTH / 2.0 + SILK_CLEARANCE;
    let half_x = if dip && rows == 2 { (body_[0] / 2.0).min(row_spacing / 2.0 - pad_size / 2.0 - clear).max(clear) } else { body_[0] / 2.0 };
    let silk = rect(SILK, [-half_x, -body_[1] / 2.0], [half_x, body_[1] / 2.0], SILK_WIDTH);
    Drawn { pads, graphics: vec![body([body_[0] / 2.0, body_[1] / 2.0], Some(1.0), Pin1::TopLeft), silk] }
}

/// Where a generated symbol's pin goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PinSide {
    Left,
    Right,
    Top,
    Bottom,
}

/// The symbol grid, in millimetres (0.1 in).
pub(super) const GRID: f64 = 2.54;

/// A box symbol. Each side's pins are `GRID` apart, listed from the top (left and right sides) or
/// the left (top and bottom), every pin end on the grid so wires meet them; the box is big enough
/// for the names.
pub(super) fn box_symbol(pins: &[(String, String, PinType, PinSide)]) -> (Vec<Pin>, Vec<Graphic>) {
    let on = |side: PinSide| pins.iter().filter(move |p| p.3 == side).collect::<Vec<_>>();
    let (left, right, top, bottom) = (on(PinSide::Left), on(PinSide::Right), on(PinSide::Top), on(PinSide::Bottom));
    // The i-th of n along a side, on the grid, the middle one (or the one above the middle) at 0.
    let place = |n: usize, i: usize| ((n.max(1) - 1) / 2) as f64 * GRID - i as f64 * GRID;
    let ys: Vec<f64> = [&left, &right].iter().flat_map(|side| (0..side.len()).map(|i| place(side.len(), i))).collect();
    let xs: Vec<f64> = [&top, &bottom].iter().flat_map(|side| (0..side.len()).map(|i| -place(side.len(), i))).collect();
    let y_top = ys.iter().copied().fold(0.0, f64::max) + GRID;
    let y_bottom = ys.iter().copied().fold(0.0, f64::min) - GRID;
    // Room for the longest name each side, at 1.27 mm text (0.6 of the height per character).
    let name = |side: &[&(String, String, PinType, PinSide)]| side.iter().map(|p| p.1.chars().count()).max().unwrap_or(0) as f64 * 1.27 * 0.6;
    let reach = xs.iter().map(|x| x.abs()).fold(0.0, f64::max) + GRID;
    let half_w = (((name(&left) + name(&right) + GRID) / 2.0).max(reach).max(2.0 * GRID) / GRID).ceil() * GRID;
    let mut out = Vec::new();
    for (side, list) in [(PinSide::Left, &left), (PinSide::Right, &right), (PinSide::Top, &top), (PinSide::Bottom, &bottom)] {
        for (i, (number, name, electrical, _)) in list.iter().enumerate() {
            let t = place(list.len(), i);
            let (at, direction) = match side {
                PinSide::Left => ([-half_w - GRID, t], PinDirection::Right),
                PinSide::Right => ([half_w + GRID, t], PinDirection::Left),
                PinSide::Top => ([-t, y_top + GRID], PinDirection::Down),
                PinSide::Bottom => ([-t, y_bottom - GRID], PinDirection::Up),
            };
            out.push(Pin { number: number.clone(), name: name.clone(), electrical: *electrical, at, direction, length: GRID });
        }
    }
    // Listed in the order they were given, whichever side each is on.
    out.sort_by_key(|p| pins.iter().position(|q| q.0 == p.number));
    (out, vec![rect(None, [-half_w, y_bottom], [half_w, y_top], 0.254)])
}

/// A two-terminal symbol (a resistor's box, or a capacitor's two plates): pin 1 left, pin 2 right.
pub(super) fn two_terminal(capacitor: bool) -> (Vec<Pin>, Vec<Graphic>) {
    let pin =
        |n: &str, at: [f64; 2], direction| Pin { number: n.into(), name: String::new(), electrical: PinType::Passive, at, direction, length: GRID };
    let pins = vec![pin("1", [-2.0 * GRID, 0.0], PinDirection::Right), pin("2", [2.0 * GRID, 0.0], PinDirection::Left)];
    let body = if capacitor {
        vec![
            line(None, vec![[-GRID, 0.0], [-0.508, 0.0]], 0.254),
            line(None, vec![[-0.508, -1.524], [-0.508, 1.524]], 0.4064),
            line(None, vec![[0.508, -1.524], [0.508, 1.524]], 0.4064),
            line(None, vec![[0.508, 0.0], [GRID, 0.0]], 0.254),
        ]
    } else {
        vec![rect(None, [-GRID, -1.016], [GRID, 1.016], 0.254)]
    };
    (pins, body)
}
