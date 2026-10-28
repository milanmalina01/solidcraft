//! Generated footprints and symbols, checked against the package numbers and the land rules by
//! hand: a pad reaches `toe` past a lead's tip, `heel` inside its inner end, `side` past its edges.

use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

fn near(v: &Value, want: f64) -> bool {
    v.as_f64().is_some_and(|x| (x - want).abs() < 1e-9)
}

fn pad<'a>(c: &'a Value, n: &str) -> &'a Value {
    c["pads"].as_array().unwrap().iter().find(|p| p["number"] == n).unwrap_or(&Value::Null)
}

fn inspect(s: &mut Session, name: &str) -> Value {
    run(s, "ecad.component.inspect", json!({"component": name}))["component"].clone()
}

#[test]
fn an_0603_chip_from_its_size_code() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "R0603", "prefix": "R"}));
    let r = run(&mut s, "ecad.footprint.generate", json!({"component": "R0603", "package": "chip", "size": "0603"}));
    assert_eq!(r["pads"], 2);
    let c = inspect(&mut s, "R0603");
    // Body 1.6 x 0.8, terminals 0.3: copper from 0.8 - 0.3 = 0.5 out to 0.8 + 0.3 (toe) = 1.1, and
    // 0.8 + 2 x 0.05 wide.
    let p2 = pad(&c, "2");
    assert!(near(&p2["at"][0], 0.8) && near(&p2["size"][0], 0.6) && near(&p2["size"][1], 0.9), "{p2}");
    assert!(near(&pad(&c, "1")["at"][0], -0.8));
    // Copper ±1.1 x ±0.45 is wider than the body: the courtyard is 0.25 beyond it.
    assert!(near(&r["courtyard"]["from"][0], -1.35) && near(&r["courtyard"]["to"][1], 0.7) && near(&r["courtyard"]["area"], 2.7 * 1.4), "{r}");
    assert!(near(&c["measured"]["min_pad_gap"]["gap"], 1.0), "{c}");
    let layers: Vec<&str> = c["footprint_graphics"].as_array().unwrap().iter().filter_map(|g| g["layer"].as_str()).collect();
    assert!(layers.contains(&"fab") && layers.contains(&"courtyard"), "{layers:?}");
    // With a resistor symbol it is ready to place.
    assert_eq!(c["ready"], false);
    let r = run(&mut s, "ecad.symbol.generate", json!({"component": "R0603", "style": "resistor"}));
    assert_eq!(r["pins"], 2);
    let c = inspect(&mut s, "R0603");
    assert_eq!((c["ready"].clone(), c["problems"].clone()), (json!(true), json!([])), "{c}");
}

#[test]
fn a_sot23_with_the_jedec_defaults() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "SOT23", "prefix": "Q"}));
    run(&mut s, "ecad.footprint.generate", json!({"component": "SOT23", "package": "sot23"}));
    let c = inspect(&mut s, "SOT23");
    // Span 2.4, foot 0.45: copper from 1.2 - 0.45 - 0.3 = 0.45 out to 1.2 + 0.3 = 1.5.
    for (n, x, y) in [("1", -0.95, -0.975), ("2", 0.95, -0.975), ("3", 0.0, 0.975)] {
        let p = pad(&c, n);
        assert!(near(&p["at"][0], x) && near(&p["at"][1], y) && near(&p["size"][0], 0.5) && near(&p["size"][1], 1.05), "{n}: {p}");
    }
    // Pads 1 and 3: 0.45 across and 0.9 up between corners; 1 and 2 are 1.9 - 0.5 = 1.4 apart.
    assert!(near(&c["measured"]["min_pad_gap"]["gap"], (0.45f64).hypot(0.9)), "{}", c["measured"]["min_pad_gap"]);
}

#[test]
fn a_soic8_numbers_down_the_left_and_up_the_right() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "SOIC8", "prefix": "U"}));
    run(&mut s, "ecad.footprint.generate", json!({"component": "SOIC8", "package": "soic", "pins": 8}));
    let c = inspect(&mut s, "SOIC8");
    // Span 6.0, foot 0.835: copper from 3.0 - 0.835 - 0.3 = 1.865 to 3.3; centre 2.5825, 1.435 long.
    for (n, x, y) in [("1", -2.5825, 1.905), ("4", -2.5825, -1.905), ("5", 2.5825, -1.905), ("8", 2.5825, 1.905)] {
        let p = pad(&c, n);
        assert!(near(&p["at"][0], x) && near(&p["at"][1], y) && near(&p["size"][0], 1.435) && near(&p["size"][1], 0.51), "{n}: {p}");
    }
    // Leads 1.27 apart, pads 0.51 wide.
    assert!(near(&c["measured"]["min_pad_gap"]["gap"], 0.76), "{}", c["measured"]["min_pad_gap"]);
}

