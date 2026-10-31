//! The design's intent in the UI (`ecad.intent`: what the nets and parts are for, see
//! `solidcraft_engine::doc::ecad::intent`). Design Intent, on the schematic's and the board's
//! RULES groups, opens a window with the board's facts (its radio bands, prototype or production,
//! IPC class, enclosure, hand assembly) and three lists: the roles still to confirm (the blocking
//! ones the checks lean on, each a look at its evidence, one by one), the low-risk roles the engine
//! proposes (accepted in one go), and the records kept. A click on a row shows its evidence and
//! facts and lets its role (and a supply's voltage and current, a connector's cable) be changed
//! before it is confirmed; a kept record can be removed. Each change is one `ecad.intent.set`,
//! made as the user's. The proposals come from `infer` (the schematic, the library and the
//! board), worked out once for each version of the document; nothing is kept until accepted.
//! The schematic's net Inspector shows the net's role from here, and opens the window on it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use egui::{Align2, Ui};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::Actor;
use solidcraft_engine::doc::ecad::infer::{self, Coverage, Inference, Proposal, Record};
use solidcraft_engine::doc::ecad::intent::{Band, BoardFacts, Confidence, Enclosure, Intent, NetRole, PartRole, Product, Source};

use crate::SolveApp;
use crate::electronics_design::Open;
use crate::electronics_editor::{mm, tr};
use crate::theme::Tokens;
use crate::ui_kit;

/// The most rows the window lists in one list.
const MAX_ROWS: usize = 300;
/// The most pieces of evidence and facts a picked row shows.
const MAX_LINES: usize = 12;

/// Every net role, in the order the role picker lists them.
const NET_ROLES: [NetRole; 16] = [
    NetRole::Power,
    NetRole::Ground,
    NetRole::Rf,
    NetRole::Diff,
    NetRole::Clock,
    NetRole::Strobe,
    NetRole::Fast,
    NetRole::Interface,
    NetRole::Analog,
    NetRole::Sensitive,
    NetRole::Slow,
    NetRole::Static,
    NetRole::Hv,
    NetRole::SwNode,
    NetRole::Fb,
    NetRole::Sense,
];

/// Every part role, in the order the role picker lists them.
const PART_ROLES: [PartRole; 30] = [
    PartRole::Decoupling,
    PartRole::Bulk,
    PartRole::Crystal,
    PartRole::LoadCap,
    PartRole::Tvs,
    PartRole::EsdArray,
    PartRole::SeriesTerm,
    PartRole::ParallelTerm,
    PartRole::PowerIc,
    PartRole::SmpsIc,
    PartRole::SmpsInCap,
    PartRole::SmpsOutCap,
    PartRole::SmpsInductor,
    PartRole::SmpsDiode,
    PartRole::FbDivider,
    PartRole::SenseResistor,
    PartRole::Ferrite,
    PartRole::Filter,
    PartRole::Connector,
    PartRole::Antenna,
    PartRole::RfIc,
    PartRole::RfMatch,
    PartRole::Module,
    PartRole::TestPoint,
    PartRole::Debug,
    PartRole::Fiducial,
    PartRole::Mounting,
    PartRole::Hot,
    PartRole::Sensitive,
    PartRole::Mechanical,
];

/// The radio bands the board facts offer: a name, and the band in MHz.
const BANDS: [(&str, f64, f64); 3] = [("ISM 2.4 GHz", 2400.0, 2483.5), ("EU868", 863.0, 870.0), ("US915", 902.0, 928.0)];

/// What the engine proposes for the design as it is now, and how much of the blocking intent is
/// settled.
pub struct Inferred {
    pub inference: Inference,
    pub coverage: Coverage,
}

/// The proposals and the coverage for the document as it is now; worked out again only after
/// it changes.
pub(crate) fn inferred(app: &mut SolveApp) -> Arc<Inferred> {
    let doc = &app.session.doc;
    if let Some((was, c)) = &app.electronics.design.inferred
        && std::ptr::eq(was.as_ptr(), Arc::as_ptr(doc))
    {
        return c.clone();
    }
    let inference = infer::infer(&doc.ecad);
    let coverage = infer::coverage(&doc.ecad, &doc.ecad.intent, &inference);
    let c = Arc::new(Inferred { inference, coverage });
    app.electronics.design.inferred = Some((Arc::downgrade(doc), c.clone()));
    c
}

/// The window's lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    /// The blocking roles not settled yet: proposals to look at, records to finish.
    #[default]
    Confirm,
    /// The low-risk proposals not kept yet.
    LowRisk,
    /// The records kept.
    Kept,
}

/// The picked row's fields as typed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edit {
    /// The role's name (`power`, `decoupling`); empty while none is picked (a net declared afresh).
    pub role: String,
    /// A supply's volts and amperes.
    pub voltage: String,
    pub current: String,
    /// A connector: whether a cable plugs into it.
    pub cable: Option<bool>,
    pub note: String,
}

