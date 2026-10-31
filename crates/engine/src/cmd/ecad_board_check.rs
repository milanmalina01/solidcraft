//! The board's gate and its fab profile. `ecad.board.check` says whether a stage (plan, place,
//! route, release) is done, as a scorecard of gates in a fixed order that can't be gamed:
//! completeness first (so unrouting or deleting never makes a board look cleaner), then shorts,
//! what the fab can build, the other critical findings, the design's intent, and the practical
//! and cosmetic findings; with the next things to do and the guide section to read for each. It
//! reads the design rule check (`ecad_drc.rs`) and the intent's coverage (`ecad_intent.rs`),
//! and remembers its last scorecard in the session, one for the user's checks (the app's Check
//! Board window) and one for an agent's, to name what got worse. Ways round a gate show: the
//! fab profile taken away fails the fab gate, rules finer than the profile count there too, and
//! the codes the board gives another level are listed. `ecad.board.profile`
//! picks the board's fab profile (one undo step) and `ecad.board.profile.inspect` lists the
//! profiles with every figure and its source.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use solidcraft_doc::ecad::Severity as DocSeverity;
use solidcraft_doc::ecad::drc::{self, Class as DocClass, DrcFinding, DrcReport};
use solidcraft_doc::ecad::footprint::PadKind;
use solidcraft_doc::ecad::infer;
use solidcraft_doc::ecad::path;
use solidcraft_doc::ecad::profiles::{self, FabChoice, FabProfile};

use super::CommandSpec;
use super::ecad::{closest, parse, reply};
use super::ecad_board::store_board;
use super::ecad_drc::{actor_out, not_applicable, profile_ref, report};
use super::ecad_quality::{is_quality_code, settings_notes};
use crate::params::bad;
use crate::schema::ecad_board_check::{
    BlockingOut, CheckParams, CheckResult, ChoiceOut, FigureOut, Gate, GateCounts, GateOut, GateStatus, NextOut, ProfileInspectParams,
    ProfileInspectResult, ProfileOut, ProfileParams, ProfileResult, RegressionOut, RuleKeptOut, RuleSetOut, Scorecard, Stage, WaiverOut,
};
use crate::schema::ecad_drc::{ClassCounts, DrcFixOut, Level};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.board.check", "Check Board", crate::checked!("ecad.board.check", check))
        .icon("interference")
        .noundo()
        .doc_only()
        .contract(&CHECK_CONTRACT)
        .output(output_schema_for::<CheckResult>)
        .params(
            "stage? plan|place|route|release (route); limit? (10: the findings listed over all gates) → {stage, done, gates: [{gate: completeness|connectivity|fab|critical|intent|practical|cosmetic, status: pass|warn|fail|waived|not_run, counts: {open, approved?, pending_user?}, by_code?: {code: n} (the intent gate's by role), blocking: [{code (the intent gate's: the role), key, message, at?, margin?, units?, fix?: {command, params?, says}, section?, needs? (the intent gate's)}] (each code's first example before any second, two a code at most, within the gate's share of limit: by_code counts the rest), note?}], scorecard: {routed, on_board, drc_errors, drc_warnings, intent_coverage, by_class: {critical, practical, cosmetic}, vias, length_mm, at_limit, quality_critical, quality_not_checked}, regressions?: [{what, was, now}], next: [{do, command?, params?, read}], waivers?: [{key, by?, note?, pending_user?}], levels?: {code: level}, not_verified, profile?, engine}: whether the stage is done, in a fixed order: completeness (footprints, placement, routing) comes first, so removing copper never makes a board look done; then shorts, the fab's figures (the profile, which every stage needs, rules finer than it, sizes, gaps, rings, holes), the other critical findings (the design rule check's and the design-quality checks': ecad.quality.inspect), the design intent's blocking roles, practical and cosmetic findings of both checks; not_verified names the quality codes that couldn't run and the first thing each needs (ecad.quality.inspect {status: not_applicable} lists all); a blocking message is cut at 150 characters (the inspect commands give it whole). At release every warning of the connectivity, fab and critical gates counts as a failure. regressions name what got worse since the caller's last check in this session (the user's and an agent's are kept apart); rules_changed? true: the rules, fab profile, levels or quality parameters changed since then, so nothing is named and this check is the new baseline. truncated? and cut? [code]: a check ran out of its work budget, so the critical gate is a warning at least (never done at release) and names the codes cut short. read and section name a section of the solidcraft-pcb guide (routing-dfm#vias: reference/routing-dfm.md, Vias; MCP skill_reference serves it). Read-only",
        ),
    CommandSpec::new("ecad.board.profile", "Fab Profile", crate::checked!("ecad.board.profile", profile))
        .icon("settings")
        .doc_only()
        .contract(&PROFILE_CONTRACT)
        .output(output_schema_for::<ProfileResult>)
        .params(
            "fab: standard|fine|hdi|null (null: none, the check then says so); apply? (true: set the rules' least track, ring, drill, hole gap, edge and silkscreen gaps and micro via from it where they still have the old profile's figure (or the default, with none), keep those set to anything else, give the micro via rules back their defaults when the new profile makes no micro vias and they came from the old one, and raise the default class's gap, track and via where they fall below); reset? (true: set every one of those rules to the profile's figure, the user's too) → {profile?: {id, name, version, hash, ring_convention}, rules_set: [{field, from, to, source}], kept?: [{field, value}] (rules left as the user set them)}: the fab profile the board is checked against; one undo step. Each rule can still be set to another value (ecad.board.rules): the check's fab_class then says what the profile can't build",
        ),
    CommandSpec::new("ecad.board.profile.inspect", "Fab Profiles", crate::checked!("ecad.board.profile.inspect", profile_inspect))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&PROFILE_INSPECT_CONTRACT)
        .output(output_schema_for::<ProfileInspectResult>)
        .params(
            "fab? (one profile) → {current?: {id, version}, profiles: [{id, name, version, read, about, ring_convention, hash, values: [{key, value, units, source, verify?}]}]}: the fab profiles, each figure with where it comes from; verify marks figures from public summaries of IPC standards, which only ever give warnings. Read-only",
        ),
];

