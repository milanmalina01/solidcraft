use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn refused(s: &mut Session, id: &str, p: Value) -> String {
    let before = s.doc.clone();
    let Err(e) = s.execute(id, &p) else { panic!("{id} {p} must be refused") };
    let e = e.to_string();
    assert!(Arc::ptr_eq(&before, &s.doc) || *before == *s.doc, "a refused {id} leaves the design as it was");
    e
}

fn resistor(s: &mut Session) -> Value {
    run(
        s,
        "ecad.component.create",
        json!({"name": "R0603", "prefix": "R", "value": "10k", "mpn": "RC0603FR-0710KL", "manufacturer": "Example Resistors", "description": "Thick film resistor, 0603, 1%"}),
    )
}

#[test]
fn an_electronic_component_is_made_read_and_listed() {
    let mut s = Session::default();
    assert_eq!(resistor(&mut s), json!({"component": {"id": 1, "name": "R0603"}}));
    assert_eq!(
        run(&mut s, "ecad.component.create", json!({"name": "LM358"})),
        json!({"component": {"id": 2, "name": "LM358"}}),
        "the prefix defaults to U"
    );
    let full = run(&mut s, "ecad.component.inspect", json!({"component": "R0603"}));
    let c = &full["component"];
    for (key, want) in [
        ("id", json!(1)),
        ("name", json!("R0603")),
        ("prefix", json!("R")),
        ("value", json!("10k")),
        ("mpn", json!("RC0603FR-0710KL")),
        ("manufacturer", json!("Example Resistors")),
        ("description", json!("Thick film resistor, 0603, 1%")),
        ("pins", json!([])),
        ("pads", json!([])),
        ("pin_map", json!(null)),
        ("ready", json!(false)),
    ] {
        assert_eq!(c[key], want, "{key}");
    }
    let codes: Vec<&str> = c["problems"].as_array().unwrap().iter().filter_map(|p| p["code"].as_str()).collect();
    assert_eq!(codes, ["no_pins", "no_pads"], "a new component says what it still needs");
    assert_eq!(run(&mut s, "ecad.component.inspect", json!({"component": 1})), full, "by id or by name");
    assert_eq!(run(&mut s, "ecad.component.inspect", json!({"component": 2}))["component"]["prefix"], "U");
    assert_eq!(
        run(&mut s, "ecad.inspect", json!({})),
        json!({"empty": false, "components": [
            {"id": 1, "name": "R0603", "prefix": "R", "value": "10k", "pins": 0, "pads": 0, "ready": false},
            {"id": 2, "name": "LM358", "prefix": "U", "value": "", "pins": 0, "pads": 0, "ready": false}
        ]})
    );
}

#[test]
fn edits_change_only_what_is_given_and_report_it() {
    let mut s = Session::default();
    resistor(&mut s);
    let undo = s.undo.len();
    let r = run(&mut s, "ecad.component.edit", json!({"component": "R0603", "value": "4k7", "mpn": "", "prefix": "R"}));
    assert_eq!(r, json!({"component": {"id": 1, "name": "R0603"}, "changed": ["value", "mpn"]}));
    let c = run(&mut s, "ecad.component.inspect", json!({"component": 1}))["component"].clone();
    assert_eq!((c["value"].clone(), c["mpn"].clone(), c["manufacturer"].clone()), (json!("4k7"), json!(""), json!("Example Resistors")));
    assert_eq!(s.undo.len(), undo + 1, "one edit is one undo step");
    let before = s.doc.clone();
    let r = run(&mut s, "ecad.component.edit", json!({"component": 1, "value": "4k7"}));
    assert_eq!(r["changed"], json!([]));
    assert!(Arc::ptr_eq(&before, &s.doc), "an edit that changes nothing leaves the design and the undo history alone");
    assert_eq!(s.undo.len(), undo + 1);
    run(&mut s, "ecad.component.edit", json!({"component": "R0603", "name": "R0805"}));
    assert_eq!(run(&mut s, "ecad.component.inspect", json!({"component": 1}))["component"]["name"], "R0805", "the id survives a rename");
}

