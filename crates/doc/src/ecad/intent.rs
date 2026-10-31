//! The design's intent: what the netlist alone doesn't say about the board, saved under
//! `ecad.intent` beside the schematic and the board. Which nets are supplies (at what voltage
//! and current), ground, clocks, RF lines or interface buses, which parts decouple which pins,
//! which are crystals, ESD protection, regulators and their loops, which connectors carry a
//! cable, and facts about the whole board (prototype or production, the radio bands it works
//! in). The checks that need such roles read them from here.
//!
//! The engine proposes most of it from the schematic and the library (see [`super::infer`]);
//! nothing is kept until it is accepted or declared. Each record says where it comes from
//! (`declared` by the caller, `inferred` and accepted as proposed, `confirmed` after a look at
//! the evidence), who made it, and the evidence. Records name nets by name and parts by
//! designator, as the schematic shows them; a name the schematic doesn't have (yet) is kept, and
//! listed as unknown.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::schematic::Actor;
use super::{MAX_DESCRIPTION, MAX_TEXT, check_text};

/// The most net, part, pair, loop and domain records, each.
pub const MAX_RECORDS: usize = 16384;
/// The most names one record lists (a loop's members, a part's `of`, a domain's parts).
pub const MAX_MEMBERS: usize = 64;
/// The most pieces of evidence one record keeps.
pub const MAX_EVIDENCE: usize = 8;
/// The most radio bands a board lists.
pub const MAX_BANDS: usize = 16;
/// The longest name (a net, a designator, a pin as `U1.VDD`), in characters.
pub const MAX_NAME: usize = 128;
/// The highest voltage, current, frequency and power a record takes.
pub const MAX_VOLTS: f64 = 100_000.0;
pub const MAX_AMPS: f64 = 10_000.0;
pub const MAX_MHZ: f64 = 1_000_000.0;
pub const MAX_WATTS: f64 = 10_000.0;

/// The design's intent (see the module notes); nothing is saved while it is empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Intent {
    #[serde(skip_serializing_if = "BoardFacts::is_empty")]
    pub board: BoardFacts,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub nets: Vec<NetIntent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<PairIntent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PartIntent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub loops: Vec<LoopIntent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<Domain>,
}

/// Where a record comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Given by the caller.
    #[default]
    Declared,
    /// Proposed by the engine and accepted as it was, unlooked at (the fast path for roles that
    /// risk little).
    Inferred,
    /// Proposed by the engine and accepted after a look at the evidence.
    Confirmed,
}

impl Source {
    /// Declared or confirmed: what a blocking role needs.
    pub fn settled(self) -> bool {
        self != Source::Inferred
    }

    pub fn name(self) -> &'static str {
        match self {
            Source::Declared => "declared",
            Source::Inferred => "inferred",
            Source::Confirmed => "confirmed",
        }
    }
}

/// How sure a proposal is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn name(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }
}

/// Whether the board is a prototype or for production (production makes some checks stricter).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Product {
    Prototype,
    Production,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Enclosure {
    None,
    Plastic,
    Metal,
}

/// A radio band the board works in, such as EU868: 863–870 MHz.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Band {
    pub name: String,
    pub lo_mhz: f64,
    pub hi_mhz: f64,
    /// It receives there (a receiver is what nearby noise desensitises).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rx: bool,
}

/// Facts about the whole board; each left out until known.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BoardFacts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product: Option<Product>,
    /// IPC class 2 or 3.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipc_class: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enclosure: Option<Enclosure>,
    /// The highest frequency of concern for emissions, in MHz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub f_concern_mhz: Option<f64>,
    /// The radio bands it works in; `Some([])` says it has no radio.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bands: Option<Vec<Band>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hand_assembly: Option<bool>,
    /// Who set them last.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<Actor>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl BoardFacts {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// What a net is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetRole {
    Power,
    Ground,
    Rf,
    /// One line of a differential pair (the pair is its own record).
    Diff,
    Clock,
    Strobe,
    Fast,
    /// A bus line of an interface (`interface` says which).
    Interface,
    Analog,
    Sensitive,
    Slow,
    /// Held at one level: a strap, a programming resistor.
    Static,
    Hv,
    /// A switching regulator's switch node.
    SwNode,
    /// A regulator's feedback.
    Fb,
    Sense,
}

