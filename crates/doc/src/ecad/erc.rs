//! The electrical rules check: what looks wrong on the schematic, worked out from the sheet and its
//! nets (see [`connect`](super::connect)) each time and never saved.
//!
//! Each finding has a stable code (see [`RULES`]), a severity, the pins and sheet objects it is
//! about, and a key made of the code and ids only (`power_not_driven|p14.4` is about part 14's
//! pin 4; `w7` is wire 7, `l3` label 3, `m5` marker 5, `c2` electronic component 2), so a key
//! outlives renumbering and an approval of it stays put.
//! - Pins on one net whose types clash: [`PIN_MATRIX`] says how badly (`pin_conflict`). A
//!   not-connected pin on a net is `nc_pin_connected` instead.
//! - A net is driven by an output, bidirectional, tri-state, passive, power output, open-collector
//!   or open-emitter pin, a power flag, or a rail or ground port. Power inputs need a power output
//!   pin, a power flag or a rail or ground port: power doesn't pass through passive parts.
//! - The schematic's `rules` give a code another level (`ignore` leaves its findings out), and its
//!   `approvals` mark findings someone accepted. An approval whose finding is gone is stale. The
//!   errors of how nets are driven and joined ([`CRITICAL`]) are critical: an agent's approval of
//!   one (or one made before who approved was kept) waits for the user's, and the finding still
//!   counts until then (`pending_user`).
//!
//! The work is O(N log N) in the sheet's objects, except for the pair tests (near misses, wires
//! across parts, overlapping parts), which have a budget. Past the budget or [`MAX_FINDINGS`]
//! findings, `truncated` is set.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{Value, json};

use super::checks::Severity;
use super::connect::{Connectivity, NetPin, from_lattice, natural_cmp};
use super::graphics::graphics_bbox;
use super::place::{STEP, lattice};
use super::schematic::{Anchor, Approval, Label, LabelKind, Level, Library, MarkerKind, Part, Pins, Schematic, WireEnd, split_designator};
use super::{Electronics, GraphicType, PinType, SymbolItem};

/// The most findings a check returns; past it, `truncated` is set.
pub const MAX_FINDINGS: usize = 10_000;
/// The most items one finding lists; the rest are counted in `more`.
pub const MAX_ITEMS: usize = 16;
/// How close, in millimetres, a wire end or pin may come to a pin or wire it doesn't touch before
/// it is a near miss.
pub const NEAR: f64 = 0.5;
/// The grid pins should lie on, in millimetres (0.05 in), so wires drawn on the sheet's grid meet
/// them.
pub const PIN_GRID: f64 = 1.27;
/// The most pair tests (near misses, wires across parts, overlapping parts) one check makes.
pub const MAX_TESTS: usize = 1 << 22;

/// A finding's code, the level it has unless the schematic's rules say otherwise (`None`: the
/// pin matrix's, by the clashing types), and what it means.
#[derive(Clone, Copy, Debug)]
pub struct Rule {
    pub code: &'static str,
    pub level: Option<Level>,
    pub about: &'static str,
}

const fn rule(code: &'static str, level: Option<Level>, about: &'static str) -> Rule {
    Rule { code, level, about }
}

const ERROR: Option<Level> = Some(Level::Error);
const WARNING: Option<Level> = Some(Level::Warning);

/// Every code the check gives, in the order findings are listed (errors before warnings).
pub const RULES: &[Rule] = &[
    rule("pin_conflict", None, "pins whose types clash are on one net (the pin matrix sets the level)"),
    rule("pin_unconnected", ERROR, "a pin touches nothing (one with a no-connect marker, or of type not_connected, is exempt)"),
    rule("nc_pin_connected", ERROR, "a pin of type not_connected is on a net"),
    rule("input_not_driven", ERROR, "a net with input pins has nothing to drive it"),
    rule("power_not_driven", ERROR, "a net with power input pins has no power output pin, power flag, rail or ground port"),
    rule("rails_shorted", ERROR, "two rail or ground names are on one net"),
    rule("wire_isolated", ERROR, "wires that reach no pin and no label"),
    rule("label_unconnected", ERROR, "a label or power port that touches nothing"),
    rule("net_single_pin", WARNING, "a net that reaches fewer than two pins"),
    rule("wire_dangling", WARNING, "a wire end that touches nothing"),
    rule("near_miss", WARNING, "a wire end or pin within 0.5 mm of a pin or wire it doesn't touch"),
    rule("pin_on_wire", WARNING, "a pin inside a wire's segment, which doesn't connect to it"),
    rule("pin_off_grid", WARNING, "a part's pins are off the 1.27 mm grid"),
    rule("marker_unconnected", WARNING, "a power flag on nothing"),
    rule("no_connect_on_net", WARNING, "a no-connect marker on a pin that is on a net"),
    rule("net_multiple_names", WARNING, "a net with several names"),
    rule("names_differ_by_case", WARNING, "names that differ only in case, which are separate nets"),
    rule("name_kind_clash", WARNING, "one name used by more than one kind of label (net label, rail, ground)"),
    rule("parts_overlap", WARNING, "two parts' bodies overlap"),
    rule("wire_crosses_part", WARNING, "a wire runs across a part's body"),
    rule("value_missing", WARNING, "a part with no value"),
    rule("designator_prefix_mismatch", WARNING, "a designator that doesn't start with its component's prefix"),
    rule("part_no_footprint", WARNING, "a part whose component variant has no footprint, so the board can't place it"),
    rule("library_problem", WARNING, "a placed component has errors in the library"),
];

/// The rule for `code`.
pub fn find_rule(code: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.code == code)
}

/// The codes whose errors (by their own level or the pin matrix's, whatever the schematic's rules
/// give them) are critical: a fight between drivers, a supply or input nothing drives, two rails
/// on one net. An agent's approval of one waits for the user's, and an agent can't lower them.
pub const CRITICAL: &[&str] = &["pin_conflict", "nc_pin_connected", "input_not_driven", "power_not_driven", "rails_shorted"];

