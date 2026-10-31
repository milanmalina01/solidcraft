//! What the engine proposes for the design's intent (see [`super::intent`]), from the schematic,
//! the library and, once parts are placed, the board: each proposal a record as it would be
//! kept, the evidence it rests on, and how sure it is. Nothing here is saved; a proposal is kept
//! only when it is accepted (`ecad.intent.set`).
//!
//! What it reads: ground and rail ports and net names (`GND`, `+3V3`, `VBUS`), pin types and
//! names (power pins, `VIN`, `FB`, `ANT`, `SCK`, `D+`), designator prefixes (`C`, `L`, `Y`, `J`,
//! `ANT`), values (`100nF`, `2.2nH`, `0R`) and the components' descriptions (`buck-boost`,
//! `ESD`, `antenna`, `LoRa`). A capacitor from a supply to ground serves the IC supply pin or
//! power output (a regulator's, a charger's) nearest it (on the board once both are placed, else
//! on the sheet); one of 4.7 µF or more is bulk. A pin named as a DC/DC switch pin (`DCC`, `SW`,
//! `LX`) whose net meets an inductor makes a switch node, also of an IC no description calls a
//! regulator (a radio's own DC/DC). Roles are blocking when the placement and routing checks
//! lean on them (supplies, ground, clocks, RF, pairs and buses, decoupling, crystals,
//! protection, connectors, antennas, regulators): those need a look before they are accepted. A
//! supply that only reaches supply-detect inputs (a USB device's `VBUS` pin) carries no load
//! current and is asked nothing. The rest is low risk and may be accepted in one go, but a
//! regulator's capacitors, inductor, switch nodes and loops only once the regulator is confirmed
//! (its topology is the datasheet's). A connector's cable is proposed when its kind tells (a USB
//! receptacle, a coaxial or a wire-to-board connector), and only a chip or a module is taken for
//! a radio.
//!
//! The work is bounded by the design's caps: each net and part is looked at a fixed number of
//! times, and the RF lines are followed through at most [`RF_HOPS`] parts.

use std::collections::{BTreeMap, BTreeSet};

use super::Electronics;
use super::airwires::footprint_pads;
use super::connect::{Connectivity, NetPin};
use super::intent::{
    Band, BoardFacts, Confidence, Intent, Interface, LoopIntent, LoopKind, NetIntent, NetRole, PairIntent, PartIntent, PartRole, Source, Voltage,
};
use super::schematic::LabelKind;
use super::symbol::PinType;

/// How many parts an RF line is followed through, from an antenna, an RF pin or an RF connector.
pub const RF_HOPS: usize = 6;
/// A capacitor this large or larger is bulk, not decoupling, in farads.
pub const BULK_FARADS: f64 = 4.7e-6;
/// The most current a net may carry and still be accepted unlooked at, in amperes.
pub const LOW_RISK_AMPS: f64 = 0.5;

/// A record as it would be kept.
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    Net(NetIntent),
    Part(PartIntent),
    Pair(PairIntent),
    Loop(LoopIntent),
    Board(BoardFacts),
}

impl Record {
    /// `net|GND`, `part|C5`, `pair|USB`, `loop|U3.buck_hot`, `board`: what it is about, as
    /// proposals and the records are keyed.
    pub fn key(&self) -> String {
        match self {
            Record::Net(r) => format!("net|{}", r.net),
            Record::Part(r) => format!("part|{}", r.part),
            Record::Pair(r) => format!("pair|{}", r.name),
            Record::Loop(r) => format!("loop|{}", r.name),
            Record::Board(_) => "board".to_string(),
        }
    }

    /// The role, for listings.
    pub fn role(&self) -> &'static str {
        match self {
            Record::Net(r) => r.role.name(),
            Record::Part(r) => r.role.name(),
            Record::Pair(_) => "pair",
            Record::Loop(r) => r.kind.name(),
            Record::Board(_) => "board",
        }
    }

    /// The record with its source and who accepted it set.
    pub fn accepted(mut self, source: Source, by: super::schematic::Actor) -> Self {
        match &mut self {
            Record::Net(r) => (r.source, r.by) = (source, Some(by)),
            Record::Part(r) => (r.source, r.by) = (source, Some(by)),
            Record::Pair(r) => (r.source, r.by) = (source, Some(by)),
            Record::Loop(r) => (r.source, r.by) = (source, Some(by)),
            Record::Board(r) => r.by = Some(by),
        }
        self
    }
}

/// One proposal.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    /// As [`Record::key`].
    pub key: String,
    pub record: Record,
    pub confidence: Confidence,
    pub evidence: Vec<String>,
    /// It needs a look before it is accepted (see the module notes).
    pub blocking: bool,
    /// What it still lacks that the files can't tell (a supply's current, whether a connector
    /// carries a cable, the radio band): asked of the user.
    pub needs: Vec<&'static str>,
    /// The proposal it stands or falls with (a regulator's parts and loops with the regulator's,
    /// `part|U3`): accepted in bulk only once that one is settled.
    pub after: Option<String>,
}

/// What [`infer`] found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inference {
    /// Board facts first, then nets, pairs, parts and loops, each in the schematic's order.
    pub proposals: Vec<Proposal>,
    /// The ICs' supply pins (`U1.B1`), which decoupling is assigned to.
    pub power_pins: Vec<String>,
}

impl Inference {
    pub fn get(&self, key: &str) -> Option<&Proposal> {
        self.proposals.iter().find(|p| p.key == key)
    }

    /// The proposals `ecad.intent.set {accept_low_risk}` accepts: not blocking, no record kept
    /// for their subject in `intent`, a net carrying at most [`LOW_RISK_AMPS`], and those that
    /// stand with another proposal (a regulator's parts) only once a settled record has that
    /// one's role.
    pub fn low_risk(&self, intent: &Intent) -> Vec<&Proposal> {
        let kept = intent.keys();
        let settled = |key: &str| {
            let role = self.get(key).map(|x| x.record.role());
            let (kind, name) = key.split_once('|').unwrap_or((key, ""));
            match kind {
                "net" => intent.net(name).is_some_and(|r| r.source.settled() && Some(r.role.name()) == role),
                "part" => intent.part(name).is_some_and(|r| r.source.settled() && Some(r.role.name()) == role),
                _ => false,
            }
        };
        self.proposals
            .iter()
            .filter(|x| !x.blocking && !kept.contains(&x.key))
            .filter(|x| match &x.record {
                Record::Net(n) => n.current_a.is_none_or(|a| a <= LOW_RISK_AMPS),
                Record::Board(_) => false,
                _ => true,
            })
            .filter(|x| x.after.as_deref().is_none_or(settled))
            .collect()
    }
}

/// A part as the inference sees it.
struct PartInfo {
    id: u64,
    designator: String,
    prefix: String,
    value: String,
    /// The component's name, description, part number and manufacturer, lowercase.
    text: String,
    pins: usize,
    dnp: bool,
}

impl PartInfo {
    fn is(&self, words: &[&str]) -> bool {
        words.iter().any(|w| self.text.contains(w))
    }

    /// Whether the text has one of `words` as a whole word (`ble`, not `cable` or `table`).
    fn has_word(&self, words: &[&str]) -> bool {
        self.text.split(|c: char| !c.is_alphanumeric()).any(|t| words.contains(&t))
    }

    fn two_pin(&self, prefixes: &[&str]) -> bool {
        self.pins == 2 && prefixes.contains(&self.prefix.as_str())
    }

    fn capacitor(&self) -> bool {
        self.two_pin(&["C"])
    }

    fn inductor(&self) -> bool {
        self.two_pin(&["L", "FB"])
    }

    /// An inductor a converter stores its energy in (a microhenry or more, or a value it can't
    /// read), not a ferrite bead or a small one for high frequencies in series with it.
    fn main_inductor(&self) -> bool {
        if self.prefix == "FB" {
            return false;
        }
        match parse_value(&self.value) {
            Some((v, 'H' | '?')) => v >= MAIN_HENRIES,
            _ => true,
        }
    }

    fn resistor(&self) -> bool {
        self.two_pin(&["R"])
    }

    /// A chip: an IC, a module, a regulator (not a passive, connector, crystal or LED).
    fn ic(&self) -> bool {
        matches!(self.prefix.as_str(), "U" | "IC" | "Q" | "M" | "MOD") || (self.pins > 4 && !self.connector() && !self.crystal())
    }

    fn connector(&self) -> bool {
        matches!(self.prefix.as_str(), "J" | "P" | "CN" | "CON" | "X") && !self.crystal() || self.is(&["connector", "receptacle", "header", "socket"])
    }

    fn crystal(&self) -> bool {
        matches!(self.prefix.as_str(), "Y" | "XTAL") || self.is(&["crystal", "resonator"])
    }

    /// An ESD or TVS part, by its text.
    fn protection(&self) -> bool {
        self.is(&["esd", "tvs", "transient", "suppressor", "usblc"])
    }

    /// What power IC its text says it is, in a few words: a charger, a power-path or
    /// power-management IC, a power switch, a linear regulator (a switching one is [`smps`]'s).
    fn power_kind(&self) -> Option<&'static str> {
        if self.is(&["charger"]) {
            Some("a battery charger")
        } else if self.is(&["power-path", "power path", "pmic", "power management", "battery management"]) {
            Some("a power-management IC")
        } else if self.is(&["load switch", "power switch", "ideal diode"]) {
            Some("a power switch")
        } else if self.has_word(&["ldo"]) || self.is(&["linear regulator"]) {
            Some("a linear regulator")
        } else if self.is(&["regulator"]) {
            Some("a regulator")
        } else {
            None
        }
    }

    fn mounted(&self) -> bool {
        !self.dnp && !self.value.eq_ignore_ascii_case("nc") && !self.value.eq_ignore_ascii_case("dnp")
    }
}

