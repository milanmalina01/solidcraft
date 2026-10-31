//! The board's design-quality checks (see `solidcraft_doc::ecad::quality`): read their findings
//! (`ecad.quality.inspect`, summary first), give codes levels and parameters with a reason
//! (`ecad.quality.rules`, which also lists the catalogue: what each code finds, why, what it
//! needs, the guide section, the parameters with defaults and sources), approve findings that
//! are fine with a note and take approvals back (`ecad.quality.approve`/`unapprove`). The checks
//! run afresh on every call; only the approvals, levels and parameters are saved, on the board.
//! Approvals follow the design rule check's: an agent's approval of a critical finding waits for
//! the user's, an agent leaves the user's approvals as they are and lowers no critical code.
//!
//! It also gives the `findings_delta` that mutating `ecad.board.*` commands add to their replies
//! (see `Session::execute`; a call's `findings_delta: false` leaves it out): the design rule and
//! quality errors and warnings an edit brought, and how many it settled, from a full check of the
//! board before and after (the one before kept from the last edit's).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use solidcraft_doc::ecad::drc::{Bound as DocBound, CheckLevel, Class as DocClass, DrcItem};
use solidcraft_doc::ecad::quality::{self, MAX_FINDINGS, ParamSetting, QualityFinding, QualityReport, Status, catalog};
use solidcraft_doc::ecad::schematic::MAX_APPROVALS;
use solidcraft_doc::ecad::{Actor as DocActor, MAX_DESCRIPTION, Severity as DocSeverity, check_text};

use super::CommandSpec;
use super::ecad::{closest, parse, reply};
use super::ecad_board::store_board;
use super::ecad_drc::{actor_out, approval_note, approve_keys, class_out, level, level_out, profile_ref, rank, region_test, severity_out, take_back};
use crate::params::bad;
use crate::schema::ecad_drc::{ApproveParams, Bound, DrcFixOut, DrcItemKind, LimitOut, MeasureOut, UnapproveParams};
use crate::schema::ecad_quality::{
    ApprovalResult, CodeOut, CountsOut, DeltaFindingOut, FindingOut, FindingsDeltaOut, InspectParams, InspectResult, NotApplicableOut, ObjectOut,
    ParamOut, RulesParams, RulesResult, StatusParam, SummaryOut, WorstOut,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.quality.inspect", "Quality Checks", crate::checked!("ecad.quality.inspect", inspect))
        .icon("interference")
        .noundo()
        .doc_only()
        .contract(&INSPECT_CONTRACT)
        .output(output_schema_for::<InspectResult>)
        .params(
            "families? [pi|si|rf|esd|silk|dfm]; codes? [code]; nets? [name]; parts? [designator]; region? {rect: [[x0, y0], [x1, y1]]} | {circle: {at, radius}}; layer?; class? critical|practical|cosmetic; severity? error|warning|info; status? fail|review|not_applicable; include_approved? (false); summary? (false: counts, the worst 5 and what couldn't run, no list); limit? (25, at most 10000) offset? → {counts: {errors, warnings, infos, approved, pending_user, stale}, summary: {by_class: {critical: {fail, review, not_applicable}, practical, cosmetic}, by_code, worst: [{code, key, message, margin?, units?}]}, findings: [{code, family, class, severity, status, key, approved, pending_user?, by?, note?, principle, section, message, message_key, args, at, layer?, objects: [{ref, kind, id, key, pad?, end?}], nets?, measured?: {value, units}, limit?: {value, units, param, bound}, margin?, regime?, fix?: [{command, params?, says}], verify?}], approved?, more?, not_applicable?: [{code, count, needs: [what]}] (no finding of a code listed here is no pass: it says what it needs, such as intent: driver or edge_ns of a net), not_built? (families not built yet), stale?, profile?, engine, truncated?}: the design-quality checks, which need the design's intent (ecad.intent.inspect / ecad.intent.set): return paths of fast nets over gaps in their reference plane, layer changes without a return via, ground stitching against the frequency of concern, and the families as they are built. ecad.quality.rules lists every code with what it finds, why, what it needs, its guide section and parameters. A key names ids only (`pi.return_path|t5|z9`). Read-only",
        ),
    CommandSpec::new("ecad.quality.rules", "Quality Rules", crate::checked!("ecad.quality.rules", rules))
        .icon("settings")
        .doc_only()
        .contract(&RULES_CONTRACT)
        .output(output_schema_for::<RulesResult>)
        .params(
            "rules? {code: error|warning|info|ignore|null}; params? {code: {param: value|null}} (null: back to the default; each within the range the catalogue gives); note? (why: needed with params) → {rules, params: {code.param: value}, codes: [{code, family, class, severity, level?, needs, section, about, principle, params: [{name, default, value?, units, source, about, min, max, verify?, by?, note?}]}], not_built?, counts}: the board's levels and parameters for the quality checks, one undo step; with neither, the catalogue. Only the user lowers a critical code or leaves it out; an agent's parameter changes are kept with who made them and shown by ecad.board.check",
        ),
    CommandSpec::new("ecad.quality.approve", "Approve Quality Finding", crate::checked!("ecad.quality.approve", approve))
        .icon("select")
        .doc_only()
        .contract(&APPROVE_CONTRACT)
        .output(output_schema_for::<ApprovalResult>)
        .params(
            "keys: [key] (as ecad.quality.inspect gives them; each a finding's now), note? (why it is fine; an agent must give one) → {added, changed, confirmed, kept, pending_user, counts}: approved findings no longer count; saved on the board with who approved. An agent's approval of a critical finding stays pending_user (it still counts) until the user approves it in the app; an agent leaves the user's approvals as they are (kept)",
        ),
    CommandSpec::new("ecad.quality.unapprove", "Unapprove Quality Finding", crate::checked!("ecad.quality.unapprove", unapprove))
        .icon("restore")
        .doc_only()
        .contract(&UNAPPROVE_CONTRACT)
        .output(output_schema_for::<ApprovalResult>)
        .params(
            "keys? [key] | all? true | stale? true (the approvals no finding has now; refused while a check is cut short) → {removed, kept, counts}: an agent takes back only its own approvals (and stale ones); the user's stay (kept)",
        ),
];

