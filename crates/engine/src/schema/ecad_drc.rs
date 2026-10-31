//! Contracts and result types for the commands in `cmd/ecad_drc.rs`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ecad_board::CopperLayerParam;

/// How serious a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// What leaving a finding costs: critical breaks the board or its build, practical makes it
/// worse or harder to build, cosmetic only looks untidy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Critical,
    Practical,
    Cosmetic,
}

/// Who made an approval or a record: the user (in the app's windows) or an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    User,
    Agent,
}

/// A level the board gives a code: `ignore` leaves its findings out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Error,
    Warning,
    Info,
    Ignore,
}

/// A circle on the board.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CircleParam {
    pub at: [f64; 2],
    /// In millimetres.
    pub radius: f64,
}

/// A part of the board: a box (`rect`: two opposite corners) or a `circle`; give one.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RegionParam {
    pub rect: Option<[[f64; 2]; 2]>,
    pub circle: Option<CircleParam>,
}

/// `ecad.drc.inspect`: the design rule check's findings. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct InspectParams {
    /// Only these codes (as `codes` in the reply lists them); left out, every code.
    pub codes: Option<Vec<String>>,
    /// Only findings on this copper layer (and those on no one layer).
    pub layer: Option<CopperLayerParam>,
    /// Only findings of this severity.
    pub severity: Option<Severity>,
    /// Only findings of this class.
    pub class: Option<Class>,
    /// Only findings in this part of the board.
    pub region: Option<RegionParam>,
    /// Only the counts (and the profile), no findings; left out, false.
    pub summary: Option<bool>,
    /// The most findings to list (50 when left out, at most 10000), after `offset` of them.
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    /// List the approved findings too (in `approved`); left out, true.
    pub include_approved: Option<bool>,
}

/// What kind of board object a finding names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DrcItemKind {
    Pad,
    Footprint,
    Track,
    TrackEnd,
    Via,
    Hole,
    Pour,
    Keepout,
    Graphic,
}

/// A board object a finding is about.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DrcItemOut {
    /// How messages name it: `R1.2`, `R1`, `track 5`…
    #[serde(rename = "ref")]
    pub reference: String,
    pub kind: DrcItemKind,
    /// The board object to pick: the pad's footprint, the track of a track end.
    pub id: u64,
    /// A pad's number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pad: Option<String>,
    /// A track end: 0 its first point, 1 its last.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<u8>,
}

/// What was measured.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MeasureOut {
    pub value: f64,
    /// `mm`, `ratio`, `nets`, `connections`, `layers`, `spokes`, `profiles`, `mm²`, `findings`,
    /// or `protection`: a categorical measure, named by `text` (a via's protection: `open`,
    /// `tented`, `plugged`, `filled`, `filled_capped`) and ranked by `value` in that order from 0.
    pub units: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Which way a limit goes.
#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Bound {
    /// The measure must be at least the limit.
    Min,
    /// The measure must be at most the limit.
    Max,
}

/// What a finding measured against.
#[derive(Debug, Serialize, JsonSchema)]
pub struct LimitOut {
    pub value: f64,
    pub units: String,
    /// A categorical limit's name (`filled_capped`), as the measure's `text`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The rule or profile figure it comes from: `rules.clearance`, `profile.aspect_ratio`,
    /// `class.POWER.track_width`…
    pub param: String,
    pub bound: Bound,
}

/// A command that would deal with a finding.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct DrcFixOut {
    pub command: String,
    /// Left out when the call needs choices the check can't make (`says` tells them).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    pub says: String,
}

/// One finding.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DrcFindingOut {
    /// Stable, such as `clearance` (`codes` lists them).
    pub code: String,
    pub class: Class,
    /// After the board's levels.
    pub severity: Severity,
    /// The code and the sorted ids it is about (`clearance|p12.3|t5`); what `ecad.drc.approve`
    /// takes.
    pub key: String,
    pub approved: bool,
    /// An agent approved it and it is critical: it counts until the user confirms it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pending_user: bool,
    /// Who approved it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<Actor>,
    /// Why it was approved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// In English.
    pub message: String,
    /// The UI's message id and its arguments.
    pub message_key: String,
    pub args: BTreeMap<String, String>,
    /// Where on the board, in millimetres.
    pub at: [f64; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    pub items: Vec<DrcItemOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nets: Vec<String>,
    pub measured: MeasureOut,
    pub limit: LimitOut,
    /// How far inside the limit the measure is, in its units: negative past it.
    pub margin: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fix: Vec<DrcFixOut>,
    /// The guide section that explains it: `routing-dfm#vias`.
    pub section: String,
    /// Why it is a warning only: its figures come from public summaries of a standard.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
}

