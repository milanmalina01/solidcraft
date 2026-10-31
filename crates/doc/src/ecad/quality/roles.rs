//! What the checks know of the design's intent (see [`super::super::intent`]): each board net's
//! role and record, its edge-rate tier and where that came from, the ground nets, the parts by
//! role, and the board's frequency of concern. Only kept records count (declared, confirmed, or
//! accepted as proposed); a proposal nobody accepted is not intent.
//!
//! **Tiers** (owned by `signal-integrity#edge-rates`): T0 is a rise time of 5 ns or more, or a
//! static, DC or slow analog net; T1 0.5–5 ns; T2 0.1–0.5 ns; T3 below 0.1 ns. The rise time
//! comes, in this order, from the net's declared `edge_ns`; its `driver` class (standard drive
//! 3 ns, high drive 1 ns, LVDS 0.3 ns, SerDes 0.05 ns, a clock 0.07/f); its interface (USB full
//! speed is slew-limited, T0; USB high speed T2), its own or its kept pair's; its role (ground,
//! power, static, slow, analog, sense and feedback nets are T0: a kept `slow` record says its
//! edges don't matter, a LED, a button, a reset); else, for a net an IC pin drives (an output,
//! bidirectional, three-state or open-drain pin), the standard drive's 3 ns, marked as assumed
//! until a driver or edge is declared: tier-gated checks give warnings at most on an assumed tier.
//! Never from how often the net toggles. Clock, RF and switch-node roles are T1 at least whatever
//! else says, except a crystal's own nets (a near-sine at its frequency: T0, left to the crystal
//! checks). A net with none of these is unverified: tier-gated checks report it as not
//! applicable.

use std::collections::{BTreeMap, BTreeSet};

use super::super::Electronics;
use super::super::airwires::BoardNets;
use super::super::connect::Connectivity;
use super::super::intent::{Driver, Intent, Interface, NetIntent, NetRole, PairIntent, PartIntent, PartRole};
use super::super::symbol::PinType;

/// The standard drive's rise time, in nanoseconds: the fast end of an MCU's 3–10 ns.
pub const STANDARD_NS: f64 = 3.0;
/// High drive's: the fast end of 1–3 ns.
pub const HIGH_NS: f64 = 1.0;
pub const LVDS_NS: f64 = 0.3;
pub const SERDES_NS: f64 = 0.05;
/// USB full speed's slowest legal edge is 4 ns and its drivers limit their slew: the guide calls
/// it T0.
pub const USB_FS_NS: f64 = 5.0;
pub const USB_HS_NS: f64 = 0.4;

/// An edge-rate tier (see the module notes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    T0,
    T1,
    T2,
    T3,
}

impl Tier {
    /// The tier of a rise time in nanoseconds.
    pub fn of(rt_ns: f64) -> Tier {
        if rt_ns >= 5.0 {
            Tier::T0
        } else if rt_ns >= 0.5 {
            Tier::T1
        } else if rt_ns >= 0.1 {
            Tier::T2
        } else {
            Tier::T3
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tier::T0 => "T0",
            Tier::T1 => "T1",
            Tier::T2 => "T2",
            Tier::T3 => "T3",
        }
    }
}

/// Where a net's rise time comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edge {
    /// The record's `edge_ns`.
    Declared,
    /// The record's driver class.
    Driver(Driver),
    /// The record's interface.
    Interface(Interface),
    /// The record's role: a DC or slow net (T0), or a role that is T1 at least.
    Role(NetRole),
    /// An IC pin drives it and nothing is declared: the standard drive, until confirmed.
    Assumed,
    /// No record, nothing drives it, and supply pins are on it: a supply (DC).
    Supply,
    /// A pin of a kept crystal is on it: a near-sine, which the crystal checks own.
    Crystal,
}

impl Edge {
    /// How a finding words it.
    pub fn says(self, rt_ns: Option<f64>) -> String {
        let ns = rt_ns.map_or(String::new(), |v| format!(", {} ns", fmt(v)));
        match self {
            Edge::Declared => format!("declared edge{ns}"),
            Edge::Driver(d) => format!("{} driver{ns}", driver_name(d)),
            Edge::Interface(i) => format!("{} edges{ns}", i.name()),
            Edge::Role(r) => format!("{} role", r.name()),
            Edge::Assumed => format!("assumed standard drive{ns}: declare the net's driver or edge_ns to confirm"),
            Edge::Supply => "supply pins, no record".into(),
            Edge::Crystal => "a crystal's net, which the crystal checks judge".into(),
        }
    }
}

fn driver_name(d: Driver) -> &'static str {
    match d {
        Driver::Standard => "standard",
        Driver::High => "high-drive",
        Driver::Lvds => "LVDS",
        Driver::Serdes => "SerDes",
        Driver::Clock => "clock",
    }
}

