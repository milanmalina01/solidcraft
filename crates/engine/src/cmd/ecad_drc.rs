//! The design rule check of the board: read its findings, approve those that are fine (with a
//! note), take approvals back, and give codes another level. The check runs afresh on every call
//! (see `solidcraft_doc::ecad::drc`); only the approvals and the levels are saved, on the board.
//! An approval records who made it (`Session::actor`): an agent must say why, and its approval
//! of a critical finding (as one of no known author) waits for the user's, who confirms it by
//! approving the key again. An agent leaves the user's approvals as they are, takes back only its
//! own, and lowers no critical code's level. Each change is one undo step, all or none, and a
//! call that changes nothing makes none.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use solidcraft_doc::ecad::drc::{self, Bound as DocBound, CheckLevel, Class as DocClass, DrcFinding, DrcItem, DrcReport, RULES};
use solidcraft_doc::ecad::profiles::FabProfile;
use solidcraft_doc::ecad::schematic::MAX_APPROVALS;
use solidcraft_doc::ecad::{Actor as DocActor, Approval, MAX_DESCRIPTION, Severity as DocSeverity, check_text};

use super::CommandSpec;
use super::ecad::{closest, parse, reply};
use super::ecad_board::store_board;
use crate::params::bad;
use crate::schema::ecad_drc::{
    Actor, ApprovalResult, ApproveParams, Bound, Class, ClassCounts, DrcCodeOut, DrcCounts, DrcFindingOut, DrcFixOut, DrcItemKind, DrcItemOut,
    InspectParams, InspectResult, Level, LimitOut, MeasureOut, NotApplicableOut, ProfileRef, RegionParam, RulesParams, RulesResult, Severity,
    UnapproveParams,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.drc.inspect", "DRC", crate::checked!("ecad.drc.inspect", inspect))
        .icon("interference")
        .noundo()
        .doc_only()
        .contract(&INSPECT_CONTRACT)
        .output(output_schema_for::<InspectResult>)
        .params(
            "codes? [code]; layer? (a copper layer: its findings and those on no one layer); severity? error|warning|info; class? critical|practical|cosmetic; region? {rect: [[x0, y0], [x1, y1]]} | {circle: {at, radius}}; summary? (counts only); limit? (50) offset?; include_approved? (true) → {counts: {errors, warnings, infos, approved, pending_user, stale, at_limit, by_code, by_class}, profile?: {id, name, version, hash, ring_convention}, findings: [{code, class, severity, key, approved, pending_user?, by?, note?, message, message_key, args, at, layer?, items: [{ref, kind, id, pad?, end?}], nets?, measured: {value, units}, limit: {value, units, param, bound: min|max}, margin (negative: past the limit), fix?: [{command, params?, says}], section, verify?}], approved, more?, stale, not_applicable?: [{code, needs}] (codes not checked: no fab profile, or left out by the board's levels; no finding of one is no pass), truncated?}: the design rule check of the board against its rules and fab profile, errors first: shorts, clearance (class-aware, per layer, pour fills included), track width, annular rings (on the hole the profile measures from: finished, or the drill tool), drills, aspect ratios, holes near inner-layer copper, vias in pads, hole to hole, copper to the edge, drills and footprints off the board, overlapping courtyards, keepouts, starved thermal reliefs; then warnings and infos: tracks under their class's width, silkscreen on pads, unrouted connections, dangling ends, via stubs, pours on unknown nets or filling nothing, no fab profile, what the board needs of the fab (fab_class). ecad.drc.rules lists every code. A key names ids only (`clearance|p12.3|t5`), so it outlives other edits. measured is signed (minus how far two things overlap, or copper is past the edge). section names a section of the solidcraft-pcb guide (routing-dfm#vias: reference/routing-dfm.md, Vias; MCP skill_reference serves it). Read-only",
        ),
    CommandSpec::new("ecad.drc.approve", "Approve DRC Finding", crate::checked!("ecad.drc.approve", approve))
        .icon("select")
        .doc_only()
        .contract(&APPROVE_CONTRACT)
        .output(output_schema_for::<ApprovalResult>)
        .params(
            "keys: [key] (as ecad.drc.inspect gives them; each must be a finding's now), note? (why it is fine; an agent must give one) → {added, changed, confirmed, kept, pending_user, counts}: approved findings no longer count; the approvals are saved on the board with who made them. An agent's approval of a critical finding stays pending_user (it still counts) until the user approves it in the app; an agent leaves the user's approvals as they are (kept)",
        ),
    CommandSpec::new("ecad.drc.unapprove", "Unapprove DRC Finding", crate::checked!("ecad.drc.unapprove", unapprove))
        .icon("restore")
        .doc_only()
        .contract(&UNAPPROVE_CONTRACT)
        .output(output_schema_for::<ApprovalResult>)
        .params("keys? [key] | all? true | stale? true (the approvals no finding has now; refused while the check is cut short by its budget) → {removed, kept, counts}: an agent takes back only its own approvals (and stale ones); the user's stay (kept)"),
    CommandSpec::new("ecad.drc.rules", "DRC Levels", crate::checked!("ecad.drc.rules", rules))
        .icon("settings")
        .doc_only()
        .contract(&RULES_CONTRACT)
        .output(output_schema_for::<RulesResult>)
        .params(
            "rules: {code: error|warning|info|ignore|null} (null: back to the code's own level; {} changes nothing) → {rules, codes: [{code, class, severity, level?, section, param, about}], counts}: the board's levels for the design rule check's codes; ignore leaves a code's findings out (ecad.drc.inspect and ecad.board.check list it as not checked). Only the user lowers a critical code: an agent may raise one, and set the practical and cosmetic codes as it likes",
        ),
];