/// The pin types in the order of [`PIN_MATRIX`]'s rows and columns.
pub const PIN_TYPES: [PinType; 11] = [
    PinType::Input,
    PinType::Output,
    PinType::Bidirectional,
    PinType::TriState,
    PinType::Passive,
    PinType::PowerInput,
    PinType::PowerOutput,
    PinType::OpenCollector,
    PinType::OpenEmitter,
    PinType::Unspecified,
    PinType::NotConnected,
];

const O: Option<Severity> = None;
const W: Option<Severity> = Some(Severity::Warning);
const E: Option<Severity> = Some(Severity::Error);

/// How badly two pin types clash on one net (symmetric; `None` is fine). A cell on the diagonal
/// needs two pins of the type.
/// - Outputs and power outputs drive both levels: two on one net fight. Open collector or open
///   emitter against one of them is an error too; against a power output it is a warning, since a
///   rail has one level.
/// - Bidirectional, tri-state, open-collector and open-emitter pins share a net safely only when
///   enables or timing keep them apart, which the check can't see: a warning across families.
/// - Inputs and passive pins clash with nothing; whether a net is driven is its own check.
/// - Unspecified pins can't be trusted with anything: a warning. A not-connected pin on a net is
///   an error with everything (reported as `nc_pin_connected`).
pub const PIN_MATRIX: [[Option<Severity>; 11]; 11] = [
    //   In Out Bi Tri Pas PIn POut OC OE Uns NC
    [O, O, O, O, O, O, O, O, O, W, E], // Input
    [O, E, W, W, O, W, E, E, E, W, E], // Output
    [O, W, O, O, O, W, W, W, W, W, E], // Bidirectional
    [O, W, O, O, O, W, W, W, W, W, E], // Tri-state
    [O, O, O, O, O, O, O, O, O, W, E], // Passive
    [O, W, W, W, O, O, O, W, W, W, E], // Power input
    [O, E, W, W, O, O, E, W, W, W, E], // Power output
    [O, E, W, W, O, W, W, O, W, W, E], // Open collector
    [O, E, W, W, O, W, W, W, O, W, E], // Open emitter
    [W, W, W, W, W, W, W, W, W, W, E], // Unspecified
    [E, E, E, E, E, E, E, E, E, E, E], // Not connected
];

/// A pin type's row in [`PIN_MATRIX`].
pub fn type_index(t: PinType) -> usize {
    match t {
        PinType::Input => 0,
        PinType::Output => 1,
        PinType::Bidirectional => 2,
        PinType::TriState => 3,
        PinType::Passive => 4,
        PinType::PowerInput => 5,
        PinType::PowerOutput => 6,
        PinType::OpenCollector => 7,
        PinType::OpenEmitter => 8,
        PinType::Unspecified => 9,
        PinType::NotConnected => 10,
    }
}

/// How badly pins of types `a` and `b` clash on one net.
pub fn clash(a: PinType, b: PinType) -> Option<Severity> {
    PIN_MATRIX.get(type_index(a)).and_then(|row| row.get(type_index(b))).copied().flatten()
}

/// A pin type as messages name it.
pub fn type_name(t: PinType) -> &'static str {
    match t {
        PinType::Input => "input",
        PinType::Output => "output",
        PinType::Bidirectional => "bidirectional",
        PinType::TriState => "tri-state",
        PinType::Passive => "passive",
        PinType::PowerInput => "power input",
        PinType::PowerOutput => "power output",
        PinType::OpenCollector => "open-collector",
        PinType::OpenEmitter => "open-emitter",
        PinType::Unspecified => "unspecified",
        PinType::NotConnected => "not-connected",
    }
}

/// The pin types that drive a net.
const DRIVERS: [PinType; 7] = [
    PinType::Output,
    PinType::Bidirectional,
    PinType::TriState,
    PinType::Passive,
    PinType::PowerOutput,
    PinType::OpenCollector,
    PinType::OpenEmitter,
];

/// What a finding is about.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Pin(NetPin),
    Part { id: u64, designator: String },
    Wire(u64),
    Label { id: u64, name: String },
    Marker(u64),
}

impl Item {
    /// The sheet object to pick for it: a pin's part, or the object itself.
    pub fn id(&self) -> u64 {
        match self {
            Item::Pin(p) => p.part,
            Item::Part { id, .. } | Item::Label { id, .. } => *id,
            Item::Wire(id) | Item::Marker(id) => *id,
        }
    }

    /// How messages name it: `R1.2`, `R1`, `wire 7`, the label's name, `marker 5`.
    pub fn reference(&self) -> String {
        match self {
            Item::Pin(p) => p.reference(),
            Item::Part { designator, .. } => designator.clone(),
            Item::Wire(id) => format!("wire {id}"),
            Item::Label { name, .. } => name.clone(),
            Item::Marker(id) => format!("marker {id}"),
        }
    }
}

/// A command that would deal with a finding, as an agent runs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    pub command: &'static str,
    pub params: Value,
}

/// One finding of the check.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub code: &'static str,
    /// After the schematic's rules.
    pub severity: Severity,
    /// The code and the ids of what it is about (see the module notes).
    pub key: String,
    /// The schematic's approvals hold the key (and, for a critical finding, the user's).
    pub approved: bool,
    /// An error of a [`CRITICAL`] code by its own level (or the pin matrix's), whatever the
    /// schematic's rules give it: an agent's approval of it waits for the user's.
    pub critical: bool,
    /// Critical, and approved by an agent or before who approved was kept: it still counts until
    /// the user confirms it.
    pub pending_user: bool,
    /// The net it is on, by name.
    pub net: Option<String>,
    /// Where on the sheet, in millimetres.
    pub at: Option<[f64; 2]>,
    /// At most [`MAX_ITEMS`]; `more` counts the rest.
    pub items: Vec<Item>,
    pub more: usize,
    pub message: String,
    pub fix: Option<Fix>,
}

