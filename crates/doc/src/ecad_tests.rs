use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::airwires::footprint_pads;
use super::board::{Board, BoardGraphic, Track, Via, ViaProtection, ViaRole};
use super::checks::Severity;
use super::current::{self, CurrentError, Fit, OZ_MM};
use super::graphics::GraphicType;
use super::impedance::{self, ImpedanceError, LineKind, Medium};
use super::intent::{NetIntent, PairIntent, PartIntent};
use super::layers::{CopperLayer, DrawLayer};
use super::place::{Motion, snap_point};
use super::quality::{self, Status, catalog};
use super::schematic::{Actor, Approval};
use super::stackup::default_stackup;
use super::stitch::{self, Ask, Pattern, ViaSpec};
use super::{
    ElectronicComponent, Electronics, FootprintItem, MAX_COMPONENTS, MAX_DESCRIPTION, MAX_TEXT, NetClass, Pad, PadKind, PadShape, Pin, PinDirection,
    PinType, Side, SymbolItem, Variant, overbar, path,
};
use crate::Document;
use crate::format::{FORMAT, format_string};

fn saved(doc: &Document) -> Value {
    serde_json::from_str(&doc.to_json()).unwrap()
}

/// The format stamp every design this build saves gets, electronics or not.
fn stamp() -> (Value, Option<Value>) {
    let v = saved(&Document::new("B"));
    (v["format"].clone(), v.get("format_pending").cloned())
}

#[test]
fn a_design_without_electronics_saves_no_ecad_key_and_reopens_unchanged() {
    let doc = Document::new("A");
    assert!(doc.ecad.is_empty());
    let v = saved(&doc);
    assert!(v.get("ecad").is_none(), "empty electronics are not saved");
    assert_eq!((v["format"].clone(), v.get("format_pending").cloned()), stamp());
    assert_eq!(v["format"], json!(format_string(FORMAT)));
    assert_eq!(Document::from_json(&doc.to_json()).unwrap(), doc);
}

#[test]
fn an_empty_ecad_object_opens_as_no_electronics_and_is_not_saved_again() {
    let mut v = saved(&Document::new("A"));
    v["ecad"] = json!({});
    let doc = Document::from_json(&v.to_string()).unwrap();
    assert!(doc.ecad.is_empty());
    assert!(saved(&doc).get("ecad").is_none());
}

#[test]
fn unknown_electronics_keys_are_refused_not_dropped() {
    for ecad in [json!({"boards": []}), json!({"library": {"components": [{"name": "R1"}]}})] {
        let mut v = saved(&Document::new("A"));
        v["ecad"] = ecad.clone();
        assert!(Document::from_json(&v.to_string()).is_err(), "{ecad} must not open and lose its contents on save");
    }
    for junk in [json!(null), json!(7), json!("board"), json!([]), json!([{}])] {
        let mut v = saved(&Document::new("A"));
        v["ecad"] = junk.clone();
        assert!(Document::from_json(&v.to_string()).is_err(), "`ecad: {junk}` is not electronics");
    }
}

#[test]
fn designs_saved_by_the_last_release_open_with_no_electronics() {
    let mut v = saved(&Document::new("A"));
    v["format"] = json!("solidcraft/41");
    v.as_object_mut().unwrap().remove("format_pending");
    let doc = Document::from_json(&v.to_string()).unwrap();
    assert!(doc.ecad.is_empty());
    let again = saved(&doc);
    assert_eq!((again["format"].clone(), again.get("format_pending").cloned()), stamp());
    assert!(again.get("ecad").is_none());
}

#[test]
fn a_design_with_a_later_electronics_change_asks_for_an_update() {
    let mut v = saved(&Document::new("A"));
    let mut names = crate::format::pending_names();
    names.push("electronics-later".into());
    v["format_pending"] = json!(names);
    let e = Document::from_json(&v.to_string()).unwrap_err().to_string();
    assert!(e.contains("electronics-later") && e.contains("update SolidCraft"), "{e}");
}

fn resistor(id: u64, name: &str) -> ElectronicComponent {
    ElectronicComponent {
        id,
        name: name.into(),
        prefix: "R".into(),
        value: "10k".into(),
        mpn: "RC0603FR-0710KL".into(),
        manufacturer: "Example Resistors".into(),
        description: "Thick film resistor, 0603, 1%".into(),
        symbol: None,
        variants: Vec::new(),
    }
}

fn with_components(components: Value) -> Result<Document, String> {
    let mut v = saved(&Document::new("A"));
    v["ecad"] = json!({ "components": components });
    Document::from_json(&v.to_string()).map_err(|e| e.to_string())
}

#[test]
fn electronic_components_round_trip_through_the_design_file() {
    let mut doc = Document::new("A");
    doc.ecad.components = vec![resistor(1, "R0603"), ElectronicComponent { prefix: "U".into(), value: String::new(), ..resistor(2, "LM358") }];
    doc.ecad.next_id = 3;
    let v = saved(&doc);
    assert_eq!(v["ecad"]["components"][0]["mpn"], json!("RC0603FR-0710KL"));
    assert!(v["ecad"]["components"][1].get("value").is_none(), "empty texts are not saved");
    assert_eq!(v["ecad"]["next_id"], json!(3));
    assert_eq!(Document::from_json(&doc.to_json()).unwrap(), doc);
}

#[test]
fn the_id_counter_never_falls_behind_an_id_in_use() {
    let doc = with_components(json!([{"id": 7, "name": "R0603", "prefix": "R"}])).unwrap();
    assert_eq!(doc.ecad.next_id, 8);
    let mut ecad = doc.ecad.clone();
    assert_eq!(ecad.take_id(), Ok(8));
    assert_eq!(ecad.take_id(), Ok(9));
    let mut fresh = Electronics::default();
    assert_eq!(fresh.take_id(), Ok(1), "ids start at 1");
    let mut full = Electronics { next_id: super::MAX_ID - 1, ..Default::default() };
    assert_eq!(full.take_id(), Ok(super::MAX_ID - 1));
    assert!(full.take_id().is_err(), "the counter runs out instead of repeating an id");
}

#[test]
fn damaged_or_unknown_electronic_components_are_refused() {
    let long = "x".repeat(MAX_TEXT + 1);
    let cases = [
        json!([{"id": 1, "name": "R0603", "prefix": "R", "datasheet": "r0603.pdf"}]),
        json!([{"id": 1, "name": "R0603", "prefix": "R", "footprint": {"drill": 1}}]),
        json!([{"id": 0, "name": "R0603", "prefix": "R"}]),
        json!([{"id": 1, "name": "R0603", "prefix": "R"}, {"id": 1, "name": "C0603", "prefix": "C"}]),
        json!([{"id": 1, "name": "R0603", "prefix": "R"}, {"id": 2, "name": "R0603", "prefix": "R"}]),
        json!([{"id": 1, "name": "", "prefix": "R"}]),
        json!([{"id": 1, "name": " R0603", "prefix": "R"}]),
        json!([{"id": 1, "name": "R\n0603", "prefix": "R"}]),
        json!([{"id": 1, "name": long, "prefix": "R"}]),
        json!([{"id": 1, "name": "R0603", "prefix": "R1"}]),
        json!([{"id": 1, "name": "R0603", "prefix": ""}]),
        json!([{"id": 1, "name": "R0603", "prefix": "RESISTORS"}]),
        json!([{"id": 1, "name": "R0603", "prefix": "R", "description": "d".repeat(MAX_DESCRIPTION + 1)}]),
        json!([{"id": 1, "name": "R0603"}]),
        json!([{"id": -1, "name": "R0603", "prefix": "R"}]),
        json!([{"id": 1.5, "name": "R0603", "prefix": "R"}]),
        json!([{"id": 9007199254740992u64, "name": "R0603", "prefix": "R"}]),
        json!([{"id": u64::MAX, "name": "R0603", "prefix": "R"}]),
        json!({"R0603": {}}),
    ];
    for components in cases {
        assert!(with_components(components.clone()).is_err(), "{components} must not open");
    }
    let mut v = saved(&Document::new("A"));
    v["ecad"] = json!({"components": [], "boards": []});
    assert!(Document::from_json(&v.to_string()).is_err(), "a later part is refused, not dropped");
}

#[test]
fn the_component_cap_is_enforced() {
    let mut ecad = Electronics::default();
    for i in 0..=MAX_COMPONENTS {
        let id = ecad.take_id().unwrap();
        ecad.components.push(resistor(id, &format!("R{i}")));
    }
    assert!(ecad.validate().unwrap_err().contains("at most"));
    ecad.components.pop();
    assert!(ecad.validate().is_ok());
}

#[test]
fn pins_and_pads_round_trip_and_connect_by_number() {
    let mut doc = Document::new("A");
    let mut symbol = SymbolItem { id: 1, name: "Resistor".into(), ..Default::default() };
    symbol.pins = vec![
        Pin {
            number: "1".into(),
            name: String::new(),
            name_hidden: false,
            electrical: PinType::Passive,
            at: [-5.08, 0.0],
            direction: PinDirection::Right,
            length: 2.54,
        },
        Pin {
            number: "2".into(),
            name: "B".into(),
            name_hidden: false,
            electrical: PinType::Input,
            at: [5.08, 0.0],
            direction: PinDirection::Left,
            length: 2.54,
        },
    ];
    let mut footprint = FootprintItem { id: 2, name: "R0603".into(), ..Default::default() };
    footprint.pads = vec![
        Pad {
            number: "1".into(),
            at: [-0.8, 0.0],
            size: [0.9, 1.0],
            rotation: 0.0,
            side: Side::Top,
            shape: PadShape::Rect,
            corner: 0.0,
            kind: PadKind::Smd,
            drill: None,
        },
        Pad {
            number: "2".into(),
            at: [0.8, 0.0],
            size: [0.9, 1.0],
            rotation: 90.0,
            side: Side::Bottom,
            shape: PadShape::Rect,
            corner: 0.0,
            kind: PadKind::Smd,
            drill: None,
        },
    ];
    let r = ElectronicComponent {
        symbol: Some(1),
        variants: vec![Variant { name: "0603".into(), footprint: Some(2), package: None, pin_map: None }],
        ..resistor(3, "R0603")
    };
    (doc.ecad.symbols, doc.ecad.footprints, doc.ecad.components) = (vec![symbol], vec![footprint], vec![r]);
    // What opening makes of the counter: one past the largest id.
    doc.ecad.next_id = 4;
    let v = saved(&doc);
    assert_eq!(v["ecad"]["components"][0]["symbol"], json!(1), "a component names its library items by id");
    assert_eq!(v["ecad"]["components"][0]["variants"], json!([{"name": "0603", "footprint": 2}]));
    let pins = &v["ecad"]["symbols"][0]["pins"];
    assert!(pins[0].get("electrical").is_none() && pins[0].get("name").is_none(), "defaults are not saved: {pins}");
    assert_eq!(pins[1]["electrical"], json!("input"));
    let pads = &v["ecad"]["footprints"][0]["pads"];
    assert!(pads[0].get("rotation").is_none() && pads[0].get("side").is_none(), "{pads}");
    assert_eq!(pads[1]["side"], json!("bottom"));
    let back = Document::from_json(&doc.to_json()).unwrap();
    assert_eq!(back, doc);
    let c = &back.ecad.components[0];
    assert_eq!(back.ecad.resolve(c, None).connections(), vec![("1", vec!["1"]), ("2", vec!["2"])]);
    assert_eq!(back.ecad.symbol_of(c).pins[0].body_end(), [-2.54, 0.0]);
}

#[test]
fn damaged_pins_and_pads_are_refused() {
    let base = |key: &str| if key == "symbols" { json!({"id": 1, "name": "Resistor"}) } else { json!({"id": 1, "name": "R0603"}) };
    let pin = json!({"number": "1", "at": [0, 0], "direction": "up", "length": 2.54});
    let pad = json!({"number": "1", "at": [0, 0], "size": [1, 1]});
    let mut cases = Vec::new();
    for (key, list) in [("symbols", json!({"pins": [pin, pin]})), ("footprints", json!({"pads": [pad, pad]}))] {
        let mut item = base(key);
        item.as_object_mut().unwrap().extend(list.as_object().unwrap().clone());
        cases.push((key, item));
    }
    for (key, item, field, junk) in [
        ("symbols", "pins", "length", json!(0)),
        ("symbols", "pins", "direction", json!("sideways")),
        ("symbols", "pins", "at", json!([0, 1e6])),
        ("symbols", "pins", "number", json!("")),
        ("symbols", "pins", "graphics", json!([])),
        ("footprints", "pads", "size", json!([0, 1])),
        ("footprints", "pads", "rotation", json!(400)),
        ("footprints", "pads", "drill", json!(0.8)),
    ] {
        let mut one = if key == "symbols" { pin.clone() } else { pad.clone() };
        one[field] = junk;
        let mut c = base(key);
        c[item] = json!([one]);
        cases.push((key, c));
    }
    for (key, c) in cases {
        let mut v = saved(&Document::new("A"));
        v["ecad"] = json!({ key: [c] });
        assert!(Document::from_json(&v.to_string()).is_err(), "{key}: {c} must not open");
    }
}

// The design-quality checks, on the scratch boards in `ecad_tests/`: each the `ecad` of a design
// made on the CLI (a few generated parts placed on a 4-layer board, with the intent the family's
// checks read), and each case the commands its check was tried with there, done here on the model
// as the board commands do them.

/// The scratch board `name`: `return`, `current`, `esd`, `decap`, `smps`, `pair`, `silk` or
/// `xtal_rf`.
fn scratch(name: &str) -> Electronics {
    let text = match name {
        "return" => include_str!("ecad_tests/return.json"),
        "current" => include_str!("ecad_tests/current.json"),
        "esd" => include_str!("ecad_tests/esd.json"),
        "decap" => include_str!("ecad_tests/decap.json"),
        "smps" => include_str!("ecad_tests/smps.json"),
        "pair" => include_str!("ecad_tests/pair.json"),
        "silk" => include_str!("ecad_tests/silk.json"),
        "xtal_rf" => include_str!("ecad_tests/xtal_rf.json"),
        _ => panic!("no scratch board `{name}`"),
    };
    let mut v = saved(&Document::new("A"));
    v["ecad"] = serde_json::from_str(text).unwrap();
    Document::from_json(&v.to_string()).unwrap().ecad
}

fn layer(v: &Value) -> CopperLayer {
    CopperLayer::parse(v.as_str().unwrap()).unwrap()
}

fn xy(v: &Value) -> [f64; 2] {
    serde_json::from_value(v.clone()).unwrap()
}

fn number(p: &Value, key: &str) -> Option<f64> {
    p.get(key).and_then(Value::as_f64)
}

/// A via's span from a command's `from` and `to` (the top and the bottom when left out).
fn span(p: &Value) -> (CopperLayer, CopperLayer) {
    (p.get("from").map_or(CopperLayer::Top, layer), p.get("to").map_or(CopperLayer::Bottom, layer))
}

/// A new via's size on `net` across `span`, as the commands give it: the rules' micro via between
/// neighbouring layers that aren't the top and the bottom, else the net class's.
fn via_size(board: &Board, net: Option<&str>, (from, to): (CopperLayer, CopperLayer)) -> (f64, f64) {
    let n = board.copper_layers;
    let r = &board.rules;
    if to.index(n) == from.index(n) + 1 && !(from == CopperLayer::Top && to == CopperLayer::Bottom) {
        return (r.microvia_diameter, r.microvia_drill);
    }
    let s = r.sizes(net);
    (s.via_diameter, s.via_drill)
}

fn new_via(id: u64, at: [f64; 2], (diameter, drill): (f64, f64), (from, to): (CopperLayer, CopperLayer), net: Option<String>) -> Via {
    Via { id, at, diameter, drill, from, to, net, protection: ViaProtection::default(), role: ViaRole::Plain, pattern: None }
}