/// A value such as `100nF`, `4.7uF`, `2.2nH`, `0R`, `2R2`, `1k5`: the number in base units and
/// its unit letter (`F`, `H`, `R` for ohms; `?` when none is given).
pub fn parse_value(text: &str) -> Option<(f64, char)> {
    let token = text.split_whitespace().next()?;
    let multiplier = |c: char| -> Option<f64> {
        Some(match c {
            'p' => 1e-12,
            'n' => 1e-9,
            'u' | 'µ' => 1e-6,
            'm' => 1e-3,
            'k' | 'K' => 1e3,
            'M' => 1e6,
            'G' => 1e9,
            'R' | 'r' => 1.0,
            _ => return None,
        })
    };
    let chars: Vec<char> = token.chars().collect();
    let digits_end = chars.iter().position(|c| !(c.is_ascii_digit() || *c == '.')).unwrap_or(chars.len());
    let whole: String = chars.get(..digits_end)?.iter().collect();
    if whole.is_empty() {
        return None;
    }
    let rest = chars.get(digits_end..).unwrap_or_default();
    let mut value: f64 = whole.parse().ok()?;
    let mut unit = '?';
    let mut i = 0;
    if let Some(&c) = rest.first()
        && let Some(m) = multiplier(c)
    {
        // `2R2`, `1k5`, `4u7`: the letter is the decimal point.
        let frac_end = rest.iter().skip(1).position(|c| !c.is_ascii_digit()).map_or(rest.len(), |p| p + 1);
        let frac: String = rest.get(1..frac_end).unwrap_or_default().iter().collect();
        if !frac.is_empty() && !whole.contains('.') {
            value = format!("{whole}.{frac}").parse().ok()?;
        }
        value *= m;
        if c == 'R' || c == 'r' {
            unit = 'R';
        }
        i = frac_end;
    }
    match rest.get(i) {
        Some('F' | 'f') => unit = 'F',
        Some('H' | 'h') => unit = 'H',
        Some('Ω' | 'R') => unit = 'R',
        _ => {}
    }
    value.is_finite().then_some((value, unit))
}

fn prefix_of(designator: &str) -> String {
    designator.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_uppercase()
}

fn upper(s: &str) -> String {
    s.to_ascii_uppercase()
}

/// How well a pin's name says it is a converter's power ground: 0 for PGND or an exposed pad,
/// 1 for a plain GND or VSS, 2 for any other name, 3 for an analog or signal ground.
fn ground_rank(name: &str) -> u8 {
    let n = upper(name);
    if n.contains("AGND") || n.contains("SGND") || n.contains("AVSS") {
        3
    } else if n.contains("PGND") || n.contains("PVSS") || matches!(n.as_str(), "EP" | "PAD" | "EPAD" | "TAB" | "THERMAL") {
        0
    } else if n.contains("GND") || n.contains("VSS") {
        1
    } else {
        2
    }
}

/// A name a ground net has.
fn ground_name(name: &str) -> bool {
    let n = upper(name);
    let n = n.trim_start_matches(['+', '-']);
    ["GND", "AGND", "DGND", "PGND", "SGND", "GNDA", "GNDD", "GNDPWR", "VSS", "VSSA", "0V", "EARTH", "CHASSIS"].contains(&n)
        || n.starts_with("GND_")
        || n.ends_with("_GND")
}

/// The voltage a rail's name says: `+3V3` 3.3, `5V` 5, `1V8` 1.8, `+12V` 12, `VBUS` 5.
fn name_volts(name: &str) -> Option<f64> {
    let n = upper(name);
    let n = n.trim_start_matches('+');
    if n == "VBUS" || n.starts_with("VBUS_") || n == "USB_VBUS" {
        return Some(5.0);
    }
    let core = n.trim_start_matches('V').trim_start_matches("CC_").trim_start_matches("DD_");
    let (a, b) = core.split_once('V')?;
    if a.is_empty() || !a.chars().all(|c| c.is_ascii_digit()) || !b.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let v: f64 = if b.is_empty() { a.parse().ok()? } else { format!("{a}.{b}").parse().ok()? };
    (v > 0.0 && v <= super::intent::MAX_VOLTS).then_some(v)
}

/// A name a supply net has.
fn rail_name(name: &str) -> bool {
    let n = upper(name);
    if n.starts_with('+') || name_volts(name).is_some() {
        return true;
    }
    ["VBUS", "VBAT", "VSYS", "VCC", "VDD", "VIN", "VOUT", "VREG", "VSUP", "VMAIN", "VPP", "VBATT", "VDDIO", "AVDD", "DVDD"]
        .iter()
        .any(|p| n == *p || n.starts_with(&format!("{p}_")))
}

/// A pin name an IC's supply pin has: `VDD`, `VDDH`, `VCC`, `DEC1`, `AVDD`, `VDDIO`, `VBAT`…
fn supply_pin_name(name: &str) -> bool {
    let n = upper(name);
    let n = n.trim_start_matches(['~', '{', '}']);
    ["VDD", "VCC", "AVDD", "DVDD", "IOVDD", "VDDA", "VDDIO", "DEC", "VBAT", "VIN", "PVIN", "VBUS", "VREG", "VCORE"].iter().any(|p| n.starts_with(p))
}

/// A pin name a DC/DC converter's switch pin has: `DCC`, `DCCH`, `SW`, `SW1`, `LX`, `LX2`.
fn switch_pin_name(name: &str) -> bool {
    let n = upper(name);
    matches!(n.trim_end_matches(|c: char| c.is_ascii_digit()), "DCC" | "DCCH" | "SW" | "VSW" | "LX")
}

/// A pin name a supply-detect input has: `VBUS` (on an IC that feeds no supply: a USB device's),
/// `VBUS_DET`, `USB_DET`, `VSENSE`.
fn detect_pin_name(name: &str) -> bool {
    let n = upper(name);
    let n = n.trim_start_matches(['~', '{']).trim_end_matches('}');
    n == "VBUS" || n.contains("DET") || n.contains("SENSE")
}

/// A pin name an RF pin has: `ANT`, `RF`, `RFIO`, `RF_IN`.
fn rf_pin_name(name: &str) -> bool {
    let n = upper(name);
    // An antenna switch's control, an RF block's supply or enable is no RF line.
    if ["SW", "EN", "CTRL", "CTL", "VDD", "VCC", "VBAT"].iter().any(|w| n.contains(w)) {
        return false;
    }
    n == "ANT" || n == "RF" || n.starts_with("RF_") || n.starts_with("RFIO") || n == "RFP" || n == "RFN" || n.starts_with("ANT_")
}

/// The interface a pin or net name belongs to, and whether it is the bus's clock.
fn interface_of(name: &str) -> Option<(Interface, bool)> {
    let n = upper(name);
    let words: Vec<&str> = n.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |w: &str| words.contains(&w);
    if n.contains("QSPI") {
        return Some((Interface::Qspi, has("SCK") || has("SCLK") || has("CLK")));
    }
    if has("SWDIO") || has("SWDCLK") || has("SWCLK") || has("SWO") {
        return Some((Interface::Swd, false));
    }
    if has("SDA") || has("SCL") || n.contains("I2C") {
        return Some((Interface::I2c, false));
    }
    if has("MOSI") || has("MISO") || has("COPI") || has("CIPO") || has("SCK") || has("SCLK") || n.contains("SPI") || has("NSS") {
        return Some((Interface::Spi, has("SCK") || has("SCLK")));
    }
    if has("TXD") || has("RXD") || has("UART") || has("CTS") || has("RTS") {
        return Some((Interface::Uart, false));
    }
    if has("TCK") || has("TMS") || has("TDI") || has("TDO") {
        return Some((Interface::Jtag, has("TCK")));
    }
    if has("CANH") || has("CANL") {
        return Some((Interface::Can, false));
    }
    None
}

/// A differential pair's stem, its name and the polarity from a net name: `USB_DP` → (`USB_D`,
/// `USB`, true).
fn pair_stem(name: &str) -> Option<(String, String, bool)> {
    let n = upper(name);
    for (suffix, positive) in [("_P", true), ("_N", false), ("+", true), ("-", false), ("DP", true), ("DN", false), ("D+", true), ("D-", false)] {
        if let Some(stem) = n.strip_suffix(suffix)
            && !stem.is_empty()
        {
            let display = name.get(..stem.len()).unwrap_or(stem).trim_end_matches(['_', '-']).to_string();
            let stem = if suffix.starts_with('D') { format!("{stem}D") } else { stem.to_string() };
            return Some((stem, display, positive));
        }
    }
    None
}

/// What the inference works from.
struct Ctx<'a> {
    ecad: &'a Electronics,
    conn: &'a Connectivity,
    parts: BTreeMap<u64, PartInfo>,
    /// Ground nets, supply rails, and an IC's own supply nodes: a net only one IC drives with a
    /// power output (an internal regulator's), no rail port and no rail name.
    ground: BTreeSet<usize>,
    rails: BTreeSet<usize>,
    local: BTreeSet<usize>,
    /// Where each pin lies on the board and whether its part is on the bottom, by part id and pin
    /// number, for the placed parts.
    board_at: BTreeMap<(u64, String), ([f64; 2], bool)>,
    /// Each part's pins with their nets, by pin number (worked out once: the rules ask often).
    part_nets: BTreeMap<u64, BTreeMap<String, usize>>,
}

