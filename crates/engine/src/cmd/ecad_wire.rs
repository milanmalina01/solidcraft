//! Wires, net labels, power ports and markers on the schematic, net renaming, and
//! `ecad.schematic.add`, which puts parts and all of these on the sheet in one call. Each call works
//! on a copy of the sheet and stores it whole once it passes the design file's checks, so a refused
//! call leaves the design as it was, a call is one undo step, and a call that changes nothing (a
//! wire, label or marker the sheet already has is not added twice) makes none. Replies give each
//! new wire, label and marker its net.

use std::collections::{BTreeMap, BTreeSet};
use std::iter::once;

use serde_json::Value;
use solidcraft_doc::Document;
use solidcraft_doc::ecad::connect::{Connectivity, connectivity};
use solidcraft_doc::ecad::place::{lattice, snap_point};
use solidcraft_doc::ecad::route::{PortRoom, RouteEnd, STUB, angle, auto_route, auto_route_through, opposite, simplify};
use solidcraft_doc::ecad::schematic::{
    MAX_LABELS, MAX_MARKERS, MAX_PARTS, MAX_PLACED_PINS, MAX_WIRE_POINTS, MAX_WIRES, Pins, check_net_name, check_sheet_point, part_bbox,
};
use solidcraft_doc::ecad::{Anchor, Label, LabelKind, Library, Marker, MarkerKind, PinDirection, Schematic, Wire};

use super::CommandSpec;
use super::ecad::{parse, reply, store_schematic};
use super::ecad_netlist::{find_net, net_out};
use super::ecad_schematic::{Index, find_pin, kind_name, place_rows, quarter, sheet_point, sheet_result};
use crate::params::bad;
use crate::schema::ecad_schematic::{ItemKind as KindOut, ItemOut, PinRef, PlaceSpec};
use crate::schema::ecad_wire::{
    AddParams, ConnectBy, ConnectParams, ConnectSpec, CrossOut, End, LabelParams, LabelSpec, MarkerKind as MarkerKindIn, MarkerParams, MarkerSpec,
    NetRenameOut, OnWire, PowerParams, PowerSpec, PowerStyle, RenameParams, Route, WireEditParams, WireParams, WireResult, WireSpec,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

/// How the ends of rows are given, for the `.params` texts.
macro_rules! end_doc {
    () => {
        "a pin \"R1.2\" (designator, dot, pin number or name) | {part, pin}, a point [x, y] in mm (on a pin: that pin), or a point on a wire {wire, at} (a T; off the wire, the error names the nearest point on it)"
    };
}

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.schematic.add", "Add to Schematic", crate::checked!("ecad.schematic.add", add))
        .icon("new")
        .doc_only()
        .contract(&ADD_CONTRACT)
        .output(output_schema_for::<WireResult>)
        .params(
            "parts?, wires?, connect?, labels?, power?, markers? (rows as ecad.schematic.place, .wire, .connect (its nets), .label, .power (its ports) and .marker take them) → {created, changed, removed, parts, nets, crosses}: everything in one undo step, all or none, in that order, so later rows can name the parts placed here by the designators they give or get",
        ),
    CommandSpec::new("ecad.schematic.wire", "Wire", crate::checked!("ecad.schematic.wire", wire))
        .icon("line")
        .doc_only()
        .contract(&WIRE_CONTRACT)
        .output(output_schema_for::<WireResult>)
        .params(concat!(
            "wires: [{from, via? [[x, y]…], to, route? auto|straight}]; an end is ",
            end_doc!(),
            " → {created: [{id, kind, net}], nets, crosses}. auto (the default): 2.54 mm straight out of each pin, then an L (horizontal first unless its corner is inside a part), a Z through the middle for pins facing each other, or one straight segment when they line up; straight: straight from point to point. crosses: where the new wires cross others without joining"
        )),
    CommandSpec::new("ecad.schematic.connect", "Connect Pins", crate::checked!("ecad.schematic.connect", connect))
        .icon("chain")
        .doc_only()
        .contract(&CONNECT_CONTRACT)
        .output(output_schema_for::<WireResult>)
        .params(
            "nets: [{pins: [\"R1.2\" | {part, pin}…], name?, by? wires|labels}] → {created, nets, crosses}. wires (the default): an auto-routed wire from each pin to the next (as ecad.schematic.wire routes them), and with name, a net label on the first pin; labels: a 2.54 mm wire out of each pin ending in a net label `name` (name needed), so no wire crosses the sheet. crosses: where the new wires cross others without joining",
        ),
    CommandSpec::new("ecad.schematic.label", "Label", crate::checked!("ecad.schematic.label", label))
        .icon("text")
        .doc_only()
        .contract(&LABEL_CONTRACT)
        .output(output_schema_for::<WireResult>)
        .params(concat!(
            "labels: [{name (1 to 64 characters, no spaces or `:`), at, rotation? (degrees, a multiple of 90; left out: away from the pin it is on, else 0)}]; at is ",
            end_doc!(),
            " → {created, nets}: net labels; labels and power ports with the same name are one net"
        )),
    CommandSpec::new("ecad.schematic.power", "Power", crate::checked!("ecad.schematic.power", power))
        .icon("point")
        .doc_only()
        .contract(&POWER_CONTRACT)
        .output(output_schema_for::<WireResult>)
        .params(concat!(
            "ports: [{name (such as +3V3 or GND), at, rotation? (degrees, a multiple of 90, the way it points; left out: up for a rail and down for ground, but straight out from its pin where that would point into the part, the pin is one of a column of pins on a side, or it would run over another pin or label), style? rail|ground (left out: ground for names starting GND or VSS and for 0V, else a rail)}]; at is ",
            end_doc!(),
            " → {created, nets}: power ports; a port supplies its net, and ports and labels with the same name are one net"
        )),
    CommandSpec::new("ecad.schematic.marker", "No Connect", crate::checked!("ecad.schematic.marker", marker))
        .icon("point")
        .doc_only()
        .contract(&MARKER_CONTRACT)
        .output(output_schema_for::<WireResult>)
        .params(concat!(
            "markers: [{kind?: no_connect|power_flag (left out: no_connect), at}]; at is ",
            end_doc!(),
            " → {created, nets}: no_connect goes on a pin left unconnected on purpose; power_flag marks a net powered from off the sheet"
        )),
    CommandSpec::new("ecad.schematic.marker.power_flag", "Power Flag", crate::checked!("ecad.schematic.marker.power_flag", power_flag))
        .icon("point")
        .doc_only()
        .contract(&MARKER_CONTRACT)
        .output(output_schema_for::<WireResult>)
        .params(
            "as ecad.schematic.marker, with each marker's kind? power_flag when left out → {created, nets}: a power flag on a pin or a wire point says its net is powered from off the sheet (a supply the electrical check cannot see)",
        ),
    CommandSpec::new("ecad.schematic.wire.edit", "Edit Wire", crate::checked!("ecad.schematic.wire.edit", wire_edit))
        .icon("edit")
        .doc_only()
        .contract(&WIRE_EDIT_CONTRACT)
        .output(output_schema_for::<WireResult>)
        .params("wire: id; via: [[x, y], …] (the bends in mm, from its first end; empty: straight) → {created, changed, …, nets}: its ends, on pins or points, stay; a level or upright segment that slides straight across itself (a segment drag) takes along the T feet, labels and markers on it, and the wires on those feet hang on as ecad.schematic.move hangs them"),
    CommandSpec::new("ecad.schematic.net.rename", "Rename Net", crate::checked!("ecad.schematic.net.rename", rename))
        .icon("rename")
        .doc_only()
        .contract(&RENAME_CONTRACT)
        .output(output_schema_for::<WireResult>)
        .params(
            "net (its name, such as VCC or net:R1.2, or a pin on it, such as R1.2), to (1 to 64 characters, no spaces or `:`), merge? (false: refused, listing both nets' pins, when another net has the name `to`) → {created, changed, nets, renamed: {from, to, joined?}}: every label and power port on the net takes the name; an unnamed net gains a net label on its lowest pin (or its lowest wire's first end); a net class that lists the net lists it by its new name (in the same undo step)",
        ),
];