/// The net of the copper at `at` on `layer` (a pad, then a track, then a via), whose class a new
/// track's sizes come from.
fn net_at(e: &Electronics, at: [f64; 2], layer: CopperLayer) -> Option<String> {
    let nets = e.board_nets();
    let board = &e.board;
    let name = |n: Option<usize>| n.and_then(|n| nets.name(n)).map(str::to_string);
    if let Some(p) = nets.pads.iter().find(|p| p.on(layer) && p.contains(at)) {
        return name(p.net);
    }
    let on = |t: &Track| {
        t.layer == layer && path::segments(&t.points, &t.curves, false).any(|(a, b, s)| path::segment_distance(at, a, b, s) <= t.width / 2.0)
    };
    if let Some(i) = board.tracks.iter().position(on) {
        return name(nets.track_nets.get(i).copied().flatten());
    }
    let i = board.vias.iter().position(|v| v.on(layer, board.copper_layers) && (v.at[0] - at[0]).hypot(v.at[1] - at[1]) <= v.diameter / 2.0)?;
    name(nets.via_nets.get(i).copied().flatten())
}

/// Whether a via of `span` at `at` is there already: a via covering both ends of the span, or a
/// through-hole pad.
fn covered(e: &Electronics, at: [f64; 2], (a, b): (CopperLayer, CopperLayer)) -> bool {
    let n = e.board.copper_layers;
    e.board.vias.iter().any(|v| (v.at[0] - at[0]).hypot(v.at[1] - at[1]) <= v.diameter / 2.0 && v.on(a, n) && v.on(b, n))
        || e.board.footprints.iter().flat_map(|f| footprint_pads(e, f, None)).any(|p| p.top && p.bottom && p.contains(at))
}

/// The board footprint of the part `designator`.
fn footprint_of(e: &Electronics, designator: &str) -> u64 {
    let part = e.schematic.parts.iter().find(|p| p.designator == designator).unwrap().id;
    e.board.footprint_of(part).unwrap().id
}

/// Kept as `ecad.intent.set` keeps it: by an agent, with the call's note, in place of the record
/// on the same subject.
fn record(r: &Value, note: &Value) -> Value {
    let mut r = r.clone();
    r["by"] = json!("agent");
    if r.get("note").is_none() {
        r["note"] = note.clone();
    }
    r
}

/// Do `commands` (`[name, params]` pairs, the params as the board commands take them) on `e`:
/// `pour`, `track` (with `via` and `legs`), `via`, `move` (one footprint, `to` and `rotation`),
/// `intent` (`nets`, `parts`, `pairs`) and `stitch` (top to bottom).
fn run(e: &mut Electronics, commands: Value) {
    for c in commands.as_array().unwrap() {
        let (name, p) = (c[0].as_str().unwrap(), &c[1]);
        match name {
            "pour" => {
                let outline = match (p.get("rect"), p.get("points")) {
                    (Some(r), _) => {
                        let ([w, h], [x, y]) = (xy(r), p.get("center").map_or([0.0, 0.0], xy));
                        let (w, h) = (w / 2.0, h / 2.0);
                        [[x - w, y - h], [x + w, y - h], [x + w, y + h], [x - w, y + h]].map(snap_point).to_vec()
                    }
                    (None, Some(points)) => serde_json::from_value(points.clone()).unwrap(),
                    _ => panic!("a pour needs `rect` or `points`: {p}"),
                };
                let mut thermal = json!({"spoke_width": 0.3, "gap": 0.3, "spokes": 4});
                for (k, v) in p.get("thermal").and_then(Value::as_object).into_iter().flatten() {
                    thermal[k] = v.clone();
                }
                let id = e.board.take_id().unwrap();
                let pour = json!({
                    "id": id,
                    "layer": p["layer"],
                    "net": p.get("net"),
                    "outline": outline,
                    "clearance": p.get("clearance"),
                    "connect": p.get("connect").cloned().unwrap_or(json!("thermal")),
                    "thermal": thermal,
                    "remove_islands": p.get("remove_islands").cloned().unwrap_or(json!(true)),
                });
                e.board.pours.push(serde_json::from_value(pour).unwrap());
            }
            "track" => {
                let first = json!({"layer": p["layer"], "points": p["points"], "width": p.get("width"), "via": p.get("via")});
                let legs: Vec<Value> = std::iter::once(first).chain(p.get("legs").and_then(Value::as_array).cloned().unwrap_or_default()).collect();
                let net = net_at(e, xy(&legs[0]["points"][0]), layer(&legs[0]["layer"]));
                let width = number(p, "width").unwrap_or_else(|| e.board.rules.sizes(net.as_deref()).track_width);
                let board = e.board.clone();
                for leg in &legs {
                    let points: Vec<[f64; 2]> = serde_json::from_value(leg["points"].clone()).unwrap();
                    let last = *points.last().unwrap();
                    let via = leg.get("via").filter(|v| !v.is_null());
                    if !(points.len() == 1 && via.is_some()) {
                        let id = e.board.take_id().unwrap();
                        let width = number(leg, "width").unwrap_or(width);
                        e.board.tracks.push(Track { id, layer: layer(&leg["layer"]), width, points, curves: Vec::new() });
                    }
                    if let Some(v) = via {
                        let span = span(v);
                        let (d, h) = via_size(&board, net.as_deref(), span);
                        if !covered(e, last, span) {
                            let id = e.board.take_id().unwrap();
                            e.board.vias.push(new_via(id, last, (number(v, "diameter").unwrap_or(d), number(v, "drill").unwrap_or(h)), span, None));
                        }
                    }
                }
            }
            "via" => {
                let (span, net) = (span(p), p.get("net").and_then(Value::as_str).map(str::to_string));
                let (d, h) = via_size(&e.board, net.as_deref(), span);
                let id = e.board.take_id().unwrap();
                e.board.vias.push(new_via(id, xy(&p["at"]), (number(p, "diameter").unwrap_or(d), number(p, "drill").unwrap_or(h)), span, net));
            }
            "move" => {
                let items = p["items"].as_array().unwrap();
                assert_eq!(items.len(), 1, "one footprint at a time: {p}");
                let id = footprint_of(e, items[0].as_str().unwrap());
                let (about, to) = (e.board.position(id).unwrap(), xy(&p["to"]));
                let rotation = u16::try_from(p.get("rotation").and_then(Value::as_u64).unwrap_or(0)).unwrap();
                let m = Motion { about, mirror: false, rotation, by: snap_point([to[0] - about[0], to[1] - about[1]]) };
                let mut board = e.board.clone();
                e.move_on_board(&mut board, &[(BTreeSet::from([id]), m)]);
                e.board = board;
            }
            "intent" => {
                let note = p.get("note").cloned().unwrap_or(json!("test"));
                let list = |key: &str| p.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
                for r in list("nets") {
                    let r: NetIntent = serde_json::from_value(record(&r, &note)).unwrap();
                    e.intent.nets.retain(|x| x.net != r.net);
                    e.intent.nets.push(r);
                }
                for r in list("parts") {
                    let r: PartIntent = serde_json::from_value(record(&r, &note)).unwrap();
                    e.intent.parts.retain(|x| x.part != r.part);
                    e.intent.parts.push(r);
                }
                for r in list("pairs") {
                    let r: PairIntent = serde_json::from_value(record(&r, &note)).unwrap();
                    e.intent.pairs.retain(|x| x.name != r.name);
                    e.intent.pairs.push(r);
                }
            }
            "stitch" => {
                let net = p["net"].as_str().unwrap().to_string();
                let span = (CopperLayer::Top, CopperLayer::Bottom);
                let (diameter, drill) = via_size(&e.board, Some(&net), span);
                let pattern: Pattern = serde_json::from_value(p["pattern"].clone()).unwrap();
                let ask = Ask {
                    net: net.clone(),
                    pattern,
                    area: None,
                    tracks: Vec::new(),
                    nets: Vec::new(),
                    pitch: number(p, "pitch"),
                    offset: None,
                    via: ViaSpec { diameter, drill, from: span.0, to: span.1 },
                    clearance: None,
                    under_parts: p.get("under_parts").and_then(Value::as_bool).unwrap_or(false),
                    target: None,
                    budget: None,
                };
                let plan = stitch::plan(e, &e.board_nets(), &ask).unwrap();
                assert!(!plan.spots.is_empty(), "{:?}", plan.skipped);
                for at in plan.spots {
                    let id = e.board.take_id().unwrap();
                    let via = new_via(id, at, (diameter, drill), span, Some(net.clone()));
                    e.board.vias.push(Via { protection: ViaProtection::Tented, role: ViaRole::Stitch, pattern: Some(pattern), ..via });
                }
            }
            _ => panic!("no command `{name}` here"),
        }
    }
    if let Err(err) = e.validate() {
        panic!("the commands left a board that doesn't open: {err}");
    }
}

