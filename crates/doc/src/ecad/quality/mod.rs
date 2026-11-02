//! The design-quality checks of the board: what the design rule check (see [`super::drc`]) can't
//! tell from geometry and the fab profile alone, because it needs to know what the board is for
//! (see [`super::intent`]): return paths of fast nets, decoupling and regulator loops, current,
//! ESD paths, crystals, RF lines, pairs, silkscreen. Worked out from the board each time, never
//! saved; only the approvals, the levels the board gives codes and its parameters are
//! (`quality_approvals`, `quality_levels`, `quality_params` on the board).
//!
//! **Findings** take the design rule check's shape ([`DrcFinding`]: code, class, severity, a key
//! of the code and the sorted ids of the objects it is about, approval, message, where, layer,
//! objects, nets, what was measured against which limit with units and the margin, the commands
//! that deal with it, the guide section, and the `verify` note), with what the quality checks add
//! ([`QualityFinding`]): the family, a status (`fail`, or `review` for what needs a person's
//! look), the regime it was judged in (the net's tier and where it came from, the reference
//! plane, the frequency), and the principle in one sentence. A code that can't run for want of
//! intent says what it needs in `not_applicable` ("intent: driver or edge_ns of SPI_SCK"), never
//! a pass.
//!
//! **A family of checks** is one module (`pi_return.rs` …) that declares its codes (see
//! [`catalog::Code`]: class, usual severity, needs, owner section, about, principle, parameters
//! with defaults and sources) and one `run(&mut Cx)`. [`catalog::FAMILIES`] lists the modules; a
//! module's codes are its own, so families are built without touching each other. `run` reads:
//! - the design (`cx.ecad`), the board (`cx.board`), its connections and pours' fills
//!   (`cx.nets`), the schematic's nets and pins (`cx.conn`);
//! - the intent through [`roles::Roles`] (`cx.roles`): each net's record, role and edge-rate tier
//!   (assumed standard drive for an IC-driven net with nothing declared), the ground nets, part
//!   records by role, pairs, loops, the frequency of concern;
//! - shared geometry through [`geom::Geo`] (`cx.geo`): each layer's planes (which net covers a
//!   point, the void it is in), what pierces a layer, the reference plane under a run of points,
//!   points along a segment, nearest points;
//! - its thresholds through [`Cx::param`] (the board's value, else the catalogue's default).
//!
//! It asks [`Cx::wants`] before working on a code (the caller's filter and the board's levels),
//! charges its work to [`Cx::spend`] (a budget per family: past it the rest is left out and the
//! report is `truncated`), and reports with [`Cx::add`] (a [`Draft`] made only when the finding
//! is kept) or [`Cx::not_applicable`]. Findings come out errors first, then warnings and infos,
//! each in the catalogue's order; the same board always gives the same findings.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Electronics;
use super::airwires::BoardNets;
use super::board::Board;
use super::checks::Severity;
use super::connect::Connectivity;
use super::drc::{Bound, CheckLevel, Class, DrcFinding, DrcItem, Fix, Limit, MAX_ITEMS};
use super::layers::CopperLayer;
use super::schematic::{Actor, Approval};

pub mod catalog;
mod esd;
pub mod geom;
pub mod metrics;
mod pair;
mod pi_current;
mod pi_decap;
mod pi_return;
mod pi_smps;
mod rf;
pub mod roles;
mod silk_dfm;
mod xtal;

pub use catalog::{Code, FAMILIES, Family, Param};
pub use geom::Geo;
pub use roles::{Roles, Tier};

/// The most findings a run returns; past it, `truncated` is set.
pub const MAX_FINDINGS: usize = 10_000;
/// The work one family may do (points looked up, pairs tested), each time it runs.
pub const MAX_WORK: usize = 1 << 24;
/// The most different needs one code's `not_applicable` lists (the rest are counted).
pub const MAX_NEEDS: usize = 8;
/// The most approvals, levels and parameters a board keeps for the quality checks.
pub const MAX_PARAMS: usize = 1024;

/// What a finding asks for: a fix, or a person's look.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Fail,
    Review,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Fail => "fail",
            Status::Review => "review",
        }
    }
}

