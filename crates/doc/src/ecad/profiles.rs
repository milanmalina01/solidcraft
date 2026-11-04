//! Fab profiles: what a board house can build, as presets a board picks one of. Each figure
//! carries where it comes from (a fab's published capability page, read on the date the profile
//! gives, or the engine's own default where no source gives one), and figures that come only
//! from public summaries of an IPC standard are marked `verify`: the checks that rest on them
//! alone report warnings, never errors, until someone checks them against the standard.
//!
//! Picking a profile sets the rules' minimums from it ([`FabProfile::switch`]: those the user set
//! to another value than the old profile's stay); the user may still set any rule to another value, and the design rule check then reports the rules the profile
//! can't build (`fab_class`). What the rules don't hold (how a ring is measured, the aspect
//! ratio, the gap from a hole to copper on inner layers, micro via depth) the check reads from
//! the profile itself. A board saves the profile's id and the version it picked, so a later
//! change of a profile's figures shows.

use serde::{Deserialize, Serialize};

use super::rules::Rules;

/// The profile new boards get.
pub const DEFAULT_PROFILE: &str = "standard";

/// One figure of a profile, in millimetres (a ratio for aspect ratios).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Figure {
    pub value: f64,
    /// Where it comes from, in a few words.
    pub source: &'static str,
    /// Taken from a public summary of an IPC standard: not checked against the standard itself.
    pub verify: bool,
}

const fn fab(value: f64, source: &'static str) -> Figure {
    Figure { value, source, verify: false }
}

const fn ipc(value: f64, source: &'static str) -> Figure {
    Figure { value, source, verify: true }
}

/// How a profile measures an annular ring (and the hole in an aspect ratio or a hole-to-copper
/// gap): from the finished hole, or from the drill tool, which is the finished hole plus the
/// plating allowance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RingConvention {
    Hole,
    Tool,
}

impl RingConvention {
    pub fn name(self) -> &'static str {
        match self {
            RingConvention::Hole => "hole",
            RingConvention::Tool => "tool",
        }
    }
}

/// What a profile's laser-drilled micro vias may be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Micro {
    /// The smallest laser drill.
    pub drill: Figure,
    /// The most depth / drill.
    pub aspect: Figure,
    /// The deepest one, in millimetres.
    pub depth: Figure,
}

/// A fab profile (see the module notes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FabProfile {
    /// Stable, such as `standard`.
    pub id: &'static str,
    pub name: &'static str,
    /// Goes up whenever a figure changes.
    pub version: u32,
    /// When its sources were read.
    pub read: &'static str,
    pub about: &'static str,
    /// The narrowest track and the least copper gap.
    pub track: Figure,
    pub gap: Figure,
    /// The smallest mechanical drill, as the finished hole.
    pub drill: Figure,
    /// The least annular ring, measured as `ring_convention` says.
    pub ring: Figure,
    pub ring_convention: RingConvention,
    /// The drill tool is the finished hole plus this.
    pub plating_allowance: Figure,
    /// The most board thickness / hole for a mechanical drill (the hole as `ring_convention`).
    pub aspect_ratio: Figure,
    pub hole_to_hole: Figure,
    /// The least gap from copper to a routed board edge.
    pub copper_to_edge: Figure,
    /// On inner layers, the least gap from a via's hole (and a plated hole's) to copper not
    /// joined to it.
    pub drill_to_copper: Figure,
    pub drill_to_copper_pth: Figure,
    pub silk_to_pad: Figure,
    /// The least gap from an open via's ring to a surface-mount pad's copper, so a solder mask
    /// web stays between them and the solder doesn't wick into the via (`via_near_pad`).
    pub via_to_pad: Figure,
    /// How far, in degrees, a track leaving a surface-mount pad off square to its edge still
    /// counts as square to it (`pad_entry` flags a thin one leaving by the corner so).
    pub pad_entry_angle: Figure,
    /// The least opening, in degrees, of the empty gap where a track bends or meets a pad or
    /// another track (`acute_angle`).
    pub acute_angle: Figure,
    /// Laser micro vias; `None` when it builds none.
    pub micro: Option<Micro>,
}