type Found = Vec<(&'static str, Severity, Status)>;

/// What the quality checks `codes` find on `e`: each finding's code, severity and status, in the
/// report's order.
fn quality_of(e: &Electronics, codes: &[&str]) -> Found {
    let only: BTreeSet<&str> = codes.iter().copied().collect();
    let r = quality::check(e, &e.board_nets(), Some(&only));
    assert!(!r.truncated && r.cut.is_empty(), "{codes:?} ran out of work");
    r.findings.iter().map(|q| (q.finding.code, q.finding.severity, q.status)).collect()
}

/// The scratch board `board` after `commands`, and what `codes` find on it.
fn case(board: &str, commands: Value, codes: &[&str]) -> Found {
    let mut e = scratch(board);
    run(&mut e, commands);
    quality_of(&e, codes)
}

/// The severities of the findings of `code` that ask for a fix.
fn fails(found: &Found, code: &str) -> Vec<Severity> {
    found.iter().filter(|(c, _, s)| *c == code && *s == Status::Fail).map(|f| f.1).collect()
}

const RETURN: &[&str] = &["pi.return_path", "pi.layer_change", "pi.stitching"];

#[test]
fn return_paths_flag_a_slotted_plane_a_lone_layer_change_and_sparse_stitching() {
    // The board: SIG (fast, 1 ns) from R1 to R2 along y = 0, OTHER on J1, GND and VCC; four layers,
    // 2.4 GHz the frequency of concern.
    let gnd = |layer: &str| json!(["pour", {"layer": layer, "net": "GND", "rect": [30, 20], "remove_islands": false}]);
    let sig = json!(["track", {"layer": "top", "points": [[-9.2, 0], [9.2, 0]], "width": 0.2}]);
    // OTHER's track on inner1 cuts a slot into the plane under SIG.
    let slot = json!(["track", {"layer": "inner1", "points": [[3, 7.27], [0, 7.27], [0, -4]], "width": 0.2}]);
    assert_eq!(fails(&case("return", json!([gnd("inner1"), sig, slot]), RETURN), "pi.return_path"), [Severity::Error]);
    assert_eq!(case("return", json!([gnd("inner1"), sig]), RETURN), []);
    // SIG drops to the bottom (over inner2) and back up: its return has to change planes.
    let hops = json!([
        gnd("inner1"),
        gnd("inner2"),
        ["track", {"layer": "top", "points": [[-9.2, 0], [-3, 0]], "width": 0.2, "via": {}}],
        ["track", {"layer": "bottom", "points": [[-3, 0], [3, 0]], "width": 0.2, "via": {}}],
        ["track", {"layer": "top", "points": [[3, 0], [9.2, 0]], "width": 0.2}],
    ]);
    assert_eq!(fails(&case("return", hops.clone(), RETURN), "pi.layer_change"), [Severity::Warning]);
    let mut tied = hops.as_array().unwrap().clone();
    tied.push(json!(["via", {"at": [0, -2], "net": "GND"}]));
    assert_eq!(fails(&case("return", Value::Array(tied), RETURN), "pi.layer_change"), [], "a ground via beside the change ties the planes");
    // Two ground planes tied only at the corners, then stitched over the board.
    let mut sparse = vec![gnd("inner1"), gnd("inner2"), sig.clone()];
    sparse.extend([[-13, -8], [-13, 8], [13, -8], [13, 8]].map(|at| json!(["via", {"at": at, "net": "GND"}])));
    assert_eq!(fails(&case("return", Value::Array(sparse.clone()), RETURN), "pi.stitching"), [Severity::Warning]);
    sparse.push(json!(["stitch", {"net": "GND", "pattern": "grid", "under_parts": true}]));
    assert_eq!(case("return", Value::Array(sparse), RETURN), []);
}

const CURRENT: &[&str] = &["pi.current", "pi.bottleneck"];

#[test]
fn current_checks_flag_a_thin_track_and_a_neck_in_a_pour() {
    // The board: VIN carries 3 A (declared) from J1 to the loads U1 and U2; 20 °C rise allowed.
    let feed = |width: f64| json!([["track", {"layer": "top", "points": [[-25, 1.27], [-22, 0], [18.5, 0]], "width": width}]]);
    assert_eq!(fails(&case("current", feed(0.2), CURRENT), "pi.current"), [Severity::Error], "0.2 mm warms by about 130 °C");
    assert_eq!(fails(&case("current", feed(0.7), CURRENT), "pi.current"), [Severity::Warning], "0.7 mm by about 31 °C");
    assert_eq!(case("current", feed(2.5), CURRENT), []);
    // A bottom VIN pour from J1, narrowed between x -10 and 0, fed to U1 through four vias.
    let neck = |y0: f64, y1: f64| {
        let outline = json!([[-28, -4], [-10, -4], [-10, y0], [0, y0], [0, -4], [15, -4], [15, 6], [0, 6], [0, y1], [-10, y1], [-10, 6], [-28, 6]]);
        let mut c = vec![json!(["pour", {"layer": "bottom", "net": "VIN", "points": outline, "connect": "solid"}])];
        c.extend([0.0, 0.8, -0.8, 1.6].map(|y| json!(["via", {"at": [13, y], "diameter": 0.6, "drill": 0.3}])));
        c.push(json!(["track", {"layer": "top", "points": [[13, 0], [18.5, 0]], "width": 2.5}]));
        Value::Array(c)
    };
    assert_eq!(fails(&case("current", neck(1.0, 1.6), CURRENT), "pi.bottleneck"), [Severity::Warning], "a 0.6 mm neck");
    assert_eq!(case("current", neck(0.0, 3.0), CURRENT), [], "a 3 mm neck");
    // Without the current there is nothing to check against: not applicable, never a pass.
    let mut e = scratch("current");
    run(&mut e, feed(0.2));
    run(&mut e, json!([["intent", {"nets": [{"net": "VIN", "role": "power", "voltage": {"nominal": 5}}]}]]));
    let r = quality::check(&e, &e.board_nets(), Some(&CURRENT.iter().copied().collect()));
    assert!(r.findings.is_empty(), "{:?}", r.findings);
    assert_eq!(r.not_applicable.iter().map(|n| n.code).collect::<Vec<_>>(), ["pi.bottleneck", "pi.current"]);
    assert!(r.not_applicable.iter().all(|n| n.count == 1 && n.needs.iter().any(|x| x.contains("VIN"))), "{:?}", r.not_applicable);
}

const ESD: &[&str] = &["esd.path", "esd.ground"];

#[test]
fn esd_checks_flag_a_bypassed_protector_and_an_unground_one() {
    // The board: DATA from the connector J1 to U1, the TVS D1 on it near J1, D1's ground pad tied
    // to GND by two vias.
    let ground = json!([
        ["track", {"layer": "top", "points": [[-16.0, 2.245], [-16.45, 2.95]], "width": 0.25}],
        ["track", {"layer": "top", "points": [[-16.0, 2.245], [-15.55, 2.95]], "width": 0.25}],
        ["via", {"at": [-16.45, 2.95], "net": "GND"}],
        ["via", {"at": [-15.55, 2.95], "net": "GND"}],
    ]);
    let through = json!([
        ["track", {"layer": "top", "points": [[-20, 1.27], [-16.95, 0.295]], "width": 0.25}],
        ["track", {"layer": "top", "points": [[-15.05, 0.295], [-14, 0.295], [10, 1.27], [11.2, 1.27]], "width": 0.25}],
    ]);
    let both = |a: &Value, b: &Value| Value::Array(a.as_array().unwrap().iter().chain(b.as_array().unwrap()).cloned().collect());
    assert_eq!(case("esd", both(&through, &ground), ESD), []);
    // DATA routed past D1 without touching it.
    let past = json!([["track", {"layer": "top", "points": [[-20, 1.27], [-18, -0.8], [9, -0.8], [11.2, 1.27]], "width": 0.25}]]);
    assert_eq!(fails(&case("esd", both(&past, &ground), ESD), "esd.path"), [Severity::Error]);
    // Through D1, but its ground pad joined to nothing.
    assert_eq!(fails(&case("esd", through.clone(), ESD), "esd.ground"), [Severity::Error]);
    let mut solid = both(&through, &ground).as_array().unwrap().clone();
    solid.push(json!(["pour", {"layer": "top", "net": "GND", "rect": [10, 8], "center": [-16, 3], "connect": "solid"}]));
    assert_eq!(case("esd", Value::Array(solid), ESD), []);
}

const DECAP: &[&str] = &["pi.decap_assigned", "pi.decap_distance", "pi.decap_mounting", "pi.decap_topology"];

#[test]
fn decoupling_checks_flag_an_unvia_ed_pad_and_a_capacitor_off_the_supply_path() {
    // The board: U1 (QFN-16) with VDD on pins 1 and 2, C1 to C3 its capacitors, J1 the supply.
    let assigned = json!(["intent", {"parts": [
        {"part": "C1", "role": "decoupling", "of": ["U1.1"]},
        {"part": "C2", "role": "decoupling", "of": ["U1.5"]},
        {"part": "C3", "role": "decoupling", "of": ["U1.6"]},
    ], "note": "scratch"}]);
    let c1 = json!(["move", {"items": ["C1"], "to": [-3.2, 0.75], "rotation": 180}]);
    let planes = [
        json!(["pour", {"layer": "inner1", "net": "GND", "rect": [40, 30], "remove_islands": false}]),
        json!(["pour", {"layer": "inner2", "net": "VDD", "rect": [40, 30], "remove_islands": false}]),
    ];
    let ground_via = json!(["track", {"layer": "top", "points": [[-3.725, 0.75], [-3.725, 1.35]], "width": 0.2, "via": {}}]);
    let mounted = |vdd: Value| json!([assigned, planes[0], planes[1], c1, vdd, ground_via]);
    let good = mounted(json!(["track", {"layer": "top", "points": [[-2.675, 0.75], [-2.675, 1.35]], "width": 0.2, "via": {}}]));
    assert_eq!(fails(&case("decap", good, DECAP), "pi.decap_mounting"), []);
    // C1's VDD pad runs 9 mm off to nowhere: no via to the VDD plane beside it.
    let none = mounted(json!(["track", {"layer": "top", "points": [[-2.675, 0.75], [-2.675, 9.0], [-12.0, 9.0]], "width": 0.2}]));
    assert_eq!(fails(&case("decap", none, DECAP), "pi.decap_mounting"), [Severity::Warning]);
    // The supply from J1 reaches U1.1 before C1 (the capacitor hangs off the far side), against
    // the supply coming in through C1's pad.
    let feed = |points: Value| json!([assigned, c1, ["track", {"layer": "top", "points": points, "width": 0.2}], ["track", {"layer": "top", "points": [[-2.675, 0.75], [-1.425, 0.75]], "width": 0.2}]]);
    let between = feed(json!([[-17.81, 0.0], [-17.81, -2.5], [-2.0, -2.5], [-2.0, 0.4], [-1.425, 0.75]]));
    assert_eq!(fails(&case("decap", between, DECAP), "pi.decap_topology"), [Severity::Warning]);
    let through = feed(json!([[-17.81, 0.0], [-17.81, 2.5], [-2.675, 2.5], [-2.675, 0.75]]));
    assert_eq!(fails(&case("decap", through, DECAP), "pi.decap_topology"), []);
}

const SMPS: &[&str] = &["pi.smps_hot_loop", "pi.smps_switch_node", "pi.smps_feedback"];

#[test]
fn regulator_checks_flag_a_far_input_capacitor_and_a_trace_beside_the_switch_node() {
    // The board: the buck U1, its input capacitor C1, inductor L1, output capacitor C2 and the
    // feedback divider R1, R2; a ground plane on inner1.
    let stage = |c1_x: f64, bend: f64| {
        json!([
            ["pour", {"layer": "inner1", "net": "GND", "rect": [40, 30], "remove_islands": false}],
            ["move", {"items": ["C1"], "to": [c1_x, 0.635], "rotation": 270}],
            ["track", {"layer": "top", "points": [[c1_x, 1.16], [bend, 1.905], [-2.58, 1.905]], "width": 0.25}],
            ["track", {"layer": "top", "points": [[c1_x, 0.11], [bend, -0.635], [-2.58, -0.635]], "width": 0.25}],
            ["move", {"items": ["L1"], "to": [2.0, 5.0], "rotation": 0}],
            ["track", {"layer": "top", "points": [[-2.58, 0.635], [0.0, 0.635], [0.0, 5.0], [1.05, 5.0]], "width": 0.25}],
            ["move", {"items": ["C2"], "to": [6.0, 5.0], "rotation": 0}],
            ["track", {"layer": "top", "points": [[2.95, 5.0], [5.475, 5.0]], "width": 0.25}],
            ["move", {"items": ["R1"], "to": [-2.6, -4.0], "rotation": 0}],
            ["move", {"items": ["R2"], "to": [-0.5, -4.0], "rotation": 0}],
            ["track", {"layer": "top", "points": [[-2.58, -1.905], [-2.075, -3.0], [-2.075, -4.0]], "width": 0.25}],
            ["track", {"layer": "top", "points": [[-2.075, -4.0], [-1.025, -4.0]], "width": 0.25}],
            ["track", {"layer": "top", "points": [[-3.125, -4.0], [-3.125, -6.0], [7.5, -6.0], [7.5, 3.0], [5.475, 3.0], [5.475, 5.0]], "width": 0.25}],
        ])
    };
    let good = case("smps", stage(-4.4, -3.6), SMPS);
    assert_eq!(fails(&good, "pi.smps_hot_loop"), [], "{good:?}");
    assert_eq!(good.iter().filter(|f| f.2 == Status::Review).count(), 1, "the loop's size is listed for a look: {good:?}");
    assert_eq!(fails(&case("smps", stage(-8.0, -6.0), SMPS), "pi.smps_hot_loop"), [Severity::Warning; 2], "C1 4.4 mm from U1");
    // The feedback net run up beside the switch node, against a run that keeps away.
    let with = |points: Value| {
        let mut c = stage(-4.4, -3.6).as_array().unwrap().clone();
        c.push(json!(["track", {"layer": "top", "points": points, "width": 0.25}]));
        Value::Array(c)
    };
    assert_eq!(
        fails(&case("smps", with(json!([[-1.025, -4.0], [0.5, -4.0], [0.5, 2.0], [0.5, 4.0]])), SMPS), "pi.smps_switch_node"),
        [Severity::Warning]
    );
    assert_eq!(fails(&case("smps", with(json!([[-1.025, -4.0], [4.5, -4.0], [4.5, 1.0]])), SMPS), "pi.smps_switch_node"), []);
}

const XTAL: &[&str] = &["si.xtal_loop", "si.xtal_keepout"];
const RF: &[&str] = &["rf.line", "rf.reference", "rf.antenna_keepout", "rf.coplanar_gap", "rf.fence"];

#[test]
fn crystal_checks_flag_long_crystal_nets() {
    // The board: the MCU U1, its crystal Y1 with load capacitors C1 and C2, ground on inner1.
    let plane = json!(["pour", {"layer": "inner1", "net": "GND", "rect": [50, 30], "remove_islands": false}]);
    let routed = |x: f64, caps: f64| {
        json!([
            ["track", {"layer": "top", "points": [[-7.5825, 1.905], [x, 2.77]], "width": 0.2}],
            ["track", {"layer": "top", "points": [[-7.5825, 0.635], [x, -0.23]], "width": 0.2}],
            ["track", {"layer": "top", "points": [[caps, 3.5], [x, 2.77]], "width": 0.2}],
            ["track", {"layer": "top", "points": [[caps, -1.0], [x, -0.23]], "width": 0.2}],
        ])
    };
    let mut good = vec![plane.clone()];
    good.extend(routed(-11.0, -12.7).as_array().unwrap().clone());
    assert_eq!(case("xtal_rf", Value::Array(good), XTAL), []);
    let mut far = vec![
        plane,
        json!(["move", {"items": ["Y1"], "to": [-15.5, 1.27]}]),
        json!(["move", {"items": ["C1"], "to": [-18, 3.5]}]),
        json!(["move", {"items": ["C2"], "to": [-18, -1]}]),
    ];
    far.extend(routed(-15.5, -17.2).as_array().unwrap().clone());
    assert_eq!(fails(&case("xtal_rf", Value::Array(far), XTAL), "si.xtal_loop"), [Severity::Warning; 2], "8 mm of copper each, 5 mm allowed");
}

#[test]
fn rf_checks_flag_a_mismatched_line_and_a_foreign_plane_under_it() {
    // The board: the radio U1's RF pin to the antenna along a 2.4 GHz line over GND on inner1.
    let plane = json!(["pour", {"layer": "inner1", "net": "GND", "rect": [50, 30], "remove_islands": false}]);
    let line = |width: f64| json!(["track", {"layer": "top", "points": [[-2.4175, 1.905], [3, 1.905], [7.095, 6], [18.5, 6]], "width": width}]);
    let good = case("xtal_rf", json!([plane, line(0.3349)]), RF);
    assert!(good.iter().all(|f| f.0 == "rf.antenna_keepout" && f.2 == Status::Review), "only the antenna's keepout to look at: {good:?}");
    assert_eq!(fails(&case("xtal_rf", json!([plane, line(0.6)]), RF), "rf.line"), [Severity::Error], "0.6 mm is far from 50 Ω");
    // Another net's track on inner1 runs under the line, cutting its plane.
    let foreign = json!(["track", {"layer": "top", "points": [[-0.8, -6], [-0.8, -4]], "width": 0.2, "via": {}, "legs": [{"layer": "inner1", "points": [[-0.8, -4], [5, -4], [5, 8]]}]}]);
    assert_eq!(fails(&case("xtal_rf", json!([plane, line(0.3349), foreign]), RF), "rf.reference"), [Severity::Error]);
}

#[test]
fn pair_checks_flag_a_pair_that_splits_apart() {
    // The board: U1 and U2 joined by DP and DN, a USB high-speed pair with a 0.2 mm class gap.
    let pair = json!(["intent", {"pairs": [{"name": "USB", "p": "DP", "n": "DN", "interface": "usb2_hs"}], "note": "test pair"}]);
    let dp = json!(["track", {"layer": "top", "points": [[-15, 14.2], [-14, 14.8], [14, 14.8], [15, 14.2]], "width": 0.2}]);
    let dn = json!(["track", {"layer": "top", "points": [[-15, 15.8], [-14, 15.2], [14, 15.2], [15, 15.8]], "width": 0.2}]);
    let codes = ["si.pair", "si.skew"];
    assert_eq!(case("pair", json!([pair, dp, dn]), &codes), []);
    let apart = json!(["track", {"layer": "top", "points": [[-15, 15.8], [-14, 15.2], [-6.2, 15.2], [-6, 15.35], [6, 15.35], [6.2, 15.2], [14, 15.2], [15, 15.8]], "width": 0.2}]);
    assert_eq!(fails(&case("pair", json!([pair, dp, apart]), &codes), "si.pair"), [Severity::Warning], "0.35 mm apart for 12 mm");
}

#[test]
fn assembly_checks_flag_tombstoning_pads_and_solid_pads_in_a_pour() {
    // The board: chips of three sizes, one pad of each on a track; a top GND pour.
    let tracks = [5.525, 10.375, 15.8].map(|x| json!(["track", {"layer": "top", "points": [[x, -2], [x, -4.5]], "width": 0.2}]));
    let pour = |connect: &str| json!(["pour", {"layer": "top", "net": "GND", "rect": [40, 30], "remove_islands": false, "connect": connect}]);
    let mut solid = tracks.to_vec();
    solid.push(pour("solid"));
    let tomb = case("silk", Value::Array(solid.clone()), &["dfm.tombstone"]);
    assert_eq!(fails(&tomb, "dfm.tombstone").first(), Some(&Severity::Error), "{tomb:?}");
    let mut even = tracks.to_vec();
    even.extend([4.475, 9.625, 14.2].map(|x| json!(["track", {"layer": "top", "points": [[x, -2], [x, -4.5]], "width": 0.2}])));
    assert_eq!(case("silk", Value::Array(even), &["dfm.tombstone"]), [], "both pads of each on a like track");
    let in_pour = case("silk", Value::Array(solid), &["dfm.pad_to_pour"]);
    assert!(!in_pour.is_empty() && in_pour.iter().all(|f| (f.0, f.1) == ("dfm.pad_to_pour", Severity::Warning)), "{in_pour:?}");
    let mut thermal = tracks.to_vec();
    thermal.push(pour("thermal"));
    assert_eq!(case("silk", Value::Array(thermal), &["dfm.pad_to_pour"]), []);
}

#[test]
fn quality_findings_are_the_same_each_run_and_approvals_hold_them() {
    let mut e = scratch("current");
    run(&mut e, json!([["track", {"layer": "top", "points": [[-25, 1.27], [-22, 0], [18.5, 0]], "width": 0.2}]]));
    let nets = e.board_nets();
    let first = quality::check(&e, &nets, None);
    assert_eq!(quality::check(&e, &nets, None), first, "the same board, the same findings");
    assert_eq!(e.quality(), first);
    let current = first.findings.iter().find(|q| q.finding.code == "pi.current").unwrap();
    assert!(current.measured && current.finding.measured > current.finding.limit.value, "{current:?}");
    let key = current.finding.key.clone();
    // The user's approval takes it out of the count; an agent's of a critical finding waits for theirs.
    for (by, approved, pending) in [(Actor::User, 1, 0), (Actor::Agent, 0, 1)] {
        let mut e = e.clone();
        e.board.quality_approvals = vec![Approval { key: key.clone(), note: "carries less in the product".into(), by: Some(by) }];
        let r = e.quality();
        assert_eq!((r.approved, r.pending_user), (approved, pending), "by {by:?}");
        assert_eq!(r.errors, first.errors - approved, "by {by:?}");
    }
    // An approval of a code that ran with nothing under its key is stale; one of a code that
    // didn't run is not.
    let mut e2 = e.clone();
    e2.board.quality_approvals = vec![
        Approval { key: "pi.current|t999".into(), note: String::new(), by: Some(Actor::User) },
        Approval { key: key.clone(), note: String::new(), by: Some(Actor::User) },
    ];
    assert_eq!(e2.quality().stale, ["pi.current|t999"]);
    let only: BTreeSet<&str> = BTreeSet::from(["pi.current"]);
    assert!(quality::check(&e2, &nets, Some(&only)).stale.is_empty(), "a filtered run names no approval stale");
    // A small budget cuts the run short and says so; it never hangs or panics.
    let short = quality::check_within(&e, &nets, None, 16);
    assert!(short.truncated && !short.cut.is_empty(), "{:?}", short.cut);
    assert!(short.stale.is_empty());
}

#[test]
fn quality_settings_keep_to_the_catalogue() {
    let mut board = scratch("return").board;
    assert!(quality::validate_settings(&board).is_ok());
    board.quality_levels.insert("pi.no_such_code".into(), super::drc::CheckLevel::Ignore);
    assert!(quality::validate_settings(&board).unwrap_err().contains("no code"));
    let mut board = scratch("return").board;
    let p = catalog::codes().find_map(|c| c.params.first().map(|p| (c.code, *p))).unwrap();
    let key = format!("{}.{}", p.0, p.1.name);
    for (value, ok) in [(p.1.default, true), (p.1.min, true), (p.1.max, true), (p.1.max + 1.0, false), (f64::NAN, false), (f64::INFINITY, false)] {
        board.quality_params.insert(key.clone(), quality::ParamSetting { value, note: String::new(), by: Some(Actor::User) });
        assert_eq!(quality::validate_settings(&board).is_ok(), ok, "{key} = {value}");
    }
    board.quality_params.clear();
    board.quality_params.insert(format!("{}.no_such_param", p.0), quality::ParamSetting { value: 1.0, note: String::new(), by: None });
    assert!(quality::validate_settings(&board).unwrap_err().contains("no parameter"));
    let mut board = scratch("return").board;
    let twice = Approval { key: "pi.current|t1".into(), note: String::new(), by: None };
    board.quality_approvals = vec![twice.clone(), twice];
    assert!(quality::validate_settings(&board).unwrap_err().contains("two quality approvals"));
    // The board's own parameter replaces the default: a 40 °C rise passes a 0.7 mm feed.
    let mut e = scratch("current");
    run(&mut e, json!([["track", {"layer": "top", "points": [[-25, 1.27], [-22, 0], [18.5, 0]], "width": 0.7}]]));
    assert_eq!(fails(&quality_of(&e, CURRENT), "pi.current"), [Severity::Warning]);
    assert_eq!(quality::param_value(&e.board, "pi.current", "rise"), catalog::find("pi.current").unwrap().param("rise").unwrap().default);
    e.board
        .quality_params
        .insert("pi.current.rise".into(), quality::ParamSetting { value: 40.0, note: "the user allows 40 °C".into(), by: Some(Actor::User) });
    assert_eq!(quality::param_value(&e.board, "pi.current", "rise"), 40.0);
    assert_eq!(quality_of(&e, CURRENT), []);
}

#[test]
fn the_quality_catalogue_is_consistent() {
    let mut seen = BTreeSet::new();
    for f in quality::FAMILIES {
        assert!(!f.codes.is_empty(), "{} has no codes", f.module);
        for c in f.codes {
            assert!(seen.insert(c.code), "{} twice", c.code);
            assert!(["pi", "si", "rf", "esd", "silk", "dfm"].contains(&c.family()), "{}", c.code);
            assert!(c.section.contains('#') && !c.about.is_empty() && !c.principle.is_empty(), "{}", c.code);
            assert_eq!(catalog::find(c.code).map(|x| x.code), Some(c.code));
            assert!(c.worst_class() <= c.class && c.worst_severity() <= c.severity, "{}", c.code);
            let mut names = BTreeSet::new();
            for p in c.params {
                assert!(names.insert(p.name), "{}.{} twice", c.code, p.name);
                assert!(p.min <= p.default && p.default <= p.max && p.default.is_finite(), "{}.{}", c.code, p.name);
                assert!(!p.units.is_empty() && !p.source.is_empty(), "{}.{}", c.code, p.name);
            }
        }
    }
    assert!(catalog::find("pi.nothing").is_none());
    assert!(catalog::not_built().is_empty(), "every family has codes");
    assert_eq!(catalog::families(), ["pi", "esd", "si", "rf", "silk", "dfm"]);
    assert_eq!(catalog::anchor("Pad to pour"), "pad-to-pour");
    assert_eq!(catalog::anchor("2. Critical loops"), "2-critical-loops");
}

#[test]
fn quality_codes_match_skill_sections() {
    // The guide every quality and design rule check code points into: `skills/solidcraft-pcb`.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/solidcraft-pcb/reference");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|x| x == "md") {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            files.push((name, std::fs::read_to_string(&path).unwrap()));
        }
    }
    assert!(files.len() >= 5, "{}", dir.display());
    let refs: Vec<(&str, &str)> = files.iter().map(|(n, t)| (n.as_str(), t.as_str())).collect();
    let misses = catalog::section_misses(&refs);
    assert!(misses.is_empty(), "sections or links the guide lacks:\n{}", misses.join("\n"));
    // And it does catch a renamed heading (`pi.return_path` points at power-ground#return-paths).
    let renamed: Vec<(&str, String)> = files.iter().map(|(n, t)| (n.as_str(), t.replace("## Return paths", "## Return currents"))).collect();
    let renamed: Vec<(&str, &str)> = renamed.iter().map(|(n, t)| (*n, t.as_str())).collect();
    let misses = catalog::section_misses(&renamed);
    assert!(misses.iter().any(|m| m.starts_with("pi.return_path → power-ground#return-paths")), "{misses:?}");
}

