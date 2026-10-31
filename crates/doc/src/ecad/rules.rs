//! The board's design rules: the sizes new tracks and vias get, the least gaps and sizes the
//! design rule check holds copper and holes to, and the net classes that give some nets sizes of
//! their own. A net in no class is in the default class, whose values are the rules' own. Between
//! two pieces of copper the larger of their classes' clearances applies. A class may also keep
//! the current its nets carry (with the temperature rise allowed and the copper), from which the
//! current calculator proposes its width; the width it has is what routes and is checked.

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use super::check_text;
use super::connect::Connectivity;
use super::current::{COPPER, CURRENT, RISE, mm_to_oz, oz_to_mm};
use super::footprint::MIN_DRILL;
use super::schematic::Schematic;

/// The most net classes a board has.
pub const MAX_NET_CLASSES: usize = 64;
/// The most nets all the classes name together.
pub const MAX_CLASS_NETS: usize = 16384;
/// The longest class name, in characters.
pub const MAX_CLASS_NAME: usize = 32;
/// The longest net name a class lists, in characters.
pub const MAX_NET_NAME: usize = 128;
/// The name of the class every net without one is in; no class of the board's has it.
pub const DEFAULT_CLASS: &str = "Default";
/// The largest size or gap a rule takes, in millimetres.
pub const MAX_RULE: f64 = 10.0;
/// The widest and narrowest track a rule gives, in millimetres.
pub const RULE_TRACK: std::ops::RangeInclusive<f64> = 0.01..=20.0;

/// A differential pair's track width and the gap between its two tracks, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffPair {
    pub width: f64,
    pub gap: f64,
}

/// Nets that share sizes: a power class with wide tracks, an RF class with more clearance, a
/// differential pair's width and gap.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetClass {
    pub name: String,
    /// The schematic nets in it, by name; a name the schematic has no net for is kept (and
    /// listed as unknown) so a class can be set up before the schematic has the net.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nets: Vec<String>,
    /// In millimetres.
    pub track_width: f64,
    pub clearance: f64,
    pub via_diameter: f64,
    pub via_drill: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_pair: Option<DiffPair>,
    /// The current its nets carry, in amperes, so the class keeps why it has its width: the
    /// current calculator (`super::current`) proposes a width from it, while `track_width` stays
    /// what new tracks get and what the check holds them to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<f64>,
    /// The temperature rise allowed at that current, in °C; [`super::current::DEFAULT_RISE`]
    /// when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temp_rise: Option<f64>,
    /// The copper's thickness the width is for, in oz/ft²; left out, the stack-up's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copper_oz: Option<f64>,
}

/// The design rules, in millimetres (see the module notes). The defaults' minimums are the
/// standard prototype fab profile's (see [`super::profiles`]), which new boards get.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rules {
    /// What new tracks get.
    pub track_width: f64,
    /// The least gap between copper of two nets, on one layer.
    pub clearance: f64,
    /// What new vias get.
    pub via_diameter: f64,
    pub via_drill: f64,
    /// The narrowest track the check passes.
    pub min_track_width: f64,
    /// The narrowest copper ring round a via's or a through-hole pad's drill.
    pub min_annular_ring: f64,
    /// The smallest drill, for vias and holes (micro vias: [`Rules::microvia_drill`]).
    pub min_drill: f64,
    /// The least gap between the edges of two drilled holes.
    pub hole_to_hole: f64,
    /// The least gap between copper and the board's edge or an unplated hole.
    pub copper_to_edge: f64,
    /// The least gap between silkscreen and a pad.
    pub silk_to_pad: f64,
    /// What new micro vias (between two neighbouring layers) get; a via between neighbouring
    /// layers no wider than this is a micro via, and its drill may be as small as
    /// `microvia_drill`.
    pub microvia_diameter: f64,
    pub microvia_drill: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub net_classes: Vec<NetClass>,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            track_width: 0.25,
            clearance: 0.2,
            via_diameter: 0.6,
            via_drill: 0.3,
            min_track_width: 0.15,
            min_annular_ring: 0.15,
            min_drill: 0.3,
            hole_to_hole: 0.25,
            copper_to_edge: 0.3,
            silk_to_pad: 0.15,
            microvia_diameter: 0.3,
            microvia_drill: 0.1,
            net_classes: Vec::new(),
        }
    }
}

/// A gap or size rule: finite, 0 or more, at most [`MAX_RULE`].
fn check_gap(name: &str, v: f64) -> Result<(), String> {
    if !(v.is_finite() && (0.0..=MAX_RULE).contains(&v)) {
        return Err(format!("the rules' `{name}` must be 0 to {MAX_RULE} mm"));
    }
    Ok(())
}

/// A via size: the drill at least [`MIN_DRILL`], the via wider than its drill and at most
/// [`MAX_RULE`].
fn check_via_size(what: &str, diameter: f64, drill: f64) -> Result<(), String> {
    if !(drill.is_finite() && drill >= MIN_DRILL) {
        return Err(format!("{what}: the drill must be at least {MIN_DRILL} mm"));
    }
    if !(diameter.is_finite() && diameter > drill && diameter <= MAX_RULE) {
        return Err(format!("{what}: the diameter must be more than the drill and at most {MAX_RULE} mm"));
    }
    Ok(())
}

