//! Copper pours (made, changed, and their fills read back), keepouts, and via edits (span, size,
//! a net of their own). A pour's fill is worked out from the board on every read and never saved
//! (see `solidcraft_doc::ecad::fill`). Each change is one undo step, all or none, as in
//! `ecad_board.rs`; pours and keepouts are deleted, moved and flipped like any board object.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use solidcraft_doc::ecad::airwires::footprint_pads;
use solidcraft_doc::ecad::board::{MAX_BOARD, MAX_OUTLINE_POINTS, MAX_VIAS, check_via};
use solidcraft_doc::ecad::footprint::MAX_PADS;
use solidcraft_doc::ecad::path::{check_curves, tidy};
use solidcraft_doc::ecad::place::snap_point;
use solidcraft_doc::ecad::pour::DEFAULT_MIN_WIDTH;
use solidcraft_doc::ecad::{Board, Connect, CopperLayer, Forbid, Hatch, Keepout, Pour, PourFill, Thermal};

use super::CommandSpec;
use super::ecad::{closest, parse, reply};
use super::ecad_board::{Index, board_point, board_result, board_result_nets, no_layer, store_board};
use crate::params::bad;
use crate::schema::ecad_board::{BoardResult, CircleSpec, CopperLayerParam};
use crate::schema::ecad_board_pour::{
    ConnectParam, FillOut, FillSummary, ForbidOut, ForbidSpec, HatchSpec, KeepoutEditParams, KeepoutOut, KeepoutParams, KeepoutResult, PolygonOut,
    PourConnectParams, PourEditParams, PourInspectParams, PourInspectResult, PourOut, PourParams, PourResult, ThermalOut, ThermalSpec, ViaEditParams,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.board.pour", "Polygon", crate::checked!("ecad.board.pour", pour))
        .icon("boundary_fill")
        .doc_only()
        .contract(&POUR_CONTRACT)
        .output(output_schema_for::<PourResult>)
        .params(
            "layer; net? (a schematic net's name; left out, a pour on no net); points? [[x, y]…] with curves? [sweep…] | rect? [w, h] with center? | circle? {center, radius} (the outline); clearance? (mm; left out, the net class's); min_width? (mm, 0.2: thinner copper goes); connect? thermal|solid|none; thermal? {spoke_width? (0.3), gap? (0.3), spokes? 2|4}; priority? (0 to 1000; higher fills first where pours overlap); remove_islands? (true); hatch? {width, pitch} → {pour: {id, …, fill: {polygons, area, islands_removed, truncated?}}, created, unrouted}: a copper pour filled round other nets' copper (each grown by the larger clearance of the two nets), the board's edge (copper_to_edge), holes and keepouts; it joins the copper of its net it touches, so their airwires go. The fill is never saved: ecad.board.pour.inspect reads it",
        ),
    CommandSpec::new("ecad.board.pour.edit", "Edit Polygon", crate::checked!("ecad.board.pour.edit", pour_edit))
        .icon("edit")
        .doc_only()
        .contract(&POUR_EDIT_CONTRACT)
        .output(output_schema_for::<PourResult>)
        .params(
            "pour: id; then any of ecad.board.pour's fields (net: null takes it off its net, clearance: null back to the class's, hatch: null solid; points/rect/circle replace the outline; thermal's fields left out stay) → {pour, changed, unrouted}",
        ),
    CommandSpec::new("ecad.board.pour.inspect", "Inspect Polygons", crate::checked!("ecad.board.pour.inspect", pour_inspect))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&POUR_INSPECT_CONTRACT)
        .output(output_schema_for::<PourInspectResult>)
        .params(
            "pour? (id; left out, every pour); polygons? (true: list them) → {fills: [{pour, layer, net, polygons (count), area, islands_removed, truncated?, shapes: [{outline, holes?}]}], truncated?}: each pour's fill as the board has it now (worked out, not saved): outlines counter-clockwise, holes clockwise. Read-only",
        ),
    CommandSpec::new("ecad.board.keepout", "Keepout", crate::checked!("ecad.board.keepout", keepout))
        .icon("shield")
        .doc_only()
        .contract(&KEEPOUT_CONTRACT)
        .output(output_schema_for::<KeepoutResult>)
        .params(
            "layers? [layer] (left out or [], every copper layer); points? with curves? | rect? [w, h] with center? | circle? {center, radius}; forbid? {tracks?, vias?, pours?, components?} (each true when left out) → {keepout, created}: pours keep out of it; the design rule check reports tracks, vias and footprints (by courtyard) in it that it forbids",
        ),
    CommandSpec::new("ecad.board.keepout.edit", "Edit Keepout", crate::checked!("ecad.board.keepout.edit", keepout_edit))
        .icon("edit")
        .doc_only()
        .contract(&KEEPOUT_EDIT_CONTRACT)
        .output(output_schema_for::<KeepoutResult>)
        .params("keepout: id; then any of ecad.board.keepout's fields (forbid's fields left out stay) → {keepout, changed}"),
    CommandSpec::new("ecad.board.pour.connect", "Pour Connection", crate::checked!("ecad.board.pour.connect", pour_connect))
        .icon("boundary_fill")
        .doc_only()
        .contract(&POUR_CONNECT_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "footprint: designator|id; pads? [number] (left out, the footprint as a whole); connect: thermal|solid|none|null (null: back to the pour's own; for pads, the footprint's when it has one; null also clears a pad number the footprint no longer has, which ecad.board.inspect lists as stale_connect) → {changed, footprints, unrouted}: how every pour of a pad's net joins it, over the pour's own connect (a pad's setting over its footprint's, saved with the board); the fill and the design rule check's relief findings follow it",
        ),
    CommandSpec::new("ecad.board.via.edit", "Edit Via", crate::checked!("ecad.board.via.edit", via_edit))
        .icon("hole")
        .doc_only()
        .contract(&VIA_EDIT_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "vias: [id] (all or none); from? to? (the span: top to bottom is a through via, an outer layer to an inner one blind, inner to inner buried, neighbouring layers no wider than the rules' micro via a micro via); diameter? drill? (mm); net? (a net of its own, for a stitching via; null takes it away); protection? open|tented|plugged|filled|filled_capped → {changed, unrouted}",
        ),
];

