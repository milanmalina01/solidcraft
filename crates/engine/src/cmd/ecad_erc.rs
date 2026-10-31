//! The electrical rules check of the schematic: read its findings, approve those that are fine
//! (with a note), and give a code another level. The check runs afresh on every call (see
//! `solidcraft_doc::ecad::erc`); only the approvals and the rule levels are saved, on the
//! schematic. Approvals keep who made them, as the design rule check's do: an agent's approval
//! of a critical error waits for the user's, an agent leaves the user's approvals as they are,
//! and only the user lowers a critical code. Each change is one undo step, all or none, and a
//! call that changes nothing makes none.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use solidcraft_doc::ecad::erc::{self, Finding, Item, RULES, Report};
use solidcraft_doc::ecad::schematic::MAX_APPROVALS;
use solidcraft_doc::ecad::{Actor as DocActor, Level, Severity};

use super::CommandSpec;
use super::ecad::{closest, electrical_out, parse, reply, store_schematic};
use crate::params::bad;
use crate::schema::ecad_erc::{
    ApprovalResult, ApproveParams, CodeOut, DefaultLevel, ErcCounts, FindingItemOut, FindingOut, FixOut, InspectParams, InspectResult, RuleLevel,
    RulesParams, RulesResult, Severity as SeverityOut, UnapproveParams,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.erc.inspect", "Electrical Check", crate::checked!("ecad.erc.inspect", inspect))
        .icon("interference")
        .noundo()
        .doc_only()
        .contract(&INSPECT_CONTRACT)
        .output(output_schema_for::<InspectResult>)
        .params(
            "severity? error|warning, include_approved? (false) → {findings: [{code, severity, key, approved, note?, net?, at?, items: [{ref, part?, pin?, type?, wire?, label?, marker?}], more?, message, fix?: {id, params}}], counts: {errors, warnings, approved, stale}, stale: [keys], rules, truncated?}: the electrical rules check of the schematic, errors first. Pins on a net whose types clash (pin_conflict, by the pin matrix), pins that touch nothing (pin_unconnected; a no-connect marker exempts one), nets nothing drives (input_not_driven; power_not_driven needs a power output pin, a power flag, or a rail or ground port), shorted rails, isolated wires and labels, then warnings (dangling ends, near misses, pins on a wire or off the 1.27 mm grid, several names, overlapping parts, values, footprints…); ecad.erc.rules lists every code. A key names ids only, so it outlives renumbering. Read-only",
        ),
    CommandSpec::new("ecad.erc.approve", "Approve Finding", crate::checked!("ecad.erc.approve", approve))
        .icon("select")
        .doc_only()
        .contract(&APPROVE_CONTRACT)
        .output(output_schema_for::<ApprovalResult>)
        .params(
            "keys: [key] (as ecad.erc.inspect gives them; each must be a finding's now), note? (why it is fine; an agent must give one) → {added, changed, confirmed, kept, pending_user, counts}: approved findings no longer count as errors or warnings; an approved key takes the new note; the approval keeps who made it (user or agent), and an agent leaves the user's approvals as they are (kept). An agent's approval of a critical error (pins that fight, an input or supply nothing drives, shorted rails) stays pending_user (it still counts) until the user approves it in the app",
        ),
    CommandSpec::new("ecad.erc.unapprove", "Unapprove Finding", crate::checked!("ecad.erc.unapprove", unapprove))
        .icon("restore")
        .doc_only()
        .contract(&UNAPPROVE_CONTRACT)
        .output(output_schema_for::<ApprovalResult>)
        .params("keys? [key] | all? true | stale? true (the approvals no finding has now) → {removed, kept, counts}: an agent takes back only its own approvals (and stale ones); the user's stay (kept)"),
    CommandSpec::new("ecad.erc.rules", "Check Rules", crate::checked!("ecad.erc.rules", rules))
        .icon("settings")
        .doc_only()
        .contract(&RULES_CONTRACT)
        .output(output_schema_for::<RulesResult>)
        .params(
            "rules: {code: error|warning|ignore|null} (null: back to the code's own level; {} changes nothing) → {rules, codes: [{code, default (error|warning|matrix), level?, about}], counts}: ignore leaves a code's findings out. Only the user lowers a critical code (pin_conflict, nc_pin_connected, input_not_driven, power_not_driven, rails_shorted)",
        ),
];

