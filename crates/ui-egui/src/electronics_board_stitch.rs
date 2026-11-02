//! The Stitch Vias tool (see `electronics_board`): vias of one net (the design intent's ground
//! net, else GND, by default) set out as `ecad.board.stitch` sets them out
//! (`solidcraft_doc::ecad::stitch`): a grid over a clicked pour or the whole board, a fence along
//! a clicked track (a double-click: its whole run, as a pick does) or along the RF nets, or a via
//! beside each signal via that changes reference plane. What is picked is worked out as a dry run
//! beside the window (on a copy of the design) whenever the board or the options change, and
//! shows as ghost vias, with the count, what was skipped and why; Place (or Enter) makes them in
//! one step, Esc lets the pick go. Remove Stitching takes the net's stitching vias of the pattern
//! where the pick is (in the picked pour, beside the picked tracks, by the signal vias, on the
//! whole board picked; every pattern's on the whole board with nothing picked; every pattern's
//! under the parts picked, by a Shift-click or before the tool was chosen) away, in one step
//! too, and says how many first. A via stitched before vias kept their pattern may be of any: a
//! pick leaves those and counts them, and a second button takes them too.

use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Pos2, Stroke, Ui};
use serde_json::{Value, json};
use solidcraft_engine::doc::ecad::board::check_via;
use solidcraft_engine::doc::ecad::intent::NetRole;
use solidcraft_engine::doc::ecad::stitch::{self, Pattern};
use solidcraft_engine::doc::ecad::{ViaProtection, ViaRole};

use crate::SolveApp;
use crate::electronics_board::{Tool, VIA_PROTECTIONS, active_layer, board, stored_nets};
use crate::electronics_board_route::{self as route, ViaSpan};
use crate::electronics_editor::tr;
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;
use crate::ui_kit;

/// What the tool works on.
#[derive(Clone, Debug, PartialEq)]
pub enum StitchPick {
    Board,
    Pour(u64),
    Tracks(Vec<u64>),
    Nets(Vec<String>),
    LayerChanges,
    /// Footprints, by id: Remove Stitching takes the vias under them (nothing is placed).
    Parts(Vec<u64>),
}

/// The dry run of the pick as the options and the board were: where the vias would go, how many
/// spots were skipped by why and why the rest is as it is, or why the call was refused.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub(crate) key: (u64, u64),
    params: Value,
    pub spots: Vec<[f64; 2]>,
    pub skipped: Vec<(String, usize)>,
    pub why: Vec<String>,
    /// A fence's tracks that got no via.
    pub unfenced: usize,
    /// Needed stitching: the planes' weakest spot before and after, in millivolts, and whether
    /// the target was reached.
    pub ground: Option<(Option<f64>, Option<f64>, bool)>,
    pub error: Option<String>,
}

/// A dry run being worked out beside the window, for the document revision and parameters.
#[derive(Debug)]
struct Running {
    key: (u64, u64),
    params: Value,
    rx: Receiver<Preview>,
    /// When it started: the note says how long it has been working.
    started: web_time::Instant,
}

/// The stitching vias Remove Stitching takes for a pick, worked out for the document revision,
/// the pick, the pattern and the net; `whole` when that is all of the net's. `older`: those where
/// the pick is with no pattern on record (stitched before vias kept it), which it leaves.
#[derive(Clone, Debug, PartialEq)]
struct Removal {
    key: (u64, u64),
    pick: Option<StitchPick>,
    pattern: Pattern,
    net: Option<String>,
    ids: Vec<u64>,
    older: Vec<u64>,
    whole: bool,
}

/// The tool's options.
#[derive(Debug)]
pub struct StitchOptions {
    /// The net, by name; the design intent's ground net (else GND) while `None`.
    pub net: Option<String>,
    pub pattern: Pattern,
    /// Between neighbouring vias; the pattern's own while `None`.
    pub pitch: Option<f64>,
    /// Diameter and drill typed; while `None`, the net class's (the rules' micro via between
    /// neighbouring layers).
    pub via: Option<[f64; 2]>,
    pub span: ViaSpan,
    /// What fills or covers the holes: tented, as stitching vias usually are, by default.
    pub protection: ViaProtection,
    /// A grid's or fence's vias may stand under parts (they keep out of courtyards by default).
    pub under_parts: bool,
    /// Needed stitching's most vias; the command's own while `None`.
    pub budget: Option<usize>,
    pub pick: Option<StitchPick>,
    pub search: String,
    pub preview: Option<Preview>,
    running: Option<Running>,
    /// The next preview that comes back goes to the status line (the pick was just made).
    announce: bool,
    removal: Option<Removal>,
}

impl Default for StitchOptions {
    fn default() -> Self {
        StitchOptions {
            net: None,
            pattern: Pattern::Grid,
            pitch: None,
            via: None,
            span: ViaSpan::Through,
            protection: ViaProtection::Tented,
            under_parts: false,
            budget: None,
            pick: None,
            search: String::new(),
            preview: None,
            running: None,
            announce: false,
            removal: None,
        }
    }
}

const GROUND: &str = "GND";

