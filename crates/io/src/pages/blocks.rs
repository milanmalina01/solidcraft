//! The block diagram: a box per frame, laid on a tidy grid in the order the frames sit on the
//! sheet, and lines between frames that share nets, labelled by net or by bus (names sharing a
//! prefix: `QSPI_IO0`…`QSPI_CS` → "QSPI (6)"). A line gets an arrow only when the pins say who
//! drives: an output in one frame and only inputs in the other. Supply rails are not lines (one per
//! rail per box buried the signals): each box has a short labelled stub on its top edge per rail,
//! "VSYS → p. 4" for one it makes (a link to the first frame it feeds) and "+3V3 ← p. 4" for one
//! it takes (from the page of the frame that makes it), all in one form on the page, and a small
//! power tree in the bottom left corner shows the flow: a frame, the rails it makes, the frames
//! they feed, and so on. A rail no pin makes starts at the frame where a connector is on it (power
//! comes in there), else at the frame with the fewest power inputs on it, with no arrow. Ground is
//! left out, and so are parts not fitted (they make, take and bring in nothing).
//!
//! What a frame does with a rail it takes is read from its pins there (a series resistor or
//! inductor followed once to the pins beyond it): it draws power (a supply pin, an LED, a
//! connector), it only detects or senses it (a chip's `VBUS` pin where the chip makes no rail,
//! `…DET`, `SENSE`, `TS`, or a net the design's intent marks as sense; never a connector's pin), or
//! it only pulls signals up to it. Only a supply is a supply edge; the others are thin dashed lines
//! with their word ("VBUS (detect) ← p. 2"): not what powers the frame, though a detect input may
//! draw a little (the nRF52840's `VBUS` feeds its USB regulator). A battery's supply rail (`VBAT`,
//! `BAT`…) starts at the battery as a source of its own, "Battery (J3)": its cells, else the
//! connector its lead plugs into (one that says battery, or one with nothing but the rail and
//! ground on it; another header on the rail takes it like any frame). A charger's pin on it (a
//! power output, or a two-way pin named for the battery) is a two-headed edge: it charges the
//! battery and draws from it.

use std::collections::{BTreeMap, BTreeSet};

use solidcraft_doc::ecad::connect::natural_cmp;
use solidcraft_doc::ecad::intent::{Intent, NetRole};
use solidcraft_doc::ecad::schematic::Pins;
use solidcraft_doc::ecad::{Frame, LabelKind, PinType};

use super::MIN_TEXT;
use super::border::Sheet;
use super::page::{Align, Baseline, Item, Link, Page, Rgb, Stroke, Text};
use super::sheet::Ctx;

/// Text sizes: a box's title, its parts, a line's label, a rail stub's, the power tree's.
const TITLE: f64 = 3.4;
const DETAIL: f64 = 2.2;
const LABEL: f64 = 2.0;
const STUB: f64 = 1.9;
const TREE_TEXT: f64 = 1.9;
/// A rail stub's rise above its box, and the half width of its bar.
const STUB_RISE: f64 = 2.2;
const STUB_BAR: f64 = 1.1;
/// From a stub's left end to its text, and between stubs.
const STUB_LEAD: f64 = 2.5;
const STUB_GAP: f64 = 1.6;
/// The power tree: a node's height, the rows' pitch, the gap between columns, a node's padding
/// and widest, and the most rows it takes (the rest are counted in its caption).
const NODE_H: f64 = 3.6;
const PITCH: f64 = 4.6;
const COL_GAP: f64 = 7.0;
const NODE_PAD: f64 = 1.2;
const NODE_MAX: f64 = 46.0;
const MAX_TREE_ROWS: usize = 18;
const CAPTION_H: f64 = 5.0;
/// A dashed (detect, sense, pull-up) line's dash and gap, and the room its word takes besides its
/// width.
const SIGNAL_DASH: [f64; 2] = [0.9, 0.6];
const WORD_PAD: f64 = 2.4;
/// A resistor this small (in ohms) in series with a rail feeds what is beyond it; a larger one
/// with nothing beyond it in its frame pulls a signal up.
const SERIES_OHMS: f64 = 100.0;

/// One signal line between two frames.
struct Edge {
    a: u64,
    b: u64,
    label: String,
    /// `Some(true)`: `a` drives `b`; `Some(false)`: `b` drives `a`.
    drives: Option<bool>,
}

/// What a frame does with a supply rail it takes, as its pins there say; the strongest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Use {
    /// It charges the rail and draws from it: a charger's pins on a battery's rail.
    TwoWay,
    /// It draws power from it: a supply pin, an LED, a connector, a series part on to one.
    Supply,
    /// Only supply-detect inputs (a USB device's `VBUS` pin, `VBUS_DET`): not what powers the
    /// frame, though it may draw a little.
    Detect,
    /// Only sense inputs (`SENSE`, `TS`, `NTC`), or a net the intent marks as sense.
    Sense,
    /// Only pull-ups to signals, or inputs tied to it: a logic level, no supply.
    PullUp,
}

impl Use {
    /// The word on its line in the power tree and on its stub; none for a supply.
    fn word(self) -> Option<&'static str> {
        match self {
            Use::Detect => Some("detect"),
            Use::Sense => Some("sense"),
            Use::PullUp => Some("pull-up"),
            Use::TwoWay | Use::Supply => None,
        }
    }

    /// Whether the frame takes its power from it (so the tree grows on from the frame).
    fn powers(self) -> bool {
        matches!(self, Use::TwoWay | Use::Supply)
    }
}

/// A supply rail between frames: from the frame that makes it (else where it comes in) to the
/// frames that take it, with what each does with it, in print order.
struct Rail {
    name: String,
    source: u64,
    /// Which way it flows is known: an output pin makes it, it comes in on a connector, or a
    /// battery holds it up (arrows into the frames it feeds).
    made: bool,
    takers: Vec<(u64, Use)>,
    /// A battery's rail: the battery's designators (`J3`); the battery is its source, and its
    /// takers may include the battery's own frame (a charger beside it).
    battery: Option<String>,
}

impl Rail {
    /// What frame `id` does with it, if it takes it.
    fn use_by(&self, id: u64) -> Option<Use> {
        self.takers.iter().find(|(t, _)| *t == id).map(|(_, u)| *u)
    }

    /// The frames it reaches beyond its source's own.
    fn others(&self) -> impl Iterator<Item = &(u64, Use)> {
        self.takers.iter().filter(move |(t, _)| *t != self.source)
    }
}

/// A node of the power tree: a frame, a rail out of the node before it, or a battery.
#[derive(Clone, Copy)]
enum Node {
    Frame(u64),
    Rail(usize),
    /// The battery of rail `k`, a root of the tree: `Battery (J3)`.
    Battery(usize),
}

/// A node in the tree: its column, its row, the node it hangs from and how (a frame's use of the
/// rail it hangs from; a supply otherwise).
#[derive(Clone, Copy)]
struct Placed {
    node: Node,
    col: usize,
    row: usize,
    parent: Option<usize>,
    link: Use,
}

/// The power tree laid out in columns (a frame, its rails, the frames they feed, their rails…)
/// and rows (one per frame fed, a node on the row of its first child).
#[derive(Default)]
struct Tree {
    nodes: Vec<Placed>,
    rows: usize,
    /// Frames fed that are left out past [`MAX_TREE_ROWS`].
    hidden: usize,
}

impl Tree {
    fn push(&mut self, node: Node, col: usize, parent: Option<usize>, link: Use) -> usize {
        self.nodes.push(Placed { node, col, row: self.rows, parent, link });
        self.nodes.len() - 1
    }

    /// A new row of its own.
    fn next_row(&mut self) -> usize {
        self.rows += 1;
        self.rows - 1
    }

    fn set_row(&mut self, node: usize, row: usize) {
        if let Some(n) = self.nodes.get_mut(node) {
            n.row = row;
        }
    }
}

