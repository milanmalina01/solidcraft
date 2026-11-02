//! Electronics: commands on the design's electronics (`doc.ecad`), the Electronics workspace's data,
//! and what the `ecad.*` command files share.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::DeserializeOwned;
use serde_json::Value;
use solidcraft_doc::Document;
use solidcraft_doc::ecad::{
    Detached, ElectronicComponent, Electronics, FootprintItem, MAX_VARIANTS, PackageItem, PadKind, PadShape, PinDirection, PinType, Resolved,
    Schematic, SymbolItem, Variant, connectivity,
};

use super::CommandSpec;
use crate::params::bad;
use crate::schema::ecad::{
    ComponentSummary, GroundViewParams, GroundViewResult, InspectParams, InspectResult, ItemSummary, LayersViewResult, SchematicCounts, ViewParams,
    ViewResult,
};
use crate::schema::ecad_component::{ComponentId, ComponentRef, Direction, Electrical, Kind, Number, RowSpec, Shape, Side};
use crate::schema::ecad_library::Counts;
use crate::schema::ecad_schematic::{DetachedOut, NetChangeOut};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.inspect", "Inspect Electronics", crate::checked!("ecad.inspect", inspect))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&INSPECT_CONTRACT)
        .output(output_schema_for::<InspectResult>)
        .params(
            "No parameters. Returns {empty, counts: {symbols, footprints, packages, components}, symbols?: [{id, name, count (pins)}], footprints?: [{id, name, count (pads)}], packages?: [{id, name}], components?: [{id, name, prefix, value, symbol, variants: [names], pins, pads (the first variant's), ready}], schematic?: {parts, wires, labels, markers, approvals, errors, warnings (the electrical rules check's, not approved)}} (and param_warnings in the warn parameter-check mode): what the design's electronics hold, as data for agents. Read-only",
        ),
    // The SWITCH group of an electronics design: the views of the one design. The 3D board comes
    // next.
    CommandSpec::new("ecad.view.schematic", "Schematic", crate::checked!("ecad.view.schematic", view_schematic))
        .icon("ws_electronics")
        .noundo()
        .doc_only()
        .enabled(electronics_design)
        .contract(&VIEW_CONTRACT)
        .output(output_schema_for::<ViewResult>)
        .params("No parameters → {view: schematic}: an electronics design's page shows its schematic. Changes nothing in the design"),
    CommandSpec::new("ecad.view.board", "Board", crate::checked!("ecad.view.board", view_board))
        .icon("rect")
        .noundo()
        .doc_only()
        .enabled(electronics_design)
        .contract(&VIEW_CONTRACT)
        .output(output_schema_for::<ViewResult>)
        .params("No parameters → {view: board}: an electronics design's page shows its circuit board. Changes nothing in the design"),
    // Board view toggles the ribbon offers (the Inspector's Layers tab has them too).
    CommandSpec::new("ecad.view.ground", "Ground Map", crate::checked!("ecad.view.ground", view_ground))
        .icon("eye")
        .noundo()
        .doc_only()
        .enabled(electronics_design)
        .contract(&GROUND_VIEW_CONTRACT)
        .output(output_schema_for::<GroundViewResult>)
        .params("shown? (left out: toggled) → {layer: ground, shown?}: the board view's Ground connectivity map (how weakly the ground net's copper on the active layer is tied, as ecad.board.ground.inspect measures it) shown or hidden. Changes nothing in the design"),
    CommandSpec::new("ecad.view.layers", "Layers", crate::checked!("ecad.view.layers", view_layers))
        .icon("eye")
        .noundo()
        .doc_only()
        .enabled(electronics_design)
        .contract(&VIEW_CONTRACT)
        .output(output_schema_for::<LayersViewResult>)
        .params("No parameters → {panel: layers}: the board view's Layers panel (each layer shown or hidden, the silkscreen names mode, the ground map). Changes nothing in the design"),
    CommandSpec::new("ecad.view.board_3d", "3D Board", crate::checked!("ecad.view.board_3d", view_later))
        .icon("box")
        .noundo()
        .doc_only()
        .enabled(|_| Err(COMING_NEXT.into()))
        .contract(&VIEW_CONTRACT)
        .output(output_schema_for::<ViewResult>)
        .params("No parameters: an electronics design's board in 3D, which comes after the board; refused until then"),
];

