//! The board's design-quality checks (see `electronics_board`), on its RULES tab next to DRC and
//! from Check Board's rows: what the design rule check can't tell from geometry alone, because it
//! needs to know what the board is for (return paths of fast nets, decoupling, regulators' loops,
//! current, ESD paths, crystals, RF lines, pairs, silkscreen; `ecad.quality.inspect`). The window
//! is laid out as the DRC window: the counts, filters by family, class, severity, status and
//! layer, then the findings by code (errors first) and the approved ones. A click on one frames it
//! on the board and picks what it is about, with what was measured against which limit, how it
//! was judged (the net's speed and where that came from, the plane, the frequency), the principle
//! in one sentence, its fixes (a button runs a fix's command, one undo step; a fix that needs a
//! choice says what to do) and its approval with a note (`ecad.quality.approve`, as the user's):
//! an agent's approval of a critical finding waits here for the user's. What the checks couldn't
//! look at for want of the design's intent is asked in plain words, each question a click into
//! Design Intent. While the window is open the board shows a mark at each finding.
//!
//! The checks take a while on a large board, so they run when the window opens, on Check Again
//! and after the window's own changes, not after every edit; the window says when the board has
//! changed since. Check Board shares the run: its quality rows read in the UI's words, and its
//! Not Checked section asks the same questions.
//!
//! Design Rules holds a folded Quality Checks section: each code's level and its parameters (the
//! first in reach, the rest folded), with the default and where it comes from; a change is one
//! `ecad.quality.rules`, kept with the reason typed above them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use egui::{Align2, FontId, Stroke, Ui, vec2};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::drc::{CheckLevel, Class, DrcFinding};
use solidcraft_engine::doc::ecad::quality::catalog::{self, Code, Param};
use solidcraft_engine::doc::ecad::quality::{NotApplicable, ParamSetting, QualityFinding, QualityReport, Status};
use solidcraft_engine::doc::ecad::{Actor, CopperLayer, Severity};

use crate::SolveApp;
use crate::electronics_board::{Layer, board};
use crate::electronics_board_route::layer_name;
use crate::electronics_design::Open;
use crate::electronics_editor::{mm, tr};
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;
use crate::ui_kit;

/// The most findings the window lists under each heading, and marks on the board.
const MAX_ROWS: usize = 300;
const MAX_MARKS: usize = 2000;
/// The most fixes a finding shows.
const MAX_FIXES: usize = 3;

/// The checks as they ran last, for the document and revision they ran on, and the reason Design
/// Rules keeps with a parameter changed there.
#[derive(Default)]
pub struct QualityState {
    report: Option<((u64, u64), Arc<QualityReport>)>,
    note: String,
}

/// Which findings the window shows by status: those to fix, those to look at, or only the codes
/// that couldn't check everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusFilter {
    Fail,
    Review,
    NotChecked,
}

/// The Quality window: the finding picked (by key), the note to approve it with, and the filters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QualityWindow {
    pub picked: Option<String>,
    pub note: String,
    /// Only findings of this family (`pi`, `si`…).
    pub family: Option<&'static str>,
    pub class: Option<Class>,
    pub severity: Option<Severity>,
    pub status: Option<StatusFilter>,
    /// Only findings on this layer (and those on none).
    pub layer: Option<CopperLayer>,
    /// Only findings of these codes (Check Board's lines); empty for all.
    pub codes: BTreeSet<String>,
    /// Only the approvals an agent made that wait for the user's confirmation.
    pub pending: bool,
}

fn key(app: &SolveApp) -> (u64, u64) {
    (app.session.uid, app.session.revision)
}

/// Run the checks on the board as it is, and keep what they find: the session's run, which
/// Check Board's `ecad.board.check` shares, so opening it checks the board once.
fn run(app: &mut SolveApp) -> Arc<QualityReport> {
    let r = app.session.quality_report();
    app.electronics.board.quality.report = Some((key(app), r.clone()));
    r
}

/// The checks for the board as it is: the kept ones while nothing changed, else run now.
fn report_now(app: &mut SolveApp) -> Arc<QualityReport> {
    match &app.electronics.board.quality.report {
        Some((k, r)) if *k == key(app) => r.clone(),
        _ => run(app),
    }
}

/// Whether the kept checks are for the board as it is.
fn current(app: &SolveApp) -> bool {
    app.electronics.board.quality.report.as_ref().is_some_and(|(k, _)| *k == key(app))
}

/// The checks kept for the board as it was at `at` (a document and revision), if they ran then.
pub(crate) fn kept_for(app: &SolveApp, at: (u64, u64)) -> Option<Arc<QualityReport>> {
    app.electronics.board.quality.report.as_ref().filter(|(k, _)| *k == at).map(|(_, r)| r.clone())
}

/// Run the checks for the board as it is now unless they have: after Check Board's run, so its
/// quality rows read in the UI's words and its questions come from the same board.
pub(crate) fn checked_now(app: &mut SolveApp) {
    if board(app).is_empty() {
        return;
    }
    drop(report_now(app));
}

/// Whether `code` is one of the quality checks' (`pi.return_path`), not the design rule check's.
pub(crate) fn is_code(code: &str) -> bool {
    code.contains('.') && catalog::find(code).is_some()
}

/// Whether `key` is a quality finding's (`pi.return_path|t5|z9`).
pub(crate) fn is_key(key: &str) -> bool {
    is_code(key.split_once('|').map_or(key, |(code, _)| code))
}

