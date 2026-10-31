//! Copy items from an electronics library into this document: the symbols, footprints, 3D
//! packages and electronic components given, as a library saves them. Each copy takes a new id
//! and keeps the references between the copies; a name already taken here gets a number. A record
//! already here with the same content and name (or that name numbered, an earlier copy's) is that
//! record, so copying a component again (or another that shares its footprint) copies nothing
//! twice.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::DeserializeOwned;
use serde_json::Value;
use solidcraft_doc::ecad::{ElectronicComponent, Electronics, FootprintItem, MAX_COMPONENTS, MAX_ITEMS, MAX_TEXT, PackageItem, SymbolItem};

use super::CommandSpec;
use super::ecad::{commit, item_id, package_missing, parse, reply};
use crate::params::bad;
use crate::schema::ecad_import::{ImportParams, ImportResult, ImportedOut};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("ecad.library.import", "Import from Library", crate::checked!("ecad.library.import", import))
    .icon("import")
    .doc_only()
    .contract(&IMPORT_CONTRACT)
    .output(output_schema_for::<ImportResult>)
    .params(
        "items: {symbols?, footprints?, packages?, components?} (records as a design file saves them under `ecad`, ids unique among them; a component's symbol, footprints and 3D packages must be among them) → {symbols, footprints, packages, components: [{from, to, name, reused?}], models_missing?}: copies them with new ids, the references between them kept; a name taken here gets a number (Resistor 2); one here with the same content and that name (or that name numbered, an earlier copy) is reused, not copied. A 3D package keeps its model, a SolidCraft component of the document it came from (models_missing lists those this one lacks). One undo step",
    )];

const IMPORT_CONTRACT: TypedContract<ImportParams> = TypedContract::new();

/// What the four kinds of record share for copying.
trait Record: Clone + PartialEq + DeserializeOwned {
    /// The key under `items`.
    const KEY: &'static str;
    fn id(&self) -> u64;
    fn name(&self) -> &str;
    /// The same record with another id and name.
    fn renamed(self, id: u64, name: String) -> Self;
    fn list(e: &mut Electronics) -> &mut Vec<Self>;
    fn validate(&self) -> std::result::Result<(), String>;
}

