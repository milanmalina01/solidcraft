use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_doc::ecad::{Anchor, Approval, Label, LabelKind, Marker, MarkerKind, Wire};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn refused(s: &mut Session, id: &str, p: Value) -> String {
    let (before, undo) = (s.doc.clone(), s.undo.len());
    let Err(e) = s.execute(id, &p) else { panic!("{id} {p} must be refused") };
    assert!(Arc::ptr_eq(&before, &s.doc) || *before == *s.doc, "a refused {id} leaves the design as it was");
    assert_eq!(s.undo.len(), undo);
    e.to_string()
}

/// A session with the resistor R0603 (pins 1 at (−5.08, 0) and 2 at (5.08, 0)).
fn resistor() -> Session {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "R0603", "prefix": "R", "value": "10k"}));
    run(&mut s, "ecad.symbol.generate", json!({"symbol": "R0603", "component": "R0603", "style": "resistor"}));
    s
}

/// Place R0603 at `at`; its part id.
fn place(s: &mut Session, at: [f64; 2]) -> u64 {
    run(s, "ecad.schematic.place", json!({"parts": [{"component": "R0603", "at": at}]}))["created"][0]["id"].as_u64().unwrap()
}

fn pin(part: u64, pin: &str) -> Anchor {
    Anchor::Pin { part, pin: pin.into() }
}

/// A wire put straight on the sheet, as the wire commands to come will.
fn wire(s: &mut Session, from: Anchor, via: Vec<[f64; 2]>, to: Anchor) -> u64 {
    let sheet = &mut s.doc_mut().ecad.schematic;
    let id = sheet.take_id().unwrap();
    sheet.wires.push(Wire { id, from, via, to });
    id
}

fn label(s: &mut Session, name: &str, at: Anchor) -> u64 {
    let sheet = &mut s.doc_mut().ecad.schematic;
    let id = sheet.take_id().unwrap();
    sheet.labels.push(Label { id, name: name.into(), kind: LabelKind::Net, at, rotation: 0 });
    id
}

fn marker(s: &mut Session, kind: MarkerKind, at: Anchor) -> u64 {
    let sheet = &mut s.doc_mut().ecad.schematic;
    let id = sheet.take_id().unwrap();
    sheet.markers.push(Marker { id, kind, at });
    id
}

fn inspect(s: &mut Session) -> Value {
    run(s, "ecad.schematic.inspect", json!({}))
}

fn names(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|i| i["name"].as_str().map(str::to_string)).collect()
}

#[test]
fn a_placed_part_gives_its_pins_where_worked_out_by_hand() {
    let mut s = resistor();
    let undo = s.undo.len();
    let r = run(&mut s, "ecad.schematic.place", json!({"parts": [{"component": "R0603", "at": [10, 20], "rotation": 90}]}));
    assert_eq!(s.undo.len(), undo + 1, "one call, one undo step");
    assert_eq!(r["created"], json!([{"id": 1, "kind": "part", "name": "R1"}]));
    assert_eq!((r["changed"].clone(), r["removed"].clone()), (json!([]), json!([])));
    let part = &r["parts"][0];
    assert_eq!((part["designator"].clone(), part["value"].clone(), part["library_value"].clone()), (json!("R1"), json!("10k"), json!(true)));
    assert_eq!(part["component"], json!({"id": 1, "name": "R0603"}));
    // Turned a quarter about (10, 20): pin 1 (−5.08, 0) pointing right goes to (10, 14.92) pointing
    // up; pin 2 (5.08, 0) pointing left to (10, 25.08) pointing down.
    assert_eq!(
        part["pins"],
        json!([
            {"number": "1", "name": "", "type": "passive", "at": [10.0, 14.92], "direction": "up"},
            {"number": "2", "name": "", "type": "passive", "at": [10.0, 25.08], "direction": "down"}
        ])
    );
    // The resistor symbol: a 5.08 x 2.032 body (line 0.254) between the pins, {designator} (12
    // characters at 0.6 of 1.27 wide, line 0.127) on top from (−2.54, 1.516) and {value} under it
    // from (−2.54, −2.786): x −5.08 to −2.54 + 9.144 + 0.0635 = 6.6675, y ±2.8495. Turned a quarter
    // about (10, 20): x 10 ∓ 2.8495, y 20 − 5.08 to 20 + 6.6675.
    assert_eq!(part["bbox"], json!([[7.1505, 14.92], [12.8495, 26.6675]]));
}

