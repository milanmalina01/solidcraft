//! Stitching vias (see `solidcraft_doc::ecad::stitch`): a grid of vias of one net over an area, a
//! fence along both sides of tracks, or a via beside each signal via that changes reference
//! plane, each checked as a placed via is, all placed in one step and marked as stitching vias;
//! and those taken away again by net, area or id. As in `ecad_board.rs`, each call changes a copy
//! of the board and stores it whole once it passes the design file's checks, so a call is one
//! undo step and a refused one changes nothing.

use std::collections::BTreeSet;

use serde_json::Value;
use solidcraft_doc::ecad::board::{MAX_VIAS, check_via};
use solidcraft_doc::ecad::copper::inside;
use solidcraft_doc::ecad::drc::{least_via_diameter, via_size_error};
use solidcraft_doc::ecad::path;
use solidcraft_doc::ecad::stitch::{self, Area, Ask, MAX_NAMED, MAX_STITCH, Pattern, ViaSpec};
use solidcraft_doc::ecad::{Board, BoardNets, Via, ViaProtection, ViaRole};

use super::CommandSpec;
use super::ecad::{parse, reply};
use super::ecad_board::{Index, board_point, board_result, store_board, via_size, via_span};
use super::ecad_board_pour::known_net_in;
use crate::params::bad;
use crate::schema::ecad_board::BoardResult;
use crate::schema::ecad_board_stitch::{
    NeededOut, RegionArea, RegionParam, StitchParams, StitchPattern, StitchRemoveParams, StitchRemoveResult, StitchResult, StitchViaOut, TieOut,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.board.stitch", "Stitch Vias", crate::checked!("ecad.board.stitch", stitch_vias))
        .icon("pattern_rect")
        .doc_only()
        .contract(&STITCH_CONTRACT)
        .output(output_schema_for::<StitchResult>)
        .params(
            "net (GND as a rule); pattern: grid|fence|layer_change|needed; region? \"board\"|{pour: id (a pour of the net)}|{rect: [[x0, y0], [x1, y1]]}|{polygon: [[x, y]…]} (the whole board when left out); tracks? [id] and nets? [name] or class? (a fence's tracks: those, and every track of those nets; a layer change's signal nets; at most 4096 each); pitch? (mm between neighbours; grid: by default no via-free circle wider than λg/10 at the intent's highest frequency of concern or radio band, offsets included (λg/10 × 0.678), else 5; fence: λg/20 at the intent's f_concern, else at twice the nets' (else the board's highest band's) frequency, else 2; raised to the via + hole to hole when closer, and why says so); offset? (fence: mm from the centreline; by default half the width + the clearance between the nets + the via's radius where the net's copper runs beside the line, at least 4 dielectric heights on inner layers, at least 2 widths and 3 heights for a line with no ground beside it); via? {diameter?, drill?, from?, to?} (the net class's via, through, by default; the rules' micro via between neighbouring layers; a diameter left out grows to the least ring the fab profile passes); clearance? (mm kept to other nets' copper, never less than the rules'); protection? (tented by default); target? (needed: mV, 0.5 by default) budget? (needed: the most vias, 20 by default, at most 200); under_parts? (grid, fence and needed: true lets vias stand inside the courtyard of a footprint on a side the via shows on; left out, they keep out, skipped as under_part); dry_run? → {count, would_create?, skipped: {reason: n}, looked_at, moved? (grid vias placed up to 0.16 pitch off a spot that failed), pitch_used, offset_used?, unfenced? [track id], ground? (needed: {before, after: {worst_mv, at?, layer?, isolated_pct, weak_pct, converged}, target_mv, budget, reached, solves, cell, coarse?}), via, why, created, unrouted}: grid = staggered rows centred in the area, clear of its edge, none within 0.16 pitch of a via of the net that already joins the span; fence = both sides of each segment (arcs too): a via beside every bend, tee, pad of the line and open end (within 1 mm), then the runs between at no more than the pitch; layer_change = for each signal via whose layers reference different planes, one via of the net joining both within 2 mm (1 mm preferred) where none is, two on opposite sides within 1 mm (or λg/20) for an RF net (signal nets: those given, else the intent's RF/clock/fast nets, else all nets with no pour); needed = one via at a time at the most weakly tied legal spot of the net's planes (the ground connectivity solve of ecad.board.ground.inspect; spots near RF lines, switching regulators, layer changes and connectors weigh more), solved again after each, until the worst is within target or budget vias are placed. Each via passes the clearance to other nets on every layer of its span, hole to hole, copper to edge, keepouts, stays out of SMD pads, doesn't cut another net's plane in two, and lands on the net's copper (fill, track, pad, via) on two layers or more; the rest are skipped and counted. A via size whose drill, ring or aspect ratio the design rule check would call an error is refused. At most 2000 vias; one undo step; the vias are marked as stitching vias (role stitch)",
        ),
    CommandSpec::new("ecad.board.stitch.remove", "Remove Stitching", crate::checked!("ecad.board.stitch.remove", stitch_remove))
        .icon("delete")
        .doc_only()
        .contract(&REMOVE_CONTRACT)
        .output(output_schema_for::<StitchRemoveResult>)
        .params(
            "net? (only its stitching vias); region? \"board\"|{pour}|{rect}|{polygon} (only those in it; a pour the board has, with a fill); ids? [id] (only these, each a stitching via); parts? [designator|id] (only those under these footprints: the via's pad meets the courtyard, else the footprint's box, on a side the via shows on; at most 4096); pattern? grid|fence|layer_change|needed (only those its pattern placed); unrecorded? (with pattern: also those stitched before vias kept their pattern, which have none on record and may be of any) → {count, unrecorded? (such vias a pattern left), removed, unrouted}: the stitching vias (role stitch) that match all that is given, in one undo step; vias placed otherwise stay",
        ),
];

