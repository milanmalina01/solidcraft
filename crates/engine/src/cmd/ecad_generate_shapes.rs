//! Standard schematic symbol shapes as plain geometry: no design, no command. Each is SolidCraft's
//! own drawing of the conventional form engineers read on any schematic (the IEC 60617 and IEEE
//! 315 graphic symbols: a diode's triangle and bar, an inductor's humps, a transistor in its
//! circle), set on the 0.1 in grid: every pin end lies on it so wires meet them, two-terminal
//! shapes run left to right with their pins 5.08 mm either side of the middle (as the resistor
//! and capacitor do), and a polarised part's anode (or plus) is on the left.
//!
//! Each pin has a role, the name the shape gives it (`A`, `K`, `B`, `G`, `+`, or its number where
//! the shape has no polarity), and a usual number (a diode's cathode is 1, a SOT-23 transistor's
//! base or gate 1); callers renumber them to their footprint's pads.

use solidcraft_doc::ecad::{Graphic, GraphicType, PinDirection};

use super::ecad_generate_geometry::{GRID, NAME_HEIGHT, graphic, line, name_text, names, two_terminal};

/// The body's line width, as the resistor's and capacitor's.
const W: f64 = 0.254;
/// A plate's line width (a capacitor's, a transistor's base bar).
const PLATE: f64 = 0.4064;
/// An enclosure's line width (a crystal's case).
const THIN: f64 = 0.1524;
/// Half the grid: the body's usual half height.
const H: f64 = GRID / 2.0;
/// A wire junction dot's radius.
const DOT: f64 = 0.3;

/// One pin of a shape, before the caller numbers it.
pub(super) struct ShapePin {
    /// The name the shape gives the pin, or its usual number where the shape has no polarity.
    pub role: &'static str,
    /// The usual number.
    pub number: String,
    /// The name it gets when the caller gives none: the role for a polarised pin, else none.
    pub name: String,
    pub at: [f64; 2],
    pub direction: PinDirection,
}

/// A shape's pins (in the order its reply lists them) and its body.
pub(super) struct Shape {
    pub pins: Vec<ShapePin>,
    pub graphics: Vec<Graphic>,
}

/// A standard shape, with what tells its variants apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Resistor,
    Capacitor,
    PolarizedCapacitor,
    Inductor,
    Ferrite,
    Fuse,
    Diode,
    Zener,
    Schottky,
    /// Transient voltage suppressor: one way (drawn as a Zener) or both ways.
    Tvs {
        bidirectional: bool,
    },
    /// `count` LEDs (1 to 4) side by side; more than one share a `common_cathode` or a common anode.
    Led {
        count: usize,
        common_cathode: bool,
    },
    /// A quartz crystal, with its two case pins when `case`.
    Crystal {
        case: bool,
    },
    /// A ceramic resonator with its built-in load capacitors to a middle ground pin.
    Resonator,
    /// A push button, normally open; `four` pins in two joined pairs.
    Switch {
        four: bool,
    },
    Battery,
    Antenna,
    TestPoint,
    Npn,
    Pnp,
    Nmos,
    Pmos,
}

fn pin(role: &'static str, number: &str, named: bool, at: [f64; 2], direction: PinDirection) -> ShapePin {
    ShapePin { role, number: number.into(), name: if named { role.into() } else { String::new() }, at, direction }
}

/// The left pin of a two-terminal shape, at 2 grid steps left.
fn left(role: &'static str, number: &str, named: bool) -> ShapePin {
    pin(role, number, named, [-2.0 * GRID, 0.0], PinDirection::Right)
}

/// The right pin of a two-terminal shape, at 2 grid steps right.
fn right(role: &'static str, number: &str, named: bool) -> ShapePin {
    pin(role, number, named, [2.0 * GRID, 0.0], PinDirection::Left)
}

fn seg(a: [f64; 2], b: [f64; 2], width: f64) -> Graphic {
    line(None, vec![a, b], width)
}