const POUR_CONTRACT: TypedContract<PourParams> = TypedContract::new();
const POUR_EDIT_CONTRACT: TypedContract<PourEditParams> = TypedContract::new();
const POUR_INSPECT_CONTRACT: TypedContract<PourInspectParams> = TypedContract::new();
const KEEPOUT_CONTRACT: TypedContract<KeepoutParams> = TypedContract::new();
const KEEPOUT_EDIT_CONTRACT: TypedContract<KeepoutEditParams> = TypedContract::new();
const VIA_EDIT_CONTRACT: TypedContract<ViaEditParams> = TypedContract::new();
const POUR_CONNECT_CONTRACT: TypedContract<PourConnectParams> = TypedContract::new();

/// The most corners of fill polygons one reply lists.
const MAX_REPLY_CORNERS: usize = 200_000;

/// The schematic's net names.
fn net_names(s: &Session) -> Vec<String> {
    s.doc.ecad.connectivity().nets.into_iter().map(|n| n.name).collect()
}

/// A net name the schematic has, or why not (with the nearest name).
pub(super) fn known_net(s: &Session, name: &str, cmd: &str) -> Result<String> {
    known_net_in(&net_names(s), name, cmd)
}

/// A net name among the schematic's `names`, or why not (with the nearest name): for a call
/// that checks many.
pub(super) fn known_net_in(names: &[String], name: &str, cmd: &str) -> Result<String> {
    if names.iter().any(|n| n == name) {
        return Ok(name.to_string());
    }
    let mut msg = format!("`net`: the schematic has no net `{name}`");
    if let Some(near) = closest(name, names.iter().map(String::as_str)) {
        msg.push_str(&format!(" (did you mean `{near}`?)"));
    } else if names.is_empty() {
        msg.push_str(" (it has no nets yet)");
    }
    Err(bad(cmd, msg))
}