#[test]
fn a_qfn16_with_its_exposed_pad() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "QFN16", "prefix": "U"}));
    run(&mut s, "ecad.footprint.generate", json!({"component": "QFN16", "package": "qfn", "pins": 16, "body": [3, 3], "thermal": [1.7, 1.7]}));
    let c = inspect(&mut s, "QFN16");
    assert_eq!(c["pads"].as_array().map(Vec::len), Some(17));
    // Terminals 0.4 long from the 1.5 edge: copper 1.05 to 1.8, centre 1.425; four a side 0.5 apart.
    let p1 = pad(&c, "1");
    assert!(near(&p1["at"][0], -1.425) && near(&p1["at"][1], 0.75) && near(&p1["size"][0], 0.75) && near(&p1["size"][1], 0.35), "{p1}");
    let p16 = pad(&c, "16");
    assert!(near(&p16["at"][0], -0.75) && near(&p16["at"][1], 1.425), "counter-clockwise, ending at the top left: {p16}");
    assert!(near(&pad(&c, "17")["size"][0], 1.7));
    // Neighbours 0.5 apart less 0.35; corner pads 0.125 apart each way; the exposed pad 0.2 away.
    assert!(near(&c["measured"]["min_pad_gap"]["gap"], 0.15), "{}", c["measured"]["min_pad_gap"]);
    // The exposed pad has no pin until it is mapped to one.
    run(
        &mut s,
        "ecad.symbol.generate",
        json!({"component": "QFN16", "style": "box", "pins": (1..=16).map(|n| json!({"number": n})).collect::<Vec<_>>()}),
    );
    let c = inspect(&mut s, "QFN16");
    assert_eq!(c["unconnected_pads"], json!(["17"]));
}

#[test]
fn a_dip8_and_a_header() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "DIP8", "prefix": "U"}));
    run(&mut s, "ecad.footprint.generate", json!({"component": "DIP8", "package": "dip", "pins": 8}));
    let c = inspect(&mut s, "DIP8");
    assert!(near(&pad(&c, "1")["at"][0], -3.81) && near(&pad(&c, "1")["at"][1], 3.81));
    assert!(near(&pad(&c, "8")["at"][0], 3.81) && near(&pad(&c, "8")["at"][1], 3.81));
    assert_eq!((pad(&c, "1")["shape"].clone(), pad(&c, "2")["shape"].clone()), (json!("rect"), json!("round")), "pin 1 is square");
    assert!(near(&c["measured"]["pads"][1]["annular_ring"], 0.4));
    // The outline runs between the rows, clear of the pads' inner edges at ±(3.81 - 0.8).
    let silk = c["footprint_graphics"].as_array().unwrap().iter().find(|g| g["layer"] == "silkscreen").unwrap().clone();
    assert!(silk["from"][0].as_f64().unwrap() > -3.01 + 0.2, "{silk}");
    // A two-row header numbers across the rows: 1 and 2 side by side, 3 under 1.
    run(&mut s, "ecad.component.create", json!({"name": "J1", "prefix": "J"}));
    run(&mut s, "ecad.footprint.generate", json!({"component": "J1", "package": "header", "pins": 6, "rows": 2}));
    let c = inspect(&mut s, "J1");
    assert!(near(&pad(&c, "2")["at"][0], 1.27) && near(&pad(&c, "2")["at"][1], 2.54), "{}", pad(&c, "2"));
    assert!(near(&pad(&c, "3")["at"][0], -1.27) && near(&pad(&c, "3")["at"][1], 0.0), "{}", pad(&c, "3"));
    assert_eq!(c["pads"][1]["number"], "2", "listed in number order");
    // A long header's own body is longer than a body the caller may give.
    run(&mut s, "ecad.component.create", json!({"name": "J40", "prefix": "J"}));
    let r = run(&mut s, "ecad.footprint.generate", json!({"component": "J40", "package": "header", "pins": 40}));
    assert_eq!(r["pads"], 40);
}