// The current calculator, against the published closed forms worked out by hand.

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * b.abs().max(1.0)
}

#[test]
fn track_widths_follow_ipc_2221_and_the_2152_fit() {
    // 100 mil of 1 oz on an outer layer carries 4.68 A at 10 °C (IPC-2221), 3.56 A by the 2152 fit.
    let c = current::width_for(4.68, 10.0, OZ_MM, true).unwrap();
    assert!(close(c.width, 2.546_22, 1e-5) && c.valid_range, "{c:?}");
    assert!(close(current::rise_2152(3.56, 2.54, OZ_MM).unwrap(), 10.010_33, 1e-5));
    // An inner layer gets half the k: 2^(1/0.725) = 2.6 times the width.
    let (outer, inner) = (current::width_for(1.0, 10.0, OZ_MM, true).unwrap(), current::width_for(1.0, 10.0, OZ_MM, false).unwrap());
    assert!(close(outer.width, 0.302_98, 1e-4) && close(inner.width, 0.788_19, 1e-4), "{outer:?} {inner:?}");
    assert!(close(current::rise_for(2.0, 0.5, OZ_MM, true).unwrap().rise, 21.1687, 1e-4));
    // Width and rise are each other's inverse.
    for (i, dt) in [(0.1, 5.0), (3.0, 20.0), (30.0, 80.0)] {
        let w = current::width_for(i, dt, OZ_MM, true).unwrap().width;
        assert!(close(current::rise_for(i, w, OZ_MM, true).unwrap().rise, dt, 1e-9), "{i} A {dt} °C");
        let w = current::width_2152(i, dt, OZ_MM).unwrap();
        assert!(close(current::rise_2152(i, w, OZ_MM).unwrap(), dt, 1e-9), "{i} A {dt} °C by the 2152 fit");
    }
    let fit = current::width_for_fit(Fit::Ipc2152, 1.0, 10.0, OZ_MM, false).unwrap();
    assert!(close(fit.width, 0.279_37, 1e-4) && fit.valid_range, "{fit:?}");
    assert_eq!(current::width_for_fit(Fit::Ipc2221, 1.0, 10.0, OZ_MM, true).unwrap(), outer);
    // Outside the fits' published range the answer stands but says so.
    assert!(!current::width_for(40.0, 10.0, OZ_MM, true).unwrap().valid_range, "past 35 A");
    assert!(!current::width_for(1.0, 5.0, OZ_MM, true).unwrap().valid_range, "below a 10 °C rise");
    assert!(!current::in_range_2152(10.0, current::oz_to_mm(0.5)) && current::in_range_2152(10.0, current::oz_to_mm(2.0)));
    assert!(close(current::mm_to_oz(current::oz_to_mm(2.0)), 2.0, 1e-12));
}

#[test]
fn track_losses_follow_copper_resistivity() {
    // 100 mm of 1 mm by 35 µm at 20 °C: 1.724e-8 Ω·m · 0.1 m / 3.5e-8 m² = 49.257 mΩ.
    let l = current::loss(1.0, 1.0, 0.035, 100.0, 20.0).unwrap();
    assert!(close(l.resistance, 0.049_257_1, 1e-5) && close(l.voltage_drop, l.resistance, 1e-12) && close(l.power, l.resistance, 1e-12), "{l:?}");
    // At 45 °C it is 1 + 0.00393 · 25 times that; 2 A drop twice and lose four times as much.
    let hot = current::loss(2.0, 1.0, 0.035, 100.0, 45.0).unwrap();
    assert!(close(hot.resistance, 0.054_096_7, 1e-5) && close(hot.voltage_drop, 0.108_193, 1e-5) && close(hot.power, 0.216_387, 1e-5), "{hot:?}");
    assert_eq!(current::loss(1.0, 1.0, 0.035, 0.0, 20.0), Err(CurrentError::Length));
    assert_eq!(current::loss(1.0, 1.0, 0.035, 10.0, f64::NAN), Err(CurrentError::NoResult));
    assert_eq!(current::loss(1.0, 0.0, 0.035, 10.0, 20.0), Err(CurrentError::NoResult));
}

#[test]
fn the_current_calculator_refuses_hostile_numbers() {
    for v in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e9] {
        assert_eq!(current::width_for(v, 10.0, OZ_MM, true), Err(CurrentError::Current), "current {v}");
        assert_eq!(current::width_for(1.0, v, OZ_MM, true), Err(CurrentError::Rise), "rise {v}");
        assert_eq!(current::width_for(1.0, 10.0, v, true), Err(CurrentError::Copper), "copper {v}");
        assert_eq!(current::rise_for(1.0, v, OZ_MM, true), Err(CurrentError::Width), "width {v}");
        assert!(current::width_for_fit(Fit::Ipc2152, 1.0, v, OZ_MM, true).is_err(), "2152 rise {v}");
        assert!(current::rise_2152(1.0, v, OZ_MM).is_none_or(f64::is_finite));
        assert!(current::width_2152(1.0, v, OZ_MM).is_none_or(|w| w.is_finite() && w > 0.0));
    }
    for e in [CurrentError::Current, CurrentError::Copper, CurrentError::NoResult] {
        assert!(e.key().starts_with("electronics.current_err_") && !e.to_string().is_empty());
    }
    assert!(CurrentError::Copper.args().iter().any(|(k, _)| *k == "max_oz"));
}

#[test]
fn net_class_widths_come_from_its_current_and_the_stack_up() {
    let four = default_stackup(4, 1.6);
    assert_eq!(current::stack_copper(&four), (0.035, 0.0175));
    assert_eq!(current::stack_copper(&default_stackup(2, 1.6)), (0.035, 0.035), "no inner layers: the outer copper");
    let mut class = NetClass {
        name: "PWR".into(),
        nets: vec!["VIN".into()],
        track_width: 1.0,
        clearance: 0.2,
        via_diameter: 0.6,
        via_drill: 0.3,
        diff_pair: None,
        current: None,
        temp_rise: None,
        copper_oz: None,
    };
    assert!(current::class_widths(&class, &four).is_none(), "no current, nothing asked");
    class.current = Some(1.0);
    let w = current::class_widths(&class, &four).unwrap();
    assert_eq!(w.rise, current::DEFAULT_RISE);
    assert_eq!((w.outer, w.inner), (current::width_for(1.0, 10.0, 0.035, true).unwrap(), current::width_for(1.0, 10.0, 0.0175, false).unwrap()));
    class.copper_oz = Some(2.0);
    class.temp_rise = Some(20.0);
    let w = current::class_widths(&class, &four).unwrap();
    assert_eq!(w.inner, current::width_for(1.0, 20.0, current::oz_to_mm(2.0), false).unwrap(), "the class's own copper");
}

// The impedance calculator, against IPC-2141's closed forms and Wadell's coplanar line worked out
// by hand.

fn medium(outer: bool, h1: f64, h2: f64, t: f64) -> Medium {
    Medium { outer, h1, h2, t, er: 4.3 }
}

#[test]
fn line_impedances_follow_the_published_formulas() {
    // Microstrip, 0.35 mm over 0.2 mm of εr 4.3, 35 µm copper: 87/√5.71 · ln(1.196/0.315).
    let ms = medium(true, 0.2, 0.0, 0.035);
    let z = impedance::impedance(&ms, LineKind::Microstrip, 0.35, None).unwrap();
    assert!(close(z.z0, 48.5748, 1e-4) && close(z.er_eff.unwrap(), 3.238_64, 1e-4) && z.valid_range, "{z:?}");
    // A centred stripline between planes 0.2 mm above and below; off centre, the harmonic mean.
    let sl = medium(false, 0.2, 0.2, 0.0175);
    assert!(close(impedance::impedance(&sl, LineKind::Stripline, 0.15, None).unwrap().z0, 50.7137, 1e-4));
    let off = impedance::impedance(&medium(false, 0.2, 0.4, 0.0175), LineKind::Stripline, 0.15, None).unwrap().z0;
    assert!(close(off, 58.8714, 1e-4), "{off}");
    assert_eq!(impedance::impedance(&medium(false, 0.4, 0.2, 0.0175), LineKind::Stripline, 0.15, None).unwrap().z0, off, "either way up");
    // An edge-coupled pair on the outer layer, and a coplanar line with ground beside it.
    let diff = impedance::impedance(&medium(true, 0.2, 0.0, 0.035), LineKind::Differential, 0.2, Some(0.2)).unwrap();
    assert!(close(diff.z0, 107.797, 1e-4), "{diff:?}");
    let cpw = impedance::impedance(&ms, LineKind::Coplanar, 0.3, Some(0.15)).unwrap();
    assert!(close(cpw.z0, 54.1964, 1e-4) && close(cpw.er_eff.unwrap(), 3.012_65, 1e-4) && cpw.valid_range, "{cpw:?}");
    let narrow = impedance::impedance(&ms, LineKind::Coplanar, 0.3, Some(0.05)).unwrap();
    assert_eq!(narrow.out_of_range.map(|o| o.input), Some("t/s"), "copper 0.7 of the gap");
    let wide = impedance::impedance(&ms, LineKind::Microstrip, 1.0, None).unwrap();
    assert_eq!(wide.out_of_range.map(|o| o.input), Some("w/h"), "w/h 5 is past the formula's 2");
    // The width search finds the width back.
    for (m, kind, gap) in [
        (ms, LineKind::Microstrip, None),
        (sl, LineKind::Stripline, None),
        (ms, LineKind::Coplanar, Some(0.15)),
        (ms, LineKind::Differential, Some(0.2)),
    ] {
        let (w, z) = impedance::width_for(&m, kind, 50.0, gap).unwrap();
        assert!(close(z.z0, 50.0, 1e-5), "{kind:?}: {w} mm gives {}", z.z0);
        assert!(close(impedance::impedance(&m, kind, w, gap).unwrap().z0, 50.0, 1e-5));
    }
}