/// A figure from public summaries of fab design-for-manufacture guides, not from a standard or a
/// fab's capability page: the checks that rest on it report warnings.
const fn dfm(value: f64, source: &'static str) -> Figure {
    Figure { value, source, verify: true }
}

const ENTRY: &str = "public layout guides: leave a pad square to its edge, along its long axis; 30° is the engine's tolerance";
const ACID_TRAP: &str = "public fab DFM summaries flag sharp copper pockets (acid traps); 70° is the engine's tolerance for the empty gap's opening";

/// A fab figure read from a search engine's summary of the fab's capability pages, not from the
/// page itself: marked to verify.
const fn summary(value: f64, source: &'static str) -> Figure {
    Figure { value, source, verify: true }
}

const JLC_CAPABILITIES: &str = "jlcpcb.com capabilities (2026-11-04): 0.10/0.10 mm least track and spacing, 1 oz, 1-2 layers";

const EUROCIRCUITS_TOOL: &str = "Eurocircuits: drill tool = finished hole + 0.10 mm";
const NO_SOURCE: &str = "engine default; none of the sources gives one";

/// The presets, plainest first.
pub static PROFILES: &[FabProfile] = &[
    FabProfile {
        id: "standard",
        name: "Standard prototype",
        version: 1,
        read: "2026-10",
        about: "what most prototype fabs build without a price step: 0.15/0.15 mm track and gap, 0.3 mm holes, rings on the finished hole",
        track: fab(0.15, "Eurocircuits pattern class 6 (0.15/0.15 mm); JLCPCB builds 0.10 mm"),
        gap: fab(0.15, "Eurocircuits pattern class 6 (0.15/0.15 mm); JLCPCB builds 0.10 mm"),
        drill: fab(0.3, "JLCPCB vias from 0.15 mm, 0.20 preferred; Eurocircuits drill class C 0.25 mm finished"),
        ring: fab(0.15, "JLCPCB multilayer ring 0.15 mm least (0.20 recommended), on the finished hole"),
        ring_convention: RingConvention::Hole,
        plating_allowance: fab(0.10, EUROCIRCUITS_TOOL),
        aspect_ratio: fab(8.0, "Eurocircuits 1:8 on the drill tool; taken on the finished hole here, which is stricter"),
        hole_to_hole: fab(0.25, NO_SOURCE),
        copper_to_edge: fab(0.3, "JLCPCB 0.2 mm to a routed edge, Eurocircuits keeps legend 0.25 mm: 0.3 mm leaves room"),
        drill_to_copper: fab(0.2, "JLCPCB inner layers: 0.2 mm from via holes"),
        drill_to_copper_pth: fab(0.3, "JLCPCB inner layers: 0.3 mm from plated through holes"),
        silk_to_pad: fab(0.15, "JLCPCB legend 0.15 mm from pads"),
        via_to_pad: dfm(0.25, "public DFM summaries: about 0.25 mm of solder mask web between an open via and an SMD pad"),
        pad_entry_angle: dfm(30.0, ENTRY),
        acute_angle: dfm(70.0, ACID_TRAP),
        micro: None,
    },
    FabProfile {
        id: "jlcpcb_2layer",
        name: "JLCPCB 2-layer standard",
        version: 1,
        read: "2026-11",
        about: "JLCPCB's 2-layer 1 oz FR-4 board at its standard price: 0.10/0.10 mm track and gap, 0.3 mm via holes, rings on the finished hole; read from jlcpcb.com's PCB capabilities page on 2026-11-04",
        track: fab(0.10, JLC_CAPABILITIES),
        gap: fab(0.10, JLC_CAPABILITIES),
        drill: fab(
            0.3,
            "jlcpcb.com capabilities (2026-11-04): 0.15 mm least hole on 2 layers, 0.2 mm the preferred least, and 0.2-0.25 mm holes under a 0.45 mm via cost more: 0.3 mm keeps the standard price",
        ),
        ring: fab(
            0.15,
            "jlcpcb.com capabilities (2026-11-04): a via 0.1 mm (0.15 mm preferred) over its hole, a plated hole's ring 0.18 mm least and 0.25 mm advised (2 layers, 1 oz): 0.15 mm, one figure for both",
        ),
        ring_convention: RingConvention::Hole,
        plating_allowance: summary(0.10, "jlcpcb.com: the drill tool is about 0.1 mm over the finished hole; not on the capabilities page, verify"),
        aspect_ratio: summary(8.0, NO_SOURCE),
        hole_to_hole: fab(0.45, "jlcpcb.com capabilities (2026-11-04): pad holes 0.45 mm apart, via holes 0.2 mm"),
        copper_to_edge: fab(0.2, "jlcpcb.com capabilities (2026-11-04): copper 0.2 mm from routed edges and slots"),
        drill_to_copper: summary(0.2, "only for inner layers, which a 2-layer board hasn't"),
        drill_to_copper_pth: summary(0.3, "only for inner layers, which a 2-layer board hasn't"),
        silk_to_pad: fab(0.15, "jlcpcb.com capabilities (2026-11-04): legend 0.15 mm from pads"),
        via_to_pad: dfm(0.25, "public DFM summaries: about 0.25 mm of solder mask web between an open via and an SMD pad"),
        pad_entry_angle: dfm(30.0, ENTRY),
        acute_angle: dfm(70.0, ACID_TRAP),
        micro: None,
    },
    FabProfile {
        id: "fine",
        name: "Fine pitch",
        version: 1,
        read: "2026-10",
        about: "0.10/0.10 mm track and gap and 0.2 mm holes for 0.5 mm pitch parts, rings on the drill tool",
        track: fab(0.10, "Eurocircuits pattern class 8 (0.10/0.10 mm); JLCPCB 1 oz 0.10 mm on 2 layers"),
        gap: fab(0.10, "Eurocircuits pattern class 8 (0.10/0.10 mm); JLCPCB 1 oz 0.10 mm on 2 layers"),
        drill: fab(0.2, "JLCPCB via hole 0.20 mm preferred; Eurocircuits drill class D 0.15 mm finished"),
        ring: fab(0.10, "Eurocircuits pattern class 8 outer ring 0.10 mm, on the drill tool"),
        ring_convention: RingConvention::Tool,
        plating_allowance: fab(0.10, EUROCIRCUITS_TOOL),
        aspect_ratio: fab(8.0, "Eurocircuits 1:8 on the drill tool"),
        hole_to_hole: fab(0.25, NO_SOURCE),
        copper_to_edge: fab(0.25, "Eurocircuits 0.25 mm to a routed edge; JLCPCB 0.2 mm"),
        drill_to_copper: fab(0.2, "Eurocircuits inner isolation max(inner ring + 0.075, 0.20) mm from the tool"),
        drill_to_copper_pth: fab(0.2, "Eurocircuits inner isolation max(inner ring + 0.075, 0.20) mm from the tool"),
        silk_to_pad: fab(0.10, "Eurocircuits legend 0.10 mm from exposed copper"),
        via_to_pad: dfm(0.2, "public DFM summaries: a mask web of 0.2 mm or more between an open via and an SMD pad at finer registration"),
        pad_entry_angle: dfm(30.0, ENTRY),
        acute_angle: dfm(70.0, ACID_TRAP),
        micro: None,
    },
    FabProfile {
        id: "hdi",
        name: "HDI (laser micro vias)",
        version: 1,
        read: "2026-10",
        about: "0.09/0.09 mm track and gap, 0.15 mm mechanical holes and laser micro vias between neighbouring layers, rings on the drill tool",
        track: fab(0.09, "Eurocircuits pattern class 9 (0.09/0.09 mm); JLCPCB multilayer 0.09/0.09 mm"),
        gap: fab(0.09, "Eurocircuits pattern class 9 (0.09/0.09 mm); JLCPCB multilayer 0.09/0.09 mm"),
        drill: fab(0.15, "JLCPCB via holes from 0.15 mm; Eurocircuits drill class D 0.15 mm finished, 0.25 mm tool"),
        ring: fab(0.10, "Eurocircuits pattern class 9 outer ring 0.10 mm, on the drill tool"),
        ring_convention: RingConvention::Tool,
        plating_allowance: fab(0.10, EUROCIRCUITS_TOOL),
        aspect_ratio: fab(8.0, "Eurocircuits 1:8 on the drill tool (drill class D up to 2.0 mm thick)"),
        hole_to_hole: fab(0.25, NO_SOURCE),
        copper_to_edge: fab(0.25, "Eurocircuits 0.25 mm to a routed edge; JLCPCB 0.2 mm"),
        drill_to_copper: fab(0.2, "Eurocircuits inner isolation max(inner ring + 0.075, 0.20) mm from the tool"),
        drill_to_copper_pth: fab(0.2, "Eurocircuits inner isolation max(inner ring + 0.075, 0.20) mm from the tool"),
        silk_to_pad: fab(0.10, "Eurocircuits legend 0.10 mm from exposed copper"),
        via_to_pad: dfm(0.15, "public DFM summaries: HDI builds hold mask registration for about 0.15 mm between an open via and an SMD pad"),
        pad_entry_angle: dfm(30.0, ENTRY),
        acute_angle: dfm(70.0, ACID_TRAP),
        micro: Some(Micro {
            drill: fab(0.10, NO_SOURCE),
            aspect: ipc(1.0, "IPC-6012 / IPC-T-50 micro via at most 1:1, from NCAB's public summary"),
            depth: ipc(0.25, "IPC-T-50 micro via at most 0.25 mm deep, from NCAB's public summary"),
        }),
    },
];