/// Whether a net name is drawn as ground.
fn is_ground(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.starts_with("GND") || n.starts_with("VSS") || n == "0V" || n.starts_with("AGND") || n.starts_with("DGND")
}

/// A designator's letters, upper case (`FB` of `FB2`).
fn prefix(designator: &str) -> String {
    designator.chars().take_while(char::is_ascii_alphabetic).collect::<String>().to_ascii_uppercase()
}

/// Whether a designator's prefix is a connector's or a battery's (`J1`, `P2`, `CN3`, `BT1`): where
/// a supply comes onto the board.
fn is_entry(designator: &str) -> bool {
    matches!(prefix(designator).as_str(), "J" | "P" | "CN" | "X" | "BT" | "BAT" | "USB")
}

/// Whether a designator's prefix is a cell's (`BT1`).
fn is_cell(designator: &str) -> bool {
    matches!(prefix(designator).as_str(), "BT" | "BAT")
}

/// Whether a net or pin name is a battery's: `VBAT`, `BAT`, `BATT`, `VBATT`, `BAT+`, `+VBAT`,
/// `V_BAT`, `VBAT_…` (a net only when it is a supply: `BAT_TS` on a pack's third wire is a signal).
fn is_battery(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    let n = n.trim_start_matches('+').trim_end_matches('+').replace("V_BAT", "VBAT");
    matches!(n.as_str(), "VBAT" | "BAT" | "BATT" | "VBATT" | "BATTERY") || n.starts_with("VBAT_") || n.starts_with("BAT_")
}

/// A pin name, plain and upper case, without the overbar markup.
fn pin_name(name: &str) -> String {
    solidcraft_doc::ecad::overbar::plain(name).trim().to_ascii_uppercase()
}

/// A supply-detect input's pin name: `VBUS` (on a chip that makes no rail: a USB device's),
/// `VBUS_DET`, `USB_DET`.
fn detect_pin(name: &str) -> bool {
    name == "VBUS" || name.contains("DET")
}

/// A sense input's pin name: `SENSE`, `VSNS`, `TS`, `NTC`, `THERM`.
fn sense_pin(name: &str) -> bool {
    name.contains("SENSE") || name.contains("SNS") || name == "TS" || name.contains("NTC") || name.contains("THERM")
}

/// What the parts a rail reaches say about the frames that take it.
struct Look<'a, 'b> {
    c: &'a Ctx<'b>,
    membership: &'a BTreeMap<u64, u64>,
    /// Each part's nets.
    part_nets: &'a BTreeMap<u64, Vec<usize>>,
    /// Chips with a power output on a supply net: their `VBUS` pin is a supply, not a detect input.
    feeders: &'a BTreeSet<u64>,
    /// Nets the intent marks as sense, by name.
    sense: &'a BTreeSet<&'a str>,
    ground: &'a BTreeSet<usize>,
}

impl Look<'_, '_> {
    /// What frame `f` does with rail `net` (its parts in `skip` left out: a battery's own), the
    /// strongest use its pins there show; a frame with nothing but capacitors on it takes it as a
    /// supply.
    fn use_of(&self, net: usize, f: u64, skip: &BTreeSet<u64>, battery: bool) -> Use {
        let mut found = BTreeSet::new();
        self.look(net, f, skip, battery, true, &mut found);
        found.first().copied().unwrap_or(Use::Supply)
    }

    /// The uses frame `f`'s pins on `net` show (`first`: a series part is followed on, once).
    fn look(&self, net: usize, f: u64, skip: &BTreeSet<u64>, battery: bool, first: bool, found: &mut BTreeSet<Use>) {
        let Some(n) = self.c.conn.nets.get(net) else { return };
        if self.sense.contains(n.name.as_str()) {
            found.insert(Use::Sense);
            return;
        }
        for p in &n.pins {
            if self.membership.get(&p.part) != Some(&f) || skip.contains(&p.part) {
                continue;
            }
            let Some(part) = self.c.sheet.part(p.part) else { continue };
            // A part not fitted draws nothing.
            if part.dnp {
                continue;
            }
            let pc = self.c.library.get(&part.component);
            let two = pc.is_some_and(|pc| pc.symbol.pins.len() == 2);
            if two && p.electrical == PinType::Passive {
                let head = prefix(&p.designator);
                // A capacitor holds the rail up and draws nothing of its own; a diode or an LED
                // draws from it.
                if head == "C" {
                    continue;
                }
                if !matches!(head.as_str(), "R" | "L" | "FB" | "F") {
                    found.insert(Use::Supply);
                    continue;
                }
                // A resistor, an inductor, a ferrite or a fuse in series: what is beyond it.
                let beyond = self.part_nets.get(&p.part).and_then(|ns| ns.iter().copied().find(|o| *o != net));
                let mut there = BTreeSet::new();
                if let Some(o) = beyond.filter(|o| first && !self.ground.contains(o)) {
                    if self.c.conn.nets.get(o).is_some_and(|x| x.power) {
                        // On to another rail: it feeds it.
                        there.insert(Use::Supply);
                    } else {
                        let mut past = skip.clone();
                        past.insert(p.part);
                        self.look(o, f, &past, battery, false, &mut there);
                    }
                }
                if there.is_empty() {
                    let value = part.value.as_deref().or(pc.map(|pc| pc.component.value.as_str())).unwrap_or_default();
                    let ohms = solidcraft_doc::ecad::infer::parse_value(value).map(|(v, _)| v);
                    let pull = head == "R" && ohms.is_none_or(|v| v >= SERIES_OHMS);
                    there.insert(if pull { Use::PullUp } else { Use::Supply });
                }
                found.extend(there);
                continue;
            }
            // A pin of a chip, a module or a connector. A connector's pin carries the rail on, as
            // a supply, whatever its name (a USB host port's `VBUS` gives power).
            let name = pin_name(&p.name);
            let u = if is_entry(&p.designator) {
                Use::Supply
            } else if detect_pin(&name) && !self.feeders.contains(&p.part) {
                Use::Detect
            } else if sense_pin(&name) {
                Use::Sense
            } else if battery && first && (p.electrical == PinType::PowerOutput || (p.electrical == PinType::Bidirectional && is_battery(&name))) {
                // A charger's pin right on the battery's rail (never past a resistor: a divider to
                // an ADC input only reads it).
                Use::TwoWay
            } else {
                match p.electrical {
                    PinType::Input | PinType::Output | PinType::Bidirectional | PinType::TriState | PinType::OpenCollector | PinType::OpenEmitter => {
                        Use::PullUp
                    }
                    _ => Use::Supply,
                }
            };
            found.insert(u);
        }
    }
}

/// A bus key: the name up to its first `_`, else its letters before trailing digits.
fn bus_key(name: &str) -> String {
    match name.split_once('_') {
        Some((head, _)) if !head.is_empty() => head.to_string(),
        _ => name.trim_end_matches(|c: char| c.is_ascii_digit()).to_string(),
    }
}

/// "QSPI (6), SDA, SCL": nets grouped by bus where two or more share a key.
fn summary(nets: &[String]) -> String {
    let mut groups: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    for n in nets {
        groups.entry(bus_key(n)).or_default().push(n);
    }
    let mut parts: Vec<(usize, String)> = Vec::new();
    for (key, members) in groups {
        if members.len() >= 2 && !key.is_empty() {
            parts.push((members.len(), format!("{key} ({})", members.len())));
        } else {
            parts.extend(members.into_iter().map(|m| (1, m.clone())));
        }
    }
    parts.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let shown: Vec<String> = parts.iter().take(3).map(|p| p.1.clone()).collect();
    let mut s = shown.join(", ");
    if parts.len() > 3 {
        s.push_str(&format!(" +{}", parts.len() - 3));
    }
    s
}

