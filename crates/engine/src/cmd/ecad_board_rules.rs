//! The board's design rules and net classes, read back with each class's nets, the impedance
//! calculator over the stack-up, and the current calculator (the track width a current needs).
//! Each change is one undo step, all or none, as in `ecad_board.rs`. The rules live on the
//! design's board record, which every electronics design has (empty until something is put on
//! it), so net classes can be made from the schematic before the board view was ever opened.

use std::collections::BTreeSet;

use serde_json::Value;
use solidcraft_doc::ecad::current::{self, AMBIENT, COPPER, CurrentError, DEFAULT_AMBIENT, DEFAULT_RISE, Fit};
use solidcraft_doc::ecad::impedance::{self, LineKind};
use solidcraft_doc::ecad::rules::{MAX_CLASS_NETS, check_class_name, check_net_name};
use solidcraft_doc::ecad::stackup::surroundings;
use solidcraft_doc::ecad::{Board, DiffPair, NetClass, Rules};

use super::CommandSpec;
use super::ecad::{closest, parse, reply};
use super::ecad_board::{no_layer, store_board};
use crate::params::bad;
use crate::schema::ecad_board_rules::{
    CopperParam, CurrentFormulaParam, CurrentLayerParam, CurrentParams, CurrentResult, DiffPairSpec, ImpedanceParams, ImpedanceResult, LineKindParam,
    NetClassDeleteParams, NetClassOut, NetClassParams, OutOfRangeOut, ProposedWidths, RulesInspectParams, RulesOut, RulesParams, RulesResult,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.board.rules", "Design Rules", crate::checked!("ecad.board.rules", rules))
        .icon("settings")
        .doc_only()
        .contract(&RULES_CONTRACT)
        .output(output_schema_for::<RulesResult>)
        .params(
            "Any of track_width, clearance, via_diameter, via_drill (the default class: what new tracks and vias get, the least copper gap), min_track_width, min_annular_ring, min_drill, hole_to_hole, copper_to_edge, silk_to_pad, microvia_diameter, microvia_drill (mm; what the design rule check holds the board to) → {rules, classes: [{name, track_width, clearance, via_diameter, via_drill, diff_pair?, nets, unknown_nets?}], default_nets}: all in one step; a value out of range is refused by name",
        ),
    CommandSpec::new("ecad.board.netclass", "Net Class", crate::checked!("ecad.board.netclass", netclass))
        .icon("list")
        .doc_only()
        .contract(&NETCLASS_CONTRACT)
        .output(output_schema_for::<RulesResult>)
        .params(
            "name (a new class is made when none has it, ignoring case); rename?; nets? [name] (in place of its nets) | add_nets? [name], remove_nets? [name]; track_width?, clearance?, via_diameter?, via_drill? (mm; a new class takes the default class's for what is left out); diff_pair? {width, gap}|null; current? (A), temp_rise? (°C, 10 when left out), copper_oz? (the stack-up's when left out), each |null: what the class's width is for → {rules, classes: [{…, current?, proposed?: {outer, inner, temp_rise, copper_outer, copper_inner, valid_range}}], default_nets, moved: [\"net: from\"]}: a net is in one class at most, so naming it here moves it from its old class; a net the schematic doesn't have yet is kept and listed in unknown_nets; a schematic edit that renames a listed net (Rename Net, a label, a part renumbered under an unnamed net's net:R1.2) moves the name in the class with it. Clearance between two nets is the larger of their classes'. track_width stays what routes and what the DRC holds the class to: a current only proposes a width (ecad.board.current.inspect). Works before the board has anything on it",
        ),
    CommandSpec::new("ecad.board.netclass.delete", "Delete Net Class", crate::checked!("ecad.board.netclass.delete", netclass_delete))
        .icon("delete")
        .doc_only()
        .contract(&NETCLASS_DELETE_CONTRACT)
        .output(output_schema_for::<RulesResult>)
        .params("name → {rules, classes, default_nets}: its nets go back to the default class"),
    CommandSpec::new("ecad.board.rules.inspect", "Inspect Design Rules", crate::checked!("ecad.board.rules.inspect", rules_inspect))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&RULES_INSPECT_CONTRACT)
        .output(output_schema_for::<RulesResult>)
        .params("No parameters → {rules, classes: [{name, …, nets, unknown_nets?}], default_nets}: the design rules, the net classes, and the schematic's nets in no class. Read-only"),
    CommandSpec::new("ecad.board.impedance.inspect", "Impedance", crate::checked!("ecad.board.impedance.inspect", impedance_inspect))
        .icon("measure")
        .noundo()
        .doc_only()
        .contract(&IMPEDANCE_CONTRACT)
        .output(output_schema_for::<ImpedanceResult>)
        .params(
            "layer; kind: microstrip (outer layer over its plane) | stripline (inner layer between planes) | coplanar (outer layer, ground beside it and under it) | differential (edge-coupled pair); width? (mm: its impedance) | target? (Ω: the width for it, by bisection); gap? (mm, for coplanar and differential) → {z0, width, gap?, kind, valid_range, er_eff?, formula, h, h2?, t, er}: closed-form estimates over the stack-up (IPC-2141 microstrip, stripline and edge-coupled pairs; Wadell's conductor-backed coplanar waveguide), solder mask not counted; valid_range is false outside the formula's range, and out_of_range? {input (w/h, er, w/(b-t), t/b or t/s), value, low?, high?} names the first input outside it (coplanar: copper thicker than a fifth of the gap, our own bound for a formula worked out for thin copper). Read-only",
        ),
    CommandSpec::new("ecad.board.current.inspect", "Current Capacity", crate::checked!("ecad.board.current.inspect", current_inspect))
        .icon("measure")
        .noundo()
        .doc_only()
        .contract(&CURRENT_CONTRACT)
        .output(output_schema_for::<CurrentResult>)
        .params(
            "current (A); temp_rise? (°C above ambient, 10 when left out: the width for it) | width? (mm: the rise it warms by); ambient? (°C, 25); copper? {oz}|{mm} (left out: the board's stack-up's copper, the thinnest outer and inner layer); layer? outer (left out) | inner | a copper layer of the board (its own copper from the stack-up unless copper is given); length? (mm); formula? ipc2221 (left out) | ipc2152 → {current, width_outer, width_inner, width (on the layer), temp_rise, ambient, temperature (ambient + rise), layer, copper, copper_oz, copper_outer, copper_inner, length?, resistance? (Ω at temperature), voltage_drop? (V), power? (W), formula, valid_range}: ipc2221 is IPC-2221's I = k·ΔT^0.44·A^0.725 (A in mil², k 0.048 outer, 0.024 inner), valid_range false outside its published range (up to 35 A, 0.5–3 oz, 10–100 °C rise, width up to 10 mm); ipc2152 is Brooks and Adam's fit to IPC-2152's external data, ΔT = 215.3·I²·W^-1.15·Th^-1 (W, Th in mil), for inner layers too (they cool about as well), valid_range false outside 1–3 oz, above a 100 °C rise, 35 A or a 10 mm width: what the quality check pi.current uses. Read-only",
        ),
];