impl Ctx<'_> {
    fn part(&self, id: u64) -> Option<&PartInfo> {
        self.parts.get(&id)
    }

    fn net_name(&self, i: usize) -> &str {
        self.conn.nets.get(i).map_or("", |n| n.name.as_str())
    }

    fn pins(&self, i: usize) -> &[NetPin] {
        self.conn.nets.get(i).map_or(&[], |n| n.pins.as_slice())
    }

    /// The nets of a two-pin part's pins, in pin order.
    fn two_nets(&self, p: &PartInfo) -> Option<(usize, usize)> {
        let nets: Vec<usize> = self.part_nets.get(&p.id)?.values().copied().collect();
        match nets.as_slice() {
            [a, b] => Some((*a, *b)),
            _ => None,
        }
    }

    /// The nets a part's pins are on, each once, ground left out.
    fn nets_of(&self, part: u64) -> BTreeSet<usize> {
        self.part_nets.get(&part).map(|m| m.values().copied().filter(|n| !self.ground.contains(n)).collect()).unwrap_or_default()
    }

    /// The net of the part's pin `pin`.
    fn net_of(&self, part: u64, pin: &str) -> Option<usize> {
        self.conn.pin_net(part, pin)
    }

    /// Where a pin is: on the board when its part is placed (`true`), else on the sheet.
    fn at(&self, pin: &NetPin) -> ([f64; 2], bool) {
        match self.board_at.get(&(pin.part, pin.pin.clone())) {
            Some((at, _)) => (*at, true),
            None => (pin.at, false),
        }
    }

    /// Whether the pin's part is on the board.
    fn placed(&self, pin: &NetPin) -> bool {
        self.board_at.contains_key(&(pin.part, pin.pin.clone()))
    }

    /// How far apart two pins are for decoupling: on the board, a part on the other side counts
    /// [`OTHER_SIDE`] further (its loop goes through vias); else on the sheet.
    fn reach(&self, a: &NetPin, b: &NetPin) -> (f64, bool) {
        let key = |p: &NetPin| (p.part, p.pin.clone());
        match (self.board_at.get(&key(a)), self.board_at.get(&key(b))) {
            (Some((pa, sa)), Some((pb, sb))) => (dist(*pa, *pb) + if sa == sb { 0.0 } else { OTHER_SIDE }, true),
            _ => (dist(a.at, b.at), false),
        }
    }

    fn is_ic_pin(&self, pin: &NetPin) -> bool {
        self.part(pin.part).is_some_and(PartInfo::ic)
    }

    /// An IC's supply pin on the net `net` (not ground): a power input, a supply by its name, or
    /// the output of its own regulator on its own supply node (`DEC1`), which is decoupled too. A
    /// bidirectional pin named as a charger's or a power path's (`BAT`, `OUT`, `SYS`) is one too:
    /// a battery charger's capacitors sit on those.
    fn supply_pin(&self, pin: &NetPin, net: usize) -> bool {
        self.is_ic_pin(pin)
            && match pin.electrical {
                PinType::PowerInput => true,
                PinType::PowerOutput => self.local.contains(&net),
                PinType::Bidirectional => supply_pin_name(&pin.name) || matches!(upper(&pin.name).as_str(), "BAT" | "OUT" | "SYS" | "VSYS"),
                _ => supply_pin_name(&pin.name),
            }
    }

    /// A pin a capacitor on the net `net` may serve: an IC's supply pin, or an IC's power output
    /// (a regulator's or a charger's, whose capacitor sits at it).
    fn served_pin(&self, pin: &NetPin, net: usize) -> bool {
        self.supply_pin(pin, net) || (pin.electrical == PinType::PowerOutput && self.is_ic_pin(pin))
    }
}

/// What a pin on the other side of the board counts extra, in millimetres, when the nearest
/// supply pin to a capacitor is looked for.
const OTHER_SIDE: f64 = 5.0;

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn mm(v: f64) -> String {
    format!("{:.1}", v)
}

fn net_record(net: &str, role: NetRole) -> NetIntent {
    NetIntent {
        net: net.to_string(),
        role,
        current_a: None,
        voltage: None,
        edge_ns: None,
        driver: None,
        z_target: None,
        z_tol: None,
        interface: None,
        f_mhz: None,
        external: None,
        domain: None,
        source: Source::Inferred,
        by: None,
        confidence: None,
        evidence: Vec::new(),
        note: String::new(),
    }
}

fn part_record(part: &str, role: PartRole, of: Vec<String>) -> PartIntent {
    PartIntent {
        part: part.to_string(),
        role,
        of,
        cable: None,
        power_w: None,
        in_loop: None,
        source: Source::Inferred,
        by: None,
        confidence: None,
        evidence: Vec::new(),
        note: String::new(),
    }
}

/// `text` on one line (control characters as spaces) within [`super::MAX_DESCRIPTION`]
/// characters, cut with `…`: as a record's evidence may be.
fn one_line(text: &str) -> String {
    let max = super::MAX_DESCRIPTION;
    let clean: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if clean.chars().count() <= max {
        return clean;
    }
    let mut cut: String = clean.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// The proposals being collected: one per subject, the first rule to propose it winning.
#[derive(Default)]
struct Found {
    list: Vec<Proposal>,
    keys: BTreeSet<String>,
}

impl Found {
    fn add(&mut self, record: Record, confidence: Confidence, evidence: Vec<String>, blocking: bool, needs: Vec<&'static str>) {
        // Every proposal rests on evidence; one with none is no proposal.
        if evidence.is_empty() {
            return;
        }
        let key = record.key();
        if !self.keys.insert(key.clone()) {
            return;
        }
        // As a record keeps it: at most so many pieces, each one line within its length.
        let evidence: Vec<String> = evidence.into_iter().take(super::intent::MAX_EVIDENCE).map(|e| one_line(&e)).collect();
        let record = match record {
            Record::Net(mut r) => {
                (r.confidence, r.evidence) = (Some(confidence), evidence.clone());
                Record::Net(r)
            }
            Record::Part(mut r) => {
                (r.confidence, r.evidence) = (Some(confidence), evidence.clone());
                Record::Part(r)
            }
            Record::Pair(mut r) => {
                (r.confidence, r.evidence) = (Some(confidence), evidence.clone());
                Record::Pair(r)
            }
            Record::Loop(mut r) => {
                (r.confidence, r.evidence) = (Some(confidence), evidence.clone());
                Record::Loop(r)
            }
            r @ Record::Board(_) => r,
        };
        self.list.push(Proposal { key, record, confidence, evidence, blocking, needs, after: None });
    }

    /// As [`Found::add`], a proposal that stands with the proposal `after` (see
    /// [`Proposal::after`]).
    fn add_after(&mut self, after: &str, record: Record, confidence: Confidence, evidence: Vec<String>) {
        let key = record.key();
        if self.keys.contains(&key) {
            return;
        }
        self.add(record, confidence, evidence, false, Vec::new());
        if let Some(p) = self.list.last_mut().filter(|p| p.key == key) {
            p.after = Some(after.to_string());
        }
    }

    /// As [`Found::add_after`] when `after` is given, else a proposal of its own that needs no
    /// look.
    fn add_with(&mut self, after: Option<&str>, record: Record, confidence: Confidence, evidence: Vec<String>) {
        match after {
            Some(a) => self.add_after(a, record, confidence, evidence),
            None => self.add(record, confidence, evidence, false, Vec::new()),
        }
    }

    fn has(&self, key: &str) -> bool {
        self.keys.contains(key)
    }
}

/// The proposals for `ecad`'s design (see the module notes).
pub fn infer(ecad: &Electronics) -> Inference {
    let conn = ecad.connectivity();
    let sheet = &ecad.schematic;
    let mut parts = BTreeMap::new();
    for p in &sheet.parts {
        let c = ecad.component(p.component);
        let text = c.map(|c| format!("{} {} {} {}", c.name, c.description, c.mpn, c.manufacturer).to_lowercase()).unwrap_or_default();
        let value = p.value.clone().or_else(|| c.map(|c| c.value.clone())).unwrap_or_default();
        let pins = c.map_or(0, |c| ecad.symbol_of(c).pins.len());
        parts.insert(p.id, PartInfo { id: p.id, designator: p.designator.clone(), prefix: prefix_of(&p.designator), value, text, pins, dnp: p.dnp });
    }
    let mut board_at = BTreeMap::new();
    for f in &ecad.board.footprints {
        let Some(placed) = ecad.placed(f) else { continue };
        let pads: BTreeMap<String, [f64; 2]> = footprint_pads(ecad, f, None).into_iter().map(|p| (p.pad.number.clone(), p.at())).collect();
        let bottom = f.side == super::footprint::Side::Bottom;
        for (pin, on) in placed.resolved.connections() {
            if let Some(at) = on.first().and_then(|pad| pads.get(*pad)) {
                board_at.insert((placed.part.id, pin.to_string()), (*at, bottom));
            }
        }
    }
    let labels: BTreeMap<u64, LabelKind> = sheet.labels.iter().map(|l| (l.id, l.kind)).collect();
    let mut ground = BTreeSet::new();
    let mut rails = BTreeSet::new();
    let mut local = BTreeSet::new();
    let is_ic = |id: u64| parts.get(&id).is_some_and(PartInfo::ic);
    for (i, n) in conn.nets.iter().enumerate() {
        let ground_port = n.labels.iter().any(|l| labels.get(l) == Some(&LabelKind::Ground));
        let drivers: BTreeSet<u64> = n.pins.iter().filter(|p| p.electrical == PinType::PowerOutput).map(|p| p.part).collect();
        let ics: BTreeSet<u64> = n.pins.iter().filter(|p| is_ic(p.part)).map(|p| p.part).collect();
        if ground_port || ground_name(&n.name) {
            ground.insert(i);
        } else if n.power || rail_name(&n.name) {
            rails.insert(i);
        } else if drivers.len() == 1 && ics.len() == 1 {
            local.insert(i);
        } else if !drivers.is_empty() {
            rails.insert(i);
        }
    }
    let mut part_nets: BTreeMap<u64, BTreeMap<String, usize>> = BTreeMap::new();
    for (i, n) in conn.nets.iter().enumerate() {
        for q in &n.pins {
            part_nets.entry(q.part).or_default().insert(q.pin.clone(), i);
        }
    }
    let c = Ctx { ecad, conn: &conn, parts, ground, rails, local, board_at, part_nets };
    let mut f = Found::default();
    board_facts(&c, &mut f);
    grounds(&c, &mut f, &labels);
    let claimed = smps(&c, &mut f);
    dc_dc(&c, &mut f);
    crystals(&c, &mut f);
    rf(&c, &mut f);
    power_ics(&c, &mut f);
    rails_out(&c, &mut f);
    pairs(&c, &mut f);
    protection(&c, &mut f);
    connectors(&c, &mut f);
    let power_pins = decoupling(&c, &mut f, &claimed);
    signals(&c, &mut f);
    small_parts(&c, &mut f);
    // Board facts, then nets, pairs, parts and loops.
    let order = |p: &Proposal| match p.record {
        Record::Board(_) => 0,
        Record::Net(_) => 1,
        Record::Pair(_) => 2,
        Record::Part(_) => 3,
        Record::Loop(_) => 4,
    };
    let mut proposals = f.list;
    proposals.sort_by_key(order);
    Inference { proposals, power_pins }
}

/// The radio bands, from the radios' descriptions: 2.4 GHz radios give their band; a sub-GHz
/// radio's band depends on the region, which only the user can say.
fn board_facts(c: &Ctx, f: &mut Found) {
    let mut bands = Vec::new();
    let mut evidence = Vec::new();
    let mut needs = Vec::new();
    let mut seen = BTreeSet::new();
    // Only a chip or a module is a radio: a capacitor's or a regulator's text may name a band.
    for p in c.parts.values().filter(|p| p.ic()) {
        let ghz = p.has_word(&["ble", "bluetooth", "zigbee", "wifi", "thread"]) || p.is(&["802.15.4", "2.4 ghz", "2.4ghz", "wi-fi"]);
        if ghz && !p.is(&["antenna"]) && seen.insert("2g4") {
            bands.push(Band { name: "ISM 2.4 GHz".into(), lo_mhz: 2400.0, hi_mhz: 2483.5, rx: true });
            evidence.push(format!("{} is a 2.4 GHz radio (its description)", p.designator));
        }
        if (p.is(&["lora", "sx126", "sx127", "sub-ghz", "433 mhz"]) || p.has_word(&["868", "915", "868mhz", "915mhz"])) && seen.insert("subg") {
            needs.push("bands");
            evidence.push(format!("{} is a sub-GHz radio: its band (EU868 863-870 MHz, US915 902-928 MHz, …) depends on the region", p.designator));
        }
    }
    if bands.is_empty() && needs.is_empty() {
        return;
    }
    let facts = BoardFacts { bands: Some(bands), ..BoardFacts::default() };
    f.add(Record::Board(facts), Confidence::Medium, evidence, true, needs);
}

fn grounds(c: &Ctx, f: &mut Found, labels: &BTreeMap<u64, LabelKind>) {
    for i in c.ground.iter().copied() {
        let Some(n) = c.conn.nets.get(i) else { continue };
        let mut evidence = Vec::new();
        if n.labels.iter().any(|l| labels.get(l) == Some(&LabelKind::Ground)) {
            evidence.push("a ground port on the sheet".to_string());
        }
        if ground_name(&n.name) {
            evidence.push(format!("named {}", n.name));
        }
        let mut r = net_record(&n.name, NetRole::Ground);
        r.voltage = Some(Voltage { nominal: 0.0, peak: None });
        f.add(Record::Net(r), Confidence::High, evidence, true, Vec::new());
    }
}

/// The current a regulator's description gives (`2 A buck-boost`, `600 mA`), in amperes.
fn rated_amps(text: &str) -> Option<f64> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let next = words.get(i + 1).copied().unwrap_or("");
        let (num, unit) = match w.strip_suffix("ma").or_else(|| w.strip_suffix('a')) {
            Some(n) if !n.is_empty() => (n, if w.ends_with("ma") { 1e-3 } else { 1.0 }),
            _ if next == "a" || next.starts_with("a,") => (*w, 1.0),
            _ if next == "ma" => (*w, 1e-3),
            _ => continue,
        };
        if let Ok(v) = num.parse::<f64>()
            && v > 0.0
            && v * unit <= super::intent::MAX_AMPS
        {
            return Some(v * unit);
        }
    }
    None
}

