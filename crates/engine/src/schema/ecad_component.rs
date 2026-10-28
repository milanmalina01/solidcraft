//! Contracts and result types for the commands in `cmd/ecad_component.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// An electronic component, by its id (as a reply gave it) or by its name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ComponentRef {
    /// Its id.
    Id(u64),
    /// Its name, exactly.
    Name(String),
}

impl<'de> Deserialize<'de> for ComponentRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::String(s) => Ok(ComponentRef::Name(s)),
            v => v.as_u64().map(ComponentRef::Id).ok_or_else(|| serde::de::Error::custom("must be an electronic component's id or name")),
        }
    }
}

/// A pin or pad number: text, or a whole number (`1` is `"1"`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Number {
    Text(String),
    Whole(u64),
}

impl Number {
    pub fn text(&self) -> String {
        match self {
            Number::Text(t) => t.clone(),
            Number::Whole(n) => n.to_string(),
        }
    }
}

impl<'de> Deserialize<'de> for Number {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::String(s) => Ok(Number::Text(s)),
            v => v.as_u64().map(Number::Whole).ok_or_else(|| serde::de::Error::custom("must be a pin or pad number: text or a whole number")),
        }
    }
}

/// What a pin does electrically, for the electrical rules check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Electrical {
    Input,
    Output,
    Bidirectional,
    TriState,
    Passive,
    PowerInput,
    PowerOutput,
    OpenCollector,
    OpenEmitter,
    Unspecified,
    NotConnected,
}

/// The way a pin runs from its connecting end toward the symbol's body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Right,
    Left,
    Up,
    Down,
}

/// Which side of the board a surface-mount pad is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Top,
    Bottom,
}

/// A pad's outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Rect,
    /// A circle: both `size` sides equal.
    Round,
    /// A rectangle with half-round ends across its shorter side.
    Oval,
    /// Corners rounded by `corner` × the shorter side.
    RoundedRect,
}

/// What a pad is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Surface-mount: copper on one side, no drill.
    Smd,
    /// A plated hole with copper around it on both sides.
    ThroughHole,
    /// An unplated hole: no copper.
    Hole,
}

/// A drill: a diameter, or `[width, height]` for a slot (before the pad's rotation).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum DrillSpec {
    Round(f64),
    Slot([f64; 2]),
}

impl DrillSpec {
    pub fn sides(self) -> [f64; 2] {
        match self {
            DrillSpec::Round(d) => [d, d],
            DrillSpec::Slot(s) => s,
        }
    }
}

impl<'de> Deserialize<'de> for DrillSpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        if let Some(n) = v.as_f64() {
            return Ok(DrillSpec::Round(n));
        }
        serde_json::from_value::<[f64; 2]>(v)
            .map(DrillSpec::Slot)
            .map_err(|_| serde::de::Error::custom("must be a diameter or [width, height] in millimetres"))
    }
}

/// Repeat one pin or pad `count` times, `pitch` apart, numbering on from its number.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct RowSpec {
    /// How many, the first included: 1 to 4096.
    pub count: u32,
    /// From one to the next, in millimetres.
    pub pitch: [f64; 2],
}

/// What a graphic is; which other keys it takes follows from this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GraphicKind {
    /// `points`, joined in order.
    Line,
    /// Corners `from` and `to`.
    Rect,
    /// `center` and `radius`.
    Circle,
    /// `center`, `radius`, `start` and `sweep` in degrees (counter-clockwise when positive).
    Arc,
    /// `points`, closed.
    Polygon,
    /// `text` from `at` (its lower left), `height` tall, turned by `rotation`.
    Text,
}

/// A board layer for footprint graphics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// Printed on the board.
    Silkscreen,
    /// The part's real outline, for assembly drawings.
    Fab,
    /// The keep-out other parts' courtyards must not overlap.
    Courtyard,
}

/// One line, shape or text, in millimetres. Give the keys its `type` needs.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct GraphicSpec {
    #[serde(rename = "type")]
    pub kind: GraphicKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<Vec<[f64; 2]>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    /// Line width; 0.15 when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// Filled (rectangles, circles, polygons).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<bool>,
}

