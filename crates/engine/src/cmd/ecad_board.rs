//! The circuit board: its outline, the footprints brought over from the schematic (forward
//! annotation), moved, turned and flipped to the other side with the ends of their tracks, copper
//! tracks and vias, and the board read back as data with each pad's net and the airwires still to
//! route. The layer stack, holes, drawings and track rework are in `ecad_board_edit.rs` and
//! `ecad_board_draw.rs`, pours, keepouts and via edits in `ecad_board_pour.rs`, the rules and net
//! classes in `ecad_board_rules.rs`. Each call changes a copy
//! of the board and stores it whole once it passes the design file's checks, so a refused call
//! leaves the design as it was, a call is one undo step, and a call that changes nothing makes none.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::Value;
use solidcraft_doc::ecad::airwires::footprint_pads;
use solidcraft_doc::ecad::board::{
    BOARD_GAP, BOARD_GRID, MAX_BOARD, MAX_BOARD_FOOTPRINTS, MAX_BOARD_GRAPHICS, MAX_HOLES, MAX_LEGS, MAX_OUTLINE_POINTS, MAX_TRACK_POINTS,
    MAX_TRACKS, MAX_VIAS, THICKNESS, check_board_point, check_track_width, check_via, footprint_box,
};
use solidcraft_doc::ecad::path::{self, check_curves, tidy};
use solidcraft_doc::ecad::place::{Flow, flow_with, snap, snap_point};
use solidcraft_doc::ecad::pour::{MAX_KEEPOUTS, MAX_POURS};
use solidcraft_doc::ecad::{
    Board, BoardFootprint, BoardGraphic, BoardKind, BoardNets, CopperLayer, Hole, Keepout, Motion, Pour, StackKind, StackLayer, Track, Via, ViaKind,
    ViaRole, bbox,
};

use super::CommandSpec;
use super::ecad::{closest, parse, reply, side_out};
use super::ecad_board_pour::{keepout_out, known_net, pour_out};
use super::ecad_board_rules::{classes_out, rules_out};
use super::ecad_graphics::graphic_out;
use super::ecad_schematic::quarter;
use crate::params::bad;
use crate::schema::ecad_board::{
    AirwireOut, BoardGraphicOut, BoardItemOut, BoardKind as KindOut, BoardResult, Copper, DeleteParams, FootprintOut, HoleOut, InspectParams,
    InspectResult, ItemsParams, LayerKind, MoveParams, NetOut, OutlineParams, OutlineResult, PadOut, RotateParams, StackLayerOut, SyncParams,
    SyncResult, TrackEditParams, TrackLeg, TrackOut, TrackParams, ViaKindOut, ViaOut, ViaParams,
};
use crate::schema::ecad_board_edit::{StackKindParam, StackLayerSpec};
use crate::schema::ecad_schematic::ItemRef;
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.board.outline", "Outline", crate::checked!("ecad.board.outline", outline))
        .icon("rect_rounded")
        .doc_only()
        .contract(&OUTLINE_CONTRACT)
        .output(output_schema_for::<OutlineResult>)
        .params(
            "rect? [width, height] (mm) with center? [x, y] (the origin by default) and corner_radius? (mm, rounded corners) | circle? {center, radius} | points? [[x, y]…] (the corners, at least three, or two with an arc; [] takes the outline away) with curves? [sweep…] (degrees per edge, the closing edge last; counter-clockwise when positive, 0 straight); thickness? (mm; 1.6 by default) → {outline, curves?, size, thickness}",
        ),
    CommandSpec::new("ecad.board.sync", "Sync from Schematic", crate::checked!("ecad.board.sync", sync))
        .icon("update")
        .doc_only()
        .contract(&SYNC_CONTRACT)
        .output(output_schema_for::<SyncResult>)
        .params(
            "No parameters → {added: [{id, part, designator, footprint, at, …, pads: [{number, at, copper, net}]}], removed, kept, no_footprint: [designators], unrouted}: a footprint for each schematic part that has none (its variant's footprint, laid out in rows under the outline on the 1.27 mm grid); footprints whose part is gone go; moved ones stay where they are. Parts whose variant has no footprint are listed in no_footprint",
        ),
    CommandSpec::new("ecad.board.move", "Move on Board", crate::checked!("ecad.board.move", move_items))
        .icon("move")
        .doc_only()
        .contract(&MOVE_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "items: [designator|id] (footprints by their part's designator; footprints, tracks, vias, holes and drawings by id); by? [dx, dy] | to? [x, y] (where the first item goes); rotation? (degrees, a multiple of 90); flip? (to the other side: footprints change side, tracks and drawings change layer); about? [x, y] (the one item's position, or the middle of them); check? (true: refused when it brings the moved copper, or tracks whose ends follow it, closer to another net's copper than the clearance where it wasn't) → {created, changed, removed, footprints, unrouted}. The ends of tracks left behind that sit on a moved footprint's pad or a moved via move with it (on a flip, ends on surface-mount pads stay)",
        ),
    CommandSpec::new("ecad.board.rotate", "Rotate", crate::checked!("ecad.board.rotate", rotate))
        .icon("spin")
        .doc_only()
        .contract(&ROTATE_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "items: [designator|id]; rotation? (degrees, a multiple of 90; 90 by default); check? (as ecad.board.move's) → {changed, footprints, unrouted}: footprints alone each turn about their own origin; with tracks, vias, holes or drawings among them, everything turns as one about the middle of them; track ends on a turned footprint's pads or via follow",
        ),
    CommandSpec::new("ecad.board.flip", "Flip", crate::checked!("ecad.board.flip", flip))
        .icon("mirror")
        .doc_only()
        .contract(&ITEMS_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "items: [designator|id]; check? (as ecad.board.move's) → {changed, footprints, unrouted}: footprints go to the other side of the board, footprints alone each mirrored about its own origin, a mix with tracks, vias, holes or drawings all as one about the middle of them; tracks and drawings change to the layer in their place on the turned-over stack; track ends on a flipped footprint's through-hole pads follow, ends on its surface-mount pads stay",
        ),
    CommandSpec::new("ecad.board.delete", "Delete", crate::checked!("ecad.board.delete", delete))
        .icon("delete")
        .doc_only()
        .contract(&DELETE_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "items?: [designator|id]; segments?: [{track, segment}] (single segments cut out of tracks: what is left before a cut keeps the id, each piece after it is a new track); all or none → {created, changed, removed, unrouted}; tracks on a deleted footprint's pads stay. Sync from Schematic places a deleted footprint again while its part is on the schematic",
        ),
    CommandSpec::new("ecad.board.track", "Route", crate::checked!("ecad.board.track", track))
        .icon("trace")
        .doc_only()
        .contract(&TRACK_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "layer: top|inner1…inner14|bottom (a layer of the stack); width? (mm; left out, the track width of the net class of the copper at the first point, else the rules' 0.25); points: [[x, y]…] (2 or more; a point inside a pad, on another track of the layer or on a via of the layer joins it; one alone with `via` is just the via, for a route that changes layer where it starts; a run that turns straight back on itself is refused); curves? [sweep…] (degrees per segment, counter-clockwise when positive, 0 straight); via? {from?, to?, diameter?, drill?, protection?} (a via at the last point in the same step, for going on on another layer: top to bottom when the span is left out; sizes the net class's, or the rules' micro via between neighbouring layers; none where a via covering that span or a through-hole pad is already); legs? [{layer, width?, points, curves?, via?}] (the route's next legs, each as the first, usually starting on the via before; a width left out is the first leg's; up to 63; the whole route is one undo step) → {created: [{id, kind, net}], unrouted}",
        ),
    CommandSpec::new("ecad.board.track.edit", "Edit Track", crate::checked!("ecad.board.track.edit", track_edit))
        .icon("edit")
        .doc_only()
        .contract(&TRACK_EDIT_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params("tracks: [id]; layer? (a copper layer of the stack); width? (mm) → {changed: [{id, kind, net}], unrouted}: all or none"),
    CommandSpec::new("ecad.board.via", "Via", crate::checked!("ecad.board.via", via))
        .icon("hole")
        .doc_only()
        .contract(&VIA_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "at: [x, y]; from? to? (its span; top to bottom when left out: an outer layer to an inner one is blind, inner to inner buried, neighbouring layers no wider than the rules' micro via a micro via); diameter? drill? (mm; the net class's via, 0.6 and 0.3 by default, or the rules' micro via between neighbouring layers); net? (a net of its own: a stitching via that joins its net's pours); protection? open|tented|plugged|filled|filled_capped (open when left out; only filled_capped may sit in a pad) → {created: [{id, kind, net}], unrouted}: a via, joining the copper of the layers of its span it lies on",
        ),
    CommandSpec::new("ecad.board.inspect", "Inspect Board", crate::checked!("ecad.board.inspect", inspect))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&INSPECT_CONTRACT)
        .output(output_schema_for::<InspectResult>)
        .params(
            "No parameters → {outline, outline_curves?, size, thickness, copper_layers, stack: [{name, kind}], stackup: [{kind, thickness, er?, loss_tangent?}], custom_stackup, rules, net_classes?, name_height?, footprints: [{id, part, designator, value, footprint, at, rotation, side, bbox, name_at, value_at, show_name, show_value, pads: [{number, at, copper, net}]}], tracks: [{id, layer, width, points, curves?, length, net}], vias: [{id, at, diameter, drill, from, to, kind, net, own_net?}], holes: [{id, at, drill}], graphics: [{id, layer, mirror?, type, …}], pours: [{id, layer, net, outline, …, fill: {polygons, area, islands_removed}}], keepouts: [{id, layers, all_layers, outline, forbid}], nets: [{name, pads: [\"R1.2\"…], airwires}], airwires: [{net, from, to}], unrouted, truncated?}. Pads take their nets from the schematic; copper joins on one layer where a track's point lies in a pad, on another track of its layer or on a via (through-hole pads are on every layer, vias on their span), and through a pour's fill; airwires are the shortest connections still to route in each net. Read-only",
        ),
];

