//! The schematic: one flat sheet of parts (placed copies of electronic components' symbols), the
//! wires between their pins, labels that name nets (net labels, rail and ground ports) and
//! markers (no-connect, power flag), with the electrical rules check's approvals and rule levels.
//! Nets follow from these and are never saved.
//!
//! Sheet objects share one id space, and an id is never reused. A part refers to its electronic
//! component by id and shows the symbol the component uses now, so a library edit moves the pins
//! wires hang on. Only what is saved is checked: a point that follows from a pin (a wire end on a
//! pin) may move with a library edit or a move without making the sheet invalid.

use std::collections::{BTreeMap, BTreeSet};
use std::iter::once;

use serde::{Deserialize, Serialize};

use super::frame::{Frame, MAX_FRAMES, normalized};
use super::place::{Motion, Placement, is_quarter, lattice, snap_point};
use super::{ElectronicComponent, MAX_DESCRIPTION, MAX_ID, MAX_TEXT, Pin, SymbolItem, check_text};

/// The most parts a schematic holds.
pub const MAX_PARTS: usize = 4096;
/// The most pins all the parts have together.
pub const MAX_PLACED_PINS: usize = 65536;
/// The most wires a schematic holds.
pub const MAX_WIRES: usize = 16384;
/// The most points one wire has, its two ends included.
pub const MAX_WIRE_POINTS: usize = 256;
/// The most points all the wires have together.
pub const MAX_ALL_WIRE_POINTS: usize = 65536;
/// The most diagonal segments between saved wire points on the sheet.
pub const MAX_DIAGONALS: usize = 256;
/// The most labels (net labels and power ports) a schematic holds.
pub const MAX_LABELS: usize = 8192;
/// The most markers a schematic holds.
pub const MAX_MARKERS: usize = 8192;
/// The most approved findings a schematic keeps.
pub const MAX_APPROVALS: usize = 4096;
/// The longest designator, in characters.
pub const MAX_DESIGNATOR: usize = 16;
/// The longest net name, in characters.
pub const MAX_NET_NAME: usize = 64;
/// The longest approval key or rule code, in characters.
pub const MAX_KEY: usize = 256;
/// How far from the origin a saved sheet point may be, in millimetres (an A0 sheet is 1189 mm
/// long).
pub const MAX_SHEET: f64 = 2000.0;

/// The schematic, saved under `ecad.schematic`; nothing is saved while it is empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schematic {
    /// The placed parts, in the order they were placed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wires: Vec<Wire>,
    /// Net labels and power ports: a name on a point that joins every point with that name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<Label>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Section frames: named rectangles a print splits the sheet into pages by (see
    /// [`super::frame`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<Frame>,
    /// Findings of the electrical rules check someone accepted, by key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<Approval>,
    /// A level for a finding's code, in place of its own.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, Level>,
    /// The id the next sheet object gets; ids are never reused.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub next_id: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero_turn(v: &u16) -> bool {
    *v == 0
}

/// One placed copy of an electronic component.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub id: u64,
    /// The electronic component's id; the part shows the symbol it uses now.
    pub component: u64,
    /// The component's variant it is built with; without one, the component's first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Unique on the sheet ignoring case, such as `R1`.
    pub designator: String,
    /// The part's own value; without one, it shows the component's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub placement: Placement,
    /// Where the designator text goes, from `placement.at` along the sheet's axes; without one, the
    /// usual spot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub designator_at: Option<[f64; 2]>,
    /// Where the value text goes, as `designator_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_at: Option<[f64; 2]>,
    /// Do not populate: on the sheet and in the netlist, left off the board's assembly.
    #[serde(default, skip_serializing_if = "is_false")]
    pub dnp: bool,
}

/// What a wire end, a label or a marker sits on: a part's pin (it follows the pin) or a point on
/// the sheet. Saved as `{"part", "pin"}` or `[x, y]`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Anchor {
    Pin { part: u64, pin: String },
    Point([f64; 2]),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PinAnchor {
    part: u64,
    pin: String,
}

impl<'de> Deserialize<'de> for Anchor {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(d)? {
            v @ serde_json::Value::Array(_) => serde_json::from_value(v).map(Anchor::Point).map_err(D::Error::custom),
            v @ serde_json::Value::Object(_) => {
                serde_json::from_value::<PinAnchor>(v).map(|p| Anchor::Pin { part: p.part, pin: p.pin }).map_err(D::Error::custom)
            }
            _ => Err(D::Error::custom("an anchor is a part's pin {\"part\", \"pin\"} or a point [x, y]")),
        }
    }
}

impl Anchor {
    /// The saved point; `None` on a pin.
    pub fn point(&self) -> Option<[f64; 2]> {
        match self {
            Anchor::Point(p) => Some(*p),
            Anchor::Pin { .. } => None,
        }
    }