/// An area's outline as given: `points` (with `curves`), a `rect` round `center`, or a `circle`;
/// `None` when none is given.
fn area(
    points: Option<Vec<[f64; 2]>>,
    curves: Option<Vec<f64>>,
    rect: Option<[f64; 2]>,
    center: Option<[f64; 2]>,
    circle: Option<CircleSpec>,
    cmd: &str,
) -> Result<Option<(Vec<[f64; 2]>, Vec<f64>)>> {
    if usize::from(points.is_some()) + usize::from(rect.is_some()) + usize::from(circle.is_some()) > 1 {
        return Err(bad(cmd, "give one of `points`, `rect` or `circle`"));
    }
    if rect.is_none() && center.is_some() {
        return Err(bad(cmd, "`center` goes with `rect`"));
    }
    if points.is_none() && curves.is_some() {
        return Err(bad(cmd, "`curves` goes with `points`"));
    }
    if let Some([w, h]) = rect {
        if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 && w <= 2.0 * MAX_BOARD && h <= 2.0 * MAX_BOARD) {
            return Err(bad(cmd, format!("`rect` must be a width and a height, each more than 0 and at most {} mm", 2.0 * MAX_BOARD)));
        }
        let [cx, cy] = board_point(center.unwrap_or([0.0, 0.0]), cmd, "`center`")?;
        let (x, y) = (w / 2.0, h / 2.0);
        let corners = [[cx - x, cy - y], [cx + x, cy - y], [cx + x, cy + y], [cx - x, cy + y]].map(snap_point).to_vec();
        return Ok(Some((corners, Vec::new())));
    }
    if let Some(c) = circle {
        let center = board_point(c.center, cmd, "`circle.center`")?;
        if !(c.radius.is_finite() && c.radius > 0.0 && c.radius <= MAX_BOARD) {
            return Err(bad(cmd, format!("`circle.radius` must be more than 0 and at most {MAX_BOARD} mm")));
        }
        let r = c.radius;
        return Ok(Some(([[center[0] + r, center[1]], [center[0] - r, center[1]]].map(snap_point).to_vec(), vec![180.0, 180.0])));
    }
    let Some(points) = points else { return Ok(None) };
    if points.len() > MAX_OUTLINE_POINTS {
        return Err(bad(cmd, format!("an outline has at most {MAX_OUTLINE_POINTS} corners")));
    }
    let mut corners = points.iter().enumerate().map(|(i, q)| board_point(*q, cmd, &format!("points[{i}]"))).collect::<Result<Vec<_>>>()?;
    let mut curves = curves.unwrap_or_default();
    check_curves(&curves, corners.len()).map_err(|e| bad(cmd, e))?;
    tidy(&mut corners, &mut curves, true);
    Ok(Some((corners, curves)))
}

pub(super) fn connect(c: ConnectParam) -> Connect {
    match c {
        ConnectParam::Thermal => Connect::Thermal,
        ConnectParam::Solid => Connect::Solid,
        ConnectParam::None => Connect::None,
    }
}

pub(super) fn connect_out(c: Connect) -> ConnectParam {
    match c {
        Connect::Thermal => ConnectParam::Thermal,
        Connect::Solid => ConnectParam::Solid,
        Connect::None => ConnectParam::None,
    }
}

fn thermal(mut t: Thermal, spec: Option<ThermalSpec>) -> Thermal {
    if let Some(spec) = spec {
        t.spoke_width = spec.spoke_width.unwrap_or(t.spoke_width);
        t.gap = spec.gap.unwrap_or(t.gap);
        t.spokes = spec.spokes.unwrap_or(t.spokes);
    }
    t
}

fn hatch(h: HatchSpec) -> Hatch {
    Hatch { width: h.width, pitch: h.pitch }
}

fn round(v: f64) -> f64 {
    (v * 1e4).round() / 1e4 + 0.0
}

/// What a fill comes to.
fn summary(fill: Option<&PourFill>) -> FillSummary {
    match fill {
        Some(f) => FillSummary { polygons: f.region.len(), area: round(f.area()), islands_removed: f.islands_removed, truncated: f.truncated },
        None => FillSummary { polygons: 0, area: 0.0, islands_removed: 0, truncated: false },
    }
}

/// A pour as replies give it, with its fill from `fills`.
pub(super) fn pour_out(board: &Board, p: &Pour, fills: &[PourFill], names: &[String]) -> PourOut {
    let fill = fills.iter().find(|f| f.pour == p.id);
    let class = board.rules.sizes(p.net.as_deref()).clearance;
    PourOut {
        id: p.id,
        layer: p.layer.name(),
        net: p.net.clone(),
        net_found: p.net.as_ref().is_some_and(|n| names.contains(n)),
        outline: p.outline.clone(),
        curves: p.curves.clone(),
        clearance: p.clearance,
        effective_clearance: fill.map_or(p.clearance.unwrap_or(class), |f| f.clearance),
        min_width: p.min_width,
        connect: connect_out(p.connect),
        thermal: ThermalOut { spoke_width: p.thermal.spoke_width, gap: p.thermal.gap, spokes: p.thermal.spokes },
        priority: p.priority,
        remove_islands: p.remove_islands,
        hatch: p.hatch.map(|h| HatchSpec { width: h.width, pitch: h.pitch }),
        fill: summary(fill),
    }
}