impl NetRole {
    pub fn name(self) -> &'static str {
        match self {
            NetRole::Power => "power",
            NetRole::Ground => "ground",
            NetRole::Rf => "rf",
            NetRole::Diff => "diff",
            NetRole::Clock => "clock",
            NetRole::Strobe => "strobe",
            NetRole::Fast => "fast",
            NetRole::Interface => "interface",
            NetRole::Analog => "analog",
            NetRole::Sensitive => "sensitive",
            NetRole::Slow => "slow",
            NetRole::Static => "static",
            NetRole::Hv => "hv",
            NetRole::SwNode => "sw_node",
            NetRole::Fb => "fb",
            NetRole::Sense => "sense",
        }
    }

    /// A role the placement and routing gates need confirmed or declared.
    pub fn blocking(self) -> bool {
        matches!(self, NetRole::Power | NetRole::Ground | NetRole::Clock | NetRole::Rf | NetRole::Hv | NetRole::Diff | NetRole::Interface)
    }
}

/// An interface a net or a pair belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interface {
    Usb2Hs,
    Usb2Fs,
    Qspi,
    Spi,
    I2c,
    Uart,
    Rs485,
    Swd,
    Jtag,
    Can,
    I2s,
    Sdio,
}

impl Interface {
    pub fn name(self) -> &'static str {
        match self {
            Interface::Usb2Hs => "usb2_hs",
            Interface::Usb2Fs => "usb2_fs",
            Interface::Qspi => "qspi",
            Interface::Spi => "spi",
            Interface::I2c => "i2c",
            Interface::Uart => "uart",
            Interface::Rs485 => "rs485",
            Interface::Swd => "swd",
            Interface::Jtag => "jtag",
            Interface::Can => "can",
            Interface::I2s => "i2s",
            Interface::Sdio => "sdio",
        }
    }
}

/// What drives a net, for its edge rate when none is declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Driver {
    Standard,
    High,
    Lvds,
    Serdes,
    Clock,
}

/// A supply's voltage, in volts.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Voltage {
    pub nominal: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak: Option<f64>,
}

/// What a net is for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetIntent {
    pub net: String,
    pub role: NetRole,
    /// The most current it carries, in amperes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_a: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voltage: Option<Voltage>,
    /// Its edges' rise time, in nanoseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_ns: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<Driver>,
    /// The impedance it is routed for, in ohms, and the tolerance as a share (0.1 for ±10 %).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z_target: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z_tol: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<Interface>,
    /// Its frequency of concern (an RF line's carrier, a clock's), in MHz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub f_mhz: Option<f64>,
    /// It leaves the board (through a connector).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default)]
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Actor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// What a part is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartRole {
    /// A capacitor decoupling the IC power pin (or pins) in `of`.
    Decoupling,
    /// A larger capacitor holding up a rail.
    Bulk,
    Crystal,
    /// A crystal's load capacitor; `of` names the crystal.
    LoadCap,
    /// Protection of the connector pins in `of`.
    Tvs,
    EsdArray,
    SeriesTerm,
    ParallelTerm,
    /// A power IC that is no switching regulator: a charger, a power-path or power-management
    /// IC, a power switch, a linear regulator.
    PowerIc,
    SmpsIc,
    SmpsInCap,
    SmpsOutCap,
    SmpsInductor,
    SmpsDiode,
    FbDivider,
    SenseResistor,
    Ferrite,
    Filter,
    /// `cable` says whether a cable plugs into it.
    Connector,
    Antenna,
    RfIc,
    RfMatch,
    Module,
    TestPoint,
    Debug,
    Fiducial,
    Mounting,
    /// It runs hot (`power_w`).
    Hot,
    Sensitive,
    /// Placed for mechanical reasons, so it stays where it is.
    Mechanical,
}

