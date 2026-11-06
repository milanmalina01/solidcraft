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

/// SOT-23 as `ecad.footprint.generate` draws it (see the electronics skill's reference), as a
/// table: 1 at [-0.95, -0.975], 2 at [0.95, -0.975], 3 at [0, 0.975], each 0.5 x 1.05 mm.
const SOT23_TABLE: [(&str, [f64; 2]); 3] = [("1", [-0.95, -0.975]), ("2", [0.95, -0.975]), ("3", [0.0, 0.975])];
const SOT23_PAD: [f64; 2] = [0.5, 1.05];

fn sot23(s: &mut Session) {
    run(s, "ecad.footprint.generate", json!({"footprint": "SOT-23", "package": "sot23"}));
}

/// The table with `f` done to each pad's centre and size.
fn sot23_table(f: impl Fn([f64; 2], [f64; 2]) -> ([f64; 2], [f64; 2])) -> Value {
    let pads: Vec<Value> = SOT23_TABLE
        .iter()
        .map(|(n, at)| {
            let (at, size) = f(*at, SOT23_PAD);
            json!({"number": n, "at": at, "size": size})
        })
        .collect();
    json!(pads)
}

/// Each pad of a footprint as `ecad.footprint.inspect` gives it: number, centre, size.
fn footprint_pads(s: &mut Session, footprint: &str) -> Vec<(String, [f64; 2], [f64; 2])> {
    let r = run(s, "ecad.footprint.inspect", json!({"footprint": footprint}));
    let xy = |v: &Value| [v[0].as_f64().unwrap(), v[1].as_f64().unwrap()];
    r["footprint"]["pads"].as_array().unwrap().iter().map(|p| (p["number"].as_str().unwrap().to_string(), xy(&p["at"]), xy(&p["size"]))).collect()
}

fn verify(s: &mut Session, footprint: &str, pads: Value) -> Value {
    run(s, "ecad.footprint.verify", json!({"footprint": footprint, "pads": pads}))
}

#[test]
fn verify_passes_an_exact_table_and_changes_nothing() {
    let mut s = Session::default();
    sot23(&mut s);
    let (before, undo) = (s.doc.clone(), s.undo.len());
    let r = verify(&mut s, "SOT-23", sot23_table(|at, size| (at, size)));
    assert!(Arc::ptr_eq(&before, &s.doc) && s.undo.len() == undo, "read-only");
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["footprint"]["name"], "SOT-23");
    assert_eq!((r["footprint_pads"].clone(), r["table_pads"].clone(), r["tolerance"].clone()), (json!(3), json!(3), json!(0.05)));
    assert_eq!((r["offset"].clone(), r["worst"].clone(), r["orientation"].clone()), (json!([0.0, 0.0]), json!(0.0), Value::Null));
    assert_eq!(
        (r["missing"].clone(), r["extra"].clone(), r["mismatches"].clone(), r["problems"].clone()),
        (json!([]), json!([]), json!([]), json!([]))
    );
}

