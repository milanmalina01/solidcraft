//! The schematic: place parts (copies of electronic components' symbols) on the sheet, change,
//! move, copy, delete and number them, and read the sheet back as data. Each call changes a copy
//! of the sheet and stores it whole once it passes the design file's checks, so a refused call
//! leaves the design as it was, a call is one undo step, and a call that changes nothing makes
//! none.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use solidcraft_doc::Document;
use solidcraft_doc::ecad::connect::{connectivity, net_changes};
use solidcraft_doc::ecad::frame::MAX_FRAMES;
use solidcraft_doc::ecad::place::{GRID, flow, snap_point};
use solidcraft_doc::ecad::route::moved;
use solidcraft_doc::ecad::schematic::{
    MAX_LABELS, MAX_MARKERS, MAX_PARTS, MAX_WIRES, Order, Pins, check_designator, check_sheet_point, designator, part_bbox, split_designator,
};
use solidcraft_doc::ecad::{
    Anchor, Detached, Frame, ItemKind, Label, LabelKind, Level, Library, MAX_TEXT, Marker, MarkerKind, Motion, Part, Placement, Schematic, Wire,
    WireEnd, bbox, check_text,
};

use super::CommandSpec;
use super::ecad::{closest, direction_out, electrical_out, find, parse, reply, store_schematic};
use crate::params::bad;
use crate::schema::ecad_component::ComponentId;
use crate::schema::ecad_schematic::{
    AnchorOut, AnnotateOrder, AnnotateParams, ApprovalOut, DeleteParams, DetachedOut, DuplicateParams, EditParams, InspectParams, InspectResult,
    ItemKind as KindOut, ItemOut, ItemRef, LabelKind as LabelKindOut, LabelOut, Level as LevelOut, MarkerKind as MarkerKindOut, MarkerOut,
    MoveParams, NetChangeOut, PartRecord, PinRef, PlaceParams, PlaceSpec, RenameOut, SheetPin, SheetResult, WireEnd as WireEndOut, WireOut,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.schematic.place", "Add Part", crate::checked!("ecad.schematic.place", place))
        .icon("component")
        .doc_only()
        .contract(&PLACE_CONTRACT)
        .output(output_schema_for::<SheetResult>)
        .params(
            "parts: [{component: id|name (it shows the component's library symbol), at? [x, y] (mm; left out: in rows under what the sheet holds, on the 2.54 mm grid), rotation? 0|90|180|270, mirror?, designator? (left out: the prefix and one more than its highest number), value? (left out: the component's), variant? (by name; left out: the component's first)}] → {created, changed, removed, parts: [{id, designator, component, value, variant, at, rotation, mirror, bbox, pins: [{number, name, type, at, direction}]}]}",
        ),
    CommandSpec::new("ecad.schematic.edit", "Edit Schematic Parts", crate::checked!("ecad.schematic.edit", edit))
        .icon("edit")
        .doc_only()
        .contract(&EDIT_CONTRACT)
        .output(output_schema_for::<SheetResult>)
        .params(
            "parts: [{part: designator|id, designator?, value? (null: the component's), component? (swap: what is on pins with the same numbers stays), variant? (by name; null: the component's first), dnp?, designator_at?, value_at? (offsets from at; null: the usual spot)}] → {created, changed, removed, parts, detached}",
        ),
    CommandSpec::new("ecad.schematic.move", "Move on Schematic", crate::checked!("ecad.schematic.move", move_items))
        .icon("move")
        .doc_only()
        .contract(&MOVE_CONTRACT)
        .output(output_schema_for::<SheetResult>)
        .params(
            "items: [designator|title|id] (parts by designator, frames by title; parts, wires, labels, markers and frames by id); by? [dx, dy] | to? [x, y] (where the first item goes); rotation? (degrees, a multiple of 90); mirror?; about? [x, y] (the one item's position, a frame's middle, or the middle of them) → {created, changed, removed, parts, netlist_changed: [{net, lost: [\"R1.2\"], gained, more?}]}; connections are kept where they can be: a wire end on a moved pin, label, marker, wire end or bend (or inside a segment that moves whole) moves with it, and so do labels and markers there; a wire with both ends moving moves whole (a chain of such wires too); a wire with one end moving keeps whatever stands on it (T feet, labels, markers) and runs onto no other pin, label, wire or part body where it can: the bend next to the end slides so its leg stays horizontal or vertical (a straight wire gains an elbow), else the end jogs onto the old path ahead of what stands on it, else it is auto-routed to the first point that stays; an end on a turned or mirrored pin leaves it the new way; a moved wire's end that is a T into a wire left behind stays, and of a picked wire only the segments that move whole take what is on them along; turned or mirrored, power ports on the parts' pins keep pointing the usual way (supply up, ground down, or straight out where that runs into something); netlist_changed lists any connection the move made or broke anyway (empty: none)",
        ),
    CommandSpec::new("ecad.schematic.duplicate", "Duplicate on Schematic", crate::checked!("ecad.schematic.duplicate", duplicate))
        .icon("copy")
        .doc_only()
        .contract(&DUPLICATE_CONTRACT)
        .output(output_schema_for::<SheetResult>)
        .params(
            "items: [designator|title|id]; by: [dx, dy] → {created, …, parts}: copies with the next designators; wires and labels between copied parts join the copies; a frame copies as a frame titled `<title> 2` (the next free number)",
        ),
    CommandSpec::new("ecad.schematic.delete", "Delete", crate::checked!("ecad.schematic.delete", delete))
        .icon("delete")
        .doc_only()
        .contract(&DELETE_CONTRACT)
        .output(output_schema_for::<SheetResult>)
        .params(
            "items: [designator|title|id] (parts by designator, frames by title; parts, wires, labels, markers and frames by id; all or none) → {removed, detached}: wires and labels on a deleted part's pins stay, at the points where the pins were; its no-connect markers go",
        ),
    CommandSpec::new("ecad.schematic.annotate", "Renumber", crate::checked!("ecad.schematic.annotate", annotate))
        .icon("rename")
        .doc_only()
        .contract(&ANNOTATE_CONTRACT)
        .output(output_schema_for::<SheetResult>)
        .params(
            "order: position (top to bottom, then left to right) | placement; keep_existing (keep designators that are the prefix and a number); start? (1) → {changed, renames: [{part, from, to}]}",
        ),
    CommandSpec::new("ecad.schematic.inspect", "Inspect Schematic", crate::checked!("ecad.schematic.inspect", inspect))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&INSPECT_CONTRACT)
        .output(output_schema_for::<InspectResult>)
        .params(
            "part? designator|id | pin? \"R1.2\"|{part, pin} → {parts: [{…, pins: [{number, name, type, at, direction}]}], wires: [{id, from, via, to}], labels, markers, approvals, rules}; ends give their point `at` and, on a pin, its part and pin. Read-only",
        ),
];