/// The Design Intent window: the list shown, its search and role filter, the row picked (by key)
/// with its fields, and a net to declare a role for (one with no proposal or record).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IntentWindow {
    pub tab: Tab,
    pub search: String,
    /// Only rows of this role (Check Board's lines).
    pub role: Option<String>,
    pub picked: Option<String>,
    pub edit: Edit,
    pub declare: Option<String>,
    /// Scroll the picked row into view once (opened on it from elsewhere).
    pub reveal: bool,
}

/// A role's name in the UI's words.
pub(crate) fn role_name(role: &str) -> String {
    let key = format!("electronics.role_{role}");
    let text = tr(&key);
    if text == key { role.replace('_', " ") } else { text }
}

fn source_name(s: Source) -> String {
    tr(match s {
        Source::Declared => "electronics.intent_source_declared",
        Source::Inferred => "electronics.intent_source_inferred",
        Source::Confirmed => "electronics.intent_source_confirmed",
    })
}

/// How a record came to be kept: `Confirmed by you`.
fn kept_text(source: Source, by: Option<Actor>) -> String {
    let key = if by == Some(Actor::Agent) { "electronics.intent_by_agent" } else { "electronics.intent_by_you" };
    crate::i18n::format(key, &[("source", &source_name(source))])
}

fn confidence_name(c: Confidence) -> String {
    tr(match c {
        Confidence::High => "electronics.intent_confidence_high",
        Confidence::Medium => "electronics.intent_confidence_medium",
        Confidence::Low => "electronics.intent_confidence_low",
    })
}

/// What a role still lacks, in the UI's words.
pub(crate) fn needs_text(needs: &[&str]) -> String {
    let words: Vec<String> = needs
        .iter()
        .map(|n| {
            let key = format!("electronics.intent_needs_{n}");
            let text = tr(&key);
            if text == key { n.replace('_', " ") } else { text }
        })
        .collect();
    crate::i18n::format("electronics.intent_needs", &[("needs", &words.join(", "))])
}

/// What a record is about, as its row names it.
fn subject(r: &Record) -> String {
    match r {
        Record::Net(x) => x.net.clone(),
        Record::Part(x) => x.part.clone(),
        Record::Pair(x) => x.name.clone(),
        Record::Loop(x) => x.name.clone(),
        Record::Board(_) => tr("electronics.intent_board"),
    }
}

/// A record's source, who made it, its evidence and its note.
fn meta(r: &Record) -> (Source, Option<Actor>, &[String], &str) {
    match r {
        Record::Net(x) => (x.source, x.by, x.evidence.as_slice(), x.note.as_str()),
        Record::Part(x) => (x.source, x.by, x.evidence.as_slice(), x.note.as_str()),
        Record::Pair(x) => (x.source, x.by, x.evidence.as_slice(), x.note.as_str()),
        Record::Loop(x) => (x.source, x.by, x.evidence.as_slice(), x.note.as_str()),
        Record::Board(x) => (Source::Declared, x.by, &[], x.note.as_str()),
    }
}

fn interface_name(name: &str) -> String {
    let key = format!("electronics.interface_{name}");
    let text = tr(&key);
    if text == key { name.to_uppercase() } else { text }
}

/// A record's facts, one line each: a supply's voltage and current, what a part serves, a pair's
/// nets, a loop's members.
fn facts(r: &Record) -> Vec<String> {
    let f = |key: &str, args: &[(&str, &str)]| crate::i18n::format(key, args);
    let mut out = Vec::new();
    match r {
        Record::Net(x) => {
            if let Some(v) = x.voltage {
                out.push(f("electronics.intent_fact_voltage", &[("v", &mm(v.nominal))]));
            }
            if let Some(a) = x.current_a {
                out.push(f("electronics.intent_fact_current", &[("a", &mm(a))]));
            }
            if let Some(i) = x.interface {
                out.push(f("electronics.intent_fact_interface", &[("name", &interface_name(i.name()))]));
            }
            if let Some(m) = x.f_mhz {
                out.push(f("electronics.intent_fact_frequency", &[("f", &mm(m))]));
            }
            if let Some(z) = x.z_target {
                out.push(f("electronics.intent_fact_impedance", &[("z", &mm(z))]));
            }
            if x.external == Some(true) {
                out.push(tr("electronics.intent_fact_external"));
            }
        }
        Record::Part(x) => {
            if !x.of.is_empty() {
                out.push(f("electronics.intent_fact_of", &[("of", &x.of.join(", "))]));
            }
            match x.cable {
                Some(true) => out.push(tr("electronics.intent_fact_cable_yes")),
                Some(false) => out.push(tr("electronics.intent_fact_cable_no")),
                None => {}
            }
            if let Some(l) = &x.in_loop {
                out.push(f("electronics.intent_fact_loop", &[("name", l)]));
            }
            if let Some(w) = x.power_w {
                out.push(f("electronics.intent_fact_power", &[("w", &mm(w))]));
            }
        }
        Record::Pair(x) => {
            out.push(f("electronics.intent_fact_pair", &[("p", &x.p), ("n", &x.n)]));
            if let Some(i) = x.interface {
                out.push(f("electronics.intent_fact_interface", &[("name", &interface_name(i.name()))]));
            }
            if let Some(s) = x.skew_mm {
                out.push(f("electronics.intent_fact_skew", &[("mm", &mm(s))]));
            }
        }
        Record::Loop(x) => out.push(f("electronics.intent_fact_members", &[("members", &x.members.join(", "))])),
        Record::Board(_) => {}
    }
    out
}