const CHECK_CONTRACT: TypedContract<CheckParams> = TypedContract::new();
const PROFILE_CONTRACT: TypedContract<ProfileParams> = TypedContract::new();
const PROFILE_INSPECT_CONTRACT: TypedContract<ProfileInspectParams> = TypedContract::new();

/// The most findings `ecad.board.check` lists by default, and when asked.
const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 200;
/// The most waivers it lists.
const MAX_WAIVERS: usize = 3;
/// The most characters of a blocking entry's message and its fix's words (ecad.drc.inspect and
/// ecad.quality.inspect give them whole).
const MAX_MESSAGE: usize = 150;
const MAX_SAYS: usize = 100;
/// The checks' own version, bumped when what they find changes.
const CHECKS_VERSION: u32 = 1;

/// The design rule check's codes each gate takes (the rest by class).
const CONNECTIVITY: &[&str] = &["short"];
const FAB: &[&str] = &[
    "clearance",
    "track_width",
    "annular_ring",
    "drill",
    "via_aspect_ratio",
    "drill_to_copper",
    "via_in_pad",
    "hole_to_hole",
    "copper_to_edge",
    "hole_outside",
    "profile_missing",
];
const PLACEMENT: &[&str] = &["footprint_outside", "courtyard_overlap", "keepout"];

fn round(v: f64) -> f64 {
    (v * 1e4).round() / 1e4 + 0.0
}

/// To 0.01 mm: where a blocking finding is, enough to find it.
fn round2(v: f64) -> f64 {
    (v * 1e2).round() / 1e2 + 0.0
}

/// Whether a finding is what the fab can't build: one of [`FAB`]'s codes, or the rules asking
/// finer than the profile (or micro vias it makes none of), which `fab_class` rates critical.
fn fab(f: &DrcFinding) -> bool {
    FAB.contains(&f.code) || (f.code == "fab_class" && f.class == DocClass::Critical)
}

/// The gate a finding counts in.
fn gate_of(f: &DrcFinding) -> Gate {
    if f.code == "unrouted" {
        Gate::Completeness
    } else if CONNECTIVITY.contains(&f.code) {
        Gate::Connectivity
    } else if fab(f) {
        Gate::Fab
    } else {
        match f.class {
            DocClass::Critical => Gate::Critical,
            DocClass::Practical => Gate::Practical,
            DocClass::Cosmetic => Gate::Cosmetic,
        }
    }
}

/// Whether a gate runs at `stage`, and which of the fab gate's codes it takes before routing.
fn runs(gate: Gate, stage: Stage) -> bool {
    match stage {
        Stage::Plan => matches!(gate, Gate::Completeness | Gate::Fab | Gate::Intent),
        Stage::Place => !matches!(gate, Gate::Connectivity),
        Stage::Route | Stage::Release => true,
    }
}

/// Whether a finding counts at `stage`: before routing, only placement, the profile, what the
/// rules ask of it and the design-quality findings (placement's among them); at the plan stage
/// only the profile.
fn counts_at(f: &DrcFinding, stage: Stage) -> bool {
    let profile = f.code == "profile_missing" || (f.code == "fab_class" && f.class == DocClass::Critical);
    match stage {
        Stage::Plan => profile,
        Stage::Place => PLACEMENT.contains(&f.code) || profile || f.code == "fab_class" || f.code == "silk_over_pad" || is_quality_code(f.code),
        Stage::Route | Stage::Release => true,
    }
}