/// The supply-detect inputs (`U1.AD2 (VBUS)`) the supply net `net` reaches when it feeds nothing
/// else: its IC pins are all named as detect or sense inputs, on ICs that feed no supply
/// (`feeding`) and are no power IC, and its other parts are capacitors to ground, protection,
/// connectors, and series resistors or inductors from one of the `sources` (a net with a
/// connector or a power output). Such a net carries no load current, or a few milliamperes into
/// a USB device's VBUS pin (which may power the chip's own USB regulator).
fn detect_only(c: &Ctx, net: usize, feeding: &BTreeSet<u64>, sources: &BTreeSet<usize>) -> Option<Vec<String>> {
    let mut detect = Vec::new();
    for q in c.pins(net) {
        let p = c.part(q.part)?;
        if !p.mounted() || p.connector() || p.protection() {
            continue;
        }
        if p.ic() {
            if !detect_pin_name(&q.name) || feeding.contains(&p.id) || p.power_kind().is_some() {
                return None;
            }
            detect.push(format!("{} ({})", q.reference(), q.name));
            continue;
        }
        let (a, b) = c.two_nets(p)?;
        let other = if a == net { b } else { a };
        let fine = (p.capacitor() && c.ground.contains(&other)) || ((p.resistor() || p.inductor()) && other != net && sources.contains(&other));
        if !fine {
            return None;
        }
    }
    (!detect.is_empty()).then_some(detect)
}

/// Supplies: rail ports, power outputs and rail names, with the voltage their names give and the
/// current the regulator driving them is rated for. A supply that only reaches supply-detect
/// inputs (see [`detect_only`]) needs no look and no current.
fn rails_out(c: &Ctx, f: &mut Found) {
    let battery = c.parts.values().any(|p| p.is(&["li-ion", "li-po", "lipo", "lithium"]));
    // The parts that feed a supply with a power output, and the nets a supply may come from.
    let feeding: BTreeSet<u64> =
        c.rails.iter().flat_map(|r| c.pins(*r).iter().filter(|q| q.electrical == PinType::PowerOutput).map(|q| q.part)).collect();
    let sources: BTreeSet<usize> = (0..c.conn.nets.len())
        .filter(|n| c.pins(*n).iter().any(|q| q.electrical == PinType::PowerOutput || c.part(q.part).is_some_and(PartInfo::connector)))
        .collect();
    for i in c.rails.iter().copied() {
        let name = c.net_name(i).to_string();
        let mut r = net_record(&name, NetRole::Power);
        let mut evidence = Vec::new();
        if c.conn.nets.get(i).is_some_and(|n| n.power) {
            evidence.push("a rail port on the sheet".to_string());
        }
        if rail_name(&name) {
            evidence.push(format!("named {name}, as supplies are"));
        }
        let drivers: Vec<&NetPin> = c.pins(i).iter().filter(|p| p.electrical == PinType::PowerOutput).collect();
        for d in &drivers {
            evidence.push(format!("driven by {} ({})", d.reference(), d.name));
        }
        if let Some(v) = name_volts(&name) {
            r.voltage = Some(Voltage { nominal: v, peak: None });
            evidence.push(format!("the name says {v} V"));
        } else if upper(&name).starts_with("VBAT") && battery {
            r.voltage = Some(Voltage { nominal: 3.7, peak: Some(4.2) });
            evidence.push("a one-cell lithium battery (a charger's description): 3.7 V, 4.2 V full".to_string());
        }
        for d in &drivers {
            if let Some(p) = c.part(d.part)
                && let Some(a) = rated_amps(&p.text)
            {
                r.current_a = Some(a);
                evidence.push(format!("{} is rated {a} A (its description)", p.designator));
                break;
            }
        }
        r.external = c.pins(i).iter().any(|p| c.part(p.part).is_some_and(PartInfo::connector)).then_some(true);
        let detect = if drivers.is_empty() { detect_only(c, i, &feeding, &sources) } else { None };
        if let Some(pins) = &detect {
            // A USB device's VBUS pin draws a little (the chip's USB regulator); a detect or sense
            // pin draws nothing.
            let vbus = c
                .pins(i)
                .iter()
                .any(|q| c.part(q.part).is_some_and(PartInfo::ic) && upper(&q.name).trim_start_matches(['~', '{']).trim_end_matches('}') == "VBUS");
            evidence.push(if vbus {
                format!(
                    "it feeds only a USB device's VBUS input or supply-detect pins, {}: a few mA at most, no copper sizing needed",
                    pins.join(", ")
                )
            } else {
                format!("it feeds only supply-detect inputs, {}: no load current to size its copper for", pins.join(", "))
            });
        }
        let mut needs = r.needs();
        if detect.is_some() {
            needs.retain(|n| *n != "current_a");
        }
        let confidence = if drivers.is_empty() { Confidence::Medium } else { Confidence::High };
        f.add(Record::Net(r), confidence, evidence, detect.is_none(), needs);
    }
}