/// A code's name in the UI's words.
pub(crate) fn code_name(code: &str) -> String {
    let key = format!("electronics.quality_code_{}", code.replace('.', "_"));
    let text = tr(&key);
    if text == key { code.to_string() } else { text }
}

fn family_name(family: &str) -> String {
    let key = format!("electronics.quality_family_{family}");
    let text = tr(&key);
    if text == key { family.to_string() } else { text }
}

fn class_name(c: Class) -> String {
    tr(&format!("electronics.gate_{}", c.name()))
}

/// Pick and frame on the board the finding with the key `key` (Check Board's rows); `false` when
/// the checks find none such now.
pub(crate) fn show_key(app: &mut SolveApp, key: &str) -> bool {
    let r = report_now(app);
    let Some(q) = r.findings.iter().find(|q| q.finding.key == key) else { return false };
    crate::electronics_board_rules::show_finding(app, &q.finding);
    true
}

/// A finding's message in the UI's words, when the kept checks (for the board as it is) have it.
pub(crate) fn message_of(app: &SolveApp, key: &str) -> Option<String> {
    let (at, r) = app.electronics.board.quality.report.as_ref()?;
    if *at != self::key(app) {
        return None;
    }
    r.findings.iter().find(|q| q.finding.key == key).map(|q| crate::electronics_board_rules::message(&q.finding))
}

/// The board Inspector's line for the checks once they have run: errors, warnings and approved.
pub(crate) fn summary(app: &SolveApp) -> Option<String> {
    let (at, r) = app.electronics.board.quality.report.as_ref()?;
    let counts = [("errors", r.errors.to_string()), ("warnings", r.warnings.to_string()), ("approved", r.approved.to_string())];
    let args: Vec<(&str, &str)> = counts.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let key = if *at == key(app) { "electronics.quality_summary" } else { "electronics.quality_summary_stale" };
    Some(crate::i18n::format(key, &args))
}

/// Quality Checks on the toolbar: open its window (checking afresh when the board changed since
/// the last run), or close it.
pub(crate) fn toggle(app: &mut SolveApp) {
    if !current(app) {
        app.electronics.board.quality.report = None;
    }
    crate::electronics_design::toggle(app, Open::Quality(QualityWindow::default()));
}

/// The window on the findings of `codes` (Check Board's rows and next steps).
pub(crate) fn open_codes(app: &mut SolveApp, codes: BTreeSet<String>) {
    app.electronics.design.open = Some(Open::Quality(QualityWindow { codes, ..QualityWindow::default() }));
}

/// The window on the approvals an agent made that wait for the user (Check Board).
pub(crate) fn open_pending(app: &mut SolveApp) {
    app.electronics.design.open = Some(Open::Quality(QualityWindow { pending: true, ..QualityWindow::default() }));
}

/// Whether the window's filters keep `q`.
fn kept(w: &QualityWindow, q: &QualityFinding) -> bool {
    let f = &q.finding;
    let status = match w.status {
        None => true,
        Some(StatusFilter::Fail) => q.status == Status::Fail,
        Some(StatusFilter::Review) => q.status == Status::Review,
        Some(StatusFilter::NotChecked) => false,
    };
    status
        && w.family.is_none_or(|x| x == q.family)
        && w.class.is_none_or(|c| c == f.class)
        && w.severity.is_none_or(|s| s == f.severity)
        && w.layer.is_none_or(|l| f.layer.is_none_or(|fl| fl == l))
        && (w.codes.is_empty() || w.codes.contains(f.code))
        && (!w.pending || f.pending_user)
}

/// Whether the window's filters keep what `code` couldn't check (no severity, layer or approval
/// filter is on).
fn kept_code(w: &QualityWindow, code: &str) -> bool {
    let Some(c) = catalog::find(code) else { return false };
    matches!(w.status, None | Some(StatusFilter::NotChecked))
        && !w.pending
        && w.severity.is_none()
        && w.family.is_none_or(|f| f == c.family())
        && w.class.is_none_or(|x| x == c.class)
        && (w.codes.is_empty() || w.codes.contains(code))
}

/// What the window was asked to do.
enum Act {
    Pick(String),
    Approve(String, String),
    Unapprove(String),
    ClearStale,
    /// Show every finding again (the code and waiting filters off).
    ShowAll,
    Again,
    /// The fix with this index of the finding with this key.
    Fix(String, usize),
    /// Design Intent, on a record's key (`net|VBUS`; empty for its first page).
    Intent(String),
}

/// An approval's note and who made it, by key.
type Approvals = BTreeMap<String, (String, Option<Actor>)>;

