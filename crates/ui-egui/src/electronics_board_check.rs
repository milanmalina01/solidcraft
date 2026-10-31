//! Check Board (the board's RULES tab and the schematic's RULES group): `ecad.board.check` for a
//! stage (plan, place, route, release) and its scorecard. The gates come in their fixed order:
//! completeness first (so taking copper away never makes a board look done), then shorts, what
//! the fab can build, the other critical findings, the design's intent, practical and cosmetic
//! findings; each with its status and its findings by code, the design rule check's and the
//! design-quality checks' alike. A click on a code lists its findings (the DRC or Quality window
//! filtered to it, or Design Intent for a role), a click on a finding shows it on the board (or
//! picks the role in Design Intent). Then the board in numbers, what got worse since the last
//! check, the next things to do (each with a button that does it or opens where it is done), the
//! approvals an agent made that wait for the user's confirmation, and what isn't checked: the
//! quality checks that couldn't run everywhere ask what they need as plain questions, each a click
//! into Design Intent.
//!
//! The check runs when the window opens, when the stage changes and on Check Again, not after
//! every edit; it is the user's check, compared with the user's last, while an agent's checks are
//! compared with the agent's own. The window says when the board has changed since. The quality
//! checks run with it for the app's own words (see `electronics_board_quality`).

use std::collections::BTreeSet;
use std::sync::Arc;

use egui::{Align2, Ui};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::quality::{NotApplicable, QualityReport};

use crate::SolveApp;
use crate::electronics_board_rules::DrcWindow;
use crate::electronics_design::Open;
use crate::electronics_editor::tr;
use crate::theme::Tokens;
use crate::ui_kit;

/// The most findings a check lists over all the gates (each gate gets at most a third).
const LIMIT: usize = 60;

/// How far the board has come, as `ecad.board.check` takes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    Plan,
    Place,
    #[default]
    Route,
    Release,
}

impl Stage {
    const ALL: [Stage; 4] = [Stage::Plan, Stage::Place, Stage::Route, Stage::Release];

    fn name(self) -> &'static str {
        match self {
            Stage::Plan => "plan",
            Stage::Place => "place",
            Stage::Route => "route",
            Stage::Release => "release",
        }
    }
}

/// The Check Board window: the stage it checks, and the row picked (a finding's or role's key).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckWindow {
    pub stage: Stage,
    pub picked: Option<String>,
}

/// A check the window ran: for which document and revision, at which stage, and its reply (or
/// why it was refused).
pub struct Run {
    key: (u64, u64),
    stage: Stage,
    reply: Result<Arc<Value>, String>,
}

fn key(app: &SolveApp) -> (u64, u64) {
    (app.session.uid, app.session.revision)
}

/// Check the board at `stage` and keep the reply: as the user's check, so an agent's next one
/// still names what got worse since its own last. The quality checks run for the same board, so
/// their rows and questions read in the UI's words.
fn run(app: &mut SolveApp, stage: Stage) {
    let reply = app.run_as_user("ecad.board.check", json!({"stage": stage.name(), "limit": LIMIT})).map(Arc::new);
    app.electronics.design.board_check = Some(Run { key: key(app), stage, reply });
    crate::electronics_board_quality::checked_now(app);
}

/// A gate's name in the UI's words.
fn gate_name(gate: &str) -> String {
    tr(&format!("electronics.gate_{gate}"))
}

/// A code's name in the UI's words: a design rule check code, a quality code, a role, or
/// completeness's own.
fn code_name(gate: &str, code: &str) -> String {
    if gate == "intent" {
        return crate::electronics_intent::role_name(code);
    }
    if crate::electronics_board_quality::is_code(code) {
        return crate::electronics_board_quality::code_name(code);
    }
    let key = format!("electronics.drc_code_{code}");
    let text = tr(&key);
    if text != key {
        return text;
    }
    let key = format!("electronics.check_code_{code}");
    let text = tr(&key);
    if text == key { code.replace('_', " ") } else { text }
}

fn status_name(status: &str) -> String {
    let key = format!("electronics.gate_status_{status}");
    let text = tr(&key);
    if text == key { status.to_string() } else { text }
}

