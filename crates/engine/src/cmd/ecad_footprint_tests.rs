//! Pins and pads together: the symbol's pins connect to the footprint's pads by number.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn refused(s: &mut Session, id: &str, p: Value) -> String {
    let before = s.doc.clone();
    let Err(e) = s.execute(id, &p) else { panic!("{id} {p} must be refused") };
    assert!(Arc::ptr_eq(&before, &s.doc) || *before == *s.doc, "a refused {id} leaves the design as it was");
    e.to_string()
}

fn close(a: &Value, b: [f64; 2]) -> bool {
    a.as_array().is_some_and(|v| v.len() == 2 && v.iter().zip(b).all(|(x, y)| x.as_f64().is_some_and(|x| (x - y).abs() < 1e-9)))
}

/// An 0603 resistor: the symbol R0603 with two pins 10.16 mm apart on the schematic, the footprint
/// R0603 with two 0.9 x 1.0 mm pads 1.6 mm apart on the board, and the component R0603 built
/// with both.
fn r0603(s: &mut Session) {
    run(s, "ecad.component.create", json!({"name": "R0603", "prefix": "R", "value": "10k"}));
    run(s, "ecad.symbol.create", json!({"name": "R0603", "component": "R0603"}));
    run(
        s,
        "ecad.pin.add",
        json!({"symbol": "R0603", "pins": [
            {"number": 1, "at": [-5.08, 0], "direction": "right"},
            {"number": 2, "at": [5.08, 0], "direction": "left"}
        ]}),
    );
    run(s, "ecad.footprint.create", json!({"name": "R0603", "component": "R0603"}));
    run(
        s,
        "ecad.pad.add",
        json!({"footprint": "R0603", "pads": [
            {"number": 1, "at": [-0.8, 0], "size": [0.9, 1.0]},
            {"number": 2, "at": [0.8, 0], "size": [0.9, 1.0]}
        ]}),
    );
}

/// A component `name` with a symbol and a footprint of its own, both named `name` and empty.
fn empty(s: &mut Session, name: &str, prefix: &str) {
    run(s, "ecad.component.create", json!({"name": name, "prefix": prefix}));
    run(s, "ecad.symbol.create", json!({"name": name, "component": name}));
    run(s, "ecad.footprint.create", json!({"name": name, "component": name}));
}

fn component(s: &mut Session, name: &str) -> Value {
    run(s, "ecad.component.inspect", json!({"component": name}))["component"].clone()
}

fn footprint(s: &mut Session, name: &str) -> Value {
    run(s, "ecad.footprint.inspect", json!({"footprint": name}))["footprint"].clone()
}

fn symbol(s: &mut Session, name: &str) -> Value {
    run(s, "ecad.symbol.inspect", json!({"symbol": name}))["symbol"].clone()
}

/// A footprint's measured pad numbered `n`.
fn measured<'a>(f: &'a Value, n: &str) -> &'a Value {
    f["measured"].as_array().and_then(|m| m.iter().find(|p| p["number"] == n)).unwrap_or(&Value::Null)
}

#[test]
fn a_resistor_is_built_and_measured() {
    let mut s = Session::default();
    r0603(&mut s);
    let c = component(&mut s, "R0603");
    assert_eq!(c["pins"][0], json!({"number": "1", "name": "", "electrical": "passive", "at": [-5.08, 0.0], "direction": "right", "length": 2.54}));
    let f = footprint(&mut s, "R0603");
    assert_eq!(
        f["pads"][1],
        json!({"number": "2", "at": [0.8, 0.0], "size": [0.9, 1.0], "rotation": 0.0, "side": "top", "shape": "rect", "corner": 0.0, "kind": "smd", "drill": null})
    );
    let sym = symbol(&mut s, "R0603");
    assert!(close(&sym["body_ends"][0]["body_end"], [-2.54, 0.0]), "{sym}");
    assert!(close(&sym["body_ends"][1]["body_end"], [2.54, 0.0]), "{sym}");
    assert!(close(&sym["bbox"][0], [-5.08, 0.0]) && close(&sym["bbox"][1], [5.08, 0.0]), "{sym}");
    assert!((measured(&f, "1")["area"].as_f64().unwrap() - 0.9).abs() < 1e-12, "{f}");
    assert!(close(&f["copper_bbox"][0], [-1.25, -0.5]) && close(&f["copper_bbox"][1], [1.25, 0.5]), "{f}");
    let v = &c["variants"][0];
    assert_eq!(v["name"], "R0603", "the first variant is named after its footprint");
    assert_eq!(v["connections"], json!([{"pin": "1", "pads": ["1"]}, {"pin": "2", "pads": ["2"]}]));
    assert_eq!((v["unconnected_pins"].clone(), v["unconnected_pads"].clone()), (json!([]), json!([])));
    let listed = run(&mut s, "ecad.inspect", json!({}));
    assert_eq!((listed["components"][0]["pins"].clone(), listed["components"][0]["pads"].clone()), (json!(2), json!(2)));
    assert_eq!(listed["footprints"], json!([{"id": 3, "name": "R0603", "count": 2}]));
}

