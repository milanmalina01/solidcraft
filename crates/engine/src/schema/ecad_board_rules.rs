//! Contracts and result types for the commands in `cmd/ecad_board_rules.rs`: the design rules,
//! net classes, and the impedance and current calculators.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use solidcraft_doc::ecad::CopperLayer;

use super::ecad_board::{CopperLayerParam, copper_names};
use super::ecad_schematic::nullable;

/// `ecad.board.rules`: change design rules, in millimetres; what is left out stays.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RulesParams {
    /// What new tracks get (the default class's width).
    pub track_width: Option<f64>,
    /// The least gap between copper of two nets (the default class's).
    pub clearance: Option<f64>,
    /// What new vias get (the default class's).
    pub via_diameter: Option<f64>,
    pub via_drill: Option<f64>,
    /// The narrowest track the check passes.
    pub min_track_width: Option<f64>,
    /// The narrowest copper ring round a drill.
    pub min_annular_ring: Option<f64>,
    /// The smallest drill (micro vias: `microvia_drill`).
    pub min_drill: Option<f64>,
    /// The least gap between two drilled holes' edges.
    pub hole_to_hole: Option<f64>,
    /// The least gap between copper and the board's edge or an unplated hole.
    pub copper_to_edge: Option<f64>,
    /// The least gap between silkscreen and a pad.
    pub silk_to_pad: Option<f64>,
    /// What new micro vias get; a via between neighbouring layers no wider is a micro via.
    pub microvia_diameter: Option<f64>,
    pub microvia_drill: Option<f64>,
}

/// A differential pair's width and gap, in millimetres.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct DiffPairSpec {
    pub width: f64,
    pub gap: f64,
}

/// `ecad.board.netclass`: make a net class, or change one by name.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct NetClassParams {
    /// The class (ignoring case); a new one is made when the board has none by that name.
    pub name: String,
    /// A new name for it.
    pub rename: Option<String>,
    /// Its nets, by name, in place of those it has; a net in another class moves to this one.
    pub nets: Option<Vec<String>>,
    /// Nets to put in it, besides those it has.
    pub add_nets: Option<Vec<String>>,
    /// Nets to take out of it (back to the default class).
    pub remove_nets: Option<Vec<String>>,
    /// In millimetres; a new class takes the default class's for what is left out.
    pub track_width: Option<f64>,
    pub clearance: Option<f64>,
    pub via_diameter: Option<f64>,
    pub via_drill: Option<f64>,
    /// `null` takes the pair's sizes away.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<DiffPairSpec>")]
    pub diff_pair: Option<Option<DiffPairSpec>>,
    /// The current its nets carry, in amperes, kept with the class so it remembers why it has
    /// its width (`ecad.board.current.inspect` works the width out); `null` takes it away.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<f64>")]
    pub current: Option<Option<f64>>,
    /// The temperature rise allowed at that current, in °C (10 when left out); `null` takes it
    /// away.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<f64>")]
    pub temp_rise: Option<Option<f64>>,
    /// The copper's thickness the width is for, in oz/ft² (the stack-up's when left out); `null`
    /// takes it away.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<f64>")]
    pub copper_oz: Option<Option<f64>>,
}

/// `ecad.board.netclass.delete`: a net class goes; its nets go back to the default class.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct NetClassDeleteParams {
    pub name: String,
}

/// `ecad.board.rules.inspect` takes no parameters.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RulesInspectParams {}

/// The design rules, in millimetres.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RulesOut {
    pub track_width: f64,
    pub clearance: f64,
    pub via_diameter: f64,
    pub via_drill: f64,
    pub min_track_width: f64,
    pub min_annular_ring: f64,
    pub min_drill: f64,
    pub hole_to_hole: f64,
    pub copper_to_edge: f64,
    pub silk_to_pad: f64,
    pub microvia_diameter: f64,
    pub microvia_drill: f64,
}