const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();
const APPROVE_CONTRACT: TypedContract<ApproveParams> = TypedContract::new();
const UNAPPROVE_CONTRACT: TypedContract<UnapproveParams> = TypedContract::new();
const RULES_CONTRACT: TypedContract<RulesParams> = TypedContract::new();

/// The most findings `ecad.drc.inspect` lists when no `limit` is given.
const DEFAULT_LIMIT: usize = 50;

pub(super) fn severity_out(s: DocSeverity) -> Severity {
    match s {
        DocSeverity::Error => Severity::Error,
        DocSeverity::Warning => Severity::Warning,
        DocSeverity::Info => Severity::Info,
    }
}

pub(super) fn class_out(c: DocClass) -> Class {
    match c {
        DocClass::Critical => Class::Critical,
        DocClass::Practical => Class::Practical,
        DocClass::Cosmetic => Class::Cosmetic,
    }
}

pub(super) fn actor_out(a: DocActor) -> Actor {
    match a {
        DocActor::User => Actor::User,
        DocActor::Agent => Actor::Agent,
    }
}

pub(super) fn level_out(l: CheckLevel) -> Level {
    match l {
        CheckLevel::Error => Level::Error,
        CheckLevel::Warning => Level::Warning,
        CheckLevel::Info => Level::Info,
        CheckLevel::Ignore => Level::Ignore,
    }
}

pub(super) fn level(l: Level) -> CheckLevel {
    match l {
        Level::Error => CheckLevel::Error,
        Level::Warning => CheckLevel::Warning,
        Level::Info => CheckLevel::Info,
        Level::Ignore => CheckLevel::Ignore,
    }
}

fn item_out(i: &DrcItem) -> DrcItemOut {
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
    DrcItemOut { reference: i.reference(), kind, id: i.id(), pad, end }
}

/// To 0.0001, and no `-0`.
fn round(v: f64) -> f64 {
    (v * 1e4).round() / 1e4 + 0.0
}