/// A blocking entry for a finding, with its first fix and its guide section.
fn blocking(f: &DrcFinding) -> BlockingOut {
    BlockingOut {
        code: f.code.to_string(),
        key: f.key.clone(),
        message: clipped(&f.message, MAX_MESSAGE),
        at: Some([round2(f.at[0]), round2(f.at[1])]),
        margin: Some((f.margin * 1e3).round() / 1e3 + 0.0),
        units: Some(f.limit.units.to_string()),
        fix: f.fix.first().map(|x| DrcFixOut {
            command: x.command.to_string(),
            params: (!x.params.is_null()).then(|| x.params.clone()),
            says: clipped(&x.says, MAX_SAYS),
        }),
        section: Some(f.section.to_string()),
        needs: Vec::new(),
    }
}

/// `text` cut to at most `n` characters, with `…` where it was cut.
fn clipped(text: &str, n: usize) -> String {
    match text.char_indices().nth(n) {
        Some((at, _)) => format!("{}…", text.get(..at).unwrap_or(text).trim_end()),
        None => text.to_string(),
    }
}

/// A blocking entry that is no finding (a completeness line, a missing role).
fn line(code: &str, key: String, message: String) -> BlockingOut {
    BlockingOut { code: code.into(), key, message, at: None, margin: None, units: None, fix: None, section: None, needs: Vec::new() }
}

/// The status of a gate from its open findings' severities (at release a warning fails; no fab
/// profile fails at every stage, as every stage's checks lean on it), and whether it had
/// approved ones.
fn status(open: &[&DrcFinding], approved: usize, release: bool) -> GateStatus {
    let fails = |f: &DrcFinding| f.severity == DocSeverity::Error || (release && f.severity == DocSeverity::Warning) || f.code == "profile_missing";
    if open.iter().any(|f| fails(f)) {
        GateStatus::Fail
    } else if open.iter().any(|f| f.severity == DocSeverity::Warning) {
        GateStatus::Warn
    } else if approved > 0 && open.iter().all(|f| f.severity == DocSeverity::Info) {
        GateStatus::Waived
    } else {
        GateStatus::Pass
    }
}

/// Connections the nets need (each net's copper pads less one), and those still to route.
fn connections(s: &Session) -> (usize, usize) {
    let nets = s.board_nets();
    let mut pads: BTreeMap<usize, usize> = BTreeMap::new();
    for p in nets.pads.iter().filter(|p| p.pad.kind != PadKind::Hole) {
        if let Some(n) = p.net {
            *pads.entry(n).or_default() += 1;
        }
    }
    let total: usize = pads.values().map(|n| n.saturating_sub(1)).sum();
    (total, nets.airwires.len().min(total))
}

