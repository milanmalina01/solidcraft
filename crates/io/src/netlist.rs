//! The schematic's netlist as a file: SolidCraft's own JSON (`solidcraft.netlist`, version 1),
//! for other tools and for checking a design in git. Its parts with what builds them, and each net
//! with its pins and the footprint pads each pin connects to, all in natural order (R2 before R10).
//!
//! ```json
//! {"format": "solidcraft.netlist", "version": 1,
//!  "parts": [{"id", "designator", "component", "value", "footprint", "dnp"}],
//!  "nets": [{"name", "named", "nodes": [{"ref", "part", "pin", "name", "type", "pads"}]}]}
//! ```
//!
//! A net with no pins (wires and labels only) is left out; pins that touch nothing are on no net.

use std::collections::BTreeMap;

use serde::Serialize;
use solidcraft_doc::ecad::connect::natural_cmp;
use solidcraft_doc::ecad::{Electronics, PinType};

/// The `format` the file declares.
pub const NETLIST_FORMAT: &str = "solidcraft.netlist";
/// The version of the format this build writes.
pub const NETLIST_VERSION: u32 = 1;

#[derive(Serialize)]
struct File<'a> {
    format: &'static str,
    version: u32,
    parts: Vec<PartRow<'a>>,
    nets: Vec<NetRow<'a>>,
}

#[derive(Serialize)]
struct PartRow<'a> {
    id: u64,
    designator: &'a str,
    /// The electronic component's name.
    component: &'a str,
    value: String,
    /// The footprint of the variant it is built with; `null` without one.
    footprint: Option<&'a str>,
    dnp: bool,
}

#[derive(Serialize)]
struct NetRow<'a> {
    name: &'a str,
    /// A label or power port gives the name (`false` for `net:R1.2`).
    named: bool,
    nodes: Vec<Node<'a>>,
}

#[derive(Serialize)]
struct Node<'a> {
    #[serde(rename = "ref")]
    reference: String,
    part: u64,
    pin: &'a str,
    name: &'a str,
    #[serde(rename = "type")]
    electrical: PinType,
    /// The footprint pads the pin connects to, by number.
    pads: Vec<&'a str>,
}

/// A netlist written out, with what it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Netlist {
    /// The JSON text, ending in a newline.
    pub text: String,
    pub nets: usize,
    pub parts: usize,
    /// The schematic was past a cap, so not every connection is in it.
    pub truncated: bool,
}

/// The netlist of `ecad`'s schematic as JSON (see the module notes).
pub fn netlist_json(ecad: &Electronics) -> Result<Netlist, String> {
    let sheet = &ecad.schematic;
    let c = ecad.connectivity();
    let mut parts: Vec<PartRow> = Vec::with_capacity(sheet.parts.len());
    // Each part's pins with the pads they connect to, by part id.
    let mut pads: BTreeMap<u64, BTreeMap<&str, Vec<&str>>> = BTreeMap::new();
    for p in &sheet.parts {
        let Some(component) = ecad.component(p.component) else { continue };
        let r = ecad.resolve(component, p.variant.as_deref());
        let footprint = r.variant.and_then(|v| v.footprint).map(|_| r.footprint.name.as_str());
        parts.push(PartRow {
            id: p.id,
            designator: &p.designator,
            component: &component.name,
            value: p.value.clone().unwrap_or_else(|| component.value.clone()),
            footprint,
            dnp: p.dnp,
        });
        pads.insert(p.id, r.connections().into_iter().collect());
    }
    parts.sort_by(|a, b| natural_cmp(a.designator, b.designator));
    let nets: Vec<NetRow> = c
        .nets
        .iter()
        .filter(|n| !n.pins.is_empty())
        .map(|n| NetRow {
            name: &n.name,
            named: n.named,
            nodes: n
                .pins
                .iter()
                .map(|p| Node {
                    reference: p.reference(),
                    part: p.part,
                    pin: &p.pin,
                    name: &p.name,
                    electrical: p.electrical,
                    pads: pads.get(&p.part).and_then(|pins| pins.get(p.pin.as_str())).cloned().unwrap_or_default(),
                })
                .collect(),
        })
        .collect();
    let (net_count, part_count) = (nets.len(), parts.len());
    let file = File { format: NETLIST_FORMAT, version: NETLIST_VERSION, parts, nets };
    let mut text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    text.push('\n');
    Ok(Netlist { text, nets: net_count, parts: part_count, truncated: c.truncated })
}
