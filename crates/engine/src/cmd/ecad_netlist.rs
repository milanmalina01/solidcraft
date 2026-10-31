//! The schematic's nets read back as data: each net's pins, wires, labels and markers, and what is
//! left unconnected; and the netlist written out as a file (`solidcraft_io::netlist`). Worked out
//! from the sheet on every call (see `solidcraft_doc::ecad::connect`); never saved.

use serde_json::Value;
use solidcraft_doc::ecad::connect::{Connectivity, Dangling, Net, NetPin, PinOnWire};
use solidcraft_doc::ecad::schematic::Pins;
use solidcraft_doc::ecad::{Library, Schematic, WireEnd};

use super::CommandSpec;
use super::ecad::{closest, electrical_out, parse, reply};
use super::ecad_schematic::{Index, find_pin};
use crate::params::bad;
use crate::schema::ecad_netlist::{
    DanglingOut, ExportParams, ExportResult, InspectParams, InspectResult, NetOut, NetPinOut, NetlistFormat, PinOnWireOut,
};
use crate::schema::ecad_schematic::{PinRef, WireEnd as WireEndOut};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.netlist.inspect", "Inspect Netlist", crate::checked!("ecad.netlist.inspect", inspect))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&INSPECT_CONTRACT)
        .output(output_schema_for::<InspectResult>)
        .params(
            "net? (a net's name, such as GND or net:R1.2, or a pin on it, such as R1.2) → {nets: [{name, named, names, power, pins: [{ref, part, pin, name, type, at}], wires, labels, markers}], unconnected: [{ref, …}], pin_on_wire: [{ref, part, pin, wire, at}], dangling: [{wire, end, at}], junctions: [[x, y]], truncated?}. Wires join at their ends and bends and where one's end or bend lies inside another (a T); pins that touch join; labels and power ports with the same name join. Wires that only cross don't join, and neither does a pin inside a wire (pin_on_wire). An unnamed net is called after its lowest pin (net:R1.2). Read-only",
        ),
    CommandSpec::new("ecad.netlist.export", "Export Netlist", crate::checked!("ecad.netlist.export", export))
        .icon("export")
        .noundo()
        .doc_only()
        .contract(&EXPORT_CONTRACT)
        .output(output_schema_for::<ExportResult>)
        .params(
            "format? json (the default), path? (a .json file to write; left out, the reply carries the text) → {format, text?, path?, nets, parts, truncated?}. The JSON: {format: solidcraft.netlist, version: 1, parts: [{id, designator, component, value, footprint, dnp}], nets: [{name, named, nodes: [{ref, part, pin, name, type, pads}]}]}, in natural order (R2 before R10); pads are the footprint pads each pin connects to. Nets with no pins are left out. Changes nothing in the design",
        ),
];

const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();
const EXPORT_CONTRACT: TypedContract<ExportParams> = TypedContract::new();

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.netlist.export";
    let p: ExportParams = parse(CMD, p)?;
    let format = p.format.unwrap_or(NetlistFormat::Json);
    let n = solidcraft_io::netlist::netlist_json(&s.doc.ecad).map_err(|e| bad(CMD, e))?;
    let path = match p.path {
        Some(path) if !path.to_ascii_lowercase().ends_with(".json") => {
            return Err(bad(CMD, format!("`path` must name a .json file (got `{path}`); leave it out for the text")));
        }
        Some(path) => {
            solidcraft_io::vfs::write(&path, n.text.as_bytes()).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
            Some(path)
        }
        None => None,
    };
    let text = path.is_none().then_some(n.text);
    reply(ExportResult { format, text, path, nets: n.nets, parts: n.parts, truncated: n.truncated, param_warnings: Vec::new() })
}

fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.netlist.inspect";
    let p: InspectParams = parse(CMD, p)?;
    let sheet = &s.doc.ecad.schematic;
    let library = s.doc.ecad.library();
    let c = solidcraft_doc::ecad::connectivity(sheet, &library);
    let Some(name) = p.net else {
        return reply(InspectResult {
            nets: c.nets.iter().map(net_out).collect(),
            unconnected: c.unconnected.iter().map(pin_out).collect(),
            pin_on_wire: c.pin_on_wire.iter().map(pin_on_wire_out).collect(),
            dangling: c.dangling.iter().map(dangling_out).collect(),
            junctions: c.junctions.clone(),
            truncated: c.truncated,
            param_warnings: Vec::new(),
        });
    };
    let k = find_net(sheet, &library, &c, &name, CMD, "`net`")?;
    let Some(net) = c.nets.get(k) else { return Err(bad(CMD, format!("`net`: no net is named `{name}`"))) };
    let on = |w: u64| net.wires.binary_search(&w).is_ok();
    reply(InspectResult {
        nets: vec![net_out(net)],
        unconnected: Vec::new(),
        pin_on_wire: c.pin_on_wire.iter().filter(|x| on(x.wire)).map(pin_on_wire_out).collect(),
        dangling: c.dangling.iter().filter(|d| on(d.wire)).map(dangling_out).collect(),
        junctions: Vec::new(),
        truncated: c.truncated,
        param_warnings: Vec::new(),
    })
}

/// The index of the net `name` names: a net's name, or a pin on it (`R1.2`). `what` names it in
/// errors.
pub(super) fn find_net(sheet: &Schematic, library: &Library, c: &Connectivity, name: &str, cmd: &str, what: &str) -> Result<usize> {
    if let Some(k) = c.index_of(name) {
        return Ok(k);
    }
    if !name.starts_with("net:") && name.contains('.') {
        let index = Index::new(sheet);
        let pins = Pins::new(sheet, library);
        if let Ok((part, pin)) = find_pin(sheet, &index, &pins, library, &PinRef::Text(name.to_string()), cmd, what) {
            return c.pin_net(part, &pin).ok_or_else(|| bad(cmd, format!("{what}: {name} touches nothing, so it is on no net")));
        }
    }
    let mut msg = format!("{what}: no net is named `{name}`");
    if let Some(near) = closest(name, c.nets.iter().map(|n| n.name.as_str())) {
        msg.push_str(&format!(" (did you mean `{near}`?)"));
    } else if c.nets.is_empty() {
        msg.push_str(" (the schematic has no nets yet: wire pins with ecad.schematic.wire or ecad.schematic.connect)");
    } else {
        msg.push_str(" (ecad.netlist.inspect lists the nets)");
    }
    Err(bad(cmd, msg))
}

pub(super) fn pin_out(p: &NetPin) -> NetPinOut {
    NetPinOut { reference: p.reference(), part: p.part, pin: p.pin.clone(), name: p.name.clone(), electrical: electrical_out(p.electrical), at: p.at }
}

pub(super) fn net_out(n: &Net) -> NetOut {
    NetOut {
        name: n.name.clone(),
        named: n.named,
        names: n.names.clone(),
        power: n.power,
        pins: n.pins.iter().map(pin_out).collect(),
        wires: n.wires.clone(),
        labels: n.labels.clone(),
        markers: n.markers.clone(),
    }
}

fn pin_on_wire_out(x: &PinOnWire) -> PinOnWireOut {
    PinOnWireOut { reference: x.pin.reference(), part: x.pin.part, pin: x.pin.pin.clone(), wire: x.wire, at: x.pin.at }
}

fn dangling_out(d: &Dangling) -> DanglingOut {
    DanglingOut {
        wire: d.wire,
        end: match d.end {
            WireEnd::From => WireEndOut::From,
            WireEnd::To => WireEndOut::To,
        },
        at: d.at,
    }
}
