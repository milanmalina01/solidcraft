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

pub mod checks;
pub mod footprint;
pub mod graphics;
pub mod symbol;

pub use checks::{Gap, Problem, Severity};
pub use footprint::{Footprint, Pad, PadKind, PadShape, Side};
pub use graphics::{BoardLayer, Graphic, GraphicType};
pub use symbol::{Pin, PinDirection, PinType, Symbol};

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
/// The longest pin or pad number, in characters.
pub const MAX_NUMBER: usize = 16;
/// How far from the origin a symbol or footprint reaches, in millimetres.
pub const MAX_COORD: f64 = 1000.0;

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
    /// The schematic symbol.
    #[serde(default, skip_serializing_if = "Symbol::is_empty")]
    pub symbol: Symbol,
    /// The footprint on the board.
    #[serde(default, skip_serializing_if = "Footprint::is_empty")]
    pub footprint: Footprint,
    /// Which pads each pin connects to, by number; without one, each pin connects to the pad with
    /// its number. A pin left out of the map connects to no pad.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_map: Option<std::collections::BTreeMap<String, Vec<String>>>,
    /// The 3D package: a SolidCraft component placed on the footprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<Package>,
}

/// An electronic component's 3D package: the SolidCraft component `model`, its origin moved to
/// `translate` from the footprint's origin and turned `rotation` degrees about z. The board's top
/// is z = 0 and +z points away from it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    /// The SolidCraft component's id (never 0, the design itself).
    pub model: u64,
    #[serde(default, skip_serializing_if = "is_origin")]
    pub translate: [f64; 3],
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub rotation: f64,
}

fn is_origin(v: &[f64; 3]) -> bool {
    *v == [0.0; 3]
}

fn is_zero_f(v: &f64) -> bool {
    *v == 0.0
}