/// The Quality window: the counts, the filters, the findings by code (errors first), what the
/// checks couldn't look at, and the approved ones; the picked one with its details.
pub(crate) fn window(app: &mut SolveApp, ctx: &egui::Context, mut w: QualityWindow) {
    // Another document's checks, or none yet, run now; this board's, older, wait for Check Again.
    let r = match &app.electronics.board.quality.report {
        Some((k, r)) if k.0 == app.session.uid => r.clone(),
        _ => run(app),
    };
    let fresh = current(app);
    let notes: Approvals = board(app).quality_approvals.iter().map(|a| (a.key.clone(), (a.note.clone(), a.by))).collect();
    let stack = board(app).stack();
    let title = tr("electronics.quality_title");
    let mut act: Option<Act> = None;
    let closed = ui_kit::Window::new(egui::Id::new("electronics_board_quality"), "interference", &title)
        .placed(Align2::RIGHT_TOP, crate::electronics_design::beside(app, ctx))
        .resizable()
        .show_body(ctx, |ui| {
            let counts = crate::i18n::format(
                "electronics.quality_counts",
                &[
                    ("errors", &r.errors.to_string()),
                    ("warnings", &r.warnings.to_string()),
                    ("infos", &r.infos.to_string()),
                    ("approved", &r.approved.to_string()),
                ],
            );
            ui_kit::text(ui, &counts, ui_kit::Text::Note("electronics.quality_counts"));
            if r.pending_user > 0 {
                let key = if r.pending_user == 1 { "electronics.drc_pending_summary_one" } else { "electronics.drc_pending_summary" };
                let text = crate::i18n::format(key, &[("n", &r.pending_user.to_string())]);
                ui_kit::text(ui, &text, ui_kit::Text::Error("electronics.drc_pending_summary"));
            }
            if !fresh {
                ui_kit::text(ui, &tr("electronics.check_changed"), ui_kit::Text::Note("electronics.check_changed"));
            }
            if crate::electronics_editor::button(ui, "quality-again", &tr("electronics.check_again"), true) {
                act = Some(Act::Again);
            }
            if r.truncated {
                ui_kit::text(ui, &tr("electronics.drc_truncated"), ui_kit::Text::Error("electronics.drc_truncated"));
            }
            filters(ui, &mut w, &stack, r.pending_user);
            // Check Board's filters, said in words with a way back to every finding.
            if !w.codes.is_empty() || w.pending {
                let mut what: Vec<String> = w.codes.iter().map(|c| code_name(c)).collect();
                if w.pending {
                    what.push(tr("electronics.drc_waiting"));
                }
                let text = crate::i18n::format("electronics.drc_only", &[("what", &what.join(", "))]);
                ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.drc_only"));
                if crate::electronics_editor::button(ui, "quality-show-all", &tr("electronics.drc_show_all"), true) {
                    act = Some(Act::ShowAll);
                }
            }
            let mut shown = false;
            for code in catalog::codes().map(|c| c.code) {
                let rows: Vec<&QualityFinding> = r.findings.iter().filter(|q| q.finding.code == code && !q.finding.approved && kept(&w, q)).collect();
                if rows.is_empty() {
                    continue;
                }
                shown = true;
                // Open when it holds errors, the picked finding, or is what Check Board asked for.
                let open = rows.iter().any(|q| q.finding.severity == Severity::Error)
                    || w.codes.contains(code)
                    || w.pending
                    || rows.iter().any(|q| w.picked.as_deref() == Some(q.finding.key.as_str()));
                ui_kit::section_count(ui, ("electronics_quality_code", code), &code_name(code), open, Some(rows.len()), |ui| {
                    finding_rows(ui, &rows, w.picked.as_deref(), &mut w.note, &notes, &mut act);
                });
            }
            let mut missing: Vec<&NotApplicable> = r.not_applicable.iter().filter(|n| kept_code(&w, n.code)).collect();
            missing.sort_by_key(|n| catalog::codes().position(|c| c.code == n.code));
            if !missing.is_empty() {
                shown = true;
                let open = w.status == Some(StatusFilter::NotChecked);
                let heading = tr("electronics.check_not_verified");
                ui_kit::section_count(ui, "electronics_quality_not_checked", &heading, open, Some(missing.len()), |ui| {
                    ui_kit::text(ui, &tr("electronics.quality_not_checked_help"), ui_kit::Text::Note("electronics.quality_not_checked_help"));
                    if let Some(k) = not_checked(ui, &missing, "quality") {
                        act = Some(Act::Intent(k));
                    }
                });
            }
            let approved: Vec<&QualityFinding> = r.findings.iter().filter(|q| q.finding.approved && kept(&w, q)).collect();
            if !approved.is_empty() {
                shown = true;
                let heading = tr("electronics.checks_approved");
                ui_kit::section_count(ui, "electronics_quality_approved", &heading, false, Some(approved.len()), |ui| {
                    finding_rows(ui, &approved, w.picked.as_deref(), &mut w.note, &notes, &mut act);
                });
            }
            if !shown {
                let key =
                    if r.findings.is_empty() && r.not_applicable.is_empty() { "electronics.quality_none" } else { "electronics.quality_none_shown" };
                ui_kit::text(ui, &tr(key), ui_kit::Text::Note(key));
            }
            // A run cut short names none stale (findings it did not reach may hold them).
            if !r.truncated && !r.stale.is_empty() {
                ui.add_space(Tokens::get().space_8);
                let text = crate::i18n::format("electronics.checks_stale", &[("n", &r.stale.len().to_string())]);
                ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.checks_stale"));
                if crate::electronics_editor::button(ui, "quality-clear-stale", &tr("electronics.checks_clear_stale"), true) {
                    act = Some(Act::ClearStale);
                }
            }
        });
    match act {
        Some(Act::Pick(key)) => {
            if let Some(q) = r.findings.iter().find(|q| q.finding.key == key) {
                crate::electronics_board_rules::show_finding(app, &q.finding);
            }
            // The note being typed belongs to the finding picked before.
            if w.picked.as_deref() != Some(key.as_str()) {
                w.note.clear();
            }
            w.picked = Some(key);
        }
        Some(Act::Approve(key, text)) => {
            if app.run_as_user("ecad.quality.approve", json!({"keys": [key], "note": text})).is_ok() {
                w.note.clear();
                app.set_status(tr("electronics.drc_approved_now"), false);
                drop(run(app));
            }
        }
        // The user's own approvals are the user's to take back.
        Some(Act::Unapprove(key)) => {
            if app.run_as_user("ecad.quality.unapprove", json!({"keys": [key]})).is_ok() {
                app.set_status(tr("electronics.drc_unapproved_now"), false);
                drop(run(app));
            }
        }
        Some(Act::ClearStale) => {
            if app.run_as_user("ecad.quality.unapprove", json!({"stale": true})).is_ok() {
                app.set_status(tr("electronics.drc_stale_cleared"), false);
                drop(run(app));
            }
        }
        Some(Act::ShowAll) => {
            w.codes.clear();
            w.pending = false;
        }
        Some(Act::Again) => drop(run(app)),
        Some(Act::Fix(key, i)) => {
            let fix = r.findings.iter().find(|q| q.finding.key == key).and_then(|q| q.finding.fix.get(i)).cloned();
            if let Some(fix) = fix
                && app.run(fix.command, fix.params.clone()).is_ok()
            {
                app.set_status(crate::i18n::format("electronics.quality_fix_done", &[("fix", &fix.says)]), false);
                drop(run(app));
            }
        }
        // Design Intent takes the window's place.
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
    app.electronics.design.open = (!closed).then_some(Open::Quality(w));
}

/// The family, class, severity (with Waiting for You while an agent's approvals wait) and status
/// chips, and the layer dropdown.
fn filters(ui: &mut Ui, w: &mut QualityWindow, stack: &[CopperLayer], pending: usize) {
    let all = tr("electronics.drc_all");
    ui_kit::wrapped_row(ui, crate::i18n::tr("electronics.quality_field_family"), |ui| {
        if ui_kit::choice_chip(ui, "quality:family:all", &all, w.family.is_none()).clicked() {
            w.family = None;
        }
        for f in catalog::families() {
            if ui_kit::choice_chip(ui, &format!("quality:family:{f}"), &family_name(f), w.family == Some(f)).clicked() {
                w.family = Some(f);
            }
        }
    });
    ui_kit::wrapped_row(ui, crate::i18n::tr("electronics.quality_field_class"), |ui| {
        for c in [None, Some(Class::Critical), Some(Class::Practical), Some(Class::Cosmetic)] {
            let (key, name) = c.map_or(("all".to_string(), all.clone()), |c| (c.name().to_string(), class_name(c)));
            if ui_kit::choice_chip(ui, &format!("quality:class:{key}"), &name, w.class == c).clicked() {
                w.class = c;
            }
        }
    });
    ui_kit::wrapped_row(ui, crate::i18n::tr("electronics.field_show"), |ui| {
        for (s, key, name) in [
            (None, "all", "electronics.drc_all"),
            (Some(Severity::Error), "errors", "electronics.checks_errors"),
            (Some(Severity::Warning), "warnings", "electronics.checks_warnings"),
            (Some(Severity::Info), "infos", "electronics.quality_notes"),
        ] {
            if ui_kit::choice_chip(ui, &format!("quality:severity:{key}"), crate::i18n::tr(name), w.severity == s && !w.pending).clicked() {
                w.severity = s;
                w.pending = false;
            }
        }
        if (pending > 0 || w.pending)
            && ui_kit::choice_chip(ui, "quality:severity:waiting", crate::i18n::tr("electronics.drc_waiting"), w.pending).clicked()
        {
            w.pending = !w.pending;
        }
    });
    ui_kit::wrapped_row(ui, crate::i18n::tr("electronics.quality_field_status"), |ui| {
        for (s, key, name) in [
            (None, "all", "electronics.drc_all"),
            (Some(StatusFilter::Fail), "fail", "electronics.quality_status_fail"),
            (Some(StatusFilter::Review), "review", "electronics.quality_status_review"),
            (Some(StatusFilter::NotChecked), "not_applicable", "electronics.quality_status_not_checked"),
        ] {
            if ui_kit::choice_chip(ui, &format!("quality:status:{key}"), crate::i18n::tr(name), w.status == s).clicked() {
                w.status = s;
            }
        }
    });
    let all_layers = tr("electronics.drc_all_layers");
    let shown = w.layer.map_or_else(|| all_layers.clone(), layer_name);
    ui_kit::row(ui, crate::i18n::tr("electronics.field_layer"), |ui| {
        ui_kit::dropdown_menu(ui, "electronics_quality_layer", &shown, |ui| {
            if ui_kit::list_picker(ui, "quality_layer_all", &all_layers, w.layer.is_none()).clicked() {
                w.layer = None;
            }
            for l in stack {
                if ui_kit::list_picker(ui, &format!("quality_layer_{}", l.name()), &layer_name(*l), w.layer == Some(*l)).clicked() {
                    w.layer = Some(*l);
                }
            }
        });
    });
}

/// A finding's row title: what it is about (`U1.A22, C112.1`), or its code's name.
fn title(f: &DrcFinding) -> String {
    let about: Vec<String> = f.items.iter().take(2).map(crate::electronics_board_rules::item_name).collect();
    if about.is_empty() { code_name(f.code) } else { about.join(", ") }
}

/// What a fix's button does: run its command (a board edit with its parameters), open Design
/// Intent (on the record it is about), or nothing (a fix that needs a choice, or one that only
/// shows numbers: its words say what to do).
enum FixButton {
    Run,
    Intent(String),
}

fn fix_button(f: &DrcFinding, fix: &solidcraft_engine::doc::ecad::drc::Fix) -> Option<FixButton> {
    if fix.command.starts_with("ecad.intent.") {
        let named = |field: &str| fix.params.get(field).and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str).map(str::to_string);
        let record = named("parts").map(|p| format!("part|{p}")).or_else(|| named("nets").map(|n| format!("net|{n}")));
        let record = record.or_else(|| f.nets.first().map(|n| format!("net|{n}"))).unwrap_or_default();
        return Some(FixButton::Intent(record));
    }
    (!fix.params.is_null() && !fix.command.ends_with(".inspect")).then_some(FixButton::Run)
}