/// Findings by class, not approved.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct ClassCounts {
    pub critical: usize,
    pub practical: usize,
    pub cosmetic: usize,
}

/// How many findings there are.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DrcCounts {
    /// Errors, warnings and infos not approved (an agent's approval of a critical finding counts
    /// only once the user confirms it).
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub approved: usize,
    /// Critical findings an agent approved, waiting for the user.
    pub pending_user: usize,
    /// Approvals no finding has now.
    pub stale: usize,
    /// Copper gaps that pass by less than 10 % of their clearance.
    pub at_limit: usize,
    /// Findings not approved, by code and by class.
    pub by_code: BTreeMap<String, usize>,
    pub by_class: ClassCounts,
}

/// The fab profile a check measured against.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProfileRef {
    pub id: String,
    pub name: String,
    pub version: u32,
    /// Of its figures, so a report says which numbers it used.
    pub hash: String,
    /// How it measures rings: `hole` (the finished hole) or `tool` (the drill tool).
    pub ring_convention: String,
}

/// A code the check gives.
#[derive(Debug, Serialize, JsonSchema)]
pub struct DrcCodeOut {
    pub code: String,
    pub class: Class,
    /// Its own severity.
    pub severity: Severity,
    /// The level the board gives it, when it does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    pub section: String,
    pub param: String,
    pub about: String,
}

/// What `ecad.drc.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct InspectResult {
    /// Over the whole board (not only the filter's).
    pub counts: DrcCounts,
    /// The fab profile it checked against; left out when none is chosen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileRef>,
    /// Not approved, after the filter, `offset` and `limit`: errors first, then warnings, then
    /// infos, each grouped by code.
    pub findings: Vec<DrcFindingOut>,
    /// The approved findings after the filter (and `limit`), with their notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approved: Vec<DrcFindingOut>,
    /// How many findings the filter matched past `limit`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale: Vec<String>,
    /// The codes that weren't checked, each with what it needs (`board.fab_profile`) or what
    /// left it out (`board.drc_levels`): no finding of one of these is no pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_applicable: Vec<NotApplicableOut>,
    /// The board was past a budget or there were more than 10000 findings, so not everything is
    /// listed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// A code the check didn't run.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NotApplicableOut {
    pub code: String,
    pub needs: String,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// `ecad.drc.approve`: accept findings, so they no longer count.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ApproveParams {
    /// Findings' keys, as `ecad.drc.inspect` gives them.
    pub keys: Vec<String>,
    /// Why they are fine (at most 256 characters); an agent must give one.
    pub note: Option<String>,
}

/// `ecad.drc.unapprove`: drop approvals.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct UnapproveParams {
    pub keys: Option<Vec<String>>,
    /// Every approval.
    pub all: Option<bool>,
    /// The approvals no finding has now.
    pub stale: Option<bool>,
}

/// What `ecad.drc.approve` and `ecad.drc.unapprove` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ApprovalResult {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    /// Agents' approvals the user confirmed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confirmed: Vec<String>,
    /// The user's approvals an agent left as they were (an agent's call neither changes nor
    /// takes them back).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<String>,
    /// Critical findings this call approved as an agent: they count until the user confirms.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_user: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    pub counts: DrcCounts,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// `ecad.drc.rules`: give codes another level; `null` puts a code back to its own.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RulesParams {
    /// Code → error | warning | info | ignore | null; `{}` changes nothing and lists the codes.
    pub rules: BTreeMap<String, Option<Level>>,
}

/// What `ecad.drc.rules` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RulesResult {
    /// The levels the board gives now.
    pub rules: BTreeMap<String, Level>,
    /// Every code, in the order findings are listed.
    pub codes: Vec<DrcCodeOut>,
    pub counts: DrcCounts,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