/// A finding as replies give it, with its approval's note.
pub(super) fn finding_out(f: &DrcFinding, notes: &BTreeMap<&str, &str>) -> DrcFindingOut {
    DrcFindingOut {
        code: f.code.to_string(),
        class: class_out(f.class),
        severity: severity_out(f.severity),
        key: f.key.clone(),
        approved: f.approved,
        pending_user: f.pending_user,
        by: f.by.map(actor_out),
        note: notes.get(f.key.as_str()).filter(|n| !n.is_empty()).map(|n| n.to_string()),
        message: f.message.clone(),
        message_key: f.message_key.clone(),
        args: f.args.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        at: [round(f.at[0]), round(f.at[1])],
        layer: f.layer.map(|l| l.name()),
        items: f.items.iter().map(item_out).collect(),
        nets: f.nets.clone(),
        measured: MeasureOut { value: round(f.measured), units: f.limit.units.to_string(), text: f.measured_text.map(str::to_string) },
        limit: LimitOut {
            value: round(f.limit.value),
            units: f.limit.units.to_string(),
            text: f.limit.text.map(str::to_string),
            param: f.limit.param.clone(),
            bound: match f.limit.bound {
                DocBound::Min => Bound::Min,
                DocBound::Max => Bound::Max,
            },
        },
        margin: round(f.margin),
        fix: f
            .fix
            .iter()
            .map(|x| DrcFixOut { command: x.command.to_string(), params: (!x.params.is_null()).then(|| x.params.clone()), says: x.says.clone() })
            .collect(),
        section: f.section.to_string(),
        verify: f.note.clone(),
    }
}

pub(super) fn counts(r: &DrcReport) -> DrcCounts {
    let mut by_code = BTreeMap::new();
    let mut by_class = ClassCounts::default();
    for f in r.findings.iter().filter(|f| !f.approved) {
        *by_code.entry(f.code.to_string()).or_insert(0) += 1;
        match f.class {
            DocClass::Critical => by_class.critical += 1,
            DocClass::Practical => by_class.practical += 1,
            DocClass::Cosmetic => by_class.cosmetic += 1,
        }
    }
    DrcCounts {
        errors: r.errors,
        warnings: r.warnings,
        infos: r.infos,
        approved: r.approved,
        pending_user: r.pending_user,
        stale: r.stale.len(),
        at_limit: r.at_limit,
        by_code,
        by_class,
    }
}

pub(super) fn profile_ref(p: &FabProfile) -> ProfileRef {
    ProfileRef {
        id: p.id.to_string(),
        name: p.name.to_string(),
        version: p.version,
        hash: p.hash(),
        ring_convention: p.ring_convention.name().to_string(),
    }
}

/// The check of the stored design (its connections shared with the other commands').
pub(super) fn report(s: &Session) -> DrcReport {
    drc::check(&s.doc.ecad, &s.board_nets())
}

/// Whether `at` is in `r`, checked first to be a box or a circle.
pub(super) fn region_test(r: &RegionParam, cmd: &str) -> Result<Box<dyn Fn([f64; 2]) -> bool>> {
    let finite = |p: [f64; 2]| p.iter().all(|v| v.is_finite() && v.abs() <= solidcraft_doc::ecad::board::MAX_BOARD * 2.0);
    match (r.rect, r.circle) {
        (Some([a, b]), None) if finite(a) && finite(b) => {
            let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
            Ok(Box::new(move |p: [f64; 2]| p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1]))
        }
        (None, Some(c)) if finite(c.at) && c.radius.is_finite() && c.radius >= 0.0 && c.radius <= solidcraft_doc::ecad::board::MAX_BOARD * 4.0 => {
            Ok(Box::new(move |p: [f64; 2]| (p[0] - c.at[0]).hypot(p[1] - c.at[1]) <= c.radius))
        }
        _ => Err(bad(cmd, "`region` is one of {rect: [[x0, y0], [x1, y1]]} or {circle: {at: [x, y], radius}}, within the board's 2000 mm")),
    }
}

/// The codes `list` names, each one the check has.
fn code_set(list: &[String], cmd: &str) -> Result<BTreeSet<String>> {
    if list.len() > RULES.len() * 4 {
        return Err(bad(cmd, format!("`codes` names at most {} codes", RULES.len())));
    }
    for (i, c) in list.iter().enumerate() {
        if drc::find_rule(c).is_none() {
            let mut msg = format!("`codes[{i}]`: there is no code `{c}`");
            if let Some(near) = closest(c, RULES.iter().map(|r| r.code)) {
                msg.push_str(&format!(" (did you mean `{near}`?)"));
            }
            return Err(bad(cmd, msg));
        }
    }
    Ok(list.iter().cloned().collect())
}

fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.drc.inspect";
    let p: InspectParams = parse(CMD, p)?;
    let codes = p.codes.as_deref().map(|c| code_set(c, CMD)).transpose()?;
    if let Some(l) = p.layer
        && !s.doc.ecad.board.has_layer(l.0)
    {
        return Err(bad(CMD, super::ecad_board::no_layer(&s.doc.ecad.board, l.0.name())));
    }
    let region = p.region.as_ref().map(|r| region_test(r, CMD)).transpose()?;
    let limit = p.limit.unwrap_or(DEFAULT_LIMIT);
    if limit > drc::MAX_FINDINGS {
        return Err(bad(CMD, format!("`limit` is at most {}", drc::MAX_FINDINGS)));
    }
    let offset = p.offset.unwrap_or(0);
    let r = report(s);
    let board = &s.doc.ecad.board;
    let notes: BTreeMap<&str, &str> = board.approvals.iter().map(|a| (a.key.as_str(), a.note.as_str())).collect();
    let wanted = |f: &&DrcFinding| {
        codes.as_ref().is_none_or(|c| c.contains(f.code))
            && p.layer.is_none_or(|l| f.layer.is_none_or(|fl| fl == l.0))
            && p.severity.is_none_or(|want| severity_out(f.severity) == want)
            && p.class.is_none_or(|want| class_out(f.class) == want)
            && region.as_ref().is_none_or(|inside| inside(f.at))
    };
    let summary = p.summary.unwrap_or(false);
    let open: Vec<&DrcFinding> = r.findings.iter().filter(wanted).filter(|f| !f.approved).collect();
    let (findings, more) = if summary {
        (Vec::new(), 0)
    } else {
        let listed: Vec<DrcFindingOut> = open.iter().skip(offset).take(limit).map(|f| finding_out(f, &notes)).collect();
        let more = open.len().saturating_sub(offset).saturating_sub(listed.len());
        (listed, more)
    };
    let approved = if summary || !p.include_approved.unwrap_or(true) {
        Vec::new()
    } else {
        r.findings.iter().filter(wanted).filter(|f| f.approved).take(limit).map(|f| finding_out(f, &notes)).collect()
    };
    reply(InspectResult {
        counts: counts(&r),
        profile: r.profile.map(profile_ref),
        findings,
        approved,
        more,
        stale: r.stale.clone(),
        not_applicable: not_applicable(&r),
        truncated: r.truncated,
        param_warnings: Vec::new(),
    })
}

/// The codes the check didn't run, with what each needs.
pub(super) fn not_applicable(r: &DrcReport) -> Vec<NotApplicableOut> {
    r.not_applicable.iter().map(|(code, needs)| NotApplicableOut { code: code.to_string(), needs: needs.to_string() }).collect()
}

/// The approval keys `keys` names, at least one, each once.
pub(super) fn key_list(keys: &[String], cmd: &str) -> Result<Vec<String>> {
    if keys.is_empty() || keys.len() > MAX_APPROVALS {
        return Err(bad(cmd, format!("name 1 to {MAX_APPROVALS} findings in `keys`, by the keys ecad.drc.inspect gives")));
    }
    let mut seen = BTreeSet::new();
    Ok(keys.iter().filter(|k| seen.insert(k.as_str())).cloned().collect())
}

/// The note an approval keeps: trimmed, within its length, and given when an agent approves.
pub(super) fn approval_note(s: &Session, note: Option<String>, cmd: &str) -> Result<String> {
    let note = note.unwrap_or_default().trim().to_string();
    check_text("note", &note, MAX_DESCRIPTION).map_err(|e| bad(cmd, e))?;
    if note.is_empty() && s.actor == DocActor::Agent {
        return Err(bad(cmd, "an agent's approval needs a `note`: why the finding is fine (what was checked, against which source)"));
    }
    Ok(note)
}

/// How a call by `actor` changes the approvals `list` for `keys` (with `note`); the critical
/// ones (`critical`) an agent approves wait for the user. Returns what it added, changed,
/// confirmed, kept and left pending.
pub(super) struct Approved {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub confirmed: Vec<String>,
    pub kept: Vec<String>,
    pub pending_user: Vec<String>,
}