/// The rows of `rows` (at most [`MAX_ROWS`]): each finding's title and message, whether it asks
/// for a look rather than a fix, and who approved it; the `picked` one with what it measured, how
/// it was judged, why it matters, its fixes and its approval (`note` and Approve; Unapprove; or,
/// for an agent's approval waiting for the user, Confirm).
fn finding_rows(ui: &mut Ui, rows: &[&QualityFinding], picked: Option<&str>, note: &mut String, notes: &Approvals, act: &mut Option<Act>) {
    for q in rows.iter().take(MAX_ROWS) {
        let f = &q.finding;
        let selected = picked == Some(f.key.as_str());
        let response = ui_kit::text(ui, &title(f), ui_kit::Text::NameCell(selected));
        // Only the part of a row the list shows: one scrolled out of sight has no handle.
        let shown = response.rect.intersect(ui.clip_rect());
        if shown.is_positive() {
            crate::scenario::publish_target(&format!("electronics:quality:{}", f.key), shown);
        }
        if response.clicked() {
            *act = Some(Act::Pick(f.key.clone()));
        }
        let style = if f.severity == Severity::Error && !f.approved {
            ui_kit::Text::Error("electronics.quality_message")
        } else {
            ui_kit::Text::Note("electronics.quality_message")
        };
        ui_kit::text(ui, &crate::electronics_board_rules::message(f), style);
        if q.status == Status::Review {
            ui_kit::text(ui, &tr("electronics.quality_review"), ui_kit::Text::Note("electronics.quality_review"));
        }
        // Who approved it shows on the row; one waiting for the user stands out.
        if f.approved || f.pending_user {
            let (n, by) = notes.get(&f.key).cloned().unwrap_or((String::new(), f.by));
            let text = crate::electronics_board_rules::approval_words(f.pending_user, by, &n);
            let style =
                if f.pending_user { ui_kit::Text::Error("electronics.quality_approval") } else { ui_kit::Text::Note("electronics.quality_approval") };
            ui_kit::text(ui, &text, style);
        }
        if !selected {
            continue;
        }
        details(ui, q, act);
        if f.pending_user {
            // The user's approval confirms the agent's, keeping its note unless one is typed.
            let (enter, _) = ui_kit::text_row(ui, &tr("electronics.checks_note"), note, &tr("electronics.drc_confirm_hint"));
            if crate::electronics_editor::button(ui, "quality-confirm", &tr("electronics.drc_confirm"), true) || enter {
                *act = Some(Act::Approve(f.key.clone(), note.trim().to_string()));
            }
            if crate::electronics_editor::button(ui, "quality-unapprove", &tr("electronics.checks_unapprove"), true) {
                *act = Some(Act::Unapprove(f.key.clone()));
            }
        } else if f.approved {
            if crate::electronics_editor::button(ui, "quality-unapprove", &tr("electronics.checks_unapprove"), true) {
                *act = Some(Act::Unapprove(f.key.clone()));
            }
        } else {
            let (enter, _) = ui_kit::text_row(ui, &tr("electronics.checks_note"), note, &tr("electronics.checks_note_hint"));
            if crate::electronics_editor::button(ui, "quality-approve", &tr("electronics.checks_approve"), true) || enter {
                *act = Some(Act::Approve(f.key.clone(), note.trim().to_string()));
            }
        }
    }
    if rows.len() > MAX_ROWS {
        let more = crate::i18n::format("electronics.checks_more", &[("n", &(rows.len() - MAX_ROWS).to_string())]);
        ui_kit::text(ui, &more, ui_kit::Text::Note("electronics.checks_more"));
    }
}