const OUTLINE_CONTRACT: TypedContract<OutlineParams> = TypedContract::new();
const SYNC_CONTRACT: TypedContract<SyncParams> = TypedContract::new();
const MOVE_CONTRACT: TypedContract<MoveParams> = TypedContract::new();
const ROTATE_CONTRACT: TypedContract<RotateParams> = TypedContract::new();
const ITEMS_CONTRACT: TypedContract<ItemsParams> = TypedContract::new();
const DELETE_CONTRACT: TypedContract<DeleteParams> = TypedContract::new();
const TRACK_CONTRACT: TypedContract<TrackParams> = TypedContract::new();
const TRACK_EDIT_CONTRACT: TypedContract<TrackEditParams> = TypedContract::new();
const VIA_CONTRACT: TypedContract<ViaParams> = TypedContract::new();
const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();

/// The most items one call names: everything a board can hold.
pub(super) const MAX_ITEMS: usize = MAX_BOARD_FOOTPRINTS + MAX_TRACKS + MAX_VIAS + MAX_HOLES + MAX_BOARD_GRAPHICS + MAX_POURS + MAX_KEEPOUTS;
/// How wide the rows of footprints placed under a board with no outline grow, in millimetres.
const SYNC_WIDTH: f64 = 100.0;
/// The narrowest the rows of footprints placed under an outline grow, in millimetres.
const SYNC_MIN_WIDTH: f64 = 50.0;

/// The board's objects by id and its footprints by their part's designator (lower case).
pub(super) struct Index {
    kinds: BTreeMap<u64, BoardKind>,
    designators: BTreeMap<String, u64>,
}

impl Index {
    pub(super) fn new(s: &Session) -> Self {
        let ecad = &s.doc.ecad;
        let board = &ecad.board;
        let mut kinds = BTreeMap::new();
        kinds.extend(board.footprints.iter().map(|f| (f.id, BoardKind::Footprint)));
        kinds.extend(board.tracks.iter().map(|t| (t.id, BoardKind::Track)));
        kinds.extend(board.vias.iter().map(|v| (v.id, BoardKind::Via)));
        kinds.extend(board.holes.iter().map(|h| (h.id, BoardKind::Hole)));
        kinds.extend(board.graphics.iter().map(|g| (g.id, BoardKind::Graphic)));
        kinds.extend(board.pours.iter().map(|p| (p.id, BoardKind::Pour)));
        kinds.extend(board.keepouts.iter().map(|k| (k.id, BoardKind::Keepout)));
        let designators =
            board.footprints.iter().filter_map(|f| Some((ecad.schematic.part(f.part)?.designator.to_ascii_lowercase(), f.id))).collect();
        Index { kinds, designators }
    }

    /// What the board object `id` is.
    pub(super) fn kind(&self, id: u64) -> Option<BoardKind> {
        self.kinds.get(&id).copied()
    }