/// What the check found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Errors, then warnings, each in the order of [`RULES`]; approved ones included (marked).
    /// Findings of codes the rules ignore are left out.
    pub findings: Vec<Finding>,
    /// Errors and warnings not approved (an approval waiting for the user's counts as none), the
    /// approved findings, and those waiting.
    pub errors: usize,
    pub warnings: usize,
    pub approved: usize,
    pub pending_user: usize,
    /// Approval keys no finding has now.
    pub stale: Vec<String>,
    /// Some of the sheet was past a cap or a budget, or there were more than [`MAX_FINDINGS`]
    /// findings, so not everything is listed.
    pub truncated: bool,
}

impl Electronics {
    /// The electrical rules check of the schematic (see [`erc`]).
    pub fn erc(&self) -> Report {
        erc(self, &self.connectivity())
    }
}

/// What one finding says, made only when it is kept.
struct Draft {
    /// The pin matrix's level, for `pin_conflict`.
    clash: Option<Severity>,
    net: Option<String>,
    at: Option<[f64; 2]>,
    items: Vec<Item>,
    message: String,
    fix: Option<Fix>,
}

impl Draft {
    fn new(at: Option<[f64; 2]>, items: Vec<Item>, message: String) -> Self {
        Draft { clash: None, net: None, at, items, message, fix: None }
    }

    fn on(mut self, net: &str) -> Self {
        self.net = Some(net.to_string());
        self
    }

    fn fix(mut self, command: &'static str, params: Value) -> Self {
        self.fix = Some(Fix { command, params });
        self
    }
}

/// The findings being collected.
struct Found<'a> {
    sheet: &'a Schematic,
    approved: BTreeMap<&'a str, &'a Approval>,
    /// Every key found, kept or not, to tell stale approvals.
    seen: BTreeSet<String>,
    list: Vec<Finding>,
    truncated: bool,
}

impl Found<'_> {
    /// A finding `code` with the key items `key`; `make` says the rest when it is kept.
    fn add(&mut self, code: &'static str, key: &[String], make: impl FnOnce() -> Draft) {
        let key = format!("{code}|{}", key.join("|"));
        let level = self.sheet.rules.get(code).copied().or_else(|| find_rule(code).and_then(|r| r.level));
        if !self.sheet.approvals.is_empty() {
            self.seen.insert(key.clone());
        }
        if level == Some(Level::Ignore) {
            return;
        }
        if self.list.len() >= MAX_FINDINGS {
            self.truncated = true;
            return;
        }
        let d = make();
        let severity = match level {
            Some(Level::Warning) => Severity::Warning,
            Some(_) => Severity::Error,
            None => d.clash.unwrap_or(Severity::Error),
        };
        // Critical by the code's own level, so lowering it doesn't make it less so.
        let own = find_rule(code).and_then(|r| r.level).map_or(d.clash.unwrap_or(Severity::Error), |l| match l {
            Level::Warning => Severity::Warning,
            _ => Severity::Error,
        });
        let critical = CRITICAL.contains(&code) && own == Severity::Error;
        let mut items = d.items;
        let more = items.len().saturating_sub(MAX_ITEMS);
        items.truncate(MAX_ITEMS);
        let approval = self.approved.get(key.as_str()).copied();
        let pending_user = critical && approval.is_some_and(|a| !a.by_user());
        let approved = approval.is_some() && !pending_user;
        self.list.push(Finding {
            code,
            severity,
            key,
            approved,
            critical,
            pending_user,
            net: d.net,
            at: d.at,
            items,
            more,
            message: d.message,
            fix: d.fix,
        });
    }
}

/// What the checks share.
struct Sheet<'a> {
    ecad: &'a Electronics,
    sheet: &'a Schematic,
    library: Library<'a>,
    pins: Pins<'a>,
    c: &'a Connectivity,
    /// How many pair tests are left.
    budget: usize,
}

/// A pin as keys name it: `p14.4`.
fn pin_key(p: &NetPin) -> String {
    format!("p{}.{}", p.part, p.pin)
}

/// Pins by part id, then number: the order keys pick the lowest from, which renumbering doesn't
/// change.
fn by_id(a: &&NetPin, b: &&NetPin) -> Ordering {
    a.part.cmp(&b.part).then_with(|| natural_cmp(&a.pin, &b.pin))
}

/// Pins as messages list them: `R1.1, U1.4` (at most [`MAX_ITEMS`], then `…`).
fn refs<'a>(pins: impl IntoIterator<Item = &'a NetPin>) -> String {
    let all: Vec<String> = pins.into_iter().map(NetPin::reference).collect();
    let mut out = all.iter().take(MAX_ITEMS).cloned().collect::<Vec<_>>().join(", ");
    if all.len() > MAX_ITEMS {
        out.push_str(", …");
    }
    out
}