const RULES_CONTRACT: TypedContract<RulesParams> = TypedContract::new();
const NETCLASS_CONTRACT: TypedContract<NetClassParams> = TypedContract::new();
const NETCLASS_DELETE_CONTRACT: TypedContract<NetClassDeleteParams> = TypedContract::new();
const RULES_INSPECT_CONTRACT: TypedContract<RulesInspectParams> = TypedContract::new();
const IMPEDANCE_CONTRACT: TypedContract<ImpedanceParams> = TypedContract::new();
const CURRENT_CONTRACT: TypedContract<CurrentParams> = TypedContract::new();

/// Millimetres in a reply: to 0.0001.
fn round_mm(v: f64) -> f64 {
    (v * 1e4).round() / 1e4 + 0.0
}

/// Ohms, volts and watts in a reply: six significant figures.
fn round_sig(v: f64) -> f64 {
    if !(v.is_finite() && v != 0.0) {
        return v;
    }
    let scale = 10f64.powi(5 - v.abs().log10().floor() as i32);
    if scale.is_finite() { (v * scale).round() / scale } else { v }
}

/// The rules as replies give them.
pub(super) fn rules_out(r: &Rules) -> RulesOut {
    RulesOut {
        track_width: r.track_width,
        clearance: r.clearance,
        via_diameter: r.via_diameter,
        via_drill: r.via_drill,
        min_track_width: r.min_track_width,
        min_annular_ring: r.min_annular_ring,
        min_drill: r.min_drill,
        hole_to_hole: r.hole_to_hole,
        copper_to_edge: r.copper_to_edge,
        silk_to_pad: r.silk_to_pad,
        microvia_diameter: r.microvia_diameter,
        microvia_drill: r.microvia_drill,
    }
}

/// The schematic's net names.
fn net_names(s: &Session) -> Vec<String> {
    s.doc.ecad.connectivity().nets.into_iter().map(|n| n.name).collect()
}

