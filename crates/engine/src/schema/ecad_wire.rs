//! Contracts and result types for the commands in `cmd/ecad_wire.rs`: wires, net labels, power
//! ports and markers on the schematic.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ecad_netlist::NetOut;
use super::ecad_schematic::{ItemOut, PartRecord, PinOf, PinRef, PlaceSpec};

/// A point on a wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct OnWire {
    /// The wire's id.
    pub wire: u64,
    /// A point on it (an end, a bend or inside a segment), in millimetres.
    pub at: [f64; 2],
}

/// Where a wire end, label or marker goes: a pin (`"R1.2"` or `{part, pin}`), a point `[x, y]` in
/// millimetres (a point on a pin is that pin), or a point on a wire `{wire, at}`.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum End {
    Pin(PinRef),
    Point([f64; 2]),
    OnWire(OnWire),
}

impl<'de> Deserialize<'de> for End {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match Value::deserialize(d)? {
            Value::String(s) => Ok(End::Pin(PinRef::Text(s))),
            v @ Value::Array(_) => serde_json::from_value(v).map(End::Point).map_err(D::Error::custom),
            v @ Value::Object(_) if v.get("wire").is_some() => serde_json::from_value(v).map(End::OnWire).map_err(D::Error::custom),
            v @ Value::Object(_) => serde_json::from_value::<PinOf>(v).map(|p| End::Pin(PinRef::Of(p))).map_err(D::Error::custom),
            _ => Err(D::Error::custom("must be a pin such as \"R1.2\" or {part, pin}, a point [x, y], or a point on a wire {wire, at}")),
        }
    }
}

/// How a new wire runs between its points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// Out of each pin 2.54 mm, then an L (horizontal first unless its corner is inside a part),
    /// a Z for pins facing each other, or straight when the points line up.
    #[default]
    Auto,
    /// Straight from point to point, as given.
    Straight,
}

/// One wire.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct WireSpec {
    pub from: End,
    /// Points the wire passes through, in order, in millimetres.
    pub via: Option<Vec<[f64; 2]>>,
    pub to: End,
    /// `auto` when left out.
    pub route: Option<Route>,
}

/// `ecad.schematic.wire`: draw wires, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct WireParams {
    pub wires: Vec<WireSpec>,
}

/// `ecad.schematic.wire.edit`: give a wire new bends; its ends stay where they are.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct WireEditParams {
    /// The wire's id.
    pub wire: u64,
    /// The bends `[x, y]` in millimetres, in order from its first end; empty for a straight wire.
    pub via: Vec<[f64; 2]>,
}

/// How `connect` joins the pins.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectBy {
    /// A wire from each pin to the next, auto-routed.
    #[default]
    Wires,
    /// A 2.54 mm wire out of each pin, ending in a net label `name`.
    Labels,
}

/// Pins to put on one net.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ConnectSpec {
    /// In the order the wires chain them.
    pub pins: Vec<PinRef>,
    /// The net's name: a net label on the first pin (`by: wires`), or on every stub (`by: labels`,
    /// where it is needed).
    pub name: Option<String>,
    /// `wires` when left out.
    pub by: Option<ConnectBy>,
}

/// `ecad.schematic.connect`: put pins on nets, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ConnectParams {
    pub nets: Vec<ConnectSpec>,
}

/// One net label.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct LabelSpec {
    /// 1 to 64 characters with no spaces or `:`; labels with the same name are one net.
    pub name: String,
    pub at: End,
    /// The way it points, degrees counter-clockwise (a multiple of 90); left out, away from the
    /// pin it is on, or 0.
    pub rotation: Option<i32>,
}

/// `ecad.schematic.label`: put net labels on the sheet, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct LabelParams {
    pub labels: Vec<LabelSpec>,
}

/// How a power port is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PowerStyle {
    /// A supply rail, such as +3V3.
    Rail,
    /// Ground.
    Ground,
}

/// One power port.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct PowerSpec {
    /// The rail's name, such as +3V3 or GND; ports and labels with the same name are one net.
    pub name: String,
    pub at: End,
    /// The way it points, degrees counter-clockwise (a multiple of 90); left out, up for a rail
    /// and down for ground.
    pub rotation: Option<i32>,
    /// Left out: ground for names starting GND or VSS and for 0V, a rail otherwise.
    pub style: Option<PowerStyle>,
}

/// `ecad.schematic.power`: put power ports on the sheet, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PowerParams {
    pub ports: Vec<PowerSpec>,
}

/// A marker's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    /// The pin is left unconnected on purpose; it goes on a pin.
    NoConnect,
    /// The net is powered from off the sheet.
    PowerFlag,
}

/// One marker.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct MarkerSpec {
    /// Left out: the command's own kind (a no-connect for `ecad.schematic.marker` and
    /// `ecad.schematic.add`, a power flag for `ecad.schematic.marker.power_flag`).
    pub kind: Option<MarkerKind>,
    pub at: End,
}

/// `ecad.schematic.marker`: put markers on the sheet, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct MarkerParams {
    pub markers: Vec<MarkerSpec>,
}

/// `ecad.schematic.add`: parts, wires, connections, labels, power ports and markers in one call
/// and one undo step, in that order, so later rows can name the parts placed here.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct AddParams {
    pub parts: Option<Vec<PlaceSpec>>,
    pub wires: Option<Vec<WireSpec>>,
    pub connect: Option<Vec<ConnectSpec>>,
    pub labels: Option<Vec<LabelSpec>>,
    pub power: Option<Vec<PowerSpec>>,
    pub markers: Option<Vec<MarkerSpec>>,
}

/// `ecad.schematic.net.rename`: give a net a name.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RenameParams {
    /// The net's name now (an unnamed one's too, such as `net:R1.2`), or a pin on it (`R1.2`).
    pub net: String,
    /// The new name: 1 to 64 characters with no spaces or `:`.
    pub to: String,
    /// When another net already has the name `to`, join the two; refused otherwise.
    pub merge: Option<bool>,
}

/// Two wires crossing without connecting.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CrossOut {
    /// The new wire.
    pub wire: u64,
    /// The wire it crosses.
    pub other: u64,
    /// Where, in millimetres.
    pub at: [f64; 2],
}

/// A net renamed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NetRenameOut {
    pub from: String,
    pub to: String,
    /// The net that had the name `to` already, now joined to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub joined: Option<String>,
}

/// What the commands that put wires, labels and markers on the sheet return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct WireResult {
    /// New sheet objects, in sheet order; wires, labels and markers with their net.
    pub created: Vec<ItemOut>,
    /// Sheet objects that changed.
    pub changed: Vec<ItemOut>,
    pub removed: Vec<ItemOut>,
    /// The created and changed parts in full, with their pins on the sheet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PartRecord>,
    /// The nets the created and changed objects are on, in full.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nets: Vec<NetOut>,
    /// Where new wires cross other wires without connecting (at most 64).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crosses: Vec<CrossOut>,
    /// `ecad.schematic.net.rename`: the rename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renamed: Option<NetRenameOut>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
