//! An electronics design document: the view its page shows (the schematic or the board, each with
//! its own camera), its Browser tree (the design and its views: the schematic, the board and the 3D
//! board still to come), and the windows beside the Inspector. Add Part lists the design's own
//! electronic components and those of every open electronics library; one from a library is
//! copied into the design first (`ecad.library.import`), then Place is armed with the copy. Netlist
//! lists the schematic's nets and Rename Net renames one. Electrical Check lists the electrical
//! rules check's findings (a click picks and frames what one is about; Approve takes a note), and
//! Export Netlist saves the netlist as a file. The board's Design Rules and DRC windows open in the
//! same place (`electronics_board_rules`), and so do Net Classes, Design Intent
//! (`electronics_intent`) and Check Board (`electronics_board_check`), which both views open (the
//! schematic's DESIGN tab and the board's RULES tab) and which stay open across them, as does
//! Export Pages (`electronics_export`, both views' OUTPUT group). Approvals
//! made here are the user's (`SolveApp::run_as_user`) and each says who made it. The sheet itself
//! is `electronics_schematic`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Weak};

use egui::{Align2, Ui, vec2};
use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_engine::doc::ecad::connect::Connectivity;
use solidcraft_engine::doc::ecad::erc::{self, Finding, Item, Report};
use solidcraft_engine::doc::ecad::schematic::{Pins, part_bbox};
use solidcraft_engine::doc::ecad::{Actor, Electronics, NetPin, Severity};
use solidcraft_engine::doc::{Document, DocumentKind};

use crate::SolveApp;
use crate::browser::Row;
use crate::electronics_editor::tr;
use crate::theme::Tokens;
use crate::ui_kit;

/// The most nets the Netlist window lists.
const MAX_NETS: usize = 2000;
/// The most findings the Electrical Check window lists under each heading.
const MAX_FINDINGS: usize = 500;
/// The least a finding is framed across, in millimetres.
const FRAME_MM: f64 = 20.0;

/// The window beside the Inspector (Add Part is the sheet's own, `Sheet::choosing`).
#[derive(Clone, Debug, PartialEq)]
pub enum Open {
    Netlist,
    /// The net being renamed, the new name, and whether to join a net that has it.
    Rename {
        net: String,
        to: String,
        merge: bool,
    },
    /// The Electrical Check window: the finding picked (by key), and the note to approve it with.
    Checks {
        picked: Option<String>,
        note: String,
    },
    /// The board's Design Rules window.
    Rules(crate::electronics_board_rules::RulesWindow),
    /// The board's design rule check.
    Drc(crate::electronics_board_rules::DrcWindow),
    /// The board's design-quality checks.
    Quality(crate::electronics_board_quality::QualityWindow),
    /// The net classes with the width-from-current calculator, from either view.
    NetClasses(crate::electronics_board_rules::ClassesWindow),
    /// What the design's nets and parts are for, from either view.
    Intent(crate::electronics_intent::IntentWindow),
    /// The board's gate (`ecad.board.check`), from either view.
    BoardCheck(crate::electronics_board_check::CheckWindow),
    /// Export Pages, from either view; its choices are kept in [`Design::export`].
    Export,
}

impl Open {
    fn command(&self) -> &'static str {
        match self {
            Open::Netlist => "ecad.netlist.inspect",
            Open::Rename { .. } => "ecad.schematic.net.rename",
            Open::Checks { .. } => "ecad.erc.inspect",
            Open::Rules(_) => "ecad.board.rules",
            Open::Drc(_) => "ecad.drc.inspect",
            Open::Quality(_) => "ecad.quality.inspect",
            Open::NetClasses(_) => "ecad.board.netclass",
            Open::Intent(_) => "ecad.intent.inspect",
            Open::BoardCheck(_) => "ecad.board.check",
            Open::Export => "ecad.export.pages",
        }
    }
}

/// The schematic's nets and its electrical rules check, worked out once for each version of the
/// document.
pub struct Checked {
    pub nets: Connectivity,
    pub report: Report,
}

/// What an electronics design's page shows: the SWITCH group's view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Schematic,
    Board,
}

/// An electronics design's view, windows and the Add Part search.
#[derive(Default)]
pub struct Design {
    pub view: View,
    /// The camera of the view not shown, kept for when it shows again.
    parked: crate::electronics_workspace::PageView,
    pub open: Option<Open>,
    pub search: String,
    /// The nets and the check, with the document they were worked out for (held weakly: an
    /// edit makes a new document, so a match means nothing changed).
    checked: Option<(Weak<Document>, Arc<Checked>)>,
    /// What the engine proposes for the design's intent, held the same way.
    pub(crate) inferred: Option<(Weak<Document>, Arc<crate::electronics_intent::Inferred>)>,
    /// Check Board's last run.
    pub(crate) board_check: Option<crate::electronics_board_check::Run>,
    /// Export Pages' choices, its pages and preview, and a save being written.
    pub(crate) export: crate::electronics_export::Export,
    /// What the user was doing last frame (see [`fresh_status`]), and when.
    watch: Option<(Watch, f64)>,
}