/// The board's classes as replies give them, with the nets `known` doesn't have.
pub(super) fn classes_out(board: &Board, known: &[String]) -> Vec<NetClassOut> {
    let stack = board.stackup_or_default();
    board
        .rules
        .net_classes
        .iter()
        .map(|c| NetClassOut {
            name: c.name.clone(),
            track_width: c.track_width,
            clearance: c.clearance,
            via_diameter: c.via_diameter,
            via_drill: c.via_drill,
            diff_pair: c.diff_pair.map(|d| DiffPairSpec { width: d.width, gap: d.gap }),
            nets: c.nets.clone(),
            unknown_nets: c.nets.iter().filter(|n| !known.contains(n)).cloned().collect(),
            current: c.current,
            temp_rise: c.temp_rise,
            copper_oz: c.copper_oz,
            proposed: current::class_widths(c, &stack).map(|w| ProposedWidths {
                outer: round_mm(w.outer.width),
                inner: round_mm(w.inner.width),
                temp_rise: w.rise,
                copper_outer: round_mm(w.outer.copper),
                copper_inner: round_mm(w.inner.copper),
                valid_range: w.outer.valid_range && w.inner.valid_range,
            }),
        })
        .collect()
}

fn result(s: &Session, moved: Vec<String>) -> Result<Value> {
    let board = &s.doc.ecad.board;
    let known = net_names(s);
    let default_nets = known.iter().filter(|n| board.rules.class_of(n).is_none()).cloned().collect();
    reply(RulesResult { rules: rules_out(&board.rules), classes: classes_out(board, &known), default_nets, moved, param_warnings: Vec::new() })
}

fn rules(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.rules";
    let p: RulesParams = parse(CMD, p)?;
    let mut board = s.doc.ecad.board.clone();
    let r = &mut board.rules;
    for (slot, v) in [
        (&mut r.track_width, p.track_width),
        (&mut r.clearance, p.clearance),
        (&mut r.via_diameter, p.via_diameter),
        (&mut r.via_drill, p.via_drill),
        (&mut r.min_track_width, p.min_track_width),
        (&mut r.min_annular_ring, p.min_annular_ring),
        (&mut r.min_drill, p.min_drill),
        (&mut r.hole_to_hole, p.hole_to_hole),
        (&mut r.copper_to_edge, p.copper_to_edge),
        (&mut r.silk_to_pad, p.silk_to_pad),
        (&mut r.microvia_diameter, p.microvia_diameter),
        (&mut r.microvia_drill, p.microvia_drill),
    ] {
        if let Some(v) = v {
            *slot = v;
        }
    }
    board.rules.validate().map_err(|e| bad(CMD, e))?;
    store_board(s, board, CMD)?;
    result(s, Vec::new())
}