const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();
const RULES_CONTRACT: TypedContract<RulesParams> = TypedContract::new();
const APPROVE_CONTRACT: TypedContract<ApproveParams> = TypedContract::new();
const UNAPPROVE_CONTRACT: TypedContract<UnapproveParams> = TypedContract::new();

/// The most findings `ecad.quality.inspect` lists when no `limit` is given.
const DEFAULT_LIMIT: usize = 25;
/// The worst findings the summary names.
const WORST: usize = 5;
/// The checks' own version, bumped when what they find changes.
pub(super) const QUALITY_VERSION: u32 = 1;
/// The families' prefixes a filter may name.
const FAMILY_PREFIXES: &[&str] = &["dfm", "esd", "pi", "rf", "si", "silk"];
/// The most introduced findings a `findings_delta` lists.
const DELTA_LIST: usize = 20;

fn round(v: f64) -> f64 {
    (v * 1e4).round() / 1e4 + 0.0
}

/// The checks of the stored design (its connections shared with the other commands'), every code
/// (worked out once for each design) or those in `only`.
pub(super) fn report(s: &Session, only: Option<&BTreeSet<&str>>) -> std::sync::Arc<QualityReport> {
    match only {
        None => s.quality_report(),
        Some(_) => std::sync::Arc::new(quality::check(&s.doc.ecad, &s.board_nets(), only)),
    }
}

fn object_out(i: &DrcItem) -> ObjectOut {
    let (kind, pad, end) = match i {
        DrcItem::Pad { number, .. } => (DrcItemKind::Pad, Some(number.clone()), None),
        DrcItem::Footprint { .. } => (DrcItemKind::Footprint, None, None),
        DrcItem::Track(_) => (DrcItemKind::Track, None, None),
        DrcItem::TrackEnd(_, e) => (DrcItemKind::TrackEnd, None, Some(*e)),
        DrcItem::Via(_) => (DrcItemKind::Via, None, None),
        DrcItem::Hole(_) => (DrcItemKind::Hole, None, None),
        DrcItem::Pour(_) => (DrcItemKind::Pour, None, None),
        DrcItem::Keepout(_) => (DrcItemKind::Keepout, None, None),
        DrcItem::Graphic(_) => (DrcItemKind::Graphic, None, None),
    };
    ObjectOut { reference: i.reference(), kind, id: i.id(), key: i.key(), pad, end }
}

fn status_out(s: Status) -> StatusParam {
    match s {
        Status::Fail => StatusParam::Fail,
        Status::Review => StatusParam::Review,
    }
}

