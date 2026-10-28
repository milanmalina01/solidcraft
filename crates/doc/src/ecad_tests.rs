use serde_json::{Value, json};

use super::{ElectronicComponent, Electronics, MAX_COMPONENTS, MAX_DESCRIPTION, MAX_TEXT};
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
        json!([{"id": 1, "name": "R0603", "prefix": "R", "footprint": {}}]),
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