/// Net names as given: each in its form, each once, at most [`MAX_CLASS_NETS`].
fn net_list(names: &[String], what: &str, cmd: &str) -> Result<Vec<String>> {
    if names.len() > MAX_CLASS_NETS {
        return Err(bad(cmd, format!("`{what}` names at most {MAX_CLASS_NETS} nets")));
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (i, n) in names.iter().enumerate() {
        check_net_name(n).map_err(|e| bad(cmd, format!("`{what}[{i}]`: {e}")))?;
        if seen.insert(n.as_str()) {
            out.push(n.clone());
        }
    }
    Ok(out)
}

fn netclass(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.netclass";
    let p: NetClassParams = parse(CMD, p)?;
    let mut board = s.doc.ecad.board.clone();
    let rules = &mut board.rules;
    let name = p.name.trim().to_string();
    let k = match rules.net_classes.iter().position(|c| c.name.eq_ignore_ascii_case(&name)) {
        Some(k) => k,
        None => {
            check_class_name(&name).map_err(|e| bad(CMD, e))?;
            rules.net_classes.push(NetClass {
                name: name.clone(),
                nets: Vec::new(),
                track_width: rules.track_width,
                clearance: rules.clearance,
                via_diameter: rules.via_diameter,
                via_drill: rules.via_drill,
                diff_pair: None,
                current: None,
                temp_rise: None,
                copper_oz: None,
            });
            rules.net_classes.len() - 1
        }
    };
    if let Some(new) = &p.rename {
        let new = new.trim();
        check_class_name(new).map_err(|e| bad(CMD, format!("`rename`: {e}")))?;
        if rules.net_classes.iter().enumerate().any(|(i, c)| i != k && c.name.eq_ignore_ascii_case(new)) {
            return Err(bad(CMD, format!("`rename`: another net class is named `{new}`")));
        }
    }
    let nets = p.nets.as_deref().map(|n| net_list(n, "nets", CMD)).transpose()?;
    let add = p.add_nets.as_deref().map(|n| net_list(n, "add_nets", CMD)).transpose()?.unwrap_or_default();
    let remove = p.remove_nets.as_deref().map(|n| net_list(n, "remove_nets", CMD)).transpose()?.unwrap_or_default();
    let Some(class) = rules.net_classes.get_mut(k) else { return Err(bad(CMD, "that net class is gone")) };
    if let Some(new) = &p.rename {
        class.name = new.trim().to_string();
    }
    for (slot, v) in [
        (&mut class.track_width, p.track_width),
        (&mut class.clearance, p.clearance),
        (&mut class.via_diameter, p.via_diameter),
        (&mut class.via_drill, p.via_drill),
    ] {
        if let Some(v) = v {
            *slot = v;
        }
    }
    if let Some(d) = p.diff_pair {
        class.diff_pair = d.map(|d| DiffPair { width: d.width, gap: d.gap });
    }
    for (slot, v) in [(&mut class.current, p.current), (&mut class.temp_rise, p.temp_rise), (&mut class.copper_oz, p.copper_oz)] {
        if let Some(v) = v {
            *slot = v;
        }
    }
    if let Some(nets) = nets {
        class.nets = nets;
    }
    for n in add {
        if !class.nets.contains(&n) {
            class.nets.push(n);
        }
    }
    class.nets.retain(|n| !remove.contains(n));
    class.validate().map_err(|e| bad(CMD, e))?;
    // A net is in one class: those named here leave their old ones.
    let mine: BTreeSet<String> = class.nets.iter().cloned().collect();
    let mut moved = Vec::new();
    for (_, other) in rules.net_classes.iter_mut().enumerate().filter(|(i, _)| *i != k) {
        for n in other.nets.iter().filter(|n| mine.contains(*n)) {
            moved.push(format!("{n}: {}", other.name));
        }
        other.nets.retain(|n| !mine.contains(n));
    }
    rules.validate().map_err(|e| bad(CMD, e))?;
    store_board(s, board, CMD)?;
    result(s, moved)
}

fn netclass_delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.netclass.delete";
    let p: NetClassDeleteParams = parse(CMD, p)?;
    let mut board = s.doc.ecad.board.clone();
    let Some(k) = board.rules.net_classes.iter().position(|c| c.name.eq_ignore_ascii_case(p.name.trim())) else {
        let mut msg = format!("the board has no net class `{}`", p.name);
        if let Some(near) = closest(&p.name, board.rules.net_classes.iter().map(|c| c.name.as_str())) {
            msg.push_str(&format!(" (did you mean `{near}`?)"));
        }
        return Err(bad(CMD, msg));
    };
    board.rules.net_classes.remove(k);
    store_board(s, board, CMD)?;
    result(s, Vec::new())
}

fn rules_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.rules.inspect";
    let _: RulesInspectParams = parse(CMD, p)?;
    result(s, Vec::new())
}

fn impedance_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.impedance.inspect";
    let p: ImpedanceParams = parse(CMD, p)?;
    let board = &s.doc.ecad.board;
    let layer = p.layer.0;
    if !board.has_layer(layer) {
        return Err(bad(CMD, no_layer(board, layer.name())));
    }
    let kind = match p.kind {
        LineKindParam::Microstrip => LineKind::Microstrip,
        LineKindParam::Stripline => LineKind::Stripline,
        LineKindParam::Coplanar => LineKind::Coplanar,
        LineKindParam::Differential => LineKind::Differential,
    };
    let m = impedance::medium(&board.stackup_or_default(), layer, board.copper_layers).map_err(|e| bad(CMD, e))?;
    let (width, z) = match (p.width, p.target) {
        (Some(w), None) => (w, impedance::impedance(&m, kind, w, p.gap).map_err(|e| bad(CMD, e))?),
        (None, Some(t)) => impedance::width_for(&m, kind, t, p.gap).map_err(|e| bad(CMD, e))?,
        _ => return Err(bad(CMD, "give the `width` (for its impedance) or a `target` impedance (for its width), not both")),
    };
    let round = |v: f64| (v * 1e4).round() / 1e4 + 0.0;
    reply(ImpedanceResult {
        z0: (z.z0 * 100.0).round() / 100.0,
        width: round(width),
        gap: p.gap.filter(|_| matches!(kind, LineKind::Coplanar | LineKind::Differential)),
        kind: p.kind,
        valid_range: z.valid_range,
        out_of_range: z.out_of_range.map(|o| OutOfRangeOut { input: o.input.to_string(), value: round(o.value), low: o.low, high: o.high }),
        er_eff: z.er_eff.map(|e| (e * 1000.0).round() / 1000.0),
        formula: z.formula.to_string(),
        h: m.h1,
        h2: (!m.outer).then_some(m.h2),
        t: m.t,
        er: (m.er * 1000.0).round() / 1000.0,
        param_warnings: Vec::new(),
    })
}