fn check(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.check";
    let p: CheckParams = parse(CMD, p)?;
    let stage = p.stage.unwrap_or(Stage::Route);
    let limit = p.limit.unwrap_or(DEFAULT_LIMIT);
    if limit > MAX_LIMIT {
        return Err(bad(CMD, format!("`limit` is at most {MAX_LIMIT}")));
    }
    let release = stage == Stage::Release;
    let r: DrcReport = report(s);
    let q = super::ecad_quality::report(s, None);
    // The design rule check's findings and the design-quality checks', which share their shape.
    let all: Vec<&DrcFinding> = r.findings.iter().chain(q.findings.iter().map(|x| &x.finding)).collect();
    let ecad = &s.doc.ecad;
    let board = &ecad.board;
    let inference = infer::infer(ecad);
    let cov = infer::coverage(ecad, &ecad.intent, &inference);
    // Completeness: footprints for the parts, the parts on the board, the connections routed.
    let mut with_footprint = 0usize;
    let mut no_footprint: Vec<String> = Vec::new();
    for part in &ecad.schematic.parts {
        let has =
            ecad.component(part.component).is_some_and(|c| ecad.resolve(c, part.variant.as_deref()).variant.and_then(|v| v.footprint).is_some());
        if has {
            with_footprint += 1;
        } else {
            no_footprint.push(part.designator.clone());
        }
    }
    let placed = board.footprints.iter().filter(|f| ecad.placed(f).is_some()).count();
    let (total, open_wires) = connections(s);
    // The listed findings are shared out so the later gates get some too: each at most a third.
    let mut left = limit;
    let per = limit.div_ceil(3).max(1);
    let mut take = |list: Vec<BlockingOut>| -> Vec<BlockingOut> {
        let n = list.len().min(left).min(per);
        left -= n;
        list.into_iter().take(n).collect()
    };
    let mut gates: Vec<GateOut> = Vec::new();
    // Completeness.
    {
        let mut items: Vec<BlockingOut> = Vec::new();
        let mut open = 0usize;
        let mut by_code: BTreeMap<String, usize> = BTreeMap::new();
        if !no_footprint.is_empty() {
            open += no_footprint.len();
            by_code.insert("no_footprint".into(), no_footprint.len());
            let names = no_footprint.iter().take(8).cloned().collect::<Vec<_>>();
            items.push(line(
                "no_footprint",
                format!("parts|{}", names.join(",")),
                format!("{} parts have no footprint: {}", no_footprint.len(), names.join(", ")),
            ));
        }
        if stage != Stage::Plan {
            if board.outline.is_empty() {
                open += 1;
                by_code.insert("no_outline".into(), 1);
                items.push(line("no_outline", "board".into(), "the board has no outline".into()));
            }
            let missing = with_footprint.saturating_sub(placed);
            if missing > 0 {
                open += missing;
                by_code.insert("not_placed".into(), missing);
                let mut l = line("not_placed", "board".into(), format!("{missing} of {with_footprint} footprints are not on the board"));
                (l.margin, l.units) = (Some(-(missing as f64)), Some("footprints".into()));
                items.push(l);
            }
        }
        // A connection the user approved leaving open (a jumper, a part not fitted) is waived;
        // an agent's approval of one waits for the user, and still counts.
        let unrouted: Vec<&DrcFinding> = r.findings.iter().filter(|f| f.code == "unrouted").collect();
        let waived = unrouted.iter().filter(|f| f.approved).count();
        let pending_user = unrouted.iter().filter(|f| f.pending_user).count();
        let still_open = open_wires.saturating_sub(waived);
        if matches!(stage, Stage::Route | Stage::Release) && still_open > 0 {
            open += still_open;
            by_code.insert("unrouted".into(), still_open);
            items.extend(unrouted.iter().filter(|f| !f.approved).take(limit).map(|f| blocking(f)));
        }
        let status = match (open, waived) {
            (0, 0) => GateStatus::Pass,
            (0, _) => GateStatus::Waived,
            _ => GateStatus::Fail,
        };
        gates.push(GateOut {
            gate: Gate::Completeness,
            status,
            counts: GateCounts { open, approved: waived, pending_user },
            by_code,
            blocking: take(items),
            note: None,
        });
    }
    // The design rule check's gates.
    for gate in [Gate::Connectivity, Gate::Fab, Gate::Critical] {
        gates.push(drc_gate(&all, gate, stage, release, &mut take));
    }
    // Intent.
    {
        let items: Vec<BlockingOut> = cov
            .missing
            .iter()
            .map(|m| BlockingOut {
                needs: m.needs.iter().map(|n| n.to_string()).collect(),
                ..line(m.role, m.key.clone(), format!("{} ({}) needs {}: {}", m.key, m.role, m.needs.join(", "), m.why))
            })
            .collect();
        let open = cov.missing.len();
        let mut by_code: BTreeMap<String, usize> = BTreeMap::new();
        for m in &cov.missing {
            *by_code.entry(m.role.to_string()).or_default() += 1;
        }
        let status = if open > 0 { GateStatus::Fail } else { GateStatus::Pass };
        gates.push(GateOut {
            gate: Gate::Intent,
            status,
            counts: GateCounts { open, ..GateCounts::default() },
            by_code,
            blocking: spread(items, &mut take),
            note: None,
        });
    }
    for gate in [Gate::Practical, Gate::Cosmetic] {
        let mut g = drc_gate(&all, gate, stage, release, &mut take);
        if gate == Gate::Cosmetic && g.status == GateStatus::Pass && g.counts.open == 0 {
            g.note = Some("no cosmetic check is built yet".into());
        }
        gates.push(g);
    }
    // The codes the board gives another level, on the gates they count in; quality parameters an
    // agent set on critical codes, on the critical gate.
    let (quality_levels, agent_params) = settings_notes(board);
    let levels: BTreeMap<String, Level> = board.drc_levels.iter().map(|(code, l)| (code.clone(), level_out(*l))).collect();
    for g in gates.iter_mut() {
        let here: Vec<String> = board
            .drc_levels
            .iter()
            .filter_map(|(code, l)| {
                let rule = drc::find_rule(code)?;
                let gate = match code.as_str() {
                    "unrouted" => Gate::Completeness,
                    c if CONNECTIVITY.contains(&c) => Gate::Connectivity,
                    c if FAB.contains(&c) || c == "fab_class" => Gate::Fab,
                    _ => match rule.class {
                        DocClass::Critical => Gate::Critical,
                        DocClass::Practical => Gate::Practical,
                        DocClass::Cosmetic => Gate::Cosmetic,
                    },
                };
                (gate == g.gate).then(|| format!("{code} ({})", level_name(*l)))
            })
            .collect();
        let here: Vec<String> =
            here.into_iter().chain(quality_levels.iter().filter(|(c, _)| class_gate(*c) == g.gate).map(|(_, x)| x.clone())).collect();
        let mut notes: Vec<String> = Vec::new();
        if !here.is_empty() {
            notes.push(format!("the board gives these codes another level: {}", here.join(", ")));
        }
        if g.gate == Gate::Critical && !agent_params.is_empty() {
            notes.push(format!("an agent set these critical checks' parameters: {}", agent_params.join("; ")));
        }
        if !notes.is_empty() {
            g.note = Some(notes.join("; "));
        }
    }
    // A check cut short by its budget hasn't looked everywhere: the critical gate says so and
    // can't pass on it.
    if r.truncated || q.truncated {
        let cut: Vec<&str> = q.cut.iter().copied().take(12).collect();
        let what = match (r.truncated, cut.is_empty()) {
            (true, true) => "the design rule check ran out of its work budget".to_string(),
            (true, false) => format!("the design rule check and these quality checks ran out of their work budget: {}", cut.join(", ")),
            (false, true) => "the quality checks were cut short (more findings than a report lists)".to_string(),
            (false, false) => format!("these quality checks ran out of their work budget: {}", cut.join(", ")),
        };
        for g in gates.iter_mut().filter(|g| g.gate == Gate::Critical) {
            if matches!(g.status, GateStatus::Pass | GateStatus::Waived) {
                g.status = GateStatus::Warn;
            }
            g.note = Some(match g.note.take() {
                Some(n) => format!("{what}: not every finding is in; {n}"),
                None => format!("{what}: not every finding is in"),
            });
        }
    }
    for g in gates.iter_mut().filter(|g| !runs(g.gate, stage)) {
        g.status = GateStatus::NotRun;
        g.blocking.clear();
        g.by_code.clear();
        g.note = Some("not part of this stage".into());
    }
    let done = !(release && (r.truncated || q.truncated))
        && gates.iter().all(|g| match g.status {
            GateStatus::Fail => false,
            GateStatus::Warn => !(release && matches!(g.gate, Gate::Connectivity | Gate::Fab | Gate::Critical)),
            _ => true,
        });
    // The scorecard.
    let open = |c: DocClass| all.iter().filter(|f| !f.approved && f.class == c).count();
    let length: f64 = board.tracks.iter().map(|t| path::length(&t.points, &t.curves, false)).sum();
    let scorecard = Scorecard {
        routed: format!("{}/{}", total - open_wires, total),
        on_board: format!("{placed}/{with_footprint}"),
        drc_errors: r.errors,
        drc_warnings: r.warnings,
        intent_coverage: format!("{}/{}", cov.blocking_confirmed, cov.blocking_total),
        by_class: ClassCounts { critical: open(DocClass::Critical), practical: open(DocClass::Practical), cosmetic: open(DocClass::Cosmetic) },
        vias: board.vias.len(),
        length_mm: round(length),
        at_limit: r.at_limit,
        quality_critical: q
            .findings
            .iter()
            .filter(|x| !x.finding.approved && x.finding.class == DocClass::Critical && x.finding.severity != DocSeverity::Info)
            .count(),
        quality_not_checked: q.not_applicable.len(),
    };
    // What got worse since the last check of this document.
    let mut now: BTreeMap<&'static str, f64> = BTreeMap::new();
    now.insert("routed", (total - open_wires) as f64);
    now.insert("on_board", placed as f64);
    now.insert("drc_errors", r.errors as f64);
    now.insert("critical", scorecard.by_class.critical as f64);
    now.insert("intent_confirmed", cov.blocking_confirmed as f64);
    now.insert("pours", board.pours.len() as f64);
    now.insert("rules", rules_print(board));
    let mut regressions = Vec::new();
    // Each caller's last check: the user's in the app don't hide what an agent made worse. A
    // change of rules, profile or levels is no regression: the check counts differently, so it
    // starts again from here.
    let last = s.last_board_check.get(&s.actor).filter(|(uid, _)| *uid == s.uid);
    let rules_changed = last.is_some_and(|(_, was)| was.get("rules") != now.get("rules"));
    if let Some((_, was)) = last.filter(|_| !rules_changed) {
        for (what, worse_up) in
            [("routed", false), ("on_board", false), ("intent_confirmed", false), ("pours", false), ("drc_errors", true), ("critical", true)]
        {
            let (Some(a), Some(b)) = (was.get(what), now.get(what)) else { continue };
            if (worse_up && b > a) || (!worse_up && b < a) {
                regressions.push(RegressionOut { what: what.to_string(), was: *a, now: *b });
            }
        }
    }
    let next = next_steps(&gates, &all, &cov, &inference.low_risk(&ecad.intent), stage);
    let approvals: BTreeMap<&str, &DrcFinding> = all.iter().filter(|f| f.approved || f.pending_user).map(|f| (f.key.as_str(), *f)).collect();
    let waivers: Vec<WaiverOut> = board
        .approvals
        .iter()
        .chain(&board.quality_approvals)
        .filter(|a| approvals.contains_key(a.key.as_str()))
        .take(MAX_WAIVERS)
        .map(|a| WaiverOut {
            key: a.key.clone(),
            by: a.by.map(actor_out),
            note: clipped(&a.note, MAX_SAYS),
            pending_user: approvals.get(a.key.as_str()).is_some_and(|f| f.pending_user),
        })
        .collect();
    let waivers_more = approvals.len().saturating_sub(waivers.len());
    let mut not_verified: Vec<String> = not_applicable(&r)
        .into_iter()
        .map(|x| match x.needs.as_str() {
            "board.drc_levels" => format!("{}: left out by the board's levels", x.code),
            needs => format!("{}: not checked, it needs {needs}", x.code),
        })
        .collect();
    not_verified.extend(q.not_applicable.iter().map(|n| match n.needs.first().map(String::as_str) {
        Some("board.quality_levels") => format!("{}: left out by the board's levels", n.code),
        // The first need names what to settle; ecad.quality.inspect {status: not_applicable}
        // lists them all.
        _ => format!(
            "{} ({} place{} not checked): needs {}{}",
            n.code,
            n.count,
            if n.count == 1 { "" } else { "s" },
            n.needs.first().map_or(String::new(), |x| clipped(x, 60)),
            if n.needs.len() > 1 { "; …" } else { "" }
        ),
    }));
    let not_built = solidcraft_doc::ecad::quality::catalog::not_built();
    if !not_built.is_empty() {
        not_verified.push(format!("these design-quality checks are not built yet: {}", not_built.join(", ")));
    }
    not_verified.push(
        "not modelled or checked yet: solder mask and paste openings, fabrication outputs (Gerber, drill, BOM, placement), an assembly profile; nor circuit function, safety and compliance"
            .to_string(),
    );
    let result = CheckResult {
        stage,
        done,
        gates,
        scorecard,
        regressions,
        rules_changed,
        next,
        waivers,
        waivers_more,
        levels,
        not_verified,
        profile: r.profile.map(profile_ref),
        engine: format!(
            "solidcraft-engine {}, checks {CHECKS_VERSION}, quality checks {}",
            env!("CARGO_PKG_VERSION"),
            super::ecad_quality::QUALITY_VERSION
        ),
        truncated: r.truncated || q.truncated,
        cut: q.cut.iter().map(|c| c.to_string()).collect(),
        param_warnings: Vec::new(),
    };
    s.last_board_check.insert(s.actor, (s.uid, now));
    reply(result)
}

