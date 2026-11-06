//! The design's electronics library: make, rename, copy, delete and read the symbols and
//! footprints electronic components use, and read the whole library with who uses what. One
//! symbol or footprint serves every component that names it, so an edit to it shows in all of
//! them; deleting one a component uses is refused unless `force` clears the references.

use serde_json::Value;
use solidcraft_doc::ecad::footprint::MAX_PADS;
use solidcraft_doc::ecad::graphics::shapes_bbox;
use solidcraft_doc::ecad::{
    BoardLayer, FootprintItem, GraphicType, MAX_COORD, MAX_DESCRIPTION, MAX_ITEMS, PadKind, Severity as DocSeverity, Side, SymbolItem,
    check_item_name, check_number, check_point, check_text,
};

use super::CommandSpec;
use super::ecad::{
    changed, commit, counts, find, find_footprint, find_symbol, id_of, item_id, kind_out, load_footprint, load_symbol, parse, reply, set_symbol,
    set_variant, shape_out, side_out,
};
use super::ecad_component::{pins_out, problems_out};
use crate::params::bad;
use crate::schema::ecad_component::{
    ComponentId, CourtyardOut, GapOut, LayerGraphicOut, PackageModel, PadMeasure, PadOut, PinMeasure, ProblemOut, Severity,
};
use crate::schema::ecad_library::{
    FootprintCopyParams, FootprintCreateParams, FootprintDeleteParams, FootprintEditParams, FootprintInspectParams, FootprintInspectResult,
    FootprintRecord, FootprintResult, FootprintSummary, FootprintVerifyParams, FootprintVerifyResult, ItemDeleteResult, ItemEditResult,
    LibraryInspectParams, LibraryInspectResult, PackageSummary, SymbolCopyParams, SymbolCreateParams, SymbolDeleteParams, SymbolDeleteResult,
    SymbolEditParams, SymbolInspectParams, SymbolInspectResult, SymbolRecord, SymbolResult, SymbolSummary, VariantUse, VerifyBody, VerifyField,
    VerifyMismatch, VerifyOrientation,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.symbol.create", "New Symbol", crate::checked!("ecad.symbol.create", symbol_create))
        .icon("new")
        .doc_only()
        .contract(&SYMBOL_CREATE_CONTRACT)
        .output(output_schema_for::<SymbolResult>)
        .params(
            "name (unique among the symbols, such as Resistor); description?; component?: id|name (it becomes that electronic component's symbol) → {symbol: {id, name}, component?}: an empty library symbol; give it pins with ecad.pin.add or ecad.symbol.generate",
        ),
    CommandSpec::new("ecad.symbol.edit", "Edit Symbol", crate::checked!("ecad.symbol.edit", symbol_edit))
        .icon("edit")
        .doc_only()
        .contract(&SYMBOL_EDIT_CONTRACT)
        .output(output_schema_for::<ItemEditResult>)
        .params("symbol: id|name; name?, description? (an empty text clears it) → {item: {id, name}, changed: [keys]}"),
    CommandSpec::new("ecad.symbol.copy", "Copy Symbol", crate::checked!("ecad.symbol.copy", symbol_copy))
        .icon("copy")
        .doc_only()
        .contract(&SYMBOL_COPY_CONTRACT)
        .output(output_schema_for::<SymbolResult>)
        .params("symbol: id|name; name (the copy's, unique) → {symbol: {id, name}}: the same pins and body, used by no component yet"),
    CommandSpec::new("ecad.symbol.delete", "Delete Symbol", crate::checked!("ecad.symbol.delete", symbol_delete))
        .icon("delete")
        .doc_only()
        .contract(&SYMBOL_DELETE_CONTRACT)
        .output(output_schema_for::<SymbolDeleteResult>)
        .params(
            "symbol: id|name; force? (a symbol an electronic component uses is refused without it; with it those components are left with no symbol and no pin maps) → {deleted: {id, name}, cleared?: [components]}; a symbol of a component placed on the schematic is refused either way",
        ),
    CommandSpec::new("ecad.symbol.inspect", "Inspect Symbol", crate::checked!("ecad.symbol.inspect", symbol_inspect))
        .icon("inspect")
        .noundo()
        .contract(&SYMBOL_INSPECT_CONTRACT)
        .output(output_schema_for::<SymbolInspectResult>)
        .params("symbol: id|name → {symbol: {id, name, description, pins, graphics, body_ends, bbox, problems, used_by: [components]}}. Read-only"),
    CommandSpec::new("ecad.footprint.create", "New Footprint", crate::checked!("ecad.footprint.create", footprint_create))
        .icon("new")
        .doc_only()
        .contract(&FOOTPRINT_CREATE_CONTRACT)
        .output(output_schema_for::<FootprintResult>)
        .params(
            "name (unique among the footprints, such as SOT-23); description?; component?: id|name; variant? (with component: the variant it becomes the footprint of; its first when left out, made, named after the footprint, when it has none or none of that name) → {footprint: {id, name}, component?, variant?}: an empty library footprint; give it pads with ecad.pad.add or ecad.footprint.generate",
        ),
    CommandSpec::new("ecad.footprint.edit", "Edit Footprint", crate::checked!("ecad.footprint.edit", footprint_edit))
        .icon("edit")
        .doc_only()
        .contract(&FOOTPRINT_EDIT_CONTRACT)
        .output(output_schema_for::<ItemEditResult>)
        .params("footprint: id|name; name?, description? (an empty text clears it) → {item: {id, name}, changed: [keys]}"),
    CommandSpec::new("ecad.footprint.copy", "Copy Footprint", crate::checked!("ecad.footprint.copy", footprint_copy))
        .icon("copy")
        .doc_only()
        .contract(&FOOTPRINT_COPY_CONTRACT)
        .output(output_schema_for::<FootprintResult>)
        .params("footprint: id|name; name (the copy's, unique) → {footprint: {id, name}}: the same pads and graphics, used by no variant yet"),
    CommandSpec::new("ecad.footprint.delete", "Delete Footprint", crate::checked!("ecad.footprint.delete", footprint_delete))
        .icon("delete")
        .doc_only()
        .contract(&FOOTPRINT_DELETE_CONTRACT)
        .output(output_schema_for::<ItemDeleteResult>)
        .params(
            "footprint: id|name; force? (a footprint a variant uses is refused without it; with it those variants are left with no footprint and no pin map) → {deleted: {id, name}, cleared?: [{component, variant}]}",
        ),
    CommandSpec::new("ecad.footprint.inspect", "Inspect Footprint", crate::checked!("ecad.footprint.inspect", footprint_inspect))
        .icon("inspect")
        .noundo()
        .contract(&FOOTPRINT_INSPECT_CONTRACT)
        .output(output_schema_for::<FootprintInspectResult>)
        .params(
            "footprint: id|name → {footprint: {id, name, description, pads, graphics, measured: [{number, area, bbox, annular_ring}], copper_bbox, min_pad_gap, courtyard, problems, ready, used_by: [{component, variant}]}}. Read-only",
        ),
    CommandSpec::new("ecad.footprint.verify", "Verify Footprint", crate::checked!("ecad.footprint.verify", footprint_verify))
        .icon("inspect")
        .noundo()
        .contract(&FOOTPRINT_VERIFY_CONTRACT)
        .output(output_schema_for::<FootprintVerifyResult>)
        .params(
            "footprint: id|name; pads: [{number, at: [x, y], size: [w, h]}] (a table read from a datasheet, mm, top view); tolerance? (mm, 0.05, at most 1); body?: [w, h] (mm, against the box of the fab outline) → {ok, footprint, tolerance, footprint_pads, table_pads, offset, missing: [in the table only], extra: [in the footprint only], mismatches: [{number, field: at|size, table, footprint, difference}], worst, orientation: mirror_x|mirror_y|rotate_90|rotate_180|rotate_270|null, orientation_also?: [others that fit as well], body?: {table, footprint, difference}, problems}: pads compare by number after the table is moved so the mean centre of the pads both have lands on the footprint's (offset), with no rotation; a turned pad's size is its turned extent; orientation names what, done to the table, would make at least two pads that differ match; drills, sides and pad shapes aren't compared, and problems' not_compared names the pads with a drill or on the bottom. Read-only",
        ),
    CommandSpec::new("ecad.library.inspect", "Inspect Library", crate::checked!("ecad.library.inspect", library_inspect))
        .icon("library")
        .noundo()
        .doc_only()
        .contract(&LIBRARY_INSPECT_CONTRACT)
        .output(output_schema_for::<LibraryInspectResult>)
        .params(
            "No parameters → {counts: {symbols, footprints, packages, components}, symbols: [{id, name, pins, used_by: [components]}], footprints: [{id, name, pads, used_by: [{component, variant}]}], packages: [{id, name, model, used_by}]}. Read-only",
        ),
];

