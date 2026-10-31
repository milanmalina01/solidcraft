//! Contracts and result types for the commands in `cmd/ecad_netlist.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_component::Electrical;
use super::ecad_schematic::WireEnd;

/// `ecad.netlist.inspect`: the schematic's nets. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct InspectParams {
    /// Only this net: its name (such as GND or `net:R1.2`), or a pin on it (`R1.2`).
    pub net: Option<String>,
}

/// A pin on the schematic.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NetPinOut {
    /// The designator, a dot and the pin's number, such as `R1.2`.
    #[serde(rename = "ref")]
    pub reference: String,
    pub part: u64,
    /// The pin's number.
    pub pin: String,
    /// The pin's name, `""` when it has none.
    pub name: String,
    #[serde(rename = "type")]
    pub electrical: Electrical,
    /// Where a wire connects, in millimetres.
    pub at: [f64; 2],
}

/// One net.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NetOut {
    /// A label's or power port's name, or `net:` and its lowest pin (`net:R1.2`), or `net:wire`
    /// and its lowest wire id.
    pub name: String,
    /// Whether a label or power port names it.
    pub named: bool,
    /// Every name its labels and power ports give, sorted.
    pub names: Vec<String>,
    /// A rail or ground port is on it.
    pub power: bool,
    pub pins: Vec<NetPinOut>,
    /// Its wires', labels' and markers' ids.
    pub wires: Vec<u64>,
    pub labels: Vec<u64>,
    pub markers: Vec<u64>,
}

/// A pin inside a wire's segment, which doesn't connect to it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PinOnWireOut {
    #[serde(rename = "ref")]
    pub reference: String,
    pub part: u64,
    pub pin: String,
    pub wire: u64,
    pub at: [f64; 2],
}

/// A wire end on a point nothing else touches.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DanglingOut {
    pub wire: u64,
    pub end: WireEnd,
    pub at: [f64; 2],
}

/// What `ecad.netlist.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct InspectResult {
    /// The nets by name (N2 before N10); with `net`, that one.
    pub nets: Vec<NetOut>,
    /// Pins that touch nothing (markers aside); left out with `net`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unconnected: Vec<NetPinOut>,
    /// Pins inside a wire's segment, which don't connect to it; with `net`, those on its wires.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pin_on_wire: Vec<PinOnWireOut>,
    /// Wire ends on a point nothing else touches; with `net`, those of its wires.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dangling: Vec<DanglingOut>,
    /// Where junction dots go, in millimetres; left out with `net`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub junctions: Vec<[f64; 2]>,
    /// The sheet was past a cap, so not every connection is worked out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// A netlist file format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetlistFormat {
    /// SolidCraft's own JSON (`solidcraft.netlist`, version 1).
    Json,
}

/// `ecad.netlist.export`: the schematic's netlist as a file, or as text.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ExportParams {
    /// Left out: json.
    pub format: Option<NetlistFormat>,
    /// A .json file to write it to; left out, the reply carries the text and nothing is written.
    pub path: Option<String>,
}

/// What `ecad.netlist.export` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ExportResult {
    pub format: NetlistFormat,
    /// The netlist, when no `path` was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The file written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// How many nets (with pins) and parts it lists.
    pub nets: usize,
    pub parts: usize,
    /// The sheet was past a cap, so not every connection is in it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