pub(super) fn approve_keys(list: &mut Vec<Approval>, keys: Vec<String>, note: &str, actor: DocActor, critical: &BTreeSet<&str>) -> Approved {
    let mut out = Approved { added: Vec::new(), changed: Vec::new(), confirmed: Vec::new(), kept: Vec::new(), pending_user: Vec::new() };
    for k in keys {
        match list.iter_mut().find(|a| a.key == k) {
            // An agent leaves the user's approvals alone.
            Some(a) if actor == DocActor::Agent && a.by_user() => out.kept.push(k),
            Some(a) => {
                // The user confirms an agent's (or an approval made before who made it was kept).
                let confirm = actor == DocActor::User && !a.by_user();
                let renote = !note.is_empty() && a.note != note;
                if renote {
                    a.note = note.to_string();
                }
                if confirm {
                    a.by = Some(DocActor::User);
                    out.confirmed.push(k);
                } else {
                    // An agent's note on an approval of no known author makes it the agent's.
                    if renote && actor == DocActor::Agent {
                        a.by = Some(DocActor::Agent);
                    }
                    if actor == DocActor::Agent && critical.contains(k.as_str()) {
                        out.pending_user.push(k.clone());
                    }
                    if renote {
                        out.changed.push(k);
                    }
                }
            }
            None => {
                if actor == DocActor::Agent && critical.contains(k.as_str()) {
                    out.pending_user.push(k.clone());
                }
                list.push(Approval { key: k.clone(), note: note.to_string(), by: Some(actor) });
                out.added.push(k);
            }
        }
    }
    out
}