/// A keepout as replies give it.
pub(super) fn keepout_out(k: &Keepout) -> KeepoutOut {
    KeepoutOut {
        id: k.id,
        layers: k.layers.iter().map(|l| l.name()).collect(),
        all_layers: k.layers.is_empty(),
        outline: k.outline.clone(),
        curves: k.curves.clone(),
        forbid: ForbidOut { tracks: k.forbid.tracks, vias: k.forbid.vias, pours: k.forbid.pours, components: k.forbid.components },
    }
}

/// Store `board` and reply with the pour `id` and what changed from `old`.
fn pour_reply(s: &mut Session, board: Board, old: &Board, id: u64, cmd: &str) -> Result<Value> {
    store_board(s, board, cmd)?;
    let (result, nets) = board_result_nets(s, old);
    let board = &s.doc.ecad.board;
    let Some(p) = board.pour(id) else { return Err(bad(cmd, format!("pour {id} is gone"))) };
    reply(PourResult { pour: pour_out(board, p, &nets.fills, &nets.names), board: result })
}

fn layer_of(board: &Board, l: CopperLayerParam, cmd: &str) -> Result<CopperLayer> {
    if !board.has_layer(l.0) {
        return Err(bad(cmd, no_layer(board, l.0.name())));
    }
    Ok(l.0)
}

fn pour(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.pour";
    let p: PourParams = parse(CMD, p)?;
    let old = s.doc.ecad.board.clone();
    let layer = layer_of(&old, p.layer, CMD)?;
    let Some((outline, curves)) = area(p.points, p.curves, p.rect, p.center, p.circle, CMD)? else {
        return Err(bad(CMD, "give the pour's outline: `points`, a `rect` or a `circle`"));
    };
    let net = p.net.map(|n| known_net(s, &n, CMD)).transpose()?;
    let mut board = old.clone();
    let id = board.take_id().map_err(|e| bad(CMD, e))?;
    board.pours.push(Pour {
        id,
        layer,
        net,
        outline,
        curves,
        clearance: p.clearance,
        min_width: p.min_width.unwrap_or(DEFAULT_MIN_WIDTH),
        connect: p.connect.map_or(Connect::Thermal, connect),
        thermal: thermal(Thermal::default(), p.thermal),
        priority: p.priority.unwrap_or(0),
        remove_islands: p.remove_islands.unwrap_or(true),
        hatch: p.hatch.map(hatch),
    });
    pour_reply(s, board, &old, id, CMD)
}

fn pour_edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.pour.edit";
    let p: PourEditParams = parse(CMD, p)?;
    let old = s.doc.ecad.board.clone();
    let Some(k) = old.pours.iter().position(|x| x.id == p.pour) else {
        return Err(bad(CMD, format!("the board has no pour with the id {}", p.pour)));
    };
    let layer = p.layer.map(|l| layer_of(&old, l, CMD)).transpose()?;
    let outline = area(p.points, p.curves, p.rect, p.center, p.circle, CMD)?;
    let net = match p.net {
        Some(Some(n)) => Some(Some(known_net(s, &n, CMD)?)),
        other => other,
    };
    let mut board = old.clone();
    let Some(x) = board.pours.get_mut(k) else { return Err(bad(CMD, format!("the board has no pour with the id {}", p.pour))) };
    if let Some(l) = layer {
        x.layer = l;
    }
    if let Some((points, curves)) = outline {
        x.outline = points;
        x.curves = curves;
    }
    if let Some(n) = net {
        x.net = n;
    }
    if let Some(c) = p.clearance {
        x.clearance = c;
    }
    if let Some(w) = p.min_width {
        x.min_width = w;
    }
    if let Some(c) = p.connect {
        x.connect = connect(c);
    }
    x.thermal = thermal(x.thermal, p.thermal);
    if let Some(v) = p.priority {
        x.priority = v;
    }
    if let Some(v) = p.remove_islands {
        x.remove_islands = v;
    }
    if let Some(h) = p.hatch {
        x.hatch = h.map(hatch);
    }
    let id = x.id;
    pour_reply(s, board, &old, id, CMD)
}

