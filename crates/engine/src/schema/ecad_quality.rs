//! Contracts and result types for the commands in `cmd/ecad_quality.rs`, and the
//! `findings_delta` that mutating `ecad.board.*` commands add to their replies.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_board::CopperLayerParam;
use super::ecad_drc::{Actor, Class, DrcFixOut, DrcItemKind, Level, LimitOut, MeasureOut, ProfileRef, RegionParam, Severity};

/// A finding's status, or the codes that couldn't run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StatusParam {
    Fail,
    Review,
    NotApplicable,
}

/// `ecad.quality.inspect`: the design-quality findings. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct InspectParams {
    /// Only these families (the codes' prefixes: `pi`, `si`, `rf`, `esd`, `silk`, `dfm`).
    pub families: Option<Vec<String>>,
    /// Only these codes (`ecad.quality.rules` lists them).
    pub codes: Option<Vec<String>>,
    /// Only findings about these nets.
    pub nets: Option<Vec<String>>,
    /// Only findings about these parts, by designator.
    pub parts: Option<Vec<String>>,
    pub region: Option<RegionParam>,
    /// Only findings on this copper layer (and those on no one layer).
    pub layer: Option<CopperLayerParam>,
    pub class: Option<Class>,
    pub severity: Option<Severity>,
    /// `not_applicable` lists only the codes that couldn't run.
    pub status: Option<StatusParam>,
    /// List the approved findings too (in `approved`); left out, false.
    pub include_approved: Option<bool>,
    /// Counts, the worst few and what couldn't run, no findings list; left out, false.
    pub summary: Option<bool>,
    /// The most findings to list (25 when left out, at most 10000), after `offset` of them.
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// A board object a finding is about.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ObjectOut {
    /// How messages name it: `R1.2`, `R1`, `track 5`…
    #[serde(rename = "ref")]
    pub reference: String,
    pub kind: DrcItemKind,
    /// The board object to pick: the pad's footprint, the track of a track end.
    pub id: u64,
    /// As finding keys name it: `t5`, `v7`, `p12.3`.
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pad: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<u8>,
}

/// One finding.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FindingOut {
    pub code: String,
    /// The code's prefix: `pi`.
    pub family: String,
    pub class: Class,
    /// After the board's levels.
    pub severity: Severity,
    /// `fail` (fix it) or `review` (a person's look settles it).
    pub status: StatusParam,
    /// The code and the sorted ids it is about; what `ecad.quality.approve` takes.
    pub key: String,
    pub approved: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pending_user: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<Actor>,
    /// Why it was approved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Why it matters, in one sentence.
    pub principle: String,
    /// The guide section that owns it: `power-ground#return-paths`.
    pub section: String,
    pub message: String,
    pub message_key: String,
    pub args: BTreeMap<String, String>,
    pub at: [f64; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<ObjectOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nets: Vec<String>,
    /// What was measured against which limit (left out when it counts no measure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured: Option<MeasureOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<LimitOut>,
    /// How far inside the limit, in its units: negative past it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin: Option<f64>,
    /// What it was judged in: the net's tier and its source, the reference plane, the frequency.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regime: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fix: Vec<DrcFixOut>,
    /// Why it is a warning only: its figures come from public summaries of a standard.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
}

/// Findings not approved of one class, by status.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct StatusCounts {
    /// Findings to fix and findings for a person to look at (any severity), not approved.
    pub fail: usize,
    pub review: usize,
    /// The same findings by severity: errors and warnings are what the gates count.
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    /// Codes of the class that couldn't run everywhere.
    pub not_applicable: usize,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct ByClass {
    pub critical: StatusCounts,
    pub practical: StatusCounts,
    pub cosmetic: StatusCounts,
}

/// One of the worst findings.
#[derive(Debug, Serialize, JsonSchema)]
pub struct WorstOut {
    pub code: String,
    pub key: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
}

/// The counts first.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct SummaryOut {
    pub by_class: ByClass,
    /// Findings not approved, by code.
    pub by_code: BTreeMap<String, usize>,
    /// Critical errors first, then the rest by severity, at most 5.
    pub worst: Vec<WorstOut>,
}

/// A code that couldn't run, or not everywhere.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NotApplicableOut {
    pub code: String,
    /// The places it couldn't check.
    pub count: usize,
    /// What it needs, each once (at most 8): `intent: driver or edge_ns of LED_G`.
    pub needs: Vec<String>,
}

/// The whole board's counts.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CountsOut {
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub approved: usize,
    pub pending_user: usize,
    pub stale: usize,
}

/// What `ecad.quality.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct InspectResult {
    pub counts: CountsOut,
    pub summary: SummaryOut,
    /// Not approved, after the filter, `offset` and `limit`: errors first, then warnings and
    /// infos, each in catalogue order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<FindingOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approved: Vec<FindingOut>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
    /// Codes that couldn't run (or not everywhere) and what each needs: no finding of one of
    /// these is no pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_applicable: Vec<NotApplicableOut>,
    /// Families whose checks are not built yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_built: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileRef>,
    /// The engine's and the checks' versions.
    pub engine: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// The codes whose family's work budget ran out: their findings may not be all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cut: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// `ecad.quality.rules`: give codes levels and parameters; with nothing, list the catalogue.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RulesParams {
    /// Code → error | warning | info | ignore | null (null: back to the code's own).
    pub rules: Option<BTreeMap<String, Option<Level>>>,
    /// Code → {parameter: value | null} (null: back to the default).
    pub params: Option<BTreeMap<String, BTreeMap<String, Option<f64>>>>,
    /// Why (at most 256 characters); needed with `params`, and from an agent with `rules`.
    pub note: Option<String>,
}

/// A parameter of a code.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ParamOut {
    pub name: String,
    pub default: f64,
    /// The board's, when it sets one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    pub units: String,
    pub source: String,
    pub about: String,
    pub min: f64,
    pub max: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub verify: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<Actor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A code of the catalogue.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CodeOut {
    pub code: String,
    pub family: String,
    pub class: Class,
    pub severity: Severity,
    /// The class and severity some of its findings rise to, when more than `class` and
    /// `severity`: only the user lowers a code that may be critical below its worst severity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst_class: Option<Class>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst_severity: Option<Severity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    pub needs: String,
    pub section: String,
    pub about: String,
    pub principle: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ParamOut>,
}

/// What `ecad.quality.rules` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RulesResult {
    /// The levels the board gives now.
    pub rules: BTreeMap<String, Level>,
    /// The parameters the board sets now, by `code.param`.
    pub params: BTreeMap<String, f64>,
    pub codes: Vec<CodeOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_built: Vec<String>,
    /// Parameters the user set that an agent's call left as they are (`code.param`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<String>,
    pub counts: CountsOut,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What `ecad.quality.approve` and `ecad.quality.unapprove` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ApprovalResult {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confirmed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_user: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    pub counts: CountsOut,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// A finding a board edit brought.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct DeltaFindingOut {
    pub code: String,
    pub key: String,
    pub message: String,
}

/// What a mutating `ecad.board.*` command changed in the design rule and quality findings
/// (errors and warnings not approved), when it checks (`findings_delta`, true by default for
/// programs, off for the app's own edits).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct FindingsDeltaOut {
    /// At most 20; `introduced_more` counts the rest.
    pub introduced: Vec<DeltaFindingOut>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub introduced_more: usize,
    pub resolved: usize,
    /// A check was cut short by its budget: what it didn't reach isn't compared.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}