/// What the window was asked to do.
enum Act {
    Stage(Stage),
    Again,
    /// A gate's code: list its findings.
    Code(String, String),
    /// A finding or role of a gate: show it.
    Finding(String, String),
    /// The next step with this index.
    Next(usize),
    /// The approvals waiting for the user: the design rule check's, or the quality checks'.
    Pending,
    QualityPending,
    /// Design Intent on a record's key (a quality check's question; empty for its first page).
    Intent(String),
}

/// What Check Board shows of the quality checks run with it: what they couldn't check, and how
/// many of their approvals wait for the user.
struct Quality {
    not_checked: Vec<NotApplicable>,
    pending: usize,
}

fn text_of<'a>(v: &'a Value, field: &str) -> &'a str {
    v.get(field).and_then(Value::as_str).unwrap_or_default()
}

fn count_of(v: &Value, field: &str) -> usize {
    v.get(field).and_then(Value::as_u64).map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX))
}

/// The Check Board window (see the module notes).
pub(crate) fn window(app: &mut SolveApp, ctx: &egui::Context, mut w: CheckWindow) {
    let stale_doc = app.electronics.design.board_check.as_ref().is_none_or(|r| r.stage != w.stage || r.key.0 != app.session.uid);
    if stale_doc {
        run(app, w.stage);
    }
    let Some(last) = app.electronics.design.board_check.as_ref() else { return };
    let fresh = last.key == key(app);
    let reply = last.reply.clone();
    // The quality checks run with this check, for what they couldn't check and their approvals.
    let report: Option<Arc<QualityReport>> = crate::electronics_board_quality::kept_for(app, last.key);
    let quality = Quality {
        not_checked: report.as_ref().map(|r| r.not_applicable.clone()).unwrap_or_default(),
        pending: report.as_ref().map_or(0, |r| r.pending_user),
    };
    // The findings' messages in the UI's words, while the DRC window's check (or the quality
    // checks run with this one) is for this board.
    let messages: Vec<(String, String)> = match &reply {
        Ok(r) => r
            .get("gates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|g| g.get("blocking").and_then(Value::as_array).into_iter().flatten())
            .filter_map(|b| {
                let k = text_of(b, "key");
                let message = if crate::electronics_board_quality::is_key(k) {
                    crate::electronics_board_quality::message_of(app, k)
                } else {
                    crate::electronics_board_rules::message_of(app, k)
                };
                Some((k.to_string(), message?))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    let title = tr("electronics.check_title");
    let mut act: Option<Act> = None;
    let closed = ui_kit::Window::new(egui::Id::new("electronics_board_check"), "interference", &title)
        .placed(Align2::RIGHT_TOP, crate::electronics_design::beside(app, ctx))
        .resizable()
        .show_body(ctx, |ui| {
            ui_kit::wrapped_row(ui, crate::i18n::tr("electronics.field_stage"), |ui| {
                for s in Stage::ALL {
                    let label = tr(&format!("electronics.check_stage_{}", s.name()));
                    if ui_kit::choice_chip(ui, &format!("check:stage:{}", s.name()), &label, w.stage == s).clicked() && w.stage != s {
                        act = Some(Act::Stage(s));
                    }
                }
            });
            ui_kit::text(ui, &tr(&format!("electronics.check_stage_{}_help", w.stage.name())), ui_kit::Text::Note("electronics.check_stage_help"));
            if !fresh {
                ui_kit::text(ui, &tr("electronics.check_changed"), ui_kit::Text::Note("electronics.check_changed"));
            }
            if crate::electronics_editor::button(ui, "check-again", &tr("electronics.check_again"), true) {
                act = Some(Act::Again);
            }
            let r = match &reply {
                Ok(r) => r,
                Err(e) => {
                    ui_kit::text(ui, e, ui_kit::Text::Error("electronics.check_refused"));
                    return;
                }
            };
            body(ui, r, &w, &messages, &quality, &mut act);
        });
    match act {
        Some(Act::Stage(s)) => {
            w.stage = s;
            w.picked = None;
            run(app, s);
        }
        Some(Act::Again) => run(app, w.stage),
        Some(Act::Code(gate, code)) => open_code(app, &gate, &code),
        Some(Act::Finding(gate, k)) => {
            if gate == "intent" {
                crate::electronics_intent::show(app, &k);
                return;
            }
            to_board(app);
            let shown = if crate::electronics_board_quality::is_key(&k) {
                crate::electronics_board_quality::show_key(app, &k)
            } else {
                crate::electronics_board_rules::show_key(app, &k)
            };
            if !shown {
                app.set_status(tr("electronics.check_finding_gone"), false);
            }
            w.picked = Some(k);
        }
        Some(Act::Next(i)) => {
            let step = reply.as_ref().ok().and_then(|r| r.get("next")?.get(i).cloned());
            if let Some(step) = step
                && next_step(app, &step)
            {
                return;
            }
            // A step done here (an accept, a sync) is checked again at once.
            run(app, w.stage);
        }
        Some(Act::Pending) => {
            to_board(app);
            app.electronics.design.open = Some(Open::Drc(DrcWindow { pending: true, ..DrcWindow::default() }));
            return;
        }
        Some(Act::QualityPending) => {
            to_board(app);
            crate::electronics_board_quality::open_pending(app);
            return;
        }
        Some(Act::Intent(k)) => {
            if k.is_empty() {
                app.electronics.design.open = Some(Open::Intent(crate::electronics_intent::IntentWindow::default()));
            } else {
                crate::electronics_intent::show(app, &k);
            }
            return;
        }
        None => {}
    }
    // A step above may have opened another window in its place.
    if matches!(app.electronics.design.open, Some(Open::BoardCheck(_))) || app.electronics.design.open.is_none() {
        app.electronics.design.open = (!closed).then_some(Open::BoardCheck(w));
    }
}

/// The reply: whether the stage is done, the gates, the scorecard, what got worse, the next
/// steps, the approvals waiting, and what the check doesn't look at (the quality checks' needs as
/// questions).
fn body(ui: &mut Ui, r: &Value, w: &CheckWindow, messages: &[(String, String)], quality: &Quality, act: &mut Option<Act>) {
    let t = Tokens::get();
    let stage = tr(&format!("electronics.check_stage_{}", w.stage.name()));
    let gates: Vec<&Value> = r.get("gates").and_then(Value::as_array).map(|g| g.iter().collect()).unwrap_or_default();
    let failing = gates.iter().filter(|g| text_of(g, "status") == "fail").count();
    if r.get("done").and_then(Value::as_bool).unwrap_or(false) {
        let text = crate::i18n::format("electronics.check_done", &[("stage", &stage)]);
        ui_kit::text(ui, &text, ui_kit::Text::Heading("electronics.check_done"));
    } else {
        let key = if failing == 1 { "electronics.check_not_done_one" } else { "electronics.check_not_done" };
        let text = crate::i18n::format(key, &[("stage", &stage), ("n", &failing.to_string())]);
        ui_kit::text(ui, &text, ui_kit::Text::Error("electronics.check_not_done"));
    }
    let profile = r.get("profile").and_then(|p| p.get("id")).and_then(Value::as_str).and_then(solidcraft_engine::doc::ecad::profiles::find);
    let text = match profile {
        Some(p) => crate::i18n::format("electronics.drc_profile", &[("profile", &crate::electronics_board_rules::profile_name(p))]),
        None => tr("electronics.drc_no_profile"),
    };
    ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.drc_profile"));
    if r.get("truncated").and_then(Value::as_bool).unwrap_or(false) {
        ui_kit::text(ui, &tr("electronics.drc_truncated"), ui_kit::Text::Error("electronics.drc_truncated"));
    }
    // The gates, in the order the check gives them (its fixed order).
    for (n, g) in gates.iter().enumerate() {
        let gate = text_of(g, "gate");
        let status = text_of(g, "status");
        let counts = g.get("counts").cloned().unwrap_or(Value::Null);
        let open = count_of(&counts, "open");
        let heading = format!("{}. {} · {}", n + 1, gate_name(gate), status_name(status));
        let unfold = matches!(status, "fail" | "warn");
        ui_kit::section_count(ui, ("electronics_check_gate", gate), &heading, unfold, (open > 0).then_some(open), |ui| {
            gate_body(ui, g, gate, status, w, messages, act);
        });
    }
    scorecard(ui, r);
    // A change of rules or profile counts differently: no "worse", a new baseline.
    if r.get("rules_changed").and_then(Value::as_bool).unwrap_or(false) {
        ui.add_space(t.space_6);
        ui_kit::text(ui, &tr("electronics.check_rules_changed"), ui_kit::Text::Note("electronics.check_rules_changed"));
    }
    let regressions = r.get("regressions").and_then(Value::as_array).cloned().unwrap_or_default();
    if !regressions.is_empty() {
        ui.add_space(t.space_6);
        ui_kit::text(ui, &tr("electronics.check_worse"), ui_kit::Text::Heading("electronics.check_worse"));
        for x in &regressions {
            let what = text_of(x, "what");
            let key = format!("electronics.check_metric_{what}");
            let name = if tr(&key) == key { what.replace('_', " ") } else { tr(&key) };
            let num = |f: &str| x.get(f).and_then(Value::as_f64).map(crate::electronics_editor::mm).unwrap_or_default();
            let text = crate::i18n::format("electronics.check_worse_line", &[("what", &name), ("was", &num("was")), ("now", &num("now"))]);
            ui_kit::text(ui, &text, ui_kit::Text::Error("electronics.check_worse_line"));
        }
    }
    let next = r.get("next").and_then(Value::as_array).cloned().unwrap_or_default();
    if !next.is_empty() {
        ui.add_space(t.space_6);
        ui_kit::text(ui, &tr("electronics.check_next"), ui_kit::Text::Heading("electronics.check_next"));
        for (i, step) in next.iter().enumerate() {
            let (text, button) = step_words(step, &gates);
            ui_kit::text(ui, &format!("{}. {text}", i + 1), ui_kit::Text::Inline);
            if let Some(label) = button
                && crate::electronics_editor::button(ui, &format!("check-next-{i}"), &label, true)
            {
                *act = Some(Act::Next(i));
            }
        }
    }
    let pending: usize = gates.iter().map(|g| g.get("counts").map_or(0, |c| count_of(c, "pending_user"))).sum();
    if pending > 0 {
        ui.add_space(t.space_6);
        let key = if pending == 1 { "electronics.drc_pending_summary_one" } else { "electronics.drc_pending_summary" };
        let text = crate::i18n::format(key, &[("n", &pending.to_string())]);
        ui_kit::text(ui, &text, ui_kit::Text::Error("electronics.drc_pending_summary"));
        // The design rule check's approvals in the DRC window, the quality checks' in theirs.
        if pending > quality.pending && crate::electronics_editor::button(ui, "check-pending", &tr("electronics.check_review_pending"), true) {
            *act = Some(Act::Pending);
        }
        if quality.pending > 0
            && crate::electronics_editor::button(ui, "check-quality-pending", &tr("electronics.check_review_quality_pending"), true)
        {
            *act = Some(Act::QualityPending);
        }
    }
    // The quality checks' own lines are asked as questions below, not listed in the agent's words.
    let unchecked: Vec<&str> = r
        .get("not_verified")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|line| quality.not_checked.is_empty() || !line.split_once(':').is_some_and(|(c, _)| crate::electronics_board_quality::is_code(c)))
        .collect();
    let mut missing: Vec<&NotApplicable> = quality.not_checked.iter().collect();
    missing.sort_by_key(|n| solidcraft_engine::doc::ecad::quality::catalog::codes().position(|c| c.code == n.code));
    if !(unchecked.is_empty() && missing.is_empty()) {
        let heading = tr("electronics.check_not_verified");
        let count = (!missing.is_empty()).then_some(missing.len());
        ui_kit::section_count(ui, "electronics_check_not_verified", &heading, false, count, |ui| {
            if !missing.is_empty() {
                ui_kit::text(ui, &tr("electronics.quality_not_checked_help"), ui_kit::Text::Note("electronics.quality_not_checked_help"));
                if let Some(k) = crate::electronics_board_quality::not_checked(ui, &missing, "check") {
                    *act = Some(Act::Intent(k));
                }
            }
            for line in unchecked {
                ui_kit::text(ui, &format!("• {line}"), ui_kit::Text::Note("electronics.check_not_verified_line"));
            }
        });
    }
}

/// A gate's lines: why it didn't run, its findings by code (each a click to list them), then the
/// findings holding it back (each a click to show it).
fn gate_body(ui: &mut Ui, g: &Value, gate: &str, status: &str, w: &CheckWindow, messages: &[(String, String)], act: &mut Option<Act>) {
    if status == "not_run" {
        let stage = tr(&format!("electronics.check_stage_{}", w.stage.name()));
        let text = crate::i18n::format("electronics.check_not_run", &[("stage", &stage)]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.check_not_run"));
        return;
    }
    let counts = g.get("counts").cloned().unwrap_or(Value::Null);
    let (approved, pending) = (count_of(&counts, "approved"), count_of(&counts, "pending_user"));
    if approved > 0 || pending > 0 {
        let text = crate::i18n::format("electronics.check_gate_approved", &[("approved", &approved.to_string()), ("pending", &pending.to_string())]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.check_gate_approved"));
    }
    let by_code: Vec<(String, usize)> = g
        .get("by_code")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_u64().map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX)))).collect())
        .unwrap_or_default();
    if by_code.is_empty() {
        let key = if gate == "cosmetic" { "electronics.check_no_cosmetic" } else { "electronics.check_gate_clear" };
        ui_kit::text(ui, &tr(key), ui_kit::Text::Note(key));
    }
    let blocking = g.get("blocking").and_then(Value::as_array).cloned().unwrap_or_default();
    // One row per code with its count, and its examples (two at most) under it.
    for (code, n) in &by_code {
        let text = format!("{} · {n}", code_name(gate, code));
        let response = ui_kit::text(ui, &text, ui_kit::Text::NameCell(false));
        crate::scenario::publish_target(&format!("electronics:check:{gate}:{code}"), response.rect);
        if response.clicked() {
            *act = Some(Act::Code(gate.to_string(), code.clone()));
        }
        for b in blocking.iter().filter(|b| text_of(b, "code") == code).take(2) {
            example(ui, b, gate, w, messages, act);
        }
    }
    // Lines of no code listed above (completeness's own).
    for b in blocking.iter().filter(|b| !by_code.iter().any(|(c, _)| c == text_of(b, "code"))) {
        example(ui, b, gate, w, messages, act);
    }
}