/// One finding (see the module notes).
#[derive(Clone, Debug, PartialEq)]
pub struct QualityFinding {
    /// The shared part: `code`, `class`, `severity`, `key`, approval, message, `at`, `layer`,
    /// objects (`items`), `nets`, measure, `fix`, `section`, `note` (verify).
    pub finding: DrcFinding,
    /// The code's prefix: `pi`.
    pub family: &'static str,
    pub status: Status,
    /// What it was judged in: the net's tier and its source, the reference plane, the frequency.
    pub regime: Option<String>,
    pub principle: &'static str,
    /// It measured something: otherwise the finding's measure is the count stand-in (1 finding,
    /// limit 0), which replies leave out.
    pub measured: bool,
}

/// A code that didn't run, or didn't run everywhere: what it needs, and how often.
#[derive(Clone, Debug, PartialEq)]
pub struct NotApplicable {
    pub code: &'static str,
    /// The places it couldn't check (nets, parts, the board).
    pub count: usize,
    /// What it needs, each once, at most [`MAX_NEEDS`]: `intent: driver or edge_ns of LED_G`,
    /// `intent: the board's f_concern_mhz or radio bands`, `board.quality_levels` (left out by
    /// the board's levels).
    pub needs: Vec<String>,
}

/// What a run found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QualityReport {
    /// Errors, then warnings, then infos, each in catalogue order; approved ones included.
    pub findings: Vec<QualityFinding>,
    pub not_applicable: Vec<NotApplicable>,
    /// Not approved (an agent's approval of a critical finding counts only once the user
    /// confirms it), approved, and waiting for the user.
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub approved: usize,
    pub pending_user: usize,
    /// Approval keys no finding has now, of codes that ran (a code left out by its level keeps
    /// its approvals); empty when the run was filtered or `truncated`.
    pub stale: Vec<String>,
    /// The codes that ran.
    pub ran: Vec<&'static str>,
    pub truncated: bool,
    /// The codes whose family's budget ran out: what they found is not all there is.
    pub cut: Vec<&'static str>,
}