const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();
const APPROVE_CONTRACT: TypedContract<ApproveParams> = TypedContract::new();
const UNAPPROVE_CONTRACT: TypedContract<UnapproveParams> = TypedContract::new();
const RULES_CONTRACT: TypedContract<RulesParams> = TypedContract::new();

fn severity_out(s: Severity) -> SeverityOut {
    match s {
        Severity::Error => SeverityOut::Error,
        // The electrical check gives no infos.
        Severity::Warning | Severity::Info => SeverityOut::Warning,
    }
}

fn level_out(l: Level) -> RuleLevel {
    match l {
        Level::Error => RuleLevel::Error,
        Level::Warning => RuleLevel::Warning,
        Level::Ignore => RuleLevel::Ignore,
    }
}

fn level(l: RuleLevel) -> Level {
    match l {
        RuleLevel::Error => Level::Error,
        RuleLevel::Warning => Level::Warning,
        RuleLevel::Ignore => Level::Ignore,
    }
}

fn item_out(i: &Item) -> FindingItemOut {
    let mut out = FindingItemOut { reference: i.reference(), part: None, pin: None, electrical: None, wire: None, label: None, marker: None };
    match i {
        Item::Pin(p) => {
            out.part = Some(p.part);
            out.pin = Some(p.pin.clone());
            out.electrical = Some(electrical_out(p.electrical));
        }
        Item::Part { id, .. } => out.part = Some(*id),
        Item::Wire(id) => out.wire = Some(*id),
        Item::Label { id, .. } => out.label = Some(*id),
        Item::Marker(id) => out.marker = Some(*id),
    }
    out
}

/// A finding as replies give it, with its approval's note.
pub(super) fn finding_out(f: &Finding, notes: &BTreeMap<&str, &str>) -> FindingOut {
    FindingOut {
        code: f.code.to_string(),
        severity: severity_out(f.severity),
        key: f.key.clone(),
        approved: f.approved,
        pending_user: f.pending_user,
        note: notes.get(f.key.as_str()).filter(|n| !n.is_empty()).map(|n| n.to_string()),
        net: f.net.clone(),
        at: f.at,
        items: f.items.iter().map(item_out).collect(),
        more: f.more,
        message: f.message.clone(),
        fix: f.fix.as_ref().map(|x| FixOut { id: x.command.to_string(), params: x.params.clone() }),
    }
}

fn counts(r: &Report) -> ErcCounts {
    ErcCounts { errors: r.errors, warnings: r.warnings, approved: r.approved, pending_user: r.pending_user, stale: r.stale.len() }
}

/// The check of the stored design.
fn report(s: &Session) -> Report {
    s.doc.ecad.erc()
}

fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.erc.inspect";
    let p: InspectParams = parse(CMD, p)?;
    let r = report(s);
    let sheet = &s.doc.ecad.schematic;
    let notes: BTreeMap<&str, &str> = sheet.approvals.iter().map(|a| (a.key.as_str(), a.note.as_str())).collect();
    let with_approved = p.include_approved.unwrap_or(false);
    let findings = r
        .findings
        .iter()
        .filter(|f| with_approved || !f.approved)
        .filter(|f| p.severity.is_none_or(|want| severity_out(f.severity) == want))
        .map(|f| finding_out(f, &notes))
        .collect();
    reply(InspectResult {
        findings,
        counts: counts(&r),
        stale: r.stale.clone(),
        rules: sheet.rules.iter().map(|(code, l)| (code.clone(), level_out(*l))).collect(),
        truncated: r.truncated,
        param_warnings: Vec::new(),
    })
}

/// The approval keys `keys` names, at least one, each once.
fn key_list(keys: &[String], cmd: &str) -> Result<Vec<String>> {
    if keys.is_empty() || keys.len() > MAX_APPROVALS {
        return Err(bad(cmd, format!("name 1 to {MAX_APPROVALS} findings in `keys`, by the keys ecad.erc.inspect gives")));
    }
    let mut seen = BTreeSet::new();
    Ok(keys.iter().filter(|k| seen.insert(k.as_str())).cloned().collect())
}