/// The regulators: a switching converter's IC, its switch nodes and inductor, its input and
/// output capacitors, feedback divider and hot loops. Returns the capacitors it took, which
/// decoupling leaves alone.
fn smps(c: &Ctx, f: &mut Found) -> BTreeSet<u64> {
    let mut claimed = BTreeSet::new();
    for p in c.parts.values().filter(|p| p.ic()) {
        let (buck, boost) = if p.is(&["buck-boost", "buck boost", "buck/boost"]) {
            (true, true)
        } else if p.is(&["linear", "ldo"]) {
            continue;
        } else if p.is(&["buck", "step-down", "step down"]) {
            (true, false)
        } else if p.is(&["boost", "step-up", "step up"]) {
            (false, true)
        } else {
            continue;
        };
        let pins: Vec<&NetPin> = c.conn.nets.iter().flat_map(|n| n.pins.iter()).filter(|q| q.part == p.id).collect();
        let net = |q: &NetPin| c.net_of(q.part, &q.pin);
        // Switch nodes: a pin's net that also has an inductor and is no supply.
        let mut sw: Vec<(&NetPin, usize)> = Vec::new();
        for q in &pins {
            let Some(n) = net(q) else { continue };
            if c.rails.contains(&n) || c.ground.contains(&n) {
                continue;
            }
            if c.pins(n).iter().any(|o| c.part(o.part).is_some_and(PartInfo::inductor)) {
                sw.push((q, n));
            }
        }
        if sw.is_empty() {
            continue;
        }
        let topology = match (buck, boost) {
            (true, true) => "buck-boost",
            (true, false) => "buck",
            _ => "boost",
        };
        let evidence = vec![
            format!("{} is a {topology} converter (its description)", p.designator),
            "confirm the topology against its datasheet: its switch nodes, capacitors, inductor and hot loops are accepted with it".to_string(),
        ];
        let ic = format!("part|{}", p.designator);
        f.add(Record::Part(part_record(&p.designator, PartRole::SmpsIc, Vec::new())), Confidence::Medium, evidence, true, Vec::new());
        for (q, n) in &sw {
            let mut r = net_record(c.net_name(*n), NetRole::SwNode);
            r.domain = Some(p.designator.clone());
            f.add_after(&ic, Record::Net(r), Confidence::Medium, vec![format!("{} ({}) meets an inductor", q.reference(), q.name)]);
            for o in c.pins(*n).iter().filter(|o| c.part(o.part).is_some_and(PartInfo::inductor)) {
                let Some(l) = c.part(o.part) else { continue };
                let e = vec![format!("on {}'s switch node {}", p.designator, c.net_name(*n))];
                f.add_after(&ic, Record::Part(part_record(&l.designator, PartRole::SmpsInductor, vec![p.designator.clone()])), Confidence::Medium, e);
            }
        }
        // Its ground pin, the one the pulsed current returns through: a power ground (PGND, or an
        // exposed pad) first, then a plain GND or VSS, then any pin on a ground net; an analog or
        // signal ground (AGND, SGND) only when there is nothing else.
        let grounded: Vec<&&NetPin> = pins.iter().filter(|q| net(q).is_some_and(|n| c.ground.contains(&n))).collect();
        let gnd_pin = grounded.iter().min_by_key(|q| ground_rank(&q.name)).map(|q| q.reference());
        // The capacitors from a pin's supply net to ground, with their pins.
        let caps_at = |q: &NetPin| -> Vec<(&PartInfo, NetPin)> {
            let Some(n) = net(q) else { return Vec::new() };
            c.pins(n)
                .iter()
                .filter_map(|o| {
                    let cap = c.part(o.part).filter(|x| x.capacitor() && x.mounted())?;
                    let (a, b) = c.two_nets(cap)?;
                    let other = if a == n { b } else { a };
                    c.ground.contains(&other).then(|| (cap, o.clone()))
                })
                .collect()
        };
        let input = pins.iter().find(|q| {
            let n = upper(&q.name);
            q.electrical == PinType::PowerInput && (n.contains("VIN") || n == "IN" || n.starts_with("PVIN"))
        });
        let output = pins.iter().find(|q| {
            let n = upper(&q.name);
            q.electrical == PinType::PowerOutput || n == "VOUT" || n == "OUT" || n == "VO"
        });
        let sw_pin = |k: usize| sw.get(k).or_else(|| sw.first()).map(|(q, _)| q.reference());
        for (pin, role, kind, sw_k) in [(input, PartRole::SmpsInCap, LoopKind::BuckHot, 0), (output, PartRole::SmpsOutCap, LoopKind::BoostHot, 1)] {
            let Some(pin) = pin else { continue };
            let found = caps_at(pin);
            // On a shared rail only the capacitors nearer this pin than any other IC's supply pin
            // or power output (a charger's output capacitor is the charger's).
            let others: Vec<&NetPin> =
                net(pin).map(|n| c.pins(n).iter().filter(|o| o.part != p.id && c.served_pin(o, n)).collect()).unwrap_or_default();
            // Every distance on one footing: the board's once all of these are placed, else the
            // sheet's (a placed pin's board spot is no distance from an unplaced one's on the sheet).
            let board = c.placed(pin) && found.iter().all(|(_, o)| c.placed(o)) && others.iter().all(|o| c.placed(o));
            let apart = |a: &NetPin, b: &NetPin| if board { c.reach(a, b).0 } else { dist(a.at, b.at) };
            let mut caps: Vec<(&PartInfo, f64, NetPin)> = found.into_iter().map(|(cap, o)| (cap, apart(&o, pin), o)).collect();
            caps.sort_by(|a, b| a.1.total_cmp(&b.1));
            for (k, (cap, d, cap_pin)) in caps.iter().enumerate() {
                let nearer = others.iter().all(|o| apart(cap_pin, o) >= *d - 1e-9);
                if k > 0 && !nearer {
                    continue;
                }
                claimed.insert(cap.id);
                let e = vec![format!(
                    "{} ({}) runs from {}'s {} net {} to ground",
                    cap.designator,
                    cap.value,
                    p.designator,
                    pin.name,
                    c.net_name(net(pin).unwrap_or_default())
                )];
                f.add_after(&ic, Record::Part(part_record(&cap.designator, role, vec![pin.reference()])), Confidence::Medium, e);
            }
            let wanted = match kind {
                LoopKind::BuckHot => buck,
                _ => boost,
            };
            if let (true, Some((cap, _, cap_pin)), Some(g), Some(s)) = (wanted, caps.first(), &gnd_pin, sw_pin(sw_k)) {
                // Round the loop: the capacitor's pad on the pin's net beside the pin, its ground
                // pad beside the IC's ground.
                let other = c.part_nets.get(&cap.id).and_then(|m| m.keys().find(|k| **k != cap_pin.pin)).cloned().unwrap_or_default();
                let mut members = vec![cap_pin.reference(), pin.reference(), s, g.clone(), format!("{}.{other}", cap.designator)];
                members.dedup();
                let name = format!("{}.{}", p.designator, kind.name());
                let r = LoopIntent {
                    name,
                    kind,
                    members,
                    reference_ratio: None,
                    source: Source::Inferred,
                    by: None,
                    confidence: None,
                    evidence: Vec::new(),
                    note: String::new(),
                };
                let e = vec![format!("{topology}: {} {}'s capacitor, the switch pin and ground carry its pulsed current", cap.designator, pin.name)];
                f.add_after(&ic, Record::Loop(r), Confidence::Medium, e);
            }
        }
        // The feedback divider: resistors on the FB pin's net.
        for q in pins.iter().filter(|q| matches!(upper(&q.name).as_str(), "FB" | "VFB" | "ADJ" | "FEEDBACK")) {
            let Some(n) = net(q) else { continue };
            let mut r = net_record(c.net_name(n), NetRole::Fb);
            r.domain = Some(p.designator.clone());
            f.add_after(&ic, Record::Net(r), Confidence::High, vec![format!("{} is {}'s feedback pin", q.reference(), p.designator)]);
            for o in c.pins(n).iter().filter(|o| c.part(o.part).is_some_and(PartInfo::resistor)) {
                let Some(res) = c.part(o.part) else { continue };
                let e = vec![format!("on {}'s feedback net {}", p.designator, c.net_name(n))];
                f.add_after(&ic, Record::Part(part_record(&res.designator, PartRole::FbDivider, vec![q.reference()])), Confidence::High, e);
            }
        }
    }
    claimed
}

/// How many inductors a DC/DC switch node is followed through (a bead or a small inductor for
/// high frequencies may sit between the switch pin and the converter's own inductor).
const SW_HOPS: usize = 3;
/// The least inductance a converter's own inductor has; less is a choke for high frequencies.
const MAIN_HENRIES: f64 = 1e-6;

/// DC/DC converters by their switch pins, also those no description names (a radio's own): a pin
/// named as a switch pin whose net meets an inductor makes a switch node, and so does each net
/// on from a bead or a small inductor (below [`MAIN_HENRIES`]) that only joins it to the next
/// one; the inductors on the way, the converter's own (the first of a microhenry or more)
/// included, are the converter's. Past its own inductor is the converter's output, smooth: a
/// supply of the converter's (no switch node), and an inductor that only joins on from there
/// filters it. A regulator [`smps`] found keeps them with
/// it. Each net is followed once.
fn dc_dc(c: &Ctx, f: &mut Found) {
    let inductor = |o: &NetPin| c.part(o.part).is_some_and(PartInfo::inductor);
    let smps_ics: BTreeSet<String> = f.list.iter().filter(|p| p.record.role() == PartRole::SmpsIc.name()).map(|p| p.key.clone()).collect();
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    // For a net that joins only inductors, the first two of them (one is the inductor it is
    // reached through): worked out once per net.
    let mut joins: BTreeMap<usize, Option<Vec<u64>>> = BTreeMap::new();
    for (i, n) in c.conn.nets.iter().enumerate() {
        if seen.contains(&i) || c.rails.contains(&i) || c.ground.contains(&i) || !n.pins.iter().any(inductor) {
            continue;
        }
        let Some(q) = n.pins.iter().find(|q| c.is_ic_pin(q) && switch_pin_name(&q.name)) else { continue };
        let Some(ic) = c.part(q.part) else { continue };
        let ic_key = format!("part|{}", ic.designator);
        let after = smps_ics.contains(&ic_key).then_some(ic_key.as_str());
        seen.insert(i);
        let mut frontier = vec![(i, format!("{} ({}) is a DC/DC switch pin, and its net meets an inductor", q.reference(), q.name))];
        for _ in 0..SW_HOPS {
            let mut next = Vec::new();
            for (net, why) in frontier {
                let mut r = net_record(c.net_name(net), NetRole::SwNode);
                r.domain = Some(ic.designator.clone());
                f.add_with(after, Record::Net(r), Confidence::Medium, vec![why]);
                for o in c.pins(net).iter().filter(|o| inductor(o)) {
                    let Some(l) = c.part(o.part) else { continue };
                    let e = vec![format!("on {}'s switch node {}", ic.designator, c.net_name(net))];
                    let r = part_record(&l.designator, PartRole::SmpsInductor, vec![ic.designator.clone()]);
                    f.add_with(after, Record::Part(r), Confidence::Medium, e);
                    // On to the net past it while that only joins it to the next inductor.
                    let Some((a, b)) = c.two_nets(l) else { continue };
                    let other = if a == net { b } else { a };
                    if seen.contains(&other) || c.ground.contains(&other) || c.rails.contains(&other) {
                        continue;
                    }
                    if l.main_inductor() {
                        // Past the converter's own inductor: its output, with no switching edge.
                        seen.insert(other);
                        let pins = c.pins(other);
                        // A supply of its own (a radio's DC/DC output), unless another switch pin
                        // is on it (a buck-boost's second switch node).
                        if !pins.iter().any(|x| c.is_ic_pin(x) && switch_pin_name(&x.name)) {
                            let mut r = net_record(c.net_name(other), NetRole::Power);
                            r.domain = Some(ic.designator.clone());
                            let e = vec![format!(
                                "past {}, {}'s DC/DC inductor: {} is the converter's output, a supply with no switching edge",
                                l.designator,
                                ic.designator,
                                c.net_name(other)
                            )];
                            f.add_with(after, Record::Net(r), Confidence::Medium, e);
                        }
                        if pins.iter().all(inductor) {
                            for x in pins.iter().filter(|x| x.part != l.id).filter_map(|x| c.part(x.part)) {
                                let e = vec![format!(
                                    "past {}, {}'s inductor, on its output {}: it filters the output, it carries no switching edge",
                                    l.designator,
                                    ic.designator,
                                    c.net_name(other)
                                )];
                                f.add_with(
                                    after,
                                    Record::Part(part_record(&x.designator, PartRole::Filter, vec![ic.designator.clone()])),
                                    Confidence::Medium,
                                    e,
                                );
                            }
                        }
                        continue;
                    }
                    let parts = joins.entry(other).or_insert_with(|| {
                        let pins = c.pins(other);
                        pins.iter().all(inductor).then(|| pins.iter().map(|x| x.part).take(2).collect())
                    });
                    let Some(to) = parts.as_ref().and_then(|ps| ps.iter().find(|p| **p != l.id)).and_then(|p| c.part(*p)) else { continue };
                    seen.insert(other);
                    let (from, to) = (&l.designator, &to.designator);
                    next.push((other, format!("between {from} and {to} on {}'s DC/DC path from {} ({})", ic.designator, q.reference(), q.name)));
                }
            }
            frontier = next;
        }
    }
}