/// Every record `intent` keeps, by key.
fn kept_records(intent: &Intent) -> BTreeMap<String, Record> {
    let nets = intent.nets.iter().cloned().map(Record::Net);
    let parts = intent.parts.iter().cloned().map(Record::Part);
    let pairs = intent.pairs.iter().cloned().map(Record::Pair);
    let loops = intent.loops.iter().cloned().map(Record::Loop);
    nets.chain(parts).chain(pairs).chain(loops).map(|r| (r.key(), r)).collect()
}

/// One row of a list.
struct Row<'a> {
    key: String,
    subject: String,
    role: &'static str,
    proposal: Option<&'a Proposal>,
    kept: Option<&'a Record>,
    /// What it still lacks before the gate takes it (the to-confirm list's).
    needs: Vec<&'static str>,
    /// It names a net or part the schematic doesn't have.
    unknown: bool,
}

impl Row<'_> {
    /// The record as it is kept, or as it is proposed.
    fn record(&self) -> Option<&Record> {
        self.kept.or(self.proposal.map(|p| &p.record))
    }

    /// Its status line: how it was kept, or how sure the proposal is, and what it lacks.
    fn status(&self) -> String {
        let mut parts = Vec::new();
        match (self.kept, self.proposal) {
            (Some(r), _) => {
                let (source, by, ..) = meta(r);
                parts.push(kept_text(source, by));
            }
            (None, Some(p)) => parts.push(crate::i18n::format("electronics.intent_proposed", &[("confidence", &confidence_name(p.confidence))])),
            (None, None) => {}
        }
        if self.unknown {
            parts.push(tr("electronics.intent_unknown"));
        }
        if !self.needs.is_empty() {
            parts.push(needs_text(&self.needs));
        }
        parts.join(" · ")
    }
}

/// The rows of `tab` that the search and the role filter keep; the low-risk ones are those
/// Accept takes (a regulator's parts once the regulator is confirmed).
fn rows<'a>(tab: Tab, inf: &'a Inferred, intent: &Intent, kept: &'a BTreeMap<String, Record>, w: &IntentWindow) -> Vec<Row<'a>> {
    let unknown: BTreeSet<&str> = inf.coverage.unknown.iter().map(String::as_str).collect();
    let row = |key: &str, needs: Vec<&'static str>| -> Option<Row<'a>> {
        let proposal = inf.inference.get(key);
        let kept = kept.get(key);
        let record = kept.or(proposal.map(|p| &p.record))?;
        Some(Row { key: key.to_string(), subject: subject(record), role: record.role(), proposal, kept, needs, unknown: unknown.contains(key) })
    };
    let all: Vec<Row<'a>> = match tab {
        // The board's facts have their own section.
        Tab::Confirm => inf.coverage.missing.iter().filter(|m| m.key != "board").filter_map(|m| row(&m.key, m.needs.clone())).collect(),
        Tab::LowRisk => inf.inference.low_risk(intent).into_iter().filter_map(|p| row(&p.key, Vec::new())).collect(),
        Tab::Kept => kept.keys().filter_map(|k| row(k, Vec::new())).collect(),
    };
    let search = w.search.trim().to_lowercase();
    all.into_iter()
        .filter(|r| w.role.as_deref().is_none_or(|x| x == r.role))
        .filter(|r| search.is_empty() || r.subject.to_lowercase().contains(&search) || role_name(r.role).to_lowercase().contains(&search))
        .collect()
}

/// The picked row's fields as the record has them.
fn edit_of(r: &Record) -> Edit {
    let (.., note) = meta(r);
    let mut e = Edit { role: r.role().to_string(), note: note.to_string(), ..Edit::default() };
    match r {
        Record::Net(x) => {
            e.voltage = x.voltage.map(|v| mm(v.nominal)).unwrap_or_default();
            e.current = x.current_a.map(mm).unwrap_or_default();
        }
        Record::Part(x) => e.cable = x.cable,
        _ => {}
    }
    e
}