    /// The part and pin; `None` on a point.
    pub fn pin(&self) -> Option<(u64, &str)> {
        match self {
            Anchor::Pin { part, pin } => Some((*part, pin.as_str())),
            Anchor::Point(_) => None,
        }
    }
}

/// A wire: from one end through `via` to the other, straight between points.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wire {
    pub id: u64,
    pub from: Anchor,
    /// The bends, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub via: Vec<[f64; 2]>,
    pub to: Anchor,
}

/// A name on a point: points with the same name are one net.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Label {
    pub id: u64,
    /// 1 to 64 characters, no spaces, `:` or control characters.
    pub name: String,
    pub kind: LabelKind,
    pub at: Anchor,
    /// The way it points, degrees counter-clockwise: 0, 90, 180 or 270.
    #[serde(default, skip_serializing_if = "is_zero_turn")]
    pub rotation: u16,
}

/// How a label is drawn and what its name means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelKind {
    /// Names a signal.
    Net,
    /// A power port: a supply rail such as +3V3.
    Rail,
    /// A power port drawn as ground, such as GND.
    Ground,
}

/// A mark on a pin or point that the electrical rules check reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub id: u64,
    pub kind: MarkerKind,
    /// A no-connect marker is always on a pin.
    pub at: Anchor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    /// The pin is left unconnected on purpose.
    NoConnect,
    /// The net is powered from off the sheet.
    PowerFlag,
}

/// An accepted finding: its key (`code|item|…`, items by id, such as `p14.4` for part 14's pin 4),
/// why it is fine, and who accepted it (left out on approvals made before that was kept: not
/// known, so a critical one waits for the user's confirmation as an agent's does).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Actor>,
}

impl Approval {
    /// Whether the user made it, in the app's own windows; an agent's, or one made before who
    /// made it was kept, is one the user still has to confirm, for a critical finding.
    pub fn by_user(&self) -> bool {
        self.by == Some(Actor::User)
    }
}

/// Who made a change that records it: the user, in the app's own windows, or an agent (a script,
/// the control channel, MCP or the assistant's chat).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    User,
    #[default]
    Agent,
}

impl Actor {
    pub fn name(self) -> &'static str {
        match self {
            Actor::User => "user",
            Actor::Agent => "agent",
        }
    }
}

/// A rule level for a finding's code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Error,
    Warning,
    Ignore,
}

/// What a sheet object is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ItemKind {
    Part,
    Wire,
    Label,
    Marker,
    Frame,
}

/// Which end of a wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireEnd {
    From,
    To,
}

/// A wire end, label or marker whose pin went (its part deleted, the pin deleted or its
/// component swapped): now at the point where the pin was, or `removed` with it (a no-connect
/// marker, or a wire left with no length).
#[derive(Clone, Debug, PartialEq)]
pub struct Detached {
    pub kind: ItemKind,
    pub id: u64,
    pub end: Option<WireEnd>,
    pub part: u64,
    pub designator: String,
    pub pin: String,
    pub at: [f64; 2],
    pub removed: bool,
}

/// The order `annotate` numbers parts in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// Top to bottom, then left to right.
    Position,
    /// The order they were placed in.
    Placement,
}

/// An electronic component as the sheet sees it: the record, and the symbol it shows (an empty
/// one while it has none).
#[derive(Clone, Copy, Debug)]
pub struct Placeable<'a> {
    pub component: &'a ElectronicComponent,
    pub symbol: &'a SymbolItem,
}

/// Electronic components by id, for checking and resolving a sheet.
pub type Library<'a> = BTreeMap<u64, Placeable<'a>>;

/// Each part and its component's pins by number, looked up once.
pub struct Pins<'a> {
    parts: BTreeMap<u64, (&'a Part, u64)>,
    components: BTreeMap<u64, BTreeMap<&'a str, &'a Pin>>,
}

impl<'a> Pins<'a> {
    /// The parts of `sheet` whose component `library` has.
    pub fn new(sheet: &'a Schematic, library: &Library<'a>) -> Self {
        let mut parts = BTreeMap::new();
        let mut components = BTreeMap::new();
        for p in &sheet.parts {
            let Some(c) = library.get(&p.component).copied() else { continue };
            components.entry(c.component.id).or_insert_with(|| c.symbol.pins.iter().map(|q| (q.number.as_str(), q)).collect::<BTreeMap<_, _>>());
            parts.insert(p.id, (p, c.component.id));
        }
        Pins { parts, components }
    }