/// One example under its code's row: what it is about (a click picks it on the board, or opens
/// its role in Design Intent) and its message; the intent gate's in plain words (what the role
/// needs), not the agent's.
fn example(ui: &mut Ui, b: &Value, gate: &str, w: &CheckWindow, messages: &[(String, String)], act: &mut Option<Act>) {
    let k = text_of(b, "key");
    let code = text_of(b, "code");
    let selected = w.picked.as_deref() == Some(k);
    // A finding's own message is its line (its code is the row above); a role names what it is
    // and, under it, what it needs.
    let (title, message) = if gate == "intent" {
        let subject = k.split_once('|').map_or_else(|| tr("electronics.intent_board"), |(_, n)| n.to_string());
        let needs: Vec<&str> = b.get("needs").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        (format!("{subject} · {}", crate::electronics_intent::role_name(code)), Some(crate::electronics_intent::needs_text(&needs)))
    } else {
        let message = messages.iter().find(|(m, _)| m == k).map_or_else(|| text_of(b, "message").to_string(), |(_, text)| text.clone());
        (message, None)
    };
    let response = ui_kit::text(ui, &format!("  {title}"), ui_kit::Text::NameCell(selected));
    crate::scenario::publish_target(&format!("electronics:check:finding:{k}"), response.rect);
    // Completeness's own lines (no footprint, no outline, not placed) are not findings.
    let shows = gate == "intent" || !matches!(code, "no_footprint" | "no_outline" | "not_placed");
    if response.clicked() && shows {
        *act = Some(Act::Finding(gate.to_string(), k.to_string()));
    }
    if let Some(message) = message {
        ui_kit::text(ui, &message, ui_kit::Text::Note("electronics.check_message"));
    }
}