/// What the window was asked to do.
enum Act {
    Tab(Tab),
    Pick(String),
    /// Keep the picked row's record with the fields typed (confirmed, or declared when its role
    /// changed).
    Save(String),
    Remove(String),
    LowRisk,
    Board(Value),
    /// Keep the role typed for the net being declared.
    Declare(String),
    ClearRole,
}

/// The record `r` (kept, or proposed) with the fields typed in `e`, as `ecad.intent.set` takes it
/// under the list it names; why not, in the UI's words.
fn edited(r: &Record, e: &Edit, kept: bool) -> Result<(&'static str, Value), String> {
    let (list, mut v) = match r {
        Record::Net(x) => ("nets", serde_json::to_value(x)),
        Record::Part(x) => ("parts", serde_json::to_value(x)),
        Record::Pair(x) => ("pairs", serde_json::to_value(x)),
        Record::Loop(x) => ("loops", serde_json::to_value(x)),
        Record::Board(_) => return Err(tr("electronics.intent_cant_save")),
    };
    let v = v.as_mut().map_err(|_| tr("electronics.intent_cant_save"))?;
    let Some(obj) = v.as_object_mut() else { return Err(tr("electronics.intent_cant_save")) };
    // The engine sets who and drops the proposal's confidence.
    obj.remove("by");
    obj.remove("confidence");
    let role_changed = matches!(r, Record::Net(_) | Record::Part(_)) && !e.role.is_empty() && e.role != r.role();
    if role_changed {
        obj.insert("role".into(), json!(e.role));
        // What the old role served and why it was proposed don't hold for the new one.
        obj.remove("of");
        obj.remove("evidence");
    }
    // A role changed, or a declared record kept, is declared; a proposal looked at is confirmed.
    let declared = role_changed || (kept && meta(r).0 == Source::Declared);
    obj.insert("source".into(), json!(if declared { "declared" } else { "confirmed" }));
    let number = |text: &str, label: &str| -> Result<Option<f64>, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        crate::prefs::parse_number(text).map(Some).ok_or_else(|| crate::i18n::format("electronics.not_a_number", &[("field", label), ("text", text)]))
    };
    let role = if e.role.is_empty() { r.role() } else { e.role.as_str() };
    if matches!(r, Record::Net(_)) && role == NetRole::Power.name() {
        if let Some(volts) = number(&e.voltage, &tr("electronics.field_voltage"))? {
            let peak = obj.get("voltage").and_then(|x| x.get("peak")).cloned();
            let mut voltage = json!({"nominal": volts});
            if let Some(p) = peak.filter(|p| !p.is_null()) {
                voltage["peak"] = p;
            }
            obj.insert("voltage".into(), voltage);
        }
        if let Some(amps) = number(&e.current, &tr("electronics.field_current"))? {
            obj.insert("current_a".into(), json!(amps));
        }
    }
    if matches!(r, Record::Part(_)) {
        if role != PartRole::Connector.name() {
            obj.remove("cable");
        } else if let Some(c) = e.cable {
            obj.insert("cable".into(), json!(c));
        }
    }
    let note = e.note.trim();
    if note.is_empty() {
        obj.remove("note");
    } else {
        obj.insert("note".into(), json!(note));
    }
    Ok((list, v.clone()))
}

/// Open the window on the record or proposal `key` (`net|+3V3`): in the list that holds it, picked;
/// a net with neither is offered to declare.
pub(crate) fn show(app: &mut SolveApp, key: &str) {
    let inf = inferred(app);
    let kept = kept_records(&app.session.doc.ecad.intent);
    let missing = inf.coverage.missing.iter().any(|m| m.key == key);
    let proposal = inf.inference.get(key);
    let tab = if missing {
        Tab::Confirm
    } else if kept.contains_key(key) {
        Tab::Kept
    } else if proposal.is_some_and(|p| !p.blocking) {
        Tab::LowRisk
    } else {
        Tab::Kept
    };
    let record = kept.get(key).or(proposal.map(|p| &p.record));
    let declare = match record {
        None => key.strip_prefix("net|").map(str::to_string),
        Some(_) => None,
    };
    let edit = record.map(edit_of).unwrap_or_default();
    open_with(app, IntentWindow { tab, picked: record.map(|_| key.to_string()), edit, declare, reveal: true, ..IntentWindow::default() });
}

/// Open the window on the roles of `role` still to confirm (Check Board's intent lines).
pub(crate) fn show_role(app: &mut SolveApp, role: &str) {
    open_with(app, IntentWindow { tab: Tab::Confirm, role: Some(role.to_string()), ..IntentWindow::default() });
}

/// What a record's key (`net|VSYS`, `part|C22`, `board`) is about, for the status line.
fn subject_of(key: &str) -> String {
    match key.split_once('|') {
        Some((_, what)) => what.to_string(),
        None => tr("electronics.intent_board_facts"),
    }
}