/// A finding as replies give it, with its approval's note.
pub(super) fn finding_out(q: &QualityFinding, notes: &BTreeMap<&str, &str>) -> FindingOut {
    let f = &q.finding;
    let (measured, limit, margin) = if q.measured {
        (
            Some(MeasureOut { value: round(f.measured), units: f.limit.units.to_string(), text: None }),
            Some(LimitOut {
                value: round(f.limit.value),
                units: f.limit.units.to_string(),
                text: None,
                param: f.limit.param.clone(),
                bound: match f.limit.bound {
                    DocBound::Min => Bound::Min,
                    DocBound::Max => Bound::Max,
                },
            }),
            Some(round(f.margin)),
        )
    } else {
        (None, None, None)
    };
    FindingOut {
        code: f.code.to_string(),
        family: q.family.to_string(),
        class: class_out(f.class),
        severity: severity_out(f.severity),
        status: status_out(q.status),
        key: f.key.clone(),
        approved: f.approved,
        pending_user: f.pending_user,
        by: f.by.map(actor_out),
        note: notes.get(f.key.as_str()).filter(|n| !n.is_empty()).map(|n| n.to_string()),
        principle: q.principle.to_string(),
        section: f.section.to_string(),
        message: f.message.clone(),
        message_key: f.message_key.clone(),
        args: f.args.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        at: [round(f.at[0]), round(f.at[1])],
        layer: f.layer.map(|l| l.name()),
        objects: f.items.iter().map(object_out).collect(),
        nets: f.nets.clone(),
        measured,
        limit,
        margin,
        regime: q.regime.clone(),
        fix: f
            .fix
            .iter()
            .map(|x| DrcFixOut { command: x.command.to_string(), params: (!x.params.is_null()).then(|| x.params.clone()), says: x.says.clone() })
            .collect(),
        verify: f.note.clone(),
    }
}

pub(super) fn counts(r: &QualityReport) -> CountsOut {
    CountsOut { errors: r.errors, warnings: r.warnings, infos: r.infos, approved: r.approved, pending_user: r.pending_user, stale: r.stale.len() }
}

fn not_applicable(r: &QualityReport) -> Vec<NotApplicableOut> {
    r.not_applicable.iter().map(|n| NotApplicableOut { code: n.code.to_string(), count: n.count, needs: n.needs.clone() }).collect()
}

/// The counts by class and status, by code, and the worst few of `open`.
fn summary(open: &[&QualityFinding], r: &QualityReport) -> SummaryOut {
    let mut out = SummaryOut::default();
    for q in open {
        let s = match q.finding.class {
            DocClass::Critical => &mut out.by_class.critical,
            DocClass::Practical => &mut out.by_class.practical,
            DocClass::Cosmetic => &mut out.by_class.cosmetic,
        };
        match q.status {
            Status::Fail => s.fail += 1,
            Status::Review => s.review += 1,
        }
        match q.finding.severity {
            DocSeverity::Error => s.errors += 1,
            DocSeverity::Warning => s.warnings += 1,
            DocSeverity::Info => s.infos += 1,
        }
        *out.by_code.entry(q.finding.code.to_string()).or_default() += 1;
    }
    for n in &r.not_applicable {
        let s = match catalog::find(n.code).map(|c| c.class) {
            Some(DocClass::Critical) => &mut out.by_class.critical,
            Some(DocClass::Practical) => &mut out.by_class.practical,
            _ => &mut out.by_class.cosmetic,
        };
        s.not_applicable += 1;
    }
    // Class and severity first, then how far past its limit each is as a share of the limit
    // (units differ: °C, mm, crossings), then catalogue order.
    let order: BTreeMap<&str, usize> = catalog::codes().enumerate().map(|(i, c)| (c.code, i)).collect();
    let past = |q: &QualityFinding| {
        let f = &q.finding;
        if !q.measured {
            return 0.0;
        }
        let scale = f.limit.value.abs();
        if scale > 1e-9 { f.margin / scale } else { f.margin.min(0.0) }
    };
    let mut worst: Vec<&&QualityFinding> = open.iter().filter(|q| q.finding.severity != DocSeverity::Info).collect();
    worst.sort_by(|a, b| {
        (a.finding.class, a.finding.severity)
            .cmp(&(b.finding.class, b.finding.severity))
            .then(past(a).total_cmp(&past(b)))
            .then(order.get(a.finding.code).cmp(&order.get(b.finding.code)))
    });
    out.worst = worst
        .into_iter()
        .take(WORST)
        .map(|q| WorstOut {
            code: q.finding.code.to_string(),
            key: q.finding.key.clone(),
            message: q.finding.message.clone(),
            margin: q.measured.then(|| round(q.finding.margin)),
            units: q.measured.then(|| q.finding.limit.units.to_string()),
        })
        .collect();
    out
}