/// Millimetres as messages give them.
fn mm(v: f64) -> String {
    let s = format!("{:.4}", (v * 1e4).round() / 1e4 + 0.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn point(p: [f64; 2]) -> String {
    format!("({}, {})", mm(p[0]), mm(p[1]))
}

/// The electrical rules check of `ecad`'s schematic, whose nets are `c` (see the module notes).
pub fn erc(ecad: &Electronics, c: &Connectivity) -> Report {
    let sheet = &ecad.schematic;
    let library = ecad.library();
    let pins = Pins::new(sheet, &library);
    let mut s = Sheet { ecad, sheet, library, pins, c, budget: MAX_TESTS };
    let mut f = Found {
        sheet,
        approved: sheet.approvals.iter().map(|a| (a.key.as_str(), a)).collect(),
        seen: BTreeSet::new(),
        list: Vec::new(),
        truncated: c.truncated,
    };
    nets(&s, &mut f);
    pin_checks(&s, &mut f);
    wire_checks(&mut s, &mut f);
    marker_checks(&s, &mut f);
    name_checks(&s, &mut f);
    part_checks(&mut s, &mut f);
    if s.budget == 0 {
        f.truncated = true;
    }
    let order = |code: &str| RULES.iter().position(|r| r.code == code).unwrap_or(RULES.len());
    let mut findings = f.list;
    findings.sort_by_key(|x| (x.severity != Severity::Error, order(x.code)));
    let approved = findings.iter().filter(|x| x.approved).count();
    let pending_user = findings.iter().filter(|x| x.pending_user).count();
    let open = |sev: Severity| findings.iter().filter(|x| !x.approved && x.severity == sev).count();
    let (errors, warnings) = (open(Severity::Error), open(Severity::Warning));
    let stale = sheet.approvals.iter().filter(|a| !f.seen.contains(&a.key)).map(|a| a.key.clone()).collect();
    Report { findings, errors, warnings, approved, pending_user, stale, truncated: f.truncated }
}

/// The checks of each net: clashing pins, what drives it, its names, and nets that reach too
/// little.
fn nets(s: &Sheet, f: &mut Found) {
    let markers = s.sheet.markers.iter().zip(&s.c.marker_nets);
    let flagged: BTreeSet<usize> = markers.filter(|(m, _)| m.kind == MarkerKind::PowerFlag).filter_map(|(_, k)| *k).collect();
    let labels: BTreeMap<u64, &Label> = s.sheet.labels.iter().map(|l| (l.id, l)).collect();
    for (k, net) in s.c.nets.iter().enumerate() {
        let name = net.name.as_str();
        let mut of: [Vec<&NetPin>; 11] = Default::default();
        for p in &net.pins {
            if let Some(v) = of.get_mut(type_index(p.electrical)) {
                v.push(p);
            }
        }
        for v in &mut of {
            v.sort_by(by_id);
        }
        let typed = |t: PinType| of.get(type_index(t)).map(Vec::as_slice).unwrap_or_default();

        // Clashing types, a pair of types at a time; not-connected pins are reported on their own.
        for a in 0..PIN_TYPES.len() - 1 {
            for b in a..PIN_TYPES.len() - 1 {
                let (Some(&ta), Some(&tb)) = (PIN_TYPES.get(a), PIN_TYPES.get(b)) else { continue };
                let Some(level) = clash(ta, tb) else { continue };
                let (pa, pb) = (typed(ta), typed(tb));
                let key: Vec<String> = if a == b {
                    if pa.len() < 2 {
                        continue;
                    }
                    pa.iter().take(2).map(|p| pin_key(p)).collect()
                } else {
                    let (Some(x), Some(y)) = (pa.first(), pb.first()) else { continue };
                    let mut key = vec![pin_key(x), pin_key(y)];
                    key.sort();
                    key
                };
                f.add("pin_conflict", &key, || {
                    let items: Vec<&NetPin> = if a == b { pa.to_vec() } else { pa.iter().chain(pb).copied().collect() };
                    let message = if a == b {
                        format!("{} {} pins are connected on {name}: {}", pa.len(), type_name(ta), refs(pa.iter().copied()))
                    } else {
                        format!(
                            "{} pin{} {} {} connected to {} pin{} {} on {name}",
                            type_name(ta),
                            if pa.len() == 1 { "" } else { "s" },
                            refs(pa.iter().copied()),
                            if pa.len() == 1 { "is" } else { "are" },
                            type_name(tb),
                            if pb.len() == 1 { "" } else { "s" },
                            refs(pb.iter().copied()),
                        )
                    };
                    let at = items.first().map(|p| p.at);
                    let mut d = Draft::new(at, items.into_iter().cloned().map(Item::Pin).collect(), message).on(name);
                    d.clash = Some(level);
                    d
                });
            }
        }
        for p in typed(PinType::NotConnected) {
            f.add("nc_pin_connected", &[pin_key(p)], || {
                let message = format!("{} is not connected inside its part but is on {name}: take it off the net", p.reference());
                Draft::new(Some(p.at), vec![Item::Pin((*p).clone())], message).on(name)
            });
        }

        // What drives the net.
        let supplied = net.power || flagged.contains(&k);
        let driven = supplied || DRIVERS.iter().any(|t| !typed(*t).is_empty());
        let inputs = typed(PinType::Input);
        if !driven && let Some(first) = inputs.first() {
            f.add("input_not_driven", &[pin_key(first)], || {
                let message =
                    format!("{name} goes to input pins ({}) but nothing drives it: connect an output or a power port", refs(inputs.iter().copied()));
                Draft::new(Some(first.at), inputs.iter().map(|p| Item::Pin((*p).clone())).collect(), message).on(name)
            });
        }
        let power_in = typed(PinType::PowerInput);
        if !supplied
            && typed(PinType::PowerOutput).is_empty()
            && let Some(first) = power_in.first()
        {
            f.add("power_not_driven", &[pin_key(first)], || {
                let message = format!(
                    "{name} feeds power inputs ({}) but nothing supplies it: add a rail or ground port, or a power flag where it comes from off the sheet",
                    refs(power_in.iter().copied())
                );
                Draft::new(Some(first.at), power_in.iter().map(|p| Item::Pin((*p).clone())).collect(), message)
                    .on(name)
                    .fix("ecad.schematic.marker", json!({"markers": [{"kind": "power_flag", "at": first.reference()}]}))
            });
        }

        // Its names: the lowest label id of each, in name order.
        let mut names: BTreeMap<&str, (&Label, bool)> = BTreeMap::new();
        for l in net.labels.iter().filter_map(|id| labels.get(id).copied()) {
            names.entry(l.name.as_str()).or_insert((l, false)).1 |= l.kind != LabelKind::Net;
        }
        let mut named: Vec<(&Label, bool)> = names.into_values().collect();
        named.sort_by(|(a, _), (b, _)| natural_cmp(&a.name, &b.name));
        let label_items = |v: &[&Label]| v.iter().map(|l| Item::Label { id: l.id, name: l.name.clone() }).collect::<Vec<_>>();
        let label_key = |v: &[&Label]| {
            let mut ids: Vec<u64> = v.iter().map(|l| l.id).collect();
            ids.sort_unstable();
            ids.into_iter().take(2).map(|id| format!("l{id}")).collect::<Vec<_>>()
        };
        let at = |l: &Label| s.pins.anchor(&l.at);
        let power: Vec<&Label> = named.iter().filter(|(_, p)| *p).map(|(l, _)| *l).collect();
        let all: Vec<&Label> = named.iter().map(|(l, _)| *l).collect();
        if power.len() >= 2 {
            f.add("rails_shorted", &label_key(&power), || {
                let list = power.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ");
                let message = format!("{list} are connected to each other: two supplies are shorted");
                Draft::new(power.first().and_then(|l| at(l)), label_items(&power), message).on(name)
            });
        } else if all.len() >= 2 {
            f.add("net_multiple_names", &label_key(&all), || {
                let list = all.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ");
                let message = format!("one net has the names {list} and goes by {name}: keep one name, or split the net");
                Draft::new(all.first().and_then(|l| at(l)), label_items(&all), message).on(name)
            });
        }

        // Nets that reach too little.
        if net.pins.is_empty() && net.labels.is_empty() {
            let Some(first) = net.wires.first() else { continue };
            f.add("wire_isolated", &[format!("w{first}")], || {
                let ids = net.wires.iter().map(u64::to_string).collect::<Vec<_>>().join(", ");
                let message = format!("wire {ids} reaches no pin and no label: wire it to a pin, or delete it");
                let at = s.sheet.wires.iter().find(|w| w.id == *first).and_then(|w| s.pins.anchor(&w.from));
                Draft::new(at, net.wires.iter().map(|w| Item::Wire(*w)).collect(), message)
                    .on(name)
                    .fix("ecad.schematic.delete", json!({"items": net.wires.iter().take(MAX_ITEMS).collect::<Vec<_>>()}))
            });
        } else if net.pins.is_empty() && net.wires.is_empty() {
            for l in net.labels.iter().filter_map(|id| labels.get(id).copied()) {
                f.add("label_unconnected", &[format!("l{}", l.id)], || {
                    let what = if l.kind == LabelKind::Net { "net label" } else { "power port" };
                    let message = format!("the {what} {} touches nothing: put it on a wire or a pin, or delete it", l.name);
                    Draft::new(at(l), vec![Item::Label { id: l.id, name: l.name.clone() }], message)
                        .on(name)
                        .fix("ecad.schematic.delete", json!({"items": [l.id]}))
                });
            }
        } else if net.pins.len() < 2 {
            let mut lowest: Vec<&NetPin> = net.pins.iter().collect();
            lowest.sort_by(by_id);
            let key = match (lowest.first(), net.wires.first(), net.labels.first()) {
                (Some(p), _, _) => pin_key(p),
                (None, Some(w), _) => format!("w{w}"),
                (None, None, Some(l)) => format!("l{l}"),
                (None, None, None) => continue,
            };
            f.add("net_single_pin", &[key], || {
                let (message, at) = match lowest.first() {
                    Some(p) => (format!("{name} reaches only {}: connect it to another pin", p.reference()), Some(p.at)),
                    None => (format!("{name} reaches no pin: connect it to pins, or delete it"), None),
                };
                let mut items: Vec<Item> = lowest.iter().map(|p| Item::Pin((*p).clone())).collect();
                items.extend(net.wires.iter().map(|w| Item::Wire(*w)));
                Draft::new(at, items, message).on(name)
            });
        }
    }
}

/// The checks of each pin: left unconnected, on a wire's segment, off the grid.
fn pin_checks(s: &Sheet, f: &mut Found) {
    let no_connect: BTreeSet<(u64, &str)> = s.sheet.markers.iter().filter(|m| m.kind == MarkerKind::NoConnect).filter_map(|m| m.at.pin()).collect();
    for p in &s.c.unconnected {
        if p.electrical == PinType::NotConnected || no_connect.contains(&(p.part, p.pin.as_str())) {
            continue;
        }
        f.add("pin_unconnected", &[pin_key(p)], || {
            let name = if p.name.is_empty() { String::new() } else { format!(" ({})", p.name) };
            let message = format!("{}{name} connects to nothing: wire it, or mark it no-connect if it is left open on purpose", p.reference());
            Draft::new(Some(p.at), vec![Item::Pin(p.clone())], message)
                .fix("ecad.schematic.marker", json!({"markers": [{"kind": "no_connect", "at": p.reference()}]}))
        });
    }
    for x in &s.c.pin_on_wire {
        f.add("pin_on_wire", &[pin_key(&x.pin)], || {
            let message = format!(
                "{} lies on wire {} without connecting to it (a wire joins a pin only at its end or bend): end the wire on the pin, or move the part",
                x.pin.reference(),
                x.wire
            );
            Draft::new(Some(x.pin.at), vec![Item::Pin(x.pin.clone()), Item::Wire(x.wire)], message)
        });
    }
    let step = (PIN_GRID / STEP).round() as i64;
    for part in &s.sheet.parts {
        let Some(c) = s.library.get(&part.component) else { continue };
        let off: Vec<(&super::Pin, [i64; 2])> = c
            .symbol
            .pins
            .iter()
            .map(|q| (q, part.placement.lattice_point(q.at)))
            .filter(|(_, q)| q[0].rem_euclid(step) != 0 || q[1].rem_euclid(step) != 0)
            .collect();
        let Some((_, first)) = off.first() else { continue };
        f.add("pin_off_grid", &[format!("p{}", part.id)], || {
            let to = |v: i64| v.saturating_add(step / 2).div_euclid(step).saturating_mul(step).saturating_sub(v);
            let by = from_lattice([to(first[0]), to(first[1])]);
            let list = off.iter().take(MAX_ITEMS).map(|(q, _)| format!("{}.{}", part.designator, q.number)).collect::<Vec<_>>().join(", ");
            let message = format!(
                "{}'s pins {list} are off the {} mm grid, so wires drawn on the grid miss them: move the part by {}",
                part.designator,
                mm(PIN_GRID),
                point(by)
            );
            let items = off.iter().map(|(q, at)| Item::Pin(sheet_pin(part, q, *at))).collect();
            Draft::new(Some(from_lattice(*first)), items, message).fix("ecad.schematic.move", json!({"items": [part.designator.clone()], "by": by}))
        });
    }
}

/// A part's pin as findings list it.
fn sheet_pin(part: &Part, pin: &super::Pin, at: [i64; 2]) -> NetPin {
    NetPin {
        part: part.id,
        designator: part.designator.clone(),
        pin: pin.number.clone(),
        name: pin.name.clone(),
        electrical: pin.electrical,
        at: from_lattice(at),
    }
}

/// A grid of cells, each with the things (by index) whose box reaches into it; boxes too big for
/// the cells are tested against every point.
struct Cells {
    size: f64,
    cells: HashMap<(i64, i64), Vec<usize>>,
    wide: Vec<usize>,
}

impl Cells {
    fn new(size: f64) -> Self {
        Cells { size, cells: HashMap::new(), wide: Vec::new() }
    }

    fn cell(&self, v: f64) -> i64 {
        // Out-of-range values saturate; the sheet's points are within its limits.
        (v / self.size).floor() as i64
    }

    fn insert(&mut self, i: usize, [[x0, y0], [x1, y1]]: [[f64; 2]; 2]) {
        let (cx0, cx1, cy0, cy1) = (self.cell(x0), self.cell(x1), self.cell(y0), self.cell(y1));
        let many = cx1.saturating_sub(cx0).saturating_add(1).saturating_mul(cy1.saturating_sub(cy0).saturating_add(1));
        if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) || !(1..=64).contains(&many) {
            self.wide.push(i);
            return;
        }
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                self.cells.entry((cx, cy)).or_default().push(i);
            }
        }
    }

    /// What may reach `p`.
    fn near(&self, [x, y]: [f64; 2]) -> impl Iterator<Item = usize> + '_ {
        self.cells.get(&(self.cell(x), self.cell(y))).into_iter().flatten().chain(&self.wide).copied()
    }
}