/// The profile with the id `id`.
pub fn find(id: &str) -> Option<&'static FabProfile> {
    PROFILES.iter().find(|p| p.id == id)
}

/// The profile a board picked: its id and the version of its figures then.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FabChoice {
    pub id: String,
    pub version: u32,
}

impl FabChoice {
    /// The choice of `p` as it is now.
    pub fn of(p: &FabProfile) -> Self {
        FabChoice { id: p.id.to_string(), version: p.version }
    }

    /// The id names a profile this build has.
    pub fn validate(&self) -> Result<(), String> {
        if find(&self.id).is_none() {
            let ids: Vec<&str> = PROFILES.iter().map(|p| p.id).collect();
            return Err(format!("the board's fab profile `{}` is not one of {}", self.id, ids.join(", ")));
        }
        if self.version == 0 {
            return Err("a fab profile's version starts at 1".into());
        }
        Ok(())
    }

    pub fn profile(&self) -> Option<&'static FabProfile> {
        find(&self.id)
    }
}

/// A rule [`FabProfile::apply`] set, with its value before and after.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleChange {
    pub field: &'static str,
    pub from: f64,
    pub to: f64,
    pub source: &'static str,
}

/// What picking a profile in place of another did to the rules (see [`FabProfile::switch`]):
/// the rules it set, and those it kept because the user had set them to something else.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Switch {
    pub set: Vec<RuleChange>,
    pub kept: Vec<(&'static str, f64)>,
}