const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();
const VIEW_CONTRACT: TypedContract<ViewParams> = TypedContract::new();
const GROUND_VIEW_CONTRACT: TypedContract<GroundViewParams> = TypedContract::new();

/// Why the 3D board is greyed out.
const COMING_NEXT: &str = "Coming next";

/// The views are an electronics design's.
fn electronics_design(s: &Session) -> std::result::Result<(), String> {
    match s.doc.kind {
        solidcraft_doc::DocumentKind::ElectronicsDesign => Ok(()),
        _ => Err("Only in an electronics design".into()),
    }
}

fn view_schematic(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.view.schematic";
    let _: ViewParams = parse(CMD, p)?;
    electronics_design(s).map_err(|e| bad(CMD, e))?;
    reply(ViewResult { view: "schematic".into(), param_warnings: Vec::new() })
}

fn view_board(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.view.board";
    let _: ViewParams = parse(CMD, p)?;
    electronics_design(s).map_err(|e| bad(CMD, e))?;
    reply(ViewResult { view: "board".into(), param_warnings: Vec::new() })
}

fn view_ground(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.view.ground";
    let p: GroundViewParams = parse(CMD, p)?;
    electronics_design(s).map_err(|e| bad(CMD, e))?;
    reply(GroundViewResult { layer: "ground".into(), shown: p.shown })
}

fn view_layers(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.view.layers";
    let _: ViewParams = parse(CMD, p)?;
    electronics_design(s).map_err(|e| bad(CMD, e))?;
    reply(LayersViewResult { panel: "layers".into() })
}

/// The 3D board: not built yet.
fn view_later(_: &mut Session, _: &Value) -> Result<Value> {
    Err(bad("ecad.view.board_3d", "the 3D board comes after the board; an electronics design shows its schematic or board until then"))
}

/// A summary of the design's electronics. Reads the document only.
fn inspect(s: &mut Session, _p: &Value) -> Result<Value> {
    let ecad = &s.doc.ecad;
    let item = |id: u64, name: &str, count: Option<usize>| ItemSummary { id, name: name.to_string(), count };
    let components = ecad
        .components
        .iter()
        .map(|c| {
            let first = ecad.resolve(c, None);
            ComponentSummary {
                id: c.id,
                name: c.name.clone(),
                prefix: c.prefix.clone(),
                value: c.value.clone(),
                symbol: c.symbol.and_then(|id| ecad.symbol(id)).map(|s| s.name.clone()),
                variants: c.variants.iter().map(|v| v.name.clone()).collect(),
                pins: first.symbol.pins.len(),
                pads: first.footprint.pads.len(),
                ready: ready(s, c),
            }
        })
        .collect();
    let sheet = &ecad.schematic;
    let schematic = sheet.has_content().then(|| {
        let erc = ecad.erc();
        SchematicCounts {
            parts: sheet.parts.len(),
            wires: sheet.wires.len(),
            labels: sheet.labels.len(),
            markers: sheet.markers.len(),
            approvals: sheet.approvals.len(),
            errors: erc.errors,
            warnings: erc.warnings,
        }
    });
    reply(InspectResult {
        empty: !ecad.has_content(),
        counts: counts(ecad),
        symbols: ecad.symbols.iter().map(|x| item(x.id, &x.name, Some(x.pins.len()))).collect(),
        footprints: ecad.footprints.iter().map(|x| item(x.id, &x.name, Some(x.pads.len()))).collect(),
        packages: ecad.packages.iter().map(|x| item(x.id, &x.name, None)).collect(),
        components,
        schematic,
        param_warnings: Vec::new(),
    })
}

pub(super) fn counts(ecad: &Electronics) -> Counts {
    Counts { symbols: ecad.symbols.len(), footprints: ecad.footprints.len(), packages: ecad.packages.len(), components: ecad.components.len() }
}

/// Whether the 3D package `p` (if any) models a SolidCraft component that is gone.
pub(super) fn package_missing(s: &Session, p: Option<&PackageItem>) -> bool {
    p.is_some_and(|p| !s.doc.components.iter().any(|m| m.id == p.model))
}

/// Ready to place with one variant: its own checks pass, and its 3D package (if any) is there.
pub(super) fn variant_ready(s: &Session, r: &Resolved) -> bool {
    r.ready() && !package_missing(s, r.package)
}