const ADD_CONTRACT: TypedContract<AddParams> = TypedContract::new();
const WIRE_CONTRACT: TypedContract<WireParams> = TypedContract::new();
const CONNECT_CONTRACT: TypedContract<ConnectParams> = TypedContract::new();
const LABEL_CONTRACT: TypedContract<LabelParams> = TypedContract::new();
const POWER_CONTRACT: TypedContract<PowerParams> = TypedContract::new();
const MARKER_CONTRACT: TypedContract<MarkerParams> = TypedContract::new();
const RENAME_CONTRACT: TypedContract<RenameParams> = TypedContract::new();
const WIRE_EDIT_CONTRACT: TypedContract<WireEditParams> = TypedContract::new();

/// The most crossings a reply lists.
const MAX_CROSSES: usize = 64;
/// The most segment pairs tested for crossings in one reply.
const MAX_CROSS_TESTS: usize = 1 << 22;
/// The most pins a reply's net-rename clash lists for each net.
const MAX_CLASH_PINS: usize = 16;

/// What one call puts on the sheet, row lists in the order `add` takes them; `*_key` names the
/// lists in errors.
struct Rows {
    parts: Vec<PlaceSpec>,
    wires: Vec<WireSpec>,
    connect: Vec<ConnectSpec>,
    labels: Vec<LabelSpec>,
    power: Vec<PowerSpec>,
    markers: Vec<MarkerSpec>,
    connect_key: &'static str,
    power_key: &'static str,
}

impl Rows {
    fn new(connect_key: &'static str, power_key: &'static str) -> Self {
        Rows {
            parts: Vec::new(),
            wires: Vec::new(),
            connect: Vec::new(),
            labels: Vec::new(),
            power: Vec::new(),
            markers: Vec::new(),
            connect_key,
            power_key,
        }
    }
}