fn approve(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.erc.approve";
    let p: ApproveParams = parse(CMD, p)?;
    let keys = key_list(&p.keys, CMD)?;
    let note = super::ecad_drc::approval_note(s, p.note, CMD)?;
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
            msg.push_str(" (ecad.erc.inspect lists the findings and their keys)");
        }
        return Err(bad(CMD, msg));
    }
    let mut sheet = s.doc.ecad.schematic.clone();
    // The critical errors an agent approves wait for the user (see `erc::CRITICAL`).
    let critical: BTreeSet<&str> = r.findings.iter().filter(|f| f.critical).map(|f| f.key.as_str()).collect();
    let done = super::ecad_drc::approve_keys(&mut sheet.approvals, keys, &note, s.actor, &critical);
    if sheet.approvals.len() > MAX_APPROVALS {
        return Err(bad(
            CMD,
            format!("a schematic keeps at most {MAX_APPROVALS} approvals: drop stale ones with ecad.erc.unapprove {{stale: true}}"),
        ));
    }
    store_schematic(s, sheet, CMD)?;
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
    const CMD: &str = "ecad.erc.unapprove";
    let p: UnapproveParams = parse(CMD, p)?;
    let all = p.all.unwrap_or(false);
    let stale = p.stale.unwrap_or(false);
    let mut sheet = s.doc.ecad.schematic.clone();
    let gone: BTreeSet<String> = match (&p.keys, all, stale) {
        (Some(keys), false, false) => {
            let keys = key_list(keys, CMD)?;
            for (i, k) in keys.iter().enumerate() {
                if !sheet.approvals.iter().any(|a| a.key == *k) {
                    let mut msg = format!("`keys[{i}]`: `{k}` is not approved");
                    if let Some(near) = closest(k, sheet.approvals.iter().map(|a| a.key.as_str())) {
                        msg.push_str(&format!(" (did you mean `{near}`?)"));
                    }
                    return Err(bad(CMD, msg));
                }
            }
            keys.into_iter().collect()
        }
        (None, true, false) => sheet.approvals.iter().map(|a| a.key.clone()).collect(),
        (None, false, true) => report(s).stale.into_iter().collect(),
        _ => return Err(bad(CMD, "give one of `keys`, `all: true` or `stale: true`")),
    };
    let (removed, kept) = super::ecad_drc::take_back(&mut sheet.approvals, &gone, s.actor, stale);
    store_schematic(s, sheet, CMD)?;
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

fn rules(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.erc.rules";
    let p: RulesParams = parse(CMD, p)?;
    let mut sheet = s.doc.ecad.schematic.clone();
    for (code, l) in &p.rules {
        if erc::find_rule(code).is_none() {
            let mut msg = format!("`rules`: there is no code `{code}`");
            if let Some(near) = closest(code, RULES.iter().map(|r| r.code)) {
                msg.push_str(&format!(" (did you mean `{near}`?)"));
            } else {
                msg.push_str(" (call ecad.erc.rules with {rules: {}} to list the codes)");
            }
            return Err(bad(CMD, msg));
        }
        // Only the user lowers a critical code (as a critical error's approval is the user's).
        let strict = |l: Option<Level>| match l.or_else(|| erc::find_rule(code).and_then(|r| r.level)) {
            Some(Level::Ignore) => 0,
            Some(Level::Warning) => 1,
            Some(Level::Error) | None => 2,
        };
        let to = l.map(level);
        if s.actor == DocActor::Agent && erc::CRITICAL.contains(&code.as_str()) && strict(to) < strict(sheet.rules.get(code).copied()) {
            return Err(bad(
                CMD,
                format!(
                    "`rules`: `{code}` is critical: only the user lowers or leaves out its findings; approve them one by one with a note instead (an agent's approval of a critical error waits for the user's)"
                ),
            ));
        }
        match to {
            Some(l) => sheet.rules.insert(code.clone(), l),
            None => sheet.rules.remove(code),
        };
    }
    store_schematic(s, sheet, CMD)?;
    let r = report(s);
    let sheet = &s.doc.ecad.schematic;
    let codes = RULES
        .iter()
        .map(|x| CodeOut {
            code: x.code.to_string(),
            default: match x.level {
                Some(Level::Warning) => DefaultLevel::Warning,
                Some(_) => DefaultLevel::Error,
                None => DefaultLevel::Matrix,
            },
            level: sheet.rules.get(x.code).copied().map(level_out),
            about: x.about.to_string(),
        })
        .collect();
    reply(RulesResult {
        rules: sheet.rules.iter().map(|(code, l)| (code.clone(), level_out(*l))).collect(),
        codes,
        counts: counts(&r),
        param_warnings: Vec::new(),
    })
}