#[test]
fn parts_given_no_position_go_in_rows_and_number_on() {
    let mut s = resistor();
    let r = run(&mut s, "ecad.schematic.place", json!({"parts": [{"component": "R0603"}, {"component": "R0603", "value": "4k7"}]}));
    assert_eq!(names(&r["created"]), ["R1", "R2"]);
    let at: Vec<Value> = r["parts"].as_array().unwrap().iter().map(|p| p["at"].clone()).collect();
    // The box (x −5.08 to 6.6675, y ±2.8495) goes under the origin: x 5.08, y −2.8495 down to the
    // grid, −5.08; the next 10.16 on from 5.08 + 6.6675, its left edge 5.08 more and up to the grid.
    assert_eq!(at, [json!([5.08, -5.08]), json!([27.94, -5.08])], "left to right under the origin, on the grid");
    assert_eq!((r["parts"][1]["value"].clone(), r["parts"][1]["library_value"].clone()), (json!("4k7"), json!(false)));
    // The next row goes 10.16 under the first, whose boxes reach down to −5.08 − 2.8495 = −7.9295:
    // its top at −18.0895, the origin 2.8495 lower and down to the grid.
    let r = run(&mut s, "ecad.schematic.place", json!({"parts": [{"component": "R0603", "designator": "R7"}, {"component": "R0603"}]}));
    assert_eq!(names(&r["created"]), ["R7", "R8"], "a designator given counts before the numbers handed out");
    assert_eq!(r["parts"][0]["at"], json!([5.08, -22.86]));
    // The highest number goes on: a gap is never filled, though the highest, deleted, comes back.
    run(&mut s, "ecad.schematic.delete", json!({"items": ["R2", "R8"]}));
    let r = run(&mut s, "ecad.schematic.place", json!({"parts": [{"component": "R0603", "at": [0, 50]}]}));
    assert_eq!(names(&r["created"]), ["R8"]);
    let e = refused(&mut s, "ecad.schematic.place", json!({"parts": [{"component": "R0603", "designator": "r7"}]}));
    assert!(e.contains("designated `r7`"), "designators are unique ignoring case: {e}");
}

#[test]
fn bad_rows_are_refused_and_name_the_row() {
    let mut s = resistor();
    run(&mut s, "ecad.component.create", json!({"name": "Blank"}));
    for (p, says) in [
        (json!({"parts": []}), "1 to 4096 parts"),
        (json!({"parts": [{"component": "R0630"}]}), "did you mean `R0603`"),
        (json!({"parts": [{"component": "Blank"}]}), "has no symbol yet"),
        (json!({"parts": [{"component": "R0603"}, {"component": "R0603", "rotation": 45}]}), "parts[1]: `rotation` must be 0, 90, 180 or 270"),
        (json!({"parts": [{"component": "R0603", "at": [2001, 0]}]}), "within 2000 mm"),
        (json!({"parts": [{"component": "R0603", "designator": "1R"}]}), "parts[0]: the designator `1R`"),
        (json!({"parts": [{"component": "R0603", "value": "v".repeat(65)}]}), "longer than 64"),
    ] {
        let e = refused(&mut s, "ecad.schematic.place", p.clone());
        assert!(e.contains(says), "{p}: {e}");
    }
    let rows: Vec<Value> = (0..4097).map(|_| json!({"component": "R0603"})).collect();
    assert!(refused(&mut s, "ecad.schematic.place", json!({"parts": rows})).contains("1 to 4096 parts"));
    assert_eq!(run(&mut s, "ecad.schematic.place", json!({"parts": [{"component": "R0603", "rotation": -90}]}))["parts"][0]["rotation"], 270);
}