fn open_with(app: &mut SolveApp, w: IntentWindow) {
    app.electronics.board.stack_open = false;
    app.electronics.design.open = Some(Open::Intent(w));
    app.electronics.sheet.choosing = false;
}

/// The schematic net Inspector's role line for `net`: its kept role and how it was kept, its
/// proposed role, or none yet.
pub(crate) fn net_role(app: &mut SolveApp, net: &str) -> String {
    if let Some(r) = app.session.doc.ecad.intent.net(net) {
        return format!("{} · {}", role_name(r.role.name()), kept_text(r.source, r.by));
    }
    let inf = inferred(app);
    match inf.inference.get(&format!("net|{net}")) {
        Some(p) => crate::i18n::format("electronics.intent_role_proposed", &[("role", &role_name(p.record.role()))]),
        None => tr("electronics.intent_no_role"),
    }
}

/// The Design Intent window (see the module notes).
pub(crate) fn window(app: &mut SolveApp, ctx: &egui::Context, mut w: IntentWindow) {
    let inf = inferred(app);
    let intent = app.session.doc.ecad.intent.clone();
    let kept = kept_records(&intent);
    let low_risk = inf.inference.low_risk(&intent).len();
    let to_confirm = inf.coverage.missing.iter().filter(|m| m.key != "board").count();
    let title = tr("electronics.intent_title");
    let mut act: Option<Act> = None;
    let closed = ui_kit::Window::new(egui::Id::new("electronics_intent"), "inspect", &title)
        .placed(Align2::RIGHT_TOP, crate::electronics_design::beside(app, ctx))
        .resizable()
        .show_body(ctx, |ui| {
            let c = &inf.coverage;
            let summary = crate::i18n::format(
                "electronics.intent_coverage",
                &[("confirmed", &c.blocking_confirmed.to_string()), ("total", &c.blocking_total.to_string())],
            );
            let style = if c.blocking_confirmed < c.blocking_total {
                ui_kit::Text::Error("electronics.intent_coverage")
            } else {
                ui_kit::Text::Note("electronics.intent_coverage")
            };
            ui_kit::text(ui, &summary, style);
            if c.ic_power_pins > 0 {
                let pins = crate::i18n::format(
                    "electronics.intent_power_pins",
                    &[
                        ("assigned", &c.ic_power_pins_assigned.to_string()),
                        ("total", &c.ic_power_pins.to_string()),
                        ("proposed", &c.ic_power_pins_proposed.to_string()),
                    ],
                );
                ui_kit::text(ui, &pins, ui_kit::Text::Note("electronics.intent_power_pins"));
            }
            if !c.unknown.is_empty() {
                let names: Vec<&str> = c.unknown.iter().take(8).map(|k| k.split_once('|').map_or(k.as_str(), |(_, n)| n)).collect();
                let text =
                    crate::i18n::format("electronics.intent_unknown_names", &[("n", &c.unknown.len().to_string()), ("names", &names.join(", "))]);
                ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.intent_unknown_names"));
            }
            ui_kit::section(ui, "electronics_intent_board", &tr("electronics.intent_board"), true, |ui| {
                if let Some(v) = board_facts(ui, &intent.board, &inf) {
                    act = Some(Act::Board(v));
                }
            });
            if let Some(net) = w.declare.clone() {
                let heading = crate::i18n::format("electronics.intent_declare", &[("net", &net)]);
                ui_kit::section(ui, ("electronics_intent_declare", net.as_str()), &heading, true, |ui| {
                    role_picker(ui, "electronics_intent_declare_role", false, &mut w.edit.role);
                    ui_kit::text_row(ui, &tr("electronics.checks_note"), &mut w.edit.note, &tr("electronics.intent_note_hint"));
                    if crate::electronics_editor::button(ui, "intent-declare", &tr("electronics.intent_save"), !w.edit.role.is_empty()) {
                        act = Some(Act::Declare(net.clone()));
                    }
                });
            }
            ui.add_space(Tokens::get().space_6);
            ui.horizontal_wrapped(|ui| {
                for (tab, key, label, n) in [
                    (Tab::Confirm, "confirm", "electronics.intent_tab_confirm", to_confirm),
                    (Tab::LowRisk, "low_risk", "electronics.intent_tab_low_risk", low_risk),
                    (Tab::Kept, "kept", "electronics.intent_tab_kept", kept.len()),
                ] {
                    let text = format!("{} ({n})", tr(label));
                    if ui_kit::choice_chip(ui, &format!("intent:tab:{key}"), &text, w.tab == tab).clicked() {
                        act = Some(Act::Tab(tab));
                    }
                }
            });
            if w.tab == Tab::LowRisk
                && low_risk > 0
                && crate::electronics_editor::button(
                    ui,
                    "intent-accept-low-risk",
                    &if low_risk == 1 {
                        tr("electronics.intent_accept_low_risk_one")
                    } else {
                        crate::i18n::format("electronics.intent_accept_low_risk", &[("n", &low_risk.to_string())])
                    },
                    true,
                )
            {
                act = Some(Act::LowRisk);
            }
            ui_kit::search_field(ui, egui::Id::new("electronics_intent_search"), &mut w.search, &tr("electronics.library_search"));
            if let Some(role) = w.role.clone() {
                let text = crate::i18n::format("electronics.intent_only_role", &[("role", &role_name(&role))]);
                ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.intent_only_role"));
                if crate::electronics_editor::button(ui, "intent-all-roles", &tr("electronics.drc_show_all"), true) {
                    act = Some(Act::ClearRole);
                }
            }
            let list = rows(w.tab, &inf, &intent, &kept, &w);
            if list.is_empty() {
                let note = match w.tab {
                    Tab::Confirm => "electronics.intent_none_confirm",
                    Tab::LowRisk => "electronics.intent_none_low_risk",
                    Tab::Kept => "electronics.intent_none_kept",
                };
                ui_kit::text(ui, &tr(note), ui_kit::Text::Note(note));
            }
            for row in list.iter().take(MAX_ROWS) {
                let selected = w.picked.as_deref() == Some(row.key.as_str());
                let text = format!("{} · {}", row.subject, role_name(row.role));
                let response = ui_kit::text(ui, &text, ui_kit::Text::NameCell(selected));
                if selected && w.reveal {
                    response.scroll_to_me(Some(egui::Align::TOP));
                    w.reveal = false;
                }
                crate::scenario::publish_target(&format!("electronics:intent:{}", row.key), response.rect);
                if response.clicked() {
                    act = Some(Act::Pick(row.key.clone()));
                }
                // Red only for what a look can't settle (a value only the product tells): a row
                // that needs just a look is the tab's ordinary business.
                let style = if row.needs.iter().all(|n| *n == "confirm") {
                    ui_kit::Text::Note("electronics.intent_status")
                } else {
                    ui_kit::Text::Error("electronics.intent_status")
                };
                ui_kit::text(ui, &row.status(), style);
                if selected && let Some(a) = picked_row(ui, row, &mut w.edit) {
                    act = Some(a);
                }
            }
            if list.len() > MAX_ROWS {
                let more = crate::i18n::format("electronics.checks_more", &[("n", &(list.len() - MAX_ROWS).to_string())]);
                ui_kit::text(ui, &more, ui_kit::Text::Note("electronics.checks_more"));
            }
            ui_kit::text(ui, &tr("electronics.intent_help"), ui_kit::Text::Note("electronics.intent_help"));
        });
    match act {
        Some(Act::Tab(tab)) => {
            w.tab = tab;
            w.picked = None;
        }
        Some(Act::Pick(key)) => {
            if w.picked.as_deref() != Some(key.as_str()) {
                let record = kept.get(&key).or(inf.inference.get(&key).map(|p| &p.record));
                w.edit = record.map(edit_of).unwrap_or_default();
                w.picked = Some(key);
            }
        }
        Some(Act::Save(key)) => {
            let record = kept.get(&key).or(inf.inference.get(&key).map(|p| &p.record));
            match record.map(|r| edited(r, &w.edit, kept.contains_key(&key))) {
                Some(Ok((list, v))) => {
                    let mut params = json!({});
                    params[list] = json!([v]);
                    if app.run_as_user("ecad.intent.set", params).is_ok() {
                        w.picked = None;
                        app.set_status(crate::i18n::format("electronics.intent_saved", &[("what", &subject_of(&key))]), false);
                    }
                }
                Some(Err(e)) => app.set_status(e, true),
                None => {}
            }
        }
        Some(Act::Remove(key)) => {
            if app.run_as_user("ecad.intent.set", json!({"remove": [key]})).is_ok() {
                w.picked = None;
                app.set_status(crate::i18n::format("electronics.intent_removed", &[("what", &subject_of(&key))]), false);
            }
        }
        Some(Act::LowRisk) => {
            if let Ok(reply) = app.run_as_user("ecad.intent.set", json!({"accept_low_risk": true})) {
                let n = reply.get("changed").and_then(Value::as_array).map_or(0, Vec::len);
                app.set_status(crate::i18n::format("electronics.intent_accepted", &[("n", &n.to_string())]), false);
            }
        }
        Some(Act::Board(v)) => drop(app.run_as_user("ecad.intent.set", json!({"board": v}))),
        Some(Act::Declare(net)) => {
            let mut record = json!({"net": net, "role": w.edit.role, "source": "declared"});
            if !w.edit.note.trim().is_empty() {
                record["note"] = json!(w.edit.note.trim());
            }
            if app.run_as_user("ecad.intent.set", json!({"nets": [record]})).is_ok() {
                w.declare = None;
                w.tab = Tab::Kept;
                w.picked = Some(format!("net|{net}"));
            }
        }
        Some(Act::ClearRole) => w.role = None,
        None => {}
    }
    app.electronics.design.open = (!closed).then_some(Open::Intent(w));
}