#[test]
fn a_rotated_pad_is_measured_in_the_board_frame() {
    let mut s = Session::default();
    run(&mut s, "ecad.footprint.create", json!({"name": "TEST"}));
    run(&mut s, "ecad.pad.add", json!({"footprint": "TEST", "pads": [{"number": "A", "at": [10, 0], "size": [1, 2], "rotation": 45}]}));
    let f = footprint(&mut s, "TEST");
    // Half extents of a 1 x 2 rectangle turned 45°: (0.5 + 1) * sin 45° = 1.0606601…
    let h = 1.5 * std::f64::consts::FRAC_1_SQRT_2;
    let m = measured(&f, "A");
    assert!(close(&m["bbox"][0], [10.0 - h, -h]) && close(&m["bbox"][1], [10.0 + h, h]), "{m}");
    assert!((m["area"].as_f64().unwrap() - 2.0).abs() < 1e-12, "rotation keeps the area");
}

#[test]
fn unmatched_numbers_are_reported_as_unconnected() {
    let mut s = Session::default();
    r0603(&mut s);
    run(
        &mut s,
        "ecad.pin.add",
        json!({"symbol": "R0603", "pins": [{"number": 3, "name": "NC", "electrical": "not_connected", "at": [0, -5.08], "direction": "up"}]}),
    );
    run(&mut s, "ecad.pad.add", json!({"footprint": "R0603", "pads": [{"number": "MP", "at": [0, 2], "size": [0.5, 0.5], "side": "bottom"}]}));
    let v = component(&mut s, "R0603")["variants"][0].clone();
    assert_eq!((v["unconnected_pins"].clone(), v["unconnected_pads"].clone()), (json!(["3"]), json!(["MP"])));
    assert_eq!(v["connections"][2], json!({"pin": "3", "pads": []}));
    assert_eq!(footprint(&mut s, "R0603")["pads"][2]["side"], "bottom");
}

#[test]
fn each_call_is_one_undo_step_and_replace_rebuilds() {
    let mut s = Session::default();
    run(&mut s, "ecad.footprint.create", json!({"name": "R0603"}));
    let undo = s.undo.len();
    let pads = json!([{"number": 1, "at": [-0.8, 0], "size": [0.9, 1.0]}, {"number": 2, "at": [0.8, 0], "size": [0.9, 1.0]}]);
    assert_eq!(
        run(&mut s, "ecad.pad.add", json!({"footprint": "R0603", "pads": pads})),
        json!({"footprint": {"id": 1, "name": "R0603"}, "added": 2, "pads": 2, "pin_map_cleared": false})
    );
    assert_eq!(s.undo.len(), undo + 1, "two pads, one undo step");
    let before = footprint(&mut s, "R0603");
    run(&mut s, "ecad.pad.add", json!({"footprint": "R0603", "pads": pads, "replace": true}));
    assert_eq!(footprint(&mut s, "R0603"), before, "replace gives the same footprint again");
    assert_eq!(s.undo.len(), undo + 1, "a call that changes nothing adds no undo step");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(footprint(&mut s, "R0603")["pads"], json!([]));
}