/// Ready to place: a variant at least, and every variant ready.
pub(super) fn ready(s: &Session, c: &ElectronicComponent) -> bool {
    let ecad = &s.doc.ecad;
    !c.variants.is_empty() && c.variants.iter().all(|v| variant_ready(s, &ecad.resolve_variant(c, Some(v))))
}

/// The call's parameters as the command's contract types; a mismatch is the caller's error, worded
/// as the strict parameter check words it (`` `components[1]` must be … ``).
pub(super) fn parse<T: DeserializeOwned>(cmd: &str, p: &Value) -> Result<T> {
    serde_path_to_error::deserialize(p).map_err(|e| bad(cmd, crate::schema::parse_issue(&e).message))
}

/// A reply type as the command's result.
pub(super) fn reply(v: impl serde::Serialize) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| EngineError::Other(e.to_string()))
}

/// What the id `id` is in the design's electronics, for errors.
fn what_is(ecad: &Electronics, id: u64) -> Option<&'static str> {
    if ecad.component(id).is_some() {
        Some("an electronic component")
    } else if ecad.symbol(id).is_some() {
        Some("a symbol")
    } else if ecad.footprint(id).is_some() {
        Some("a footprint")
    } else if ecad.package(id).is_some() {
        Some("a 3D package")
    } else {
        None
    }
}

/// The id of the `kind` of record `r` names among `items` (each id and name), or an error that
/// says what there is: an id of another kind is called that, and a name a few letters off is
/// suggested. `list` says what lists them.
fn find_in<'a>(
    ecad: &Electronics,
    kind: &str,
    items: impl Iterator<Item = (u64, &'a str)> + Clone,
    r: &ComponentRef,
    cmd: &str,
    list: &str,
) -> Result<u64> {
    match r {
        ComponentRef::Id(id) => {
            if items.clone().any(|(i, _)| i == *id) {
                return Ok(*id);
            }
            let mut msg = format!("no {kind} has the id {id}");
            if let Some(what) = what_is(ecad, *id) {
                msg.push_str(&format!(" ({id} is {what})"));
            }
            Err(bad(cmd, msg))
        }
        ComponentRef::Name(name) => {
            if let Some((id, _)) = items.clone().find(|(_, n)| n == name) {
                return Ok(id);
            }
            let mut msg = format!("no {kind} is named `{name}`");
            if let Some(near) = closest(name, items.map(|(_, n)| n)) {
                msg.push_str(&format!(" (did you mean `{near}`?)"));
            } else {
                msg.push_str(&format!(" ({list} lists them)"));
            }
            Err(bad(cmd, msg))
        }
    }
}

/// The id of the electronic component `r` names, or an error that says what there is: the name of
/// a SolidCraft component is called that, and a name a few letters off is suggested.
pub(super) fn find(doc: &Document, r: &ComponentRef, cmd: &str) -> Result<u64> {
    let ecad = &doc.ecad;
    if let ComponentRef::Name(name) = r
        && ecad.named(name).is_none()
        && doc.components.iter().any(|c| c.name == *name)
    {
        return Err(bad(
            cmd,
            format!(
                "no electronic component is named `{name}` (`{name}` is a SolidCraft component, which holds bodies; ecad.inspect lists the electronic components)"
            ),
        ));
    }
    find_in(ecad, "electronic component", ecad.components.iter().map(|c| (c.id, c.name.as_str())), r, cmd, "ecad.inspect")
}

/// The id of the library symbol `r` names.
pub(super) fn find_symbol(doc: &Document, r: &ComponentRef, cmd: &str) -> Result<u64> {
    let ecad = &doc.ecad;
    find_in(ecad, "symbol", ecad.symbols.iter().map(|x| (x.id, x.name.as_str())), r, cmd, "ecad.library.inspect")
}

/// The id of the library footprint `r` names.
pub(super) fn find_footprint(doc: &Document, r: &ComponentRef, cmd: &str) -> Result<u64> {
    let ecad = &doc.ecad;
    find_in(ecad, "footprint", ecad.footprints.iter().map(|x| (x.id, x.name.as_str())), r, cmd, "ecad.library.inspect")
}

/// The id of the library 3D package `r` names.
pub(super) fn find_package(doc: &Document, r: &ComponentRef, cmd: &str) -> Result<u64> {
    let ecad = &doc.ecad;
    find_in(ecad, "3D package", ecad.packages.iter().map(|x| (x.id, x.name.as_str())), r, cmd, "ecad.library.inspect")
}