fn approve(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.drc.approve";
    let p: ApproveParams = parse(CMD, p)?;
    let keys = key_list(&p.keys, CMD)?;
    let note = approval_note(s, p.note, CMD)?;
    let r = report(s);
    let found: BTreeSet<&str> = r.findings.iter().map(|f| f.key.as_str()).collect();
    for (i, k) in keys.iter().enumerate() {
        if found.contains(k.as_str()) {
            continue;
        }
        let mut msg = format!("`keys[{i}]`: no finding has the key `{k}`");
        if let Some(near) = closest(k, found.iter().copied()) {
            msg.push_str(&format!(" (did you mean `{near}`?)"));
        } else if found.is_empty() {
            msg.push_str(" (the check finds nothing now)");
        } else {
            msg.push_str(" (ecad.drc.inspect lists the findings and their keys)");
        }
        return Err(bad(CMD, msg));
    }
    let critical: BTreeSet<&str> = r.findings.iter().filter(|f| f.class == DocClass::Critical).map(|f| f.key.as_str()).collect();
    let mut board = s.doc.ecad.board.clone();
    let done = approve_keys(&mut board.approvals, keys, &note, s.actor, &critical);
    if board.approvals.len() > MAX_APPROVALS {
        return Err(bad(CMD, format!("a board keeps at most {MAX_APPROVALS} approvals: drop stale ones with ecad.drc.unapprove {{stale: true}}")));
    }
    store_board(s, board, CMD)?;
    let r = report(s);
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
    const CMD: &str = "ecad.drc.unapprove";
    let p: UnapproveParams = parse(CMD, p)?;
    let all = p.all.unwrap_or(false);
    let stale = p.stale.unwrap_or(false);
    let mut board = s.doc.ecad.board.clone();
    let gone: BTreeSet<String> = match (&p.keys, all, stale) {
        (Some(keys), false, false) => {
            let keys = key_list(keys, CMD)?;
            for (i, k) in keys.iter().enumerate() {
                if !board.approvals.iter().any(|a| a.key == *k) {
                    let mut msg = format!("`keys[{i}]`: `{k}` is not approved");
                    if let Some(near) = closest(k, board.approvals.iter().map(|a| a.key.as_str())) {
                        msg.push_str(&format!(" (did you mean `{near}`?)"));
                    }
                    return Err(bad(CMD, msg));
                }
            }
            keys.into_iter().collect()
        }
        (None, true, false) => board.approvals.iter().map(|a| a.key.clone()).collect(),
        (None, false, true) => {
            let r = report(s);
            // Findings past the budget were not reached, so their approvals look stale.
            if r.truncated {
                return Err(bad(CMD, "the check was cut short by its budget, so it can't tell which approvals are stale: take them back by `keys`"));
            }
            r.stale.into_iter().collect()
        }
        _ => return Err(bad(CMD, "give one of `keys`, `all: true` or `stale: true`")),
    };
    let (removed, kept) = take_back(&mut board.approvals, &gone, s.actor, stale);
    store_board(s, board, CMD)?;
    let r = report(s);
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

/// Take back the approvals in `list` whose keys are `gone`, as `actor` may: an agent leaves the
/// user's as they are (unless they are `stale`, approving nothing now). The keys removed and
/// kept.
pub(super) fn take_back(list: &mut Vec<Approval>, gone: &BTreeSet<String>, actor: DocActor, stale: bool) -> (Vec<String>, Vec<String>) {
    let leaves = |a: &Approval| actor == DocActor::Agent && a.by_user() && !stale;
    let kept: Vec<String> = list.iter().filter(|a| gone.contains(&a.key) && leaves(a)).map(|a| a.key.clone()).collect();
    let removed: Vec<String> = list.iter().filter(|a| gone.contains(&a.key) && !leaves(a)).map(|a| a.key.clone()).collect();
    list.retain(|a| !gone.contains(&a.key) || leaves(a));
    (removed, kept)
}

/// Every code with the board's level for it.
pub(super) fn codes_out(levels: &BTreeMap<String, CheckLevel>) -> Vec<DrcCodeOut> {
    RULES
        .iter()
        .map(|x| DrcCodeOut {
            code: x.code.to_string(),
            class: class_out(x.class),
            severity: severity_out(x.severity),
            level: levels.get(x.code).copied().map(level_out),
            section: x.section.to_string(),
            param: x.param.to_string(),
            about: x.about.to_string(),
        })
        .collect()
}

/// How strict a level is (`None`: the code's own, `own`): ignore below info below warning below
/// error.
pub(super) fn rank(l: Option<CheckLevel>, own: DocSeverity) -> u8 {
    match l {
        Some(CheckLevel::Ignore) => 0,
        Some(CheckLevel::Info) => 1,
        Some(CheckLevel::Warning) => 2,
        Some(CheckLevel::Error) => 3,
        None => match own {
            DocSeverity::Info => 1,
            DocSeverity::Warning => 2,
            DocSeverity::Error => 3,
        },
    }
}

fn rules(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.drc.rules";
    let p: RulesParams = parse(CMD, p)?;
    if p.rules.len() > RULES.len() {
        return Err(bad(CMD, format!("`rules` names at most {} codes", RULES.len())));
    }
    let mut board = s.doc.ecad.board.clone();
    for (code, l) in &p.rules {
        if drc::find_rule(code).is_none() {
            let mut msg = format!("`rules`: there is no code `{code}`");
            if let Some(near) = closest(code, RULES.iter().map(|r| r.code)) {
                msg.push_str(&format!(" (did you mean `{near}`?)"));
            } else {
                msg.push_str(" (call ecad.drc.rules with {rules: {}} to list the codes)");
            }
            return Err(bad(CMD, msg));
        }
        let to = l.map(level);
        if s.actor == DocActor::Agent
            && let Some(rule) = drc::find_rule(code)
            && rule.class == DocClass::Critical
            && rank(to, rule.severity) < rank(board.drc_levels.get(code).copied(), rule.severity)
        {
            return Err(bad(
                CMD,
                format!(
                    "`rules`: `{code}` is critical: only the user lowers or leaves out a critical code's findings; approve them one by one with a note instead (an agent's approval of a critical finding waits for the user's)"
                ),
            ));
        }
        match to {
            Some(l) => board.drc_levels.insert(code.clone(), l),
            None => board.drc_levels.remove(code),
        };
    }
    store_board(s, board, CMD)?;
    let r = report(s);
    let levels = &s.doc.ecad.board.drc_levels;
    reply(RulesResult {
        rules: levels.iter().map(|(code, l)| (code.clone(), level_out(*l))).collect(),
        codes: codes_out(levels),
        counts: counts(&r),
        param_warnings: Vec::new(),
    })
}