#[test]
fn copies_get_a_new_id_and_deletes_take_all_or_none() {
    let mut s = Session::default();
    resistor(&mut s);
    assert_eq!(
        run(&mut s, "ecad.component.copy", json!({"component": "R0603", "name": "R0603 1k"})),
        json!({"component": {"id": 2, "name": "R0603 1k"}})
    );
    assert_eq!(run(&mut s, "ecad.component.inspect", json!({"component": 2}))["component"]["mpn"], "RC0603FR-0710KL");
    let e = refused(&mut s, "ecad.component.delete", json!({"components": ["R0603", "R9999"]}));
    assert!(e.contains("R9999"), "{e}");
    assert_eq!(
        run(&mut s, "ecad.component.delete", json!({"components": [2, "R0603", 1]})),
        json!({"deleted": [{"id": 1, "name": "R0603"}, {"id": 2, "name": "R0603 1k"}]})
    );
    assert_eq!(run(&mut s, "ecad.component.create", json!({"name": "R0603"}))["component"]["id"], 3, "ids are never reused");
    assert_eq!(run(&mut s, "ecad.component.delete", json!({"components": [3]}))["deleted"][0]["id"], 3);
    assert_eq!(run(&mut s, "ecad.inspect", json!({})), json!({"empty": true}), "the id counter alone is no electronics");
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(run(&mut s, "ecad.inspect", json!({}))["components"].as_array().map(Vec::len), Some(2), "undo brings both back");
}

#[test]
fn wrong_names_say_what_there_is() {
    let mut s = Session::default();
    resistor(&mut s);
    let e = refused(&mut s, "ecad.component.inspect", json!({"component": "R0630"}));
    assert!(e.contains("did you mean `R0603`"), "{e}");
    let e = refused(&mut s, "ecad.component.inspect", json!({"component": 9}));
    assert!(e.contains("no electronic component has the id 9"), "{e}");
    run(&mut s, "component.create", json!({"name": "Enclosure"}));
    let e = refused(&mut s, "ecad.component.edit", json!({"component": "Enclosure", "value": "1"}));
    assert!(e.contains("is a SolidCraft component"), "{e}");
    for (p, says) in [
        (json!({"name": "R0603"}), "already named"),
        (json!({"name": " R1"}), "space"),
        (json!({"name": ""}), "needs a name"),
        (json!({"name": "R1", "prefix": "R1"}), "letters"),
        (json!({"name": "R1", "value": "x".repeat(65)}), "longer than 64"),
        (json!({"name": "R1", "description": "line\nbreak"}), "line break"),
        (json!({"prefix": "R"}), "name"),
        (json!({"name": 7}), "`name` must be text"),
    ] {
        let e = refused(&mut s, "ecad.component.create", p.clone());
        assert!(e.contains(says), "{p}: {e}");
    }
    let e = refused(&mut s, "ecad.component.edit", json!({"component": [1], "value": "1"}));
    assert!(e.contains("`component` must be an electronic component's id or name"), "{e}");
    let e = refused(&mut s, "ecad.component.delete", json!({"components": ["R0603", true]}));
    assert!(e.contains("`components[1]` must be"), "{e}");
    let e = refused(&mut s, "ecad.component.delete", json!({"components": []}));
    assert!(e.contains("at least one"), "{e}");
}

#[test]
fn electronics_edits_never_evaluate_the_model() {
    let mut s = Session::default();
    run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
    let evaluations = s.evaluations;
    resistor(&mut s);
    run(&mut s, "ecad.component.edit", json!({"component": "R0603", "value": "1k"}));
    run(&mut s, "ecad.component.copy", json!({"component": "R0603", "name": "R2"}));
    run(&mut s, "ecad.component.delete", json!({"components": ["R2"]}));
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.evaluations, evaluations, "only the electronics changed: nothing is evaluated again");
    assert_eq!(s.world_state().bodies.len(), 1, "the model is still there");
}