const SYMBOL_CREATE_CONTRACT: TypedContract<SymbolCreateParams> = TypedContract::new();
const SYMBOL_EDIT_CONTRACT: TypedContract<SymbolEditParams> = TypedContract::new();
const SYMBOL_COPY_CONTRACT: TypedContract<SymbolCopyParams> = TypedContract::new();
const SYMBOL_DELETE_CONTRACT: TypedContract<SymbolDeleteParams> = TypedContract::new();
const SYMBOL_INSPECT_CONTRACT: TypedContract<SymbolInspectParams> = TypedContract::new();
const FOOTPRINT_CREATE_CONTRACT: TypedContract<FootprintCreateParams> = TypedContract::new();
const FOOTPRINT_EDIT_CONTRACT: TypedContract<FootprintEditParams> = TypedContract::new();
const FOOTPRINT_COPY_CONTRACT: TypedContract<FootprintCopyParams> = TypedContract::new();
const FOOTPRINT_DELETE_CONTRACT: TypedContract<FootprintDeleteParams> = TypedContract::new();
const FOOTPRINT_INSPECT_CONTRACT: TypedContract<FootprintInspectParams> = TypedContract::new();
const FOOTPRINT_VERIFY_CONTRACT: TypedContract<FootprintVerifyParams> = TypedContract::new();
const LIBRARY_INSPECT_CONTRACT: TypedContract<LibraryInspectParams> = TypedContract::new();

