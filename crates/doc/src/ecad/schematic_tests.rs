use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::{
    Anchor, Approval, ItemKind, Label, LabelKind, Level, MAX_DIAGONALS, MAX_PARTS, Marker, MarkerKind, Order, Part, Pins, Schematic, Wire, WireEnd,
    split_designator,
};
use crate::Document;
use crate::ecad::connect::{Connectivity, MAX_CHANGED_PINS, NetChange, natural_cmp, net_changes};
use crate::ecad::frame::{self, Frame};
use crate::ecad::place::lattice;
use crate::ecad::route;
use crate::ecad::{ElectronicComponent, Electronics, Motion, Pin, PinDirection, PinType, Placement, SymbolItem};

fn pin(number: &str, at: [f64; 2], direction: PinDirection) -> Pin {
    Pin { number: number.into(), name: String::new(), name_hidden: false, electrical: PinType::Passive, at, direction, length: 2.54 }
}

/// The symbol every resistor here shows.
const SYMBOL: u64 = 2;

/// The library symbol [`SYMBOL`] with pins 1 (−5.08, 0) and 2 (5.08, 0), as the resistor
/// generator makes them.
fn resistor_symbol() -> SymbolItem {
    let pins = vec![pin("1", [-5.08, 0.0], PinDirection::Right), pin("2", [5.08, 0.0], PinDirection::Left)];
    SymbolItem { id: SYMBOL, name: "Resistor".into(), pins, ..Default::default() }
}

/// An electronic component that shows [`SYMBOL`].
fn resistor(id: u64, name: &str) -> ElectronicComponent {
    ElectronicComponent {
        id,
        name: name.into(),
        prefix: "R".into(),
        value: "10k".into(),
        mpn: String::new(),
        manufacturer: String::new(),
        description: String::new(),
        symbol: Some(SYMBOL),
        variants: Vec::new(),
    }
}

fn part(id: u64, component: u64, designator: &str, at: [f64; 2]) -> Part {
    Part {
        id,
        component,
        variant: None,
        designator: designator.into(),
        value: None,
        placement: Placement { at, rotation: 0, mirror: false },
        designator_at: None,
        value_at: None,
        dnp: false,
    }
}

fn on(part: u64, pin: &str) -> Anchor {
    Anchor::Pin { part, pin: pin.into() }
}

/// Two resistors R1 (at the origin) and R2 (at (20, 0)), a wire from R1.2 to R2.1 with a bend, a
/// label on R2.2, a no-connect on R1.1 and a power flag on a point.
fn sheet() -> Electronics {
    let mut e = Electronics { symbols: vec![resistor_symbol()], components: vec![resistor(1, "R0603")], next_id: 3, ..Default::default() };
    let s = &mut e.schematic;
    s.parts = vec![part(1, 1, "R1", [0.0, 0.0]), part(2, 1, "R2", [20.0, 0.0])];
    s.wires = vec![Wire { id: 3, from: on(1, "2"), via: vec![[10.0, 0.0]], to: on(2, "1") }];
    s.labels = vec![Label { id: 4, name: "VOUT".into(), kind: LabelKind::Net, at: on(2, "2"), rotation: 0 }];
    s.markers = vec![
        Marker { id: 5, kind: MarkerKind::NoConnect, at: on(1, "1") },
        Marker { id: 6, kind: MarkerKind::PowerFlag, at: Anchor::Point([0.0, 10.0]) },
    ];
    s.next_id = 7;
    e
}

fn saved(doc: &Document) -> Value {
    serde_json::from_str(&doc.to_json()).unwrap()
}

/// A design file whose `ecad` is `ecad`.
fn open(ecad: Value) -> Result<Document, String> {
    let mut v = saved(&Document::new("A"));
    v["ecad"] = ecad;
    Document::from_json(&v.to_string()).map_err(|e| e.to_string())
}

fn ecad_json(e: &Electronics) -> Value {
    serde_json::to_value(e).unwrap()
}

#[test]
fn a_schematic_round_trips_through_the_design_file() {
    let mut doc = Document::new("A");
    doc.ecad = sheet();
    let s = &mut doc.ecad.schematic;
    s.parts[1].value = Some("4k7".into());
    s.parts[1].placement = Placement { at: [20.0, 0.0], rotation: 90, mirror: true };
    s.parts[1].designator_at = Some([1.27, 2.54]);
    s.parts[1].dnp = true;
    s.approvals = vec![Approval { key: "pin_unconnected|p1.1".into(), note: "spare".into(), by: None }];
    s.rules.insert("net_single_pin".into(), Level::Ignore);
    let v = saved(&doc);
    let sheet = &v["ecad"]["schematic"];
    assert_eq!(sheet["parts"][0], json!({"id": 1, "component": 1, "designator": "R1", "placement": {"at": [0.0, 0.0]}}), "defaults are not saved");
    assert_eq!(sheet["parts"][1]["placement"], json!({"at": [20.0, 0.0], "rotation": 90, "mirror": true}));
    assert_eq!(sheet["wires"][0], json!({"id": 3, "from": {"part": 1, "pin": "2"}, "via": [[10.0, 0.0]], "to": {"part": 2, "pin": "1"}}));
    assert_eq!(sheet["markers"][1], json!({"id": 6, "kind": "power_flag", "at": [0.0, 10.0]}));
    assert_eq!(sheet["rules"], json!({"net_single_pin": "ignore"}));
    assert_eq!(sheet["next_id"], json!(7));
    assert_eq!(Document::from_json(&doc.to_json()).unwrap(), doc);
}

