//! Contracts and result types for the commands in `cmd/ecad_library.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_component::{
    ComponentId, ComponentRef, CourtyardOut, GapOut, GraphicSpec, LayerGraphicOut, Number, PackageModel, PadMeasure, PadOut, PinMeasure, PinOut,
    ProblemOut,
};

/// `ecad.symbol.create`: a new, empty library symbol.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SymbolCreateParams {
    /// Unique among the symbols, such as `Resistor`; up to 64 characters.
    pub name: String,
    /// Up to 256 characters.
    pub description: Option<String>,
    /// An electronic component to give it to: the new symbol becomes the one it shows.
    pub component: Option<ComponentRef>,
}

/// `ecad.footprint.create`: a new, empty library footprint.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FootprintCreateParams {
    /// Unique among the footprints, such as `SOT-23`; up to 64 characters.
    pub name: String,
    /// Up to 256 characters.
    pub description: Option<String>,
    /// An electronic component to give it to, as the footprint of `variant`.
    pub component: Option<ComponentRef>,
    /// The component's variant, by name: its first when left out, made (named after the
    /// footprint) when the component has none or none with that name.
    pub variant: Option<String>,
}

/// `ecad.symbol.edit`: rename a library symbol, or change its description (an empty text clears it).
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SymbolEditParams {
    pub symbol: ComponentRef,
    pub name: Option<String>,
    pub description: Option<String>,
}

/// `ecad.footprint.edit`: rename a library footprint, or change its description (an empty text
/// clears it).
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FootprintEditParams {
    pub footprint: ComponentRef,
    pub name: Option<String>,
    pub description: Option<String>,
}

/// `ecad.symbol.copy`: a new library symbol with the same pins and body and a new name.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SymbolCopyParams {
    pub symbol: ComponentRef,
    /// The copy's name, unique among the symbols.
    pub name: String,
}

/// `ecad.footprint.copy`: a new library footprint with the same pads and graphics and a new name.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FootprintCopyParams {
    pub footprint: ComponentRef,
    /// The copy's name, unique among the footprints.
    pub name: String,
}

/// `ecad.symbol.delete`: delete a library symbol.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SymbolDeleteParams {
    pub symbol: ComponentRef,
    /// Clear the components' references to it (and their pin maps); without it, a symbol a
    /// component uses is refused. A component placed on the schematic is refused either way.
    pub force: Option<bool>,
}

/// `ecad.footprint.delete`: delete a library footprint.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FootprintDeleteParams {
    pub footprint: ComponentRef,
    /// Clear the variants' references to it (and their pin maps); without it, a footprint a
    /// variant uses is refused.
    pub force: Option<bool>,
}

/// `ecad.symbol.inspect`: everything saved for one library symbol. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SymbolInspectParams {
    pub symbol: ComponentRef,
}

/// `ecad.footprint.inspect`: everything saved for one library footprint. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FootprintInspectParams {
    pub footprint: ComponentRef,
}

/// One pad of a table read from a datasheet, as seen from the top.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct VerifyPad {
    /// The pad's number; unique in the table.
    pub number: Number,
    /// The pad's centre, in millimetres, from any origin the table likes.
    pub at: [f64; 2],
    /// Its extent along x and along y, in millimetres. A pad the footprint turns compares by its turned extent.
    pub size: [f64; 2],
}

/// `ecad.footprint.verify`: compare a table of pads with a library footprint. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FootprintVerifyParams {
    pub footprint: ComponentRef,
    /// The table's pads; at most one of each number.
    pub pads: Vec<VerifyPad>,
    /// How far a value may differ, in millimetres; 0.05 when left out, at most 1.
    pub tolerance: Option<f64>,
    /// The part's body `[w, h]` in millimetres, to compare with the box of the footprint's fabrication outline.
    pub body: Option<[f64; 2]>,
}

/// `ecad.library.inspect` takes no parameters. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct LibraryInspectParams {}

/// A variant of an electronic component, as replies name it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct VariantUse {
    pub component: ComponentId,
    pub variant: String,
}

/// What `ecad.symbol.create` and `ecad.symbol.copy` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SymbolResult {
    pub symbol: ComponentId,
    /// With `component`: the electronic component that shows it now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentId>,
    /// With `component`: whether a pin map that named pins the new symbol doesn't have was dropped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pin_map_cleared: bool,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.footprint.create` and `ecad.footprint.copy` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FootprintResult {
    pub footprint: ComponentId,
    /// With `component`: the electronic component and the variant built on it now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<ComponentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// With `component`: whether a pin map that named pads the new footprint doesn't have was dropped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pin_map_cleared: bool,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.symbol.edit` and `ecad.footprint.edit` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ItemEditResult {
    /// The symbol or footprint, after the edit.
    pub item: ComponentId,
    /// The fields that changed; empty when nothing did (then there is no undo step).
    pub changed: Vec<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.symbol.delete` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SymbolDeleteResult {
    pub deleted: ComponentId,
    /// With `force`: the electronic components that showed it and now have no symbol.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleared: Vec<ComponentId>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.footprint.delete` and `ecad.package.delete` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ItemDeleteResult {
    pub deleted: ComponentId,
    /// With `force`: the variants that used it and now have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cleared: Vec<VariantUse>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// A library symbol's saved fields, then what is measured from them.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SymbolRecord {
    pub id: u64,
    pub name: String,
    pub description: String,
    pub pins: Vec<PinOut>,
    /// The body, as `ecad.symbol.set_graphics` takes it.
    pub graphics: Vec<GraphicSpec>,
    /// Where each pin meets the body.
    pub body_ends: Vec<PinMeasure>,
    /// The box around every pin and the body, `[[x0, y0], [x1, y1]]`; null when it is empty.
    pub bbox: Option<[[f64; 2]; 2]>,
    pub problems: Vec<ProblemOut>,
    /// The electronic components that show it.
    pub used_by: Vec<ComponentId>,
}