#[test]
fn edits_and_deletes_by_number() {
    let mut s = Session::default();
    r0603(&mut s);
    let r = run(&mut s, "ecad.pad.edit", json!({"footprint": "R0603", "pad": 2, "rotation": 90, "at": [0.8, 0]}));
    assert_eq!(r["changed"], json!(["rotation"]));
    let r = run(&mut s, "ecad.pin.edit", json!({"symbol": "R0603", "pin": "1", "name": "A", "electrical": "passive"}));
    assert_eq!((r["pin"].clone(), r["changed"].clone()), (json!("1"), json!(["name"])));
    let e = refused(&mut s, "ecad.pin.edit", json!({"symbol": "R0603", "pin": 1, "number": 2}));
    assert!(e.contains("already has a pin numbered `2`"), "{e}");
    let e = refused(&mut s, "ecad.pad.delete", json!({"footprint": "R0603", "pads": [1, 9]}));
    assert!(e.contains("pads[1]") && e.contains("no pad numbered `9`"), "{e}");
    assert_eq!(run(&mut s, "ecad.pad.delete", json!({"footprint": "R0603", "pads": [2, "2"]}))["deleted"], json!(["2"]));
    assert_eq!(component(&mut s, "R0603")["variants"][0]["unconnected_pins"], json!(["2"]));
    assert_eq!(run(&mut s, "ecad.pin.delete", json!({"symbol": "R0603", "pins": [2]}))["deleted"], json!(["2"]));
}

#[test]
fn bad_pins_and_pads_are_refused_with_their_index() {
    let mut s = Session::default();
    r0603(&mut s);
    let pin = |extra: Value| {
        let mut p = json!({"number": 9, "at": [0, 0], "direction": "up"});
        for (k, v) in extra.as_object().into_iter().flatten() {
            p[k] = v.clone();
        }
        json!({"symbol": "R0603", "pins": [{"number": 8, "at": [0, 1], "direction": "up"}, p]})
    };
    for (p, says) in [
        (pin(json!({"number": 8})), "pins[1]: the symbol already has a pin numbered `8`"),
        (pin(json!({"number": 1})), "already has a pin numbered `1`"),
        (pin(json!({"length": 0})), "pins[1]: pin 9: `length`"),
        (pin(json!({"at": [2000, 0]})), "within 1000 mm"),
        (pin(json!({"direction": "sideways"})), "`pins[1].direction`"),
        (pin(json!({"number": " 9"})), "space"),
        (pin(json!({"number": -1})), "`pins[1].number` must be a pin or pad number"),
        (json!({"symbol": "R0603", "pins": []}), "at least one pin"),
    ] {
        let e = refused(&mut s, "ecad.pin.add", p.clone());
        assert!(e.contains(says), "{p}: {e}");
    }
    for (pad, says) in [
        (json!({"number": 3, "at": [0, 0], "size": [0, 1]}), "pads[0]: pad 3: each `size` side"),
        (json!({"number": 3, "at": [0, 0], "size": [-1, 1]}), "each `size` side"),
        (json!({"number": 3, "at": [0, 0], "size": [1, 1], "rotation": 720}), "`rotation`"),
        (json!({"number": 1, "at": [0, 0], "size": [1, 1]}), "already has a pad numbered `1`"),
        (json!({"number": 3, "at": [0, 0], "size": [1]}), "`pads[0].size`"),
        (json!({"number": 3, "at": [0, 0], "size": [1, 1], "side": "inner"}), "`pads[0].side`"),
    ] {
        let e = refused(&mut s, "ecad.pad.add", json!({"footprint": "R0603", "pads": [pad]}));
        assert!(e.contains(says), "{pad}: {e}");
    }
    // A pin goes on a symbol and a pad on a footprint: the old `component` key is refused by name.
    let e = refused(&mut s, "ecad.pin.add", json!({"component": "R0603", "pins": [{"number": 7, "at": [0, 0], "direction": "up"}]}));
    assert!(e.contains("`symbol` is required"), "{e}");
    let e = refused(&mut s, "ecad.pad.add", json!({"footprint": "R0630", "pads": [{"number": 7, "at": [0, 0], "size": [1, 1]}]}));
    assert!(e.contains("R0603"), "a wrong name says what there is: {e}");
}