#[test]
fn an_empty_sheet_is_not_saved_and_only_sheet_objects_are_content() {
    let mut e = Electronics { components: vec![resistor(1, "R0603")], next_id: 2, ..Default::default() };
    assert!(ecad_json(&e).get("schematic").is_none());
    e.schematic.next_id = 9;
    assert!(!e.schematic.has_content() && !e.schematic.is_empty(), "the counter alone is saved but shows nothing");
    e.components.clear();
    assert!(!e.has_content());
    let doc = open(json!({"schematic": {}})).unwrap();
    assert!(doc.ecad.is_empty(), "an empty schematic object opens as nothing");
    assert!(sheet().has_content());
}

#[test]
fn unknown_or_misshapen_schematic_keys_are_refused() {
    let base = ecad_json(&sheet());
    let mut cases = Vec::new();
    for (path, junk) in [
        ("/schematic/sheets", json!([])),
        ("/schematic/parts/0/footprint", json!(1)),
        ("/schematic/parts/0/placement/scale", json!(2)),
        ("/schematic/wires/0/from/side", json!("top")),
        ("/schematic/wires/0/from", json!("R1.2")),
        ("/schematic/wires/0/to", json!([1.0])),
        ("/schematic/labels/0/kind", json!("bus")),
        ("/schematic/markers/0/kind", json!("x")),
        ("/schematic/rules", json!({"pin_unconnected": "fatal"})),
        ("/schematic/approvals", json!([{"key": "a", "by": "me"}])),
        ("/schematic", json!([])),
    ] {
        let mut v = base.clone();
        let (parent, key) = path.rsplit_once('/').unwrap();
        v.pointer_mut(parent).unwrap()[key] = junk;
        cases.push((path, v));
    }
    for (path, v) in cases {
        assert!(open(v.clone()).is_err(), "{path}: {v} must not open");
    }
    assert!(open(base).is_ok());
}

#[test]
fn damaged_schematics_are_refused() {
    let base = ecad_json(&sheet());
    let set = |path: &str, value: Value| {
        let mut v = base.clone();
        let (parent, key) = path.rsplit_once('/').unwrap();
        v.pointer_mut(parent).unwrap()[key] = value;
        v
    };
    let cases = [
        ("an id of 0", set("/schematic/wires/0/id", json!(0))),
        ("an id shared by two kinds", set("/schematic/labels/0/id", json!(3))),
        ("a part of a component that isn't there", set("/schematic/parts/0/component", json!(9))),
        ("a designator starting with a digit", set("/schematic/parts/0/designator", json!("1R"))),
        ("a designator with a space", set("/schematic/parts/0/designator", json!("R 1"))),
        ("a 17-character designator", set("/schematic/parts/0/designator", json!("R1234567890123456"))),
        ("designators differing only in case", set("/schematic/parts/1/designator", json!("r1"))),
        ("a part past the sheet", set("/schematic/parts/0/placement/at", json!([2000.5, 0]))),
        ("a part turned 45°", set("/schematic/parts/0/placement/rotation", json!(45))),
        ("a value too long", set("/schematic/parts/0/value", json!("v".repeat(65)))),
        ("a wire on a pin the part doesn't have", set("/schematic/wires/0/to", json!({"part": 2, "pin": "3"}))),
        ("a wire on a part that isn't there", set("/schematic/wires/0/to", json!({"part": 8, "pin": "1"}))),
        ("a bend past the sheet", set("/schematic/wires/0/via", json!([[0, -2001]]))),
        ("a no-connect on a point", set("/schematic/markers/0/at", json!([1, 1]))),
        ("a net name with a space", set("/schematic/labels/0/name", json!("V OUT"))),
        ("a net name with a colon", set("/schematic/labels/0/name", json!("net:R1.2"))),
        ("an empty net name", set("/schematic/labels/0/name", json!(""))),
        ("a 65-character net name", set("/schematic/labels/0/name", json!("N".repeat(65)))),
        ("a label turned 45°", set("/schematic/labels/0/rotation", json!(45))),
        ("two bends in a row at one point", set("/schematic/wires/0/via", json!([[10, 0], [10, 0]]))),
        ("a point end on its bend", set("/schematic/wires/0/to", json!([10, 0]))),
        ("two approvals with one key", set("/schematic/approvals", json!([{"key": "a"}, {"key": "a"}]))),
        ("an approval with no key", set("/schematic/approvals", json!([{"key": ""}]))),
        ("a rule that isn't a code", set("/schematic/rules", json!({"Pin Unconnected": "ignore"}))),
    ];
    for (what, v) in cases {
        assert!(open(v.clone()).is_err(), "{what}: {v} must not open");
    }
    // A component with no symbol can't be placed.
    let mut e = sheet();
    e.components[0].symbol = None;
    assert!(e.validate().unwrap_err().contains("has no symbol"));
}