/// What one finding says, made only when it is kept (see [`Cx::add`]).
pub struct Draft {
    at: [f64; 2],
    layer: Option<CopperLayer>,
    items: Vec<DrcItem>,
    nets: Vec<String>,
    measured: Option<(f64, f64, &'static str, Bound)>,
    param: Option<String>,
    value: Option<f64>,
    required: Option<f64>,
    severity: Option<Severity>,
    class: Option<Class>,
    status: Status,
    regime: Option<String>,
    fix: Vec<Fix>,
    verify: Option<String>,
    message: String,
    /// Which of the code's messages the UI words it with, when it has more than one.
    variant: Option<&'static str>,
    args: Vec<(&'static str, String)>,
}

impl Draft {
    /// A finding at `at` about `items` (its first two are the message's `a` and `b`).
    pub fn new(at: [f64; 2], items: Vec<DrcItem>, message: impl Into<String>) -> Self {
        Draft {
            at,
            layer: None,
            items,
            nets: Vec::new(),
            measured: None,
            param: None,
            value: None,
            required: None,
            severity: None,
            class: None,
            status: Status::Fail,
            regime: None,
            fix: Vec::new(),
            verify: None,
            message: message.into(),
            variant: None,
            args: Vec::new(),
        }
    }

    pub fn layer(mut self, layer: CopperLayer) -> Self {
        self.layer = Some(layer);
        self.args.push(("layer", layer.name()));
        self
    }

    pub fn nets(mut self, nets: Vec<String>) -> Self {
        self.args.push(("nets", nets.join(", ")));
        self.nets = nets;
        self
    }

    /// What was measured, its limit, units, and which way the limit goes; in millimetres, the
    /// UI's `value` and `required` too.
    pub fn measure(mut self, value: f64, limit: f64, units: &'static str, bound: Bound) -> Self {
        self.measured = Some((value, limit, units, bound));
        self.args.push(("value", num(value)));
        self.args.push(("required", num(limit)));
        if units == "mm" {
            (self.value, self.required) = (Some(value), Some(limit));
        }
        self
    }

    /// The parameter the limit comes from, when it isn't the code's first: `reach`, read as
    /// `<code>.reach`; `""` for a limit no parameter sets (no crossing at all).
    pub fn param(mut self, name: &str) -> Self {
        self.param = Some(name.to_string());
        self
    }

    /// In place of the code's usual severity.
    pub fn severity(mut self, s: Severity) -> Self {
        self.severity = Some(s);
        self
    }

    /// In place of the code's usual class.
    pub fn class(mut self, c: Class) -> Self {
        self.class = Some(c);
        self
    }

    /// A person's look, not a fix, settles it.
    pub fn review(mut self) -> Self {
        self.status = Status::Review;
        self
    }

    pub fn regime(mut self, r: impl Into<String>) -> Self {
        self.regime = Some(r.into());
        self
    }

    /// A command that would deal with it: `params` `Null` when the call needs choices the check
    /// can't make (`says` tells what to do).
    pub fn fix(mut self, command: &'static str, params: Value, says: impl Into<String>) -> Self {
        self.fix.push(Fix { command, params, says: says.into() });
        self
    }

    /// Its figures come from public summaries of a standard: never an error (the board's level
    /// for the code still wins).
    pub fn verify(mut self, note: impl Into<String>) -> Self {
        self.verify = Some(note.into());
        self
    }

    /// The UI words it with the code's message `variant` (`electronics.quality_msg_<code>_<variant>`)
    /// rather than its first.
    pub fn variant(mut self, v: &'static str) -> Self {
        self.variant = Some(v);
        self
    }

    /// A named argument for the UI's message.
    pub fn arg(mut self, name: &'static str, v: impl Into<String>) -> Self {
        self.args.push((name, v.into()));
        self
    }
}

/// A number as messages give it: at most 4 decimals, no trailing zeros.
pub fn num(v: f64) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    let s = format!("{:.4}", (v * 1e4).round() / 1e4 + 0.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The findings and needs being collected.
struct Found<'a> {
    approved: BTreeMap<&'a str, &'a Approval>,
    levels: &'a BTreeMap<String, CheckLevel>,
    seen: BTreeSet<String>,
    /// The keys listed, so a key is listed once.
    keys: BTreeSet<String>,
    list: Vec<QualityFinding>,
    needs: BTreeMap<&'static str, (usize, Vec<String>)>,
    truncated: bool,
}

/// What a family's `run` works with (see the module notes).
pub struct Cx<'a> {
    pub ecad: &'a Electronics,
    pub board: &'a Board,
    pub nets: &'a BoardNets,
    pub conn: &'a Connectivity,
    pub roles: &'a Roles<'a>,
    pub geo: &'a Geo<'a>,
    only: Option<&'a BTreeSet<&'a str>>,
    work: usize,
    /// The family running, and whether its budget ran out.
    family: Option<&'static Family>,
    cut: Vec<&'static str>,
    found: Found<'a>,
}

impl<'a> Cx<'a> {
    /// Whether to check `code`: the caller asked for it (or for every code), and the board's
    /// levels don't leave it out.
    pub fn wants(&self, code: &str) -> bool {
        self.only.is_none_or(|o| o.contains(code)) && self.found.levels.get(code) != Some(&CheckLevel::Ignore)
    }

    /// The threshold `name` of `code`: the board's (`ecad.quality.rules`), else the catalogue's
    /// default (0 for a parameter the catalogue lacks: a family's own mistake).
    pub fn param(&self, code: &str, name: &str) -> f64 {
        param_value(self.board, code, name)
    }

    /// Charge `n` units of work to the family's budget; `false` (and the report truncated) once
    /// it is spent: stop.
    pub fn spend(&mut self, n: usize) -> bool {
        if self.work >= n {
            self.work -= n;
            true
        } else {
            self.work = 0;
            self.found.truncated = true;
            if let Some(f) = self.family {
                for c in f.codes.iter().map(|c| c.code).filter(|c| self.only.is_none_or(|o| o.contains(c))) {
                    if !self.cut.contains(&c) {
                        self.cut.push(c);
                    }
                }
            }
            false
        }
    }

    /// Whether the family's budget is spent.
    pub fn spent(&self) -> bool {
        self.work == 0
    }