const STITCH_CONTRACT: TypedContract<StitchParams> = TypedContract::new();
const REMOVE_CONTRACT: TypedContract<StitchRemoveParams> = TypedContract::new();

/// The area `region` names, its points checked; why not when it names none or more than one.
fn area_of(region: &RegionParam, cmd: &str) -> Result<Area> {
    let RegionParam::Area(RegionArea { pour, rect, polygon }) = region else { return Ok(Area::Board) };
    match (pour, rect, polygon) {
        (Some(id), None, None) => Ok(Area::Pour(*id)),
        (None, Some([a, b]), None) => {
            let (a, b) = (board_point(*a, cmd, "`region.rect`")?, board_point(*b, cmd, "`region.rect`")?);
            let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
            if hi[0] - lo[0] <= 0.0 || hi[1] - lo[1] <= 0.0 {
                return Err(bad(cmd, "`region.rect` encloses no area"));
            }
            Ok(Area::Polygon(vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]))
        }
        (None, None, Some(points)) => {
            if points.len() < 3 || points.len() > solidcraft_doc::ecad::board::MAX_OUTLINE_POINTS {
                return Err(bad(cmd, format!("`region.polygon` needs 3 to {} corners", solidcraft_doc::ecad::board::MAX_OUTLINE_POINTS)));
            }
            let points =
                points.iter().enumerate().map(|(i, q)| board_point(*q, cmd, &format!("`region.polygon[{i}]`"))).collect::<Result<Vec<_>>>()?;
            Ok(Area::Polygon(points))
        }
        _ => Err(bad(cmd, "`region` takes one of `pour`, `rect` or `polygon` (or \"board\")")),
    }
}