/// The rules a profile sets to its figures (its minimums, and its micro via's drill and
/// diameter), each as the default rules get it from `p`, or the default itself for none.
fn profile_rules(p: Option<&FabProfile>) -> Vec<(&'static str, f64)> {
    let mut r = Rules::default();
    if let Some(p) = p {
        p.apply(&mut r);
    }
    vec![
        ("min_track_width", r.min_track_width),
        ("min_annular_ring", r.min_annular_ring),
        ("min_drill", r.min_drill),
        ("hole_to_hole", r.hole_to_hole),
        ("copper_to_edge", r.copper_to_edge),
        ("silk_to_pad", r.silk_to_pad),
        ("microvia_drill", r.microvia_drill),
        ("microvia_diameter", r.microvia_diameter),
    ]
}

/// The rule `field` of [`profile_rules`] in `rules`.
fn rule_slot<'a>(rules: &'a mut Rules, field: &str) -> Option<&'a mut f64> {
    Some(match field {
        "min_track_width" => &mut rules.min_track_width,
        "min_annular_ring" => &mut rules.min_annular_ring,
        "min_drill" => &mut rules.min_drill,
        "hole_to_hole" => &mut rules.hole_to_hole,
        "copper_to_edge" => &mut rules.copper_to_edge,
        "silk_to_pad" => &mut rules.silk_to_pad,
        "microvia_drill" => &mut rules.microvia_drill,
        "microvia_diameter" => &mut rules.microvia_diameter,
        _ => return None,
    })
}