/// A library footprint's saved fields, then what is measured from them.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FootprintRecord {
    pub id: u64,
    pub name: String,
    pub description: String,
    pub pads: Vec<PadOut>,
    /// The silkscreen, fabrication and courtyard graphics.
    pub graphics: Vec<LayerGraphicOut>,
    pub measured: Vec<PadMeasure>,
    /// The box around all the copper; null with no pads.
    pub copper_bbox: Option<[[f64; 2]; 2]>,
    /// The two pads closest together on a shared copper layer; null with fewer than two.
    pub min_pad_gap: Option<GapOut>,
    /// The top courtyard's box; null without one.
    pub courtyard: Option<CourtyardOut>,
    /// Errors first.
    pub problems: Vec<ProblemOut>,
    /// Pads, a courtyard and no errors.
    pub ready: bool,
    /// The variants built on it.
    pub used_by: Vec<VariantUse>,
}

/// What `ecad.symbol.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SymbolInspectResult {
    pub symbol: SymbolRecord,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.footprint.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FootprintInspectResult {
    pub footprint: FootprintRecord,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Which value of a pad differs.
#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerifyField {
    At,
    Size,
}

/// A pad's value that differs from the table's by more than the tolerance.
#[derive(Debug, Serialize, JsonSchema)]
pub struct VerifyMismatch {
    pub number: String,
    pub field: VerifyField,
    /// The table's value; a centre with `offset` added.
    pub table: [f64; 2],
    /// The footprint's value; a size is the pad's extent with its rotation.
    pub footprint: [f64; 2],
    /// A centre's distance from the table's, or the larger of a size's two differences, in millimetres.
    pub difference: f64,
}

/// What, done to the table, makes it match the footprint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub enum VerifyOrientation {
    /// x becomes -x: the usual sign of a bottom view.
    #[serde(rename = "mirror_x")]
    MirrorX,
    /// y becomes -y.
    #[serde(rename = "mirror_y")]
    MirrorY,
    /// Turned 90 degrees counter-clockwise.
    #[serde(rename = "rotate_90")]
    Rotate90,
    #[serde(rename = "rotate_180")]
    Rotate180,
    #[serde(rename = "rotate_270")]
    Rotate270,
}

/// The table's body against the footprint's fabrication outline.
#[derive(Debug, Serialize, JsonSchema)]
pub struct VerifyBody {
    pub table: [f64; 2],
    /// The size of the box round the fabrication outline; null without one, which `problems` says.
    pub footprint: Option<[f64; 2]>,
    /// The larger of the two differences, in millimetres; null without an outline.
    pub difference: Option<f64>,
}

/// What `ecad.footprint.verify` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FootprintVerifyResult {
    /// Every pad matches within the tolerance, none is missing or extra, the body (when given)
    /// matches, and `problems` has no error. Warnings don't count against it. Drills, sides and
    /// pad shapes aren't compared.
    pub ok: bool,
    pub footprint: ComponentId,
    /// The tolerance used, in millimetres.
    pub tolerance: f64,
    pub footprint_pads: usize,
    pub table_pads: usize,
    /// What was added to the table's centres, so the mean of the centres of the pads both have
    /// lands on the footprint's; null when they have none in common.
    pub offset: Option<[f64; 2]>,
    /// Numbers in the table, not in the footprint.
    pub missing: Vec<String>,
    /// Numbers in the footprint, not in the table.
    pub extra: Vec<String>,
    /// Compared as the table gives them.
    pub mismatches: Vec<VerifyMismatch>,
    /// The largest difference of any compared value, in or out of tolerance, in millimetres; null
    /// when nothing could be compared.
    pub worst: Option<f64>,
    /// When two or more pads both have don't match as given but do after this (the first that
    /// fits, in the order mirror_x, mirror_y, rotate_90, rotate_180, rotate_270); null otherwise.
    pub orientation: Option<VerifyOrientation>,
    /// The others that fit as well, in the same order: a row of pads reversed is a mirror or a
    /// half turn, and the pads can't say which.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orientation_also: Vec<VerifyOrientation>,
    /// With `body`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<VerifyBody>,
    /// What's wrong with the footprint itself, errors first: the codes `ecad.footprint.inspect`
    /// gives, `no_pin_1_mark` (a warning: more than two copper pads and no silkscreen drawing, so
    /// nothing can mark pin 1; a drawing there isn't proof that it marks pin 1), `not_compared` (a
    /// warning: the pads with a drill or on the bottom, which the table doesn't say) and
    /// `body_not_checked` (an error: `body` given and no fabrication outline to compare it with).
    pub problems: Vec<ProblemOut>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// How many of each the design's electronics hold.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Counts {
    pub symbols: usize,
    pub footprints: usize,
    pub packages: usize,
    pub components: usize,
}

/// One library symbol in a list.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SymbolSummary {
    pub id: u64,
    pub name: String,
    pub pins: usize,
    pub used_by: Vec<ComponentId>,
}

/// One library footprint in a list.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FootprintSummary {
    pub id: u64,
    pub name: String,
    pub pads: usize,
    pub used_by: Vec<VariantUse>,
}

/// One library 3D package in a list.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PackageSummary {
    pub id: u64,
    pub name: String,
    pub model: PackageModel,
    pub used_by: Vec<VariantUse>,
}

/// What `ecad.library.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LibraryInspectResult {
    pub counts: Counts,
    pub symbols: Vec<SymbolSummary>,
    pub footprints: Vec<FootprintSummary>,
    pub packages: Vec<PackageSummary>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
