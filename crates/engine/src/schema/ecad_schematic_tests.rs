use serde_json::{Value, json};

use super::{AnnotateParams, DeleteParams, DuplicateParams, EditParams, InspectParams, InspectResult, MoveParams, PlaceParams, SheetResult};
use crate::Session;
use crate::schema::tests::{assert_keys, conforms};
use crate::schema::{ParamCheck, input_schema_for, output_schema_for};

#[test]
fn schemas_and_the_key_check_agree() {
    assert_keys(input_schema_for::<PlaceParams>(), &["parts"]);
    assert_keys(input_schema_for::<EditParams>(), &["parts"]);
    assert_keys(input_schema_for::<MoveParams>(), &["about", "by", "items", "mirror", "rotation", "to"]);
    assert_keys(input_schema_for::<DuplicateParams>(), &["by", "items"]);
    assert_keys(input_schema_for::<DeleteParams>(), &["items"]);
    assert_keys(input_schema_for::<AnnotateParams>(), &["keep_existing", "order", "start"]);
    assert_keys(input_schema_for::<InspectParams>(), &["part", "pin"]);
}

#[test]
fn rows_refuse_unknown_keys_in_the_schema_and_the_reader() {
    for (schema, row) in [(input_schema_for::<PlaceParams>(), "PlaceSpec"), (input_schema_for::<EditParams>(), "PartEdit")] {
        assert_eq!(schema.as_value()["$defs"][row]["additionalProperties"], json!(false), "{row}");
    }
    assert_eq!(input_schema_for::<InspectParams>().as_value()["$defs"]["PinOf"]["additionalProperties"], json!(false));
    let place = |row: Value| serde_json::from_value::<PlaceParams>(json!({"parts": [row]}));
    assert!(place(json!({"component": "R0603"})).is_ok());
    assert!(place(json!({"component": "R0603", "angle": 90})).is_err());
    let edit = |row: Value| serde_json::from_value::<EditParams>(json!({"parts": [row]}));
    assert!(edit(json!({"part": "R1", "value": null})).is_ok_and(|p| p.parts.first().is_some_and(|e| e.value == Some(None))));
    assert!(edit(json!({"part": "R1"})).is_ok_and(|p| p.parts.first().is_some_and(|e| e.value.is_none())));
    assert!(edit(json!({"part": "R1", "colour": "red"})).is_err());
    let pin = |pin: Value| serde_json::from_value::<InspectParams>(json!({"pin": pin}));
    assert!(pin(json!("R1.2")).is_ok() && pin(json!({"part": "R1", "pin": 2})).is_ok());
    assert!(pin(json!({"part": "R1", "pin": 2, "net": "x"})).is_err() && pin(json!(7)).is_err());
}

#[test]
fn replies_match_their_schemas_and_strict_refuses_unknown_keys() {
    let mut s = Session { param_check: ParamCheck::Warn, ..Default::default() };
    s.execute("ecad.component.create", &json!({"name": "R0603", "prefix": "R", "value": "10k"})).unwrap();
    s.execute("ecad.symbol.generate", &json!({"symbol": "R0603", "component": "R0603", "style": "resistor"})).unwrap();
    let sheet = output_schema_for::<SheetResult>();
    let calls = [
        ("ecad.schematic.place", json!({"parts": [{"component": "R0603", "at": [0, 0]}, {"component": "R0603"}]}), sheet.clone()),
        ("ecad.schematic.edit", json!({"parts": [{"part": "R1", "value": "4k7", "designator_at": [0, 2.54]}]}), sheet.clone()),
        ("ecad.schematic.move", json!({"items": ["R1"], "by": [2.54, 0], "rotation": 90}), sheet.clone()),
        ("ecad.schematic.duplicate", json!({"items": ["R1"], "by": [0, -10.16]}), sheet.clone()),
        ("ecad.schematic.annotate", json!({"order": "position", "keep_existing": false}), sheet.clone()),
        ("ecad.schematic.inspect", json!({}), output_schema_for::<InspectResult>()),
        ("ecad.schematic.inspect", json!({"pin": "R1.2"}), output_schema_for::<InspectResult>()),
        ("ecad.inspect", json!({}), output_schema_for::<crate::schema::ecad::InspectResult>()),
        ("ecad.schematic.delete", json!({"items": ["R2"]}), sheet.clone()),
    ];
    for (id, p, schema) in calls {
        let schema = schema.to_value();
        let mut warned = p;
        warned["bogus"] = json!(1);
        let reply = s.execute(id, &warned).unwrap();
        assert!(conforms(&schema, &schema, &reply), "{id}: {reply} doesn't match {schema}");
        assert!(reply["param_warnings"].as_array().is_some_and(|w| !w.is_empty()), "{id}: {reply}");
    }
    s.param_check = ParamCheck::Strict;
    let e = s.execute("ecad.schematic.place", &json!({"parts": [], "prats": []})).unwrap_err().to_string();
    assert!(e.starts_with("invalid parameters for `ecad.schematic.place`: unknown key `prats`"), "{e}");
}