#[test]
fn edits_change_what_is_given_and_null_goes_back_to_the_library() {
    let mut s = resistor();
    place(&mut s, [0.0, 0.0]);
    let undo = s.undo.len();
    let r = run(&mut s, "ecad.schematic.edit", json!({"parts": [{"part": "R1", "value": "4k7", "dnp": true, "designator_at": [0, 2.54]}]}));
    assert_eq!(r["changed"], json!([{"id": 1, "kind": "part", "name": "R1"}]));
    let p = &r["parts"][0];
    assert_eq!((p["value"].clone(), p["library_value"].clone(), p["dnp"].clone()), (json!("4k7"), json!(false), json!(true)));
    assert_eq!(p["designator_at"], json!([0.0, 2.54]));
    let r = run(&mut s, "ecad.schematic.edit", json!({"parts": [{"part": 1, "value": null, "designator_at": null, "designator": "R10"}]}));
    let p = &r["parts"][0];
    assert_eq!((p["value"].clone(), p["library_value"].clone(), p["designator_at"].clone()), (json!("10k"), json!(true), json!(null)));
    assert_eq!(p["designator"], "R10");
    assert_eq!(s.undo.len(), undo + 2);
    let before = s.doc.clone();
    let r = run(&mut s, "ecad.schematic.edit", json!({"parts": [{"part": "r10", "dnp": true}]}));
    assert_eq!(r["changed"], json!([]));
    assert!(Arc::ptr_eq(&before, &s.doc), "an edit that changes nothing leaves the design and the undo history alone");
    let w = wire(&mut s, pin(1, "2"), Vec::new(), Anchor::Point([10.16, 0.0]));
    let e = refused(&mut s, "ecad.schematic.edit", json!({"parts": [{"part": w, "dnp": true}]}));
    assert!(e.contains("is a wire, not a part"), "{e}");
    let e = refused(&mut s, "ecad.schematic.edit", json!({"parts": [{"part": "R11"}]}));
    assert!(e.contains("did you mean `R10`"), "{e}");
}

#[test]
fn a_swapped_component_keeps_what_is_on_pins_numbered_alike() {
    let mut s = resistor();
    run(&mut s, "ecad.component.create", json!({"name": "TP", "prefix": "TP"}));
    run(&mut s, "ecad.symbol.generate", json!({"symbol": "TP", "component": "TP", "style": "box", "pins": [{"number": 1}]}));
    let r1 = place(&mut s, [0.0, 0.0]);
    let a = wire(&mut s, pin(r1, "1"), Vec::new(), Anchor::Point([-10.16, 0.0]));
    let b = wire(&mut s, pin(r1, "2"), Vec::new(), Anchor::Point([10.16, 0.0]));
    let r = run(&mut s, "ecad.schematic.edit", json!({"parts": [{"part": "R1", "component": "TP"}]}));
    assert_eq!(r["parts"][0]["component"]["name"], "TP");
    assert_eq!(r["parts"][0]["designator"], "R1", "the designator stays");
    assert_eq!(
        r["detached"],
        json!([{"kind": "wire", "id": b, "end": "from", "part": r1, "designator": "R1", "pin": "2", "at": [5.08, 0.0], "removed": false}])
    );
    let wires = inspect(&mut s)["wires"].clone();
    assert_eq!(wires[0]["id"], a);
    assert_eq!(wires[0]["from"]["pin"], "1", "pin 1 is on both, so its wire stays on it");
    assert_eq!(wires[1]["from"], json!({"at": [5.08, 0.0]}));
}