const PLACE_CONTRACT: TypedContract<PlaceParams> = TypedContract::new();
const EDIT_CONTRACT: TypedContract<EditParams> = TypedContract::new();
const MOVE_CONTRACT: TypedContract<MoveParams> = TypedContract::new();
const DUPLICATE_CONTRACT: TypedContract<DuplicateParams> = TypedContract::new();
const DELETE_CONTRACT: TypedContract<DeleteParams> = TypedContract::new();
const ANNOTATE_CONTRACT: TypedContract<AnnotateParams> = TypedContract::new();
const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();

/// The most items one call names: everything a sheet can hold.
const MAX_ITEMS: usize = MAX_PARTS + MAX_WIRES + MAX_LABELS + MAX_MARKERS + MAX_FRAMES;

/// The sheet's objects by id, its parts by designator and its frames by title (both lower case),
/// looked up once.
pub(super) struct Index {
    kinds: BTreeMap<u64, ItemKind>,
    designators: BTreeMap<String, u64>,
    titles: BTreeMap<String, u64>,
}

impl Index {
    pub(super) fn new(sheet: &Schematic) -> Self {
        let mut kinds = BTreeMap::new();
        kinds.extend(sheet.parts.iter().map(|p| (p.id, ItemKind::Part)));
        kinds.extend(sheet.wires.iter().map(|w| (w.id, ItemKind::Wire)));
        kinds.extend(sheet.labels.iter().map(|l| (l.id, ItemKind::Label)));
        kinds.extend(sheet.markers.iter().map(|m| (m.id, ItemKind::Marker)));
        kinds.extend(sheet.frames.iter().map(|f| (f.id, ItemKind::Frame)));
        let designators = sheet.parts.iter().map(|p| (p.designator.to_ascii_lowercase(), p.id)).collect();
        let titles = sheet.frames.iter().map(|f| (f.title.to_ascii_lowercase(), f.id)).collect();
        Index { kinds, designators, titles }
    }

    /// The sheet object `r` names (a text names a part by designator, else a frame by title);
    /// `what` names it in errors (`items[2]`).
    pub(super) fn find(&self, sheet: &Schematic, r: &ItemRef, cmd: &str, what: &str) -> Result<(ItemKind, u64)> {
        match r {
            ItemRef::Id(id) => {
                self.kinds.get(id).map(|k| (*k, *id)).ok_or_else(|| bad(cmd, format!("{what}: the schematic has nothing with the id {id}")))
            }
            ItemRef::Designator(d) => {
                if let Some(id) = self.designators.get(&d.to_ascii_lowercase()) {
                    return Ok((ItemKind::Part, *id));
                }
                if let Some(id) = self.titles.get(&d.to_ascii_lowercase()) {
                    return Ok((ItemKind::Frame, *id));
                }
                let mut msg = format!("{what}: no part on the schematic is designated `{d}`, and no frame is titled so");
                if let Some(near) = closest(d, sheet.parts.iter().map(|p| p.designator.as_str())) {
                    msg.push_str(&format!(" (did you mean `{near}`?)"));
                } else if sheet.parts.is_empty() {
                    msg.push_str(" (the schematic has no parts yet: place some with ecad.schematic.place)");
                }
                Err(bad(cmd, msg))
            }
        }
    }

    /// The part `r` names.
    pub(super) fn part(&self, sheet: &Schematic, r: &ItemRef, cmd: &str, what: &str) -> Result<u64> {
        match self.find(sheet, r, cmd, what)? {
            (ItemKind::Part, id) => Ok(id),
            (kind, id) => Err(bad(cmd, format!("{what}: {id} is a {}, not a part", kind_name(kind)))),
        }
    }
}