    /// The symbol pin `number` of `part`, as the component has it.
    pub fn pin(&self, part: u64, number: &str) -> Option<&'a Pin> {
        let (_, c) = self.parts.get(&part)?;
        self.components.get(c)?.get(number).copied()
    }

    /// Where the pin `number` of `part` lies on the sheet.
    pub fn at(&self, part: u64, number: &str) -> Option<[f64; 2]> {
        let (p, _) = self.parts.get(&part)?;
        self.pin(part, number).map(|q| p.placement.apply(q.at))
    }

    /// Where an anchor lies on the sheet.
    pub fn anchor(&self, a: &Anchor) -> Option<[f64; 2]> {
        match a {
            Anchor::Point(p) => Some(*p),
            Anchor::Pin { part, pin } => self.at(*part, pin),
        }
    }

    /// The part `id` (whose component the library has).
    pub fn part(&self, id: u64) -> Option<&'a Part> {
        self.parts.get(&id).map(|(p, _)| *p)
    }

    /// A wire's points on the sheet, in order: its ends where they are now, and its bends. An end
    /// on a pin that isn't there is left out.
    pub fn wire(&self, w: &Wire) -> Vec<[f64; 2]> {
        let from = self.anchor(&w.from);
        let to = self.anchor(&w.to);
        from.into_iter().chain(w.via.iter().copied()).chain(to).collect()
    }

    /// A saved anchor is a pin the part has, or a point on the sheet.
    fn check(&self, a: &Anchor) -> Result<(), String> {
        match a {
            Anchor::Point(p) => check_sheet_point("the point", *p),
            Anchor::Pin { part, pin } => match self.parts.get(part) {
                None => Err(format!("it is on part {part}, which the sheet doesn't have")),
                Some((p, _)) if self.pin(*part, pin).is_none() => Err(format!("it is on pin `{pin}` of {}, which has no such pin", p.designator)),
                Some(_) => Ok(()),
            },
        }
    }
}

/// A designator: 1 to [`MAX_DESIGNATOR`] ASCII letters, digits or `_`, starting with a letter.
pub fn check_designator(d: &str) -> Result<(), String> {
    let ok = d.len() <= MAX_DESIGNATOR
        && d.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
        && d.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if !ok {
        return Err(format!("the designator `{d}` must be 1 to {MAX_DESIGNATOR} letters, digits or `_`, starting with a letter, such as R1"));
    }
    Ok(())
}

/// A net name: 1 to [`MAX_NET_NAME`] characters with no spaces, `:` or control characters (`:`
/// is kept for the names of unnamed nets, such as `net:R1.2`).
pub fn check_net_name(name: &str) -> Result<(), String> {
    let n = name.chars().count();
    if n == 0 || n > MAX_NET_NAME || name.chars().any(|c| c.is_whitespace() || c == ':' || c.is_control()) {
        return Err(format!(
            "the net name `{name}` must be 1 to {MAX_NET_NAME} characters with no spaces, `:` or control characters, such as VCC or SDA"
        ));
    }
    Ok(())
}

/// A finding's code, as rules name it: 1 to 64 lower-case letters, digits or `_`.
pub fn check_code(code: &str) -> Result<(), String> {
    if code.is_empty() || code.len() > 64 || !code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
        return Err(format!("the rule `{code}` must be a finding's code, such as pin_unconnected"));
    }
    Ok(())
}

/// A point within [`MAX_SHEET`] of the origin on both axes.
pub fn check_sheet_point(what: &str, [x, y]: [f64; 2]) -> Result<(), String> {
    if !(x.is_finite() && y.is_finite() && x.abs() <= MAX_SHEET && y.abs() <= MAX_SHEET) {
        return Err(format!("{what} must be within {MAX_SHEET} mm of the origin"));
    }
    Ok(())
}

/// A designator's letters and number: `("R", 12)` for R12; `None` unless it is letters then a
/// number.
pub fn split_designator(d: &str) -> Option<(&str, u64)> {
    let head = d.trim_end_matches(|c: char| c.is_ascii_digit());
    let digits = d.strip_prefix(head)?;
    if head.is_empty() || digits.is_empty() || !head.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    Some((head, digits.parse().ok()?))
}

/// The designator `prefix` + `n`, if it is a valid one.
pub fn designator(prefix: &str, n: u64) -> Result<String, String> {
    let d = format!("{prefix}{n}");
    check_designator(&d).map(|_| d)
}

/// A part's symbol box on the sheet.
pub fn part_bbox(part: &Part, symbol: &SymbolItem) -> Option<[[f64; 2]; 2]> {
    symbol.bbox().map(|b| part.placement.bbox(b))
}

impl Part {
    /// Its symbol's pins as they lie on the sheet: where a wire connects, and the way each runs
    /// toward the body (a wire leaves the other way).
    pub fn pins(&self, symbol: &SymbolItem) -> Vec<Pin> {
        symbol.pins.iter().map(|p| self.placement.pin(p)).collect()
    }