/// A track width a rule gives: within [`RULE_TRACK`].
fn check_track(what: &str, w: f64) -> Result<(), String> {
    if !RULE_TRACK.contains(&w) {
        return Err(format!("{what} must be {} to {} mm", RULE_TRACK.start(), RULE_TRACK.end()));
    }
    Ok(())
}

impl NetClass {
    /// The name's form, sizes in range, the via's drill smaller than the via, a pair's width
    /// and gap positive.
    pub fn validate(&self) -> Result<(), String> {
        check_class_name(&self.name)?;
        let at = |e: String| format!("net class `{}`: {e}", self.name);
        check_track("`track_width`", self.track_width).map_err(at)?;
        if !(self.clearance.is_finite() && self.clearance > 0.0 && self.clearance <= MAX_RULE) {
            return Err(at(format!("`clearance` must be more than 0 and at most {MAX_RULE} mm")));
        }
        check_via_size("its via", self.via_diameter, self.via_drill).map_err(at)?;
        if let Some(d) = self.diff_pair {
            check_track("the pair's `width`", d.width).map_err(at)?;
            if !(d.gap.is_finite() && d.gap > 0.0 && d.gap <= MAX_RULE) {
                return Err(at(format!("the pair's `gap` must be more than 0 and at most {MAX_RULE} mm")));
            }
        }
        for n in &self.nets {
            check_net_name(n).map_err(at)?;
        }
        check_current(self.current, self.temp_rise, self.copper_oz).map_err(at)
    }
}

/// A class's current, rise and copper: each, when given, in the current calculator's range.
pub fn check_current(current: Option<f64>, rise: Option<f64>, copper_oz: Option<f64>) -> Result<(), String> {
    let within = |r: &std::ops::RangeInclusive<f64>, v: Option<f64>| v.is_none_or(|v| v.is_finite() && r.contains(&v));
    if !within(&CURRENT, current) {
        return Err(format!("`current` must be {} to {} A", CURRENT.start(), CURRENT.end()));
    }
    if !within(&RISE, rise) {
        return Err(format!("`temp_rise` must be {} to {} °C", RISE.start(), RISE.end()));
    }
    if !within(&COPPER, copper_oz.map(oz_to_mm)) {
        return Err(format!("`copper_oz` must be {:.2} to {:.1} oz", mm_to_oz(*COPPER.start()), mm_to_oz(*COPPER.end())));
    }
    Ok(())
}

/// A class name: 1 to [`MAX_CLASS_NAME`] characters, no control characters or surrounding
/// spaces, and not [`DEFAULT_CLASS`].
pub fn check_class_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.trim() != name {
        return Err("a net class needs a name with no spaces round it".into());
    }
    check_text("name", name, MAX_CLASS_NAME)?;
    if name.eq_ignore_ascii_case(DEFAULT_CLASS) {
        return Err(format!("`{DEFAULT_CLASS}` is the class of every net in no other class: its values are the rules' own (ecad.board.rules)"));
    }
    Ok(())
}

/// A net name a class lists: 1 to [`MAX_NET_NAME`] characters, no control characters.
pub fn check_net_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a net name can't be empty".into());
    }
    check_text("net", name, MAX_NET_NAME)
}

impl Rules {
    pub fn is_default(&self) -> bool {
        *self == Rules::default()
    }

    /// Each value finite and in its range, vias' drills smaller than the vias, and the classes
    /// valid, with unique names (ignoring case) and each net in one class at most.
    pub fn validate(&self) -> Result<(), String> {
        check_track("the rules' `track_width`", self.track_width)?;
        if !(self.clearance.is_finite() && self.clearance > 0.0 && self.clearance <= MAX_RULE) {
            return Err(format!("the rules' `clearance` must be more than 0 and at most {MAX_RULE} mm"));
        }
        check_via_size("the rules' via", self.via_diameter, self.via_drill)?;
        check_via_size("the rules' micro via", self.microvia_diameter, self.microvia_drill)?;
        for (name, v) in [
            ("min_track_width", self.min_track_width),
            ("min_annular_ring", self.min_annular_ring),
            ("min_drill", self.min_drill),
            ("hole_to_hole", self.hole_to_hole),
            ("copper_to_edge", self.copper_to_edge),
            ("silk_to_pad", self.silk_to_pad),
        ] {
            check_gap(name, v)?;
        }
        if self.net_classes.len() > MAX_NET_CLASSES {
            return Err(format!("a board has at most {MAX_NET_CLASSES} net classes"));
        }
        let mut names = std::collections::BTreeSet::new();
        let mut nets = std::collections::BTreeMap::new();
        let mut total = 0usize;
        for c in &self.net_classes {
            c.validate()?;
            if !names.insert(c.name.to_lowercase()) {
                return Err(format!("two net classes are named `{}`", c.name));
            }
            total = total.saturating_add(c.nets.len());
            for n in &c.nets {
                if let Some(other) = nets.insert(n.as_str(), c.name.as_str()) {
                    return Err(format!("net `{n}` is in two net classes, `{other}` and `{}`", c.name));
                }
            }
        }
        if total > MAX_CLASS_NETS {
            return Err(format!("the net classes name at most {MAX_CLASS_NETS} nets together"));
        }
        Ok(())
    }