#[test]
fn pins_and_pads_survive_save_and_open() {
    let dir = std::env::temp_dir().join(format!("solidcraft-ecad-pads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("parts.sc");
    let mut s = Session::default();
    run(&mut s, "file.new", json!({"kind": "electronics_library"}));
    r0603(&mut s);
    run(&mut s, "ecad.pad.edit", json!({"footprint": "R0603", "pad": 2, "rotation": 30, "side": "bottom"}));
    let before = (component(&mut s, "R0603"), footprint(&mut s, "R0603"), symbol(&mut s, "R0603"));
    run(&mut s, "file.save", json!({"path": path}));
    run(&mut s, "file.new", json!({}));
    run(&mut s, "doc.open", json!({"path": path}));
    assert!(!s.is_dirty());
    assert_eq!((component(&mut s, "R0603"), footprint(&mut s, "R0603"), symbol(&mut s, "R0603")), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostile_pin_and_pad_params_never_panic() {
    let keys = [
        "symbol",
        "footprint",
        "pins",
        "pads",
        "pin",
        "pad",
        "number",
        "name",
        "name_hidden",
        "electrical",
        "at",
        "direction",
        "length",
        "size",
        "rotation",
        "side",
        "shape",
        "corner",
        "kind",
        "drill",
        "row",
        "replace",
    ];
    for id in [
        "ecad.pin.add",
        "ecad.pin.edit",
        "ecad.pin.delete",
        "ecad.pad.add",
        "ecad.pad.add.through_hole",
        "ecad.pad.add.hole",
        "ecad.pad.edit",
        "ecad.pad.delete",
    ] {
        for key in keys {
            for junk in crate::tests::hostile_values() {
                let mut s = Session::default();
                r0603(&mut s);
                run(&mut s, "ecad.schematic.place", json!({"parts": [{"component": "R0603", "at": [0, 0]}]}));
                let mut p = json!({
                    "symbol": "R0603", "footprint": "R0603", "pin": 1, "pad": 1,
                    "pins": [{"number": 5, "at": [0, 0], "direction": "up"}],
                    "pads": [{"number": 5, "at": [0, 0], "size": [1, 1]}]
                });
                p[key] = junk.clone();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(id, &p)));
                assert!(!matches!(r, Err(_) | Ok(Err(crate::EngineError::Internal(..)))), "{id} {key}={junk} panicked");
                assert!(s.doc.ecad.validate().is_ok(), "{id} {key}={junk} left a design the file would refuse");
                for item in ["pins", "pads"] {
                    if item == key {
                        continue;
                    }
                    let mut p = p.clone();
                    let Some(first) = p.get_mut(item).and_then(|l| l.get_mut(0)).and_then(Value::as_object_mut) else { continue };
                    first.insert("number".into(), json!(6));
                    first.insert(key.into(), junk.clone());
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(id, &p)));
                    assert!(!matches!(r, Err(_) | Ok(Err(crate::EngineError::Internal(..)))), "{id} {item}[0].{key}={junk} panicked");
                    assert!(s.doc.ecad.validate().is_ok(), "{id} {item}[0].{key}={junk} left a design the file would refuse");
                }
            }
        }
    }
}

