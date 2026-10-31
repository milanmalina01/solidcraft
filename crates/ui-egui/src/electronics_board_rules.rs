//! The board's rules (see `electronics_board`), on its RULES tab. Design Rules opens a window with
//! the board's fab profile (a preset whose figures, each with its source, set the rules'
//! minimums: `ecad.board.profile`), the design rules in groups (clearance, sizes, vias, holes,
//! silkscreen, edge) and the net classes: a table of them, the picked one's sizes, and its nets
//! picked from the design's nets with a search; each change is one `ecad.board.rules`,
//! `ecad.board.netclass` or `ecad.board.netclass.delete`. Under them, folded, the design-quality
//! checks' levels and parameters (see `electronics_board_quality`). DRC opens the design rule
//! check's findings, grouped by code with a count each and filtered by severity and layer (and, from
//! Check Board, by code or to the approvals waiting for the user); a click on one frames it on the
//! board and picks what it is about, with what was measured against which limit, and the picked
//! one can be approved with a note (`ecad.drc.approve`, as the user's) or taken back. Each
//! approval says who made it: one an agent made of a critical finding still counts until the user
//! confirms it here. While that window is open the board shows a mark at each finding, and the
//! check runs again once the board has stopped changing for a moment (its budget keeps it
//! bounded).
//!
//! Net Classes (on the board's RULES tab and the schematic's DESIGN tab) opens the class table and
//! the picked class's editor on their own, with Width from Current: a current, the temperature
//! rise allowed, the ambient, the copper and an optional length give the narrowest track on outer
//! and inner layers (IPC-2221, `ecad::current`, worked out as the fields are typed) with its
//! resistance, drop and loss; Use sets the class's width and keeps the current with it, in one
//! `ecad.board.netclass`. A class's width stays what routes and what the check holds it to.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use egui::{Align2, FontId, Stroke, Ui, vec2};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::current::{self, AMBIENT, Capacity, CurrentError, DEFAULT_RISE, FORMULA, Loss};
use solidcraft_engine::doc::ecad::drc::{self, Bound, DrcFinding, DrcItem, DrcReport, RULES};
use solidcraft_engine::doc::ecad::profiles::{self, FabProfile, RingConvention};
use solidcraft_engine::doc::ecad::rules::{DEFAULT_CLASS, MAX_CLASS_NAME, RULE_TRACK};
use solidcraft_engine::doc::ecad::{Actor, CopperLayer, NetClass, Rules, Severity, StackLayer};

use crate::SolveApp;
use crate::electronics_board::{Layer, board, placed_box, stored_nets};
use crate::electronics_board_route::layer_name;
use crate::electronics_design::Open;
use crate::electronics_editor::{mm, tr};
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;
use crate::ui_kit;

/// The least a finding is framed across, in millimetres.
pub(crate) const FRAME_MM: f64 = 8.0;
/// A footprint no wider or taller than this, in millimetres, is framed whole with its finding.
const SMALL_MM: f64 = 25.0;
/// The board has to stay unchanged this long before the open DRC window checks it again.
const SETTLE: Duration = Duration::from_millis(400);
/// The most findings the DRC window lists under each heading, and marks on the board.
const MAX_ROWS: usize = 300;
const MAX_MARKS: usize = 2000;
/// The most nets the Net Classes section lists for picking.
const MAX_NET_ROWS: usize = 300;

/// The design rule check worked out last, for the document and revision it was worked out for,
/// and when the board was first seen at a revision after it.
#[derive(Default)]
pub struct DrcState {
    report: Option<((u64, u64), Arc<DrcReport>)>,
    changed: Option<((u64, u64), web_time::Instant)>,
}

/// The Design Rules window: the net class picked in its table, and the search of its nets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RulesWindow {
    pub class: Option<String>,
    pub search: String,
}

/// The Net Classes window: the class picked in its table, the search of its nets, and Width from
/// Current's fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClassesWindow {
    pub list: RulesWindow,
    pub calc: CurrentCalc,
}

/// The copper Width from Current works on.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Copper {
    /// The board's stack-up: its thinnest outer and inner copper.
    #[default]
    Stack,
    /// A weight, in oz/ft².
    Oz(f64),
    /// The thickness typed, in millimetres.
    Mm,
}

/// The copper weights offered, in oz/ft².
const OUNCES: [f64; 4] = [0.5, 1.0, 2.0, 3.0];
/// The temperature rises offered, in °C.
const RISES: [f64; 4] = [10.0, 20.0, 30.0, 45.0];

/// Width from Current's fields, as typed: the answer follows them as they change.
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentCalc {
    /// The class the fields were filled from last (`Some(None)` for the default class), so a
    /// class picked with a current of its own fills them.
    pub from: Option<Option<String>>,
    /// Amperes, °C above the ambient, and °C.
    pub current: String,
    pub rise: String,
    pub ambient: String,
    pub copper: Copper,
    /// Millimetres, for [`Copper::Mm`].
    pub copper_mm: String,
    /// The track runs on an inner layer.
    pub inner: bool,
    /// Millimetres; empty for none.
    pub length: String,
}

impl Default for CurrentCalc {
    fn default() -> Self {
        CurrentCalc {
            from: None,
            current: "1".into(),
            rise: mm(DEFAULT_RISE),
            ambient: mm(current::DEFAULT_AMBIENT),
            copper: Copper::Stack,
            copper_mm: mm(solidcraft_engine::doc::ecad::stackup::OUTER_COPPER),
            inner: false,
            length: String::new(),
        }
    }
}

/// The DRC window: the finding picked (by key), the note to approve it with, and the filters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrcWindow {
    pub picked: Option<String>,
    pub note: String,
    /// Only findings on this layer (and those on none).
    pub layer: Option<CopperLayer>,
    /// Only errors (`Some(Error)`) or warnings.
    pub severity: Option<Severity>,
    /// Only findings of these codes (Check Board's lines); empty for all.
    pub codes: BTreeSet<String>,
    /// Only the approvals an agent made that wait for the user's confirmation.
    pub pending: bool,
}

/// Design Rules or DRC on the toolbar: open its window, or close it.
pub(crate) fn toggle(app: &mut SolveApp, id: &str) {
    let window = if id == "ecad.drc.inspect" { Open::Drc(DrcWindow::default()) } else { Open::Rules(RulesWindow::default()) };
    crate::electronics_design::toggle(app, window);
}

/// The Net Classes window, open on the net class `name` (one just made); the Layer Stack window
/// gives way. Width from Current keeps what was typed in it.
pub(crate) fn show_class(app: &mut SolveApp, name: &str) {
    app.electronics.board.stack_open = false;
    let calc = match &app.electronics.design.open {
        Some(Open::NetClasses(w)) => w.calc.clone(),
        _ => CurrentCalc::default(),
    };
    app.electronics.design.open =
        Some(Open::NetClasses(ClassesWindow { list: RulesWindow { class: Some(name.to_string()), search: String::new() }, calc }));
    app.electronics.sheet.choosing = false;
}

fn key(app: &SolveApp) -> (u64, u64) {
    (app.session.uid, app.session.revision)
}

/// The check as it ran last, if it has (for the board as it is, or before its last changes).
pub(crate) fn cached(app: &SolveApp) -> Option<&DrcReport> {
    app.electronics.board.drc.report.as_ref().map(|(_, r)| r.as_ref())
}

/// Whether the cached check is for the board as it is.
pub(crate) fn current(app: &SolveApp) -> bool {
    app.electronics.board.drc.report.as_ref().is_some_and(|(k, _)| *k == key(app))
}

/// The check for the board as it is, worked out now when the cached one is older.
fn report_now(app: &mut SolveApp) -> Arc<DrcReport> {
    let now = key(app);
    if let Some((k, r)) = &app.electronics.board.drc.report
        && *k == now
    {
        return r.clone();
    }
    let r = Arc::new(drc::check(&app.session.doc.ecad, &stored_nets(app)));
    app.electronics.board.drc = DrcState { report: Some((now, r.clone())), changed: None };
    r
}

/// The check worked out for the board as it is now, when the cached one is older: after a check
/// ran elsewhere (`ecad.drc.inspect`, `ecad.board.check`), so the Inspector's summary follows it.
pub(crate) fn checked_now(app: &mut SolveApp) {
    if app.session.doc.ecad.board.is_empty() {
        return;
    }
    drop(report_now(app));
}