impl PartRole {
    pub fn name(self) -> &'static str {
        match self {
            PartRole::Decoupling => "decoupling",
            PartRole::Bulk => "bulk",
            PartRole::Crystal => "crystal",
            PartRole::LoadCap => "load_cap",
            PartRole::Tvs => "tvs",
            PartRole::EsdArray => "esd_array",
            PartRole::SeriesTerm => "series_term",
            PartRole::ParallelTerm => "parallel_term",
            PartRole::PowerIc => "power_ic",
            PartRole::SmpsIc => "smps_ic",
            PartRole::SmpsInCap => "smps_in_cap",
            PartRole::SmpsOutCap => "smps_out_cap",
            PartRole::SmpsInductor => "smps_inductor",
            PartRole::SmpsDiode => "smps_diode",
            PartRole::FbDivider => "fb_divider",
            PartRole::SenseResistor => "sense_resistor",
            PartRole::Ferrite => "ferrite",
            PartRole::Filter => "filter",
            PartRole::Connector => "connector",
            PartRole::Antenna => "antenna",
            PartRole::RfIc => "rf_ic",
            PartRole::RfMatch => "rf_match",
            PartRole::Module => "module",
            PartRole::TestPoint => "test_point",
            PartRole::Debug => "debug",
            PartRole::Fiducial => "fiducial",
            PartRole::Mounting => "mounting",
            PartRole::Hot => "hot",
            PartRole::Sensitive => "sensitive",
            PartRole::Mechanical => "mechanical",
        }
    }

    /// A role the gates need confirmed or declared: what an IC's power pins hang on, the
    /// crystal, the protection and what leaves the board, the antenna.
    pub fn blocking(self) -> bool {
        matches!(
            self,
            PartRole::Decoupling
                | PartRole::Crystal
                | PartRole::Tvs
                | PartRole::EsdArray
                | PartRole::Connector
                | PartRole::Antenna
                | PartRole::SmpsIc
        )
    }
}

/// What a part is for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartIntent {
    /// Its designator.
    pub part: String,
    pub role: PartRole,
    /// What it serves: pins (`U1.VDD` by name, or `U1.B1` by number), parts or nets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub of: Vec<String>,
    /// A connector: whether a cable plugs into it in the product.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_w: Option<f64>,
    /// The loop it is a member of, by name.
    #[serde(default, rename = "loop", skip_serializing_if = "Option::is_none")]
    pub in_loop: Option<String>,
    #[serde(default)]
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Actor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// A differential pair: its positive and negative nets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairIntent {
    pub name: String,
    pub p: String,
    pub n: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<Interface>,
    /// The most length difference allowed, in millimetres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skew_mm: Option<f64>,
    #[serde(default)]
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Actor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// What a current loop is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopKind {
    /// A buck stage's input loop: the input capacitor and the switches.
    BuckHot,
    /// A boost stage's output loop: the switch, the rectifier and the output capacitor.
    BoostHot,
    Gate,
    Decap,
    Crystal,
    Tvs,
    Sense,
}

impl LoopKind {
    pub fn name(self) -> &'static str {
        match self {
            LoopKind::BuckHot => "buck_hot",
            LoopKind::BoostHot => "boost_hot",
            LoopKind::Gate => "gate",
            LoopKind::Decap => "decap",
            LoopKind::Crystal => "crystal",
            LoopKind::Tvs => "tvs",
            LoopKind::Sense => "sense",
        }
    }
}

/// A current loop to keep small: its members as pins (`C3.1`, `U2.VIN`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopIntent {
    pub name: String,
    pub kind: LoopKind,
    pub members: Vec<String>,
    /// How its area may compare with a vendor reference layout's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_ratio: Option<f64>,
    #[serde(default)]
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Actor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<Confidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainKind {
    Analog,
    Digital,
    Power,
    Rf,
    Isolated,
}

/// A group of parts that share a kind of circuit (an RF section, an isolated side).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Domain {
    pub name: String,
    pub kind: DomainKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<String>,
    #[serde(default)]
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Actor>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// A name a record gives (a net, a designator, a pin): 1 to [`MAX_NAME`] characters, no control
/// characters.
pub fn check_name(what: &str, name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err(format!("{what} can't be empty"));
    }
    check_text(what, name, MAX_NAME)
}

fn check_list(what: &str, names: &[String]) -> Result<(), String> {
    if names.len() > MAX_MEMBERS {
        return Err(format!("{what} lists at most {MAX_MEMBERS} names"));
    }
    for n in names {
        check_name(what, n)?;
    }
    Ok(())
}