/// Rounded up to 0.001 mm, so a size made from a sum of figures doesn't fall short by a hair.
fn up(v: f64) -> f64 {
    (v * 1000.0 - 1e-6).ceil() / 1000.0
}

impl FabProfile {
    /// The hole a ring, an aspect ratio or a hole-to-copper gap is measured from, for a
    /// mechanical drill of finished size `drill`.
    pub fn hole(&self, drill: f64) -> f64 {
        match self.ring_convention {
            RingConvention::Hole => drill,
            RingConvention::Tool => drill + self.plating_allowance.value,
        }
    }

    /// Every figure, with its rules' or the check's name and units, for replies.
    pub fn figures(&self) -> Vec<(&'static str, &'static str, Figure)> {
        let mut out = vec![
            ("track", "mm", self.track),
            ("gap", "mm", self.gap),
            ("drill", "mm", self.drill),
            ("ring", "mm", self.ring),
            ("plating_allowance", "mm", self.plating_allowance),
            ("aspect_ratio", "ratio", self.aspect_ratio),
            ("hole_to_hole", "mm", self.hole_to_hole),
            ("copper_to_edge", "mm", self.copper_to_edge),
            ("drill_to_copper", "mm", self.drill_to_copper),
            ("drill_to_copper_pth", "mm", self.drill_to_copper_pth),
            ("silk_to_pad", "mm", self.silk_to_pad),
            ("via_to_pad", "mm", self.via_to_pad),
            ("pad_entry_angle", "degrees", self.pad_entry_angle),
            ("acute_angle", "degrees", self.acute_angle),
        ];
        if let Some(m) = self.micro {
            out.extend([("microvia_drill", "mm", m.drill), ("microvia_aspect", "ratio", m.aspect), ("microvia_depth", "mm", m.depth)]);
        }
        out
    }

    /// A short hash of the figures and the convention (FNV-1a), so a report says which numbers it
    /// was checked against.
    pub fn hash(&self) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for b in bytes {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        eat(self.id.as_bytes());
        eat(&self.version.to_le_bytes());
        eat(self.ring_convention.name().as_bytes());
        for (name, _, f) in self.figures() {
            eat(name.as_bytes());
            eat(&f.value.to_le_bytes());
        }
        format!("{:08x}", h >> 32)
    }

    /// Pick this profile in place of `old` (none: the defaults) without loosening what the user
    /// set: each rule the profile sets that still has `old`'s figure (or the default) takes this
    /// one's; one set to anything else is kept, and listed when it differs from this profile's.
    /// A profile with no micro vias gives the micro via rules back their defaults where they
    /// came from `old`. What new copper gets (the default class's gap, track and via) is only
    /// ever raised, to the profile's least, as [`FabProfile::apply`] does.
    pub fn switch(&self, old: Option<&FabProfile>, rules: &mut Rules) -> Switch {
        let mut out = Switch::default();
        let was = profile_rules(old);
        let now = profile_rules(Some(self));
        let source = |field: &str| match field {
            "min_track_width" => self.track.source,
            "min_annular_ring" | "microvia_diameter" => self.ring.source,
            "min_drill" => self.drill.source,
            "hole_to_hole" => self.hole_to_hole.source,
            "copper_to_edge" => self.copper_to_edge.source,
            "silk_to_pad" => self.silk_to_pad.source,
            _ => self.micro.map_or("the defaults: the profile makes no micro vias", |m| m.drill.source),
        };
        for ((field, from), (_, to)) in was.iter().zip(&now) {
            let Some(slot) = rule_slot(rules, field) else { continue };
            if (*slot - from).abs() <= 1e-9 {
                if (*slot - to).abs() > 1e-9 {
                    out.set.push(RuleChange { field, from: *slot, to: *to, source: source(field) });
                    *slot = *to;
                }
            } else if (*slot - to).abs() > 1e-9 {
                out.kept.push((field, *slot));
            }
        }
        // What new copper gets: raised where it falls below the profile, as `apply` raises it.
        let mut raised = rules.clone();
        for c in self.apply(&mut raised) {
            if matches!(c.field, "clearance" | "track_width" | "via_drill" | "via_diameter") && c.to > c.from {
                if let Some(slot) = match c.field {
                    "clearance" => Some(&mut rules.clearance),
                    "track_width" => Some(&mut rules.track_width),
                    "via_drill" => Some(&mut rules.via_drill),
                    "via_diameter" => Some(&mut rules.via_diameter),
                    _ => None,
                } {
                    *slot = c.to;
                }
                out.set.push(c);
            }
        }
        out
    }