fn add(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.add";
    let p: AddParams = parse(CMD, p)?;
    let rows = Rows {
        parts: p.parts.unwrap_or_default(),
        wires: p.wires.unwrap_or_default(),
        connect: p.connect.unwrap_or_default(),
        labels: p.labels.unwrap_or_default(),
        power: p.power.unwrap_or_default(),
        markers: p.markers.unwrap_or_default(),
        ..Rows::new("connect", "power")
    };
    if rows.parts.is_empty()
        && rows.wires.is_empty()
        && rows.connect.is_empty()
        && rows.labels.is_empty()
        && rows.power.is_empty()
        && rows.markers.is_empty()
    {
        return Err(bad(CMD, "give rows in at least one of `parts`, `wires`, `connect`, `labels`, `power` and `markers`"));
    }
    run(s, rows, CMD)
}

fn wire(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.wire";
    let p: WireParams = parse(CMD, p)?;
    if p.wires.is_empty() {
        return Err(bad(CMD, "give 1 or more wires in `wires`"));
    }
    run(s, Rows { wires: p.wires, ..Rows::new("connect", "power") }, CMD)
}

fn wire_edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.wire.edit";
    let p: WireEditParams = parse(CMD, p)?;
    if p.via.len() > MAX_WIRE_POINTS {
        return Err(bad(CMD, format!("a wire has at most {MAX_WIRE_POINTS} bends")));
    }
    for (i, v) in p.via.iter().enumerate() {
        check_sheet_point(&format!("via[{i}]"), *v).map_err(|e| bad(CMD, e))?;
    }
    let old = s.doc.ecad.schematic.clone();
    if !old.wires.iter().any(|w| w.id == p.wire) {
        return Err(bad(CMD, format!("the schematic has no wire {}", p.wire)));
    }
    // What stood on a segment that slid across itself comes along, and wires on it hang on.
    let sheet = solidcraft_doc::ecad::route::reshaped(&old, &s.doc.ecad.library(), p.wire, &p.via);
    store_schematic(s, sheet, CMD)?;
    wire_result(s, &old, None)
}

fn connect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.connect";
    let p: ConnectParams = parse(CMD, p)?;
    if p.nets.is_empty() {
        return Err(bad(CMD, "give 1 or more nets in `nets`"));
    }
    run(s, Rows { connect: p.nets, ..Rows::new("nets", "power") }, CMD)
}

fn label(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.label";
    let p: LabelParams = parse(CMD, p)?;
    if p.labels.is_empty() {
        return Err(bad(CMD, "give 1 or more labels in `labels`"));
    }
    run(s, Rows { labels: p.labels, ..Rows::new("connect", "power") }, CMD)
}

fn power(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.power";
    let p: PowerParams = parse(CMD, p)?;
    if p.ports.is_empty() {
        return Err(bad(CMD, "give 1 or more power ports in `ports`"));
    }
    run(s, Rows { power: p.ports, ..Rows::new("connect", "ports") }, CMD)
}

fn marker(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.marker";
    let p: MarkerParams = parse(CMD, p)?;
    if p.markers.is_empty() {
        return Err(bad(CMD, "give 1 or more markers in `markers`"));
    }
    run(s, Rows { markers: p.markers, ..Rows::new("connect", "power") }, CMD)
}

/// `ecad.schematic.marker` with power flags where no kind is given: the toolbar's Power Flag.
fn power_flag(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.marker.power_flag";
    let mut p: MarkerParams = parse(CMD, p)?;
    if p.markers.is_empty() {
        return Err(bad(CMD, "give 1 or more markers in `markers`"));
    }
    for m in &mut p.markers {
        m.kind.get_or_insert(MarkerKindIn::PowerFlag);
    }
    run(s, Rows { markers: p.markers, ..Rows::new("connect", "power") }, CMD)
}