    /// The board object `r` names; `what` names it in errors (`items[2]`).
    pub(super) fn find(&self, s: &Session, r: &ItemRef, cmd: &str, what: &str) -> Result<u64> {
        match r {
            ItemRef::Id(id) => {
                if self.kinds.contains_key(id) {
                    return Ok(*id);
                }
                let mut msg = format!("{what}: the board has nothing with the id {id}");
                if s.doc.ecad.schematic.kind_of(*id).is_some() {
                    msg.push_str(" (that is a schematic id: name a footprint by its part's designator)");
                }
                Err(bad(cmd, msg))
            }
            ItemRef::Designator(d) => {
                if let Some(id) = self.designators.get(&d.to_ascii_lowercase()) {
                    return Ok(*id);
                }
                let mut msg = format!("{what}: no footprint on the board is for a part designated `{d}`");
                let ecad = &s.doc.ecad;
                if ecad.schematic.designated(d).is_some() {
                    msg.push_str(" (the part has no footprint yet: ecad.board.sync places it)");
                } else if let Some(near) =
                    closest(d, ecad.board.footprints.iter().filter_map(|f| ecad.schematic.part(f.part)).map(|p| p.designator.as_str()))
                {
                    msg.push_str(&format!(" (did you mean `{near}`?)"));
                }
                Err(bad(cmd, msg))
            }
        }
    }
}

/// The board objects `items` name, at least one, each once.
pub(super) fn selection(s: &Session, items: &[ItemRef], cmd: &str) -> Result<Vec<u64>> {
    if items.is_empty() || items.len() > MAX_ITEMS {
        return Err(bad(cmd, format!("name 1 to {MAX_ITEMS} board objects in `items`, footprints by designator or anything by id")));
    }
    let index = Index::new(s);
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for (i, r) in items.iter().enumerate() {
        let id = index.find(s, r, cmd, &format!("items[{i}]"))?;
        if seen.insert(id) {
            out.push(id);
        }
    }
    Ok(out)
}

/// Store `board` once it passes the design file's checks against the schematic; a refused board,
/// or one equal to what is stored, changes nothing (no undo step).
pub(super) fn store_board(s: &mut Session, board: Board, cmd: &str) -> Result<()> {
    board.validate(&s.doc.ecad.schematic).map_err(|e| bad(cmd, e))?;
    if s.doc.ecad.board != board {
        s.doc_mut().ecad.board = board;
    }
    Ok(())
}

/// A point given on the board, rounded to 0.0001 mm.
pub(super) fn board_point(p: [f64; 2], cmd: &str, what: &str) -> Result<[f64; 2]> {
    check_board_point(what, p).map_err(|e| bad(cmd, e))?;
    Ok(snap_point(p))
}

/// A distance to move by: finite, and no more than twice across the board's room.
fn offset(p: [f64; 2], cmd: &str, what: &str) -> Result<[f64; 2]> {
    let limit = 2.0 * MAX_BOARD;
    if !(p[0].is_finite() && p[1].is_finite() && p[0].abs() <= limit && p[1].abs() <= limit) {
        return Err(bad(cmd, format!("{what} must be within {limit} mm")));
    }
    Ok(snap_point(p))
}

/// The middle of `points` on the board grid.
fn middle(points: &[[f64; 2]]) -> [f64; 2] {
    bbox(points.iter().copied()).map_or([0.0, 0.0], |[[x0, y0], [x1, y1]]| {
        snap_point([((x0 + x1) / 2.0 / BOARD_GRID).round() * BOARD_GRID, ((y0 + y1) / 2.0 / BOARD_GRID).round() * BOARD_GRID])
    })
}

fn outline(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.outline";
    let p: OutlineParams = parse(CMD, p)?;
    let mut board = s.doc.ecad.board.clone();
    if usize::from(p.rect.is_some()) + usize::from(p.points.is_some()) + usize::from(p.circle.is_some()) > 1 {
        return Err(bad(CMD, "give one of `rect`, `circle` or `points`"));
    }
    if p.rect.is_none() && (p.center.is_some() || p.corner_radius.is_some()) {
        return Err(bad(CMD, "`center` and `corner_radius` go with `rect`"));
    }
    if p.points.is_none() && p.curves.is_some() {
        return Err(bad(CMD, "`curves` goes with `points`"));
    }
    let edge = match (p.rect, p.circle, p.points) {
        (Some(size), _, _) => Some(rect_outline(size, p.center.unwrap_or([0.0, 0.0]), p.corner_radius.unwrap_or(0.0))?),
        (None, Some(c), _) => {
            let center = board_point(c.center, CMD, "`circle.center`")?;
            if !(c.radius.is_finite() && c.radius > 0.0 && c.radius <= MAX_BOARD) {
                return Err(bad(CMD, format!("`circle.radius` must be more than 0 and at most {MAX_BOARD} mm")));
            }
            let r = c.radius;
            let ends = [[center[0] + r, center[1]], [center[0] - r, center[1]]].map(snap_point).to_vec();
            Some((ends, vec![180.0, 180.0]))
        }
        (None, None, Some(points)) => {
            if points.len() > MAX_OUTLINE_POINTS {
                return Err(bad(CMD, format!("an outline has at most {MAX_OUTLINE_POINTS} corners")));
            }
            let mut corners = points.iter().enumerate().map(|(i, q)| board_point(*q, CMD, &format!("points[{i}]"))).collect::<Result<Vec<_>>>()?;
            let mut curves = p.curves.unwrap_or_default();
            check_curves(&curves, corners.len()).map_err(|e| bad(CMD, e))?;
            tidy(&mut corners, &mut curves, true);
            Some((corners, curves))
        }
        (None, None, None) if p.thickness.is_some() => None,
        (None, None, None) => return Err(bad(CMD, "give `rect` [width, height], a `circle` or the corners in `points`")),
    };
    if let Some((corners, curves)) = edge {
        board.outline = corners;
        board.outline_curves = curves;
    }
    if let Some(t) = p.thickness {
        if !THICKNESS.contains(&t) {
            return Err(bad(CMD, format!("`thickness` must be {} to {} mm", THICKNESS.start(), THICKNESS.end())));
        }
        board.set_thickness(t).map_err(|e| bad(CMD, e))?;
    }
    store_board(s, board, CMD)?;
    let board = &s.doc.ecad.board;
    reply(OutlineResult {
        outline: board.outline.clone(),
        curves: board.outline_curves.clone(),
        size: size(board),
        thickness: board.thickness,
        param_warnings: Vec::new(),
    })
}