/// `name` is free for a `kind` of library item among `items` (each id and name), or for `except`
/// renamed; and there is room for a new one when `except` is `None`.
pub(super) fn free_item_name<'a>(
    kind: &str,
    mut items: impl ExactSizeIterator<Item = (u64, &'a str)>,
    name: &str,
    except: Option<u64>,
    cmd: &str,
) -> Result<()> {
    check_item_name(kind, name).map_err(|e| bad(cmd, e))?;
    if except.is_none() && items.len() >= MAX_ITEMS {
        return Err(bad(cmd, format!("a design holds at most {MAX_ITEMS} of each kind of library item")));
    }
    match items.find(|(_, n)| *n == name) {
        Some((id, _)) if Some(id) != except => Err(bad(cmd, format!("a {kind} is already named `{name}`"))),
        _ => Ok(()),
    }
}

fn symbol_names(s: &Session) -> impl ExactSizeIterator<Item = (u64, &str)> {
    s.doc.ecad.symbols.iter().map(|x| (x.id, x.name.as_str()))
}

fn footprint_names(s: &Session) -> impl ExactSizeIterator<Item = (u64, &str)> {
    s.doc.ecad.footprints.iter().map(|x| (x.id, x.name.as_str()))
}

/// An empty library symbol named `name`, for `ecad.symbol.create` and the generator.
pub(super) fn new_symbol(id: u64, name: String, description: String) -> SymbolItem {
    SymbolItem { id, name, description, pins: Vec::new(), graphics: Vec::new() }
}

/// An empty library footprint named `name`.
pub(super) fn new_footprint(id: u64, name: String, description: String) -> FootprintItem {
    FootprintItem { id, name, description, pads: Vec::new(), graphics: Vec::new(), assembly_rotation: 0.0 }
}

fn symbol_create(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.symbol.create";
    let p: SymbolCreateParams = parse(CMD, p)?;
    free_item_name("symbol", symbol_names(s), &p.name, None, CMD)?;
    let component = p.component.as_ref().map(|r| find(&s.doc, r, CMD)).transpose()?;
    let mut next = s.doc.ecad.clone();
    let sym = new_symbol(next.take_id().map_err(|e| bad(CMD, e))?, p.name, p.description.unwrap_or_default());
    sym.validate().map_err(|e| bad(CMD, e))?;
    let (id, out) = (sym.id, item_id(sym.id, &sym.name));
    next.symbols.push(sym);
    let mut pin_map_cleared = false;
    if let Some(c) = component {
        pin_map_cleared = set_symbol(&s.doc.ecad, &mut next, c, Some(id)).1;
    }
    let component = component.and_then(|c| next.component(c)).map(id_of);
    commit(s, next, CMD)?;
    reply(SymbolResult { symbol: out, component, pin_map_cleared, param_warnings: Vec::new() })
}