pub(super) fn kind_name(k: ItemKind) -> &'static str {
    match k {
        ItemKind::Part => "part",
        ItemKind::Wire => "wire",
        ItemKind::Label => "label",
        ItemKind::Marker => "marker",
        ItemKind::Frame => "frame",
    }
}

/// The sheet objects `items` name, at least one.
fn selection(sheet: &Schematic, items: &[ItemRef], cmd: &str) -> Result<Vec<(ItemKind, u64)>> {
    if items.is_empty() || items.len() > MAX_ITEMS {
        return Err(bad(cmd, format!("name 1 to {MAX_ITEMS} sheet objects in `items`, parts by designator or anything by id")));
    }
    let index = Index::new(sheet);
    let mut out: Vec<(ItemKind, u64)> = Vec::new();
    let mut seen = BTreeSet::new();
    for (i, r) in items.iter().enumerate() {
        let item = index.find(sheet, r, cmd, &format!("items[{i}]"))?;
        if seen.insert(item.1) {
            out.push(item);
        }
    }
    Ok(out)
}

/// A quarter turn given in degrees: a multiple of 90, taken modulo 360.
pub(super) fn quarter(rotation: Option<i32>, cmd: &str, what: &str) -> Result<u16> {
    match rotation {
        None => Ok(0),
        Some(r) if r % 90 == 0 => Ok(r.rem_euclid(360) as u16),
        Some(r) => Err(bad(cmd, format!("{what}`rotation` must be 0, 90, 180 or 270 degrees (got {r})"))),
    }
}

/// A point given on the sheet, rounded to 0.0001 mm.
pub(super) fn sheet_point(p: [f64; 2], cmd: &str, what: &str) -> Result<[f64; 2]> {
    check_sheet_point(what, p).map_err(|e| bad(cmd, e))?;
    Ok(snap_point(p))
}

/// A distance to move by: finite, and no more than twice across the sheet.
fn offset(p: [f64; 2], cmd: &str, what: &str) -> Result<[f64; 2]> {
    let limit = 2.0 * solidcraft_doc::ecad::schematic::MAX_SHEET;
    if !(p[0].is_finite() && p[1].is_finite() && p[0].abs() <= limit && p[1].abs() <= limit) {
        return Err(bad(cmd, format!("{what} must be within {limit} mm")));
    }
    Ok(snap_point(p))
}

fn place(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.place";
    let p: PlaceParams = parse(CMD, p)?;
    if p.parts.is_empty() || p.parts.len() > MAX_PARTS {
        return Err(bad(CMD, format!("give 1 to {MAX_PARTS} parts in `parts`")));
    }
    let old = s.doc.ecad.schematic.clone();
    let mut sheet = old.clone();
    place_rows(&s.doc, &s.doc.ecad.library(), &mut sheet, p.parts, CMD)?;
    store_schematic(s, sheet, CMD)?;
    reply(sheet_result(s, &old, &[], Vec::new()))
}

/// Place `parts` on `sheet` (all or none), as `ecad.schematic.place` does; errors name the rows
/// `parts[i]`.
pub(super) fn place_rows(doc: &Document, library: &Library, sheet: &mut Schematic, parts: Vec<PlaceSpec>, cmd: &str) -> Result<()> {
    {
        struct Row<'a> {
            component: solidcraft_doc::ecad::Placeable<'a>,
            placement: Placement,
            flow: bool,
            designator: Option<String>,
            value: Option<String>,
            variant: Option<String>,
        }
        let mut rows = Vec::with_capacity(parts.len());
        for (i, spec) in parts.into_iter().enumerate() {
            let what = format!("parts[{i}]");
            let id = find(doc, &spec.component, cmd)?;
            let Some(c) = library.get(&id).copied() else { return Err(bad(cmd, format!("{what}: no electronic component has the id {id}"))) };
            if c.symbol.is_empty() {
                return Err(bad(
                    cmd,
                    format!(
                        "{what}: `{}` has no symbol yet: give it one with ecad.symbol.generate {{symbol, component}} or ecad.component.edit",
                        c.component.name
                    ),
                ));
            }
            if let Some(v) = &spec.variant
                && c.component.variant(v).is_none()
            {
                return Err(bad(cmd, format!("{what}: `{}` has no variant named `{v}`", c.component.name)));
            }
            let rotation = quarter(spec.rotation, cmd, &format!("{what}: "))?;
            let at = spec.at.map(|a| sheet_point(a, cmd, &format!("{what}: `at`"))).transpose()?;
            if let Some(d) = &spec.designator {
                check_designator(d).map_err(|e| bad(cmd, format!("{what}: {e}")))?;
            }
            if let Some(v) = &spec.value {
                check_text("value", v, MAX_TEXT).map_err(|e| bad(cmd, format!("{what}: {e}")))?;
            }
            let placement = Placement { at: at.unwrap_or_default(), rotation, mirror: spec.mirror.unwrap_or(false) };
            rows.push(Row { component: c, placement, flow: at.is_none(), designator: spec.designator, value: spec.value, variant: spec.variant });
        }
        // Parts given no position go in rows under everything else, those placed here included.
        let placed = rows.iter().filter(|r| !r.flow).filter_map(|r| r.component.symbol.bbox().map(|b| r.placement.bbox(b)));
        let content = bbox(sheet.bbox(library).into_iter().flatten().chain(placed.flatten()));
        let boxes: Vec<[[f64; 2]; 2]> =
            rows.iter().filter(|r| r.flow).map(|r| r.component.symbol.bbox().map(|b| r.placement.bbox(b)).unwrap_or_default()).collect();
        let mut origins = flow(content, &boxes).into_iter();
        for r in rows.iter_mut().filter(|r| r.flow) {
            r.placement.at = origins.next().unwrap_or_default();
        }
        // Designators given first, so the numbers handed out go past them.
        let mut numbers = sheet.numbers();
        for d in rows.iter().filter_map(|r| r.designator.as_deref()) {
            if let Some((head, n)) = split_designator(d) {
                let top = numbers.entry(head.to_ascii_uppercase()).or_insert(0);
                *top = (*top).max(n);
            }
        }
        for (i, r) in rows.into_iter().enumerate() {
            let designator = match r.designator {
                Some(d) => d,
                None => {
                    let top = numbers.entry(r.component.component.prefix.to_ascii_uppercase()).or_insert(0);
                    *top = top.saturating_add(1);
                    designator(&r.component.component.prefix, *top).map_err(|e| bad(cmd, format!("parts[{i}]: {e}")))?
                }
            };
            let id = sheet.take_id().map_err(|e| bad(cmd, e))?;
            sheet.parts.push(Part {
                id,
                component: r.component.component.id,
                variant: r.variant,
                designator,
                value: r.value,
                placement: r.placement,
                designator_at: None,
                value_at: None,
                dnp: false,
            });
        }
    }
    Ok(())
}

