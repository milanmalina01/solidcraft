//! Export Pages (the OUTPUT group of the schematic's DESIGN tab and of the board's DOCUMENT tab):
//! the design's print pages (`ecad.export.pages`) as one PDF, an SVG or PNG per page, or the parts
//! list as CSV. The window ticks the kinds of page and the frames that get a page of their own,
//! and sets the paper, orientation, style, format, the resolution of PNG pages and whether ground
//! and supply nets are cross-referenced. It lists the pages that makes, with their paper and scale
//! and the warnings (as `ecad.export.pages.inspect` does), previews the picked page as the PNG
//! writer draws it at a low resolution, and edits the title block's fields and the cover's revision
//! history (`ecad.design.info`, one undo step each: a table of the revisions, newest first, the
//! picked one's fields, Add and Remove). Save… asks where (a folder for SVG and PNG pages; in the
//! browser the files come down as downloads). The window scrolls as one: the page list shows every
//! page, and the preview is as wide as the window. The File menu's Export Parts List saves the CSV
//! straight away, as Save… does with CSV picked.
//!
//! The pages are worked out beside the window, again after any change to the design or to the
//! window's choices, and Save… writes them there too, from a copy of the design as it was asked
//! for. The browser has no threads, so there both happen at once, and a change to the design keeps
//! the pages in hand (the window says so) until Refresh, so editing never waits on them. The list
//! names each page by its kind (and frame) in the interface's language.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use egui::{Align2, Ui};
use serde_json::{Value, json};
use solidcraft_engine::Session;
use solidcraft_engine::doc::ecad::info::{MAX_REVISIONS, Revision};
use solidcraft_engine::io::pages::{self, Book, PageKind};

use crate::SolveApp;
use crate::electronics_editor::tr;
use crate::theme::Tokens;
use crate::ui_kit;

/// How wide the preview is drawn before the window has been laid out, in points; then as wide as
/// the window is, in steps of [`PREVIEW_STEP`] (so a resize redraws it now and then, not on every
/// frame), at least [`PREVIEW_LEAST`]. The page is drawn at the resolution that fills it.
const PREVIEW_POINTS: f32 = 420.0;
const PREVIEW_STEP: f32 = 80.0;
const PREVIEW_LEAST: f32 = 240.0;
/// The finest a preview is drawn, in dots an inch.
const PREVIEW_DPI: f64 = 150.0;
/// Whether the pages are worked out beside the window (the desktop) rather than on its thread
/// (the browser).
const BESIDE: bool = cfg!(not(target_arch = "wasm32"));

/// What the pages are written as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    #[default]
    Pdf,
    Svg,
    Png,
    Csv,
}

impl Format {
    const ALL: [Format; 4] = [Format::Pdf, Format::Svg, Format::Png, Format::Csv];

    /// As the command names it, and its files' extension.
    fn name(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Svg => "svg",
            Format::Png => "png",
            Format::Csv => "csv",
        }
    }
}

/// The paper sizes, orientations and styles offered, as the command names them; the first two
/// lists start with the one the command picks itself.
const PAPERS: [&str; 5] = ["auto", "a4", "a3", "letter", "tabloid"];
const ORIENTATIONS: [&str; 3] = ["auto", "landscape", "portrait"];
const STYLES: [&str; 3] = ["paper", "monochrome", "dark"];

/// The window's choices; kept while the design's tab is open, so the window opens as it was left.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// The kinds of page left out, by name (every kind is ticked at first).
    off: BTreeSet<&'static str>,
    /// The frames left out of the section pages, by id.
    frames_off: BTreeSet<u64>,
    /// Indexes into [`PAPERS`], [`ORIENTATIONS`] and [`STYLES`].
    paper: usize,
    orientation: usize,
    style: usize,
    format: Format,
    /// PNG only, dots an inch.
    dpi: f64,
    power_xrefs: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            off: BTreeSet::new(),
            frames_off: BTreeSet::new(),
            paper: 0,
            orientation: 0,
            style: 0,
            format: Format::Pdf,
            dpi: pages::DEFAULT_DPI,
            power_xrefs: false,
        }
    }
}