/// Put `rows` on a copy of the sheet, in order, and store it.
fn run(s: &mut Session, rows: Rows, cmd: &str) -> Result<Value> {
    for (key, n, max) in [
        ("parts", rows.parts.len(), MAX_PARTS),
        ("wires", rows.wires.len(), MAX_WIRES),
        (rows.connect_key, rows.connect.len(), MAX_WIRES),
        ("labels", rows.labels.len(), MAX_LABELS),
        (rows.power_key, rows.power.len(), MAX_LABELS),
        ("markers", rows.markers.len(), MAX_MARKERS),
    ] {
        if n > max {
            return Err(bad(cmd, format!("give at most {max} rows in `{key}`")));
        }
    }
    let old = s.doc.ecad.schematic.clone();
    let mut sheet = old.clone();
    {
        let doc: &Document = &s.doc;
        let library = doc.ecad.library();
        if !rows.parts.is_empty() {
            place_rows(doc, &library, &mut sheet, rows.parts, cmd)?;
        }
        let new = {
            let view = View::new(&sheet, &library, cmd);
            let mut new = New::default();
            for (i, r) in rows.wires.iter().enumerate() {
                new.wires.push(view.wire(r, &format!("wires[{i}]"))?);
            }
            for (i, r) in rows.connect.iter().enumerate() {
                view.connect(r, &format!("{}[{i}]", rows.connect_key), &mut new)?;
                if new.wires.len() > MAX_WIRES {
                    return Err(bad(cmd, format!("a schematic holds at most {MAX_WIRES} wires")));
                }
            }
            for (i, r) in rows.labels.iter().enumerate() {
                new.labels.push(view.label(r, &format!("labels[{i}]"))?);
            }
            // Ports keep clear of the pins and labels already there, and of each other.
            let mut room = (!rows.power.is_empty()).then(|| PortRoom::new(&sheet, &library));
            for (i, r) in rows.power.iter().enumerate() {
                let port = view.power(r, &format!("{}[{i}]", rows.power_key), room.as_ref())?;
                if let Some(room) = room.as_mut() {
                    room.add(&port);
                }
                new.labels.push(port);
            }
            for (i, r) in rows.markers.iter().enumerate() {
                new.markers.push(view.marker(r, &format!("markers[{i}]"))?);
            }
            new
        };
        new.apply(&mut sheet, cmd)?;
    }
    store_schematic(s, sheet, cmd)?;
    wire_result(s, &old, None)
}

/// What a call adds, before the objects get ids.
#[derive(Default)]
struct New {
    wires: Vec<Wire>,
    labels: Vec<Label>,
    markers: Vec<Marker>,
}

/// An anchor as a sortable key: points by their lattice point.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Pin(u64, String),
    Point([i64; 2]),
}

fn key(a: &Anchor) -> Key {
    match a {
        Anchor::Pin { part, pin } => Key::Pin(*part, pin.clone()),
        Anchor::Point(p) => Key::Point(lattice(*p)),
    }
}

/// A wire as a key, the same whichever way it runs.
fn wire_key(w: &Wire) -> (Key, Vec<[i64; 2]>, Key) {
    let ahead = (key(&w.from), w.via.iter().map(|p| lattice(*p)).collect::<Vec<_>>(), key(&w.to));
    let back = (key(&w.to), w.via.iter().rev().map(|p| lattice(*p)).collect::<Vec<_>>(), key(&w.from));
    ahead.min(back)
}

fn label_key(l: &Label) -> (String, u8, Key) {
    let kind = match l.kind {
        LabelKind::Net => 0,
        LabelKind::Rail => 1,
        LabelKind::Ground => 2,
    };
    (l.name.clone(), kind, key(&l.at))
}

fn marker_key(m: &Marker) -> (u8, Key) {
    (u8::from(m.kind == MarkerKind::PowerFlag), key(&m.at))
}

impl New {
    /// Give the new objects ids and put them on `sheet`, leaving out any the sheet already has.
    fn apply(self, sheet: &mut Schematic, cmd: &str) -> Result<()> {
        let mut wires: BTreeSet<_> = sheet.wires.iter().map(wire_key).collect();
        for mut w in self.wires {
            if wires.insert(wire_key(&w)) {
                w.id = sheet.take_id().map_err(|e| bad(cmd, e))?;
                sheet.wires.push(w);
            }
        }
        let mut labels: BTreeSet<_> = sheet.labels.iter().map(label_key).collect();
        for mut l in self.labels {
            if labels.insert(label_key(&l)) {
                l.id = sheet.take_id().map_err(|e| bad(cmd, e))?;
                sheet.labels.push(l);
            }
        }
        let mut markers: BTreeSet<_> = sheet.markers.iter().map(marker_key).collect();
        for mut m in self.markers {
            if markers.insert(marker_key(&m)) {
                m.id = sheet.take_id().map_err(|e| bad(cmd, e))?;
                sheet.markers.push(m);
            }
        }
        Ok(())
    }
}

/// A row's end, resolved: what it is anchored on, where that is, and at a pin, the way a wire
/// leaves it.
#[derive(Clone)]
struct EndAt {
    anchor: Anchor,
    at: [f64; 2],
    out: Option<PinDirection>,
}

impl EndAt {
    fn route(&self) -> RouteEnd {
        RouteEnd { at: self.at, out: self.out }
    }
}

/// The sheet as it is, looked up once to resolve rows.
struct View<'a> {
    sheet: &'a Schematic,
    library: &'a Library<'a>,
    index: Index,
    pins: Pins<'a>,
    /// The lowest part and pin on each lattice point.
    on: BTreeMap<[i64; 2], (u64, String)>,
    /// The parts' boxes, which the auto route's corners keep out of.
    boxes: Vec<[[f64; 2]; 2]>,
    cmd: &'a str,
}

