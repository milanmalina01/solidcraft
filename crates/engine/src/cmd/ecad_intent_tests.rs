//! The design's intent on the test board (`ecad::tests::board`): SIG fast, GND ground and VCC
//! power declared, 2400 MHz of concern; the headers J1 and J2 and the capacitor C1 left to the
//! inference.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_doc::ecad::Actor;

use crate::Session;
use crate::cmd::ecad::tests::{board, run};

fn refused(s: &mut Session, id: &str, p: Value) -> String {
    let (before, undo) = (s.doc.clone(), s.undo.len());
    let Err(e) = s.execute(id, &p) else { panic!("{id} {p} must be refused") };
    assert!(Arc::ptr_eq(&before, &s.doc), "a refused {id} leaves the design as it was");
    assert_eq!(s.undo.len(), undo);
    e.to_string()
}

fn proposal<'a>(r: &'a Value, key: &str) -> &'a Value {
    r["proposals"].as_array().and_then(|p| p.iter().find(|x| x["key"] == key)).unwrap_or(&Value::Null)
}

#[test]
fn the_inference_proposes_roles_and_asks_what_only_the_user_knows() {
    let mut s = board();
    let (doc, undo) = (s.doc.clone(), s.undo.len());
    let r = run(&mut s, "ecad.intent.inspect", json!({"propose": true}));
    assert!(Arc::ptr_eq(&doc, &s.doc) && s.undo.len() == undo, "proposing changes nothing");
    let declared: Vec<(&str, &str)> =
        r["nets"].as_array().unwrap().iter().map(|n| (n["net"].as_str().unwrap(), n["role"].as_str().unwrap())).collect();
    assert_eq!(declared, [("SIG", "fast"), ("GND", "ground"), ("VCC", "power")]);
    assert!(r["nets"].as_array().unwrap().iter().all(|n| n["source"] == "declared" && n["by"] == "agent"));
    // A header is a connector; whether a cable plugs into it is the user's to say, so it blocks.
    let j1 = proposal(&r, "part|J1");
    assert_eq!((j1["role"].clone(), j1["blocking"].clone(), j1["needs"].clone()), (json!("connector"), json!(true), json!(["cable"])), "{r}");
    assert_eq!(j1["record"]["source"], "inferred");
    // A net with nothing fast on it is slow; a capacitor from a rail to ground decouples it.
    assert_eq!(proposal(&r, "net|OTHER")["role"], "slow");
    assert_eq!(proposal(&r, "part|C1")["record"]["of"], json!(["VCC"]));
    let missing: Vec<&str> = r["coverage"]["blocking"]["missing"].as_array().unwrap().iter().filter_map(|m| m["key"].as_str()).collect();
    assert_eq!(missing, ["part|J1", "part|J2"]);
    // Filters.
    let only = run(&mut s, "ecad.intent.inspect", json!({"nets": ["GND"]}));
    assert_eq!(only["nets"].as_array().map(Vec::len), Some(1));
    let blocking = run(&mut s, "ecad.intent.inspect", json!({"propose": true, "blocking": true}));
    assert!(blocking["proposals"].as_array().unwrap().iter().all(|p| p["blocking"] == true), "{blocking}");
}

#[test]
fn what_an_agent_sets_says_who_said_it() {
    let mut s = board();
    let e = refused(&mut s, "ecad.intent.set", json!({"parts": [{"part": "J1", "role": "connector", "cable": false}]}));
    assert!(e.contains("`cable` is the user's to tell"), "{e}");
    let undo = s.undo.len();
    let r = run(&mut s, "ecad.intent.set", json!({"parts": [{"part": "J1", "role": "connector", "cable": false, "note": "the user said no cable"}]}));
    assert_eq!(r["changed"], json!(["part|J1"]));
    assert_eq!(s.undo.len(), undo + 1);
    let missing: Vec<&str> = r["coverage"]["blocking"]["missing"].as_array().unwrap().iter().filter_map(|m| m["key"].as_str()).collect();
    assert_eq!(missing, ["part|J2"], "J1 is answered");
    let j1 = run(&mut s, "ecad.intent.inspect", json!({"parts": ["J1"]}))["parts"][0].clone();
    assert_eq!(j1, json!({"part": "J1", "role": "connector", "cable": false, "note": "the user said no cable", "source": "declared", "by": "agent"}));
    // The user's own answer is saved as the user's.
    s.actor = Actor::User;
    run(&mut s, "ecad.intent.set", json!({"parts": [{"part": "J2", "role": "connector", "cable": true}]}));
    let j2 = run(&mut s, "ecad.intent.inspect", json!({"parts": ["J2"]}))["parts"][0].clone();
    assert_eq!((j2["by"].clone(), j2["cable"].clone()), (json!("user"), json!(true)));
    s.actor = Actor::Agent;
    let r = run(&mut s, "ecad.intent.set", json!({"parts": [{"part": "J2", "role": "connector", "cable": false, "note": "the user said so"}]}));
    assert_eq!(r["kept"], json!(["part|J2"]), "an agent leaves the user's records as they are");
    // A name the schematic doesn't have is kept, and listed as unknown.
    let r = run(&mut s, "ecad.intent.set", json!({"nets": [{"net": "NOPE", "role": "slow"}]}));
    assert!(r["coverage"]["unknown"].to_string().contains("NOPE"), "{r}");
    for (p, says) in [
        (json!({"nets": [{"net": "SIG", "role": "speedy"}]}), "role"),
        (json!({"board": {"f_concern_mhz": -1}}), "f_concern_mhz"),
        (json!({"remove": ["net|NOWHERE"]}), "NOWHERE"),
    ] {
        let e = refused(&mut s, "ecad.intent.set", p.clone());
        assert!(e.contains(says), "{p}: {e}");
    }
    // Taken off, a record goes back to what the inference proposes.
    let r = run(&mut s, "ecad.intent.set", json!({"remove": ["net|SIG"]}));
    assert_eq!(r["removed"], json!(["net|SIG"]), "{r}");
    let nets = run(&mut s, "ecad.intent.inspect", json!({"nets": ["SIG"], "propose": true}));
    assert!(nets["nets"].as_array().is_none_or(Vec::is_empty), "{nets}");
}