    /// A finding of `code` about the objects whose keys are `keys` (see [`DrcItem::key`], plus
    /// anything that tells two findings of the code apart); `make` says the rest when it is kept.
    pub fn add(&mut self, code: &'static str, keys: &[String], make: impl FnOnce() -> Draft) {
        let Some(entry) = catalog::find(code) else { return };
        let mut keys = keys.to_vec();
        keys.sort();
        keys.dedup();
        let mut key = code.to_string();
        for k in keys.iter().take(MAX_ITEMS) {
            key.push('|');
            key.push_str(k);
        }
        if !self.found.approved.is_empty() {
            self.found.seen.insert(key.clone());
        }
        let level = self.found.levels.get(code).copied();
        if level == Some(CheckLevel::Ignore) || self.found.keys.contains(&key) {
            return;
        }
        if self.found.list.len() >= MAX_FINDINGS {
            self.found.truncated = true;
            return;
        }
        let d = make();
        self.found.keys.insert(key.clone());
        let class = d.class.unwrap_or(entry.class);
        let mut own = d.severity.unwrap_or(entry.severity);
        if d.verify.is_some() && own == Severity::Error {
            own = Severity::Warning;
        }
        let severity = match level {
            Some(CheckLevel::Error) => Severity::Error,
            Some(CheckLevel::Warning) => Severity::Warning,
            Some(CheckLevel::Info) => Severity::Info,
            _ => own,
        };
        let mut items = d.items;
        items.truncate(MAX_ITEMS);
        let approval = self.found.approved.get(key.as_str()).copied();
        let pending_user = approval.is_some_and(|a| !a.by_user() && class == Class::Critical);
        let approved = approval.is_some() && !pending_user;
        let mut args = d.args;
        for (name, item) in ["a", "b"].into_iter().zip(&items) {
            args.push((name, item.reference()));
        }
        let measured = d.measured.is_some();
        let (value, limit, units, bound) = d.measured.unwrap_or((1.0, 0.0, "findings", Bound::Max));
        let margin = match bound {
            Bound::Min => value - limit,
            Bound::Max => limit - value,
        };
        let param = d
            .param
            .or_else(|| entry.params.first().map(|p| p.name.to_string()))
            .filter(|p| !p.is_empty())
            .map_or(String::new(), |p| format!("{code}.{p}"));
        self.found.list.push(QualityFinding {
            finding: DrcFinding {
                code,
                class,
                severity,
                key,
                approved,
                pending_user,
                by: approval.and_then(|a| a.by),
                message: d.message,
                message_key: match d.variant {
                    Some(v) => format!("electronics.quality_msg_{}_{v}", code.replace('.', "_")),
                    None => format!("electronics.quality_msg_{}", code.replace('.', "_")),
                },
                args,
                at: d.at,
                layer: d.layer,
                items,
                nets: d.nets,
                measured: value,
                measured_text: None,
                limit: Limit { value: limit, units, text: None, param, bound },
                margin,
                value: d.value,
                required: d.required,
                fix: d.fix,
                section: entry.section,
                note: d.verify,
                span: None,
            },
            family: entry.family(),
            status: d.status,
            regime: d.regime,
            principle: entry.principle,
            measured,
        });
    }

    /// `code` couldn't check one place for want of `needs` (`intent: current_a of VBUS`).
    pub fn not_applicable(&mut self, code: &'static str, needs: impl Into<String>) {
        let (count, list) = self.found.needs.entry(code).or_default();
        *count += 1;
        let needs = needs.into();
        if list.len() < MAX_NEEDS && !list.contains(&needs) {
            list.push(needs);
        }
    }
}

/// A parameter the board sets in place of the catalogue's default, with why and who set it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamSetting {
    pub value: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Actor>,
}

