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

fn pad<'a>(f: &'a Value, n: &str) -> &'a Value {
    f["pads"].as_array().unwrap().iter().find(|p| p["number"] == n).unwrap_or(&Value::Null)
}

fn inspect(s: &mut Session, name: &str) -> Value {
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
fn an_0603_chip_from_its_size_code() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "R0603", "prefix": "R"}));
    let r = run(&mut s, "ecad.footprint.generate", json!({"footprint": "R0603", "component": "R0603", "package": "chip", "size": "0603"}));
    assert_eq!((r["pads"].clone(), r["created"].clone(), r["variant"].clone()), (json!(2), json!(true), json!("R0603")));
    let f = footprint(&mut s, "R0603");
    // Body 1.6 x 0.8, terminals 0.3: copper from 0.8 - 0.3 = 0.5 out to 0.8 + 0.3 (toe) = 1.1, and
    // 0.8 + 2 x 0.05 wide.
    let p2 = pad(&f, "2");
    assert!(near(&p2["at"][0], 0.8) && near(&p2["size"][0], 0.6) && near(&p2["size"][1], 0.9), "{p2}");
    assert!(near(&pad(&f, "1")["at"][0], -0.8));
    // Copper ±1.1 x ±0.45 is wider than the body: the courtyard is 0.25 beyond it.
    assert!(near(&r["courtyard"]["from"][0], -1.35) && near(&r["courtyard"]["to"][1], 0.7) && near(&r["courtyard"]["area"], 2.7 * 1.4), "{r}");
    assert!(near(&f["min_pad_gap"]["gap"], 1.0), "{f}");
    let layers: Vec<&str> = f["graphics"].as_array().unwrap().iter().filter_map(|g| g["layer"].as_str()).collect();
    assert!(layers.contains(&"fab") && layers.contains(&"courtyard"), "{layers:?}");
    // With a resistor symbol it is ready to place.
    assert_eq!(inspect(&mut s, "R0603")["ready"], false);
    let r = run(&mut s, "ecad.symbol.generate", json!({"symbol": "R0603", "component": "R0603", "style": "resistor"}));
    assert_eq!(r["pins"], 2);
    let c = inspect(&mut s, "R0603");
    assert_eq!((c["ready"].clone(), c["problems"].clone(), c["variants"][0]["problems"].clone()), (json!(true), json!([]), json!([])), "{c}");
    // Again with the same name: the footprint is made again in place, not a second one.
    let r = run(&mut s, "ecad.footprint.generate", json!({"footprint": "R0603", "package": "chip", "size": "0805"}));
    assert_eq!(r["created"], false);
    assert_eq!(run(&mut s, "ecad.library.inspect", json!({}))["counts"]["footprints"], 1);
}

#[test]
fn a_sot23_with_the_jedec_defaults() {
    let mut s = Session::default();
    run(&mut s, "ecad.footprint.generate", json!({"footprint": "SOT23", "package": "sot23"}));
    let f = footprint(&mut s, "SOT23");
    // Span 2.4, foot 0.45: copper from 1.2 - 0.45 - 0.3 = 0.45 out to 1.2 + 0.3 = 1.5.
    for (n, x, y) in [("1", -0.95, -0.975), ("2", 0.95, -0.975), ("3", 0.0, 0.975)] {
        let p = pad(&f, n);
        assert!(near(&p["at"][0], x) && near(&p["at"][1], y) && near(&p["size"][0], 0.5) && near(&p["size"][1], 1.05), "{n}: {p}");
    }
    // Pads 1 and 3: 0.45 across and 0.9 up between corners; 1 and 2 are 1.9 - 0.5 = 1.4 apart.
    assert!(near(&f["min_pad_gap"]["gap"], (0.45f64).hypot(0.9)), "{}", f["min_pad_gap"]);
}

