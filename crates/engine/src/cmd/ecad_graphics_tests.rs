//! Graphics on symbols and footprints, and the courtyard round a footprint.

use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn near(v: &Value, want: f64) -> bool {
    v.as_f64().is_some_and(|x| (x - want).abs() < 1e-9)
}

fn r0603(s: &mut Session) {
    run(s, "ecad.component.create", json!({"name": "R0603", "prefix": "R"}));
    run(
        s,
        "ecad.pin.add",
        json!({"component": "R0603", "pins": [{"number": 1, "at": [-5.08, 0], "direction": "right"}, {"number": 2, "at": [5.08, 0], "direction": "left"}]}),
    );
    run(
        s,
        "ecad.pad.add",
        json!({"component": "R0603", "pads": [{"number": 1, "at": [-0.8, 0], "size": [0.9, 1.0]}, {"number": 2, "at": [0.8, 0], "size": [0.9, 1.0]}]}),
    );
}

#[test]
fn a_courtyard_goes_round_the_pads_and_the_body() {
    let mut s = Session::default();
    r0603(&mut s);
    let c = run(&mut s, "ecad.component.inspect", json!({"component": "R0603"}))["component"].clone();
    assert!(c["problems"].as_array().unwrap().iter().any(|p| p["code"] == "courtyard_missing"), "{c}");
    // The 1.6 x 0.8 mm body of an 0603 on the fabrication layer.
    let r = run(
        &mut s,
        "ecad.footprint.set_graphics",
        json!({"component": "R0603", "layer": "fab", "graphics": [{"type": "rect", "from": [-0.8, -0.4], "to": [0.8, 0.4], "width": 0.1}]}),
    );
    assert_eq!(r["count"], 1);
    let r = run(&mut s, "ecad.footprint.courtyard", json!({"component": "R0603"}));
    // Copper reaches ±1.25 x ±0.5, past the body; 0.25 more each way.
    let cy = &r["courtyard"];
    assert!(near(&cy["from"][0], -1.5) && near(&cy["from"][1], -0.75) && near(&cy["to"][0], 1.5) && near(&cy["to"][1], 0.75), "{r}");
    assert!(near(&cy["area"], 4.5), "{r}");
    let c = run(&mut s, "ecad.component.inspect", json!({"component": "R0603"}))["component"].clone();
    assert!(near(&c["measured"]["courtyard"]["area"], 4.5), "{c}");
    assert_eq!(c["problems"], json!([]), "{c}");
    assert_eq!(c["footprint_graphics"].as_array().map(Vec::len), Some(2));
    // Again: the same courtyard, not a second one.
    run(&mut s, "ecad.footprint.courtyard", json!({"component": "R0603"}));
    let c = run(&mut s, "ecad.component.inspect", json!({"component": "R0603"}))["component"].clone();
    assert_eq!(c["footprint_graphics"].as_array().map(Vec::len), Some(2));
    // A pad moved out of it is an error until the courtyard is made again.
    run(&mut s, "ecad.pad.edit", json!({"component": "R0603", "pad": 2, "at": [1.5, 0]}));
    let c = run(&mut s, "ecad.component.inspect", json!({"component": "R0603"}))["component"].clone();
    assert_eq!(
        (c["ready"].clone(), c["problems"][0]["code"].clone(), c["problems"][0]["items"].clone()),
        (json!(false), json!("pad_outside_courtyard"), json!(["2"]))
    );
    run(&mut s, "ecad.footprint.courtyard", json!({"component": "R0603"}));
    assert_eq!(run(&mut s, "ecad.component.inspect", json!({"component": "R0603"}))["component"]["ready"], true);
}