/// A footprint graphic and its board layer and side.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LayerGraphicOut {
    pub layer: Layer,
    pub side: Side,
    #[serde(flatten)]
    pub graphic: GraphicSpec,
}

/// A 3D package and what it measures in the footprint's frame.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PackageOut {
    /// The SolidCraft component; its `name` is null when it is gone.
    pub model: PackageModel,
    pub translate: [f64; 3],
    /// Degrees about z.
    pub rotation: f64,
    /// The model's bodies' box in the footprint's frame, `[[x0, y0, z0], [x1, y1, z1]]`; null with no bodies.
    pub bbox: Option<[[f64; 3]; 2]>,
    /// How far it stands above the board (the box's top), in millimetres.
    pub height: Option<f64>,
}

/// The SolidCraft component a package is.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PackageModel {
    pub id: u64,
    pub name: Option<String>,
}

/// A courtyard's box and area.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CourtyardOut {
    pub from: [f64; 2],
    pub to: [f64; 2],
    /// In square millimetres.
    pub area: f64,
}

/// `ecad.component.create`: a new electronic component, with no symbol or footprint yet.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CreateParams {
    /// Unique in the design, such as `R0603` or `LM358`; up to 64 characters.
    pub name: String,
    /// The designator prefix, 1 to 8 letters: `R` for R1, R2…; `U` when left out.
    pub prefix: Option<String>,
    /// The value printed on the schematic, such as `10k`.
    pub value: Option<String>,
    /// The manufacturer part number.
    pub mpn: Option<String>,
    pub manufacturer: Option<String>,
    /// Up to 256 characters.
    pub description: Option<String>,
}

/// `ecad.component.edit`: change the fields given; an empty text clears `value`, `mpn`,
/// `manufacturer` or `description`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct EditParams {
    /// The electronic component to change.
    pub component: ComponentRef,
    /// A new name, unique in the design.
    pub name: Option<String>,
    pub prefix: Option<String>,
    pub value: Option<String>,
    pub mpn: Option<String>,
    pub manufacturer: Option<String>,
    pub description: Option<String>,
}

/// `ecad.component.copy`: a new electronic component with the same fields and a new name.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CopyParams {
    /// The electronic component to copy.
    pub component: ComponentRef,
    /// The copy's name, unique in the design.
    pub name: String,
}

/// `ecad.component.delete`: delete electronic components, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DeleteParams {
    /// At least one, by id or name.
    pub components: Vec<ComponentRef>,
}

/// `ecad.component.map_pins`: which pads each pin connects to.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct MapPinsParams {
    pub component: ComponentRef,
    /// Pin number to the pad numbers it connects to, such as `{"2": ["2", "4"]}` for a tab; it
    /// replaces any map there was, and a pin left out connects to no pad.
    pub map: Option<std::collections::BTreeMap<String, Vec<Number>>>,
    /// Go back to connecting each pin to the pad with its number.
    pub by_number: Option<bool>,
}

/// What `ecad.component.map_pins` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MapPinsResult {
    pub component: ComponentId,
    pub connections: Vec<Connection>,
    pub unconnected_pins: Vec<String>,
    pub unconnected_pads: Vec<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// How serious a problem is: an error keeps a component from being ready to place.
#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// Something wrong or missing in an electronic component.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProblemOut {
    /// Stable: `pads_overlap`, `pin_unconnected`, `pad_outside_courtyard`, `package_missing`, `courtyard_missing`,
    /// `pad_unconnected`, `no_pins`, `no_pads`.
    pub code: String,
    pub severity: Severity,
    /// The pins or pads it is about, by number.
    pub items: Vec<String>,
    /// What is wrong and what to do.
    pub message: String,
}

/// Two pads and the gap between their copper.
#[derive(Debug, Serialize, JsonSchema)]
pub struct GapOut {
    pub pads: [String; 2],
    /// In millimetres; negative where they overlap.
    pub gap: f64,
}

/// `ecad.component.inspect`: everything saved for one electronic component. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct InspectParams {
    pub component: ComponentRef,
}

/// An electronic component as replies name it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ComponentId {
    pub id: u64,
    pub name: String,
}