/// Cluster values into groups no wider than `gap` from each group's first: the group of each.
fn cluster(values: &[(u64, f64)], gap: f64, descending: bool) -> BTreeMap<u64, usize> {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| if descending { b.1.total_cmp(&a.1) } else { a.1.total_cmp(&b.1) });
    let mut out = BTreeMap::new();
    let mut group = 0usize;
    let mut first: Option<f64> = None;
    for (id, v) in sorted {
        match first {
            Some(f) if (v - f).abs() <= gap => {}
            Some(_) => {
                group += 1;
                first = Some(v);
            }
            None => first = Some(v),
        }
        out.insert(id, group);
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    c: &Ctx,
    page: &mut Page,
    geometry: &Sheet,
    frames: &[&Frame],
    membership: &BTreeMap<u64, u64>,
    section_page: &BTreeMap<u64, usize>,
    intent: &Intent,
) {
    let pal = c.pal;
    let pins = Pins::new(c.sheet, c.library);
    // Each net's frames, and its driving and driven frames.
    let frame_of = |p: [f64; 2]| frames.iter().find(|f| f.contains(p)).map(|f| f.id);
    let mut ground_nets: BTreeSet<usize> = BTreeSet::new();
    let mut net_frames: BTreeMap<usize, BTreeSet<u64>> = BTreeMap::new();
    for (l, net) in c.sheet.labels.iter().zip(&c.conn.label_nets) {
        if l.kind == LabelKind::Ground || is_ground(&l.name) {
            ground_nets.insert(*net);
        }
        if let Some(f) = pins.anchor(&l.at).and_then(frame_of) {
            net_frames.entry(*net).or_default().insert(f);
        }
    }
    // A part not fitted still joins frames by its nets, but drives, takes and brings in nothing.
    let fitted = |part: u64| c.sheet.part(part).is_some_and(|q| !q.dnp);
    let mut outputs: BTreeMap<usize, BTreeSet<u64>> = BTreeMap::new();
    let mut inputs: BTreeMap<usize, BTreeSet<u64>> = BTreeMap::new();
    let mut power_inputs: BTreeMap<(usize, u64), usize> = BTreeMap::new();
    let mut entries: BTreeSet<(usize, u64)> = BTreeSet::new();
    // Each net's frames with a fitted part on it: the frames a rail can reach.
    let mut power_frames: BTreeMap<usize, BTreeSet<u64>> = BTreeMap::new();
    for (k, n) in c.conn.nets.iter().enumerate() {
        if is_ground(&n.name) {
            ground_nets.insert(k);
        }
        for p in &n.pins {
            let Some(f) = membership.get(&p.part) else { continue };
            net_frames.entry(k).or_default().insert(*f);
            if !fitted(p.part) {
                continue;
            }
            power_frames.entry(k).or_default().insert(*f);
            // A connector brings power in; an `X` that is an oscillator (its pin there a supply
            // input) does not.
            if is_entry(&p.designator) && !(prefix(&p.designator) == "X" && p.electrical == PinType::PowerInput) {
                entries.insert((k, *f));
            }
            match p.electrical {
                PinType::Output | PinType::PowerOutput => {
                    outputs.entry(k).or_default().insert(*f);
                }
                PinType::Input | PinType::PowerInput => {
                    inputs.entry(k).or_default().insert(*f);
                    if p.electrical == PinType::PowerInput {
                        *power_inputs.entry((k, *f)).or_default() += 1;
                    }
                }
                _ => {}
            }
        }
    }
    // What the frames on a rail do with it.
    let mut part_nets: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (k, n) in c.conn.nets.iter().enumerate() {
        for p in &n.pins {
            part_nets.entry(p.part).or_default().push(k);
        }
    }
    let feeders: BTreeSet<u64> = c
        .conn
        .nets
        .iter()
        .filter(|n| n.power && !is_ground(&n.name))
        .flat_map(|n| n.pins.iter().filter(|p| p.electrical == PinType::PowerOutput).map(|p| p.part))
        .collect();
    let sense: BTreeSet<&str> = intent.nets.iter().filter(|n| n.role == NetRole::Sense).map(|n| n.net.as_str()).collect();
    let look = Look { c, membership, part_nets: &part_nets, feeders: &feeders, sense: &sense, ground: &ground_nets };
    let none = BTreeSet::new();
    // The lines: signals per pair of frames; the rails, from the frame that makes them.
    let mut signals: BTreeMap<(u64, u64), Vec<usize>> = BTreeMap::new();
    let mut edges: Vec<Edge> = Vec::new();
    let mut rails: Vec<Rail> = Vec::new();
    let order = |id: u64| frames.iter().position(|f| f.id == id).unwrap_or(usize::MAX);
    for (k, fs) in &net_frames {
        if ground_nets.contains(k) {
            continue;
        }
        let Some(net) = c.conn.nets.get(*k) else { continue };
        // A battery's supply rail: from the battery to each frame with something else fitted on
        // it, the battery's own too (a charger beside it). The battery is its cells, else the
        // connectors its lead plugs into: one that says battery, or one with nothing but this rail
        // and ground on it (a header that brings the battery out to other boards is a taker).
        if net.power && is_battery(&net.name) {
            let on: Vec<&solidcraft_doc::ecad::NetPin> =
                net.pins.iter().filter(|p| is_entry(&p.designator) && membership.contains_key(&p.part) && fitted(p.part)).collect();
            let says = |part: u64| {
                c.sheet.part(part).and_then(|q| c.library.get(&q.component)).is_some_and(|pc| {
                    let x = &pc.component;
                    [&x.name, &x.value, &x.description].iter().any(|t| t.to_ascii_lowercase().contains("batter"))
                })
            };
            let lead = |part: u64| says(part) || part_nets.get(&part).is_some_and(|ns| ns.iter().all(|o| o == k || ground_nets.contains(o)));
            let cells: Vec<&solidcraft_doc::ecad::NetPin> = if on.iter().any(|p| is_cell(&p.designator)) {
                on.into_iter().filter(|p| is_cell(&p.designator)).collect()
            } else {
                on.into_iter().filter(|p| lead(p.part)).collect()
            };
            if let Some(source) = cells.first().and_then(|p| membership.get(&p.part)).copied() {
                let held: BTreeSet<u64> = cells.iter().map(|p| p.part).collect();
                let mut names: Vec<&str> = cells.iter().map(|p| p.designator.as_str()).collect();
                names.sort_by(|a, b| natural_cmp(a, b));
                names.dedup();
                let mut takers: Vec<(u64, Use)> = fs
                    .iter()
                    .copied()
                    .filter(|f| net.pins.iter().any(|p| membership.get(&p.part) == Some(f) && !held.contains(&p.part) && fitted(p.part)))
                    .map(|f| (f, look.use_of(*k, f, &held, true)))
                    .collect();
                takers.sort_by_key(|(f, _)| order(*f));
                if !takers.is_empty() {
                    rails.push(Rail { name: net.name.clone(), source, made: true, takers, battery: Some(names.join(", ")) });
                }
                continue;
            }
        }
        if fs.len() < 2 {
            continue;
        }
        if net.power {
            // Only frames with a fitted part on it: a debug header not fitted draws nothing.
            let Some(fs) = power_frames.get(k).filter(|f| f.len() >= 2) else { continue };
            let maker = outputs.get(k).and_then(|o| o.iter().find(|f| fs.contains(f)).copied());
            // A rail no pin makes comes in on a connector or a battery: from the frame that has one
            // on it, else the one that takes the least of it (the first in print order of those),
            // with no arrow.
            let fewest = || {
                fs.iter()
                    .min_by_key(|f| {
                        let key = (*k, **f);
                        (!entries.contains(&key), power_inputs.get(&key).copied().unwrap_or(0), frames.iter().position(|x| x.id == **f))
                    })
                    .copied()
            };
            let (source, made) = match maker {
                Some(f) => (f, true),
                // Power comes in where the connector is.
                None => match fewest() {
                    Some(f) => (f, entries.contains(&(*k, f))),
                    None => continue,
                },
            };
            let mut takers: Vec<(u64, Use)> = fs.iter().copied().filter(|f| *f != source).map(|f| (f, look.use_of(*k, f, &none, false))).collect();
            takers.sort_by_key(|(f, _)| order(*f));
            rails.push(Rail { name: net.name.clone(), source, made, takers, battery: None });
            continue;
        }
        let list: Vec<u64> = fs.iter().copied().collect();
        for (i, a) in list.iter().enumerate() {
            for b in list.iter().skip(i + 1) {
                signals.entry((*a, *b)).or_default().push(*k);
            }
        }
    }
    for ((a, b), nets) in signals {
        let names: Vec<String> =
            nets.iter().filter_map(|k| c.conn.nets.get(*k)).map(|n| if n.named { n.name.clone() } else { "net".into() }).collect();
        let drives = |from: u64, to: u64| {
            nets.iter()
                .all(|k| outputs.get(k).is_some_and(|o| o.contains(&from) && !o.contains(&to)) && inputs.get(k).is_some_and(|i| i.contains(&to)))
        };
        let d = if drives(a, b) {
            Some(true)
        } else if drives(b, a) {
            Some(false)
        } else {
            None
        };
        let unnamed = names.iter().filter(|n| *n == "net").count();
        let mut named: Vec<String> = names.into_iter().filter(|n| n != "net").collect();
        if unnamed > 0 {
            named.push(if unnamed == 1 { "1 net".into() } else { format!("{unnamed} nets") });
        }
        edges.push(Edge { a, b, label: summary(&named), drives: d });
    }
    rails.sort_by(|x, y| order(x.source).cmp(&order(y.source)).then_with(|| solidcraft_doc::ecad::connect::natural_cmp(&x.name, &y.name)));

    // Each signal line runs from the busier box to the quieter one, where its label goes.
    let mut degree: BTreeMap<u64, usize> = BTreeMap::new();
    for e in &edges {
        *degree.entry(e.a).or_default() += 1;
        *degree.entry(e.b).or_default() += 1;
    }
    for e in &mut edges {
        let busy = |id: u64| degree.get(&id).copied().unwrap_or(0);
        if busy(e.b) > busy(e.a) {
            std::mem::swap(&mut e.a, &mut e.b);
            e.drives = e.drives.map(|d| !d);
        }
    }

    // The grid: rows and columns from where the frames sit.
    let heights: Vec<f64> = frames.iter().map(|f| f.rect[1][1] - f.rect[0][1]).collect();
    let widths: Vec<f64> = frames.iter().map(|f| f.rect[1][0] - f.rect[0][0]).collect();
    let median = |v: &[f64]| {
        let mut v = v.to_vec();
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(50.0)
    };
    let rows = cluster(&frames.iter().map(|f| (f.id, f.middle()[1])).collect::<Vec<_>>(), median(&heights) * 0.7, true);
    let cols = cluster(&frames.iter().map(|f| (f.id, f.middle()[0])).collect::<Vec<_>>(), median(&widths) * 0.5, false);
    let mut cell: BTreeMap<u64, (usize, usize)> = BTreeMap::new();
    let mut taken: BTreeSet<(usize, usize)> = BTreeSet::new();
    for f in frames {
        let (mut col, row) = (cols.get(&f.id).copied().unwrap_or(0), rows.get(&f.id).copied().unwrap_or(0));
        while taken.contains(&(col, row)) {
            col += 1;
        }
        taken.insert((col, row));
        cell.insert(f.id, (col, row));
    }
    let ncols = cell.values().map(|c| c.0 + 1).max().unwrap_or(1);
    let nrows = cell.values().map(|c| c.1 + 1).max().unwrap_or(1);
    // Room: the drawing area above the title block and the power tree, under the legend. The tree
    // goes in the bottom left corner, beside the title block when it fits there.
    let [a0, a1] = geometry.area;
    let tb = geometry.title_block;
    let page_of = |id: u64| section_page.get(&id).copied();
    let title_of = |id: u64| frames.iter().find(|f| f.id == id).map_or(String::new(), |f| f.title.clone());
    let tree = power_tree(&rails, frames);
    let tree_size = tree_size(c, &tree, &rails, &title_of, &page_of);
    let tree_at = tree_size.as_ref().map(|(_, size)| {
        let beside = size[0] <= tb[0][0] - 6.0 - a0[0];
        let bottom = if beside { a1[1] } else { tb[0][1] - 4.0 };
        [a0[0], bottom - size[1]]
    });
    // Under the legend: two lines, three with the power tree's.
    let top = a0[1] + if tree.nodes.is_empty() { 10.0 } else { 14.0 };
    let bottom = tree_at.map_or(tb[0][1], |at| tb[0][1].min(at[1])) - 4.0;
    let (cw, rh) = ((a1[0] - a0[0]) / ncols as f64, (bottom - top) / nrows as f64);
    let (bw, bh) = ((cw * 0.56).min(66.0), (rh * 0.52).min(30.0));
    let boxes: BTreeMap<u64, [[f64; 2]; 2]> = cell
        .iter()
        .map(|(id, (col, row))| {
            let cx = a0[0] + cw * (*col as f64 + 0.5);
            let cy = top + rh * (*row as f64 + 0.5);
            (*id, [[cx - bw / 2.0, cy - bh / 2.0], [cx + bw / 2.0, cy + bh / 2.0]])
        })
        .collect();
    // The gutters between rows and between columns: lines run there, never through a box.
    let gutter_x = |k: usize| a0[0] + cw * k as f64;
    let gutter_y = |j: usize| top + rh * j as f64;

    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
    enum Side {
        Left,
        Right,
        Top,
        Bottom,
    }
    /// How a line runs: straight between facing sides of neighbours, or out of the first box
    /// into the row gutter, along it to the column gutter by the second box, and in from the side.
    #[derive(Clone, Copy)]
    enum Way {
        Straight(Side, Side),
        Gutters { exit: Side, row_gutter: usize, col_gutter: usize, entry: Side },
    }
    let mut ways: Vec<Option<Way>> = Vec::new();
    for e in &edges {
        let (Some(&(ca, ra)), Some(&(cb, rb))) = (cell.get(&e.a), cell.get(&e.b)) else {
            ways.push(None);
            continue;
        };
        let way = if ra == rb && ca.abs_diff(cb) == 1 {
            if cb > ca { Way::Straight(Side::Right, Side::Left) } else { Way::Straight(Side::Left, Side::Right) }
        } else if ca == cb && ra.abs_diff(rb) == 1 {
            if rb > ra { Way::Straight(Side::Bottom, Side::Top) } else { Way::Straight(Side::Top, Side::Bottom) }
        } else {
            let (exit, row_gutter) = if rb > ra { (Side::Bottom, ra + 1) } else { (Side::Top, ra) };
            let (entry, col_gutter) = if cb > ca { (Side::Left, cb) } else { (Side::Right, cb + 1) };
            Way::Gutters { exit, row_gutter, col_gutter, entry }
        };
        ways.push(Some(way));
    }
    // Tracks: each line its own lane in every gutter it runs along.
    let mut row_lanes: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut col_lanes: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, w) in ways.iter().enumerate() {
        if let Some(Way::Gutters { row_gutter, col_gutter, .. }) = w {
            row_lanes.entry(*row_gutter).or_default().push(i);
            col_lanes.entry(*col_gutter).or_default().push(i);
        }
    }
    let lane = |lanes: &BTreeMap<usize, Vec<usize>>, g: usize, i: usize, room: f64| {
        let list = lanes.get(&g).map(Vec::as_slice).unwrap_or(&[]);
        let n = list.len().max(1) as f64;
        let k = list.iter().position(|x| *x == i).unwrap_or(0) as f64;
        let step = (room * 0.8 / n).min(2.6);
        (k - (n - 1.0) / 2.0) * step
    };
    // Each box's rail stubs along its top edge, ending at its right: the rails it makes, then the
    // ones it takes. Its top side's signal lines keep left of them; with none, they may take
    // nearly the whole edge.
    let mut top_used: BTreeSet<u64> = BTreeSet::new();
    for (e, w) in edges.iter().zip(&ways) {
        match w {
            Some(Way::Straight(Side::Top, _)) | Some(Way::Gutters { exit: Side::Top, .. }) => {
                top_used.insert(e.a);
            }
            Some(Way::Straight(Side::Bottom, Side::Top)) => {
                top_used.insert(e.b);
            }
            _ => {}
        }
    }
    // The rail, what follows it, the page it links to, and whether the frame takes no power from
    // it (it only detects, senses or pulls up to it).
    type Stub = (String, String, Option<usize>, bool);
    let mut stubs = Vec::new();
    let mut room: BTreeMap<u64, f64> = BTreeMap::new();
    let mut lists: Vec<(u64, [[f64; 2]; 2], Vec<Stub>, f64)> = Vec::new();
    for f in frames {
        let Some(b) = boxes.get(&f.id) else { continue };
        let mut list: Vec<Stub> = Vec::new();
        for r in rails.iter().filter(|r| r.source == f.id) {
            let takers: Vec<u64> = r.others().map(|(t, _)| *t).collect();
            if takers.is_empty() {
                continue;
            }
            let pages: Vec<String> = takers.iter().filter_map(|t| page_of(*t)).map(|n| (n + 1).to_string()).collect();
            let rest = if pages.len() == takers.len() && pages.len() <= 2 {
                format!(" → p. {}", pages.join(", "))
            } else {
                format!(" → {} frames", takers.len())
            };
            // A link to the first frame it feeds.
            list.push((r.name.clone(), rest, takers.iter().find_map(|t| page_of(*t)), false));
        }
        for r in rails.iter().filter(|r| r.source != f.id) {
            let Some(u) = r.use_by(f.id) else { continue };
            let arrow = if u == Use::TwoWay { "↔" } else { "←" };
            let word = u.word().map_or(String::new(), |w| format!(" ({w})"));
            let rest = match page_of(r.source) {
                Some(n) => format!("{word} {arrow} p. {}", n + 1),
                None => format!("{word} {arrow} {}", c.font.fit(&title_of(r.source), STUB, 24.0)),
            };
            list.push((r.name.clone(), rest, page_of(r.source), !u.powers()));
        }
        if list.is_empty() {
            continue;
        }
        // The rails it takes power from before those it only detects, senses or pulls up to, so
        // those give way first.
        list.sort_by_key(|s| s.3);
        let avail = (b[1][0] - b[0][0]) * if top_used.contains(&f.id) { 0.75 } else { 0.95 };
        lists.push((f.id, *b, list, avail));
    }
    let width = |s: &Stub, size: f64| STUB_LEAD + c.font.width(&s.0, size) + c.font.width(&s.1, size);
    let total = |l: &[Stub], size: f64| l.iter().map(|s| width(s, size)).sum::<f64>() + STUB_GAP * l.len().saturating_sub(1) as f64;
    let closer = |s: &mut Stub| s.1 = s.1.replace("→ ", "→").replace("← ", "←").replace("↔ ", "↔").replace("p. ", "p.").replace(", ", ",");
    // One form on the whole page: where a box's stubs are too long, every box's pages are written
    // closer ("→p.4"), then smaller (to the least text size); past that a box's last rails give way
    // to a "+n" (the power tree has them all). A rail never loses where it goes or comes from.
    let fits = |size: f64, close: bool| {
        lists.iter().all(|(_, _, l, avail)| {
            let mut l = l.clone();
            if close {
                l.iter_mut().for_each(closer);
            }
            total(&l, size) <= *avail
        })
    };
    let (size, close) = if fits(STUB, false) {
        (STUB, false)
    } else if fits(STUB, true) {
        (STUB, true)
    } else {
        (MIN_TEXT, true)
    };
    // What the stubs show, for their legend: a two-way rail, a dashed one, a "+n".
    let (mut two_way, mut dashed, mut hidden) = (false, false, false);
    let more_width = |n: usize| if n > 0 { STUB_GAP + c.font.width(&format!("+{n}"), size) } else { 0.0 };
    for (id, b, mut list, avail) in lists {
        if close {
            list.iter_mut().for_each(closer);
        }
        let mut more = 0usize;
        while list.len() > 1 && total(&list, size) + more_width(more) > avail {
            list.pop();
            more += 1;
        }
        two_way |= list.iter().any(|s| s.1.contains('↔'));
        dashed |= list.iter().any(|s| s.3);
        hidden |= more > 0;
        let y = b[0][1];
        let mut x = b[1][0] - 1.5 - total(&list, size) - more_width(more);
        room.insert(id, x - 2.0);
        for s in &list {
            let sx = x + STUB_BAR;
            let ty = y - STUB_RISE;
            // A rail the frame takes no power from: a thin dashed stem, as in the power tree.
            let (line, bar) = if s.3 {
                (Stroke::dashed(0.22, pal.power, [0.6, 0.45]), Stroke::solid(0.3, pal.power))
            } else {
                (Stroke::solid(0.3, pal.power), Stroke::solid(0.45, pal.power))
            };
            stubs.push(Item::Line { points: vec![[sx, y], [sx, ty]], closed: false, stroke: line });
            stubs.push(Item::Line { points: vec![[sx - STUB_BAR, ty], [sx + STUB_BAR, ty]], closed: false, stroke: bar });
            let tx = x + STUB_LEAD;
            let nw = c.font.width(&s.0, size);
            let rw = c.font.width(&s.1, size);
            stubs.push(Item::Rect {
                rect: [[tx - 0.4, ty - size * 0.75], [tx + nw + rw + 0.4, ty + size * 0.55]],
                radius: 0.4,
                fill: Some(pal.background),
                stroke: None,
            });
            stubs.push(Item::Text(Text::new([tx, ty], s.0.clone(), size, pal.power).align(Align::Start, Baseline::Middle).bold()));
            let rest_colour = if s.2.is_some() { pal.link } else { pal.dim };
            stubs.push(Item::Text(Text::new([tx + nw, ty], s.1.clone(), size, rest_colour).align(Align::Start, Baseline::Middle)));
            if let Some(n) = s.2 {
                page.links.push(Link { rect: [[tx, ty - size * 0.75], [tx + nw + rw, ty + size * 0.55]], page: n, at: None });
            }
            x += width(s, size) + STUB_GAP;
        }
        if more > 0 {
            stubs.push(Item::Text(Text::new([x, y - STUB_RISE], format!("+{more}"), size, pal.dim).align(Align::Start, Baseline::Middle)));
        }
    }

    // Ports: where each line meets each box, spread along the side.
    let mut ports: BTreeMap<(u64, Side), Vec<(usize, f64)>> = BTreeMap::new();
    for (i, (e, w)) in edges.iter().zip(&ways).enumerate() {
        let (Some(w), Some(ba), Some(bb)) = (w, boxes.get(&e.a), boxes.get(&e.b)) else { continue };
        let centre = |b: &[[f64; 2]; 2]| [(b[0][0] + b[1][0]) / 2.0, (b[0][1] + b[1][1]) / 2.0];
        let (sa, sb) = match w {
            Way::Straight(sa, sb) => (*sa, *sb),
            Way::Gutters { exit, entry, .. } => (*exit, *entry),
        };
        let key = |s: Side, other: [f64; 2]| if matches!(s, Side::Left | Side::Right) { other[1] } else { other[0] };
        ports.entry((e.a, sa)).or_default().push((i, key(sa, centre(bb))));
        let from_a = match w {
            Way::Gutters { row_gutter, .. } => gutter_y(*row_gutter) + lane(&row_lanes, *row_gutter, i, rh - bh),
            Way::Straight(..) => key(sb, centre(ba)),
        };
        ports.entry((e.b, sb)).or_default().push((i, from_a));
    }
    let mut at: BTreeMap<(usize, u64), [f64; 2]> = BTreeMap::new();
    for ((id, side), list) in &mut ports {
        let Some(b) = boxes.get(id) else { continue };
        list.sort_by(|x, y| x.1.total_cmp(&y.1));
        let n = list.len() as f64;
        for (k, (edge, _)) in list.iter().enumerate() {
            let t = (k as f64 + 1.0) / (n + 1.0);
            let p = match side {
                Side::Left => [b[0][0], b[0][1] + (b[1][1] - b[0][1]) * t],
                Side::Right => [b[1][0], b[0][1] + (b[1][1] - b[0][1]) * t],
                Side::Top => {
                    let right = room.get(id).copied().unwrap_or(b[1][0]).max(b[0][0] + 4.0);
                    [b[0][0] + (right - b[0][0]) * t, b[0][1]]
                }
                Side::Bottom => [b[0][0] + (b[1][0] - b[0][0]) * t, b[1][1]],
            };
            at.insert((*edge, *id), p);
        }
    }

    let mut lines = Vec::new();
    let mut labels = Vec::new();
    for (i, (e, w)) in edges.iter().zip(&ways).enumerate() {
        let (Some(w), Some(pa), Some(pb)) = (w, at.get(&(i, e.a)), at.get(&(i, e.b))) else { continue };
        let (pa, pb) = (*pa, *pb);
        let points = match w {
            Way::Straight(sa, _) => {
                if matches!(sa, Side::Left | Side::Right) {
                    let y = (pa[1] + pb[1]) / 2.0;
                    vec![[pa[0], y], [pb[0], y]]
                } else {
                    let x = (pa[0] + pb[0]) / 2.0;
                    vec![[x, pa[1]], [x, pb[1]]]
                }
            }
            Way::Gutters { row_gutter, col_gutter, .. } => {
                let y = gutter_y(*row_gutter) + lane(&row_lanes, *row_gutter, i, rh - bh);
                let x = gutter_x(*col_gutter) + lane(&col_lanes, *col_gutter, i, cw - bw);
                vec![pa, [pa[0], y], [x, y], [x, pb[1]], pb]
            }
        };
        let colour = pal.wire;
        lines.push(Item::Line { points: points.clone(), closed: false, stroke: Stroke::solid(0.35, colour) });
        let (first, last) = (points.first().copied().unwrap_or(pa), points.last().copied().unwrap_or(pb));
        let (second, before_last) =
            (points.get(1).copied().unwrap_or(last), points.len().checked_sub(2).and_then(|k| points.get(k)).copied().unwrap_or(first));
        match e.drives {
            Some(true) => lines.push(arrow(before_last, last, colour, 2.0)),
            Some(false) => lines.push(arrow(second, first, colour, 2.0)),
            None => {}
        }
        if e.label.is_empty() {
            continue;
        }
        // The label, on a patch of the page: by the second box on a line that bends, in the
        // middle of a straight one.
        let size = LABEL;
        let t = match w {
            Way::Gutters { entry, .. } => {
                let right = *entry == Side::Right;
                let x = if right { last[0] + 2.2 } else { last[0] - 2.2 };
                Text::new([x, last[1] - 0.2], e.label.clone(), size, colour).align(if right { Align::Start } else { Align::End }, Baseline::Baseline)
            }
            Way::Straight(Side::Left | Side::Right, _) => {
                Text::new([(first[0] + last[0]) / 2.0, first[1] - 0.8], e.label.clone(), size, colour).align(Align::Middle, Baseline::Baseline)
            }
            Way::Straight(..) => {
                Text::new([first[0] + 1.6, (first[1] + last[1]) / 2.0], e.label.clone(), size, colour).align(Align::Start, Baseline::Middle)
            }
        };
        let tw = c.font.width(&t.text, size);
        let x0 = match t.align {
            Align::Start => t.at[0],
            Align::Middle => t.at[0] - tw / 2.0,
            Align::End => t.at[0] - tw,
        };
        let (y0, y1) = match t.baseline {
            Baseline::Middle => (t.at[1] - size * 0.55, t.at[1] + size * 0.55),
            _ => (t.at[1] - size * 0.85, t.at[1] + size * 0.15),
        };
        labels.push(Item::Rect { rect: [[x0 - 0.6, y0 - 0.2], [x0 + tw + 0.6, y1 + 0.2]], radius: 0.6, fill: Some(pal.background), stroke: None });
        labels.push(Item::Text(t));
    }
    page.add("lines", lines);

    let mut items = Vec::new();
    for f in frames {
        let Some(b) = boxes.get(&f.id) else { continue };
        items.push(Item::Rect { rect: *b, radius: 2.0, fill: Some(pal.body), stroke: Some(Stroke::solid(0.45, pal.ink)) });
        let w = b[1][0] - b[0][0];
        let title = c.font.fit(&f.title, TITLE, w - 6.0);
        items.push(Item::Text(Text::new([b[0][0] + 3.0, b[0][1] + 3.0], title, TITLE, pal.ink).align(Align::Start, Baseline::Top).bold()));
        // Its main parts: the ones with the most pins.
        let mut main: Vec<(usize, String)> = c
            .sheet
            .parts
            .iter()
            .filter(|p| membership.get(&p.id) == Some(&f.id))
            .filter_map(|p| {
                let pc = c.library.get(&p.component)?;
                let n = pc.symbol.pins.len();
                let what = p.value.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| pc.component.name.clone());
                (n >= 3).then(|| (n, format!("{} {}", p.designator, what)))
            })
            .collect();
        main.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let count = c.sheet.parts.iter().filter(|p| membership.get(&p.id) == Some(&f.id)).count();
        let room = (((b[1][1] - b[0][1]) - 12.0) / (DETAIL * 1.45)).floor().max(0.0) as usize;
        let mut y = b[0][1] + 9.5;
        for (_, line) in main.iter().take(room.saturating_sub(1).max(1)) {
            items.push(Item::Text(
                Text::new([b[0][0] + 3.0, y], c.font.fit(line, DETAIL, w - 6.0), DETAIL, pal.dim).align(Align::Start, Baseline::Top),
            ));
            y += DETAIL * 1.45;
        }
        let page_no = section_page.get(&f.id);
        let parts = if count == 1 { "1 part".to_string() } else { format!("{count} parts") };
        let foot = match page_no {
            Some(n) => format!("{parts} · p. {}", n + 1),
            None => parts,
        };
        items.push(Item::Text(Text::new([b[1][0] - 3.0, b[1][1] - 2.4], foot, DETAIL, pal.link).align(Align::End, Baseline::Baseline)));
        if let Some(n) = page_no {
            page.links.push(Link { rect: *b, page: *n, at: None });
        }
    }
    page.add("blocks", items);
    page.add("rails", stubs);
    page.add("labels", labels);
    if let (Some((cols, _)), Some(at)) = (&tree_size, tree_at) {
        let (items, links) = draw_tree(c, &tree, &rails, cols, at, &title_of, &page_of);
        // A tree wider than the page is cut at its edge.
        page.add("power tree", [Item::Clip { rect: [a0, a1], items }]);
        page.links.extend(links);
    }
    let mut y = a0[1] + 1.0;
    let mut items = Vec::new();
    items.push(Item::Line { points: vec![[a0[0], y + 1.2], [a0[0] + 10.0, y + 1.2]], closed: false, stroke: Stroke::solid(0.35, pal.wire) });
    items.push(Item::Text(
        Text::new([a0[0] + 12.5, y + 1.2], "Signals (bus: shared name prefix, count)", DETAIL, pal.dim).align(Align::Start, Baseline::Middle),
    ));
    y += 4.0;
    // A rail stub as the boxes have them.
    let (sx, ty) = (a0[0] + 5.0, y + 1.2 - 0.6);
    items.push(Item::Line { points: vec![[sx, ty + STUB_RISE], [sx, ty]], closed: false, stroke: Stroke::solid(0.3, pal.power) });
    items.push(Item::Line { points: vec![[sx - STUB_BAR, ty], [sx + STUB_BAR, ty]], closed: false, stroke: Stroke::solid(0.45, pal.power) });
    // Only what the stubs on this page show.
    let mut text = "Supply rails: → made in this frame, ← from the page of the frame that makes it".to_string();
    if two_way {
        text.push_str(", ↔ charges and draws (a battery's)");
    }
    if dashed {
        text.push_str("; dashed: (detect), (sense) or (pull-up), a signal or reference, not what powers the frame");
    }
    if hidden {
        text.push_str("; +n: n more rails, in the power tree");
    }
    items.push(Item::Text(Text::new([a0[0] + 12.5, y + 1.2], text, DETAIL, pal.dim).align(Align::Start, Baseline::Middle)));
    if !tree.nodes.is_empty() {
        // The power tree's lines, each with a sample.
        y += 4.0;
        let ly = y + 1.2;
        let mut x = a0[0];
        let power = Stroke::solid(0.3, pal.power);
        let keys: [(&str, u8); 3] =
            [("feeds", 0), ("charges and draws (a battery)", 1), ("detect, sense or pull-up: a signal or reference, not what powers the frame", 2)];
        items.push(Item::Text(Text::new([a0[0] + 12.5, ly], "Power tree (bottom left):", DETAIL, pal.dim).align(Align::Start, Baseline::Middle)));
        x += 12.5 + c.font.width("Power tree (bottom left):", DETAIL) + 3.0;
        for (word, kind) in keys {
            if kind == 1 && !tree.nodes.iter().any(|n| n.link == Use::TwoWay) {
                continue;
            }
            if kind == 2 && !tree.nodes.iter().any(|n| !n.link.powers()) {
                continue;
            }
            let (from, to) = ([x, ly], [x + 9.0, ly]);
            if kind == 2 {
                items.push(Item::Line { points: vec![from, to], closed: false, stroke: Stroke::dashed(0.25, pal.power, SIGNAL_DASH) });
            } else {
                items.push(Item::Line { points: vec![from, to], closed: false, stroke: power });
                items.push(arrow(from, to, pal.power, 1.5));
                if kind == 1 {
                    items.push(arrow(to, from, pal.power, 1.5));
                }
            }
            items.push(Item::Text(Text::new([x + 10.5, ly], word, DETAIL, pal.dim).align(Align::Start, Baseline::Middle)));
            x += 10.5 + c.font.width(word, DETAIL) + 5.0;
        }
    }
    page.add("legend", items);
}