#[test]
fn verify_names_a_moved_pad_and_a_wrong_size() {
    let mut s = Session::default();
    sot23(&mut s);
    // Pad 3 is 0.12 mm too high in the table. The mean moves 0.04 up, so the offset is -0.04, pad 3
    // differs by 0.08 and pads 1 and 2 by 0.04, inside the tolerance.
    let moved = |at: [f64; 2], size| (if at[0] == 0.0 { [at[0], at[1] + 0.12] } else { at }, size);
    let r = verify(&mut s, "SOT-23", sot23_table(moved));
    assert_eq!(r["ok"], false);
    assert_eq!(r["offset"], json!([0.0, -0.04]));
    assert_eq!(r["worst"], json!(0.08));
    assert_eq!(r["mismatches"], json!([{"number": "3", "field": "at", "table": [0.0, 1.055], "footprint": [0.0, 0.975], "difference": 0.08}]));
    assert_eq!(r["orientation"], Value::Null, "no mirror or turn explains a moved pad");
    // A tighter tolerance names the other two; a looser one passes, and worst still says how far.
    let tight = run(&mut s, "ecad.footprint.verify", json!({"footprint": "SOT-23", "pads": sot23_table(moved), "tolerance": 0.01}));
    assert_eq!(tight["mismatches"].as_array().unwrap().len(), 3);
    let loose = run(&mut s, "ecad.footprint.verify", json!({"footprint": "SOT-23", "pads": sot23_table(moved), "tolerance": 0.1}));
    assert_eq!((loose["ok"].clone(), loose["worst"].clone()), (json!(true), json!(0.08)));

    let r = verify(&mut s, "SOT-23", sot23_table(|at, [w, h]| (at, [w + 0.2, h])));
    assert_eq!(r["ok"], false);
    let m = &r["mismatches"][0];
    assert_eq!((m["field"].clone(), m["table"].clone(), m["footprint"].clone()), (json!("size"), json!([0.7, 1.05]), json!([0.5, 1.05])));
    assert_eq!((m["difference"].clone(), r["worst"].clone()), (json!(0.2), json!(0.2)));
}

#[test]
fn verify_reports_missing_and_extra_pads_without_moving_the_rest() {
    let mut s = Session::default();
    sot23(&mut s);
    // A pad in the table only: the rest still line up, whatever its position.
    let mut pads = sot23_table(|at, size| (at, size));
    pads.as_array_mut().unwrap().push(json!({"number": "EP", "at": [40, 40], "size": [1, 1]}));
    let r = verify(&mut s, "SOT-23", pads);
    assert_eq!((r["ok"].clone(), r["missing"].clone(), r["extra"].clone()), (json!(false), json!(["EP"]), json!([])));
    assert_eq!((r["mismatches"].clone(), r["offset"].clone(), r["table_pads"].clone()), (json!([]), json!([0.0, 0.0]), json!(4)));
    // A pad in the footprint only: likewise.
    let mut pads = sot23_table(|at, size| (at, size));
    pads.as_array_mut().unwrap().pop();
    let r = verify(&mut s, "SOT-23", pads);
    assert_eq!((r["ok"].clone(), r["missing"].clone(), r["extra"].clone()), (json!(false), json!([]), json!(["3"])));
    assert_eq!((r["mismatches"].clone(), r["offset"].clone(), r["footprint_pads"].clone()), (json!([]), json!([0.0, 0.0]), json!(3)));
    // Nothing in common: nothing to compare, and no number that looks like a pass.
    let r = verify(&mut s, "SOT-23", json!([{"number": "X", "at": [0, 0], "size": [1, 1]}]));
    assert_eq!((r["ok"].clone(), r["offset"].clone(), r["worst"].clone()), (json!(false), Value::Null, Value::Null));
}