/// Power ICs that are no switching regulator: a charger, a power-path or power-management IC, a
/// power switch or a linear regulator, by its description, whose output feeds a supply.
fn power_ics(c: &Ctx, f: &mut Found) {
    // Each part's first output pin on a supply: a power output, or a pin named as an output.
    let mut outputs: BTreeMap<u64, (&NetPin, usize)> = BTreeMap::new();
    for r in c.rails.iter().copied() {
        for q in c.pins(r) {
            if q.electrical == PinType::PowerOutput || matches!(upper(&q.name).as_str(), "OUT" | "VOUT" | "VO" | "SYS" | "VSYS") {
                outputs.entry(q.part).or_insert((q, r));
            }
        }
    }
    for p in c.parts.values().filter(|p| p.ic()) {
        let (Some(kind), Some((q, r))) = (p.power_kind(), outputs.get(&p.id)) else { continue };
        let e = vec![format!("{} is {kind} (its description)", p.designator), format!("{} ({}) feeds {}", q.reference(), q.name, c.net_name(*r))];
        f.add(Record::Part(part_record(&p.designator, PartRole::PowerIc, Vec::new())), Confidence::Medium, e, false, Vec::new());
    }
}

/// Crystals, their two nets (clocks) and load capacitors.
fn crystals(c: &Ctx, f: &mut Found) {
    for p in c.parts.values().filter(|p| p.crystal()) {
        let nets = c.nets_of(p.id);
        if nets.len() != 2 {
            continue;
        }
        // Both nets go to pins of one IC.
        let ics: Vec<BTreeSet<u64>> = nets.iter().map(|n| c.pins(*n).iter().filter(|q| c.is_ic_pin(q)).map(|q| q.part).collect()).collect();
        let Some(ic) = ics.first().and_then(|a| ics.get(1).and_then(|b| a.intersection(b).next().copied())) else { continue };
        let of: Vec<String> = nets.iter().filter_map(|n| c.pins(*n).iter().find(|q| q.part == ic).map(NetPin::reference)).collect();
        let ic_name = c.part(ic).map_or(String::new(), |x| x.designator.clone());
        let e = vec![format!("{} ({}) joins {} only", p.designator, p.value, of.join(" and "))];
        f.add(Record::Part(part_record(&p.designator, PartRole::Crystal, of.clone())), Confidence::High, e, true, Vec::new());
        for n in &nets {
            let mut r = net_record(c.net_name(*n), NetRole::Clock);
            r.domain = Some(ic_name.clone());
            if let Some(hz) = hertz(&p.value) {
                r.f_mhz = Some(hz / 1e6);
            }
            f.add(Record::Net(r), Confidence::High, vec![format!("a pin of crystal {}", p.designator)], true, Vec::new());
            for q in c.pins(*n).iter().filter(|q| c.part(q.part).is_some_and(|x| x.capacitor() && x.mounted())) {
                let Some(cap) = c.part(q.part) else { continue };
                if c.two_nets(cap).is_some_and(|(a, b)| c.ground.contains(&a) || c.ground.contains(&b)) {
                    let e = vec![format!("{} ({}) runs from {}'s net {} to ground", cap.designator, cap.value, p.designator, c.net_name(*n))];
                    f.add(
                        Record::Part(part_record(&cap.designator, PartRole::LoadCap, vec![p.designator.clone()])),
                        Confidence::High,
                        e,
                        false,
                        Vec::new(),
                    );
                }
            }
        }
    }
}

/// A frequency a crystal's value gives (`32MHz`, `32 MHz`, `32.768 kHz`, `32.768KHZ`), in hertz;
/// `None` when it names no unit of hertz.
fn hertz(value: &str) -> Option<f64> {
    let text: String = value.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    let at = text.find("hz")?;
    let n = text.get(..at)?;
    let (number, scale) = if let Some(x) = n.strip_suffix('g') {
        (x, 1e9)
    } else if let Some(x) = n.strip_suffix('m') {
        (x, 1e6)
    } else if let Some(x) = n.strip_suffix('k') {
        (x, 1e3)
    } else {
        (n, 1.0)
    };
    let v: f64 = number.parse().ok()?;
    (v.is_finite() && v > 0.0).then_some(v * scale)
}

/// A two-pin part an RF line runs through: a small inductor or capacitor, or a 0 Ω link.
fn rf_series(p: &PartInfo) -> bool {
    let Some((v, unit)) = parse_value(&p.value) else { return false };
    (p.inductor() && (unit == 'H' || unit == '?') && v <= 100e-9)
        || (p.capacitor() && (unit == 'F' || unit == '?') && v <= 100e-12)
        || (p.resistor() && v <= 1.0)
}

/// RF: antennas, RF connectors and RF pins, and the nets their lines run through.
fn rf(c: &Ctx, f: &mut Found) {
    let mut start: BTreeMap<usize, String> = BTreeMap::new();
    for p in c.parts.values() {
        // A matching part's description may say what it matches: only a part of its own is the
        // antenna.
        let passive = matches!(p.prefix.as_str(), "C" | "L" | "R" | "FB");
        let antenna = matches!(p.prefix.as_str(), "ANT" | "AE") || (p.is(&["antenna"]) && !passive && !p.connector());
        let coax = p.connector() && p.is(&["coax", "u.fl", "sma ", "mmcx", "ipex", "50 ohm"]);
        if antenna {
            f.add(
                Record::Part(part_record(&p.designator, PartRole::Antenna, Vec::new())),
                Confidence::High,
                vec![format!("{} is an antenna ({})", p.designator, p.value)],
                true,
                Vec::new(),
            );
        }
        if antenna || coax {
            for i in c.nets_of(p.id) {
                start.entry(i).or_insert_with(|| format!("{} is on {}", c.net_name(i), if antenna { "the antenna" } else { "an RF connector" }));
            }
        }
    }
    let mut rf_parts: BTreeSet<u64> = BTreeSet::new();
    for (i, n) in c.conn.nets.iter().enumerate() {
        for q in n.pins.iter().filter(|q| c.is_ic_pin(q) && rf_pin_name(&q.name)) {
            start.entry(i).or_insert_with(|| format!("{} is an RF pin ({})", q.reference(), q.name));
            rf_parts.insert(q.part);
        }
    }
    // Follow the lines through small series parts.
    let mut reached: BTreeMap<usize, String> = start.clone();
    let mut frontier: Vec<usize> = start.keys().copied().collect();
    let mut matching: BTreeSet<u64> = BTreeSet::new();
    for _ in 0..RF_HOPS {
        let mut next = Vec::new();
        for n in frontier {
            for q in c.pins(n) {
                let Some(p) = c.part(q.part).filter(|p| p.pins == 2 && (p.capacitor() || p.inductor() || p.resistor())) else { continue };
                let Some((a, b)) = c.two_nets(p) else { continue };
                let other = if a == n { b } else { a };
                if c.ground.contains(&other) {
                    if rf_series(p) {
                        matching.insert(p.id);
                    }
                    continue;
                }
                if !rf_series(p) || c.rails.contains(&other) {
                    continue;
                }
                matching.insert(p.id);
                if let std::collections::btree_map::Entry::Vacant(e) = reached.entry(other) {
                    e.insert(format!("through {} ({}) from {}", p.designator, p.value, c.net_name(n)));
                    next.push(other);
                }
            }
        }
        frontier = next;
    }
    for (n, why) in &reached {
        let mut r = net_record(c.net_name(*n), NetRole::Rf);
        r.z_target = Some(50.0);
        f.add(Record::Net(r), Confidence::High, vec![why.clone()], true, Vec::new());
    }
    for id in matching {
        let Some(p) = c.part(id) else { continue };
        let on: Vec<String> = c.two_nets(p).map(|(a, b)| vec![c.net_name(a).to_string(), c.net_name(b).to_string()]).unwrap_or_default();
        let e = vec![format!("{} ({}) sits on the RF line {}", p.designator, p.value, on.join(" / "))];
        f.add(Record::Part(part_record(&p.designator, PartRole::RfMatch, on)), Confidence::Medium, e, false, Vec::new());
    }
    for id in rf_parts {
        let Some(p) = c.part(id) else { continue };
        let role = if p.is(&["module"]) { PartRole::Module } else { PartRole::RfIc };
        let e = vec![format!("{} has an RF pin", p.designator)];
        f.add(Record::Part(part_record(&p.designator, role, Vec::new())), Confidence::High, e, false, Vec::new());
    }
}

/// Differential pairs, from `_P`/`_N`, `+`/`-` and `DP`/`DN` names and the net classes' pairs:
/// two signal nets (no ground or supply: `BAT+`/`BAT-` is no pair) that reach one IC or a
/// connector together.
fn pairs(c: &Ctx, f: &mut Found) {
    let mut stems: BTreeMap<String, (Option<usize>, Option<usize>, String)> = BTreeMap::new();
    for (i, n) in c.conn.nets.iter().enumerate() {
        if c.ground.contains(&i) || c.rails.contains(&i) || c.local.contains(&i) {
            continue;
        }
        if let Some((stem, display, positive)) = pair_stem(&n.name) {
            let e = stems.entry(stem).or_insert((None, None, display));
            if positive { e.0 = Some(i) } else { e.1 = Some(i) }
        }
    }
    let high_speed = c.parts.values().any(|p| p.is(&["high-speed", "high speed", "480 mb", "usb 3", "usb3"]));
    // The ICs and connectors a net reaches.
    let ends = |i: usize| -> BTreeSet<u64> {
        c.pins(i).iter().filter(|q| c.part(q.part).is_some_and(|x| x.ic() || x.connector())).map(|q| q.part).collect()
    };
    for (_, (p, n, display)) in stems {
        let (Some(p), Some(n)) = (p, n) else { continue };
        if ends(p).is_disjoint(&ends(n)) {
            continue;
        }
        let (pn, nn) = (c.net_name(p).to_string(), c.net_name(n).to_string());
        let usb = upper(&pn).contains("USB") || c.pins(p).iter().any(|q| q.name == "D+" || upper(&q.name) == "DP");
        let mut evidence = vec![format!("{pn} and {nn} are named as a pair")];
        let class = &c.ecad.board.rules;
        if let Some(cl) = class.class_of(&pn).filter(|cl| cl.diff_pair.is_some()) {
            evidence.push(format!("net class {} has a differential pair", cl.name));
        }
        let interface = if usb {
            evidence.push(if high_speed {
                "a part names high-speed USB".to_string()
            } else {
                "no part names a high-speed USB PHY: full speed".to_string()
            });
            Some(if high_speed { Interface::Usb2Hs } else { Interface::Usb2Fs })
        } else {
            None
        };
        let name = if display.is_empty() { pn.clone() } else { display };
        let skew_mm = None;
        let r = PairIntent {
            name,
            p: pn.clone(),
            n: nn.clone(),
            interface,
            skew_mm,
            source: Source::Inferred,
            by: None,
            confidence: None,
            evidence: Vec::new(),
            note: String::new(),
        };
        f.add(Record::Pair(r), Confidence::Medium, evidence.clone(), true, Vec::new());
        for net in [&pn, &nn] {
            let mut r = net_record(net, NetRole::Diff);
            r.interface = interface;
            r.external = [p, n].iter().any(|k| c.pins(*k).iter().any(|q| c.part(q.part).is_some_and(PartInfo::connector))).then_some(true);
            f.add(Record::Net(r), Confidence::Medium, evidence.clone(), true, Vec::new());
        }
    }
}