/// A copy of the electronic component `r` names, to change and [`store`].
pub(super) fn load(s: &Session, r: &ComponentRef, cmd: &str) -> Result<ElectronicComponent> {
    let id = find(&s.doc, r, cmd)?;
    s.doc.ecad.component(id).cloned().ok_or_else(|| bad(cmd, format!("no electronic component has the id {id}")))
}

/// A copy of the library symbol `r` names, to change and [`store_symbol`].
pub(super) fn load_symbol(s: &Session, r: &ComponentRef, cmd: &str) -> Result<SymbolItem> {
    let id = find_symbol(&s.doc, r, cmd)?;
    s.doc.ecad.symbol(id).cloned().ok_or_else(|| bad(cmd, format!("no symbol has the id {id}")))
}

/// A copy of the library footprint `r` names, to change and [`store_footprint`].
pub(super) fn load_footprint(s: &Session, r: &ComponentRef, cmd: &str) -> Result<FootprintItem> {
    let id = find_footprint(&s.doc, r, cmd)?;
    s.doc.ecad.footprint(id).cloned().ok_or_else(|| bad(cmd, format!("no footprint has the id {id}")))
}

/// A copy of the library 3D package `r` names.
pub(super) fn load_package(s: &Session, r: &ComponentRef, cmd: &str) -> Result<PackageItem> {
    let id = find_package(&s.doc, r, cmd)?;
    s.doc.ecad.package(id).cloned().ok_or_else(|| bad(cmd, format!("no 3D package has the id {id}")))
}

/// Store `next` as the design's electronics once it passes the design file's checks, with the
/// sheet's saved points that lie on pins tied to them; a refused one, or one equal to what is
/// stored, changes nothing (no undo step). Every edit comes through here.
pub(super) fn commit(s: &mut Session, mut next: Electronics, cmd: &str) -> Result<()> {
    next.settle().map_err(|e| bad(cmd, e))?;
    if s.doc.ecad != next {
        s.doc_mut().ecad = next;
    }
    Ok(())
}

/// What a library edit may change on the schematic under it, taken before the edit (see
/// [`sheet_after`]): the nets, and which parts' texts were moved by hand.
pub(super) struct SheetBefore {
    nets: solidcraft_doc::ecad::Connectivity,
    moved: BTreeMap<u64, (bool, bool)>,
}

/// The schematic as a library edit finds it; `None` with no parts on it (nothing to change).
pub(super) fn sheet_before(s: &Session) -> Option<SheetBefore> {
    let sheet = &s.doc.ecad.schematic;
    if sheet.parts.is_empty() {
        return None;
    }
    let moved = sheet.parts.iter().map(|p| (p.id, (p.designator_at.is_some(), p.value_at.is_some()))).filter(|(_, m)| m.0 || m.1).collect();
    Some(SheetBefore { nets: s.doc.ecad.connectivity(), moved })
}

/// What the library edit since `before` changed on the schematic: the nets whose pins changed
/// (pins that moved within their parts can land on other wires or leave their own), and the
/// designators of the parts whose moved texts went back to where the new drawing puts them.
pub(super) fn sheet_after(s: &Session, before: Option<SheetBefore>) -> (Vec<NetChangeOut>, Vec<String>) {
    let Some(before) = before else { return (Vec::new(), Vec::new()) };
    let changes = solidcraft_doc::ecad::connect::net_changes(&before.nets, &s.doc.ecad.connectivity());
    let nets = changes.into_iter().map(|c| NetChangeOut { net: c.net, lost: c.lost, gained: c.gained, more: c.more }).collect();
    let reset = s
        .doc
        .ecad
        .schematic
        .parts
        .iter()
        .filter(|p| before.moved.get(&p.id).is_some_and(|(d, v)| (*d && p.designator_at.is_none()) || (*v && p.value_at.is_none())))
        .map(|p| p.designator.clone())
        .collect();
    (nets, reset)
}

/// What let go of a pin and still has, in the stored sheet, as replies give it.
pub(super) fn detached_now(s: &Session, gone: &[Detached]) -> Vec<DetachedOut> {
    let sheet = &s.doc.ecad.schematic;
    gone.iter().filter(|d| super::ecad_schematic::still_detached(sheet, d)).map(super::ecad_schematic::detached_out).collect()
}