impl<'a> View<'a> {
    fn new(sheet: &'a Schematic, library: &'a Library<'a>, cmd: &'a str) -> Self {
        let mut on = BTreeMap::new();
        let mut boxes = Vec::with_capacity(sheet.parts.len());
        for p in &sheet.parts {
            let Some(c) = library.get(&p.component) else { continue };
            boxes.extend(part_bbox(p, c.symbol));
            for pin in &c.symbol.pins {
                on.entry(p.placement.lattice_point(pin.at)).or_insert_with(|| (p.id, pin.number.clone()));
            }
        }
        View { sheet, library, index: Index::new(sheet), pins: Pins::new(sheet, library), on, boxes, cmd }
    }

    fn bad(&self, msg: String) -> crate::EngineError {
        bad(self.cmd, msg)
    }

    /// The pin `number` of `part` as an end.
    fn pin_end(&self, part: u64, number: String, what: &str) -> Result<EndAt> {
        let (Some(p), Some(q)) = (self.pins.part(part), self.pins.pin(part, &number)) else {
            return Err(self.bad(format!("{what}: part {part} has no pin `{number}`")));
        };
        let placed = p.placement.pin(q);
        Ok(EndAt { anchor: Anchor::Pin { part, pin: number }, at: snap_point(placed.at), out: Some(opposite(placed.direction)) })
    }

    /// A pin a row names.
    fn pin(&self, r: &PinRef, what: &str) -> Result<EndAt> {
        let (part, number) = find_pin(self.sheet, &self.index, &self.pins, self.library, r, self.cmd, what)?;
        self.pin_end(part, number, what)
    }

    /// A point (on the lattice): the pin there, if there is one.
    fn point_end(&self, at: [f64; 2], what: &str) -> Result<EndAt> {
        match self.on.get(&lattice(at)) {
            Some((part, pin)) => self.pin_end(*part, pin.clone(), what),
            None => Ok(EndAt { anchor: Anchor::Point(at), at, out: None }),
        }
    }

    /// A point on a wire.
    fn on_wire(&self, o: &OnWire, what: &str) -> Result<EndAt> {
        let Some(w) = self.sheet.wires.iter().find(|w| w.id == o.wire) else {
            let msg = match self.sheet.kind_of(o.wire) {
                Some(k) => format!("{what}: {} is a {}, not a wire", o.wire, kind_name(k)),
                None => format!("{what}: the schematic has no wire with the id {}", o.wire),
            };
            return Err(self.bad(msg));
        };
        let at = sheet_point(o.at, self.cmd, &format!("{what}: `at`"))?;
        let points = self.pins.wire(w);
        let q = lattice(at);
        let on = points.iter().any(|p| lattice(*p) == q) || points.windows(2).any(|s| matches!(s, [a, b] if on_segment(lattice(*a), lattice(*b), q)));
        if !on {
            let mut msg = format!("{what}: ({}, {}) is not on wire {}", at[0], at[1], o.wire);
            if let Some(n) = nearest(&points, at) {
                msg.push_str(&format!("; the nearest point on it is ({}, {})", n[0], n[1]));
            }
            return Err(self.bad(msg));
        }
        self.point_end(at, what)
    }

    fn end(&self, e: &End, what: &str) -> Result<EndAt> {
        match e {
            End::Pin(r) => self.pin(r, what),
            End::Point(p) => {
                let at = sheet_point(*p, self.cmd, what)?;
                self.point_end(at, what)
            }
            End::OnWire(o) => self.on_wire(o, what),
        }
    }

    /// A wire through `points` (its ends first and last) between two ends.
    fn wire_between(&self, from: &EndAt, points: Vec<[f64; 2]>, to: &EndAt, what: &str) -> Result<Wire> {
        let n = points.len();
        if n < 2 {
            return Err(self.bad(format!("{what}: the wire starts and ends at the same point ({}, {})", from.at[0], from.at[1])));
        }
        if n > MAX_WIRE_POINTS {
            return Err(self.bad(format!("{what}: a wire has at most {MAX_WIRE_POINTS} points")));
        }
        let via: Vec<[f64; 2]> = points.get(1..n - 1).map(<[_]>::to_vec).unwrap_or_default();
        for v in &via {
            check_sheet_point("the wire's route", *v).map_err(|e| self.bad(format!("{what}: {e}")))?;
        }
        Ok(Wire { id: 0, from: from.anchor.clone(), via, to: to.anchor.clone() })
    }

    fn wire(&self, r: &WireSpec, what: &str) -> Result<Wire> {
        let from = self.end(&r.from, &format!("{what}.from"))?;
        let to = self.end(&r.to, &format!("{what}.to"))?;
        let given = r.via.as_deref().unwrap_or_default();
        if given.len() > MAX_WIRE_POINTS {
            return Err(self.bad(format!("{what}: a wire has at most {MAX_WIRE_POINTS} points")));
        }
        let mut via = Vec::with_capacity(given.len());
        for (k, v) in given.iter().enumerate() {
            via.push(sheet_point(*v, self.cmd, &format!("{what}.via[{k}]"))?);
        }
        let points = match r.route.unwrap_or_default() {
            Route::Straight => simplify(once(from.at).chain(via).chain(once(to.at))),
            Route::Auto => {
                let ends: Vec<RouteEnd> =
                    once(from.route()).chain(via.iter().map(|v| RouteEnd { at: *v, out: None })).chain(once(to.route())).collect();
                auto_route_through(&ends, &self.boxes)
            }
        };
        self.wire_between(&from, points, &to, what)
    }