/// The board in numbers.
fn scorecard(ui: &mut Ui, r: &Value) {
    let Some(s) = r.get("scorecard") else { return };
    let classes = s.get("by_class").cloned().unwrap_or(Value::Null);
    let class = |f: &str| classes.get(f).and_then(Value::as_f64).map(crate::electronics_editor::mm).unwrap_or_default();
    ui.add_space(Tokens::get().space_6);
    ui_kit::text(ui, &tr("electronics.check_scorecard"), ui_kit::Text::Heading("electronics.check_scorecard"));
    let n = |f: &str| {
        s.get(f).map(|v| v.as_f64().map_or_else(|| v.as_str().unwrap_or_default().to_string(), crate::electronics_editor::mm)).unwrap_or_default()
    };
    let lines = [
        ("electronics.check_routed", crate::i18n::format("electronics.check_routed_value", &[("routed", &n("routed"))])),
        ("electronics.check_on_board", n("on_board")),
        (
            "electronics.check_drc",
            crate::i18n::format("electronics.check_drc_value", &[("errors", &n("drc_errors")), ("warnings", &n("drc_warnings"))]),
        ),
        ("electronics.check_intent", crate::i18n::format("electronics.check_intent_value", &[("coverage", &n("intent_coverage"))])),
        (
            "electronics.check_classes",
            crate::i18n::format(
                "electronics.check_classes_value",
                &[("critical", &class("critical")), ("practical", &class("practical")), ("cosmetic", &class("cosmetic"))],
            ),
        ),
        (
            "electronics.check_quality",
            crate::i18n::format("electronics.check_quality_value", &[("critical", &n("quality_critical")), ("n", &n("quality_not_checked"))]),
        ),
        (
            "electronics.check_copper",
            crate::i18n::format(
                "electronics.check_copper_value",
                // The length to a tenth of a millimetre: more reads as noise.
                &[("vias", &n("vias")), ("length", &s.get("length_mm").and_then(Value::as_f64).map(|v| format!("{v:.1}")).unwrap_or_default())],
            ),
        ),
        ("electronics.check_at_limit", crate::i18n::format("electronics.check_at_limit_value", &[("n", &n("at_limit"))])),
    ];
    for (label, value) in lines {
        ui_kit::row(ui, crate::i18n::tr(label), |ui| ui_kit::text(ui, &value, ui_kit::Text::Inline));
    }
}