/// A `w` × `h` rectangle centred on `center`, its corners rounded to `radius` (at most half its
/// shorter side), counter-clockwise from its bottom edge.
fn rect_outline([w, h]: [f64; 2], center: [f64; 2], radius: f64) -> Result<(Vec<[f64; 2]>, Vec<f64>)> {
    const CMD: &str = "ecad.board.outline";
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 && w <= MAX_BOARD && h <= MAX_BOARD) {
        return Err(bad(CMD, format!("`rect` must be a width and a height, each more than 0 and at most {MAX_BOARD} mm")));
    }
    let [cx, cy] = board_point(center, CMD, "`center`")?;
    if !(radius.is_finite() && radius >= 0.0) {
        return Err(bad(CMD, "`corner_radius` must be 0 or more"));
    }
    let (x, y) = (w / 2.0, h / 2.0);
    let r = radius.min(x).min(y);
    if r <= 0.0 {
        return Ok(([[cx - x, cy - y], [cx + x, cy - y], [cx + x, cy + y], [cx - x, cy + y]].map(snap_point).to_vec(), Vec::new()));
    }
    let mut points = [
        [cx - x + r, cy - y],
        [cx + x - r, cy - y],
        [cx + x, cy - y + r],
        [cx + x, cy + y - r],
        [cx + x - r, cy + y],
        [cx - x + r, cy + y],
        [cx - x, cy + y - r],
        [cx - x, cy - y + r],
    ]
    .map(snap_point)
    .to_vec();
    let mut curves = [0.0, 90.0].repeat(4);
    tidy(&mut points, &mut curves, true);
    Ok((points, curves))
}

/// The outline's width and height.
fn size(board: &Board) -> Option<[f64; 2]> {
    board.outline_bbox().map(|[[x0, y0], [x1, y1]]| snap_point([x1 - x0, y1 - y0]))
}

fn sync(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.sync";
    let _: SyncParams = parse(CMD, p)?;
    let ecad = &s.doc.ecad;
    let mut board = ecad.board.clone();
    let removed: Vec<BoardItemOut> =
        board.forget_parts(&ecad.schematic).into_iter().map(|f| BoardItemOut { id: f.id, kind: KindOut::Footprint, name: None, net: None }).collect();
    let kept = board.footprints.len();
    // Rows go under the outline and what the board holds already.
    let held = board.footprints.iter().filter_map(|f| {
        let item = ecad.placed(f)?.resolved.footprint;
        footprint_box(item).map(|b| f.placement().bbox(b))
    });
    let content = bbox(board.outline_bbox().into_iter().flatten().chain(held.flatten()));
    let width = board.outline_bbox().map_or(SYNC_WIDTH, |[[x0, _], [x1, _]]| (x1 - x0).max(SYNC_MIN_WIDTH));
    let mut no_footprint = Vec::new();
    let mut new: Vec<(u64, [[f64; 2]; 2])> = Vec::new();
    for part in &ecad.schematic.parts {
        if board.footprint_of(part.id).is_some() {
            continue;
        }
        let Some(c) = ecad.component(part.component) else { continue };
        let r = ecad.resolve(c, part.variant.as_deref());
        if r.variant.and_then(|v| v.footprint).is_none() {
            no_footprint.push(part.designator.clone());
            continue;
        }
        new.push((part.id, footprint_box(r.footprint).unwrap_or_default()));
    }
    let boxes: Vec<[[f64; 2]; 2]> = new.iter().map(|(_, b)| *b).collect();
    let origins = flow_with(content, &boxes, Flow { gap: BOARD_GAP, width, grid: BOARD_GRID });
    let mut added = Vec::with_capacity(new.len());
    for ((part, _), at) in new.into_iter().zip(origins) {
        let id = board.take_id().map_err(|e| bad(CMD, e))?;
        board.footprints.push(BoardFootprint {
            id,
            part,
            at,
            rotation: 0,
            side: solidcraft_doc::ecad::Side::Top,
            name_at: None,
            value_at: None,
            show_name: true,
            show_value: false,
            pour_connect: None,
            pad_connect: BTreeMap::new(),
        });
        added.push(id);
    }
    store_board(s, board, CMD)?;
    let nets = s.board_nets();
    let board = &s.doc.ecad.board;
    let added = board.footprints.iter().filter(|f| added.contains(&f.id)).map(|f| footprint_out(s, f, &nets)).collect();
    reply(SyncResult { added, removed, kept, no_footprint, unrouted: nets.airwires.len(), param_warnings: Vec::new() })
}

fn move_items(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.move";
    let p: MoveParams = parse(CMD, p)?;
    let old = s.doc.ecad.board.clone();
    let items = selection(s, &p.items, CMD)?;
    let rotation = quarter(p.rotation, CMD, "")?;
    let flip = p.flip.unwrap_or(false);
    if p.by.is_some() && p.to.is_some() {
        return Err(bad(CMD, "give `by` or `to`, not both"));
    }
    if p.by.is_none() && p.to.is_none() && rotation == 0 && !flip {
        return Err(bad(CMD, "give `by` or `to` to move, `rotation` to turn or `flip` to flip"));
    }
    let places: Vec<[f64; 2]> = items.iter().filter_map(|id| old.position(*id)).collect();
    let about = match p.about {
        Some(a) => board_point(a, CMD, "`about`")?,
        None if places.len() == 1 => places.first().copied().unwrap_or_default(),
        None => middle(&places),
    };
    let turned = Motion { about, mirror: flip, rotation, by: [0.0, 0.0] };
    let by = match (p.by, p.to) {
        (Some(by), _) => offset(by, CMD, "`by`")?,
        (None, Some(to)) => {
            let to = board_point(to, CMD, "`to`")?;
            let from = places.first().map(|q| turned.point(*q)).unwrap_or(to);
            snap_point([to[0] - from[0], to[1] - from[1]])
        }
        (None, None) => [0.0, 0.0],
    };
    let mut board = old.clone();
    let moved: BTreeSet<u64> = items.into_iter().collect();
    s.doc.ecad.move_on_board(&mut board, &[(moved.clone(), Motion { by, ..turned })]);
    if p.check.unwrap_or(false) {
        refuse_clashes(s, &old, &board, &moved, CMD)?;
    }
    store_board(s, board, CMD)?;
    reply(board_result(s, &old))
}