#[test]
fn pad_shapes_and_holes_are_measured_exactly() {
    let mut s = Session::default();
    run(&mut s, "ecad.footprint.create", json!({"name": "SHAPES"}));
    run(
        &mut s,
        "ecad.pad.add",
        json!({"footprint": "SHAPES", "pads": [
            {"number": 1, "at": [0, 0], "size": [1, 1], "shape": "round"},
            {"number": 2, "at": [5, 0], "size": [2, 1.7], "shape": "oval"},
            {"number": 3, "at": [10, 0], "size": [1, 0.5], "shape": "rounded_rect", "corner": 0.25},
            {"number": 4, "at": [15, 0], "size": [1.7, 1.7], "shape": "round", "drill": 1.0},
            {"number": 5, "at": [20, 0], "size": [3.2, 3.2], "shape": "round", "kind": "hole", "drill": 3.2},
            {"number": 6, "at": [25, 0], "size": [2.5, 1.5], "shape": "oval", "drill": [1.6, 0.6]}
        ]}),
    );
    let f = footprint(&mut s, "SHAPES");
    let area = |n: &str| measured(&f, n)["area"].as_f64().unwrap();
    let pi = std::f64::consts::PI;
    // A circle of diameter 1: π/4.
    assert!((area("1") - pi / 4.0).abs() < 1e-12, "{}", area("1"));
    // A 2 x 1.7 oval: a circle of diameter 1.7 and a 0.3 x 1.7 rectangle between its halves.
    assert!((area("2") - (pi * 0.85 * 0.85 + 0.3 * 1.7)).abs() < 1e-12, "{}", area("2"));
    // A 1 x 0.5 rectangle with corners of radius 0.125: less (4 - π) r².
    assert!((area("3") - (0.5 - (4.0 - pi) * 0.125 * 0.125)).abs() < 1e-12, "{}", area("3"));
    // A 1.7 mm round pad with a 1 mm drill: the ring between them, 0.35 mm wide.
    assert!((area("4") - pi * (0.85 * 0.85 - 0.5 * 0.5)).abs() < 1e-12, "{}", area("4"));
    assert!((measured(&f, "4")["annular_ring"].as_f64().unwrap() - 0.35).abs() < 1e-12);
    assert_eq!(
        (f["pads"][3]["kind"].clone(), f["pads"][3]["drill"].clone()),
        (json!("through_hole"), json!([1.0, 1.0])),
        "a drill makes it through-hole"
    );
    assert_eq!(area("5"), 0.0, "a hole has no copper");
    // A 2.5 x 1.5 oval with a 1.6 x 0.6 slot: the slot is a 0.6 circle and a 1.0 x 0.6 rectangle.
    let slot = pi * 0.3 * 0.3 + 1.0 * 0.6;
    assert!((area("6") - (pi * 0.75 * 0.75 + 1.0 * 1.5 - slot)).abs() < 1e-12, "{}", area("6"));
    assert!((measured(&f, "6")["annular_ring"].as_f64().unwrap() - 0.45).abs() < 1e-12);
    // The toolbar's Through-hole Pad and Hole give a kind, and a drill that fits, when the call gives none.
    run(&mut s, "ecad.pad.add.through_hole", json!({"footprint": "SHAPES", "pads": [{"number": 7, "at": [30, 0], "size": [2, 1.5]}]}));
    run(&mut s, "ecad.pad.add.hole", json!({"footprint": "SHAPES", "pads": [{"number": 8, "at": [35, 0], "size": [2, 2], "shape": "round"}]}));
    let f = footprint(&mut s, "SHAPES");
    assert_eq!((f["pads"][6]["kind"].clone(), f["pads"][6]["drill"].clone()), (json!("through_hole"), json!([0.9, 0.9])), "0.6 of the shorter side");
    assert_eq!((f["pads"][7]["kind"].clone(), f["pads"][7]["drill"].clone()), (json!("hole"), json!([2.0, 2.0])), "a hole is its pad");
}