#[test]
fn moves_shift_turn_and_mirror() {
    let mut s = resistor();
    let r1 = place(&mut s, [0.0, 0.0]);
    let free = label(&mut s, "IN", Anchor::Point([-10.16, 0.0]));
    wire(&mut s, pin(r1, "1"), vec![[-10.16, 5.08]], Anchor::Point([-15.24, 5.08]));
    let loose = wire(&mut s, Anchor::Point([-15.24, 0.0]), vec![[-15.24, -5.08]], Anchor::Point([-20.32, -5.08]));
    let r = run(&mut s, "ecad.schematic.move", json!({"items": ["R1"], "by": [2.54, 0]}));
    assert_eq!(r["parts"][0]["at"], json!([2.54, 0.0]));
    assert_eq!(names(&r["changed"]), ["R1"], "a wire not moved keeps its saved points");
    let r = run(&mut s, "ecad.schematic.move", json!({"items": [r1], "to": [10.16, 10.16], "rotation": 90}));
    assert_eq!((r["parts"][0]["at"].clone(), r["parts"][0]["rotation"].clone()), (json!([10.16, 10.16]), json!(90)));
    // Mirrored about its own origin after the turn: a mirror after a turn θ is a turn −θ.
    let r = run(&mut s, "ecad.schematic.move", json!({"items": ["R1"], "mirror": true}));
    let p = &r["parts"][0];
    assert_eq!((p["rotation"].clone(), p["mirror"].clone()), (json!(270), json!(true)));
    assert_eq!(p["pins"][0]["at"], json!([10.16, 5.08]), "pin 1 stays below, mirrored about the vertical line through the part");
    let wires = inspect(&mut s)["wires"].clone();
    assert_eq!(wires[0]["from"], json!({"at": [10.16, 5.08], "part": r1, "designator": "R1", "pin": "1"}), "a wire end on a pin follows it");
    // Several items turn about the middle of their positions, on the grid: (−12.7, 0) between the
    // label at (−10.16, 0) and the wire's first end at (−15.24, 0). Saved points move.
    let r = run(&mut s, "ecad.schematic.move", json!({"items": [free, loose], "rotation": 180}));
    assert_eq!(r["changed"].as_array().map(Vec::len), Some(2));
    let sheet = inspect(&mut s);
    assert_eq!((sheet["labels"][0]["at"]["at"].clone(), sheet["labels"][0]["rotation"].clone()), (json!([-15.24, 0.0]), json!(180)));
    let moved = &sheet["wires"][1];
    assert_eq!((moved["from"]["at"].clone(), moved["via"].clone()), (json!([-10.16, 0.0]), json!([[-10.16, 5.08]])));
    assert_eq!(moved["to"]["at"], json!([-5.08, 5.08]));
    let undo = s.undo.len();
    run(&mut s, "ecad.schematic.move", json!({"items": ["R1"], "by": [0, 0]}));
    assert_eq!(s.undo.len(), undo, "a move by nothing makes no undo step");
    for (p, says) in [
        (json!({"items": ["R1"], "by": [1, 0], "to": [0, 0]}), "not both"),
        (json!({"items": ["R1"]}), "give `by` or `to`"),
        (json!({"items": []}), "name 1 to"),
        (json!({"items": ["R1"], "by": [5000, 0]}), "within 4000 mm"),
        (json!({"items": ["R1"], "by": [2000, 0]}), "within 2000 mm"),
        (json!({"items": [999], "by": [1, 0]}), "nothing with the id 999"),
    ] {
        let e = refused(&mut s, "ecad.schematic.move", p.clone());
        assert!(e.contains(says), "{p}: {e}");
    }
}

#[test]
fn duplicates_get_the_next_designators_and_keep_their_wires_between_them() {
    let mut s = resistor();
    let (r1, r2) = (place(&mut s, [0.0, 0.0]), place(&mut s, [20.32, 0.0]));
    let w = wire(&mut s, pin(r1, "2"), Vec::new(), pin(r2, "1"));
    let l = label(&mut s, "OUT", pin(r2, "2"));
    marker(&mut s, MarkerKind::NoConnect, pin(r1, "1"));
    let r = run(&mut s, "ecad.schematic.duplicate", json!({"items": ["R1", "R2", w, l], "by": [0, -20.32]}));
    let created: Vec<(String, Option<String>)> =
        r["created"].as_array().unwrap().iter().map(|i| (i["kind"].as_str().unwrap().to_string(), i["name"].as_str().map(str::to_string))).collect();
    assert_eq!(
        created,
        [("part".into(), Some("R3".into())), ("part".into(), Some("R4".into())), ("wire".into(), None), ("label".into(), Some("OUT".into()))]
    );
    let (r3, r4) = (r["created"][0]["id"].clone(), r["created"][1]["id"].clone());
    let sheet = inspect(&mut s);
    assert_eq!(sheet["parts"][2]["at"], json!([0.0, -20.32]));
    let copy = &sheet["wires"][1];
    assert_eq!((copy["from"]["part"].clone(), copy["from"]["pin"].clone()), (r3, json!("2")));
    assert_eq!((copy["to"]["part"].clone(), copy["to"]["pin"].clone()), (r4.clone(), json!("1")));
    assert_eq!(sheet["labels"][1]["at"]["part"], r4, "the label is on the copy's pin, with the same name");
    assert_eq!(sheet["markers"].as_array().map(Vec::len), Some(1), "a no-connect not named is not copied");
    // A wire copied without the part at its other end ends on the point by that pin.
    let r = run(&mut s, "ecad.schematic.duplicate", json!({"items": ["R1", w], "by": [0, -40.64]}));
    assert_eq!(names(&r["created"]), ["R5"]);
    let last = inspect(&mut s)["wires"][2].clone();
    assert_eq!(last["to"], json!({"at": [15.24, -40.64]}));
}

