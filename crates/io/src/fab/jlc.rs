//! The assembly files a board house's assembly service asks for, in the columns JLCPCB's
//! upload page takes: the parts list (Comment, Designator, Footprint, LCSC Part #), one row per
//! value, footprint and LCSC part, and the placement list (Designator, Mid X, Mid Y, Layer,
//! Rotation), one row per placed footprint, its rotation the board's plus the footprint's offset
//! from the board's table (a library footprint drawn turned from the fab's reference for it).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use solidcraft_doc::ecad::Electronics;
use solidcraft_doc::ecad::footprint::Side;

/// A CSV field, quoted when it needs to be.
fn field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

fn number(v: f64) -> String {
    let s = format!("{:.4}", (v * 1e4).round() / 1e4 + 0.0);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_string() }
}

/// Designators in natural order: R2 before R10.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let split = |s: &str| {
        let i = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
        let (p, n) = s.split_at(i);
        (p.to_string(), n.parse::<u64>().unwrap_or(0))
    };
    split(a).cmp(&split(b)).then_with(|| a.cmp(b))
}

/// One parts list row.
#[derive(Clone, Debug, PartialEq)]
pub struct BomRow {
    pub comment: String,
    pub designators: Vec<String>,
    pub footprint: String,
    pub lcsc: String,
}

/// One placement row.
#[derive(Clone, Debug, PartialEq)]
pub struct CplRow {
    pub designator: String,
    pub at: [f64; 2],
    pub bottom: bool,
    pub rotation: f64,
}

/// The parts list: the board's footprints grouped by value, footprint and LCSC part.
pub fn bom(ecad: &Electronics) -> Vec<BomRow> {
    let mut groups: BTreeMap<(String, String, String), Vec<String>> = BTreeMap::new();
    for f in &ecad.board.footprints {
        let Some(p) = ecad.placed(f) else { continue };
        if p.part.dnp {
            continue;
        }
        let c = p.resolved.component;
        let value = p.part.value.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| c.value.clone());
        let comment = if value.is_empty() { c.name.clone() } else { value };
        groups.entry((comment, p.resolved.footprint.name.clone(), c.lcsc.clone())).or_default().push(p.part.designator.clone());
    }
    let mut rows: Vec<BomRow> = groups
        .into_iter()
        .map(|((comment, footprint, lcsc), mut designators)| {
            designators.sort_by(|a, b| natural(a, b));
            BomRow { comment, designators, footprint, lcsc }
        })
        .collect();
    rows.sort_by(|a, b| natural(a.designators.first().map_or("", String::as_str), b.designators.first().map_or("", String::as_str)));
    rows
}

/// The placement list: each placed part but those marked not to populate, at the middle of its
/// footprint's pads (the land pattern's centre, which assemblers place by; a module's origin may be
/// elsewhere on its body), turned by its rotation plus its library footprint's assembly offset, or
/// the one `overrides` gives for that footprint's name.
pub fn cpl(ecad: &Electronics, overrides: &BTreeMap<String, f64>) -> Vec<CplRow> {
    let mut rows: Vec<CplRow> = ecad
        .board
        .footprints
        .iter()
        .filter_map(|f| {
            let p = ecad.placed(f)?;
            if p.part.dnp {
                return None;
            }
            let fp = p.resolved.footprint;
            let offset = overrides.get(&fp.name).copied().unwrap_or(fp.assembly_rotation);
            let rotation = (f64::from(f.rotation) + offset).rem_euclid(360.0);
            let centre = fp.copper_bbox().map_or([0.0, 0.0], |[lo, hi]| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]);
            let at = f.placement().apply(centre);
            Some(CplRow { designator: p.part.designator.clone(), at, bottom: f.side == Side::Bottom, rotation })
        })
        .collect();
    rows.sort_by(|a, b| natural(&a.designator, &b.designator));
    rows
}

pub fn bom_csv(rows: &[BomRow]) -> String {
    let mut out = String::from("Comment,Designator,Footprint,LCSC Part #\n");
    for r in rows {
        let _ = writeln!(out, "{},{},{},{}", field(&r.comment), field(&r.designators.join(",")), field(&r.footprint), field(&r.lcsc));
    }
    out
}

pub fn cpl_csv(rows: &[CplRow]) -> String {
    let mut out = String::from("Designator,Mid X,Mid Y,Layer,Rotation\n");
    for r in rows {
        let _ = writeln!(
            out,
            "{},{}mm,{}mm,{},{}",
            field(&r.designator),
            number(r.at[0]),
            number(r.at[1]),
            if r.bottom { "Bottom" } else { "Top" },
            number(r.rotation)
        );
    }
    out
}