/// The power tree of `rails`: each frame that makes a rail (those fed by no other frame first),
/// each battery, its rails, and the frames each feeds, a frame that takes its power from a rail
/// and makes rails of its own grown in turn (once: a frame met again is a leaf, and so is a
/// frame that only detects, senses or pulls up to the rail).
fn power_tree(rails: &[Rail], frames: &[&Frame]) -> Tree {
    let order = |id: u64| frames.iter().position(|f| f.id == id).unwrap_or(usize::MAX);
    let mut sources: Vec<u64> = rails.iter().filter(|r| r.battery.is_none()).map(|r| r.source).collect();
    sources.sort_by_key(|id| order(*id));
    sources.dedup();
    let fed = |id: u64| rails.iter().any(|r| (r.battery.is_some() || r.source != id) && r.use_by(id).is_some_and(Use::powers));
    let mut tree = Tree::default();
    let mut seen: BTreeSet<u64> = BTreeSet::new();
    for root in sources.iter().copied().filter(|s| !fed(*s)).collect::<Vec<u64>>() {
        if seen.insert(root) {
            grow(&mut tree, rails, &sources, &mut seen, root, 0, None, Use::Supply);
        }
    }
    for (k, _) in rails.iter().enumerate().filter(|(_, r)| r.battery.is_some()) {
        let me = tree.push(Node::Battery(k), 0, None, Use::Supply);
        let row = hang(&mut tree, rails, &sources, &mut seen, k, 1, me).unwrap_or_else(|| tree.next_row());
        tree.set_row(me, row);
    }
    for root in sources.iter().copied().filter(|s| fed(*s)).collect::<Vec<u64>>() {
        if seen.insert(root) {
            grow(&mut tree, rails, &sources, &mut seen, root, 0, None, Use::Supply);
        }
    }
    tree
}