/// Pick and frame on the board the finding with the key `key` (Check Board's rows); `false`
/// when the check finds none such now.
pub(crate) fn show_key(app: &mut SolveApp, key: &str) -> bool {
    let r = report_now(app);
    let Some(f) = r.findings.iter().find(|f| f.key == key) else { return false };
    show_finding(app, f);
    true
}

/// A finding's message in the UI's words, when the cached check (for the board as it is) has it.
pub(crate) fn message_of(app: &SolveApp, key: &str) -> Option<String> {
    if !current(app) {
        return None;
    }
    cached(app)?.findings.iter().find(|f| f.key == key).map(message)
}

/// The check for the board as it is: the cached one while nothing changed; after a change, the
/// cached one until the board has stayed the same for [`SETTLE`], then worked out again.
fn report(app: &mut SolveApp, ctx: &egui::Context) -> Arc<DrcReport> {
    let now = key(app);
    let st = &mut app.electronics.board.drc;
    if let Some((k, r)) = &st.report {
        if *k == now {
            return r.clone();
        }
        let since = match st.changed {
            Some((k, at)) if k == now => at,
            _ => {
                let at = web_time::Instant::now();
                st.changed = Some((now, at));
                at
            }
        };
        let waited = since.elapsed();
        if waited < SETTLE {
            ctx.request_repaint_after(SETTLE.saturating_sub(waited));
            return r.clone();
        }
    }
    let r = Arc::new(drc::check(&app.session.doc.ecad, &stored_nets(app)));
    app.electronics.board.drc = DrcState { report: Some((now, r.clone())), changed: None };
    r
}

/// The board Inspector's line for the check once it has run: errors, warnings and approved.
pub(crate) fn summary(app: &SolveApp) -> Option<String> {
    let r = cached(app)?;
    let counts = [("errors", r.errors.to_string()), ("warnings", r.warnings.to_string()), ("approved", r.approved.to_string())];
    let args: Vec<(&str, &str)> = counts.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let key = if current(app) { "electronics.drc_summary" } else { "electronics.drc_summary_stale" };
    Some(crate::i18n::format(key, &args))
}

/// How the UI names a finding's item: a pad or footprint by its reference, the rest by kind and id.
pub(crate) fn item_name(item: &DrcItem) -> String {
    let id = |key: &str, id: u64| crate::i18n::format(key, &[("id", &id.to_string())]);
    match item {
        DrcItem::Pad { reference, .. } => reference.clone(),
        DrcItem::Footprint { designator, .. } => designator.clone(),
        DrcItem::Track(t) => id("electronics.drc_item_track", *t),
        DrcItem::TrackEnd(t, 0) => id("electronics.drc_item_track_start", *t),
        DrcItem::TrackEnd(t, _) => id("electronics.drc_item_track_end", *t),
        DrcItem::Via(v) => id("electronics.drc_item_via", *v),
        DrcItem::Hole(h) => id("electronics.drc_item_hole", *h),
        DrcItem::Pour(p) => id("electronics.drc_item_pour", *p),
        DrcItem::Keepout(k) => id("electronics.drc_item_keepout", *k),
        DrcItem::Graphic(g) => id("electronics.drc_item_drawing", *g),
    }
}

/// A finding's message in the UI's words: its own arguments, its items and layer named as the UI
/// names them (the quality checks' findings too).
pub(crate) fn message(f: &DrcFinding) -> String {
    let mut args: BTreeMap<&str, String> = f.args.iter().map(|(k, v)| (*k, v.clone())).collect();
    for (name, item) in ["a", "b"].into_iter().zip(&f.items) {
        args.insert(name, item_name(item));
    }
    if let Some(l) = f.layer {
        args.insert("layer", layer_name(l));
    }
    // A via's protection in words ("Open"), and the via-in-pad cases that say something else.
    if let Some(p) = args.get("protection").cloned() {
        args.insert("protection", tr(&format!("electronics.via_protection_{p}")));
    }
    let key = match (f.code, args.contains_key("stacked"), args.contains_key("thermal")) {
        ("via_in_pad", true, _) => "electronics.drc_msg_via_in_pad_stacked".to_string(),
        ("via_in_pad", _, true) => "electronics.drc_msg_via_in_pad_thermal".to_string(),
        // A micro via too deep for the profile, or too deep for its laser hole.
        ("via_aspect_ratio", ..) if args.contains_key("deepest") => "electronics.drc_msg_via_aspect_ratio_depth".to_string(),
        ("via_aspect_ratio", ..) if args.contains_key("drill") => "electronics.drc_msg_via_aspect_ratio_micro".to_string(),
        // A pad joined solid that the fill misses, none or too few of a relief's spokes reaching
        // it, or spokes narrower than the least track.
        ("relief_starved", ..) if args.contains_key("solid") => "electronics.drc_msg_relief_starved_solid".to_string(),
        ("relief_starved", ..) if args.get("connected").is_some_and(|n| n == "0") => "electronics.drc_msg_relief_starved_none".to_string(),
        ("relief_starved", ..) if args.contains_key("connected") => "electronics.drc_msg_relief_starved_few".to_string(),
        ("relief_starved", ..) if args.contains_key("value") => "electronics.drc_msg_relief_starved_narrow".to_string(),
        _ => f.message_key.clone(),
    };
    let pairs: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let text = crate::i18n::format(&key, &pairs);
    if text == key { f.message.clone() } else { text }
}

/// A finding's row title: what it is about (`R1.2, via 7`).
fn title(f: &DrcFinding) -> String {
    let about: Vec<String> = f.items.iter().take(2).map(item_name).collect();
    if about.is_empty() { tr(&format!("electronics.drc_code_{}", f.code)) } else { about.join(", ") }
}

/// Whether the window's filters keep `f`.
fn kept(w: &DrcWindow, f: &DrcFinding) -> bool {
    w.severity.is_none_or(|s| s == f.severity)
        && w.layer.is_none_or(|l| f.layer.is_none_or(|fl| fl == l))
        && (w.codes.is_empty() || w.codes.contains(f.code))
        && (!w.pending || f.pending_user)
}

/// What the DRC window was asked to do.
enum Act {
    Pick(String),
    Approve(String, String),
    Unapprove(String),
    ClearStale,
    /// Show every finding again (the code and waiting filters off).
    ShowAll,
}

/// An approval's note and who made it (`None`: made before that was kept), by key.
type Approvals = BTreeMap<String, (String, Option<Actor>)>;

/// Who made an approval and what it says, in the UI's words: "Approved by you: …", or, for an
/// agent's approval of a critical finding (or one of no known author), that it waits for the
/// user.
pub(crate) fn approval_words(pending: bool, by: Option<Actor>, note: &str) -> String {
    let note = if note.trim().is_empty() { "—".to_string() } else { note.to_string() };
    let key = match (pending, by) {
        (true, None) => "electronics.approved_unknown_pending",
        (true, _) => "electronics.approved_pending",
        (false, Some(Actor::User)) => "electronics.approved_by_you",
        (false, Some(Actor::Agent)) => "electronics.approved_by_agent",
        (false, None) => "electronics.approved_unknown",
    };
    crate::i18n::format(key, &[("note", &note)])
}

fn approval_text(f: &DrcFinding, approvals: &Approvals) -> String {
    let (note, by) = approvals.get(&f.key).cloned().unwrap_or((String::new(), f.by));
    approval_words(f.pending_user, by, &note)
}

/// A number as a finding gives it: up to four decimals, no trailing zeros.
fn figure(v: f64) -> String {
    if v.is_finite() { mm(v) } else { "—".into() }
}

/// What a finding measured against which limit, and by how much it is inside it (negative: past
/// it), in the UI's words.
pub(crate) fn measured_text(f: &DrcFinding) -> String {
    // A categorical measure: what the hole is, and what it must be.
    if let (Some(m), Some(l)) = (f.measured_text, f.limit.text) {
        let (m, l) = (tr(&format!("electronics.via_protection_{m}")), tr(&format!("electronics.via_protection_{l}")));
        return crate::i18n::format("electronics.drc_measured_protection", &[("measured", &m), ("limit", &l), ("param", &f.limit.param)]);
    }
    let bound = match f.limit.bound {
        Bound::Min => "electronics.drc_limit_min",
        Bound::Max => "electronics.drc_limit_max",
    };
    let limit = crate::i18n::format(
        bound,
        &[("limit", &figure(f.limit.value)), ("units", &unit_word(f.limit.units, f.limit.value)), ("param", &f.limit.param)],
    );
    crate::i18n::format(
        "electronics.drc_measured",
        &[("measured", &figure(f.measured)), ("units", &unit_word(f.limit.units, f.measured)), ("limit", &limit), ("margin", &figure(f.margin))],
    )
}