#[test]
fn the_impedance_calculator_says_why_not() {
    let ms = medium(true, 0.2, 0.0, 0.035);
    let sl = medium(false, 0.2, 0.2, 0.0175);
    assert_eq!(impedance::impedance(&sl, LineKind::Microstrip, 0.2, None), Err(ImpedanceError::WrongLayer(LineKind::Microstrip)));
    assert_eq!(impedance::impedance(&sl, LineKind::Coplanar, 0.2, Some(0.2)), Err(ImpedanceError::WrongLayer(LineKind::Coplanar)));
    assert_eq!(impedance::impedance(&ms, LineKind::Stripline, 0.2, None), Err(ImpedanceError::WrongLayer(LineKind::Stripline)));
    for gap in [None, Some(0.0), Some(f64::NAN), Some(-1.0), Some(21.0)] {
        assert_eq!(impedance::impedance(&ms, LineKind::Coplanar, 0.2, gap), Err(ImpedanceError::Gap), "{gap:?}");
        assert_eq!(impedance::width_for(&ms, LineKind::Differential, 90.0, gap), Err(ImpedanceError::Gap), "{gap:?}");
    }
    for w in [0.0, -1.0, f64::NAN, 25.0] {
        assert_eq!(impedance::impedance(&ms, LineKind::Microstrip, w, None), Err(ImpedanceError::Width), "{w}");
    }
    assert_eq!(impedance::impedance(&ms, LineKind::Microstrip, 15.0, None), Err(ImpedanceError::TooWide), "ln of less than 1");
    for t in [0.0, 600.0, f64::NAN] {
        assert_eq!(impedance::width_for(&ms, LineKind::Microstrip, t, None), Err(ImpedanceError::Target), "{t}");
    }
    let Err(ImpedanceError::OutOfReach { low, high, .. }) = impedance::width_for(&ms, LineKind::Microstrip, 400.0, None) else { panic!("400 Ω") };
    assert!(low < 400.0 && high < low, "{low} {high}");
    assert_eq!(impedance::impedance(&medium(true, 0.0, 0.0, 0.035), LineKind::Microstrip, 0.2, None), Err(ImpedanceError::NoPlane));
    // From a stack-up: the default 2-layer board's top over its 1.53 mm core, a 4-layer board's
    // inner1 between two dielectrics, and a layer the stack doesn't have.
    let two = default_stackup(2, 1.6);
    let top = impedance::medium(&two, CopperLayer::Top, 2).unwrap();
    assert!(top.outer && close(top.h1, 1.53, 1e-9) && top.t == 0.035 && top.er == 4.3, "{top:?}");
    let inner = impedance::medium(&default_stackup(4, 1.6), CopperLayer::Inner(1), 4).unwrap();
    assert!(!inner.outer && close(inner.h1, (1.6 - 0.105) / 3.0, 1e-9) && close(inner.h2, inner.h1, 1e-12), "{inner:?}");
    assert_eq!(impedance::medium(&two, CopperLayer::Inner(1), 2), Err(ImpedanceError::NoLayer(CopperLayer::Inner(1))));
    for e in [ImpedanceError::Gap, ImpedanceError::Width, ImpedanceError::NoLayer(CopperLayer::Top)] {
        assert!(e.key().starts_with("electronics.impedance_err_") && !e.to_string().is_empty() && !e.args().is_empty(), "{e:?}");
    }
}

// Overbars in names.

#[test]
fn overbar_markup_parses_as_worked_out_by_hand() {
    let bars = |s: &str| {
        let b = overbar::parse(s);
        // The bars as (start, end) characters.
        (b.text, b.bars.iter().map(|r| (r.start, r.end)).collect::<Vec<_>>())
    };
    assert_eq!(bars("~{CE}"), ("CE".into(), vec![(0, 2)]));
    assert_eq!(bars("~{WR}/RD"), ("WR/RD".into(), vec![(0, 2)]));
    assert_eq!(bars("~{CS}/~{OE}"), ("CS/OE".into(), vec![(0, 2), (3, 5)]));
    assert_eq!(bars("~{RESET"), ("RESET".into(), vec![(0, 5)]), "an open bar runs to the end");
    assert_eq!(bars("A~B"), ("A~B".into(), vec![]), "a ~ not before {{ is a character");
    assert_eq!(bars("~{}X"), ("X".into(), vec![]), "an empty bar is no bar");
    assert_eq!(bars("~{A{B}C"), ("A{BC".into(), vec![(0, 3)]), "a {{ inside a bar is a character");
    assert_eq!(bars("~{A~{B}C"), ("A~{BC".into(), vec![(0, 4)]), "and so is ~{{");
    assert_eq!(bars("x}~{Ü}"), ("x}Ü".into(), vec![(2, 3)]), "bars count characters, and a lone }} is one");
    assert!(overbar::has_markup("~{CE}") && !overbar::has_markup("~CE") && !overbar::has_markup("CE}"));
    assert_eq!(overbar::plain("~{CS}/~{OE}"), "CS/OE");
    assert!(matches!(overbar::plain("VDD"), std::borrow::Cow::Borrowed("VDD")), "no markup, no copy");
    assert_eq!(overbar::marked("~{CS}/~{OE}"), "CS¯/OE¯");
    assert_eq!(overbar::marked("~{RESET"), "RESET¯");
}

#[test]
fn overbar_names_cut_short_keep_their_markup() {
    let b = overbar::parse("~{CS}/~{OE}");
    assert_eq!(overbar::cut(&b, 5), "~{CS}/~{OE}");
    assert_eq!(overbar::cut(&b, 4), "~{CS}/~{O}", "a bar cut short ends where the text does");
    assert_eq!(overbar::cut(&b, 1), "~{C}");
    assert_eq!(overbar::cut(&b, 3), "~{CS}/");
    assert_eq!(overbar::cut(&b, 0), "");
    // Cutting a name at its full length and parsing it again gives it back, whatever it holds.
    for name in ["~{CE}", "~{A~{B}C", "~~{A}", "~{A~}{B", "}{~", "~{", "~{x", "plain", "~{Ω}µ~{", ""] {
        let b = overbar::parse(name);
        let n = b.text.chars().count();
        assert_eq!(overbar::parse(&overbar::cut(&b, n)), b, "{name}");
        assert_eq!(overbar::plain(name), b.text, "{name}");
        assert!(overbar::marked(name).chars().count() == n + b.bars.len(), "{name}");
    }
}

// The design rule check, on the `return` scratch board: R1 and R2 (0603) at x ±10 with SIG
// between R1.2 (-9.2, 0) and R2.1 (9.2, 0), the headers J1 (OTHER, GND) and J2 (VCC, GND) and C1;
// a 30 by 20 mm board on the standard profile.

/// The design rule check's findings on `e` (each code and severity), the airwires left out.
fn drc_of(e: &Electronics) -> Vec<(&'static str, Severity)> {
    let r = e.drc();
    assert!(!r.truncated);
    r.findings.iter().filter(|f| f.code != "unrouted").map(|f| (f.code, f.severity)).collect()
}

/// `commands` on the `return` board, and what the design rule check finds.
fn drc_case(commands: Value) -> Vec<(&'static str, Severity)> {
    let mut e = scratch("return");
    run(&mut e, commands);
    drc_of(&e)
}

