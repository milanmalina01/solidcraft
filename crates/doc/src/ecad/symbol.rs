//! A schematic symbol in the design's library: its pins, each a short line whose outer end is
//! where a wire connects, and its body's graphics. Electronic components use a symbol by id, so
//! one symbol (a resistor's) serves many components.

use serde::{Deserialize, Serialize};

use super::{MAX_DESCRIPTION, MAX_ID, MAX_TEXT, check_item_name, check_number, check_point, check_text};

/// The most pins a symbol has.
pub const MAX_PINS: usize = 4096;
/// The longest pin, in millimetres.
pub const MAX_PIN_LENGTH: f64 = 100.0;
/// A pin's length when none is given: 2.54 mm (0.1 in), the usual schematic grid.
pub const DEFAULT_PIN_LENGTH: f64 = 2.54;

/// A schematic symbol.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolItem {
    /// Never 0; symbols, footprints, 3D packages and electronic components share one id space.
    pub id: u64,
    /// Unique among the symbols, such as `Resistor` or `LM358`.
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<Pin>,
    /// The body: lines, shapes and text, with no layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphics: Vec<super::Graphic>,
}

/// One pin: a line from `at`, where a wire connects, `length` long toward the body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    /// Unique in the symbol, such as `1` or `A3`; the footprint's pad with the same number is the
    /// one it connects to.
    pub number: String,
    /// What the pin does, such as `VCC` or `OUT`; several pins may share a name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The name is kept but not drawn: the body's shape says it (a diode's `A` and `K`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub name_hidden: bool,
    #[serde(default, skip_serializing_if = "PinType::is_passive")]
    pub electrical: PinType,
    /// Where a wire connects, in millimetres.
    pub at: [f64; 2],
    /// From `at` toward the body.
    pub direction: PinDirection,
    pub length: f64,
}

/// What a pin does electrically, for the electrical rules check.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinType {
    Input,
    Output,
    Bidirectional,
    TriState,
    #[default]
    Passive,
    PowerInput,
    PowerOutput,
    OpenCollector,
    OpenEmitter,
    Unspecified,
    NotConnected,
}

impl PinType {
    fn is_passive(&self) -> bool {
        *self == PinType::Passive
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// The way a pin runs from its connecting end toward the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinDirection {
    Right,
    Left,
    Up,
    Down,
}

impl PinDirection {
    /// The unit step, x right and y up.
    pub fn unit(self) -> [f64; 2] {
        match self {
            PinDirection::Right => [1.0, 0.0],
            PinDirection::Left => [-1.0, 0.0],
            PinDirection::Up => [0.0, 1.0],
            PinDirection::Down => [0.0, -1.0],
        }
    }
}

impl Pin {
    /// The end that meets the body.
    pub fn body_end(&self) -> [f64; 2] {
        let [dx, dy] = self.direction.unit();
        [self.at[0] + dx * self.length, self.at[1] + dy * self.length]
    }

    /// Whether the sheet draws its name: it has one other than its number, not hidden.
    pub fn shows_name(&self) -> bool {
        !self.name_hidden && !self.name.is_empty() && self.name != self.number
    }

    pub fn validate(&self) -> Result<(), String> {
        check_number("pin", &self.number)?;
        check_text("name", &self.name, MAX_TEXT)?;
        check_point(&format!("pin {} `at`", self.number), self.at)?;
        if !(self.length.is_finite() && self.length > 0.0 && self.length <= MAX_PIN_LENGTH) {
            return Err(format!("pin {}: `length` must be more than 0 and at most {MAX_PIN_LENGTH} mm", self.number));
        }
        Ok(())
    }
}

/// What a component without a symbol shows: nothing.
pub static NO_SYMBOL: SymbolItem = SymbolItem { id: 0, name: String::new(), description: String::new(), pins: Vec::new(), graphics: Vec::new() };

impl SymbolItem {
    /// Whether it has nothing to draw: no pins and no body.
    pub fn is_empty(&self) -> bool {
        self.pins.is_empty() && self.graphics.is_empty()
    }

    pub fn pin(&self, number: &str) -> Option<&Pin> {
        self.pins.iter().find(|p| p.number == number)
    }

    /// The renumbering from this symbol to `new`: `given` (from → to), then each pin `new` doesn't
    /// number whose name (as written) no other pin here has, to the one pin of `new` with that name
    /// when that pin's number is new too and nothing else goes to it. So a symbol redrawn with
    /// other numbers keeps what hangs on its pins by name.
    pub fn renames_to(&self, new: &SymbolItem, given: &[(String, String)]) -> Vec<(String, String)> {
        let mut out = given.to_vec();
        let named = |pins: &[Pin], name: &str| pins.iter().filter(|p| p.name == name).count();
        for p in &self.pins {
            if p.name.is_empty() || new.pin(&p.number).is_some() || out.iter().any(|(from, _)| *from == p.number) || named(&self.pins, &p.name) != 1 {
                continue;
            }
            let mut same = new.pins.iter().filter(|q| q.name == p.name);
            let (Some(q), None) = (same.next(), same.next()) else { continue };
            if self.pin(&q.number).is_none() && !out.iter().any(|(_, to)| *to == q.number) {
                out.push((p.number.clone(), q.number.clone()));
            }
        }
        out
    }

    /// The record's own rules (an id, a name, a description within its length), the caps, each
    /// pin's rules, and unique numbers.
    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0 || self.id >= MAX_ID {
            return Err(format!("symbol `{}` has the id {}, which is out of range", self.name, self.id));
        }
        check_item_name("symbol", &self.name)?;
        check_text("description", &self.description, MAX_DESCRIPTION)?;
        self.validate_drawing().map_err(|e| format!("symbol `{}`: {e}", self.name))
    }

    /// The pins and the body alone: the caps, each pin's rules, and unique numbers.
    pub fn validate_drawing(&self) -> Result<(), String> {
        if self.pins.len() > MAX_PINS {
            return Err(format!("a symbol has at most {MAX_PINS} pins"));
        }
        let mut numbers = std::collections::BTreeSet::new();
        for p in &self.pins {
            p.validate()?;
            if !numbers.insert(p.number.as_str()) {
                return Err(format!("two pins are numbered `{}`", p.number));
            }
        }
        super::graphics::validate_all(&self.graphics, false).map_err(|e| format!("symbol {e}"))
    }

    /// What is missing: a symbol with no pins can't connect to anything.
    pub fn problems(&self) -> Vec<super::Problem> {
        if !self.pins.is_empty() {
            return Vec::new();
        }
        vec![super::Problem {
            code: "no_pins",
            severity: super::Severity::Warning,
            items: Vec::new(),
            message: format!("symbol `{}` has no pins yet: add them with ecad.pin.add or ecad.symbol.generate", self.name),
        }]
    }

    /// The smallest box around every pin (both ends) and the body: `[[x0, y0], [x1, y1]]`; `None`
    /// when there is nothing.
    pub fn bbox(&self) -> Option<[[f64; 2]; 2]> {
        let body = super::graphics::graphics_bbox(&self.graphics).map(|[a, b]| vec![a, b]).unwrap_or_default();
        super::bbox(self.pins.iter().flat_map(|p| [p.at, p.body_end()]).chain(body))
    }
}