/// Which of `points` are in `area` (a pour by its fill), or why not when it names a pour the
/// board doesn't have or that fills nothing.
fn holds(board: &Board, nets: &BoardNets, area: &Area, points: &[[f64; 2]], cmd: &str) -> Result<Vec<bool>> {
    let ring = match area {
        Area::Board if board.outline.is_empty() => return Ok(vec![true; points.len()]),
        Area::Board => path::tessellate(&board.outline, &board.outline_curves, true, 0.002),
        Area::Pour(id) => {
            if board.pour(*id).is_none() {
                return Err(bad(cmd, format!("`region`: the board has no pour with the id {id}")));
            }
            let Some(f) = nets.fills.iter().find(|f| f.pour == *id) else { return Err(bad(cmd, format!("`region`: pour {id} has no fill"))) };
            return Ok(f.region.locate(points).into_iter().map(|h| h.is_some()).collect());
        }
        Area::Polygon(ring) => ring.clone(),
    };
    Ok(points.iter().map(|p| inside(*p, &ring)).collect())
}

fn stitch_vias(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.stitch";
    let p: StitchParams = parse(CMD, p)?;
    // The schematic's nets, worked out once for every name checked.
    let known: Vec<String> = s.doc.ecad.connectivity().nets.into_iter().map(|n| n.name).collect();
    let net = known_net_in(&known, &p.net, CMD)?;
    if p.tracks.as_ref().is_some_and(|t| t.len() > MAX_NAMED) || p.nets.as_ref().is_some_and(|n| n.len() > MAX_NAMED) {
        return Err(bad(CMD, format!("name at most {MAX_NAMED} `tracks` and {MAX_NAMED} `nets`")));
    }
    let old = s.doc.ecad.board.clone();
    let v = p.via.unwrap_or_default();
    let span = via_span(&old, v.from.map(|l| l.0), v.to.map(|l| l.0), CMD, "`via`: ")?;
    let (diameter, drill) = via_size(&old, Some(&net), span, v.diameter, v.drill);
    check_via(diameter, drill).map_err(|e| bad(CMD, format!("`via`: a via's {e}")))?;
    // A size the design rule check would call an error is refused here, not placed a thousand
    // times: the drill, the ring and the aspect ratio as the check measures them.
    // A diameter left out grows to the least the ring passes with, and says so.
    let rules = &old.rules;
    let mut probe = Via {
        id: 0,
        at: [0.0, 0.0],
        diameter,
        drill,
        from: span.0,
        to: span.1,
        net: None,
        protection: Default::default(),
        role: ViaRole::Stitch,
        pattern: None,
    };
    let mut grown = None;
    if v.diameter.is_none() && via_size_error(&old, &probe).is_some() {
        let least = least_via_diameter(&old, &probe);
        if least > diameter {
            grown = Some(diameter);
            probe.diameter = least;
        }
    }
    if let Some(e) = via_size_error(&old, &probe) {
        return Err(bad(CMD, format!("`via`: {e}")));
    }
    let diameter = probe.diameter;
    let mut names: Vec<String> = Vec::new();
    for n in p.nets.unwrap_or_default() {
        names.push(known_net_in(&known, &n, CMD)?);
    }
    if let Some(c) = &p.class {
        let Some(class) = rules.class(c) else {
            let have: Vec<&str> = rules.net_classes.iter().map(|k| k.name.as_str()).collect();
            return Err(bad(
                CMD,
                format!("`class`: the rules have no net class `{c}` (they have: {})", if have.is_empty() { "none".into() } else { have.join(", ") }),
            ));
        };
        let known: BTreeSet<&String> = known.iter().collect();
        names.extend(class.nets.iter().filter(|n| known.contains(n)).cloned());
    }
    names.sort();
    names.dedup();
    let pattern = match p.pattern {
        StitchPattern::Grid => Pattern::Grid,
        StitchPattern::Fence => Pattern::Fence,
        StitchPattern::LayerChange => Pattern::LayerChange,
        StitchPattern::Needed => Pattern::Needed,
    };
    let under_parts = p.under_parts.unwrap_or(false);
    let area = p.region.as_ref().map(|r| area_of(r, CMD)).transpose()?;
    let ask = Ask {
        net: net.clone(),
        pattern,
        area,
        tracks: p.tracks.unwrap_or_default(),
        nets: names,
        pitch: p.pitch,
        offset: p.offset,
        via: ViaSpec { diameter, drill, from: span.0, to: span.1 },
        clearance: p.clearance,
        under_parts,
        target: p.target,
        budget: p.budget,
    };
    let nets = s.board_nets();
    let mut plan = stitch::plan(&s.doc.ecad, &nets, &ask).map_err(|e| bad(CMD, e))?;
    if p.protection.is_none() {
        plan.why.push("tented (solder mask over the holes), as stitching vias usually are: `protection` sets another".into());
    }
    if let Some(was) = grown {
        plan.why.insert(
            0,
            format!("vias {diameter} mm across, not the net class's {was} mm: the least whose ring passes round a {drill} mm drill as the fab profile measures it"),
        );
    }
    let count = plan.spots.len();
    if count > MAX_STITCH || old.vias.len().saturating_add(count) > MAX_VIAS {
        return Err(bad(CMD, format!("{count} vias is past what one call places ({MAX_STITCH}) or the board holds ({MAX_VIAS} vias)")));
    }
    let via = StitchViaOut { diameter, drill, from: span.0.name(), to: span.1.name() };
    let ground = plan.ground.as_ref().map(needed_out);
    let skipped = plan.skipped.iter().map(|(k, n)| (k.to_string(), *n)).collect();
    if p.dry_run.unwrap_or(false) {
        let board = BoardResult {
            created: Vec::new(),
            changed: Vec::new(),
            removed: Vec::new(),
            footprints: Vec::new(),
            unrouted: nets.airwires.len(),
            param_warnings: Vec::new(),
        };
        return reply(StitchResult {
            count,
            would_create: plan.spots,
            skipped,
            looked_at: plan.looked_at,
            moved: plan.moved,
            pitch_used: plan.pitch,
            offset_used: plan.offset,
            via,
            why: plan.why,
            unfenced: plan.unfenced,
            ground,
            board,
        });
    }
    let mut board = old.clone();
    let ids = board.free_ids(count).map_err(|e| bad(CMD, e))?;
    let protection = p.protection.map_or(ViaProtection::Tented, |x| x.doc());
    for (id, at) in ids.clone().zip(&plan.spots) {
        let (net, pattern) = (Some(net.clone()), Some(pattern));
        board.vias.push(Via { id, at: *at, diameter, drill, from: span.0, to: span.1, net, protection, role: ViaRole::Stitch, pattern });
    }
    board.next_id = board.next_id.max(ids.end);
    store_board(s, board, CMD)?;
    reply(StitchResult {
        count,
        would_create: Vec::new(),
        skipped,
        looked_at: plan.looked_at,
        moved: plan.moved,
        pitch_used: plan.pitch,
        offset_used: plan.offset,
        via,
        why: plan.why,
        unfenced: plan.unfenced,
        ground,
        board: board_result(s, &old),
    })
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0 + 0.0
}