/// The view, the tools and picks of both views and the document's revision: a change of any
/// is a new action.
#[derive(Clone, Debug, PartialEq)]
struct Watch {
    view: View,
    sheet_tool: Option<&'static str>,
    board_tool: Option<&'static str>,
    sheet: std::collections::BTreeSet<u64>,
    board: std::collections::BTreeSet<u64>,
    revision: (u64, u64),
}

/// A new action (a tool started or put down, a pick changed, a command done) clears an error the
/// status line still shows from before it.
fn fresh_status(app: &mut SolveApp) {
    let now = Watch {
        view: app.electronics.design.view,
        sheet_tool: app.electronics.sheet.tool.as_ref().map(|t| t.command()),
        board_tool: app.electronics.board.tool.as_ref().map(|t| t.command()),
        sheet: app.electronics.sheet.selection.clone(),
        board: app.electronics.board.selection.clone(),
        revision: (app.session.uid, app.session.revision),
    };
    let at = crate::now_ms();
    if let Some((was, then)) = &app.electronics.design.watch
        && *was != now
        && app.status.as_ref().is_some_and(|(_, set, error)| *error && *set < *then)
    {
        app.status = None;
    }
    app.electronics.design.watch = Some((now, at));
}

/// The schematic's nets and electrical rules check as the document is now; worked out again only
/// after it changes.
pub(crate) fn checked(app: &mut SolveApp) -> Arc<Checked> {
    let doc = &app.session.doc;
    if let Some((was, c)) = &app.electronics.design.checked
        && std::ptr::eq(was.as_ptr(), Arc::as_ptr(doc))
    {
        return c.clone();
    }
    let nets = doc.ecad.connectivity();
    let report = erc::erc(&doc.ecad, &nets);
    let c = Arc::new(Checked { nets, report });
    app.electronics.design.checked = Some((Arc::downgrade(doc), c.clone()));
    c
}

/// Show the view `view` names (`schematic` or `board`): the page takes that view's own camera,
/// and the tools and drags of the other view go down (a route drawn so far is made first).
pub(crate) fn set_view(app: &mut SolveApp, view: &str) {
    let view = match view {
        "board" => View::Board,
        _ => View::Schematic,
    };
    if app.electronics.design.view == view {
        return;
    }
    // A drag in hand is dropped, not released as an edit when the view comes back.
    crate::electronics_board::put_down(app);
    let e = &mut app.electronics;
    std::mem::swap(&mut e.page, &mut e.design.parked);
    e.design.view = view;
    // The board's windows are the board's.
    if view == View::Schematic && matches!(e.design.open, Some(Open::Rules(_) | Open::Drc(_) | Open::Quality(_))) {
        e.design.open = None;
    }
    e.sheet.tool = None;
    e.sheet.choosing = false;
    e.sheet.drop_drag();
}

/// A toolbar command of an electronics design not handled by the sheet's tools: the SWITCH
/// group, Duplicate, Rename Net, Netlist, Net Classes, Design Intent, Check Board and Export
/// Pages. `true` when taken.
pub(crate) fn start(app: &mut SolveApp, id: &str) -> bool {
    match id {
        // The SWITCH group: the command says which view, and `set_view` follows it.
        "ecad.view.schematic" | "ecad.view.board" | "ecad.view.ground" | "ecad.view.layers" => drop(app.run(id, json!({}))),
        // Tidy Names: the board's names laid out again in its mode (Fit unless it has one), in
        // one step.
        "ecad.board.labels" => crate::electronics_board_layers::tidy_names(app),
        "ecad.schematic.duplicate" => duplicate(app),
        "ecad.netlist.inspect" => toggle(app, Open::Netlist),
        "ecad.erc.inspect" => toggle(app, Open::Checks { picked: None, note: String::new() }),
        "ecad.netlist.export" => export_netlist(app),
        "ecad.board.netclass" => toggle(app, Open::NetClasses(crate::electronics_board_rules::ClassesWindow::default())),
        "ecad.intent.inspect" => toggle(app, Open::Intent(crate::electronics_intent::IntentWindow::default())),
        "ecad.export.pages" => toggle(app, Open::Export),
        // Check Board checks afresh each time it opens.
        "ecad.board.check" => {
            app.electronics.design.board_check = None;
            toggle(app, Open::BoardCheck(crate::electronics_board_check::CheckWindow::default()));
        }
        "ecad.schematic.net.rename" => {
            let net = picked_net(app).unwrap_or_default();
            let to = app.session.doc.ecad.connectivity().net(&net).filter(|n| n.named).map(|n| n.name.clone()).unwrap_or_default();
            toggle(app, Open::Rename { net, to, merge: false });
        }
        _ => return false,
    }
    true
}

/// Export Netlist: a save picker, then `ecad.netlist.export` writes the file there.
fn export_netlist(app: &mut SolveApp) {
    let name: String = app.session.doc.name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') { c } else { '_' }).collect();
    let name = if name.trim().is_empty() { "design".to_string() } else { name.trim().to_string() };
    app.pick_save_command(&format!("{name}-netlist.json"), &["json"], "ecad.netlist.export", json!({"format": "json"}));
}