    /// Its symbol's graphics as they lie on the sheet, ready to draw: mirrored arcs run the other
    /// way and text stays readable.
    pub fn graphics(&self, symbol: &SymbolItem) -> Vec<super::Graphic> {
        symbol.graphics.iter().map(|g| self.placement.graphic(g)).collect()
    }
}

/// A wire's saved points, in order; a pin end is `None` (it follows its pin).
fn saved_points(w: &Wire) -> Vec<Option<[f64; 2]>> {
    once(w.from.point()).chain(w.via.iter().map(|p| Some(*p))).chain(once(w.to.point())).collect()
}

/// Drop a bend equal to the point before it, or to a point end after it; `false` when the wire
/// is left as one point.
fn tidy(w: &mut Wire) -> bool {
    let mut prev = w.from.point().map(lattice);
    let mut via = Vec::with_capacity(w.via.len());
    for p in &w.via {
        let q = lattice(*p);
        if prev != Some(q) {
            via.push(*p);
        }
        prev = Some(q);
    }
    if let (Some(last), Some(to)) = (via.last(), w.to.point())
        && lattice(*last) == lattice(to)
    {
        via.pop();
    }
    w.via = via;
    !(w.via.is_empty() && matches!((w.from.point(), w.to.point()), (Some(a), Some(b)) if lattice(a) == lattice(b)))
}

impl Schematic {
    /// Whether nothing is saved for the schematic.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether the sheet has anything on it (approvals, rules and the id counter alone are none).
    pub fn has_content(&self) -> bool {
        !(self.parts.is_empty() && self.wires.is_empty() && self.labels.is_empty() && self.markers.is_empty() && self.frames.is_empty())
    }

    pub fn part(&self, id: u64) -> Option<&Part> {
        self.parts.iter().find(|p| p.id == id)
    }

