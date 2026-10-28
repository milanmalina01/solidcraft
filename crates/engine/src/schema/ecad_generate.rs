//! Contracts and result types for the commands in `cmd/ecad_generate.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_component::{ComponentId, ComponentRef, CourtyardOut, Electrical, Number};

/// A package family a footprint can be made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Package {
    /// Two-terminal chip resistor or capacitor, by EIA size (`size`, such as `0603`).
    Chip,
    /// Three gull-wing leads (JEDEC TO-236).
    Sot23,
    /// Gull-wing leads in two rows (`pins`).
    Soic,
    /// No-lead terminals on four sides (`pins`, `body`), with an optional exposed pad (`thermal`).
    Qfn,
    /// Through-hole pins in two rows (`pins`).
    Dip,
    /// Through-hole pins in one or two rows (`pins`, `rows`).
    Header,
}

/// `ecad.footprint.generate`: a whole footprint for a standard package, replacing the pads and the
/// fabrication, silkscreen and courtyard graphics. Leave out what the package's defaults give.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FootprintParams {
    pub component: ComponentRef,
    pub package: Package,
    /// A chip's EIA size code in inches: 0201, 0402, 0603, 0805, 1206, 1210, 2010, 2512.
    pub size: Option<String>,
    /// How many pins (leads, terminals) in all, the exposed pad left out.
    pub pins: Option<u32>,
    /// From one pin to the next, in millimetres.
    pub pitch: Option<f64>,
    /// Lead span tip to tip (SOT-23, SOIC), or the distance between two rows of through-hole pins.
    pub span: Option<f64>,
    /// A lead's (or terminal's) length along the row's axis, in millimetres.
    pub lead: Option<f64>,
    /// A lead's width, in millimetres.
    pub lead_width: Option<f64>,
    /// The body's size `[x, y]` in millimetres.
    pub body: Option<[f64; 2]>,
    /// A QFN's exposed pad `[w, h]`, numbered after the last pin.
    pub thermal: Option<[f64; 2]>,
    /// A header's rows: 1 or 2.
    pub rows: Option<u32>,
    /// A through-hole pin's drill, in millimetres.
    pub drill: Option<f64>,
    /// A through-hole pin's pad diameter, in millimetres.
    pub pad: Option<f64>,
    /// How far a pad reaches past a lead's tip, in millimetres.
    pub toe: Option<f64>,
    /// How far a pad reaches inside a lead's inner end, in millimetres.
    pub heel: Option<f64>,
    /// How far a pad reaches past each side of a lead, in millimetres.
    pub side: Option<f64>,
    /// The courtyard's margin, in millimetres; 0.25 when left out.
    pub margin: Option<f64>,
}

/// Where a generated symbol pin goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PinSide {
    Left,
    Right,
    Top,
    Bottom,
}

/// One pin of a generated box symbol.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct GeneratedPin {
    pub number: Number,
    pub name: Option<String>,
    /// `passive` when left out.
    pub electrical: Option<Electrical>,
    /// When left out, the first half of the pins go left and the rest right.
    pub side: Option<PinSide>,
}

/// A symbol style.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SymbolStyle {
    /// A box with pins round it (`pins`).
    Box,
    /// A resistor: pins 1 and 2 either side of a box.
    Resistor,
    /// A capacitor: pins 1 and 2 either side of two plates.
    Capacitor,
}

/// `ecad.symbol.generate`: a whole symbol, replacing the pins and the body.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SymbolParams {
    pub component: ComponentRef,
    pub style: SymbolStyle,
    /// A box's pins, in the order they go down each side.
    pub pins: Option<Vec<GeneratedPin>>,
}

/// What `ecad.footprint.generate` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FootprintResult {
    pub component: ComponentId,
    /// How many pads it made.
    pub pads: usize,
    pub courtyard: CourtyardOut,
    /// Whether a pin-to-pad map that named pads no longer there was dropped.
    pub pin_map_cleared: bool,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.symbol.generate` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SymbolResult {
    pub component: ComponentId,
    /// How many pins it made.
    pub pins: usize,
    /// The box round the symbol, pins included.
    pub bbox: Option<[[f64; 2]; 2]>,
    /// Whether a pin-to-pad map that named pins no longer there was dropped.
    pub pin_map_cleared: bool,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{FootprintParams, SymbolParams};
    use crate::schema::input_schema_for;
    use crate::schema::tests::assert_keys;

    #[test]
    fn schemas_and_the_key_check_agree() {
        assert_keys(
            input_schema_for::<FootprintParams>(),
            &[
                "body",
                "component",
                "drill",
                "heel",
                "lead",
                "lead_width",
                "margin",
                "package",
                "pad",
                "pins",
                "pitch",
                "rows",
                "side",
                "size",
                "span",
                "thermal",
                "toe",
            ],
        );
        assert_keys(input_schema_for::<SymbolParams>(), &["component", "pins", "style"]);
    }
}
