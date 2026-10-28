//! An electronic component's schematic symbol: its pins, each a short line whose outer end is
//! where a wire connects. The body's graphics come with a later change.

use serde::{Deserialize, Serialize};

use super::{MAX_TEXT, check_number, check_point, check_text};

/// The most pins a symbol has.
pub const MAX_PINS: usize = 4096;
/// The longest pin, in millimetres.
pub const MAX_PIN_LENGTH: f64 = 100.0;
/// A pin's length when none is given: 2.54 mm (0.1 in), the usual schematic grid.
pub const DEFAULT_PIN_LENGTH: f64 = 2.54;

/// The schematic symbol.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Symbol {
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

impl Symbol {
    pub fn is_empty(&self) -> bool {
        self.pins.is_empty() && self.graphics.is_empty()
    }

    pub fn pin(&self, number: &str) -> Option<&Pin> {
        self.pins.iter().find(|p| p.number == number)
    }

    /// The caps, each pin's rules, and unique numbers.
    pub fn validate(&self) -> Result<(), String> {
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

    /// The smallest box around every pin (both ends) and the body: `[[x0, y0], [x1, y1]]`; `None`
    /// when there is nothing.
    pub fn bbox(&self) -> Option<[[f64; 2]; 2]> {
        let body = super::graphics::graphics_bbox(&self.graphics).map(|[a, b]| vec![a, b]).unwrap_or_default();
        super::bbox(self.pins.iter().flat_map(|p| [p.at, p.body_end()]).chain(body))
    }
}