#[test]
fn rows_number_on_and_place_at_the_pitch() {
    let mut s = Session::default();
    run(&mut s, "ecad.footprint.create", json!({"name": "DIP8"}));
    // Two rows of four, 2.54 mm pitch and 7.62 mm apart, numbered round the package: 1-4 down the
    // left, 5-8 up the right.
    run(
        &mut s,
        "ecad.pad.add",
        json!({"footprint": "DIP8", "pads": [
            {"number": 1, "at": [-3.81, 3.81], "size": [1.6, 1.6], "shape": "round", "drill": 0.8, "row": {"count": 4, "pitch": [0, -2.54]}},
            {"number": 5, "at": [3.81, -3.81], "size": [1.6, 1.6], "shape": "round", "drill": 0.8, "row": {"count": 4, "pitch": [0, 2.54]}}
        ]}),
    );
    let pads = footprint(&mut s, "DIP8")["pads"].clone();
    let at = |n: &str| pads.as_array().unwrap().iter().find(|p| p["number"] == n).map(|p| p["at"].clone()).unwrap();
    assert!(close(&at("4"), [-3.81, -3.81]) && close(&at("8"), [3.81, 3.81]), "{pads}");
    assert_eq!(pads.as_array().map(Vec::len), Some(8));
    let e = refused(
        &mut s,
        "ecad.pad.add",
        json!({"footprint": "DIP8", "pads": [{"number": "A1", "at": [0, 0], "size": [1, 1], "row": {"count": 2, "pitch": [1, 0]}}]}),
    );
    assert!(e.contains("whole number"), "{e}");
    let e = refused(
        &mut s,
        "ecad.pad.add",
        json!({"footprint": "DIP8", "pads": [{"number": 9, "at": [0, 0], "size": [1, 1], "row": {"count": 0, "pitch": [1, 0]}}]}),
    );
    assert!(e.contains("`row.count` must be 1 to 4096"), "{e}");
    let e = refused(
        &mut s,
        "ecad.pad.add",
        json!({"footprint": "DIP8", "pads": [{"number": 8, "at": [10, 0], "size": [1, 1], "row": {"count": 2, "pitch": [1, 0]}}]}),
    );
    assert!(e.contains("numbered `8`"), "a row that meets an existing number is refused whole: {e}");
    // Pins number on in rows the same way.
    run(&mut s, "ecad.symbol.create", json!({"name": "DIP8"}));
    run(
        &mut s,
        "ecad.pin.add",
        json!({"symbol": "DIP8", "pins": [{"number": 1, "at": [-7.62, 3.81], "direction": "right", "row": {"count": 4, "pitch": [0, -2.54]}}]}),
    );
    let pins = symbol(&mut s, "DIP8")["pins"].clone();
    assert_eq!(pins.as_array().map(Vec::len), Some(4));
    assert!(close(&pins[3]["at"], [-7.62, -3.81]), "{pins}");
}

#[test]
fn impossible_pads_are_refused_with_the_reason() {
    let mut s = Session::default();
    run(&mut s, "ecad.footprint.create", json!({"name": "X"}));
    for (pad, says) in [
        (json!({"number": 1, "at": [0, 0], "size": [1, 2], "shape": "round"}), "sides must be equal"),
        (json!({"number": 1, "at": [0, 0], "size": [1, 1], "shape": "rounded_rect", "corner": 0.6}), "`corner`"),
        (json!({"number": 1, "at": [0, 0], "size": [1, 1], "corner": 0.2}), "only a rounded rectangle"),
        (json!({"number": 1, "at": [0, 0], "size": [1, 1], "drill": 1.0}), "smaller than the pad"),
        (json!({"number": 1, "at": [0, 0], "size": [1, 1], "kind": "smd", "drill": 0.5}), "no `drill`"),
        (json!({"number": 1, "at": [0, 0], "size": [1, 1], "kind": "through_hole"}), "needs a `drill`"),
        (json!({"number": 1, "at": [0, 0], "size": [2, 2], "drill": 0.01}), "at least 0.05"),
        (json!({"number": 1, "at": [0, 0], "size": [2, 2], "drill": 1, "side": "bottom"}), "goes through the board"),
        (json!({"number": 1, "at": [0, 0], "size": [2, 2], "drill": "big"}), "`pads[0].drill` must be a diameter"),
    ] {
        let e = refused(&mut s, "ecad.pad.add", json!({"footprint": "X", "pads": [pad]}));
        assert!(e.contains(says), "{pad}: {e}");
    }
}

/// The component's problems and its first variant's, but for a missing courtyard.
fn problems(s: &mut Session, name: &str) -> Vec<(String, Vec<String>)> {
    let c = component(s, name);
    let all = c["problems"].as_array().into_iter().flatten().chain(c["variants"][0]["problems"].as_array().into_iter().flatten());
    all.map(|p| (p["code"].as_str().unwrap().to_string(), p["items"].as_array().unwrap().iter().map(|i| i.as_str().unwrap().to_string()).collect()))
        .filter(|(code, _)| code != "courtyard_missing")
        .collect()
}