/// ESD and TVS parts, of the connector pins their nets reach.
fn protection(c: &Ctx, f: &mut Found) {
    for p in c.parts.values().filter(|p| p.protection()) {
        let nets: Vec<usize> = c.nets_of(p.id).into_iter().collect();
        let of: Vec<String> = nets
            .iter()
            .flat_map(|n| c.pins(*n).iter().filter(|q| c.part(q.part).is_some_and(PartInfo::connector)).map(NetPin::reference))
            .take(super::intent::MAX_MEMBERS)
            .collect();
        if of.is_empty() {
            continue;
        }
        let role = if p.pins >= 3 { PartRole::EsdArray } else { PartRole::Tvs };
        let e = vec![format!("{} protects ({}); its nets reach {}", p.designator, p.value, of.join(", "))];
        f.add(Record::Part(part_record(&p.designator, role, of)), Confidence::High, e, true, Vec::new());
    }
}

/// Whether a cable plugs into a connector, when its kind tells: a USB receptacle, a coaxial RF
/// connector or a wire-to-board one (a battery lead) takes one in use. Why, in a few words.
fn cable_of(p: &PartInfo) -> Option<&'static str> {
    if p.is(&["receptacle", "type-c", "usb-c", "micro-b", "mini-b"]) && p.has_word(&["usb"]) {
        Some("a USB receptacle: a cable plugs into it in use")
    } else if p.is(&["coax", "u.fl", "ipex", "mmcx", "sma ", "sma-", "mhf"]) {
        Some("a coaxial RF connector: a cable to an antenna or an instrument plugs into it")
    } else if p.is(&["wire-to-board", "wire to board", "jst", "picoblade", "battery"]) {
        Some("a wire-to-board connector: a lead (a battery's, say) plugs into it")
    } else {
        None
    }
}

/// Connectors (whether a cable plugs in is proposed when the connector's kind tells, else asked
/// of the user), and debug pads.
fn connectors(c: &Ctx, f: &mut Found) {
    for p in c.parts.values().filter(|p| p.connector()) {
        if p.is(&["swd", "jtag", "debug", "tag-connect", "pogo"]) {
            f.add(
                Record::Part(part_record(&p.designator, PartRole::Debug, Vec::new())),
                Confidence::Medium,
                vec![format!("{} is a debug connector (its description)", p.designator)],
                false,
                Vec::new(),
            );
            continue;
        }
        let mut e = vec![format!("{} is a connector ({})", p.designator, p.value)];
        let mut r = part_record(&p.designator, PartRole::Connector, Vec::new());
        match cable_of(p) {
            Some(why) => {
                r.cable = Some(true);
                e.push(format!("{} is {why} (its description)", p.designator));
            }
            None => e.push("nothing in the design says whether a cable plugs into it".to_string()),
        }
        let needs = r.needs();
        f.add(Record::Part(r), Confidence::Medium, e, true, needs);
    }
}

/// Decoupling and bulk: a mounted capacitor from a supply to ground, of the IC supply pin or
/// power output nearest it. Returns the IC supply pins.
fn decoupling(c: &Ctx, f: &mut Found, claimed: &BTreeSet<u64>) -> Vec<String> {
    let mut power_pins: BTreeSet<String> = BTreeSet::new();
    for (i, n) in c.conn.nets.iter().enumerate() {
        if c.ground.contains(&i) {
            continue;
        }
        // A switch node's pin is no supply to decouple.
        if n.pins.iter().any(|q| c.part(q.part).is_some_and(PartInfo::inductor))
            && !n.pins.iter().any(|q| c.part(q.part).is_some_and(|x| x.capacitor()))
        {
            continue;
        }
        for q in n.pins.iter().filter(|q| c.supply_pin(q, i)) {
            power_pins.insert(q.reference());
        }
    }
    for p in c.parts.values().filter(|p| p.capacitor() && p.mounted() && !claimed.contains(&p.id)) {
        let Some((a, b)) = c.two_nets(p) else { continue };
        let supply = match (c.ground.contains(&a), c.ground.contains(&b)) {
            (true, false) => b,
            (false, true) => a,
            _ => continue,
        };
        let Some(own) = c.pins(supply).iter().find(|q| q.part == p.id) else { continue };
        let served: Vec<&NetPin> = c.pins(supply).iter().filter(|q| c.served_pin(q, supply)).collect();
        // Every distance on one footing: the board's once the capacitor and all these pins are
        // placed, else the sheet's.
        let board = c.placed(own) && served.iter().all(|q| c.placed(q));
        let mut near: Vec<(&NetPin, f64, bool)> =
            served.into_iter().map(|q| if board { (q, c.reach(own, q).0, true) } else { (q, dist(own.at, q.at), false) }).collect();
        near.sort_by(|x, y| x.1.total_cmp(&y.1));
        let farads = parse_value(&p.value).filter(|(_, u)| *u == 'F' || *u == '?').map(|(v, _)| v);
        let bulk = farads.is_some_and(|v| v >= BULK_FARADS - 1e-12);
        let Some((pin, d, on_board)) = near.first() else {
            if bulk || c.rails.contains(&supply) {
                let e = vec![format!("{} ({}) runs from {} to ground, near no IC supply pin", p.designator, p.value, c.net_name(supply))];
                f.add(
                    Record::Part(part_record(&p.designator, PartRole::Bulk, vec![c.net_name(supply).to_string()])),
                    Confidence::Low,
                    e,
                    false,
                    Vec::new(),
                );
            }
            continue;
        };
        // Sure of the IC when no other IC's pin (or other pin by name: `OUT` twice is one) is
        // nearly as near.
        let rival = near.iter().skip(1).find(|(q, ..)| q.part != pin.part || q.name != pin.name);
        let unique = rival.is_none_or(|(_, d2, _)| *d2 > d * 1.5 + 0.5);
        let confidence = match (on_board, unique) {
            (true, true) => Confidence::Medium,
            _ => Confidence::Low,
        };
        let where_ = if *on_board { "on the board" } else { "on the sheet" };
        let what = if c.supply_pin(pin, supply) { "IC supply pin" } else { "IC power output" };
        let mut e = vec![format!("{} ({}) runs from {} to ground", p.designator, p.value, c.net_name(supply)), {
            let raw = if *on_board { dist(c.at(own).0, c.at(pin).0) } else { *d };
            let side = if (*d - raw).abs() > 1e-9 { " on the other side" } else { "" };
            format!("the nearest {what} {where_} is {} ({}), {} mm away{side}", pin.reference(), pin.name, mm(raw))
        }];
        let role = if bulk { PartRole::Bulk } else { PartRole::Decoupling };
        if bulk {
            e.push(format!("{} is 4.7 µF or more: bulk", p.value));
        }
        f.add(Record::Part(part_record(&p.designator, role, vec![pin.reference()])), confidence, e, !bulk, Vec::new());
    }
    power_pins.into_iter().collect()
}

/// The other signals: buses and clocks by pin and net names, programming and sense pins,
/// supplies of one IC, and the rest as slow.
fn signals(c: &Ctx, f: &mut Found) {
    for (i, n) in c.conn.nets.iter().enumerate() {
        let key = format!("net|{}", n.name);
        if f.has(&key) || c.ground.contains(&i) {
            continue;
        }
        let external =
            n.pins.iter().any(|q| c.part(q.part).is_some_and(|p| p.connector() && !p.is(&["swd", "jtag", "debug", "tag-connect"]))).then_some(true);
        let ic_pins: Vec<&NetPin> = n.pins.iter().filter(|q| c.is_ic_pin(q)).collect();
        // A bus, by the net's name or its pins'.
        let bus = interface_of(&n.name)
            .map(|x| (x, format!("named {}", n.name)))
            .or_else(|| ic_pins.iter().find_map(|q| interface_of(&q.name).map(|x| (x, format!("{} is {}", q.reference(), q.name)))));
        if let Some(((interface, clock), why)) = bus {
            let mut r = net_record(&n.name, if clock { NetRole::Clock } else { NetRole::Interface });
            (r.interface, r.external) = (Some(interface), external);
            f.add(Record::Net(r), Confidence::Medium, vec![format!("{why}: {}", interface.name())], true, Vec::new());
            continue;
        }
        let clock = ic_pins.iter().find(|q| {
            let u = upper(&q.name);
            ["CLK", "XC1", "XC2", "XL1", "XL2", "XTAL", "OSC", "MCLK", "BCLK"]
                .iter()
                .any(|w| u.split(|ch: char| !ch.is_ascii_alphanumeric()).any(|x| x.starts_with(w)))
        });
        if let Some(q) = clock {
            let mut r = net_record(&n.name, NetRole::Clock);
            r.external = external;
            f.add(Record::Net(r), Confidence::Medium, vec![format!("{} is {}", q.reference(), q.name)], true, Vec::new());
            continue;
        }
        let pin_role = ic_pins.iter().find_map(|q| {
            let u = upper(&q.name);
            let u = u.trim_start_matches(['~', '{']);
            if ["ISET", "ILIM", "IREF", "RSET", "RT", "PROG", "ITERM"].iter().any(|w| u.starts_with(w)) {
                Some((NetRole::Static, q))
            } else if ["TS", "NTC", "SENSE", "ISNS", "CSP", "CSN", "THERM"].contains(&u) {
                Some((NetRole::Sense, q))
            } else {
                None
            }
        });
        if let Some((role, q)) = pin_role {
            let r = net_record(&n.name, role);
            f.add(Record::Net(r), Confidence::Medium, vec![format!("{} is {}", q.reference(), q.name)], false, Vec::new());
            continue;
        }
        // A switch node no regulator was found for: an IC pin and an inductor, no capacitor to
        // ground.
        let inductor = n.pins.iter().any(|q| c.part(q.part).is_some_and(PartInfo::inductor));
        let decoupled = n.pins.iter().any(|q| {
            c.part(q.part).is_some_and(|p| p.capacitor() && c.two_nets(p).is_some_and(|(a, b)| c.ground.contains(&a) || c.ground.contains(&b)))
        });
        if let (true, false, Some(q)) = (inductor, decoupled, ic_pins.first()) {
            let mut r = net_record(&n.name, NetRole::SwNode);
            r.domain = c.part(q.part).map(|p| p.designator.clone());
            let e = vec![format!("{} ({}) meets an inductor, and no capacitor goes to ground", q.reference(), q.name)];
            f.add(Record::Net(r), Confidence::Low, e, false, Vec::new());
            continue;
        }
        // An IC's own supply node: its supply pins and capacitors, nothing else drives it.
        let supply: Vec<&&NetPin> = ic_pins.iter().filter(|q| c.supply_pin(q, i)).collect();
        if let Some(q) = supply.first() {
            let mut r = net_record(&n.name, NetRole::Power);
            r.domain = c.part(q.part).map(|p| p.designator.clone());
            let ic = c.part(q.part).map_or(String::new(), |p| p.designator.clone());
            let e = vec![format!("{} ({}) is a supply pin, and the net is {ic}'s own: no rail port, no other IC", q.reference(), q.name)];
            f.add(Record::Net(r), Confidence::Low, e, false, Vec::new());
            continue;
        }
        let mut r = net_record(&n.name, NetRole::Slow);
        r.external = external;
        let e = vec!["no faster role found: no clock, bus, RF or supply pin, no pair".to_string()];
        f.add(Record::Net(r), Confidence::Low, e, false, Vec::new());
    }
}