/// Store `c` (new, or in place of the record with its id) after checking it as the design file
/// does; a refused record, or one equal to what is stored, changes nothing (no undo step). When it
/// shows another symbol than before, its parts on the schematic keep what is on pins the new one
/// numbers alike and let go of the others where they were (returned, as replies give them).
pub(super) fn store(s: &mut Session, c: ElectronicComponent, cmd: &str) -> Result<Vec<DetachedOut>> {
    c.validate().map_err(|e| bad(cmd, e))?;
    let old = &s.doc.ecad;
    if old.component(c.id) == Some(&c) {
        return Ok(Vec::new());
    }
    let mut next = old.clone();
    let symbol = c.symbol;
    match next.components.iter_mut().find(|o| o.id == c.id) {
        Some(slot) => *slot = ElectronicComponent { symbol: slot.symbol, ..c.clone() },
        None => next.components.push(ElectronicComponent { symbol: None, ..c.clone() }),
    }
    let (gone, _) = set_symbol(old, &mut next, c.id, symbol);
    commit(s, next, cmd)?;
    Ok(detached_now(s, &gone))
}

/// Give the electronic component `id` in `next` the symbol `symbol` (one `next`'s library has):
/// its parts on the schematic keep what is on pins the new symbol numbers alike, or names alike
/// where the number is gone (see [`SymbolItem::renames_to`]; a variant's pin map follows those),
/// and let go of the others where they were (`old`, the stored electronics, says where), and a
/// variant's pin map that names pins the new symbol doesn't have is dropped. Returns what let
/// go, and whether a map was dropped.
pub(super) fn set_symbol(old: &Electronics, next: &mut Electronics, id: u64, symbol: Option<u64>) -> (Vec<Detached>, bool) {
    let Some(c) = next.component(id) else { return (Vec::new(), false) };
    if c.symbol == symbol {
        return (Vec::new(), false);
    }
    let parts: BTreeSet<u64> = next.schematic.placed(id).map(|p| p.id).collect();
    let new = symbol.and_then(|x| next.symbol(x)).cloned().unwrap_or_default();
    let renames = c.symbol.and_then(|x| next.symbol(x)).map(|was| was.renames_to(&new, &[])).unwrap_or_default();
    let was_box = c.symbol.and_then(|x| next.symbol(x)).and_then(SymbolItem::bbox);
    let gone = next.schematic.symbol_changed(&old.library(), &parts, &new, &renames);
    next.schematic.texts_redrawn(&parts, was_box, new.bbox());
    if let Some(c) = next.components.iter_mut().find(|c| c.id == id) {
        c.symbol = symbol;
        for v in &mut c.variants {
            for (from, to) in &renames {
                v.pin_renamed(from, Some(to));
            }
        }
    }
    (gone, clear_stale_maps(next, id))
}

/// Drop the pin maps of the electronic component `id` that name pins or pads its symbol and
/// footprints don't have; `true` when one was dropped.
pub(super) fn clear_stale_maps(next: &mut Electronics, id: u64) -> bool {
    let Some(c) = next.component(id) else { return false };
    let stale: Vec<usize> =
        c.variants.iter().enumerate().filter(|(_, v)| next.resolve_variant(c, Some(v)).check_pin_map().is_err()).map(|(i, _)| i).collect();
    if let Some(c) = next.components.iter_mut().find(|c| c.id == id) {
        for i in &stale {
            if let Some(v) = c.variants.get_mut(*i) {
                v.pin_map = None;
            }
        }
    }
    !stale.is_empty()
}