/// A finding's units in words, a count of one in the singular ("1 spoke"); units with no words
/// of their own (mm) as they are.
fn unit_word(units: &str, n: f64) -> String {
    let key = if (n - 1.0).abs() < 1e-9 { format!("electronics.drc_unit_{units}_one") } else { format!("electronics.drc_unit_{units}") };
    let word = tr(&key);
    if word == key { units.to_string() } else { word }
}

/// The DRC window: the counts, the filters, then the findings by code (errors first) and the
/// approved ones; the picked one with its approval.
pub(crate) fn drc_window(app: &mut SolveApp, ctx: &egui::Context, mut w: DrcWindow) {
    let r = report(app, ctx);
    let fresh = current(app);
    let notes: Approvals = board(app).approvals.iter().map(|a| (a.key.clone(), (a.note.clone(), a.by))).collect();
    let stack = board(app).stack();
    let title_text = tr("electronics.drc_title");
    let mut act: Option<Act> = None;
    let closed = ui_kit::Window::new(egui::Id::new("electronics_board_drc"), "interference", &title_text)
        .placed(Align2::RIGHT_TOP, crate::electronics_design::beside(app, ctx))
        .resizable()
        .show_body(ctx, |ui| {
            let summary = crate::i18n::format(
                "electronics.checks_summary",
                &[("errors", &r.errors.to_string()), ("warnings", &r.warnings.to_string()), ("approved", &r.approved.to_string())],
            );
            ui_kit::text(ui, &summary, ui_kit::Text::Note("electronics.checks_summary"));
            if r.pending_user > 0 {
                let key = if r.pending_user == 1 { "electronics.drc_pending_summary_one" } else { "electronics.drc_pending_summary" };
                let text = crate::i18n::format(key, &[("n", &r.pending_user.to_string())]);
                ui_kit::text(ui, &text, ui_kit::Text::Error("electronics.drc_pending_summary"));
            }
            let profile = match r.profile {
                Some(p) => crate::i18n::format("electronics.drc_profile", &[("profile", &profile_name(p))]),
                None => tr("electronics.drc_no_profile"),
            };
            ui_kit::text(ui, &profile, ui_kit::Text::Note("electronics.drc_profile"));
            if !fresh {
                ui_kit::text(ui, &tr("electronics.drc_checking"), ui_kit::Text::Note("electronics.drc_checking"));
            }
            if r.truncated {
                ui_kit::text(ui, &tr("electronics.drc_truncated"), ui_kit::Text::Error("electronics.drc_truncated"));
            }
            filters(ui, &mut w, &stack, r.pending_user);
            // Check Board's filters, said in words with a way back to every finding.
            if !w.codes.is_empty() || w.pending {
                let mut what: Vec<String> = w.codes.iter().map(|c| tr(&format!("electronics.drc_code_{c}"))).collect();
                if w.pending {
                    what.push(tr("electronics.drc_waiting"));
                }
                let text = crate::i18n::format("electronics.drc_only", &[("what", &what.join(", "))]);
                ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.drc_only"));
                if crate::electronics_editor::button(ui, "drc-show-all", &tr("electronics.drc_show_all"), true) {
                    act = Some(Act::ShowAll);
                }
            }
            if r.findings.is_empty() {
                ui_kit::text(ui, &tr("electronics.drc_none"), ui_kit::Text::Note("electronics.drc_none"));
            }
            for rule in RULES {
                let rows: Vec<&DrcFinding> = r.findings.iter().filter(|f| f.code == rule.code && !f.approved && kept(&w, f)).collect();
                if rows.is_empty() {
                    continue;
                }
                let heading = tr(&format!("electronics.drc_code_{}", rule.code));
                // Open when it holds errors, the picked finding, or is what Check Board asked for.
                let open = rule.severity == Severity::Error
                    || w.codes.contains(rule.code)
                    || w.pending
                    || rows.iter().any(|f| w.picked.as_deref() == Some(f.key.as_str()));
                ui_kit::section_count(ui, ("electronics_drc_code", rule.code), &heading, open, Some(rows.len()), |ui| {
                    finding_rows(ui, &rows, w.picked.as_deref(), &mut w.note, &notes, &mut act);
                });
            }
            let approved: Vec<&DrcFinding> = r.findings.iter().filter(|f| f.approved && kept(&w, f)).collect();
            if !approved.is_empty() {
                let heading = tr("electronics.checks_approved");
                ui_kit::section_count(ui, "electronics_drc_approved", &heading, false, Some(approved.len()), |ui| {
                    finding_rows(ui, &approved, w.picked.as_deref(), &mut w.note, &notes, &mut act);
                });
            }
            // A check cut short names none stale (findings it did not reach may hold them).
            if !r.truncated && !r.stale.is_empty() {
                ui.add_space(Tokens::get().space_8);
                let text = crate::i18n::format("electronics.checks_stale", &[("n", &r.stale.len().to_string())]);
                ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.checks_stale"));
                if crate::electronics_editor::button(ui, "drc-clear-stale", &tr("electronics.checks_clear_stale"), true) {
                    act = Some(Act::ClearStale);
                }
            }
        });
    match act {
        Some(Act::Pick(key)) => {
            if let Some(f) = r.findings.iter().find(|f| f.key == key) {
                show_finding(app, f);
            }
            // The note being typed belongs to the finding picked before.
            if w.picked.as_deref() != Some(key.as_str()) {
                w.note.clear();
            }
            w.picked = Some(key);
        }
        Some(Act::Approve(key, text)) => {
            if app.run_as_user("ecad.drc.approve", json!({"keys": [key], "note": text})).is_ok() {
                w.note.clear();
                app.set_status(tr("electronics.drc_approved_now"), false);
            }
        }
        // The user's own approvals are the user's to take back.
        Some(Act::Unapprove(key)) => {
            if app.run_as_user("ecad.drc.unapprove", json!({"keys": [key]})).is_ok() {
                app.set_status(tr("electronics.drc_unapproved_now"), false);
            }
        }
        Some(Act::ClearStale) => {
            if app.run_as_user("ecad.drc.unapprove", json!({"stale": true})).is_ok() {
                app.set_status(tr("electronics.drc_stale_cleared"), false);
            }
        }
        Some(Act::ShowAll) => {
            w.codes.clear();
            w.pending = false;
        }
        None => {}
    }
    app.electronics.design.open = (!closed).then_some(Open::Drc(w));
}

/// The severity chips (with Waiting for You while an agent's approvals wait) and the layer
/// dropdown.
fn filters(ui: &mut Ui, w: &mut DrcWindow, stack: &[CopperLayer], pending: usize) {
    ui_kit::wrapped_row(ui, crate::i18n::tr("electronics.field_show"), |ui| {
        for (s, key, name) in [
            (None, "all", "electronics.drc_all"),
            (Some(Severity::Error), "errors", "electronics.checks_errors"),
            (Some(Severity::Warning), "warnings", "electronics.checks_warnings"),
        ] {
            if ui_kit::choice_chip(ui, &format!("drc:severity:{key}"), crate::i18n::tr(name), w.severity == s && !w.pending).clicked() {
                w.severity = s;
                w.pending = false;
            }
        }
        if (pending > 0 || w.pending)
            && ui_kit::choice_chip(ui, "drc:severity:waiting", crate::i18n::tr("electronics.drc_waiting"), w.pending).clicked()
        {
            w.pending = !w.pending;
        }
    });
    let all = tr("electronics.drc_all_layers");
    let shown = w.layer.map_or_else(|| all.clone(), layer_name);
    ui_kit::row(ui, crate::i18n::tr("electronics.field_layer"), |ui| {
        ui_kit::dropdown_menu(ui, "electronics_drc_layer", &shown, |ui| {
            if ui_kit::list_picker(ui, "drc_layer_all", &all, w.layer.is_none()).clicked() {
                w.layer = None;
            }
            for l in stack {
                if ui_kit::list_picker(ui, &format!("drc_layer_{}", l.name()), &layer_name(*l), w.layer == Some(*l)).clicked() {
                    w.layer = Some(*l);
                }
            }
        });
    });
}