fn check_evidence(evidence: &[String], note: &str) -> Result<(), String> {
    if evidence.len() > MAX_EVIDENCE {
        return Err(format!("a record keeps at most {MAX_EVIDENCE} pieces of `evidence`"));
    }
    for e in evidence {
        check_text("evidence", e, MAX_DESCRIPTION)?;
    }
    check_text("note", note, MAX_DESCRIPTION)
}

/// `v`, when given, finite and within `range`.
fn check_range(what: &str, v: Option<f64>, range: std::ops::RangeInclusive<f64>) -> Result<(), String> {
    if v.is_some_and(|v| !(v.is_finite() && range.contains(&v))) {
        return Err(format!("`{what}` must be {} to {}", range.start(), range.end()));
    }
    Ok(())
}

impl BoardFacts {
    pub fn validate(&self) -> Result<(), String> {
        if self.ipc_class.is_some_and(|c| !(2..=3).contains(&c)) {
            return Err("the board's `ipc_class` is 2 or 3".into());
        }
        check_range("f_concern_mhz", self.f_concern_mhz, 0.001..=MAX_MHZ)?;
        if let Some(bands) = &self.bands {
            if bands.len() > MAX_BANDS {
                return Err(format!("a board lists at most {MAX_BANDS} radio bands"));
            }
            for b in bands {
                check_text("band name", &b.name, MAX_TEXT)?;
                if b.name.trim().is_empty() {
                    return Err("a radio band needs a name, such as EU868".into());
                }
                check_range("lo_mhz", Some(b.lo_mhz), 0.001..=MAX_MHZ)?;
                check_range("hi_mhz", Some(b.hi_mhz), 0.001..=MAX_MHZ)?;
                if b.lo_mhz > b.hi_mhz {
                    return Err(format!("band {}: `lo_mhz` is above `hi_mhz`", b.name));
                }
            }
        }
        check_text("note", &self.note, MAX_DESCRIPTION)
    }
}

impl NetIntent {
    pub fn validate(&self) -> Result<(), String> {
        check_name("a net record's `net`", &self.net)?;
        let at = |e: String| format!("net {}: {e}", self.net);
        check_range("current_a", self.current_a, 0.0..=MAX_AMPS).map_err(at)?;
        if let Some(v) = self.voltage {
            check_range("voltage.nominal", Some(v.nominal), -MAX_VOLTS..=MAX_VOLTS).map_err(at)?;
            check_range("voltage.peak", v.peak, -MAX_VOLTS..=MAX_VOLTS).map_err(at)?;
        }
        check_range("edge_ns", self.edge_ns, 0.001..=1_000_000.0).map_err(at)?;
        check_range("z_target", self.z_target, 1.0..=1000.0).map_err(at)?;
        check_range("z_tol", self.z_tol, 0.0..=1.0).map_err(at)?;
        check_range("f_mhz", self.f_mhz, 0.0..=MAX_MHZ).map_err(at)?;
        if let Some(d) = &self.domain {
            check_name("`domain`", d).map_err(at)?;
        }
        check_evidence(&self.evidence, &self.note).map_err(at)
    }

    /// What a blocking role still lacks to be complete: a supply's voltage and current.
    pub fn needs(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.role == NetRole::Power {
            if self.voltage.is_none() {
                out.push("voltage");
            }
            if self.current_a.is_none() {
                out.push("current_a");
            }
        }
        out
    }
}

impl PartIntent {
    pub fn validate(&self) -> Result<(), String> {
        check_name("a part record's `part`", &self.part)?;
        let at = |e: String| format!("part {}: {e}", self.part);
        check_list("`of`", &self.of).map_err(at)?;
        check_range("power_w", self.power_w, 0.0..=MAX_WATTS).map_err(at)?;
        if let Some(l) = &self.in_loop {
            check_name("`loop`", l).map_err(at)?;
        }
        if self.cable.is_some() && self.role != PartRole::Connector {
            return Err(at("only a connector has `cable`".into()));
        }
        check_evidence(&self.evidence, &self.note).map_err(at)
    }

    /// What a blocking role still lacks: a connector's cable status, which only the product
    /// tells.
    pub fn needs(&self) -> Vec<&'static str> {
        if self.role == PartRole::Connector && self.cable.is_none() { vec!["cable"] } else { Vec::new() }
    }
}