#[test]
fn a_soic8_numbers_down_the_left_and_up_the_right() {
    let mut s = Session::default();
    run(&mut s, "ecad.footprint.generate", json!({"footprint": "SOIC8", "package": "soic", "pins": 8}));
    let f = footprint(&mut s, "SOIC8");
    // Span 6.0, foot 0.835: copper from 3.0 - 0.835 - 0.3 = 1.865 to 3.3; centre 2.5825, 1.435 long.
    for (n, x, y) in [("1", -2.5825, 1.905), ("4", -2.5825, -1.905), ("5", 2.5825, -1.905), ("8", 2.5825, 1.905)] {
        let p = pad(&f, n);
        assert!(near(&p["at"][0], x) && near(&p["at"][1], y) && near(&p["size"][0], 1.435) && near(&p["size"][1], 0.51), "{n}: {p}");
    }
    // Leads 1.27 apart, pads 0.51 wide.
    assert!(near(&f["min_pad_gap"]["gap"], 0.76), "{}", f["min_pad_gap"]);
}

#[test]
fn a_qfn16_with_its_exposed_pad() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "QFN16", "prefix": "U"}));
    run(
        &mut s,
        "ecad.footprint.generate",
        json!({"footprint": "QFN16", "component": "QFN16", "package": "qfn", "pins": 16, "body": [3, 3], "thermal": [1.7, 1.7]}),
    );
    let f = footprint(&mut s, "QFN16");
    assert_eq!(f["pads"].as_array().map(Vec::len), Some(17));
    // Terminals 0.4 long from the 1.5 edge: copper 1.05 to 1.8, centre 1.425; four a side 0.5 apart.
    let p1 = pad(&f, "1");
    assert!(near(&p1["at"][0], -1.425) && near(&p1["at"][1], 0.75) && near(&p1["size"][0], 0.75) && near(&p1["size"][1], 0.35), "{p1}");
    let p16 = pad(&f, "16");
    assert!(near(&p16["at"][0], -0.75) && near(&p16["at"][1], 1.425), "counter-clockwise, ending at the top left: {p16}");
    assert!(near(&pad(&f, "17")["size"][0], 1.7));
    // Neighbours 0.5 apart less 0.35; corner pads 0.125 apart each way; the exposed pad 0.2 away.
    assert!(near(&f["min_pad_gap"]["gap"], 0.15), "{}", f["min_pad_gap"]);
    // The exposed pad has no pin until it is mapped to one.
    run(
        &mut s,
        "ecad.symbol.generate",
        json!({"symbol": "QFN16", "component": "QFN16", "style": "box", "pins": (1..=16).map(|n| json!({"number": n})).collect::<Vec<_>>()}),
    );
    let c = inspect(&mut s, "QFN16");
    assert_eq!(c["variants"][0]["unconnected_pads"], json!(["17"]));
}

#[test]
fn a_dip8_and_a_header() {
    let mut s = Session::default();
    run(&mut s, "ecad.footprint.generate", json!({"footprint": "DIP8", "package": "dip", "pins": 8}));
    let f = footprint(&mut s, "DIP8");
    assert!(near(&pad(&f, "1")["at"][0], -3.81) && near(&pad(&f, "1")["at"][1], 3.81));
    assert!(near(&pad(&f, "8")["at"][0], 3.81) && near(&pad(&f, "8")["at"][1], 3.81));
    assert_eq!((pad(&f, "1")["shape"].clone(), pad(&f, "2")["shape"].clone()), (json!("rect"), json!("round")), "pin 1 is square");
    assert!(near(&measured(&f, "2")["annular_ring"], 0.4));
    // The outline runs between the rows, clear of the pads' inner edges at ±(3.81 - 0.8).
    let silk = f["graphics"].as_array().unwrap().iter().find(|g| g["layer"] == "silkscreen").unwrap().clone();
    assert!(silk["from"][0].as_f64().unwrap() > -3.01 + 0.2, "{silk}");
    // A two-row header numbers across the rows: 1 and 2 side by side, 3 under 1.
    run(&mut s, "ecad.footprint.generate", json!({"footprint": "J1", "package": "header", "pins": 6, "rows": 2}));
    let f = footprint(&mut s, "J1");
    assert!(near(&pad(&f, "2")["at"][0], 1.27) && near(&pad(&f, "2")["at"][1], 2.54), "{}", pad(&f, "2"));
    assert!(near(&pad(&f, "3")["at"][0], -1.27) && near(&pad(&f, "3")["at"][1], 0.0), "{}", pad(&f, "3"));
    assert_eq!(f["pads"][1]["number"], "2", "listed in number order");
    // A long header's own body is longer than a body the caller may give.
    let r = run(&mut s, "ecad.footprint.generate", json!({"footprint": "J40", "package": "header", "pins": 40}));
    assert_eq!(r["pads"], 40);
}