/// The rows of `rows` (at most [`MAX_ROWS`]): each finding's title and message, and who approved
/// it; the `picked` one with what it measured and its approval (`note` and Approve; the note it
/// was approved with and Unapprove; or, for an agent's approval waiting for the user, Confirm).
fn finding_rows(ui: &mut Ui, rows: &[&DrcFinding], picked: Option<&str>, note: &mut String, notes: &Approvals, act: &mut Option<Act>) {
    for f in rows.iter().take(MAX_ROWS) {
        let selected = picked == Some(f.key.as_str());
        let response = ui_kit::text(ui, &title(f), ui_kit::Text::NameCell(selected));
        // Only the part of a row the list shows: one scrolled out of sight has no handle.
        let shown = response.rect.intersect(ui.clip_rect());
        if shown.is_positive() {
            crate::scenario::publish_target(&format!("electronics:drc:{}", f.key), shown);
        }
        if response.clicked() {
            *act = Some(Act::Pick(f.key.clone()));
        }
        let style = if f.severity == Severity::Error && !f.approved {
            ui_kit::Text::Error("electronics.drc_message")
        } else {
            ui_kit::Text::Note("electronics.drc_message")
        };
        ui_kit::text(ui, &message(f), style);
        // Who approved it shows on the row; one waiting for the user stands out.
        if f.approved || f.pending_user {
            let style = if f.pending_user { ui_kit::Text::Error("electronics.drc_approval") } else { ui_kit::Text::Note("electronics.drc_approval") };
            ui_kit::text(ui, &approval_text(f, notes), style);
        }
        if !selected {
            continue;
        }
        ui_kit::text(ui, &measured_text(f), ui_kit::Text::Note("electronics.drc_measured"));
        if let Some(n) = &f.note {
            ui_kit::text(ui, n, ui_kit::Text::Note("electronics.drc_note"));
        }
        for fix in f.fix.iter().take(3) {
            let text = crate::i18n::format("electronics.drc_fix", &[("fix", &fix.says)]);
            ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.drc_fix"));
        }
        if f.pending_user {
            // The user's approval confirms the agent's, keeping its note unless one is typed.
            let (enter, _) = ui_kit::text_row(ui, &tr("electronics.checks_note"), note, &tr("electronics.drc_confirm_hint"));
            if crate::electronics_editor::button(ui, "drc-confirm", &tr("electronics.drc_confirm"), true) || enter {
                *act = Some(Act::Approve(f.key.clone(), note.trim().to_string()));
            }
            if crate::electronics_editor::button(ui, "drc-unapprove", &tr("electronics.checks_unapprove"), true) {
                *act = Some(Act::Unapprove(f.key.clone()));
            }
        } else if f.approved {
            if crate::electronics_editor::button(ui, "drc-unapprove", &tr("electronics.checks_unapprove"), true) {
                *act = Some(Act::Unapprove(f.key.clone()));
            }
        } else {
            let (enter, _) = ui_kit::text_row(ui, &tr("electronics.checks_note"), note, &tr("electronics.checks_note_hint"));
            if crate::electronics_editor::button(ui, "drc-approve", &tr("electronics.checks_approve"), true) || enter {
                *act = Some(Act::Approve(f.key.clone(), note.trim().to_string()));
            }
        }
    }
    if rows.len() > MAX_ROWS {
        let more = crate::i18n::format("electronics.checks_more", &[("n", &(rows.len() - MAX_ROWS).to_string())]);
        ui_kit::text(ui, &more, ui_kit::Text::Note("electronics.checks_more"));
    }
}

/// Pick what a finding is about on the board and frame it with its mark: round where it is, with
/// the items that are small (a track's end, a via, a hole, a footprint no bigger than
/// [`SMALL_MM`]); a whole track, pour or keepout would frame the board rather than the finding.
pub(crate) fn show_finding(app: &mut SolveApp, f: &DrcFinding) {
    let b = board(app);
    let mut points: Vec<[f64; 2]> = vec![f.at];
    let mut picked = BTreeSet::new();
    let small = |bbox: [[f64; 2]; 2]| bbox[1][0] - bbox[0][0] <= SMALL_MM && bbox[1][1] - bbox[0][1] <= SMALL_MM;
    for item in &f.items {
        let id = item.id();
        if b.kind_of(id).is_none() {
            continue;
        }
        picked.insert(id);
        if let Some(t) = b.track(id) {
            match item {
                DrcItem::TrackEnd(_, 0) => points.extend(t.points.first()),
                DrcItem::TrackEnd(..) => points.extend(t.points.last()),
                _ => {}
            }
        } else if let Some(fp) = b.footprint(id) {
            points.extend(placed_box(app, fp).filter(|bb| small(*bb)).into_iter().flatten());
        } else if b.pour(id).is_none() && b.keepout(id).is_none() {
            points.extend(b.position(id));
        }
    }
    let frame = solidcraft_engine::doc::ecad::bbox(points);
    // A route drawn so far is made before its tool goes.
    crate::electronics_board::put_down(app);
    let s = &mut app.electronics.board;
    s.selection = picked;
    s.segment = None;
    s.outline_picked = false;
    s.frame = frame;
}

/// Marks at the findings the open DRC window shows (approved ones get none): a ring with "!" in
/// the error or warning colour, the picked one ringed in the selection colour.
pub(crate) fn paint_markers(app: &SolveApp, painter: &egui::Painter, map: PageMap, t: &Tokens) {
    let Some(Open::Drc(w)) = &app.electronics.design.open else { return };
    if app.electronics.board.hidden.contains(&Layer::DrcMarkers) {
        return;
    }
    let Some(r) = cached(app) else { return };
    let font = FontId::proportional(Tokens::view().font_small);
    for f in r.findings.iter().filter(|f| !f.approved && kept(w, f)).take(MAX_MARKS) {
        let c = map.to_screen(f.at[0], f.at[1]);
        if !(c.x.is_finite() && c.y.is_finite()) {
            continue;
        }
        let ink = if f.severity == Severity::Error { t.pcb_drc_error } else { t.pcb_drc_warning };
        if w.picked.as_deref() == Some(f.key.as_str()) {
            painter.circle_stroke(c, 11.0, Stroke::new(2.0, t.sel_edge));
        }
        painter.circle(c, 7.0, t.viewport_bottom.gamma_multiply(0.8), Stroke::new(2.0, ink));
        painter.text(c + vec2(0.0, 0.5), Align2::CENTER_CENTER, "!", font.clone(), ink);
    }
}

/// The rules the Design Rules window shows, in groups: each with its heading's and its fields'
/// message keys.
const GROUPS: &[(&str, &[(&str, &str)])] = &[
    ("electronics.rules_clearance", &[("clearance", "electronics.field_clearance")]),
    ("electronics.rules_sizes", &[("track_width", "electronics.field_track_width"), ("min_track_width", "electronics.field_min_track_width")]),
    (
        "electronics.rules_vias",
        &[
            ("via_diameter", "electronics.field_via_diameter"),
            ("via_drill", "electronics.field_via_drill"),
            ("min_annular_ring", "electronics.field_min_annular_ring"),
            ("microvia_diameter", "electronics.field_microvia_diameter"),
            ("microvia_drill", "electronics.field_microvia_drill"),
        ],
    ),
    ("electronics.rules_holes", &[("min_drill", "electronics.field_min_drill"), ("hole_to_hole", "electronics.field_hole_to_hole")]),
    ("electronics.rules_silkscreen", &[("silk_to_pad", "electronics.field_silk_to_pad")]),
    ("electronics.rules_edge", &[("copper_to_edge", "electronics.field_copper_to_edge")]),
];

/// A rule's name as Design Rules labels it.
fn rule_label(field: &str) -> String {
    GROUPS.iter().flat_map(|(_, f)| f.iter()).find(|(f, _)| *f == field).map_or_else(|| field.replace('_', " "), |(_, label)| tr(label))
}

/// What picking a profile did, for the status line: "Fab profile Fine pitch: Least track width
/// 0.145 → 0.1 mm, Copper to edge 0.3 → 0.25 mm. Kept your Least track width 0.145 mm."
fn profile_status(reply: &Value) -> String {
    let Some(p) = reply.get("profile").and_then(|p| p.get("id")).and_then(Value::as_str).and_then(profiles::find) else {
        return tr("electronics.profile_cleared");
    };
    let num = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64).map(mm).unwrap_or_default();
    // The field's label without its unit ("Least width", not "Least width (mm)"): the figure
    // after it says it.
    let field = |v: &Value| {
        let label = rule_label(v.get("field").and_then(Value::as_str).unwrap_or_default());
        match label.rsplit_once(" (") {
            Some((name, unit)) if unit.ends_with(')') => name.to_string(),
            _ => label,
        }
    };
    let set: Vec<String> = reply
        .get("rules_set")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| crate::i18n::format("electronics.profile_rule_set", &[("rule", &field(c)), ("from", &num(c, "from")), ("to", &num(c, "to"))]))
        .collect();
    let mut text = if set.is_empty() {
        crate::i18n::format("electronics.profile_set_none", &[("profile", &profile_name(p))])
    } else {
        crate::i18n::format("electronics.profile_set_list", &[("profile", &profile_name(p)), ("rules", &set.join(", "))])
    };
    let kept: Vec<String> = reply
        .get("kept")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| crate::i18n::format("electronics.profile_rule_kept", &[("rule", &field(c)), ("value", &num(c, "value"))]))
        .collect();
    if !kept.is_empty() {
        text.push(' ');
        text.push_str(&crate::i18n::format("electronics.profile_kept", &[("rules", &kept.join(", "))]));
    }
    text
}