    /// One `connect` row: wires from pin to pin, or a stub and a label on each.
    fn connect(&self, r: &ConnectSpec, what: &str, new: &mut New) -> Result<()> {
        if let Some(name) = &r.name {
            check_net_name(name).map_err(|e| self.bad(format!("{what}: {e}")))?;
        }
        if r.pins.len() > MAX_PLACED_PINS {
            return Err(self.bad(format!("{what}: give at most {MAX_PLACED_PINS} pins")));
        }
        let mut ends = Vec::with_capacity(r.pins.len());
        for (k, p) in r.pins.iter().enumerate() {
            ends.push(self.pin(p, &format!("{what}.pins[{k}]"))?);
        }
        match r.by.unwrap_or_default() {
            ConnectBy::Wires => {
                if ends.len() < 2 {
                    return Err(self.bad(format!(
                        "{what}: give two or more pins to wire together (to name one pin's net, use ecad.schematic.label, or by: labels)"
                    )));
                }
                for pair in ends.windows(2) {
                    let [a, b] = pair else { continue };
                    if a.anchor == b.anchor {
                        continue;
                    }
                    let points = auto_route(a.route(), b.route(), &self.boxes);
                    new.wires.push(self.wire_between(a, points, b, what)?);
                }
                if let (Some(name), Some(first)) = (&r.name, ends.first()) {
                    let rotation = first.out.map_or(0, angle);
                    new.labels.push(Label { id: 0, name: name.clone(), kind: LabelKind::Net, at: first.anchor.clone(), rotation });
                }
            }
            ConnectBy::Labels => {
                let Some(name) = &r.name else {
                    return Err(self.bad(format!("{what}: `by: labels` needs the net's `name`")));
                };
                if ends.is_empty() {
                    return Err(self.bad(format!("{what}: give one or more pins")));
                }
                for e in &ends {
                    let out = e.out.unwrap_or(PinDirection::Right);
                    let [ux, uy] = out.unit();
                    let tip = snap_point([e.at[0] + ux * STUB, e.at[1] + uy * STUB]);
                    check_sheet_point("the label's stub", tip).map_err(|err| self.bad(format!("{what}: {err}")))?;
                    let tip_end = EndAt { anchor: Anchor::Point(tip), at: tip, out: None };
                    new.wires.push(self.wire_between(e, vec![e.at, tip], &tip_end, what)?);
                    new.labels.push(Label { id: 0, name: name.clone(), kind: LabelKind::Net, at: Anchor::Point(tip), rotation: angle(out) });
                }
            }
        }
        Ok(())
    }

    /// A row's turn, or `default`.
    fn rotation(&self, r: Option<i32>, default: u16, what: &str) -> Result<u16> {
        match r {
            Some(r) => quarter(Some(r), self.cmd, &format!("{what}: ")),
            None => Ok(default),
        }
    }

    fn label(&self, r: &LabelSpec, what: &str) -> Result<Label> {
        check_net_name(&r.name).map_err(|e| self.bad(format!("{what}: {e}")))?;
        let at = self.end(&r.at, &format!("{what}.at"))?;
        let rotation = self.rotation(r.rotation, at.out.map_or(0, angle), what)?;
        Ok(Label { id: 0, name: r.name.clone(), kind: LabelKind::Net, at: at.anchor, rotation })
    }

    fn power(&self, r: &PowerSpec, what: &str, room: Option<&PortRoom>) -> Result<Label> {
        check_net_name(&r.name).map_err(|e| self.bad(format!("{what}: {e}")))?;
        let at = self.end(&r.at, &format!("{what}.at"))?;
        let ground = match r.style {
            Some(PowerStyle::Ground) => true,
            Some(PowerStyle::Rail) => false,
            None => ground_name(&r.name),
        };
        // Up for a rail and down for ground; straight out from a pin where that would point into
        // the part, the pin is one of a column (so ports on neighbouring pins stand side by side)
        // or it would run over another pin or label (see `PortRoom::turn`).
        let (kind, default) = if ground { (LabelKind::Ground, 270) } else { (LabelKind::Rail, 90) };
        let mut port = Label { id: 0, name: r.name.clone(), kind, at: at.anchor, rotation: default };
        let usual = room.map_or_else(|| at.out.map_or(default, angle), |room| room.turn(&port));
        port.rotation = self.rotation(r.rotation, usual, what)?;
        Ok(port)
    }