/// Open `window`, or close it when one of its kind is open; the Add Part window gives way.
pub(crate) fn toggle(app: &mut SolveApp, window: Open) {
    let d = &mut app.electronics.design;
    let same = d.open.as_ref().is_some_and(|o| o.command() == window.command());
    d.open = (!same).then_some(window);
    if d.open.is_some() {
        app.electronics.sheet.choosing = false;
    }
}

/// The toolbar command whose window is open.
pub(crate) fn running(app: &SolveApp) -> Option<&'static str> {
    app.electronics.design.open.as_ref().map(Open::command)
}

/// Escape: close the window. `true` when one was open.
pub(crate) fn escape(app: &mut SolveApp) -> bool {
    app.electronics.design.open.take().is_some()
}

/// Copies of the picked sheet objects two grid steps right and down, picked in their place.
fn duplicate(app: &mut SolveApp) {
    let items: Vec<u64> = app.electronics.sheet.selection.iter().copied().collect();
    if items.is_empty() {
        app.set_status(tr("electronics.pick_on_sheet_first"), false);
        return;
    }
    let step = 2.0 * crate::electronics_schematic::GRID;
    if let Ok(reply) = app.run("ecad.schematic.duplicate", json!({"items": items, "by": [step, -step]})) {
        let created = reply.get("created").and_then(Value::as_array).into_iter().flatten().filter_map(|c| c.get("id").and_then(Value::as_u64));
        app.electronics.sheet.selection = created.collect();
    }
}

/// The net of the first picked wire or label, by name.
fn picked_net(app: &SolveApp) -> Option<String> {
    let ecad = &app.session.doc.ecad;
    let c = ecad.connectivity();
    app.electronics
        .sheet
        .selection
        .iter()
        .find_map(|id| c.wire_net(&ecad.schematic, *id).or_else(|| c.label_net(&ecad.schematic, *id)))
        .and_then(|k| c.nets.get(k))
        .map(|n| n.name.clone())
}

/// The open electronics library tabs.
fn libraries(app: &SolveApp) -> impl Iterator<Item = &Session> {
    app.docs.background().filter(|s| s.doc.kind == DocumentKind::ElectronicsLibrary)
}

/// Where an Add Part row's component is: this design, or the `n`th open library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Here,
    Library(usize),
}

/// One row of the Add Part window.
struct Choice {
    /// How well it matches the search: its name starts with it (0), its name or value holds it
    /// (1), or only its description or library's name does (2). Rows list in this order.
    rank: u8,
    source: Source,
    /// The component's id in its own document.
    id: u64,
    name: String,
    /// Where it is, and its pins and pads.
    detail: String,
}

/// The components of `ecad` (from `source`, called `from`) whose name, value, description or
/// `from` holds `search`, ranked (see [`Choice::rank`]).
fn choices_in(out: &mut Vec<Choice>, ecad: &Electronics, source: Source, from: &str, search: &str) {
    for c in &ecad.components {
        let name = c.name.to_lowercase();
        let rank = if search.is_empty() || name.starts_with(search) {
            0
        } else if name.contains(search) || c.value.to_lowercase().contains(search) {
            1
        } else if format!("{}\n{from}", c.description).to_lowercase().contains(search) {
            2
        } else {
            continue;
        };
        let r = ecad.resolve(c, None);
        let counts =
            crate::i18n::format("electronics.counts", &[("pins", &r.symbol.pins.len().to_string()), ("pads", &r.footprint.pads.len().to_string())]);
        let name = if c.value.is_empty() { c.name.clone() } else { format!("{} · {}", c.name, c.value) };
        out.push(Choice { rank, source, id: c.id, name, detail: format!("{from} · {counts}") });
    }
}

/// What `ecad.library.import` takes to copy the component `id` of `ecad`: it, its symbol, and
/// its variants' footprints and 3D packages, as the library saves them.
fn import_items(ecad: &Electronics, id: u64) -> Option<Value> {
    let c = ecad.component(id)?;
    let footprints: BTreeSet<u64> = c.variants.iter().filter_map(|v| v.footprint).collect();
    let packages: BTreeSet<u64> = c.variants.iter().filter_map(|v| v.package).collect();
    let symbols: Vec<_> = c.symbol.and_then(|s| ecad.symbol(s)).into_iter().collect();
    let footprints: Vec<_> = footprints.into_iter().filter_map(|f| ecad.footprint(f)).collect();
    let packages: Vec<_> = packages.into_iter().filter_map(|p| ecad.package(p)).collect();
    let mut items = serde_json::Map::new();
    items.insert("symbols".into(), serde_json::to_value(symbols).ok()?);
    items.insert("footprints".into(), serde_json::to_value(footprints).ok()?);
    items.insert("packages".into(), serde_json::to_value(packages).ok()?);
    items.insert("components".into(), serde_json::to_value([c]).ok()?);
    Some(Value::Object(items))
}