/// Store the library symbol `sym` (new, or in place of the one with its id), with pins renumbered
/// (`renames`, from → to, and a pin whose number went to the one of the same name; see
/// [`SymbolItem::renames_to`]); `whole` when its pin list was replaced. In every electronic component
/// that shows it: a variant's pin map follows a renumbered pin and forgets a deleted one (after a
/// `whole` replacement, a map that named a pin no longer there is dropped instead), and on the
/// schematic wire ends, labels and markers on a renumbered pin follow it, and those on a pin that
/// is gone let go at the point where it was. The sheet must still pass its checks, so an edit that
/// would leave a part with no symbol is refused. Returns what let go and whether a map was dropped.
pub(super) fn store_symbol(
    s: &mut Session,
    sym: SymbolItem,
    renames: &[(String, String)],
    whole: bool,
    cmd: &str,
) -> Result<(Vec<DetachedOut>, bool)> {
    sym.validate().map_err(|e| bad(cmd, e))?;
    let old = &s.doc.ecad;
    let was = old.symbol(sym.id);
    if was == Some(&sym) {
        return Ok((Vec::new(), false));
    }
    // Pins renumbered by the caller, and those whose number went but whose name came back.
    let renames = &was.map(|w| w.renames_to(&sym, renames)).unwrap_or_else(|| renames.to_vec());
    let mut next = old.clone();
    let library = old.library();
    let parts = next.schematic.showing(&library, sym.id);
    let gone = next.schematic.symbol_changed(&library, &parts, &sym, renames);
    // Drawn anew (a generator, a replaced pin list): moved texts were placed for the old drawing.
    if whole {
        next.schematic.texts_redrawn(&parts, was.and_then(SymbolItem::bbox), sym.bbox());
    }
    let renamed: BTreeSet<&str> = renames.iter().map(|(from, _)| from.as_str()).collect();
    let now: BTreeSet<&str> = sym.pins.iter().map(|p| p.number.as_str()).collect();
    let dropped: Vec<String> = was
        .into_iter()
        .flat_map(|w| &w.pins)
        .map(|p| p.number.clone())
        .filter(|n| !now.contains(n.as_str()) && !renamed.contains(n.as_str()))
        .collect();
    let mut cleared = false;
    for v in next.components.iter_mut().filter(|c| c.symbol == Some(sym.id)).flat_map(|c| c.variants.iter_mut()) {
        for (from, to) in renames {
            v.pin_renamed(from, Some(to));
        }
        if whole && v.pin_map.as_ref().is_some_and(|m| dropped.iter().any(|d| m.contains_key(d))) {
            v.pin_map = None;
            cleared = true;
        } else {
            for d in &dropped {
                v.pin_renamed(d, None);
            }
        }
    }
    match next.symbols.iter_mut().find(|o| o.id == sym.id) {
        Some(slot) => *slot = sym,
        None => next.symbols.push(sym),
    }
    commit(s, next, cmd)?;
    Ok((detached_now(s, &gone), cleared))
}

/// Store the library footprint `fp` (new, or in place of the one with its id), with pads
/// renumbered (`renames`, from → to); `whole` when its pad list was replaced. In every variant
/// built on it, the pin map follows a renumbered pad and forgets a deleted one (after a `whole`
/// replacement, a map that named a pad no longer there is dropped instead). Returns whether a map
/// was dropped.
pub(super) fn store_footprint(s: &mut Session, fp: FootprintItem, renames: &[(String, String)], whole: bool, cmd: &str) -> Result<bool> {
    fp.validate().map_err(|e| bad(cmd, e))?;
    let old = &s.doc.ecad;
    let was = old.footprint(fp.id);
    if was == Some(&fp) {
        return Ok(false);
    }
    let mut next = old.clone();
    let renamed: BTreeSet<&str> = renames.iter().map(|(from, _)| from.as_str()).collect();
    let now: BTreeSet<&str> = fp.pads.iter().map(|p| p.number.as_str()).collect();
    let dropped: Vec<String> = was
        .into_iter()
        .flat_map(|w| &w.pads)
        .map(|p| p.number.clone())
        .filter(|n| !now.contains(n.as_str()) && !renamed.contains(n.as_str()))
        .collect();
    let mut cleared = false;
    for v in next.components.iter_mut().flat_map(|c| c.variants.iter_mut()).filter(|v| v.footprint == Some(fp.id)) {
        for (from, to) in renames {
            v.pad_renamed(from, Some(to));
        }
        if whole && v.pin_map.as_ref().is_some_and(|m| m.values().flatten().any(|pad| dropped.contains(pad))) {
            v.pin_map = None;
            cleared = true;
        } else {
            for d in &dropped {
                v.pad_renamed(d, None);
            }
        }
    }
    match next.footprints.iter_mut().find(|o| o.id == fp.id) {
        Some(slot) => *slot = fp,
        None => next.footprints.push(fp),
    }
    commit(s, next, cmd)?;
    Ok(cleared)
}