/// The role picker for a net (`part` false) or a part: a dropdown of the roles, `role` the one
/// picked.
fn role_picker(ui: &mut Ui, id: &str, part: bool, role: &mut String) {
    let names: Vec<&'static str> = if part { PART_ROLES.iter().map(|r| r.name()).collect() } else { NET_ROLES.iter().map(|r| r.name()).collect() };
    let shown = if role.is_empty() { tr("electronics.intent_pick_role") } else { role_name(role) };
    ui_kit::row(ui, crate::i18n::tr("electronics.field_role"), |ui| {
        // Each entry is the handle `dialog:<id>:<role>`.
        ui_kit::dropdown_menu(ui, id, &shown, |ui| {
            for n in names {
                let r = ui_kit::list_picker(ui, &format!("{id}_{n}"), &role_name(n), role == n);
                crate::scenario::publish_handle(&format!("dialog:{id}:{n}"), r.rect.center());
                if r.clicked() {
                    *role = n.to_string();
                }
            }
        });
    });
}

/// The picked row: its evidence and facts, its role and the fields its role asks for, its note,
/// then Confirm (Save for a settled record) and Remove for a kept one.
fn picked_row(ui: &mut Ui, row: &Row<'_>, e: &mut Edit) -> Option<Act> {
    let record = row.record()?;
    let mut act = None;
    let (_, _, evidence, _) = meta(record);
    let evidence: &[String] = if evidence.is_empty() { row.proposal.map_or(&[], |p| p.evidence.as_slice()) } else { evidence };
    if !evidence.is_empty() {
        ui_kit::text(ui, &tr("electronics.intent_evidence"), ui_kit::Text::Heading("electronics.intent_evidence"));
        for line in evidence.iter().take(MAX_LINES) {
            ui_kit::text(ui, &format!("• {line}"), ui_kit::Text::Note("electronics.intent_evidence_line"));
        }
    }
    for line in facts(record).into_iter().take(MAX_LINES) {
        ui_kit::text(ui, &line, ui_kit::Text::Note("electronics.intent_fact"));
    }
    // What only the product tells, asked in plain words.
    for need in &row.needs {
        let ask = match *need {
            "cable" => Some(crate::i18n::format("electronics.intent_ask_cable", &[("part", &row.subject)])),
            "current_a" => Some(crate::i18n::format("electronics.intent_ask_current", &[("net", &row.subject)])),
            _ => None,
        };
        if let Some(ask) = ask {
            ui_kit::text(ui, &ask, ui_kit::Text::Error("electronics.intent_ask"));
        }
    }
    let id = format!("electronics_intent_role_{}", row.key);
    match record {
        Record::Net(_) => role_picker(ui, &id, false, &mut e.role),
        Record::Part(_) => role_picker(ui, &id, true, &mut e.role),
        _ => {}
    }
    if matches!(record, Record::Net(_)) && e.role == NetRole::Power.name() {
        ui_kit::text_row(ui, &tr("electronics.field_voltage"), &mut e.voltage, "3.3");
        ui_kit::text_row(ui, &tr("electronics.field_current"), &mut e.current, "0.5");
    }
    if matches!(record, Record::Part(_)) && e.role == PartRole::Connector.name() {
        ui_kit::wrapped_row(ui, crate::i18n::tr("electronics.field_cable"), |ui| {
            for (on, key, label) in [(true, "yes", "electronics.intent_cable_yes"), (false, "no", "electronics.intent_cable_no")] {
                if ui_kit::choice_chip(ui, &format!("intent:cable:{key}"), crate::i18n::tr(label), e.cable == Some(on)).clicked() {
                    e.cable = Some(on);
                }
            }
        });
    }
    ui_kit::text_row(ui, &tr("electronics.checks_note"), &mut e.note, &tr("electronics.intent_note_hint"));
    let settled = row.kept.is_some_and(|r| meta(r).0.settled()) && row.needs.is_empty();
    let label = if settled { "electronics.intent_save" } else { "electronics.intent_confirm" };
    if crate::electronics_editor::button(ui, "intent-confirm", &tr(label), true) {
        act = Some(Act::Save(row.key.clone()));
    }
    if row.kept.is_some() && crate::electronics_editor::button(ui, "intent-remove", &tr("electronics.intent_remove"), true) {
        act = Some(Act::Remove(row.key.clone()));
    }
    act
}