fn distance([ax, ay]: [f64; 2], [bx, by]: [f64; 2]) -> f64 {
    (ax - bx).hypot(ay - by)
}

/// The distance from `p` to the segment `a`–`b`.
fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    distance(p, [a[0] + t * dx, a[1] + t * dy])
}

/// Whether the segment `a`–`b` runs through the inside of the box (its edges and corners don't
/// count).
fn crosses_box(a: [f64; 2], b: [f64; 2], [[x0, y0], [x1, y1]]: [[f64; 2]; 2]) -> bool {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let d = [b[0] - a[0], b[1] - a[1]];
    for (p, q) in [(-d[0], a[0] - x0), (d[0], x1 - a[0]), (-d[1], a[1] - y0), (d[1], y1 - a[1])] {
        if p == 0.0 {
            if q <= 0.0 {
                return false;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
        }
    }
    t1 - t0 > 1e-9
}

/// The checks of wires: ends that touch nothing, near misses, and wires across parts.
fn wire_checks(s: &mut Sheet, f: &mut Found) {
    let end_name = |e: WireEnd| if e == WireEnd::From { "from" } else { "to" };
    for d in &s.c.dangling {
        f.add("wire_dangling", &[format!("w{}.{}", d.wire, end_name(d.end))], || {
            let message = format!("wire {}'s end at {} touches nothing: end it on a pin, a wire or a label", d.wire, point(d.at));
            Draft::new(Some(d.at), vec![Item::Wire(d.wire)], message)
        });
    }

    // Every pin, and every wire's segments, in cells.
    let all_pins: Vec<NetPin> = s
        .sheet
        .parts
        .iter()
        .filter_map(|p| Some((p, s.library.get(&p.component)?)))
        .flat_map(|(p, c)| c.symbol.pins.iter().map(move |q| sheet_pin(p, q, p.placement.lattice_point(q.at))))
        .collect();
    let mut pin_cells = Cells::new(1.0);
    for (i, p) in all_pins.iter().enumerate() {
        pin_cells.insert(i, [p.at, p.at]);
    }
    let mut segments: Vec<([f64; 2], [f64; 2], u64)> = Vec::new();
    for w in &s.sheet.wires {
        for pair in s.pins.wire(w).windows(2) {
            if let [a, b] = pair {
                segments.push((*a, *b, w.id));
            }
        }
    }
    let mut segment_cells = Cells::new(4.0);
    for (i, (a, b, _)) in segments.iter().enumerate() {
        let lo = [a[0].min(b[0]) - NEAR, a[1].min(b[1]) - NEAR];
        let hi = [a[0].max(b[0]) + NEAR, a[1].max(b[1]) + NEAR];
        segment_cells.insert(i, [lo, hi]);
    }
    let mut budget = s.budget;
    // The nearest segment within NEAR of `p` (not on it), leaving out the wire `skip`.
    let mut nearest_segment = |p: [f64; 2], skip: Option<u64>| -> Option<(u64, f64)> {
        let mut best: Option<(u64, f64)> = None;
        for i in segment_cells.near(p) {
            if budget == 0 {
                break;
            }
            budget -= 1;
            let Some((a, b, w)) = segments.get(i) else { continue };
            if Some(*w) == skip {
                continue;
            }
            let d = segment_distance(p, *a, *b);
            if d > 1e-6 && d <= NEAR && best.is_none_or(|(_, e)| d < e) {
                best = Some((*w, d));
            }
        }
        best
    };
    for d in &s.c.dangling {
        let mut near_pin: Option<(&NetPin, f64)> = None;
        let cell = (d.at[0].floor() as i64, d.at[1].floor() as i64);
        for cx in cell.0.saturating_sub(1)..=cell.0.saturating_add(1) {
            for cy in cell.1.saturating_sub(1)..=cell.1.saturating_add(1) {
                for i in pin_cells.cells.get(&(cx, cy)).into_iter().flatten() {
                    let Some(p) = all_pins.get(*i) else { continue };
                    let e = distance(d.at, p.at);
                    if e > 1e-6 && e <= NEAR && near_pin.is_none_or(|(_, b)| e < b) {
                        near_pin = Some((p, e));
                    }
                }
            }
        }
        let key = [format!("w{}.{}", d.wire, end_name(d.end))];
        if let Some((p, e)) = near_pin {
            f.add("near_miss", &key, || {
                let message = format!("wire {}'s end is {} mm from {} but not on it: move the end onto the pin", d.wire, mm(e), p.reference());
                Draft::new(Some(d.at), vec![Item::Wire(d.wire), Item::Pin(p.clone())], message)
            });
        } else if let Some((w, e)) = nearest_segment(d.at, Some(d.wire)) {
            f.add("near_miss", &key, || {
                let message = format!(
                    "wire {}'s end is {} mm from wire {w} but not on it: move the end onto that wire (an end on a wire joins it)",
                    d.wire,
                    mm(e)
                );
                Draft::new(Some(d.at), vec![Item::Wire(d.wire), Item::Wire(w)], message)
            });
        }
    }
    for p in &s.c.unconnected {
        if let Some((w, e)) = nearest_segment(p.at, None) {
            f.add("near_miss", &[pin_key(p)], || {
                let message = format!("{} is {} mm from wire {w} but not on it: end the wire on the pin", p.reference(), mm(e));
                Draft::new(Some(p.at), vec![Item::Pin(p.clone()), Item::Wire(w)], message)
            });
        }
    }

    // Wires across a part's body; a segment that ends on one of the part's pins is how a wire
    // reaches it, so it doesn't count.
    let bodies = bodies(s);
    let mut crossed: BTreeSet<(u64, u64)> = BTreeSet::new();
    'segments: for (a, b, w) in &segments {
        let (la, lb) = (lattice(*a), lattice(*b));
        for (part, body) in &bodies {
            if budget == 0 {
                break 'segments;
            }
            budget -= 1;
            if crossed.contains(&(*w, part.id)) || !crosses_box(*a, *b, *body) {
                continue;
            }
            let ends_on_pin = s.library.get(&part.component).is_some_and(|c| {
                c.symbol.pins.iter().any(|q| {
                    let at = part.placement.lattice_point(q.at);
                    at == la || at == lb
                })
            });
            if ends_on_pin {
                continue;
            }
            crossed.insert((*w, part.id));
            f.add("wire_crosses_part", &[format!("w{w}"), format!("p{}", part.id)], || {
                let message = format!("wire {w} runs across {}: route it round the part", part.designator);
                let middle = [(body[0][0] + body[1][0]) / 2.0, (body[0][1] + body[1][1]) / 2.0];
                Draft::new(Some(middle), vec![Item::Wire(*w), Item::Part { id: part.id, designator: part.designator.clone() }], message)
            });
        }
    }
    s.budget = budget;
}