#[test]
fn deleting_a_part_keeps_its_wires_at_the_old_pins_and_undo_brings_it_back() {
    let mut s = resistor();
    let (r1, r2) = (place(&mut s, [0.0, 0.0]), place(&mut s, [20.32, 0.0]));
    let w = wire(&mut s, pin(r1, "2"), Vec::new(), pin(r2, "1"));
    let nc = marker(&mut s, MarkerKind::NoConnect, pin(r1, "1"));
    let flag = marker(&mut s, MarkerKind::PowerFlag, pin(r1, "1"));
    let before = s.doc.clone();
    let r = run(&mut s, "ecad.schematic.delete", json!({"items": ["R1"]}));
    assert_eq!(r["removed"], json!([{"id": r1, "kind": "part", "name": "R1"}, {"id": nc, "kind": "marker"}]));
    assert_eq!(
        r["detached"],
        json!([
            {"kind": "wire", "id": w, "end": "from", "part": r1, "designator": "R1", "pin": "2", "at": [5.08, 0.0], "removed": false},
            {"kind": "marker", "id": nc, "part": r1, "designator": "R1", "pin": "1", "at": [-5.08, 0.0], "removed": true},
            {"kind": "marker", "id": flag, "part": r1, "designator": "R1", "pin": "1", "at": [-5.08, 0.0], "removed": false}
        ])
    );
    let sheet = inspect(&mut s);
    assert_eq!(sheet["wires"][0]["from"], json!({"at": [5.08, 0.0]}), "the wire stays, dangling where the pin was");
    assert_eq!(sheet["wires"][0]["to"]["designator"], "R2");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(*s.doc, *before, "undo brings the part and its anchors back");
    let e = refused(&mut s, "ecad.schematic.delete", json!({"items": ["R1", "R9"]}));
    assert!(e.contains("items[1]") && e.contains("R9"), "all or none: {e}");
}

#[test]
fn annotation_renumbers_and_approvals_keyed_by_id_survive_it() {
    let mut s = resistor();
    let right = place(&mut s, [20.32, 0.0]);
    place(&mut s, [0.0, 0.0]);
    let top = run(&mut s, "ecad.schematic.place", json!({"parts": [{"component": "R0603", "at": [10.16, 10.16], "designator": "PULLUP"}]}))
        ["created"][0]["id"]
        .as_u64()
        .unwrap();
    s.doc_mut().ecad.schematic.approvals.push(Approval { key: format!("pin_unconnected|p{right}.1"), note: "test".into(), by: None });
    let r = run(&mut s, "ecad.schematic.annotate", json!({"order": "position", "keep_existing": false}));
    assert_eq!(r["renames"], json!([{"part": top, "from": "PULLUP", "to": "R1"}, {"part": right, "from": "R1", "to": "R3"}]));
    assert_eq!(names(&r["changed"]), ["R3", "R1"], "in sheet order");
    assert_eq!(inspect(&mut s)["approvals"][0]["key"], format!("pin_unconnected|p{right}.1"));
    let undo = s.undo.len();
    let r = run(&mut s, "ecad.schematic.annotate", json!({"order": "position", "keep_existing": true}));
    assert_eq!((r["renames"].clone(), s.undo.len()), (Value::Null, undo), "nothing to do, no undo step");
    let r = run(&mut s, "ecad.schematic.annotate", json!({"order": "placement", "keep_existing": false, "start": 101}));
    assert_eq!(r["renames"].as_array().map(Vec::len), Some(3));
    assert!(refused(&mut s, "ecad.schematic.annotate", json!({"order": "position", "keep_existing": false, "start": 0})).contains("`start`"));
}