/// The codes `list` names (each the catalogue's), and the families' (each one the checks have).
fn wanted_codes(codes: Option<&[String]>, families: Option<&[String]>, cmd: &str) -> Result<Option<BTreeSet<&'static str>>> {
    let all: Vec<&'static str> = catalog::codes().map(|c| c.code).collect();
    let mut out: Option<BTreeSet<&'static str>> = None;
    if let Some(list) = codes {
        if list.len() > all.len().max(1) * 4 {
            return Err(bad(cmd, format!("`codes` names at most {} codes", all.len())));
        }
        let mut set = BTreeSet::new();
        for (i, c) in list.iter().enumerate() {
            let Some(code) = catalog::find(c) else {
                let mut msg = format!("`codes[{i}]`: there is no quality code `{c}`");
                if let Some(near) = closest(c, all.iter().copied()) {
                    msg.push_str(&format!(" (did you mean `{near}`?)"));
                } else {
                    msg.push_str(" (ecad.quality.rules lists them)");
                }
                return Err(bad(cmd, msg));
            };
            set.insert(code.code);
        }
        out = Some(set);
    }
    if let Some(list) = families {
        if list.len() > FAMILY_PREFIXES.len() * 4 {
            return Err(bad(cmd, format!("`families` names at most {} families", FAMILY_PREFIXES.len())));
        }
        for (i, f) in list.iter().enumerate() {
            if !FAMILY_PREFIXES.contains(&f.as_str()) {
                let mut msg = format!("`families[{i}]`: there is no family `{f}`: there are {}", FAMILY_PREFIXES.join(", "));
                if let Some(near) = closest(f, FAMILY_PREFIXES.iter().copied()) {
                    msg.push_str(&format!(" (did you mean `{near}`?)"));
                }
                return Err(bad(cmd, msg));
            }
        }
        let of: BTreeSet<&'static str> = catalog::codes().filter(|c| list.iter().any(|f| f == c.family())).map(|c| c.code).collect();
        out = Some(match out {
            Some(codes) => codes.intersection(&of).copied().collect(),
            None => of,
        });
    }
    Ok(out)
}

/// Names `list` gives that `known` lacks: an error naming the first.
fn check_names<'a>(
    list: &[String],
    known: impl Fn(&str) -> bool,
    names: impl Iterator<Item = &'a str>,
    what: &str,
    field: &str,
    cmd: &str,
) -> Result<()> {
    if list.len() > 4096 {
        return Err(bad(cmd, format!("`{field}` names at most 4096 {what}s")));
    }
    let names: Vec<&str> = names.collect();
    for (i, n) in list.iter().enumerate() {
        if !known(n) {
            let mut msg = format!("`{field}[{i}]`: the design has no {what} `{n}`");
            if let Some(near) = closest(n, names.iter().copied()) {
                msg.push_str(&format!(" (did you mean `{near}`?)"));
            }
            return Err(bad(cmd, msg));
        }
    }
    Ok(())
}

/// The part a finding's object belongs to: a pad's or a footprint's designator.
fn designator(i: &DrcItem) -> Option<&str> {
    match i {
        DrcItem::Pad { reference, .. } => reference.split_once('.').map(|(d, _)| d),
        DrcItem::Footprint { designator, .. } => Some(designator.as_str()),
        _ => None,
    }
}

fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.quality.inspect";
    let p: InspectParams = parse(CMD, p)?;
    let only = wanted_codes(p.codes.as_deref(), p.families.as_deref(), CMD)?;
    if let Some(l) = p.layer
        && !s.doc.ecad.board.has_layer(l.0)
    {
        return Err(bad(CMD, super::ecad_board::no_layer(&s.doc.ecad.board, l.0.name())));
    }
    let region = p.region.as_ref().map(|r| region_test(r, CMD)).transpose()?;
    let limit = p.limit.unwrap_or(DEFAULT_LIMIT);
    if limit > MAX_FINDINGS {
        return Err(bad(CMD, format!("`limit` is at most {MAX_FINDINGS}")));
    }
    let offset = p.offset.unwrap_or(0);
    let nets = s.board_nets();
    if let Some(list) = &p.nets {
        check_names(list, |n| nets.names.iter().any(|x| x == n), nets.names.iter().map(String::as_str), "net", "nets", CMD)?;
    }
    let ecad = &s.doc.ecad;
    if let Some(list) = &p.parts {
        let parts = &ecad.schematic.parts;
        check_names(list, |d| parts.iter().any(|x| x.designator == d), parts.iter().map(|x| x.designator.as_str()), "part", "parts", CMD)?;
    }
    let r = report(s, only.as_ref());
    let ecad = &s.doc.ecad;
    let board = &ecad.board;
    let notes: BTreeMap<&str, &str> = board.quality_approvals.iter().map(|a| (a.key.as_str(), a.note.as_str())).collect();
    let wanted = |q: &&QualityFinding| {
        let f = &q.finding;
        p.nets.as_ref().is_none_or(|n| f.nets.iter().any(|x| n.contains(x)))
            && p.parts.as_ref().is_none_or(|ps| f.items.iter().filter_map(designator).any(|d| ps.iter().any(|x| x == d)))
            && p.layer.is_none_or(|l| f.layer.is_none_or(|fl| fl == l.0))
            && p.severity.is_none_or(|want| severity_out(f.severity) == want)
            && p.class.is_none_or(|want| class_out(f.class) == want)
            && p.status.is_none_or(|want| want == status_out(q.status))
            && region.as_ref().is_none_or(|inside| inside(f.at))
    };
    let summary_only = p.summary.unwrap_or(false);
    let open: Vec<&QualityFinding> = r.findings.iter().filter(wanted).filter(|q| !q.finding.approved).collect();
    let listing = !summary_only && p.status != Some(StatusParam::NotApplicable);
    let (findings, more) = if listing {
        let listed: Vec<FindingOut> = open.iter().skip(offset).take(limit).map(|q| finding_out(q, &notes)).collect();
        let more = open.len().saturating_sub(offset).saturating_sub(listed.len());
        (listed, more)
    } else {
        (Vec::new(), 0)
    };
    let approved = if listing && p.include_approved.unwrap_or(false) {
        r.findings.iter().filter(wanted).filter(|q| q.finding.approved).take(limit).map(|q| finding_out(q, &notes)).collect()
    } else {
        Vec::new()
    };
    reply(InspectResult {
        counts: counts(&r),
        summary: summary(&open, &r),
        findings,
        approved,
        more,
        not_applicable: not_applicable(&r),
        not_built: catalog::not_built().into_iter().map(str::to_string).collect(),
        stale: r.stale.clone(),
        profile: board.fab_profile.as_ref().and_then(|c| c.profile()).map(profile_ref),
        engine: engine(),
        truncated: r.truncated,
        cut: r.cut.iter().map(|c| c.to_string()).collect(),
        param_warnings: Vec::new(),
    })
}

/// The engine's and the checks' versions, as reports give them.
pub(super) fn engine() -> String {
    format!("solidcraft-engine {}, quality checks {QUALITY_VERSION}", env!("CARGO_PKG_VERSION"))
}

/// Every code with the board's level and parameters.
fn codes_out(board: &solidcraft_doc::ecad::Board) -> Vec<CodeOut> {
    catalog::codes()
        .map(|c| CodeOut {
            code: c.code.to_string(),
            family: c.family().to_string(),
            class: class_out(c.class),
            severity: severity_out(c.severity),
            worst_class: (c.worst_class() != c.class).then(|| class_out(c.worst_class())),
            worst_severity: (c.worst_severity() != c.severity).then(|| severity_out(c.worst_severity())),
            level: board.quality_levels.get(c.code).copied().map(level_out),
            needs: c.needs.to_string(),
            section: c.section.to_string(),
            about: c.about.to_string(),
            principle: c.principle.to_string(),
            params: c
                .params
                .iter()
                .map(|x| {
                    let set = board.quality_params.get(&format!("{}.{}", c.code, x.name));
                    ParamOut {
                        name: x.name.to_string(),
                        default: x.default,
                        value: set.map(|v| v.value),
                        units: x.units.to_string(),
                        source: x.source.to_string(),
                        about: x.about.to_string(),
                        min: x.min,
                        max: x.max,
                        verify: x.verify,
                        by: set.and_then(|v| v.by).map(actor_out),
                        note: set.map(|v| v.note.clone()).filter(|n| !n.is_empty()),
                    }
                })
                .collect(),
        })
        .collect()
}