/// A print of what the check counts by: the rules, the fab profile, the checks' levels and the
/// quality checks' parameters (a hash, as a number the last check's map holds).
fn rules_print(board: &solidcraft_doc::ecad::Board) -> f64 {
    let text = serde_json::to_string(&(&board.rules, &board.fab_profile, &board.drc_levels, &board.quality_levels, &board.quality_params))
        .unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    // 52 bits fit in an f64 exactly.
    (h >> 12) as f64
}

/// A gate's examples, `take` sharing them out: each code's first before any code's second, at
/// most two a code; past the gate's share, the other codes are left to its `by_code` counts.
fn spread(list: Vec<BlockingOut>, take: &mut impl FnMut(Vec<BlockingOut>) -> Vec<BlockingOut>) -> Vec<BlockingOut> {
    let mut by_code: Vec<(String, Vec<BlockingOut>)> = Vec::new();
    for b in list {
        match by_code.iter_mut().find(|(c, _)| *c == b.code) {
            Some((_, l)) if l.len() < 2 => l.push(b),
            Some(_) => {}
            None => by_code.push((b.code.clone(), vec![b])),
        }
    }
    let mut firsts: Vec<BlockingOut> = Vec::new();
    let mut seconds: Vec<BlockingOut> = Vec::new();
    for (_, l) in by_code {
        let mut l = l.into_iter();
        firsts.extend(l.next());
        seconds.extend(l);
    }
    take(firsts.into_iter().chain(seconds).collect())
}

