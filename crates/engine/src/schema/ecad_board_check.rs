//! Contracts and result types for the commands in `cmd/ecad_board_check.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ecad_drc::{Actor, ClassCounts, DrcFixOut, Level, ProfileRef};

/// How far the board has come: each stage checks what it should be done by then.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// The schematic is done: every part has a footprint, the intent and the fab profile are set.
    Plan,
    /// The parts are placed on the board.
    Place,
    /// The board is routed.
    Route,
    /// Ready to order: every warning of the critical gates counts as an error.
    Release,
}

/// `ecad.board.check`: whether a stage is done and what to do next. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CheckParams {
    /// `route` when left out.
    pub stage: Option<Stage>,
    /// The most findings it lists, over all the gates (10 when left out, at most 200).
    pub limit: Option<usize>,
}

/// A gate of the scorecard, in the fixed order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    /// Everything there: footprints for the parts, the parts on the board, the connections routed.
    Completeness,
    /// No shorts.
    Connectivity,
    /// The fab can build it: its profile, minimum sizes and gaps, holes.
    Fab,
    /// The other critical findings: footprints off the board or on each other, keepouts.
    Critical,
    /// The blocking roles of the design's intent are settled.
    Intent,
    Practical,
    Cosmetic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Pass,
    /// Only warnings left.
    Warn,
    Fail,
    /// What it found is all approved.
    Waived,
    /// Not part of this stage, or nothing checks it yet.
    NotRun,
}

/// How many findings a gate has.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct GateCounts {
    pub open: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub approved: usize,
    /// Approved by an agent, waiting for the user.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pending_user: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A finding (or a missing role) that holds a gate back.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct BlockingOut {
    pub code: String,
    /// The finding's key (`ecad.drc.approve` takes it), or the intent record's (`net|+3V3`).
    pub key: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    /// How far past its limit, in its units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
    /// The first command that would deal with it (`ecad.drc.inspect` gives them all).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<DrcFixOut>,
    /// The guide section that explains it (`routing-dfm#vias`, see `NextOut::read`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    /// An intent role's: what it needs (`confirm`, `current_a`, `bands`…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GateOut {
    pub gate: Gate,
    pub status: GateStatus,
    pub counts: GateCounts,
    /// Its open findings by code (the intent gate's missing roles by role), so each can be
    /// listed in full (`ecad.drc.inspect {codes}`, `ecad.intent.inspect {roles}`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub by_code: std::collections::BTreeMap<String, usize>,
    /// What holds it back, at most the call's `limit` over all gates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocking: Vec<BlockingOut>,
    /// Why it didn't run, or what it can't check yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The board in numbers.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Scorecard {
    /// Connections routed of those the nets need: `206/216`.
    pub routed: String,
    /// Footprints on the board of the parts that have one: `84/84`.
    pub on_board: String,
    pub drc_errors: usize,
    pub drc_warnings: usize,
    /// Blocking intent roles settled of those there are: `40/52`.
    pub intent_coverage: String,
    /// Findings not approved, by class: a class is not a gate (the critical class spans the
    /// completeness, connectivity, fab and critical gates; the gates have their own counts).
    pub by_class: ClassCounts,
    pub vias: usize,
    pub length_mm: f64,
    /// Copper gaps that pass by less than 10 % of their clearance.
    pub at_limit: usize,
    /// Critical design-quality errors and warnings not approved (counted in `by_class` too;
    /// infos and review items are not).
    pub quality_critical: usize,
    /// Design-quality codes that couldn't run everywhere for want of intent (`not_verified`
    /// says what each needs).
    pub quality_not_checked: usize,
}

/// Something that got worse since the last check in this session.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RegressionOut {
    pub what: String,
    pub was: f64,
    pub now: f64,
}

/// What to do next, most pressing first.
#[derive(Debug, Serialize, JsonSchema)]
pub struct NextOut {
    #[serde(rename = "do")]
    pub what: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    /// The guide section to read: `routing-dfm#vias` is the Vias section of
    /// `solidcraft-pcb/reference/routing-dfm.md` (MCP `skill_reference` serves the guide).
    pub read: String,
    /// The gate the step is for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<Gate>,
}

/// An approved finding.
#[derive(Debug, Serialize, JsonSchema)]
pub struct WaiverOut {
    pub key: String,
    /// Left out for an approval made before who made it was kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<Actor>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// An agent's approval of a critical finding, waiting for the user.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending_user: bool,
}

/// What `ecad.board.check` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CheckResult {
    pub stage: Stage,
    /// Every stage's gate is done (none fails; at release, none warns either).
    pub done: bool,
    /// In the fixed order: completeness, connectivity, fab, critical, intent, practical, cosmetic.
    pub gates: Vec<GateOut>,
    pub scorecard: Scorecard,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regressions: Vec<RegressionOut>,
    /// The rules, the fab profile or the check's levels changed since the caller's last check:
    /// no regressions are named, and this check is the new baseline.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub rules_changed: bool,
    pub next: Vec<NextOut>,
    /// At most 10; `waivers_more` counts the rest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waivers: Vec<WaiverOut>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub waivers_more: usize,
    /// The design rule check's codes the board gives another level than their own (`ignore`
    /// leaves a code out): the gates they count in say so in their `note`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub levels: std::collections::BTreeMap<String, Level>,
    /// What this check doesn't look at.
    pub not_verified: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileRef>,
    /// The engine's version and the checks' (so two reports can be compared).
    pub engine: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// The design-quality codes whose work budget ran out (with `truncated`): the critical gate
    /// is a warning at least, and never done at release, while any is cut short.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cut: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// `ecad.board.profile`: pick the board's fab profile.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ProfileParams {
    /// A profile's id (`ecad.board.profile.inspect` lists them); `null` takes the board's away.
    #[serde(default, deserialize_with = "super::ecad_schematic::nullable")]
    #[schemars(with = "Option<String>")]
    pub fab: Option<Option<String>>,
    /// Set the rules' minimums from it (and raise what new copper gets where it falls below
    /// them); left out, true. A rule the user set to another value than the old profile's (or
    /// the default's, with none) is kept and listed in `kept`.
    pub apply: Option<bool>,
    /// Set every rule the profile sets to its figure, the user's too (Reset Rules to Profile).
    pub reset: Option<bool>,
}

/// A rule picking the profile left as the user set it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RuleKeptOut {
    pub field: String,
    pub value: f64,
}

/// A rule picking the profile changed.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RuleSetOut {
    pub field: String,
    pub from: f64,
    pub to: f64,
    pub source: String,
}

/// What `ecad.board.profile` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProfileResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules_set: Vec<RuleSetOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<RuleKeptOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// `ecad.board.profile.inspect`: the fab profiles and their figures. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ProfileInspectParams {
    /// Only this profile.
    pub fab: Option<String>,
}

/// One figure of a profile.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FigureOut {
    pub key: String,
    pub value: f64,
    pub units: String,
    pub source: String,
    /// From a public summary of an IPC standard, not checked against it: checks resting on it
    /// alone warn, never err.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub verify: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProfileOut {
    pub id: String,
    pub name: String,
    pub version: u32,
    /// When its sources were read.
    pub read: String,
    pub about: String,
    pub ring_convention: String,
    pub hash: String,
    pub values: Vec<FigureOut>,
}

/// The profile the board picked.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ChoiceOut {
    pub id: String,
    /// The version it was picked at.
    pub version: u32,
}

/// What `ecad.board.profile.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProfileInspectResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<ChoiceOut>,
    pub profiles: Vec<ProfileOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