/// In `next`, give the electronic component `id` a variant change (`set`): to `variant`, its first
/// when that is left out, made (named `made`) when it has none or none with that name. Returns
/// the component, the variant's name, and whether a pin map that no longer fits was dropped.
pub(super) fn set_variant(
    next: &mut Electronics,
    id: u64,
    variant: Option<&str>,
    made: &str,
    set: impl FnOnce(&mut Variant),
    cmd: &str,
) -> Result<(ComponentId, String, bool)> {
    let Some(c) = next.components.iter_mut().find(|c| c.id == id) else { return Err(bad(cmd, format!("no electronic component has the id {id}"))) };
    let at = match variant {
        Some(name) => c.variants.iter().position(|v| v.name == name),
        None => (!c.variants.is_empty()).then_some(0),
    };
    let at = match at {
        Some(at) => at,
        None => {
            if c.variants.len() >= MAX_VARIANTS {
                return Err(bad(cmd, format!("`{}` has {MAX_VARIANTS} variants already", c.name)));
            }
            c.variants.push(Variant { name: variant.unwrap_or(made).to_string(), footprint: None, package: None, pin_map: None });
            c.variants.len() - 1
        }
    };
    let Some(v) = c.variants.get_mut(at) else { return Err(bad(cmd, "that variant is gone")) };
    set(v);
    let name = v.name.clone();
    let component = id_of(c);
    Ok((component, name, clear_stale_maps(next, id)))
}

/// Store `sheet` as the schematic once it passes the design file's checks against the
/// components, with saved points that lie on pins tied to them; a refused sheet, or one equal to
/// what is stored, changes nothing (no undo step). The board's footprints of parts the sheet no
/// longer has go with them, in the same step.
pub(super) fn store_schematic(s: &mut Session, mut sheet: Schematic, cmd: &str) -> Result<()> {
    sheet.settle(&s.doc.ecad.library()).map_err(|e| bad(cmd, e))?;
    if s.doc.ecad.schematic != sheet {
        let mut board = s.doc.ecad.board.clone();
        let mut changed = !board.forget_parts(&sheet).is_empty();
        // The net classes keep their nets when the edit renames them, in the same undo step.
        if board.rules.net_classes.iter().any(|c| !c.nets.is_empty()) {
            let library = s.doc.ecad.library();
            let (before, after) = (connectivity(&s.doc.ecad.schematic, &library), connectivity(&sheet, &library));
            let mut rules = board.rules.clone();
            if rules.follow_nets(&before, &after, &sheet) && rules.validate().is_ok() {
                board.rules = rules;
                changed = true;
            }
        }
        let ecad = &mut s.doc_mut().ecad;
        ecad.schematic = sheet;
        if changed {
            ecad.board = board;
        }
    }
    Ok(())
}

pub(super) fn id_of(c: &ElectronicComponent) -> ComponentId {
    ComponentId { id: c.id, name: c.name.clone() }
}

/// A library item as replies name it.
pub(super) fn item_id(id: u64, name: &str) -> ComponentId {
    ComponentId { id, name: name.to_string() }
}

pub(super) fn electrical(e: Electrical) -> PinType {
    match e {
        Electrical::Input => PinType::Input,
        Electrical::Output => PinType::Output,
        Electrical::Bidirectional => PinType::Bidirectional,
        Electrical::TriState => PinType::TriState,
        Electrical::Passive => PinType::Passive,
        Electrical::PowerInput => PinType::PowerInput,
        Electrical::PowerOutput => PinType::PowerOutput,
        Electrical::OpenCollector => PinType::OpenCollector,
        Electrical::OpenEmitter => PinType::OpenEmitter,
        Electrical::Unspecified => PinType::Unspecified,
        Electrical::NotConnected => PinType::NotConnected,
    }
}

pub(super) fn electrical_out(e: PinType) -> Electrical {
    match e {
        PinType::Input => Electrical::Input,
        PinType::Output => Electrical::Output,
        PinType::Bidirectional => Electrical::Bidirectional,
        PinType::TriState => Electrical::TriState,
        PinType::Passive => Electrical::Passive,
        PinType::PowerInput => Electrical::PowerInput,
        PinType::PowerOutput => Electrical::PowerOutput,
        PinType::OpenCollector => Electrical::OpenCollector,
        PinType::OpenEmitter => Electrical::OpenEmitter,
        PinType::Unspecified => Electrical::Unspecified,
        PinType::NotConnected => Electrical::NotConnected,
    }
}

pub(super) fn direction(d: Direction) -> PinDirection {
    match d {
        Direction::Right => PinDirection::Right,
        Direction::Left => PinDirection::Left,
        Direction::Up => PinDirection::Up,
        Direction::Down => PinDirection::Down,
    }
}