/// The picked finding's details: what it measured against which limit, how it was judged, why it
/// matters, why it is only a warning, and its fixes with their buttons.
fn details(ui: &mut Ui, q: &QualityFinding, act: &mut Option<Act>) {
    let f = &q.finding;
    if q.measured {
        ui_kit::text(ui, &crate::electronics_board_rules::measured_text(f), ui_kit::Text::Note("electronics.drc_measured"));
    }
    if let Some(regime) = &q.regime {
        let text = crate::i18n::format("electronics.quality_regime", &[("regime", regime)]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.quality_regime"));
    }
    let principle = worded(&format!("electronics.quality_principle_{}", q.finding.code.replace('.', "_")), q.principle);
    let text = crate::i18n::format("electronics.quality_principle", &[("principle", &principle)]);
    ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.quality_principle"));
    if let Some(n) = &f.note {
        ui_kit::text(ui, n, ui_kit::Text::Note("electronics.drc_note"));
    }
    for (i, fix) in f.fix.iter().enumerate().take(MAX_FIXES) {
        let text = crate::i18n::format("electronics.drc_fix", &[("fix", &fix.says)]);
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.drc_fix"));
        let (label, then) = match fix_button(f, fix) {
            Some(FixButton::Run) => ("electronics.quality_fix_apply", Act::Fix(f.key.clone(), i)),
            Some(FixButton::Intent(record)) => ("electronics.intent_title", Act::Intent(record)),
            None => continue,
        };
        if crate::electronics_editor::button(ui, &format!("quality-fix-{i}"), &tr(label), true) {
            *act = Some(then);
        }
    }
}

/// What the checks need when they couldn't check something, by the shape of their words (see
/// `quality::NotApplicable`): the need with `{name}` for what varies, and its question's id
/// (`electronics.quality_ask_<id>`). The more particular shapes come first.
const ASKS: &[(&str, &str)] = &[
    ("intent: current_a of {net}", "current"),
    ("intent: driver or edge_ns of {net}", "edge"),
    ("intent: the ground net (a net record with role ground)", "ground"),
    ("intent: the board's f_concern_mhz or radio bands", "f_concern"),
    ("intent: f_mhz of {net}, the board's f_concern_mhz or its radio bands", "f_net"),
    ("intent: f_mhz of {net} or the board's radio bands", "f_net"),
    ("intent: f_mhz of {net} (the crystal's frequency sets its length limit)", "f_crystal"),
    ("intent: the RF nets (role rf)", "rf_nets"),
    ("intent: the antenna part (role antenna)", "antenna"),
    ("intent: pairs (the p and n nets of each differential pair)", "pairs"),
    ("intent: pair {pair} names a net the schematic lacks ({p} / {n})", "pair_nets"),
    ("intent: driver, edge_ns or interface of pair {pair} (for its T2 bend rule)", "pair_speed"),
    ("intent: tvs or esd_array parts with the connector pins they protect (of)", "tvs"),
    ("intent: the connector pins {part} protects (of)", "tvs_of"),
    ("intent: the ground role of the net on {part}'s ground pin", "tvs_ground"),
    ("intent: the role of {part} (a module proposed: accept or correct it)", "module"),
    ("intent: decoupling-of {pin} ({part} proposed: accept or correct it)", "decap_proposed"),
    ("intent: decoupling-of records for the IC power pins (ecad.intent.inspect proposes them)", "decaps"),
    ("intent: {part}'s of names {pin}, on neither of its nets", "decap_of"),
    ("intent: the source of {net} (a power output, regulator or connector on it)", "source"),
    ("intent: the hot loops of {part} (loop records)", "hot_loops"),
    ("intent: the switch node of {part}", "switch_node"),
    ("intent: the feedback net of {part}", "feedback"),
    ("intent: the regulator {net} feeds back to (its record's domain)", "feedback_of"),
    ("intent: the board is hand-assembled (hand_assembly)", "hand"),
    ("intent: the crystal role of {part} (crystal of its oscillator's pins)", "crystal"),
    ("intent: the oscillator pins {part} serves (of)", "crystal_of"),
    ("board.quality_levels", "off"),
];

/// The values of `pattern`'s `{name}` parts, when `text` reads as `pattern` (each part at least
/// one character, running to the next fixed words).
fn read<'a>(pattern: &'a str, text: &'a str) -> Option<Vec<(&'a str, &'a str)>> {
    let mut out = Vec::new();
    let (mut pat, mut rest) = (pattern, text);
    while let Some(open) = pat.find('{') {
        let (fixed, after) = pat.split_at(open);
        rest = rest.strip_prefix(fixed)?;
        let close = after.find('}')?;
        let name = after.get(1..close)?;
        pat = after.get(close + 1..)?;
        let next = pat.split('{').next().unwrap_or_default();
        let end = if next.is_empty() { rest.len() } else { rest.find(next)? };
        let value = rest.get(..end)?;
        if value.is_empty() {
            return None;
        }
        out.push((name, value));
        rest = rest.get(end..)?;
    }
    if rest == pat { Some(out) } else { None }
}