/// Arm Place with the component `id` of `source`; one from a library is copied into the design
/// first (or found there, copied before).
fn add_part(app: &mut SolveApp, source: Source, id: u64) {
    let library = match source {
        Source::Here => return crate::electronics_schematic::place(app, id),
        Source::Library(n) => n,
    };
    let Some((items, from, name, drawn)) = libraries(app).nth(library).and_then(|s| {
        let ecad = &s.doc.ecad;
        let c = ecad.component(id)?;
        Some((import_items(ecad, id)?, s.doc.name.clone(), c.name.clone(), !ecad.symbol_of(c).pins.is_empty()))
    }) else {
        return;
    };
    // Nothing is copied that could not be placed.
    if !drawn {
        app.set_status(crate::i18n::format("electronics.add_part_no_symbol", &[("name", &name), ("library", &from)]), true);
        return;
    }
    // A refusal says why on the status line.
    let Ok(reply) = app.run("ecad.library.import", json!({"items": items})) else { return };
    let copy = reply
        .get("components")
        .and_then(Value::as_array)
        .and_then(|rows| rows.iter().find(|r| r.get("from").and_then(Value::as_u64) == Some(id)))
        .and_then(|r| r.get("to").and_then(Value::as_u64));
    if let Some(copy) = copy {
        crate::electronics_schematic::place(app, copy);
    }
    if reply.get("models_missing").and_then(Value::as_array).is_some_and(|m| !m.is_empty()) {
        app.set_status(crate::i18n::format("electronics.models_stay", &[("library", &from)]), false);
    }
}

/// Where a window opens: beside the Inspector, below the toolbar; it can be moved from there.
pub(crate) fn beside(app: &SolveApp, ctx: &egui::Context) -> egui::Pos2 {
    let at = app.shell.workspace_panels.get("electronics:editor").map_or(ctx.content_rect().right_top(), |(r, _)| r.left_top());
    at - vec2(Tokens::get().space_8, 0.0)
}

/// The design's windows beside the Inspector.
pub(crate) fn windows(app: &mut SolveApp, ctx: &egui::Context) {
    fresh_status(app);
    add_part_window(app, ctx);
    crate::electronics_export::tick(app, ctx);
    match app.electronics.design.open.clone() {
        Some(Open::Netlist) => netlist_window(app, ctx),
        Some(Open::Rename { net, to, merge }) => rename_window(app, ctx, net, to, merge),
        Some(Open::Checks { picked, note }) => checks_window(app, ctx, picked, note),
        Some(Open::Rules(w)) => crate::electronics_board_rules::rules_window(app, ctx, w),
        Some(Open::Drc(w)) => crate::electronics_board_rules::drc_window(app, ctx, w),
        Some(Open::Quality(w)) => crate::electronics_board_quality::window(app, ctx, w),
        Some(Open::NetClasses(w)) => crate::electronics_board_rules::classes_window(app, ctx, w),
        Some(Open::Intent(w)) => crate::electronics_intent::window(app, ctx, w),
        Some(Open::BoardCheck(w)) => crate::electronics_board_check::window(app, ctx, w),
        Some(Open::Export) => crate::electronics_export::window(app, ctx),
        None => {}
    }
}

/// The Add Part window: the design's components and every open library's, with a search; a click
/// arms Place with one.
fn add_part_window(app: &mut SolveApp, ctx: &egui::Context) {
    if !app.electronics.sheet.choosing {
        return;
    }
    let search = app.electronics.design.search.trim().to_lowercase();
    let mut rows = Vec::new();
    choices_in(&mut rows, &app.session.doc.ecad, Source::Here, &tr("electronics.add_part_here"), &search);
    let open = libraries(app).count();
    for (n, s) in libraries(app).enumerate() {
        choices_in(&mut rows, &s.doc.ecad, Source::Library(n), &s.doc.name, &search);
    }
    // Name and value matches before description matches, each in the libraries' order.
    rows.sort_by_key(|r| r.rank);
    let title = tr("electronics.add_part_title");
    let mut chosen = None;
    let closed = ui_kit::Window::new(egui::Id::new("electronics_add_part"), "component", &title)
        .placed(Align2::RIGHT_TOP, beside(app, ctx))
        .show_body(ctx, |ui| {
            let hint = tr("electronics.library_search");
            ui_kit::search_field(ui, egui::Id::new("electronics_add_part_search"), &mut app.electronics.design.search, &hint);
            ui.add_space(Tokens::get().space_6);
            if rows.is_empty() {
                let note = match (search.is_empty(), open) {
                    (false, _) => "electronics.library_no_match",
                    (true, 0) => "electronics.add_part_none",
                    (true, _) => "electronics.add_part_empty",
                };
                ui_kit::text(ui, &tr(note), ui_kit::Text::Note(note));
            }
            for row in &rows {
                let response = ui_kit::text(ui, &row.name, ui_kit::Text::NameCell(false));
                let source = match row.source {
                    Source::Here => "here".to_string(),
                    Source::Library(n) => n.to_string(),
                };
                crate::scenario::publish_target(&format!("electronics:add_part:{source}:{}", row.name), response.rect);
                ui_kit::text(ui, &row.detail, ui_kit::Text::Note("electronics.add_part_row"));
                if response.clicked() {
                    chosen = Some((row.source, row.id));
                }
            }
            if open == 0 {
                ui_kit::text(ui, &tr("electronics.add_part_open_library"), ui_kit::Text::Note("electronics.add_part_open_library"));
            }
        });
    if let Some((source, id)) = chosen {
        add_part(app, source, id);
    } else if closed {
        app.electronics.sheet.choosing = false;
    }
}