/// A next step in the UI's words, and its button's label when the window can do it or open where
/// it is done.
fn step_words(step: &Value, gates: &[&Value]) -> (String, Option<String>) {
    let command = text_of(step, "command");
    let params = step.get("params").cloned().unwrap_or(Value::Null);
    let codes: Vec<&str> = params.get("codes").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let open_of = |name: &str| gates.iter().find(|g| text_of(g, "gate") == name).and_then(|g| g.get("counts")).map_or(0, |c| count_of(c, "open"));
    let words = |key: &str, args: &[(&str, &str)]| crate::i18n::format(key, args);
    match command {
        "ecad.drc.inspect" if codes == ["unrouted"] => {
            (words("electronics.check_next_route", &[("n", &open_of("completeness").to_string())]), Some(tr("electronics.check_show")))
        }
        "ecad.drc.inspect" | "ecad.quality.inspect" => {
            // The step's own gate (a code such as fab_class has findings in two gates).
            let named = step.get("gate").and_then(Value::as_str);
            let gate = match named {
                Some(name) => gates.iter().find(|g| text_of(g, "gate") == name),
                None => gates.iter().find(|g| codes.iter().any(|c| g.get("by_code").and_then(|b| b.get(*c)).is_some())),
            };
            let (gate, n) = gate.map_or(("", 0), |g| (text_of(g, "gate"), g.get("counts").map_or(0, |c| count_of(c, "open"))));
            let first = codes.first().map(|c| code_name(gate, c)).unwrap_or_default();
            (
                words("electronics.check_next_fix", &[("n", &n.to_string()), ("gate", &gate_name(gate)), ("code", &first)]),
                Some(tr("electronics.check_show")),
            )
        }
        "ecad.intent.set" => (tr("electronics.check_next_low_risk"), Some(tr("electronics.check_accept"))),
        "ecad.intent.inspect" => {
            (words("electronics.check_next_intent", &[("n", &open_of("intent").to_string())]), Some(tr("electronics.intent_title")))
        }
        "ecad.board.profile" => (tr("electronics.check_next_profile"), Some(tr("electronics.rules_title"))),
        "ecad.board.sync" => (tr("electronics.check_next_sync"), Some(tr("electronics.check_sync"))),
        "ecad.board.outline" => (tr("electronics.check_next_outline"), Some(tr("electronics.check_outline"))),
        "ecad.component.variant.add" => (tr("electronics.check_next_footprints"), None),
        _ => (text_of(step, "do").to_string(), None),
    }
}