#[test]
fn hostile_numbers_and_the_caps_are_refused() {
    let mut e = sheet();
    e.schematic.parts[0].placement.at = [f64::NAN, 0.0];
    assert!(e.validate().is_err(), "NaN");
    let mut e = sheet();
    e.schematic.labels[0].at = Anchor::Point([0.0, f64::INFINITY]);
    assert!(e.validate().is_err(), "infinity");
    // 4097 parts.
    let mut e = sheet();
    let s = &mut e.schematic;
    (s.wires, s.labels, s.markers) = Default::default();
    s.parts = (1..=MAX_PARTS as u64 + 1).map(|i| part(i, 1, &format!("R{i}"), [0.0, 0.0])).collect();
    s.next_id = MAX_PARTS as u64 + 2;
    assert!(e.validate().unwrap_err().contains("at most 4096 parts"));
    e.schematic.parts.pop();
    assert!(e.validate().is_ok());
    // 257 diagonal segments, between saved points only: each wire one, and a diagonal leg to a pin
    // doesn't count.
    let mut e = sheet();
    e.schematic.wires[0].via = vec![[7.0, 3.0]];
    let diagonal = |i: u64| Wire { id: 7 + i, from: Anchor::Point([i as f64, 50.0]), via: Vec::new(), to: Anchor::Point([i as f64 + 0.5, 51.0]) };
    e.schematic.wires.extend((0..MAX_DIAGONALS as u64 + 1).map(diagonal));
    e.schematic.next_id = 7 + MAX_DIAGONALS as u64 + 1;
    assert!(e.validate().unwrap_err().contains("diagonal"));
    e.schematic.wires.pop();
    assert!(e.validate().is_ok(), "256 diagonals");
    // 257 points on one wire.
    let mut e = sheet();
    e.schematic.wires[0].via = (0..255).map(|i| [i as f64, 5.0]).collect();
    assert!(e.validate().unwrap_err().contains("at most 256 points"));
    // More than 65536 pins placed: 17 parts of a 4096-pin component.
    let mut e = sheet();
    e.symbols[0].pins = (0..4096).map(|i| pin(&i.to_string(), [0.0, i as f64 * 0.1], PinDirection::Right)).collect();
    e.schematic = Schematic { parts: (1..=17).map(|i| part(i, 1, &format!("U{i}"), [0.0, 0.0])).collect(), next_id: 18, ..Default::default() };
    assert!(e.validate().unwrap_err().contains("65536 pins"));
}

#[test]
fn the_sheet_ids_are_one_space_and_never_fall_behind() {
    let mut v = ecad_json(&sheet());
    v["schematic"]["next_id"] = json!(2);
    let doc = open(v).unwrap();
    assert_eq!(doc.ecad.schematic.next_id, 7, "raised to one past the largest id in use");
    let mut s = doc.ecad.schematic.clone();
    assert_eq!((s.take_id(), s.take_id()), (Ok(7), Ok(8)));
    s.markers.clear();
    assert_eq!(s.take_id(), Ok(9), "a freed id isn't reused");
    assert_eq!(Schematic::default().take_id(), Ok(1));
    assert_eq!(s.kind_of(3), Some(ItemKind::Wire));
    assert_eq!(s.kind_of(4), Some(ItemKind::Label));
    assert_eq!(s.kind_of(99), None);
}

#[test]
fn designators_number_on_from_the_highest_per_prefix() {
    assert_eq!(split_designator("R12"), Some(("R", 12)));
    assert_eq!(split_designator("LED3"), Some(("LED", 3)));
    for d in ["R", "12", "R_1", "U1A", "R99999999999999999999"] {
        assert_eq!(split_designator(d), None, "{d}");
    }
    let mut e = sheet();
    let s = &mut e.schematic;
    s.parts.push(part(10, 1, "r7", [40.0, 0.0]));
    s.parts.push(part(11, 1, "LED2", [60.0, 0.0]));
    s.parts.push(part(12, 1, "R_9", [80.0, 0.0]));
    s.next_id = 13;
    let numbers = s.numbers();
    assert_eq!(numbers.get("R"), Some(&7), "R1, R2 and r7 (case doesn't count); R_9 is not an R");
    assert_eq!(numbers.get("LED"), Some(&2));
    assert_eq!(numbers.get("L"), None, "LED2 is not an L");
}

#[test]
fn a_renumbered_pin_keeps_its_anchors_and_approvals() {
    let mut e = sheet();
    e.schematic.approvals = vec![Approval { key: "pin_conflict|p1.2|p2.1".into(), note: String::new(), by: None }];
    let library = e.library();
    let mut s = e.schematic.clone();
    let parts = s.showing(&library, SYMBOL);
    assert_eq!(parts, BTreeSet::from([1, 2]));
    let mut symbol = e.symbols[0].clone();
    symbol.pins[1].number = "B".into();
    let detached = s.symbol_changed(&library, &parts, &symbol, &[("2".into(), "B".into())]);
    assert!(detached.is_empty(), "{detached:?}");
    assert_eq!(s.wires[0].from, on(1, "B"));
    assert_eq!(s.labels[0].at, on(2, "B"), "R2.2 is renumbered too");
    assert_eq!(s.approvals[0].key, "pin_conflict|p1.B|p2.1");
    let mut after = e.clone();
    (after.symbols[0], after.schematic) = (symbol, s);
    assert!(after.validate().is_ok());
}