/// A net's pins as a list: `R1.1, U1.4`.
fn pin_list(pins: &[NetPin]) -> String {
    pins.iter().map(NetPin::reference).collect::<Vec<_>>().join(", ")
}

/// The Netlist window: each net with its pins (a click picks its wires and labels on the sheet),
/// then the pins that touch nothing.
fn netlist_window(app: &mut SolveApp, ctx: &egui::Context) {
    let checked = checked(app);
    let c = &checked.nets;
    let title = tr("electronics.netlist_title");
    let mut picked: Option<BTreeSet<u64>> = None;
    let closed =
        ui_kit::Window::new(egui::Id::new("electronics_netlist"), "list", &title).placed(Align2::RIGHT_TOP, beside(app, ctx)).show_body(ctx, |ui| {
            let summary = crate::i18n::format(
                "electronics.netlist_summary",
                &[("nets", &c.nets.len().to_string()), ("unconnected", &c.unconnected.len().to_string())],
            );
            ui_kit::text(ui, &summary, ui_kit::Text::Note("electronics.netlist_summary"));
            if c.truncated {
                ui_kit::text(ui, &tr("electronics.netlist_truncated"), ui_kit::Text::Error("electronics.netlist_truncated"));
            }
            for net in c.nets.iter().take(MAX_NETS) {
                let ids: BTreeSet<u64> = net.wires.iter().chain(&net.labels).copied().collect();
                let response = ui_kit::text(ui, &net.name, ui_kit::Text::NameCell(!ids.is_empty() && ids == app.electronics.sheet.selection));
                crate::scenario::publish_target(&format!("electronics:net:{}", net.name), response.rect);
                ui_kit::text(ui, &pin_list(&net.pins), ui_kit::Text::Note("electronics.netlist_pins"));
                if response.clicked() {
                    picked = Some(ids);
                }
            }
            if !c.unconnected.is_empty() {
                ui.add_space(Tokens::get().space_8);
                ui_kit::text(ui, &tr("electronics.netlist_unconnected"), ui_kit::Text::Heading("electronics.netlist_unconnected"));
                ui_kit::text(ui, &pin_list(&c.unconnected), ui_kit::Text::Note("electronics.netlist_pins"));
            }
        });
    if let Some(ids) = picked {
        let s = &mut app.electronics.sheet;
        s.tool = None;
        s.selection = ids;
    }
    if closed {
        app.electronics.design.open = None;
    }
}

/// The Rename Net window: the net (the picked wire's or label's when it opened), its new name, and
/// whether to join a net that already has that name.
fn rename_window(app: &mut SolveApp, ctx: &egui::Context, mut net: String, mut to: String, mut merge: bool) {
    let names: Vec<String> = checked(app).nets.nets.iter().take(MAX_NETS).map(|n| n.name.clone()).collect();
    let title = solidcraft_engine::find_command("ecad.schematic.net.rename").map_or("Rename Net", |c| c.label);
    let title = app.tr_label(title).to_string();
    let mut go = false;
    let closed = ui_kit::Window::new(egui::Id::new("electronics_rename_net"), "rename", &title)
        .placed(Align2::RIGHT_TOP, beside(app, ctx))
        .show_body(ctx, |ui| {
            if names.is_empty() {
                ui_kit::text(ui, &tr("electronics.rename_no_nets"), ui_kit::Text::Note("electronics.rename_no_nets"));
                return;
            }
            let at = names.iter().position(|n| *n == net).unwrap_or(0);
            if let Some(name) = names
                .get(crate::electronics_editor::choice(ui, "electronics_rename_net_choice", &tr("electronics.field_net"), at, &names).unwrap_or(at))
            {
                net.clone_from(name);
            }
            let (enter, _) = ui_kit::text_row(ui, &tr("electronics.field_new_name"), &mut to, "VCC");
            if ui_kit::checkbox_row(ui, "electronics_rename_merge", &tr("electronics.rename_merge"), merge) {
                merge = !merge;
            }
            let ready = !to.trim().is_empty();
            go = (crate::electronics_editor::button(ui, "rename-net", &title, ready) || enter) && ready;
        });
    if go && app.run("ecad.schematic.net.rename", json!({"net": net, "to": to.trim(), "merge": merge})).is_ok() {
        app.electronics.design.open = None;
        return;
    }
    app.electronics.design.open = (!closed).then_some(Open::Rename { net, to, merge });
}

/// What the Electrical Check window was asked to do.
enum Act {
    Pick(String),
    Approve(String, String),
    Unapprove(String),
    ClearStale,
}