#[test]
fn a_sot23_courtyard_takes_the_body_where_it_is_wider() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "SOT23"}));
    run(
        &mut s,
        "ecad.pad.add",
        json!({"component": "SOT23", "pads": [
            {"number": 1, "at": [-0.95, -1.1], "size": [0.6, 0.8]}, {"number": 2, "at": [0.95, -1.1], "size": [0.6, 0.8]}, {"number": 3, "at": [0, 1.1], "size": [0.6, 0.8]}
        ]}),
    );
    // The 2.9 x 1.3 mm body is wider than the pads, which reach higher.
    run(
        &mut s,
        "ecad.footprint.set_graphics",
        json!({"component": "SOT23", "layer": "fab", "graphics": [{"type": "rect", "from": [-1.45, -0.65], "to": [1.45, 0.65]}]}),
    );
    let r = run(&mut s, "ecad.footprint.courtyard", json!({"component": "SOT23"}));
    let cy = &r["courtyard"];
    assert!(near(&cy["from"][0], -1.7) && near(&cy["to"][1], 1.75) && near(&cy["area"], 3.4 * 3.5), "{r}");
}

#[test]
fn a_symbol_body_frames_with_its_pins() {
    let mut s = Session::default();
    r0603(&mut s);
    let r = run(
        &mut s,
        "ecad.symbol.set_graphics",
        json!({"component": "R0603", "graphics": [
            {"type": "rect", "from": [-2.54, -1.016], "to": [2.54, 1.016], "width": 0.254},
            {"type": "text", "at": [-2.54, 1.5], "text": "R", "height": 1.27}
        ]}),
    );
    assert_eq!(r["count"], 2);
    let c = run(&mut s, "ecad.component.inspect", json!({"component": "R0603"}))["component"].clone();
    let bbox = &c["measured"]["symbol_bbox"];
    assert!(near(&bbox[0][0], -5.08) && near(&bbox[1][0], 5.08), "the pins reach further than the body: {bbox}");
    assert_eq!(c["symbol_graphics"][0], json!({"type": "rect", "from": [-2.54, -1.016], "to": [2.54, 1.016], "width": 0.254}));
    assert_eq!(c["symbol_graphics"][1]["text"], "R");
    // An empty list clears the body.
    assert_eq!(run(&mut s, "ecad.symbol.set_graphics", json!({"component": "R0603", "graphics": []}))["count"], 0);
}

#[test]
fn bad_graphics_are_refused_with_their_index() {
    let mut s = Session::default();
    r0603(&mut s);
    for (g, says) in [
        (json!({"type": "line", "points": [[0, 0]]}), "at least 2 points"),
        (json!({"type": "rect", "from": [0, 0]}), "needs `to`"),
        (json!({"type": "circle", "center": [0, 0], "radius": 0}), "`radius`"),
        (json!({"type": "arc", "center": [0, 0], "radius": 1, "start": 0, "sweep": 0}), "`sweep` must not be 0"),
        (json!({"type": "text", "at": [0, 0], "height": 1}), "needs `text`"),
        (json!({"type": "line", "points": [[0, 0], [1, 1]], "fill": true}), "no `fill`"),
        (json!({"type": "rect", "from": [0, 0], "to": [1, 1], "radius": 1}), "no `center`"),
        (json!({"type": "line", "points": [[0, 0], [5000, 0]]}), "within 1000 mm"),
        (json!({"type": "line", "points": [[0, 0], [1, 1]], "width": 50}), "`width`"),
        (json!({"type": "blob"}), "`graphics[1].type`"),
    ] {
        let before = s.doc.clone();
        let Err(e) =
            s.execute("ecad.symbol.set_graphics", &json!({"component": "R0603", "graphics": [{"type": "line", "points": [[0, 0], [1, 0]]}, g]}))
        else {
            panic!("{g} must be refused")
        };
        let e = e.to_string();
        assert!(e.contains(says) && (e.contains("graphics[1]") || e.contains("`graphics[")), "{g}: {e}");
        assert!(std::sync::Arc::ptr_eq(&before, &s.doc) || *before == *s.doc);
    }
    let e = s.execute("ecad.footprint.courtyard", &json!({"component": "R0603", "margin": 9})).unwrap_err().to_string();
    assert!(e.contains("0 to 5"), "{e}");
}