/// What `ecad.component.create` and `ecad.component.copy` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CreateResult {
    /// The new electronic component.
    pub component: ComponentId,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.component.edit` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct EditResult {
    pub component: ComponentId,
    /// The fields that changed, by key; empty when the call changed nothing (then there is no
    /// undo step).
    pub changed: Vec<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.component.delete` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DeleteResult {
    pub deleted: Vec<ComponentId>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// An electronic component's saved fields, then what is measured from them; empty texts are `""`.
/// `pins` and `pads` have the shape `ecad.pin.add` and `ecad.pad.add` take.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ComponentRecord {
    pub id: u64,
    pub name: String,
    pub prefix: String,
    pub value: String,
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    /// The symbol's pins.
    pub pins: Vec<PinOut>,
    /// The footprint's pads.
    pub pads: Vec<PadOut>,
    pub measured: Measured,
    /// Each pin with the pads it connects to (those with its number).
    pub connections: Vec<Connection>,
    /// Pins with no pad, by number.
    pub unconnected_pins: Vec<String>,
    /// Pads no pin connects to, by number.
    pub unconnected_pads: Vec<String>,
    /// Pin number to pad numbers; null when each pin connects to the pad with its number.
    pub pin_map: Option<std::collections::BTreeMap<String, Vec<String>>>,
    /// The symbol's body, as `ecad.symbol.set_graphics` takes it.
    pub symbol_graphics: Vec<GraphicSpec>,
    /// The footprint's silkscreen, fabrication and courtyard graphics.
    pub footprint_graphics: Vec<LayerGraphicOut>,
    /// The 3D package; null without one.
    pub package: Option<PackageOut>,
    /// Errors first.
    pub problems: Vec<ProblemOut>,
    /// Pins, pads and no errors: ready to place.
    pub ready: bool,
}

/// A pin as `ecad.pin.add` takes it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PinOut {
    pub number: String,
    pub name: String,
    pub electrical: Electrical,
    /// Where a wire connects, in millimetres.
    pub at: [f64; 2],
    pub direction: Direction,
    pub length: f64,
}

/// A pad as `ecad.pad.add` takes it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PadOut {
    pub number: String,
    /// The centre, in millimetres.
    pub at: [f64; 2],
    /// Width and height before the rotation, in millimetres.
    pub size: [f64; 2],
    /// Degrees counter-clockwise.
    pub rotation: f64,
    pub side: Side,
    pub shape: Shape,
    /// A rounded rectangle's corner radius as a share of the shorter side; 0 otherwise.
    pub corner: f64,
    pub kind: Kind,
    /// `[w, h]` in millimetres (equal for a round hole); null for a surface-mount pad.
    pub drill: Option<[f64; 2]>,
}

/// What follows from the saved fields, for checking a component without looking at it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Measured {
    /// Where each pin meets the body.
    pub pins: Vec<PinMeasure>,
    pub pads: Vec<PadMeasure>,
    /// The box around every pin, `[[x0, y0], [x1, y1]]`; null with no pins.
    pub symbol_bbox: Option<[[f64; 2]; 2]>,
    /// The box around all the copper; null with no pads.
    pub copper_bbox: Option<[[f64; 2]; 2]>,
    /// The two pads closest together on a shared copper layer; null with fewer than two.
    pub min_pad_gap: Option<GapOut>,
    /// The top courtyard's box; null without one.
    pub courtyard: Option<CourtyardOut>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PinMeasure {
    pub number: String,
    /// The end that meets the body, in millimetres.
    pub body_end: [f64; 2],
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PadMeasure {
    pub number: String,
    /// Copper area, in square millimetres: the pad less its drill; 0 for a hole.
    pub area: f64,
    /// The box around the pad, rotation included.
    pub bbox: [[f64; 2]; 2],
    /// A through-hole pad's narrowest copper around the drill, in millimetres; null otherwise.
    pub annular_ring: Option<f64>,
}

/// A pin and the pads it connects to.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Connection {
    pub pin: String,
    pub pads: Vec<String>,
}

/// What `ecad.component.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct InspectResult {
    pub component: ComponentRecord,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
#[path = "ecad_component_tests.rs"]
mod tests;