fn footprint_create(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.footprint.create";
    let p: FootprintCreateParams = parse(CMD, p)?;
    free_item_name("footprint", footprint_names(s), &p.name, None, CMD)?;
    let component = p.component.as_ref().map(|r| find(&s.doc, r, CMD)).transpose()?;
    if p.variant.is_some() && component.is_none() {
        return Err(bad(CMD, "`variant` is a variant of `component`: give the component too"));
    }
    let mut next = s.doc.ecad.clone();
    let fp = new_footprint(next.take_id().map_err(|e| bad(CMD, e))?, p.name, p.description.unwrap_or_default());
    fp.validate().map_err(|e| bad(CMD, e))?;
    let (id, out) = (fp.id, item_id(fp.id, &fp.name));
    next.footprints.push(fp);
    let mut used = None;
    if let Some(c) = component {
        used = Some(set_variant(&mut next, c, p.variant.as_deref(), &out.name, |v| v.footprint = Some(id), CMD)?);
    }
    commit(s, next, CMD)?;
    let (component, variant, pin_map_cleared) = match used {
        Some((c, v, cleared)) => (Some(c), Some(v), cleared),
        None => (None, None, false),
    };
    reply(FootprintResult { footprint: out, component, variant, pin_map_cleared, param_warnings: Vec::new() })
}

fn symbol_edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.symbol.edit";
    let p: SymbolEditParams = parse(CMD, p)?;
    let old = load_symbol(s, &p.symbol, CMD)?;
    let mut sym = old.clone();
    if let Some(name) = p.name {
        free_item_name("symbol", symbol_names(s), &name, Some(old.id), CMD)?;
        sym.name = name;
    }
    if let Some(d) = p.description {
        check_text("description", &d, MAX_DESCRIPTION).map_err(|e| bad(CMD, e))?;
        sym.description = d;
    }
    let changed = changed([("name", old.name != sym.name), ("description", old.description != sym.description)]);
    let item = item_id(sym.id, &sym.name);
    if !changed.is_empty() {
        super::ecad::store_symbol(s, sym, &[], false, CMD)?;
    }
    reply(ItemEditResult { item, changed, param_warnings: Vec::new() })
}

fn footprint_edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.footprint.edit";
    let p: FootprintEditParams = parse(CMD, p)?;
    let old = load_footprint(s, &p.footprint, CMD)?;
    let mut fp = old.clone();
    if let Some(name) = p.name {
        free_item_name("footprint", footprint_names(s), &name, Some(old.id), CMD)?;
        fp.name = name;
    }
    if let Some(d) = p.description {
        check_text("description", &d, MAX_DESCRIPTION).map_err(|e| bad(CMD, e))?;
        fp.description = d;
    }
    let changed = changed([("name", old.name != fp.name), ("description", old.description != fp.description)]);
    let item = item_id(fp.id, &fp.name);
    if !changed.is_empty() {
        super::ecad::store_footprint(s, fp, &[], false, CMD)?;
    }
    reply(ItemEditResult { item, changed, param_warnings: Vec::new() })
}

fn symbol_copy(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.symbol.copy";
    let p: SymbolCopyParams = parse(CMD, p)?;
    let mut sym = load_symbol(s, &p.symbol, CMD)?;
    free_item_name("symbol", symbol_names(s), &p.name, None, CMD)?;
    let mut next = s.doc.ecad.clone();
    sym.id = next.take_id().map_err(|e| bad(CMD, e))?;
    sym.name = p.name;
    let out = item_id(sym.id, &sym.name);
    next.symbols.push(sym);
    commit(s, next, CMD)?;
    reply(SymbolResult { symbol: out, component: None, pin_map_cleared: false, param_warnings: Vec::new() })
}

fn footprint_copy(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.footprint.copy";
    let p: FootprintCopyParams = parse(CMD, p)?;
    let mut fp = load_footprint(s, &p.footprint, CMD)?;
    free_item_name("footprint", footprint_names(s), &p.name, None, CMD)?;
    let mut next = s.doc.ecad.clone();
    fp.id = next.take_id().map_err(|e| bad(CMD, e))?;
    fp.name = p.name;
    let out = item_id(fp.id, &fp.name);
    next.footprints.push(fp);
    commit(s, next, CMD)?;
    reply(FootprintResult { footprint: out, component: None, variant: None, pin_map_cleared: false, param_warnings: Vec::new() })
}

/// Up to eight names, then `…`, for an error.
pub(super) fn some_names(names: &[String]) -> String {
    let mut out: Vec<&str> = names.iter().take(8).map(String::as_str).collect();
    if names.len() > out.len() {
        out.push("…");
    }
    out.join(", ")
}