fn edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.edit";
    let p: EditParams = parse(CMD, p)?;
    if p.parts.is_empty() || p.parts.len() > MAX_PARTS {
        return Err(bad(CMD, format!("give 1 to {MAX_PARTS} parts in `parts`")));
    }
    let old = s.doc.ecad.schematic.clone();
    let mut sheet = old.clone();
    let detached;
    {
        let library = s.doc.ecad.library();
        let index = Index::new(&sheet);
        let mut rows = Vec::with_capacity(p.parts.len());
        let mut swaps: BTreeMap<u64, u64> = BTreeMap::new();
        for (i, e) in p.parts.into_iter().enumerate() {
            let what = format!("parts[{i}]");
            let id = index.part(&sheet, &e.part, CMD, &what)?;
            if let Some(r) = &e.component {
                let c = find(&s.doc, r, CMD)?;
                if library.get(&c).is_some_and(|c| c.symbol.is_empty()) {
                    return Err(bad(CMD, format!("{what}: that electronic component has no symbol yet, so it can't be placed")));
                }
                swaps.insert(id, c);
            }
            if let Some(d) = &e.designator {
                check_designator(d).map_err(|err| bad(CMD, format!("{what}: {err}")))?;
            }
            if let Some(Some(v)) = &e.value {
                check_text("value", v, MAX_TEXT).map_err(|err| bad(CMD, format!("{what}: {err}")))?;
            }
            let designator_at =
                e.designator_at.map(|v| v.map(|v| sheet_point(v, CMD, &format!("{what}: `designator_at`"))).transpose()).transpose()?;
            let value_at = e.value_at.map(|v| v.map(|v| sheet_point(v, CMD, &format!("{what}: `value_at`"))).transpose()).transpose()?;
            if let Some(Some(v)) = &e.variant {
                let c = swaps.get(&id).copied().or_else(|| sheet.part(id).map(|q| q.component));
                if c.and_then(|c| library.get(&c)).is_none_or(|c| c.component.variant(v).is_none()) {
                    return Err(bad(CMD, format!("{what}: its electronic component has no variant named `{v}`")));
                }
            }
            rows.push((id, e.designator, e.value, e.variant, e.dnp, designator_at, value_at));
        }
        // A swapped part keeps what is on the pins its new component numbers alike.
        let swapped: BTreeSet<u64> = swaps.iter().filter(|(id, c)| sheet.part(**id).is_some_and(|p| p.component != **c)).map(|(id, _)| *id).collect();
        let keeps: BTreeMap<u64, BTreeSet<&str>> =
            swaps.iter().filter_map(|(id, c)| Some((*id, library.get(c)?.symbol.pins.iter().map(|q| q.number.as_str()).collect()))).collect();
        detached = sheet.detach(&swapped, &library, |part, pin| keeps.get(&part).is_some_and(|k| k.contains(pin)));
        for (id, designator, value, variant, dnp, designator_at, value_at) in rows {
            let Some(part) = sheet.parts.iter_mut().find(|q| q.id == id) else { continue };
            if let Some(c) = swaps.get(&id) {
                // A variant the new component doesn't have goes back to its first.
                if part.component != *c && part.variant.as_ref().is_some_and(|v| library.get(c).is_none_or(|n| n.component.variant(v).is_none())) {
                    part.variant = None;
                }
                part.component = *c;
            }
            if let Some(v) = variant {
                part.variant = v;
            }
            if let Some(d) = designator {
                part.designator = d;
            }
            if let Some(v) = value {
                part.value = v;
            }
            if let Some(dnp) = dnp {
                part.dnp = dnp;
            }
            if let Some(v) = designator_at {
                part.designator_at = v;
            }
            if let Some(v) = value_at {
                part.value_at = v;
            }
        }
    }
    store_schematic(s, sheet, CMD)?;
    reply(sheet_result(s, &old, &detached, Vec::new()))
}

