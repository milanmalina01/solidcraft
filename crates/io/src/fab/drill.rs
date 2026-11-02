//! Excellon drill files: the plated holes (vias and through-hole pads) and the unplated ones
//! (mounting holes and unplated pads), each its own file, metric with decimal points, a slot as
//! a routed `G85` between its ends. Tools are numbered by size, smallest first.

use std::fmt::Write as _;

use solidcraft_doc::ecad::Electronics;
use solidcraft_doc::ecad::airwires::BoardNets;
use solidcraft_doc::ecad::copper::drill_core;
use solidcraft_doc::ecad::footprint::PadKind;

use super::{FabFile, Options};

/// One hole: a drill at a point, or a slot between two.
#[derive(Clone, Debug, PartialEq)]
pub struct Hole {
    pub diameter: f64,
    pub from: [f64; 2],
    /// The slot's other end; `None` for a round hole.
    pub to: Option<[f64; 2]>,
}

/// The holes, plated then unplated.
pub fn holes(ecad: &Electronics, nets: &BoardNets) -> (Vec<Hole>, Vec<Hole>) {
    let b = &ecad.board;
    let mut plated: Vec<Hole> = b.vias.iter().map(|v| Hole { diameter: v.drill, from: v.at, to: None }).collect();
    let mut unplated: Vec<Hole> = b.holes.iter().map(|h| Hole { diameter: h.drill, from: h.at, to: None }).collect();
    for p in &nets.pads {
        let Some(core) = drill_core(p) else { continue };
        let diameter = 2.0 * core.radius;
        let hole = match core.points.as_slice() {
            [a, b] if (a[0] - b[0]).hypot(a[1] - b[1]) > 1e-6 => Hole { diameter, from: *a, to: Some(*b) },
            [a, ..] => Hole { diameter, from: *a, to: None },
            [] => continue,
        };
        if p.pad.kind == PadKind::Hole { unplated.push(hole) } else { plated.push(hole) }
    }
    (plated, unplated)
}

fn write(holes: &[Hole], plated: bool) -> String {
    let mut sizes: Vec<f64> = holes.iter().map(|h| (h.diameter * 1000.0).round() / 1000.0).collect();
    sizes.sort_by(f64::total_cmp);
    sizes.dedup();
    let mut out = String::new();
    out.push_str("M48\n");
    let _ = writeln!(out, "; #@! TF.GenerationSoftware,SolidCraft,SolidCraft,{}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "; #@! TF.FileFunction,{},1,2,{}", if plated { "Plated" } else { "NonPlated" }, if plated { "PTH" } else { "NPTH" });
    out.push_str("FMAT,2\nMETRIC\n");
    for (i, d) in sizes.iter().enumerate() {
        let _ = writeln!(out, "T{}C{d:.3}", i + 1);
    }
    out.push_str("%\nG90\nG05\n");
    for (i, d) in sizes.iter().enumerate() {
        let mine: Vec<&Hole> = holes.iter().filter(|h| ((h.diameter * 1000.0).round() / 1000.0 - d).abs() < 1e-9).collect();
        let _ = writeln!(out, "T{}", i + 1);
        for h in mine {
            match h.to {
                None => {
                    let _ = writeln!(out, "X{:.4}Y{:.4}", h.from[0], h.from[1]);
                }
                Some(to) => {
                    let _ = writeln!(out, "X{:.4}Y{:.4}G85X{:.4}Y{:.4}", h.from[0], h.from[1], to[0], to[1]);
                }
            }
        }
    }
    out.push_str("M30\n");
    out
}

/// The plated and the unplated drill file (the latter only when the board has such holes).
pub fn files(ecad: &Electronics, nets: &BoardNets, o: &Options) -> Vec<FabFile> {
    let (plated, unplated) = holes(ecad, nets);
    let mut out = vec![FabFile::text(format!("{}-PTH.drl", o.stem), "Plated,1,2,PTH".into(), write(&plated, true))];
    if !unplated.is_empty() {
        out.push(FabFile::text(format!("{}-NPTH.drl", o.stem), "NonPlated,1,2,NPTH".into(), write(&unplated, false)));
    }
    out
}