fn fmt(v: f64) -> String {
    let s = format!("{:.3}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// What the checks know of one net.
#[derive(Clone, Debug)]
pub struct NetInfo<'a> {
    /// Its index into the board's net names.
    pub index: usize,
    pub name: &'a str,
    /// Its kept record, if any.
    pub record: Option<&'a NetIntent>,
    /// An IC pin drives it.
    pub driven: bool,
    /// No record says what it is, nothing drives it, and supply pins (power inputs or outputs)
    /// are on it: taken for a supply.
    pub supply_pins: bool,
    /// Its rise time in nanoseconds, its tier and where they come from; `None` while unverified.
    pub rt_ns: Option<f64>,
    pub tier: Option<Tier>,
    pub edge: Option<Edge>,
}

impl NetInfo<'_> {
    pub fn role(&self) -> Option<NetRole> {
        self.record.map(|r| r.role)
    }

    pub fn has_role(&self, role: NetRole) -> bool {
        self.role() == Some(role)
    }

    /// Ground or a supply (by its record, or by its pins when it has none): a plane's net, not
    /// a signal.
    pub fn is_supply(&self) -> bool {
        matches!(self.role(), Some(NetRole::Ground | NetRole::Power)) || self.supply_pins
    }

    /// Its tier is T1 or faster.
    pub fn fast(&self) -> bool {
        self.tier.is_some_and(|t| t >= Tier::T1)
    }

    /// Its tier is the standard drive's, assumed with nothing declared: what rests on it is a
    /// warning at most, until a driver or edge_ns says so.
    pub fn assumed(&self) -> bool {
        self.edge == Some(Edge::Assumed)
    }

    /// Its frequency of concern in MHz, when the record gives one.
    pub fn f_mhz(&self) -> Option<f64> {
        self.record.and_then(|r| r.f_mhz)
    }

    /// The tier and where it comes from, as a finding's `regime` words it.
    pub fn regime(&self) -> String {
        match (self.tier, self.edge) {
            (Some(t), Some(e)) => format!("{} ({})", t.name(), e.says(self.rt_ns)),
            _ => "unverified".into(),
        }
    }

    /// What a tier-gated check needs of an unverified net, as `not_applicable` lists it.
    pub fn needs_edge(&self) -> String {
        format!("intent: driver or edge_ns of {}", self.name)
    }
}

/// The checks' view of the intent (see the module notes).
pub struct Roles<'a> {
    pub intent: &'a Intent,
    /// By board net index.
    pub nets: Vec<NetInfo<'a>>,
    by_name: BTreeMap<&'a str, usize>,
    /// The nets kept as ground.
    pub grounds: BTreeSet<usize>,
}

/// The rise time (none for a DC or slow net), tier and source a record gives (with the interface
/// of its kept pair, `pair`, when it names none), before the role's floor.
fn record_edge(r: &NetIntent, pair: Option<Interface>) -> Option<(Option<f64>, Tier, Edge)> {
    let timed = |rt: f64, e: Edge| Some((Some(rt), Tier::of(rt), e));
    if let Some(e) = r.edge_ns {
        return timed(e, Edge::Declared);
    }
    if let Some(d) = r.driver {
        let rt = match d {
            Driver::Standard => STANDARD_NS,
            Driver::High => HIGH_NS,
            Driver::Lvds => LVDS_NS,
            Driver::Serdes => SERDES_NS,
            // Only a clock whose edges scale with its frequency uses 0.07/f.
            Driver::Clock => r.f_mhz.filter(|f| *f > 0.0).map_or(STANDARD_NS, |f| 70.0 / f),
        };
        return timed(rt, Edge::Driver(d));
    }
    match r.interface.or(pair) {
        Some(i @ Interface::Usb2Fs) => return timed(USB_FS_NS, Edge::Interface(i)),
        Some(i @ Interface::Usb2Hs) => return timed(USB_HS_NS, Edge::Interface(i)),
        _ => {}
    }
    match r.role {
        NetRole::Ground | NetRole::Power | NetRole::Static | NetRole::Slow | NetRole::Analog | NetRole::Sense | NetRole::Fb => {
            Some((None, Tier::T0, Edge::Role(r.role)))
        }
        _ => None,
    }
}