impl Settings {
    /// What `ecad.export.pages.inspect` takes for these choices, on a sheet with `frames` (by id);
    /// `None` when nothing would print. The parts list's CSV holds the rows of its pages, so for
    /// CSV those are the pages listed.
    fn params(&self, frames: &[u64]) -> Option<Value> {
        if self.format == Format::Csv {
            return Some(json!({"pages": [PageKind::Parts.name()]}));
        }
        let ticked: Vec<u64> = frames.iter().copied().filter(|f| !self.frames_off.contains(f)).collect();
        // Sections with every frame left out print nothing of their own.
        let sections = !(frames.len() > ticked.len() && ticked.is_empty());
        let kinds: Vec<&str> =
            PageKind::ALL.iter().map(|k| k.name()).filter(|k| !self.off.contains(k) && (sections || *k != PageKind::Sections.name())).collect();
        if kinds.is_empty() {
            return None;
        }
        let mut p = json!({});
        if kinds.len() < PageKind::ALL.len() {
            p["pages"] = json!(kinds);
        }
        if ticked.len() < frames.len() && !ticked.is_empty() {
            p["frames"] = json!(ticked);
        }
        let choose = |list: &[&'static str], i: usize| list.get(i).copied().filter(|n| *n != "auto");
        if let Some(paper) = choose(&PAPERS, self.paper) {
            p["paper"] = json!(paper);
        }
        if let Some(o) = choose(&ORIENTATIONS, self.orientation) {
            p["orientation"] = json!(o);
        }
        if let Some(style) = STYLES.get(self.style).filter(|s| **s != "paper") {
            p["style"] = json!(style);
        }
        if self.power_xrefs {
            p["power_xrefs"] = json!(true);
        }
        if self.format == Format::Png {
            p["format"] = json!("png");
            p["dpi"] = json!(self.dpi);
        }
        Some(p)
    }
}

/// What tells one set of pages from another: the document, its revision and the parameters.
type Key = (u64, u64, String);

/// A page drawn: its size in pixels and its opaque RGBA rows.
type Picture = ([u32; 2], Vec<u8>);

/// What a job beside the window comes back with.
enum Done {
    /// The pages for `Key` (or why they were refused), with the page asked for drawn.
    Composed(Key, Result<(Arc<Book>, Value), String>, Option<(usize, u32, Result<Picture, String>)>),
    /// A page of the pages in hand drawn: which, at what width, and the picture.
    Drawn(Key, usize, u32, Result<Picture, String>),
}

/// The pages worked out for the choices and the design as they were.
struct Composed {
    key: Key,
    reply: Result<(Arc<Book>, Value), String>,
}

/// Export Pages' state in a design: the choices, the pages for them, the preview, and the work
/// going on beside the window.
#[derive(Default)]
pub struct Export {
    pub settings: Settings,
    /// The page the preview shows, from 0.
    page: usize,
    composed: Option<Composed>,
    job: Option<Receiver<Done>>,
    /// The page drawn: for which pages, which page, at what width.
    preview: Option<(Key, usize, u32, egui::TextureHandle)>,
    /// Why a page could not be drawn: for which pages, which page.
    preview_error: Option<(Key, usize, String)>,
    /// A save being written, from a copy of the design.
    saving: Option<Receiver<Result<Value, String>>>,
    /// The revision picked in the history, by its place in it (oldest first).
    revision: Option<usize>,
    /// The width the preview had room for when last shown, in points.
    room: Option<f32>,
}

/// Work out the pages (and draw page `page` at `width` pixels) for `params` on `session`'s design.
fn compose(session: &Session, key: Key, params: &Value, page: usize, width: u32) -> Done {
    let reply = solidcraft_engine::cmd::pages_book(session, params).map(|(book, reply)| (Arc::new(book), reply)).map_err(|e| e.to_string());
    let drawn = reply.as_ref().ok().filter(|(book, _)| !book.pages.is_empty()).map(|(book, _)| {
        let page = page.min(book.pages.len().saturating_sub(1));
        (page, width, draw(book, page, width))
    });
    Done::Composed(key, reply, drawn)
}

/// Page `page` of `book` drawn `width` pixels wide (at no more than [`PREVIEW_DPI`]).
fn draw(book: &Book, page: usize, width: u32) -> Result<Picture, String> {
    let wide = book.pages.get(page).map_or(1.0, |p| p.size[0]).max(1.0);
    let dpi = (f64::from(width) / (wide / 25.4)).clamp(*pages::DPI.start(), PREVIEW_DPI);
    pages::write_rgba(book, page, dpi)
}

/// Run `job` beside the window; its answer comes on the channel. The browser runs it at once, and
/// so does a desktop that cannot start a thread.
fn spawn(job: impl FnOnce() -> Done + Send + 'static) -> Receiver<Done> {
    let (tx, rx) = std::sync::mpsc::channel();
    let slot = Arc::new(std::sync::Mutex::new(Some(job)));
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (theirs, sender) = (slot.clone(), tx.clone());
        let started = std::thread::Builder::new().name("export-pages".into()).spawn(move || {
            if let Some(job) = theirs.lock().ok().and_then(|mut j| j.take()) {
                let _ = sender.send(job());
            }
        });
        if started.is_ok() {
            return rx;
        }
    }
    if let Some(job) = slot.lock().ok().and_then(|mut j| j.take()) {
        let _ = tx.send(job());
    }
    rx
}

/// A copy of the design to work on beside the window, with its file (the title block names it).
fn copy(app: &SolveApp) -> Session {
    let mut copy = app.session.scratch();
    copy.path.clone_from(&app.session.path);
    copy
}

/// Take up finished work: new pages, a page drawn, a save written (said on the status line, also
/// while the window is closed).
pub(crate) fn tick(app: &mut SolveApp, ctx: &egui::Context) {
    if let Some(rx) = &app.electronics.design.export.saving {
        match rx.try_recv() {
            Ok(r) => {
                app.electronics.design.export.saving = None;
                saved(app, r);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => app.electronics.design.export.saving = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
        }
    }
    let e = &mut app.electronics.design.export;
    let Some(rx) = &e.job else { return };
    let done = match rx.try_recv() {
        Ok(done) => done,
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            e.job = None;
            return;
        }
    };
    e.job = None;
    let picture = match done {
        Done::Composed(key, reply, drawn) => {
            e.composed = Some(Composed { key: key.clone(), reply });
            drawn.map(|(page, width, picture)| (key, page, width, picture))
        }
        Done::Drawn(key, page, width, picture) => Some((key, page, width, picture)),
    };
    let Some((key, page, width, picture)) = picture else { return };
    match picture {
        Ok(([w, h], rgba)) => {
            let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
            let texture = ctx.load_texture("electronics_export_preview", image, egui::TextureOptions::LINEAR);
            e.preview = Some((key, page, width, texture));
            e.preview_error = None;
        }
        Err(why) => e.preview_error = Some((key, page, why)),
    }
}