/// Each part's body on the sheet: its symbol's drawing (text aside) and the inner ends of its
/// pins, shrunk a little so parts that only touch don't overlap.
fn bodies<'a>(s: &Sheet<'a>) -> Vec<(&'a Part, [[f64; 2]; 2])> {
    let sheet: &'a Schematic = s.sheet;
    let mut out = Vec::new();
    for part in &sheet.parts {
        let Some(c) = s.library.get(&part.component) else { continue };
        if let Some(b) = body_box(part, c.symbol) {
            let b = [[b[0][0] + 1e-3, b[0][1] + 1e-3], [b[1][0] - 1e-3, b[1][1] - 1e-3]];
            if b[0][0] < b[1][0] && b[0][1] < b[1][1] {
                out.push((part, b));
            }
        }
    }
    out
}

fn body_box(part: &Part, symbol: &SymbolItem) -> Option<[[f64; 2]; 2]> {
    let drawing = graphics_bbox(symbol.graphics.iter().filter(|g| g.kind != GraphicType::Text));
    let inner = symbol.pins.iter().map(super::Pin::body_end);
    super::bbox(drawing.into_iter().flatten().chain(inner)).map(|b| part.placement.bbox(b))
}

/// The checks of markers: a power flag on nothing, a no-connect marker on a net.
fn marker_checks(s: &Sheet, f: &mut Found) {
    for (m, net) in s.sheet.markers.iter().zip(&s.c.marker_nets) {
        match (m.kind, &m.at) {
            (MarkerKind::PowerFlag, Anchor::Point(at)) if net.is_none() => {
                f.add("marker_unconnected", &[format!("m{}", m.id)], || {
                    let message = format!("the power flag at {} is on nothing: put it on a wire or a pin, or delete it", point(*at));
                    Draft::new(Some(*at), vec![Item::Marker(m.id)], message).fix("ecad.schematic.delete", json!({"items": [m.id]}))
                });
            }
            (MarkerKind::NoConnect, Anchor::Pin { part, pin }) => {
                let Some(k) = s.c.pin_net(*part, pin) else { continue };
                let Some(net) = s.c.nets.get(k) else { continue };
                let Some(p) = net.pins.iter().find(|p| p.part == *part && p.pin == *pin) else { continue };
                f.add("no_connect_on_net", &[format!("m{}", m.id)], || {
                    let message = format!("{} is marked no-connect but is on {}: delete the marker, or the wire", p.reference(), net.name);
                    Draft::new(Some(p.at), vec![Item::Marker(m.id), Item::Pin(p.clone())], message)
                        .on(&net.name)
                        .fix("ecad.schematic.delete", json!({"items": [m.id]}))
                });
            }
            _ => {}
        }
    }
}