/// Why a move (turn, flip, reshape) that `check` asked to be checked is refused: the closest copper it
/// brings too near another net's, where it wasn't before (see `drc::moved_clashes`).
pub(crate) fn refuse_clashes(s: &Session, old: &Board, board: &Board, moved: &BTreeSet<u64>, cmd: &str) -> Result<()> {
    let before = s.board_nets();
    let conn = s.doc.ecad.connectivity();
    let after = solidcraft_doc::ecad::airwires::board_nets(&s.doc.ecad, board, &conn);
    let (found, _) = solidcraft_doc::ecad::drc::moved_clashes(old, &before, board, &after, moved);
    let Some(m) = found.first() else { return Ok(()) };
    let (piece, c) = (m.moved.reference(), &m.clash);
    let mine = m.net.and_then(|n| after.name(n)).unwrap_or("no net");
    let net = c.net.and_then(|n| after.name(n)).unwrap_or("no net");
    Err(bad(
        cmd,
        format!(
            "{piece} on {mine} would be {} mm from {} on {net} ({}), and the clearance is {} mm ({} clash{} in all): leave out `check` to move it anyway",
            snap(c.gap),
            c.item.reference(),
            c.layer.name(),
            snap(c.required),
            found.len(),
            if found.len() == 1 { "" } else { "es" }
        ),
    ))
}

/// Turn (`rotation`) or flip the board objects `items`: footprints alone each about its own
/// origin; anything else with them, all as one about the middle of them (so picked tracks stay on
/// picked pads); the ends of tracks on them follow.
fn turn_each(s: &mut Session, items: &[ItemRef], rotation: u16, flip: bool, check: bool, cmd: &str) -> Result<Value> {
    let old = s.doc.ecad.board.clone();
    let ids = selection(s, items, cmd)?;
    let mut board = old.clone();
    let groups = solidcraft_doc::ecad::rework::turn_groups(&old, &ids, rotation, flip);
    s.doc.ecad.move_on_board(&mut board, &groups);
    if check {
        refuse_clashes(s, &old, &board, &ids.iter().copied().collect(), cmd)?;
    }
    store_board(s, board, cmd)?;
    reply(board_result(s, &old))
}

fn rotate(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.rotate";
    let p: RotateParams = parse(CMD, p)?;
    let rotation = quarter(Some(p.rotation.unwrap_or(90)), CMD, "")?;
    turn_each(s, &p.items, rotation, false, p.check.unwrap_or(false), CMD)
}

fn flip(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.flip";
    let p: ItemsParams = parse(CMD, p)?;
    turn_each(s, &p.items, 0, true, p.check.unwrap_or(false), CMD)
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.delete";
    let p: DeleteParams = parse(CMD, p)?;
    if p.items.is_empty() && p.segments.is_empty() {
        return Err(bad(CMD, "name the board objects in `items`, or track segments in `segments`"));
    }
    if p.segments.len() > MAX_ITEMS {
        return Err(bad(CMD, format!("name at most {MAX_ITEMS} segments")));
    }
    let old = s.doc.ecad.board.clone();
    let ids: BTreeSet<u64> = if p.items.is_empty() { BTreeSet::new() } else { selection(s, &p.items, CMD)?.into_iter().collect() };
    let mut cuts: BTreeMap<u64, BTreeSet<usize>> = BTreeMap::new();
    let tracks: BTreeSet<u64> = old.tracks.iter().map(|t| t.id).collect();
    for (i, cut) in p.segments.iter().enumerate() {
        if !tracks.contains(&cut.track) {
            return Err(bad(CMD, format!("segments[{i}]: the board has no track with the id {}", cut.track)));
        }
        // A track deleted whole needs no cut.
        if !ids.contains(&cut.track) {
            cuts.entry(cut.track).or_default().insert(cut.segment);
        }
    }
    let mut board = old.clone();
    board.remove(&ids);
    board.cut_segments(&cuts).map_err(|e| bad(CMD, e))?;
    store_board(s, board, CMD)?;
    reply(board_result(s, &old))
}

/// A track's points and sweeps as given: on the board, rounded to 0.0001 mm, a point given twice in
/// a row taken once (with the sweep of the segment out of it), at least two left.
pub(super) fn track_geometry(points: &[[f64; 2]], curves: Option<Vec<f64>>, cmd: &str) -> Result<(Vec<[f64; 2]>, Vec<f64>)> {
    path_geometry(points, curves).map_err(|e| bad(cmd, e))
}

/// A track's points and sweeps as a call gives them, checked and tidied; why not: too few or too
/// many points, one off the board, sweeps that don't fit, or a run that turns straight back on
/// itself.
fn path_geometry(points: &[[f64; 2]], curves: Option<Vec<f64>>) -> std::result::Result<(Vec<[f64; 2]>, Vec<f64>), String> {
    if points.len() < 2 || points.len() > MAX_TRACK_POINTS {
        return Err(format!("give 2 to {MAX_TRACK_POINTS} points in `points`"));
    }
    let mut out = points
        .iter()
        .enumerate()
        .map(|(i, q)| check_board_point(&format!("points[{i}]"), *q).map(|()| snap_point(*q)))
        .collect::<std::result::Result<Vec<_>, String>>()?;
    let mut curves = curves.unwrap_or_default();
    check_curves(&curves, out.len() - 1)?;
    tidy(&mut out, &mut curves, false);
    if out.len() < 2 {
        return Err("a track needs two different points".into());
    }
    if let Some(q) = path::doubles_back(&out, &curves).and_then(|i| out.get(i)) {
        return Err(format!("the track turns straight back on itself at [{}, {}]: run it one way only", q[0], q[1]));
    }
    Ok((out, curves))
}

/// The net of the copper at `at` on `layer` (a pad, a track's line, a via), by name.
pub(super) fn net_at(board: &Board, nets: &BoardNets, at: [f64; 2], layer: CopperLayer) -> Option<String> {
    let name = |n: Option<usize>| n.and_then(|n| nets.name(n)).map(str::to_string);
    if let Some(p) = nets.pads.iter().find(|p| p.on(layer) && p.contains(at)) {
        return name(p.net);
    }
    let on_track = |t: &Track| {
        t.layer == layer && path::segments(&t.points, &t.curves, false).any(|(a, b, s)| path::segment_distance(at, a, b, s) <= t.width / 2.0)
    };
    if let Some(i) = board.tracks.iter().position(on_track) {
        return name(nets.track_nets.get(i).copied().flatten());
    }
    let on_via = |v: &Via| v.on(layer, board.copper_layers) && (v.at[0] - at[0]).hypot(v.at[1] - at[1]) <= v.diameter / 2.0;
    board.vias.iter().position(on_via).and_then(|i| name(nets.via_nets.get(i).copied().flatten()))
}