fn current_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.current.inspect";
    let p: CurrentParams = parse(CMD, p)?;
    let e = |e: CurrentError| bad(CMD, e);
    let board = &s.doc.ecad.board;
    let stack = board.stackup_or_default();
    let given = match p.copper {
        Some(CopperParam::Oz(oz)) => Some(current::oz_to_mm(oz)),
        Some(CopperParam::Mm(mm)) => Some(mm),
        None => None,
    };
    if given.is_some_and(|t| !(t.is_finite() && COPPER.contains(&t))) {
        return Err(e(CurrentError::Copper));
    }
    let (stack_outer, stack_inner) = current::stack_copper(&stack);
    let (copper_outer, copper_inner) = given.map_or((stack_outer, stack_inner), |t| (t, t));
    let (outer, copper, layer) = match p.layer.unwrap_or(CurrentLayerParam::Outer) {
        CurrentLayerParam::Outer => (true, copper_outer, "outer".to_string()),
        CurrentLayerParam::Inner => (false, copper_inner, "inner".to_string()),
        CurrentLayerParam::Layer(l) => {
            if !board.has_layer(l) {
                return Err(bad(CMD, no_layer(board, l.name())));
            }
            let fallback = if l.is_inner() { copper_inner } else { copper_outer };
            let own = surroundings(&stack, l, board.copper_layers).map_or(fallback, |x| x.copper.thickness);
            (!l.is_inner(), given.unwrap_or(own), l.name())
        }
    };
    let ambient = p.ambient.unwrap_or(DEFAULT_AMBIENT);
    if !(ambient.is_finite() && AMBIENT.contains(&ambient)) {
        return Err(e(CurrentError::Ambient));
    }
    let fit = match p.formula.unwrap_or(CurrentFormulaParam::Ipc2221) {
        CurrentFormulaParam::Ipc2221 => Fit::Ipc2221,
        CurrentFormulaParam::Ipc2152 => Fit::Ipc2152,
    };
    let c = match (p.width, p.temp_rise) {
        (Some(_), Some(_)) => {
            return Err(bad(CMD, "give `temp_rise` (for the width it needs) or a track's `width` (for the rise it warms by), not both"));
        }
        (Some(w), None) => current::rise_for_fit(fit, p.current, w, copper, outer).map_err(e)?,
        (None, rise) => current::width_for_fit(fit, p.current, rise.unwrap_or(DEFAULT_RISE), copper, outer).map_err(e)?,
    };
    // The outer and inner widths for the same rise (for a given width, the rise it warms by).
    let like = |t: f64, o: bool| current::width_like_fit(fit, &c, p.current, t, o).map(|x| x.width).ok_or_else(|| e(CurrentError::NoResult));
    let (width_outer, width_inner) = (like(copper_outer, true)?, like(copper_inner, false)?);
    let temperature = ambient + c.rise;
    let loss = p.length.map(|l| current::loss(p.current, c.width, copper, l, temperature)).transpose().map_err(e)?;
    reply(CurrentResult {
        current: p.current,
        width_outer: round_mm(width_outer),
        width_inner: round_mm(width_inner),
        width: round_mm(c.width),
        temp_rise: (c.rise * 100.0).round() / 100.0,
        ambient,
        temperature: (temperature * 100.0).round() / 100.0,
        layer,
        copper: round_mm(copper),
        copper_oz: (current::mm_to_oz(copper) * 1000.0).round() / 1000.0,
        copper_outer: round_mm(copper_outer),
        copper_inner: round_mm(copper_inner),
        length: p.length.filter(|_| loss.is_some()),
        resistance: loss.map(|l| round_sig(l.resistance)),
        voltage_drop: loss.map(|l| round_sig(l.voltage_drop)),
        power: loss.map(|l| round_sig(l.power)),
        formula: fit.formula().to_string(),
        valid_range: c.valid_range,
        param_warnings: Vec::new(),
    })
}