#[test]
fn a_sot23_marks_pin_1_at_its_corner() {
    let mut s = Session::default();
    run(&mut s, "ecad.footprint.generate", json!({"footprint": "Q", "package": "sot23"}));
    let f = footprint(&mut s, "Q");
    // Pin 1 is bottom left, so the body's cut corner is there and the top-left corner is whole.
    let fab = f["graphics"].as_array().unwrap().iter().find(|g| g["layer"] == "fab").unwrap()["points"].clone();
    let has = |x: f64, y: f64| fab.as_array().unwrap().iter().any(|p| near(&p[0], x) && near(&p[1], y));
    assert!(has(-1.45, 0.65) && !has(-1.45, -0.65), "{fab}");
}

#[test]
fn a_box_symbol_puts_every_pin_end_on_the_grid() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "LM358", "prefix": "U"}));
    let names = ["OUT1", "IN1-", "IN1+", "GND", "IN2+", "IN2-", "OUT2", "VCC"];
    let pins: Vec<Value> = names.iter().enumerate().map(|(i, n)| json!({"number": i + 1, "name": n})).collect();
    let r = run(&mut s, "ecad.symbol.generate", json!({"symbol": "LM358", "component": "LM358", "style": "box", "pins": pins}));
    assert_eq!((r["pins"].clone(), r["created"].clone()), (json!(8), json!(true)));
    let pins = symbol(&mut s, "LM358")["pins"].clone();
    for pin in pins.as_array().unwrap() {
        for v in pin["at"].as_array().unwrap() {
            let k = v.as_f64().unwrap() / 2.54;
            assert!((k - k.round()).abs() < 1e-9, "{pin} is off the grid");
        }
    }
    // Four down the left from the top, four down the right.
    assert_eq!(pins[0]["direction"], "right");
    assert_eq!(pins[4]["direction"], "left");
    assert!(pins[0]["at"][1].as_f64() > pins[3]["at"][1].as_f64());
    assert_eq!(inspect(&mut s, "LM358")["symbol"]["name"], "LM358", "the component shows it");
}

#[test]
fn generators_refuse_what_they_cannot_draw() {
    let mut s = Session::default();
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
        p["footprint"] = json!("X");
        let e = s.execute("ecad.footprint.generate", &p).expect_err("refused").to_string();
        assert!(e.contains(says), "{p}: {e}");
    }
    assert_eq!(run(&mut s, "ecad.library.inspect", json!({}))["counts"]["footprints"], 0, "a refused generator makes nothing");
    let e = s.execute("ecad.symbol.generate", &json!({"symbol": "X", "style": "box"})).unwrap_err().to_string();
    assert!(e.contains("1 to 1024"), "{e}");
    let e =
        s.execute("ecad.symbol.generate", &json!({"symbol": "X", "style": "box", "pins": [{"number": 1}, {"number": 1}]})).unwrap_err().to_string();
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
        "symbol",
        "footprint",
        "component",
        "variant",
    ];
    for id in ["ecad.footprint.generate", "ecad.symbol.generate"] {
        for key in keys {
            for junk in crate::tests::hostile_values() {
                let mut s = Session::default();
                run(&mut s, "ecad.component.create", json!({"name": "X"}));
                let mut p = json!({"symbol": "X", "footprint": "X", "component": "X", "package": "qfn", "pins": 16, "body": [3, 3], "style": "box"});
                p[key] = junk.clone();
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute(id, &p)));
                assert!(!matches!(r, Err(_) | Ok(Err(crate::EngineError::Internal(..)))), "{id} {key}={junk} panicked");
                assert!(s.doc.ecad.validate().is_ok(), "{id} {key}={junk} left a design the file would refuse");
            }
        }
    }
}