fn pour_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.pour.inspect";
    let p: PourInspectParams = parse(CMD, p)?;
    let board = &s.doc.ecad.board;
    if let Some(id) = p.pour
        && board.pour(id).is_none()
    {
        return Err(bad(CMD, format!("the board has no pour with the id {id}")));
    }
    let nets = s.board_nets();
    let with_polygons = p.polygons.unwrap_or(true);
    let mut corners = 0usize;
    let mut truncated = false;
    let mut fills = Vec::new();
    for f in nets.fills.iter().filter(|f| p.pour.is_none_or(|id| f.pour == id)) {
        truncated |= f.truncated;
        let mut shapes = Vec::new();
        if with_polygons {
            for shape in f.shapes() {
                corners = corners.saturating_add(shape.corners());
                if corners > MAX_REPLY_CORNERS {
                    truncated = true;
                    break;
                }
                let pts = |c: &[[f64; 2]]| c.iter().map(|q| [round(q[0]), round(q[1])]).collect::<Vec<_>>();
                shapes.push(PolygonOut { outline: pts(&shape.outer), holes: shape.holes.iter().map(|h| pts(h)).collect() });
            }
        }
        fills.push(FillOut {
            pour: f.pour,
            layer: f.layer.name(),
            net: f.net.and_then(|n| nets.name(n)).map(str::to_string),
            summary: summary(Some(f)),
            shapes,
        });
    }
    reply(PourInspectResult { fills, truncated, param_warnings: Vec::new() })
}

/// The copper layers a keepout names, each once and on the stack; `[]` is every layer.
fn keepout_layers(board: &Board, layers: &[CopperLayerParam], cmd: &str) -> Result<Vec<CopperLayer>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for l in layers {
        let l = layer_of(board, *l, cmd)?;
        if seen.insert(l) {
            out.push(l);
        }
    }
    out.sort_by_key(|l| l.index(board.copper_layers));
    Ok(out)
}

fn forbid(mut f: Forbid, spec: Option<ForbidSpec>) -> Forbid {
    if let Some(s) = spec {
        f.tracks = s.tracks.unwrap_or(f.tracks);
        f.vias = s.vias.unwrap_or(f.vias);
        f.pours = s.pours.unwrap_or(f.pours);
        f.components = s.components.unwrap_or(f.components);
    }
    f
}

fn keepout_reply(s: &mut Session, board: Board, old: &Board, id: u64, cmd: &str) -> Result<Value> {
    store_board(s, board, cmd)?;
    let (result, _) = board_result_nets(s, old);
    let Some(k) = s.doc.ecad.board.keepout(id) else { return Err(bad(cmd, format!("keepout {id} is gone"))) };
    reply(KeepoutResult { keepout: keepout_out(k), board: result })
}

fn keepout(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.keepout";
    let p: KeepoutParams = parse(CMD, p)?;
    let old = s.doc.ecad.board.clone();
    let layers = keepout_layers(&old, p.layers.as_deref().unwrap_or_default(), CMD)?;
    let Some((outline, curves)) = area(p.points, p.curves, p.rect, p.center, p.circle, CMD)? else {
        return Err(bad(CMD, "give the keepout's outline: `points`, a `rect` or a `circle`"));
    };
    let mut board = old.clone();
    let id = board.take_id().map_err(|e| bad(CMD, e))?;
    board.keepouts.push(Keepout { id, layers, outline, curves, forbid: forbid(Forbid::default(), p.forbid) });
    keepout_reply(s, board, &old, id, CMD)
}

fn keepout_edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.keepout.edit";
    let p: KeepoutEditParams = parse(CMD, p)?;
    let old = s.doc.ecad.board.clone();
    let Some(k) = old.keepouts.iter().position(|x| x.id == p.keepout) else {
        return Err(bad(CMD, format!("the board has no keepout with the id {}", p.keepout)));
    };
    let layers = p.layers.as_deref().map(|l| keepout_layers(&old, l, CMD)).transpose()?;
    let outline = area(p.points, p.curves, p.rect, p.center, p.circle, CMD)?;
    let mut board = old.clone();
    let Some(x) = board.keepouts.get_mut(k) else { return Err(bad(CMD, format!("the board has no keepout with the id {}", p.keepout))) };
    if let Some(l) = layers {
        x.layers = l;
    }
    if let Some((points, curves)) = outline {
        x.outline = points;
        x.curves = curves;
    }
    x.forbid = forbid(x.forbid, p.forbid);
    let id = x.id;
    keepout_reply(s, board, &old, id, CMD)
}

