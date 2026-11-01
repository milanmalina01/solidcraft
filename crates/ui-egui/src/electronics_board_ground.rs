//! The board view's Ground connectivity layer (see `electronics_board_layers`): how weakly each
//! spot of the ground net's copper on the active layer is tied to its main ground, as
//! `ecad.board.ground.inspect` measures it (`solidcraft_doc::ecad::ground`), drawn as a heat map
//! from green (tied, 0 mV) through amber (the target, 0.5 mV) to red (twice it, or isolated).
//! Hidden until the Layers tab shows it; then solved beside the window whenever the design
//! changes (the last map shows until the new one is back), never on the window thread.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Color32, Rect};
use solidcraft_engine::doc::ecad::ground::{self, DEFAULT_TARGET_MV, GroundMap, Model};

use crate::SolveApp;
use crate::electronics_board::active_layer;
use crate::electronics_board_layers::Layer;
use crate::electronics_workspace::PageMap;
use crate::theme::Tokens;

/// The heat map's most cells each way.
const MAP_CELLS: usize = 96;
/// How opaque the heat map is over the copper.
const ALPHA: f32 = 0.55;

type Answer = Result<GroundMap, String>;

/// The layer's last map and the solve being worked out, each for a document revision.
#[derive(Debug, Default)]
pub struct GroundView {
    map: Option<((u64, u64), Arc<GroundMap>)>,
    running: Option<((u64, u64), Receiver<Answer>)>,
    /// Why there is no map (no ground net, no plane to measure from).
    pub error: Option<String>,
}

fn shown(app: &SolveApp) -> bool {
    !app.electronics.board.hidden.contains(&Layer::Ground)
}

/// Run the solve beside the window; the browser (and a desktop that cannot start a thread) runs
/// it at once.
fn spawn(job: impl FnOnce() -> Answer + Send + 'static) -> Receiver<Answer> {
    let (tx, rx) = std::sync::mpsc::channel();
    let slot = Arc::new(std::sync::Mutex::new(Some(job)));
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (theirs, sender) = (slot.clone(), tx.clone());
        let started = std::thread::Builder::new().name("ground-map".into()).spawn(move || {
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

/// Take a finished solve, and start one for the design as it is when the layer shows and the map
/// is older (looked at each frame the board view draws).
pub(crate) fn refresh(app: &mut SolveApp, ctx: &egui::Context) {
    if !shown(app) {
        let g = &mut app.electronics.board.ground;
        g.running = None;
        return;
    }
    let key = (app.session.uid, app.session.revision);
    let back = match app.electronics.board.ground.running.as_ref().map(|(_, rx)| rx.try_recv()) {
        Some(Ok(a)) => Some(Some(a)),
        Some(Err(TryRecvError::Disconnected)) => Some(None),
        Some(Err(TryRecvError::Empty)) | None => None,
    };
    if let Some(back) = back {
        let g = &mut app.electronics.board.ground;
        let k = g.running.take().map(|(k, _)| k).unwrap_or(key);
        match back {
            Some(Ok(m)) => {
                g.map = Some((k, Arc::new(m)));
                g.error = None;
            }
            Some(Err(e)) => {
                g.map = None;
                g.error = Some(e);
            }
            None => {}
        }
    }
    let g = &app.electronics.board.ground;
    if g.map.as_ref().is_some_and(|(k, _)| *k == key) || g.running.as_ref().is_some_and(|(k, _)| *k == key) {
        if g.running.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        return;
    }
    if g.error.is_some() && g.running.is_none() && g.map.is_none() && app.electronics.board.ground_tried == Some(key) {
        return;
    }
    app.electronics.board.ground_tried = Some(key);
    let ecad = app.session.doc.ecad.clone();
    let rx = spawn(move || {
        let nets = ecad.board_nets();
        Model::build(&ecad, &nets, &ground::Ask::default()).map(|mut m| m.solve())
    });
    app.electronics.board.ground.running = Some((key, rx));
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
}

/// The heat colour of `mv` millivolts (infinity: isolated), from the theme's green, amber and red.
fn heat(t: &Tokens, mv: f64) -> Color32 {
    let mix = |a: Color32, b: Color32, s: f32| a.lerp_to_gamma(b, s.clamp(0.0, 1.0));
    let x = if mv.is_finite() { (mv / DEFAULT_TARGET_MV) as f32 } else { 2.0 };
    let c = if x <= 1.0 { mix(t.agent_ok, t.warning, x) } else { mix(t.warning, t.error, x - 1.0) };
    c.gamma_multiply(ALPHA)
}

/// The active copper layer's heat map, under the tracks' outlines it is drawn over.
pub(crate) fn paint(app: &SolveApp, painter: &egui::Painter, map: PageMap, t: &Tokens) {
    if !shown(app) {
        return;
    }
    let Some((_, m)) = app.electronics.board.ground.map.as_ref() else { return };
    let layer = active_layer(app);
    let Some(l) = m.layers.iter().position(|x| *x == layer) else { return };
    let (side, rows) = m.coarse_map(l, MAP_CELLS);
    for (iy, row) in rows.iter().enumerate() {
        for (ix, v) in row.iter().enumerate() {
            let Some(v) = v else { continue };
            let (x0, y0) = (m.origin[0] + ix as f64 * side, m.origin[1] + iy as f64 * side);
            let (a, b) = (map.to_screen(x0, y0 + side), map.to_screen(x0 + side, y0));
            let r = Rect::from_two_pos(a, b);
            if r.is_finite() {
                painter.rect_filled(r, 0.0, heat(t, *v));
            }
        }
    }
}

/// What the Layers tab says under the layer's row while it shows: the worst spot of the planes,
/// that a solve is under way, or why there is no map.
pub(crate) fn note(app: &SolveApp) -> Option<String> {
    if !shown(app) {
        return None;
    }
    let g = &app.electronics.board.ground;
    if let Some(e) = &g.error {
        return Some(crate::i18n::format("electronics.ground_map_none", &[("why", e)]));
    }
    if let Some((_, m)) = &g.map {
        let worst = m.plane_worst(&[]).map_or_else(|| "–".to_string(), |s| crate::electronics_editor::mm((s.mv * 100.0).round() / 100.0));
        let key = if g.running.is_some() { "electronics.ground_map_updating" } else { "electronics.ground_map_worst" };
        return Some(crate::i18n::format(key, &[("mv", &worst), ("net", &m.net_name)]));
    }
    Some(crate::i18n::tr("electronics.ground_map_working").to_string())
}