fn symbol_delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.symbol.delete";
    let p: SymbolDeleteParams = parse(CMD, p)?;
    let id = find_symbol(&s.doc, &p.symbol, CMD)?;
    let ecad = &s.doc.ecad;
    let deleted = ecad.symbol(id).map(|x| item_id(x.id, &x.name)).unwrap_or_else(|| item_id(id, ""));
    let users: Vec<ComponentId> = ecad.symbol_users(id).map(id_of).collect();
    if !users.is_empty() {
        let names: Vec<String> = users.iter().map(|c| c.name.clone()).collect();
        if !p.force.unwrap_or(false) {
            return Err(bad(
                CMD,
                format!(
                    "it is the symbol of {}: give them another first (ecad.component.edit), or give `force: true` to leave them with none",
                    some_names(&names)
                ),
            ));
        }
        let placed: Vec<String> =
            ecad.schematic.parts.iter().filter(|q| users.iter().any(|c| c.id == q.component)).map(|q| q.designator.clone()).collect();
        if !placed.is_empty() {
            return Err(bad(
                CMD,
                format!("its components are placed on the schematic as {}: delete those parts first (ecad.schematic.delete)", some_names(&placed)),
            ));
        }
    }
    let mut next = ecad.clone();
    for c in &users {
        set_symbol(ecad, &mut next, c.id, None);
    }
    next.symbols.retain(|x| x.id != id);
    commit(s, next, CMD)?;
    reply(SymbolDeleteResult { deleted, cleared: users, param_warnings: Vec::new() })
}

fn footprint_delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.footprint.delete";
    let p: FootprintDeleteParams = parse(CMD, p)?;
    let id = find_footprint(&s.doc, &p.footprint, CMD)?;
    let ecad = &s.doc.ecad;
    let deleted = ecad.footprint(id).map(|x| item_id(x.id, &x.name)).unwrap_or_else(|| item_id(id, ""));
    let users: Vec<VariantUse> = ecad.footprint_users(id).map(|(c, v)| VariantUse { component: id_of(c), variant: v.name.clone() }).collect();
    if !users.is_empty() && !p.force.unwrap_or(false) {
        let names: Vec<String> = users.iter().map(|u| format!("{} ({})", u.component.name, u.variant)).collect();
        return Err(bad(
            CMD,
            format!(
                "{} {} built on it: give them another first (ecad.component.variant.edit), or give `force: true` to leave them with none",
                some_names(&names),
                if users.len() == 1 { "is" } else { "are" }
            ),
        ));
    }
    let mut next = ecad.clone();
    for v in next.components.iter_mut().flat_map(|c| c.variants.iter_mut()).filter(|v| v.footprint == Some(id)) {
        v.footprint = None;
        v.pin_map = None;
    }
    next.footprints.retain(|x| x.id != id);
    commit(s, next, CMD)?;
    reply(ItemDeleteResult { deleted, cleared: users, param_warnings: Vec::new() })
}

fn symbol_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.symbol.inspect";
    let p: SymbolInspectParams = parse(CMD, p)?;
    let sym = load_symbol(s, &p.symbol, CMD)?;
    let record = SymbolRecord {
        id: sym.id,
        name: sym.name.clone(),
        description: sym.description.clone(),
        pins: pins_out(&sym),
        graphics: sym.graphics.iter().map(super::ecad_graphics::graphic_out).collect(),
        body_ends: sym.pins.iter().map(|p| PinMeasure { number: p.number.clone(), body_end: p.body_end() }).collect(),
        bbox: sym.bbox(),
        problems: problems_out(sym.problems()),
        used_by: s.doc.ecad.symbol_users(sym.id).map(id_of).collect(),
    };
    reply(SymbolInspectResult { symbol: record, param_warnings: Vec::new() })
}

/// A footprint's pads as `ecad.pad.add` takes them.
pub(super) fn pads_out(fp: &FootprintItem) -> Vec<PadOut> {
    fp.pads
        .iter()
        .map(|p| PadOut {
            number: p.number.clone(),
            at: p.at,
            size: p.size,
            rotation: p.rotation,
            side: side_out(p.side),
            shape: shape_out(p.shape),
            corner: p.corner,
            kind: kind_out(p.kind),
            drill: p.drill,
        })
        .collect()
}