/// Fiducials, mounting holes, test points, and series terminations.
fn small_parts(c: &Ctx, f: &mut Found) {
    for p in c.parts.values() {
        let key = format!("part|{}", p.designator);
        if f.has(&key) {
            continue;
        }
        let role = if p.prefix == "FID" || p.is(&["fiducial"]) {
            Some((PartRole::Fiducial, "a fiducial"))
        } else if matches!(p.prefix.as_str(), "H" | "MH") || p.is(&["mounting hole"]) {
            Some((PartRole::Mounting, "a mounting hole"))
        } else if p.prefix == "TP" || p.is(&["test point"]) {
            Some((PartRole::TestPoint, "a test point"))
        } else {
            None
        };
        if let Some((role, what)) = role {
            f.add(
                Record::Part(part_record(&p.designator, role, Vec::new())),
                Confidence::High,
                vec![format!("{} is {what}", p.designator)],
                false,
                Vec::new(),
            );
            continue;
        }
        // A small resistor in series between an IC's output and another IC's input.
        if p.resistor()
            && parse_value(&p.value).is_some_and(|(v, _)| v > 1.0 && v <= 100.0)
            && let Some((a, b)) = c.two_nets(p)
            && ![a, b].iter().any(|n| c.ground.contains(n) || c.rails.contains(n))
        {
            let drives = |n: usize| {
                c.pins(n).iter().any(|q| c.is_ic_pin(q) && matches!(q.electrical, PinType::Output | PinType::Bidirectional | PinType::TriState))
            };
            let takes = |n: usize| c.pins(n).iter().any(|q| c.is_ic_pin(q) && matches!(q.electrical, PinType::Input | PinType::Bidirectional));
            if (drives(a) && takes(b)) || (drives(b) && takes(a)) {
                let e = vec![format!("{} ({}) runs between {} and {}, IC pins at both ends", p.designator, p.value, c.net_name(a), c.net_name(b))];
                let of = vec![c.net_name(a).to_string(), c.net_name(b).to_string()];
                f.add(Record::Part(part_record(&p.designator, PartRole::SeriesTerm, of)), Confidence::Medium, e, false, Vec::new());
            }
        }
    }
}

/// A blocking role still to settle, or a fact still to give.
#[derive(Clone, Debug, PartialEq)]
pub struct Missing {
    /// As [`Record::key`].
    pub key: String,
    pub role: &'static str,
    /// What it lacks: `confirm` (a look at the proposal), or the fields to give.
    pub needs: Vec<&'static str>,
    pub why: &'static str,
}

/// How much of the blocking intent is settled.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Coverage {
    pub blocking_total: usize,
    pub blocking_confirmed: usize,
    pub missing: Vec<Missing>,
    /// The ICs' supply pins, those a kept decoupling or bulk record names, and those a proposal
    /// names.
    pub ic_power_pins: usize,
    pub ic_power_pins_assigned: usize,
    pub ic_power_pins_proposed: usize,
    /// Records naming nets or parts the schematic doesn't have.
    pub unknown: Vec<String>,
}

/// Why a blocking role matters, or, when it lacks a fact only the product tells, the question to
/// ask the user in plain words.
fn why(role: &str, needs: &[&str]) -> &'static str {
    if needs.contains(&"cable") {
        return "ask the user: does a cable plug into it when the board is in use (a USB lead, an antenna cable, a battery lead)? A cable carries noise in and out, so the EMC checks need to know";
    }
    if needs.contains(&"current_a") {
        return "ask the user: how much current does it carry at most, in amperes? Nothing in the design gives it (no regulator's rating), and it sets how wide its copper must be";
    }
    if needs.contains(&"bands") {
        return "ask the user: which radio bands the board works in (for a sub-GHz radio, the region's: EU868 or US915); they set the coexistence check";
    }
    match role {
        "power" => "a supply's voltage and current size its copper and its decoupling",
        "ground" => "return paths and pours are checked against ground",
        "rf" => "an RF line is checked for its impedance, reference and keepouts",
        "clock" => "a clock is kept short, away from other signals and off layer changes",
        "diff" | "pair" => "a pair is routed and checked together",
        "interface" => "a bus's lines are checked as a group",
        "decoupling" => "a decoupling loop is checked against the pin it serves",
        "crystal" => "a crystal's loop and keepout are checked",
        "tvs" | "esd_array" => "protection is checked to sit between the connector and the rest",
        "connector" => "a cable makes a board an antenna: the EMC checks need to know",
        "antenna" => "an antenna's keepout and feed are checked",
        "smps_ic" => "a regulator's hot loops and switch node are checked",
        "board" => "the radio bands set the coexistence check",
        _ => "the checks that need this role can't run without it",
    }
}

/// How much of `intent` is settled against what `inference` proposes: each blocking subject (a
/// proposal's, or a kept record's with a blocking role) counts once, settled when a declared or
/// confirmed record has it with nothing left to give.
pub fn coverage(ecad: &Electronics, intent: &Intent, inference: &Inference) -> Coverage {
    let mut out = Coverage { ic_power_pins: inference.power_pins.len(), ..Coverage::default() };
    let kept: BTreeMap<String, (Record, bool)> = intent
        .nets
        .iter()
        .map(|r| (Record::Net(r.clone()), r.source.settled()))
        .chain(intent.parts.iter().map(|r| (Record::Part(r.clone()), r.source.settled())))
        .chain(intent.pairs.iter().map(|r| (Record::Pair(r.clone()), r.source.settled())))
        .map(|(r, settled)| (r.key(), (r, settled)))
        .collect();
    let mut subjects: BTreeMap<String, (&'static str, bool)> = BTreeMap::new();
    for p in &inference.proposals {
        if p.blocking {
            subjects.insert(p.key.clone(), (p.record.role(), true));
        }
    }
    for (k, (r, _)) in &kept {
        let blocking = match r {
            Record::Net(n) => n.role.blocking(),
            Record::Part(p) => p.role.blocking(),
            Record::Pair(_) => true,
            _ => false,
        };
        // A record kept as the engine proposed it is as blocking as the proposal was (an IC's own
        // supply node is power, but no rail to give a current for).
        let blocking = match inference.get(k) {
            Some(p) if p.record.role() == r.role() => p.blocking,
            _ => blocking,
        };
        // A kept record overrides a proposal's view of the subject.
        if blocking {
            subjects.insert(k.clone(), (r.role(), true));
        } else {
            subjects.remove(k);
        }
    }
    for (key, (role, _)) in &subjects {
        out.blocking_total += 1;
        let (needs, settled): (Vec<&'static str>, bool) = if key == "board" {
            let bands = intent.board.bands.is_some();
            (if bands { Vec::new() } else { vec!["bands"] }, bands)
        } else {
            match kept.get(key) {
                Some((r, settled)) => {
                    let needs = match r {
                        Record::Net(n) if inference.get(key).is_some_and(|p| !p.blocking && p.record.role() == r.role()) => Vec::new(),
                        Record::Net(n) => n.needs(),
                        Record::Part(p) => p.needs(),
                        _ => Vec::new(),
                    };
                    let done = *settled && needs.is_empty();
                    (if *settled { needs } else { vec!["confirm"] }, done)
                }
                None => {
                    let mut needs = vec!["confirm"];
                    needs.extend(inference.get(key).map(|p| p.needs.clone()).unwrap_or_default());
                    (needs, false)
                }
            }
        };
        if settled {
            out.blocking_confirmed += 1;
        } else {
            let why = why(role, &needs);
            out.missing.push(Missing { key: key.clone(), role, needs, why });
        }
    }
    let powered: BTreeSet<&str> = inference.power_pins.iter().map(String::as_str).collect();
    let named = |records: &mut dyn Iterator<Item = &PartIntent>| -> BTreeSet<String> {
        records
            .filter(|r| matches!(r.role, PartRole::Decoupling | PartRole::Bulk))
            .flat_map(|r| r.of.iter().cloned())
            .filter(|p| powered.contains(p.as_str()))
            .collect()
    };
    out.ic_power_pins_assigned = named(&mut intent.parts.iter()).len();
    let proposed: Vec<&PartIntent> =
        inference.proposals.iter().filter_map(|p| if let Record::Part(r) = &p.record { Some(r) } else { None }).collect();
    out.ic_power_pins_proposed = named(&mut proposed.into_iter()).len();
    let conn = ecad.connectivity();
    let designators: BTreeSet<&str> = ecad.schematic.parts.iter().map(|p| p.designator.as_str()).collect();
    for r in &intent.nets {
        if conn.index_of(&r.net).is_none() {
            out.unknown.push(format!("net|{}", r.net));
        }
    }
    for r in &intent.parts {
        if !designators.contains(r.part.as_str()) {
            out.unknown.push(format!("part|{}", r.part));
        }
    }
    out
}