    fn marker(&self, r: &MarkerSpec, what: &str) -> Result<Marker> {
        let at = self.end(&r.at, &format!("{what}.at"))?;
        let kind = match r.kind.unwrap_or(MarkerKindIn::NoConnect) {
            MarkerKindIn::NoConnect => MarkerKind::NoConnect,
            MarkerKindIn::PowerFlag => MarkerKind::PowerFlag,
        };
        if kind == MarkerKind::NoConnect && at.anchor.pin().is_none() {
            return Err(self.bad(format!("{what}: a no-connect marker goes on a pin, such as \"U1.5\"")));
        }
        Ok(Marker { id: 0, kind, at: at.anchor })
    }
}

/// Whether a power port named `name` is drawn as ground when no style is given: names starting
/// GND or VSS, and 0V, ignoring case.
fn ground_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.starts_with("GND") || n.starts_with("VSS") || n == "0V"
}

/// Whether `p` lies on the segment `a`–`b`, ends included (exact).
fn on_segment(a: [i64; 2], b: [i64; 2], p: [i64; 2]) -> bool {
    let w = |v: i64| i128::from(v);
    let collinear =
        (w(b[0]) - w(a[0])).checked_mul(w(p[1]) - w(a[1])).zip((w(b[1]) - w(a[1])).checked_mul(w(p[0]) - w(a[0]))).is_some_and(|(u, v)| u == v);
    collinear && p[0] >= a[0].min(b[0]) && p[0] <= a[0].max(b[0]) && p[1] >= a[1].min(b[1]) && p[1] <= a[1].max(b[1])
}

/// The point of the polyline `points` nearest to `p`, on the lattice.
fn nearest(points: &[[f64; 2]], p: [f64; 2]) -> Option<[f64; 2]> {
    let d2 = |q: [f64; 2]| (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2);
    let mut best: Option<[f64; 2]> = points.first().copied();
    for pair in points.windows(2) {
        let [a, b] = pair else { continue };
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let q = [a[0] + t * dx, a[1] + t * dy];
        if best.is_none_or(|b| d2(q) < d2(b)) {
            best = Some(q);
        }
    }
    best.map(snap_point)
}