fn footprint_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.footprint.inspect";
    let p: FootprintInspectParams = parse(CMD, p)?;
    let fp = load_footprint(s, &p.footprint, CMD)?;
    let problems = fp.problems();
    let ready = !fp.pads.is_empty() && problems.iter().all(|x| x.severity != DocSeverity::Error);
    let record = FootprintRecord {
        id: fp.id,
        name: fp.name.clone(),
        description: fp.description.clone(),
        pads: pads_out(&fp),
        graphics: fp
            .graphics
            .iter()
            .filter_map(|g| {
                Some(LayerGraphicOut {
                    layer: super::ecad_graphics::layer_out(g.layer?),
                    side: side_out(g.side),
                    graphic: super::ecad_graphics::graphic_out(g),
                })
            })
            .collect(),
        measured: fp
            .pads
            .iter()
            .map(|p| PadMeasure { number: p.number.clone(), area: p.area(), bbox: p.bbox(), annular_ring: p.annular_ring() })
            .collect(),
        copper_bbox: fp.copper_bbox(),
        min_pad_gap: fp.min_pad_gap().map(|g| GapOut { pads: g.pads, gap: g.gap }),
        courtyard: fp.courtyard(solidcraft_doc::ecad::Side::Top).map(|[from, to]| CourtyardOut {
            from,
            to,
            area: (to[0] - from[0]) * (to[1] - from[1]),
        }),
        problems: problems_out(problems),
        ready,
        used_by: s.doc.ecad.footprint_users(fp.id).map(|(c, v)| VariantUse { component: id_of(c), variant: v.name.clone() }).collect(),
    };
    reply(FootprintInspectResult { footprint: record, param_warnings: Vec::new() })
}

/// The largest tolerance `ecad.footprint.verify` takes, in millimetres: past this nothing is checked.
const MAX_VERIFY_TOLERANCE: f64 = 1.0;
/// What arithmetic leaves on two values that are the same, in millimetres.
const VERIFY_SLACK: f64 = 1e-9;

/// A pad both sides have: the table's centre and size, the footprint's centre and turned extent.
struct VerifyPair {
    number: String,
    table: ([f64; 2], [f64; 2]),
    footprint: ([f64; 2], [f64; 2]),
}

/// One pair compared: the table's centre after the mirror or turn and the offset, its size after
/// them, and the two differences.
struct VerifyDiff {
    at: [f64; 2],
    size: [f64; 2],
    at_difference: f64,
    size_difference: f64,
}

/// `at` and `size` after a mirror or a quarter turn.
fn verify_turn(turn: Option<VerifyOrientation>, [x, y]: [f64; 2], [w, h]: [f64; 2]) -> ([f64; 2], [f64; 2]) {
    match turn {
        None => ([x, y], [w, h]),
        Some(VerifyOrientation::MirrorX) => ([-x, y], [w, h]),
        Some(VerifyOrientation::MirrorY) => ([x, -y], [w, h]),
        Some(VerifyOrientation::Rotate90) => ([-y, x], [h, w]),
        Some(VerifyOrientation::Rotate180) => ([-x, -y], [w, h]),
        Some(VerifyOrientation::Rotate270) => ([y, -x], [h, w]),
    }
}

/// The offset that lands the mean of the table's centres (after `turn`) on the footprint's, and
/// each pair compared with it. `pairs` is not empty, or the offset is 0.
fn verify_compare(pairs: &[VerifyPair], turn: Option<VerifyOrientation>) -> ([f64; 2], Vec<VerifyDiff>) {
    let turned: Vec<([f64; 2], [f64; 2])> = pairs.iter().map(|q| verify_turn(turn, q.table.0, q.table.1)).collect();
    let n = pairs.len().max(1) as f64;
    let mean = |pick: &dyn Fn(usize) -> f64| (0..pairs.len()).map(pick).sum::<f64>() / n;
    let centre = |i: usize| turned.get(i).map_or([0.0; 2], |t| t.0);
    let target = |i: usize| pairs.get(i).map_or([0.0; 2], |q| q.footprint.0);
    let offset = [mean(&|i| target(i)[0] - centre(i)[0]), mean(&|i| target(i)[1] - centre(i)[1])];
    let diffs = pairs
        .iter()
        .zip(&turned)
        .map(|(q, (at, size))| {
            let moved = [at[0] + offset[0], at[1] + offset[1]];
            let (fp_at, fp_size) = q.footprint;
            VerifyDiff {
                at: moved,
                size: *size,
                at_difference: (moved[0] - fp_at[0]).hypot(moved[1] - fp_at[1]),
                size_difference: (size[0] - fp_size[0]).abs().max((size[1] - fp_size[1]).abs()),
            }
        })
        .collect();
    (offset, diffs)
}