/// Start what the window needs and nothing is working on: the pages for `key`, else the picked
/// page drawn `width` pixels wide.
fn refresh(app: &mut SolveApp, key: &Key, params: &Value, width: u32) {
    if app.electronics.design.export.job.is_some() {
        return;
    }
    let e = &app.electronics.design.export;
    let page = e.page;
    let fresh = e.composed.as_ref().filter(|c| c.key == *key);
    let Some(composed) = fresh else {
        let (session, key, params) = (copy(app), key.clone(), params.clone());
        app.electronics.design.export.job = Some(spawn(move || compose(&session, key, &params, page, width)));
        return;
    };
    let Ok((book, _)) = &composed.reply else { return };
    if book.pages.is_empty() {
        return;
    }
    let page = page.min(book.pages.len() - 1);
    let drawn = e.preview.as_ref().is_some_and(|(k, p, w, _)| k == key && *p == page && *w == width);
    // A page that could not be drawn is not tried again until something changes.
    let failed = e.preview_error.as_ref().is_some_and(|(k, p, _)| k == key && *p == page);
    if drawn || failed {
        return;
    }
    let (book, key) = (book.clone(), key.clone());
    app.electronics.design.export.job = Some(spawn(move || {
        let picture = draw(&book, page, width);
        Done::Drawn(key, page, width, picture)
    }));
}

/// The design's name as a file name: letters, digits, `-`, `_` and spaces.
fn file_stem(app: &SolveApp) -> String {
    let name: String = app.session.doc.name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') { c } else { '_' }).collect();
    if name.trim().is_empty() { "design".to_string() } else { name.trim().to_string() }
}

/// Save…: where to (the platform's save dialog; for SVG and PNG pages, a folder, where the host
/// has a folder dialog), then `ecad.export.pages` writes there.
fn save(app: &mut SolveApp, mut params: Value) {
    let format = app.electronics.design.export.settings.format;
    let stem = file_stem(app);
    let name = if format == Format::Csv { format!("{stem}-parts.csv") } else { format!("{stem}.{}", format.name()) };
    params["format"] = json!(format.name());
    if format == Format::Csv {
        params = json!({"format": "csv"});
    }
    if matches!(format, Format::Svg | Format::Png) && app.services.pick_folder.is_some() {
        // A folder: the files are named after the design there (`<design>-03-lora-radio.svg`).
        app.pick_folder(move |app, path| {
            let Some(path) = path else { return };
            params["path"] = json!(format!("{}/", path.trim_end_matches(['/', '\\'])));
            write(app, params);
        });
        return;
    }
    app.pick_save(&name, &[format.name()], move |app, path| {
        let Some(path) = path else { return };
        params["path"] = json!(path);
        write(app, params);
    });
}

/// Write the pages from a copy of the design beside the window; in the browser, here.
fn write(app: &mut SolveApp, params: Value) {
    write_with(app, "ecad.export.pages", params);
}

/// Run the export command `cmd` on a copy of the design beside the window; in the browser, here.
fn write_with(app: &mut SolveApp, cmd: &'static str, params: Value) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let session = copy(app);
        let (tx, rx) = std::sync::mpsc::channel();
        let given = params.clone();
        let job = std::thread::Builder::new().name("export-write".into()).spawn(move || {
            let mut session = session;
            let _ = tx.send(session.execute(cmd, &given).map_err(|e| e.to_string()));
        });
        if job.is_ok() {
            app.electronics.design.export.saving = Some(rx);
            return;
        }
    }
    // A refusal says why on the status line.
    if let Ok(reply) = app.run(cmd, params) {
        saved(app, Ok(reply));
    }
}

/// Save JLCPCB Order Files…: where to put the zip (the parts and placement lists go beside it),
/// then `ecad.board.export.jlc` writes them and reads them back.
fn save_jlc(app: &mut SolveApp) {
    let name = format!("{}-jlcpcb.zip", file_stem(app));
    app.pick_save(&name, &["zip"], move |app, path| {
        let Some(path) = path else { return };
        if !saving(app) {
            write_with(app, "ecad.board.export.jlc", json!({"path": path}));
        }
    });
}

