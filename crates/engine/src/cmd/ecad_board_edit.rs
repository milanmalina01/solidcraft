//! The board's layer stack, mounting holes, the footprints' name and value texts, and track
//! rework: a track reshaped, split at a point or mitered at its corners, and routed copper taken
//! away (unrouted) so its airwires come back. Each call is one undo step and is refused whole, as
//! in `ecad_board.rs`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use solidcraft_doc::ecad::airwires::footprint_pads;
use solidcraft_doc::ecad::board::{DEFAULT_HOLE_DRILL, MAX_HOLES, THICKNESS, check_hole};
use solidcraft_doc::ecad::labels::{NamesMode, layout};
use solidcraft_doc::ecad::layers::{MAX_COPPER_LAYERS, MIN_COPPER_LAYERS};
use solidcraft_doc::ecad::names::NAME_HEIGHT;
use solidcraft_doc::ecad::place::snap;
use solidcraft_doc::ecad::rework::{MiterStyle as Style, miter_keeps};
use solidcraft_doc::ecad::stackup::{THICKNESS_SLACK, check_stackup, total};
use solidcraft_doc::ecad::{Hole, StackKind, StackLayer};

use super::CommandSpec;
use super::ecad::{closest, parse, reply};
use super::ecad_board::{Index, MAX_ITEMS, board_point, board_result, refuse_clashes, stack_out, stackup_out, store_board, track_geometry};
use crate::params::bad;
use crate::schema::ecad_board::BoardResult;
use crate::schema::ecad_board_edit::{
    EditParams, HoleEditParams, HoleParams, LabelsParams, LabelsResult, MiterParams, MiterResult, MiterStyle, NamesModeParam, NamesParams,
    ReshapeParams, SplitParams, SplitResult, StackKindParam, StackParams, StackResult, ThicknessFrom, UnrouteParams,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.board.stack", "Layer Stack", crate::checked!("ecad.board.stack", stack))
        .icon("list")
        .doc_only()
        .contract(&STACK_CONTRACT)
        .output(output_schema_for::<StackResult>)
        .params(
            "copper_layers? (even, 2 to 16); thickness? (mm); stackup? [{kind: copper|core|prepreg|mask, thickness, er?, loss_tangent?}] (top first: optional mask, then copper and dielectrics in turn, one copper per layer; null: back to the default) → {copper_layers, thickness, stack: [{name, kind: outer|inner}], stackup, stackup_thickness, custom_stackup, thickness_from?, stackup_reset?, dielectrics_scaled?}: the copper layers are top, inner1 …, bottom; fewer layers are refused while tracks, drawings, pours, keepouts or via spans are on a layer that would go. A stack-up given with a thickness must be within 10 % of it; given without one, the board takes the stack-up's total. A new thickness alone scales a stack-up's dielectrics; new copper layers alone put the default stack-up back (stackup_reset). The default: 0.035 mm outer copper, 0.0175 mm inner, the rest shared by the dielectrics (εr 4.3)",
        ),
    CommandSpec::new("ecad.board.hole", "Hole", crate::checked!("ecad.board.hole", hole))
        .icon("hole")
        .doc_only()
        .contract(&HOLE_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params("at: [x, y]; drill? (mm; 3.2 by default) → {created: [{id, kind: hole}]}: an unplated mounting hole, no copper"),
    CommandSpec::new("ecad.board.hole.edit", "Edit Hole", crate::checked!("ecad.board.hole.edit", hole_edit))
        .icon("hole")
        .doc_only()
        .contract(&HOLE_EDIT_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params("holes: [id] (all or none); drill (mm) → {changed}"),
    CommandSpec::new("ecad.board.edit", "Edit Footprint Text", crate::checked!("ecad.board.edit", edit))
        .icon("edit")
        .doc_only()
        .contract(&EDIT_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "footprints: [{footprint: designator|id, name_at? [dx, dy]|null, value_at? [dx, dy]|null (from the footprint's origin along the board's axes; null for its own spot), show_name?, show_value?}] (all or none) → {changed, footprints}. Names show and values are hidden until shown",
        ),
    CommandSpec::new("ecad.board.names", "Names", crate::checked!("ecad.board.names", names))
        .icon("text")
        .doc_only()
        .contract(&NAMES_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "height? (mm, 0.1 to 10; null: each footprint's sized to its courtyard, 0.5 to 1 mm, or as its library footprint's spot says); show_names? bool (every footprint's name shown or hidden); show_values? bool (every value) → {changed, footprints}: the board's name and value texts in one step; a footprint's own spot for them stays",
        ),
    CommandSpec::new("ecad.board.labels", "Silkscreen Names", crate::checked!("ecad.board.labels", labels))
        .icon("text")
        .doc_only()
        .contract(&LABELS_CONTRACT)
        .output(output_schema_for::<LabelsResult>)
        .params(
            "mode: full|fit|minimal|none; height? (mm, at most 5; at least the least the fab prints legibly: silk.size's height and the height whose stroke, a tenth of it, the profile's least stroke is; that when left out); dry_run? → {mode, height, placed, moved_to_assembly, unplaceable, kept, why, changed, footprints}: every footprint's name (designator) laid out for the board at once: just outside its courtyard, upright (0°), off pads, open vias, holes, other names, courtyards and the board's edge band, nearer its own part than any other; full = every name (those with no legal spot stay at their usual spot, unplaceable), fit = those that fit (the rest hidden, left to the assembly drawing, which prints every name), minimal = connectors, test points, switches and LEDs only, none = none. The silk checks (silk.size, silk.clip, silk.designator) then run on the result and a name they still flag is hidden (fit, minimal) or counted (full). A name moved or shown by hand (ecad.board.edit) is never moved; values stay as they are. One undo step; the PDF's board pages print the same names",
        ),
    CommandSpec::new("ecad.board.track.reshape", "Reshape Track", crate::checked!("ecad.board.track.reshape", reshape))
        .icon("trace")
        .doc_only()
        .contract(&RESHAPE_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params("track: id; points: [[x, y]…] (2 or more); curves? [sweep…] (degrees per segment, 0 straight); check? (as ecad.board.move's) → {changed, unrouted}: the track's new path, its layer and width kept"),
    CommandSpec::new("ecad.board.track.split", "Split Track", crate::checked!("ecad.board.track.split", split))
        .icon("sketch_break")
        .doc_only()
        .contract(&SPLIT_CONTRACT)
        .output(output_schema_for::<SplitResult>)
        .params("track: id; segment (0 is from the first point to the second); at: [x, y] (taken to the nearest point of the segment) → {corner, at, changed, unrouted}: a new corner there (an arc splits into two arcs)"),
    CommandSpec::new("ecad.board.track.miter", "Miter", crate::checked!("ecad.board.track.miter", miter))
        .icon("sketch_fillet")
        .doc_only()
        .contract(&MITER_CONTRACT)
        .output(output_schema_for::<MiterResult>)
        .params(
            "track: id; corner? (a point's index, 1 to the point count − 2; every corner when left out); radius (mm); style? round|straight (round: an arc tangent to both segments; straight: a chamfer between the same touch points) → {mitered, changed, unrouted}: each cut shortened to what its segments allow; corners next to an arc, nearly straight or turning back are left, and so are corners where other copper joins the track (another track's point or line, a via, a pad), so no connection is lost",
        ),
    CommandSpec::new("ecad.board.unroute", "Unroute", crate::checked!("ecad.board.unroute", unroute))
        .icon("trim")
        .doc_only()
        .contract(&UNROUTE_CONTRACT)
        .output(output_schema_for::<BoardResult>)
        .params(
            "items?: [track or via id] (each with its routed run: the tracks and vias joined to it through tracks and vias, not through a pad); nets?: [name] (all their tracks and vias); all? (every track and via) → {removed, unrouted}: the airwires come back. Stitching vias (role stitch) stay unless named in items: ecad.board.stitch.remove takes them away",
        ),
];

const STACK_CONTRACT: TypedContract<StackParams> = TypedContract::new();
const HOLE_CONTRACT: TypedContract<HoleParams> = TypedContract::new();
const HOLE_EDIT_CONTRACT: TypedContract<HoleEditParams> = TypedContract::new();
const EDIT_CONTRACT: TypedContract<EditParams> = TypedContract::new();
const LABELS_CONTRACT: TypedContract<LabelsParams> = TypedContract::new();
const NAMES_CONTRACT: TypedContract<NamesParams> = TypedContract::new();
const RESHAPE_CONTRACT: TypedContract<ReshapeParams> = TypedContract::new();
const SPLIT_CONTRACT: TypedContract<SplitParams> = TypedContract::new();
const MITER_CONTRACT: TypedContract<MiterParams> = TypedContract::new();
const UNROUTE_CONTRACT: TypedContract<UnrouteParams> = TypedContract::new();

/// The most objects an error lists by id.
const MAX_LISTED: usize = 8;
/// The largest miter radius, in millimetres.
const MAX_MITER: f64 = 100.0;

/// `ids` for an error: the first few, then how many more.
fn listed(ids: &[u64]) -> String {
    let mut out = ids.iter().take(MAX_LISTED).map(u64::to_string).collect::<Vec<_>>().join(", ");
    if ids.len() > MAX_LISTED {
        out.push_str(&format!(" and {} more", ids.len() - MAX_LISTED));
    }
    out
}

/// Why a stack of `n` layers is refused while the objects `ids` (tracks or drawings) are on the
/// layers it would lose.
fn on_gone_layers(what: &str, ids: &[u64], n: u8) -> String {
    match ids {
        [id] => format!("{what} {id} is on an inner layer a {n}-layer stack doesn't have: move it to another layer or delete it first"),
        _ => format!("{what}s {} are on inner layers a {n}-layer stack doesn't have: move them to another layer or delete them first", listed(ids)),
    }
}

/// The most layers a stack-up given to a call has.
const MAX_STACKUP: usize = 64;

fn check_thickness(t: f64, what: &str) -> Result<()> {
    if !THICKNESS.contains(&t) {
        return Err(bad("ecad.board.stack", format!("{what} must be {} to {} mm", THICKNESS.start(), THICKNESS.end())));
    }
    Ok(())
}

fn stack(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.stack";
    let p: StackParams = parse(CMD, p)?;
    if p.copper_layers.is_none() && p.thickness.is_none() && p.stackup.is_none() {
        return Err(bad(CMD, "give `copper_layers`, `thickness` or `stackup` to change"));
    }
    let mut board = s.doc.ecad.board.clone();
    let (mut stackup_reset, mut dielectrics_scaled, mut thickness_from) = (false, false, None);
    if let Some(n) = p.copper_layers {
        let n = u8::try_from(n)
            .ok()
            .filter(|n| n.is_multiple_of(2) && (MIN_COPPER_LAYERS..=MAX_COPPER_LAYERS).contains(n))
            .ok_or_else(|| bad(CMD, format!("`copper_layers` must be an even number, {MIN_COPPER_LAYERS} to {MAX_COPPER_LAYERS}")))?;
        let tracks: Vec<u64> = board.tracks.iter().filter(|t| !t.layer.exists_in(n)).map(|t| t.id).collect();
        if !tracks.is_empty() {
            return Err(bad(CMD, on_gone_layers("track", &tracks, n)));
        }
        let drawings: Vec<u64> = board.graphics.iter().filter(|g| !g.layer.exists_in(n)).map(|g| g.id).collect();
        if !drawings.is_empty() {
            return Err(bad(CMD, on_gone_layers("drawing", &drawings, n)));
        }
        let vias: Vec<u64> = board.vias.iter().filter(|v| !(v.from.exists_in(n) && v.to.exists_in(n))).map(|v| v.id).collect();
        if !vias.is_empty() {
            return Err(bad(CMD, on_gone_layers("via", &vias, n).replace(" is on ", " spans ").replace(" are on ", " span ")));
        }
        let pours: Vec<u64> = board.pours.iter().filter(|x| !x.layer.exists_in(n)).map(|x| x.id).collect();
        if !pours.is_empty() {
            return Err(bad(CMD, on_gone_layers("pour", &pours, n)));
        }
        let keepouts: Vec<u64> = board.keepouts.iter().filter(|k| k.layers.iter().any(|l| !l.exists_in(n))).map(|k| k.id).collect();
        if !keepouts.is_empty() {
            return Err(bad(CMD, on_gone_layers("keepout", &keepouts, n)));
        }
        if n != board.copper_layers && !board.stackup.is_empty() && p.stackup.is_none() {
            board.stackup.clear();
            stackup_reset = true;
        }
        board.copper_layers = n;
    }
    match p.stackup {
        Some(Some(layers)) => {
            if layers.len() > MAX_STACKUP {
                return Err(bad(CMD, format!("a stack-up has at most {MAX_STACKUP} layers")));
            }
            let list: Vec<StackLayer> = layers
                .iter()
                .map(|l| StackLayer {
                    kind: match l.kind {
                        StackKindParam::Copper => StackKind::Copper,
                        StackKindParam::Core => StackKind::Core,
                        StackKindParam::Prepreg => StackKind::Prepreg,
                        StackKindParam::Mask => StackKind::Mask,
                    },
                    thickness: l.thickness,
                    er: l.er,
                    loss_tangent: l.loss_tangent,
                })
                .collect();
            check_stackup(&list, board.copper_layers).map_err(|e| bad(CMD, e))?;
            let sum = total(&list);
            match p.thickness {
                Some(t) => {
                    check_thickness(t, "`thickness`")?;
                    if (sum - t).abs() > THICKNESS_SLACK * t + 1e-9 {
                        return Err(bad(
                            CMD,
                            format!(
                                "the stack-up is {} mm thick, more than 10 % off the `thickness` {t} mm: give a thickness within 10 % of it, or leave `thickness` out and the board takes the stack-up's",
                                snap(sum)
                            ),
                        ));
                    }
                    board.thickness = t;
                    thickness_from = Some(ThicknessFrom::Given);
                }
                None => {
                    check_thickness(snap(sum), "the stack-up's total thickness")?;
                    board.thickness = snap(sum);
                    thickness_from = Some(ThicknessFrom::Stackup);
                }
            }
            board.stackup = list;
        }
        Some(None) => {
            board.stackup.clear();
            if let Some(t) = p.thickness {
                check_thickness(t, "`thickness`")?;
                board.thickness = t;
            }
        }
        None => {
            if let Some(t) = p.thickness {
                check_thickness(t, "`thickness`")?;
                dielectrics_scaled = !board.stackup.is_empty() && t != board.thickness;
                board.set_thickness(t).map_err(|e| bad(CMD, e))?;
            }
        }
    }
    store_board(s, board, CMD)?;
    let board = &s.doc.ecad.board;
    let stackup = board.stackup_or_default();
    reply(StackResult {
        copper_layers: board.copper_layers,
        thickness: board.thickness,
        stack: stack_out(board),
        stackup_thickness: snap(total(&stackup)),
        stackup: stackup_out(&stackup),
        custom_stackup: !board.stackup.is_empty(),
        thickness_from,
        stackup_reset,
        dielectrics_scaled,
        param_warnings: Vec::new(),
    })
}

fn hole(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.hole";
    let p: HoleParams = parse(CMD, p)?;
    let old = s.doc.ecad.board.clone();
    let at = board_point(p.at, CMD, "`at`")?;
    let drill = p.drill.unwrap_or(DEFAULT_HOLE_DRILL);
    check_hole(drill).map_err(|e| bad(CMD, e))?;
    let mut board = old.clone();
    let id = board.take_id().map_err(|e| bad(CMD, e))?;
    board.holes.push(Hole { id, at, drill });
    store_board(s, board, CMD)?;
    reply(board_result(s, &old))
}

fn hole_edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.hole.edit";
    let p: HoleEditParams = parse(CMD, p)?;
    if p.holes.is_empty() || p.holes.len() > MAX_HOLES {
        return Err(bad(CMD, format!("name 1 to {MAX_HOLES} holes in `holes`, by id")));
    }
    check_hole(p.drill).map_err(|e| bad(CMD, e))?;
    let old = s.doc.ecad.board.clone();
    let mut board = old.clone();
    let at: BTreeMap<u64, usize> = old.holes.iter().enumerate().map(|(i, h)| (h.id, i)).collect();
    for (i, id) in p.holes.iter().enumerate() {
        let Some(h) = at.get(id).and_then(|k| board.holes.get_mut(*k)) else {
            return Err(bad(CMD, format!("holes[{i}]: the board has no hole with the id {id}")));
        };
        h.drill = p.drill;
    }
    store_board(s, board, CMD)?;
    reply(board_result(s, &old))
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.edit";
    let p: EditParams = parse(CMD, p)?;
    if p.footprints.is_empty() || p.footprints.len() > MAX_ITEMS {
        return Err(bad(CMD, "name 1 or more footprints in `footprints`"));
    }
    let old = s.doc.ecad.board.clone();
    let index = Index::new(s);
    let mut board = old.clone();
    let at: BTreeMap<u64, usize> = old.footprints.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
    for (i, e) in p.footprints.iter().enumerate() {
        let what = format!("footprints[{i}]");
        let id = index.find(s, &e.footprint, CMD, &what)?;
        let Some(k) = at.get(&id).copied() else {
            return Err(bad(CMD, format!("{what}: board object {id} is not a footprint")));
        };
        let name_at = match e.name_at {
            Some(Some(v)) => Some(Some(board_point(v, CMD, &format!("{what}.name_at"))?)),
            other => other,
        };
        let value_at = match e.value_at {
            Some(Some(v)) => Some(Some(board_point(v, CMD, &format!("{what}.value_at"))?)),
            other => other,
        };
        let Some(f) = board.footprints.get_mut(k) else { continue };
        // A name moved or shown by hand is the user's: the names mode leaves it from now on.
        if let Some(v) = name_at {
            f.name_at = v;
            f.name_placed = false;
        }
        if let Some(v) = value_at {
            f.value_at = v;
        }
        if let Some(v) = e.show_name {
            f.show_name = v;
            f.name_placed = false;
        }
        if let Some(v) = e.show_value {
            f.show_value = v;
        }
    }
    store_board(s, board, CMD)?;
    reply(board_result(s, &old))
}

fn labels(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.labels";
    let p: LabelsParams = parse(CMD, p)?;
    let mode = match p.mode {
        NamesModeParam::Full => NamesMode::Full,
        NamesModeParam::Fit => NamesMode::Fit,
        NamesModeParam::Minimal => NamesMode::Minimal,
        NamesModeParam::None => NamesMode::None,
    };
    let old = s.doc.ecad.board.clone();
    let l = layout(&s.doc.ecad, mode, p.height).map_err(|e| bad(CMD, e))?;
    let out = |board| LabelsResult {
        mode: p.mode,
        height: l.height,
        placed: l.placed.clone(),
        moved_to_assembly: l.moved_to_assembly.clone(),
        unplaceable: l.unplaceable.clone(),
        kept: l.kept.clone(),
        why: l.why.clone(),
        board,
    };
    if p.dry_run.unwrap_or(false) {
        let mut dry = board_result(s, &old);
        dry.changed = Vec::new();
        return reply(out(dry));
    }
    store_board(s, l.board.clone(), CMD)?;
    reply(out(board_result(s, &old)))
}

fn names(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.names";
    let p: NamesParams = parse(CMD, p)?;
    if p.height.is_none() && p.show_names.is_none() && p.show_values.is_none() {
        return Err(bad(CMD, "give `height`, `show_names` or `show_values` to change"));
    }
    if let Some(Some(h)) = p.height
        && !NAME_HEIGHT.contains(&h)
    {
        return Err(bad(CMD, format!("`height` must be {} to {} mm", NAME_HEIGHT.start(), NAME_HEIGHT.end())));
    }
    let old = s.doc.ecad.board.clone();
    let mut board = old.clone();
    if let Some(h) = p.height {
        board.name_height = h.map(snap);
    }
    for f in &mut board.footprints {
        if let Some(v) = p.show_names {
            f.show_name = v;
        }
        if let Some(v) = p.show_values {
            f.show_value = v;
        }
    }
    store_board(s, board, CMD)?;
    reply(board_result(s, &old))
}

/// The index of the track `id` in the board, or why not.
fn track_index(s: &Session, id: u64, cmd: &str) -> Result<usize> {
    s.doc.ecad.board.tracks.iter().position(|t| t.id == id).ok_or_else(|| bad(cmd, format!("the board has no track with the id {id}")))
}

fn reshape(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.track.reshape";
    let p: ReshapeParams = parse(CMD, p)?;
    let i = track_index(s, p.track, CMD)?;
    let (points, curves) = track_geometry(&p.points, p.curves, CMD)?;
    let old = s.doc.ecad.board.clone();
    let mut board = old.clone();
    if let Some(t) = board.tracks.get_mut(i) {
        t.points = points;
        t.curves = curves;
    }
    if p.check.unwrap_or(false) {
        refuse_clashes(s, &old, &board, &BTreeSet::from([p.track]), CMD)?;
    }
    store_board(s, board, CMD)?;
    reply(board_result(s, &old))
}

fn split(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.track.split";
    let p: SplitParams = parse(CMD, p)?;
    let i = track_index(s, p.track, CMD)?;
    let at = board_point(p.at, CMD, "`at`")?;
    let old = s.doc.ecad.board.clone();
    let mut board = old.clone();
    let Some(t) = board.tracks.get_mut(i) else { return Err(bad(CMD, format!("the board has no track with the id {}", p.track))) };
    let corner = t.split(p.segment, at).map_err(|e| bad(CMD, format!("track {}: {e}", p.track)))?;
    let at = t.points.get(corner).copied().unwrap_or(at);
    store_board(s, board, CMD)?;
    reply(SplitResult { corner, at, board: board_result(s, &old) })
}

fn miter(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.track.miter";
    let p: MiterParams = parse(CMD, p)?;
    let i = track_index(s, p.track, CMD)?;
    if !(p.radius.is_finite() && p.radius > 0.0 && p.radius <= MAX_MITER) {
        return Err(bad(CMD, format!("`radius` must be more than 0 and at most {MAX_MITER} mm")));
    }
    let ecad = &s.doc.ecad;
    let old = ecad.board.clone();
    let Some(mut t) = old.tracks.get(i).cloned() else { return Err(bad(CMD, format!("the board has no track with the id {}", p.track))) };
    if let Some(c) = p.corner
        && !(c >= 1 && c < t.points.len().saturating_sub(1))
    {
        return Err(bad(CMD, format!("track {} has corners 1 to {}, not {c}", p.track, t.points.len().saturating_sub(2))));
    }
    let style = match p.style.unwrap_or(MiterStyle::Round) {
        MiterStyle::Round => Style::Round,
        MiterStyle::Straight => Style::Straight,
    };
    // Corners where other copper joins the track stay sharp, so no join is lost.
    let pads: Vec<_> = old.footprints.iter().flat_map(|f| footprint_pads(ecad, f, None)).collect();
    let original = t.clone();
    let mitered = t.miter(p.corner, p.radius, style, miter_keeps(&old, &pads, &original));
    let mut board = old.clone();
    if let Some(slot) = board.tracks.get_mut(i) {
        *slot = t;
    }
    store_board(s, board, CMD)?;
    reply(MiterResult { mitered, board: board_result(s, &old) })
}

fn unroute(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.unroute";
    let p: UnrouteParams = parse(CMD, p)?;
    let all = p.all.unwrap_or(false);
    let ecad = &s.doc.ecad;
    let old = ecad.board.clone();
    let items = p.items.unwrap_or_default();
    let names = p.nets.unwrap_or_default();
    if items.is_empty() && names.is_empty() && !all {
        return Err(bad(CMD, "give the tracks and vias in `items`, the `nets`, or `all`"));
    }
    if items.len() > MAX_ITEMS || names.len() > MAX_ITEMS {
        return Err(bad(CMD, format!("name at most {MAX_ITEMS} items or nets")));
    }
    let mut gone = BTreeSet::new();
    if all {
        gone.extend(old.tracks.iter().map(|t| t.id).chain(old.vias.iter().filter(|v| v.role.is_plain()).map(|v| v.id)));
    }
    if !items.is_empty() {
        let runs = ecad.board_runs(&old);
        // Each track's and via's run, by id.
        let run_of: BTreeMap<u64, usize> =
            old.tracks.iter().zip(&runs.tracks).map(|(t, r)| (t.id, *r)).chain(old.vias.iter().zip(&runs.vias).map(|(v, r)| (v.id, *r))).collect();
        let index = Index::new(s);
        let mut picked = BTreeSet::new();
        for (i, id) in items.iter().enumerate() {
            match (run_of.get(id), index.kind(*id)) {
                (Some(run), _) => picked.insert(*run),
                (None, Some(_)) => return Err(bad(CMD, format!("items[{i}]: board object {id} is not a track or a via"))),
                (None, None) => return Err(bad(CMD, format!("items[{i}]: the board has nothing with the id {id}"))),
            };
        }
        gone.extend(old.tracks.iter().zip(&runs.tracks).filter(|(_, r)| picked.contains(*r)).map(|(t, _)| t.id));
        // Stitching vias go only when named.
        gone.extend(
            old.vias.iter().zip(&runs.vias).filter(|(v, r)| picked.contains(*r) && (v.role.is_plain() || items.contains(&v.id))).map(|(v, _)| v.id),
        );
    }
    if !names.is_empty() {
        let nets = s.board_nets();
        let mut wanted = BTreeSet::new();
        for (i, name) in names.iter().enumerate() {
            let Some(n) = nets.names.iter().position(|m| m == name) else {
                let mut msg = format!("nets[{i}]: the schematic has no net `{name}`");
                if let Some(near) = closest(name, nets.names.iter().map(String::as_str)) {
                    msg.push_str(&format!(" (did you mean `{near}`?)"));
                }
                return Err(bad(CMD, msg));
            };
            wanted.insert(n);
        }
        let on = |n: &Option<usize>| n.is_some_and(|n| wanted.contains(&n));
        gone.extend(old.tracks.iter().zip(&nets.track_nets).filter(|(_, n)| on(n)).map(|(t, _)| t.id));
        gone.extend(old.vias.iter().zip(&nets.via_nets).filter(|(v, n)| on(n) && v.role.is_plain()).map(|(v, _)| v.id));
    }
    let mut board = old.clone();
    board.remove(&gone);
    store_board(s, board, CMD)?;
    reply(board_result(s, &old))
}