#[test]
fn a_gone_pin_lets_go_where_it_was() {
    let mut e = sheet();
    e.schematic.parts[1].placement.rotation = 90;
    let library = e.library();
    let mut s = e.schematic.clone();
    let parts = s.showing(&library, SYMBOL);
    let mut symbol = e.symbols[0].clone();
    symbol.pins.retain(|p| p.number == "1");
    let detached = s.symbol_changed(&library, &parts, &symbol, &[]);
    // R1.2 was at (5.08, 0); R2 is turned a quarter, so its pin 2 was at (20, 5.08).
    assert_eq!(s.wires[0].from, Anchor::Point([5.08, 0.0]));
    assert_eq!(s.wires[0].to, on(2, "1"), "pin 1 is still there");
    assert_eq!(s.labels[0].at, Anchor::Point([20.0, 5.08]));
    assert_eq!(s.markers.len(), 2, "the no-connect on pin 1 stays");
    let what: Vec<(ItemKind, u64, Option<WireEnd>, &str, &str, [f64; 2])> =
        detached.iter().map(|d| (d.kind, d.id, d.end, d.designator.as_str(), d.pin.as_str(), d.at)).collect();
    assert_eq!(what, [(ItemKind::Wire, 3, Some(WireEnd::From), "R1", "2", [5.08, 0.0]), (ItemKind::Label, 4, None, "R2", "2", [20.0, 5.08])]);
    // Pin 1 gone instead: its no-connect goes with it.
    let mut s = e.schematic.clone();
    let mut symbol = e.symbols[0].clone();
    symbol.pins.retain(|p| p.number == "2");
    let detached = s.symbol_changed(&library, &parts, &symbol, &[]);
    assert_eq!(s.markers.iter().map(|m| m.id).collect::<Vec<_>>(), [6]);
    assert!(detached.iter().any(|d| d.kind == ItemKind::Marker && d.id == 5 && d.removed), "{detached:?}");
}

#[test]
fn removing_a_part_keeps_its_wires_at_the_old_pins() {
    let mut e = sheet();
    // A second wire from R1.1 to the point it touches: it is left with no length and goes.
    e.schematic.wires.push(Wire { id: 7, from: on(1, "1"), via: Vec::new(), to: Anchor::Point([-5.08, 0.0]) });
    e.schematic.next_id = 8;
    assert!(e.validate().is_ok(), "a leg to a pin may have no length");
    let library = e.library();
    let mut s = e.schematic.clone();
    let detached = s.remove(&BTreeSet::from([1]), &library);
    assert_eq!(s.parts.iter().map(|p| p.designator.as_str()).collect::<Vec<_>>(), ["R2"]);
    assert_eq!(s.wires.len(), 1);
    assert_eq!(s.wires[0].from, Anchor::Point([5.08, 0.0]), "the wire stays, at the old pin");
    assert_eq!(s.markers.iter().map(|m| m.id).collect::<Vec<_>>(), [6], "R1's no-connect goes");
    assert!(detached.iter().any(|d| d.id == 7 && d.removed));
    assert!(detached.iter().any(|d| d.id == 3 && d.end == Some(WireEnd::From) && !d.removed));
    let mut after = e.clone();
    after.schematic = s;
    assert!(after.validate().is_ok());
    // Removing the wire with the part leaves nothing detached for it.
    let mut s = e.schematic.clone();
    let detached = s.remove(&BTreeSet::from([1, 3, 7]), &library);
    assert!(detached.iter().all(|d| d.kind == ItemKind::Marker), "{detached:?}");
}

#[test]
fn points_on_pins_are_tied_to_them() {
    let mut e = sheet();
    e.schematic.labels[0].at = Anchor::Point([25.08, 0.0]);
    e.schematic.markers[1].at = Anchor::Point([14.92, 0.0]);
    let library = e.library();
    let mut s = e.schematic.clone();
    s.settle(&library).unwrap();
    assert_eq!(s.labels[0].at, on(2, "2"));
    assert_eq!(s.markers[1].at, on(2, "1"));
}

#[test]
fn moving_parts_turns_their_texts_and_pinned_labels() {
    let mut e = sheet();
    e.schematic.parts[0].designator_at = Some([0.0, 2.54]);
    e.schematic.labels.push(Label { id: 7, name: "IN".into(), kind: LabelKind::Net, at: Anchor::Point([-10.0, 0.0]), rotation: 0 });
    e.schematic.next_id = 8;
    let mut s = e.schematic.clone();
    let m = Motion { about: [0.0, 0.0], rotation: 90, ..Default::default() };
    s.move_items(&BTreeSet::from([2, 7]), &m);
    assert_eq!(s.parts[1].placement, Placement { at: [0.0, 20.0], rotation: 90, mirror: false });
    assert_eq!(s.parts[0], e.schematic.parts[0], "R1 stays");
    assert_eq!(s.labels[0].rotation, 90, "the label on R2's pin turns with R2");
    assert_eq!((s.labels[1].at.clone(), s.labels[1].rotation), (Anchor::Point([0.0, -10.0]), 90));
    assert_eq!(s.wires, e.schematic.wires, "a wire not moved keeps its saved points");
    let mut s = e.schematic.clone();
    s.move_items(&BTreeSet::from([1, 3]), &Motion { about: [0.0, 0.0], mirror: true, ..Default::default() });
    assert!(s.parts[0].placement.mirror);
    assert_eq!(s.parts[0].designator_at, Some([0.0, 2.54]));
    assert_eq!(s.wires[0].via, [[-10.0, 0.0]], "a moved wire's bends move");
}