#[test]
fn verify_names_a_mirror_or_a_turn() {
    let mut s = Session::default();
    sot23(&mut s);
    // Each table is the footprint with the opposite done to it, so `names` undoes it.
    type Drawn = fn([f64; 2], [f64; 2]) -> ([f64; 2], [f64; 2]);
    let tables: [(&str, Drawn); 5] = [
        ("mirror_x", |[x, y], s| ([-x, y], s)),
        ("mirror_y", |[x, y], s| ([x, -y], s)),
        ("rotate_90", |[x, y], [w, h]| ([y, -x], [h, w])),
        ("rotate_180", |[x, y], s| ([-x, -y], s)),
        ("rotate_270", |[x, y], [w, h]| ([-y, x], [h, w])),
    ];
    for (names, drawn) in tables {
        let r = verify(&mut s, "SOT-23", sot23_table(drawn));
        assert_eq!((r["ok"].clone(), r["orientation"].clone()), (json!(false), json!(names)), "{r}");
        assert!(!r["mismatches"].as_array().unwrap().is_empty(), "mismatches are as the table gives them: {r}");
    }
    // A mirror that also has a wrong pad size is not a mirror.
    let r = verify(&mut s, "SOT-23", sot23_table(|[x, y], [w, h]| ([-x, y], [w + 0.2, h])));
    assert_eq!(r["orientation"], Value::Null, "{r}");
    // A mirror measured from elsewhere is still a mirror.
    let r = verify(&mut s, "SOT-23", sot23_table(|[x, y], size| ([5.0 - x, y + 7.0], size)));
    assert_eq!((r["orientation"].clone(), r.get("orientation_also")), (json!("mirror_x"), None), "{r}");
    // One pad in common says nothing about which way round the table is, even with its sides swapped.
    let r = verify(&mut s, "SOT-23", json!([{"number": 1, "at": [0, 0], "size": [1.05, 0.5]}]));
    assert_eq!((r["ok"].clone(), r["orientation"].clone()), (json!(false), Value::Null), "{r}");
    // A row reversed is a mirror or a half turn, and the reply doesn't pretend to know which.
    run(&mut s, "ecad.footprint.create", json!({"name": "ROW"}));
    run(
        &mut s,
        "ecad.pad.add",
        json!({"footprint": "ROW", "pads": [{"number": 1, "at": [-3, 0], "size": [1, 2], "row": {"count": 4, "pitch": [2, 0]}}]}),
    );
    let row: Vec<Value> =
        footprint_pads(&mut s, "ROW").into_iter().map(|(n, [x, y], size)| json!({"number": n, "at": [-x, y], "size": size})).collect();
    let r = verify(&mut s, "ROW", json!(row));
    assert_eq!((r["orientation"].clone(), r["orientation_also"].clone()), (json!("mirror_x"), json!(["rotate_180"])), "{r}");
}

#[test]
fn verify_moves_a_table_measured_from_elsewhere() {
    let mut s = Session::default();
    sot23(&mut s);
    let r = verify(&mut s, "SOT-23", sot23_table(|[x, y], size| ([x + 10.0, y - 3.0], size)));
    assert_eq!((r["ok"].clone(), r["offset"].clone(), r["worst"].clone()), (json!(true), json!([-10.0, 3.0]), json!(0.0)), "{r}");
}

#[test]
fn verify_compares_a_turned_pad_by_its_turned_extent() {
    let mut s = Session::default();
    run(&mut s, "ecad.footprint.create", json!({"name": "T"}));
    run(
        &mut s,
        "ecad.pad.add",
        json!({"footprint": "T", "pads": [
            {"number": 1, "at": [-3, 0], "size": [1, 2], "rotation": 90},
            {"number": 2, "at": [3, 0], "size": [1, 2], "rotation": 45},
            {"number": 3, "at": [0, 3], "size": [1, 2], "shape": "oval", "rotation": 90}
        ]}),
    );
    // Turned 90°: 2 x 1. Turned 45°: (1 + 2) sin 45° = 2.12132 each way. An oval's ends stay inside.
    let table = |second: [f64; 2]| {
        json!([
            {"number": 1, "at": [-3, 0], "size": [2, 1]},
            {"number": 2, "at": [3, 0], "size": second},
            {"number": 3, "at": [0, 3], "size": [2, 1]}
        ])
    };
    let r = verify(&mut s, "T", table([2.12132, 2.12132]));
    assert_eq!(r["mismatches"], json!([]), "{r}");
    assert!(r["worst"].as_f64().unwrap() < 1e-5, "{r}");
    let r = verify(&mut s, "T", table([1.0, 2.0]));
    assert_eq!(r["mismatches"][0]["number"], "2");
    assert_eq!(r["mismatches"][0]["footprint"], json!([2.12132, 2.12132]), "{r}");
    // The unturned size of a pad the footprint turns 90° is the wrong way round.
    let r = verify(&mut s, "T", json!([{"number": 1, "at": [-3, 0], "size": [1, 2]}]));
    assert_eq!((r["mismatches"][0]["field"].clone(), r["mismatches"][0]["difference"].clone()), (json!("size"), json!(1.0)));
}