#[test]
fn a_sot23_marks_pin_1_at_its_corner() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "Q", "prefix": "Q"}));
    run(&mut s, "ecad.footprint.generate", json!({"component": "Q", "package": "sot23"}));
    let c = inspect(&mut s, "Q");
    // Pin 1 is bottom left, so the body's cut corner is there and the top-left corner is whole.
    let fab = c["footprint_graphics"].as_array().unwrap().iter().find(|g| g["layer"] == "fab").unwrap()["points"].clone();
    let has = |x: f64, y: f64| fab.as_array().unwrap().iter().any(|p| near(&p[0], x) && near(&p[1], y));
    assert!(has(-1.45, 0.65) && !has(-1.45, -0.65), "{fab}");
}

#[test]
fn a_box_symbol_puts_every_pin_end_on_the_grid() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "LM358", "prefix": "U"}));
    let names = ["OUT1", "IN1-", "IN1+", "GND", "IN2+", "IN2-", "OUT2", "VCC"];
    let pins: Vec<Value> = names.iter().enumerate().map(|(i, n)| json!({"number": i + 1, "name": n})).collect();
    let r = run(&mut s, "ecad.symbol.generate", json!({"component": "LM358", "style": "box", "pins": pins}));
    assert_eq!(r["pins"], 8);
    let c = inspect(&mut s, "LM358");
    for pin in c["pins"].as_array().unwrap() {
        for v in pin["at"].as_array().unwrap() {
            let k = v.as_f64().unwrap() / 2.54;
            assert!((k - k.round()).abs() < 1e-9, "{pin} is off the grid");
        }
    }
    // Four down the left from the top, four down the right.
    assert_eq!(c["pins"][0]["direction"], "right");
    assert_eq!(c["pins"][4]["direction"], "left");
    assert!(c["pins"][0]["at"][1].as_f64() > c["pins"][3]["at"][1].as_f64());
}

#[test]
fn generators_refuse_what_they_cannot_draw() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "X"}));
    for (p, says) in [
        (json!({"package": "chip", "size": "0604"}), "`size` code"),
        (json!({"package": "soic", "pins": 7}), "multiple of 2"),
        (json!({"package": "soic"}), "needs `pins`"),
        (json!({"package": "qfn", "pins": 16}), "`body`"),
        (json!({"package": "header", "pins": 6, "rows": 3}), "1 or 2"),
        (json!({"package": "sot23", "pitch": -1}), "`pitch`"),
        (json!({"package": "sot23", "toe": 9}), "`toe`"),
        (json!({"package": "chip", "size": "0603", "lead": 1.0}), "shorter than half"),
        (json!({"package": "dip", "pins": 4000}), "1 to 1024"),
    ] {
        let mut p = p.clone();
        p["component"] = json!("X");
        let e = s.execute("ecad.footprint.generate", &p).expect_err("refused").to_string();
        assert!(e.contains(says), "{p}: {e}");
    }
    let e = s.execute("ecad.symbol.generate", &json!({"component": "X", "style": "box"})).unwrap_err().to_string();
    assert!(e.contains("1 to 1024"), "{e}");
    let e = s
        .execute("ecad.symbol.generate", &json!({"component": "X", "style": "box", "pins": [{"number": 1}, {"number": 1}]}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("numbered `1`"), "{e}");
}

#[test]
fn hostile_generator_params_never_panic() {
    let keys = [
        "package",
        "size",
        "pins",
        "pitch",
        "span",
        "lead",
        "lead_width",
        "body",
        "thermal",
        "rows",
        "drill",
        "pad",
        "toe",
        "heel",
        "side",
        "margin",
        "style",
    ];
    for id in ["ecad.footprint.generate", "ecad.symbol.generate"] {
        for key in keys {
            for junk in crate::tests::hostile_values() {
                let mut s = Session::default();
                run(&mut s, "ecad.component.create", json!({"name": "X"}));
                let mut p = json!({"component": "X", "package": "qfn", "pins": 16, "body": [3, 3], "style": "box"});
                p[key] = junk.clone();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(id, &p)));
                assert!(!matches!(r, Err(_) | Ok(Err(crate::EngineError::Internal(..)))), "{id} {key}={junk} panicked");
                assert!(s.doc.ecad.validate().is_ok(), "{id} {key}={junk} left a design the file would refuse");
            }
        }
    }
}