#[test]
fn inspect_only_reads_and_says_the_same_twice() {
    let spec = crate::find_command("ecad.schematic.inspect").unwrap();
    assert!(!spec.undoable && !spec.reads_model && spec.contract.is_some() && spec.output.is_some());
    assert!(crate::control_pending::command_reads_only("ecad.schematic.inspect"));
    let mut s = resistor();
    let r1 = place(&mut s, [0.0, 0.0]);
    let r2 = place(&mut s, [20.32, 0.0]);
    wire(&mut s, pin(r1, "2"), vec![[10.16, 0.0]], pin(r2, "1"));
    label(&mut s, "VOUT", pin(r2, "2"));
    let (doc, undo) = (s.doc.clone(), s.undo.len());
    let a = serde_json::to_string(&inspect(&mut s)).unwrap();
    let b = serde_json::to_string(&inspect(&mut s)).unwrap();
    assert_eq!(a, b, "byte for byte");
    assert!(Arc::ptr_eq(&doc, &s.doc) && s.undo.len() == undo);
    let one = run(&mut s, "ecad.schematic.inspect", json!({"part": "R2"}));
    assert_eq!(names(&one["labels"]), ["VOUT"]);
    assert_eq!(one["parts"].as_array().map(Vec::len), Some(1));
    assert_eq!(one["wires"][0]["via"], json!([[10.16, 0.0]]));
    for p in [json!("R2.2"), json!({"part": "R2", "pin": 2}), json!({"part": r2, "pin": "2"})] {
        let pin = run(&mut s, "ecad.schematic.inspect", json!({"pin": p}));
        assert_eq!(pin["parts"][0]["pins"], json!([{"number": "2", "name": "", "type": "passive", "at": [25.4, 0.0], "direction": "left"}]));
        assert_eq!((names(&pin["labels"]), pin["wires"].clone()), (vec!["VOUT".to_string()], json!([])));
    }
    for (p, says) in [
        (json!({"pin": "R2.9"}), "R2 has no pin numbered or named `9`"),
        (json!({"pin": "R2"}), "the designator, a dot"),
        (json!({"pin": "R22.1"}), "did you mean `R2`"),
        (json!({"part": "R1", "pin": "R1.1"}), "not both"),
    ] {
        let e = refused(&mut s, "ecad.schematic.inspect", p.clone());
        assert!(e.contains(says), "{p}: {e}");
    }
}

#[test]
fn library_edits_keep_the_schematic_in_step() {
    let mut s = resistor();
    let r1 = place(&mut s, [0.0, 0.0]);
    let w = wire(&mut s, pin(r1, "2"), Vec::new(), Anchor::Point([10.16, 0.0]));
    s.doc_mut().ecad.schematic.approvals.push(Approval { key: format!("pin_unconnected|p{r1}.2"), note: String::new(), by: None });
    // A renumbered pin keeps its wire and its approvals.
    run(&mut s, "ecad.pin.edit", json!({"symbol": "R0603", "pin": 2, "number": "B"}));
    let sheet = inspect(&mut s);
    assert_eq!(sheet["wires"][0]["from"]["pin"], "B");
    assert_eq!(sheet["approvals"][0]["key"], format!("pin_unconnected|p{r1}.B"));
    // A moved pin keeps it too; the leg to it may go diagonal.
    run(&mut s, "ecad.pin.edit", json!({"symbol": "R0603", "pin": "B", "at": [7.62, 2.54]}));
    assert_eq!(inspect(&mut s)["wires"][0]["from"]["at"], json!([7.62, 2.54]));
    // A deleted pin lets go where it was.
    let r = run(&mut s, "ecad.pin.delete", json!({"symbol": "R0603", "pins": ["B"]}));
    assert_eq!(
        r["detached"],
        json!([{"kind": "wire", "id": w, "end": "from", "part": r1, "designator": "R1", "pin": "B", "at": [7.62, 2.54], "removed": false}])
    );
    assert_eq!(inspect(&mut s)["wires"][0]["from"], json!({"at": [7.62, 2.54]}));
    // A generator that replaces the pins reports the same.
    let w1 = wire(&mut s, pin(r1, "1"), Vec::new(), Anchor::Point([-10.16, 0.0]));
    let r = run(&mut s, "ecad.symbol.generate", json!({"symbol": "R0603", "component": "R0603", "style": "box", "pins": [{"number": 3}]}));
    assert_eq!(r["detached"][0]["id"], w1);
    assert_eq!(r["detached"][0]["at"], json!([-5.08, 0.0]));
    // A placed component's symbol can't be emptied: with no pins left, its body stays.
    run(&mut s, "ecad.pin.delete", json!({"symbol": "R0603", "pins": [3]}));
    let e = refused(&mut s, "ecad.symbol.set_graphics", json!({"symbol": "R0603", "graphics": []}));
    assert!(e.contains("schematic part R1") && e.contains("has no symbol"), "{e}");
    // A pin edit with no part placed reports nothing new.
    let r = run(&mut s, "ecad.pin.add", json!({"symbol": "R0603", "pins": [{"number": 4, "at": [0, -7.62], "direction": "up"}]}));
    assert!(r.get("detached").is_none(), "{r}");
}