/// A finding's row title: what it is about, then its code in words (`U1.4 · pin unconnected`).
fn finding_title(f: &Finding) -> String {
    let about = f.items.iter().take(2).map(Item::reference).collect::<Vec<_>>().join(", ");
    let code = f.code.replace('_', " ");
    if about.is_empty() { code } else { format!("{about} · {code}") }
}

/// The Electrical Check window: the check's errors, warnings and approved findings. A click on a
/// finding picks what it is about on the sheet and frames it; the picked one can be approved with
/// a note, or an approved one taken back.
fn checks_window(app: &mut SolveApp, ctx: &egui::Context, picked: Option<String>, mut note: String) {
    let checked = checked(app);
    let r = &checked.report;
    let notes: BTreeMap<String, (String, Option<Actor>)> =
        app.session.doc.ecad.schematic.approvals.iter().map(|a| (a.key.clone(), (a.note.clone(), a.by))).collect();
    let title = tr("electronics.checks_title");
    let mut act: Option<Act> = None;
    let closed = ui_kit::Window::new(egui::Id::new("electronics_checks"), "interference", &title)
        .placed(Align2::RIGHT_TOP, beside(app, ctx))
        .show_body(ctx, |ui| {
            let summary = crate::i18n::format(
                "electronics.checks_summary",
                &[("errors", &r.errors.to_string()), ("warnings", &r.warnings.to_string()), ("approved", &r.approved.to_string())],
            );
            ui_kit::text(ui, &summary, ui_kit::Text::Note("electronics.checks_summary"));
            if r.truncated {
                ui_kit::text(ui, &tr("electronics.checks_truncated"), ui_kit::Text::Error("electronics.checks_truncated"));
            }
            if r.findings.is_empty() {
                ui_kit::text(ui, &tr("electronics.checks_none"), ui_kit::Text::Note("electronics.checks_none"));
            }
            if r.pending_user > 0 {
                let text = crate::i18n::format("electronics.erc_pending_summary", &[("n", &r.pending_user.to_string())]);
                ui_kit::text(ui, &text, ui_kit::Text::Error("electronics.erc_pending_summary"));
            }
            let groups: [(&str, Box<dyn Fn(&Finding) -> bool>); 3] = [
                ("electronics.checks_errors", Box::new(|f: &Finding| !f.approved && f.severity == Severity::Error)),
                ("electronics.checks_warnings", Box::new(|f: &Finding| !f.approved && f.severity == Severity::Warning)),
                ("electronics.checks_approved", Box::new(|f: &Finding| f.approved)),
            ];
            for (heading, keep) in &groups {
                let rows: Vec<&Finding> = r.findings.iter().filter(|f| keep(f)).collect();
                if rows.is_empty() {
                    continue;
                }
                ui.add_space(Tokens::get().space_8);
                let text = format!("{} ({})", tr(heading), rows.len());
                ui_kit::text(ui, &text, ui_kit::Text::Heading(heading));
                for f in rows.iter().take(MAX_FINDINGS) {
                    let selected = picked.as_deref() == Some(f.key.as_str());
                    let response = ui_kit::text(ui, &finding_title(f), ui_kit::Text::NameCell(selected));
                    crate::scenario::publish_target(&format!("electronics:problem:{}", f.key), response.rect);
                    if response.clicked() {
                        act = Some(Act::Pick(f.key.clone()));
                    }
                    let style = if f.severity == Severity::Error && !f.approved {
                        ui_kit::Text::Error("electronics.checks_message")
                    } else {
                        ui_kit::Text::Note("electronics.checks_message")
                    };
                    ui_kit::text(ui, &f.message, style);
                    // Who approved it, and why; an agent's approval of a critical error waits for
                    // the user's.
                    if f.approved || f.pending_user {
                        let (said, by) = notes.get(&f.key).cloned().unwrap_or_default();
                        let text = crate::electronics_board_rules::approval_words(f.pending_user, by, &said);
                        let style = if f.pending_user {
                            ui_kit::Text::Error("electronics.checks_approved_note")
                        } else {
                            ui_kit::Text::Note("electronics.checks_approved_note")
                        };
                        ui_kit::text(ui, &text, style);
                    }
                    if !selected {
                        continue;
                    }
                    if f.pending_user {
                        // The user's approval confirms the agent's, keeping its note unless one is typed.
                        let (enter, _) = ui_kit::text_row(ui, &tr("electronics.checks_note"), &mut note, &tr("electronics.drc_confirm_hint"));
                        if crate::electronics_editor::button(ui, "erc-confirm", &tr("electronics.drc_confirm"), true) || enter {
                            act = Some(Act::Approve(f.key.clone(), note.trim().to_string()));
                        }
                        if crate::electronics_editor::button(ui, "erc-unapprove", &tr("electronics.checks_unapprove"), true) {
                            act = Some(Act::Unapprove(f.key.clone()));
                        }
                    } else if f.approved {
                        if crate::electronics_editor::button(ui, "erc-unapprove", &tr("electronics.checks_unapprove"), true) {
                            act = Some(Act::Unapprove(f.key.clone()));
                        }
                    } else {
                        let (enter, _) = ui_kit::text_row(ui, &tr("electronics.checks_note"), &mut note, &tr("electronics.checks_note_hint"));
                        if crate::electronics_editor::button(ui, "erc-approve", &tr("electronics.checks_approve"), true) || enter {
                            act = Some(Act::Approve(f.key.clone(), note.trim().to_string()));
                        }
                    }
                }
                if rows.len() > MAX_FINDINGS {
                    let more = crate::i18n::format("electronics.checks_more", &[("n", &(rows.len() - MAX_FINDINGS).to_string())]);
                    ui_kit::text(ui, &more, ui_kit::Text::Note("electronics.checks_more"));
                }
            }
            if !r.stale.is_empty() {
                ui.add_space(Tokens::get().space_8);
                let text = crate::i18n::format("electronics.checks_stale", &[("n", &r.stale.len().to_string())]);
                ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.checks_stale"));
                if crate::electronics_editor::button(ui, "erc-clear-stale", &tr("electronics.checks_clear_stale"), true) {
                    act = Some(Act::ClearStale);
                }
            }
        });
    let mut picked = picked;
    match act {
        Some(Act::Pick(key)) => {
            if let Some(f) = r.findings.iter().find(|f| f.key == key) {
                show_finding(app, f);
            }
            if picked.as_deref() != Some(key.as_str()) {
                note.clear();
            }
            picked = Some(key);
        }
        Some(Act::Approve(key, text)) => {
            if app.run_as_user("ecad.erc.approve", json!({"keys": [key], "note": text})).is_ok() {
                note.clear();
            }
        }
        // The user's own approvals are the user's to take back.
        Some(Act::Unapprove(key)) => drop(app.run_as_user("ecad.erc.unapprove", json!({"keys": [key]}))),
        Some(Act::ClearStale) => drop(app.run_as_user("ecad.erc.unapprove", json!({"stale": true}))),
        None => {}
    }
    app.electronics.design.open = (!closed).then_some(Open::Checks { picked, note });
}