impl Record for SymbolItem {
    const KEY: &'static str = "symbols";
    fn id(&self) -> u64 {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn renamed(self, id: u64, name: String) -> Self {
        SymbolItem { id, name, ..self }
    }
    fn list(e: &mut Electronics) -> &mut Vec<Self> {
        &mut e.symbols
    }
    fn validate(&self) -> std::result::Result<(), String> {
        SymbolItem::validate(self)
    }
}

impl Record for FootprintItem {
    const KEY: &'static str = "footprints";
    fn id(&self) -> u64 {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn renamed(self, id: u64, name: String) -> Self {
        FootprintItem { id, name, ..self }
    }
    fn list(e: &mut Electronics) -> &mut Vec<Self> {
        &mut e.footprints
    }
    fn validate(&self) -> std::result::Result<(), String> {
        FootprintItem::validate(self)
    }
}

impl Record for PackageItem {
    const KEY: &'static str = "packages";
    fn id(&self) -> u64 {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn renamed(self, id: u64, name: String) -> Self {
        PackageItem { id, name, ..self }
    }
    fn list(e: &mut Electronics) -> &mut Vec<Self> {
        &mut e.packages
    }
    fn validate(&self) -> std::result::Result<(), String> {
        PackageItem::validate(self)
    }
}

impl Record for ElectronicComponent {
    const KEY: &'static str = "components";
    fn id(&self) -> u64 {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn renamed(self, id: u64, name: String) -> Self {
        ElectronicComponent { id, name, ..self }
    }
    fn list(e: &mut Electronics) -> &mut Vec<Self> {
        &mut e.components
    }
    fn validate(&self) -> std::result::Result<(), String> {
        ElectronicComponent::validate(self)
    }
}

/// The records under `items.<KEY>`, each read and checked as the design file reads it.
fn records<T: Record>(values: Vec<Value>, max: usize, cmd: &str) -> Result<Vec<T>> {
    if values.len() > max {
        return Err(bad(cmd, format!("`items.{}` holds at most {max} records", T::KEY)));
    }
    let mut out = Vec::with_capacity(values.len());
    for (i, v) in values.into_iter().enumerate() {
        let at = |e: String| bad(cmd, format!("`items.{}[{i}]`: {e}", T::KEY));
        let record: T = serde_path_to_error::deserialize(v).map_err(|e| at(e.to_string()))?;
        record.validate().map_err(at)?;
        out.push(record);
    }
    Ok(out)
}

/// `base n`, with `base` cut so it stays a valid name.
fn numbered(base: &str, n: usize) -> String {
    let tail = format!(" {n}");
    let head: String = base.chars().take(MAX_TEXT.saturating_sub(tail.chars().count())).collect();
    format!("{}{tail}", head.trim_end())
}

/// `base`, or `base 2`, `base 3`…, the first `taken` hasn't.
fn free_name(base: &str, taken: &BTreeSet<&str>) -> Option<String> {
    if !taken.contains(base) {
        return Some(base.to_string());
    }
    (2..=MAX_ITEMS.max(MAX_COMPONENTS) + 2).map(|n| numbered(base, n)).find(|name| !taken.contains(name.as_str()))
}

/// Whether `name` is `base`, or `base` numbered as [`free_name`] numbers it (a copy made before).
fn same_name(name: &str, base: &str) -> bool {
    name == base || name.rsplit_once(' ').and_then(|(_, n)| n.parse::<usize>().ok()).is_some_and(|n| n >= 2 && numbered(base, n) == name)
}

/// Copy `record` into `next`, or find it there: one with the same name (or that name numbered,
/// an earlier copy's) and the same content is it; otherwise the copy takes a new id and the first
/// free name after its own.
fn copy_into<T: Record>(next: &mut Electronics, record: T, cmd: &str) -> Result<ImportedOut> {
    let from = record.id();
    if let Some(here) = T::list(next)
        .iter()
        .filter(|x| same_name(x.name(), record.name()))
        .find(|here| record.clone().renamed(here.id(), here.name().to_string()) == **here)
    {
        return Ok(ImportedOut { from, to: here.id(), name: here.name().to_string(), reused: true });
    }
    let name = {
        let list = T::list(next);
        let taken: BTreeSet<&str> = list.iter().map(Record::name).collect();
        free_name(record.name(), &taken).ok_or_else(|| bad(cmd, format!("no free name is left for `{}`", record.name())))?
    };
    let to = next.take_id().map_err(|e| bad(cmd, e))?;
    T::list(next).push(record.renamed(to, name.clone()));
    Ok(ImportedOut { from, to, name, reused: false })
}

/// The new id of the item `id` names, among those copied as `kind`; an error names the component.
fn remapped(ids: &BTreeMap<u64, u64>, id: u64, kind: &str, key: &str, component: &str, cmd: &str) -> Result<u64> {
    ids.get(&id).copied().ok_or_else(|| bad(cmd, format!("`items.components`: `{component}`: its {kind} {id} is not among `items.{key}`")))
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.library.import";
    let items = parse::<ImportParams>(CMD, p)?.items;
    let symbols: Vec<SymbolItem> = records(items.symbols, MAX_ITEMS, CMD)?;
    let footprints: Vec<FootprintItem> = records(items.footprints, MAX_ITEMS, CMD)?;
    let packages: Vec<PackageItem> = records(items.packages, MAX_ITEMS, CMD)?;
    let components: Vec<ElectronicComponent> = records(items.components, MAX_COMPONENTS, CMD)?;
    if symbols.is_empty() && footprints.is_empty() && packages.is_empty() && components.is_empty() {
        return Err(bad(CMD, "give at least one record in `items`"));
    }
    // The given ids are one id space, as in the library they came from: the references need it.
    let mut given = BTreeSet::new();
    let ids = symbols.iter().map(|x| x.id).chain(footprints.iter().map(|x| x.id)).chain(packages.iter().map(|x| x.id));
    for id in ids.chain(components.iter().map(|x| x.id)) {
        if !given.insert(id) {
            return Err(bad(CMD, format!("`items`: two records have the id {id}")));
        }
    }
    let mut next = s.doc.ecad.clone();
    let mut out = ImportResult::default();
    let map = |rows: &[ImportedOut]| -> BTreeMap<u64, u64> { rows.iter().map(|r| (r.from, r.to)).collect() };
    for x in symbols {
        out.symbols.push(copy_into(&mut next, x, CMD)?);
    }
    for x in footprints {
        out.footprints.push(copy_into(&mut next, x, CMD)?);
    }
    for x in packages {
        out.packages.push(copy_into(&mut next, x, CMD)?);
    }
    let (symbol_ids, footprint_ids, package_ids) = (map(&out.symbols), map(&out.footprints), map(&out.packages));
    for mut c in components {
        let name = c.name.clone();
        if let Some(id) = c.symbol {
            c.symbol = Some(remapped(&symbol_ids, id, "symbol", "symbols", &name, CMD)?);
        }
        for v in &mut c.variants {
            if let Some(id) = v.footprint {
                v.footprint = Some(remapped(&footprint_ids, id, "footprint", "footprints", &name, CMD)?);
            }
            if let Some(id) = v.package {
                v.package = Some(remapped(&package_ids, id, "3D package", "packages", &name, CMD)?);
            }
        }
        out.components.push(copy_into(&mut next, c, CMD)?);
    }
    commit(s, next, CMD)?;
    out.models_missing = out
        .packages
        .iter()
        .filter(|row| !row.reused)
        .filter_map(|row| s.doc.ecad.package(row.to))
        .filter(|p| package_missing(s, Some(p)))
        .map(|p| item_id(p.id, &p.name))
        .collect();
    reply(out)
}