#[test]
fn electronic_components_survive_save_and_open() {
    let dir = std::env::temp_dir().join(format!("solidcraft-ecad-components-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("board.sc");
    let mut s = Session::default();
    resistor(&mut s);
    run(&mut s, "ecad.component.create", json!({"name": "LM358", "value": "dual op-amp"}));
    run(&mut s, "ecad.component.create", json!({"name": "TP1", "prefix": "TP"}));
    // Ids 1 and 2 are left, and the counter is at 4: opening must not hand out 3 again.
    run(&mut s, "ecad.component.delete", json!({"components": ["TP1"]}));
    let before = run(&mut s, "ecad.inspect", json!({}));
    let r0603 = run(&mut s, "ecad.component.inspect", json!({"component": "R0603"}));
    run(&mut s, "file.save", json!({"path": path}));
    run(&mut s, "file.new", json!({}));
    assert_eq!(run(&mut s, "ecad.inspect", json!({})), json!({"empty": true}));
    run(&mut s, "doc.open", json!({"path": path}));
    assert!(!s.is_dirty());
    assert_eq!(run(&mut s, "ecad.inspect", json!({})), before);
    assert_eq!(run(&mut s, "ecad.component.inspect", json!({"component": "R0603"})), r0603);
    assert_eq!(run(&mut s, "ecad.component.create", json!({"name": "C0603", "prefix": "C"}))["component"]["id"], 4, "the id counter is saved too");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hostile_electronic_component_params_never_panic() {
    let keys = ["component", "components", "name", "prefix", "value", "mpn", "manufacturer", "description"];
    for id in ["ecad.component.create", "ecad.component.edit", "ecad.component.copy", "ecad.component.delete", "ecad.component.inspect"] {
        for key in keys {
            for junk in crate::tests::hostile_values() {
                let mut s = Session::default();
                resistor(&mut s);
                let base = json!({"component": "R0603", "components": ["R0603"], "name": "R2"});
                let mut p = base.clone();
                p[key] = junk.clone();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(id, &p)));
                assert!(!matches!(r, Err(_) | Ok(Err(crate::EngineError::Internal(..)))), "{id} {key}={junk} panicked");
                assert!(s.doc.ecad.validate().is_ok(), "{id} {key}={junk} left a design the file would refuse");
            }
        }
    }
}

#[test]
fn a_hole_connects_no_pin() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "J", "prefix": "J"}));
    run(&mut s, "ecad.pin.add", json!({"component": "J", "pins": [{"number": 1, "at": [0, 0], "direction": "right"}]}));
    run(
        &mut s,
        "ecad.pad.add",
        json!({"component": "J", "pads": [{"number": 1, "at": [0, 0], "size": [2, 2], "shape": "round", "kind": "hole", "drill": 2}]}),
    );
    let c = run(&mut s, "ecad.component.inspect", json!({"component": "J"}))["component"].clone();
    assert_eq!(c["unconnected_pins"], json!(["1"]), "a pin numbered like a hole is not connected to it");
    assert_eq!(c["ready"], false);
    let e = refused(&mut s, "ecad.component.map_pins", json!({"component": "J", "map": {"1": [1]}}));
    assert!(e.contains("unplated hole"), "{e}");
}

#[test]
fn replacing_the_pins_drops_a_map_that_named_them() {
    let mut s = Session::default();
    resistor(&mut s);
    run(
        &mut s,
        "ecad.pin.add",
        json!({"component": "R0603", "pins": [{"number": 1, "at": [-5.08, 0], "direction": "right", "row": {"count": 2, "pitch": [10.16, 0]}}]}),
    );
    run(
        &mut s,
        "ecad.pad.add",
        json!({"component": "R0603", "pads": [{"number": 1, "at": [-0.8, 0], "size": [0.9, 1], "row": {"count": 2, "pitch": [1.6, 0]}}]}),
    );
    run(&mut s, "ecad.component.map_pins", json!({"component": "R0603", "map": {"1": [2], "2": [1]}}));
    let r = run(&mut s, "ecad.pin.add", json!({"component": "R0603", "replace": true, "pins": [{"number": "A", "at": [0, 0], "direction": "up"}]}));
    assert_eq!(r["pin_map_cleared"], true);
    let r = run(&mut s, "ecad.pad.add", json!({"component": "R0603", "pads": [{"number": 3, "at": [5, 0], "size": [1, 1]}]}));
    assert_eq!(r["pin_map_cleared"], false, "adding without replace keeps the map");
}