/// The rule `field` of `r`.
fn rule(r: &Rules, field: &str) -> f64 {
    match field {
        "track_width" => r.track_width,
        "clearance" => r.clearance,
        "via_diameter" => r.via_diameter,
        "via_drill" => r.via_drill,
        "min_track_width" => r.min_track_width,
        "min_annular_ring" => r.min_annular_ring,
        "min_drill" => r.min_drill,
        "hole_to_hole" => r.hole_to_hole,
        "copper_to_edge" => r.copper_to_edge,
        "silk_to_pad" => r.silk_to_pad,
        "microvia_diameter" => r.microvia_diameter,
        _ => r.microvia_drill,
    }
}

/// A command the Design Rules or Net Classes window runs, the class to pick once it has, and what
/// the status line says then.
struct Edit {
    command: &'static str,
    params: Value,
    pick: Option<Option<String>>,
    status: Option<String>,
}

impl Edit {
    fn class(name: &str, mut params: Value) -> Self {
        params["name"] = json!(name);
        Edit { command: "ecad.board.netclass", params, pick: None, status: None }
    }
}

/// A fab profile's name in the UI's words.
pub(crate) fn profile_name(p: &FabProfile) -> String {
    let key = format!("electronics.profile_{}", p.id);
    let text = tr(&key);
    if text == key { p.name.to_string() } else { text }
}

/// A profile figure's name in the UI's words.
fn figure_name(key: &str) -> String {
    let k = format!("electronics.profile_figure_{key}");
    let text = tr(&k);
    if text == k { key.replace('_', " ") } else { text }
}