/// A net class with its nets.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NetClassOut {
    pub name: String,
    pub track_width: f64,
    pub clearance: f64,
    pub via_diameter: f64,
    pub via_drill: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_pair: Option<DiffPairSpec>,
    pub nets: Vec<String>,
    /// Its nets the schematic has no net for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown_nets: Vec<String>,
    /// The current its nets carry (A), the rise allowed at it (°C) and the copper (oz/ft²) its
    /// width is for, as the class keeps them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temp_rise: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copper_oz: Option<f64>,
    /// With a current: the widths the current calculator proposes for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed: Option<ProposedWidths>,
}

/// The narrowest tracks a class's current asks for, in millimetres.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProposedWidths {
    pub outer: f64,
    pub inner: f64,
    /// The rise they are for (°C), and the copper (mm) on outer and inner layers.
    pub temp_rise: f64,
    pub copper_outer: f64,
    pub copper_inner: f64,
    /// Both inside the formula's published range.
    pub valid_range: bool,
}

/// What `ecad.board.rules`, `ecad.board.netclass`, `ecad.board.netclass.delete` and
/// `ecad.board.rules.inspect` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RulesResult {
    pub rules: RulesOut,
    pub classes: Vec<NetClassOut>,
    /// The schematic's nets in no class: the default class.
    pub default_nets: Vec<String>,
    /// Nets a call moved from another class, as `net: from`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub moved: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What kind of line the impedance is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LineKindParam {
    /// An outer layer over the plane under it.
    Microstrip,
    /// An inner layer between the planes above and below.
    Stripline,
    /// An outer layer with ground beside it (`gap` away) and under it.
    Coplanar,
    /// Two edge-coupled tracks `gap` apart; the impedance is between them.
    Differential,
}

/// `ecad.board.impedance.inspect`: a track's impedance over the stack-up, or the width for an
/// impedance. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ImpedanceParams {
    pub layer: CopperLayerParam,
    pub kind: LineKindParam,
    /// The track's width in millimetres: the reply gives its impedance. In place of `target`.
    pub width: Option<f64>,
    /// Ohms: the reply gives the width for it.
    pub target: Option<f64>,
    /// For `coplanar` and `differential`: the gap in millimetres.
    pub gap: Option<f64>,
}

/// An input outside the range a formula holds in.
#[derive(Debug, Serialize, JsonSchema)]
pub struct OutOfRangeOut {
    /// `w/h`, `er`, `w/(b-t)`, `t/b` or `t/s` (w the width, h the dielectric to the plane, b the
    /// distance between the planes, t the copper's thickness, s the gap).
    pub input: String,
    pub value: f64,
    /// It holds above `low` and below `high`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high: Option<f64>,
}

/// What `ecad.board.impedance.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ImpedanceResult {
    /// Ohms (between the pair's tracks for `differential`).
    pub z0: f64,
    /// In millimetres.
    pub width: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gap: Option<f64>,
    pub kind: LineKindParam,
    /// Inside the formula's published range.
    pub valid_range: bool,
    /// The first input outside it, when one is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out_of_range: Option<OutOfRangeOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub er_eff: Option<f64>,
    /// Which formula.
    pub formula: String,
    /// The dielectric to the plane (above, for a stripline), and below for a stripline.
    pub h: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h2: Option<f64>,
    /// The copper's thickness.
    pub t: f64,
    pub er: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// The copper's thickness for the current calculator.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CopperParam {
    /// In ounces per square foot (1 oz is 0.0347 mm).
    Oz(f64),
    /// In millimetres.
    Mm(f64),
}

/// Where the current calculator's track runs: on an outer or inner layer, or on a copper layer
/// of the board.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CurrentLayerParam {
    Outer,
    Inner,
    Layer(CopperLayer),
}

