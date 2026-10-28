use std::sync::Arc;

use serde_json::json;

use crate::Session;

#[test]
fn ecad_inspect_reports_a_design_without_electronics() {
    let mut s = Session::default();
    assert_eq!(s.execute("ecad.inspect", &json!({})).unwrap(), json!({"empty": true}));
}

#[test]
fn ecad_inspect_only_reads() {
    let spec = crate::find_command("ecad.inspect").unwrap();
    assert!(!spec.undoable && !spec.reads_model && spec.tab.is_empty());
    assert!(spec.contract.is_some() && spec.output.is_some());
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length": 10, "width": 10, "height": 10})).unwrap();
    let (doc, undo, revision, dirty) = (s.doc.clone(), s.undo.len(), s.revision, s.is_dirty());
    s.execute("ecad.inspect", &json!({})).unwrap();
    assert!(Arc::ptr_eq(&doc, &s.doc), "the document is not copied or changed");
    assert_eq!((s.undo.len(), s.revision, s.is_dirty()), (undo, revision, dirty));
}

#[test]
fn ecad_commands_are_listed_and_described() {
    let mut s = Session::default();
    let listed = s.execute("engine.commands", &json!({})).unwrap();
    assert!(listed.as_array().unwrap().iter().any(|c| c["id"] == "ecad.inspect"));
    let described = s.execute("engine.describe", &json!({"command": "ecad.inspect"})).unwrap();
    assert_eq!(described["input"]["type"], json!("object"));
    assert_eq!(described["output"]["properties"]["empty"]["type"], json!("boolean"));
}

#[test]
fn a_design_without_electronics_saves_and_reopens_without_them() {
    let dir = std::env::temp_dir().join(format!("solidcraft-ecad-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("board.sc");
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length": 10, "width": 10, "height": 10})).unwrap();
    let before = s.execute("ecad.inspect", &json!({})).unwrap();
    s.execute("file.save", &json!({"path": path, "format": "json"})).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(saved.get("ecad").is_none(), "empty electronics are not saved");
    s.execute("file.new", &json!({})).unwrap();
    s.execute("doc.open", &json!({"path": path})).unwrap();
    assert!(!s.is_dirty());
    assert_eq!(s.execute("ecad.inspect", &json!({})).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}