fn rules(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.quality.rules";
    let p: RulesParams = parse(CMD, p)?;
    let note = p.note.unwrap_or_default().trim().to_string();
    check_text("note", &note, MAX_DESCRIPTION).map_err(|e| bad(CMD, e))?;
    let all: Vec<&'static str> = catalog::codes().map(|c| c.code).collect();
    let no_code = |code: &str, field: &str| {
        let mut msg = format!("`{field}`: there is no quality code `{code}`");
        if let Some(near) = closest(code, all.iter().copied()) {
            msg.push_str(&format!(" (did you mean `{near}`?)"));
        } else {
            msg.push_str(" (call ecad.quality.rules with {} to list the codes)");
        }
        bad(CMD, msg)
    };
    let mut board = s.doc.ecad.board.clone();
    if let Some(levels) = &p.rules {
        if levels.len() > all.len() {
            return Err(bad(CMD, format!("`rules` names at most {} codes", all.len())));
        }
        for (code, l) in levels {
            let Some(entry) = catalog::find(code) else { return Err(no_code(code, "rules")) };
            let to = l.map(level);
            // A level rewrites every finding's severity: judged against the worst a finding of
            // the code may be, for any code some of whose findings may be critical.
            let worst = entry.worst_severity();
            if s.actor == DocActor::Agent
                && entry.worst_class() == DocClass::Critical
                && rank(to, worst) < rank(board.quality_levels.get(code).copied(), worst)
            {
                return Err(bad(
                    CMD,
                    format!(
                        "`rules`: `{code}`'s findings may be critical (up to {}): only the user lowers or leaves out such a code's findings; approve them one by one with a note instead (an agent's approval of a critical finding waits for the user's)",
                        match worst {
                            DocSeverity::Error => "errors",
                            DocSeverity::Warning => "warnings",
                            DocSeverity::Info => "infos",
                        }
                    ),
                ));
            }
            match to {
                Some(l) => board.quality_levels.insert(code.clone(), l),
                None => board.quality_levels.remove(code),
            };
        }
    }
    let mut kept: Vec<String> = Vec::new();
    if let Some(params) = &p.params {
        if !params.is_empty() && note.is_empty() {
            return Err(bad(CMD, "give a `note` with `params`: why the board needs another value (the datasheet, the fab, the product)"));
        }
        if params.len() > all.len() {
            return Err(bad(CMD, format!("`params` names at most {} codes", all.len())));
        }
        for (code, values) in params {
            let Some(entry) = catalog::find(code) else { return Err(no_code(code, "params")) };
            if values.len() > entry.params.len().max(1) {
                return Err(bad(CMD, format!("`params.{code}` names at most {} parameters", entry.params.len())));
            }
            for (name, v) in values {
                let Some(param) = entry.param(name) else {
                    let names: Vec<&str> = entry.params.iter().map(|x| x.name).collect();
                    let mut msg = format!(
                        "`params.{code}`: `{code}` has no parameter `{name}` (it has {})",
                        if names.is_empty() { "none".into() } else { names.join(", ") }
                    );
                    if let Some(near) = closest(name, names.iter().copied()) {
                        msg.push_str(&format!(" (did you mean `{near}`?)"));
                    }
                    return Err(bad(CMD, msg));
                };
                let key = format!("{code}.{name}");
                // The user's setting stays as the user left it.
                if s.actor == DocActor::Agent && board.quality_params.get(&key).is_some_and(|x| x.by == Some(DocActor::User)) {
                    kept.push(key);
                    continue;
                }
                match v {
                    Some(v) if v.is_finite() && *v >= param.min && *v <= param.max => {
                        board.quality_params.insert(key, ParamSetting { value: *v, note: note.clone(), by: Some(s.actor) });
                    }
                    Some(_) => return Err(bad(CMD, format!("`params.{code}.{name}` must be {} to {} {}", param.min, param.max, param.units))),
                    None => {
                        board.quality_params.remove(&key);
                    }
                }
            }
        }
    }
    store_board(s, board, CMD)?;
    let r = report(s, None);
    let board = &s.doc.ecad.board;
    reply(RulesResult {
        rules: board.quality_levels.iter().map(|(code, l)| (code.clone(), level_out(*l))).collect(),
        params: board.quality_params.iter().map(|(k, v)| (k.clone(), v.value)).collect(),
        codes: codes_out(board),
        not_built: catalog::not_built().into_iter().map(str::to_string).collect(),
        kept,
        counts: counts(&r),
        param_warnings: Vec::new(),
    })
}

/// The approval keys `keys` names, at least one, each once.
fn key_list(keys: &[String], cmd: &str) -> Result<Vec<String>> {
    if keys.is_empty() || keys.len() > MAX_APPROVALS {
        return Err(bad(cmd, format!("name 1 to {MAX_APPROVALS} findings in `keys`, by the keys ecad.quality.inspect gives")));
    }
    let mut seen = BTreeSet::new();
    Ok(keys.iter().filter(|k| seen.insert(k.as_str())).cloned().collect())
}