fn polygon(points: Vec<[f64; 2]>, fill: bool) -> Graphic {
    Graphic { points, fill, ..graphic(GraphicType::Polygon, None, W) }
}

fn ring(center: [f64; 2], radius: f64, width: f64) -> Graphic {
    Graphic { center: Some(center), radius: Some(radius), ..graphic(GraphicType::Circle, None, width) }
}

/// A junction dot: a ring as thick as it is wide, so it reads solid however fills are tinted.
fn dot(center: [f64; 2]) -> Graphic {
    ring(center, DOT / 2.0, DOT)
}

fn arc(center: [f64; 2], radius: f64, start: f64, sweep: f64, width: f64) -> Graphic {
    Graphic { center: Some(center), radius: Some(radius), start: Some(start), sweep: Some(sweep), ..graphic(GraphicType::Arc, None, width) }
}

/// The leads of a two-terminal shape: from its left pin's inner end to `x0`, and from `x1` to its
/// right pin's, along the middle line.
fn leads(x0: f64, x1: f64) -> [Graphic; 2] {
    [seg([-GRID, 0.0], [x0, 0.0], W), seg([x1, 0.0], [GRID, 0.0], W)]
}

/// The designator and value over the body, from `left`, value first above `top`: for shapes whose
/// pins leave downwards too.
fn names_above(left: f64, top: f64) -> [Graphic; 2] {
    [name_text("{designator}", [left, top + 0.8 + NAME_HEIGHT]), name_text("{value}", [left, top + 0.5])]
}

/// The designator and value to the right of the body, from `x`, about the middle line.
fn names_right(x: f64) -> [Graphic; 2] {
    [name_text("{designator}", [x, 0.5]), name_text("{value}", [x, -0.5 - NAME_HEIGHT])]
}

/// An arrow from `from` to its tip `to`: a line and a filled head.
fn arrow(from: [f64; 2], to: [f64; 2], head: f64) -> [Graphic; 2] {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let len = dx.hypot(dy).max(1e-9);
    let (ux, uy) = (dx / len, dy / len);
    let back = [to[0] - ux * head, to[1] - uy * head];
    let half = head * 0.45;
    let head = polygon(vec![to, [back[0] - uy * half, back[1] + ux * half], [back[0] + uy * half, back[1] - ux * half]], true);
    [seg(from, back, W), head]
}

/// The bar of a diode at `x`: plain, a Zener's (its ends bent opposite ways) or a Schottky's
/// (its ends hooked back).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bar {
    Plain,
    Zener,
    Schottky,
}

fn bar(x: f64, y: f64, kind: Bar) -> Graphic {
    let tail = 0.635;
    let points = match kind {
        Bar::Plain => vec![[x, y + H], [x, y - H]],
        Bar::Zener => vec![[x - tail, y + H], [x, y + H], [x, y - H], [x + tail, y - H]],
        Bar::Schottky => vec![[x - tail, y + H - tail], [x - tail, y + H], [x, y + H], [x, y - H], [x + tail, y - H], [x + tail, y - H + tail]],
    };
    line(None, points, W)
}

/// A diode on the line `y`, its anode at the left: the triangle pointing right and the bar.
fn diode_glyph(y: f64, kind: Bar) -> [Graphic; 2] {
    [polygon(vec![[-H, y + H], [-H, y - H], [H, y]], false), bar(H, y, kind)]
}

/// An LED's two light arrows, up and to the right from over its triangle on the line `y`.
fn light(y: f64) -> Vec<Graphic> {
    let mut out = Vec::new();
    for x in [-0.508, 0.508] {
        out.extend(arrow([x, y + 1.524], [x + 1.016, y + 2.54], 0.55));
    }
    out
}