/// A length for the reply: to the nanometre, so a match reads 0 and not arithmetic's leftovers.
fn verify_round(v: f64) -> f64 {
    (v * 1e6).round() / 1e6 + 0.0
}

fn verify_round2([x, y]: [f64; 2]) -> [f64; 2] {
    [verify_round(x), verify_round(y)]
}

fn footprint_verify(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.footprint.verify";
    const TURNS: [VerifyOrientation; 5] = [
        VerifyOrientation::MirrorX,
        VerifyOrientation::MirrorY,
        VerifyOrientation::Rotate90,
        VerifyOrientation::Rotate180,
        VerifyOrientation::Rotate270,
    ];
    let p: FootprintVerifyParams = parse(CMD, p)?;
    let fp = load_footprint(s, &p.footprint, CMD)?;
    let tolerance = p.tolerance.unwrap_or(0.05);
    if !(tolerance.is_finite() && (0.0..=MAX_VERIFY_TOLERANCE).contains(&tolerance)) {
        return Err(bad(CMD, format!("`tolerance` must be 0 to {MAX_VERIFY_TOLERANCE} mm")));
    }
    if p.pads.is_empty() || p.pads.len() > MAX_PADS {
        return Err(bad(CMD, format!("give 1 to {MAX_PADS} pads in `pads`")));
    }
    let side_ok = |v: f64| v.is_finite() && v > 0.0 && v <= MAX_COORD;
    if let Some(body) = p.body
        && !body.into_iter().all(side_ok)
    {
        return Err(bad(CMD, format!("each `body` side must be more than 0 and at most {MAX_COORD} mm")));
    }
    let mut numbers = std::collections::BTreeSet::new();
    let (mut pairs, mut missing) = (Vec::new(), Vec::new());
    for (i, q) in p.pads.iter().enumerate() {
        let number = q.number.text();
        check_number("pad", &number).map_err(|e| bad(CMD, format!("pads[{i}]: {e}")))?;
        check_point(&format!("pads[{i}]: `at`"), q.at).map_err(|e| bad(CMD, e))?;
        if !q.size.into_iter().all(side_ok) {
            return Err(bad(CMD, format!("pads[{i}]: each `size` side must be more than 0 and at most {MAX_COORD} mm")));
        }
        if !numbers.insert(number.clone()) {
            return Err(bad(CMD, format!("pads[{i}]: the table already has a pad numbered `{number}`")));
        }
        match fp.pad(&number) {
            Some(pad) => {
                let [[x0, y0], [x1, y1]] = pad.bbox();
                pairs.push(VerifyPair { number, table: (q.at, q.size), footprint: (pad.at, [x1 - x0, y1 - y0]) });
            }
            None => missing.push(number),
        }
    }
    let extra: Vec<String> = fp.pads.iter().filter(|q| !numbers.contains(&q.number)).map(|q| q.number.clone()).collect();
    let limit = tolerance + VERIFY_SLACK;
    let fits = |diffs: &[VerifyDiff]| diffs.iter().all(|d| d.at_difference <= limit && d.size_difference <= limit);

    let (offset, diffs) = verify_compare(&pairs, None);
    let worst = diffs.iter().map(|d| d.at_difference.max(d.size_difference)).fold(None, |w: Option<f64>, v| Some(w.map_or(v, |w| w.max(v))));
    let mut mismatches = Vec::new();
    for (q, d) in pairs.iter().zip(&diffs) {
        if d.at_difference > limit {
            mismatches.push(VerifyMismatch {
                number: q.number.clone(),
                field: VerifyField::At,
                table: verify_round2(d.at),
                footprint: verify_round2(q.footprint.0),
                difference: verify_round(d.at_difference),
            });
        }
        if d.size_difference > limit {
            mismatches.push(VerifyMismatch {
                number: q.number.clone(),
                field: VerifyField::Size,
                table: verify_round2(d.size),
                footprint: verify_round2(q.footprint.1),
                difference: verify_round(d.size_difference),
            });
        }
    }
    // One pad in common says nothing about which way round the table is.
    let turns: Vec<VerifyOrientation> =
        if pairs.len() < 2 || fits(&diffs) { Vec::new() } else { TURNS.into_iter().filter(|t| fits(&verify_compare(&pairs, Some(*t)).1)).collect() };

    let mut problems = problems_out(fp.problems());
    let copper = fp.pads.iter().filter(|q| q.kind != PadKind::Hole).count();
    // A placeholder such as {reference} marks nothing; ecad.quality.inspect's silk.polarity judges a mark once the part is placed.
    let silk = fp.layer(BoardLayer::Silkscreen, Side::Top).any(|g| !(g.kind == GraphicType::Text && g.text.starts_with('{')));
    if copper > 2 && !silk {
        problems.push(ProblemOut {
            code: "no_pin_1_mark".into(),
            severity: Severity::Warning,
            items: Vec::new(),
            message: "the footprint has no silkscreen drawing, so nothing marks pin 1: draw a mark beside it".into(),
        });
    }
    let unchecked: Vec<String> = fp.pads.iter().filter(|q| q.drill.is_some() || q.side == Side::Bottom).map(|q| q.number.clone()).collect();
    if !unchecked.is_empty() {
        problems.push(ProblemOut {
            code: "not_compared".into(),
            severity: Severity::Warning,
            message:
                "these pads have a drill or are on the bottom, which a table of centres and sizes doesn't say: check them with ecad.footprint.inspect"
                    .into(),
            items: unchecked,
        });
    }
    let body = p.body.map(|table| {
        let outline = shapes_bbox(fp.layer(BoardLayer::Fab, Side::Top).filter(|g| g.kind != GraphicType::Text));
        let size = outline.map(|[[x0, y0], [x1, y1]]| [x1 - x0, y1 - y0]);
        let difference = size.map(|[w, h]| (table[0] - w).abs().max((table[1] - h).abs()));
        VerifyBody { table, footprint: size.map(verify_round2), difference: difference.map(verify_round) }
    });
    if body.as_ref().is_some_and(|b| b.footprint.is_none()) {
        problems.push(ProblemOut {
            code: "body_not_checked".into(),
            severity: Severity::Error,
            items: Vec::new(),
            message: "the footprint has no fabrication outline to compare `body` with: draw the body on the fab layer, or leave out `body`".into(),
        });
    }
    problems.sort_by_key(|x| !matches!(x.severity, Severity::Error));
    let body_fits = body.as_ref().is_none_or(|b| b.difference.is_none_or(|d| d <= limit));
    let ok = missing.is_empty()
        && extra.is_empty()
        && mismatches.is_empty()
        && body_fits
        && !pairs.is_empty()
        && !problems.iter().any(|x| matches!(x.severity, Severity::Error));
    reply(FootprintVerifyResult {
        ok,
        footprint: item_id(fp.id, &fp.name),
        tolerance,
        footprint_pads: fp.pads.len(),
        table_pads: p.pads.len(),
        offset: (!pairs.is_empty()).then_some(verify_round2(offset)),
        missing,
        extra,
        mismatches,
        worst: worst.map(verify_round),
        orientation: turns.first().copied(),
        orientation_also: turns.get(1..).unwrap_or_default().to_vec(),
        body,
        problems,
        param_warnings: Vec::new(),
    })
}