/// A gate made of design rule check and design-quality findings.
fn drc_gate(all: &[&DrcFinding], gate: Gate, stage: Stage, release: bool, take: &mut impl FnMut(Vec<BlockingOut>) -> Vec<BlockingOut>) -> GateOut {
    // At release the warnings of the gates before intent fail too; practical and cosmetic ones
    // stay warnings.
    let release = release && matches!(gate, Gate::Connectivity | Gate::Fab | Gate::Critical);
    let mine: Vec<&DrcFinding> = all.iter().copied().filter(|f| gate_of(f) == gate && counts_at(f, stage)).collect();
    let open: Vec<&DrcFinding> = mine.iter().copied().filter(|f| !f.approved).collect();
    let approved = mine.iter().filter(|f| f.approved).count();
    let pending_user = mine.iter().filter(|f| f.pending_user).count();
    let status = status(&open, approved, release);
    // Errors first, then warnings; infos aren't blocking.
    let mut listed: Vec<&DrcFinding> = open.iter().copied().filter(|f| f.severity != DocSeverity::Info).collect();
    listed.sort_by_key(|f| f.severity);
    let shown = spread(listed.into_iter().map(blocking).collect(), take);
    let mut by_code: BTreeMap<String, usize> = BTreeMap::new();
    for f in &open {
        *by_code.entry(f.code.to_string()).or_default() += 1;
    }
    GateOut { gate, status, counts: GateCounts { open: open.len(), approved, pending_user }, by_code, blocking: shown, note: None }
}