/// A need in the user's words, and where it is answered: the Design Intent record it is about
/// (`net|VBUS`, `part|U7`, `board`; empty for Design Intent's first page), or `None` when the
/// board or the library has to change, not the intent.
pub(crate) struct Ask {
    pub text: String,
    pub intent: Option<String>,
}

/// What `need` (a code's `not_applicable` need) asks, in plain words.
pub(crate) fn ask(need: &str) -> Ask {
    for (pattern, id) in ASKS {
        let Some(args) = read(pattern, need) else { continue };
        let text = crate::i18n::format(&format!("electronics.quality_ask_{id}"), &args);
        let value = |name: &str| args.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);
        let record = value("part")
            .map(|p| format!("part|{p}"))
            .or_else(|| value("net").map(|n| format!("net|{n}")))
            .or_else(|| value("pin").map(|p| format!("part|{}", p.split_once('.').map_or(p, |(d, _)| d))));
        let intent = match *id {
            "off" => None,
            "ground" | "f_concern" | "hand" => Some("board".to_string()),
            _ => Some(record.unwrap_or_default()),
        };
        return Ask { text, intent };
    }
    // Any other need, by where it is answered.
    let (key, what, intent) = if let Some(what) = need.strip_prefix("intent: ") {
        ("electronics.quality_ask_intent", what, Some(String::new()))
    } else if let Some(what) = need.strip_prefix("geometry: ") {
        ("electronics.quality_ask_geometry", what, None)
    } else if let Some(what) = need.strip_prefix("library: ") {
        ("electronics.quality_ask_library", what, None)
    } else {
        return Ask { text: crate::i18n::format("electronics.quality_ask_other", &[("what", need)]), intent: None };
    };
    Ask { text: crate::i18n::format(key, &[("what", what)]), intent }
}