fn has(found: &[(&'static str, Severity)], code: &str, severity: Severity) -> bool {
    found.contains(&(code, severity))
}

/// SIG routed straight from R1.2 to R2.1.
fn sig(width: f64) -> Value {
    json!(["track", {"layer": "top", "points": [[-9.2, 0], [9.2, 0]], "width": width}])
}

#[test]
fn the_design_rule_check_finds_each_broken_rule() {
    let e = scratch("return");
    assert_eq!(drc_of(&e), [], "a placed, unrouted board breaks nothing but its airwires");
    assert_eq!(e.drc().findings.iter().filter(|f| f.code == "unrouted").count(), 6);
    assert_eq!(drc_case(json!([sig(0.2)])), []);
    // R1's two pads joined: GND and SIG short.
    assert!(has(&drc_case(json!([["track", {"layer": "top", "points": [[-10.8, 0], [-9.2, 0]], "width": 0.2}]])), "short", Severity::Error));
    // A GND track from J1.2 down beside SIG: 0.15 mm off it, then 0.3 mm.
    let beside = |y: f64| json!([sig(0.2), ["track", {"layer": "top", "points": [[3, 4.73], [3, y], [-8, y]], "width": 0.2}]]);
    assert!(has(&drc_case(beside(0.35)), "clearance", Severity::Error));
    assert!(!drc_case(beside(0.5)).iter().any(|f| f.0 == "clearance"));
    assert!(has(&drc_case(json!([sig(0.1)])), "track_width", Severity::Error), "0.1 mm against the least 0.15");
    // Vias of GND below the track: a thin ring, a small drill, two holes too close, one at the edge.
    let via = |at: [f64; 2], d: f64, h: f64| json!(["via", {"at": at, "net": "GND", "diameter": d, "drill": h}]);
    assert!(has(&drc_case(json!([via([0.0, -5.0], 0.4, 0.3)])), "annular_ring", Severity::Error));
    assert!(has(&drc_case(json!([via([0.0, -5.0], 0.6, 0.2)])), "drill", Severity::Error));
    assert!(has(&drc_case(json!([via([0.0, -5.0], 0.6, 0.3), via([0.5, -5.0], 0.6, 0.3)])), "hole_to_hole", Severity::Error));
    assert!(has(&drc_case(json!([via([14.8, 0.0], 0.6, 0.3)])), "copper_to_edge", Severity::Error));
    // What isn't wrong but unfinished: a via joining nothing, a track to nowhere (the infos list
    // the via's drill and ring, right at the profile's least).
    let lone = drc_case(json!([via([0.0, -5.0], 0.6, 0.3)]));
    assert_eq!(lone.iter().filter(|f| f.1 != Severity::Info).collect::<Vec<_>>(), [&("via_stub", Severity::Warning)], "{lone:?}");
    assert!(lone.iter().filter(|f| f.1 == Severity::Info).all(|f| f.0 == "fab_class"), "{lone:?}");
    let open = drc_case(json!([["track", {"layer": "top", "points": [[-9.2, 0], [0, 0]], "width": 0.2}]]));
    assert_eq!(open, [("dangling_track", Severity::Warning)]);
    // A keepout that forbids vias, with one in it.
    let mut e = scratch("return");
    let id = e.board.take_id().unwrap();
    e.board.keepouts.push(serde_json::from_value(json!({"id": id, "outline": [[-2, -7], [2, -7], [2, -3], [-2, -3]]})).unwrap());
    run(&mut e, json!([via([0.0, -5.0], 0.6, 0.3)]));
    assert!(has(&drc_of(&e), "keepout", Severity::Error));
    // A pour of a net the schematic doesn't have, and one off the board that fills nothing.
    let pour = |net: &str, center: [f64; 2]| json!(["pour", {"layer": "bottom", "net": net, "rect": [4, 4], "center": center}]);
    assert!(drc_case(json!([pour("NOPE", [0.0, -5.0])])).contains(&("pour_unknown_net", Severity::Warning)));
    assert!(drc_case(json!([pour("GND", [40.0, 0.0])])).contains(&("pour_empty", Severity::Warning)));
}

#[test]
fn the_design_rule_check_reads_the_fab_profile_and_the_boards_levels() {
    let mut e = scratch("return");
    run(&mut e, json!([sig(0.2)]));
    let with = e.drc();
    assert_eq!(with.profile.map(|p| p.id), Some("standard"));
    assert!(with.not_applicable.is_empty(), "{:?}", with.not_applicable);
    // No profile: rings on the finished hole, and the profile's own checks left out, saying so.
    let mut none = e.clone();
    none.board.fab_profile = None;
    let r = none.drc();
    assert!(r.profile.is_none() && r.findings.iter().any(|f| f.code == "profile_missing"));
    let left: Vec<&str> = r.not_applicable.iter().map(|(code, needs)| if *needs == "board.fab_profile" { *code } else { "" }).collect();
    assert!(left.contains(&"via_aspect_ratio") && left.contains(&"drill_to_copper"), "{:?}", r.not_applicable);
    // Rules finer than the profile builds.
    let mut fine = e.clone();
    fine.board.rules.min_track_width = 0.1;
    assert!(fine.drc().findings.iter().any(|f| f.code == "fab_class" && f.severity == Severity::Warning));
    // The board's levels: the airwires left out (and listed), a warning made an error.
    let mut levels = e.clone();
    levels.board.drc_levels.insert("unrouted".into(), super::drc::CheckLevel::Ignore);
    let r = levels.drc();
    assert!(r.findings.iter().all(|f| f.code != "unrouted") && r.not_applicable.contains(&("unrouted", "board.drc_levels")));
    let mut open = scratch("return");
    run(&mut open, json!([["track", {"layer": "top", "points": [[-9.2, 0], [0, 0]], "width": 0.2}]]));
    open.board.drc_levels.insert("dangling_track".into(), super::drc::CheckLevel::Error);
    assert!(has(&drc_of(&open), "dangling_track", Severity::Error));
}

#[test]
fn design_rule_approvals_hold_their_finding_and_go_stale_without_it() {
    let mut e = scratch("return");
    run(&mut e, json!([["track", {"layer": "top", "points": [[-10.8, 0], [-9.2, 0]], "width": 0.2}]]));
    let first = e.drc();
    assert_eq!(e.drc(), first, "the same board, the same findings");
    let short = first.findings.iter().find(|f| f.code == "short").unwrap();
    assert!(short.key.starts_with("short|") && !short.message.is_empty(), "{short:?}");
    for (by, approved, pending) in [(Some(Actor::User), 1, 0), (Some(Actor::Agent), 0, 1), (None, 0, 1)] {
        let mut e = e.clone();
        e.board.approvals = vec![Approval { key: short.key.clone(), note: "a net tie".into(), by }];
        let r = e.drc();
        assert_eq!((r.approved, r.pending_user), (approved, pending), "by {by:?}: a short is critical, only the user's approval holds it");
        assert_eq!(r.errors, first.errors - approved);
    }
    e.board.approvals = vec![Approval { key: "short|t999".into(), note: String::new(), by: Some(Actor::User) }];
    assert_eq!(e.drc().stale, ["short|t999"]);
}

// Manufacturability, on the `return` board: R1.2 is a 0.6 by 0.9 mm SIG pad at (-9.2, 0), the
// standard profile keeps 0.25 mm from an open via to a pad and 30° off square, and no pocket under
// 90°.

/// The findings of `code` on the `return` board after `commands`, by severity.
fn dfm_case(commands: Value, code: &str) -> Vec<Severity> {
    drc_case(commands).into_iter().filter(|f| f.0 == code).map(|f| f.1).collect()
}

#[test]
fn a_via_too_near_a_pad_is_found_and_a_tented_one_is_an_info() {
    // A 0.6 mm SIG via 0.45 mm below R1.2's lower edge (y -0.45), then 0.15 mm.
    let via = |y: f64| json!(["via", {"at": [-9.2, y], "net": "SIG", "diameter": 0.6, "drill": 0.3}]);
    assert_eq!(dfm_case(json!([via(-1.2)]), "via_near_pad"), []);
    assert_eq!(dfm_case(json!([via(-0.9)]), "via_near_pad"), [Severity::Warning]);
    let mut e = scratch("return");
    run(&mut e, json!([via(-0.9)]));
    let f = e.drc().findings.into_iter().find(|f| f.code == "via_near_pad").unwrap();
    assert!(close(f.measured, 0.15, 1e-9) && close(f.limit.value, 0.25, 1e-12) && f.margin < 0.0, "{f:?}");
    assert!(f.note.is_some() && f.fix.iter().any(|x| x.command == "ecad.board.via.edit"), "{f:?}");
    e.board.vias.last_mut().unwrap().protection = ViaProtection::Tented;
    let tented: Vec<Severity> = e.drc().findings.iter().filter(|f| f.code == "via_near_pad").map(|f| f.severity).collect();
    assert_eq!(tented, [Severity::Info], "solder can't reach a tented via's hole");
}

#[test]
fn a_track_leaving_a_pad_askew_or_by_its_corner_is_a_pad_entry() {
    let track = |points: Value| json!([["track", {"layer": "top", "points": points, "width": 0.2}]]);
    // Square out of the middle of the right edge.
    assert_eq!(dfm_case(track(json!([[-9.2, 0], [-5, 0]])), "pad_entry"), []);
    // At 45°: off square by more than 30°.
    let askew = drc_case(track(json!([[-9.2, 0], [-8.2, 1]])));
    assert!(has(&askew, "pad_entry", Severity::Warning) && !askew.iter().any(|f| f.0 == "acute_angle"), "one finding for both: {askew:?}");
    // Square, but 0.35 mm off the middle of an edge 0.45 mm to its corner (78 %).
    assert_eq!(dfm_case(track(json!([[-9.2, 0.35], [-5, 0.35]])), "pad_entry"), [Severity::Warning]);
}

#[test]
fn pockets_under_90_degrees_are_acute_angles() {
    let track = |points: Value| json!(["track", {"layer": "top", "points": points, "width": 0.2}]);
    // Bends: 90° and 135° pass, 26.6° doesn't.
    assert_eq!(dfm_case(json!([track(json!([[-9.2, 0], [-5, 0], [-5, -3]]))]), "acute_angle"), []);
    assert_eq!(dfm_case(json!([track(json!([[-9.2, 0], [-5, 0], [-3, -2]]))]), "acute_angle"), []);
    assert_eq!(dfm_case(json!([track(json!([[-9.2, 0], [-5, 0], [-7, -1]]))]), "acute_angle"), [Severity::Warning]);
    // A T into SIG: square passes, at 45° it leaves a 45° pocket.
    let tee = |end: [f64; 2]| json!([track(json!([[-9.2, 0], [9.2, 0]])), track(json!([[0, 0], end]))]);
    assert_eq!(dfm_case(tee([0.0, -2.0]), "acute_angle"), []);
    assert_eq!(dfm_case(tee([2.0, -2.0]), "acute_angle"), [Severity::Warning]);
    // Leaving R1.2 20° off square: a 70° pocket with the pad's edge (not yet a pad entry).
    let (s, c) = 20f64.to_radians().sin_cos();
    let found = drc_case(json!([track(json!([[-9.2, 0], [-9.2 + 3.0 * c, 3.0 * s]]))]));
    assert!(has(&found, "acute_angle", Severity::Warning) && !found.iter().any(|f| f.0 == "pad_entry"), "{found:?}");
}

#[test]
fn slivers_of_copper_and_gaps_in_a_fill_are_found() {
    // Two GND blocks on the bottom joined by a neck, and one block with a slot cut into it.
    let necked = |w: f64| {
        let h = w / 2.0;
        json!([["pour", {"layer": "bottom", "net": "GND", "remove_islands": false, "points": [
            [-6, -9], [-1, -9], [-1, -6.0 - h], [1, -6.0 - h], [1, -9], [6, -9], [6, -3], [1, -3], [1, -6.0 + h], [-1, -6.0 + h], [-1, -3], [-6, -3]
        ]}]])
    };
    let slotted = |w: f64| {
        let h = w / 2.0;
        json!([["pour", {"layer": "bottom", "net": "GND", "remove_islands": false, "points": [
            [-6, -9], [6, -9], [6, -3], [h, -3], [h, -6], [-h, -6], [-h, -3], [-6, -3]
        ]}]])
    };
    let thin = |commands: Value| {
        let mut e = scratch("return");
        run(&mut e, commands);
        e.board.pours.last_mut().unwrap().min_width = 0.05;
        e.drc().findings.into_iter().filter(|f| f.code == "sliver").collect::<Vec<_>>()
    };
    assert!(thin(necked(0.5)).is_empty());
    let neck = thin(necked(0.1));
    assert_eq!(neck.len(), 1, "{neck:?}");
    assert!(neck[0].severity == Severity::Warning && neck[0].measured < 0.15 && neck[0].limit.value == 0.15, "{:?}", neck[0]);
    assert!(neck[0].fix.iter().any(|x| x.command == "ecad.board.pour.edit" && x.params["min_width"] == 0.15));
    assert!(thin(slotted(0.5)).is_empty());
    let slot = thin(slotted(0.1));
    assert!(slot.len() == 1 && slot[0].args.contains(&("gaps", "1".to_string())), "{slot:?}");
    // At the default least width the fill itself drops the neck.
    assert_eq!(dfm_case(necked(0.1), "sliver"), []);
}

#[test]
fn a_move_that_brings_copper_too_close_names_the_clash() {
    let mut e = scratch("return");
    run(&mut e, json!([sig(0.2)]));
    let before = e.board_nets();
    let j1 = footprint_of(&e, "J1");
    let moved = BTreeSet::from([j1]);
    let clashes = |dy: f64| {
        let mut board = e.board.clone();
        e.move_on_board(&mut board, &[(moved.clone(), Motion { by: [0.0, dy], ..Motion::default() })]);
        let after = super::airwires::board_nets(&e, &board, &e.connectivity());
        super::drc::moved_clashes(&e.board, &before, &board, &after, &moved)
    };
    // J1.2 (GND, 1.7 mm across) at (3, 4.73) moved 3.6 mm down: 0.18 mm from SIG's edge.
    let (found, cut) = clashes(-3.6);
    assert!(!cut);
    let first = found.first().unwrap();
    assert!(close(first.clash.gap, 0.18, 1e-6) && first.clash.required == 0.2, "{first:?}");
    assert_eq!(first.moved.reference(), "J1.2");
    assert!(clashes(-3.0).0.is_empty(), "0.78 mm off");
    assert!(clashes(0.0).0.is_empty());
}

// Stitching vias, on the `return` board with ground planes on inner1 and inner2.

fn stitched_board() -> Electronics {
    let mut e = scratch("return");
    let gnd = |layer: &str| json!(["pour", {"layer": layer, "net": "GND", "rect": [30, 20], "remove_islands": false}]);
    run(&mut e, json!([gnd("inner1"), gnd("inner2"), sig(0.2)]));
    e
}

fn ask(net: &str, pattern: Pattern) -> Ask {
    Ask {
        net: net.into(),
        pattern,
        area: None,
        tracks: Vec::new(),
        nets: Vec::new(),
        pitch: None,
        offset: None,
        via: ViaSpec { diameter: 0.6, drill: 0.3, from: CopperLayer::Top, to: CopperLayer::Bottom },
        clearance: None,
        under_parts: false,
        target: None,
        budget: None,
    }
}

/// `e` with stitching vias at `spots`.
fn with_vias(e: &Electronics, net: &str, spots: &[[f64; 2]]) -> Electronics {
    let mut e = e.clone();
    for at in spots {
        let id = e.board.take_id().unwrap();
        let via = new_via(id, *at, (0.6, 0.3), (CopperLayer::Top, CopperLayer::Bottom), Some(net.into()));
        e.board.vias.push(Via { role: ViaRole::Stitch, pattern: Some(Pattern::Grid), ..via });
    }
    e
}

#[test]
fn a_stitching_grid_is_staggered_inside_the_board_and_breaks_no_rule() {
    let e = stitched_board();
    let nets = e.board_nets();
    let mut a = ask("GND", Pattern::Grid);
    a.pitch = Some(5.0);
    let plan = stitch::plan(&e, &nets, &a).unwrap();
    assert_eq!(stitch::plan(&e, &nets, &a).unwrap(), plan, "the same board, the same vias");
    // 28.7 by 18.7 mm inside the edge gap: rows 4.33 mm apart, 6 and 5 spots in turn, 5 rows.
    assert_eq!(plan.looked_at, 28);
    assert_eq!(plan.looked_at, plan.spots.len() + plan.skipped.values().sum::<usize>(), "{:?}", plan.skipped);
    assert!(plan.spots.len() >= 20, "{} spots, skipped {:?}", plan.spots.len(), plan.skipped);
    assert_eq!(plan.pitch, Some(5.0));
    for p in &plan.spots {
        assert!(p[0].abs() <= 15.0 - 0.6 && p[1].abs() <= 10.0 - 0.6, "{p:?} is past the edge gap");
    }
    for (i, p) in plan.spots.iter().enumerate() {
        for q in plan.spots.iter().skip(i + 1) {
            assert!((p[0] - q[0]).hypot(p[1] - q[1]) >= 5.0 * (1.0 - 2.0 * 0.16) - 1e-9, "{p:?} and {q:?}");
        }
    }
    // Kept off the parts' courtyards unless asked, and every via placed passes the check.
    let under = stitch::under_parts(&e, &e.board.footprints.iter().map(|f| f.id).collect(), &[]);
    assert!(under.is_empty());
    let after = with_vias(&e, "GND", &plan.spots);
    let r = after.drc();
    let mine: BTreeSet<String> = after.board.vias.iter().map(|v| format!("v{}", v.id)).collect();
    let bad: Vec<&str> =
        r.findings.iter().filter(|f| f.severity == Severity::Error && f.key.split('|').any(|k| mine.contains(k))).map(|f| f.code).collect();
    assert!(bad.is_empty(), "{bad:?}");
    assert!(r.findings.iter().all(|f| f.code != "via_stub"), "each joins the two planes");
    let vias: Vec<&Via> = after.board.vias.iter().collect();
    assert!(stitch::under_parts(&after, &after.board.footprints.iter().map(|f| f.id).collect(), &vias).iter().all(|u| !u));
    // pi.stitching measures how weakly the planes are tied (the ground connectivity solve), and a
    // 5 mm grid ties them within 0.5 mV; the λg/10 via-free circle is only its fallback now.
    assert_eq!(fails(&quality_of(&after, &["pi.stitching"]), "pi.stitching"), []);
    // The pitch the frequency asks for, under the parts too, stitches the planes.
    let a = Ask { under_parts: true, ..ask("GND", Pattern::Grid) };
    let plan = stitch::plan(&e, &nets, &a).unwrap();
    assert!(plan.pitch.is_some_and(|p| p < 5.0), "{:?}", plan.pitch);
    let after = with_vias(&e, "GND", &plan.spots);
    assert_eq!(quality_of(&after, &["pi.stitching"]), [], "the planes are stitched");
    assert!(!after.drc().findings.iter().any(|f| f.severity == Severity::Error), "{:?}", after.drc().findings);
}

// Silkscreen names mode, on the `silk` board packed tight: its eight chips and diodes in two rows
// 2 mm apart, J1 and U1 where they are.

fn dense() -> Electronics {
    let mut e = scratch("silk");
    let mut moves = Vec::new();
    for (k, d) in ["C1", "C2", "C3", "C4", "R1", "R2", "D1", "D2"].iter().enumerate() {
        let (x, y) = (-6.0 + 2.0 * (k % 4) as f64, if k < 4 { -6.0 } else { -4.0 });
        moves.push(json!(["move", {"items": [d], "to": [x, y]}]));
    }
    run(&mut e, Value::Array(moves));
    e
}

/// The silk findings on `e` about names.
fn name_findings(e: &Electronics) -> Vec<String> {
    let only = BTreeSet::from(["silk.size", "silk.clip", "silk.designator"]);
    quality::check(e, &e.board_nets(), Some(&only)).findings.into_iter().map(|q| q.finding.message).filter(|m| m.contains("'s name")).collect()
}

#[test]
fn the_fit_mode_leaves_no_silk_finding_on_a_dense_board_and_the_others_do_as_they_say() {
    use super::labels::{NamesMode, layout, least_height};
    let e = dense();
    assert_eq!(least_height(&e.board), 1.5, "the standard profile's 0.15 mm stroke needs 1.5 mm");
    let apply = |l: &super::labels::Layout| {
        let mut x = e.clone();
        x.board = l.board.clone();
        x.validate().unwrap();
        x
    };
    // Fit: what fits is placed, the rest is hidden, and no name finding is left.
    let fit = layout(&e, NamesMode::Fit, None).unwrap();
    assert!(!fit.placed.is_empty() && fit.unplaceable.is_empty(), "{:?}", (&fit.placed, &fit.moved_to_assembly));
    assert_eq!(fit.placed.len() + fit.moved_to_assembly.len(), 10);
    assert!(!fit.moved_to_assembly.is_empty(), "the board is dense: some names go to the assembly drawing");
    let laid = apply(&fit);
    assert_eq!(name_findings(&laid), Vec::<String>::new());
    assert_eq!(laid.board.name_height, Some(1.5));
    assert!(laid.board.footprints.iter().all(|f| f.name_placed));
    for f in &laid.board.footprints {
        let d = &laid.schematic.parts.iter().find(|p| p.id == f.part).unwrap().designator;
        assert_eq!(f.show_name, fit.placed.contains(d), "{d}");
    }
    // Full: every name shows; those with no legal spot are counted.
    let full = layout(&e, NamesMode::Full, Some(2.0)).unwrap();
    assert!(full.moved_to_assembly.is_empty() && full.height == 2.0);
    assert!(apply(&full).board.footprints.iter().all(|f| f.show_name));
    assert_eq!(full.placed.len() + full.unplaceable.len(), 10);
    // Minimal: the connector and the LED only; none: nothing.
    let minimal = layout(&e, NamesMode::Minimal, None).unwrap();
    assert!(minimal.placed.iter().all(|d| d == "J1" || d == "D1"), "{:?}", minimal.placed);
    assert!(minimal.moved_to_assembly.contains(&"R1".to_string()));
    let none = layout(&e, NamesMode::None, None).unwrap();
    assert!(none.placed.is_empty() && none.moved_to_assembly.len() == 10);
    // A height under the fab's least is raised to it, and says so; past the most, refused.
    let low = layout(&e, NamesMode::Fit, Some(0.5)).unwrap();
    assert!(low.height == 1.5 && low.why.iter().any(|w| w.contains("1.5 mm")), "{:?}", low.why);
    assert!(layout(&e, NamesMode::Fit, Some(50.0)).is_err() && layout(&e, NamesMode::Fit, Some(f64::NAN)).is_err());
    // A name set by hand is never moved or hidden, not even by none.
    let mut hand = e.clone();
    let u1 = footprint_of(&hand, "U1");
    let f = hand.board.footprints.iter_mut().find(|f| f.id == u1).unwrap();
    f.name_at = Some([0.0, 6.0]);
    for mode in [NamesMode::Fit, NamesMode::None] {
        let l = layout(&hand, mode, None).unwrap();
        let g = l.board.footprints.iter().find(|f| f.id == u1).unwrap();
        assert!(g.name_at == Some([0.0, 6.0]) && g.show_name && !g.name_placed && l.kept == ["U1"], "{mode:?}");
    }
    // Laying out again changes nothing.
    let again = layout(&laid, NamesMode::Fit, None).unwrap();
    assert!(again.changed.is_empty(), "{:?}", again.changed);
}

// Board markings, on the `return` board (four layers, a GND plane on the bottom).

#[test]
fn a_marking_stamps_its_lines_qr_and_layer_marks_and_follows_the_revision() {
    use super::marking::{self, Fields, Marking, MarkingTemplate, qr_cells};
    let mut e = scratch("return");
    run(&mut e, json!([["pour", {"layer": "bottom", "net": "GND", "rect": [30, 20], "remove_islands": false}]]));
    let template = MarkingTemplate {
        part_number: "MM-{project}-{number}".into(),
        serial_box: true,
        qr: true,
        made_by: "Workshop".into(),
        ..MarkingTemplate::default()
    };
    let fields = Fields { title: "Sensor".into(), revision: "B".into(), date: "2026-10-31".into(), ..Fields::default() };
    assert_eq!(marking::part_number(&template.part_number, &fields, "7"), "MM-Sensor-7");
    let id = e.board.take_id().unwrap();
    let m = Marking { id, side: Side::Top, at: [-14.0, 9.5], number: "7".into(), template, layer_marks: true };
    marking::stamp(&mut e.board, &m, &fields).unwrap();
    e.board.markings.push(m.clone());
    e.validate().unwrap();
    let texts: Vec<(String, String)> = e
        .board
        .graphics
        .iter()
        .filter(|g| g.marking == Some(id) && g.graphic.kind == GraphicType::Text)
        .map(|g| (g.layer.name(), g.graphic.text.clone()))
        .collect();
    for want in ["Sensor", "REV B  2026-10-31", "MM-Sensor-7", "Workshop", "S/N"] {
        assert!(texts.contains(&("silkscreen_top".into(), want.into())), "{want}: {texts:?}");
    }
    // A mark on each of the four copper layers, side by side, each in a keepout of pours only.
    let marks: Vec<&BoardGraphic> = e.board.graphics.iter().filter(|g| g.marking == Some(id) && matches!(g.layer, DrawLayer::Copper(_))).collect();
    assert_eq!(marks.iter().map(|g| g.graphic.text.as_str()).collect::<Vec<_>>(), ["1", "2", "3", "4"]);
    let keepouts: Vec<_> = e.board.keepouts.iter().filter(|k| k.marking == Some(id)).collect();
    assert_eq!(keepouts.len(), 4);
    assert!(keepouts.iter().all(|k| k.forbid.pours && !k.forbid.tracks && !k.forbid.vias && k.layers.len() == 1));
    let xs: Vec<f64> = keepouts.iter().map(|k| k.outline[0][0]).collect();
    assert!(xs.windows(2).all(|w| w[1] > w[0] + 1.0), "side by side: {xs:?}");
    // The QR code is filled squares; the bottom plane's fill keeps out of the bottom mark's area.
    assert!(qr_cells("MM-Sensor-7 B").is_some_and(|c| c.len() >= 21));
    assert!(e.board.graphics.iter().any(|g| g.marking == Some(id) && g.graphic.fill));
    let nets = e.board_nets();
    let fill = nets.fills.iter().find(|f| f.layer == CopperLayer::Bottom).unwrap();
    let bottom = keepouts.iter().find(|k| k.layers == [CopperLayer::Bottom]).unwrap();
    let mid = [(bottom.outline[0][0] + bottom.outline[2][0]) / 2.0, (bottom.outline[0][1] + bottom.outline[2][1]) / 2.0];
    assert_eq!(fill.region.locate(&[mid]), [None], "no pour copper under the bottom mark");
    // The design rule check finds nothing wrong with the marking's copper.
    let mine: BTreeSet<String> = marks.iter().map(|g| format!("g{}", g.id)).collect();
    let bad: Vec<String> = e
        .drc()
        .findings
        .into_iter()
        .filter(|f| f.severity == Severity::Error && f.key.split('|').any(|k| mine.contains(k)))
        .map(|f| f.message)
        .collect();
    assert!(bad.is_empty(), "{bad:?}");
    // A new revision makes the block again.
    marking::refresh(&mut e, &Fields { revision: "C".into(), ..fields.clone() }).unwrap();
    assert!(e.board.graphics.iter().any(|g| g.marking == Some(id) && g.graphic.text == "REV C  2026-10-31"));
    assert!(!e.board.graphics.iter().any(|g| g.graphic.text == "REV B  2026-10-31"));
    assert_eq!(e.board.keepouts.iter().filter(|k| k.marking == Some(id)).count(), 4);
    // Deleted, it stays gone.
    e.board.graphics.retain(|g| g.marking != Some(id));
    marking::refresh(&mut e, &fields).unwrap();
    assert!(e.board.markings.is_empty() && !e.board.keepouts.iter().any(|k| k.marking == Some(id)));
    // Hostile templates are refused.
    for bad in
        [MarkingTemplate { height: 0.0, ..MarkingTemplate::default() }, MarkingTemplate { made_by: "x".repeat(65), ..MarkingTemplate::default() }]
    {
        assert!(bad.validate().is_err());
    }
}

// Ground connectivity: a GND plane on inner1 (the largest, so the reference), a 16 by 2 mm GND
// strip on the bottom tied to it by one via at its left end.

/// The `return` board with that strip; `vias` more GND vias along the strip at these x.
fn strip(vias: &[f64]) -> Electronics {
    let mut e = scratch("return");
    let mut commands = vec![
        json!(["pour", {"layer": "inner1", "net": "GND", "rect": [30, 20], "remove_islands": false}]),
        json!(["pour", {"layer": "bottom", "net": "GND", "rect": [16, 2], "center": [0, -4], "remove_islands": false}]),
        json!(["via", {"at": [-7.5, -4], "net": "GND", "diameter": 0.6, "drill": 0.3}]),
    ];
    commands.extend(vias.iter().map(|x| json!(["via", {"at": [x, -4], "net": "GND", "diameter": 0.6, "drill": 0.3}])));
    run(&mut e, Value::Array(commands));
    e
}

#[test]
fn a_strip_tied_at_one_end_rises_as_worked_out_by_hand() {
    use super::ground::{Ask as GroundAsk, Model, Reference};
    let e = strip(&[]);
    let mut m = Model::build(&e, &e.board_nets(), &GroundAsk::default()).unwrap();
    assert_eq!(m.net_name, "GND", "the ground named as one");
    assert!(matches!(m.reference_is, Reference::Plane { layer: CopperLayer::Inner(1), .. }), "{:?}", m.reference_is);
    let map = m.solve();
    assert!(map.converged && map.cell == 0.25 && !map.coarse, "{} steps", map.iterations);
    // 32 mm² draw 0.32 A through the via: two hops of a 0.3 mm barrel, 25 µm thick, 1.6/3 mm long
    // (ρ 1.72e-5 Ω·mm): 0.778 mΩ, 0.249 mV. Along the strip, 1 oz (0.491 mΩ/sq) carrying
    // 10 mA/mm² from its far end 15.5 mm off: J·L²·R/2 = 0.590 mV. About 0.84 mV at the far end,
    // a little more for the current crowding into the via.
    let worst = map.plane_worst(&[CopperLayer::Bottom]).unwrap();
    assert!(worst.at[0] > 7.0 && (0.80..0.95).contains(&worst.mv), "{worst:?}");
    // The reference plane is at 0, and nothing of the planes is isolated.
    assert!(map.plane_worst(&[CopperLayer::Inner(1)]).is_some_and(|s| s.mv == 0.0));
    assert!(map.plane_isolated().is_empty());
    // A second via at the far end: the strip's middle is now the weakest, a quarter as far.
    let mut both = Model::build(&strip(&[7.5]), &strip(&[7.5]).board_nets(), &GroundAsk::default()).unwrap().solve();
    let mid = both.plane_worst(&[CopperLayer::Bottom]).unwrap();
    assert!(mid.mv < worst.mv / 2.0 && mid.at[0].abs() < 1.5, "{mid:?}");
    // No via at all: the strip is isolated.
    let mut lone = scratch("return");
    run(
        &mut lone,
        json!([
            ["pour", {"layer": "inner1", "net": "GND", "rect": [30, 20], "remove_islands": false}],
            ["pour", {"layer": "bottom", "net": "GND", "rect": [16, 2], "center": [0, -4], "remove_islands": false}],
        ]),
    );
    let alone = Model::build(&lone, &lone.board_nets(), &GroundAsk::default()).unwrap().solve();
    let iso = alone.plane_isolated();
    assert!(iso.len() == 1 && iso[0].0 == CopperLayer::Bottom && (28.0..36.0).contains(&iso[0].1), "{iso:?}");
    // Coverage and the coarse map say the same.
    let cover = map.coverage(&[CopperLayer::Bottom], 0.5);
    assert!(cover[0].tied == 1.0 && cover[0].weak > 0.0 && cover[0].isolated == 0.0, "{cover:?}");
    let (side, rows) = map.coarse_map(3, 32);
    assert!(side >= 0.25 && rows.len() <= 32 && rows.iter().all(|r| r.len() <= 32));
    both = map;
    assert!(both.unknowns > 0);
    // Hostile asks are refused.
    assert!(Model::build(&e, &e.board_nets(), &GroundAsk { net: Some("NOPE".into()), cell: None }).is_err());
    for cell in [0.0, 0.05, 0.3, f64::NAN, f64::INFINITY] {
        assert!(Model::build(&e, &e.board_nets(), &GroundAsk { net: None, cell: Some(cell) }).is_err(), "{cell}");
    }
}

#[test]
fn needed_stitching_ties_the_planes_where_they_are_weakest() {
    // Two GND planes tied only at the corners.
    let mut e = stitched_board();
    run(&mut e, Value::Array([[-13, -8], [-13, 8], [13, -8], [13, 8]].map(|at| json!(["via", {"at": at, "net": "GND"}])).to_vec()));
    let nets = e.board_nets();
    assert_eq!(fails(&quality_of(&e, &["pi.stitching"]), "pi.stitching"), [Severity::Warning], "tied weakly");
    let a = Ask { budget: Some(40), ..ask("GND", Pattern::Needed) };
    let plan = stitch::plan(&e, &nets, &a).unwrap();
    let g = plan.ground.clone().unwrap();
    assert!(g.before.converged && g.after.converged);
    let (before, after) = (g.before.worst.unwrap(), g.after.worst.unwrap());
    assert!(before > 0.5 && after < before, "{before} → {after}");
    assert!(!plan.spots.is_empty() && plan.spots.len() <= 40, "{:?}", plan.spots);
    assert!(g.reached && after <= 0.5, "{g:?}");
    // The vias break no rule, and the check passes.
    let placed = with_vias(&e, "GND", &plan.spots);
    let mine: BTreeSet<String> = placed.board.vias.iter().map(|v| format!("v{}", v.id)).collect();
    let r = placed.drc();
    assert!(!r.findings.iter().any(|f| f.severity == Severity::Error && f.key.split('|').any(|k| mine.contains(k))), "{:?}", r.findings);
    let q = quality::check(&placed, &placed.board_nets(), Some(&BTreeSet::from(["pi.stitching"])));
    assert!(q.findings.iter().all(|x| x.status != Status::Fail), "{:?}", q.findings.iter().map(|x| &x.finding.message).collect::<Vec<_>>());
    // A budget of one stops at one; a target already met places none.
    let one = stitch::plan(&e, &nets, &Ask { budget: Some(1), ..ask("GND", Pattern::Needed) }).unwrap();
    assert!(one.spots.len() == 1 && !one.ground.unwrap().reached);
    let easy = stitch::plan(&e, &nets, &Ask { target: Some(999.0), ..ask("GND", Pattern::Needed) }).unwrap();
    assert!(easy.spots.is_empty() && easy.ground.unwrap().reached);
    for bad in [Ask { budget: Some(0), ..ask("GND", Pattern::Needed) }, Ask { target: Some(-1.0), ..ask("GND", Pattern::Needed) }] {
        assert!(stitch::plan(&e, &nets, &bad).is_err());
    }
}

#[test]
fn stitching_says_why_not_and_stays_bounded() {
    let e = stitched_board();
    let nets = e.board_nets();
    let err = |a: &Ask| stitch::plan(&e, &nets, a).unwrap_err();
    assert!(err(&ask("NOPE", Pattern::Grid)).contains("no net `NOPE`"));
    for pitch in [0.5, 0.0, f64::NAN, f64::INFINITY, 101.0] {
        let mut a = ask("GND", Pattern::Grid);
        a.pitch = Some(pitch);
        assert!(err(&a).contains("pitch"), "{pitch}");
    }
    for offset in [0.0, -1.0, f64::NAN, 11.0] {
        let mut a = ask("GND", Pattern::Fence);
        a.offset = Some(offset);
        assert!(err(&a).contains("offset"), "{offset}");
    }
    let mut a = ask("GND", Pattern::Grid);
    a.clearance = Some(-0.1);
    assert!(err(&a).contains("clearance"));
    a.clearance = None;
    a.area = Some(stitch::Area::Polygon(vec![[0.0, 0.0], [1.0, 0.0]]));
    assert!(err(&a).contains("3 corners"));
    a.area = Some(stitch::Area::Pour(999));
    assert!(stitch::plan(&e, &nets, &a).is_err());
    a.tracks = vec![1; stitch::MAX_NAMED + 1];
    assert!(err(&a).contains("at most"));
    // A board 2 m across at a 1 mm pitch is millions of spots: refused, not tried.
    let mut big = e.clone();
    big.board.outline = vec![[-1000.0, -1000.0], [1000.0, -1000.0], [1000.0, 1000.0], [-1000.0, 1000.0]];
    let mut a = ask("GND", Pattern::Grid);
    a.pitch = Some(1.0);
    assert!(stitch::plan(&big, &big.board_nets(), &a).unwrap_err().contains("100000 spots"));
}

#[test]
fn stitching_pitches_come_from_the_frequency_of_concern() {
    let e = stitched_board();
    // 2.4 GHz in εr 4.3: λg/10 is 6.02 mm; staggered rows each via up to 0.16 pitches off keep
    // the via-free circles within it at 4.05 mm or less.
    let p = stitch::grid_pitch(&e, 0.6);
    let tenth = 299_792.458 / 2400.0 / 4.3_f64.sqrt() / 10.0;
    assert_eq!(p.mhz, Some(2400.0));
    assert!(p.pitch * (2.0 / 3.0_f64.sqrt() + 0.32) <= tenth + 1e-9 && p.pitch > 3.5, "{p:?}");
    let mut plain = e.clone();
    plain.intent.board = Default::default();
    assert_eq!(stitch::grid_pitch(&plain, 0.6).pitch, stitch::DEFAULT_GRID_PITCH);
    assert_eq!(stitch::grid_pitch(&plain, 0.6).mhz, None);
    assert_eq!(stitch::fence_pitch(&plain, &BTreeSet::new(), 0.6).pitch, stitch::DEFAULT_FENCE_PITCH);
    // Never closer than the vias fit: the via and the rules' hole to hole.
    plain.intent.board.f_concern_mhz = Some(100_000.0);
    assert!(stitch::grid_pitch(&plain, 0.6).pitch >= 0.6 + 0.25);
}

#[test]
fn a_fence_lines_an_rf_track_on_both_sides() {
    // The 2.4 GHz line from U1's RF pin, in a top GND pour 0.4 mm clear of it (a coplanar line),
    // over GND on inner1: without vias tying the pour down beside it the fence check fails.
    let mut e = scratch("xtal_rf");
    run(
        &mut e,
        json!([
            ["pour", {"layer": "inner1", "net": "GND", "rect": [50, 30], "remove_islands": false}],
            ["track", {"layer": "top", "points": [[-2.4175, 1.905], [3, 1.905], [7.095, 6], [18.5, 6]], "width": 0.3349}],
            ["pour", {"layer": "top", "net": "GND", "rect": [50, 30], "remove_islands": false, "clearance": 0.4}],
        ]),
    );
    assert_eq!(fails(&quality_of(&e, &["rf.fence"]), "rf.fence"), [Severity::Warning; 2]);
    let line = e.board.tracks.last().unwrap().clone();
    assert_eq!(stitch::rf_nets(&e), ["RF"]);
    let a = Ask { nets: vec!["RF".into()], under_parts: true, ..ask("GND", Pattern::Fence) };
    let plan = stitch::plan(&e, &e.board_nets(), &a).unwrap();
    assert_eq!(stitch::plan(&e, &e.board_nets(), &a).unwrap(), plan);
    assert!(plan.unfenced.is_empty() && plan.spots.len() >= 8, "{plan:?}");
    // On both sides of the line, past its half width, the gap and the via's radius.
    let side = |p: [f64; 2]| {
        path::segments(&line.points, &line.curves, false)
            .map(|(a, b, _)| (path::segment_distance(p, a, b, 0.0), ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])) > 0.0))
            .min_by(|x, y| x.0.total_cmp(&y.0))
            .unwrap()
    };
    let left: BTreeSet<bool> = plan.spots.iter().map(|p| side(*p).1).collect();
    assert_eq!(left, BTreeSet::from([false, true]), "vias on both sides: {:?}", plan.spots);
    for p in &plan.spots {
        assert!(side(*p).0 >= line.width / 2.0 + 0.2 + 0.3 - 1e-6, "{p:?} is {} mm from the line", side(*p).0);
    }
    let after = with_vias(&e, "GND", &plan.spots);
    assert_eq!(fails(&quality_of(&after, &["rf.fence"]), "rf.fence"), [], "the fence is there");
}