#[test]
fn overlapping_pads_are_found_and_fixed_from_the_reply() {
    let mut s = Session::default();
    r0603(&mut s);
    assert_eq!(component(&mut s, "R0603")["ready"], true);
    let f = footprint(&mut s, "R0603");
    assert_eq!(f["ready"], true, "{f}");
    // Copper 1.6 mm apart less two half-widths of 0.45: a 0.7 mm gap.
    assert!((f["min_pad_gap"]["gap"].as_f64().unwrap() - 0.7).abs() < 1e-12, "{f}");
    run(&mut s, "ecad.pad.edit", json!({"footprint": "R0603", "pad": 2, "at": [-0.2, 0]}));
    let f = footprint(&mut s, "R0603");
    assert_eq!(f["ready"], false);
    assert_eq!(f["problems"][0]["code"], "pads_overlap");
    assert_eq!(f["problems"][0]["items"], json!(["1", "2"]));
    assert_eq!(component(&mut s, "R0603")["ready"], false, "a component is ready only when its footprint is");
    // Pad 1 spans x -1.25..-0.35, pad 2 -0.65..0.25: 0.3 mm deep.
    assert!((f["min_pad_gap"]["gap"].as_f64().unwrap() + 0.3).abs() < 1e-12, "{f}");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(footprint(&mut s, "R0603")["ready"], true);
}

#[test]
fn sot23_gaps_follow_the_package_drawing() {
    // JEDEC TO-236 (SOT-23): leads 0.95 mm apart, rows 2.2 mm apart; 0.6 x 0.8 mm pads.
    let mut s = Session::default();
    run(&mut s, "ecad.footprint.create", json!({"name": "SOT23"}));
    run(
        &mut s,
        "ecad.pad.add",
        json!({"footprint": "SOT23", "pads": [
            {"number": 1, "at": [-0.95, -1.1], "size": [0.6, 0.8]},
            {"number": 2, "at": [0.95, -1.1], "size": [0.6, 0.8]},
            {"number": 3, "at": [0, 1.1], "size": [0.6, 0.8]}
        ]}),
    );
    let d = |a: &str, b: &str| {
        let pads = [
            solidcraft_doc::ecad::Pad { number: "1".into(), at: [-0.95, -1.1], size: [0.6, 0.8], ..pad() },
            solidcraft_doc::ecad::Pad { number: "2".into(), at: [0.95, -1.1], size: [0.6, 0.8], ..pad() },
            solidcraft_doc::ecad::Pad { number: "3".into(), at: [0.0, 1.1], size: [0.6, 0.8], ..pad() },
        ];
        let find = |n: &str| pads.iter().find(|p| p.number == n).unwrap().core();
        solidcraft_doc::ecad::checks::gap(&find(a), &find(b))
    };
    // Side by side: 1.9 apart less 0.6.
    assert!((d("1", "2") - 1.3).abs() < 1e-12);
    // Two round pads on the same centre overlap by both radii.
    let round =
        solidcraft_doc::ecad::Pad { number: "4".into(), at: [5.0, 5.0], size: [1.0, 1.0], shape: solidcraft_doc::ecad::PadShape::Round, ..pad() };
    assert!((solidcraft_doc::ecad::checks::gap(&round.core(), &round.core()) + 1.0).abs() < 1e-12);
    // Corner to corner: 0.35 across and 1.4 up.
    assert!((d("1", "3") - (0.35f64).hypot(1.4)).abs() < 1e-12, "{}", d("1", "3"));
    let f = footprint(&mut s, "SOT23");
    assert!((f["min_pad_gap"]["gap"].as_f64().unwrap() - 1.3).abs() < 1e-12, "{f}");
}

fn pad() -> solidcraft_doc::ecad::Pad {
    solidcraft_doc::ecad::Pad {
        number: String::new(),
        at: [0.0, 0.0],
        size: [1.0, 1.0],
        rotation: 0.0,
        side: Default::default(),
        shape: Default::default(),
        corner: 0.0,
        kind: Default::default(),
        drill: None,
    }
}