impl<'a> Roles<'a> {
    /// The view of `ecad`'s intent over the board's nets `nets` (named as `conn` names them).
    pub fn new(ecad: &'a Electronics, nets: &'a BoardNets, conn: &Connectivity) -> Roles<'a> {
        let intent = &ecad.intent;
        let records: BTreeMap<&str, &NetIntent> = intent.nets.iter().map(|r| (r.net.as_str(), r)).collect();
        // The interface each kept pair gives its two nets.
        let mut pair_interface: BTreeMap<&str, Interface> = BTreeMap::new();
        for p in &intent.pairs {
            if let Some(i) = p.interface {
                pair_interface.insert(p.p.as_str(), i);
                pair_interface.insert(p.n.as_str(), i);
            }
        }
        // The nets a kept crystal's pads are on (ground left out below, by its record).
        let crystals: BTreeSet<&str> = intent.parts.iter().filter(|p| p.role == PartRole::Crystal).map(|p| p.part.as_str()).collect();
        let crystal_nets: BTreeSet<usize> = nets.pads.iter().filter(|p| crystals.contains(p.designator.as_str())).filter_map(|p| p.net).collect();
        let mut out = Vec::with_capacity(nets.names.len());
        let mut grounds = BTreeSet::new();
        for (i, name) in nets.names.iter().enumerate() {
            let record = records.get(name.as_str()).copied();
            let pins = conn.net(name).map(|n| n.pins.as_slice()).unwrap_or_default();
            let driven = pins.iter().any(|p| {
                matches!(p.electrical, PinType::Output | PinType::Bidirectional | PinType::TriState | PinType::OpenCollector | PinType::OpenEmitter)
            });
            let supply_pins = record.is_none() && !driven && pins.iter().any(|p| matches!(p.electrical, PinType::PowerInput | PinType::PowerOutput));
            let pair = pair_interface.get(name.as_str()).copied();
            let mut edge = record.and_then(|r| record_edge(r, pair));
            let crystal = crystal_nets.contains(&i) && !record.is_some_and(|r| matches!(r.role, NetRole::Ground | NetRole::Power));
            if crystal && record.is_none_or(|r| r.edge_ns.is_none() && r.driver.is_none()) {
                edge = Some((None, Tier::T0, Edge::Crystal));
            }
            if edge.is_none() && driven {
                // A clock's, an RF line's or a switch node's own role says more than a guess.
                let role = record.map(|r| r.role).filter(|r| matches!(r, NetRole::Clock | NetRole::Rf | NetRole::SwNode));
                edge = Some((Some(STANDARD_NS), Tier::T1, role.map_or(Edge::Assumed, Edge::Role)));
            }
            if edge.is_none() && supply_pins {
                edge = Some((None, Tier::T0, Edge::Supply));
            }
            let (mut rt, mut tier, mut from) = match edge {
                Some((rt, t, e)) => (rt, Some(t), Some(e)),
                None => (None, None, None),
            };
            // Clock, RF and switch-node roles are T1 at least.
            let floor = !crystal && record.is_some_and(|r| matches!(r.role, NetRole::Clock | NetRole::Rf | NetRole::SwNode));
            if floor && tier.is_none_or(|t| t < Tier::T1) {
                (rt, tier, from) = (None, Some(Tier::T1), record.map(|r| Edge::Role(r.role)));
            }
            if record.is_some_and(|r| r.role == NetRole::Ground) {
                grounds.insert(i);
            }
            out.push(NetInfo { index: i, name: name.as_str(), record, driven, supply_pins, rt_ns: rt, tier, edge: from });
        }
        let by_name = out.iter().map(|n| (n.name, n.index)).collect();
        Roles { intent, nets: out, by_name, grounds }
    }

    pub fn net(&self, i: usize) -> Option<&NetInfo<'a>> {
        self.nets.get(i)
    }

    pub fn by_name(&self, name: &str) -> Option<&NetInfo<'a>> {
        self.by_name.get(name).and_then(|i| self.nets.get(*i))
    }

    /// The kept part records with `role`.
    pub fn parts(&self, role: PartRole) -> impl Iterator<Item = &'a PartIntent> + '_ {
        self.intent.parts.iter().filter(move |p| p.role == role)
    }

    pub fn pairs(&self) -> &'a [PairIntent] {
        &self.intent.pairs
    }

    /// The board's highest frequency of concern in MHz and what gives it: the intent's
    /// `f_concern_mhz`, else the top of its highest radio band (as the stitching tool takes it);
    /// `None` when neither is declared.
    pub fn f_concern(&self) -> Option<(f64, String)> {
        let b = &self.intent.board;
        if let Some(f) = b.f_concern_mhz {
            return Some((f, format!("the board's frequency of concern, {} MHz", fmt(f))));
        }
        let band = b.bands.as_ref()?.iter().max_by(|x, y| x.hi_mhz.total_cmp(&y.hi_mhz))?;
        Some((band.hi_mhz, format!("the top of the {} band, {} MHz", band.name, fmt(band.hi_mhz))))
    }
}