/// The codes of `list` that couldn't check everything, each with where and its needs as
/// questions; a question the intent answers is a click into Design Intent (`id` names the
/// window's handles). `Some` with the record (see [`Ask`]) of the question clicked.
pub(crate) fn not_checked(ui: &mut Ui, list: &[&NotApplicable], id: &str) -> Option<String> {
    let mut clicked = None;
    for n in list {
        let key = if n.count == 1 { "electronics.quality_not_checked_code_one" } else { "electronics.quality_not_checked_code" };
        let line = crate::i18n::format(key, &[("code", &code_name(n.code)), ("n", &n.count.to_string())]);
        ui_kit::text(ui, &line, ui_kit::Text::Inline);
        for (i, need) in n.needs.iter().enumerate() {
            let a = ask(need);
            let response = ui_kit::text(ui, &format!("• {}", a.text), ui_kit::Text::Note("electronics.quality_ask"));
            let Some(record) = a.intent else { continue };
            let handle = format!("electronics:{id}:ask:{}:{i}", n.code);
            let response = ui.interact(response.rect, ui.id().with(&handle), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            crate::scenario::publish_target(&handle, response.rect);
            if response.clicked() {
                clicked = Some(record);
            }
        }
    }
    clicked
}

/// Marks at the findings the open Quality window shows (approved ones get none): a ring with "!"
/// in the error, warning or note colour, the picked one ringed in the selection colour. They go
/// with the DRC markers' layer.
pub(crate) fn paint_markers(app: &SolveApp, painter: &egui::Painter, map: PageMap, t: &Tokens) {
    let Some(Open::Quality(w)) = &app.electronics.design.open else { return };
    if app.electronics.board.hidden.contains(&Layer::DrcMarkers) {
        return;
    }
    let Some((_, r)) = &app.electronics.board.quality.report else { return };
    let font = FontId::proportional(Tokens::view().font_small);
    for q in r.findings.iter().filter(|q| !q.finding.approved && kept(w, q)).take(MAX_MARKS) {
        let f = &q.finding;
        let c = map.to_screen(f.at[0], f.at[1]);
        if !(c.x.is_finite() && c.y.is_finite()) {
            continue;
        }
        let ink = match f.severity {
            Severity::Error => t.pcb_drc_error,
            Severity::Warning => t.pcb_drc_warning,
            Severity::Info => t.accent,
        };
        if w.picked.as_deref() == Some(f.key.as_str()) {
            painter.circle_stroke(c, 11.0, Stroke::new(2.0, t.sel_edge));
        }
        painter.circle(c, 7.0, t.viewport_bottom.gamma_multiply(0.8), Stroke::new(2.0, ink));
        painter.text(c + vec2(0.0, 0.5), Align2::CENTER_CENTER, "!", font.clone(), ink);
    }
}

/// The levels Design Rules offers a code after its usual one: as `ecad.quality.rules` names
/// them, and their words.
const LEVELS: [(CheckLevel, &str, &str); 4] = [
    (CheckLevel::Error, "error", "electronics.quality_level_error"),
    (CheckLevel::Warning, "warning", "electronics.quality_level_warning"),
    (CheckLevel::Info, "info", "electronics.quality_level_info"),
    (CheckLevel::Ignore, "ignore", "electronics.quality_level_ignore"),
];

fn severity_word(s: Severity) -> String {
    tr(match s {
        Severity::Error => "electronics.quality_level_error",
        Severity::Warning => "electronics.quality_level_warning",
        Severity::Info => "electronics.quality_level_info",
    })
}

/// The UI's words for `key`, else the catalogue's `fallback` (a code or parameter added since the
/// strings were written).
fn worded(key: &str, fallback: &str) -> String {
    let text = tr(key);
    if text == key { fallback.to_string() } else { text }
}

/// A parameter's name in the UI's words, else as the catalogue names it.
fn param_label(code: &str, name: &str) -> String {
    let key = format!("electronics.quality_param_{}_{name}", code.replace('.', "_"));
    let text = tr(&key);
    if text != key {
        return text;
    }
    let words = name.replace('_', " ");
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// Design Rules' Quality Checks section: the reason a change is kept with, then each family's
/// codes (folded): its level and parameters. `Some` with the `ecad.quality.rules` parameters of a
/// change (see [`apply_rules`]).
pub(crate) fn rules_section(app: &mut SolveApp, ui: &mut Ui) -> Option<Value> {
    let mut out = None;
    let (levels, set) = {
        let b = board(app);
        (b.quality_levels.clone(), b.quality_params.clone())
    };
    ui_kit::text(ui, &tr("electronics.quality_rules_help"), ui_kit::Text::Note("electronics.quality_rules_help"));
    ui_kit::text_row(ui, &tr("electronics.quality_rules_note"), &mut app.electronics.board.quality.note, &tr("electronics.quality_rules_note_hint"));
    for family in catalog::families() {
        let codes: Vec<&'static Code> = catalog::codes().filter(|c| c.family() == family).collect();
        ui_kit::section_count(ui, ("electronics_rules_quality", family), &family_name(family), false, Some(codes.len()), |ui| {
            for c in codes {
                code_rules(app, ui, c, &levels, &set, &mut out);
            }
        });
    }
    out
}

/// One code in Design Rules: its name, class and what it finds, its level, its first parameter,
/// and the others folded.
fn code_rules(
    app: &mut SolveApp,
    ui: &mut Ui,
    c: &'static Code,
    levels: &BTreeMap<String, CheckLevel>,
    set: &BTreeMap<String, ParamSetting>,
    out: &mut Option<Value>,
) {
    ui.add_space(Tokens::get().space_6);
    ui_kit::text(ui, &code_name(c.code), ui_kit::Text::Heading("electronics.quality_rules_code"));
    let what = worded(&format!("electronics.quality_about_{}", c.code.replace('.', "_")), c.about);
    let about = crate::i18n::format("electronics.quality_code_about", &[("class", &class_name(c.class)), ("about", &what)]);
    ui_kit::text(ui, &about, ui_kit::Text::Note("electronics.quality_code_about"));
    let usual = crate::i18n::format("electronics.quality_level_usual", &[("level", &severity_word(c.severity))]);
    let options: Vec<String> = std::iter::once(usual).chain(LEVELS.iter().map(|(_, _, k)| tr(k))).collect();
    let now = levels.get(c.code).and_then(|l| LEVELS.iter().position(|(x, ..)| x == l)).map_or(0, |i| i + 1);
    if let Some(i) =
        crate::electronics_editor::choice(ui, &format!("board:quality:{}:level", c.code), &tr("electronics.quality_level"), now, &options)
    {
        let level = i.checked_sub(1).and_then(|i| LEVELS.get(i)).map_or(Value::Null, |(_, name, _)| json!(name));
        let mut params = json!({"rules": {}});
        params["rules"][c.code] = level;
        *out = Some(params);
    }
    let mut params = c.params.iter();
    if let Some(p) = params.next() {
        param_row(app, ui, c, p, set, out);
    }
    let rest: Vec<&Param> = params.collect();
    if !rest.is_empty() {
        let heading = tr("electronics.quality_more_params");
        ui_kit::section_count(ui, ("electronics_rules_quality_more", c.code), &heading, false, Some(rest.len()), |ui| {
            for p in rest {
                param_row(app, ui, c, p, set, out);
            }
        });
    }
}

/// One parameter: its field (the board's value, else the default), what it is, the default and
/// its source, and, when the board sets it, who set it and why, with Use Default.
fn param_row(app: &mut SolveApp, ui: &mut Ui, c: &Code, p: &Param, set: &BTreeMap<String, ParamSetting>, out: &mut Option<Value>) {
    let key = format!("{}.{}", c.code, p.name);
    let own = set.get(&key);
    let label = format!("{} ({})", param_label(c.code, p.name), p.units);
    let change = |v: Value| {
        let mut params = json!({"params": {}});
        params["params"][c.code] = json!({});
        params["params"][c.code][p.name] = v;
        params
    };
    if let Some(v) = crate::electronics_editor::number(app, ui, &format!("board:quality:{key}"), &label, own.map_or(p.default, |s| s.value)) {
        *out = Some(change(json!(v)));
    }
    let what = worded(&format!("electronics.quality_param_about_{}_{}", c.code.replace('.', "_"), p.name), p.about);
    ui_kit::text(ui, &what, ui_kit::Text::Note("electronics.quality_param_about"));
    let default = if p.verify { "electronics.quality_param_default_verify" } else { "electronics.quality_param_default" };
    let line = crate::i18n::format(default, &[("value", &mm(p.default)), ("units", p.units), ("source", p.source)]);
    ui_kit::text(ui, &line, ui_kit::Text::Note("electronics.quality_param_default"));
    let Some(own) = own else { return };
    let who = match own.by {
        Some(Actor::User) => "electronics.quality_param_set_you",
        Some(Actor::Agent) => "electronics.quality_param_set_agent",
        None => "electronics.quality_param_set",
    };
    let note = if own.note.trim().is_empty() { "—" } else { own.note.as_str() };
    ui_kit::text(ui, &crate::i18n::format(who, &[("note", note)]), ui_kit::Text::Note("electronics.quality_param_set"));
    if crate::electronics_editor::button(ui, &format!("quality-param-default-{key}"), &tr("electronics.quality_param_reset"), true) {
        *out = Some(change(Value::Null));
    }
}

/// Make the change Design Rules' Quality Checks section asked for, as the user's: a parameter
/// with the reason typed (or, with none, that it was set there).
pub(crate) fn apply_rules(app: &mut SolveApp, mut params: Value) {
    if params.get("params").is_some() {
        let typed = app.electronics.board.quality.note.trim().to_string();
        params["note"] = json!(if typed.is_empty() { tr("electronics.quality_rules_note_default") } else { typed });
    }
    drop(app.run_as_user("ecad.quality.rules", params));
}