/// The net the vias go on: the one chosen, else the design intent's ground net, else GND, when
/// the design has it.
pub(crate) fn net(app: &SolveApp) -> Option<String> {
    if let Some(n) = app.electronics.board.stitch.net.clone() {
        return Some(n);
    }
    let nets = stored_nets(app);
    let has = |n: &str| nets.names.iter().any(|m| m == n);
    let ground = app.session.doc.ecad.intent.nets.iter().filter(|r| r.role == NetRole::Ground).map(|r| r.net.as_str()).find(|n| has(n));
    ground.or(has(GROUND).then_some(GROUND)).map(str::to_string)
}

/// The rules' micro via is the default between two neighbouring layers.
fn micro_span(app: &SolveApp) -> bool {
    let n = board(app).copper_layers;
    matches!(app.electronics.board.stitch.span, ViaSpan::Between(a, b) if a.index(n).abs_diff(b.index(n)) == 1 && (a.is_inner() || b.is_inner()))
}

/// The vias' diameter and drill: those typed, else the rules' micro via between neighbouring
/// layers, else the net class's.
fn via_size(app: &SolveApp) -> [f64; 2] {
    if let Some(v) = app.electronics.board.stitch.via {
        return v;
    }
    let rules = &board(app).rules;
    if micro_span(app) {
        return [rules.microvia_diameter, rules.microvia_drill];
    }
    let sizes = rules.sizes(net(app).as_deref());
    [sizes.via_diameter, sizes.via_drill]
}

/// What the tool works on: the pick, else, for the patterns that work on the whole board, the
/// whole board ("Where needed") or every layer change. The pick goes after Place, a net change
/// or a part picked and let go, while the pattern stays: the preview must not go with it.
fn effective_pick(app: &SolveApp) -> Option<StitchPick> {
    let o = &app.electronics.board.stitch;
    o.pick.clone().or(match o.pattern {
        Pattern::Needed => Some(StitchPick::Board),
        Pattern::LayerChange => Some(StitchPick::LayerChanges),
        Pattern::Grid | Pattern::Fence => None,
    })
}

/// `ecad.board.stitch`'s parameters for the pick (without `dry_run`); `None` with nothing picked
/// or no net. The via's size goes only when typed, so the command picks the class's or the micro
/// via and grows a ring the fab profile would not pass.
fn params(app: &SolveApp) -> Option<Value> {
    let o = &app.electronics.board.stitch;
    let pick = &effective_pick(app)?;
    let net = net(app)?;
    let mut via = json!({});
    if let Some([diameter, drill]) = o.via {
        via = json!({"diameter": diameter, "drill": drill});
    }
    if let ViaSpan::Between(a, b) = o.span {
        via["from"] = json!(a.name());
        via["to"] = json!(b.name());
    }
    let pattern = match o.pattern {
        Pattern::Grid => "grid",
        Pattern::Fence => "fence",
        Pattern::LayerChange => "layer_change",
        Pattern::Needed => "needed",
    };
    let mut p = json!({"net": net, "pattern": pattern, "via": via, "protection": o.protection.name()});
    if let (Pattern::Needed, Some(b)) = (o.pattern, o.budget) {
        p["budget"] = json!(b);
    }
    if let Some(pitch) = o.pitch {
        p["pitch"] = json!(pitch);
    }
    if o.under_parts && o.pattern != Pattern::LayerChange {
        p["under_parts"] = json!(true);
    }
    match pick {
        StitchPick::Board | StitchPick::LayerChanges => {}
        StitchPick::Pour(id) => p["region"] = json!({"pour": id}),
        StitchPick::Tracks(ids) => p["tracks"] = json!(ids),
        StitchPick::Nets(names) => p["nets"] = json!(names),
        StitchPick::Parts(_) => return None,
    }
    Some(p)
}

/// The preview a dry run's reply (or refusal) makes.
fn preview_of(key: (u64, u64), params: Value, reply: Result<Value, String>) -> Preview {
    match reply {
        Ok(v) => {
            let spots = v
                .get("would_create")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|q| serde_json::from_value(q.clone()).ok())
                .collect();
            let mut skipped: Vec<(String, usize)> = v
                .get("skipped")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .filter_map(|(k, n)| Some((k.clone(), usize::try_from(n.as_u64()?).ok()?)))
                .collect();
            skipped.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let why = v.get("why").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            let unfenced = v.get("unfenced").and_then(Value::as_array).map_or(0, Vec::len);
            let ground = v.get("ground").filter(|g| g.is_object()).map(|g| {
                let worst = |k: &str| g.get(k).and_then(|t| t.get("worst_mv")).and_then(Value::as_f64);
                (worst("before"), worst("after"), g.get("reached").and_then(Value::as_bool).unwrap_or(false))
            });
            Preview { key, params, spots, skipped, why, unfenced, ground, error: None }
        }
        Err(e) => Preview { key, params, spots: Vec::new(), skipped: Vec::new(), why: Vec::new(), unfenced: 0, ground: None, error: Some(e) },
    }
}