/// A plan's tie as replies give it.
pub(super) fn tie_out(t: &stitch::Tie) -> TieOut {
    TieOut {
        worst_mv: t.worst.filter(|v| v.is_finite()).map(round3),
        at: t.at.map(|[x, y]| [round3(x), round3(y)]),
        layer: t.layer.map(|l| l.name()),
        isolated_pct: round3(t.isolated * 100.0),
        weak_pct: round3(t.weak * 100.0),
        converged: t.converged,
    }
}

fn needed_out(n: &stitch::Needed) -> NeededOut {
    NeededOut {
        before: tie_out(&n.before),
        after: tie_out(&n.after),
        target_mv: n.target,
        budget: n.budget,
        reached: n.reached,
        solves: n.solves,
        cell: n.cell,
        coarse: n.coarse,
    }
}

/// A stitching via's pattern as replies give it.
pub(super) fn pattern_out(p: Pattern) -> StitchPattern {
    match p {
        Pattern::Grid => StitchPattern::Grid,
        Pattern::Fence => StitchPattern::Fence,
        Pattern::LayerChange => StitchPattern::LayerChange,
        Pattern::Needed => StitchPattern::Needed,
    }
}

fn stitch_remove(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.stitch.remove";
    let p: StitchRemoveParams = parse(CMD, p)?;
    let net = match p.net.as_deref() {
        Some(n) => Some(known_net_in(&s.doc.ecad.connectivity().nets.into_iter().map(|n| n.name).collect::<Vec<_>>(), n, CMD)?),
        None => None,
    };
    let area = p.region.as_ref().map(|r| area_of(r, CMD)).transpose()?;
    let old = s.doc.ecad.board.clone();
    let ids: Option<BTreeSet<u64>> = match &p.ids {
        Some(list) => {
            if list.len() > MAX_VIAS {
                return Err(bad(CMD, format!("name at most {MAX_VIAS} vias in `ids`")));
            }
            for (i, id) in list.iter().enumerate() {
                match old.via(*id) {
                    Some(v) if v.role == ViaRole::Stitch => {}
                    Some(_) => return Err(bad(CMD, format!("ids[{i}]: via {id} is not a stitching via"))),
                    None => return Err(bad(CMD, format!("ids[{i}]: the board has no via with the id {id}"))),
                }
            }
            Some(list.iter().copied().collect())
        }
        None => None,
    };
    let under: Option<BTreeSet<u64>> = match &p.parts {
        Some(list) => {
            if list.is_empty() || list.len() > MAX_NAMED {
                return Err(bad(CMD, format!("name 1 to {MAX_NAMED} footprints in `parts`, by their part's designator or by id")));
            }
            let index = Index::new(s);
            let mut out = BTreeSet::new();
            for (i, r) in list.iter().enumerate() {
                let id = index.find(s, r, CMD, &format!("parts[{i}]"))?;
                if old.footprint(id).is_none() {
                    return Err(bad(CMD, format!("parts[{i}]: board object {id} is not a footprint")));
                }
                out.insert(id);
            }
            Some(out)
        }
        None => None,
    };
    let nets = s.board_nets();
    // A via with no pattern on record (stitched before vias kept theirs) may be of any: a pattern
    // takes it only when asked to, and says how many it left.
    let unrecorded = p.unrecorded.unwrap_or(false);
    let picked: Vec<&Via> = old
        .vias
        .iter()
        .filter(|v| v.role == ViaRole::Stitch)
        .filter(|v| net.as_ref().is_none_or(|n| v.net.as_ref() == Some(n)))
        .filter(|v| p.pattern.is_none_or(|k| v.pattern.is_none_or(|m| pattern_out(m) == k)))
        .filter(|v| ids.as_ref().is_none_or(|ids| ids.contains(&v.id)))
        .collect();
    let picked: Vec<&Via> = match &under {
        Some(parts) => {
            let beneath = stitch::under_parts(&s.doc.ecad, parts, &picked);
            picked.into_iter().zip(beneath).filter(|(_, b)| *b).map(|(v, _)| v).collect()
        }
        None => picked,
    };
    let held = match &area {
        Some(a) => holds(&old, &nets, a, &picked.iter().map(|v| v.at).collect::<Vec<_>>(), CMD)?,
        None => vec![true; picked.len()],
    };
    let taken = |v: &Via| p.pattern.is_none() || v.pattern.is_some() || unrecorded;
    let left = picked.iter().zip(&held).filter(|(v, h)| **h && !taken(v)).count();
    let gone: BTreeSet<u64> = picked.iter().zip(held).filter(|(v, h)| *h && taken(v)).map(|(v, _)| v.id).collect();
    let mut board = old.clone();
    board.remove(&gone);
    store_board(s, board, CMD)?;
    reply(StitchRemoveResult { count: gone.len(), unrecorded: left, board: board_result(s, &old) })
}

#[cfg(test)]
#[path = "ecad_board_stitch_tests.rs"]
mod tests;