#[test]
fn annotation_numbers_by_position_or_placement() {
    let mut e = sheet();
    let s = &mut e.schematic;
    (s.wires, s.labels, s.markers) = Default::default();
    // R1 at the right, R2 at the left, a part named by hand at the top.
    s.parts = vec![part(1, 1, "R1", [20.0, 0.0]), part(2, 1, "R2", [0.0, 0.0]), part(3, 1, "PULLUP", [10.0, 10.0])];
    s.next_id = 4;
    let library = e.library();
    let s = &e.schematic;
    let by_position = s.annotate(&library, Order::Position, false, 1).unwrap();
    assert_eq!(by_position, [(3, "R1".to_string()), (1, "R3".to_string())], "top to bottom, then left to right; R2 stays R2");
    let by_placement = s.annotate(&library, Order::Placement, false, 1).unwrap();
    assert_eq!(by_placement, [(3, "R3".to_string())], "R1 and R2 are already right");
    let kept = s.annotate(&library, Order::Position, true, 1).unwrap();
    assert_eq!(kept, [(3, "R3".to_string())], "the others keep theirs; the new one numbers on after them");
    let from_ten = s.annotate(&library, Order::Placement, false, 10).unwrap();
    assert_eq!(from_ten.iter().map(|(_, d)| d.as_str()).collect::<Vec<_>>(), ["R10", "R11", "R12"]);
}

// How the netlist changes, and moves that keep it.

/// `e`'s sheet replaced by `sheet`, checked as a design file would be.
fn with_sheet(e: &Electronics, sheet: Schematic) -> Electronics {
    let mut after = e.clone();
    after.schematic = sheet;
    if let Err(err) = after.validate() {
        panic!("{err}");
    }
    after
}

fn nets(e: &Electronics) -> Connectivity {
    e.connectivity()
}

fn change(net: &str, lost: &[&str], gained: &[&str]) -> NetChange {
    let refs = |v: &[&str]| v.iter().map(|r| r.to_string()).collect();
    NetChange { net: net.into(), lost: refs(lost), gained: refs(gained), more: 0 }
}

#[test]
fn net_changes_list_the_pins_each_net_lost_and_gained() {
    let e = sheet();
    let before = nets(&e);
    assert_eq!(net_changes(&before, &before), []);
    assert_eq!(before.net("net:R1.2").map(|n| n.pins.len()), Some(2), "R1.2 to R2.1");
    // The wire moved from R2.1 to R2.2: R1.2 joins VOUT, R2.1 is left alone.
    let mut s = e.schematic.clone();
    s.wires[0].to = on(2, "2");
    let after = nets(&with_sheet(&e, s));
    assert_eq!(net_changes(&before, &after), [change("VOUT", &[], &["R1.2"]), change("net:R1.2", &["R2.1"], &[])]);
    assert_eq!(net_changes(&after, &before), [change("VOUT", &["R1.2"], &[]), change("net:R1.2", &[], &["R2.1"])], "and back");
    // A renamed net keeps its pins: no change.
    let mut s = e.schematic.clone();
    s.labels[0].name = "OUT".into();
    assert_eq!(net_changes(&before, &nets(&with_sheet(&e, s))), []);
    // A long list is cut short and counts the rest.
    let mut e = sheet();
    let s = &mut e.schematic;
    (s.wires, s.labels, s.markers) = Default::default();
    s.parts = (1..=70).map(|i| part(i, 1, &format!("R{i}"), [i as f64 * 20.32, 0.0])).collect();
    s.labels = (1..=70).map(|i| Label { id: 100 + i, name: format!("X{i}"), kind: LabelKind::Net, at: on(i, "1"), rotation: 0 }).collect();
    s.next_id = 200;
    let before = nets(&e);
    let mut bus = e.schematic.clone();
    bus.labels.iter_mut().for_each(|l| l.name = "BUS".into());
    let changes = net_changes(&before, &nets(&with_sheet(&e, bus)));
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!((changes[0].net.as_str(), changes[0].gained.len(), changes[0].more), ("BUS", MAX_CHANGED_PINS, 69 - MAX_CHANGED_PINS));
    assert_eq!(changes[0].gained.first().map(String::as_str), Some("R2.1"), "R1 stays, R2 before R10");
}

#[test]
fn names_sort_with_their_numbers_as_numbers() {
    let mut names = vec!["R10", "R2", "net:R1.2", "R1", "C1", "R01", "N01b", "N1"];
    names.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(names, ["C1", "N1", "N01b", "R01", "R1", "R2", "R10", "net:R1.2"]);
    assert_eq!(natural_cmp("R1", "R1"), std::cmp::Ordering::Equal);
    assert_ne!(natural_cmp("R01", "R1"), std::cmp::Ordering::Equal, "equal only when the texts are");
    assert_eq!(natural_cmp("R99999999999999999999999", "R100000000000000000000000"), std::cmp::Ordering::Less, "no overflow");
}

/// The sheet of [`sheet`] with more on it: a label on a point at the end of a wire from R2.2,
/// and a wire teed onto wire 3's bend ending in a label.
fn busy() -> Electronics {
    let mut e = sheet();
    let s = &mut e.schematic;
    s.labels.clear();
    s.wires.push(Wire { id: 7, from: on(2, "2"), via: Vec::new(), to: Anchor::Point([30.48, 0.0]) });
    s.labels.push(Label { id: 8, name: "OUT".into(), kind: LabelKind::Net, at: Anchor::Point([30.48, 0.0]), rotation: 0 });
    s.wires.push(Wire { id: 9, from: Anchor::Point([10.0, 0.0]), via: Vec::new(), to: Anchor::Point([10.0, -10.16]) });
    s.labels.push(Label { id: 10, name: "TAP".into(), kind: LabelKind::Net, at: Anchor::Point([10.0, -10.16]), rotation: 0 });
    s.next_id = 11;
    e
}