/// Where a sheet object is, for moving to a point and turning about the middle: a part's `at`,
/// a wire's first end, a label's or marker's point.
fn position(sheet: &Schematic, pins: &Pins, (kind, id): (ItemKind, u64)) -> Option<[f64; 2]> {
    match kind {
        ItemKind::Part => sheet.part(id).map(|p| p.placement.at),
        ItemKind::Wire => sheet.wires.iter().find(|w| w.id == id).and_then(|w| pins.anchor(&w.from)),
        ItemKind::Label => sheet.labels.iter().find(|l| l.id == id).and_then(|l| pins.anchor(&l.at)),
        ItemKind::Marker => sheet.markers.iter().find(|m| m.id == id).and_then(|m| pins.anchor(&m.at)),
        ItemKind::Frame => sheet.frames.iter().find(|f| f.id == id).map(|f| f.rect[0]),
    }
}

fn move_items(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.move";
    let p: MoveParams = parse(CMD, p)?;
    let old = s.doc.ecad.schematic.clone();
    let items = selection(&old, &p.items, CMD)?;
    let rotation = quarter(p.rotation, CMD, "")?;
    let mirror = p.mirror.unwrap_or(false);
    if p.by.is_some() && p.to.is_some() {
        return Err(bad(CMD, "give `by` or `to`, not both"));
    }
    if p.by.is_none() && p.to.is_none() && rotation == 0 && !mirror {
        return Err(bad(CMD, "give `by` or `to` to move, `rotation` to turn or `mirror` to mirror"));
    }
    let ids: BTreeSet<u64> = items.iter().map(|(_, id)| *id).collect();
    let (sheet, changes) = {
        let library = s.doc.ecad.library();
        let pins = Pins::new(&old, &library);
        let places: Vec<[f64; 2]> = items.iter().filter_map(|item| position(&old, &pins, *item)).collect();
        // A frame turns about its middle (both its corners count), so it stays round what it frames.
        let turning: Vec<[f64; 2]> = items
            .iter()
            .flat_map(|item| match item.0 {
                ItemKind::Frame => old.frames.iter().find(|f| f.id == item.1).map_or(Vec::new(), |f| f.rect.to_vec()),
                _ => position(&old, &pins, *item).into_iter().collect(),
            })
            .collect();
        let about = match p.about {
            Some(a) => sheet_point(a, CMD, "`about`")?,
            None if turning.len() == 1 => turning.first().copied().unwrap_or_default(),
            // Frames alone turn about their very middle, so they stay where they were.
            None if items.iter().all(|(kind, _)| *kind == ItemKind::Frame) => {
                bbox(turning.iter().copied()).map_or([0.0, 0.0], |[[x0, y0], [x1, y1]]| [(x0 + x1) / 2.0, (y0 + y1) / 2.0])
            }
            // The middle, on the grid, so parts on the grid stay on it.
            None => bbox(turning.iter().copied()).map_or([0.0, 0.0], |[[x0, y0], [x1, y1]]| {
                snap_point([((x0 + x1) / 2.0 / GRID).round() * GRID, ((y0 + y1) / 2.0 / GRID).round() * GRID])
            }),
        };
        let turned = Motion { about, mirror, rotation, by: [0.0, 0.0] };
        let by = match (p.by, p.to) {
            (Some(by), _) => offset(by, CMD, "`by`")?,
            (None, Some(to)) => {
                let to = sheet_point(to, CMD, "`to`")?;
                let from = places.first().map(|q| turned.point(*q)).unwrap_or(to);
                snap_point([to[0] - from[0], to[1] - from[1]])
            }
            (None, None) => [0.0, 0.0],
        };
        let motion = Motion { by, ..turned };
        // Wires and what stands on the moved points hang on, and ports on turned parts stay
        // upright: see `moved`. A connection the move makes or breaks anyway is reported.
        let sheet = moved(&old, &library, &ids, &motion);
        let changes = net_changes(&connectivity(&old, &library), &connectivity(&sheet, &library));
        (sheet, changes)
    };
    store_schematic(s, sheet, CMD)?;
    let mut out = sheet_result(s, &old, &[], Vec::new());
    out.netlist_changed = Some(changes.into_iter().map(|c| NetChangeOut { net: c.net, lost: c.lost, gained: c.gained, more: c.more }).collect());
    reply(out)
}

fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.duplicate";
    let p: DuplicateParams = parse(CMD, p)?;
    let old = s.doc.ecad.schematic.clone();
    let mut sheet = old.clone();
    let ids: BTreeSet<u64> = selection(&sheet, &p.items, CMD)?.into_iter().map(|(_, id)| id).collect();
    let by = offset(p.by, CMD, "`by`")?;
    let shift = |[x, y]: [f64; 2]| snap_point([x + by[0], y + by[1]]);
    {
        let library = s.doc.ecad.library();
        let pins = Pins::new(&old, &library);
        let mut numbers = sheet.numbers();
        let mut copies: BTreeMap<u64, u64> = BTreeMap::new();
        for part in old.parts.iter().filter(|q| ids.contains(&q.id)) {
            let Some(c) = library.get(&part.component) else { continue };
            let top = numbers.entry(c.component.prefix.to_ascii_uppercase()).or_insert(0);
            *top = top.saturating_add(1);
            let designator = designator(&c.component.prefix, *top).map_err(|e| bad(CMD, e))?;
            let id = sheet.take_id().map_err(|e| bad(CMD, e))?;
            let placement = Placement { at: shift(part.placement.at), ..part.placement };
            sheet.parts.push(Part { id, designator, placement, ..part.clone() });
            copies.insert(part.id, id);
        }
        // An anchor on a copied part goes on the copy; on another part, on the point by the pin.
        let carry = |a: &Anchor| match a {
            Anchor::Pin { part, pin } => match copies.get(part) {
                Some(copy) => Some(Anchor::Pin { part: *copy, pin: pin.clone() }),
                None => pins.at(*part, pin).map(|q| Anchor::Point(shift(q))),
            },
            Anchor::Point(q) => Some(Anchor::Point(shift(*q))),
        };
        for w in old.wires.iter().filter(|w| ids.contains(&w.id)) {
            let (Some(from), Some(to)) = (carry(&w.from), carry(&w.to)) else { continue };
            let id = sheet.take_id().map_err(|e| bad(CMD, e))?;
            sheet.wires.push(Wire { id, from, via: w.via.iter().map(|q| shift(*q)).collect(), to });
        }
        for l in old.labels.iter().filter(|l| ids.contains(&l.id)) {
            let Some(at) = carry(&l.at) else { continue };
            let id = sheet.take_id().map_err(|e| bad(CMD, e))?;
            sheet.labels.push(Label { id, at, ..l.clone() });
        }
        for m in old.markers.iter().filter(|m| ids.contains(&m.id)) {
            // A no-connect marker stays with its pin: it is copied only with its part.
            if m.kind == MarkerKind::NoConnect && m.at.pin().is_none_or(|(part, _)| !copies.contains_key(&part)) {
                continue;
            }
            let Some(at) = carry(&m.at) else { continue };
            let id = sheet.take_id().map_err(|e| bad(CMD, e))?;
            sheet.markers.push(Marker { id, at, ..m.clone() });
        }
        // A frame is copied as a frame of its own, titled apart (`Power 2`).
        for f in old.frames.iter().filter(|f| ids.contains(&f.id)) {
            if sheet.frames.len() >= MAX_FRAMES {
                return Err(bad(CMD, format!("a schematic holds at most {MAX_FRAMES} frames")));
            }
            let Some(title) = copy_title(&sheet, &f.title) else {
                return Err(bad(CMD, format!("no free title for a copy of frame `{}`: rename some frames", f.title)));
            };
            let id = sheet.take_id().map_err(|e| bad(CMD, e))?;
            let rect = f.rect.map(|[x, y]| [x + by[0], y + by[1]]);
            sheet.frames.push(Frame { id, title, rect, order: None });
        }
    }
    store_schematic(s, sheet, CMD)?;
    reply(sheet_result(s, &old, &[], Vec::new()))
}

/// `Power 2`, `Power 3`…: the first title no frame on `sheet` has (ignoring case) for a copy of
/// the frame titled `title`.
fn copy_title(sheet: &Schematic, title: &str) -> Option<String> {
    (2..=MAX_FRAMES + 1).find_map(|k| {
        let tail = format!(" {k}");
        let head: String = title.chars().take(MAX_TEXT.saturating_sub(tail.chars().count())).collect();
        let t = format!("{}{tail}", head.trim_end());
        (!sheet.frames.iter().any(|f| f.title.eq_ignore_ascii_case(&t))).then_some(t)
    })
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.delete";
    let p: DeleteParams = parse(CMD, p)?;
    let old = s.doc.ecad.schematic.clone();
    let mut sheet = old.clone();
    let ids: BTreeSet<u64> = selection(&sheet, &p.items, CMD)?.into_iter().map(|(_, id)| id).collect();
    let detached = sheet.remove(&ids, &s.doc.ecad.library());
    store_schematic(s, sheet, CMD)?;
    reply(sheet_result(s, &old, &detached, Vec::new()))
}

fn annotate(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.annotate";
    let p: AnnotateParams = parse(CMD, p)?;
    let start = p.start.unwrap_or(1);
    if start == 0 {
        return Err(bad(CMD, "`start` must be 1 or more"));
    }
    let order = match p.order {
        AnnotateOrder::Position => Order::Position,
        AnnotateOrder::Placement => Order::Placement,
    };
    let old = s.doc.ecad.schematic.clone();
    let mut sheet = old.clone();
    let new = old.annotate(&s.doc.ecad.library(), order, p.keep_existing, start).map_err(|e| bad(CMD, e))?;
    let mut renames = Vec::with_capacity(new.len());
    for (id, d) in new {
        if let Some(part) = sheet.parts.iter_mut().find(|q| q.id == id) {
            renames.push(RenameOut { part: id, from: std::mem::replace(&mut part.designator, d.clone()), to: d });
        }
    }
    store_schematic(s, sheet, CMD)?;
    reply(sheet_result(s, &old, &[], renames))
}

fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.schematic.inspect";
    let p: InspectParams = parse(CMD, p)?;
    let sheet = &s.doc.ecad.schematic;
    let library = s.doc.ecad.library();
    let pins = Pins::new(sheet, &library);
    let index = Index::new(sheet);
    let only: Option<(u64, Option<String>)> = match (&p.part, &p.pin) {
        (Some(_), Some(_)) => return Err(bad(CMD, "give `part` or `pin`, not both")),
        (Some(r), None) => Some((index.part(sheet, r, CMD, "`part`")?, None)),
        (None, Some(r)) => {
            let (part, pin) = find_pin(sheet, &index, &pins, &library, r, CMD, "`pin`")?;
            Some((part, Some(pin)))
        }
        (None, None) => None,
    };
    let on = |a: &Anchor| match (&only, a.pin()) {
        (None, _) => true,
        (Some((part, None)), Some((p, _))) => *part == p,
        (Some((part, Some(pin))), Some((p, q))) => *part == p && pin == q,
        (Some(_), None) => false,
    };
    let parts = sheet
        .parts
        .iter()
        .filter(|q| only.as_ref().is_none_or(|(part, _)| *part == q.id))
        .map(|q| {
            let mut r = part_record(&library, q);
            if let Some((_, Some(pin))) = &only {
                r.pins.retain(|x| x.number == *pin);
            }
            r
        })
        .collect();
    let anchor = |a: &Anchor| anchor_out(sheet, &pins, a);
    let wires = sheet
        .wires
        .iter()
        .filter(|w| on(&w.from) || on(&w.to))
        .map(|w| WireOut { id: w.id, from: anchor(&w.from), via: w.via.clone(), to: anchor(&w.to) })
        .collect();
    let labels = sheet
        .labels
        .iter()
        .filter(|l| on(&l.at))
        .map(|l| LabelOut { id: l.id, name: l.name.clone(), kind: label_kind_out(l.kind), at: anchor(&l.at), rotation: l.rotation })
        .collect();
    let markers = sheet
        .markers
        .iter()
        .filter(|m| on(&m.at))
        .map(|m| MarkerOut {
            id: m.id,
            kind: match m.kind {
                MarkerKind::NoConnect => MarkerKindOut::NoConnect,
                MarkerKind::PowerFlag => MarkerKindOut::PowerFlag,
            },
            at: anchor(&m.at),
        })
        .collect();
    let (approvals, rules) = if only.is_some() {
        (Vec::new(), BTreeMap::new())
    } else {
        (
            sheet.approvals.iter().map(|a| ApprovalOut { key: a.key.clone(), note: a.note.clone() }).collect(),
            sheet.rules.iter().map(|(code, level)| (code.clone(), level_out(*level))).collect(),
        )
    };
    let frames = if only.is_some() { Vec::new() } else { super::ecad_frame::frames_out(sheet, &library) };
    reply(InspectResult { parts, wires, labels, markers, approvals, rules, frames, param_warnings: Vec::new() })
}

/// The part and pin number `r` names: by number, then by a name only one of the part's pins has.
/// `what` names it in errors (`` `pin` ``, `wires[0].from`).
pub(super) fn find_pin(sheet: &Schematic, index: &Index, pins: &Pins, library: &Library, r: &PinRef, cmd: &str, what: &str) -> Result<(u64, String)> {
    let (part, pin) = match r {
        PinRef::Text(t) => {
            let Some((d, pin)) = t.split_once('.') else {
                return Err(bad(cmd, format!("{what}: `{t}` is not a pin: give the designator, a dot and the pin's number or name, such as R1.2")));
            };
            (index.part(sheet, &ItemRef::Designator(d.to_string()), cmd, what)?, pin.to_string())
        }
        PinRef::Of(of) => (index.part(sheet, &of.part, cmd, what)?, of.pin.text()),
    };
    if pins.pin(part, &pin).is_some() {
        return Ok((part, pin));
    }
    let symbol = sheet.part(part).and_then(|q| library.get(&q.component)).map(|c| &c.symbol.pins);
    let designator = sheet.part(part).map(|q| q.designator.as_str()).unwrap_or_default();
    let named: Vec<&str> = symbol.into_iter().flatten().filter(|q| q.name == pin).map(|q| q.number.as_str()).collect();
    match named.as_slice() {
        [one] => Ok((part, one.to_string())),
        [] => {
            let mut msg = format!("{what}: {designator} has no pin numbered or named `{pin}`");
            let names = symbol.into_iter().flatten().flat_map(|q| [q.number.as_str(), q.name.as_str()]).filter(|n| !n.is_empty());
            if let Some(near) = closest(&pin, names) {
                msg.push_str(&format!(" (did you mean `{near}`?)"));
            }
            Err(bad(cmd, msg))
        }
        several => Err(bad(cmd, format!("{what}: {designator} has several pins named `{pin}` ({}): give a number", several.join(", ")))),
    }
}

fn label_kind_out(k: LabelKind) -> LabelKindOut {
    match k {
        LabelKind::Net => LabelKindOut::Net,
        LabelKind::Rail => LabelKindOut::Rail,
        LabelKind::Ground => LabelKindOut::Ground,
    }
}

fn level_out(l: Level) -> LevelOut {
    match l {
        Level::Error => LevelOut::Error,
        Level::Warning => LevelOut::Warning,
        Level::Ignore => LevelOut::Ignore,
    }
}

fn kind_out(k: ItemKind) -> KindOut {
    match k {
        ItemKind::Part => KindOut::Part,
        ItemKind::Wire => KindOut::Wire,
        ItemKind::Label => KindOut::Label,
        ItemKind::Marker => KindOut::Marker,
        ItemKind::Frame => KindOut::Frame,
    }
}