/// Pick what a finding is about on the sheet (a pin's part) and frame it, on the schematic.
fn show_finding(app: &mut SolveApp, f: &Finding) {
    if app.electronics.design.view != View::Schematic {
        drop(app.run("ecad.view.schematic", json!({})));
    }
    let ecad = &app.session.doc.ecad;
    let sheet = &ecad.schematic;
    let library = ecad.library();
    let pins = Pins::new(sheet, &library);
    let mut points: Vec<[f64; 2]> = f.at.into_iter().collect();
    for item in &f.items {
        match item {
            Item::Pin(p) => points.push(p.at),
            Item::Part { id, .. } => {
                let part = sheet.part(*id);
                points.extend(part.and_then(|p| part_bbox(p, library.get(&p.component)?.symbol)).into_iter().flatten());
            }
            Item::Wire(id) => points.extend(sheet.wires.iter().find(|w| w.id == *id).map(|w| pins.wire(w)).unwrap_or_default()),
            Item::Label { id, .. } => points.extend(sheet.labels.iter().find(|l| l.id == *id).and_then(|l| pins.anchor(&l.at))),
            Item::Marker(id) => points.extend(sheet.markers.iter().find(|m| m.id == *id).and_then(|m| pins.anchor(&m.at))),
        }
    }
    let frame = solidcraft_engine::doc::ecad::bbox(points).map(|[a, b]| {
        let grow = |lo: f64, hi: f64| {
            let half = ((hi - lo) / 2.0).max(FRAME_MM / 2.0);
            ((lo + hi) / 2.0 - half, (lo + hi) / 2.0 + half)
        };
        let ((x0, x1), (y0, y1)) = (grow(a[0], b[0]), grow(a[1], b[1]));
        [[x0, y0], [x1, y1]]
    });
    let picked: BTreeSet<u64> = f.items.iter().map(Item::id).filter(|id| sheet.kind_of(*id).is_some()).collect();
    let s = &mut app.electronics.sheet;
    s.tool = None;
    s.selection = picked;
    s.frame = frame;
}