/// Say what a save wrote, or why it could not.
fn saved(app: &mut SolveApp, reply: Result<Value, String>) {
    let reply = match reply {
        Ok(r) => r,
        Err(why) => {
            app.set_status(crate::i18n::format("electronics.export_failed", &[("why", &why)]), true);
            return;
        }
    };
    let files = reply.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
    let bytes: u64 = files.iter().filter_map(|f| f.get("bytes").and_then(Value::as_u64)).sum();
    let size = if bytes >= 1 << 20 {
        crate::i18n::format("electronics.export_size_mb", &[("n", &format!("{:.1}", bytes as f64 / f64::from(1u32 << 20)))])
    } else {
        crate::i18n::format("electronics.export_size_kb", &[("n", &bytes.div_ceil(1024).to_string())])
    };
    let first = files.first().and_then(|f| f.get("path")).and_then(Value::as_str).unwrap_or_default().to_string();
    // The order files are read back: say so when they don't match the board.
    if let Some(v) = reply.get("verified").filter(|v| v.get("ok").and_then(Value::as_bool) == Some(false)) {
        let count = |k: &str| v.get(k).and_then(Value::as_array).map_or(0, Vec::len).to_string();
        let text =
            crate::i18n::format("electronics.export_jlc_unverified", &[("path", &first), ("opens", &count("opens")), ("shorts", &count("shorts"))]);
        app.set_status(text, true);
        return;
    }
    let rows = files.first().and_then(|f| f.get("rows")).and_then(Value::as_u64);
    let text = match (files.as_slice(), rows) {
        // The parts list's CSV: its rows, not pages.
        ([_], Some(rows)) => {
            let key = if rows == 1 { "electronics.export_done_csv_one" } else { "electronics.export_done_csv" };
            crate::i18n::format(key, &[("rows", &rows.to_string()), ("path", &first), ("size", &size)])
        }
        ([one], None) => {
            let pages = one.get("pages").and_then(Value::as_array).map_or(0, Vec::len);
            crate::i18n::format("electronics.export_done_one", &[("path", &first), ("pages", &pages.to_string()), ("size", &size)])
        }
        (many, _) => crate::i18n::format("electronics.export_done_many", &[("files", &many.len().to_string()), ("path", &first), ("size", &size)]),
    };
    app.set_status(text, false);
}

/// File > Export Parts List: where to save the parts list's CSV, then `ecad.export.pages` writes it
/// (the status line says how many rows), whatever the Export window's choices are. Not while
/// another save is being written (its status line would be lost).
pub(crate) fn save_parts_list(app: &mut SolveApp) {
    if saving(app) {
        return;
    }
    let name = format!("{}-parts.csv", file_stem(app));
    app.pick_save(&name, &["csv"], move |app, path| {
        let Some(path) = path else { return };
        if !saving(app) {
            write(app, json!({"format": "csv", "path": path}));
        }
    });
}

/// Whether a save of pages or the parts list is being written.
pub(crate) fn saving(app: &SolveApp) -> bool {
    app.electronics.design.export.saving.is_some()
}

/// Open Export Pages (File > Export Pages…); the Add Part window gives way.
pub(crate) fn open(app: &mut SolveApp) {
    app.electronics.design.open = Some(crate::electronics_design::Open::Export);
    app.electronics.sheet.choosing = false;
}