fn library_inspect(s: &mut Session, _p: &Value) -> Result<Value> {
    let ecad = &s.doc.ecad;
    let uses = |it: &mut dyn Iterator<Item = (&solidcraft_doc::ecad::ElectronicComponent, &solidcraft_doc::ecad::Variant)>| -> Vec<VariantUse> {
        it.map(|(c, v)| VariantUse { component: id_of(c), variant: v.name.clone() }).collect()
    };
    let symbols = ecad
        .symbols
        .iter()
        .map(|x| SymbolSummary { id: x.id, name: x.name.clone(), pins: x.pins.len(), used_by: ecad.symbol_users(x.id).map(id_of).collect() })
        .collect();
    let footprints = ecad
        .footprints
        .iter()
        .map(|x| FootprintSummary { id: x.id, name: x.name.clone(), pads: x.pads.len(), used_by: uses(&mut ecad.footprint_users(x.id)) })
        .collect();
    let packages = ecad
        .packages
        .iter()
        .map(|x| PackageSummary {
            id: x.id,
            name: x.name.clone(),
            model: PackageModel { id: x.model, name: s.doc.components.iter().find(|m| m.id == x.model).map(|m| m.name.clone()) },
            used_by: uses(&mut ecad.package_users(x.id)),
        })
        .collect();
    reply(LibraryInspectResult { counts: counts(ecad), symbols, footprints, packages, param_warnings: Vec::new() })
}