impl PairIntent {
    pub fn validate(&self) -> Result<(), String> {
        check_name("a pair's `name`", &self.name)?;
        let at = |e: String| format!("pair {}: {e}", self.name);
        check_name("`p`", &self.p).map_err(at)?;
        check_name("`n`", &self.n).map_err(at)?;
        if self.p == self.n {
            return Err(at("`p` and `n` are the same net".into()));
        }
        check_range("skew_mm", self.skew_mm, 0.0..=1000.0).map_err(at)?;
        check_evidence(&self.evidence, &self.note).map_err(at)
    }
}

impl LoopIntent {
    pub fn validate(&self) -> Result<(), String> {
        check_name("a loop's `name`", &self.name)?;
        let at = |e: String| format!("loop {}: {e}", self.name);
        if self.members.len() < 2 {
            return Err(at("a loop has 2 or more `members`".into()));
        }
        check_list("`members`", &self.members).map_err(at)?;
        check_range("reference_ratio", self.reference_ratio, 0.0..=1000.0).map_err(at)?;
        check_evidence(&self.evidence, &self.note).map_err(at)
    }
}

impl Domain {
    pub fn validate(&self) -> Result<(), String> {
        check_name("a domain's `name`", &self.name)?;
        let at = |e: String| format!("domain {}: {e}", self.name);
        check_list("`parts`", &self.parts).map_err(at)?;
        check_text("note", &self.note, MAX_DESCRIPTION).map_err(at)
    }
}

impl Intent {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The caps, each record's rules, and one record per net, part, pair, loop and domain.
    pub fn validate(&self) -> Result<(), String> {
        for (what, n) in [
            ("net records", self.nets.len()),
            ("part records", self.parts.len()),
            ("pairs", self.pairs.len()),
            ("loops", self.loops.len()),
            ("domains", self.domains.len()),
        ] {
            if n > MAX_RECORDS {
                return Err(format!("the intent holds at most {MAX_RECORDS} {what}"));
            }
        }
        self.board.validate()?;
        fn unique<'a>(what: &str, names: impl Iterator<Item = &'a str>) -> Result<(), String> {
            let mut seen = BTreeSet::new();
            for n in names {
                if !seen.insert(n) {
                    return Err(format!("the intent has two {what} for `{n}`"));
                }
            }
            Ok(())
        }
        for r in &self.nets {
            r.validate()?;
        }
        for r in &self.parts {
            r.validate()?;
        }
        for r in &self.pairs {
            r.validate()?;
        }
        for r in &self.loops {
            r.validate()?;
        }
        for r in &self.domains {
            r.validate()?;
        }
        unique("net records", self.nets.iter().map(|r| r.net.as_str()))?;
        unique("part records", self.parts.iter().map(|r| r.part.as_str()))?;
        unique("pairs", self.pairs.iter().map(|r| r.name.as_str()))?;
        unique("loops", self.loops.iter().map(|r| r.name.as_str()))?;
        unique("domains", self.domains.iter().map(|r| r.name.as_str()))
    }

    /// The keys of every record kept: `net|X`, `part|C5`, `pair|USB`, `loop|U3.buck_hot`,
    /// `domain|RF`, and `board` once a board fact is.
    pub fn keys(&self) -> BTreeSet<String> {
        let nets = self.nets.iter().map(|r| format!("net|{}", r.net));
        let parts = self.parts.iter().map(|r| format!("part|{}", r.part));
        let pairs = self.pairs.iter().map(|r| format!("pair|{}", r.name));
        let loops = self.loops.iter().map(|r| format!("loop|{}", r.name));
        let domains = self.domains.iter().map(|r| format!("domain|{}", r.name));
        let board = (!self.board.is_empty()).then(|| "board".to_string());
        nets.chain(parts).chain(pairs).chain(loops).chain(domains).chain(board).collect()
    }

    pub fn net(&self, name: &str) -> Option<&NetIntent> {
        self.nets.iter().find(|r| r.net == name)
    }

    pub fn part(&self, designator: &str) -> Option<&PartIntent> {
        self.parts.iter().find(|r| r.part == designator)
    }

    pub fn pair(&self, name: &str) -> Option<&PairIntent> {
        self.pairs.iter().find(|r| r.name == name)
    }

    pub fn loop_named(&self, name: &str) -> Option<&LoopIntent> {
        self.loops.iter().find(|r| r.name == name)
    }
}