/// The next things to do: one for each gate that doesn't pass, in the gates' order, at most 4.
/// `low` are the low-risk proposals still to accept.
fn next_steps(gates: &[GateOut], all: &[&DrcFinding], cov: &infer::Coverage, low: &[&infer::Proposal], stage: Stage) -> Vec<NextOut> {
    let mut out = Vec::new();
    let open = |gate: Gate| -> Vec<&DrcFinding> {
        all.iter().copied().filter(|f| !f.approved && gate_of(f) == gate && counts_at(f, stage) && f.severity != DocSeverity::Info).collect()
    };
    for g in gates.iter().filter(|g| matches!(g.status, GateStatus::Fail | GateStatus::Warn)) {
        let step = match g.gate {
            Gate::Completeness => match g.blocking.first().map(|b| b.code.as_str()) {
                Some("no_footprint") => NextOut {
                    what: "give the parts without a footprint one (a variant with a footprint)".into(),
                    command: Some("ecad.component.variant.add".into()),
                    params: None,
                    read: "workflow#stages".into(),
                    gate: None,
                },
                Some("no_outline") => NextOut {
                    what: "draw the board's outline".into(),
                    command: Some("ecad.board.outline".into()),
                    params: None,
                    read: "placement#edges".into(),
                    gate: None,
                },
                Some("not_placed") => NextOut {
                    what: "put the missing footprints on the board".into(),
                    command: Some("ecad.board.sync".into()),
                    params: Some(json!({})),
                    read: "placement#floorplan".into(),
                    gate: None,
                },
                _ => NextOut {
                    what: format!("route the {} open connections", g.counts.open),
                    command: Some("ecad.drc.inspect".into()),
                    params: Some(json!({"codes": ["unrouted"], "limit": DEFAULT_LIMIT})),
                    read: "workflow#done".into(),
                    gate: None,
                },
            },
            Gate::Intent => {
                if !low.is_empty() {
                    out.push(NextOut {
                        what: format!("accept the {} low-risk proposals in one go", low.len()),
                        command: Some("ecad.intent.set".into()),
                        params: Some(json!({"accept_low_risk": true, "note": "low-risk roles as proposed"})),
                        read: "workflow#intent".into(),
                        gate: Some(Gate::Intent),
                    });
                }
                NextOut {
                    what: format!(
                        "settle the {} blocking roles: confirm proposals after a look at their evidence; ask the user what only the product tells (a supply's most current, cables, radio bands) and give it with a note saying so",
                        cov.missing.len()
                    ),
                    command: Some("ecad.intent.inspect".into()),
                    params: Some(json!({"blocking": true})),
                    read: "workflow#intent".into(),
                    gate: None,
                }
            }
            gate => {
                let open = open(gate);
                let Some(f) = open.first() else { continue };
                // The profile first: it sets how rings are measured, the least drill, and the
                // checks that run.
                if let Some(missing) = open.iter().find(|x| x.code == "profile_missing") {
                    NextOut {
                        what: "pick the fab profile first: it sets how rings are measured, the least drill and gaps, and the aspect-ratio and inner-layer hole checks".into(),
                        command: Some("ecad.board.profile".into()),
                        params: Some(json!({"fab": profiles::DEFAULT_PROFILE})),
                        read: missing.section.into(),
                        gate: None,
                    }
                } else {
                    // The design rule check's findings first: they are the cheaper to fix.
                    let drc_codes: BTreeSet<&str> = open.iter().map(|x| x.code).filter(|c| !is_quality_code(c)).collect();
                    let (command, codes): (&str, BTreeSet<&str>) = if drc_codes.is_empty() {
                        ("ecad.quality.inspect", open.iter().map(|x| x.code).collect())
                    } else {
                        ("ecad.drc.inspect", drc_codes)
                    };
                    let read = open.iter().find(|x| codes.contains(x.code)).map_or(f.section, |x| x.section);
                    let codes: Vec<&str> = codes.into_iter().collect();
                    NextOut {
                        what: format!("fix the {} {} findings (the codes in params): {command} gives each one's fix", g.counts.open, gate_name(gate)),
                        command: Some(command.into()),
                        params: Some(json!({"codes": codes, "limit": DEFAULT_LIMIT})),
                        read: read.into(),
                        gate: None,
                    }
                }
            }
        };
        out.push(NextOut { gate: Some(g.gate), ..step });
        if out.len() >= 4 {
            break;
        }
    }
    out
}