#[test]
fn a_placed_component_is_deleted_only_with_its_parts() {
    let mut s = resistor();
    let r1 = place(&mut s, [0.0, 0.0]);
    let w = wire(&mut s, pin(r1, "2"), Vec::new(), Anchor::Point([10.16, 0.0]));
    let e = refused(&mut s, "ecad.component.delete", json!({"components": ["R0603"]}));
    assert!(e.contains("placed on the schematic as R1") && e.contains("with_parts"), "{e}");
    let undo = s.undo.len();
    let r = run(&mut s, "ecad.component.delete", json!({"components": ["R0603"], "with_parts": true}));
    assert_eq!(r["parts"], json!([{"id": r1, "kind": "part", "name": "R1"}]));
    assert_eq!(r["detached"][0]["id"], w);
    assert_eq!(s.undo.len(), undo + 1, "one undo step for the component and its parts");
    // The symbol stays in the library; the errors and warnings are the electrical rules check's.
    let listed = run(&mut s, "ecad.inspect", json!({}));
    let erc = run(&mut s, "ecad.erc.inspect", json!({}))["counts"].clone();
    assert_eq!((listed["schematic"]["errors"].clone(), listed["schematic"]["warnings"].clone()), (erc["errors"].clone(), erc["warnings"].clone()));
    assert_eq!(listed["counts"], json!({"symbols": 1, "footprints": 0, "packages": 0, "components": 0}));
    assert_eq!(listed["schematic"], json!({"parts": 0, "wires": 1, "labels": 0, "markers": 0, "approvals": 0, "errors": 1, "warnings": 2}));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(inspect(&mut s)["parts"][0]["designator"], "R1");
}

#[test]
fn a_prefix_change_never_renumbers_but_annotation_does() {
    let mut s = resistor();
    place(&mut s, [0.0, 0.0]);
    run(&mut s, "ecad.component.edit", json!({"component": "R0603", "prefix": "C"}));
    assert_eq!(inspect(&mut s)["parts"][0]["designator"], "R1");
    let r = run(&mut s, "ecad.schematic.place", json!({"parts": [{"component": "R0603", "at": [20.32, 0]}]}));
    assert_eq!(names(&r["created"]), ["C1"]);
    let r = run(&mut s, "ecad.schematic.annotate", json!({"order": "placement", "keep_existing": true}));
    assert_eq!(r["renames"], json!([{"part": 1, "from": "R1", "to": "C2"}]));
}

#[test]
fn ecad_inspect_counts_what_the_sheet_holds() {
    let mut s = resistor();
    assert!(run(&mut s, "ecad.inspect", json!({})).get("schematic").is_none());
    let r1 = place(&mut s, [0.0, 0.0]);
    label(&mut s, "VCC", pin(r1, "1"));
    let counts = run(&mut s, "ecad.inspect", json!({}))["schematic"].clone();
    let erc = run(&mut s, "ecad.erc.inspect", json!({}))["counts"].clone();
    assert_eq!((counts["errors"].clone(), counts["warnings"].clone()), (erc["errors"].clone(), erc["warnings"].clone()), "the check's own counts");
    assert_eq!(counts, json!({"parts": 1, "wires": 0, "labels": 1, "markers": 0, "approvals": 0, "errors": 1, "warnings": 2}));
}