/// Adds `frame` at `col` under `parent` (hanging by `link`), its rails and what they feed;
/// returns its node. Each frame grows once, so the depth is at most twice the frames.
#[allow(clippy::too_many_arguments)]
fn grow(t: &mut Tree, rails: &[Rail], sources: &[u64], seen: &mut BTreeSet<u64>, frame: u64, col: usize, parent: Option<usize>, link: Use) -> usize {
    let me = t.push(Node::Frame(frame), col, parent, link);
    let mut first: Option<usize> = None;
    for (k, _) in rails.iter().enumerate().filter(|(_, r)| r.source == frame && r.battery.is_none()) {
        if let Some(row) = hang(t, rails, sources, seen, k, col + 1, me) {
            first.get_or_insert(row);
        }
    }
    let row = first.unwrap_or_else(|| t.next_row());
    t.set_row(me, row);
    me
}

/// Adds rail `k` at `col` under `parent` and the frames it reaches (beside the source's own,
/// except for a battery's); returns the rail's row, `None` when it is past the rows or reaches
/// no frame.
fn hang(t: &mut Tree, rails: &[Rail], sources: &[u64], seen: &mut BTreeSet<u64>, k: usize, col: usize, parent: usize) -> Option<usize> {
    let r = rails.get(k)?;
    let mut takers: Vec<(u64, Use)> = r.takers.iter().copied().filter(|(f, _)| r.battery.is_some() || *f != r.source).collect();
    if takers.is_empty() {
        return None;
    }
    if t.rows >= MAX_TREE_ROWS {
        t.hidden += takers.len();
        return None;
    }
    let rail = t.push(Node::Rail(k), col, Some(parent), Use::Supply);
    // The frames that grow no further first (those it only reaches by a signal after those it
    // powers), so a long branch hangs below them rather than a lone frame far down below it.
    let grows = |id: u64, u: Use| u.powers() && sources.contains(&id) && !seen.contains(&id);
    takers.sort_by_key(|(id, u)| (grows(*id, *u), !u.powers()));
    let mut first: Option<usize> = None;
    for (taker, u) in takers {
        if t.rows >= MAX_TREE_ROWS {
            t.hidden += 1;
            continue;
        }
        let child = if u.powers() && sources.contains(&taker) && seen.insert(taker) {
            grow(t, rails, sources, seen, taker, col + 1, Some(rail), u)
        } else {
            let n = t.push(Node::Frame(taker), col + 1, Some(rail), u);
            t.rows += 1;
            n
        };
        if first.is_none() {
            first = t.nodes.get(child).map(|n| n.row);
        }
    }
    let row = first.unwrap_or_else(|| t.next_row());
    t.set_row(rail, row);
    Some(row)
}