/// The checks of names across the sheet: spellings that differ only in case, and one name used
/// by more than one kind of label.
fn name_checks(s: &Sheet, f: &mut Found) {
    // Lower case → spelling → its lowest label; name → kind → its lowest label.
    let mut spellings: BTreeMap<String, BTreeMap<&str, &Label>> = BTreeMap::new();
    let mut kinds: BTreeMap<&str, BTreeMap<u8, &Label>> = BTreeMap::new();
    for l in &s.sheet.labels {
        let slot = spellings.entry(l.name.to_lowercase()).or_default().entry(l.name.as_str()).or_insert(l);
        if l.id < slot.id {
            *slot = l;
        }
        let kind = match l.kind {
            LabelKind::Net => 0,
            LabelKind::Rail => 1,
            LabelKind::Ground => 2,
        };
        let slot = kinds.entry(l.name.as_str()).or_default().entry(kind).or_insert(l);
        if l.id < slot.id {
            *slot = l;
        }
    }
    let at = |l: &Label| s.pins.anchor(&l.at);
    let item = |l: &Label| Item::Label { id: l.id, name: l.name.clone() };
    for by in spellings.values().filter(|v| v.len() >= 2) {
        let mut labels: Vec<&Label> = by.values().copied().collect();
        labels.sort_by_key(|l| l.id);
        let key: Vec<String> = labels.iter().take(2).map(|l| format!("l{}", l.id)).collect();
        f.add("names_differ_by_case", &key, || {
            let list = labels.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ");
            let message = format!("{list} differ only in case, so they are separate nets: use one spelling");
            Draft::new(labels.first().and_then(|l| at(l)), labels.iter().map(|l| item(l)).collect(), message)
        });
    }
    for (name, by) in kinds.iter().filter(|(_, v)| v.len() >= 2) {
        let mut labels: Vec<&Label> = by.values().copied().collect();
        labels.sort_by_key(|l| l.id);
        let Some(first) = labels.first() else { continue };
        f.add("name_kind_clash", &[format!("l{}", first.id)], || {
            let what: Vec<&str> = by
                .keys()
                .map(|k| match k {
                    0 => "a net label",
                    1 => "a rail",
                    _ => "ground",
                })
                .collect();
            let message = format!("{name} is used as {}: one name should be one kind of label", what.join(" and as "));
            Draft::new(at(first), labels.iter().map(|l| item(l)).collect(), message)
        });
    }
}