#[test]
fn moves_keep_the_netlist() {
    let e = busy();
    let library = e.library();
    let before = nets(&e);
    assert_eq!(before.net("TAP").map(|n| n.pins.iter().map(|p| p.reference()).collect::<Vec<_>>()), Some(vec!["R1.2".into(), "R2.1".into()]));
    assert_eq!(before.net("OUT").map(|n| n.pins.len()), Some(1));
    let motions = [
        Motion { by: [0.0, 30.48], ..Motion::default() },
        Motion { by: [-40.64, 15.24], ..Motion::default() },
        Motion { about: [20.0, 0.0], rotation: 90, ..Motion::default() },
        Motion { about: [0.0, 0.0], rotation: 180, by: [0.0, 50.8], ..Motion::default() },
        Motion { about: [20.0, 0.0], mirror: true, by: [0.0, -30.48], ..Motion::default() },
    ];
    let picks: [&[u64]; 8] = [&[1], &[2], &[1, 2], &[3], &[1, 3], &[2, 3], &[2, 7, 8], &[1, 2, 3, 7, 8, 9, 10]];
    for (k, m) in motions.iter().enumerate() {
        for ids in picks {
            let ids: BTreeSet<u64> = ids.iter().copied().collect();
            let after = with_sheet(&e, route::moved(&e.schematic, &library, &ids, m));
            let changes = net_changes(&before, &nets(&after));
            // No connection comes apart; one the new routes make is reported (the command says
            // so), and a move straight up into empty sheet makes none.
            assert!(changes.iter().all(|c| c.lost.is_empty()), "moving {ids:?} by {m:?}: {changes:?}");
            if k == 0 {
                assert_eq!(changes, [], "moving {ids:?} by {m:?}");
            }
            assert!(nets(&after).dangling.is_empty(), "moving {ids:?} by {m:?}: {:?}", nets(&after).dangling);
        }
    }
    // A part moved with its wire: the bend the tee stands on moves along, and the tee with it.
    let with_wire = route::moved(&e.schematic, &library, &BTreeSet::from([2, 3]), &motions[1]);
    let tee = with_wire.wires.iter().find(|w| w.id == 9).unwrap();
    assert_eq!(tee.from, Anchor::Point([-30.64, 15.24]), "the tee's foot went along with the bend");
    assert!(with_wire.wires[0].via.contains(&[-30.64, 15.24]), "{:?}", with_wire.wires[0].via);
    // What moves goes along: R2 moved up carries its wires' ends and the point label at their end
    // keeps to the wire.
    let up = route::moved(&e.schematic, &library, &BTreeSet::from([2]), &motions[0]);
    assert_eq!(up.parts[1].placement.at, [20.0, 30.48]);
    assert_eq!(up.parts[0], e.schematic.parts[0], "R1 stays");
    let pins = Pins::new(&up, &library);
    let end = up.wires.iter().find(|w| w.id == 7).and_then(|w| pins.anchor(&w.to)).unwrap();
    assert_eq!(up.labels.iter().find(|l| l.id == 8).and_then(|l| pins.anchor(&l.at)), Some(end), "OUT on the wire's end");
}

#[test]
fn moved_wires_run_level_and_upright() {
    let e = busy();
    let library = e.library();
    let level = |s: &Schematic| {
        let pins = Pins::new(s, &library);
        s.wires.iter().all(|w| pins.wire(w).windows(2).all(|ab| lattice(ab[0])[0] == lattice(ab[1])[0] || lattice(ab[0])[1] == lattice(ab[1])[1]))
    };
    assert!(level(&e.schematic));
    for (ids, m) in [
        (BTreeSet::from([2]), Motion { by: [0.0, 30.48], ..Motion::default() }),
        (BTreeSet::from([1]), Motion { by: [-10.16, -20.32], ..Motion::default() }),
        (BTreeSet::from([3]), Motion { by: [0.0, 5.08], ..Motion::default() }),
    ] {
        let after = route::moved(&e.schematic, &library, &ids, &m);
        assert!(level(&after), "moving {ids:?}: {:?}", after.wires);
    }
    // Wire 3 alone moved up: its ends stay on their pins and it runs straight between them, the
    // tee still on it.
    let straight = route::moved(&e.schematic, &library, &BTreeSet::from([3]), &Motion { by: [0.0, 5.08], ..Motion::default() });
    assert!(straight.wires[0].via.is_empty(), "{:?}", straight.wires[0]);
    assert_eq!(straight.wires.iter().find(|w| w.id == 9), e.schematic.wires.iter().find(|w| w.id == 9));
    // Dragging wire 3's bend up takes the T on it along.
    let reshaped = route::reshaped(&e.schematic, &library, 3, &[[10.0, 0.0], [10.0, 5.08]]);
    let after = with_sheet(&e, reshaped);
    assert_eq!(net_changes(&nets(&e), &nets(&after)), []);
}