/// A tree node's text: a frame's title or a battery's (and its page, apart), a rail's name.
fn node_text(n: Node, rails: &[Rail], title_of: &dyn Fn(u64) -> String, page_of: &dyn Fn(u64) -> Option<usize>) -> (String, String) {
    let page = |id: u64| page_of(id).map_or(String::new(), |p| format!("  p. {}", p + 1));
    match n {
        Node::Frame(id) => (title_of(id), page(id)),
        Node::Rail(k) => (rails.get(k).map_or(String::new(), |r| r.name.clone()), String::new()),
        Node::Battery(k) => match rails.get(k) {
            Some(r) => (format!("Battery ({})", r.battery.as_deref().unwrap_or_default()), page(r.source)),
            None => (String::new(), String::new()),
        },
    }
}

/// The tree's columns (left edge and width, from its left) and its whole size with the caption;
/// `None` for no tree. The gap before a column with a frame that hangs by a word (detect, sense,
/// pull-up) is wide enough for the word on its line.
fn tree_size(
    c: &Ctx,
    tree: &Tree,
    rails: &[Rail],
    title_of: &dyn Fn(u64) -> String,
    page_of: &dyn Fn(u64) -> Option<usize>,
) -> Option<(Vec<[f64; 2]>, [f64; 2])> {
    if tree.nodes.is_empty() {
        return None;
    }
    let ncols = tree.nodes.iter().map(|n| n.col + 1).max().unwrap_or(1);
    let mut widths = vec![0.0f64; ncols];
    let mut gaps = vec![COL_GAP; ncols];
    for n in &tree.nodes {
        let (text, page) = node_text(n.node, rails, title_of, page_of);
        // A little over the text, so the bold rail names fit.
        let w = (c.font.width(&text, TREE_TEXT) + c.font.width(&page, TREE_TEXT) + 2.0 * NODE_PAD + 0.6).min(NODE_MAX);
        if let Some(slot) = widths.get_mut(n.col) {
            *slot = slot.max(w);
        }
        if let Some(word) = n.link.word()
            && let Some(slot) = gaps.get_mut(n.col)
        {
            *slot = slot.max(COL_GAP + c.font.width(word, TREE_TEXT - 0.2) + WORD_PAD);
        }
    }
    let mut x = 0.0;
    let mut right = 0.0;
    let cols: Vec<[f64; 2]> = widths
        .iter()
        .enumerate()
        .map(|(k, w)| {
            x += if k > 0 { gaps.get(k).copied().unwrap_or(COL_GAP) } else { 0.0 };
            let at = [x, *w];
            x += w;
            right = x;
            at
        })
        .collect();
    Some((cols, [right, CAPTION_H + tree.rows.max(1) as f64 * PITCH]))
}

