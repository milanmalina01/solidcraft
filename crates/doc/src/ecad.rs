//! The design's electronics: what the Electronics workspace and the `ecad.*` commands edit.
//!
//! Each part (the electronic components, then their symbols, footprints and 3D packages, later
//! the schematic and the board) comes with its own design-format change, and unknown keys are
//! refused rather than dropped, so a build that doesn't know a part refuses the design instead of
//! losing it on save.
//!
//! Conventions for everything saved here: lengths in millimetres whatever the design's units;
//! x right and y up, seen from the top of the board; angles in degrees, counter-clockwise.

use serde::{Deserialize, Serialize};

/// The most electronic components a design holds.
pub const MAX_COMPONENTS: usize = 4096;
/// The longest name, value, manufacturer part number or manufacturer, in characters.
pub const MAX_TEXT: usize = 64;
/// The longest description, in characters.
pub const MAX_DESCRIPTION: usize = 256;
/// The longest designator prefix (`R`, `C`, `U`, `LED`…), in letters.
pub const MAX_PREFIX: usize = 8;
/// Ids stay below this, so every JSON reader keeps them exact.
pub const MAX_ID: u64 = 1 << 53;

/// The design's electronics, saved under the design-file key `ecad`; nothing is saved while it is
/// empty, so a design without electronics has no `ecad` key.
///
/// Planned parts: each electronic component's symbol, footprint, 3D package and pin-to-pad map,
/// then the netlist, the schematic and the board. The 3D board is generated from them, never
/// saved twice.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Electronics {
    /// The electronic components, in the order they were made.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ElectronicComponent>,
    /// The id the next electronic component gets; ids are never reused.
    #[serde(skip_serializing_if = "is_zero")]
    pub next_id: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// A part that goes on a circuit board (a resistor, a chip, a connector), with what a parts list
/// needs to buy it. Not a SolidCraft component, which is a container of bodies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectronicComponent {
    /// Never 0, and unique in the design.
    pub id: u64,
    /// Unique in the design, such as `R0603` or `LM358`.
    pub name: String,
    /// The letters that start each placed copy's designator: `R` for R1, R2….
    pub prefix: String,
    /// The value printed on the schematic, such as `10k` or `100nF`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    /// The manufacturer part number.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mpn: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manufacturer: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl Electronics {
    /// Whether nothing is saved for the electronics (no `ecad` key in the design file).
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether the design has electronics to show. The id counter alone, left after the last
    /// electronic component is deleted, is none.
    pub fn has_content(&self) -> bool {
        !self.components.is_empty()
    }

    /// The electronic component with this id.
    pub fn component(&self, id: u64) -> Option<&ElectronicComponent> {
        self.components.iter().find(|c| c.id == id)
    }

    /// The electronic component with this name (exactly).
    pub fn named(&self, name: &str) -> Option<&ElectronicComponent> {
        self.components.iter().find(|c| c.name == name)
    }

    /// A new electronic component's id, which no other one has had in this design.
    pub fn take_id(&mut self) -> Result<u64, String> {
        let id = self.next_id.max(1).max(self.components.iter().map(|c| c.id.saturating_add(1)).max().unwrap_or(1));
        if id >= MAX_ID {
            return Err("no electronic component ids are left in this design".into());
        }
        self.next_id = id + 1;
        Ok(id)
    }

    /// Everything the design file and the commands require: the caps, unique ids and names, and
    /// each record's own rules (see [`ElectronicComponent::validate`]).
    pub fn validate(&self) -> Result<(), String> {
        if self.components.len() > MAX_COMPONENTS {
            return Err(format!("a design holds at most {MAX_COMPONENTS} electronic components"));
        }
        if self.next_id > MAX_ID {
            return Err(format!("the electronic component id counter {} is out of range", self.next_id));
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut names = std::collections::BTreeSet::new();
        for c in &self.components {
            c.validate()?;
            if !ids.insert(c.id) {
                return Err(format!("two electronic components have the id {}", c.id));
            }
            if !names.insert(c.name.as_str()) {
                return Err(format!("two electronic components are named `{}`", c.name));
            }
        }
        Ok(())
    }
}

impl ElectronicComponent {
    /// The record's own rules: a name, a designator prefix and texts within their lengths.
    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0 || self.id >= MAX_ID {
            return Err(format!("electronic component `{}` has the id {}, which is out of range", self.name, self.id));
        }
        check_name(&self.name)?;
        check_prefix(&self.prefix)?;
        for (what, text, max) in [
            ("value", &self.value, MAX_TEXT),
            ("mpn", &self.mpn, MAX_TEXT),
            ("manufacturer", &self.manufacturer, MAX_TEXT),
            ("description", &self.description, MAX_DESCRIPTION),
        ] {
            check_text(what, text, max)?;
        }
        Ok(())
    }
}

/// An electronic component's name: 1 to [`MAX_TEXT`] characters, no surrounding spaces and no
/// line breaks or other control characters.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("an electronic component needs a name".into());
    }
    if name.trim() != name {
        return Err(format!("the name `{name}` starts or ends with a space"));
    }
    check_text("name", name, MAX_TEXT)
}

/// A designator prefix: 1 to [`MAX_PREFIX`] ASCII letters.
pub fn check_prefix(prefix: &str) -> Result<(), String> {
    if prefix.is_empty() || prefix.len() > MAX_PREFIX || !prefix.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(format!("the designator prefix `{prefix}` must be 1 to {MAX_PREFIX} letters, such as R, C, U or LED"));
    }
    Ok(())
}

/// A text of at most `max` characters, with no line breaks or other control characters.
pub fn check_text(what: &str, text: &str, max: usize) -> Result<(), String> {
    if text.chars().count() > max {
        return Err(format!("`{what}` is longer than {max} characters"));
    }
    if text.chars().any(char::is_control) {
        return Err(format!("`{what}` has a line break or another control character"));
    }
    Ok(())
}

/// What the file holds under `ecad`: an object with these keys and no others.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Saved {
    components: Vec<ElectronicComponent>,
    next_id: u64,
}

/// An object only (a derived reader would also take a list, as the fields in order), with only
/// the keys this build knows: a newer build's parts are refused, never dropped. The records are
/// checked as the commands check them, and the id counter never falls behind an id in use.
impl<'de> Deserialize<'de> for Electronics {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let object = serde_json::Map::<String, serde_json::Value>::deserialize(d)?;
        let saved: Saved = serde_json::from_value(serde_json::Value::Object(object)).map_err(D::Error::custom)?;
        let mut out = Electronics { components: saved.components, next_id: saved.next_id };
        out.validate().map_err(D::Error::custom)?;
        let floor = out.components.iter().map(|c| c.id + 1).max().unwrap_or(0);
        out.next_id = out.next_id.max(floor);
        Ok(out)
    }
}

#[cfg(test)]
#[path = "ecad_tests.rs"]
mod tests;