    /// The part with this designator, ignoring case.
    pub fn designated(&self, d: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.designator.eq_ignore_ascii_case(d))
    }

    /// The parts of one electronic component.
    pub fn placed(&self, component: u64) -> impl Iterator<Item = &Part> {
        self.parts.iter().filter(move |p| p.component == component)
    }

    /// What the sheet object `id` is.
    pub fn kind_of(&self, id: u64) -> Option<ItemKind> {
        if self.parts.iter().any(|p| p.id == id) {
            Some(ItemKind::Part)
        } else if self.wires.iter().any(|w| w.id == id) {
            Some(ItemKind::Wire)
        } else if self.labels.iter().any(|l| l.id == id) {
            Some(ItemKind::Label)
        } else if self.markers.iter().any(|m| m.id == id) {
            Some(ItemKind::Marker)
        } else if self.frames.iter().any(|f| f.id == id) {
            Some(ItemKind::Frame)
        } else {
            None
        }
    }

    /// Every sheet object's id.
    fn ids(&self) -> impl Iterator<Item = u64> + '_ {
        let parts = self.parts.iter().map(|p| p.id);
        let marks = self.markers.iter().map(|m| m.id).chain(self.frames.iter().map(|f| f.id));
        parts.chain(self.wires.iter().map(|w| w.id)).chain(self.labels.iter().map(|l| l.id)).chain(marks)
    }

    /// One past the largest id in use (1 on an empty sheet).
    pub fn floor(&self) -> u64 {
        self.ids().map(|id| id.saturating_add(1)).max().unwrap_or(1)
    }

    /// A new sheet object's id, which no other one has had in this design.
    pub fn take_id(&mut self) -> Result<u64, String> {
        let id = self.next_id.max(self.floor());
        if id >= MAX_ID {
            return Err("no schematic ids are left in this design".into());
        }
        self.next_id = id + 1;
        Ok(id)
    }

    /// The highest number each designator prefix has (upper case): `R` → 7 with R1 and R7.
    pub fn numbers(&self) -> BTreeMap<String, u64> {
        let mut out: BTreeMap<String, u64> = BTreeMap::new();
        for p in &self.parts {
            if let Some((head, n)) = split_designator(&p.designator) {
                let top = out.entry(head.to_ascii_uppercase()).or_insert(0);
                *top = (*top).max(n);
            }
        }
        out
    }

    /// Every anchor on the sheet.
    fn anchors(&self) -> impl Iterator<Item = &Anchor> {
        let wires = self.wires.iter().flat_map(|w| [&w.from, &w.to]);
        wires.chain(self.labels.iter().map(|l| &l.at)).chain(self.markers.iter().map(|m| &m.at))
    }

    /// The box round everything on the sheet: the parts' symbols and the saved points.
    pub fn bbox(&self, library: &Library) -> Option<[[f64; 2]; 2]> {
        let parts = self.parts.iter().filter_map(|p| part_bbox(p, library.get(&p.component)?.symbol)).flatten();
        let points = self.wires.iter().flat_map(|w| w.via.iter().copied()).chain(self.anchors().filter_map(Anchor::point));
        super::bbox(parts.chain(points))
    }

    /// Everything the design file and the commands require: the caps; unique, non-zero ids below
    /// `next_id`; parts of components with a symbol (and the variant they name), with designators
    /// unique ignoring case;
    /// anchors on pins their part has; no-connect markers on pins; names, approvals and rules in
    /// their forms; saved points on the sheet, with no point repeated in a row along a wire.
    pub fn validate(&self, library: &Library) -> Result<(), String> {
        for (what, n, max) in [
            ("parts", self.parts.len(), MAX_PARTS),
            ("wires", self.wires.len(), MAX_WIRES),
            ("labels", self.labels.len(), MAX_LABELS),
            ("markers", self.markers.len(), MAX_MARKERS),
            ("frames", self.frames.len(), MAX_FRAMES),
            ("approvals", self.approvals.len(), MAX_APPROVALS),
        ] {
            if n > max {
                return Err(format!("a schematic holds at most {max} {what}"));
            }
        }
        if self.next_id > MAX_ID {
            return Err(format!("the schematic id counter {} is out of range", self.next_id));
        }
        let mut ids = BTreeSet::new();
        let mut check_id = |kind: &str, id: u64| {
            if id == 0 || id >= self.next_id {
                return Err(format!("schematic {kind} {id}: the id must be more than 0 and less than `next_id` ({})", self.next_id));
            }
            if !ids.insert(id) {
                return Err(format!("two schematic objects have the id {id}"));
            }
            Ok(())
        };
        let mut designators = BTreeSet::new();
        let mut placed_pins = 0usize;
        for p in &self.parts {
            check_id("part", p.id)?;
            let at = |e: String| format!("schematic part {}: {e}", p.designator);
            check_designator(&p.designator)?;
            if !designators.insert(p.designator.to_ascii_lowercase()) {
                return Err(format!("two schematic parts are designated `{}` (designators must differ in more than case)", p.designator));
            }
            let Some(c) = library.get(&p.component) else {
                return Err(at(format!("its electronic component {} is not in the design", p.component)));
            };
            if c.symbol.is_empty() {
                return Err(at(format!("its electronic component `{}` has no symbol", c.component.name)));
            }
            if let Some(v) = &p.variant
                && c.component.variant(v).is_none()
            {
                return Err(at(format!("its electronic component `{}` has no variant `{v}`", c.component.name)));
            }
            if let Some(v) = &p.value {
                check_text("value", v, MAX_TEXT).map_err(at)?;
            }
            check_sheet_point("`at`", p.placement.at).map_err(at)?;
            if !is_quarter(p.placement.rotation) {
                return Err(at("`rotation` must be 0, 90, 180 or 270".into()));
            }
            for (what, v) in [("`designator_at`", p.designator_at), ("`value_at`", p.value_at)] {
                if let Some(v) = v {
                    check_sheet_point(what, v).map_err(at)?;
                }
            }
            placed_pins = placed_pins.saturating_add(c.symbol.pins.len());
        }
        if placed_pins > MAX_PLACED_PINS {
            return Err(format!("a schematic's parts have at most {MAX_PLACED_PINS} pins together"));
        }
        let pins = Pins::new(self, library);
        let (mut total, mut diagonals) = (0usize, 0usize);
        for w in &self.wires {
            check_id("wire", w.id)?;
            let at = |e: String| format!("schematic wire {}: {e}", w.id);
            for (end, a) in [("from", &w.from), ("to", &w.to)] {
                pins.check(a).map_err(|e| at(format!("`{end}`: {e}")))?;
            }
            let n = w.via.len().saturating_add(2);
            if n > MAX_WIRE_POINTS {
                return Err(at(format!("a wire has at most {MAX_WIRE_POINTS} points")));
            }
            total = total.saturating_add(n);
            for v in &w.via {
                check_sheet_point("a `via` point", *v).map_err(at)?;
            }
            for pair in saved_points(w).windows(2) {
                if let [Some(a), Some(b)] = pair {
                    let (a, b) = (lattice(*a), lattice(*b));
                    if a == b {
                        return Err(at("it has the same point twice in a row".into()));
                    }
                    if a[0] != b[0] && a[1] != b[1] {
                        diagonals += 1;
                    }
                }
            }
        }
        if total > MAX_ALL_WIRE_POINTS {
            return Err(format!("a schematic's wires have at most {MAX_ALL_WIRE_POINTS} points together"));
        }
        if diagonals > MAX_DIAGONALS {
            return Err(format!("a schematic has at most {MAX_DIAGONALS} diagonal wire segments"));
        }
        for l in &self.labels {
            check_id("label", l.id)?;
            let at = |e: String| format!("schematic label {} (`{}`): {e}", l.id, l.name);
            check_net_name(&l.name).map_err(at)?;
            if !is_quarter(l.rotation) {
                return Err(at("`rotation` must be 0, 90, 180 or 270".into()));
            }
            pins.check(&l.at).map_err(at)?;
        }
        for m in &self.markers {
            check_id("marker", m.id)?;
            let at = |e: String| format!("schematic marker {}: {e}", m.id);
            pins.check(&m.at).map_err(at)?;
            if m.kind == MarkerKind::NoConnect && m.at.pin().is_none() {
                return Err(at("a no-connect marker goes on a pin".into()));
            }
        }
        // Frame titles name frames, so they differ in more than case.
        let mut titles = BTreeSet::new();
        for f in &self.frames {
            check_id("frame", f.id)?;
            f.validate().map_err(|e| format!("schematic frame {}: {e}", f.id))?;
            if !titles.insert(f.title.to_ascii_lowercase()) {
                return Err(format!("two schematic frames are titled `{}`: frame titles differ in more than case", f.title));
            }
        }
        let mut keys = BTreeSet::new();
        for a in &self.approvals {
            if a.key.is_empty() || a.key.chars().count() > MAX_KEY || a.key.chars().any(char::is_control) {
                return Err(format!("an approval's key must be 1 to {MAX_KEY} characters with no control characters"));
            }
            if !keys.insert(a.key.as_str()) {
                return Err(format!("two approvals have the key `{}`", a.key));
            }
            check_text("note", &a.note, MAX_DESCRIPTION)?;
        }
        for code in self.rules.keys() {
            check_code(code)?;
        }
        Ok(())
    }

    /// Tie each saved point that lies on a pin to that pin (the lowest part and pin, where pins
    /// touch), so it follows the pin from then on.
    pub fn attach(&mut self, library: &Library) {
        let mut on: BTreeMap<[i64; 2], (u64, String)> = BTreeMap::new();
        for p in &self.parts {
            let Some(c) = library.get(&p.component) else { continue };
            for pin in &c.symbol.pins {
                on.entry(p.placement.lattice_point(pin.at)).or_insert_with(|| (p.id, pin.number.clone()));
            }
        }
        if on.is_empty() {
            return;
        }
        let tie = |a: &mut Anchor| {
            if let Anchor::Point(at) = a
                && let Some((part, pin)) = on.get(&lattice(*at))
            {
                *a = Anchor::Pin { part: *part, pin: pin.clone() };
            }
        };
        for w in &mut self.wires {
            tie(&mut w.from);
            tie(&mut w.to);
        }
        for l in &mut self.labels {
            tie(&mut l.at);
        }
        for m in &mut self.markers {
            tie(&mut m.at);
        }
    }

    /// What every command stores: checked against `library`, then saved points on pins tied to
    /// them (which never makes a valid sheet invalid).
    pub fn settle(&mut self, library: &Library) -> Result<(), String> {
        self.validate(library)?;
        self.attach(library);
        Ok(())
    }

    /// The parts that show the symbol `symbol` (through their components, as `library` has them).
    pub fn showing(&self, library: &Library, symbol: u64) -> BTreeSet<u64> {
        let shows = |p: &&Part| library.get(&p.component).is_some_and(|c| c.component.symbol == Some(symbol));
        self.parts.iter().filter(shows).map(|p| p.id).collect()
    }

    /// A pin of the symbol `parts` show renumbered from `from` to `to`: anchors on it and
    /// approvals that name it follow.
    pub fn pin_renamed(&mut self, parts: &BTreeSet<u64>, from: &str, to: &str) {
        let rename = |a: &mut Anchor| {
            if let Anchor::Pin { part, pin } = a
                && parts.contains(part)
                && pin == from
            {
                *pin = to.to_string();
            }
        };
        for w in &mut self.wires {
            rename(&mut w.from);
            rename(&mut w.to);
        }
        for l in &mut self.labels {
            rename(&mut l.at);
        }
        for m in &mut self.markers {
            rename(&mut m.at);
        }
        for a in &mut self.approvals {
            let items: Vec<String> = a
                .key
                .split('|')
                .map(|item| match item.strip_prefix('p').and_then(|rest| rest.split_once('.')) {
                    Some((part, pin)) if pin == from && part.parse::<u64>().is_ok_and(|id| parts.contains(&id)) => format!("p{part}.{to}"),
                    _ => item.to_string(),
                })
                .collect();
            a.key = items.join("|");
        }
    }

    /// Where each pin of `parts` that an anchor holds lies now.
    fn anchored_at(&self, parts: &BTreeSet<u64>, library: &Library) -> BTreeMap<(u64, String), [f64; 2]> {
        let pins = Pins::new(self, library);
        let mut out = BTreeMap::new();
        for a in self.anchors() {
            if let Anchor::Pin { part, pin } = a
                && parts.contains(part)
                && let Some(at) = pins.at(*part, pin)
            {
                out.insert((*part, pin.clone()), at);
            }
        }
        out
    }

    /// Let go of the pins of `parts` that `keep` doesn't keep: wire ends, labels and power flags
    /// on them become points where the pin is now (in `library`), and no-connect markers on them
    /// go. A wire left with a repeated point loses it, and one left as a single point goes.
    pub fn detach(&mut self, parts: &BTreeSet<u64>, library: &Library, keep: impl Fn(u64, &str) -> bool) -> Vec<Detached> {
        let was = self.anchored_at(parts, library);
        if was.is_empty() {
            return Vec::new();
        }
        let lost = |part: u64, pin: &str| if keep(part, pin) { None } else { was.get(&(part, pin.to_string())).copied() };
        let names: BTreeMap<u64, String> = self.parts.iter().filter(|p| parts.contains(&p.id)).map(|p| (p.id, p.designator.clone())).collect();
        let mut out = Vec::new();
        let mut leave = |kind: ItemKind, id: u64, end: Option<WireEnd>, a: &mut Anchor, remove: bool| {
            let Anchor::Pin { part, pin } = a else { return false };
            let Some(at) = lost(*part, pin) else { return false };
            let at = snap_point(at);
            let designator = names.get(part).cloned().unwrap_or_default();
            out.push(Detached { kind, id, end, part: *part, designator, pin: pin.clone(), at, removed: remove });
            if !remove {
                *a = Anchor::Point(at);
            }
            true
        };
        for w in &mut self.wires {
            leave(ItemKind::Wire, w.id, Some(WireEnd::From), &mut w.from, false);
            leave(ItemKind::Wire, w.id, Some(WireEnd::To), &mut w.to, false);
        }
        for l in &mut self.labels {
            leave(ItemKind::Label, l.id, None, &mut l.at, false);
        }
        let mut gone = BTreeSet::new();
        for m in &mut self.markers {
            if leave(ItemKind::Marker, m.id, None, &mut m.at, m.kind == MarkerKind::NoConnect) && m.kind == MarkerKind::NoConnect {
                gone.insert(m.id);
            }
        }
        self.markers.retain(|m| !gone.contains(&m.id));
        let touched: BTreeSet<u64> = out.iter().filter(|d| d.kind == ItemKind::Wire).map(|d| d.id).collect();
        let mut short = BTreeSet::new();
        for w in self.wires.iter_mut().filter(|w| touched.contains(&w.id)) {
            if !tidy(w) {
                short.insert(w.id);
            }
        }
        self.wires.retain(|w| !short.contains(&w.id));
        for d in &mut out {
            if d.kind == ItemKind::Wire && short.contains(&d.id) {
                d.removed = true;
            }
        }
        out
    }

    /// Keep the sheet in step with `parts` showing a new symbol (a new version of theirs, or
    /// another one; `library` still has what they showed): anchors on pins renumbered (`renames`,
    /// from → to) follow them, and anchors on pins the new symbol doesn't have let go where the
    /// pin was (see [`Schematic::detach`]). Wires on pins that moved within their part hang on
    /// with a jog at the pin, their bends left where they are (see [`super::follow`]).
    pub fn symbol_changed(&mut self, library: &Library, parts: &BTreeSet<u64>, new: &SymbolItem, renames: &[(String, String)]) -> Vec<Detached> {
        if parts.is_empty() {
            return Vec::new();
        }
        let was = super::follow::wire_ends(self, library, parts);
        for (from, to) in renames {
            self.pin_renamed(parts, from, to);
        }
        // A renumbered pin keeps its anchors; a pin that is gone kept its number, so the old
        // symbol in `library` still says where it was.
        let keep: BTreeSet<&str> = new.pins.iter().map(|p| p.number.as_str()).collect();
        let gone = self.detach(parts, library, |_, pin| keep.contains(pin));
        super::follow::follow(self, parts, new, &was);
        gone
    }

    /// The designators and values of `parts` back where their symbol puts them, when the symbol
    /// they show was drawn anew with another box (`was` to `now`, pins included): a text the user
    /// moved was placed for the old drawing, and would sit across its wires or pins now.
    pub fn texts_redrawn(&mut self, parts: &BTreeSet<u64>, was: Option<[[f64; 2]; 2]>, now: Option<[[f64; 2]; 2]>) {
        let same = match (was, now) {
            (Some([a, b]), Some([c, d])) => [lattice(a), lattice(b)] == [lattice(c), lattice(d)],
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        for p in self.parts.iter_mut().filter(|p| parts.contains(&p.id)) {
            p.designator_at = None;
            p.value_at = None;
        }
    }

    /// Remove the sheet objects `ids`; anchors on the pins of parts that go let go where the pins
    /// were (see [`Schematic::detach`]).
    pub fn remove(&mut self, ids: &BTreeSet<u64>, library: &Library) -> Vec<Detached> {
        self.wires.retain(|w| !ids.contains(&w.id));
        self.labels.retain(|l| !ids.contains(&l.id));
        self.markers.retain(|m| !ids.contains(&m.id));
        self.frames.retain(|f| !ids.contains(&f.id));
        let parts: BTreeSet<u64> = self.parts.iter().filter(|p| ids.contains(&p.id)).map(|p| p.id).collect();
        let detached = self.detach(&parts, library, |_, _| false);
        self.parts.retain(|p| !ids.contains(&p.id));
        detached
    }

    /// Move the sheet objects `ids` rigidly: parts with their designator and value texts,
    /// labels and markers on points, and wires' saved points. A label on a pin stays on it, turned
    /// with the part or on its own. Wires not in `ids` keep their saved points; their ends on pins
    /// follow the pins.
    pub fn move_items(&mut self, ids: &BTreeSet<u64>, m: &Motion) {
        let mut moved = BTreeSet::new();
        for p in self.parts.iter_mut().filter(|p| ids.contains(&p.id)) {
            p.placement = m.placement(&p.placement);
            p.designator_at = p.designator_at.map(|v| snap_point(m.vector(v)));
            p.value_at = p.value_at.map(|v| snap_point(m.vector(v)));
            moved.insert(p.id);
        }
        let shift = |a: &mut Anchor| {
            if let Anchor::Point(at) = a {
                *at = m.point(*at);
            }
        };
        for w in self.wires.iter_mut().filter(|w| ids.contains(&w.id)) {
            shift(&mut w.from);
            w.via = w.via.iter().map(|p| m.point(*p)).collect();
            shift(&mut w.to);
        }
        for l in &mut self.labels {
            let pinned = l.at.pin().is_some_and(|(part, _)| moved.contains(&part));
            if ids.contains(&l.id) {
                shift(&mut l.at);
            }
            if ids.contains(&l.id) || pinned {
                l.rotation = m.angle(l.rotation);
            }
        }
        for k in self.markers.iter_mut().filter(|k| ids.contains(&k.id)) {
            shift(&mut k.at);
        }
        for f in self.frames.iter_mut().filter(|f| ids.contains(&f.id)) {
            let [a, b] = f.rect;
            f.rect = normalized([m.point(a), m.point(b)]);
        }
    }

    /// New designators from annotation: each part numbered under its component's prefix (prefixes
    /// that differ only in case count together) from `start`, in `order`. With `keep`, a part
    /// already designated its prefix and a number keeps it, and the others number on after the
    /// highest kept. Returns the parts whose designator changes, with the new one.
    pub fn annotate(&self, library: &Library, order: Order, keep: bool, start: u64) -> Result<Vec<(u64, String)>, String> {
        let mut parts: Vec<(&Part, &ElectronicComponent)> =
            self.parts.iter().filter_map(|p| Some((p, library.get(&p.component)?.component))).collect();
        if order == Order::Position {
            parts.sort_by_key(|(p, _)| {
                let [x, y] = lattice(p.placement.at);
                (std::cmp::Reverse(y), x, p.id)
            });
        }
        let kept =
            |p: &Part, c: &ElectronicComponent| keep && split_designator(&p.designator).is_some_and(|(head, _)| head.eq_ignore_ascii_case(&c.prefix));
        let mut next: BTreeMap<String, u64> = BTreeMap::new();
        for (p, c) in &parts {
            let n = next.entry(c.prefix.to_ascii_uppercase()).or_insert(start);
            if kept(p, c)
                && let Some((_, k)) = split_designator(&p.designator)
            {
                *n = (*n).max(k.saturating_add(1));
            }
        }
        let mut out = Vec::new();
        for (p, c) in parts {
            if kept(p, c) {
                continue;
            }
            let n = next.entry(c.prefix.to_ascii_uppercase()).or_insert(start);
            let d = designator(&c.prefix, *n).map_err(|_| format!("the numbers for prefix {} run past {MAX_DESIGNATOR} characters", c.prefix))?;
            *n = n.saturating_add(1);
            if d != p.designator {
                out.push((p.id, d));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "schematic_tests.rs"]
mod tests;