fn via_edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.via.edit";
    let p: ViaEditParams = parse(CMD, p)?;
    if p.vias.is_empty() || p.vias.len() > MAX_VIAS {
        return Err(bad(CMD, format!("name 1 to {MAX_VIAS} vias in `vias`, by id")));
    }
    if p.from.is_none() && p.to.is_none() && p.diameter.is_none() && p.drill.is_none() && p.net.is_none() && p.protection.is_none() {
        return Err(bad(CMD, "give `from`, `to`, `diameter`, `drill`, `net` or `protection` to change"));
    }
    let old = s.doc.ecad.board.clone();
    let from = p.from.map(|l| layer_of(&old, l, CMD)).transpose()?;
    let to = p.to.map(|l| layer_of(&old, l, CMD)).transpose()?;
    let net = match p.net {
        Some(Some(n)) => Some(Some(known_net(s, &n, CMD)?)),
        other => other,
    };
    let mut board = old.clone();
    let at: BTreeMap<u64, usize> = old.vias.iter().enumerate().map(|(i, v)| (v.id, i)).collect();
    for (i, id) in p.vias.iter().enumerate() {
        let Some(v) = at.get(id).and_then(|k| board.vias.get_mut(*k)) else {
            return Err(bad(CMD, format!("vias[{i}]: the board has no via with the id {id}")));
        };
        if let Some(l) = from {
            v.from = l;
        }
        if let Some(l) = to {
            v.to = l;
        }
        v.diameter = p.diameter.unwrap_or(v.diameter);
        v.drill = p.drill.unwrap_or(v.drill);
        if let Some(n) = &net {
            v.net.clone_from(n);
        }
        if let Some(x) = p.protection {
            v.protection = x.doc();
        }
        check_via(v.diameter, v.drill).map_err(|e| bad(CMD, format!("vias[{i}]: a via's {e}")))?;
        if v.from.index(old.copper_layers) >= v.to.index(old.copper_layers) {
            return Err(bad(CMD, format!("vias[{i}]: `from` ({}) must be a layer above `to` ({})", v.from.name(), v.to.name())));
        }
    }
    store_board(s, board, CMD)?;
    reply(board_result_nets(s, &old).0)
}

fn pour_connect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.pour.connect";
    let p: PourConnectParams = parse(CMD, p)?;
    let Some(how) = p.connect else { return Err(bad(CMD, "give `connect`: thermal, solid or none, or null for the pour's own")) };
    let how = how.map(connect);
    let old = s.doc.ecad.board.clone();
    let id = Index::new(s).find(s, &p.footprint, CMD, "`footprint`")?;
    let Some(k) = old.footprints.iter().position(|f| f.id == id) else { return Err(bad(CMD, format!("board object {id} is not a footprint"))) };
    let mut board = old.clone();
    match p.pads {
        None => {
            if let Some(f) = board.footprints.get_mut(k) {
                f.pour_connect = how;
            }
        }
        Some(pads) => {
            if pads.is_empty() || pads.len() > MAX_PADS {
                return Err(bad(CMD, format!("name 1 to {MAX_PADS} pads in `pads`, or leave it out for the whole footprint")));
            }
            let numbers: BTreeSet<String> =
                old.footprints.get(k).map(|f| footprint_pads(&s.doc.ecad, f, None).into_iter().map(|p| p.pad.number).collect()).unwrap_or_default();
            // An override saved for a pad the footprint no longer has (its part's footprint
            // changed) can still be cleared.
            let saved = |n: &String| how.is_none() && old.footprints.get(k).is_some_and(|f| f.pad_connect.contains_key(n));
            for n in &pads {
                if !numbers.contains(n) && !saved(n) {
                    let mut msg = format!("`pads`: the footprint has no pad `{n}`");
                    if let Some(near) = closest(n, numbers.iter().map(String::as_str)) {
                        msg.push_str(&format!(" (did you mean `{near}`?)"));
                    }
                    return Err(bad(CMD, msg));
                }
            }
            if let Some(f) = board.footprints.get_mut(k) {
                for n in pads {
                    match how {
                        Some(c) => f.pad_connect.insert(n, c),
                        None => f.pad_connect.remove(&n),
                    };
                }
            }
        }
    }
    store_board(s, board, CMD)?;
    reply(board_result(s, &old))
}
