//! Contracts and result types for the commands in `cmd/ecad_schematic.rs`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ecad_component::{ComponentId, ComponentRef, Direction, Electrical, Number};

/// A sheet object: a part by its designator, else a frame by its title (both ignoring case), or
/// any sheet object by its id (as a reply gave it; parts, wires, labels, markers and frames share
/// one id space).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum ItemRef {
    Id(u64),
    Designator(String),
}

impl<'de> Deserialize<'de> for ItemRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::String(s) => Ok(ItemRef::Designator(s)),
            v => {
                v.as_u64().map(ItemRef::Id).ok_or_else(|| serde::de::Error::custom("must be a part's designator, a frame's title or a schematic id"))
            }
        }
    }
}

/// A part's pin given as its part and its number or name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct PinOf {
    pub part: ItemRef,
    /// The pin's number, or a name only one of its pins has.
    pub pin: Number,
}

/// A part's pin: `"R1.2"` (the designator, then the pin's number or name after the first `.`) or
/// `{part, pin}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum PinRef {
    Text(String),
    Of(PinOf),
}

impl<'de> Deserialize<'de> for PinRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match Value::deserialize(d)? {
            Value::String(s) => Ok(PinRef::Text(s)),
            v @ Value::Object(_) => serde_json::from_value(v).map(PinRef::Of).map_err(D::Error::custom),
            _ => Err(D::Error::custom("must be a pin such as \"R1.2\", or {part, pin}")),
        }
    }
}

/// A key that may be `null`: left out is `None`, `null` is `Some(None)`.
pub(super) fn nullable<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// One part to place.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct PlaceSpec {
    /// The electronic component to place; it needs a symbol.
    pub component: ComponentRef,
    /// Where its symbol's origin goes, in millimetres; left out, it goes in rows under what the
    /// sheet holds.
    pub at: Option<[f64; 2]>,
    /// Degrees counter-clockwise: 0, 90, 180 or 270 (any multiple of 90 is taken modulo 360).
    pub rotation: Option<i32>,
    /// Mirrored left to right before the turn.
    pub mirror: Option<bool>,
    /// Unique on the sheet ignoring case, 1 to 16 letters, digits or `_` from a letter; left out,
    /// the component's prefix and one more than the highest number it has on the sheet.
    pub designator: Option<String>,
    /// The part's own value; left out, it shows the component's.
    pub value: Option<String>,
    /// The component's variant it is built with, by name; left out, the component's first.
    pub variant: Option<String>,
}

/// `ecad.schematic.place`: place parts, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PlaceParams {
    pub parts: Vec<PlaceSpec>,
}

/// The fields to change of one part.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct PartEdit {
    pub part: ItemRef,
    pub designator: Option<String>,
    /// The part's own value; `null` shows the component's again.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<String>")]
    pub value: Option<Option<String>>,
    /// Another electronic component for the part: wires, labels and markers stay on the pins with
    /// the same numbers, and let go of the others where they were.
    pub component: Option<ComponentRef>,
    /// The component's variant it is built with, by name; `null` is the component's first.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<String>")]
    pub variant: Option<Option<String>>,
    /// Do not populate.
    pub dnp: Option<bool>,
    /// Where the designator text goes, from the part's `at` along the sheet's axes; `null` is
    /// the usual spot.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<[f64; 2]>")]
    pub designator_at: Option<Option<[f64; 2]>>,
    /// Where the value text goes, as `designator_at`.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<[f64; 2]>")]
    pub value_at: Option<Option<[f64; 2]>>,
}

/// `ecad.schematic.edit`: change parts, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct EditParams {
    pub parts: Vec<PartEdit>,
}

/// `ecad.schematic.move`: move sheet objects rigidly.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct MoveParams {
    /// Parts by designator, or any sheet objects by id.
    pub items: Vec<ItemRef>,
    /// How far, in millimetres.
    pub by: Option<[f64; 2]>,
    /// Where the first item's position (a part's `at`, a wire's first end, a label's or marker's
    /// point) goes, after any turn and mirror; in place of `by`.
    pub to: Option<[f64; 2]>,
    /// Degrees counter-clockwise about `about`: a multiple of 90.
    pub rotation: Option<i32>,
    /// Mirror left to right about the vertical line through `about` (before the turn).
    pub mirror: Option<bool>,
    /// The point to turn and mirror about; left out, the one item's position, or the middle of
    /// the items' positions on the 2.54 mm grid.
    pub about: Option<[f64; 2]>,
}

/// `ecad.schematic.duplicate`: copy sheet objects.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DuplicateParams {
    pub items: Vec<ItemRef>,
    /// How far the copies are from the originals, in millimetres.
    pub by: [f64; 2],
}

/// `ecad.schematic.delete`: delete sheet objects, all or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DeleteParams {
    pub items: Vec<ItemRef>,
}

/// The order parts are numbered in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnotateOrder {
    /// Top to bottom, then left to right.
    Position,
    /// The order they were placed in.
    Placement,
}

/// `ecad.schematic.annotate`: number the parts' designators.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct AnnotateParams {
    pub order: AnnotateOrder,
    /// Keep each designator that is already its component's prefix and a number, and number the
    /// others on after the highest kept.
    pub keep_existing: bool,
    /// The first number of each prefix; 1 when left out.
    pub start: Option<u64>,
}

