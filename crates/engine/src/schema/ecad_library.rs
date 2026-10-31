//! Contracts and result types for the commands in `cmd/ecad_library.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_component::{
    ComponentId, ComponentRef, CourtyardOut, GapOut, GraphicSpec, LayerGraphicOut, PackageModel, PadMeasure, PadOut, PinMeasure, PinOut, ProblemOut,
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