/// A via's span from what a call gives (top to bottom when left out), top layer first; why not
/// when a layer isn't on the stack or the two are one.
pub(super) fn via_span(
    board: &Board,
    from: Option<CopperLayer>,
    to: Option<CopperLayer>,
    cmd: &str,
    what: &str,
) -> Result<(CopperLayer, CopperLayer)> {
    let (a, b) = (from.unwrap_or(CopperLayer::Top), to.unwrap_or(CopperLayer::Bottom));
    for l in [a, b] {
        if !board.has_layer(l) {
            return Err(bad(cmd, format!("{what}{}", no_layer(board, l.name()))));
        }
    }
    let n = board.copper_layers;
    match a.index(n).cmp(&b.index(n)) {
        std::cmp::Ordering::Less => Ok((a, b)),
        std::cmp::Ordering::Greater => Ok((b, a)),
        std::cmp::Ordering::Equal => Err(bad(cmd, format!("{what}a via goes between two different layers (`from` and `to`)"))),
    }
}

/// A new via's diameter and drill: those given, or the class's of `net` (the rules' micro via
/// for a span of two neighbouring layers that isn't top to bottom).
pub(super) fn via_size(board: &Board, net: Option<&str>, span: (CopperLayer, CopperLayer), diameter: Option<f64>, drill: Option<f64>) -> (f64, f64) {
    let n = board.copper_layers;
    let neighbours = span.1.index(n) == span.0.index(n) + 1 && !(span.0 == CopperLayer::Top && span.1 == CopperLayer::Bottom);
    let r = &board.rules;
    let (d, h) = if neighbours {
        (r.microvia_diameter, r.microvia_drill)
    } else {
        let sizes = r.sizes(net);
        (sizes.via_diameter, sizes.via_drill)
    };
    (diameter.unwrap_or(d), drill.unwrap_or(h))
}

fn track(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.track";
    let p: TrackParams = parse(CMD, p)?;
    let old = s.doc.ecad.board.clone();
    let more = p.legs.unwrap_or_default();
    if more.len() >= MAX_LEGS {
        return Err(bad(CMD, format!("a route has at most {MAX_LEGS} legs, the first and its `legs`")));
    }
    let first = TrackLeg { layer: p.layer, width: p.width, points: p.points, curves: p.curves, via: p.via };
    let legs: Vec<TrackLeg> = std::iter::once(first).chain(more).collect();
    // The net of the copper it starts on gives the class's sizes.
    let needs_net = p.width.is_none() || legs.iter().any(|l| l.via.as_ref().is_some_and(|v| v.diameter.is_none() || v.drill.is_none()));
    let net = match legs.first() {
        Some(l) if needs_net => {
            let nets = s.board_nets();
            l.points.first().and_then(|q| net_at(&old, &nets, *q, l.layer.0))
        }
        _ => None,
    };
    let width = p.width.unwrap_or_else(|| old.rules.sizes(net.as_deref()).track_width);
    let mut board = old.clone();
    for (i, leg) in legs.into_iter().enumerate() {
        let what = if i == 0 { String::new() } else { format!("legs[{}]: ", i - 1) };
        let layer = leg.layer.0;
        if !old.has_layer(layer) {
            return Err(bad(CMD, format!("{what}{}", no_layer(&old, layer.name()))));
        }
        // One point with a via is the via alone.
        let (last, made) = match (leg.points.as_slice(), &leg.via) {
            ([q], Some(_)) => (board_point(*q, CMD, &format!("{what}points[0]"))?, None),
            _ => {
                let (points, curves) = path_geometry(&leg.points, leg.curves).map_err(|e| bad(CMD, format!("{what}{e}")))?;
                (points.last().copied().unwrap_or_default(), Some((points, curves)))
            }
        };
        if let Some((points, curves)) = made {
            let width = leg.width.unwrap_or(width);
            check_track_width(width).map_err(|e| bad(CMD, format!("{what}{e}")))?;
            let id = board.take_id().map_err(|e| bad(CMD, e))?;
            board.tracks.push(Track { id, layer, width, points, curves });
        }
        if let Some(v) = leg.via {
            let at = format!("{what}`via`: ");
            let span = via_span(&old, v.from.map(|l| l.0), v.to.map(|l| l.0), CMD, &at)?;
            let (diameter, drill) = via_size(&old, net.as_deref(), span, v.diameter, v.drill);
            check_via(diameter, drill).map_err(|e| bad(CMD, format!("{at}{e}")))?;
            // A via an earlier leg dropped there counts too.
            if !covered(s, &board, last, span) {
                let id = board.take_id().map_err(|e| bad(CMD, e))?;
                let protection = v.protection.map(|p| p.doc()).unwrap_or_default();
                board.vias.push(Via {
                    id,
                    at: last,
                    diameter,
                    drill,
                    from: span.0,
                    to: span.1,
                    net: None,
                    protection,
                    role: ViaRole::Plain,
                    pattern: None,
                });
            }
        }
    }
    store_board(s, board, CMD)?;
    reply(board_result(s, &old))
}

/// Whether `at` joins the layers of `span` already: a via whose span covers it, or a
/// through-hole pad, is there.
fn covered(s: &Session, board: &Board, at: [f64; 2], span: (CopperLayer, CopperLayer)) -> bool {
    let ecad = &s.doc.ecad;
    let n = board.copper_layers;
    board.vias.iter().any(|v| (v.at[0] - at[0]).hypot(v.at[1] - at[1]) <= v.diameter / 2.0 && v.on(span.0, n) && v.on(span.1, n))
        || board.footprints.iter().flat_map(|f| footprint_pads(ecad, f, None)).any(|p| p.top && p.bottom && p.contains(at))
}

/// Why a layer the stack doesn't have is refused.
pub(super) fn no_layer(board: &Board, name: String) -> String {
    let names: Vec<String> = board.stack().into_iter().map(|l| l.name()).collect();
    format!("the board's stack has no copper layer `{name}`: it has {} (ecad.board.stack adds layers)", names.join(", "))
}

