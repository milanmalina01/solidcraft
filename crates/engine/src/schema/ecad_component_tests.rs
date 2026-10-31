use serde_json::json;

use super::{CopyParams, CreateParams, CreateResult, DeleteParams, DeleteResult, EditParams, EditResult, InspectParams, InspectResult};
use crate::Session;
use crate::schema::tests::{assert_keys, conforms};
use crate::schema::{ParamCheck, input_schema_for, output_schema_for};

#[test]
fn schemas_and_the_key_check_agree() {
    assert_keys(
        input_schema_for::<CreateParams>(),
        &["description", "footprint", "manufacturer", "mpn", "name", "package", "prefix", "symbol", "value"],
    );
    assert_keys(input_schema_for::<EditParams>(), &["component", "description", "manufacturer", "mpn", "name", "prefix", "symbol", "value"]);
    assert_keys(input_schema_for::<CopyParams>(), &["component", "name"]);
    assert_keys(input_schema_for::<DeleteParams>(), &["components", "with_parts"]);
    assert_keys(input_schema_for::<InspectParams>(), &["component"]);
}

#[test]
fn replies_match_their_schemas_and_strict_refuses_unknown_keys() {
    let mut s = Session { param_check: ParamCheck::Warn, ..Default::default() };
    let calls = [
        ("ecad.component.create", json!({"name": "R0603", "prefix": "R", "value": "10k"}), output_schema_for::<CreateResult>()),
        ("ecad.component.edit", json!({"component": "R0603", "mpn": "RC0603"}), output_schema_for::<EditResult>()),
        ("ecad.component.copy", json!({"component": 1, "name": "R2"}), output_schema_for::<CreateResult>()),
        ("ecad.component.inspect", json!({"component": "R2"}), output_schema_for::<InspectResult>()),
        ("ecad.component.delete", json!({"components": ["R2"]}), output_schema_for::<DeleteResult>()),
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
    let e = s.execute("ecad.component.create", &json!({"name": "R3", "nmae": "R3"})).unwrap_err().to_string();
    assert!(e.starts_with("invalid parameters for `ecad.component.create`: unknown key `nmae`"), "{e}");
}