/// The sign of the turn `a` → `b` → `c` (exact; `None` past i128).
fn turn(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> Option<i8> {
    let w = |v: i64| i128::from(v);
    let u = (w(b[0]) - w(a[0])).checked_mul(w(c[1]) - w(a[1]))?;
    let v = (w(b[1]) - w(a[1])).checked_mul(w(c[0]) - w(a[0]))?;
    Some(match u.cmp(&v) {
        std::cmp::Ordering::Greater => 1,
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
    })
}

/// Where the segments `a`–`b` and `c`–`d` cross inside both (not at an end, not along a line).
fn crossing(a: [i64; 2], b: [i64; 2], c: [i64; 2], d: [i64; 2]) -> Option<[f64; 2]> {
    let (t1, t2, t3, t4) = (turn(a, b, c)?, turn(a, b, d)?, turn(c, d, a)?, turn(c, d, b)?);
    if t1 * t2 >= 0 || t3 * t4 >= 0 {
        return None;
    }
    let f = |q: [i64; 2]| [q[0] as f64, q[1] as f64];
    let (a, b, c, d) = (f(a), f(b), f(c), f(d));
    let (r, s) = ([b[0] - a[0], b[1] - a[1]], [d[0] - c[0], d[1] - c[1]]);
    let den = r[0] * s[1] - r[1] * s[0];
    if den == 0.0 {
        return None;
    }
    let t = ((c[0] - a[0]) * s[1] - (c[1] - a[1]) * s[0]) / den;
    Some(snap_point([(a[0] + t * r[0]) / 1e4, (a[1] + t * r[1]) / 1e4]))
}

/// Where the wires `new` cross other wires without joining, at most [`MAX_CROSSES`].
fn crosses(sheet: &Schematic, library: &Library, new: &BTreeSet<u64>) -> Vec<CrossOut> {
    if new.is_empty() {
        return Vec::new();
    }
    let pins = Pins::new(sheet, library);
    let segments = |w: &Wire| -> Vec<([i64; 2], [i64; 2])> {
        let points: Vec<[i64; 2]> = pins.wire(w).into_iter().map(lattice).collect();
        points.windows(2).filter_map(|s| if let [a, b] = s { Some((*a, *b)) } else { None }).collect()
    };
    let all: Vec<(u64, Vec<([i64; 2], [i64; 2])>)> = sheet.wires.iter().map(|w| (w.id, segments(w))).collect();
    let mut out = Vec::new();
    let mut budget = MAX_CROSS_TESTS;
    for (id, mine) in all.iter().filter(|(id, _)| new.contains(id)) {
        for (other, theirs) in &all {
            // Each pair of new wires once.
            if other == id || (new.contains(other) && other < id) {
                continue;
            }
            for &(a, b) in mine {
                for &(c, d) in theirs {
                    if budget == 0 || out.len() >= MAX_CROSSES {
                        return out;
                    }
                    budget -= 1;
                    if let Some(at) = crossing(a, b, c, d) {
                        out.push(CrossOut { wire: *id, other: *other, at });
                    }
                }
            }
        }
    }
    out
}

/// The net of each sheet object (by id) that is on one.
fn nets_by_id(sheet: &Schematic, c: &Connectivity) -> BTreeMap<u64, usize> {
    let wires = sheet.wires.iter().zip(&c.wire_nets).map(|(w, k)| (w.id, *k));
    let labels = sheet.labels.iter().zip(&c.label_nets).map(|(l, k)| (l.id, *k));
    let markers = sheet.markers.iter().zip(&c.marker_nets).filter_map(|(m, k)| k.map(|k| (m.id, k)));
    wires.chain(labels).chain(markers).collect()
}

/// What a call did, from the sheet before it (`old`) to the one stored now, with nets.
fn wire_result(s: &Session, old: &Schematic, renamed: Option<NetRenameOut>) -> Result<Value> {
    let base = sheet_result(s, old, &[], Vec::new());
    let sheet = &s.doc.ecad.schematic;
    let library = s.doc.ecad.library();
    let c = connectivity(sheet, &library);
    let by_id = nets_by_id(sheet, &c);
    let mut touched = BTreeSet::new();
    let mut with_net = |items: Vec<ItemOut>| -> Vec<ItemOut> {
        items
            .into_iter()
            .map(|mut item| {
                if item.kind != KindOut::Part
                    && let Some(k) = by_id.get(&item.id)
                {
                    touched.insert(*k);
                    item.net = c.nets.get(*k).map(|n| n.name.clone());
                }
                item
            })
            .collect()
    };
    let created = with_net(base.created);
    let changed = with_net(base.changed);
    let new_wires: BTreeSet<u64> = created.iter().filter(|i| i.kind == KindOut::Wire).map(|i| i.id).collect();
    reply(WireResult {
        created,
        changed,
        removed: base.removed,
        parts: base.parts,
        nets: touched.iter().filter_map(|k| c.nets.get(*k)).map(net_out).collect(),
        crosses: crosses(sheet, &library, &new_wires),
        renamed,
        param_warnings: Vec::new(),
    })
}

fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.net.rename";
    let p: RenameParams = parse(CMD, p)?;
    check_net_name(&p.to).map_err(|e| bad(CMD, format!("`to`: {e}")))?;
    let old = s.doc.ecad.schematic.clone();
    let mut sheet = old.clone();
    let renamed = {
        let library = s.doc.ecad.library();
        let c = connectivity(&old, &library);
        let k = find_net(&old, &library, &c, &p.net, CMD, "`net`")?;
        let Some(net) = c.nets.get(k) else { return Err(bad(CMD, format!("`net`: no net is named `{}`", p.net))) };
        let other = c.nets.iter().enumerate().find(|(j, n)| *j != k && n.names.contains(&p.to)).map(|(_, n)| n);
        if let Some(other) = other
            && !p.merge.unwrap_or(false)
        {
            let pins = |n: &solidcraft_doc::ecad::Net| {
                let mut list: Vec<String> = n.pins.iter().take(MAX_CLASH_PINS).map(|q| q.reference()).collect();
                if n.pins.len() > MAX_CLASH_PINS {
                    list.push(format!("and {} more", n.pins.len() - MAX_CLASH_PINS));
                }
                if list.is_empty() { String::from("no pins") } else { list.join(", ") }
            };
            return Err(bad(
                CMD,
                format!(
                    "`to`: `{}` already names the net `{}` ({}); renaming `{}` ({}) would join the two: pass merge: true to join them",
                    p.to,
                    other.name,
                    pins(other),
                    net.name,
                    pins(net)
                ),
            ));
        }
        if net.labels.is_empty() {
            // An unnamed net gains a label: on its lowest pin, else on its lowest wire's first end.
            let pins = Pins::new(&old, &library);
            let at = match net.pins.first() {
                Some(q) => Some(Anchor::Pin { part: q.part, pin: q.pin.clone() }),
                None => net.wires.first().and_then(|id| old.wires.iter().find(|w| w.id == *id)).map(|w| w.from.clone()),
            };
            let Some(at) = at else { return Err(bad(CMD, format!("`net`: `{}` has nothing to put a label on", net.name))) };
            let rotation = match &at {
                Anchor::Pin { part, pin } => {
                    pins.part(*part).zip(pins.pin(*part, pin)).map_or(0, |(p, q)| angle(opposite(p.placement.direction(q.direction))))
                }
                Anchor::Point(_) => 0,
            };
            let id = sheet.take_id().map_err(|e| bad(CMD, e))?;
            sheet.labels.push(Label { id, name: p.to.clone(), kind: LabelKind::Net, at, rotation });
        } else {
            let ids: BTreeSet<u64> = net.labels.iter().copied().collect();
            for l in sheet.labels.iter_mut().filter(|l| ids.contains(&l.id)) {
                l.name = p.to.clone();
            }
        }
        NetRenameOut { from: net.name.clone(), to: p.to.clone(), joined: other.map(|n| n.name.clone()) }
    };
    store_schematic(s, sheet, CMD)?;
    wire_result(s, &old, Some(renamed))
}