#[test]
fn a_layer_change_gets_a_ground_via_beside_it() {
    let mut e = scratch("return");
    let gnd = |layer: &str| json!(["pour", {"layer": layer, "net": "GND", "rect": [30, 20], "remove_islands": false}]);
    run(
        &mut e,
        json!([
            gnd("inner1"),
            gnd("inner2"),
            ["track", {"layer": "top", "points": [[-9.2, 0], [-3, 0]], "width": 0.2, "via": {}}],
            ["track", {"layer": "bottom", "points": [[-3, 0], [3, 0]], "width": 0.2, "via": {}}],
            ["track", {"layer": "top", "points": [[3, 0], [9.2, 0]], "width": 0.2}],
        ]),
    );
    assert_eq!(fails(&quality_of(&e, &["pi.layer_change"]), "pi.layer_change"), [Severity::Warning]);
    let mut a = ask("GND", Pattern::LayerChange);
    a.nets = vec!["SIG".into()];
    let plan = stitch::plan(&e, &e.board_nets(), &a).unwrap();
    assert!(!plan.spots.is_empty(), "{plan:?}");
    for p in &plan.spots {
        let near = [[-3.0, 0.0], [3.0, 0.0]].iter().map(|v: &[f64; 2]| (p[0] - v[0]).hypot(p[1] - v[1])).fold(f64::INFINITY, f64::min);
        assert!(near <= stitch::CHANGE_REACH + 1e-9, "{p:?} is {near} mm from the signal's vias");
    }
    let after = with_vias(&e, "GND", &plan.spots);
    assert_eq!(quality_of(&after, &["pi.layer_change"]), []);
    // Stitching again finds nothing more to do.
    assert!(stitch::plan(&after, &after.board_nets(), &a).unwrap().spots.is_empty());
}