    /// The class `net` is in; `None` for the default class.
    pub fn class_of(&self, net: &str) -> Option<&NetClass> {
        self.net_classes.iter().find(|c| c.nets.iter().any(|n| n == net))
    }

    /// Keeps the classes on their nets across a schematic edit that renamed some (Rename Net, a
    /// label added, changed or taken away, a part renumbered under an unnamed net's `net:R1.2`): a
    /// net a class lists that `before` has but `after` has no net of that name for is listed by
    /// the name of the net that `after` (worked out for `sheet`) has its first remaining pin on,
    /// else its first wire, else its first label, unless a class lists that name already. `true`
    /// when a class changed.
    pub fn follow_nets(&mut self, before: &Connectivity, after: &Connectivity, sheet: &Schematic) -> bool {
        let mut listed: BTreeSet<String> = self.net_classes.iter().flat_map(|c| c.nets.iter().cloned()).collect();
        let gone: Vec<String> = listed.iter().filter(|n| after.index_of(n).is_none() && before.index_of(n).is_some()).cloned().collect();
        if gone.is_empty() {
            return false;
        }
        let wires: HashMap<u64, usize> = sheet.wires.iter().zip(&after.wire_nets).map(|(w, k)| (w.id, *k)).collect();
        let labels: HashMap<u64, usize> = sheet.labels.iter().zip(&after.label_nets).map(|(l, k)| (l.id, *k)).collect();
        let mut to: HashMap<String, String> = HashMap::new();
        for n in gone {
            let Some(net) = before.net(&n) else { continue };
            let k = net
                .pins
                .iter()
                .find_map(|p| after.pin_net(p.part, &p.pin))
                .or_else(|| net.wires.iter().find_map(|w| wires.get(w).copied()))
                .or_else(|| net.labels.iter().find_map(|l| labels.get(l).copied()));
            let Some(name) = k.and_then(|k| after.nets.get(k)).map(|m| m.name.clone()) else { continue };
            if name.chars().count() <= MAX_NET_NAME && listed.insert(name.clone()) {
                to.insert(n, name);
            }
        }
        if to.is_empty() {
            return false;
        }
        for n in self.net_classes.iter_mut().flat_map(|c| c.nets.iter_mut()) {
            if let Some(m) = to.get(n.as_str()) {
                n.clone_from(m);
            }
        }
        true
    }

    /// The class named `name` (ignoring case).
    pub fn class(&self, name: &str) -> Option<&NetClass> {
        self.net_classes.iter().find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// The sizes of the class of `net` (the default class without a net or a class).
    pub fn sizes(&self, net: Option<&str>) -> Sizes {
        self.class_sizes(net.and_then(|n| self.class_of(n)))
    }

    /// The sizes of `class` (the default class's without one).
    fn class_sizes(&self, class: Option<&NetClass>) -> Sizes {
        match class {
            Some(c) => Sizes {
                class: Some(c.name.clone()),
                track_width: c.track_width,
                clearance: c.clearance,
                via_diameter: c.via_diameter,
                via_drill: c.via_drill,
                diff_pair: c.diff_pair,
            },
            None => Sizes {
                class: None,
                track_width: self.track_width,
                clearance: self.clearance,
                via_diameter: self.via_diameter,
                via_drill: self.via_drill,
                diff_pair: None,
            },
        }
    }

    /// Each net's sizes, in the order of `names` (a schematic's net names).
    pub fn sizes_by_net(&self, names: &[String]) -> Vec<Sizes> {
        // Each net's class looked up once (the first class that names it, as `class_of`).
        let mut class: HashMap<&str, &NetClass> = HashMap::new();
        for c in &self.net_classes {
            for n in &c.nets {
                class.entry(n.as_str()).or_insert(c);
            }
        }
        names.iter().map(|n| self.class_sizes(class.get(n.as_str()).copied())).collect()
    }
}

/// What one net class gives a net.
#[derive(Clone, Debug, PartialEq)]
pub struct Sizes {
    /// The class's name; `None` for the default class.
    pub class: Option<String>,
    pub track_width: f64,
    pub clearance: f64,
    pub via_diameter: f64,
    pub via_drill: f64,
    pub diff_pair: Option<DiffPair>,
}

/// The clearance between copper of nets `a` and `b` (indexes into `by_net`, see
/// [`Rules::sizes_by_net`]; `None` is the default class): the larger of their classes'.
pub fn clearance_between(rules: &Rules, by_net: &[Sizes], a: Option<usize>, b: Option<usize>) -> f64 {
    let of = |n: Option<usize>| n.and_then(|n| by_net.get(n)).map_or(rules.clearance, |s| s.clearance);
    of(a).max(of(b))
}
