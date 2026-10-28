//! The design's electronics: what the Electronics workspace and the `ecad.*` commands edit.
//!
//! Empty for now. Each later part (the component library, the netlist, the schematic and the
//! board) comes with its own design-format change, and unknown keys are refused rather than
//! dropped, so a build that doesn't know a part refuses the design instead of losing it on save.

use serde::{Deserialize, Serialize};

/// The design's electronics, saved under the design-file key `ecad`; nothing is saved while it is
/// empty, so a design without electronics has no `ecad` key.
///
/// Planned parts: the component library (each electronic component's symbol, footprint, 3D
/// package and pin-to-pad map), the netlist (component instances, nets and net classes), the
/// schematic (sheets, placed symbols, wires and labels) and the board (outline, stackup, placed
/// footprints, tracks, vias and pours). The 3D board is generated from them, never saved twice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Electronics {}

impl Electronics {
    /// Whether the design has no electronics, so nothing is saved for them.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// An object only, and no key is known yet: a newer build's parts are refused, never dropped. (A
/// derived reader would also take a list, as the fields in order.)
impl<'de> Deserialize<'de> for Electronics {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let keys = serde_json::Map::<String, serde_json::Value>::deserialize(d)?;
        match keys.keys().next() {
            Some(key) => Err(serde::de::Error::unknown_field(key, &[])),
            None => Ok(Electronics {}),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

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
}