fn approve(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.quality.approve";
    let p: ApproveParams = parse(CMD, p)?;
    let keys = key_list(&p.keys, CMD)?;
    let note = approval_note(s, p.note, CMD)?;
    let r = report(s, None);
    let found: BTreeSet<&str> = r.findings.iter().map(|q| q.finding.key.as_str()).collect();
    for (i, k) in keys.iter().enumerate() {
        if found.contains(k.as_str()) {
            continue;
        }
        let mut msg = format!("`keys[{i}]`: no quality finding has the key `{k}`");
        if let Some(near) = closest(k, found.iter().copied()) {
            msg.push_str(&format!(" (did you mean `{near}`?)"));
        } else if found.is_empty() {
            msg.push_str(" (the checks find nothing now)");
        } else {
            msg.push_str(" (ecad.quality.inspect lists the findings and their keys)");
        }
        return Err(bad(CMD, msg));
    }
    let critical: BTreeSet<&str> = r.findings.iter().filter(|q| q.finding.class == DocClass::Critical).map(|q| q.finding.key.as_str()).collect();
    let mut board = s.doc.ecad.board.clone();
    let done = approve_keys(&mut board.quality_approvals, keys, &note, s.actor, &critical);
    if board.quality_approvals.len() > MAX_APPROVALS {
        return Err(bad(
            CMD,
            format!("a board keeps at most {MAX_APPROVALS} quality approvals: drop stale ones with ecad.quality.unapprove {{stale: true}}"),
        ));
    }
    store_board(s, board, CMD)?;
    let r = report(s, None);
    reply(ApprovalResult {
        added: done.added,
        changed: done.changed,
        confirmed: done.confirmed,
        kept: done.kept,
        pending_user: done.pending_user,
        removed: Vec::new(),
        counts: counts(&r),
        param_warnings: Vec::new(),
    })
}

fn unapprove(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.quality.unapprove";
    let p: UnapproveParams = parse(CMD, p)?;
    let all = p.all.unwrap_or(false);
    let stale = p.stale.unwrap_or(false);
    let mut board = s.doc.ecad.board.clone();
    let gone: BTreeSet<String> = match (&p.keys, all, stale) {
        (Some(keys), false, false) => {
            let keys = key_list(keys, CMD)?;
            for (i, k) in keys.iter().enumerate() {
                if !board.quality_approvals.iter().any(|a| a.key == *k) {
                    let mut msg = format!("`keys[{i}]`: `{k}` is not approved");
                    if let Some(near) = closest(k, board.quality_approvals.iter().map(|a| a.key.as_str())) {
                        msg.push_str(&format!(" (did you mean `{near}`?)"));
                    }
                    return Err(bad(CMD, msg));
                }
            }
            keys.into_iter().collect()
        }
        (None, true, false) => board.quality_approvals.iter().map(|a| a.key.clone()).collect(),
        (None, false, true) => {
            let r = report(s, None);
            if r.truncated {
                return Err(bad(
                    CMD,
                    "the checks were cut short by their budget, so they can't tell which approvals are stale: take them back by `keys`",
                ));
            }
            r.stale.iter().cloned().collect()
        }
        _ => return Err(bad(CMD, "give one of `keys`, `all: true` or `stale: true`")),
    };
    let (removed, kept) = take_back(&mut board.quality_approvals, &gone, s.actor, stale);
    store_board(s, board, CMD)?;
    let r = report(s, None);
    reply(ApprovalResult {
        added: Vec::new(),
        changed: Vec::new(),
        confirmed: Vec::new(),
        kept,
        pending_user: Vec::new(),
        removed,
        counts: counts(&r),
        param_warnings: Vec::new(),
    })
}

/// The errors and warnings not approved, of the design rule check and the quality checks, by key
/// (each with its code and message), and whether either check was cut short.
pub(crate) type Open = (BTreeMap<String, (String, String)>, bool);

/// The work each quality family may do for a `findings_delta` (an eighth of a full check's):
/// past it the delta says `truncated`, and ecad.board.check looks at everything.
const DELTA_WORK: usize = quality::MAX_WORK / 8;

/// The open findings (see [`Open`]) of the stored design.
pub(crate) fn open_findings(s: &Session) -> Open {
    let nets = s.board_nets();
    let ecad = &s.doc.ecad;
    let drc = solidcraft_doc::ecad::drc::check(ecad, &nets);
    let q = quality::check_within(ecad, &nets, None, DELTA_WORK);
    let counts = |sev: DocSeverity, approved: bool| sev != DocSeverity::Info && !approved;
    let mut out = BTreeMap::new();
    for f in drc.findings.iter().filter(|f| counts(f.severity, f.approved)) {
        out.insert(f.key.clone(), (f.code.to_string(), f.message.clone()));
    }
    for f in q.findings.iter().map(|q| &q.finding).filter(|f| counts(f.severity, f.approved)) {
        out.insert(f.key.clone(), (f.code.to_string(), f.message.clone()));
    }
    (out, drc.truncated || q.truncated)
}