#[test]
fn every_shape_puts_its_pins_on_the_grid_with_their_roles() {
    let mut s = Session::default();
    // (style, extra parameters, the pins by number with their roles)
    let two = |a: &str, b: &str| vec![("1".to_string(), a.to_string()), ("2".to_string(), b.to_string())];
    let numbered = |n: usize| (1..=n).map(|i| (i.to_string(), i.to_string())).collect::<Vec<_>>();
    let cases: Vec<(&str, Value, Vec<(String, String)>)> = vec![
        ("resistor", json!({}), numbered(2)),
        ("capacitor", json!({}), numbered(2)),
        ("polarized_capacitor", json!({}), two("+", "-")),
        ("inductor", json!({}), numbered(2)),
        ("ferrite", json!({}), numbered(2)),
        ("fuse", json!({}), numbered(2)),
        ("diode", json!({}), two("K", "A")),
        ("led", json!({}), two("K", "A")),
        ("zener", json!({}), two("K", "A")),
        ("schottky", json!({}), two("K", "A")),
        ("tvs", json!({}), two("K", "A")),
        ("tvs", json!({"bidirectional": true}), numbered(2)),
        ("crystal", json!({}), numbered(2)),
        ("crystal", json!({"count": 4}), numbered(4)),
        ("resonator", json!({}), numbered(3)),
        ("switch", json!({}), numbered(2)),
        ("switch", json!({"count": 4}), numbered(4)),
        ("battery", json!({}), two("+", "-")),
        ("antenna", json!({}), numbered(1)),
        ("test_point", json!({}), numbered(1)),
        ("npn", json!({}), vec![("1".into(), "B".into()), ("2".into(), "E".into()), ("3".into(), "C".into())]),
        ("pnp", json!({}), vec![("1".into(), "B".into()), ("2".into(), "E".into()), ("3".into(), "C".into())]),
        ("nmos", json!({}), vec![("1".into(), "G".into()), ("2".into(), "S".into()), ("3".into(), "D".into())]),
        ("pmos", json!({}), vec![("1".into(), "G".into()), ("2".into(), "S".into()), ("3".into(), "D".into())]),
        // Several LEDs share an anode unless the cathode is common.
        ("led", json!({"count": 3}), vec![("1".into(), "A".into()), ("2".into(), "K1".into()), ("3".into(), "K2".into()), ("4".into(), "K3".into())]),
        (
            "led",
            json!({"count": 3, "common": "cathode"}),
            vec![("1".into(), "K".into()), ("2".into(), "A1".into()), ("3".into(), "A2".into()), ("4".into(), "A3".into())],
        ),
    ];
    for (i, (style, extra, want)) in cases.into_iter().enumerate() {
        let mut p = json!({"symbol": format!("S{i}"), "style": style});
        for (k, v) in extra.as_object().into_iter().flatten() {
            p[k] = v.clone();
        }
        let r = run(&mut s, "ecad.symbol.generate", p.clone());
        let got: Vec<(String, String)> = r["pin_list"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| (q["number"].as_str().unwrap().to_string(), q["role"].as_str().unwrap_or_default().to_string()))
            .collect();
        let mut sorted = got.clone();
        sorted.sort();
        assert_eq!(sorted, want, "{p}");
        let sym = symbol(&mut s, &format!("S{i}"));
        for pin in sym["pins"].as_array().unwrap() {
            for v in pin["at"].as_array().unwrap() {
                let k = v.as_f64().unwrap() / 2.54;
                assert!((k - k.round()).abs() < 1e-9, "{p}: {pin} is off the grid");
            }
        }
        assert!(sym["graphics"].as_array().is_some_and(|g| !g.is_empty()), "{p}: a shape is drawn");
        assert_eq!(r["bbox"], sym["bbox"], "{p}");
    }
    // A pin named after a role takes that role, whatever its number.
    let r = run(
        &mut s,
        "ecad.symbol.generate",
        json!({"symbol": "Q", "style": "npn", "pins": [{"number": 1, "name": "E"}, {"number": 2, "name": "B"}, {"number": 3, "name": "C"}]}),
    );
    let base = r["pin_list"].as_array().unwrap().iter().find(|q| q["role"] == "B").unwrap().clone();
    assert_eq!(base["number"], "2");
    let e = s.execute("ecad.symbol.generate", &json!({"symbol": "X", "style": "npn", "pins": [{"number": 1}]})).unwrap_err().to_string();
    assert!(e.contains('3'), "an npn has three pins: {e}");
    let e = s.execute("ecad.symbol.generate", &json!({"symbol": "X", "style": "led", "count": 5})).unwrap_err().to_string();
    assert!(e.contains("`count`"), "{e}");
}