pub(super) fn direction_out(d: PinDirection) -> Direction {
    match d {
        PinDirection::Right => Direction::Right,
        PinDirection::Left => Direction::Left,
        PinDirection::Up => Direction::Up,
        PinDirection::Down => Direction::Down,
    }
}

pub(super) fn side(s: Side) -> solidcraft_doc::ecad::Side {
    match s {
        Side::Top => solidcraft_doc::ecad::Side::Top,
        Side::Bottom => solidcraft_doc::ecad::Side::Bottom,
    }
}

pub(super) fn side_out(s: solidcraft_doc::ecad::Side) -> Side {
    match s {
        solidcraft_doc::ecad::Side::Top => Side::Top,
        solidcraft_doc::ecad::Side::Bottom => Side::Bottom,
    }
}

pub(super) fn shape(s: Shape) -> PadShape {
    match s {
        Shape::Rect => PadShape::Rect,
        Shape::Round => PadShape::Round,
        Shape::Oval => PadShape::Oval,
        Shape::RoundedRect => PadShape::RoundedRect,
    }
}

pub(super) fn shape_out(s: PadShape) -> Shape {
    match s {
        PadShape::Rect => Shape::Rect,
        PadShape::Round => Shape::Round,
        PadShape::Oval => Shape::Oval,
        PadShape::RoundedRect => Shape::RoundedRect,
    }
}

pub(super) fn kind(k: Kind) -> PadKind {
    match k {
        Kind::Smd => PadKind::Smd,
        Kind::ThroughHole => PadKind::ThroughHole,
        Kind::Hole => PadKind::Hole,
    }
}

pub(super) fn kind_out(k: PadKind) -> Kind {
    match k {
        PadKind::Smd => Kind::Smd,
        PadKind::ThroughHole => Kind::ThroughHole,
        PadKind::Hole => Kind::Hole,
    }
}

/// The most items one `row` makes.
pub(super) const MAX_ROW: u32 = 4096;

/// How many items a list of specs with optional rows makes.
pub(super) fn row_total(rows: impl Iterator<Item = Option<RowSpec>>) -> usize {
    rows.fold(0usize, |n, row| n.saturating_add(row.map_or(1, |r| r.count as usize)))
}

/// One numbered item at `at`, or a row of `count` of them `pitch` apart, numbering on from
/// `number` (which must then be a whole number). `what` names the item in errors (`pads[2]`).
pub(super) fn row_items(number: &Number, at: [f64; 2], row: Option<RowSpec>, cmd: &str, what: &str) -> Result<Vec<(String, [f64; 2])>> {
    let Some(RowSpec { count, pitch }) = row else { return Ok(vec![(number.text(), at)]) };
    if count == 0 || count > MAX_ROW {
        return Err(bad(cmd, format!("{what}: `row.count` must be 1 to {MAX_ROW}")));
    }
    if !(pitch[0].is_finite() && pitch[1].is_finite()) {
        return Err(bad(cmd, format!("{what}: `row.pitch` must be two numbers in millimetres")));
    }
    let first = match number {
        Number::Whole(n) => *n,
        Number::Text(t) => t.parse::<u64>().map_err(|_| bad(cmd, format!("{what}: a row numbers on from a whole number, not `{t}`")))?,
    };
    (0..u64::from(count))
        .map(|k| {
            let n = first.checked_add(k).ok_or_else(|| bad(cmd, format!("{what}: the row's numbers run out")))?;
            let k = k as f64;
            Ok((n.to_string(), [at[0] + k * pitch[0], at[1] + k * pitch[1]]))
        })
        .collect()
}

/// The names of the fields that differ, from `(name, differs)` pairs.
pub(super) fn changed<'a>(fields: impl IntoIterator<Item = (&'a str, bool)>) -> Vec<String> {
    fields.into_iter().filter(|(_, differs)| *differs).map(|(name, _)| name.to_string()).collect()
}

/// The name in `names` nearest to `name`, ignoring case, when it is only a few letters off.
pub(super) fn closest<'a>(name: &str, names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let name = name.to_lowercase();
    if name.chars().count() > solidcraft_doc::ecad::MAX_TEXT {
        return None;
    }
    let near = (name.chars().count() / 4).max(2);
    names.map(|n| (super::edit_distance(&name, &n.to_lowercase()), n)).filter(|(d, _)| *d <= near).min_by_key(|(d, _)| *d).map(|(_, n)| n)
}

#[cfg(test)]
#[path = "ecad_tests.rs"]
pub(super) mod tests;