/// The Fab Profile section of Design Rules: the board's profile (a dropdown of the presets, or
/// none), what it is, how it measures rings, whether the board took an older version of its
/// figures, Reset Rules to Profile (only while the rules differ from it), and its figures with
/// their sources. `Some` with the `ecad.board.profile` parameters when one is picked.
fn profile_section(ui: &mut Ui, chosen: Option<(&'static FabProfile, u32)>, rules: &Rules) -> Option<Value> {
    let mut pick = None;
    let none = tr("electronics.profile_none");
    let shown = chosen.map_or_else(|| none.clone(), |(p, _)| profile_name(p));
    ui_kit::row(ui, crate::i18n::tr("electronics.field_fab_profile"), |ui| {
        // Each entry is the handle `dialog:electronics_fab_profile:<id>` (`:none` for none).
        ui_kit::dropdown_menu(ui, "electronics_fab_profile", &shown, |ui| {
            for p in profiles::PROFILES {
                let r = ui_kit::list_picker(ui, &format!("fab_profile_{}", p.id), &profile_name(p), chosen.is_some_and(|(c, _)| c.id == p.id));
                crate::scenario::publish_handle(&format!("dialog:electronics_fab_profile:{}", p.id), r.rect.center());
                if r.clicked() {
                    pick = Some(json!({"fab": p.id}));
                }
            }
            ui.separator();
            let r = ui_kit::list_picker(ui, "fab_profile_none", &none, chosen.is_none());
            crate::scenario::publish_handle("dialog:electronics_fab_profile:none", r.rect.center());
            if r.clicked() && chosen.is_some() {
                pick = Some(json!({"fab": null}));
            }
        });
    });
    let Some((p, version)) = chosen else {
        ui_kit::text(ui, &tr("electronics.profile_none_help"), ui_kit::Text::Error("electronics.profile_none_help"));
        return pick;
    };
    ui_kit::text(ui, p.about, ui_kit::Text::Note("electronics.profile_about"));
    let ring = match p.ring_convention {
        RingConvention::Hole => tr("electronics.profile_ring_hole"),
        RingConvention::Tool => crate::i18n::format("electronics.profile_ring_tool", &[("allowance", &mm(p.plating_allowance.value))]),
    };
    ui_kit::text(ui, &ring, ui_kit::Text::Note("electronics.profile_ring"));
    if version != p.version {
        let text = crate::i18n::format("electronics.profile_old_version", &[("was", &version.to_string()), ("now", &p.version.to_string())]);
        ui_kit::text(ui, &text, ui_kit::Text::Error("electronics.profile_old_version"));
    }
    if p.differs(rules) && crate::electronics_editor::button(ui, "board-profile-apply", &tr("electronics.profile_reset"), true) {
        pick = Some(json!({"fab": p.id, "reset": true}));
    }
    ui_kit::section(ui, ("electronics_profile_figures", p.id), &tr("electronics.profile_figures"), false, |ui| {
        for (key, units, f) in p.figures() {
            let value = if units == "mm" { format!("{} mm", mm(f.value)) } else { format!("1:{}", mm(f.value)) };
            let line = format!("{} · {value}", figure_name(key));
            ui_kit::text(ui, &line, ui_kit::Text::Inline);
            let source = if f.verify { crate::i18n::format("electronics.profile_verify", &[("source", f.source)]) } else { f.source.to_string() };
            ui_kit::text(ui, &source, ui_kit::Text::Note("electronics.profile_source"));
        }
        let read = crate::i18n::format("electronics.profile_read", &[("date", p.read), ("version", &p.version.to_string())]);
        ui_kit::text(ui, &read, ui_kit::Text::Note("electronics.profile_read"));
    });
    pick
}

/// The Design Rules window: the fab profile, the rules by group, then the net classes.
pub(crate) fn rules_window(app: &mut SolveApp, ctx: &egui::Context, mut w: RulesWindow) {
    let rules = board(app).rules.clone();
    let stack = board(app).stackup_or_default();
    let names = stored_nets(app).names.clone();
    let chosen = board(app).fab_profile.as_ref().and_then(|c| Some((c.profile()?, c.version)));
    // A class renamed or deleted elsewhere is no longer picked.
    if w.class.as_deref().is_some_and(|c| rules.class(c).is_none()) {
        w.class = None;
    }
    let title_text = tr("electronics.rules_title");
    let mut edit: Option<Edit> = None;
    let mut quality: Option<Value> = None;
    let closed = ui_kit::Window::new(egui::Id::new("electronics_board_rules"), "settings", &title_text)
        .placed(Align2::RIGHT_TOP, crate::electronics_design::beside(app, ctx))
        .resizable()
        .show_body(ctx, |ui| {
            ui_kit::section(ui, "electronics_rules_profile", &tr("electronics.rules_profile"), true, |ui| {
                if let Some(params) = profile_section(ui, chosen, &rules) {
                    edit = Some(Edit { command: "ecad.board.profile", params, pick: None, status: None });
                }
            });
            for (heading, fields) in GROUPS {
                ui_kit::section(ui, ("electronics_rules_group", *heading), &tr(heading), true, |ui| {
                    for (field, label) in *fields {
                        if let Some(v) = crate::electronics_editor::number(app, ui, &format!("board:rules:{field}"), &tr(label), rule(&rules, field))
                        {
                            let mut params = json!({});
                            params[*field] = json!(v);
                            edit = Some(Edit { command: "ecad.board.rules", params, pick: None, status: None });
                        }
                    }
                });
            }
            ui_kit::section(ui, "electronics_rules_classes", &tr("electronics.rules_classes"), true, |ui| {
                if let Some(e) = classes(app, ui, &rules, &names, &stack, &mut w, "electronics.class_help") {
                    edit = Some(e);
                }
            });
            // The design-quality checks' levels and parameters, folded (`electronics_board_quality`).
            ui_kit::section(ui, "electronics_rules_quality", &tr("electronics.rules_quality"), false, |ui| {
                quality = crate::electronics_board_quality::rules_section(app, ui);
            });
        });
    if let Some(params) = quality {
        crate::electronics_board_quality::apply_rules(app, params);
    }
    if let Some(e) = edit
        && let Ok(reply) = app.run(e.command, e.params)
    {
        if let Some(pick) = e.pick {
            w.class = pick;
        }
        // Picking a profile says which rules it set, and which of the user's it kept.
        if e.command == "ecad.board.profile" {
            app.set_status(profile_status(&reply), false);
        }
    }
    app.electronics.design.open = (!closed).then_some(Open::Rules(w));
}

/// A size or pair for a class row: `0.25`, or `0.6/0.3`.
fn pair_text(a: f64, b: f64) -> String {
    format!("{}/{}", mm(a), mm(b))
}

/// A class's current in its row: `2 A · 10 °C → 0.79 mm outer / 2.05 mm inner` (the widths
/// rounded up), or "—" without one.
pub(crate) fn current_text(c: &NetClass, stack: &[StackLayer]) -> String {
    match current::class_widths(c, stack) {
        Some(x) => crate::i18n::format(
            "electronics.class_current_summary",
            &[("current", &mm(x.current)), ("rise", &mm(x.rise)), ("outer", &width_text(x.outer.width)), ("inner", &width_text(x.inner.width))],
        ),
        None => "—".to_string(),
    }
}

/// A proposed width as Use sets it: rounded up to 0.01 mm (it is the narrowest that will do).
fn proposed(w: f64) -> f64 {
    (w * 100.0 - 1e-9).ceil() / 100.0
}

fn width_text(w: f64) -> String {
    format!("{:.2}", proposed(w))
}

/// The Net Classes section: the table (the default class first), Add Net Class, and the picked
/// class's name, sizes, pair, nets and Delete.
fn classes(app: &mut SolveApp, ui: &mut Ui, rules: &Rules, names: &[String], stack: &[StackLayer], w: &mut RulesWindow, help: &str) -> Option<Edit> {
    let (mut edit, picked) = class_table(ui, rules, names, stack, w);
    let Some(c) = picked else {
        ui_kit::text(ui, &tr(help), ui_kit::Text::Note(help));
        return edit;
    };
    if let Some(e) = class_fields(app, ui, rules, names, stack, w, &c) {
        edit = Some(e);
    }
    edit
}

/// The class table (the default class first; a cell's full text on hover) and Add Net Class,
/// with the class picked in it.
fn class_table(ui: &mut Ui, rules: &Rules, names: &[String], stack: &[StackLayer], w: &mut RulesWindow) -> (Option<Edit>, Option<NetClass>) {
    let mut edit = None;
    let in_class: BTreeSet<&str> = rules.net_classes.iter().flat_map(|c| c.nets.iter().map(String::as_str)).collect();
    let default_nets = names.iter().filter(|n| !in_class.contains(n.as_str())).count();
    let headers = [
        tr("electronics.class_name"),
        tr("electronics.class_width"),
        tr("electronics.class_clearance"),
        tr("electronics.class_via"),
        tr("electronics.class_pair"),
        tr("electronics.class_nets"),
        tr("electronics.class_current"),
    ];
    let columns = [
        ui_kit::Column::new(&headers[0], 1.1).flex(),
        ui_kit::Column::new(&headers[1], 0.8).flex(),
        ui_kit::Column::new(&headers[2], 0.9).flex(),
        ui_kit::Column::new(&headers[3], 1.1).flex(),
        ui_kit::Column::new(&headers[4], 1.1).flex(),
        ui_kit::Column::new(&headers[5], 0.5).flex(),
        ui_kit::Column::new(&headers[6], 1.4).flex(),
    ];
    ui_kit::table(ui, "electronics_net_classes", &columns, "", |ui, table| {
        // A row is picked by a click on it or on any of its cells.
        let default_name = tr("electronics.class_default");
        let mut hit = false;
        let response = table.row(ui, DEFAULT_CLASS, w.class.is_none(), |row| {
            for text in [
                default_name.clone(),
                mm(rules.track_width),
                mm(rules.clearance),
                pair_text(rules.via_diameter, rules.via_drill),
                "—".to_string(),
                default_nets.to_string(),
                "—".to_string(),
            ] {
                hit |= row.cell(|ui| ui_kit::text(ui, &text, ui_kit::Text::Cell)).on_hover_text(text.as_str()).clicked();
            }
        });
        if response.clicked() || hit {
            w.class = None;
        }
        for c in &rules.net_classes {
            let pair = c.diff_pair.map_or_else(|| "—".to_string(), |d| pair_text(d.width, d.gap));
            let mut hit = false;
            let response = table.row(ui, &c.name, w.class.as_deref() == Some(c.name.as_str()), |row| {
                for text in [
                    c.name.clone(),
                    mm(c.track_width),
                    mm(c.clearance),
                    pair_text(c.via_diameter, c.via_drill),
                    pair,
                    c.nets.len().to_string(),
                    current_text(c, stack),
                ] {
                    hit |= row.cell(|ui| ui_kit::text(ui, &text, ui_kit::Text::Cell)).on_hover_text(text.as_str()).clicked();
                }
            });
            if response.clicked() || hit {
                w.class = Some(c.name.clone());
            }
        }
    });
    let taken: Vec<String> = rules.net_classes.iter().map(|c| c.name.clone()).chain([DEFAULT_CLASS.to_string()]).collect();
    if crate::electronics_editor::button(ui, "board-netclass-add", &tr("electronics.class_add"), true) {
        let name = crate::electronics_editor::free_from(&tr("electronics.class_new"), &taken);
        edit = Some(Edit { pick: Some(Some(name.clone())), ..Edit::class(&name, json!({})) });
    }
    let picked = w.class.as_deref().and_then(|n| rules.class(n)).cloned();
    (edit, picked)
}

/// The picked class's name, sizes, pair, nets (in a section of their own) and Delete.
fn class_fields(
    app: &mut SolveApp,
    ui: &mut Ui,
    rules: &Rules,
    names: &[String],
    stack: &[StackLayer],
    w: &mut RulesWindow,
    c: &NetClass,
) -> Option<Edit> {
    let mut edit = None;
    ui.add_space(Tokens::get().space_6);
    let key = |field: &str| format!("board:class:{}:{field}", c.name);
    if let Some(n) = crate::electronics_editor::field(app, ui, &key("name"), &tr("electronics.class_name"), &c.name) {
        let n = n.trim().to_string();
        edit = Some(Edit { pick: Some(Some(n.clone())), ..Edit::class(&c.name, json!({"rename": n})) });
    }
    for (field, label, v) in
        [("track_width", "electronics.field_track_width", c.track_width), ("clearance", "electronics.field_clearance", c.clearance)]
    {
        if let Some(x) = crate::electronics_editor::number(app, ui, &key(field), &tr(label), v) {
            let mut params = json!({});
            params[field] = json!(x);
            edit = Some(Edit::class(&c.name, params));
        }
    }
    // A width narrower than the class's current asks for says so (the check holds tracks to the
    // width, not to the current).
    if let Some(x) = current::class_widths(c, stack)
        && c.track_width + 1e-9 < proposed(x.outer.width)
    {
        let text = crate::i18n::format(
            "electronics.class_width_short",
            &[("width", &mm(c.track_width)), ("outer", &width_text(x.outer.width)), ("current", &mm(x.current))],
        );
        ui_kit::text(ui, &text, ui_kit::Text::Error("electronics.class_width_short"));
    }
    if let Some([d, h]) = crate::electronics_editor::pair(app, ui, &key("via"), &tr("electronics.field_via"), [c.via_diameter, c.via_drill]) {
        edit = Some(Edit::class(&c.name, json!({"via_diameter": d, "via_drill": h})));
    }
    if ui_kit::checkbox_row(ui, &key("pair_on"), &tr("electronics.class_diff_pair"), c.diff_pair.is_some()) {
        let pair = match c.diff_pair {
            Some(_) => Value::Null,
            None => json!({"width": c.track_width, "gap": c.clearance}),
        };
        edit = Some(Edit::class(&c.name, json!({"diff_pair": pair})));
    }
    if let Some(d) = c.diff_pair
        && let Some([width, gap]) = crate::electronics_editor::pair(app, ui, &key("pair"), &tr("electronics.field_pair_width_gap"), [d.width, d.gap])
    {
        edit = Some(Edit::class(&c.name, json!({"diff_pair": {"width": width, "gap": gap}})));
    }
    // Its nets: the design's, with a search; a tick puts a net in the class (out of any other).
    let heading = tr("electronics.class_nets_heading");
    ui_kit::section(ui, "electronics_class_nets", &heading, true, |ui| {
        ui_kit::search_field(ui, egui::Id::new("electronics_class_net_search"), &mut w.search, &tr("electronics.library_search"));
        let search = w.search.trim().to_lowercase();
        let mine: BTreeSet<&str> = c.nets.iter().map(String::as_str).collect();
        let shown: Vec<&String> = names.iter().filter(|n| search.is_empty() || n.to_lowercase().contains(&search)).collect();
        for n in shown.iter().take(MAX_NET_ROWS) {
            let on = mine.contains(n.as_str());
            let other = if on { None } else { rules.class_of(n).map(|o| o.name.clone()) };
            let label = match &other {
                Some(o) => crate::i18n::format("electronics.class_net_elsewhere", &[("net", n), ("class", o)]),
                None => (*n).clone(),
            };
            if ui_kit::checkbox_row(ui, &key(&format!("net:{n}")), &label, on) {
                let field = if on { "remove_nets" } else { "add_nets" };
                let mut params = json!({});
                params[field] = json!([n]);
                edit = Some(Edit::class(&c.name, params));
            }
        }
        if shown.len() > MAX_NET_ROWS {
            let more = crate::i18n::format("electronics.checks_more", &[("n", &(shown.len() - MAX_NET_ROWS).to_string())]);
            ui_kit::text(ui, &more, ui_kit::Text::Note("electronics.checks_more"));
        }
        let unknown: Vec<&str> = c.nets.iter().map(String::as_str).filter(|n| !names.iter().any(|m| m == n)).collect();
        if !unknown.is_empty() {
            let text = crate::i18n::format("electronics.class_unknown_nets", &[("nets", &unknown.join(", "))]);
            ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.class_unknown_nets"));
        }
    });
    if crate::electronics_editor::button(ui, "board-netclass-delete", &tr("electronics.class_delete"), true) {
        edit = Some(Edit { command: "ecad.board.netclass.delete", params: json!({"name": c.name}), pick: Some(None), status: None });
    }
    edit
}

/// The Net Classes window, from the schematic or the board (resizable): the class table, Width
/// from Current for the picked class, then that class's sizes and nets (as in Design Rules).
pub(crate) fn classes_window(app: &mut SolveApp, ctx: &egui::Context, mut w: ClassesWindow) {
    let b = board(app);
    let (rules, stack) = (b.rules.clone(), b.stackup_or_default());
    // Until the board has anything of its own, there is no stack-up to speak of: the copper
    // starts at 1 oz rather than "From stack-up".
    let has_board = b.has_content() || !b.stackup.is_empty() || b.copper_layers != solidcraft_engine::doc::ecad::layers::MIN_COPPER_LAYERS;
    let names: Vec<String> = crate::electronics_design::checked(app).nets.nets.iter().map(|n| n.name.clone()).collect();
    // A class renamed or deleted elsewhere is no longer picked.
    if w.list.class.as_deref().is_some_and(|c| rules.class(c).is_none()) {
        w.list.class = None;
    }
    // A class picked with a current of its own fills the calculator from it.
    if w.calc.from.as_ref() != Some(&w.list.class) {
        if let Some(c) = w.list.class.as_deref().and_then(|n| rules.class(n)) {
            fill(&mut w.calc, c);
        }
        w.calc.from = Some(w.list.class.clone());
    }
    let title = tr("electronics.rules_classes");
    let mut edit: Option<Edit> = None;
    let closed = ui_kit::Window::new(egui::Id::new("electronics_board_net_classes"), "list", &title)
        .placed(Align2::RIGHT_TOP, crate::electronics_design::beside(app, ctx))
        .resizable()
        .show_body(ctx, |ui| {
            let (e, picked) = class_table(ui, &rules, &names, &stack, &mut w.list);
            if e.is_some() {
                edit = e;
            }
            // Width from Current right under the table, in a section of its own, so it is in
            // reach before the class's long list of nets.
            let class = w.list.class.as_deref().and_then(|n| rules.class(n));
            let heading = tr("electronics.current_title");
            ui_kit::section(ui, "electronics_width_from_current", &heading, true, |ui| {
                if let Some(e) = width_from_current(ui, class, &stack, has_board, rules.min_track_width, &mut w.calc) {
                    edit = Some(e);
                }
            });
            match picked {
                Some(c) => {
                    if let Some(e) = class_fields(app, ui, &rules, &names, &stack, &mut w.list, &c) {
                        edit = Some(e);
                    }
                }
                None => {
                    ui_kit::text(ui, &tr("electronics.class_help_window"), ui_kit::Text::Note("electronics.class_help_window"));
                }
            }
        });
    if let Some(e) = edit {
        let status = e.status.clone();
        if app.run(e.command, e.params).is_ok() {
            if let Some(pick) = e.pick {
                w.list.class = pick;
                w.calc.from = Some(w.list.class.clone());
            }
            if let Some(text) = status {
                app.set_status(text, false);
            }
        }
    }
    app.electronics.design.open = (!closed).then_some(Open::NetClasses(w));
}

/// Fill the calculator from a class that keeps a current: its current, rise and copper.
fn fill(calc: &mut CurrentCalc, c: &NetClass) {
    let Some(i) = c.current else { return };
    calc.current = mm(i);
    calc.rise = mm(c.temp_rise.unwrap_or(DEFAULT_RISE));
    calc.copper = match c.copper_oz {
        None => Copper::Stack,
        Some(oz) if OUNCES.iter().any(|o| (o - oz).abs() < 1e-9) => Copper::Oz(oz),
        Some(oz) => {
            calc.copper_mm = mm((current::oz_to_mm(oz) * 1e4).round() / 1e4);
            Copper::Mm
        }
    };
}

/// What Width from Current works out.
struct Worked {
    current: f64,
    outer: Capacity,
    inner: Capacity,
    temperature: f64,
    /// With a length: its millimetres and the track's loss, on the layer picked.
    loss: Option<(f64, Loss)>,
    /// The class's copper as Use keeps it: `None` for the stack-up's.
    copper_oz: Option<f64>,
}

/// A calculator error in the UI's words.
fn current_error(e: CurrentError) -> String {
    let args = e.args();
    let pairs: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
    crate::i18n::format(e.key(), &pairs)
}

/// A field's number; why not in the UI's words.
fn typed_number(text: &str, label: &str) -> Result<f64, String> {
    crate::prefs::parse_number(text).ok_or_else(|| crate::i18n::format("electronics.not_a_number", &[("field", label), ("text", text)]))
}

/// The calculator's answer for its fields, on the copper `copper` resolves to (the stack-up's
/// thinnest outer and inner copper for [`Copper::Stack`]).
fn work(calc: &CurrentCalc, copper: Copper, stack: &[StackLayer]) -> Result<Worked, String> {
    let i = typed_number(&calc.current, &tr("electronics.field_current"))?;
    let rise = typed_number(&calc.rise, &tr("electronics.field_temp_rise"))?;
    let ambient = typed_number(&calc.ambient, &tr("electronics.field_ambient"))?;
    if !AMBIENT.contains(&ambient) {
        return Err(current_error(CurrentError::Ambient));
    }
    let (t_outer, t_inner, copper_oz) = match copper {
        Copper::Stack => {
            let (o, n) = current::stack_copper(stack);
            (o, n, None)
        }
        Copper::Oz(oz) => (current::oz_to_mm(oz), current::oz_to_mm(oz), Some(oz)),
        Copper::Mm => {
            let t = typed_number(&calc.copper_mm, &tr("electronics.field_copper_mm"))?;
            (t, t, Some(current::mm_to_oz(t)))
        }
    };
    let outer = current::width_for(i, rise, t_outer, true).map_err(current_error)?;
    let inner = current::width_for(i, rise, t_inner, false).map_err(current_error)?;
    let temperature = ambient + rise;
    let loss = if calc.length.trim().is_empty() {
        None
    } else {
        let length = typed_number(&calc.length, &tr("electronics.field_length"))?;
        let on = if calc.inner { inner } else { outer };
        Some((length, current::loss(i, on.width, on.copper, length, temperature).map_err(current_error)?))
    };
    Ok(Worked { current: i, outer, inner, temperature, loss, copper_oz })
}

/// A value with an SI prefix and three significant figures: `12.3 mΩ`, `1.05 V`.
fn si(v: f64, unit: &str) -> String {
    let (scaled, prefix) = match v.abs() {
        a if a >= 1.0 || a == 0.0 => (v, ""),
        a if a >= 1e-3 => (v * 1e3, "m"),
        _ => (v * 1e6, "µ"),
    };
    let digits = match scaled.abs() {
        a if a >= 100.0 => 0,
        a if a >= 10.0 => 1,
        _ => 2,
    };
    format!("{scaled:.digits$} {prefix}{unit}")
}

/// Width from Current for `class` (the default class's width without one): the current, the rise
/// (with presets), the ambient, the copper, the layer and an optional length; the widths on outer
/// and inner layers and, on the layer picked, the temperature, resistance, drop and loss, worked
/// out as they are typed; then Use outer width and Use inner width (never narrower than the rules'
/// least track width `min`, which the check holds every track to), and Forget Current for a class
/// that keeps one.
fn width_from_current(
    ui: &mut Ui,
    class: Option<&NetClass>,
    stack: &[StackLayer],
    has_board: bool,
    min: f64,
    calc: &mut CurrentCalc,
) -> Option<Edit> {
    let mut edit = None;
    ui_kit::text_row(ui, &tr("electronics.field_current"), &mut calc.current, "2");
    ui_kit::text_row(ui, &tr("electronics.field_temp_rise"), &mut calc.rise, "10");
    ui.horizontal_wrapped(|ui| {
        for r in RISES {
            let on = crate::prefs::parse_number(&calc.rise).is_some_and(|v| (v - r).abs() < 1e-9);
            if ui_kit::choice_chip(ui, &format!("current:rise:{r}"), &format!("{} °C", mm(r)), on).clicked() {
                calc.rise = mm(r);
            }
        }
    });
    ui_kit::text_row(ui, &tr("electronics.field_ambient"), &mut calc.ambient, "25");
    // Without a board of its own the stack-up says nothing: 1 oz until another copper is picked.
    let copper = match calc.copper {
        Copper::Stack if !has_board => Copper::Oz(1.0),
        c => c,
    };
    let (stack_outer, stack_inner) = current::stack_copper(stack);
    let stack_name = crate::i18n::format("electronics.current_copper_stack", &[("outer", &mm(stack_outer)), ("inner", &mm(stack_inner))]);
    let oz_name =
        |oz: f64| crate::i18n::format("electronics.current_copper_oz", &[("oz", &mm(oz)), ("mm", &mm((current::oz_to_mm(oz) * 1e4).round() / 1e4))]);
    let shown = match copper {
        Copper::Stack => stack_name.clone(),
        Copper::Oz(oz) => oz_name(oz),
        Copper::Mm => tr("electronics.current_copper_mm"),
    };
    ui_kit::row(ui, &tr("electronics.field_copper"), |ui| {
        ui_kit::dropdown_menu(ui, "electronics_current_copper", &shown, |ui| {
            // Each entry is the handle `dialog:electronics_current_copper:<stack|oz|mm>`.
            let mut entry = |ui: &mut Ui, on: bool, text: &str, key: &str, pick: Copper| {
                let r = ui_kit::list_picker(ui, &format!("current_copper_{key}"), text, on);
                crate::scenario::publish_handle(&format!("dialog:electronics_current_copper:{key}"), r.rect.center());
                if r.clicked() {
                    calc.copper = pick;
                }
            };
            if has_board {
                entry(ui, copper == Copper::Stack, &stack_name, "stack", Copper::Stack);
            }
            for oz in OUNCES {
                entry(ui, copper == Copper::Oz(oz), &oz_name(oz), &mm(oz), Copper::Oz(oz));
            }
            entry(ui, copper == Copper::Mm, &tr("electronics.current_copper_mm"), "mm", Copper::Mm);
        });
    });
    if copper == Copper::Mm {
        ui_kit::text_row(ui, &tr("electronics.field_copper_mm"), &mut calc.copper_mm, "0.035");
    }
    ui_kit::wrapped_row(ui, &tr("electronics.field_layer"), |ui| {
        for (inner, key, name) in [(false, "outer", "electronics.current_outer"), (true, "inner", "electronics.current_inner")] {
            if ui_kit::choice_chip(ui, &format!("current:layer:{key}"), &tr(name), calc.inner == inner).clicked() {
                calc.inner = inner;
            }
        }
    });
    ui_kit::text_row(ui, &tr("electronics.field_length"), &mut calc.length, &tr("electronics.current_length_hint"));
    let worked = work(calc, copper, stack);
    let x = match &worked {
        Ok(x) => x,
        Err(e) => {
            ui_kit::text(ui, e, ui_kit::Text::Error("electronics.current_error"));
            return None;
        }
    };
    let widths = crate::i18n::format("electronics.current_widths", &[("outer", &width_text(x.outer.width)), ("inner", &width_text(x.inner.width))]);
    ui_kit::text(ui, &widths, ui_kit::Text::Heading("electronics.current_widths"));
    let on = if calc.inner { x.inner } else { x.outer };
    let copper_text = crate::i18n::format(
        "electronics.current_copper_used",
        &[
            ("outer", &mm((x.outer.copper * 1e4).round() / 1e4)),
            ("inner", &mm((x.inner.copper * 1e4).round() / 1e4)),
            ("temperature", &mm((x.temperature * 10.0).round() / 10.0)),
        ],
    );
    ui_kit::text(ui, &copper_text, ui_kit::Text::Note("electronics.current_copper_used"));
    if let Some((length, l)) = x.loss {
        let key = if calc.inner { "electronics.current_loss_inner" } else { "electronics.current_loss_outer" };
        let text = crate::i18n::format(
            key,
            &[("length", &mm(length)), ("resistance", &si(l.resistance, "Ω")), ("drop", &si(l.voltage_drop, "V")), ("power", &si(l.power, "W"))],
        );
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.current_loss"));
    }
    let formula = crate::i18n::format("electronics.impedance_formula", &[("formula", FORMULA)]);
    ui_kit::text(ui, &formula, ui_kit::Text::Note("electronics.current_formula"));
    if !on.valid_range {
        ui_kit::text(ui, &tr("electronics.current_out_of_range"), ui_kit::Text::Error("electronics.current_out_of_range"));
    }
    let name = class.map_or_else(|| tr("electronics.class_default"), |c| c.name.clone());
    for (key, label, cap) in [("outer", "electronics.current_use_outer", x.outer), ("inner", "electronics.current_use_inner", x.inner)] {
        let width = proposed(cap.width).max(min);
        let fits = RULE_TRACK.contains(&width);
        if crate::electronics_editor::button(ui, &format!("current-use-{key}"), &tr(label), fits) {
            let status = Some(crate::i18n::format("electronics.current_used", &[("class", &name), ("width", &mm(width))]));
            edit = Some(match class {
                // The class keeps why it has its width.
                Some(c) => Edit {
                    status,
                    ..Edit::class(
                        &c.name,
                        json!({"track_width": width, "current": x.current, "temp_rise": cap.rise, "copper_oz": x.copper_oz.map(|o| (o * 1e4).round() / 1e4)}),
                    )
                },
                None => Edit { command: "ecad.board.rules", params: json!({"track_width": width}), pick: None, status },
            });
        }
        if !fits {
            let text = crate::i18n::format("electronics.current_too_wide", &[("width", &width_text(cap.width)), ("max", &mm(*RULE_TRACK.end()))]);
            ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.current_too_wide"));
        } else if proposed(cap.width) < min {
            let text = crate::i18n::format("electronics.current_min_width", &[("width", &width_text(cap.width)), ("min", &mm(min))]);
            ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.current_min_width"));
        }
    }
    if let Some(c) = class.filter(|c| c.current.is_some())
        && crate::electronics_editor::button(ui, "current-forget", &tr("electronics.current_forget"), true)
    {
        edit = Some(Edit::class(&c.name, json!({"current": null, "temp_rise": null, "copper_oz": null})));
    }
    let help = if class.is_some() { "electronics.current_help" } else { "electronics.current_help_default" };
    ui_kit::text(ui, &tr(help), ui_kit::Text::Note(help));
    edit
}

/// A class name made from a net's: at most [`MAX_CLASS_NAME`] characters with room for a count,
/// no spaces round it, not the default class's, and free of the board's classes.
pub(crate) fn class_name_for(net: &str, rules: &Rules) -> String {
    let base: String = net.trim().chars().filter(|c| !c.is_control()).take(MAX_CLASS_NAME.saturating_sub(4)).collect();
    let base = base.trim();
    let base = if base.is_empty() || base.eq_ignore_ascii_case(DEFAULT_CLASS) { tr("electronics.class_new") } else { base.to_string() };
    let taken: Vec<String> = rules.net_classes.iter().map(|c| c.name.clone()).chain([DEFAULT_CLASS.to_string()]).collect();
    crate::electronics_editor::free_from(&base, &taken)
}