fn track_edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.track.edit";
    let p: TrackEditParams = parse(CMD, p)?;
    if p.tracks.is_empty() || p.tracks.len() > MAX_TRACKS {
        return Err(bad(CMD, format!("name 1 to {MAX_TRACKS} tracks in `tracks`, by id")));
    }
    if p.layer.is_none() && p.width.is_none() {
        return Err(bad(CMD, "give `layer` or `width` to change"));
    }
    if let Some(w) = p.width {
        check_track_width(w).map_err(|e| bad(CMD, e))?;
    }
    let old = s.doc.ecad.board.clone();
    let mut board = old.clone();
    let at: BTreeMap<u64, usize> = old.tracks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
    for (i, id) in p.tracks.iter().enumerate() {
        let Some(t) = at.get(id).and_then(|k| board.tracks.get_mut(*k)) else {
            return Err(bad(CMD, format!("tracks[{i}]: the board has no track with the id {id}")));
        };
        if let Some(layer) = p.layer {
            if !old.has_layer(layer.0) {
                return Err(bad(CMD, no_layer(&old, layer.0.name())));
            }
            t.layer = layer.0;
        }
        if let Some(w) = p.width {
            t.width = w;
        }
    }
    store_board(s, board, CMD)?;
    reply(board_result(s, &old))
}

fn via(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.via";
    let p: ViaParams = parse(CMD, p)?;
    let old = s.doc.ecad.board.clone();
    let at = board_point(p.at, CMD, "`at`")?;
    let span = via_span(&old, p.from.map(|l| l.0), p.to.map(|l| l.0), CMD, "")?;
    let net = p.net.map(|n| known_net(s, &n, CMD)).transpose()?;
    let (diameter, drill) = via_size(&old, net.as_deref(), span, p.diameter, p.drill);
    check_via(diameter, drill).map_err(|e| bad(CMD, format!("a via's {e}")))?;
    let mut board = old.clone();
    let id = board.take_id().map_err(|e| bad(CMD, e))?;
    let protection = p.protection.map(|p| p.doc()).unwrap_or_default();
    board.vias.push(Via { id, at, diameter, drill, from: span.0, to: span.1, net, protection, role: ViaRole::Plain, pattern: None });
    store_board(s, board, CMD)?;
    reply(board_result(s, &old))
}

fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.inspect";
    let _: InspectParams = parse(CMD, p)?;
    let nets = s.board_nets();
    let board = &s.doc.ecad.board;
    let name = |n: Option<usize>| n.and_then(|n| nets.name(n)).map(str::to_string);
    let footprints = board.footprints.iter().map(|f| footprint_out(s, f, &nets)).collect();
    let tracks = board
        .tracks
        .iter()
        .zip(&nets.track_nets)
        .map(|(t, n)| TrackOut {
            id: t.id,
            layer: t.layer.name(),
            width: t.width,
            points: t.points.clone(),
            curves: t.curves.clone(),
            length: snap(path::length(&t.points, &t.curves, false)),
            net: name(*n),
        })
        .collect();
    let holes = board.holes.iter().map(|h| HoleOut { id: h.id, at: h.at, drill: h.drill }).collect();
    let graphics = board
        .graphics
        .iter()
        .map(|g| BoardGraphicOut { id: g.id, layer: g.layer.name(), mirror: g.mirror, graphic: graphic_out(&g.graphic) })
        .collect();
    let vias = board.vias.iter().zip(&nets.via_nets).map(|(v, n)| via_out(board, v, name(*n))).collect();
    let mut by_net: BTreeMap<usize, (Vec<String>, usize)> = BTreeMap::new();
    for pad in &nets.pads {
        if let Some(n) = pad.net {
            by_net.entry(n).or_default().0.push(pad.reference());
        }
    }
    for a in &nets.airwires {
        by_net.entry(a.net).or_default().1 += 1;
    }
    let net_list = by_net.into_iter().filter_map(|(n, (pads, airwires))| Some(NetOut { name: nets.name(n)?.to_string(), pads, airwires })).collect();
    let airwires = nets
        .airwires
        .iter()
        .map(|a| AirwireOut { net: nets.name(a.net).unwrap_or_default().to_string(), from: snap_point(a.from), to: snap_point(a.to) })
        .collect();
    let pours = board.pours.iter().map(|p| pour_out(board, p, &nets.fills, &nets.names)).collect();
    let keepouts = board.keepouts.iter().map(keepout_out).collect();
    reply(InspectResult {
        outline: board.outline.clone(),
        outline_curves: board.outline_curves.clone(),
        size: size(board),
        thickness: board.thickness,
        copper_layers: board.copper_layers,
        stack: stack_out(board),
        stackup: stackup_out(&board.stackup_or_default()),
        custom_stackup: !board.stackup.is_empty(),
        rules: rules_out(&board.rules),
        net_classes: classes_out(board, &nets.names),
        name_height: board.name_height,
        footprints,
        tracks,
        vias,
        holes,
        graphics,
        pours,
        keepouts,
        nets: net_list,
        airwires,
        unrouted: nets.airwires.len(),
        truncated: nets.truncated,
        param_warnings: Vec::new(),
    })
}

/// A via as replies give it; `net` is the net of the copper it joins.
pub(super) fn via_out(board: &Board, v: &Via, net: Option<String>) -> ViaOut {
    let kind = match v.kind(board.copper_layers, &board.rules) {
        ViaKind::Through => ViaKindOut::Through,
        ViaKind::Blind => ViaKindOut::Blind,
        ViaKind::Buried => ViaKindOut::Buried,
        ViaKind::Micro => ViaKindOut::Micro,
    };
    ViaOut {
        id: v.id,
        at: v.at,
        diameter: v.diameter,
        drill: v.drill,
        from: v.from.name(),
        to: v.to.name(),
        kind,
        net,
        own_net: v.net.clone(),
        protection: crate::schema::ecad_board::ViaProtectionParam::of(v.protection),
        role: (v.role == ViaRole::Stitch).then(|| "stitch".to_string()),
        pattern: v.pattern.map(super::ecad_board_stitch::pattern_out),
    }
}

/// A stack-up as replies give it.
pub(super) fn stackup_out(layers: &[StackLayer]) -> Vec<StackLayerSpec> {
    layers
        .iter()
        .map(|l| StackLayerSpec {
            kind: match l.kind {
                StackKind::Copper => StackKindParam::Copper,
                StackKind::Core => StackKindParam::Core,
                StackKind::Prepreg => StackKindParam::Prepreg,
                StackKind::Mask => StackKindParam::Mask,
            },
            thickness: snap(l.thickness),
            er: l.er,
            loss_tangent: l.loss_tangent,
        })
        .collect()
}