/// The tree drawn with its top left at `at`: frames as boxes (with their page, a link to it), a
/// battery as a box in the rails' colour, rails as pills, joined by bracket lines: an arrow into
/// each frame a rail of known direction feeds, two heads where a charger charges a battery and
/// draws from it, and a thin dashed line with its word to a frame that only detects, senses or
/// pulls up to the rail.
fn draw_tree(
    c: &Ctx,
    tree: &Tree,
    rails: &[Rail],
    cols: &[[f64; 2]],
    at: [f64; 2],
    title_of: &dyn Fn(u64) -> String,
    page_of: &dyn Fn(u64) -> Option<usize>,
) -> (Vec<Item>, Vec<Link>) {
    let pal = c.pal;
    let mut lines = Vec::new();
    let mut nodes = Vec::new();
    let mut links = Vec::new();
    let caption = if tree.hidden > 0 { format!("Power tree ({} more fed frames not shown)", tree.hidden) } else { "Power tree".to_string() };
    nodes.push(Item::Text(Text::new([at[0], at[1]], caption, TREE_TEXT + 0.4, pal.ink).align(Align::Start, Baseline::Top).bold()));
    let rect = |col: usize, row: usize| {
        let [x, w] = cols.get(col).copied().unwrap_or([0.0, 0.0]);
        let y = at[1] + CAPTION_H + row as f64 * PITCH + PITCH / 2.0;
        [[at[0] + x, y - NODE_H / 2.0], [at[0] + x + w, y + NODE_H / 2.0]]
    };
    let stroke = Stroke::solid(0.3, pal.power);
    // Rails a two-headed line leaves, so the head into the rail is drawn once.
    let mut back: BTreeSet<usize> = BTreeSet::new();
    for n in &tree.nodes {
        let r = rect(n.col, n.row);
        let y = (r[0][1] + r[1][1]) / 2.0;
        if let Some(parent) = n.parent.and_then(|p| tree.nodes.get(p)) {
            let pr = rect(parent.col, parent.row);
            let (px, py) = (pr[1][0], (pr[0][1] + pr[1][1]) / 2.0);
            let mx = px + COL_GAP / 2.0;
            let points = vec![[px, py], [mx, py], [mx, y], [r[0][0], y]];
            let rail = match (n.node, parent.node) {
                (Node::Frame(_), Node::Rail(k)) => rails.get(k).map(|x| (k, x)),
                _ => None,
            };
            match (rail, n.link.word()) {
                (Some(_), Some(word)) => {
                    // A signal, not a supply: thin, dashed, with its word.
                    lines.push(Item::Line { points, closed: false, stroke: Stroke::dashed(0.25, pal.power, SIGNAL_DASH) });
                    let size = TREE_TEXT - 0.2;
                    let tx = mx + 1.0;
                    let tw = c.font.width(word, size);
                    lines.push(Item::Rect {
                        rect: [[tx - 0.4, y - size * 0.6], [tx + tw + 0.4, y + size * 0.6]],
                        radius: 0.4,
                        fill: Some(pal.background),
                        stroke: None,
                    });
                    lines.push(Item::Text(Text::new([tx, y], word, size, pal.dim).align(Align::Start, Baseline::Middle)));
                }
                (Some((k, x)), None) => {
                    lines.push(Item::Line { points, closed: false, stroke });
                    if x.made || n.link == Use::TwoWay {
                        lines.push(arrow([mx, y], [r[0][0], y], pal.power, 1.5));
                    }
                    if n.link == Use::TwoWay && back.insert(k) {
                        lines.push(arrow([mx, py], [px, py], pal.power, 1.5));
                    }
                }
                (None, _) => lines.push(Item::Line { points, closed: false, stroke }),
            }
        }
        let (text, page) = node_text(n.node, rails, title_of, page_of);
        let width = r[1][0] - r[0][0];
        let box_page = match n.node {
            Node::Frame(id) => Some((page_of(id), Stroke::solid(0.25, pal.ink))),
            Node::Battery(k) => Some((rails.get(k).and_then(|x| page_of(x.source)), Stroke::solid(0.4, pal.power))),
            Node::Rail(_) => None,
        };
        match box_page {
            Some((target, outline)) => {
                nodes.push(Item::Rect { rect: r, radius: 0.6, fill: Some(pal.body), stroke: Some(outline) });
                let pw = c.font.width(&page, TREE_TEXT);
                let title = c.font.fit(&text, TREE_TEXT, width - 2.0 * NODE_PAD - pw);
                let tw = c.font.width(&title, TREE_TEXT);
                nodes.push(Item::Text(Text::new([r[0][0] + NODE_PAD, y], title, TREE_TEXT, pal.ink).align(Align::Start, Baseline::Middle)));
                if !page.is_empty() {
                    nodes.push(Item::Text(Text::new([r[0][0] + NODE_PAD + tw, y], page, TREE_TEXT, pal.link).align(Align::Start, Baseline::Middle)));
                }
                if let Some(p) = target {
                    links.push(Link { rect: r, page: p, at: None });
                }
            }
            None => {
                nodes.push(Item::Rect { rect: r, radius: NODE_H / 2.0, fill: Some(pal.power.mix(pal.background, 0.88)), stroke: Some(stroke) });
                let name = c.font.fit(&text, TREE_TEXT, width - 2.0 * NODE_PAD);
                nodes.push(Item::Text(
                    Text::new([(r[0][0] + r[1][0]) / 2.0, y], name, TREE_TEXT, pal.power).align(Align::Middle, Baseline::Middle).bold(),
                ));
            }
        }
    }
    lines.extend(nodes);
    (lines, links)
}

/// An arrowhead at `to`, pointing from `from`.
fn arrow(from: [f64; 2], to: [f64; 2], colour: Rgb, size: f64) -> Item {
    let d = [to[0] - from[0], to[1] - from[1]];
    let len = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1e-9);
    let u = [d[0] / len, d[1] / len];
    let n = [-u[1], u[0]];
    let base = [to[0] - u[0] * size, to[1] - u[1] * size];
    let w = size * 0.42;
    Item::Area {
        rings: vec![vec![to, [base[0] + n[0] * w, base[1] + n[1] * w], [base[0] - n[0] * w, base[1] - n[1] * w]]],
        fill: colour,
        stroke: None,
    }
}