/// A level as replies give it.
fn level_out(l: drc::CheckLevel) -> Level {
    match l {
        drc::CheckLevel::Error => Level::Error,
        drc::CheckLevel::Warning => Level::Warning,
        drc::CheckLevel::Info => Level::Info,
        drc::CheckLevel::Ignore => Level::Ignore,
    }
}

fn level_name(l: drc::CheckLevel) -> &'static str {
    match l {
        drc::CheckLevel::Error => "error",
        drc::CheckLevel::Warning => "warning",
        drc::CheckLevel::Info => "info",
        drc::CheckLevel::Ignore => "ignore",
    }
}

/// The gate a class's findings count in when nothing else places them.
fn class_gate(c: DocClass) -> Gate {
    match c {
        DocClass::Critical => Gate::Critical,
        DocClass::Practical => Gate::Practical,
        DocClass::Cosmetic => Gate::Cosmetic,
    }
}

fn gate_name(g: Gate) -> &'static str {
    match g {
        Gate::Completeness => "completeness",
        Gate::Connectivity => "connectivity",
        Gate::Fab => "fab",
        Gate::Critical => "critical",
        Gate::Intent => "intent",
        Gate::Practical => "practical",
        Gate::Cosmetic => "cosmetic",
    }
}

/// The profile with the id `id`, or an error naming those there are.
fn find_profile(id: &str, cmd: &str) -> Result<&'static FabProfile> {
    profiles::find(id).ok_or_else(|| {
        let ids: Vec<&str> = profiles::PROFILES.iter().map(|p| p.id).collect();
        let mut msg = format!("there is no fab profile `{id}`: there are {}", ids.join(", "));
        if let Some(near) = closest(id, ids.iter().copied()) {
            msg.push_str(&format!(" (did you mean `{near}`?)"));
        }
        bad(cmd, msg)
    })
}

fn profile(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.profile";
    let p: ProfileParams = parse(CMD, p)?;
    let Some(fab) = p.fab else { return Err(bad(CMD, "give `fab`: a profile's id, or null for none")) };
    let mut board = s.doc.ecad.board.clone();
    let mut rules_set = Vec::new();
    let mut kept = Vec::new();
    let chosen = match fab {
        Some(id) => {
            let prof = find_profile(&id, CMD)?;
            let old = board.fab_profile.as_ref().and_then(|c| c.profile());
            board.fab_profile = Some(FabChoice::of(prof));
            if p.reset.unwrap_or(false) || p.apply.unwrap_or(true) {
                let mut rules = board.rules.clone();
                if p.reset.unwrap_or(false) {
                    rules_set = prof.reset(&mut rules);
                } else {
                    let done = prof.switch(old, &mut rules);
                    rules_set = done.set;
                    kept = done.kept;
                }
                rules.validate().map_err(|e| bad(CMD, e))?;
                board.rules = rules;
            }
            Some(prof)
        }
        None => {
            board.fab_profile = None;
            None
        }
    };
    store_board(s, board, CMD)?;
    reply(ProfileResult {
        profile: chosen.map(profile_ref),
        rules_set: rules_set
            .into_iter()
            .map(|c| RuleSetOut { field: c.field.into(), from: round(c.from), to: round(c.to), source: c.source.into() })
            .collect(),
        kept: kept.into_iter().map(|(field, value)| RuleKeptOut { field: field.into(), value: round(value) }).collect(),
        param_warnings: Vec::new(),
    })
}

fn profile_out(p: &FabProfile) -> ProfileOut {
    ProfileOut {
        id: p.id.into(),
        name: p.name.into(),
        version: p.version,
        read: p.read.into(),
        about: p.about.into(),
        ring_convention: p.ring_convention.name().into(),
        hash: p.hash(),
        values: p
            .figures()
            .into_iter()
            .map(|(key, units, f)| FigureOut { key: key.into(), value: f.value, units: units.into(), source: f.source.into(), verify: f.verify })
            .collect(),
    }
}

fn profile_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.profile.inspect";
    let p: ProfileInspectParams = parse(CMD, p)?;
    let profiles: Vec<ProfileOut> = match &p.fab {
        Some(id) => vec![profile_out(find_profile(id, CMD)?)],
        None => profiles::PROFILES.iter().map(profile_out).collect(),
    };
    let current = s.doc.ecad.board.fab_profile.as_ref().map(|c| ChoiceOut { id: c.id.clone(), version: c.version });
    reply(ProfileInspectResult { current, profiles, param_warnings: Vec::new() })
}