impl<'de> Deserialize<'de> for CurrentLayerParam {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let bad = || D::Error::custom("must be outer, inner or a copper layer: top, inner1 … inner14, bottom");
        match Value::deserialize(d)? {
            Value::String(s) if s == "outer" => Ok(CurrentLayerParam::Outer),
            Value::String(s) if s == "inner" => Ok(CurrentLayerParam::Inner),
            Value::String(s) => CopperLayer::parse(&s).map(CurrentLayerParam::Layer).ok_or_else(bad),
            _ => Err(bad()),
        }
    }
}

impl JsonSchema for CurrentLayerParam {
    fn schema_name() -> Cow<'static, str> {
        "CurrentLayer".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let mut names = vec!["outer".to_string(), "inner".to_string()];
        names.extend(copper_names());
        json_schema!({
            "description": "outer or inner, or a copper layer of the board (top, inner1 … inner14, bottom): its copper comes from the stack-up.",
            "type": "string",
            "enum": names,
        })
    }
}

/// Which formula the current calculator uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CurrentFormulaParam {
    /// IPC-2221's chart: I = k·ΔT^0.44·A^0.725, k 0.048 outer and 0.024 inner.
    Ipc2221,
    /// Brooks and Adam's fit to IPC-2152's external data, for every layer: ΔT = 215.3·I²·W^-1.15·Th^-1
    /// (W and Th in mil); what the design-quality check `pi.current` uses.
    Ipc2152,
}

/// `ecad.board.current.inspect`: the track width a current needs (IPC-2221, or the IPC-2152
/// fit), or the temperature rise of a track's width, with its resistance, drop and loss.
/// Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CurrentParams {
    /// In amperes.
    pub current: f64,
    /// The rise above the ambient the track may warm by, in °C (10 when left out): the reply
    /// gives the width for it. In place of `width`.
    pub temp_rise: Option<f64>,
    /// In °C (25 when left out).
    pub ambient: Option<f64>,
    /// `{"oz": 1}` or `{"mm": 0.035}`; left out, the board's stack-up's copper.
    pub copper: Option<CopperParam>,
    /// `outer` (left out), `inner`, or a copper layer of the board.
    pub layer: Option<CurrentLayerParam>,
    /// The track's length in millimetres, for its resistance, voltage drop and power lost.
    pub length: Option<f64>,
    /// A track's width in millimetres: the reply gives how much it warms by. In place of
    /// `temp_rise`.
    pub width: Option<f64>,
    /// `ipc2221` (left out) or `ipc2152`.
    pub formula: Option<CurrentFormulaParam>,
}

/// What `ecad.board.current.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CurrentResult {
    /// In amperes.
    pub current: f64,
    /// The narrowest track for the current at the rise, on an outer and on an inner layer, in
    /// millimetres.
    pub width_outer: f64,
    pub width_inner: f64,
    /// On the layer asked for: the width given, or the one worked out.
    pub width: f64,
    /// °C above the ambient: the rise given, or the one a given width warms by.
    pub temp_rise: f64,
    /// °C: the ambient and the track's temperature (the ambient plus the rise).
    pub ambient: f64,
    pub temperature: f64,
    /// `outer`, `inner`, or the copper layer named.
    pub layer: String,
    /// The copper the width is for, in millimetres and oz/ft², and the outer and inner layers'
    /// (the stack-up's thinnest unless `copper` is given).
    pub copper: f64,
    pub copper_oz: f64,
    pub copper_outer: f64,
    pub copper_inner: f64,
    /// With `length`: millimetres, the resistance in ohms at `temperature`, and the volts it
    /// drops and the watts it loses at the current.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resistance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voltage_drop: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power: Option<f64>,
    /// Which formula.
    pub formula: String,
    /// The layer's answer is inside the formula's published range: IPC-2221's up to 35 A, 0.5
    /// to 3 oz, a 10 to 100 °C rise and a width up to 10 mm; the IPC-2152 fit's 1 to 3 oz and a
    /// rise up to 100 °C, held to the same 35 A and 10 mm.
    pub valid_range: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