/// `ecad.schematic.inspect`: the sheet, or what is on one part or pin. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct InspectParams {
    /// Only this part, and the wires, labels and markers on its pins.
    pub part: Option<ItemRef>,
    /// Only this pin, and the wires, labels and markers on it.
    pub pin: Option<PinRef>,
}

/// What a sheet object is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Part,
    Wire,
    Label,
    Marker,
    Frame,
}

/// A sheet object as replies name it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ItemOut {
    pub id: u64,
    pub kind: ItemKind,
    /// A part's designator, a label's name, a frame's title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The net a wire, label or marker is on (the wire, label and marker commands give it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
}

/// Which end of a wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WireEnd {
    From,
    To,
}

/// A wire end, label or marker whose pin went (its part deleted, the pin deleted or its
/// component swapped): now on the point where the pin was, or removed with it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DetachedOut {
    pub kind: ItemKind,
    pub id: u64,
    /// For a wire, the end.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<WireEnd>,
    /// The part the pin was on, by id and designator.
    pub part: u64,
    pub designator: String,
    pub pin: String,
    /// Where the pin was, in millimetres.
    pub at: [f64; 2],
    /// It went with the pin: a no-connect marker, or a wire left with no length.
    pub removed: bool,
}

/// A part's pin as it lies on the sheet.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SheetPin {
    pub number: String,
    pub name: String,
    #[serde(rename = "type")]
    pub electrical: Electrical,
    /// Where a wire connects, in millimetres.
    pub at: [f64; 2],
    /// From `at` toward the body; a wire leaves the other way.
    pub direction: Direction,
}

/// A part, with what follows from its component's symbol.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PartRecord {
    pub id: u64,
    pub designator: String,
    pub component: ComponentId,
    /// The value it shows.
    pub value: String,
    /// Whether that is the component's value (the part has none of its own).
    pub library_value: bool,
    /// The variant it is built with; null for the component's first.
    pub variant: Option<String>,
    pub at: [f64; 2],
    /// Degrees counter-clockwise.
    pub rotation: u16,
    pub mirror: bool,
    pub dnp: bool,
    /// The designator text's offset from `at`; null for the usual spot.
    pub designator_at: Option<[f64; 2]>,
    pub value_at: Option<[f64; 2]>,
    /// The box round its symbol on the sheet, pins included.
    pub bbox: Option<[[f64; 2]; 2]>,
    pub pins: Vec<SheetPin>,
}

/// What a wire end, label or marker sits on, and where that is.
#[derive(Debug, Serialize, JsonSchema)]
pub struct AnchorOut {
    /// In millimetres; on a pin, where the pin is now.
    pub at: [f64; 2],
    /// On a pin: the part, its designator and the pin's number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub designator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WireOut {
    pub id: u64,
    pub from: AnchorOut,
    pub via: Vec<[f64; 2]>,
    pub to: AnchorOut,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LabelKind {
    Net,
    Rail,
    Ground,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LabelOut {
    pub id: u64,
    pub name: String,
    pub kind: LabelKind,
    pub at: AnchorOut,
    pub rotation: u16,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    NoConnect,
    PowerFlag,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct MarkerOut {
    pub id: u64,
    pub kind: MarkerKind,
    pub at: AnchorOut,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ApprovalOut {
    pub key: String,
    pub note: String,
}

/// A rule level for a finding's code.
#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Error,
    Warning,
    Ignore,
}

/// A designator annotation changed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RenameOut {
    pub part: u64,
    pub from: String,
    pub to: String,
}

/// What the commands that change the sheet return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SheetResult {
    /// New sheet objects, in sheet order.
    pub created: Vec<ItemOut>,
    /// Sheet objects that changed (a point tied to a pin it lies on counts).
    pub changed: Vec<ItemOut>,
    pub removed: Vec<ItemOut>,
    /// The created and changed parts in full, with their pins on the sheet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PartRecord>,
    /// Wire ends, labels and markers that let go of a pin that went.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<DetachedOut>,
    /// `ecad.schematic.annotate`: the designators it changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renames: Vec<RenameOut>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
    /// `ecad.schematic.move`: the nets whose pins the move changed (empty when it changed no
    /// connection, as a move should).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub netlist_changed: Option<Vec<NetChangeOut>>,
}

/// One net whose pins an edit changed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NetChangeOut {
    /// The net's name (`net:R1.2` for an unnamed one).
    pub net: String,
    /// Pins it no longer has, as `R1.2`.
    pub lost: Vec<String>,
    /// Pins it has now that it didn't have, as `R1.2`.
    pub gained: Vec<String>,
    /// How many more pins changed than the lists show.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// What `ecad.schematic.inspect` returns, in sheet order.
#[derive(Debug, Serialize, JsonSchema)]
pub struct InspectResult {
    pub parts: Vec<PartRecord>,
    pub wires: Vec<WireOut>,
    pub labels: Vec<LabelOut>,
    pub markers: Vec<MarkerOut>,
    /// The accepted findings (left out with `part` or `pin`).
    pub approvals: Vec<ApprovalOut>,
    /// Rule levels by finding code (left out with `part` or `pin`).
    pub rules: BTreeMap<String, Level>,
    /// The section frames, in print order (left out with `part` or `pin`, and while there are
    /// none; `ecad.schematic.frame` makes them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<super::ecad_frame::FrameOut>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

#[cfg(test)]
#[path = "ecad_schematic_tests.rs"]
mod tests;