/// What changed between the open findings `before` and `after`.
pub(crate) fn findings_delta(before: &Open, after: &Open) -> FindingsDeltaOut {
    let introduced: Vec<DeltaFindingOut> = after
        .0
        .iter()
        .filter(|(k, _)| !before.0.contains_key(*k))
        .map(|(k, (code, message))| DeltaFindingOut { code: code.clone(), key: k.clone(), message: message.clone() })
        .collect();
    let resolved = before.0.keys().filter(|k| !after.0.contains_key(*k)).count();
    let more = introduced.len().saturating_sub(DELTA_LIST);
    FindingsDeltaOut {
        introduced: introduced.into_iter().take(DELTA_LIST).collect(),
        introduced_more: more,
        resolved,
        truncated: before.1 || after.1,
    }
}

/// Whether a design-quality code is one of the levels' or approvals' codes (for the board check).
pub(super) fn is_quality_code(code: &str) -> bool {
    code.contains('.') && catalog::find(code).is_some()
}

/// The quality levels the board gives, as `code (level)`, and the parameters an agent set on
/// critical codes, as the board check's notes give them.
pub(super) fn settings_notes(board: &solidcraft_doc::ecad::Board) -> (Vec<(DocClass, String)>, Vec<String>) {
    let levels = board
        .quality_levels
        .iter()
        .filter_map(|(code, l)| {
            let c = catalog::find(code)?;
            let name = match l {
                CheckLevel::Error => "error",
                CheckLevel::Warning => "warning",
                CheckLevel::Info => "info",
                CheckLevel::Ignore => "ignore",
            };
            Some((c.worst_class(), format!("{code} ({name})")))
        })
        .collect();
    let params = board
        .quality_params
        .iter()
        .filter(|(k, v)| {
            v.by != Some(DocActor::User)
                && k.rsplit_once('.').and_then(|(code, _)| catalog::find(code)).is_some_and(|c| c.worst_class() == DocClass::Critical)
        })
        .map(|(k, v)| format!("{k} = {} (by an agent: {})", v.value, if v.note.is_empty() { "no note" } else { v.note.as_str() }))
        .collect();
    (levels, params)
}

/// The parameter every board edit takes on top of its own: whether its reply gives
/// `findings_delta` (see `Session::execute`).
pub(crate) const DELTA_PARAM: &str = "findings_delta";

/// Whether `spec` is a board edit, whose call takes `findings_delta` and whose reply gives it.
pub(crate) fn board_edit(spec: &CommandSpec) -> bool {
    spec.id.starts_with("ecad.board.") && spec.undoable
}

/// A board edit's input schema with `findings_delta` among its properties (its own properties,
/// `check` among them, kept as they are).
pub(crate) fn with_check(spec: &CommandSpec, mut schema: Value) -> Value {
    if board_edit(spec)
        && let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut)
    {
        props.entry(DELTA_PARAM).or_insert_with(|| {
            serde_json::json!({"type": "boolean", "description": "Report what the edit changed in the design rule and quality findings (findings_delta in the reply): true by default for programs (the control channel, MCP, scripts), false for the app's own window edits. It checks the whole board after the edit, within a smaller work budget than ecad.board.check (truncated says when that ran out): on a routed 6-layer board of about 150 parts it adds about half a second."})
        });
    }
    schema
}

/// A board edit's output schema with `findings_delta` among its properties.
pub(crate) fn with_delta(spec: &CommandSpec, mut schema: Value) -> Value {
    if !board_edit(spec) {
        return schema;
    }
    let mut delta = output_schema_for::<FindingsDeltaOut>().to_value();
    let defs = delta.as_object_mut().and_then(|o| {
        o.remove("$schema");
        o.remove("title");
        o.remove("$defs")
    });
    if let (Some(Value::Object(defs)), Some(root)) = (defs, schema.as_object_mut()) {
        let into = root.entry("$defs").or_insert_with(|| Value::Object(Default::default()));
        if let Some(into) = into.as_object_mut() {
            for (k, v) in defs {
                into.entry(k).or_insert(v);
            }
        }
    }
    if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        props.insert("findings_delta".into(), delta);
    }
    schema
}

#[cfg(test)]
#[path = "ecad_quality_tests.rs"]
mod tests;