/// The board's quality approvals, levels and parameters: within their caps, keys of codes and
/// parameters the catalogue has, values in the parameters' ranges.
pub fn validate_settings(board: &Board) -> Result<(), String> {
    if board.quality_approvals.len() > super::schematic::MAX_APPROVALS {
        return Err(format!("a board keeps at most {} quality approvals", super::schematic::MAX_APPROVALS));
    }
    for (what, n) in [("levels", board.quality_levels.len()), ("parameters", board.quality_params.len())] {
        if n > MAX_PARAMS {
            return Err(format!("a board keeps at most {MAX_PARAMS} quality-check {what}"));
        }
    }
    let mut keys = BTreeSet::new();
    for a in &board.quality_approvals {
        if a.key.is_empty() || a.key.chars().count() > super::schematic::MAX_KEY || a.key.chars().any(char::is_control) {
            return Err(format!("a quality approval's key must be 1 to {} characters with no control characters", super::schematic::MAX_KEY));
        }
        if !keys.insert(a.key.as_str()) {
            return Err(format!("two quality approvals have the key `{}`", a.key));
        }
        super::check_text("note", &a.note, super::MAX_DESCRIPTION)?;
    }
    for code in board.quality_levels.keys() {
        if catalog::find(code).is_none() {
            return Err(format!("the quality checks have no code `{code}` to give a level"));
        }
    }
    for (key, s) in &board.quality_params {
        let param = key.rsplit_once('.').and_then(|(code, name)| catalog::find(code)?.param(name));
        let Some(param) = param else { return Err(format!("the quality checks have no parameter `{key}`")) };
        if !(s.value.is_finite() && s.value >= param.min && s.value <= param.max) {
            return Err(format!("quality parameter `{key}` must be {} to {} {}", num(param.min), num(param.max), param.units));
        }
        super::check_text("note", &s.note, super::MAX_DESCRIPTION)?;
    }
    Ok(())
}

/// The value of `code`'s parameter `name` on `board`: the board's, else the default.
pub fn param_value(board: &Board, code: &str, name: &str) -> f64 {
    if let Some(v) = board.quality_params.get(&format!("{code}.{name}")) {
        return v.value;
    }
    catalog::find(code).and_then(|c| c.param(name)).map_or(0.0, |p| p.default)
}

impl Electronics {
    /// The design-quality checks of the board, every code (see [`check`]).
    pub fn quality(&self) -> QualityReport {
        check(self, &self.board_nets(), None)
    }
}

/// The design-quality checks of `ecad`'s board, its connections `nets` (pours filled): every
/// code, or those in `only` (see the module notes).
///
/// An agent's parameter on a code whose findings may be critical counts as an approval of what
/// it leaves out: the codes are checked again without the agents' parameters, and the findings
/// only that run has are listed as waiting for the user (`pending_user`, still counted) until the
/// user sets the parameter.
pub fn check(ecad: &Electronics, nets: &BoardNets, only: Option<&BTreeSet<&str>>) -> QualityReport {
    check_within(ecad, nets, only, MAX_WORK)
}

/// [`check`] with `work` units for each family instead of [`MAX_WORK`] (a quick look after an
/// edit: past it, the report is `truncated` and names the codes cut short).
pub fn check_within(ecad: &Electronics, nets: &BoardNets, only: Option<&BTreeSet<&str>>, work: usize) -> QualityReport {
    let work = work.min(MAX_WORK);
    let mut report = run(ecad, nets, only, work);
    let board = &ecad.board;
    let set_by_agent: BTreeSet<&'static str> = board
        .quality_params
        .iter()
        .filter(|(_, v)| v.by != Some(Actor::User))
        .filter_map(|(k, _)| catalog::find(k.rsplit_once('.')?.0))
        .filter(|c| c.worst_class() == Class::Critical && only.is_none_or(|o| o.contains(c.code)) && report.ran.contains(&c.code))
        .map(|c| c.code)
        .collect();
    if set_by_agent.is_empty() {
        return report;
    }
    let mut plain = ecad.clone();
    plain.board.quality_params.retain(|k, v| v.by == Some(Actor::User) || k.rsplit_once('.').is_none_or(|(code, _)| !set_by_agent.contains(code)));
    let base = run(&plain, nets, Some(&set_by_agent), work);
    let have: BTreeSet<String> = report.findings.iter().map(|q| q.finding.key.clone()).collect();
    for mut q in base.findings.into_iter().filter(|q| !q.finding.approved && !have.contains(&q.finding.key)) {
        if report.findings.len() >= MAX_FINDINGS {
            report.truncated = true;
            break;
        }
        let set: Vec<String> = board
            .quality_params
            .iter()
            .filter(|(k, v)| v.by != Some(Actor::User) && k.rsplit_once('.').is_some_and(|(code, _)| code == q.finding.code))
            .map(|(k, v)| format!("{k} = {}", num(v.value)))
            .collect();
        q.finding.pending_user = true;
        q.finding.by = Some(Actor::Agent);
        q.finding.note = Some(format!("left out by an agent's parameter ({}): it counts until the user sets that parameter", set.join(", ")));
        report.findings.push(q);
    }
    let order: BTreeMap<&str, usize> = catalog::codes().enumerate().map(|(i, c)| (c.code, i)).collect();
    report.findings.sort_by_key(|f| (f.finding.severity, order.get(f.finding.code).copied().unwrap_or(usize::MAX)));
    let open = |s: Severity| report.findings.iter().filter(|f| !f.finding.approved && f.finding.severity == s).count();
    (report.errors, report.warnings, report.infos) = (open(Severity::Error), open(Severity::Warning), open(Severity::Info));
    report.pending_user = report.findings.iter().filter(|f| f.finding.pending_user).count();
    report.truncated |= base.truncated;
    report
}