/// A crystal's body about the origin: the quartz between its two plates, and leads out to
/// `reach` either side.
fn quartz(reach: f64) -> Vec<Graphic> {
    let plate = 1.016;
    vec![
        graphic_rect([-0.508, -1.016], [0.508, 1.016]),
        seg([-plate, -1.524], [-plate, 1.524], W),
        seg([plate, -1.524], [plate, 1.524], W),
        seg([-reach, 0.0], [-plate, 0.0], W),
        seg([plate, 0.0], [reach, 0.0], W),
    ]
}

fn graphic_rect(from: [f64; 2], to: [f64; 2]) -> Graphic {
    super::ecad_generate_geometry::rect(None, from, to, W)
}

/// A push button's contacts and plunger about the origin, its leads out to the grid either side.
fn button() -> Vec<Graphic> {
    let c = 1.524;
    let r = 0.381;
    vec![
        seg([-GRID, 0.0], [-c - r, 0.0], W),
        seg([c + r, 0.0], [GRID, 0.0], W),
        ring([-c, 0.0], r, W),
        ring([c, 0.0], r, W),
        seg([-c - r, 1.016], [c + r, 1.016], W),
        seg([0.0, 1.016], [0.0, 2.032], W),
        seg([-0.762, 2.032], [0.762, 2.032], W),
    ]
}

/// A plus sign centred on `at`.
fn plus(at: [f64; 2]) -> [Graphic; 2] {
    let s = 0.381;
    [seg([at[0] - s, at[1]], [at[0] + s, at[1]], W), seg([at[0], at[1] - s], [at[0], at[1] + s], W)]
}

/// A transistor's circle.
const CIRCLE: ([f64; 2], f64) = ([0.635, 0.0], 3.175);

