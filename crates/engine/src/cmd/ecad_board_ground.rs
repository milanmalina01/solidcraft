//! Ground connectivity (see `solidcraft_doc::ecad::ground`): `ecad.board.ground.inspect` solves
//! how well a ground net's copper is tied to its main ground and gives the worst spots, each
//! layer's shares and a coarse map. Read-only.

use serde_json::Value;
use solidcraft_doc::ecad::ground::{self, DEFAULT_TARGET_MV, Model, Reference};

use super::CommandSpec;
use super::ecad::{parse, reply};
use crate::params::bad;
use crate::schema::ecad_board_ground::{GroundInspectParams, GroundInspectResult, GroundLayerOut, GroundReferenceOut, GroundSpotOut};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new(
    "ecad.board.ground.inspect",
    "Ground Connectivity",
    crate::checked!("ecad.board.ground.inspect", ground_inspect),
)
.icon("inspect")
.noundo()
.doc_only()
.contract(&CONTRACT)
.output(output_schema_for::<GroundInspectResult>)
.params(
    "net? (the intent's ground, else GND); layers? [layer] (only these in the reply); cell? (0.1–0.25 mm, 0.25 by default; wider on a board too large, coarse says so); target? (mV, 0.5); spots? (8, at most 64); map? (cells each way, 32, 4–128; 0 leaves it out) → {net, reference: {kind: entry|plane, part?, pour?, layer?, area_mm2?}, cell, coarse?, origin, map_cell, target_mv, worst: [{at, layer, mv}], worst_copper?, layers: [{layer, copper_mm2, tied_pct, isolated_pct, weak_pct, map?: rows from the bottom up, null no copper, -1 isolated}], converged, iterations, unknowns}: each copper layer on a grid, the net's copper (fills, tracks, pads, via rings) a resistive sheet of its stack-up copper, its vias and through-hole pads plated barrels (25 µm) between layers; every mm² of the net's copper draws 10 mA into the main ground (the power entry connector's ground pads when the intent keeps one, else the largest shape of the net's fills), and each cell's rise in mV says how weakly it is tied. worst lists the planes' (fills') spots; copper joined to the main ground by nothing is isolated. Solved by conjugate gradients, at most 4000 steps (converged false: cut short). ecad.board.stitch pattern needed places vias against it, and pi.stitching checks it. Read-only",
)];

const CONTRACT: TypedContract<GroundInspectParams> = TypedContract::new();

fn round3(v: f64) -> f64 {
    if v.is_finite() { (v * 1000.0).round() / 1000.0 + 0.0 } else { 0.0 }
}

fn ground_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.ground.inspect";
    let p: GroundInspectParams = parse(CMD, p)?;
    let target = p.target.unwrap_or(DEFAULT_TARGET_MV);
    if !(target.is_finite() && target > 0.0 && target <= 1000.0) {
        return Err(bad(CMD, "`target` must be more than 0 and at most 1000 mV"));
    }
    let spots = p.spots.unwrap_or(8);
    if spots > 64 {
        return Err(bad(CMD, "`spots` must be at most 64"));
    }
    let most = p.map.unwrap_or(32);
    if most != 0 && !(4..=128).contains(&most) {
        return Err(bad(CMD, "`map` must be 0, or 4 to 128 cells"));
    }
    let board = &s.doc.ecad.board;
    let layers: Vec<_> = p.layers.unwrap_or_default().into_iter().map(|l| l.0).collect();
    for l in &layers {
        if !board.has_layer(*l) {
            return Err(bad(CMD, format!("`layers`: the board has no {} layer", l.name())));
        }
    }
    let nets = s.board_nets();
    let mut model = Model::build(&s.doc.ecad, &nets, &ground::Ask { net: p.net.clone(), cell: p.cell, max_cells: None }).map_err(|e| bad(CMD, e))?;
    let map = model.solve();
    let spot = |x: &ground::Spot| GroundSpotOut { at: [round3(x.at[0]), round3(x.at[1])], layer: x.layer.name(), mv: round3(x.mv) };
    let worst: Vec<GroundSpotOut> = map.spots(&layers, true, spots, 1.0).iter().map(spot).collect();
    let any = map.spots(&layers, false, 1, 0.0);
    let worst_copper = any.first().filter(|w| worst.first().is_none_or(|p| w.mv > p.mv + 1e-9)).map(spot);
    let cover = map.coverage(&layers, target);
    let mut map_cell = map.cell;
    let mut out_layers = Vec::new();
    for c in &cover {
        let rows = if most == 0 {
            None
        } else {
            let l = map.layers.iter().position(|x| *x == c.layer).unwrap_or(usize::MAX);
            let (side, rows) = map.coarse_map(l, most);
            map_cell = side;
            Some(rows.into_iter().map(|r| r.into_iter().map(|v| v.map(|v| if v.is_finite() { round3(v) } else { -1.0 })).collect()).collect())
        };
        out_layers.push(GroundLayerOut {
            layer: c.layer.name(),
            copper_mm2: round3(c.copper_mm2),
            tied_pct: round3(c.tied * 100.0),
            isolated_pct: round3(c.isolated * 100.0),
            weak_pct: round3(c.weak * 100.0),
            map: rows,
        });
    }
    let reference = match &map.reference {
        Reference::Entry(d) => GroundReferenceOut { kind: "entry".into(), part: Some(d.clone()), pour: None, layer: None, area_mm2: None },
        Reference::Plane { pour, layer, area } => {
            GroundReferenceOut { kind: "plane".into(), part: None, pour: Some(*pour), layer: Some(layer.name()), area_mm2: Some(round3(*area)) }
        }
    };
    reply(GroundInspectResult {
        net: map.net_name.clone(),
        reference,
        cell: map.cell,
        coarse: map.coarse,
        origin: [round3(map.origin[0]), round3(map.origin[1])],
        map_cell,
        target_mv: target,
        worst,
        worst_copper,
        layers: out_layers,
        converged: map.converged,
        iterations: map.iterations,
        unknowns: map.unknowns,
    })
}