// Fab profiles.

#[test]
fn fab_profiles_are_complete_and_their_ids_stable() {
    use super::profiles::{self, FabChoice};
    assert_eq!(profiles::find(profiles::DEFAULT_PROFILE).map(|p| p.id), Some("standard"));
    assert_eq!(Board::default().fab_profile, profiles::find("standard").map(FabChoice::of), "new boards get the standard profile");
    let mut hashes = BTreeSet::new();
    for p in profiles::PROFILES {
        assert!(p.version >= 1 && FabChoice::of(p).validate().is_ok(), "{}", p.id);
        for (name, units, f) in p.figures() {
            assert!(f.value.is_finite() && f.value > 0.0 && !f.source.is_empty() && !units.is_empty(), "{}.{name}", p.id);
        }
        let h = p.hash();
        assert!(h.len() == 8 && h.chars().all(|c| c.is_ascii_hexdigit()), "{h}");
        assert!(hashes.insert(h), "{} shares a hash", p.id);
    }
    assert!(FabChoice { id: "cheap".into(), version: 1 }.validate().unwrap_err().contains("not one of standard, fine, hdi"));
    assert!(FabChoice { id: "standard".into(), version: 0 }.validate().is_err());
    // Rings measured on the finished hole, or on the drill tool (the hole plus the plating).
    assert_eq!(profiles::find("standard").unwrap().hole(0.3), 0.3);
    assert!(close(profiles::find("fine").unwrap().hole(0.3), 0.4, 1e-12));
    // A board saved with a profile this build doesn't have doesn't open.
    let mut v = saved(&Document::new("A"));
    v["ecad"] = json!({"board": {"outline": [[0, 0], [10, 0], [10, 10], [0, 10]], "fab_profile": {"id": "cheap", "version": 1}}});
    assert!(Document::from_json(&v.to_string()).is_err());
}

#[test]
fn picking_a_profile_keeps_what_the_user_set() {
    use super::rules::Rules;
    let (standard, fine) = (super::profiles::find("standard").unwrap(), super::profiles::find("fine").unwrap());
    let mut rules = Rules::default();
    assert!(!standard.differs(&rules), "the default rules are the standard profile's");
    assert!(standard.apply(&mut rules).is_empty());
    let s = fine.switch(Some(standard), &mut rules);
    let set: Vec<(&str, f64)> = s.set.iter().map(|c| (c.field, c.to)).collect();
    assert_eq!(set, [("min_track_width", 0.1), ("min_annular_ring", 0.1), ("min_drill", 0.2), ("copper_to_edge", 0.25), ("silk_to_pad", 0.1)]);
    assert!(s.kept.is_empty() && s.set.iter().all(|c| !c.source.is_empty()));
    assert!(!fine.differs(&rules));
    // A rule the user set stays, and is listed; what new copper gets is only ever raised.
    let mut rules = Rules { min_drill: 0.4, ..Rules::default() };
    let s = fine.switch(Some(standard), &mut rules);
    assert_eq!(s.kept, [("min_drill", 0.4)]);
    assert_eq!(rules.min_drill, 0.4);
    let mut rules = Rules::default();
    fine.switch(Some(standard), &mut rules);
    rules.clearance = 0.1;
    let s = standard.switch(Some(fine), &mut rules);
    assert_eq!(rules.clearance, 0.15, "the gap new copper gets rises to the profile's least");
    assert!(s.set.iter().any(|c| c.field == "clearance" && c.from == 0.1 && c.to == 0.15));
    assert_eq!(rules.min_track_width, 0.15, "back to the standard figure");
    assert!(rules.min_drill == 0.3 && rules.min_annular_ring == 0.15);
    // Reset brings back what the user changed.
    rules.min_track_width = 0.5;
    assert!(standard.differs(&rules));
    let changed = standard.reset(&mut rules);
    assert!(changed.iter().any(|c| c.field == "min_track_width" && c.to == 0.15) && !standard.differs(&rules));
}

// What the engine proposes for the intent, on two small boards: `decap` (the MCU U1 with VDD on
// pins 1 and 2, VDDA 5, DEC 6 and VDDB 9, C1 to C3 of 100 nF, the connector J1) and `xtal_rf` (a
// 2.4 GHz radio U1 with a 32 MHz crystal Y1 and its 12 pF load capacitors, an antenna ANT1).

#[test]
fn intent_inference_proposes_supplies_decoupling_and_connectors() {
    use super::infer::{Record, coverage, infer};
    use super::intent::{Confidence, Intent, PartRole, Source};
    let mut e = scratch("decap");
    e.intent = Intent::default();
    let inf = infer(&e);
    assert_eq!(infer(&e), inf, "the same design, the same proposals");
    assert_eq!(inf.power_pins, ["U1.1", "U1.2", "U1.5", "U1.6", "U1.9"]);
    let roles: Vec<(&str, &str)> = inf.proposals.iter().map(|p| (p.key.as_str(), p.record.role())).collect();
    for want in [("net|GND", "ground"), ("net|VDD", "power"), ("part|J1", "connector"), ("part|C1", "decoupling"), ("part|C3", "decoupling")] {
        assert!(roles.contains(&want), "{want:?} in {roles:?}");
    }
    let gnd = inf.get("net|GND").unwrap();
    assert!(gnd.blocking && gnd.confidence == Confidence::High && !gnd.evidence.is_empty());
    assert_eq!(inf.get("net|VDD").unwrap().needs, ["voltage", "current_a"], "a supply's current is the user's to say");
    assert_eq!(inf.get("part|J1").unwrap().needs, ["cable"]);
    // Each capacitor serves the supply pin nearest it on the board.
    let of = |key: &str| match &inf.get(key).unwrap().record {
        Record::Part(r) => (r.role, r.of.clone()),
        r => panic!("{r:?}"),
    };
    assert_eq!(of("part|C1"), (PartRole::Decoupling, vec!["U1.1".to_string()]));
    assert_eq!(of("part|C2"), (PartRole::Decoupling, vec!["U1.5".to_string()]));
    // The low-risk ones are accepted in one go: none blocking, none of a kept record.
    let low: Vec<&str> = inf.low_risk(&e.intent).iter().map(|p| p.key.as_str()).collect();
    assert!(!low.is_empty() && low.iter().all(|k| !inf.get(k).unwrap().blocking), "{low:?}");
    let mut kept = e.intent.clone();
    let first = low[0];
    if let Record::Net(r) = inf.get(first).unwrap().record.clone() {
        kept.nets.push(r);
    }
    assert!(!inf.low_risk(&kept).iter().any(|p| p.key == first), "a kept record isn't proposed again");
    // How much is settled: nothing yet, then C1 confirmed.
    let c = coverage(&e, &e.intent, &inf);
    assert!(c.blocking_total >= 6 && c.blocking_confirmed == 0 && c.missing.len() == c.blocking_total, "{c:?}");
    assert!(c.missing.iter().all(|m| !m.why.is_empty()));
    assert_eq!((c.ic_power_pins, c.ic_power_pins_assigned), (5, 0));
    let mut settled = e.intent.clone();
    if let Record::Part(mut r) = inf.get("part|C1").unwrap().record.clone() {
        r.source = Source::Confirmed;
        settled.parts.push(r);
    }
    let c = coverage(&e, &settled, &inf);
    assert_eq!((c.blocking_confirmed, c.ic_power_pins_assigned), (1, 1), "{c:?}");
    assert!(c.unknown.is_empty() && !c.missing.iter().any(|m| m.key == "part|C1"));
    // A declared record of a net the schematic doesn't have counts, and is listed as unknown.
    settled.nets.push(serde_json::from_value(json!({"net": "U9_GND", "role": "ground"})).unwrap());
    let c = coverage(&e, &settled, &inf);
    assert_eq!(c.blocking_total, 7);
    assert_eq!(c.unknown, ["net|U9_GND"]);
    assert!(settled.validate().is_ok());
}

#[test]
fn intent_inference_finds_the_crystal_and_the_radio() {
    use super::infer::{Record, infer};
    use super::intent::{Intent, PartRole};
    let mut e = scratch("xtal_rf");
    e.intent = Intent::default();
    let inf = infer(&e);
    let role = |key: &str| inf.get(key).map(|p| p.record.role());
    assert_eq!((role("net|XC1"), role("net|XC2"), role("net|RF")), (Some("clock"), Some("clock"), Some("rf")));
    assert_eq!((role("part|Y1"), role("part|ANT1"), role("part|C1")), (Some("crystal"), Some("antenna"), Some("load_cap")));
    let Some(Record::Part(y1)) = inf.get("part|Y1").map(|p| &p.record) else { panic!("no crystal") };
    assert_eq!((y1.role, y1.of.clone()), (PartRole::Crystal, vec!["U1.1".to_string(), "U1.2".to_string()]));
    let Some(Record::Part(c1)) = inf.get("part|C1").map(|p| &p.record) else { panic!("no load capacitor") };
    assert_eq!(c1.of, ["Y1"]);
    assert!(inf.get("part|Y1").unwrap().blocking && !inf.get("part|C1").unwrap().blocking);
}

#[test]
fn values_parse_as_datasheets_write_them() {
    use super::infer::parse_value;
    let near = |text: &str, v: f64, unit: char| {
        let (got, u) = parse_value(text).unwrap_or_else(|| panic!("{text}"));
        assert!(close(got, v, 1e-12) && u == unit, "{text}: {got} {u}");
    };
    near("100nF", 1e-7, 'F');
    near("4.7uF", 4.7e-6, 'F');
    near("4µ7", 4.7e-6, '?');
    near("2.2nH", 2.2e-9, 'H');
    near("0R", 0.0, 'R');
    near("2R2", 2.2, 'R');
    near("1k5", 1500.0, '?');
    near("10k", 10_000.0, '?');
    near("1MΩ", 1e6, 'R');
    near("12pF 50V", 12e-12, 'F');
    for junk in ["", "  ", "nF", "abc", "1.2.3", "-5", &"9".repeat(400)] {
        assert_eq!(parse_value(junk), None, "{junk}");
    }
}

#[test]
fn intent_records_refuse_hostile_values() {
    use super::intent::Intent;
    let base = |extra: Value| -> Result<(), String> {
        let intent: Intent = serde_json::from_value(extra).map_err(|e| e.to_string())?;
        intent.validate()
    };
    assert!(base(json!({"nets": [{"net": "VIN", "role": "power", "current_a": 3.0, "voltage": {"nominal": 5.0}}]})).is_ok());
    for bad in [
        json!({"nets": [{"net": "VIN", "role": "power", "current_a": -1.0}]}),
        json!({"nets": [{"net": "VIN", "role": "power", "current_a": 1e9}]}),
        json!({"nets": [{"net": "VIN", "role": "power", "voltage": {"nominal": 1e9}}]}),
        json!({"nets": [{"net": "", "role": "power"}]}),
        json!({"nets": [{"net": "A\nB", "role": "power"}]}),
        json!({"nets": [{"net": "VIN", "role": "power"}, {"net": "VIN", "role": "ground"}]}),
        json!({"nets": [{"net": "VIN", "role": "power", "evidence": vec!["e"; 9]}]}),
        json!({"nets": [{"net": "VIN", "role": "nonsense"}]}),
        json!({"parts": [{"part": "C1", "role": "decoupling", "cable": true}]}),
        json!({"parts": [{"part": "C1", "role": "decoupling", "of": vec!["U1.1"; 65]}]}),
        json!({"pairs": [{"name": "USB", "p": "D", "n": "D"}]}),
        json!({"board": {"ipc_class": 4}}),
        json!({"board": {"bands": [{"name": "X", "lo_mhz": 900.0, "hi_mhz": 800.0, "rx": true}]}}),
        json!({"board": {"f_concern_mhz": 0.0}}),
        json!({"loops": [{"name": "L", "kind": "buck_hot", "members": ["C1"]}]}),
    ] {
        assert!(base(bad.clone()).is_err(), "{bad} must be refused");
    }
}

// What a print's title block says.

#[test]
fn design_info_round_trips_and_keeps_to_its_lengths() {
    use super::info::{DesignInfo, MAX_REVISIONS, Revision};
    let mut doc = Document::new("A");
    doc.ecad.info = DesignInfo {
        title: "LoRa node".into(),
        revision: "B".into(),
        date: "29 Oct 2026".into(),
        author: "M. M.".into(),
        revisions: vec![Revision { rev: "A".into(), date: "2026-09-01".into(), by: String::new(), note: "first board".into() }],
        ..DesignInfo::default()
    };
    let v = saved(&doc);
    assert_eq!(v["ecad"]["info"]["revisions"], json!([{"rev": "A", "date": "2026-09-01", "note": "first board"}]), "empty fields are not saved");
    assert!(v["ecad"]["info"].get("company").is_none());
    assert_eq!(Document::from_json(&doc.to_json()).unwrap(), doc);
    assert!(saved(&Document::new("A")).get("ecad").is_none(), "no info, nothing saved");
    let mut bad = v.clone();
    bad["ecad"]["info"]["logo"] = json!("x.png");
    assert!(Document::from_json(&bad.to_string()).is_err(), "an unknown key is refused");
    let ok = doc.ecad.info.clone();
    assert!(ok.validate().is_ok());
    for (what, info) in [
        ("a title with a line break", DesignInfo { title: "Lo\nRa".into(), ..ok.clone() }),
        ("a 65-character author", DesignInfo { author: "a".repeat(65), ..ok.clone() }),
        ("a 257-character note", DesignInfo { notes: "n".repeat(257), ..ok.clone() }),
        ("a revision with no name", DesignInfo { revisions: vec![Revision { rev: " ".into(), ..Revision::default() }], ..ok.clone() }),
        ("too many revisions", DesignInfo { revisions: vec![Revision { rev: "A".into(), ..Revision::default() }; MAX_REVISIONS + 1], ..ok.clone() }),
    ] {
        assert!(info.validate().is_err(), "{what}");
        let mut doc = doc.clone();
        doc.ecad.info = info;
        assert!(Document::from_json(&doc.to_json()).is_err(), "{what} doesn't open");
    }
}