/// One run of the checks, `work` units for each family (see [`check`]).
fn run(ecad: &Electronics, nets: &BoardNets, only: Option<&BTreeSet<&str>>, work: usize) -> QualityReport {
    let board = &ecad.board;
    let conn = ecad.connectivity();
    let roles = Roles::new(ecad, nets, &conn);
    let geo = Geo::new(board, nets);
    let found = Found {
        approved: board.quality_approvals.iter().map(|a| (a.key.as_str(), a)).collect(),
        levels: &board.quality_levels,
        seen: BTreeSet::new(),
        keys: BTreeSet::new(),
        list: Vec::new(),
        needs: BTreeMap::new(),
        truncated: false,
    };
    let mut cx = Cx { ecad, board, nets, conn: &conn, roles: &roles, geo: &geo, only, work: 0, family: None, cut: Vec::new(), found };
    let mut ran = Vec::new();
    for family in FAMILIES {
        let wanted: Vec<&'static str> = family.codes.iter().map(|c| c.code).filter(|c| cx.wants(c)).collect();
        if wanted.is_empty() {
            continue;
        }
        cx.work = work;
        cx.family = Some(family);
        (family.run)(&mut cx);
        ran.extend(wanted);
    }
    cx.family = None;
    let cut = std::mem::take(&mut cx.cut);
    for code in catalog::codes().map(|c| c.code) {
        if only.is_none_or(|o| o.contains(code)) && board.quality_levels.get(code) == Some(&CheckLevel::Ignore) {
            cx.not_applicable(code, "board.quality_levels");
        }
    }
    let Found { seen, mut list, needs, truncated, .. } = cx.found;
    let order: BTreeMap<&str, usize> = catalog::codes().enumerate().map(|(i, c)| (c.code, i)).collect();
    list.sort_by_key(|f| (f.finding.severity, order.get(f.finding.code).copied().unwrap_or(usize::MAX)));
    let open = |s: Severity| list.iter().filter(|f| !f.finding.approved && f.finding.severity == s).count();
    // Only a code that ran can tell an approval of it is stale.
    let stale = if only.is_none() && !truncated {
        board
            .quality_approvals
            .iter()
            .filter(|a| !seen.contains(&a.key) && ran.iter().any(|c| a.key.split('|').next() == Some(*c)))
            .map(|a| a.key.clone())
            .collect()
    } else {
        Vec::new()
    };
    QualityReport {
        errors: open(Severity::Error),
        warnings: open(Severity::Warning),
        infos: open(Severity::Info),
        approved: list.iter().filter(|f| f.finding.approved).count(),
        pending_user: list.iter().filter(|f| f.finding.pending_user).count(),
        not_applicable: needs.into_iter().map(|(code, (count, needs))| NotApplicable { code, count, needs }).collect(),
        findings: list,
        stale,
        ran,
        truncated,
        cut,
    }
}