/// A transistor's three pins: left, a grid step from the circle; top right and bottom right, led
/// out from the body a grid step past the circle ([`transistor_leads`]), so their numbers print
/// clear of it.
fn transistor_pins(left_: (&'static str, &str), top: (&'static str, &str), bottom: (&'static str, &str)) -> Vec<ShapePin> {
    let mut pins = vec![
        pin(left_.0, left_.1, true, [-2.0 * GRID, 0.0], PinDirection::Right),
        pin(top.0, top.1, true, [GRID, 3.0 * GRID], PinDirection::Down),
        pin(bottom.0, bottom.1, true, [GRID, -3.0 * GRID], PinDirection::Up),
    ];
    pins.sort_by(|a, b| a.number.cmp(&b.number));
    pins
}

/// The leads from a transistor's body out through its circle to the inner ends of its top and
/// bottom pins.
fn transistor_leads() -> [Graphic; 2] {
    [seg([GRID, GRID], [GRID, 2.0 * GRID], W), seg([GRID, -GRID], [GRID, -2.0 * GRID], W)]
}

/// A bipolar transistor: base on the left, collector up, emitter down, the emitter's arrow out
/// (`npn`) or in.
fn bjt(npn: bool) -> Shape {
    let bar_x = -0.635;
    let mut g = vec![
        ring(CIRCLE.0, CIRCLE.1, W),
        seg([-GRID, 0.0], [bar_x, 0.0], W),
        seg([bar_x, -1.524], [bar_x, 1.524], PLATE),
        seg([bar_x, 0.635], [GRID, GRID], W),
        seg([bar_x, -0.635], [GRID, -GRID], W),
    ];
    // Along the emitter, from the bar to the pin.
    let (from, to) = ([bar_x, -0.635], [GRID, -GRID]);
    let at = |t: f64| [from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t];
    let [_, head] = if npn { arrow(at(0.45), at(0.8), 0.8) } else { arrow(at(0.75), at(0.4), 0.8) };
    g.push(head);
    g.extend(transistor_leads());
    g.extend(names_right(4.318));
    Shape { pins: transistor_pins(("B", "1"), ("C", "3"), ("E", "2")), graphics: g }
}

/// An enhancement MOSFET: gate on the left, drain up, source down, the body joined to the source,
/// its arrow in (`n`) or out.
fn mosfet(n: bool) -> Shape {
    let (gate, channel, rail) = (-0.889, -0.254, 1.27);
    let mut g = vec![
        ring(CIRCLE.0, CIRCLE.1, W),
        seg([-GRID, 0.0], [gate, 0.0], W),
        seg([gate, -1.524], [gate, 1.524], W),
        seg([channel, 0.889], [channel, 1.651], PLATE),
        seg([channel, -0.381], [channel, 0.381], PLATE),
        seg([channel, -1.651], [channel, -0.889], PLATE),
        line(None, vec![[channel, rail], [GRID, rail], [GRID, GRID]], W),
        line(None, vec![[channel, -rail], [GRID, -rail], [GRID, -GRID]], W),
        line(None, vec![[channel, 0.0], [GRID, 0.0], [GRID, -rail]], W),
        dot([GRID, -rail]),
    ];
    let [_, head] = if n { arrow([GRID, 0.0], [channel + 0.3, 0.0], 0.762) } else { arrow([channel, 0.0], [1.651, 0.0], 0.762) };
    g.push(head);
    g.extend(transistor_leads());
    g.extend(names_right(4.318));
    Shape { pins: transistor_pins(("G", "1"), ("D", "3"), ("S", "2")), graphics: g }
}

/// `count` LEDs stacked a grid step apart either side of the middle line, anodes left; with more
/// than one, the common pin's side is joined by a bus.
fn leds(count: usize, common_cathode: bool) -> Shape {
    let count = count.clamp(1, 4);
    if count == 1 {
        let mut g = vec![];
        g.extend(leads(-H, H));
        g.extend(diode_glyph(0.0, Bar::Plain));
        g.extend(light(0.0));
        g.extend(names(-GRID, 2.8, -H));
        return Shape { pins: vec![right("K", "1", true), left("A", "2", true)], graphics: g };
    }
    const ANODES: [&str; 4] = ["A1", "A2", "A3", "A4"];
    const CATHODES: [&str; 4] = ["K1", "K2", "K3", "K4"];
    let rows: Vec<f64> = (0..count).map(|i| (count - 1) as f64 * GRID - i as f64 * 2.0 * GRID).collect();
    let (top, bottom) = (rows.first().copied().unwrap_or(0.0), rows.last().copied().unwrap_or(0.0));
    let bus = if common_cathode { GRID } else { -GRID };
    let mut g = vec![seg([bus, top], [bus, bottom], W), dot([bus, 0.0])];
    let mut pins = vec![if common_cathode { right("K", "1", true) } else { left("A", "1", true) }];
    for (i, y) in rows.iter().copied().enumerate() {
        g.push(seg([-GRID, y], [-H, y], W));
        g.push(seg([H, y], [GRID, y], W));
        g.extend(diode_glyph(y, Bar::Plain));
        g.extend(light(y));
        if i > 0 && i + 1 < count && y != 0.0 {
            g.push(dot([bus, y]));
        }
        let number = (i + 2).to_string();
        pins.push(if common_cathode {
            pin(ANODES.get(i).copied().unwrap_or("A"), &number, true, [-2.0 * GRID, y], PinDirection::Right)
        } else {
            pin(CATHODES.get(i).copied().unwrap_or("K"), &number, true, [2.0 * GRID, y], PinDirection::Left)
        });
    }
    g.extend(names(-GRID, top + 2.8, bottom - H));
    Shape { pins, graphics: g }
}

/// The shape of `kind`.
pub(super) fn shape(kind: Kind) -> Shape {
    let two = || vec![left("1", "1", false), right("2", "2", false)];
    let polar = |a: &'static str, k: &'static str| vec![right(k, "1", true), left(a, "2", true)];
    match kind {
        Kind::Resistor | Kind::Capacitor => {
            let (pins, graphics) = two_terminal(kind == Kind::Capacitor);
            let pins = pins
                .into_iter()
                .map(|p| ShapePin { role: if p.number == "1" { "1" } else { "2" }, number: p.number, name: p.name, at: p.at, direction: p.direction })
                .collect();
            Shape { pins, graphics }
        }
        Kind::PolarizedCapacitor => {
            let mut g = vec![seg([-0.508, -1.524], [-0.508, 1.524], PLATE)];
            // The minus plate bows toward the plus one: an arc whose middle is 0.508 right of
            // the middle line's centre and whose ends reach the plus plate's height.
            let r = 2.794;
            g.push(arc([0.508 + r, 0.0], r, 147.0, 66.0, PLATE));
            g.extend(leads(-0.508, 0.508));
            g.extend(plus([-1.778, 1.27]));
            g.extend(names(-GRID, 1.524, -1.524));
            Shape { pins: vec![left("+", "1", true), right("-", "2", true)], graphics: g }
        }
        Kind::Inductor => {
            let r = GRID / 4.0;
            let mut g: Vec<Graphic> = (0..4).map(|i| arc([-GRID + r + 2.0 * r * f64::from(i), 0.0], r, 180.0, -180.0, W)).collect();
            g.extend(names(-GRID, r + 0.3, -0.5));
            Shape { pins: two(), graphics: g }
        }
        Kind::Ferrite => {
            // A filled bar across the line, slanting up to the right.
            let (c, s) = (60_f64.to_radians().cos(), 60_f64.to_radians().sin());
            let (l, w) = (1.27, 0.381);
            let corner = |a: f64, b: f64| [a * l * c - b * w * s, a * l * s + b * w * c];
            let mut g = vec![seg([-GRID, 0.0], [GRID, 0.0], W)];
            g.push(polygon(vec![corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)], true));
            g.extend(names(-GRID, 1.4, -1.4));
            Shape { pins: two(), graphics: g }
        }
        Kind::Fuse => {
            let mut g = vec![graphic_rect([-1.905, -0.762], [1.905, 0.762]), seg([-GRID, 0.0], [GRID, 0.0], W)];
            g.extend(names(-GRID, 0.762, -0.762));
            Shape { pins: two(), graphics: g }
        }
        Kind::Diode | Kind::Zener | Kind::Schottky | Kind::Tvs { bidirectional: false } => {
            let kind = match kind {
                Kind::Zener | Kind::Tvs { .. } => Bar::Zener,
                Kind::Schottky => Bar::Schottky,
                _ => Bar::Plain,
            };
            let mut g = leads(-H, H).to_vec();
            g.extend(diode_glyph(0.0, kind));
            g.extend(names(-GRID, H, -H));
            Shape { pins: polar("A", "K"), graphics: g }
        }
        Kind::Tvs { bidirectional: true } => {
            // Two diodes point to point through a Zener bar in the middle.
            let x = 2.286;
            let mut g = leads(-x, x).to_vec();
            g.push(polygon(vec![[-x, H], [-x, -H], [0.0, 0.0]], false));
            g.push(polygon(vec![[x, H], [x, -H], [0.0, 0.0]], false));
            g.push(bar(0.0, 0.0, Bar::Zener));
            g.extend(names(-GRID, H, -H));
            Shape { pins: two(), graphics: g }
        }
        Kind::Led { count, common_cathode } => leds(count, common_cathode),
        Kind::Crystal { case: false } => {
            let mut g = quartz(GRID);
            g.extend(names(-GRID, 1.524, -1.524));
            Shape { pins: two(), graphics: g }
        }
        Kind::Crystal { case: true } => {
            // The quartz in its case, the two case pins down from the case's bottom. The case is an
            // outline, not a rectangle, which pages fill as a body and would hide the quartz.
            let half = 1.5 * GRID;
            let mut g = quartz(2.0 * GRID);
            let case = vec![[-half, -GRID], [half, -GRID], [half, GRID], [-half, GRID]];
            g.push(Graphic { width: THIN, ..polygon(case, false) });
            g.extend(names_above(-half, GRID));
            let pins = vec![
                pin("1", "1", false, [-3.0 * GRID, 0.0], PinDirection::Right),
                pin("2", "2", false, [-GRID, -2.0 * GRID], PinDirection::Up),
                pin("3", "3", false, [3.0 * GRID, 0.0], PinDirection::Left),
                pin("4", "4", false, [GRID, -2.0 * GRID], PinDirection::Up),
            ];
            Shape { pins, graphics: g }
        }
        Kind::Resonator => {
            // The quartz between pins 1 and 3, a load capacitor from each side down to a ground
            // bus on the middle pin.
            let x = 1.5 * GRID;
            let mut g = quartz(2.0 * GRID);
            for side in [-x, x] {
                g.push(seg([side, 0.0], [side, -0.889], W));
                g.push(seg([side - 0.762, -0.889], [side + 0.762, -0.889], PLATE));
                g.push(seg([side - 0.762, -1.651], [side + 0.762, -1.651], PLATE));
                g.push(seg([side, -1.651], [side, -GRID], W));
                g.push(dot([side, 0.0]));
            }
            g.push(seg([-x, -GRID], [x, -GRID], W));
            g.push(dot([0.0, -GRID]));
            g.extend(names_above(-x, 1.524));
            let pins = vec![
                pin("1", "1", false, [-3.0 * GRID, 0.0], PinDirection::Right),
                pin("2", "2", false, [0.0, -2.0 * GRID], PinDirection::Up),
                pin("3", "3", false, [3.0 * GRID, 0.0], PinDirection::Left),
            ];
            Shape { pins, graphics: g }
        }
        Kind::Switch { four: false } => {
            let mut g = button();
            g.extend(names(-GRID, 2.032, -0.5));
            Shape { pins: two(), graphics: g }
        }
        Kind::Switch { four: true } => {
            // Each side's two pins joined, as the part joins them inside.
            let mut g = button();
            for x in [-GRID, GRID] {
                g.push(seg([x, GRID], [x, -GRID], W));
                g.push(dot([x, 0.0]));
            }
            g.extend(names(-GRID, GRID + 0.3, -GRID - 0.3));
            let pins = vec![
                pin("1", "1", false, [-2.0 * GRID, GRID], PinDirection::Right),
                pin("2", "2", false, [-2.0 * GRID, -GRID], PinDirection::Right),
                pin("3", "3", false, [2.0 * GRID, GRID], PinDirection::Left),
                pin("4", "4", false, [2.0 * GRID, -GRID], PinDirection::Left),
            ];
            Shape { pins, graphics: g }
        }
        Kind::Battery => {
            // Two cells, each a long thin plate (plus) and a short thick one.
            let mut g = leads(-H, H).to_vec();
            for x in [-H, 0.508] {
                g.push(seg([x, -1.778], [x, 1.778], W));
                g.push(seg([x + 0.762, -0.889], [x + 0.762, 0.889], 0.508));
            }
            g.extend(plus([-2.032, 1.27]));
            g.extend(names(-GRID, 1.778, -1.778));
            Shape { pins: vec![left("+", "1", true), right("-", "2", true)], graphics: g }
        }
        Kind::Antenna => {
            let mut g = vec![seg([0.0, -GRID], [0.0, GRID], W), polygon(vec![[-1.524, GRID], [1.524, GRID], [0.0, 0.0]], false)];
            g.extend(names_right(2.032));
            Shape { pins: vec![pin("1", "1", false, [0.0, -2.0 * GRID], PinDirection::Up)], graphics: g }
        }
        Kind::TestPoint => {
            let mut g = vec![ring([0.0, 0.762], 0.762, W)];
            g.extend(names_right(1.524));
            Shape { pins: vec![pin("1", "1", false, [0.0, -GRID], PinDirection::Up)], graphics: g }
        }
        Kind::Npn => bjt(true),
        Kind::Pnp => bjt(false),
        Kind::Nmos => mosfet(true),
        Kind::Pmos => mosfet(false),
    }
}