/// The board's facts: its radio bands (as kept, else as proposed), prototype or production, IPC
/// class, enclosure and hand assembly. `Some` with the `board` facts to set when one changes.
fn board_facts(ui: &mut Ui, facts: &BoardFacts, inf: &Inferred) -> Option<Value> {
    let mut out = None;
    let proposal = inf.inference.get("board");
    let proposed: Vec<Band> = match proposal.map(|p| &p.record) {
        Some(Record::Board(b)) => b.bands.clone().unwrap_or_default(),
        _ => Vec::new(),
    };
    let kept = facts.bands.is_some();
    let bands: Vec<Band> = facts.bands.clone().unwrap_or_else(|| proposed.clone());
    let on: BTreeSet<&str> = bands.iter().map(|b| b.name.as_str()).collect();
    // The presets, then any other band kept.
    let mut offered: Vec<Band> = BANDS.iter().map(|(name, lo, hi)| Band { name: (*name).to_string(), lo_mhz: *lo, hi_mhz: *hi, rx: true }).collect();
    offered.extend(bands.iter().filter(|b| !BANDS.iter().any(|(n, ..)| *n == b.name)).cloned());
    let band_json = |list: &[Band]| -> Value { serde_json::to_value(list).unwrap_or_else(|_| json!([])) };
    ui_kit::wrapped_row(ui, crate::i18n::tr("electronics.field_bands"), |ui| {
        for b in &offered {
            let text = crate::i18n::format("electronics.intent_band", &[("name", &b.name), ("lo", &mm(b.lo_mhz)), ("hi", &mm(b.hi_mhz))]);
            if ui_kit::choice_chip(ui, &format!("intent:band:{}", b.name), &text, on.contains(b.name.as_str())).clicked() {
                let mut next: Vec<Band> = bands.iter().filter(|x| x.name != b.name).cloned().collect();
                if !on.contains(b.name.as_str()) {
                    next.push(b.clone());
                }
                out = Some(json!({"bands": band_json(&next)}));
            }
        }
        let none = facts.bands.as_ref().is_some_and(Vec::is_empty);
        if ui_kit::choice_chip(ui, "intent:band:none", crate::i18n::tr("electronics.intent_no_radio"), none).clicked() {
            out = Some(json!({"bands": []}));
        }
    });
    if !kept && let Some(p) = proposal {
        let note = if p.needs.is_empty() { "electronics.intent_bands_proposed" } else { "electronics.intent_bands_needed" };
        ui_kit::text(ui, &tr(note), ui_kit::Text::Error(note));
        for line in p.evidence.iter().take(MAX_LINES) {
            ui_kit::text(ui, &format!("• {line}"), ui_kit::Text::Note("electronics.intent_evidence_line"));
        }
        if p.needs.is_empty() && crate::electronics_editor::button(ui, "intent-bands-confirm", &tr("electronics.intent_bands_confirm"), true) {
            out = Some(json!({"bands": band_json(&proposed)}));
        }
    }
    let chips = |ui: &mut Ui, label: &str, key: &str, options: &[(&str, &str, bool)]| -> Option<String> {
        let mut picked = None;
        ui_kit::wrapped_row(ui, crate::i18n::tr(label), |ui| {
            for (value, name, on) in options {
                if ui_kit::choice_chip(ui, &format!("intent:{key}:{value}"), crate::i18n::tr(name), *on).clicked() {
                    picked = Some((*value).to_string());
                }
            }
        });
        picked
    };
    let product = facts.product;
    if let Some(v) = chips(
        ui,
        "electronics.field_product",
        "product",
        &[
            ("prototype", "electronics.intent_prototype", product == Some(Product::Prototype)),
            ("production", "electronics.intent_production", product == Some(Product::Production)),
        ],
    ) {
        out = Some(json!({"product": v}));
    }
    let class = facts.ipc_class;
    if let Some(v) = chips(
        ui,
        "electronics.field_ipc_class",
        "ipc_class",
        &[("2", "electronics.intent_ipc_2", class == Some(2)), ("3", "electronics.intent_ipc_3", class == Some(3))],
    ) {
        out = Some(json!({"ipc_class": if v == "3" { 3 } else { 2 }}));
    }
    let enclosure = facts.enclosure;
    if let Some(v) = chips(
        ui,
        "electronics.field_enclosure",
        "enclosure",
        &[
            ("none", "electronics.intent_enclosure_none", enclosure == Some(Enclosure::None)),
            ("plastic", "electronics.intent_enclosure_plastic", enclosure == Some(Enclosure::Plastic)),
            ("metal", "electronics.intent_enclosure_metal", enclosure == Some(Enclosure::Metal)),
        ],
    ) {
        out = Some(json!({"enclosure": v}));
    }
    let hand = facts.hand_assembly == Some(true);
    if ui_kit::checkbox_row(ui, "electronics_intent_hand_assembly", &tr("electronics.intent_hand_assembly"), hand) {
        out = Some(json!({"hand_assembly": !hand}));
    }
    if let Some(by) = facts.by {
        let who = if by == Actor::Agent { "electronics.intent_board_by_agent" } else { "electronics.intent_board_by_you" };
        ui_kit::text(ui, &tr(who), ui_kit::Text::Note("electronics.intent_board_by"));
    }
    out
}