#[test]
fn a_new_drawing_keeps_the_wires_on_the_pins_it_keeps() {
    let mut s = Session::default();
    run(&mut s, "ecad.component.create", json!({"name": "R0603", "prefix": "R", "value": "10k"}));
    run(&mut s, "ecad.symbol.generate", json!({"symbol": "R0603", "component": "R0603", "style": "resistor"}));
    run(&mut s, "ecad.schematic.place", json!({"parts": [{"component": "R0603", "at": [0, 0]}]}));
    run(&mut s, "ecad.schematic.wire", json!({"wires": [{"from": "R1.1", "to": [-15.24, 0]}, {"from": "R1.2", "to": [15.24, 0]}]}));
    run(&mut s, "ecad.schematic.label", json!({"labels": [{"name": "IN", "at": [-15.24, 0]}, {"name": "OUT", "at": [15.24, 0]}]}));
    // Another shape with its pins where they were: nothing moves, nothing changes.
    let r = run(&mut s, "ecad.symbol.generate", json!({"symbol": "R0603", "style": "capacitor"}));
    assert!(r.get("netlist_changed").is_none() && r.get("detached").is_none(), "{r}");
    // A box with its pins further out and renumbered: the wires follow the pins they were on.
    let r = run(
        &mut s,
        "ecad.symbol.generate",
        json!({"symbol": "R0603", "style": "box", "pins": [{"number": "A", "side": "left"}, {"number": "B", "side": "right"}], "renames": {"1": "A", "2": "B"}}),
    );
    assert!(r.get("detached").is_none(), "{r}");
    assert_eq!(
        r["netlist_changed"],
        json!([{"net": "IN", "lost": ["R1.1"], "gained": ["R1.A"]}, {"net": "OUT", "lost": ["R1.2"], "gained": ["R1.B"]}]),
        "the same connections under the pins' new numbers"
    );
    let wires = run(&mut s, "ecad.schematic.inspect", json!({}))["wires"].clone();
    assert_eq!(wires[0]["from"], json!({"at": [-7.62, 0.0], "part": 1, "designator": "R1", "pin": "A"}));
    assert_eq!(wires[1]["from"]["pin"], "B");
    // A shape with none of those pins lets the wires go where the pins were, and says so.
    let r = run(&mut s, "ecad.symbol.generate", json!({"symbol": "R0603", "style": "test_point"}));
    assert_eq!(r["detached"].as_array().map(Vec::len), Some(2), "{r}");
    assert_eq!(r["netlist_changed"], json!([{"net": "IN", "lost": ["R1.A"], "gained": []}, {"net": "OUT", "lost": ["R1.B"], "gained": []}]));
    let net = run(&mut s, "ecad.netlist.inspect", json!({}));
    assert_eq!(net["dangling"].as_array().map(Vec::len), Some(2), "{net}");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(run(&mut s, "ecad.netlist.inspect", json!({}))["nets"][0]["pins"][0]["ref"], "R1.A", "undo puts the pins and wires back");
}