/// What the window was asked to do.
enum Act {
    Pick(usize),
    Info(&'static str, String),
    /// The whole revision history as it should be, the revision to pick then, and the title
    /// block's fields that follow the newest one (its revision and date), in the same call.
    Revisions(Vec<Revision>, Option<usize>, Vec<(&'static str, String)>),
    PickRevision(usize),
    Save(Value),
    /// Save JLCPCB Order Files….
    SaveJlc,
    /// Work the pages out again for the design as it is now (the browser).
    Refresh,
}

/// The Export Pages window (see the module notes).
pub(crate) fn window(app: &mut SolveApp, ctx: &egui::Context) {
    let sheet = &app.session.doc.ecad.schematic;
    let frames: Vec<(u64, String)> =
        solidcraft_engine::doc::ecad::frame::in_order(&sheet.frames).into_iter().map(|f| (f.id, f.title.clone())).collect();
    let ids: Vec<u64> = frames.iter().map(|(id, _)| *id).collect();
    let params = app.electronics.design.export.settings.params(&ids);
    let room = app.electronics.design.export.room.map_or(PREVIEW_POINTS, |w| (w / PREVIEW_STEP).floor().max(1.0) * PREVIEW_STEP);
    let width = (room.max(PREVIEW_LEAST) * ctx.pixels_per_point()).round().clamp(64.0, 4096.0) as u32;
    let live = params.as_ref().map(|p| (app.session.uid, app.session.revision, p.to_string()));
    // In the browser the pages in hand stay through a change to the design (not to the choices)
    // until Refresh: working them out there holds the window up.
    let key = match (&live, &app.electronics.design.export.composed) {
        (Some(l), Some(c)) if !BESIDE && c.key.0 == l.0 && c.key.2 == l.2 => Some(c.key.clone()),
        _ => live.clone(),
    };
    let stale = key != live;
    if let (Some(key), Some(p)) = (&key, &params) {
        refresh(app, key, p, width);
    }
    let info = app.session.doc.ecad.info.clone();
    let printed = solidcraft_engine::cmd::title_info(&app.session);
    let title = tr("electronics.export_title");
    // In the order asked: a title block field given up as Save… is pressed is changed first.
    let mut acts: Vec<Act> = Vec::new();
    let mut settings = app.electronics.design.export.settings.clone();
    let mut shown_room = None;
    let closed = ui_kit::Window::new(egui::Id::new("electronics_export"), "export", &title)
        .placed(Align2::RIGHT_TOP, crate::electronics_design::beside(app, ctx))
        .cards(1.75)
        .resizable()
        .show_body(ctx, |ui| {
            // A board with an outline can be ordered: its order files come first, above the pages.
            if !app.session.doc.ecad.board.outline.is_empty() {
                ui_kit::text(ui, &tr("electronics.export_order_heading"), ui_kit::Text::Heading("electronics.export_order_heading"));
                ui_kit::text(ui, &tr("electronics.export_jlc_note"), ui_kit::Text::Note("electronics.export_jlc_note"));
                let can = app.electronics.design.export.saving.is_none() && !app.file_picker_busy();
                if crate::electronics_editor::button(ui, "export-jlc", &tr("electronics.export_jlc"), can) {
                    acts.push(Act::SaveJlc);
                }
                ui.add_space(Tokens::get().space_8);
            }
            choices(ui, &mut settings, &frames);
            ui.add_space(Tokens::get().space_8);
            title_block(app, ui, &info, &printed, &mut acts);
            ui.add_space(Tokens::get().space_6);
            let picked = app.electronics.design.export.revision;
            revisions(app, ui, &info, picked, &mut acts);
            ui.add_space(Tokens::get().space_8);
            let e = &app.electronics.design.export;
            let working = e.job.is_some();
            let composed = e.composed.as_ref().filter(|c| Some(&c.key) == key.as_ref());
            let ready = match (&params, composed) {
                (None, _) => {
                    ui_kit::text(ui, &tr("electronics.export_nothing"), ui_kit::Text::Error("electronics.export_nothing"));
                    false
                }
                (Some(_), None) => {
                    ui_kit::text(ui, &tr("electronics.export_working"), ui_kit::Text::Note("electronics.export_working"));
                    false
                }
                (Some(_), Some(c)) => match &c.reply {
                    Ok((book, reply)) => {
                        if stale {
                            ui_kit::text(ui, &tr("electronics.export_stale"), ui_kit::Text::Note("electronics.export_stale"));
                            if crate::electronics_editor::button(ui, "export-refresh", &tr("electronics.export_refresh"), !working) {
                                acts.push(Act::Refresh);
                            }
                        }
                        let page = e.page.min(book.pages.len().saturating_sub(1));
                        list(ui, reply, &frames, page, e.settings.format == Format::Csv, &mut acts);
                        if !book.pages.is_empty() {
                            let number = book.entries.get(page).map_or(page + 1, |x| x.number);
                            shown_room = Some(ui.available_width());
                            preview(ui, e, key.as_ref(), page, number, working);
                        }
                        !book.pages.is_empty()
                    }
                    Err(why) => {
                        ui_kit::text(ui, why, ui_kit::Text::Error("electronics.export_refused"));
                        false
                    }
                },
            };
            ui.add_space(Tokens::get().space_8);
            let busy = app.electronics.design.export.saving.is_some();
            if busy {
                ui_kit::text(ui, &tr("electronics.export_writing"), ui_kit::Text::Note("electronics.export_writing"));
            }
            let ok = ready && !busy && !app.file_picker_busy();
            if crate::electronics_editor::button(ui, "export-save", &tr("electronics.export_save"), ok)
                && let Some(p) = params.clone()
            {
                acts.push(Act::Save(p));
            }
        });
    let e = &mut app.electronics.design.export;
    if e.settings != settings {
        e.settings = settings;
    }
    if let Some(w) = shown_room.filter(|w| w.is_finite() && *w > 0.0) {
        e.room = Some(w);
    }
    for act in acts {
        match act {
            Act::Pick(page) => app.electronics.design.export.page = page,
            Act::Info(field, text) => {
                let mut p = json!({});
                p[field] = json!(text.trim());
                drop(app.run("ecad.design.info", p));
            }
            Act::Revisions(list, pick, fields) => {
                let mut p = json!({ "revisions": list });
                for (field, value) in fields {
                    p[field] = json!(value);
                }
                if app.run("ecad.design.info", p).is_ok() {
                    app.electronics.design.export.revision = pick;
                }
            }
            Act::PickRevision(k) => app.electronics.design.export.revision = Some(k),
            Act::Save(params) => save(app, params),
            Act::SaveJlc => save_jlc(app),
            Act::Refresh => app.electronics.design.export.composed = None,
        }
    }
    if closed {
        app.electronics.design.open = None;
    }
}

/// The kinds of page, the frames that get a page, paper, orientation, style, format, resolution and
/// power cross-references.
fn choices(ui: &mut Ui, s: &mut Settings, frames: &[(u64, String)]) {
    ui_kit::wrapped_row(ui, &tr("electronics.export_field_format"), |ui| {
        for f in Format::ALL {
            let label = tr(&format!("electronics.export_format_{}", f.name()));
            if ui_kit::choice_chip(ui, &format!("export:format:{}", f.name()), &label, s.format == f).clicked() {
                s.format = f;
            }
        }
    });
    let help = format!("electronics.export_format_{}_help", s.format.name());
    ui_kit::text(ui, &tr(&help), ui_kit::Text::Note("electronics.export_format_help"));
    if s.format == Format::Csv {
        return;
    }
    ui.add_space(Tokens::get().space_6);
    ui_kit::text(ui, &tr("electronics.export_pages_heading"), ui_kit::Text::Heading("electronics.export_pages_heading"));
    let labels: Vec<String> = PageKind::ALL.iter().map(|k| tr(&format!("electronics.export_page_{}", k.name()))).collect();
    let mut ticks: Vec<bool> = PageKind::ALL.iter().map(|k| !s.off.contains(k.name())).collect();
    {
        let mut items: Vec<(&str, &mut bool)> = labels.iter().map(String::as_str).zip(ticks.iter_mut()).collect();
        ui_kit::checkbox_grid(ui, &mut items, 3);
    }
    for (k, on) in PageKind::ALL.iter().zip(&ticks) {
        if *on {
            s.off.remove(k.name());
        } else {
            s.off.insert(k.name());
        }
    }
    ui_kit::text(ui, &tr("electronics.export_pages_help"), ui_kit::Text::Note("electronics.export_pages_help"));
    // The frames that get a section page each.
    if !frames.is_empty() && !s.off.contains(PageKind::Sections.name()) {
        let titles: Vec<&str> = frames.iter().map(|(_, t)| t.as_str()).collect();
        let mut ticks: Vec<bool> = frames.iter().map(|(id, _)| !s.frames_off.contains(id)).collect();
        ui_kit::text(ui, &tr("electronics.export_frames_heading"), ui_kit::Text::Heading("electronics.export_frames_heading"));
        {
            let mut items: Vec<(&str, &mut bool)> = titles.iter().copied().zip(ticks.iter_mut()).collect();
            ui_kit::checkbox_grid(ui, &mut items, 2);
        }
        for ((id, _), on) in frames.iter().zip(&ticks) {
            if *on {
                s.frames_off.remove(id);
            } else {
                s.frames_off.insert(*id);
            }
        }
    }
    s.frames_off.retain(|id| frames.iter().any(|(f, _)| f == id));
    ui.add_space(Tokens::get().space_6);
    let named = |prefix: &str, list: &[&str]| -> Vec<String> { list.iter().map(|n| tr(&format!("electronics.export_{prefix}_{n}"))).collect() };
    for (field, id, list, at) in [
        ("electronics.export_field_paper", "electronics_export_paper", named("paper", &PAPERS), &mut s.paper),
        ("electronics.export_field_orientation", "electronics_export_orientation", named("orientation", &ORIENTATIONS), &mut s.orientation),
        ("electronics.export_field_style", "electronics_export_style", named("style", &STYLES), &mut s.style),
    ] {
        let options: Vec<&str> = list.iter().map(String::as_str).collect();
        ui_kit::choice_row(ui, &tr(field), id, at, &options);
    }
    if s.paper == 0 {
        ui_kit::text(ui, &tr("electronics.export_paper_auto_help"), ui_kit::Text::Note("electronics.export_paper_auto_help"));
    }
    if s.format == Format::Png {
        let mut dpi = s.dpi;
        ui_kit::row(ui, &tr("electronics.export_field_dpi"), |ui| {
            ui_kit::number_field(ui, &mut dpi, *pages::DPI.start()..=*pages::DPI.end(), 1.0, "");
        });
        if dpi.is_finite() {
            s.dpi = dpi.clamp(*pages::DPI.start(), *pages::DPI.end()).round();
        }
    }
    if ui_kit::checkbox_row(ui, "electronics_export_power_xrefs", &tr("electronics.export_power_xrefs"), s.power_xrefs) {
        s.power_xrefs = !s.power_xrefs;
    }
}

/// The title block's fields, as saved; one left empty shows what prints in its place.
fn title_block(app: &mut SolveApp, ui: &mut Ui, info: &solidcraft_engine::doc::ecad::DesignInfo, printed: &pages::TitleInfo, acts: &mut Vec<Act>) {
    ui_kit::text(ui, &tr("electronics.export_info_heading"), ui_kit::Text::Heading("electronics.export_info_heading"));
    for (field, saved, shown) in [
        ("title", &info.title, &printed.title),
        ("revision", &info.revision, &printed.revision),
        ("date", &info.date, &printed.date),
        ("author", &info.author, &printed.author),
        ("company", &info.company, &printed.company),
        ("notes", &info.notes, &printed.notes),
    ] {
        let key = format!("export:info:{field}");
        let mut text = crate::electronics_editor::typed(app, &key, saved);
        let label = tr(&format!("electronics.export_info_{field}"));
        let hint = if saved.is_empty() { shown.as_str() } else { "" };
        let response = ui_kit::row(ui, &label, |ui| ui_kit::cell_editor(ui, egui::Id::new(("electronics_field", &key)), &mut text, hint, false));
        if let Some(v) = crate::electronics_editor::settle(app, ui, &key, &response, text, saved) {
            acts.push(Act::Info(field, v));
        }
    }
    ui_kit::text(ui, &tr("electronics.export_info_help"), ui_kit::Text::Note("electronics.export_info_help"));
}

/// The name after `rev` for a new revision: the next letter (`B` after `A`), the next number (`1.3`
/// after `1.2`), else `A` when there is none.
fn next_rev(rev: &str) -> String {
    let rev = rev.trim();
    let digits = rev.len().saturating_sub(rev.trim_end_matches(|c: char| c.is_ascii_digit()).len());
    let cut = rev.len().saturating_sub(digits);
    if digits > 0
        && let (Some(head), Some(tail)) = (rev.get(..cut), rev.get(cut..))
        && let Ok(n) = tail.parse::<u64>()
        && let Some(n) = n.checked_add(1)
    {
        return format!("{head}{n}");
    }
    match rev.chars().last() {
        Some(c) if c.is_ascii_alphabetic() && !matches!(c, 'z' | 'Z') => {
            let next = char::from(u8::try_from(u32::from(c) + 1).unwrap_or(b'A'));
            format!("{}{next}", rev.get(..rev.len().saturating_sub(1)).unwrap_or_default())
        }
        Some(_) => format!("{rev}.1"),
        None => "A".to_string(),
    }
}

/// The cover's revision history: a table, newest first (a click picks one), the picked one's
/// fields, Remove and Add Revision; each change sends the whole history (one undo step).
fn revisions(app: &mut SolveApp, ui: &mut Ui, info: &solidcraft_engine::doc::ecad::DesignInfo, picked: Option<usize>, acts: &mut Vec<Act>) {
    ui_kit::text(ui, &tr("electronics.export_revisions_heading"), ui_kit::Text::Heading("electronics.export_revisions_heading"));
    let list = &info.revisions;
    let fields = ["rev", "date", "by", "note"];
    let headers: Vec<String> = fields.iter().map(|f| tr(&format!("electronics.export_revision_{f}"))).collect();
    let shares = [0.6, 1.0, 1.0, 2.0];
    let columns: Vec<ui_kit::Column<'_>> = headers.iter().zip(shares).map(|(h, w)| ui_kit::Column::new(h, w).flex()).collect();
    let mut pick = None;
    ui_kit::table(ui, "electronics_export_revisions", &columns, &tr("electronics.export_revisions_empty"), |ui, table| {
        for (k, r) in list.iter().enumerate().rev() {
            let mut hit = false;
            let response = table.row(ui, &format!("revision:{k}"), picked == Some(k), |row| {
                for text in [&r.rev, &r.date, &r.by, &r.note] {
                    hit |= row.cell(|ui| ui_kit::text(ui, text, ui_kit::Text::Cell)).on_hover_text(text.as_str()).clicked();
                }
            });
            if response.clicked() || hit {
                pick = Some(k);
            }
        }
    });
    if let Some(k) = pick {
        acts.push(Act::PickRevision(k));
    }
    if let Some(k) = picked
        && let Some(r) = list.get(k)
    {
        for (field, value) in fields.iter().zip([&r.rev, &r.date, &r.by, &r.note]) {
            let key = format!("export:revision:{k}:{field}");
            let mut text = crate::electronics_editor::typed(app, &key, value);
            let label = tr(&format!("electronics.export_revision_{field}"));
            let response = ui_kit::row(ui, &label, |ui| ui_kit::cell_editor(ui, egui::Id::new(("electronics_field", &key)), &mut text, "", false));
            if let Some(v) = crate::electronics_editor::settle(app, ui, &key, &response, text, value) {
                let mut next = list.clone();
                let v = v.trim().to_string();
                // The newest revision's name and date are the title block's while they match it.
                let newest = k + 1 == list.len();
                let mut also = Vec::new();
                match *field {
                    "rev" if newest && r.rev == info.revision => also.push(("revision", v.clone())),
                    "date" if newest && r.date == info.date => also.push(("date", v.clone())),
                    _ => {}
                }
                if let Some(slot) = next.get_mut(k) {
                    match *field {
                        "rev" => slot.rev = v,
                        "date" => slot.date = v,
                        "by" => slot.by = v,
                        _ => slot.note = v,
                    }
                }
                acts.push(Act::Revisions(next, Some(k), also));
            }
        }
        if crate::electronics_editor::button(ui, "export-revision-remove", &tr("electronics.export_revision_remove"), true) {
            let next: Vec<Revision> = list.iter().enumerate().filter(|(j, _)| *j != k).map(|(_, r)| r.clone()).collect();
            acts.push(Act::Revisions(next, None, Vec::new()));
        }
    }
    let room = list.len() < MAX_REVISIONS;
    if crate::electronics_editor::button(ui, "export-revision-add", &tr("electronics.export_revision_add"), room) {
        // After the newest: its name moved on, today's fields as the title block has them; the
        // title block (and so the cover and every page) then shows it as the current revision.
        let last = list.last().map_or(info.revision.as_str(), |r| r.rev.as_str());
        let rev = if list.is_empty() && !info.revision.trim().is_empty() { info.revision.trim().to_string() } else { next_rev(last) };
        let also = if rev == info.revision { Vec::new() } else { vec![("revision", rev.clone())] };
        let mut next = list.clone();
        next.push(Revision { rev, date: info.date.clone(), by: info.author.clone(), note: String::new() });
        let k = next.len().saturating_sub(1);
        acts.push(Act::Revisions(next, Some(k), also));
    }
    ui_kit::text(ui, &tr("electronics.export_revisions_help"), ui_kit::Text::Note("electronics.export_revisions_help"));
}

/// A page's name as the list shows it, in the interface's language: a section page by its frame's
/// title, the others by their kind, numbered `k` of `n` where a kind takes several pages.
fn page_name(row: &Value, frames: &[(u64, String)], k: usize, n: usize) -> String {
    let kind = row.get("kind").and_then(Value::as_str).unwrap_or_default();
    let base = match (kind, row.get("frame").and_then(Value::as_u64)) {
        ("sections", Some(id)) => {
            let saved = row.get("title").and_then(Value::as_str).unwrap_or_default();
            return frames.iter().find(|(f, _)| *f == id).map_or_else(|| saved.to_string(), |(_, t)| t.clone());
        }
        // Without frames the sheet prints as tiles; with them, the parts in none.
        ("sections", None) if frames.is_empty() => tr("electronics.export_row_tile"),
        ("sections", None) => tr("electronics.export_row_other_parts"),
        ("inner_layers", _) => return crate::i18n::format("electronics.export_row_inner", &[("n", &k.to_string())]),
        (kind, _) => tr(&format!("electronics.export_page_{kind}")),
    };
    if n > 1 {
        crate::i18n::format("electronics.export_row_numbered", &[("title", &base), ("k", &k.to_string()), ("n", &n.to_string())])
    } else {
        base
    }
}

/// The pages the choices make: how many, on what paper, each with its paper and scale (a click
/// previews it), then the warnings (none for the parts list's CSV: they are about pages). Every
/// row is shown (the window scrolls); a row off screen publishes no handle.
fn list(ui: &mut Ui, reply: &Value, frames: &[(u64, String)], picked: usize, csv: bool, acts: &mut Vec<Act>) {
    let rows: Vec<&Value> = reply.get("pages").and_then(Value::as_array).map(|p| p.iter().collect()).unwrap_or_default();
    // Pages of one kind (sections without a frame apart) are numbered k of n.
    let group = |row: &Value| (row.get("kind").and_then(Value::as_str).unwrap_or_default().to_string(), row.get("frame").is_some());
    let mut counts: std::collections::BTreeMap<(String, bool), usize> = std::collections::BTreeMap::new();
    for row in &rows {
        *counts.entry(group(row)).or_default() += 1;
    }
    let mut seen: std::collections::BTreeMap<(String, bool), usize> = std::collections::BTreeMap::new();
    let word = |prefix: &str, v: &Value, field: &str| {
        tr(&format!("electronics.export_{prefix}_{}", v.get(field).and_then(Value::as_str).unwrap_or_default()))
    };
    let summary = crate::i18n::format(
        "electronics.export_summary",
        &[("count", &rows.len().to_string()), ("paper", &word("paper", reply, "paper")), ("style", &word("style", reply, "style"))],
    );
    ui_kit::text(ui, &summary, ui_kit::Text::Heading("electronics.export_summary"));
    for (i, row) in rows.iter().enumerate() {
        let n = row.get("page").and_then(Value::as_u64).unwrap_or(i as u64 + 1);
        let g = group(row);
        let k = seen.entry(g.clone()).or_default();
        *k += 1;
        let title = page_name(row, frames, *k, counts.get(&g).copied().unwrap_or(1));
        let mut detail = format!("{} {}", word("paper", row, "paper"), word("orientation", row, "orientation").to_lowercase());
        if let Some(scale) = row.get("scale").and_then(Value::as_str) {
            detail.push_str(&format!(" · {scale}"));
        }
        if let Some([w, h]) = row.get("pixels").and_then(Value::as_array).map(|p| [p.first(), p.get(1)]) {
            let px = |v: Option<&Value>| v.and_then(Value::as_u64).unwrap_or(0).to_string();
            detail.push_str(&format!(" · {} × {} px", px(w), px(h)));
        }
        let response = ui_kit::text(ui, &format!("{n}  {title}"), ui_kit::Text::NameCell(i == picked));
        if ui.is_rect_visible(response.rect) {
            crate::scenario::publish_target(&format!("electronics:export:page:{n}"), response.rect);
        }
        ui_kit::text(ui, &detail, ui_kit::Text::Note("electronics.export_page_row"));
        if response.clicked() {
            acts.push(Act::Pick(i));
        }
    }
    let warnings: Vec<&str> = reply.get("warnings").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    if !warnings.is_empty() && !csv {
        ui.add_space(Tokens::get().space_6);
        let heading = crate::i18n::format("electronics.export_warnings", &[("n", &warnings.len().to_string())]);
        ui_kit::text(ui, &heading, ui_kit::Text::Heading("electronics.export_warnings"));
        for w in warnings {
            ui_kit::text(ui, w, ui_kit::Text::Note("electronics.export_warning"));
        }
    }
}

/// The picked page as it prints, as wide as the window is; the last one drawn while the next
/// is being worked out. `page` is its place in the export, `number` its number in the whole book.
fn preview(ui: &mut Ui, e: &Export, key: Option<&Key>, page: usize, number: usize, working: bool) {
    ui.add_space(Tokens::get().space_6);
    let heading = crate::i18n::format("electronics.export_preview", &[("page", &number.to_string())]);
    ui_kit::text(ui, &heading, ui_kit::Text::Heading("electronics.export_preview"));
    if let Some((_, _, why)) = e.preview_error.as_ref().filter(|(k, p, _)| Some(k) == key && *p == page) {
        ui_kit::text(ui, why, ui_kit::Text::Error("electronics.export_preview_failed"));
        return;
    }
    let Some((k, p, _, texture)) = &e.preview else {
        ui_kit::text(ui, &tr("electronics.export_working"), ui_kit::Text::Note("electronics.export_working"));
        return;
    };
    if (Some(k) != key || *p != page) && working {
        ui_kit::text(ui, &tr("electronics.export_working"), ui_kit::Text::Note("electronics.export_working"));
    }
    let shown = texture.size_vec2();
    let (w, h) = (shown.x, shown.y);
    let wide = ui.available_width().max(1.0);
    let size = egui::vec2(wide, if w > 0.0 { wide * h / w } else { wide });
    let response = ui.add(egui::Image::new(texture).fit_to_exact_size(size).corner_radius(Tokens::get().radius_sm));
    if ui.is_rect_visible(response.rect) {
        crate::scenario::publish_target("electronics:export:preview", response.rect);
    }
}