    /// Whether resetting `rules` to the profile ([`FabProfile::reset`]) would change any.
    pub fn differs(&self, rules: &Rules) -> bool {
        let mut r = rules.clone();
        !self.reset(&mut r).is_empty()
    }

    /// Every rule the profile sets back to its figure (the micro via's to the defaults when it
    /// makes none), and what new copper gets raised to its least: Reset Rules to Profile.
    pub fn reset(&self, rules: &mut Rules) -> Vec<RuleChange> {
        let mut out = self.apply(rules);
        if self.micro.is_none() {
            let d = Rules::default();
            for (field, slot, to) in [
                ("microvia_drill", &mut rules.microvia_drill, d.microvia_drill),
                ("microvia_diameter", &mut rules.microvia_diameter, d.microvia_diameter),
            ] {
                if (*slot - to).abs() > 1e-9 {
                    out.push(RuleChange { field, from: *slot, to, source: "the defaults: the profile makes no micro vias" });
                    *slot = to;
                }
            }
        }
        out
    }

    /// Set `rules`' minimums to the profile's, and raise what new copper gets where it would
    /// fall below them (the default class's gap, track and via, the micro via); returns what
    /// changed. Net classes keep their own sizes: the check reports those the profile can't
    /// build.
    pub fn apply(&self, rules: &mut Rules) -> Vec<RuleChange> {
        let mut out = Vec::new();
        let mut set = |field: &'static str, slot: &mut f64, to: f64, source: &'static str| {
            if (*slot - to).abs() > 1e-9 {
                out.push(RuleChange { field, from: *slot, to, source });
                *slot = to;
            }
        };
        set("min_track_width", &mut rules.min_track_width, self.track.value, self.track.source);
        set("min_annular_ring", &mut rules.min_annular_ring, self.ring.value, self.ring.source);
        set("min_drill", &mut rules.min_drill, self.drill.value, self.drill.source);
        set("hole_to_hole", &mut rules.hole_to_hole, self.hole_to_hole.value, self.hole_to_hole.source);
        set("copper_to_edge", &mut rules.copper_to_edge, self.copper_to_edge.value, self.copper_to_edge.source);
        set("silk_to_pad", &mut rules.silk_to_pad, self.silk_to_pad.value, self.silk_to_pad.source);
        let raise = |v: f64, least: f64| if v < least - 1e-9 { least } else { v };
        let to = raise(rules.clearance, self.gap.value);
        set("clearance", &mut rules.clearance, to, self.gap.source);
        let to = raise(rules.track_width, self.track.value);
        set("track_width", &mut rules.track_width, to, self.track.source);
        let to = raise(rules.via_drill, self.drill.value);
        set("via_drill", &mut rules.via_drill, to, self.drill.source);
        let to = raise(rules.via_diameter, up(self.hole(rules.via_drill) + 2.0 * self.ring.value));
        set("via_diameter", &mut rules.via_diameter, to, self.ring.source);
        if let Some(m) = self.micro {
            set("microvia_drill", &mut rules.microvia_drill, m.drill.value, m.drill.source);
            let to = raise(rules.microvia_diameter, up(rules.microvia_drill + 2.0 * self.ring.value));
            set("microvia_diameter", &mut rules.microvia_diameter, to, self.ring.source);
        }
        out
    }
}