/// An anchor and where it is now.
fn anchor_out(sheet: &Schematic, pins: &Pins, a: &Anchor) -> AnchorOut {
    let at = snap_point(pins.anchor(a).unwrap_or_default());
    match a {
        Anchor::Point(_) => AnchorOut { at, part: None, designator: None, pin: None },
        Anchor::Pin { part, pin } => {
            AnchorOut { at, part: Some(*part), designator: sheet.part(*part).map(|q| q.designator.clone()), pin: Some(pin.clone()) }
        }
    }
}

/// A part in full, with its pins where they lie on the sheet.
pub(super) fn part_record(library: &Library, p: &Part) -> PartRecord {
    let c = library.get(&p.component).copied();
    let pins = c
        .map(|c| {
            c.symbol
                .pins
                .iter()
                .map(|q| {
                    let w = p.placement.pin(q);
                    SheetPin {
                        number: w.number,
                        name: w.name,
                        electrical: electrical_out(w.electrical),
                        at: snap_point(w.at),
                        direction: direction_out(w.direction),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    PartRecord {
        id: p.id,
        designator: p.designator.clone(),
        component: ComponentId { id: p.component, name: c.map(|c| c.component.name.clone()).unwrap_or_default() },
        value: p.value.clone().or_else(|| c.map(|c| c.component.value.clone())).unwrap_or_default(),
        library_value: p.value.is_none(),
        variant: p.variant.clone(),
        at: p.placement.at,
        rotation: p.placement.rotation,
        mirror: p.placement.mirror,
        dnp: p.dnp,
        designator_at: p.designator_at,
        value_at: p.value_at,
        bbox: c.and_then(|c| part_bbox(p, c.symbol)).map(|[a, b]| [snap_point(a), snap_point(b)]),
        pins,
    }
}

/// A detached anchor as replies give it.
pub(super) fn detached_out(d: &Detached) -> DetachedOut {
    DetachedOut {
        kind: kind_out(d.kind),
        id: d.id,
        end: d.end.map(|e| match e {
            WireEnd::From => WireEndOut::From,
            WireEnd::To => WireEndOut::To,
        }),
        part: d.part,
        designator: d.designator.clone(),
        pin: d.pin.clone(),
        at: d.at,
        removed: d.removed,
    }
}

/// Whether a detached anchor is still on its point in `sheet` (storing ties a point that lies on
/// another pin to that pin); one removed with its pin is reported as such.
pub(super) fn still_detached(sheet: &Schematic, d: &Detached) -> bool {
    if d.removed {
        return true;
    }
    let anchor = match d.kind {
        ItemKind::Wire => sheet.wires.iter().find(|w| w.id == d.id).map(|w| if d.end == Some(WireEnd::To) { &w.to } else { &w.from }),
        ItemKind::Label => sheet.labels.iter().find(|l| l.id == d.id).map(|l| &l.at),
        ItemKind::Marker => sheet.markers.iter().find(|m| m.id == d.id).map(|m| &m.at),
        ItemKind::Part | ItemKind::Frame => None,
    };
    anchor.is_some_and(|a| a.point().is_some())
}

/// A sheet object, to tell whether it changed.
#[derive(PartialEq)]
enum Object<'a> {
    Part(&'a Part),
    Wire(&'a Wire),
    Label(&'a Label),
    Marker(&'a Marker),
    Frame(&'a Frame),
}

/// Every sheet object in sheet order, with how replies name it.
fn objects(sheet: &Schematic) -> Vec<(ItemOut, Object<'_>)> {
    let named = |id, kind, name: Option<&str>| ItemOut { id, kind, name: name.map(str::to_string), net: None };
    let parts = sheet.parts.iter().map(|p| (named(p.id, KindOut::Part, Some(&p.designator)), Object::Part(p)));
    let wires = sheet.wires.iter().map(|w| (named(w.id, KindOut::Wire, None), Object::Wire(w)));
    let labels = sheet.labels.iter().map(|l| (named(l.id, KindOut::Label, Some(&l.name)), Object::Label(l)));
    let markers = sheet.markers.iter().map(|m| (named(m.id, KindOut::Marker, None), Object::Marker(m)));
    let frames = sheet.frames.iter().map(|f| (named(f.id, KindOut::Frame, Some(&f.title)), Object::Frame(f)));
    parts.chain(wires).chain(labels).chain(markers).chain(frames).collect()
}

/// What a call did to the sheet, from the sheet before it (`old`) to the one stored now.
pub(super) fn sheet_result(s: &Session, old: &Schematic, detached: &[Detached], renames: Vec<RenameOut>) -> SheetResult {
    let new = &s.doc.ecad.schematic;
    let before: BTreeMap<u64, Object> = objects(old).into_iter().map(|(item, o)| (item.id, o)).collect();
    let after = objects(new);
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
    let removed = objects(old).into_iter().filter(|(item, _)| !ids.contains(&item.id)).map(|(item, _)| item).collect();
    let library = s.doc.ecad.library();
    let parts = new.parts.iter().filter(|p| shown.contains(&p.id)).map(|p| part_record(&library, p)).collect();
    let detached = detached.iter().filter(|d| still_detached(new, d)).map(detached_out).collect();
    SheetResult { created, changed, removed, parts, detached, renames, param_warnings: Vec::new(), netlist_changed: None }
}

#[cfg(test)]
#[path = "ecad_schematic_tests.rs"]
mod tests;
