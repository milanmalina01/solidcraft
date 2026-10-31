//! 3D packages: a link to a SolidCraft component and where it sits on the footprint.

use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn near3(v: &Value, want: [f64; 3]) -> bool {
    v.as_array().is_some_and(|a| a.len() == 3 && a.iter().zip(want).all(|(x, w)| x.as_f64().is_some_and(|x| (x - w).abs() < 1e-6)))
}

/// An 0603's 1.6 x 0.8 x 0.45 mm body as a SolidCraft component, its origin under the middle.
fn body(s: &mut Session) -> u64 {
    let id = run(s, "component.create", json!({"name": "R0603 body"}))["component"].as_u64().unwrap();
    run(s, "solid.box", json!({"length": 1.6, "width": 0.8, "height": 0.45, "corner": [-0.8, -0.4, 0]}));
    run(s, "component.activate", json!({"component": 0}));
    run(s, "ecad.component.create", json!({"name": "R0603", "prefix": "R"}));
    id
}

#[test]
fn a_package_is_measured_on_the_footprint() {
    let mut s = Session::default();
    let id = body(&mut s);
    let r = run(&mut s, "ecad.package.create", json!({"name": "R0603", "model": "R0603 body", "component": "R0603"}));
    assert_eq!(
        (r["component"]["name"].clone(), r["variant"].clone()),
        (json!("R0603"), json!("R0603")),
        "a variant is made, named after the package"
    );
    let p = &r["package"];
    assert_eq!(p["model"], json!({"id": id, "name": "R0603 body"}));
    assert!(near3(&p["bbox"][0], [-0.8, -0.4, 0.0]) && near3(&p["bbox"][1], [0.8, 0.4, 0.45]), "{p}");
    assert!((p["height"].as_f64().unwrap() - 0.45).abs() < 1e-6);
    // A quarter turn swaps the sides; a lift raises it.
    let r = run(&mut s, "ecad.package.edit", json!({"package": "R0603", "rotation": 90, "translate": [0, 0, 0.1]}));
    assert_eq!(r["changed"], json!(["translate", "rotation"]));
    let p = &r["package"];
    assert!(near3(&p["bbox"][0], [-0.4, -0.8, 0.1]) && near3(&p["bbox"][1], [0.4, 0.8, 0.55]), "{p}");
    let c = run(&mut s, "ecad.component.inspect", json!({"component": "R0603"}))["component"].clone();
    assert_eq!(c["variants"][0]["package"]["model"]["name"], "R0603 body");
    assert!(near3(&c["variants"][0]["package"]["bbox"][1], [0.4, 0.8, 0.55]), "every variant that uses it follows");
}

#[test]
fn deleting_the_model_deletes_the_package_and_undo_brings_both_back() {
    let mut s = Session::default();
    let id = body(&mut s);
    run(&mut s, "ecad.package.create", json!({"name": "R0603", "model": id, "component": "R0603"}));
    run(&mut s, "component.delete", json!({"component": id}));
    let c = run(&mut s, "ecad.component.inspect", json!({"component": "R0603"}))["component"].clone();
    assert_eq!(c["variants"][0]["package"], json!(null), "{c}");
    assert_eq!(run(&mut s, "ecad.library.inspect", json!({}))["counts"]["packages"], 0);
    s.execute("edit.undo", &json!({})).unwrap();
    let c = run(&mut s, "ecad.component.inspect", json!({"component": "R0603"}))["component"].clone();
    assert_eq!(c["variants"][0]["package"]["model"]["id"], json!(id));
    // A package a variant uses goes only with `force`, which leaves the variant with none.
    let e = s.execute("ecad.package.delete", &json!({"package": "R0603"})).unwrap_err().to_string();
    assert!(e.contains("R0603 (R0603)") && e.contains("force"), "{e}");
    let r = run(&mut s, "ecad.package.delete", json!({"package": "R0603", "force": true}));
    assert_eq!(r["cleared"], json!([{"component": {"id": 1, "name": "R0603"}, "variant": "R0603"}]));
    assert_eq!(run(&mut s, "ecad.component.inspect", json!({"component": "R0603"}))["component"]["variants"][0]["package"], json!(null));
}

#[test]
fn a_package_is_a_solidcraft_component() {
    let mut s = Session::default();
    body(&mut s);
    for (model, says) in
        [(json!(0), "the design itself"), (json!("R0603"), "is an electronic component"), (json!("Nope"), "SolidCraft component of this design")]
    {
        let e = s.execute("ecad.package.create", &json!({"name": "P", "model": model})).unwrap_err().to_string();
        assert!(e.contains(says), "{model}: {e}");
    }
    let e = s.execute("ecad.package.create", &json!({"name": "P", "model": "R0603 body", "translate": [0, 0, 5000]})).unwrap_err().to_string();
    assert!(e.contains("`translate`"), "{e}");
    let e = s.execute("ecad.package.create", &json!({"name": "P", "model": "R0603 body", "variant": "0603"})).unwrap_err().to_string();
    assert!(e.contains("give the component too"), "{e}");
    run(&mut s, "ecad.package.create", json!({"name": "P", "model": "R0603 body"}));
    let e = s.execute("ecad.package.create", &json!({"name": "P", "model": "R0603 body"})).unwrap_err().to_string();
    assert!(e.contains("`P`"), "names are unique among the packages: {e}");
}

#[test]
fn a_package_from_a_step_file() {
    let dir = std::env::temp_dir().join(format!("solidcraft-ecad-step-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("r0603.step");
    let mut a = Session::default();
    run(&mut a, "solid.box", json!({"length": 1.6, "width": 0.8, "height": 0.45, "corner": [-0.8, -0.4, 0]}));
    run(&mut a, "file.export", json!({"path": path.to_string_lossy()}));
    let mut s = Session::default();
    let r = run(&mut s, "file.insert_step", json!({"path": path.to_string_lossy()}));
    let p = run(&mut s, "ecad.package.create", json!({"name": "R0603", "model": r["component"]}))["package"].clone();
    assert!(near3(&p["bbox"][0], [-0.8, -0.4, 0.0]) && near3(&p["bbox"][1], [0.8, 0.4, 0.45]), "{p}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_package_holds_the_bodies_of_the_components_under_it() {
    let mut s = Session::default();
    let outer = run(&mut s, "component.create", json!({"name": "Package"}))["component"].as_u64().unwrap();
    run(&mut s, "component.create", json!({"name": "Leads", "parent": outer}));
    run(&mut s, "solid.box", json!({"length": 2, "width": 1, "height": 0.5, "corner": [-1, -0.5, 0]}));
    run(&mut s, "component.activate", json!({"component": 0}));
    let p = run(&mut s, "ecad.package.create", json!({"name": "U1", "model": "Package"}))["package"].clone();
    assert!(near3(&p["bbox"][1], [1.0, 0.5, 0.5]), "{p}");
}