#[test]
fn a_new_symbol_with_its_pins_elsewhere_keeps_the_wires_on_them() {
    let e = busy();
    let library = e.library();
    let before = nets(&e);
    let parts = e.schematic.showing(&library, SYMBOL);
    // A longer body: the pins 2.54 mm further out.
    let mut longer = e.symbols[0].clone();
    longer.pins = vec![pin("1", [-7.62, 0.0], PinDirection::Right), pin("2", [7.62, 0.0], PinDirection::Left)];
    // Pins on the top and bottom, the part upright.
    let mut upright = e.symbols[0].clone();
    upright.pins = vec![pin("1", [0.0, 5.08], PinDirection::Down), pin("2", [0.0, -5.08], PinDirection::Up)];
    for new in [longer, upright] {
        let mut s = e.schematic.clone();
        let detached = s.symbol_changed(&library, &parts, &new, &[]);
        assert!(detached.is_empty(), "{detached:?}");
        let mut after = e.clone();
        after.symbols[0] = new.clone();
        let after = with_sheet(&after, s);
        assert_eq!(net_changes(&before, &nets(&after)), [], "{:?}", new.pins);
        assert_eq!(after.schematic.wires[0].from, on(1, "2"), "still on the pin");
        let library = after.library();
        let pins = Pins::new(&after.schematic, &library);
        for w in &after.schematic.wires {
            let points = pins.wire(w);
            assert!(
                points.windows(2).all(|ab| lattice(ab[0])[0] == lattice(ab[1])[0] || lattice(ab[0])[1] == lattice(ab[1])[1]),
                "wire {} {points:?}",
                w.id
            );
        }
        assert!(after.schematic.wires[0].via.contains(&[10.0, 0.0]), "the bend the tee stands on stays: {:?}", after.schematic.wires[0].via);
    }
}

// Section frames.

fn frame_of(id: u64, title: &str, rect: [[f64; 2]; 2], order: Option<u32>) -> Frame {
    Frame { id, title: title.into(), rect, order }
}

#[test]
fn frames_split_the_sheet_by_the_middle_of_each_part() {
    let mut e = sheet();
    e.schematic.frames =
        vec![frame_of(7, "Left", [[-10.0, -10.0], [10.0, 10.0]], None), frame_of(8, "Right", [[10.0, -10.0], [30.0, 10.0]], Some(1))];
    e.schematic.next_id = 9;
    assert!(e.validate().is_ok());
    let order: Vec<u64> = frame::in_order(&e.schematic.frames).iter().map(|f| f.id).collect();
    assert_eq!(order, [8, 7], "frames with an order first");
    let library = e.library();
    assert_eq!(frame::part_frames(&e.schematic, &library), [(1, 7), (2, 8)].into());
    assert!(frame::overlaps(&e.schematic.frames).is_empty(), "sharing an edge isn't overlapping");
    // An overlapping frame first in print order takes R2.
    e.schematic.frames.push(frame_of(9, "Middle", [[15.0, -5.0], [25.0, 5.0]], Some(0)));
    e.schematic.next_id = 10;
    assert_eq!(frame::overlaps(&e.schematic.frames), [(8, 9)]);
    assert_eq!(frame::part_frames(&e.schematic, &e.library()).get(&2), Some(&9));
    let f = &e.schematic.frames[0];
    assert!(f.contains([10.0, 0.0]) && !f.contains([10.01, 0.0]) && f.middle() == [0.0, 0.0]);
    assert_eq!(frame::normalized([[5.0, 1.0], [-5.0, -1.0]]), [[-5.0, -1.0], [5.0, 1.0]]);
    // Moving a frame moves its rectangle.
    let mut s = e.schematic.clone();
    s.move_items(&BTreeSet::from([7]), &Motion { about: [0.0, 0.0], rotation: 90, ..Motion::default() });
    assert_eq!(s.frames[0].rect, [[-10.0, -10.0], [10.0, 10.0]]);
    s.move_items(&BTreeSet::from([7]), &Motion { by: [5.0, 0.0], ..Motion::default() });
    assert_eq!(s.frames[0].rect, [[-5.0, -10.0], [15.0, 10.0]]);
}

#[test]
fn damaged_frames_are_refused() {
    for (what, f) in [
        ("no title", frame_of(7, "", [[0.0, 0.0], [10.0, 10.0]], None)),
        ("a padded title", frame_of(7, " Power", [[0.0, 0.0], [10.0, 10.0]], None)),
        ("a title with a line break", frame_of(7, "Po\nwer", [[0.0, 0.0], [10.0, 10.0]], None)),
        ("a 65-character title", frame_of(7, &"P".repeat(65), [[0.0, 0.0], [10.0, 10.0]], None)),
        ("the upper right first", frame_of(7, "Power", [[10.0, 10.0], [0.0, 0.0]], None)),
        ("a side under 1 mm", frame_of(7, "Power", [[0.0, 0.0], [0.5, 10.0]], None)),
        ("a corner past the sheet", frame_of(7, "Power", [[0.0, 0.0], [3000.0, 10.0]], None)),
        ("a NaN corner", frame_of(7, "Power", [[f64::NAN, 0.0], [10.0, 10.0]], None)),
        ("an order past 9999", frame_of(7, "Power", [[0.0, 0.0], [10.0, 10.0]], Some(10_000))),
    ] {
        assert!(f.validate().is_err(), "{what}");
    }
    assert!(frame_of(7, "Power", [[0.0, 0.0], [1.0, 1.0]], Some(9999)).validate().is_ok());
    let base = ecad_json(&sheet());
    let with = |frames: Value| {
        let mut v = base.clone();
        v["schematic"]["frames"] = frames;
        open(v)
    };
    let one = json!({"id": 7, "title": "Power", "rect": [[0, 0], [10, 10]]});
    assert!(with(json!([one])).is_ok());
    assert!(with(json!([{"id": 3, "title": "Power", "rect": [[0, 0], [10, 10]]}])).is_err(), "an id the wire has");
    assert!(with(json!([one, {"id": 8, "title": "power", "rect": [[20, 0], [30, 10]]}])).is_err(), "titles differing only in case");
    assert!(with(json!([{"id": 7, "title": "Power", "rect": [[0, 0], [10, 10]], "colour": "red"}])).is_err(), "an unknown key");
    let many: Vec<Value> = (0..65).map(|i| json!({"id": 100 + i, "title": format!("F{i}"), "rect": [[0, 0], [10, 10]]})).collect();
    assert!(with(Value::Array(many)).unwrap_err().contains("64"));
}