impl Package {
    pub fn validate(&self) -> Result<(), String> {
        if self.model == 0 {
            return Err("a 3D package is a SolidCraft component, not the design itself".into());
        }
        if !self.translate.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD) {
            return Err(format!("the 3D package's `translate` must be within {MAX_COORD} mm"));
        }
        if !(self.rotation.is_finite() && self.rotation.abs() <= 360.0) {
            return Err("the 3D package's `rotation` must be -360 to 360 degrees".into());
        }
        Ok(())
    }

    /// A point of the model, in the footprint's frame.
    pub fn place(&self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        let (s, c) = self.rotation.to_radians().sin_cos();
        [x * c - y * s + self.translate[0], x * s + y * c + self.translate[1], z + self.translate[2]]
    }
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

    /// Point the 3D packages modelled by a component `gone` says is going at `to` instead, or clear
    /// them (`None`).
    pub fn repoint_packages(&mut self, gone: impl Fn(u64) -> bool, to: Option<u64>) {
        for c in &mut self.components {
            if c.package.as_ref().is_some_and(|p| gone(p.model)) {
                match (to, &mut c.package) {
                    (Some(to), Some(p)) => p.model = to,
                    _ => c.package = None,
                }
            }
        }
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
        self.symbol.validate().map_err(|e| format!("electronic component `{}`: {e}", self.name))?;
        self.footprint.validate().map_err(|e| format!("electronic component `{}`: {e}", self.name))?;
        self.check_pin_map().map_err(|e| format!("electronic component `{}`: {e}", self.name))?;
        if let Some(p) = &self.package {
            p.validate().map_err(|e| format!("electronic component `{}`: {e}", self.name))?;
        }
        Ok(())
    }

    /// The pin-to-pad map names real pins and pads, and gives each pad to one pin at most.
    pub fn check_pin_map(&self) -> Result<(), String> {
        let Some(map) = &self.pin_map else { return Ok(()) };
        let mut taken = std::collections::BTreeMap::new();
        for (pin, pads) in map {
            if self.symbol.pin(pin).is_none() {
                return Err(format!("the pin map names pin `{pin}`, which the symbol doesn't have"));
            }
            if pads.len() > footprint::MAX_PADS {
                return Err(format!("pin {pin} is mapped to more than {} pads", footprint::MAX_PADS));
            }
            for pad in pads {
                if self.footprint.pad(pad).is_none() {
                    return Err(format!("the pin map gives pin {pin} pad `{pad}`, which the footprint doesn't have"));
                }
                if let Some(other) = taken.insert(pad.as_str(), pin.as_str()) {
                    return Err(format!("pad {pad} is mapped to both pin {other} and pin {pin}"));
                }
            }
        }
        Ok(())
    }

    /// The pads `pin` connects to. A hole has no copper, so it connects nothing, mapped or not.
    fn pads_of(&self, pin: &str) -> Vec<&str> {
        let copper = |pad: &&str| self.footprint.pad(pad).is_some_and(|p| p.kind != PadKind::Hole);
        match &self.pin_map {
            Some(map) => map.get(pin).map(|pads| pads.iter().map(String::as_str).filter(copper).collect()).unwrap_or_default(),
            None => self.footprint.pad(pin).filter(|p| p.kind != PadKind::Hole).map(|p| vec![p.number.as_str()]).unwrap_or_default(),
        }
    }

    /// Each pin with the pads it connects to.
    pub fn connections(&self) -> Vec<(&str, Vec<&str>)> {
        self.symbol.pins.iter().map(|pin| (pin.number.as_str(), self.pads_of(&pin.number))).collect()
    }

    /// Pins with no pad to connect to.
    pub fn unconnected_pins(&self) -> Vec<&str> {
        self.symbol.pins.iter().filter(|p| self.pads_of(&p.number).is_empty()).map(|p| p.number.as_str()).collect()
    }

    /// Copper pads no pin connects to (holes have no copper to connect).
    pub fn unconnected_pads(&self) -> Vec<&str> {
        let connected: std::collections::BTreeSet<&str> = self.symbol.pins.iter().flat_map(|p| self.pads_of(&p.number)).collect();
        self.footprint.pads.iter().filter(|p| p.kind != PadKind::Hole && !connected.contains(p.number.as_str())).map(|p| p.number.as_str()).collect()
    }

    /// Keep the pin map in step when a pin is renumbered (`to` is `Some`) or deleted (`None`).
    pub fn pin_renamed(&mut self, from: &str, to: Option<&str>) {
        if let Some(map) = &mut self.pin_map
            && let Some(pads) = map.remove(from)
            && let Some(to) = to
        {
            map.insert(to.to_string(), pads);
        }
    }

    /// Keep the pin map in step when a pad is renumbered (`to` is `Some`) or deleted (`None`).
    pub fn pad_renamed(&mut self, from: &str, to: Option<&str>) {
        if let Some(map) = &mut self.pin_map {
            for pads in map.values_mut() {
                match to {
                    Some(to) => {
                        for pad in pads.iter_mut() {
                            if pad.as_str() == from {
                                *pad = to.to_string();
                            }
                        }
                    }
                    None => pads.retain(|pad| pad.as_str() != from),
                }
            }
        }
    }
}

/// A pin or pad number: 1 to [`MAX_NUMBER`] characters, no surrounding spaces, no control
/// characters.
pub fn check_number(what: &str, number: &str) -> Result<(), String> {
    if number.is_empty() || number.trim() != number {
        return Err(format!("a {what} number must not be empty or start or end with a space (got `{number}`)"));
    }
    check_text(&format!("{what} number"), number, MAX_NUMBER)
}

/// A point within [`MAX_COORD`] of the origin on both axes.
pub fn check_point(what: &str, [x, y]: [f64; 2]) -> Result<(), String> {
    if !(x.is_finite() && y.is_finite() && x.abs() <= MAX_COORD && y.abs() <= MAX_COORD) {
        return Err(format!("{what} must be within {MAX_COORD} mm of the origin"));
    }
    Ok(())
}

/// The smallest box around `points`: `[[x0, y0], [x1, y1]]`, `None` for no points.
pub fn bbox(points: impl IntoIterator<Item = [f64; 2]>) -> Option<[[f64; 2]; 2]> {
    points.into_iter().fold(None, |acc, [x, y]| match acc {
        None => Some([[x, y], [x, y]]),
        Some([[x0, y0], [x1, y1]]) => Some([[x0.min(x), y0.min(y)], [x1.max(x), y1.max(y)]]),
    })
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