/// Show the board (Check Board stays open).
fn to_board(app: &mut SolveApp) {
    if app.electronics.design.view != crate::electronics_design::View::Board {
        drop(app.run("ecad.view.board", json!({})));
    }
}

/// List a gate's findings of `code`: the DRC window filtered to it, Design Intent on the role,
/// or, for completeness's own lines, the tool that deals with them.
fn open_code(app: &mut SolveApp, gate: &str, code: &str) {
    match (gate, code) {
        // The board's facts are the window's first section, not a row.
        ("intent", "board") => crate::electronics_intent::show(app, "board"),
        ("intent", role) => crate::electronics_intent::show_role(app, role),
        ("completeness", "no_footprint") => app.set_status(tr("electronics.check_next_footprints"), false),
        ("completeness", "no_outline") => {
            to_board(app);
            crate::electronics_board::start(app, "ecad.board.outline");
        }
        ("completeness", "not_placed") => {
            to_board(app);
            crate::electronics_board::start(app, "ecad.board.sync");
        }
        (_, code) => {
            to_board(app);
            let codes: BTreeSet<String> = [code.to_string()].into();
            if crate::electronics_board_quality::is_code(code) {
                crate::electronics_board_quality::open_codes(app, codes);
            } else {
                app.electronics.design.open = Some(Open::Drc(DrcWindow { codes, ..DrcWindow::default() }));
            }
        }
    }
}