#[test]
fn nets_join_at_points_and_tees_and_take_the_ports_names() {
    let mut e = sheet();
    let s = &mut e.schematic;
    (s.wires, s.labels, s.markers) = Default::default();
    // R3 turned a quarter below the wire: pin 1 at (10, -25.08), pin 2 at (10, -14.92).
    s.parts.push(Part { placement: Placement { at: [10.0, -20.0], rotation: 90, mirror: false }, ..part(7, 1, "R3", [10.0, -20.0]) });
    s.wires = vec![
        Wire { id: 10, from: on(1, "2"), via: vec![[10.0, 0.0]], to: on(2, "1") },
        // A tee: R3.2's wire ends inside wire 10's first segment.
        Wire { id: 11, from: on(7, "2"), via: vec![[7.62, -14.92]], to: Anchor::Point([7.62, 0.0]) },
        // Crossing wire 10 joins nothing.
        Wire { id: 12, from: Anchor::Point([8.89, 5.08]), via: Vec::new(), to: Anchor::Point([8.89, -5.08]) },
        // Running over R1.1 doesn't join it either.
        Wire { id: 13, from: Anchor::Point([-10.16, 0.0]), via: Vec::new(), to: Anchor::Point([-2.54, 0.0]) },
    ];
    let label = |id: u64, name: &str, kind: LabelKind, at: Anchor| Label { id, name: name.into(), kind, at, rotation: 0 };
    s.labels = vec![
        label(20, "LOCAL", LabelKind::Net, on(2, "2")),
        label(21, "+3V3", LabelKind::Rail, on(2, "2")),
        label(22, "GND", LabelKind::Ground, on(7, "1")),
        label(23, "GND", LabelKind::Ground, Anchor::Point([30.48, 30.48])),
        label(24, "SENSE", LabelKind::Net, Anchor::Point([8.89, 5.08])),
    ];
    s.next_id = 30;
    assert!(e.validate().is_ok(), "{:?}", e.validate());
    let c = e.connectivity();
    let names: Vec<&str> = c.nets.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["+3V3", "GND", "SENSE", "net:R1.2", "net:wire13"]);
    let pins = |name: &str| c.net(name).unwrap().pins.iter().map(|p| p.reference()).collect::<Vec<_>>();
    assert_eq!(pins("net:R1.2"), ["R1.2", "R2.1", "R3.2"], "joined at the bend and the tee");
    assert_eq!(c.net("net:R1.2").unwrap().wires, [10, 11]);
    let rail = c.net("+3V3").unwrap();
    assert!(rail.power && rail.named && rail.names == ["+3V3", "LOCAL"], "a rail port's name before a net label's: {rail:?}");
    assert_eq!((pins("GND"), c.net("GND").unwrap().labels.clone()), (vec!["R3.1".to_string()], vec![22, 23]), "ports with one name join");
    assert_eq!(c.net("SENSE").map(|n| (n.pins.len(), n.wires.clone())), Some((0, vec![12])));
    assert!(!c.net("net:wire13").unwrap().named);
    assert_eq!(c.pin_net(1, "1"), None);
    assert_eq!(c.unconnected.iter().map(|p| p.reference()).collect::<Vec<_>>(), ["R1.1"]);
    assert_eq!(c.pin_on_wire.iter().map(|p| (p.pin.reference(), p.wire)).collect::<Vec<_>>(), [("R1.1".to_string(), 13)]);
    assert!(c.junctions.contains(&[7.62, 0.0]), "a dot at the tee: {:?}", c.junctions);
    assert!(!c.junctions.contains(&[8.89, 0.0]), "none where wires only cross");
    let mut loose: Vec<(u64, [f64; 2])> = c.dangling.iter().map(|d| (d.wire, d.at)).collect();
    loose.sort_by(|a, b| a.0.cmp(&b.0).then(a.1[0].total_cmp(&b.1[0])));
    assert_eq!(loose, [(12, [8.89, -5.08]), (13, [-10.16, 0.0]), (13, [-2.54, 0.0])]);
    assert!(!c.truncated);
    assert_eq!(c.wire_net(&e.schematic, 11), c.index_of("net:R1.2"));
    assert_eq!(c.label_net(&e.schematic, 23), c.index_of("GND"));
}