/// The board's copper layers, top to bottom.
pub(super) fn stack_out(board: &Board) -> Vec<StackLayerOut> {
    board
        .stack()
        .into_iter()
        .map(|l| StackLayerOut { name: l.name(), kind: if l.is_inner() { LayerKind::Inner } else { LayerKind::Outer } })
        .collect()
}

/// A footprint in full, with its pads where they lie and their nets.
fn footprint_out(s: &Session, f: &BoardFootprint, nets: &BoardNets) -> FootprintOut {
    let ecad = &s.doc.ecad;
    let placed = ecad.placed(f);
    let item = placed.as_ref().map(|p| p.resolved.footprint);
    let value = placed.as_ref().map(|p| p.part.value.clone().unwrap_or_else(|| p.resolved.component.value.clone())).unwrap_or_default();
    let pads: Vec<PadOut> = nets
        .pads
        .iter()
        .filter(|pad| pad.footprint == f.id)
        .map(|pad| PadOut {
            number: pad.pad.number.clone(),
            at: snap_point(pad.at()),
            copper: match (pad.top, pad.bottom) {
                (true, true) => Copper::Both,
                (true, false) => Copper::Top,
                (false, true) => Copper::Bottom,
                (false, false) => Copper::None,
            },
            net: pad.net.and_then(|n| nets.name(n)).map(str::to_string),
            connect: f.pad_connect.get(&pad.pad.number).copied().map(super::ecad_board_pour::connect_out),
        })
        .collect();
    FootprintOut {
        id: f.id,
        part: f.part,
        designator: placed.as_ref().map(|p| p.part.designator.clone()).unwrap_or_default(),
        value,
        footprint: item.map(|i| i.name.clone()).unwrap_or_default(),
        at: f.at,
        rotation: f.rotation,
        side: side_out(f.side),
        bbox: item.and_then(footprint_box).map(|b| f.placement().bbox(b)).map(|[a, b]| [snap_point(a), snap_point(b)]),
        name_at: f.name_at,
        value_at: f.value_at,
        show_name: f.show_name,
        show_value: f.show_value,
        pour_connect: f.pour_connect.map(super::ecad_board_pour::connect_out),
        stale_connect: f.pad_connect.keys().filter(|n| !pads.iter().any(|p| p.number == **n)).cloned().collect(),
        pads,
    }
}

/// A board object, to tell whether it changed.
#[derive(PartialEq)]
enum Object<'a> {
    Footprint(&'a BoardFootprint),
    Track(&'a Track),
    Via(&'a Via),
    Hole(&'a Hole),
    Graphic(&'a BoardGraphic),
    Pour(&'a Pour),
    Keepout(&'a Keepout),
}

/// Every board object in board order, with how replies name it (nets from `nets`, when given).
fn objects<'a>(s: &Session, board: &'a Board, nets: Option<&BoardNets>) -> Vec<(BoardItemOut, Object<'a>)> {
    let sheet = &s.doc.ecad.schematic;
    let net = |n: Option<&Option<usize>>| n.copied().flatten().and_then(|n| nets?.name(n)).map(str::to_string);
    let footprints = board.footprints.iter().map(|f| {
        let name = sheet.part(f.part).map(|p| p.designator.clone());
        (BoardItemOut { id: f.id, kind: KindOut::Footprint, name, net: None }, Object::Footprint(f))
    });
    let tracks = board.tracks.iter().enumerate().map(|(i, t)| {
        let n = net(nets.and_then(|n| n.track_nets.get(i)));
        (BoardItemOut { id: t.id, kind: KindOut::Track, name: None, net: n }, Object::Track(t))
    });
    let vias = board.vias.iter().enumerate().map(|(i, v)| {
        let n = net(nets.and_then(|n| n.via_nets.get(i)));
        (BoardItemOut { id: v.id, kind: KindOut::Via, name: None, net: n }, Object::Via(v))
    });
    let holes = board.holes.iter().map(|h| (BoardItemOut { id: h.id, kind: KindOut::Hole, name: None, net: None }, Object::Hole(h)));
    let graphics = board.graphics.iter().map(|g| (BoardItemOut { id: g.id, kind: KindOut::Graphic, name: None, net: None }, Object::Graphic(g)));
    let pours = board.pours.iter().map(|p| (BoardItemOut { id: p.id, kind: KindOut::Pour, name: None, net: p.net.clone() }, Object::Pour(p)));
    let keepouts = board.keepouts.iter().map(|k| (BoardItemOut { id: k.id, kind: KindOut::Keepout, name: None, net: None }, Object::Keepout(k)));
    footprints.chain(tracks).chain(vias).chain(holes).chain(graphics).chain(pours).chain(keepouts).collect()
}

/// What a call did to the board, from the board before it (`old`) to the one stored now.
pub(super) fn board_result(s: &Session, old: &Board) -> BoardResult {
    board_result_nets(s, old).0
}

/// As [`board_result`], with the board's connections it worked out.
pub(super) fn board_result_nets(s: &Session, old: &Board) -> (BoardResult, Arc<BoardNets>) {
    let nets = s.board_nets();
    let new = &s.doc.ecad.board;
    let before: BTreeMap<u64, Object> = objects(s, old, None).into_iter().map(|(item, o)| (item.id, o)).collect();
    let after = objects(s, new, Some(&nets));
    let ids: BTreeSet<u64> = after.iter().map(|(item, _)| item.id).collect();
    let (mut created, mut changed) = (Vec::new(), Vec::new());
    let mut shown = BTreeSet::new();
    for (item, o) in after {
        match before.get(&item.id) {
            None => {
                shown.insert(item.id);
                created.push(item);
            }
            Some(was) if *was != o => {
                shown.insert(item.id);
                changed.push(item);
            }
            Some(_) => {}
        }
    }
    let removed = objects(s, old, None).into_iter().filter(|(item, _)| !ids.contains(&item.id)).map(|(item, _)| item).collect();
    let footprints = new.footprints.iter().filter(|f| shown.contains(&f.id)).map(|f| footprint_out(s, f, &nets)).collect();
    let unrouted = nets.airwires.len();
    (BoardResult { created, changed, removed, footprints, unrouted, param_warnings: Vec::new() }, nets)
}