#[test]
fn verify_reports_what_inspect_knows_and_the_body() {
    let mut s = Session::default();
    run(&mut s, "ecad.footprint.create", json!({"name": "F"}));
    let pads = json!([
        {"number": 1, "at": [-2, 0], "size": [1, 1]},
        {"number": 2, "at": [2, 0], "size": [1, 1]},
        {"number": 3, "at": [0, 2], "size": [1, 1]}
    ]);
    run(&mut s, "ecad.pad.add", json!({"footprint": "F", "pads": pads}));
    let codes = |r: &Value| r["problems"].as_array().unwrap().iter().map(|p| p["code"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    // Warnings are listed and leave `ok` alone: no courtyard, and three pads with no silkscreen.
    let r = verify(&mut s, "F", pads.clone());
    assert_eq!((r["ok"].clone(), codes(&r)), (json!(true), vec!["courtyard_missing".to_string(), "no_pin_1_mark".to_string()]), "{r}");
    run(&mut s, "ecad.draw.circle", json!({"footprint": "F", "layer": "silkscreen", "center": [-3, 1], "radius": 0.2, "fill": true}));
    assert_eq!(codes(&verify(&mut s, "F", pads.clone())), ["courtyard_missing"]);
    // A courtyard that doesn't hold a pad is an error, even when every value matches.
    run(&mut s, "ecad.draw.rect", json!({"footprint": "F", "layer": "courtyard", "from": [-1, -1], "to": [1, 1]}));
    let r = verify(&mut s, "F", pads.clone());
    assert_eq!((r["ok"].clone(), r["problems"][0]["code"].clone()), (json!(false), json!("pad_outside_courtyard")), "{r}");
    run(&mut s, "ecad.footprint.courtyard", json!({"footprint": "F"}));
    assert_eq!(verify(&mut s, "F", pads.clone())["ok"], true);
    // A body with no outline to compare it with is not a pass; with one, it's compared.
    let with_body = |s: &mut Session, body: [f64; 2]| run(s, "ecad.footprint.verify", json!({"footprint": "F", "pads": pads, "body": body}));
    let r = with_body(&mut s, [3.0, 2.0]);
    assert_eq!(
        (r["ok"].clone(), r["body"]["footprint"].clone(), r["problems"][0]["code"].clone()),
        (json!(false), Value::Null, json!("body_not_checked"))
    );
    run(&mut s, "ecad.draw.rect", json!({"footprint": "F", "layer": "fab", "from": [-1.5, -1], "to": [1.5, 1]}));
    assert_eq!(with_body(&mut s, [3.0, 2.0])["ok"], true);
    let r = with_body(&mut s, [3.2, 2.0]);
    assert_eq!(
        (r["ok"].clone(), r["body"]["footprint"].clone(), r["body"]["difference"].clone()),
        (json!(false), json!([3.0, 2.0]), json!(0.2)),
        "{r}"
    );
    // Overlapping pads are an error too.
    run(&mut s, "ecad.pad.edit", json!({"footprint": "F", "pad": 3, "at": [2, 0.5]}));
    let moved = json!([
        {"number": 1, "at": [-2, 0], "size": [1, 1]},
        {"number": 2, "at": [2, 0], "size": [1, 1]},
        {"number": 3, "at": [2, 0.5], "size": [1, 1]}
    ]);
    let r = verify(&mut s, "F", moved);
    assert_eq!((r["ok"].clone(), r["mismatches"].clone(), r["problems"][0]["code"].clone()), (json!(false), json!([]), json!("pads_overlap")), "{r}");
}

#[test]
fn verify_says_what_a_table_cannot_check() {
    let mut s = Session::default();
    // A generated body is the box of its fab outline, chamfer and all: SOT-23 is 2.9 x 1.3 mm.
    sot23(&mut s);
    let r = run(&mut s, "ecad.footprint.verify", json!({"footprint": "SOT-23", "pads": sot23_table(|at, size| (at, size)), "body": [2.9, 1.3]}));
    assert_eq!((r["ok"].clone(), r["body"]["difference"].clone()), (json!(true), json!(0.0)), "{r}");
    // A through-hole pad, a bare hole and a bottom pad match, and the reply names them as not fully checked.
    run(&mut s, "ecad.footprint.create", json!({"name": "D"}));
    let pads = json!([
        {"number": 1, "at": [-2, 0], "size": [1.7, 1.7], "kind": "through_hole", "drill": 1.0},
        {"number": 2, "at": [2, 0], "size": [1, 1]},
        {"number": 3, "at": [0, 3], "size": [1, 1], "side": "bottom"},
        {"number": "MH", "at": [0, -3], "size": [2, 2], "shape": "round", "kind": "hole", "drill": 2}
    ]);
    run(&mut s, "ecad.pad.add", json!({"footprint": "D", "pads": pads}));
    run(&mut s, "ecad.footprint.courtyard", json!({"footprint": "D"}));
    let table: Vec<Value> = footprint_pads(&mut s, "D").into_iter().map(|(n, at, size)| json!({"number": n, "at": at, "size": size})).collect();
    let r = verify(&mut s, "D", json!(table));
    let not_compared = r["problems"].as_array().unwrap().iter().find(|p| p["code"] == "not_compared").cloned().unwrap_or(Value::Null);
    assert_eq!((r["ok"].clone(), not_compared["items"].clone()), (json!(true), json!(["1", "3", "MH"])), "{r}");
    assert_eq!(not_compared["severity"], "warning", "{r}");
}

#[test]
fn verify_refuses_hostile_tables() {
    let mut s = Session::default();
    sot23(&mut s);
    let good = json!([{"number": 1, "at": [0, 0], "size": [1, 1]}]);
    let huge: Vec<Value> = (0..4097).map(|i| json!({"number": i, "at": [0, 0], "size": [1, 1]})).collect();
    let cases = [
        (json!({"footprint": "NOPE", "pads": good}), "NOPE"),
        (json!({"footprint": "SOT-23", "pads": []}), "give 1 to 4096"),
        (json!({"footprint": "SOT-23", "pads": huge}), "give 1 to 4096"),
        (json!({"footprint": "SOT-23", "pads": good, "tolerance": -1}), "`tolerance`"),
        (json!({"footprint": "SOT-23", "pads": good, "tolerance": 1e308}), "`tolerance`"),
        (json!({"footprint": "SOT-23", "pads": good, "body": [0, 1]}), "`body`"),
        (json!({"footprint": "SOT-23", "pads": good, "body": [1e308, 1]}), "`body`"),
        (json!({"footprint": "SOT-23", "pads": [{"number": 1, "at": [1e308, 0], "size": [1, 1]}]}), "pads[0]: `at`"),
        (json!({"footprint": "SOT-23", "pads": [{"number": 1, "at": [f64::NAN, 0], "size": [1, 1]}]}), "pads[0].at"),
        (json!({"footprint": "SOT-23", "pads": [{"number": 1, "at": [0, 0], "size": [-1, 1]}]}), "pads[0]: each `size`"),
        (json!({"footprint": "SOT-23", "pads": [{"number": 1, "at": [0, 0], "size": [1e308, 1]}]}), "pads[0]: each `size`"),
        (json!({"footprint": "SOT-23", "pads": [{"number": "", "at": [0, 0], "size": [1, 1]}]}), "pads[0]"),
        (json!({"footprint": "SOT-23", "pads": [good[0], good[0]]}), "pads[1]: the table already has"),
    ];
    for (p, says) in cases {
        let e = refused(&mut s, "ecad.footprint.verify", p.clone());
        assert!(e.contains(says), "{p}: {e}");
    }
}