/// The checks of parts: overlapping bodies, values, designators, footprints and the library.
fn part_checks(s: &mut Sheet, f: &mut Found) {
    let part_item = |p: &Part| Item::Part { id: p.id, designator: p.designator.clone() };
    let mut bodies = bodies(s);
    bodies.sort_by(|(_, a), (_, b)| a[0][0].total_cmp(&b[0][0]));
    let mut budget = s.budget;
    'bodies: for (i, (p, a)) in bodies.iter().enumerate() {
        for (q, b) in bodies.iter().skip(i + 1) {
            if b[0][0] >= a[1][0] {
                break;
            }
            if budget == 0 {
                break 'bodies;
            }
            budget -= 1;
            if b[0][1] >= a[1][1] || a[0][1] >= b[1][1] {
                continue;
            }
            let (x, y) = if p.id < q.id { (p, q) } else { (q, p) };
            f.add("parts_overlap", &[format!("p{}", x.id), format!("p{}", y.id)], || {
                let message = format!("{} and {} overlap: move one of them", x.designator, y.designator);
                let middle = [(a[0][0].max(b[0][0]) + a[1][0].min(b[1][0])) / 2.0, (a[0][1].max(b[0][1]) + a[1][1].min(b[1][1])) / 2.0];
                Draft::new(Some(middle), vec![part_item(x), part_item(y)], message)
            });
        }
    }
    s.budget = budget;

    // Library problems, one finding per component with the parts that use it.
    let mut troubled: BTreeMap<u64, (Vec<&Part>, Vec<String>)> = BTreeMap::new();
    for part in &s.sheet.parts {
        let Some(c) = s.ecad.component(part.component) else { continue };
        let at = s.library.get(&part.component).and_then(|x| super::schematic::part_bbox(part, x.symbol)).map(|[a, _]| a).or(Some(part.placement.at));
        let shown = part.value.clone().unwrap_or_else(|| c.value.clone());
        if shown.trim().is_empty() {
            f.add("value_missing", &[format!("p{}", part.id)], || {
                let message = format!("{} has no value: give it one, such as 10k or 100nF", part.designator);
                Draft::new(at, vec![part_item(part)], message)
            });
        }
        let letters = match split_designator(&part.designator) {
            Some((head, _)) => head.to_string(),
            None => part.designator.chars().take_while(char::is_ascii_alphabetic).collect(),
        };
        if !letters.eq_ignore_ascii_case(&c.prefix) {
            f.add("designator_prefix_mismatch", &[format!("p{}", part.id)], || {
                let message =
                    format!("{} is a {} (prefix {}): renumber it, such as with Renumber keeping the others", part.designator, c.name, c.prefix);
                Draft::new(at, vec![part_item(part)], message).fix("ecad.schematic.annotate", json!({"order": "position", "keep_existing": true}))
            });
        }
        let r = s.ecad.resolve(c, part.variant.as_deref());
        if r.variant.and_then(|v| v.footprint).is_none() {
            f.add("part_no_footprint", &[format!("p{}", part.id)], || {
                let variant = r.variant.map(|v| format!(" (variant {})", v.name)).unwrap_or_default();
                let message = format!(
                    "{}'s component {}{variant} has no footprint, so the board can't place it: give the variant one",
                    part.designator, c.name
                );
                Draft::new(at, vec![part_item(part)], message)
            });
        }
        let problems: Vec<String> = r
            .symbol_problems()
            .into_iter()
            .chain(r.problems().into_iter().filter(|x| x.severity == Severity::Error))
            .map(|x| format!("{}: {}", x.code, x.message))
            .collect();
        if !problems.is_empty() {
            let slot = troubled.entry(c.id).or_default();
            slot.0.push(part);
            for x in problems {
                if !slot.1.contains(&x) {
                    slot.1.push(x);
                }
            }
        }
    }
    for (id, (parts, problems)) in &troubled {
        let Some(c) = s.ecad.component(*id) else { continue };
        f.add("library_problem", &[format!("c{id}")], || {
            let first = problems.first().cloned().unwrap_or_default();
            let others = if problems.len() > 1 { format!(" (and {} more)", problems.len() - 1) } else { String::new() };
            let placed = parts.iter().map(|p| p.designator.as_str()).collect::<Vec<_>>().join(", ");
            let message = format!("{} has problems in the library, {first}{others}; it is placed as {placed}: fix it in the library", c.name);
            let at = parts.first().map(|p| p.placement.at);
            Draft::new(at, parts.iter().map(|p| part_item(p)).collect(), message)
        });
    }
}