/// Run `job` beside the window; its answer comes on the channel. The browser runs it at once, and
/// so does a desktop that cannot start a thread.
fn spawn(job: impl FnOnce() -> Preview + Send + 'static) -> Receiver<Preview> {
    let (tx, rx) = std::sync::mpsc::channel();
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(job)));
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (theirs, sender) = (slot.clone(), tx.clone());
        let started = std::thread::Builder::new().name("stitch-preview".into()).spawn(move || {
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

/// The dry run of the pick, worked out again beside the window when the board or the options
/// changed (looked at each frame the tool is on): the last one shows until the new one is back.
/// And what Remove Stitching would take.
pub(crate) fn refresh(app: &mut SolveApp) {
    if !matches!(app.electronics.board.tool, Some(Tool::Stitch)) {
        app.electronics.board.stitch.running = None;
        return;
    }
    let key = (app.session.uid, app.session.revision);
    let back = match app.electronics.board.stitch.running.as_ref().map(|r| r.rx.try_recv()) {
        Some(Ok(p)) => Some(Some(p)),
        Some(Err(TryRecvError::Disconnected)) => Some(None),
        Some(Err(TryRecvError::Empty)) | None => None,
    };
    if let Some(back) = back {
        let s = &mut app.electronics.board.stitch;
        s.running = None;
        if let Some(p) = back {
            s.preview = Some(p);
            if std::mem::take(&mut s.announce) {
                let text = summary(app);
                let error = app.electronics.board.stitch.preview.as_ref().is_some_and(|x| x.error.is_some());
                app.set_status(text, error);
            }
        }
    }
    refresh_removal(app, key);
    let Some(p) = params(app) else {
        let s = &mut app.electronics.board.stitch;
        s.preview = None;
        s.running = None;
        return;
    };
    let s = &app.electronics.board.stitch;
    if s.preview.as_ref().is_some_and(|x| x.key == key && x.params == p) || s.running.as_ref().is_some_and(|r| r.key == key && r.params == p) {
        return;
    }
    // A dry run changes nothing, so it runs on a copy of the design and leaves no trace.
    let mut scratch = app.session.scratch();
    let asked = p.clone();
    let mut dry = p.clone();
    dry["dry_run"] = json!(true);
    let rx = spawn(move || preview_of(key, asked, scratch.execute("ecad.board.stitch", &dry).map_err(|e| e.to_string())));
    // Needed stitching solves the ground map several times: what it found goes to the status.
    if app.electronics.board.stitch.pattern == Pattern::Needed {
        app.electronics.board.stitch.announce = true;
    }
    app.electronics.board.stitch.running = Some(Running { key, params: p, rx, started: web_time::Instant::now() });
}

/// Whether a dry run is being worked out.
fn working(app: &SolveApp) -> bool {
    app.electronics.board.stitch.running.is_some()
}

/// The net's stitching vias of the pattern where the pick is (see the module notes), worked out
/// again when the board, the pick, the pattern or the net changed.
fn refresh_removal(app: &mut SolveApp, key: (u64, u64)) {
    // Parts gone since they were picked (an undo, a schematic edit, an agent's) leave the pick;
    // with none left, so does it.
    let gone = match &mut app.electronics.board.stitch.pick {
        Some(StitchPick::Parts(parts)) => {
            let ecad = &app.session.doc.ecad;
            parts.retain(|id| ecad.board.footprint(*id).is_some());
            parts.is_empty()
        }
        _ => false,
    };
    if gone {
        app.electronics.board.stitch.pick = None;
    }
    let pick = app.electronics.board.stitch.pick.clone();
    let pattern = app.electronics.board.stitch.pattern;
    let name = net(app);
    if app.electronics.board.stitch.removal.as_ref().is_some_and(|r| r.key == key && r.pick == pick && r.pattern == pattern && r.net == name) {
        return;
    }
    let b = board(app);
    let nets = stored_nets(app);
    let ours = |v: &solidcraft_engine::doc::ecad::Via| v.role == ViaRole::Stitch && name.is_some() && v.net == name;
    // With something picked, only the pattern's (and those stitched before patterns were kept,
    // which may be of any: told apart below); under parts, every pattern's.
    let any = matches!(pick, None | Some(StitchPick::Parts(_)));
    let kept = |v: &solidcraft_engine::doc::ecad::Via| any || v.pattern.is_none_or(|p| p == pattern);
    let mine: Vec<&solidcraft_engine::doc::ecad::Via> = b.vias.iter().filter(|v| ours(v) && kept(v)).collect();
    let mut whole = false;
    let ids: Vec<u64> = match &pick {
        None => {
            whole = true;
            mine.iter().map(|v| v.id).collect()
        }
        Some(StitchPick::Board) => mine.iter().map(|v| v.id).collect(),
        Some(StitchPick::Pour(id)) => match nets.fills.iter().find(|f| f.pour == *id) {
            Some(f) => {
                let at: Vec<[f64; 2]> = mine.iter().map(|v| v.at).collect();
                mine.iter().zip(f.region.locate(&at)).filter(|(_, h)| h.is_some()).map(|(v, _)| v.id).collect()
            }
            None => Vec::new(),
        },
        Some(StitchPick::Tracks(_) | StitchPick::Nets(_)) => {
            // Beside the picked tracks: within the track's half width, the widest clearance, a
            // via and a millimetre more (a fence's offset) of their centreline.
            let picked: BTreeSet<u64> = match &pick {
                Some(StitchPick::Tracks(ids)) => ids.iter().copied().collect(),
                Some(StitchPick::Nets(names)) => {
                    let wanted: BTreeSet<usize> = nets.names.iter().enumerate().filter(|(_, n)| names.contains(n)).map(|(i, _)| i).collect();
                    b.tracks.iter().zip(&nets.track_nets).filter(|(_, n)| n.is_some_and(|n| wanted.contains(&n))).map(|(t, _)| t.id).collect()
                }
                _ => BTreeSet::new(),
            };
            let gap = b.rules.net_classes.iter().map(|c| c.clearance).fold(b.rules.clearance, f64::max);
            let tracks: Vec<_> = b.tracks.iter().filter(|t| picked.contains(&t.id)).collect();
            mine.iter()
                .filter(|v| {
                    tracks.iter().any(|t| {
                        let reach = t.width / 2.0 + gap + v.diameter + 1.0;
                        solidcraft_engine::doc::ecad::path::segments(&t.points, &t.curves, false)
                            .any(|(a, q, s)| solidcraft_engine::doc::ecad::path::segment_distance(v.at, a, q, s) <= reach)
                    })
                })
                .map(|v| v.id)
                .collect()
        }
        Some(StitchPick::Parts(parts)) => {
            let parts: BTreeSet<u64> = parts.iter().copied().collect();
            let under = stitch::under_parts(&app.session.doc.ecad, &parts, &mine);
            mine.iter().zip(under).filter(|(_, u)| *u).map(|(v, _)| v.id).collect()
        }
        Some(StitchPick::LayerChanges) => {
            // By the signal vias: within a layer change's reach of a via of another net.
            let cell = |p: [f64; 2]| ((p[0] / stitch::CHANGE_REACH).floor() as i64, (p[1] / stitch::CHANGE_REACH).floor() as i64);
            let mut cells: HashMap<(i64, i64), Vec<[f64; 2]>> = HashMap::new();
            for v in b.vias.iter().filter(|v| v.role.is_plain() && v.net != name) {
                cells.entry(cell(v.at)).or_default().push(v.at);
            }
            mine.iter()
                .filter(|v| {
                    let (cx, cy) = cell(v.at);
                    (cx - 1..=cx + 1).any(|x| {
                        (cy - 1..=cy + 1).any(|y| {
                            cells
                                .get(&(x, y))
                                .is_some_and(|list| list.iter().any(|q| (q[0] - v.at[0]).hypot(q[1] - v.at[1]) <= stitch::CHANGE_REACH + 1e-6))
                        })
                    })
                })
                .map(|v| v.id)
                .collect()
        }
    };
    let recorded: BTreeSet<u64> = mine.iter().filter(|v| v.pattern.is_some()).map(|v| v.id).collect();
    drop(nets);
    let (ids, older): (Vec<u64>, Vec<u64>) = match &pick {
        None | Some(StitchPick::Parts(_)) => (ids, Vec::new()),
        Some(_) => ids.into_iter().partition(|id| recorded.contains(id)),
    };
    app.electronics.board.stitch.removal = Some(Removal { key, pick, pattern, net: name, ids, older, whole });
}

/// A click with the tool: a grid takes the pour under the pointer (the net's, the active layer's
/// first); a fence the track under it, or with `double` the whole run the track is part of. With
/// `shift`, the part under it joins the parts picked (or leaves them), for Remove Stitching under
/// them.
pub(crate) fn click(app: &mut SolveApp, map: PageMap, p: Pos2, double: bool, shift: bool) {
    if shift {
        if !double {
            pick_part(app, map, p);
        }
        return;
    }
    let pattern = app.electronics.board.stitch.pattern;
    match pattern {
        Pattern::Grid | Pattern::Needed => {
            let (x, y) = map.to_page(p);
            let nets = stored_nets(app);
            let want = net(app).and_then(|n| nets.names.iter().position(|m| *m == n));
            let active = active_layer(app);
            let mut fills: Vec<_> = nets.fills.iter().filter(|f| want.is_some() && f.net == want).collect();
            fills.sort_by_key(|f| f.layer != active);
            let hit = fills.into_iter().find(|f| f.region.locate(&[[x, y]]).first().is_some_and(Option::is_some)).map(|f| f.pour);
            drop(nets);
            match hit {
                Some(id) => pick(app, StitchPick::Pour(id)),
                None => app.set_status(tr("electronics.stitch_no_pour"), true),
            }
        }
        Pattern::Fence => {
            let Some(id) = route::copper_at(app, map, p, true).filter(|id| board(app).track(*id).is_some()) else {
                app.set_status(tr("electronics.stitch_no_track"), true);
                return;
            };
            let ids = if double { run_of(app, id) } else { vec![id] };
            pick(app, StitchPick::Tracks(ids));
        }
        Pattern::LayerChange => {}
    }
}

/// Shift-click: the part under `p` joins the parts picked, or leaves them; the status line says
/// how many stitching vias are under them.
fn pick_part(app: &mut SolveApp, map: PageMap, p: Pos2) {
    if net(app).is_none() {
        app.set_status(tr("electronics.stitch_no_net"), true);
        return;
    }
    let Some(id) = crate::electronics_board::footprint_at(app, map, p) else {
        app.set_status(tr("electronics.stitch_no_part"), true);
        return;
    };
    let mut parts = match app.electronics.board.stitch.pick.take() {
        Some(StitchPick::Parts(parts)) => parts,
        _ => Vec::new(),
    };
    if let Some(i) = parts.iter().position(|x| *x == id) {
        parts.remove(i);
    } else {
        parts.push(id);
    }
    let s = &mut app.electronics.board.stitch;
    s.preview = None;
    s.running = None;
    s.pick = (!parts.is_empty()).then_some(StitchPick::Parts(parts));
    let key = (app.session.uid, app.session.revision);
    refresh_removal(app, key);
    if matches!(app.electronics.board.stitch.pick, Some(StitchPick::Parts(_))) {
        let n = app.electronics.board.stitch.removal.as_ref().map_or(0, |r| r.ids.len());
        let text = crate::i18n::format("electronics.stitch_under_parts_found", &[("n", &n.to_string()), ("parts", &parts_name(app))]);
        app.set_status(text, false);
    }
}

/// The picked parts by their designators (`U2, U3`), or how many when there are more than a few.
fn parts_name(app: &SolveApp) -> String {
    let Some(StitchPick::Parts(ids)) = &app.electronics.board.stitch.pick else { return String::new() };
    let ecad = &app.session.doc.ecad;
    if ids.len() > 3 {
        return crate::i18n::format("electronics.board_objects", &[("n", &ids.len().to_string())]);
    }
    let names: Vec<String> = ids
        .iter()
        .map(|id| ecad.board.footprint(*id).and_then(|f| ecad.schematic.part(f.part)).map_or_else(|| id.to_string(), |p| p.designator.clone()))
        .collect();
    names.join(", ")
}

/// The tracks of the routed run track `id` is part of.
fn run_of(app: &SolveApp, id: u64) -> Vec<u64> {
    let b = board(app);
    let runs = app.session.doc.ecad.board_runs(b);
    let Some(run) = b.tracks.iter().position(|t| t.id == id).and_then(|i| runs.tracks.get(i)).copied() else { return vec![id] };
    b.tracks.iter().zip(&runs.tracks).filter(|(_, r)| **r == run).map(|(t, _)| t.id).collect()
}

/// Pick `what`: its dry run starts, and the status line says what it finds when it is back.
fn pick(app: &mut SolveApp, what: StitchPick) {
    if net(app).is_none() {
        app.set_status(tr("electronics.stitch_no_net"), true);
        return;
    }
    let s = &mut app.electronics.board.stitch;
    s.pick = Some(what);
    s.preview = None;
    s.announce = true;
    refresh(app);
    if working(app) {
        app.set_status(tr("electronics.stitch_working"), false);
    }
}

/// "38 vias, 6 spots skipped: 4 too close to other copper, 2 in pads", or why the pick can't be
/// stitched.
fn summary(app: &SolveApp) -> String {
    let Some(x) = &app.electronics.board.stitch.preview else { return String::new() };
    if let Some(e) = &x.error {
        return e.clone();
    }
    let n = x.spots.len().to_string();
    let total: usize = x.skipped.iter().map(|(_, k)| k).sum();
    if total == 0 {
        return crate::i18n::format("electronics.stitch_preview", &[("n", &n)]);
    }
    let reasons: Vec<String> =
        x.skipped.iter().map(|(why, k)| crate::i18n::format(&format!("electronics.stitch_skip_{why}"), &[("n", &k.to_string())])).collect();
    crate::i18n::format("electronics.stitch_preview_skipped", &[("n", &n), ("skipped", &total.to_string()), ("reasons", &reasons.join(", "))])
}

/// Place the previewed vias (Enter, or the Place button): one `ecad.board.stitch`.
pub(crate) fn place(app: &mut SolveApp) {
    let Some(p) = params(app) else {
        let key = if net(app).is_none() { "electronics.stitch_no_net" } else { "electronics.stitch_pick_first" };
        app.set_status(tr(key), net(app).is_none());
        return;
    };
    match app.run("ecad.board.stitch", p) {
        Ok(v) => {
            let n = v.get("count").and_then(Value::as_u64).unwrap_or(0);
            app.set_status(crate::i18n::format("electronics.stitch_placed", &[("n", &n.to_string())]), false);
            let s = &mut app.electronics.board.stitch;
            s.pick = None;
            s.preview = None;
            s.running = None;
        }
        Err(e) => app.set_status(e, true),
    }
}

/// Remove Stitching: the net's stitching vias the button counted (see [`refresh_removal`]); with
/// `older`, those with no pattern on record where the pick is.
fn remove(app: &mut SolveApp, older: bool) {
    let Some(ids) =
        app.electronics.board.stitch.removal.as_ref().map(|r| if older { r.older.clone() } else { r.ids.clone() }).filter(|ids| !ids.is_empty())
    else {
        return;
    };
    // Under parts, by the parts: the same vias, as an agent would name them.
    let params = match (&app.electronics.board.stitch.pick, net(app)) {
        (Some(StitchPick::Parts(parts)), Some(n)) if !older => json!({"net": n, "parts": parts}),
        _ => json!({"ids": ids}),
    };
    match app.run("ecad.board.stitch.remove", params) {
        Ok(v) => {
            let n = v.get("count").and_then(Value::as_u64).unwrap_or(0);
            app.set_status(crate::i18n::format("electronics.stitch_removed", &[("n", &n.to_string())]), false);
        }
        Err(e) => app.set_status(e, true),
    }
}

/// Esc with a pick: it goes (the tool stays). `false` with none.
pub(crate) fn step_back(app: &mut SolveApp) -> bool {
    if !matches!(app.electronics.board.tool, Some(Tool::Stitch)) {
        return false;
    }
    let s = &mut app.electronics.board.stitch;
    s.preview = None;
    s.running = None;
    s.pick.take().is_some()
}

/// The ghost vias of the preview, ringed as the Via tool rings the via it would place; while a
/// new one is being worked out, the last one dimmer.
pub(crate) fn paint(app: &SolveApp, painter: &egui::Painter, map: PageMap, t: &Tokens) {
    let s = &app.electronics.board.stitch;
    if s.running.is_some() {
        painter.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
    // The picked parts, boxed as a pick is.
    if let Some(StitchPick::Parts(ids)) = &s.pick {
        let b = board(app);
        for f in ids.iter().filter_map(|id| b.footprint(*id)) {
            if let Some([lo, hi]) = crate::electronics_board::placed_box(app, f) {
                let r = egui::Rect::from_two_pos(map.to_screen(lo[0], lo[1]), map.to_screen(hi[0], hi[1])).expand(3.0);
                painter.rect_stroke(r, 2.0, Stroke::new(1.5, t.sel_edge), egui::StrokeKind::Outside);
            }
        }
    }
    let Some(x) = &s.preview else { return };
    let [diameter, _] = via_size(app);
    let r = ((diameter / 2.0 * map.scale()) as f32).max(3.0);
    let ghost = t.sel_edge.gamma_multiply(if s.running.is_some() { 0.4 } else { 0.8 });
    for q in &x.spots {
        let c = map.to_screen(q[0], q[1]);
        if c.x.is_finite() && c.y.is_finite() {
            painter.circle_filled(c, r, ghost.gamma_multiply(0.3));
            painter.circle_stroke(c, r, Stroke::new(1.5, ghost));
        }
    }
    // The picked tracks, marked along their middle.
    if let Some(StitchPick::Tracks(ids)) = &s.pick {
        let b = board(app);
        for t_ in ids.iter().filter_map(|id| b.track(*id)) {
            let line: Vec<Pos2> =
                solidcraft_engine::doc::ecad::path::tessellate(&t_.points, &t_.curves, false, crate::electronics_board::arc_tolerance(map))
                    .into_iter()
                    .map(|q| map.to_screen(q[0], q[1]))
                    .collect();
            painter.add(egui::Shape::line(line, Stroke::new(1.5, t.sel_edge)));
        }
    }
}

/// The tool's options in the Inspector: net, pattern, pitch (with the pattern's own and why),
/// the via's size, span and hole, what to stitch, the preview with its reasons and Place, and
/// Remove Stitching. Returns its help's message key.
pub(crate) fn options(app: &mut SolveApp, ui: &mut Ui) -> &'static str {
    let current = net(app);
    let mut search = app.electronics.board.stitch.search.clone();
    let chosen =
        crate::electronics_board_pour::net_row(app, ui, "electronics_stitch_net", &tr("electronics.field_net"), current.as_deref(), &mut search);
    app.electronics.board.stitch.search = search;
    if let Some(n) = chosen {
        app.electronics.board.stitch.net = n;
        app.electronics.board.stitch.pick = None;
    }
    if current.is_none() {
        ui_kit::text(ui, crate::i18n::tr("electronics.stitch_no_net"), ui_kit::Text::Note("electronics.stitch_no_net"));
    }
    let patterns = [
        (Pattern::Grid, "electronics.stitch_grid"),
        (Pattern::Fence, "electronics.stitch_fence"),
        (Pattern::LayerChange, "electronics.stitch_layer_change"),
        (Pattern::Needed, "electronics.stitch_needed"),
    ];
    let names: Vec<String> = patterns.iter().map(|(_, k)| tr(k)).collect();
    let at = patterns.iter().position(|(p, _)| *p == app.electronics.board.stitch.pattern).unwrap_or(0);
    if let Some((p, _)) = crate::electronics_editor::choice(ui, "electronics_stitch_pattern", &tr("electronics.field_pattern"), at, &names)
        .and_then(|i| patterns.get(i))
    {
        let s = &mut app.electronics.board.stitch;
        s.pattern = *p;
        s.pick = match *p {
            Pattern::LayerChange => Some(StitchPick::LayerChanges),
            // Needed stitching works where the planes need it: the whole board unless a pour is picked.
            Pattern::Needed => Some(StitchPick::Board),
            _ => None,
        };
        s.preview = None;
        s.running = None;
    }
    let pattern = app.electronics.board.stitch.pattern;
    let [diameter, drill] = via_size(app);
    // The pattern's own pitch, and what it comes from.
    let fenced: BTreeSet<String> = match &app.electronics.board.stitch.pick {
        Some(StitchPick::Nets(n)) => n.iter().cloned().collect(),
        Some(StitchPick::Tracks(ids)) => {
            let nets = stored_nets(app);
            let b = board(app);
            ids.iter()
                .filter_map(|id| b.tracks.iter().position(|t| t.id == *id))
                .filter_map(|i| nets.track_nets.get(i).copied().flatten())
                .filter_map(|n| nets.name(n).map(str::to_string))
                .collect()
        }
        _ => BTreeSet::new(),
    };
    let ecad = &app.session.doc.ecad;
    let own = match pattern {
        Pattern::Grid => Some((stitch::grid_pitch(ecad, diameter), "electronics.stitch_pitch_grid")),
        Pattern::Fence => Some((stitch::fence_pitch(ecad, &fenced, diameter), "electronics.stitch_pitch_fence")),
        Pattern::LayerChange | Pattern::Needed => None,
    };
    if let Some((d, key)) = own {
        let pitch = crate::electronics_editor::mm(d.pitch);
        let hint = match d.mhz {
            Some(f) => crate::i18n::format(key, &[("pitch", &pitch), ("mhz", &crate::electronics_editor::mm(f))]),
            None => crate::i18n::format("electronics.stitch_pitch_usual", &[("pitch", &pitch)]),
        };
        let old = app.electronics.board.stitch.pitch;
        if let Some(p) = crate::electronics_board_pour::optional_number(app, ui, "stitch:pitch", &tr("electronics.field_pitch"), old, &hint) {
            match p {
                Some(v) if !(v.is_finite() && v > diameter && v <= stitch::MAX_PITCH) => {
                    app.set_status(crate::i18n::format("electronics.stitch_bad_pitch", &[("via", &crate::electronics_editor::mm(diameter))]), true)
                }
                p => app.electronics.board.stitch.pitch = p,
            }
        }
        ui_kit::text(ui, &hint, ui_kit::Text::Note("electronics.stitch_pitch_note"));
    }
    // The via: the net class's (the rules' micro via between neighbouring layers) or one typed,
    // through or between two layers, and what covers its hole.
    if let Some([d, h]) = crate::electronics_editor::pair(app, ui, "stitch:via", crate::i18n::tr("electronics.field_via"), [diameter, drill]) {
        if check_via(d, h).is_ok() {
            app.electronics.board.stitch.via = Some([d, h]);
        } else {
            app.set_status(tr("electronics.board_bad_via"), true);
        }
    }
    if app.electronics.board.stitch.via.is_some()
        && crate::electronics_editor::button(ui, "stitch-via-class", &tr("electronics.board_via_net_class"), true)
    {
        app.electronics.board.stitch.via = None;
    }
    let span = app.electronics.board.stitch.span;
    app.electronics.board.stitch.span = route::span_rows(app, ui, "stitch", span);
    if app.electronics.board.stitch.via.is_none() && micro_span(app) {
        ui_kit::text(ui, crate::i18n::tr("electronics.stitch_micro_note"), ui_kit::Text::Note("electronics.stitch_micro_note"));
    }
    let now = app.electronics.board.stitch.protection;
    let shown = tr(&format!("electronics.via_protection_{}", now.name()));
    let mut protection = None;
    ui_kit::row(ui, crate::i18n::tr("electronics.field_via_protection"), |ui| {
        // Each entry is the handle `dialog:electronics_stitch_protection:<name>`.
        ui_kit::dropdown_menu(ui, "electronics_stitch_protection", &shown, |ui| {
            for p in VIA_PROTECTIONS {
                let r = ui_kit::list_picker(
                    ui,
                    &format!("stitch_protection_{}", p.name()),
                    &tr(&format!("electronics.via_protection_{}", p.name())),
                    now == p,
                );
                crate::scenario::publish_handle(&format!("dialog:electronics_stitch_protection:{}", p.name()), r.rect.center());
                if r.clicked() {
                    protection = Some(p);
                }
            }
        });
    });
    if let Some(p) = protection {
        app.electronics.board.stitch.protection = p;
    }
    // A module's datasheet often asks for no vias under it: a grid and a fence keep out of
    // courtyards unless allowed.
    if pattern != Pattern::LayerChange {
        let on = app.electronics.board.stitch.under_parts;
        if ui_kit::checkbox_row(ui, "stitch:under_parts", &tr("electronics.stitch_under_parts"), on) {
            app.electronics.board.stitch.under_parts = !on;
        }
    }
    // What to stitch.
    match pattern {
        Pattern::Grid => {
            ui_kit::text(ui, crate::i18n::tr("electronics.stitch_grid_help"), ui_kit::Text::Note("electronics.stitch_grid_help"));
            if crate::electronics_editor::button(ui, "stitch-board", &tr("electronics.stitch_whole_board"), true) {
                pick(app, StitchPick::Board);
            }
        }
        Pattern::Fence => {
            ui_kit::text(ui, crate::i18n::tr("electronics.stitch_fence_help"), ui_kit::Text::Note("electronics.stitch_fence_help"));
            if crate::electronics_editor::button(ui, "stitch-rf", &tr("electronics.stitch_rf_nets"), true) {
                let names = stitch::rf_nets(&app.session.doc.ecad);
                if names.is_empty() {
                    app.set_status(tr("electronics.stitch_no_rf"), true);
                } else {
                    pick(app, StitchPick::Nets(names));
                }
            }
        }
        Pattern::LayerChange => {
            ui_kit::text(ui, crate::i18n::tr("electronics.stitch_layer_change_help"), ui_kit::Text::Note("electronics.stitch_layer_change_help"));
            if crate::electronics_editor::button(ui, "stitch-changes", &tr("electronics.stitch_find_changes"), true) {
                pick(app, StitchPick::LayerChanges);
            }
        }
        Pattern::Needed => {
            ui_kit::text(ui, crate::i18n::tr("electronics.stitch_needed_help"), ui_kit::Text::Note("electronics.stitch_needed_help"));
            let hint = crate::i18n::format("electronics.stitch_budget_usual", &[("n", &stitch::DEFAULT_NEEDED_BUDGET.to_string())]);
            let old = app.electronics.board.stitch.budget.map(|b| b as f64);
            if let Some(b) =
                crate::electronics_board_pour::optional_number(app, ui, "stitch:budget", &tr("electronics.field_stitch_budget"), old, &hint)
            {
                match b {
                    Some(v) if !(v.is_finite() && v >= 1.0 && v <= stitch::MAX_NEEDED_BUDGET as f64 && v.fract() == 0.0) => app
                        .set_status(crate::i18n::format("electronics.stitch_bad_budget", &[("most", &stitch::MAX_NEEDED_BUDGET.to_string())]), true),
                    b => app.electronics.board.stitch.budget = b.map(|v| v as usize),
                }
            }
            if crate::electronics_editor::button(ui, "stitch-board", &tr("electronics.stitch_whole_board"), true) {
                pick(app, StitchPick::Board);
            }
        }
    }
    refresh(app);
    if let Some(r) = app.electronics.board.stitch.running.as_ref() {
        let seconds = r.started.elapsed().as_secs();
        let text = if seconds >= 1 {
            crate::i18n::format("electronics.stitch_working_for", &[("s", &seconds.to_string())])
        } else {
            crate::i18n::tr("electronics.stitch_working").to_string()
        };
        ui_kit::text(ui, &text, ui_kit::Text::Note("electronics.stitch_working"));
    }
    if app.electronics.board.stitch.preview.is_some() {
        let text = summary(app);
        let failed = app.electronics.board.stitch.preview.as_ref().is_some_and(|x| x.error.is_some());
        let key = if failed { "electronics.stitch_error" } else { "electronics.stitch_preview" };
        ui_kit::text(ui, &text, ui_kit::Text::Note(key));
        // Needed stitching: how weakly the planes are tied now, and after the vias shown.
        if let Some((before, after, reached)) = app.electronics.board.stitch.preview.as_ref().and_then(|x| x.ground) {
            let mv = |v: Option<f64>| v.map_or_else(|| "–".to_string(), crate::electronics_editor::mm);
            let key = if reached { "electronics.stitch_needed_reached" } else { "electronics.stitch_needed_short" };
            let line = crate::i18n::format(key, &[("before", &mv(before)), ("after", &mv(after))]);
            ui_kit::text(ui, &line, ui_kit::Text::Note("electronics.stitch_needed_tie"));
        }
        let (why, unfenced) = app.electronics.board.stitch.preview.as_ref().map(|x| (x.why.clone(), x.unfenced)).unwrap_or_default();
        if unfenced > 0 {
            let line = crate::i18n::format("electronics.stitch_unfenced", &[("n", &unfenced.to_string())]);
            ui_kit::text(ui, &line, ui_kit::Text::Note("electronics.stitch_unfenced"));
        }
        // Place before the reasons, so it stays in sight however long they run.
        let n = app.electronics.board.stitch.preview.as_ref().map_or(0, |x| x.spots.len());
        let label = crate::i18n::format("electronics.stitch_place", &[("n", &n.to_string())]);
        if crate::electronics_editor::button(ui, "stitch-place", &label, n > 0 && !working(app)) {
            place(app);
        }
        // The engine's reasons, as it gives them.
        for line in &why {
            ui_kit::text(ui, line, ui_kit::Text::Note("electronics.stitch_why"));
        }
    }
    let (n, older, whole) = app.electronics.board.stitch.removal.as_ref().map_or((0, 0, true), |r| (r.ids.len(), r.older.len(), r.whole));
    let kind = tr(match pattern {
        Pattern::Grid => "electronics.stitch_kind_grid",
        Pattern::Fence => "electronics.stitch_kind_fence",
        Pattern::LayerChange => "electronics.stitch_kind_layer_change",
        Pattern::Needed => "electronics.stitch_kind_needed",
    });
    ui_kit::text(ui, crate::i18n::tr("electronics.stitch_parts_help"), ui_kit::Text::Note("electronics.stitch_parts_help"));
    let label = match &app.electronics.board.stitch.pick {
        _ if whole => crate::i18n::format("electronics.stitch_remove_all", &[("n", &n.to_string()), ("net", current.as_deref().unwrap_or(""))]),
        Some(StitchPick::Parts(_)) => crate::i18n::format("electronics.stitch_remove_under", &[("n", &n.to_string()), ("parts", &parts_name(app))]),
        Some(StitchPick::Board) => crate::i18n::format("electronics.stitch_remove_pattern", &[("n", &n.to_string()), ("pattern", &kind)]),
        _ => crate::i18n::format("electronics.stitch_remove_here", &[("n", &n.to_string()), ("pattern", &kind)]),
    };
    if crate::electronics_editor::button(ui, "stitch-remove", &label, n > 0) {
        remove(app, false);
    }
    if older > 0 {
        let line = crate::i18n::format("electronics.stitch_older", &[("n", &older.to_string())]);
        ui_kit::text(ui, &line, ui_kit::Text::Note("electronics.stitch_older"));
        let label = crate::i18n::format("electronics.stitch_remove_older", &[("n", &older.to_string())]);
        if crate::electronics_editor::button(ui, "stitch-remove-older", &label, true) {
            remove(app, true);
        }
    }
    "electronics.stitch_help"
}
