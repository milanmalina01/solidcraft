//! Contracts and result types for the commands in `cmd/ecad_export.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use super::ecad_frame::FrameRef;

/// `ecad.design.info`: the title block's fields; each given one is set (`""` clears it).
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct InfoParams {
    pub title: Option<String>,
    pub revision: Option<String>,
    /// As it should print (`2026-10-29`); never filled in from the clock.
    pub date: Option<String>,
    pub author: Option<String>,
    pub company: Option<String>,
    /// A short note (at most 256 characters).
    pub notes: Option<String>,
    /// The whole revision history, oldest first (the cover prints it newest first), at most 50;
    /// it replaces the saved one (`[]` clears it).
    pub revisions: Option<Vec<RevisionIn>>,
}

/// One revision as given.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct RevisionIn {
    /// Its name (`A`, `1.1`); not empty.
    pub rev: String,
    /// As it should print; never filled in from the clock.
    pub date: Option<String>,
    /// Who made it.
    pub by: Option<String>,
    /// What changed (at most 256 characters).
    pub note: Option<String>,
}

/// One revision.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct RevisionOut {
    pub rev: String,
    pub date: String,
    pub by: String,
    pub note: String,
}

/// `ecad.design.info.inspect`: nothing to give.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct InfoInspectParams {}

/// The title block's fields.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct InfoOut {
    pub title: String,
    pub revision: String,
    pub date: String,
    pub author: String,
    pub company: String,
    pub notes: String,
    /// The revision history, oldest first.
    pub revisions: Vec<RevisionOut>,
}

/// What the design info commands return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct InfoResult {
    /// As saved with the design.
    pub info: InfoOut,
    /// As the title block prints it: an empty field falls back to the design's properties
    /// (project, revision, author, organization), the title to the design's name.
    pub printed: InfoOut,
    /// The fields the call changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// A kind of page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageKind {
    /// The title, the contents, the revisions and a picture of the board's top side.
    /// `overview` (the page it replaced) is taken as this.
    #[serde(alias = "overview")]
    Cover,
    /// A page per frame (and one for parts in no frame).
    Sections,
    BlockDiagram,
    Parts,
    BoardTop,
    BoardBottom,
    /// A page per inner copper layer.
    InnerLayers,
    AssemblyTop,
    AssemblyBottom,
    Stackup,
    /// Every named net and the pages it is on, at the end of the book: printed when a list of
    /// cross-references is cut short (`+6 more`) or `power_xrefs` is on (and `pages` does not
    /// leave it out).
    NetIndex,
}

/// A page-kind list's schema with `overview` among the kinds: old scripts give it, and a call
/// takes it as `cover` (the schema's derive leaves serde's aliases out). Results never hold it.
fn with_overview(schema: &mut schemars::Schema) {
    if let Some(items) = schema.get_mut("items") {
        let kind = items.take();
        *items = serde_json::json!({
            "anyOf": [kind, {"const": "overview", "type": "string", "deprecated": true, "description": "The old name of `cover`; taken as it."}]
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Paper {
    A4,
    A3,
    Letter,
    Tabloid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Landscape,
    Portrait,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    /// White page, dark lines, a little colour (the default for PDF and CSV).
    Paper,
    /// Black and greys.
    Monochrome,
    /// The app's dark colours, for slides and READMEs.
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PagesFormat {
    /// One PDF with every page.
    Pdf,
    /// One SVG per page.
    Svg,
    /// One PNG per page.
    Png,
    /// The parts list as CSV.
    Csv,
}

/// `ecad.export.pages`: write the print pages.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ExportParams {
    /// The kinds of page; left out: all that apply.
    #[schemars(transform = with_overview)]
    pub pages: Option<Vec<PageKind>>,
    /// The frames that get a page, by id or title; left out: all.
    pub frames: Option<Vec<FrameRef>>,
    /// Left out: A4 when every section fits it at full-size text, else A3.
    pub paper: Option<Paper>,
    /// Every page this way. Left out: landscape, but a section page turns to portrait when that
    /// prints it at least 1.25 times larger (a tall frame).
    pub orientation: Option<Orientation>,
    /// Left out: paper.
    pub style: Option<Style>,
    /// Cross-reference the supply nets too (left out: no; they would be everywhere). Ground is
    /// never cross-referenced on a section page: the net index lists its pages.
    pub power_xrefs: Option<bool>,
    pub format: PagesFormat,
    /// PNG only: dots an inch, 36 to 1200 (left out: 200).
    pub dpi: Option<f64>,
    /// pdf: a .pdf file; csv: a .csv file; svg and png: a folder (an existing one, or a path
    /// ending in `/`), the files named after the design, or a name stem (`out/lora` gives
    /// `out/lora-01-cover.svg`, …).
    pub path: String,
}

/// `ecad.export.pages.inspect`: the pages an export would make, without writing anything.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct InspectParams {
    #[schemars(transform = with_overview)]
    pub pages: Option<Vec<PageKind>>,
    /// By id or title.
    pub frames: Option<Vec<FrameRef>>,
    pub paper: Option<Paper>,
    pub orientation: Option<Orientation>,
    pub style: Option<Style>,
    pub power_xrefs: Option<bool>,
    /// Left out: pdf. For csv only the parts list's pages are listed (the CSV holds their rows).
    pub format: Option<PagesFormat>,
    pub dpi: Option<f64>,
}

/// One page.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PageOut {
    /// Its number in the whole book, from 1 (a part of the book keeps the whole book's numbers).
    pub page: usize,
    pub kind: PageKind,
    pub title: String,
    /// The frame a section page shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<u64>,
    pub paper: Paper,
    pub orientation: Orientation,
    /// Width and height, in millimetres.
    pub size: [f64; 2],
    /// The drawing's scale as the title block states it (`1.5:1`, `1:2`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<String>,
    /// PNG only: its size in pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pixels: Option<[u32; 2]>,
}

/// A file written.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FileOut {
    pub path: String,
    /// The pages in it, by their numbers in the whole book (from 1).
    pub pages: Vec<usize>,
    pub bytes: u64,
    /// csv: the parts list's rows (its header line apart).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<usize>,
}

/// What `ecad.export.pages` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ExportResult {
    pub files: Vec<FileOut>,
    pub warnings: Vec<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.export.pages.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct InspectResult {
    pub pages: Vec<PageOut>,
    pub count: usize,
    pub paper: Paper,
    pub style: Style,
    pub warnings: Vec<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