#[test]
fn the_schematic_saves_and_reopens_and_unknown_keys_are_refused() {
    let dir = std::env::temp_dir().join(format!("solidcraft-ecad-schematic-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("sheet.sc");
    let mut s = resistor();
    let r1 = place(&mut s, [0.0, 0.0]);
    run(&mut s, "ecad.schematic.place", json!({"parts": [{"component": "R0603", "rotation": 90, "mirror": true, "value": "1k"}]}));
    wire(&mut s, pin(r1, "2"), vec![[10.16, 0.0]], Anchor::Point([10.16, 10.16]));
    marker(&mut s, MarkerKind::NoConnect, pin(r1, "1"));
    let before = inspect(&mut s);
    run(&mut s, "file.save", json!({"path": path, "format": "json"}));
    run(&mut s, "file.new", json!({}));
    run(&mut s, "doc.open", json!({"path": path}));
    assert!(!s.is_dirty());
    assert_eq!(inspect(&mut s), before);
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["ecad"]["schematic"]["parts"][0]["angle"] = json!(90);
    let bad = dir.join("bad.sc");
    std::fs::write(&bad, v.to_string()).unwrap();
    run(&mut s, "file.new", json!({}));
    assert!(s.execute("doc.open", &json!({"path": bad})).is_err(), "a key this build doesn't know is refused, not dropped");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_move_keeps_the_netlist_and_says_what_it_made_or_broke_anyway() {
    let mut s = resistor();
    for at in [[0.0, 0.0], [20.32, 0.0], [0.0, -20.32]] {
        place(&mut s, at);
    }
    run(&mut s, "ecad.schematic.wire", json!({"wires": [{"from": "R1.2", "to": "R2.1"}]}));
    run(&mut s, "ecad.schematic.label", json!({"labels": [{"name": "VIN", "at": "R1.1"}]}));
    let before = run(&mut s, "ecad.netlist.inspect", json!({}))["nets"].clone();
    // A part moved with a wire on it: the wire follows, the netlist stays.
    let r = run(&mut s, "ecad.schematic.move", json!({"items": ["R2"], "by": [0, 5.08]}));
    assert_eq!(r["netlist_changed"], json!([]));
    assert_eq!(names(&r["changed"]), ["R2"]);
    assert_eq!(r["changed"][1]["kind"], "wire", "the wire's leg was moved too: {r}");
    let r = run(&mut s, "ecad.schematic.move", json!({"items": ["R1"], "by": [0, -10.16]}));
    assert_eq!(r["netlist_changed"], json!([]));
    let after = run(&mut s, "ecad.netlist.inspect", json!({}))["nets"].clone();
    let refs = |nets: &Value| -> Vec<Vec<String>> {
        nets.as_array()
            .unwrap()
            .iter()
            .map(|n| n["pins"].as_array().unwrap().iter().map(|p| p["ref"].as_str().unwrap().to_string()).collect())
            .collect()
    };
    assert_eq!(refs(&after), refs(&before), "the same pins on the same nets");
    // A pin put down on another pin joins it: the move says so.
    let r = run(&mut s, "ecad.schematic.move", json!({"items": ["R3"], "to": [30.48, 5.08]}));
    assert_eq!(r["netlist_changed"], json!([{"net": "net:R2.2", "lost": [], "gained": ["R2.2", "R3.1"]}]));
    // And taken off it again, the join is broken.
    let r = run(&mut s, "ecad.schematic.move", json!({"items": ["R3"], "by": [0, -10.16]}));
    let changed = r["netlist_changed"].as_array().unwrap();
    assert!(changed.iter().any(|c| c["lost"].as_array().is_some_and(|l| l.contains(&json!("R3.1")))), "{r}");
    // Turned and mirrored, the wires stay on the pins.
    let r = run(&mut s, "ecad.schematic.move", json!({"items": ["R2"], "rotation": 90}));
    assert_eq!(r["netlist_changed"], json!([]), "{r}");
    let r = run(&mut s, "ecad.schematic.move", json!({"items": ["R1", "R2"], "mirror": true}));
    assert_eq!(r["netlist_changed"], json!([]), "{r}");
    let nets = run(&mut s, "ecad.netlist.inspect", json!({"net": "R1.2"}))["nets"][0]["pins"].clone();
    let on: Vec<&str> = nets.as_array().unwrap().iter().filter_map(|p| p["ref"].as_str()).collect();
    assert_eq!(on, ["R1.2", "R2.1"]);
}