#[test]
fn round_and_rotated_pads_measure_their_true_gap() {
    let gap = |a: solidcraft_doc::ecad::Pad, b: solidcraft_doc::ecad::Pad| solidcraft_doc::ecad::checks::gap(&a.core(), &b.core());
    let round = |x: f64, d: f64| solidcraft_doc::ecad::Pad { at: [x, 0.0], size: [d, d], shape: solidcraft_doc::ecad::PadShape::Round, ..pad() };
    // Two 1 mm circles with centres 3 mm apart: 2 mm of gap.
    assert!((gap(round(0.0, 1.0), round(3.0, 1.0)) - 2.0).abs() < 1e-12);
    // A 1 x 1 square turned 45° reaches √2/2 from its centre.
    let diamond = solidcraft_doc::ecad::Pad { at: [0.0, 0.0], rotation: 45.0, ..pad() };
    assert!((gap(diamond, round(2.0, 1.0)) - (2.0 - 0.5 - std::f64::consts::FRAC_1_SQRT_2)).abs() < 1e-12);
    // Two ovals end to end along x, 0.5 mm apart.
    let oval = |x: f64| solidcraft_doc::ecad::Pad { at: [x, 0.0], size: [2.0, 1.0], shape: solidcraft_doc::ecad::PadShape::Oval, ..pad() };
    assert!((gap(oval(0.0), oval(2.5)) - 0.5).abs() < 1e-12);
}

#[test]
fn a_tab_maps_one_pin_to_two_pads() {
    // A regulator whose middle pin is also the tab: pin 2 to pads 2 and 4.
    let mut s = Session::default();
    empty(&mut s, "REG", "U");
    run(
        &mut s,
        "ecad.pin.add",
        json!({"symbol": "REG", "pins": [{"number": 1, "at": [-7.62, 0], "direction": "right", "row": {"count": 3, "pitch": [0, -2.54]}}]}),
    );
    run(
        &mut s,
        "ecad.pad.add",
        json!({"footprint": "REG", "pads": [
            {"number": 1, "at": [-2.3, -3.15], "size": [1.0, 1.8], "row": {"count": 3, "pitch": [2.3, 0]}},
            {"number": 4, "at": [0, 3.15], "size": [3.3, 1.8]}
        ]}),
    );
    assert_eq!(problems(&mut s, "REG"), vec![("pad_unconnected".to_string(), vec!["4".to_string()])]);
    let r = run(&mut s, "ecad.component.map_pins", json!({"component": "REG", "map": {"1": [1], "2": [2, 4], "3": [3]}}));
    assert_eq!(r["variant"], "REG");
    assert_eq!(r["connections"][1], json!({"pin": "2", "pads": ["2", "4"]}));
    assert_eq!((r["unconnected_pins"].clone(), r["unconnected_pads"].clone()), (json!([]), json!([])));
    assert!(problems(&mut s, "REG").is_empty());
    // Renumbering and deleting keep the map in step.
    run(&mut s, "ecad.pad.edit", json!({"footprint": "REG", "pad": 4, "number": "TAB"}));
    let v = component(&mut s, "REG")["variants"][0].clone();
    assert_eq!(v["pin_map"]["2"], json!(["2", "TAB"]));
    run(&mut s, "ecad.pin.delete", json!({"symbol": "REG", "pins": [3]}));
    let v = component(&mut s, "REG")["variants"][0].clone();
    assert!(v["pin_map"].get("3").is_none() && v["unconnected_pads"] == json!(["3"]), "{v}");
    for (map, says) in [(json!({"9": [1]}), "doesn't have"), (json!({"1": [9]}), "doesn't have"), (json!({"1": [1], "2": [1]}), "mapped to both")] {
        let e = refused(&mut s, "ecad.component.map_pins", json!({"component": "REG", "map": map}));
        assert!(e.contains(says), "{map}: {e}");
    }
    let e = refused(&mut s, "ecad.component.map_pins", json!({"component": "REG", "variant": "SOT-89", "by_number": true}));
    assert!(e.contains("SOT-89"), "{e}");
    run(&mut s, "ecad.component.map_pins", json!({"component": "REG", "by_number": true}));
    assert_eq!(component(&mut s, "REG")["variants"][0]["pin_map"], json!(null));
}