/// Do a next step, or open where it is done; `true` when another window took Check Board's place.
fn next_step(app: &mut SolveApp, step: &Value) -> bool {
    let params = step.get("params").cloned().unwrap_or(Value::Null);
    match text_of(step, "command") {
        command @ ("ecad.drc.inspect" | "ecad.quality.inspect") => {
            to_board(app);
            let codes: BTreeSet<String> =
                params.get("codes").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            if command == "ecad.quality.inspect" {
                crate::electronics_board_quality::open_codes(app, codes);
            } else {
                app.electronics.design.open = Some(Open::Drc(DrcWindow { codes, ..DrcWindow::default() }));
            }
            true
        }
        "ecad.intent.set" => {
            if let Ok(reply) = app.run_as_user("ecad.intent.set", json!({"accept_low_risk": true})) {
                let n = reply.get("changed").and_then(Value::as_array).map_or(0, Vec::len);
                app.set_status(crate::i18n::format("electronics.intent_accepted", &[("n", &n.to_string())]), false);
            }
            false
        }
        "ecad.intent.inspect" => {
            app.electronics.design.open = Some(Open::Intent(crate::electronics_intent::IntentWindow::default()));
            true
        }
        "ecad.board.profile" => {
            to_board(app);
            app.electronics.design.open = Some(Open::Rules(crate::electronics_board_rules::RulesWindow::default()));
            true
        }
        "ecad.board.sync" => {
            to_board(app);
            crate::electronics_board::start(app, "ecad.board.sync");
            false
        }
        "ecad.board.outline" => {
            to_board(app);
            crate::electronics_board::start(app, "ecad.board.outline");
            true
        }
        _ => false,
    }
}
