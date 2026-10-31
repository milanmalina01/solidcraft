//! Contracts and result types for the commands in `cmd/ecad_erc.rs`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ecad_component::Electrical;

/// How serious a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// A level the schematic's rules give a code: `ignore` leaves its findings out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuleLevel {
    Error,
    Warning,
    Ignore,
}

/// A code's own level: `matrix` for `pin_conflict`, whose level comes from the clashing types.
#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DefaultLevel {
    Error,
    Warning,
    Matrix,
}

/// `ecad.erc.inspect`: the electrical rules check's findings. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct InspectParams {
    /// Only findings of this severity; left out, errors and warnings.
    pub severity: Option<Severity>,
    /// List approved findings too (marked `approved`, with their note); left out, false.
    pub include_approved: Option<bool>,
}

/// What a finding is about: a pin (`ref` such as `U1.4`, with `part`, `pin` and `type`), a part
/// (`ref` its designator, with `part`), or a wire, label or marker by id.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FindingItemOut {
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub electrical: Option<Electrical>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wire: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker: Option<u64>,
}

/// A command that would deal with a finding: run `id` with `params`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FixOut {
    pub id: String,
    pub params: Value,
}

/// One finding.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FindingOut {
    /// Stable, such as `power_not_driven` (`ecad.erc.rules` lists them).
    pub code: String,
    /// After the schematic's rules.
    pub severity: Severity,
    /// The code and the ids it is about (`power_not_driven|p14.4`: part 14's pin 4); what
    /// `ecad.erc.approve` takes.
    pub key: String,
    pub approved: bool,
    /// A critical finding an agent approved (or someone before who approved was kept): it still
    /// counts until the user confirms it in the app.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending_user: bool,
    /// Why it was approved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The net it is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
    /// Where on the sheet, in millimetres.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    /// At most 16; `more` counts the rest.
    pub items: Vec<FindingItemOut>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
    pub message: String,
    /// A command that would deal with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<FixOut>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// How many findings there are.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ErcCounts {
    /// Errors and warnings not approved.
    pub errors: usize,
    pub warnings: usize,
    /// Approved findings.
    pub approved: usize,
    /// Critical findings approved by an agent, waiting for the user's confirmation (they count
    /// in `errors`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pending_user: usize,
    /// Approvals no finding has now.
    pub stale: usize,
}

/// What `ecad.erc.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct InspectResult {
    /// Errors first, then warnings, each grouped by code.
    pub findings: Vec<FindingOut>,
    pub counts: ErcCounts,
    /// The keys of approvals no finding has now (`ecad.erc.unapprove {stale: true}` drops them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale: Vec<String>,
    /// The levels the schematic's rules give codes in place of their own.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, RuleLevel>,
    /// The sheet was past a cap or the check's budget, or there were more than 10000 findings, so
    /// not everything is listed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// `ecad.erc.approve`: accept findings, so they no longer count as errors or warnings.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ApproveParams {
    /// Findings' keys, as `ecad.erc.inspect` gives them.
    pub keys: Vec<String>,
    /// Why they are fine (at most 256 characters); an approved key takes the new note.
    pub note: Option<String>,
}

/// `ecad.erc.unapprove`: take approvals back; give one of `keys`, `all` or `stale`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct UnapproveParams {
    /// Approved keys.
    pub keys: Option<Vec<String>>,
    /// Every approval.
    pub all: Option<bool>,
    /// The approvals no finding has now.
    pub stale: Option<bool>,
}

/// What `ecad.erc.approve` and `ecad.erc.unapprove` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ApprovalResult {
    /// Keys approved by this call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    /// Approved keys whose note changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    /// Agents' approvals the user confirmed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confirmed: Vec<String>,
    /// The user's approvals an agent left as they were.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<String>,
    /// Critical findings this call approved as an agent: they count until the user confirms.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_user: Vec<String>,
    /// Keys no longer approved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    pub counts: ErcCounts,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// `ecad.erc.rules`: give codes another level; `null` puts a code back to its own.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct RulesParams {
    /// Code → error | warning | ignore | null; `{}` changes nothing and lists the codes.
    pub rules: BTreeMap<String, Option<RuleLevel>>,
}

/// A code the check gives.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CodeOut {
    pub code: String,
    /// Its own level.
    pub default: DefaultLevel,
    /// The level the schematic's rules give it, when they do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<RuleLevel>,
    /// What it finds.
    pub about: String,
}

/// What `ecad.erc.rules` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RulesResult {
    /// The levels the schematic's rules give now.
    pub rules: BTreeMap<String, RuleLevel>,
    /// Every code, in the order findings are listed.
    pub codes: Vec<CodeOut>,
    pub counts: ErcCounts,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
