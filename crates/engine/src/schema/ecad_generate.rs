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
    /// The library footprint; a name no footprint has makes a new one.
    pub footprint: ComponentRef,
    /// An electronic component to give the footprint to, as the footprint of `variant`.
    pub component: Option<ComponentRef>,
    /// The component's variant, by name: its first when left out, made (named after the
    /// footprint) when the component has none or none with that name.
    pub variant: Option<String>,
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

/// One pin of a generated symbol: a box's, or the number (and name) for one of a shape's pins.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct GeneratedPin {
    pub number: Number,
    /// On a shape, a name that is one of its pins' roles (`A`, `K`, `B`…) picks that pin; the
    /// shape keeps it but doesn't draw it.
    pub name: Option<String>,
    /// `passive` when left out.
    pub electrical: Option<Electrical>,
    /// A box's only: when left out, the first half of the pins go left and the rest right.
    pub side: Option<PinSide>,
}

/// A symbol style: a box, or a standard shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SymbolStyle {
    /// A box with pins round it (`pins`).
    Box,
    /// A resistor: pins 1 and 2 either side of a box.
    Resistor,
    /// A capacitor: pins 1 and 2 either side of two plates.
    Capacitor,
    /// A polarised (electrolytic) capacitor: plus (1) left, minus (2) right, one plate curved.
    PolarizedCapacitor,
    /// An inductor: four humps between pins 1 and 2.
    Inductor,
    /// A ferrite bead: a slanted filled bar across the line between pins 1 and 2.
    Ferrite,
    /// A fuse: a box with the line through it, pins 1 and 2.
    Fuse,
    /// A diode: anode (A, 2) left, cathode (K, 1) right.
    Diode,
    /// A light-emitting diode, with two light arrows; `count` 2 to 4 LEDs in one symbol share a
    /// `common` anode (pin 1, the cathodes K1… 2 on) or cathode.
    Led,
    /// A Zener diode, as a diode.
    Zener,
    /// A Schottky diode, as a diode.
    Schottky,
    /// A transient voltage suppressor: as a Zener, or `bidirectional` (pins 1 and 2).
    Tvs,
    /// A quartz crystal: pins 1 and 2; with `count` 4, pins 1 and 3 and the case pins 2 and 4.
    Crystal,
    /// A ceramic resonator with its load capacitors: pins 1 and 3, ground 2 in the middle.
    Resonator,
    /// A push button, normally open: pins 1 and 2; with `count` 4, 1 and 2 joined on the left
    /// and 3 and 4 on the right.
    Switch,
    /// A battery of two cells: plus (1) left, minus (2) right.
    Battery,
    /// An antenna: pin 1 below.
    Antenna,
    /// A test point: pin 1 below a ring.
    TestPoint,
    /// An NPN transistor: base (B, 1) left, collector (C, 3) up, emitter (E, 2) down.
    Npn,
    /// A PNP transistor, as an NPN.
    Pnp,
    /// An N-channel MOSFET: gate (G, 1) left, drain (D, 3) up, source (S, 2) down.
    Nmos,
    /// A P-channel MOSFET, as an N-channel one.
    Pmos,
}

/// Which side of several LEDs in one symbol is shared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommonPin {
    Anode,
    Cathode,
}

/// `ecad.symbol.generate`: a whole symbol, replacing the pins and the body.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SymbolParams {
    /// The library symbol; a name no symbol has makes a new one.
    pub symbol: ComponentRef,
    /// An electronic component to give the symbol to.
    pub component: Option<ComponentRef>,
    pub style: SymbolStyle,
    /// A box's pins, in the order they go down each side; for a shape, its pins' numbers (one
    /// for each, matched by role name, else in the reply's `pin_list` order).
    pub pins: Option<Vec<GeneratedPin>>,
    /// A crystal's or a switch's pins (2 or 4), or how many LEDs (1 to 4); from `pins` when left out.
    pub count: Option<u32>,
    /// Several LEDs' shared pin: `anode` when left out.
    pub common: Option<CommonPin>,
    /// A TVS conducting both ways.
    pub bidirectional: Option<bool>,
    /// The symbol's pins renumbered (old number → new): what hangs on them on the schematic, and
    /// pin maps, follow. A pin whose number goes and whose name comes back follows by name anyway.
    pub renames: Option<std::collections::BTreeMap<String, String>>,
}

/// A pin a generated symbol has.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GeneratedPinOut {
    pub number: String,
    pub name: String,
    /// The pin's place in the shape: the name it gives the pin (`A`, `K`, `+`…) or its usual
    /// number (left out for a box).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Where a wire connects, in millimetres.
    pub at: [f64; 2],
}

/// What `ecad.footprint.generate` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FootprintResult {
    pub footprint: ComponentId,
    /// Whether the call made the footprint.
    pub created: bool,
    /// With `component`: the electronic component and the variant built on it now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
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
    pub symbol: ComponentId,
    /// Whether the call made the symbol.
    pub created: bool,
    /// With `component`: the electronic component that shows it now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentId>,
    /// How many pins it made.
    pub pins: usize,
    /// Each pin it made, a shape's in its order, so pads can be mapped.
    pub pin_list: Vec<GeneratedPinOut>,
    /// The box round the symbol, pins included.
    pub bbox: Option<[[f64; 2]; 2]>,
    /// Whether a pin-to-pad map that named pins no longer there was dropped.
    pub pin_map_cleared: bool,
    /// Wire ends, labels and markers on the schematic that let go of a pin that is gone, at the
    /// point where it was (left out when there are none).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<super::ecad_schematic::DetachedOut>,
    /// The nets whose pins the edit changed on the schematic: pins that moved within their parts
    /// can land on other wires or leave their own (left out when none did).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub netlist_changed: Vec<super::ecad_schematic::NetChangeOut>,
    /// The parts whose moved designator or value went back to where the newly drawn symbol puts
    /// it, by designator (left out when none did).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub texts_reset: Vec<String>,
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
                "footprint",
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
                "variant",
            ],
        );
        assert_keys(input_schema_for::<SymbolParams>(), &["bidirectional", "common", "component", "count", "pins", "renames", "style", "symbol"]);
    }
}