/// What is picked in an electronics document, for the control channel's `ui.selection`: the view,
/// and each picked schematic or board object's id, kind and name (a part's designator, a label's
/// or port's net name); on the board also the picked track segment and whether the outline is
/// picked; in a library the picked pin, pad or drawing. `None` outside the Electronics workspace.
pub(crate) fn selection(app: &SolveApp) -> Option<Value> {
    if !crate::electronics_workspace::active(app) {
        return None;
    }
    let ecad = &app.session.doc.ecad;
    if crate::electronics_workspace::schematic(app) {
        let s = &ecad.schematic;
        let items: Vec<Value> = app
            .electronics
            .sheet
            .selection
            .iter()
            .map(|id| {
                use solidcraft_engine::doc::ecad::schematic::ItemKind;
                let (kind, name) = match s.kind_of(*id) {
                    Some(ItemKind::Part) => ("part", s.part(*id).map(|p| p.designator.clone())),
                    Some(ItemKind::Wire) => ("wire", None),
                    Some(ItemKind::Label) => ("label", s.labels.iter().find(|l| l.id == *id).map(|l| l.name.clone())),
                    Some(ItemKind::Marker) => ("marker", None),
                    Some(ItemKind::Frame) => ("frame", s.frames.iter().find(|f| f.id == *id).map(|f| f.title.clone())),
                    None => ("gone", None),
                };
                json!({"id": id, "kind": kind, "name": name})
            })
            .collect();
        return Some(json!({"view": "schematic", "items": items}));
    }
    if crate::electronics_workspace::board(app) {
        let b = &ecad.board;
        let v = &app.electronics.board;
        let items: Vec<Value> = v
            .selection
            .iter()
            .map(|id| {
                use solidcraft_engine::doc::ecad::board::BoardKind;
                let footprint = || b.footprint(*id).and_then(|f| ecad.schematic.part(f.part)).map(|p| p.designator.clone());
                let (kind, name) = match b.kind_of(*id) {
                    Some(BoardKind::Footprint) => ("footprint", footprint()),
                    Some(BoardKind::Track) => ("track", None),
                    Some(BoardKind::Via) => ("via", None),
                    Some(BoardKind::Hole) => ("hole", None),
                    Some(BoardKind::Graphic) => ("graphic", None),
                    Some(BoardKind::Pour) => ("pour", b.pour(*id).and_then(|p| p.net.clone())),
                    Some(BoardKind::Keepout) => ("keepout", None),
                    None => ("gone", None),
                };
                json!({"id": id, "kind": kind, "name": name})
            })
            .collect();
        let segment = v.segment.map(|(track, index)| json!({"track": track, "segment": index}));
        return Some(json!({"view": "board", "items": items, "segment": segment, "outline": v.outline_picked}));
    }
    let picked = match &app.electronics.editor.picked {
        Some(crate::electronics_editor::Item::Pin(n)) => json!({"kind": "pin", "name": n}),
        Some(crate::electronics_editor::Item::Pad(n)) => json!({"kind": "pad", "name": n}),
        Some(crate::electronics_editor::Item::Graphic(i)) => json!({"kind": "graphic", "index": i}),
        None => Value::Null,
    };
    Some(json!({"view": "library", "picked": picked}))
}

/// The Browser card: the design, and under it its views, the Schematic, the Board and the 3D
/// Board still to come. A click on a view shows it (as the SWITCH group does), or frames it afresh
/// when it shows already. A double-click on the design renames it, as F2 does.
pub(crate) fn browser(app: &mut SolveApp, ui: &mut Ui) {
    let t = Tokens::get();
    ui.add_space(t.space_3);
    let name = app.session.doc.name.clone();
    let root = node(app, ui, "electronics:design", 0, "ws_electronics", &name);
    if !root {
        return;
    }
    let view = app.electronics.design.view;
    for (key, icon, label, shows, command) in [
        ("schematic", "line", "electronics.mode_schematic", View::Schematic, "ecad.view.schematic"),
        ("board", "rect", "electronics.view_board", View::Board, "ecad.view.board"),
    ] {
        let text = tr(label);
        let r = row(ui, key, Row { depth: 1, icon, label: &text, name: true, selected: view == shows, ..Default::default() });
        if r.clicked {
            if view == shows {
                app.electronics.page.fitted = false;
            } else {
                drop(app.run(command, json!({})));
            }
        }
    }
    let board_3d = tr("electronics.view_board_3d");
    let r = row(ui, "board_3d", Row { depth: 1, icon: "box", label: &board_3d, name: true, dim: true, ..Default::default() });
    if let Some(resp) = r.resp {
        resp.on_hover_text(tr("electronics.coming_next"));
    }
}

/// A Browser row of the tree.
fn row(ui: &mut Ui, key: &str, look: Row) -> crate::browser::RowResp {
    let r = crate::browser::draw_row(ui, ui.id().with(("electronics_design_row", key)), &look);
    if let Some(rect) = r.rect {
        crate::scenario::publish_target(&format!("electronics:design:{key}"), rect);
    }
    r
}

/// The design's row, which folds (open at first) on its arrow and is renamed on a double-click,
/// keyed `key`; `true` while it is open.
fn node(app: &mut SolveApp, ui: &mut Ui, key: &str, depth: usize, icon: &str, label: &str) -> bool {
    let open = crate::browser::is_open(app, key, true);
    let r = row(ui, key, Row { depth, fold: Some(open), icon, label, name: true, ..Default::default() });
    if r.fold {
        crate::browser::toggle(app, key, true);
    } else if r.double {
        rename(app, r.rect.map(|x| x.left_bottom()).unwrap_or_default());
    }
    open
}

/// Rename the electronics document (its tab follows): the box opens at `at` and Enter runs
/// `component.rename` on the design itself.
pub(crate) fn rename(app: &mut SolveApp, at: egui::Pos2) {
    crate::context_menu::start_rename(app, crate::context_menu::RenameWhat::Component(0), at);
}
