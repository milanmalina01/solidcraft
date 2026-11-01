//! The design rule check of the board: what breaks the rules (see [`super::rules`]) or the fab
//! profile's figures (see [`super::profiles`]), or looks unfinished, worked out from the board
//! and its connections (see [`super::airwires`], pours filled) each time and never saved.
//!
//! Each finding has a stable code (see [`RULES`]), a class (critical, practical or cosmetic: what
//! it costs to leave it), a severity, where it is, the board objects it is about, what was
//! measured against which limit (with units, and the margin: negative when the limit is broken),
//! the commands that would deal with it, and the guide section that explains it. Its key is made
//! of the code and those objects' ids only (`t5` track 5, `v7` via 7, `p12.3` pad 3 of footprint
//! 12, `f12` footprint 12, `h4` hole 4, `z9` pour 9, `k3` keepout 3, `g2` drawing 2; a track's
//! ends are `t5.0` and `t5.1`), sorted, so a key outlives other edits and an approval of it
//! (saved on the board) stays put. An agent's approval of a critical finding (or one made before
//! who approved was kept) waits for the user's: until then the finding still counts, marked
//! `pending_user`. Messages come in English
//! and as a message key with named arguments for the UI to word. The board's levels
//! (`drc_levels`) give codes another severity, or leave them out.
//!
//! Rings, aspect ratios and hole-to-copper gaps are measured from the hole the profile says:
//! the finished hole, or the drill tool (the finished hole plus the plating allowance); a micro
//! via's laser hole is its drill. Without a profile the finished hole is used, and the checks
//! that need a profile's own figures (aspect ratio, drill to copper) are left out and say so
//! (`profile_missing`). A check that rests only on figures from public summaries of an IPC
//! standard reports a warning, never an error, with a note that says so.
//!
//! Copper of two nets is told apart by island (copper joined into one is one net, even if it
//! wrongly joins two: that island is one `short` finding, and its copper is not checked against
//! itself). Clearance between two pieces is the larger of their net classes'; a pour's fill
//! counts with its pour's clearance in place of its net class's (as the fill keeps it), and
//! between two fills the larger of the two pours' clearances applies. Copper drawings are copper
//! on no net: what one touches it joins (touching two nets is a `short`), and it keeps its
//! clearance to the rest. Arcs are checked as straight pieces within 0.002 mm of them.
//!
//! The pair tests (clearance, holes, edges, courtyards, keepouts, silkscreen, drill to copper)
//! find their neighbours through a grid of cells, so the work grows with the board, not its
//! square; past a budget of tests (and of neighbours looked at), or [`MAX_FINDINGS`] findings,
//! the rest is left out and `truncated` is set. A truncated check names no approval stale: the
//! findings it did not reach may still hold them.

mod dfm;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Electronics;
use super::airwires::{BoardNets, BoardPad};
use super::board::{Board, THICKNESS, Via, ViaKind, ViaProtection, footprint_box};
use super::checks::{Severity, gap};
use super::copper::{TOLERANCE, core_bbox, disc, drill_core, graphic_cores, inside, pad_core, pieces, track_cores};
use super::footprint::{Core, PadKind, Side};
use super::graphics::{BoardLayer, GraphicType};
use super::grid::{Grid, boxes_meet, grow};
use super::layers::{CopperLayer, DrawLayer};
use super::path;
use super::place::lattice;
use super::profiles::{FabProfile, RingConvention};
use super::rules::{Sizes, clearance_between};
use super::schematic::{Actor, Approval};
use super::stackup::{StackKind, StackLayer};

/// The most findings a check returns; past it, `truncated` is set.
pub const MAX_FINDINGS: usize = 10_000;
/// The most items one finding lists.
pub const MAX_ITEMS: usize = 16;
/// The most pair tests one check makes.
pub const MAX_TESTS: usize = 1 << 23;
/// The most neighbours the pair tests look at (a look is cheaper than a test: most are of the
/// same copper, and skipped).
pub const MAX_VISITS: usize = 1 << 26;
/// How much closer than its clearance a pour's fill may come before it is a finding, in
/// millimetres: the fill's lattice and arcs.
const FILL_SLACK: f64 = 0.01;
/// The grid's cell, in millimetres.
const CELL: f64 = 2.0;
/// The most edges of pour fills the clearance check looks at.
const MAX_FILL_EDGES: usize = 1 << 19;
/// A gap or size this close above a limit is "at the limit", as a share of it.
pub const AT_LIMIT: f64 = 0.1;
/// A feature this close above the profile's least is at the profile's minimum, in millimetres.
const AT_MINIMUM: f64 = 0.005;
/// More features than this at the profile's minimum make `fab_class` a warning.
pub const MAX_AT_MINIMUM: usize = 5;
/// The largest drill of a plugged or tented via that may sit in a thermal pad with a warning only
/// (TI SLUA271), in millimetres.
const THERMAL_VIA_DRILL: f64 = 0.3;

/// What leaving a finding costs: a critical one breaks the board or its build, a practical one
/// makes it worse or harder to build, a cosmetic one only looks untidy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Critical,
    Practical,
    Cosmetic,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Critical => "critical",
            Class::Practical => "practical",
            Class::Cosmetic => "cosmetic",
        }
    }
}

/// A level the board gives a code in place of its own: `ignore` leaves its findings out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckLevel {
    Error,
    Warning,
    Info,
    Ignore,
}

/// A finding's code, its class and severity, the guide section that explains it, the limit it
/// measures against, and what it means.
#[derive(Clone, Copy, Debug)]
pub struct DrcRule {
    pub code: &'static str,
    pub severity: Severity,
    pub class: Class,
    /// `routing-dfm#vias`: the guide file and section.
    pub section: &'static str,
    /// The rule or profile figure the limit comes from, such as `rules.clearance`.
    pub param: &'static str,
    pub about: &'static str,
}

const fn rule(code: &'static str, severity: Severity, class: Class, section: &'static str, param: &'static str, about: &'static str) -> DrcRule {
    DrcRule { code, severity, class, section, param, about }
}

const ERROR: Severity = Severity::Error;
const WARNING: Severity = Severity::Warning;
const INFO: Severity = Severity::Info;
const C: Class = Class::Critical;
const P: Class = Class::Practical;

/// Every code the check gives, in the order findings are listed (errors before warnings, then
/// infos).
pub const RULES: &[DrcRule] = &[
    rule("short", ERROR, C, "routing-dfm#connectivity", "nets", "copper of two nets touches, or is joined through other copper"),
    rule(
        "clearance",
        ERROR,
        C,
        "routing-dfm#widths-and-clearances",
        "rules.clearance",
        "copper of two nets is closer than their clearance on a layer",
    ),
    rule(
        "track_width",
        ERROR,
        C,
        "routing-dfm#widths-and-clearances",
        "rules.min_track_width",
        "a track is narrower than the rules' least track width",
    ),
    rule(
        "annular_ring",
        ERROR,
        C,
        "routing-dfm#vias",
        "rules.min_annular_ring",
        "the copper ring round a via's or a through-hole pad's hole (as the fab profile measures it) is narrower than the least annular ring",
    ),
    rule(
        "drill",
        ERROR,
        C,
        "manufacturing#profiles",
        "rules.min_drill",
        "a drill is smaller than the least drill (a micro via's than the micro via drill)",
    ),
    rule(
        "via_aspect_ratio",
        ERROR,
        C,
        "routing-dfm#vias",
        "profile.aspect_ratio",
        "a hole is deeper for its size than the fab profile drills and plates (micro vias: deeper than wide, or deeper than the profile's micro via)",
    ),
    rule(
        "drill_to_copper",
        ERROR,
        C,
        "routing-dfm#vias",
        "profile.drill_to_copper",
        "on an inner layer, a hole is closer to copper it doesn't join than the fab profile allows",
    ),
    rule(
        "via_in_pad",
        ERROR,
        C,
        "routing-dfm#vias",
        "via.protection",
        "a via's land or hole is in a surface-mount pad and it isn't filled and capped (on a thermal pad, a small plugged or tented via is a warning; a laser micro via in a pad is fine, and one stacked on another and not copper-filled is an info)",
    ),
    rule("hole_to_hole", ERROR, C, "manufacturing#profiles", "rules.hole_to_hole", "two drilled holes are closer than the least hole-to-hole gap"),
    rule(
        "copper_to_edge",
        ERROR,
        C,
        "routing-dfm#copper",
        "rules.copper_to_edge",
        "copper is closer to the board's edge or an unplated hole than the rules allow, or outside the board",
    ),
    rule("hole_outside", ERROR, C, "placement#edges", "board.outline", "a drill or a hole is outside the board or crosses its edge"),
    rule("footprint_outside", ERROR, C, "placement#edges", "board.outline", "a footprint's courtyard reaches outside the board"),
    rule("courtyard_overlap", ERROR, C, "placement#spacing", "courtyard.gap", "two footprints' courtyards overlap on one side"),
    rule("keepout", ERROR, C, "rf-emc#antennas", "keepout", "a track, a via or a footprint is in a keepout that forbids it"),
    rule(
        "relief_starved",
        ERROR,
        P,
        "routing-dfm#pad-to-pour",
        "pour.spokes",
        "a pad a pour relieves joins it through no spoke, or spokes narrower than the least track (fewer than 2, or half those asked for: a warning)",
    ),
    rule(
        "track_width_class",
        WARNING,
        P,
        "routing-dfm#widths-and-clearances",
        "class.track_width",
        "a track is narrower than its net class's track width",
    ),
    rule(
        "silk_over_pad",
        WARNING,
        P,
        "silkscreen-assembly#clipping",
        "rules.silk_to_pad",
        "silkscreen is closer to a pad than the rules' silk-to-pad gap",
    ),
    rule(
        "via_near_pad",
        WARNING,
        P,
        "routing-dfm#vias",
        "profile.via_to_pad",
        "an open via's ring is closer to a surface-mount pad than the fab profile's via-to-pad gap, so no solder mask web stays between them and the solder can wick into the via (a tented, plugged or filled via: an info)",
    ),
    rule(
        "pad_entry",
        WARNING,
        P,
        "routing-dfm#pad-entry",
        "profile.pad_entry_angle",
        "a track leaves a surface-mount pad near its corner, or further off square to the pad's edge than the fab profile's entry angle",
    ),
    rule(
        "acute_angle",
        WARNING,
        P,
        "routing-dfm#corners",
        "profile.acute_angle",
        "a track bends, or meets a pad or another track of its net, at a pocket under the fab profile's least angle (an acid trap)",
    ),
    rule(
        "sliver",
        WARNING,
        P,
        "routing-dfm#copper",
        "profile.track",
        "a pour's fill has copper narrower than the least track, or a gap narrower than the least gap",
    ),
    rule("unrouted", WARNING, C, "workflow#done", "connections.open", "a connection still to route (one per airwire)"),
    rule("dangling_track", WARNING, P, "routing-dfm#connectivity", "track.end_joins", "a track end touches nothing"),
    rule("via_stub", WARNING, P, "power-ground#stitching", "via.layers_joined", "a via joins copper on one layer only, or none"),
    rule("pour_unknown_net", WARNING, P, "power-ground#pours", "pour.net", "a pour's net is not one of the schematic's"),
    rule("pour_empty", WARNING, P, "power-ground#pours", "pour.area", "a pour fills nothing"),
    rule(
        "profile_missing",
        WARNING,
        C,
        "manufacturing#profiles",
        "board.fab_profile",
        "no fab profile is chosen: rings are measured on the finished hole and the profile's own checks are left out",
    ),
    // Critical for what the fab can't build (rules finer than the profile, micro vias it makes
    // none of); the features at its minimums and a profile's newer version are practical.
    rule(
        "fab_class",
        INFO,
        C,
        "manufacturing#profiles",
        "profile",
        "what the board needs of the fab: rules finer than the profile builds (a warning), and the features right at its minimums (more than 5: a warning)",
    ),
];

/// The rule for `code`.
pub fn find_rule(code: &str) -> Option<&'static DrcRule> {
    RULES.iter().find(|r| r.code == code)
}

/// What a finding is about.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DrcItem {
    /// A footprint's pad: the footprint's id, the pad's number, and `R1.2`.
    Pad {
        footprint: u64,
        number: String,
        reference: String,
    },
    /// A footprint: its id and its part's designator.
    Footprint {
        id: u64,
        designator: String,
    },
    Track(u64),
    /// One end of a track: 0 its first point, 1 its last.
    TrackEnd(u64, u8),
    Via(u64),
    Hole(u64),
    Pour(u64),
    Keepout(u64),
    Graphic(u64),
}

impl DrcItem {
    /// How keys name it (see the module notes).
    pub fn key(&self) -> String {
        match self {
            DrcItem::Pad { footprint, number, .. } => format!("p{footprint}.{number}"),
            DrcItem::Footprint { id, .. } => format!("f{id}"),
            DrcItem::Track(id) => format!("t{id}"),
            DrcItem::TrackEnd(id, end) => format!("t{id}.{end}"),
            DrcItem::Via(id) => format!("v{id}"),
            DrcItem::Hole(id) => format!("h{id}"),
            DrcItem::Pour(id) => format!("z{id}"),
            DrcItem::Keepout(id) => format!("k{id}"),
            DrcItem::Graphic(id) => format!("g{id}"),
        }
    }

    /// The board object to pick for it.
    pub fn id(&self) -> u64 {
        match self {
            DrcItem::Pad { footprint, .. } => *footprint,
            DrcItem::Footprint { id, .. } => *id,
            DrcItem::Track(id) | DrcItem::TrackEnd(id, _) | DrcItem::Via(id) | DrcItem::Hole(id) => *id,
            DrcItem::Pour(id) | DrcItem::Keepout(id) | DrcItem::Graphic(id) => *id,
        }
    }

    /// How messages name it: `R1.2`, `R1`, `track 5`, `via 7`, …
    pub fn reference(&self) -> String {
        match self {
            DrcItem::Pad { reference, .. } => reference.clone(),
            DrcItem::Footprint { designator, .. } => designator.clone(),
            DrcItem::Track(id) => format!("track {id}"),
            DrcItem::TrackEnd(id, 0) => format!("the start of track {id}"),
            DrcItem::TrackEnd(id, _) => format!("the end of track {id}"),
            DrcItem::Via(id) => format!("via {id}"),
            DrcItem::Hole(id) => format!("hole {id}"),
            DrcItem::Pour(id) => format!("pour {id}"),
            DrcItem::Keepout(id) => format!("keepout {id}"),
            DrcItem::Graphic(id) => format!("drawing {id}"),
        }
    }
}

/// Which way a limit goes: the measure must be at least it, or at most it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    Min,
    Max,
}

/// What a finding measured against.
#[derive(Clone, Debug, PartialEq)]
pub struct Limit {
    pub value: f64,
    /// `mm`, `ratio`, `degrees`, `nets`, `connections`, `layers`, `spokes`, `profiles`, `mm²`, or
    /// `protection` for a categorical measure (see [`DrcFinding::measured_text`]).
    pub units: &'static str,
    /// A categorical limit's name (`filled_capped`); its rank is `value`.
    pub text: Option<&'static str>,
    /// The rule or profile figure it comes from (see [`DrcRule::param`]).
    pub param: String,
    pub bound: Bound,
}

/// A command that would deal with a finding: run `command` with `params` (`Null` when the call
/// needs choices the check can't make: `says` tells what to do).
#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    pub command: &'static str,
    pub params: Value,
    pub says: String,
}

/// One finding of the check.
#[derive(Clone, Debug, PartialEq)]
pub struct DrcFinding {
    pub code: &'static str,
    pub class: Class,
    /// After the board's levels.
    pub severity: Severity,
    /// The code and the sorted keys of its items (see the module notes).
    pub key: String,
    /// The board's approvals hold the key (and, for a critical finding, the user's).
    pub approved: bool,
    /// An agent approved it (or someone before who approved was kept), and it is critical: it
    /// still counts until the user confirms it.
    pub pending_user: bool,
    /// Who approved it; `None` while it isn't approved, or for an approval made before who made
    /// it was kept.
    pub by: Option<Actor>,
    /// In English.
    pub message: String,
    /// The UI's message: `electronics.drc_msg_<code>`, with `args` (`a`, `b`, `nets`, `layer`,
    /// `value`, `required`, as the message has them).
    pub message_key: String,
    pub args: Vec<(&'static str, String)>,
    /// Where on the board, in millimetres.
    pub at: [f64; 2],
    pub layer: Option<CopperLayer>,
    /// At most [`MAX_ITEMS`].
    pub items: Vec<DrcItem>,
    /// The nets it is about, by name.
    pub nets: Vec<String>,
    /// What was measured, in the limit's units, and the margin: how far inside the limit it is
    /// (negative: past it).
    pub measured: f64,
    /// A categorical measure's name (a via's protection: `open`), whose rank is `measured`
    /// (open 0, tented 1, plugged 2, filled 3, filled and capped 4).
    pub measured_text: Option<&'static str>,
    pub limit: Limit,
    pub margin: f64,
    /// The same in millimetres for the findings measured so, as the UI words them.
    pub value: Option<f64>,
    pub required: Option<f64>,
    pub fix: Vec<Fix>,
    pub section: &'static str,
    /// Why it is only a warning, when its figures come from public summaries of a standard.
    pub note: Option<String>,
}

/// What the check found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrcReport {
    /// Errors, then warnings, then infos, each in the order of [`RULES`]; approved ones included
    /// (marked).
    pub findings: Vec<DrcFinding>,
    /// Errors, warnings and infos not approved (an agent's approval of a critical finding
    /// waiting for the user's counts as not approved), the approved findings, and those waiting.
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub approved: usize,
    pub pending_user: usize,
    /// Approval keys no finding has now; empty when `truncated` (findings not reached may have
    /// them).
    pub stale: Vec<String>,
    /// Gaps between copper of two nets that pass, but by less than [`AT_LIMIT`] of their
    /// clearance (pairs of objects).
    pub at_limit: usize,
    /// The fab profile it checked against; `None` without one.
    pub profile: Option<&'static FabProfile>,
    /// The codes that didn't run, each with what it needs (`board.fab_profile`) or what left it
    /// out (`board.drc_levels`): a code missing here and from the findings passed; one listed
    /// here was not checked.
    pub not_applicable: Vec<(&'static str, &'static str)>,
    /// Some of the board was past a budget, or there were more than [`MAX_FINDINGS`] findings.
    pub truncated: bool,
}

impl Electronics {
    /// The design rule check of the board (see [`check`]).
    pub fn drc(&self) -> DrcReport {
        check(self, &self.board_nets())
    }
}

/// Millimetres as messages give them.
fn mm(v: f64) -> String {
    let s = format!("{:.4}", (v * 1e4).round() / 1e4 + 0.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Rounded up to 0.01 mm: a size a fix proposes.
fn up(v: f64) -> f64 {
    (v * 100.0 - 1e-6).ceil() / 100.0
}

/// How far a fix moves something: rounded up to 0.01 mm and at least that, so no fix asks for a
/// move finer than can be built or placed.
fn step(v: f64) -> f64 {
    up(v.max(0.0)).max(0.01)
}

/// What one finding says, made only when it is kept.
struct Draft {
    at: [f64; 2],
    layer: Option<CopperLayer>,
    items: Vec<DrcItem>,
    nets: Vec<String>,
    /// What was measured, its limit, units and bound, and the limit's parameter when it isn't
    /// the rule's own.
    measured: Option<(f64, f64, &'static str, Bound)>,
    /// A categorical measure's and limit's names.
    texts: Option<(&'static str, &'static str)>,
    param: Option<String>,
    /// Millimetres, for the UI's messages.
    value: Option<f64>,
    required: Option<f64>,
    severity: Option<Severity>,
    /// In place of the rule's, for a finding that costs more than its code mostly does.
    class: Option<Class>,
    fix: Vec<Fix>,
    note: Option<String>,
    message: String,
    args: Vec<(&'static str, String)>,
}

impl Draft {
    fn new(at: [f64; 2], items: Vec<DrcItem>, message: String) -> Self {
        Draft {
            at,
            layer: None,
            items,
            nets: Vec::new(),
            measured: None,
            texts: None,
            param: None,
            value: None,
            required: None,
            severity: None,
            class: None,
            fix: Vec::new(),
            note: None,
            message,
            args: Vec::new(),
        }
    }

    fn layer(mut self, layer: Option<CopperLayer>) -> Self {
        self.layer = layer;
        if let Some(l) = layer {
            self.args.push(("layer", l.name()));
        }
        self
    }

    /// A gap or size in millimetres that must be at least `required`.
    fn measure(mut self, value: f64, required: f64) -> Self {
        self.value = Some(value);
        self.required = Some(required);
        self.measured = Some((value, required, "mm", Bound::Min));
        self.args.push(("value", mm(value)));
        self.args.push(("required", mm(required)));
        self
    }

    /// A gap in millimetres that must be at least `required`, measured signed (minus how far
    /// two things overlap, or how far copper is past the edge), so the margin says how far past
    /// the limit it is; the UI's words give 0 for an overlap.
    fn gap(mut self, g: f64, required: f64) -> Self {
        self = self.measure(g.max(0.0), required);
        self.measured = Some((g, required, "mm", Bound::Min));
        self
    }

    /// A measure in other units, or one that must stay at most its limit.
    fn count(mut self, value: f64, limit: f64, units: &'static str, bound: Bound) -> Self {
        self.measured = Some((value, limit, units, bound));
        self
    }

    /// A categorical measure (a via's protection) that must rank at least as `limit`.
    fn protection(mut self, measured: ViaProtection, limit: ViaProtection) -> Self {
        self.measured = Some((measured.rank(), limit.rank(), "protection", Bound::Min));
        self.texts = Some((measured.name(), limit.name()));
        self
    }

    fn param(mut self, param: impl Into<String>) -> Self {
        self.param = Some(param.into());
        self
    }

    fn severity(mut self, s: Severity) -> Self {
        self.severity = Some(s);
        self
    }

    fn class(mut self, c: Class) -> Self {
        self.class = Some(c);
        self
    }

    fn fix(mut self, command: &'static str, params: Value, says: impl Into<String>) -> Self {
        self.fix.push(Fix { command, params, says: says.into() });
        self
    }

    fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    fn arg(mut self, name: &'static str, v: String) -> Self {
        self.args.push((name, v));
        self
    }

    fn nets(mut self, nets: Vec<String>) -> Self {
        self.args.push(("nets", nets.join(", ")));
        self.nets = nets;
        self
    }
}

/// The findings being collected.
struct Found<'a> {
    approved: BTreeMap<&'a str, &'a Approval>,
    levels: &'a BTreeMap<String, CheckLevel>,
    seen: BTreeSet<String>,
    list: Vec<DrcFinding>,
    truncated: bool,
}

impl Found<'_> {
    /// A finding `code` about the items whose keys are `keys`; `make` says the rest when kept.
    fn add(&mut self, code: &'static str, keys: &[String], make: impl FnOnce() -> Draft) {
        let mut keys = keys.to_vec();
        keys.sort();
        keys.dedup();
        let mut key = code.to_string();
        // Keys stay short: the first ids name the finding well enough.
        for k in keys.iter().take(MAX_ITEMS) {
            key.push('|');
            key.push_str(k);
        }
        if !self.approved.is_empty() {
            self.seen.insert(key.clone());
        }
        let level = self.levels.get(code).copied();
        if level == Some(CheckLevel::Ignore) {
            return;
        }
        if self.list.len() >= MAX_FINDINGS {
            self.truncated = true;
            return;
        }
        let d = make();
        let rule = find_rule(code);
        let class = d.class.unwrap_or_else(|| rule.map_or(Class::Critical, |r| r.class));
        let severity = match level {
            Some(CheckLevel::Error) => Severity::Error,
            Some(CheckLevel::Warning) => Severity::Warning,
            Some(CheckLevel::Info) => Severity::Info,
            _ => d.severity.unwrap_or_else(|| rule.map_or(Severity::Error, |r| r.severity)),
        };
        let mut items = d.items;
        items.truncate(MAX_ITEMS);
        let approval = self.approved.get(key.as_str()).copied();
        // An approval made before who made it was kept may be an agent's too.
        let pending_user = approval.is_some_and(|a| !a.by_user() && class == Class::Critical);
        let approved = approval.is_some() && !pending_user;
        let mut args = d.args;
        for (name, item) in ["a", "b"].into_iter().zip(&items) {
            args.push((name, item.reference()));
        }
        // Every finding measures something: one with no number of its own counts itself.
        let (measured, limit, units, bound) = d.measured.unwrap_or((1.0, 0.0, "findings", Bound::Max));
        let margin = match bound {
            Bound::Min => measured - limit,
            Bound::Max => limit - measured,
        };
        let param = d.param.unwrap_or_else(|| rule.map_or("", |r| r.param).to_string());
        self.list.push(DrcFinding {
            code,
            class,
            severity,
            key,
            approved,
            pending_user,
            by: approval.and_then(|a| a.by),
            message: d.message,
            message_key: format!("electronics.drc_msg_{code}"),
            args,
            at: d.at,
            layer: d.layer,
            items,
            nets: d.nets,
            measured,
            measured_text: d.texts.map(|t| t.0),
            limit: Limit { value: limit, units, text: d.texts.map(|t| t.1), param, bound },
            margin,
            value: d.value,
            required: d.required,
            fix: d.fix,
            section: rule.map_or("", |r| r.section),
            note: d.note,
        });
    }
}

/// How copper is told apart: by net, or (for copper on no net, and islands that join two nets)
/// by island.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Owner {
    Net(usize),
    Island(usize),
    /// A copper drawing, by index into the board's drawings.
    Drawing(usize),
}

/// A piece of copper on one layer.
struct Piece {
    /// An index into the check's items.
    item: usize,
    core: Core,
    owner: Owner,
    /// The net its clearance comes from.
    net: Option<usize>,
    /// A pour's fill edge: the pour's own clearance, and it only meets other copper.
    fill: Option<f64>,
}

/// What the checks share.
struct Ctx<'a> {
    ecad: &'a Electronics,
    board: &'a Board,
    nets: &'a BoardNets,
    sizes: Vec<Sizes>,
    /// Each island's nets.
    island_nets: BTreeMap<usize, BTreeSet<usize>>,
    /// The board's outline as straight pieces, and as a ring; empty without one.
    edges: Vec<([f64; 2], [f64; 2])>,
    ring: Vec<[f64; 2]>,
    edge_grid: Grid,
    /// How many pair tests, and looks at neighbours, are left.
    budget: usize,
    visits: usize,
    /// The fab profile it checks against, and the stack-up (for holes' depths).
    profile: Option<&'static FabProfile>,
    stackup: Vec<StackLayer>,
    /// Copper pairs that pass by less than [`AT_LIMIT`], and pairs within [`AT_MINIMUM`] of the
    /// profile's least gap (by item, as the clearance check numbers them).
    at_limit: BTreeSet<(usize, usize)>,
    gap_at_minimum: BTreeSet<(usize, usize)>,
    smallest_gap: Option<f64>,
}

impl Ctx<'_> {
    /// The hole a mechanical drill of finished size `drill` is measured from (see the module
    /// notes).
    fn hole(&self, drill: f64) -> f64 {
        self.profile.map_or(drill, |p| p.hole(drill))
    }

    /// How messages name that hole.
    fn hole_words(&self, drill: f64) -> String {
        match self.profile {
            Some(p) if p.ring_convention == RingConvention::Tool => {
                format!("its drill tool ({} mm finished + {} mm plating)", mm(drill), mm(p.plating_allowance.value))
            }
            _ => format!("its {} mm finished hole", mm(drill)),
        }
    }

    fn name(&self, n: usize) -> String {
        self.nets.name(n).unwrap_or_default().to_string()
    }

    /// The owner and the clearance net of copper in island `root`.
    fn owner(&self, root: usize) -> (Owner, Option<usize>) {
        match self.island_nets.get(&root) {
            Some(set) if set.len() == 1 => {
                let n = set.iter().next().copied();
                (n.map_or(Owner::Island(root), Owner::Net), n)
            }
            Some(set) => (Owner::Island(root), set.iter().next().copied()),
            None => (Owner::Island(root), None),
        }
    }

    /// Spend `n` tests; false (and nothing spent) once the budget is gone.
    fn spend(&mut self, n: usize) -> bool {
        if self.budget < n {
            self.budget = 0;
            return false;
        }
        self.budget -= n;
        true
    }

    /// Look at `n` neighbours; false (and nothing spent) once that budget is gone.
    fn visit(&mut self, n: usize) -> bool {
        if self.visits < n {
            self.visits = 0;
            return false;
        }
        self.visits -= n;
        true
    }

    /// Whether `p` is on the board (true without an outline, and once the budget is gone: each
    /// point walks the whole outline).
    fn on_board(&mut self, p: [f64; 2]) -> bool {
        self.ring.len() < 3 || !self.spend(self.ring.len()) || inside(p, &self.ring)
    }

    /// The least gap between `core` and the board's edge, with where; `None` without an outline.
    fn edge_gap(&mut self, core: &Core) -> Option<(f64, [f64; 2])> {
        if self.edges.is_empty() {
            return None;
        }
        let near = self.edge_grid.near_box(grow(core_bbox(core), self.board.rules.copper_to_edge.max(1.0)), self.edges.len());
        let mut best: Option<(f64, [f64; 2])> = None;
        for i in near {
            if !self.spend(1) {
                break;
            }
            let Some((a, b)) = self.edges.get(i) else { continue };
            let edge = Core { points: vec![*a, *b], radius: 0.0 };
            let g = gap(core, &edge);
            if best.is_none_or(|(d, _)| g < d) {
                best = Some((g, between(core, &edge)));
            }
        }
        best
    }
}

/// A point between two cores where they come closest (inside both where they overlap).
fn between(a: &Core, b: &Core) -> [f64; 2] {
    let segments = |c: &Core| -> Vec<([f64; 2], [f64; 2])> {
        let n = c.points.len();
        match n {
            0 => Vec::new(),
            1 => c.points.first().map(|p| vec![(*p, *p)]).unwrap_or_default(),
            _ => (0..n).filter_map(|i| Some((*c.points.get(i)?, *c.points.get((i + 1) % n)?))).collect(),
        }
    };
    let mut best = (f64::INFINITY, [0.0, 0.0], [0.0, 0.0]);
    for (p0, p1) in segments(a) {
        for (q0, q1) in segments(b) {
            let (d, p, q) = segment_closest(p0, p1, q0, q1);
            if d < best.0 {
                best = (d, p, q);
            }
        }
    }
    let (d, p, q) = best;
    if !d.is_finite() {
        return a.points.first().copied().unwrap_or_default();
    }
    // Off each core's middle line by its radius, towards the other.
    let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
    let l = dx.hypot(dy);
    if l <= 1e-12 {
        return p;
    }
    let (ux, uy) = (dx / l, dy / l);
    let pa = [p[0] + ux * a.radius, p[1] + uy * a.radius];
    let qb = [q[0] - ux * b.radius, q[1] - uy * b.radius];
    [(pa[0] + qb[0]) / 2.0, (pa[1] + qb[1]) / 2.0]
}

/// The closest points of segments `p0`–`p1` and `q0`–`q1`, and their distance.
fn segment_closest(p0: [f64; 2], p1: [f64; 2], q0: [f64; 2], q1: [f64; 2]) -> (f64, [f64; 2], [f64; 2]) {
    let project = |x: [f64; 2], a: [f64; 2], b: [f64; 2]| path::project(x, a, b, 0.0).0;
    let dist = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let mut best = (f64::INFINITY, p0, q0);
    for (p, q) in [(p0, project(p0, q0, q1)), (p1, project(p1, q0, q1)), (project(q0, p0, p1), q0), (project(q1, p0, p1), q1)] {
        let d = dist(p, q);
        if d < best.0 {
            best = (d, p, q);
        }
    }
    // Crossing segments meet inside both.
    let (r, s) = ([p1[0] - p0[0], p1[1] - p0[1]], [q1[0] - q0[0], q1[1] - q0[1]]);
    let den = r[0] * s[1] - r[1] * s[0];
    if den.abs() > 1e-15 {
        let t = ((q0[0] - p0[0]) * s[1] - (q0[1] - p0[1]) * s[0]) / den;
        let u = ((q0[0] - p0[0]) * r[1] - (q0[1] - p0[1]) * r[0]) / den;
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            let x = [p0[0] + r[0] * t, p0[1] + r[1] * t];
            best = (0.0, x, x);
        }
    }
    best
}

/// The pad as findings name it.
fn pad_item(p: &BoardPad) -> DrcItem {
    DrcItem::Pad { footprint: p.footprint, number: p.pad.number.clone(), reference: p.reference() }
}

/// The design rule check of `ecad`'s board, whose connections are `nets` (see the module notes).
pub fn check(ecad: &Electronics, nets: &BoardNets) -> DrcReport {
    let board = &ecad.board;
    let ring = if board.outline.is_empty() { Vec::new() } else { path::tessellate(&board.outline, &board.outline_curves, true, TOLERANCE) };
    let edges = if board.outline.is_empty() { Vec::new() } else { pieces(&board.outline, &board.outline_curves, true) };
    let mut edge_grid = Grid::new(CELL);
    for (i, (a, b)) in edges.iter().enumerate() {
        edge_grid.insert(i, path::segment_bbox(*a, *b, 0.0));
    }
    let mut island_nets: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (p, root) in nets.pads.iter().zip(&nets.pad_islands) {
        if let Some(n) = p.net {
            island_nets.entry(*root).or_default().insert(n);
        }
    }
    for (own, root) in nets.via_own_nets.iter().zip(&nets.via_islands) {
        if let Some(n) = own {
            island_nets.entry(*root).or_default().insert(*n);
        }
    }
    for (f, roots) in nets.fills.iter().zip(&nets.fill_islands) {
        if let Some(n) = f.net {
            for root in roots {
                island_nets.entry(*root).or_default().insert(n);
            }
        }
    }
    let mut c = Ctx {
        ecad,
        board,
        nets,
        sizes: board.rules.sizes_by_net(&nets.names),
        island_nets,
        edges,
        ring,
        edge_grid,
        budget: MAX_TESTS,
        visits: MAX_VISITS,
        profile: board.fab_profile.as_ref().and_then(|p| p.profile()),
        stackup: board.stackup_or_default(),
        at_limit: BTreeSet::new(),
        gap_at_minimum: BTreeSet::new(),
        smallest_gap: None,
    };
    let mut f = Found {
        approved: board.approvals.iter().map(|a| (a.key.as_str(), a)).collect(),
        levels: &board.drc_levels,
        seen: BTreeSet::new(),
        list: Vec::new(),
        truncated: nets.truncated,
    };
    shorts(&c, &mut f);
    clearances(&mut c, &mut f);
    sizes(&c, &mut f);
    aspect_ratios(&c, &mut f);
    drill_to_copper(&mut c, &mut f);
    vias_in_pads(&mut c, &mut f);
    holes(&mut c, &mut f);
    edges_and_outline(&mut c, &mut f);
    footprints(&mut c, &mut f);
    keepouts(&mut c, &mut f);
    reliefs(&c, &mut f);
    silkscreen(&mut c, &mut f);
    dfm::vias_near_pads(&mut c, &mut f);
    dfm::pad_entries_and_pockets(&mut c, &mut f);
    dfm::slivers(&mut c, &mut f);
    unrouted(&c, &mut f);
    ends(&mut c, &mut f);
    pours(&c, &mut f);
    profile(&c, &mut f);
    if c.budget == 0 || c.visits == 0 {
        f.truncated = true;
    }
    let order = |code: &str| RULES.iter().position(|r| r.code == code).unwrap_or(RULES.len());
    let mut findings = f.list;
    findings.sort_by_key(|x| (x.severity, order(x.code)));
    let approved = findings.iter().filter(|x| x.approved).count();
    let pending_user = findings.iter().filter(|x| x.pending_user).count();
    let open = |sev: Severity| findings.iter().filter(|x| !x.approved && x.severity == sev).count();
    let (errors, warnings, infos) = (open(Severity::Error), open(Severity::Warning), open(Severity::Info));
    // Past a budget, findings not reached may hold approvals: none is named stale then.
    let stale = if f.truncated { Vec::new() } else { board.approvals.iter().filter(|a| !f.seen.contains(&a.key)).map(|a| a.key.clone()).collect() };
    // The checks that read the profile's own figures don't run without one, and the board's
    // levels may leave codes out.
    let mut not_applicable = if c.profile.is_none() && board.has_content() {
        vec![("via_aspect_ratio", "board.fab_profile"), ("drill_to_copper", "board.fab_profile")]
    } else {
        Vec::new()
    };
    for (code, level) in &board.drc_levels {
        if *level == CheckLevel::Ignore
            && let Some(r) = find_rule(code)
        {
            not_applicable.push((r.code, "board.drc_levels"));
        }
    }
    DrcReport {
        findings,
        errors,
        warnings,
        infos,
        approved,
        pending_user,
        stale,
        at_limit: c.at_limit.len(),
        profile: c.profile,
        not_applicable,
        truncated: f.truncated,
    }
}

/// Islands that join two nets or more.
fn shorts(c: &Ctx, f: &mut Found) {
    let shorted: Vec<(&usize, &BTreeSet<usize>)> = c.island_nets.iter().filter(|(_, s)| s.len() >= 2).collect();
    if shorted.is_empty() {
        return;
    }
    let places = Touches::new(c);
    for (root, set) in shorted {
        let names: Vec<String> = set.iter().map(|n| c.name(*n)).collect();
        let mut items: Vec<DrcItem> = Vec::new();
        let mut at = None;
        // One pad of each net, the second net's pad marking the place.
        let mut shown = BTreeSet::new();
        for (p, r) in c.nets.pads.iter().zip(&c.nets.pad_islands) {
            if r == root
                && let Some(n) = p.net
                && shown.insert(n)
            {
                items.push(pad_item(p));
                if shown.len() == 2 {
                    at = Some(p.at());
                }
            }
        }
        let n = names.len() as f64;
        let message = format!("copper joins the nets {}: a short circuit", names.join(", "));
        // Where two nets' copper meets: the two pieces that touch, and the point.
        if let Some((a, b, point)) = places.meeting(c, *root) {
            let mut keys: Vec<String> = items.iter().map(DrcItem::key).collect();
            keys.extend([a.key(), b.key()]);
            let message = format!("{} and {} touch, joining the nets {}: a short circuit", a.reference(), b.reference(), names.join(", "));
            f.add("short", &keys, || {
                Draft::new(point, vec![a, b], message).nets(names).count(n, 1.0, "nets", Bound::Max).fix(
                    "ecad.board.inspect",
                    Value::Null,
                    "separate the two where they touch (move, reroute or delete one)",
                )
            });
            continue;
        }
        let tracks = c.board.tracks.iter().zip(&c.nets.track_islands).filter(|(_, r)| *r == root).map(|(t, _)| DrcItem::Track(t.id));
        let vias = c.board.vias.iter().zip(&c.nets.via_islands).filter(|(_, r)| *r == root).map(|(v, _)| DrcItem::Via(v.id));
        let copper: Vec<DrcItem> = tracks.chain(vias).collect();
        let at = at.or_else(|| c.board.vias.iter().zip(&c.nets.via_islands).find(|(_, r)| *r == root).map(|(v, _)| v.at)).unwrap_or_default();
        // The pads name it too (one of each net), so shorts of pads alone have keys of their own.
        let keys: Vec<String> = items.iter().chain(&copper).map(DrcItem::key).collect();
        items.extend(copper);
        f.add("short", &keys, || {
            Draft::new(at, items, message).nets(names).count(n, 1.0, "nets", Bound::Max).fix(
                "ecad.board.inspect",
                Value::Null,
                "find where the nets' copper meets and separate it",
            )
        });
    }
}

/// What touches what on the board (pads, tracks, vias and pours' islands, numbered as the
/// islands are: pads, tracks, vias, then each fill's shapes), for finding where a short is.
struct Touches {
    edges: Vec<(usize, usize, [f64; 2])>,
    /// Each fill's first node.
    fill_base: Vec<usize>,
}

impl Touches {
    fn new(c: &Ctx) -> Self {
        let nets = c.nets;
        let (np, nt, nv) = (nets.pads.len(), c.board.tracks.len(), c.board.vias.len());
        let mut edges = super::airwires::contacts(c.board, &nets.pads);
        let mut fill_base = Vec::with_capacity(nets.fills.len());
        let mut next = np + nt + nv;
        for fill in &nets.fills {
            fill_base.push(next);
            for (shape, node) in &fill.joins {
                let (other, at) = match node {
                    super::fill::Node::Pad(i) => (*i, nets.pads.get(*i).map(BoardPad::at)),
                    super::fill::Node::Track(t) => (np + t, c.board.tracks.get(*t).and_then(|x| x.points.first().copied())),
                    super::fill::Node::Via(v) => (np + nt + v, c.board.vias.get(*v).map(|x| x.at)),
                    // Overlapping islands of two pours of one net are one net's copper.
                    super::fill::Node::Fill(..) => continue,
                };
                if let Some(at) = at {
                    edges.push((next + shape, other, at));
                }
            }
            next += fill.region.len();
        }
        Touches { edges, fill_base }
    }

    /// The node `node` as a finding's item, the island it is in, and the net it is on of its
    /// own (a pad's, a via's own net, a pour's).
    fn node(&self, c: &Ctx, node: usize) -> Option<(DrcItem, usize, Option<usize>)> {
        let nets = c.nets;
        let (np, nt, nv) = (nets.pads.len(), c.board.tracks.len(), c.board.vias.len());
        if node < np {
            let p = nets.pads.get(node)?;
            return Some((pad_item(p), *nets.pad_islands.get(node)?, p.net));
        }
        if node < np + nt {
            let t = node - np;
            return Some((DrcItem::Track(c.board.tracks.get(t)?.id), *nets.track_islands.get(t)?, None));
        }
        if node < np + nt + nv {
            let v = node - np - nt;
            return Some((DrcItem::Via(c.board.vias.get(v)?.id), *nets.via_islands.get(v)?, nets.via_own_nets.get(v).copied().flatten()));
        }
        let k = self.fill_base.iter().rposition(|b| *b <= node)?;
        let (fill, base) = (nets.fills.get(k)?, self.fill_base.get(k)?);
        let shape = node - base;
        Some((DrcItem::Pour(fill.pour), *nets.fill_islands.get(k)?.get(shape)?, fill.net))
    }

    /// In the island `root`, the two pieces of copper where two nets meet and the point: each
    /// piece takes the net of the copper of its own net it is fewest joins from (pads, vias with
    /// a net of their own, pours), and the join between two pieces of different nets that lies
    /// on the shortest way from one net to the other is where they meet.
    fn meeting(&self, c: &Ctx, root: usize) -> Option<(DrcItem, DrcItem, [f64; 2])> {
        let mut nodes: BTreeMap<usize, (DrcItem, Option<usize>)> = BTreeMap::new();
        let mut adjacent: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (a, b, _) in &self.edges {
            for n in [*a, *b] {
                if let std::collections::btree_map::Entry::Vacant(slot) = nodes.entry(n)
                    && let Some((item, _, net)) = self.node(c, n).filter(|x| x.1 == root)
                {
                    slot.insert((item, net));
                }
            }
            if nodes.contains_key(a) && nodes.contains_key(b) {
                adjacent.entry(*a).or_default().push(*b);
                adjacent.entry(*b).or_default().push(*a);
            }
        }
        // Breadth first from every piece with a net of its own.
        let mut label: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
        let mut queue = std::collections::VecDeque::new();
        for (n, (_, net)) in &nodes {
            if let Some(net) = net {
                label.insert(*n, (*net, 0));
                queue.push_back(*n);
            }
        }
        while let Some(n) = queue.pop_front() {
            let Some(&(net, d)) = label.get(&n) else { continue };
            for m in adjacent.get(&n).into_iter().flatten() {
                if let std::collections::btree_map::Entry::Vacant(slot) = label.entry(*m) {
                    slot.insert((net, d + 1));
                    queue.push_back(*m);
                }
            }
        }
        let mut best: Option<(usize, usize, usize, [f64; 2])> = None;
        for (a, b, at) in &self.edges {
            let (Some(&(na, da)), Some(&(nb, db))) = (label.get(a), label.get(b)) else { continue };
            if na != nb && best.is_none_or(|x| da + db < x.0) {
                best = Some((da + db, *a, *b, *at));
            }
        }
        let (_, a, b, at) = best?;
        Some((nodes.get(&a)?.0.clone(), nodes.get(&b)?.0.clone(), at))
    }
}

/// Copper of two owners closer than their clearance, layer by layer.
fn clearances(c: &mut Ctx, f: &mut Found) {
    let board = c.board;
    let nets = c.nets;
    let layers = board.copper_layers;
    // The check's items: pads, then tracks, vias, pours and drawings.
    let mut items: Vec<DrcItem> = nets.pads.iter().map(pad_item).collect();
    let track_base = items.len();
    items.extend(board.tracks.iter().map(|t| DrcItem::Track(t.id)));
    let via_base = items.len();
    items.extend(board.vias.iter().map(|v| DrcItem::Via(v.id)));
    let pour_base = items.len();
    items.extend(nets.fills.iter().map(|x| DrcItem::Pour(x.pour)));
    let graphic_base = items.len();
    items.extend(board.graphics.iter().map(|g| DrcItem::Graphic(g.id)));
    let mut worst: BTreeMap<(usize, usize), (f64, f64, [f64; 2], CopperLayer)> = BTreeMap::new();
    // What each copper drawing touches (owner → an item of it, where, the layer), and the
    // drawings' near misses (kept unless the drawing touches the other copper's owner).
    let mut touches: BTreeMap<usize, BTreeMap<Owner, (usize, [f64; 2], CopperLayer)>> = BTreeMap::new();
    let mut drawing_near: BTreeMap<(usize, usize), (f64, f64, [f64; 2], CopperLayer, usize, Owner)> = BTreeMap::new();
    let mut fill_edges = 0usize;
    let max_gap =
        c.sizes.iter().map(|s| s.clearance).fold(board.rules.clearance, f64::max).max(nets.fills.iter().map(|x| x.clearance).fold(0.0, f64::max));
    for layer in board.stack() {
        let mut list: Vec<Piece> = Vec::new();
        for (i, p) in nets.pads.iter().enumerate().filter(|(_, p)| p.on(layer)) {
            let Some(root) = nets.pad_islands.get(i) else { continue };
            let (owner, net) = c.owner(*root);
            list.push(Piece { item: i, core: pad_core(p), owner, net, fill: None });
        }
        for (t, track) in board.tracks.iter().enumerate().filter(|(_, t)| t.layer == layer) {
            let Some(root) = nets.track_islands.get(t) else { continue };
            let (owner, net) = c.owner(*root);
            for core in track_cores(track) {
                list.push(Piece { item: track_base + t, core, owner, net, fill: None });
            }
        }
        for (v, via) in board.vias.iter().enumerate().filter(|(_, v)| v.on(layer, layers)) {
            let Some(root) = nets.via_islands.get(v) else { continue };
            let (owner, net) = c.owner(*root);
            list.push(Piece { item: via_base + v, core: disc(via.at, via.diameter / 2.0), owner, net, fill: None });
        }
        for (k, (fill, roots)) in nets.fills.iter().zip(&nets.fill_islands).enumerate().filter(|(_, (x, _))| x.layer == layer) {
            for (s, shape) in fill.shapes().iter().enumerate() {
                let Some(root) = roots.get(s) else { continue };
                let (owner, net) = c.owner(*root);
                for contour in std::iter::once(&shape.outer).chain(&shape.holes) {
                    let n = contour.len();
                    fill_edges = fill_edges.saturating_add(n);
                    if fill_edges > MAX_FILL_EDGES {
                        f.truncated = true;
                        continue;
                    }
                    for i in 0..n {
                        let (Some(a), Some(b)) = (contour.get(i), contour.get((i + 1) % n)) else { continue };
                        list.push(Piece {
                            item: pour_base + k,
                            core: Core { points: vec![*a, *b], radius: 0.0 },
                            owner,
                            net,
                            fill: Some(fill.clearance),
                        });
                    }
                }
            }
        }
        for (k, g) in board.graphics.iter().enumerate().filter(|(_, g)| g.layer == DrawLayer::Copper(layer)) {
            for core in graphic_cores(&g.graphic, &|p| p) {
                list.push(Piece { item: graphic_base + k, core, owner: Owner::Drawing(k), net: None, fill: None });
            }
        }
        let mut grid = Grid::new(CELL);
        for (i, p) in list.iter().enumerate() {
            grid.insert(i, core_bbox(&p.core));
        }
        'pieces: for (i, a) in list.iter().enumerate() {
            let near = grid.near_box(grow(core_bbox(&a.core), max_gap), list.len());
            if !c.visit(near.len()) {
                break 'pieces;
            }
            for j in near {
                if j <= i {
                    continue;
                }
                let Some(b) = list.get(j) else { continue };
                let drawing = |p: &Piece| match p.owner {
                    Owner::Drawing(d) => Some(d),
                    _ => None,
                };
                if a.owner == b.owner || a.item == b.item || (drawing(a).is_some() && drawing(b).is_some()) {
                    continue;
                }
                if !c.spend(1) {
                    break 'pieces;
                }
                // A fill keeps its pour's clearance in place of its net class's.
                let required = match (a.fill, b.fill) {
                    (Some(x), Some(y)) => x.max(y),
                    (Some(x), None) => x.max(clearance_between(&board.rules, &c.sizes, b.net, b.net)),
                    (None, Some(y)) => y.max(clearance_between(&board.rules, &c.sizes, a.net, a.net)),
                    (None, None) => clearance_between(&board.rules, &c.sizes, a.net, b.net),
                };
                let slack = if a.fill.is_some() || b.fill.is_some() { FILL_SLACK } else { 0.0 };
                let g = gap(&a.core, &b.core);
                let pair = (a.item.min(b.item), a.item.max(b.item));
                // A drawing joins what it touches; its near misses wait for all it touches.
                if let Some((d, other)) = drawing(a).map(|d| (d, b)).or_else(|| drawing(b).map(|d| (d, a))) {
                    if g <= 1e-9 {
                        touches.entry(d).or_default().entry(other.owner).or_insert((other.item, between(&a.core, &b.core), layer));
                    } else if g < required - slack - 1e-6 && drawing_near.get(&pair).is_none_or(|w| g < w.0) {
                        drawing_near.insert(pair, (g, required, between(&a.core, &b.core), layer, d, other.owner));
                    }
                    continue;
                }
                if g < required - slack - 1e-6 && worst.get(&pair).is_none_or(|w| g < w.0) {
                    worst.insert(pair, (g, required, between(&a.core, &b.core), layer));
                }
                // Fills keep their gap by making it: only other copper's gaps tell how close the
                // design comes to its limits.
                if a.fill.is_none() && b.fill.is_none() && g > 1e-9 {
                    c.smallest_gap = Some(c.smallest_gap.map_or(g, |s| s.min(g)));
                    if g >= required - 1e-6 && g < required * (1.0 + AT_LIMIT) {
                        c.at_limit.insert(pair);
                    }
                    if let Some(p) = c.profile
                        && g >= p.gap.value - 1e-6
                        && g < p.gap.value + AT_MINIMUM
                    {
                        c.gap_at_minimum.insert(pair);
                    }
                }
            }
        }
    }
    for (pair, (g, required, at, layer, d, other)) in drawing_near {
        if !touches.get(&d).is_some_and(|t| t.contains_key(&other)) && worst.get(&pair).is_none_or(|w| g < w.0) {
            worst.insert(pair, (g, required, at, layer));
        }
    }
    let item_net = |i: usize| -> Vec<String> {
        let n = if i < track_base {
            nets.pads.get(i).zip(nets.pad_islands.get(i).copied()).map(|(_, r)| r)
        } else if i < via_base {
            nets.track_islands.get(i - track_base).copied()
        } else if i < pour_base {
            nets.via_islands.get(i - via_base).copied()
        } else {
            None
        };
        n.and_then(|r| c.island_nets.get(&r)).map(|s| s.iter().map(|n| c.name(*n)).collect()).unwrap_or_default()
    };
    // A drawing that touches copper of two owners joins them.
    let owner_nets = |o: &Owner| -> Vec<String> {
        match o {
            Owner::Net(n) => vec![c.name(*n)],
            Owner::Island(r) => c.island_nets.get(r).map(|s| s.iter().map(|n| c.name(*n)).collect()).unwrap_or_default(),
            Owner::Drawing(_) => Vec::new(),
        }
    };
    for (d, touched) in &touches {
        if touched.len() < 2 {
            continue;
        }
        let Some(drawn) = items.get(graphic_base + d).cloned() else { continue };
        let drawn_id = drawn.id();
        let mut names: Vec<String> = Vec::new();
        let mut found = vec![drawn];
        let mut at = None;
        for (owner, (item, where_, layer)) in touched {
            for n in owner_nets(owner) {
                if !names.contains(&n) {
                    names.push(n);
                }
            }
            found.extend(items.get(*item).cloned());
            at = at.or(Some((*where_, *layer)));
        }
        let Some((at, layer)) = at else { continue };
        let keys: Vec<String> = found.iter().map(DrcItem::key).collect();
        let refs: Vec<String> = found.iter().map(DrcItem::reference).collect();
        let (first, rest) = (refs.first().cloned().unwrap_or_default(), refs.get(1..).unwrap_or_default().join(", "));
        let message = format!("{first} joins copper of {rest}: a short circuit");
        let n = names.len().max(2) as f64;
        f.add("short", &keys, || {
            Draft::new(at, found, message).layer(Some(layer)).nets(names).count(n, 1.0, "nets", Bound::Max).fix(
                "ecad.board.delete",
                json!({"items": [drawn_id]}),
                "delete the copper drawing, or move it off one of the nets",
            )
        });
    }
    for ((a, b), (g, required, at, layer)) in worst {
        let (Some(ia), Some(ib)) = (items.get(a).cloned(), items.get(b).cloned()) else { continue };
        let mut names = item_net(a);
        for n in item_net(b) {
            if !names.contains(&n) {
                names.push(n);
            }
        }
        let keys = [ia.key(), ib.key()];
        let (ra, rb) = (ia.reference(), ib.reference());
        // A track among them can be narrowed by the gap it lacks, while it stays wide enough.
        let narrow = [&ia, &ib].into_iter().find_map(|i| match i {
            DrcItem::Track(id) => c.board.track(*id).map(|t| (*id, up(t.width - 2.0 * (required - g) - 0.005))),
            _ => None,
        });
        let least = c.board.rules.min_track_width;
        if g <= 1e-9 {
            let message = format!("{ra} and {rb} touch on {}: a short circuit", layer.name());
            f.add("short", &keys, || {
                Draft::new(at, vec![ia, ib], message).layer(Some(layer)).nets(names).gap(g, required).param("rules.clearance").fix(
                    "ecad.board.move",
                    Value::Null,
                    "move one of them off the other (ecad.board.move, or reroute the track)",
                )
            });
        } else {
            let message = format!("{ra} and {rb} are {} mm apart on {}; the clearance is {} mm", mm(g), layer.name(), mm(required));
            f.add("clearance", &keys, || {
                let d = Draft::new(at, vec![ia, ib], message).layer(Some(layer)).nets(names).measure(g, required);
                match narrow {
                    Some((id, w)) if w >= least - 1e-9 => d.fix(
                        "ecad.board.track.edit",
                        json!({"tracks": [id], "width": w}),
                        format!("narrow track {id} to {} mm, or move it {} mm away", mm(w), mm(step(required - g))),
                    ),
                    _ => d.fix("ecad.board.move", Value::Null, format!("move them {} mm further apart", mm(step(required - g)))),
                }
            });
        }
    }
}

/// The one edit that gives via `v` a drill and a land its drill, ring and aspect ratio findings
/// all pass with: at least the least drill (a micro via's own), wide enough for the profile's
/// aspect ratio, and the least ring round the hole as the profile measures it; never smaller
/// than it is. Its `ecad.board.via.edit` params and what it says.
fn via_fix(c: &Ctx, v: &Via) -> (Value, String) {
    let board = c.board;
    let rules = &board.rules;
    let n = board.copper_layers;
    let micro = v.kind(n, rules) == ViaKind::Micro;
    let mut drill = v.drill.max(if micro { rules.microvia_drill } else { rules.min_drill });
    if let Some(p) = c.profile {
        let (a, b) = v.span(n);
        let deep = if v.is_through() { board.thickness } else { depth(&c.stackup, a, b) };
        match (micro, p.micro) {
            (true, Some(m)) => drill = drill.max(deep / m.aspect.value.max(1e-9)),
            (true, None) => {}
            // The hole as the profile measures it is the drill plus what its convention adds.
            (false, _) => drill = drill.max(deep / p.aspect_ratio.value.max(1e-9) - (c.hole(v.drill) - v.drill)),
        }
    }
    let drill = if drill > v.drill + 1e-9 { up(drill) } else { v.drill };
    let hole = if micro { drill } else { c.hole(drill) };
    let diameter = up(hole + 2.0 * rules.min_annular_ring).max(v.diameter);
    if drill > v.drill {
        (json!({"vias": [v.id], "drill": drill, "diameter": diameter}), format!("drill it {} mm, {} mm across", mm(drill), mm(diameter)))
    } else {
        (json!({"vias": [v.id], "diameter": diameter}), format!("make it {} mm across", mm(diameter)))
    }
}

/// What a micro via `v` through the copper layers `span` (indexes, top first), `deep` mm deep,
/// needs to be at most `most` mm deep (and at most `deepest`): the dielectric it goes through that
/// much thinner in the stack-up, in words. Its depth is that dielectric and the copper of the
/// layer it starts on.
fn microvia_stack_fix(c: &Ctx, v: &Via, (a, b): (usize, usize), deep: f64, most: f64, deepest: f64) -> String {
    let n = c.board.copper_layers;
    let copper: f64 = c.stackup.iter().filter(|l| l.kind == StackKind::Copper).skip(a).take(b.saturating_sub(a)).map(|l| l.thickness).sum();
    let now = (deep - copper).max(0.0);
    let thinner = ((most.min(deepest) - copper) * 1000.0 + 1e-6).floor() / 1000.0;
    let [from, to] = [a, b].map(|i| CopperLayer::at(i, n).map_or_else(|| i.to_string(), CopperLayer::name));
    if thinner <= 0.0 {
        return format!("the copper of {from} alone is too deep for a {} mm laser via: drill it wider", mm(v.drill));
    }
    format!(
        "make the dielectric between {from} and {to} {} mm thick or thinner in the stack-up (now {} mm), so a {} mm laser via is at most {} mm deep; lines of a set impedance on {from} get narrower with it",
        mm(thinner),
        mm(now),
        mm(v.drill),
        mm(most.min(deepest))
    )
}

/// Why the check would call via `v` on `board` an error for its size alone (its drill, its ring
/// round the hole as the fab profile measures it, its aspect ratio), as [`sizes`] and
/// [`aspect_ratios`] measure them, in English; `None` when its size passes. A tool that places
/// many vias of one size refuses a size here rather than make as many findings.
pub fn via_size_error(board: &Board, v: &Via) -> Option<String> {
    let rules = &board.rules;
    let n = board.copper_layers;
    let profile = board.fab_profile.as_ref().and_then(|p| p.profile());
    let micro = v.kind(n, rules) == ViaKind::Micro;
    let least = if micro { rules.microvia_drill } else { rules.min_drill };
    if v.drill < least - 1e-9 {
        return Some(format!(
            "a {} mm drill is below the rules' least {}, {} mm",
            mm(v.drill),
            if micro { "micro via drill" } else { "drill" },
            mm(least)
        ));
    }
    let hole = if micro { v.drill } else { profile.map_or(v.drill, |p| p.hole(v.drill)) };
    let ring = (v.diameter - hole) / 2.0;
    if ring < rules.min_annular_ring - 1e-9 {
        let round =
            if hole > v.drill + 1e-9 { format!("its {} mm drill tool (the finished hole and the plating)", mm(hole)) } else { "its hole".into() };
        return Some(format!(
            "a {}/{} mm via has a {} mm ring round {round}; the least is {} mm (make it {} mm across)",
            mm(v.diameter),
            mm(v.drill),
            mm(ring),
            mm(rules.min_annular_ring),
            mm(up(hole + 2.0 * rules.min_annular_ring))
        ));
    }
    let p = profile?;
    let (a, b) = v.span(n);
    let deep = if v.is_through() { board.thickness } else { depth(&board.stackup_or_default(), a, b) };
    if micro {
        let m = p.micro.as_ref()?;
        let ratio = deep / v.drill.max(1e-9);
        if ratio > m.aspect.value + 1e-9 && !m.aspect.verify {
            return Some(format!("a micro via {} mm deep for a {} mm hole is {ratio:.2}:1; the most is {}:1", mm(deep), mm(v.drill), m.aspect.value));
        }
        if deep > m.depth.value + 1e-9 && !m.depth.verify {
            return Some(format!("a micro via {} mm deep is past the deepest, {} mm", mm(deep), mm(m.depth.value)));
        }
        return None;
    }
    let ratio = deep / hole.max(1e-9);
    (ratio > p.aspect_ratio.value + 1e-9)
        .then(|| format!("a via {} mm deep through a {} mm hole is {ratio:.2}:1; the most is {}:1", mm(deep), mm(hole), p.aspect_ratio.value))
}

/// The least diameter via `v`'s ring passes with on `board` (its hole as the fab profile
/// measures it, and the least annular ring round it), rounded up to 0.01 mm.
pub fn least_via_diameter(board: &Board, v: &Via) -> f64 {
    let rules = &board.rules;
    let micro = v.kind(board.copper_layers, rules) == ViaKind::Micro;
    let hole = if micro { v.drill } else { board.fab_profile.as_ref().and_then(|p| p.profile()).map_or(v.drill, |p| p.hole(v.drill)) };
    up(hole + 2.0 * rules.min_annular_ring)
}

/// Tracks, vias and pads against the size rules; rings as the profile measures them.
fn sizes(c: &Ctx, f: &mut Found) {
    let board = c.board;
    let rules = &board.rules;
    for (t, track) in board.tracks.iter().enumerate() {
        let at = track.points.first().copied().unwrap_or_default();
        let key = [DrcItem::Track(track.id).key()];
        if track.width < rules.min_track_width - 1e-9 {
            let message = format!("track {} is {} mm wide; the least is {} mm", track.id, mm(track.width), mm(rules.min_track_width));
            f.add("track_width", &key, || {
                Draft::new(at, vec![DrcItem::Track(track.id)], message).layer(Some(track.layer)).measure(track.width, rules.min_track_width).fix(
                    "ecad.board.track.edit",
                    json!({"tracks": [track.id], "width": rules.min_track_width}),
                    format!("widen it to {} mm", mm(rules.min_track_width)),
                )
            });
            continue;
        }
        let net = c.nets.track_nets.get(t).copied().flatten();
        if let Some(s) = net.and_then(|n| c.sizes.get(n))
            && let Some(class) = &s.class
            && track.width < s.track_width - 1e-9
        {
            let name = c.name(net.unwrap_or_default());
            let message = format!("track {} on net {name} is {} mm wide; its class {class} has {} mm", track.id, mm(track.width), mm(s.track_width));
            f.add("track_width_class", &key, || {
                Draft::new(at, vec![DrcItem::Track(track.id)], message)
                    .layer(Some(track.layer))
                    .nets(vec![name])
                    .measure(track.width, s.track_width)
                    .param(format!("class.{class}.track_width"))
                    .arg("class", class.clone())
                    .fix(
                        "ecad.board.track.edit",
                        json!({"tracks": [track.id], "width": s.track_width}),
                        format!("widen it to its class's {} mm", mm(s.track_width)),
                    )
            });
        }
    }
    for v in &board.vias {
        let item = DrcItem::Via(v.id);
        let key = [item.key()];
        let micro = v.kind(board.copper_layers, rules) == ViaKind::Micro;
        // A laser hole is its drill; a mechanical one as the profile measures it.
        let hole = if micro { v.drill } else { c.hole(v.drill) };
        let ring = (v.diameter - hole) / 2.0;
        if ring < rules.min_annular_ring - 1e-9 {
            let words = if micro { format!("its {} mm laser hole", mm(v.drill)) } else { c.hole_words(v.drill) };
            let message = format!("via {} has a {} mm ring round {words}; the least is {} mm", v.id, mm(ring), mm(rules.min_annular_ring));
            let (params, says) = via_fix(c, v);
            f.add("annular_ring", &key, || {
                Draft::new(v.at, vec![item.clone()], message).measure(ring, rules.min_annular_ring).arg("hole", words.clone()).fix(
                    "ecad.board.via.edit",
                    params,
                    says,
                )
            });
        }
        let (least, param) = if micro { (rules.microvia_drill, "rules.microvia_drill") } else { (rules.min_drill, "rules.min_drill") };
        if v.drill < least - 1e-9 {
            let message = format!("via {} has a {} mm drill; the least is {} mm", v.id, mm(v.drill), mm(least));
            let (params, says) = via_fix(c, v);
            f.add("drill", &key, || {
                Draft::new(v.at, vec![item.clone()], message).measure(v.drill, least).param(param).fix("ecad.board.via.edit", params, says)
            });
        }
    }
    for p in &c.nets.pads {
        let Some([dw, dh]) = p.pad.drill else { continue };
        let item = pad_item(p);
        let key = [item.key()];
        let drill = dw.min(dh);
        // Each side of a slot against the pad's own side that way: the narrower is the ring.
        if p.pad.kind == PadKind::ThroughHole {
            let [w, h] = p.pad.size;
            let ring = ((w - c.hole(dw)) / 2.0).min((h - c.hole(dh)) / 2.0);
            if ring < rules.min_annular_ring - 1e-9 {
                let words = c.hole_words(drill);
                let message =
                    format!("pad {} has a {} mm ring round {words}; the least is {} mm", p.reference(), mm(ring), mm(rules.min_annular_ring));
                f.add("annular_ring", &key, || {
                    Draft::new(p.at(), vec![item.clone()], message).measure(ring, rules.min_annular_ring).arg("hole", words.clone()).fix(
                        "ecad.pad.edit",
                        Value::Null,
                        format!("give the footprint's pad {} a larger size or a smaller drill", p.pad.number),
                    )
                });
            }
        }
        if drill < rules.min_drill - 1e-9 {
            let message = format!("pad {} has a {} mm drill; the least is {} mm", p.reference(), mm(drill), mm(rules.min_drill));
            f.add("drill", &key, || {
                Draft::new(p.at(), vec![item], message).measure(drill, rules.min_drill).fix(
                    "ecad.pad.edit",
                    Value::Null,
                    format!("drill the footprint's pad {} at least {} mm", p.pad.number, mm(rules.min_drill)),
                )
            });
        }
    }
    for h in &board.holes {
        if h.drill < rules.min_drill - 1e-9 {
            let item = DrcItem::Hole(h.id);
            let message = format!("hole {} has a {} mm drill; the least is {} mm", h.id, mm(h.drill), mm(rules.min_drill));
            f.add("drill", &[item.key()], || {
                Draft::new(h.at, vec![item], message).measure(h.drill, rules.min_drill).fix(
                    "ecad.board.hole.edit",
                    json!({"holes": [h.id], "drill": rules.min_drill}),
                    format!("drill it {} mm", mm(rules.min_drill)),
                )
            });
        }
    }
}

/// How deep a hole through the copper layers `a` to `b` (indexes, top first) of `stack` is: the
/// copper of `a` and everything down to `b`'s copper.
fn depth(stack: &[StackLayer], a: usize, b: usize) -> f64 {
    let mut copper = 0usize;
    let mut sum = 0.0;
    for l in stack {
        match l.kind {
            StackKind::Mask => {}
            StackKind::Copper => {
                if copper >= a && copper < b {
                    sum += l.thickness;
                }
                copper += 1;
            }
            StackKind::Core | StackKind::Prepreg => {
                if copper > a && copper <= b {
                    sum += l.thickness;
                }
            }
        }
    }
    sum
}

/// Holes deeper for their size than the profile drills and plates: board thickness (or a blind
/// or buried via's depth) over the hole, and micro vias deeper than wide or than the profile's
/// deepest. Needs a profile.
fn aspect_ratios(c: &Ctx, f: &mut Found) {
    let Some(profile) = c.profile else { return };
    let board = c.board;
    let n = board.copper_layers;
    let ar = profile.aspect_ratio.value;
    for v in &board.vias {
        let item = DrcItem::Via(v.id);
        let (a, b) = v.span(n);
        let deep = if v.is_through() { board.thickness } else { depth(&c.stackup, a, b) };
        if v.kind(n, &board.rules) == ViaKind::Micro {
            let Some(m) = profile.micro else { continue };
            let ratio = deep / v.drill.max(1e-9);
            let note = format!("based on public summaries of IPC-6012: {}", m.aspect.source);
            if ratio > m.aspect.value + 1e-9 {
                let message = format!(
                    "micro via {} is {} mm deep for a {} mm hole ({:.2}:1); the most is {}:1",
                    v.id,
                    mm(deep),
                    mm(v.drill),
                    ratio,
                    m.aspect.value
                );
                // A laser drill stays as it is (a wider one takes room a fine-pitch ball field
                // doesn't have): the build-up dielectric it goes through is what changes.
                let says = microvia_stack_fix(c, v, (a, b), deep, m.aspect.value * v.drill.max(0.0), m.depth.value);
                f.add("via_aspect_ratio", &[item.key()], || {
                    let d = Draft::new(v.at, vec![item.clone()], message)
                        .count(ratio, m.aspect.value, "ratio", Bound::Max)
                        .param("profile.microvia_aspect")
                        .arg("deep", mm(deep))
                        .arg("drill", mm(v.drill))
                        .arg("ratio", format!("{ratio:.2}"))
                        .arg("most", mm(m.aspect.value));
                    let d = d.fix("ecad.board.stack", Value::Null, says);
                    if m.aspect.verify { d.severity(Severity::Warning).note(note.clone()) } else { d }
                });
            } else if deep > m.depth.value + 1e-9 {
                let message = format!("micro via {} is {} mm deep; the deepest is {} mm", v.id, mm(deep), mm(m.depth.value));
                let says = microvia_stack_fix(c, v, (a, b), deep, m.depth.value, m.depth.value);
                f.add("via_aspect_ratio", &[item.key()], || {
                    let d = Draft::new(v.at, vec![item.clone()], message)
                        .count(deep, m.depth.value, "mm", Bound::Max)
                        .param("profile.microvia_depth")
                        .arg("deep", mm(deep))
                        .arg("deepest", mm(m.depth.value));
                    let d = d.fix("ecad.board.stack", Value::Null, format!("{says}, or use a stacked pair of micro vias"));
                    let note = format!("based on public summaries of IPC-T-50: {}", m.depth.source);
                    if m.depth.verify { d.severity(Severity::Warning).note(note) } else { d }
                });
            }
            continue;
        }
        let hole = c.hole(v.drill);
        let ratio = deep / hole.max(1e-9);
        if ratio > ar + 1e-9 {
            let words = c.hole_words(v.drill);
            let message = format!("via {} is {} mm deep through {words}: {:.2}:1; the most is {ar}:1", v.id, mm(deep), ratio);
            let (params, says) = via_fix(c, v);
            // Or what it goes through thinner: the board for a through via, the layers of its
            // span for a blind or buried one.
            let thinner = (deep * ar / ratio.max(1e-9) * 100.0 + 1e-6).floor() / 100.0;
            let (stack, stack_says) = if v.is_through() {
                let ok = THICKNESS.contains(&thinner);
                (ok.then(|| json!({"thickness": thinner})), format!("or make the board {} mm thick (now {})", mm(thinner), mm(deep)))
            } else {
                let [from, to] = [a, b].map(|i| CopperLayer::at(i, n).map_or_else(|| i.to_string(), CopperLayer::name));
                (None, format!("or make the stack-up from {from} to {to} {} mm deep (now {})", mm(thinner), mm(deep)))
            };
            f.add("via_aspect_ratio", &[item.key()], || {
                let d = Draft::new(v.at, vec![item.clone()], message)
                    .count(ratio, ar, "ratio", Bound::Max)
                    .arg("hole", words.clone())
                    .arg("deep", mm(deep))
                    .arg("ratio", format!("{ratio:.2}"))
                    .arg("most", mm(ar))
                    .fix("ecad.board.via.edit", params, says);
                if thinner > 0.0 { d.fix("ecad.board.stack", stack.unwrap_or(Value::Null), stack_says.clone()) } else { d }
            });
        }
    }
    for p in c.nets.pads.iter().filter(|p| p.pad.kind == PadKind::ThroughHole) {
        let Some([dw, dh]) = p.pad.drill else { continue };
        let hole = c.hole(dw.min(dh));
        let ratio = board.thickness / hole.max(1e-9);
        if ratio > ar + 1e-9 {
            let item = pad_item(p);
            let message = format!(
                "pad {}'s hole is {} mm deep through {}: {:.2}:1; the most is {ar}:1",
                p.reference(),
                mm(board.thickness),
                c.hole_words(dw.min(dh)),
                ratio
            );
            let words = c.hole_words(dw.min(dh));
            f.add("via_aspect_ratio", &[item.key()], || {
                Draft::new(p.at(), vec![item], message)
                    .count(ratio, ar, "ratio", Bound::Max)
                    .arg("hole", words)
                    .arg("deep", mm(board.thickness))
                    .arg("ratio", format!("{ratio:.2}"))
                    .arg("most", mm(ar))
                    .fix("ecad.pad.edit", Value::Null, "drill the footprint's pad larger")
            });
        }
    }
}

/// A piece of copper on one layer with what it is, for the checks past clearance.
struct Copper {
    item: DrcItem,
    core: Core,
    owner: Owner,
}

/// The pads, tracks, vias and pour fill edges on `layer`.
fn copper_on(c: &Ctx, layer: CopperLayer, fill_edges: &mut usize) -> (Vec<Copper>, bool) {
    let board = c.board;
    let nets = c.nets;
    let mut out = Vec::new();
    let mut truncated = false;
    for (i, p) in nets.pads.iter().enumerate().filter(|(_, p)| p.on(layer)) {
        let Some(root) = nets.pad_islands.get(i) else { continue };
        out.push(Copper { item: pad_item(p), core: pad_core(p), owner: c.owner(*root).0 });
    }
    for (t, track) in board.tracks.iter().enumerate().filter(|(_, t)| t.layer == layer) {
        let Some(root) = nets.track_islands.get(t) else { continue };
        let owner = c.owner(*root).0;
        out.extend(track_cores(track).map(|core| Copper { item: DrcItem::Track(track.id), core, owner }));
    }
    for (v, via) in board.vias.iter().enumerate().filter(|(_, v)| v.on(layer, board.copper_layers)) {
        let Some(root) = nets.via_islands.get(v) else { continue };
        out.push(Copper { item: DrcItem::Via(via.id), core: disc(via.at, via.diameter / 2.0), owner: c.owner(*root).0 });
    }
    for (fill, roots) in nets.fills.iter().zip(&nets.fill_islands).filter(|(x, _)| x.layer == layer) {
        for (s, shape) in fill.shapes().iter().enumerate() {
            let Some(root) = roots.get(s) else { continue };
            let owner = c.owner(*root).0;
            for contour in std::iter::once(&shape.outer).chain(&shape.holes) {
                let n = contour.len();
                *fill_edges = fill_edges.saturating_add(n);
                if *fill_edges > MAX_FILL_EDGES {
                    truncated = true;
                    continue;
                }
                for i in 0..n {
                    let (Some(a), Some(b)) = (contour.get(i), contour.get((i + 1) % n)) else { continue };
                    out.push(Copper { item: DrcItem::Pour(fill.pour), core: Core { points: vec![*a, *b], radius: 0.0 }, owner });
                }
            }
        }
    }
    (out, truncated)
}

/// On inner layers, holes closer than the profile allows to copper they don't join: vias' and
/// plated pads' holes (as the profile measures them) against other copper. Needs a profile.
fn drill_to_copper(c: &mut Ctx, f: &mut Found) {
    let Some(profile) = c.profile else { return };
    let board = c.board;
    let nets = c.nets;
    let n = board.copper_layers;
    let mut fill_edges = 0usize;
    let mut worst: BTreeMap<(DrcItem, DrcItem), (f64, f64, [f64; 2], CopperLayer)> = BTreeMap::new();
    for layer in board.stack().into_iter().filter(|l| l.is_inner()) {
        // The holes through this layer, with their copper's owner.
        let mut holes: Vec<(DrcItem, Core, Owner, f64)> = Vec::new();
        for (v, via) in board.vias.iter().enumerate().filter(|(_, v)| v.on(layer, n)) {
            let Some(root) = nets.via_islands.get(v) else { continue };
            let micro = via.kind(n, &board.rules) == ViaKind::Micro;
            let hole = if micro { via.drill } else { c.hole(via.drill) };
            holes.push((DrcItem::Via(via.id), disc(via.at, hole / 2.0), c.owner(*root).0, profile.drill_to_copper.value));
        }
        for (i, p) in nets.pads.iter().enumerate().filter(|(_, p)| p.pad.kind == PadKind::ThroughHole) {
            let (Some(root), Some(core)) = (nets.pad_islands.get(i), drill_core(p)) else { continue };
            let grow_by = (c.hole(0.0)) / 2.0;
            holes.push((
                pad_item(p),
                Core { points: core.points, radius: core.radius + grow_by },
                c.owner(*root).0,
                profile.drill_to_copper_pth.value,
            ));
        }
        if holes.is_empty() {
            continue;
        }
        let (copper, truncated) = copper_on(c, layer, &mut fill_edges);
        f.truncated |= truncated;
        let mut grid = Grid::new(CELL);
        for (i, piece) in copper.iter().enumerate() {
            grid.insert(i, core_bbox(&piece.core));
        }
        'holes: for (item, core, owner, least) in &holes {
            let near = grid.near_box(grow(core_bbox(core), *least), copper.len());
            if !c.visit(near.len()) {
                break 'holes;
            }
            for k in near {
                let Some(piece) = copper.get(k) else { continue };
                if piece.owner == *owner || piece.item == *item {
                    continue;
                }
                if !c.spend(1) {
                    break 'holes;
                }
                let g = gap(core, &piece.core);
                let pair = (item.clone(), piece.item.clone());
                // A fill's edge comes within its lattice and arcs of where it should be.
                let slack = if matches!(piece.item, DrcItem::Pour(_)) { FILL_SLACK } else { 0.0 };
                if g < least - slack - 1e-9 && worst.get(&pair).is_none_or(|w| g < w.0) {
                    worst.insert(pair, (g, *least, between(core, &piece.core), layer));
                }
            }
        }
    }
    for ((a, b), (g, least, at, layer)) in worst {
        let message = format!(
            "on {}, the hole of {} is {} mm from {}, which it doesn't join; the least is {} mm",
            layer.name(),
            a.reference(),
            mm(g.max(0.0)),
            b.reference(),
            mm(least)
        );
        let param = if matches!(a, DrcItem::Pad { .. }) { "profile.drill_to_copper_pth" } else { "profile.drill_to_copper" };
        f.add("drill_to_copper", &[a.key(), b.key()], || {
            Draft::new(at, vec![a, b], message).layer(Some(layer)).gap(g, least).param(param).fix(
                "ecad.board.move",
                Value::Null,
                format!("move the copper {} mm back from the hole", mm(step(least - g.max(0.0)))),
            )
        });
    }
}

/// The pads of each footprint by area: the largest, the next largest (a pad of another size),
/// and how many pads it has.
#[derive(Clone, Copy, Default)]
struct PadAreas {
    largest: f64,
    next: f64,
    count: usize,
}

/// Whether a surface-mount pad is a part's thermal pad: numbered so (`EP`, `PAD`, `TAB`), or the
/// one largest pad of a part with 8 or more, at least 1 mm² and half as large again as any other
/// (the library has no flag for it yet): a part whose pads are all one size has none.
fn thermal_pad(p: &BoardPad, areas: &BTreeMap<u64, PadAreas>) -> bool {
    let n = p.pad.number.to_ascii_uppercase();
    if ["EP", "EPAD", "PAD", "TAB", "THERMAL", "TH"].contains(&n.as_str()) || n.starts_with("EP") {
        return true;
    }
    let area = p.pad.area();
    areas.get(&p.footprint).is_some_and(|a| a.count >= 8 && area >= 1.0 && (area - a.largest).abs() < 1e-9 && a.largest >= 1.5 * a.next)
}

/// Vias whose land or hole is in a surface-mount pad of their own copper, unless filled and
/// capped (on a thermal pad, a small plugged or tented via is a warning). A laser micro via in a
/// pad is the usual HDI build and no error: one stacked on another micro via (their lands meet,
/// their spans touch) gets an info note while it isn't copper-filled, since a stack needs it.
fn vias_in_pads(c: &mut Ctx, f: &mut Found) {
    let board = c.board;
    let nets = c.nets;
    let n = board.copper_layers;
    let pads: Vec<(usize, &BoardPad)> = nets.pads.iter().enumerate().filter(|(_, p)| p.pad.kind == PadKind::Smd).collect();
    let mut largest: BTreeMap<u64, PadAreas> = BTreeMap::new();
    for p in &nets.pads {
        let e = largest.entry(p.footprint).or_default();
        let area = p.pad.area();
        e.count += 1;
        if area > e.largest + 1e-9 {
            (e.next, e.largest) = (e.largest, area);
        } else if area < e.largest - 1e-9 && area > e.next {
            e.next = area;
        }
    }
    let mut grid = Grid::new(CELL);
    for (k, (_, p)) in pads.iter().enumerate() {
        grid.insert(k, p.bbox());
    }
    // The micro vias, for stacks.
    let micro: Vec<usize> = (0..board.vias.len()).filter(|v| board.vias.get(*v).is_some_and(|x| x.kind(n, &board.rules) == ViaKind::Micro)).collect();
    let mut micro_grid = Grid::new(CELL);
    for (k, v) in micro.iter().enumerate() {
        if let Some(x) = board.vias.get(*v) {
            let r = x.diameter / 2.0;
            micro_grid.insert(k, [[x.at[0] - r, x.at[1] - r], [x.at[0] + r, x.at[1] + r]]);
        }
    }
    for (v, via) in board.vias.iter().enumerate() {
        if via.protection == ViaProtection::FilledCapped {
            continue;
        }
        let is_micro = via.kind(n, &board.rules) == ViaKind::Micro;
        if is_micro && via.protection == ViaProtection::Filled {
            continue;
        }
        let r = via.diameter / 2.0;
        let around = [[via.at[0] - r, via.at[1] - r], [via.at[0] + r, via.at[1] + r]];
        let near = grid.near_box(around, pads.len());
        if !c.visit(near.len()) {
            break;
        }
        let via_root = nets.via_islands.get(v).copied();
        let land = disc(via.at, r);
        for k in near {
            let Some((i, p)) = pads.get(k) else { continue };
            if !c.spend(1) {
                return;
            }
            let on = (p.top && via.on(CopperLayer::Top, n)) || (p.bottom && via.on(CopperLayer::Bottom, n));
            // Another net's pad is a short, found already.
            if !on || nets.pad_islands.get(*i).copied() != via_root {
                continue;
            }
            let g = gap(&land, &pad_core(p));
            if g >= -1e-9 {
                continue;
            }
            let (item, pad) = (DrcItem::Via(via.id), pad_item(p));
            let how = via.protection.name();
            if is_micro {
                // Only a stack wants anything of a micro via in a pad: copper fill.
                let (a0, a1) = via.span(n);
                let stacked = micro_grid.near_box(around, micro.len()).into_iter().filter_map(|m| micro.get(m).copied()).find(|w| {
                    let Some(o) = board.vias.get(*w).filter(|_| *w != v) else { return false };
                    let (b0, b1) = o.span(n);
                    (a1 == b0 || b1 == a0) && (o.at[0] - via.at[0]).hypot(o.at[1] - via.at[1]) < r + o.diameter / 2.0
                });
                let Some(other) = stacked.and_then(|w| board.vias.get(w)) else { break };
                let message = format!(
                    "micro via {} ({how}) in pad {} is stacked on micro via {}: stacked micro vias must be copper-filled",
                    via.id,
                    p.reference(),
                    other.id
                );
                let other_item = DrcItem::Via(other.id);
                f.add("via_in_pad", &[item.key(), pad.key(), other_item.key()], || {
                    Draft::new(via.at, vec![item, pad, other_item], message)
                        .layer(Some(if p.top { CopperLayer::Top } else { CopperLayer::Bottom }))
                        .protection(via.protection, ViaProtection::Filled)
                        .severity(Severity::Info)
                        .class(Class::Practical)
                        .arg("protection", how.to_string())
                        .arg("stacked", other.id.to_string())
                        .fix(
                            "ecad.board.via.edit",
                            json!({"vias": [via.id], "protection": "filled"}),
                            "fill it with copper (laser micro vias in a stack are filled)",
                        )
                });
                break;
            }
            let thermal = thermal_pad(p, &largest);
            let sealed = matches!(via.protection, ViaProtection::Plugged | ViaProtection::Tented | ViaProtection::Filled);
            let warn = thermal && sealed && via.drill <= THERMAL_VIA_DRILL + 1e-9;
            let message = if warn {
                format!(
                    "via {} ({how}) is in thermal pad {}: a small plugged via there is accepted; filled and capped is better",
                    via.id,
                    p.reference()
                )
            } else {
                format!("via {} ({how}) is in pad {}; a via in a pad must be filled and capped, or the solder wicks into it", via.id, p.reference())
            };
            f.add("via_in_pad", &[item.key(), pad.key()], || {
                let d = Draft::new(via.at, vec![item, pad], message)
                    .layer(Some(if p.top { CopperLayer::Top } else { CopperLayer::Bottom }))
                    .protection(via.protection, ViaProtection::FilledCapped)
                    .arg("protection", how.to_string())
                    .fix("ecad.board.via.edit", json!({"vias": [via.id], "protection": "filled_capped"}), "fill and cap it (or move it off the pad)");
                // A thermal pad takes a small plugged via, which costs less than filling and capping.
                let d = match (thermal, warn, via.drill <= THERMAL_VIA_DRILL + 1e-9) {
                    (true, false, true) => d.fix(
                        "ecad.board.via.edit",
                        json!({"vias": [via.id], "protection": "plugged"}),
                        "plug it: a small plugged via in a thermal pad is only a warning (TI SLUA271)",
                    ),
                    _ => d,
                };
                if warn {
                    d.severity(Severity::Warning).arg("thermal", "1".into()).note("a small plugged or tented via in a thermal pad (TI SLUA271)")
                } else {
                    d
                }
            });
        }
    }
}

/// Pads a pour joins that its copper doesn't join well: a pad joined solid that the fill doesn't
/// reach, a relieved pad none of whose spokes reach the fill, spokes narrower than the least track
/// (errors), fewer than 2 or half those asked for (warnings). Each fix is for the pad alone (its
/// footprint's `ecad.board.pour.connect`), so the pour's other pads keep their connection.
fn reliefs(c: &Ctx, f: &mut Found) {
    let least = c.board.rules.min_track_width;
    for fill in &c.nets.fills {
        let zitem = DrcItem::Pour(fill.pour);
        for r in fill.reliefs.iter().filter(|r| r.inside) {
            let Some(p) = c.nets.pads.get(r.pad) else { continue };
            let pad = pad_item(p);
            let keys = [zitem.key(), pad.key()];
            let at = p.at();
            let this_pad = |connect: &str| json!({"footprint": p.designator, "pads": [p.pad.number], "connect": connect});
            let leave_out = "or, joined by its own via or track, leave the pad out of the pour";
            let (connected, requested) = (r.connected.to_string(), r.requested.to_string());
            if r.solid {
                if r.connected == 0 {
                    let message = format!("pour {} joins pad {} solid, but its fill doesn't reach the pad", fill.pour, p.reference());
                    f.add("relief_starved", &keys, || {
                        Draft::new(at, vec![pad, zitem.clone()], message)
                            .layer(Some(fill.layer))
                            .count(0.0, 1.0, "joins", Bound::Min)
                            .arg("solid", "true".into())
                            .fix("ecad.board.move", Value::Null, "clear the copper that keeps the fill from it")
                            .fix("ecad.board.pour.connect", this_pad("none"), leave_out)
                    });
                }
                continue;
            }
            if r.connected == 0 {
                let message = format!("pour {} relieves pad {}, but none of its {} spokes reaches the fill", fill.pour, p.reference(), r.requested);
                f.add("relief_starved", &keys, || {
                    Draft::new(at, vec![pad, zitem.clone()], message)
                        .layer(Some(fill.layer))
                        .count(0.0, 1.0, "spokes", Bound::Min)
                        .arg("connected", connected.clone())
                        .arg("requested", requested.clone())
                        .fix("ecad.board.pour.connect", this_pad("solid"), "join this pad solid, or clear what blocks its spokes")
                        .fix("ecad.board.pour.connect", this_pad("none"), leave_out)
                });
            } else if r.spoke_width < least - 1e-9 {
                let message = format!(
                    "pour {}'s spokes to pad {} are {} mm wide; the least track is {} mm",
                    fill.pour,
                    p.reference(),
                    mm(r.spoke_width),
                    mm(least)
                );
                f.add("relief_starved", &keys, || {
                    Draft::new(at, vec![pad, zitem.clone()], message)
                        .layer(Some(fill.layer))
                        .measure(r.spoke_width, least)
                        .param("rules.min_track_width")
                        .fix(
                            "ecad.board.pour.edit",
                            json!({"pour": fill.pour, "thermal": {"spoke_width": least}}),
                            format!("make the spokes {} mm wide", mm(least)),
                        )
                });
            } else if (r.connected < 2 && r.requested >= 2) || u16::from(r.connected) * 2 < u16::from(r.requested) {
                let message = format!("pour {} joins pad {} through {} of its {} spokes", fill.pour, p.reference(), r.connected, r.requested);
                let want = f64::from(r.requested.min(2).max(r.requested.div_ceil(2)));
                f.add("relief_starved", &keys, || {
                    Draft::new(at, vec![pad, zitem.clone()], message)
                        .layer(Some(fill.layer))
                        .count(f64::from(r.connected), want, "spokes", Bound::Min)
                        .arg("connected", connected.clone())
                        .arg("requested", requested.clone())
                        .severity(Severity::Warning)
                        .fix("ecad.board.move", Value::Null, "clear the copper that cuts its spokes")
                        .fix("ecad.board.pour.connect", this_pad("solid"), "or join this pad solid")
                });
            }
        }
    }
}

/// A drilled hole: what it is, its core, and the layers it goes through (a via's span; `None` for
/// every layer).
struct Drill {
    item: DrcItem,
    core: Core,
    span: Option<(usize, usize)>,
    /// Unplated: an edge of the board for copper.
    unplated: bool,
}

fn drills(c: &Ctx) -> Vec<Drill> {
    let board = c.board;
    let mut out: Vec<Drill> = board
        .vias
        .iter()
        .map(|v| Drill { item: DrcItem::Via(v.id), core: disc(v.at, v.drill / 2.0), span: Some(v.span(board.copper_layers)), unplated: false })
        .collect();
    for p in &c.nets.pads {
        if let Some(core) = drill_core(p) {
            out.push(Drill { item: pad_item(p), core, span: None, unplated: p.pad.kind == PadKind::Hole });
        }
    }
    out.extend(board.holes.iter().map(|h| Drill { item: DrcItem::Hole(h.id), core: disc(h.at, h.drill / 2.0), span: None, unplated: true }));
    out
}

/// Drills too close together, and drills off the board.
fn holes(c: &mut Ctx, f: &mut Found) {
    let list = drills(c);
    let least = c.board.rules.hole_to_hole;
    let mut grid = Grid::new(CELL);
    for (i, d) in list.iter().enumerate() {
        grid.insert(i, core_bbox(&d.core));
    }
    'drills: for (i, a) in list.iter().enumerate() {
        let near = grid.near_box(grow(core_bbox(&a.core), least), list.len());
        if !c.visit(near.len()) {
            break 'drills;
        }
        for j in near {
            let Some(b) = list.get(j).filter(|_| j > i) else { continue };
            // Vias drilled through no dielectric in common (stacked micro vias share a copper
            // layer only) are not two holes in one.
            if let (Some((a0, a1)), Some((b0, b1))) = (a.span, b.span)
                && (a1 <= b0 || b1 <= a0)
            {
                continue;
            }
            if !c.spend(1) {
                break 'drills;
            }
            let g = gap(&a.core, &b.core);
            if g < least - 1e-9 {
                let message = format!(
                    "the drills of {} and {} are {} mm apart; the least is {} mm",
                    a.item.reference(),
                    b.item.reference(),
                    mm(g.max(0.0)),
                    mm(least)
                );
                let at = between(&a.core, &b.core);
                let says = format!("move one {} mm further from the other", mm(step(least - g.max(0.0))));
                f.add("hole_to_hole", &[a.item.key(), b.item.key()], || {
                    Draft::new(at, vec![a.item.clone(), b.item.clone()], message).gap(g, least).fix("ecad.board.move", Value::Null, says)
                });
            }
        }
    }
    for d in &list {
        let at = d.core.points.first().copied().unwrap_or_default();
        let edge = c.edge_gap(&d.core).map(|(g, _)| g);
        let crosses = edge.is_some_and(|g| g < 0.0);
        let off = !c.on_board(at);
        if off || crosses {
            // How far inside the edge it is: negative crossing it, more so outside.
            let inside = match (off, edge) {
                (true, Some(g)) => -g.abs() - d.core.radius * 2.0,
                (_, Some(g)) => g,
                (_, None) => 0.0,
            };
            let message = format!("the drill of {} is outside the board, or crosses its edge", d.item.reference());
            f.add("hole_outside", &[d.item.key()], || {
                Draft::new(at, vec![d.item.clone()], message).count(inside, 0.0, "mm", Bound::Min).fix(
                    "ecad.board.move",
                    Value::Null,
                    "move it onto the board",
                )
            });
        }
    }
}

/// Copper near the board's edge or an unplated hole, or off the board.
fn edges_and_outline(c: &mut Ctx, f: &mut Found) {
    let board = c.board;
    let least = board.rules.copper_to_edge;
    let unplated: Vec<Drill> = drills(c).into_iter().filter(|d| d.unplated).collect();
    let mut hole_grid = Grid::new(CELL);
    for (i, d) in unplated.iter().enumerate() {
        hole_grid.insert(i, core_bbox(&d.core));
    }
    let mut copper: Vec<(DrcItem, Option<CopperLayer>, Vec<Core>)> = Vec::new();
    for p in c.nets.pads.iter().filter(|p| p.top || p.bottom) {
        let layer = if p.top && p.bottom {
            None
        } else if p.top {
            Some(CopperLayer::Top)
        } else {
            Some(CopperLayer::Bottom)
        };
        copper.push((pad_item(p), layer, vec![pad_core(p)]));
    }
    for t in &board.tracks {
        copper.push((DrcItem::Track(t.id), Some(t.layer), track_cores(t).collect()));
    }
    for v in &board.vias {
        copper.push((DrcItem::Via(v.id), None, vec![disc(v.at, v.diameter / 2.0)]));
    }
    for g in &board.graphics {
        if let DrawLayer::Copper(l) = g.layer {
            copper.push((DrcItem::Graphic(g.id), Some(l), graphic_cores(&g.graphic, &|p| p)));
        }
    }
    for (item, layer, cores) in copper {
        let mut worst: Option<(f64, [f64; 2])> = None;
        // Where copper is off the board, and how far off: minus its gap to the edge.
        let mut outside: Option<([f64; 2], f64)> = None;
        for core in &cores {
            let edge = c.edge_gap(core);
            if let Some(first) = core.points.first()
                && !c.on_board(*first)
            {
                let off = -edge.map_or(0.0, |(g, _)| g.abs());
                if outside.is_none_or(|(_, o)| off < o) {
                    outside = Some((*first, off));
                }
            }
            if let Some((g, at)) = edge
                && g < least - 1e-9
                && worst.is_none_or(|(w, _)| g < w)
            {
                worst = Some((g, at));
            }
            let near = hole_grid.near_box(grow(core_bbox(core), least), unplated.len());
            if !c.visit(near.len()) {
                break;
            }
            for k in near {
                let Some(d) = unplated.get(k) else { continue };
                if d.item.id() == item.id() && matches!(item, DrcItem::Pad { .. }) {
                    continue;
                }
                if !c.spend(1) {
                    break;
                }
                let g = gap(core, &d.core);
                if g < least - 1e-9 && worst.is_none_or(|(w, _)| g < w) {
                    worst = Some((g, between(core, &d.core)));
                }
            }
        }
        let key = [item.key()];
        if let Some((at, off)) = outside {
            let message = format!("{} is outside the board", item.reference());
            f.add("copper_to_edge", &key, || {
                Draft::new(at, vec![item], message).layer(layer).gap(off, least).fix("ecad.board.move", Value::Null, "move it onto the board")
            });
        } else if let Some((g, at)) = worst {
            let message =
                format!("{} is {} mm from the board's edge or an unplated hole; the least is {} mm", item.reference(), mm(g.max(0.0)), mm(least));
            let says = format!("move it {} mm further in", mm(step(least - g.max(0.0))));
            f.add("copper_to_edge", &key, || {
                Draft::new(at, vec![item], message).layer(layer).gap(g, least).fix("ecad.board.move", Value::Null, says)
            });
        }
    }
}

/// A footprint's courtyard on the board, as a core (its box when it has none), and its side.
fn courtyard(c: &Ctx, fp: &super::board::BoardFootprint) -> Option<(Core, Side, DrcItem)> {
    let placed = c.ecad.placed(fp)?;
    let item = placed.resolved.footprint;
    let local = item.courtyard(Side::Top).or_else(|| footprint_box(item))?;
    let pl = fp.placement();
    let [[x0, y0], [x1, y1]] = local;
    let points = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(|p| pl.apply(p)).to_vec();
    Some((Core { points, radius: 0.0 }, fp.side, DrcItem::Footprint { id: fp.id, designator: placed.part.designator.clone() }))
}

/// Footprints off the board, and courtyards that overlap.
fn footprints(c: &mut Ctx, f: &mut Found) {
    let yards: Vec<(Core, Side, DrcItem)> = c.board.footprints.iter().filter_map(|fp| courtyard(c, fp)).collect();
    if !c.edges.is_empty() {
        for (core, _, item) in &yards {
            let off = core.points.iter().any(|p| !c.on_board(*p));
            let edge = c.edge_gap(core).map(|(g, _)| g);
            let crosses = edge.is_some_and(|g| g < -1e-6);
            if off || crosses {
                let at = core.points.first().copied().unwrap_or_default();
                let inside = match (crosses, edge) {
                    (true, Some(g)) => g,
                    (false, Some(g)) => -g.abs(),
                    (_, None) => 0.0,
                };
                let message = format!("{}'s courtyard reaches outside the board", item.reference());
                let name = item.reference();
                f.add("footprint_outside", &[item.key()], || {
                    Draft::new(at, vec![item.clone()], message).count(inside, 0.0, "mm", Bound::Min).fix(
                        "ecad.board.move",
                        json!({"items": [name]}),
                        "move it in from the edge (give `by` or `to`)",
                    )
                });
            }
        }
    }
    let mut grid = Grid::new(CELL);
    for (i, (core, _, _)) in yards.iter().enumerate() {
        grid.insert(i, core_bbox(core));
    }
    'yards: for (i, (a, side, ia)) in yards.iter().enumerate() {
        for j in grid.near_box(core_bbox(a), yards.len()) {
            let Some((b, other, ib)) = yards.get(j).filter(|_| j > i) else { continue };
            if side != other {
                continue;
            }
            if !c.spend(1) {
                break 'yards;
            }
            let g = gap(a, b);
            if g < -1e-6 {
                let at = between(a, b);
                let message = format!("the courtyards of {} and {} overlap", ia.reference(), ib.reference());
                let says = format!("move {} or {} {} mm apart", ia.reference(), ib.reference(), mm(step(-g)));
                f.add("courtyard_overlap", &[ia.key(), ib.key()], || {
                    Draft::new(at, vec![ia.clone(), ib.clone()], message).measure(g, 0.0).fix("ecad.board.move", Value::Null, says)
                });
            }
        }
    }
}

/// Whether `core` reaches into the closed polygon `ring` (whose box is `bbox`).
fn hits(core: &Core, ring: &[[f64; 2]], edges: &[([f64; 2], [f64; 2])]) -> bool {
    core.points.iter().any(|p| inside(*p, ring)) || edges.iter().any(|(a, b)| gap(core, &Core { points: vec![*a, *b], radius: 0.0 }) <= 0.0)
}

/// How deep `core` reaches into the closed polygon `ring`, in millimetres (0 when it only
/// touches its edge).
fn intrusion(core: &Core, ring: &[[f64; 2]], edges: &[([f64; 2], [f64; 2])]) -> f64 {
    let to_edge = |q: [f64; 2]| {
        edges
            .iter()
            .map(|(a, b)| gap(&Core { points: vec![q], radius: 0.0 }, &Core { points: vec![*a, *b], radius: 0.0 }))
            .fold(f64::INFINITY, f64::min)
    };
    let deepest =
        core.points.iter().filter(|q| inside(**q, ring)).map(|q| to_edge(*q)).fold(None, |m: Option<f64>, d| Some(m.map_or(d, |m| m.max(d))));
    match deepest {
        Some(d) if d.is_finite() => d + core.radius,
        _ => {
            let line = Core { points: core.points.clone(), radius: 0.0 };
            let d = edges.iter().map(|(a, b)| gap(&line, &Core { points: vec![*a, *b], radius: 0.0 })).fold(f64::INFINITY, f64::min);
            if d.is_finite() { (core.radius - d).max(0.0) } else { 0.0 }
        }
    }
}

/// What [`hits`] costs for `core`: each of its points walks the ring, and it meets each edge.
fn hits_cost(core: &Core, ring: &[[f64; 2]], edges: &[([f64; 2], [f64; 2])]) -> usize {
    core.points.len().saturating_mul(ring.len()).saturating_add(edges.len())
}

/// Tracks, vias and footprints in keepouts that forbid them.
fn keepouts(c: &mut Ctx, f: &mut Found) {
    let board = c.board;
    let layers = board.copper_layers;
    let yards: Vec<(Core, Side, DrcItem)> = board.footprints.iter().filter_map(|fp| courtyard(c, fp)).collect();
    'keepouts: for k in &board.keepouts {
        let ring = path::tessellate(&k.outline, &k.curves, true, TOLERANCE);
        let edges = pieces(&k.outline, &k.curves, true);
        let Some(bbox) = super::bbox(ring.iter().copied()) else { continue };
        let kitem = DrcItem::Keepout(k.id);
        let meets = |core: &Core| super::grid::boxes_meet(core_bbox(core), bbox);
        if k.forbid.tracks {
            for t in board.tracks.iter().filter(|t| k.on(t.layer)) {
                let mut hit = None;
                for core in track_cores(t).filter(|core| meets(core)) {
                    if !c.spend(hits_cost(&core, &ring, &edges)) {
                        break 'keepouts;
                    }
                    if hits(&core, &ring, &edges) {
                        hit = Some(core);
                        break;
                    }
                }
                let Some(core) = hit else { continue };
                let (item, at) = (DrcItem::Track(t.id), core.points.first().copied().unwrap_or_default());
                let message = format!("track {} is in keepout {}", t.id, k.id);
                let deep = intrusion(&core, &ring, &edges);
                f.add("keepout", &[item.key(), kitem.key()], || {
                    Draft::new(at, vec![item, kitem.clone()], message).layer(Some(t.layer)).count(-deep, 0.0, "mm", Bound::Min).fix(
                        "ecad.board.track.reshape",
                        Value::Null,
                        format!("route it round keepout {}, or change the keepout (ecad.board.keepout.edit)", k.id),
                    )
                });
            }
        }
        if k.forbid.vias {
            for v in board.vias.iter() {
                let on = board.stack().into_iter().any(|l| k.on(l) && v.on(l, layers));
                let core = disc(v.at, v.diameter / 2.0);
                if !(on && meets(&core)) {
                    continue;
                }
                if !c.spend(hits_cost(&core, &ring, &edges)) {
                    break 'keepouts;
                }
                if !hits(&core, &ring, &edges) {
                    continue;
                }
                let item = DrcItem::Via(v.id);
                let message = format!("via {} is in keepout {}", v.id, k.id);
                let deep = intrusion(&core, &ring, &edges);
                f.add("keepout", &[item.key(), kitem.key()], || {
                    Draft::new(v.at, vec![item, kitem.clone()], message).count(-deep, 0.0, "mm", Bound::Min).fix(
                        "ecad.board.move",
                        json!({"items": [v.id]}),
                        "move it out of the keepout (give `by` or `to`)",
                    )
                });
            }
        }
        if k.forbid.components {
            for (core, side, item) in &yards {
                if !(k.on((*side).into()) && meets(core)) {
                    continue;
                }
                if !c.spend(hits_cost(core, &ring, &edges)) {
                    break 'keepouts;
                }
                if !hits(core, &ring, &edges) {
                    continue;
                }
                let at = core.points.first().copied().unwrap_or_default();
                let message = format!("{} is in keepout {}", item.reference(), k.id);
                let deep = intrusion(core, &ring, &edges);
                let name = item.reference();
                f.add("keepout", &[item.key(), kitem.key()], || {
                    Draft::new(at, vec![item.clone(), kitem.clone()], message).layer(Some((*side).into())).count(-deep, 0.0, "mm", Bound::Min).fix(
                        "ecad.board.move",
                        json!({"items": [name]}),
                        "move it out of the keepout (give `by` or `to`)",
                    )
                });
            }
        }
    }
}

/// Silkscreen closer to pads than the rules allow: strokes, and the board's texts by their boxes
/// (the footprints' texts, their names and values among them, are not checked).
fn silkscreen(c: &mut Ctx, f: &mut Found) {
    let board = c.board;
    let least = board.rules.silk_to_pad;
    // Each stroke: who drew it and the side it is on.
    let mut strokes: Vec<(DrcItem, Side, Core)> = Vec::new();
    for fp in &board.footprints {
        let Some(placed) = c.ecad.placed(fp) else { continue };
        let pl = fp.placement();
        let item = DrcItem::Footprint { id: fp.id, designator: placed.part.designator.clone() };
        for side in [Side::Top, Side::Bottom] {
            let on = if pl.mirror { super::board::other(side) } else { side };
            for g in placed.resolved.footprint.layer(BoardLayer::Silkscreen, side).filter(|g| g.kind != GraphicType::Text) {
                for core in graphic_cores(g, &|p| pl.apply(p)) {
                    strokes.push((item.clone(), on, core));
                }
            }
        }
    }
    for g in &board.graphics {
        let side = match g.layer {
            DrawLayer::SilkscreenTop => Side::Top,
            DrawLayer::SilkscreenBottom => Side::Bottom,
            _ => continue,
        };
        for core in graphic_cores(&g.graphic, &|p| p) {
            strokes.push((DrcItem::Graphic(g.id), side, core));
        }
    }
    let pads: Vec<(&BoardPad, Core)> = c.nets.pads.iter().filter(|p| p.top || p.bottom).map(|p| (p, pad_core(p))).collect();
    let mut grid = Grid::new(CELL);
    for (i, (_, core)) in pads.iter().enumerate() {
        grid.insert(i, core_bbox(core));
    }
    let mut worst: BTreeMap<(DrcItem, usize), (f64, [f64; 2])> = BTreeMap::new();
    'strokes: for (item, side, core) in &strokes {
        let near = grid.near_box(grow(core_bbox(core), least), pads.len());
        if !c.visit(near.len()) {
            break 'strokes;
        }
        for k in near {
            let Some((p, pc)) = pads.get(k) else { continue };
            if !p.on_side(*side) {
                continue;
            }
            if !c.spend(1) {
                break 'strokes;
            }
            let g = gap(core, pc);
            if g < least - 1e-9 && worst.get(&(item.clone(), k)).is_none_or(|w| g < w.0) {
                worst.insert((item.clone(), k), (g, between(core, pc)));
            }
        }
    }
    for ((item, k), (g, at)) in worst {
        let Some((p, _)) = pads.get(k) else { continue };
        let pad = pad_item(p);
        let message =
            format!("silkscreen of {} is {} mm from pad {}; the least is {} mm", item.reference(), mm(g.max(0.0)), p.reference(), mm(least));
        let says = format!("move the silkscreen {} mm off the pad (the fab clips it otherwise)", mm(step(least - g.max(0.0))));
        f.add("silk_over_pad", &[item.key(), pad.key()], || {
            Draft::new(at, vec![item, pad], message).gap(g, least).fix("ecad.board.draw.edit", Value::Null, says)
        });
    }
}

/// One finding per airwire.
fn unrouted(c: &Ctx, f: &mut Found) {
    // What lies at each point an airwire may start or end at.
    let mut at: BTreeMap<[i64; 2], DrcItem> = BTreeMap::new();
    for p in &c.nets.pads {
        at.entry(lattice(p.at())).or_insert_with(|| pad_item(p));
    }
    for v in &c.board.vias {
        at.entry(lattice(v.at)).or_insert(DrcItem::Via(v.id));
    }
    for t in &c.board.tracks {
        for q in &t.points {
            at.entry(lattice(*q)).or_insert(DrcItem::Track(t.id));
        }
    }
    for a in &c.nets.airwires {
        let ends: Vec<DrcItem> = [a.from, a.to].iter().filter_map(|q| at.get(&lattice(*q)).cloned()).collect();
        let keys: Vec<String> = ends.iter().map(DrcItem::key).collect();
        let name = c.name(a.net);
        let mid = [(a.from[0] + a.to[0]) / 2.0, (a.from[1] + a.to[1]) / 2.0];
        let refs: Vec<String> = ends.iter().map(DrcItem::reference).collect();
        let message = format!("net {name}: {} still to route", refs.join(" to "));
        let length = (a.to[0] - a.from[0]).hypot(a.to[1] - a.from[1]);
        let says = format!("route {} mm from {:?} to {:?}", mm(length), [mm(a.from[0]), mm(a.from[1])], [mm(a.to[0]), mm(a.to[1])]);
        f.add("unrouted", &keys, || {
            Draft::new(mid, ends, message).nets(vec![name]).count(1.0, 0.0, "connections", Bound::Max).arg("length", mm(length)).fix(
                "ecad.board.track",
                Value::Null,
                says,
            )
        });
    }
}

/// Track ends on nothing, and vias that join one layer or none.
fn ends(c: &mut Ctx, f: &mut Found) {
    let board = c.board;
    let nets = c.nets;
    let layers = board.copper_layers;
    let mut pad_grid = Grid::new(CELL);
    for (i, p) in nets.pads.iter().enumerate() {
        pad_grid.insert(i, p.bbox());
    }
    let segments: Vec<(usize, [f64; 2], [f64; 2], f64)> = board
        .tracks
        .iter()
        .enumerate()
        .flat_map(|(t, track)| path::segments(&track.points, &track.curves, false).map(move |(a, b, s)| (t, a, b, s)))
        .collect();
    let mut seg_grid = Grid::new(CELL);
    for (i, (t, a, b, s)) in segments.iter().enumerate() {
        let r = board.tracks.get(*t).map_or(0.0, |x| x.width / 2.0);
        seg_grid.insert(i, grow(path::segment_bbox(*a, *b, *s), r));
    }
    let mut via_grid = Grid::new(CELL);
    for (i, v) in board.vias.iter().enumerate() {
        let r = v.diameter / 2.0;
        via_grid.insert(i, [[v.at[0] - r, v.at[1] - r], [v.at[0] + r, v.at[1] + r]]);
    }
    // Track ends inside a pour's fill on their layer.
    let mut in_fill: BTreeSet<(usize, usize)> = BTreeSet::new();
    for fill in &nets.fills {
        let mut asked = Vec::new();
        for (t, track) in board.tracks.iter().enumerate().filter(|(_, t)| t.layer == fill.layer) {
            for (k, q) in [track.points.first(), track.points.last()].into_iter().enumerate() {
                if let Some(q) = q {
                    asked.push(((t, k), *q));
                }
            }
        }
        let points: Vec<[f64; 2]> = asked.iter().map(|(_, q)| *q).collect();
        for ((key, _), hit) in asked.iter().zip(fill.region.locate(&points)) {
            if hit.is_some() {
                in_fill.insert(*key);
            }
        }
    }
    let dist = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    for (t, track) in board.tracks.iter().enumerate() {
        let last = track.points.len().saturating_sub(1);
        for (k, end) in [0, last].into_iter().enumerate() {
            let Some(q) = track.points.get(end).copied() else { continue };
            if !c.spend(8) {
                return;
            }
            let end_disc = disc(q, track.width / 2.0);
            let on_pad = pad_grid.near(q).any(|i| nets.pads.get(i).is_some_and(|p| p.on(track.layer) && gap(&pad_core(p), &end_disc) <= 0.0));
            let on_via = via_grid.near(q).any(|i| board.vias.get(i).is_some_and(|v| v.on(track.layer, layers) && dist(q, v.at) <= v.diameter / 2.0));
            let on_track = seg_grid.near(q).any(|i| {
                segments.get(i).is_some_and(|(o, a, b, s)| {
                    *o != t
                        && board
                            .tracks
                            .get(*o)
                            .is_some_and(|x| x.layer == track.layer && path::segment_distance(q, *a, *b, *s) <= x.width / 2.0 + track.width / 2.0)
                })
            });
            // The track's own other end or corner on it (a closed loop) counts too.
            let on_self =
                track.points.iter().enumerate().any(|(i, p)| i != end && dist(*p, q) <= track.width / 2.0 && (i + 1 != end && end + 1 != i));
            if on_pad || on_via || on_track || on_self || in_fill.contains(&(t, k)) {
                continue;
            }
            let item = DrcItem::TrackEnd(track.id, k as u8);
            let message = format!("{} touches nothing", item.reference());
            let id = track.id;
            f.add("dangling_track", &[item.key()], || {
                Draft::new(q, vec![item], message).layer(Some(track.layer)).count(0.0, 1.0, "connections", Bound::Min).fix(
                    "ecad.board.track.reshape",
                    Value::Null,
                    format!("route track {id} on to the copper it is for, or cut the stub off (ecad.board.track.split)"),
                )
            });
        }
    }
    // The layers a pour's fill joins each via on.
    let mut filled: BTreeMap<usize, BTreeSet<CopperLayer>> = BTreeMap::new();
    for fill in &nets.fills {
        for (_, n) in &fill.joins {
            if let super::fill::Node::Via(v) = n {
                filled.entry(*v).or_default().insert(fill.layer);
            }
        }
    }
    // Which layers' copper each via joins.
    for (v, via) in board.vias.iter().enumerate() {
        if !c.spend(8) {
            return;
        }
        let r = via.diameter / 2.0;
        let mut joined: BTreeSet<CopperLayer> = BTreeSet::new();
        for i in seg_grid.near(via.at) {
            let Some((t, a, b, s)) = segments.get(i) else { continue };
            let Some(track) = board.tracks.get(*t) else { continue };
            if via.on(track.layer, layers) && path::segment_distance(via.at, *a, *b, *s) <= track.width / 2.0 + r {
                joined.insert(track.layer);
            }
        }
        let around = [[via.at[0] - r, via.at[1] - r], [via.at[0] + r, via.at[1] + r]];
        for i in pad_grid.near_box(around, nets.pads.len()) {
            let Some(p) = nets.pads.get(i) else { continue };
            if gap(&pad_core(p), &disc(via.at, r)) <= 0.0 {
                for l in board.stack().into_iter().filter(|l| p.on(*l) && via.on(*l, layers)) {
                    joined.insert(l);
                }
            }
        }
        joined.extend(filled.get(&v).into_iter().flatten().copied());
        // Another via whose ring meets its ring (a stack of micro vias) joins it on the layers
        // both spans have.
        let (a0, a1) = via.span(layers);
        for w in via_grid.near_box(around, board.vias.len()) {
            let Some(o) = board.vias.get(w).filter(|o| w != v && dist(via.at, o.at) <= r + o.diameter / 2.0) else { continue };
            let (b0, b1) = o.span(layers);
            for i in a0.max(b0)..=a1.min(b1) {
                joined.extend(CopperLayer::at(i, layers));
            }
        }
        // A stitching via in its net's pour is only a note when it joins that one layer (the
        // plane it was to tie it to has gone, say), not a warning.
        let stitching = via.role == super::board::ViaRole::Stitch && filled.contains_key(&v);
        if joined.len() < 2 {
            let item = DrcItem::Via(via.id);
            let message = match joined.iter().next() {
                Some(l) => format!("via {} joins copper on {} only", via.id, l.name()),
                None => format!("via {} joins nothing", via.id),
            };
            let layer = joined.iter().next().copied();
            let n = joined.len() as f64;
            let id = via.id;
            f.add("via_stub", &[item.key()], || {
                let d = Draft::new(via.at, vec![item], message).layer(layer).count(n, 2.0, "layers", Bound::Min).fix(
                    "ecad.board.delete",
                    json!({"items": [id]}),
                    "delete it, or join copper to it on another layer",
                );
                if stitching { d.severity(Severity::Info) } else { d }
            });
        }
    }
}

/// Pours on a net the schematic doesn't have, and pours that fill nothing.
fn pours(c: &Ctx, f: &mut Found) {
    for p in &c.board.pours {
        let item = DrcItem::Pour(p.id);
        let at = p.outline.first().copied().unwrap_or_default();
        if let Some(name) = &p.net
            && !c.nets.names.iter().any(|n| n == name)
        {
            let message = format!("pour {} is on net {name}, which the schematic doesn't have", p.id);
            f.add("pour_unknown_net", &[item.key()], || {
                Draft::new(at, vec![item.clone()], message).layer(Some(p.layer)).nets(vec![name.clone()]).count(0.0, 1.0, "nets", Bound::Min).fix(
                    "ecad.board.pour.edit",
                    Value::Null,
                    "give it a net the schematic has (`net`)",
                )
            });
        }
        let fill = c.nets.fills.iter().find(|x| x.pour == p.id);
        if fill.is_some_and(|x| x.region.is_empty() && !x.truncated) {
            let message = format!("pour {} fills nothing", p.id);
            let id = p.id;
            f.add("pour_empty", &[item.key()], || {
                Draft::new(at, vec![item], message).layer(Some(p.layer)).count(0.0, 0.01, "mm²", Bound::Min).fix(
                    "ecad.board.delete",
                    json!({"items": [id]}),
                    "delete it, or widen its outline",
                )
            });
        }
    }
}

/// The fab profile: none chosen (`profile_missing`), and what the board needs of it
/// (`fab_class`): rules finer than it builds, micro vias it doesn't make, and the features right
/// at its minimums.
fn profile(c: &Ctx, f: &mut Found) {
    let board = c.board;
    if !board.has_content() {
        return;
    }
    let at = board.outline.first().copied().unwrap_or_default();
    let Some(choice) = &board.fab_profile else {
        let message =
            "no fab profile is chosen: rings are measured on the finished hole, and aspect ratios and inner-layer hole gaps go unchecked".to_string();
        f.add("profile_missing", &["board".to_string()], || {
            Draft::new(at, Vec::new(), message).count(0.0, 1.0, "profiles", Bound::Min).fix(
                "ecad.board.profile",
                json!({"fab": super::profiles::DEFAULT_PROFILE}),
                "pick the profile of the fab that will build it (ecad.board.profile.inspect lists them)",
            )
        });
        return;
    };
    let Some(p) = c.profile else { return };
    if choice.version != p.version {
        let message = format!(
            "the {} profile was picked at version {}; its figures are now version {}: pick it again to take them",
            p.name, choice.version, p.version
        );
        f.add("fab_class", &["version".to_string()], || {
            Draft::new(at, Vec::new(), message)
                .class(Class::Practical)
                .count(f64::from(choice.version), f64::from(p.version), "version", Bound::Min)
                .param("profile.version")
                .fix("ecad.board.profile", json!({"fab": p.id}), "pick it again")
        });
    }
    let rules = &board.rules;
    let finer = super::profiles::PROFILES.iter().find(|q| q.track.value < p.track.value - 1e-9).map_or(p.id, |q| q.id);
    // Rules finer than the profile builds.
    let mut asks: Vec<(String, f64, f64, &'static str)> = vec![
        ("rules.min_track_width".into(), rules.min_track_width, p.track.value, "track"),
        ("rules.clearance".into(), rules.clearance, p.gap.value, "gap"),
        ("rules.min_annular_ring".into(), rules.min_annular_ring, p.ring.value, "ring"),
        ("rules.min_drill".into(), rules.min_drill, p.drill.value, "drill"),
        ("rules.hole_to_hole".into(), rules.hole_to_hole, p.hole_to_hole.value, "hole_to_hole"),
        ("rules.copper_to_edge".into(), rules.copper_to_edge, p.copper_to_edge.value, "copper_to_edge"),
    ];
    for k in &rules.net_classes {
        asks.push((format!("class.{}.track_width", k.name), k.track_width, p.track.value, "track"));
        asks.push((format!("class.{}.clearance", k.name), k.clearance, p.gap.value, "gap"));
        if let Some(d) = k.diff_pair {
            asks.push((format!("class.{}.diff_pair.width", k.name), d.width, p.track.value, "track"));
            asks.push((format!("class.{}.diff_pair.gap", k.name), d.gap, p.gap.value, "gap"));
        }
    }
    for (what, value, least, figure) in asks {
        if value >= least - 1e-9 {
            continue;
        }
        let message = format!("the board asks {} mm for `{what}`; the {} profile builds {} mm at the least", mm(value), p.name, mm(least));
        // The fab can't build it: as critical as the findings the rule would otherwise give.
        f.add("fab_class", std::slice::from_ref(&what), || {
            Draft::new(at, Vec::new(), message)
                .measure(value, least)
                .param(format!("profile.{figure}"))
                .severity(Severity::Warning)
                .class(Class::Critical)
                .fix(
                    "ecad.board.profile",
                    json!({"fab": finer}),
                    "pick a profile that builds it, or set the rule back to the profile's figure (ecad.board.rules)",
                )
        });
    }
    let n = board.copper_layers;
    let micro: Vec<u64> = board.vias.iter().filter(|v| v.kind(n, rules) == ViaKind::Micro).map(|v| v.id).collect();
    if !micro.is_empty() && p.micro.is_none() {
        let message = format!("{} micro vias are on the board, and the {} profile makes none", micro.len(), p.name);
        let count = micro.len() as f64;
        let hdi = super::profiles::PROFILES.iter().find(|q| q.micro.is_some()).map_or(p.id, |q| q.id);
        f.add("fab_class", &["microvia".to_string()], || {
            Draft::new(at, Vec::new(), message)
                .count(count, 0.0, "vias", Bound::Max)
                .param("profile.microvia_drill")
                .severity(Severity::Warning)
                .class(Class::Critical)
                .fix("ecad.board.profile", json!({"fab": hdi}), "pick a profile with laser micro vias, or make them through vias")
        });
    }
    // The features right at the profile's minimums.
    let near = |v: f64, least: f64| v >= least - 1e-6 && v < least + AT_MINIMUM;
    let mut at_min: Vec<(&'static str, usize, Option<f64>, f64)> = Vec::new();
    let widths: Vec<f64> = board.tracks.iter().map(|t| t.width).collect();
    at_min.push(("track", widths.iter().filter(|w| near(**w, p.track.value)).count(), widths.iter().copied().reduce(f64::min), p.track.value));
    at_min.push(("gap", c.gap_at_minimum.len(), c.smallest_gap, p.gap.value));
    let mut rings: Vec<f64> =
        board.vias.iter().map(|v| (v.diameter - if v.kind(n, rules) == ViaKind::Micro { v.drill } else { c.hole(v.drill) }) / 2.0).collect();
    let mut drills: Vec<f64> = board.vias.iter().filter(|v| v.kind(n, rules) != ViaKind::Micro).map(|v| v.drill).collect();
    for q in c.nets.pads.iter().filter(|q| q.pad.kind == PadKind::ThroughHole) {
        if let Some([dw, dh]) = q.pad.drill {
            rings.push(((q.pad.size[0] - c.hole(dw)) / 2.0).min((q.pad.size[1] - c.hole(dh)) / 2.0));
            drills.push(dw.min(dh));
        }
    }
    at_min.push(("ring", rings.iter().filter(|r| near(**r, p.ring.value)).count(), rings.iter().copied().reduce(f64::min), p.ring.value));
    at_min.push(("drill", drills.iter().filter(|d| near(**d, p.drill.value)).count(), drills.iter().copied().reduce(f64::min), p.drill.value));
    for (figure, count, smallest, least) in at_min {
        if count == 0 {
            continue;
        }
        let message = format!(
            "{count} features are at the {} profile's least {figure} ({} mm): a design one step looser leaves room for the process",
            p.name,
            mm(least)
        );
        let key = format!("at_minimum.{figure}");
        let smallest = smallest.unwrap_or(least);
        let (command, says) = match figure {
            "track" => ("ecad.board.track.edit", "widen the tracks where there is room (ecad.board.rules raises what new tracks get)"),
            "gap" => ("ecad.board.move", "move copper apart where there is room (ecad.board.rules raises the clearance new copper keeps)"),
            _ => (
                "ecad.board.via.edit",
                "make the vias (or the footprints' pads) larger where there is room (ecad.board.rules raises what new vias get)",
            ),
        };
        f.add("fab_class", &[key], || {
            let d = Draft::new(at, Vec::new(), message)
                .class(Class::Practical)
                .measure(smallest, least)
                .param(format!("profile.{figure}"))
                .arg("count", count.to_string())
                .fix(command, Value::Null, says);
            if count > MAX_AT_MINIMUM { d.severity(Severity::Warning) } else { d }
        });
    }
}

/// The most pair tests one [`clashes`] call makes (the Route tool runs one each frame): pairs
/// whose boxes meet, the ones measured.
pub const MAX_CLASH_TESTS: usize = 1 << 18;
/// The most box tests one [`clashes`] call makes, to find the pairs to measure.
pub const MAX_CLASH_BOXES: usize = 1 << 22;
/// The most clashes one [`clashes`] call returns.
pub const MAX_CLASHES: usize = 64;

/// Copper that new copper would come closer to than the clearance (see [`clashes`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Clash {
    /// The new piece, by its index in the pieces given.
    pub piece: usize,
    pub layer: CopperLayer,
    /// The other copper, and its net (an index into the connections' names).
    pub item: DrcItem,
    pub net: Option<usize>,
    /// How far apart they are (0 where they touch or overlap) and the clearance between them, in
    /// millimetres.
    pub gap: f64,
    pub required: f64,
    /// Where they come closest.
    pub at: [f64; 2],
}

/// What new copper of the net `net` (an index into `nets`' names; `None` for copper on no net
/// yet) would come closer to than the clearance, as the check measures it: each of `pieces` on
/// its layer against the pads, tracks and vias of other nets there (a via is one disc per layer of
/// its span, given by the caller). Copper on no net counts as another net's, unless the new copper
/// is on none either. Pours are left out, as they clear round new copper, and so are copper
/// drawings. Closest first, one per piece and other object, at most [`MAX_CLASHES`]. The last
/// pieces (the newest copper of a route) are looked at first; past [`MAX_CLASH_TESTS`] tests (or
/// [`MAX_CLASH_BOXES`] box tests) the rest is not, and `true` says so.
pub fn clashes(board: &Board, nets: &BoardNets, net: Option<usize>, pieces: &[(CopperLayer, Core)]) -> (Vec<Clash>, bool) {
    let sizes = board.rules.sizes_by_net(&nets.names);
    let other = |n: Option<usize>| n != net && (n.is_some() || net.is_some());
    let max_gap = sizes.iter().map(|s| s.clearance).fold(board.rules.clearance, f64::max);
    let n = board.copper_layers;
    let mut layers: Vec<CopperLayer> = pieces.iter().map(|(l, _)| *l).collect();
    layers.sort_by_key(|l| l.index(n));
    layers.dedup();
    let mut out: Vec<Clash> = Vec::new();
    let (mut budget, mut boxes) = (MAX_CLASH_TESTS, MAX_CLASH_BOXES);
    let mut truncated = false;
    for layer in layers {
        let mine: Vec<(usize, &Core, [[f64; 2]; 2])> =
            pieces.iter().enumerate().filter(|(_, (l, _))| *l == layer).map(|(k, (_, c))| (k, c, grow(core_bbox(c), max_gap))).collect();
        let Some(area) =
            mine.iter().map(|(_, _, b)| *b).reduce(|[a0, a1], [b0, b1]| [[a0[0].min(b0[0]), a0[1].min(b0[1])], [a1[0].max(b1[0]), a1[1].max(b1[1])]])
        else {
            continue;
        };
        // The other nets' copper near the new pieces on this layer.
        let mut copper: Vec<(DrcItem, Option<usize>, Core)> = Vec::new();
        for p in nets.pads.iter().filter(|p| p.on(layer) && other(p.net) && boxes_meet(area, p.bbox())) {
            copper.push((pad_item(p), p.net, pad_core(p)));
        }
        for (t, tn) in board.tracks.iter().zip(&nets.track_nets).filter(|(t, tn)| t.layer == layer && other(**tn)) {
            if path::bbox(&t.points, &t.curves, false).is_some_and(|b| boxes_meet(area, grow(b, t.width / 2.0))) {
                copper.extend(track_cores(t).map(|c| (DrcItem::Track(t.id), *tn, c)));
            }
        }
        for (v, vn) in board.vias.iter().zip(&nets.via_nets).filter(|(v, vn)| v.on(layer, n) && other(**vn)) {
            let core = disc(v.at, v.diameter / 2.0);
            if boxes_meet(area, core_bbox(&core)) {
                copper.push((DrcItem::Via(v.id), *vn, core));
            }
        }
        let mut grid = Grid::new(CELL);
        for (i, (_, _, core)) in copper.iter().enumerate() {
            grid.insert(i, core_bbox(core));
        }
        'pieces: for (k, piece, reach) in mine.iter().rev() {
            let near = grid.near_box(*reach, copper.len());
            if near.len() > boxes {
                truncated = true;
                break;
            }
            boxes -= near.len();
            for (item, other_net, core) in near.into_iter().filter_map(|i| copper.get(i)) {
                if out.len() >= MAX_CLASHES {
                    break 'pieces;
                }
                if !boxes_meet(*reach, core_bbox(core)) {
                    continue;
                }
                if budget == 0 {
                    truncated = true;
                    break 'pieces;
                }
                budget -= 1;
                let required = clearance_between(&board.rules, &sizes, net, *other_net);
                let g = gap(piece, core);
                if g >= required - 1e-6 {
                    continue;
                }
                let found = Clash { piece: *k, layer, item: item.clone(), net: *other_net, gap: g.max(0.0), required, at: between(piece, core) };
                match out.iter_mut().find(|c| c.piece == *k && c.item == *item) {
                    Some(c) if found.gap < c.gap => *c = found,
                    Some(_) => {}
                    None => out.push(found),
                }
            }
        }
    }
    out.sort_by(|a, b| a.gap.total_cmp(&b.gap));
    (out, truncated)
}

/// The copper of the board objects `ids` (a footprint by its pads) on each layer it is on,
/// grouped by net, each piece with the object it is of (`C60.1`, `via 7`, `track 5`).
fn copper_of(board: &Board, nets: &BoardNets, ids: &BTreeSet<u64>) -> BTreeMap<Option<usize>, (Vec<(CopperLayer, Core)>, Vec<DrcItem>)> {
    let mut out: BTreeMap<Option<usize>, (Vec<(CopperLayer, Core)>, Vec<DrcItem>)> = BTreeMap::new();
    let n = board.copper_layers;
    for p in nets.pads.iter().filter(|p| ids.contains(&p.footprint)) {
        let core = pad_core(p);
        for layer in board.stack().into_iter().filter(|l| p.on(*l)) {
            let slot = out.entry(p.net).or_default();
            slot.0.push((layer, core.clone()));
            slot.1.push(pad_item(p));
        }
    }
    for (v, vn) in board.vias.iter().zip(&nets.via_nets).filter(|(v, _)| ids.contains(&v.id)) {
        for layer in board.stack().into_iter().filter(|l| v.on(*l, n)) {
            let slot = out.entry(*vn).or_default();
            slot.0.push((layer, disc(v.at, v.diameter / 2.0)));
            slot.1.push(DrcItem::Via(v.id));
        }
    }
    for (t, tn) in board.tracks.iter().zip(&nets.track_nets).filter(|(t, _)| ids.contains(&t.id)) {
        let slot = out.entry(*tn).or_default();
        for core in track_cores(t) {
            slot.0.push((t.layer, core));
            slot.1.push(DrcItem::Track(t.id));
        }
    }
    out
}

/// What the moved copper came too close to before a move (see [`moved_clashes`]), for the
/// objects it was worked out for: a drag works it out once and keeps it while the same objects
/// move.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MovedBefore {
    ids: BTreeSet<u64>,
    was: BTreeSet<(DrcItem, DrcItem)>,
}

/// A clash a move brought (see [`moved_clashes`]): the moved copper (`C60.1`, `track 5`) and its
/// net (an index into the after board's names), and what it came too close to.
#[derive(Clone, Debug, PartialEq)]
pub struct MovedClash {
    pub moved: DrcItem,
    pub net: Option<usize>,
    pub clash: Clash,
}

/// What a move, turn or flip brought too close: the clashes (see [`clashes`]) the board objects
/// `moved` have on `after`, and the tracks whose points the move changed (their ends followed),
/// that the same copper did not have with the same object on `before`. A footprint's own pads,
/// close as they may be, moved together and are left out so. Tracks and vias are measured with
/// the nets they had on `before`: where the move joins two nets, the island takes the lowest, and
/// a track would otherwise be measured as of the other net, against its own net's copper. Closest
/// first; `true` when a check ran out of tests.
pub fn moved_clashes(
    before: &Board,
    before_nets: &BoardNets,
    after: &Board,
    after_nets: &BoardNets,
    moved: &BTreeSet<u64>,
) -> (Vec<MovedClash>, bool) {
    moved_clashes_kept(before, before_nets, after, after_nets, moved, &mut None)
}

/// `after_nets` with each track and via that `before` had on a net back on that net (by name),
/// where the two differ; `None` when none does.
fn nets_kept(before: &Board, before_nets: &BoardNets, after: &Board, after_nets: &BoardNets) -> Option<BoardNets> {
    let same_names = before_nets.names == after_nets.names;
    let index = |n: Option<usize>| -> Option<usize> {
        let n = n?;
        if same_names {
            return Some(n);
        }
        let name = before_nets.name(n)?;
        after_nets.names.iter().position(|m| m == name)
    };
    let tracks: BTreeMap<u64, Option<usize>> = before.tracks.iter().zip(&before_nets.track_nets).map(|(t, n)| (t.id, index(*n))).collect();
    let vias: BTreeMap<u64, Option<usize>> = before.vias.iter().zip(&before_nets.via_nets).map(|(v, n)| (v.id, index(*n))).collect();
    let differs = |id: u64, now: Option<usize>, was: &BTreeMap<u64, Option<usize>>| was.get(&id).copied().flatten().filter(|w| Some(*w) != now);
    let changed = after.tracks.iter().zip(&after_nets.track_nets).any(|(t, n)| differs(t.id, *n, &tracks).is_some())
        || after.vias.iter().zip(&after_nets.via_nets).any(|(v, n)| differs(v.id, *n, &vias).is_some());
    if !changed {
        return None;
    }
    let mut out = after_nets.clone();
    for (t, slot) in after.tracks.iter().zip(out.track_nets.iter_mut()) {
        if let Some(was) = differs(t.id, *slot, &tracks) {
            *slot = Some(was);
        }
    }
    for (v, slot) in after.vias.iter().zip(out.via_nets.iter_mut()) {
        if let Some(was) = differs(v.id, *slot, &vias) {
            *slot = Some(was);
        }
    }
    Some(out)
}

/// [`moved_clashes`], with what `before` had kept in `kept` (worked out again when the objects
/// differ from those it was worked out for).
pub fn moved_clashes_kept(
    before: &Board,
    before_nets: &BoardNets,
    after: &Board,
    after_nets: &BoardNets,
    moved: &BTreeSet<u64>,
    kept: &mut Option<MovedBefore>,
) -> (Vec<MovedClash>, bool) {
    let mut ids = moved.clone();
    for t in &after.tracks {
        if before.track(t.id).is_some_and(|old| old.points != t.points || old.layer != t.layer) {
            ids.insert(t.id);
        }
    }
    let mut truncated = false;
    let found = |board: &Board, nets: &BoardNets, truncated: &mut bool| {
        let mut list: Vec<MovedClash> = Vec::new();
        for (net, (pieces, names)) in copper_of(board, nets, &ids) {
            let (clashes, short) = clashes(board, nets, net, &pieces);
            *truncated |= short;
            list.extend(clashes.into_iter().filter_map(|clash| Some(MovedClash { moved: names.get(clash.piece)?.clone(), net, clash })));
        }
        list
    };
    if kept.as_ref().is_none_or(|k| k.ids != ids) {
        let was = found(before, before_nets, &mut truncated).into_iter().map(|m| (m.moved, m.clash.item)).collect();
        *kept = Some(MovedBefore { ids: ids.clone(), was });
    }
    let was = kept.as_ref().map(|k| &k.was);
    let kept_nets = nets_kept(before, before_nets, after, after_nets);
    let mut now: Vec<MovedClash> = found(after, kept_nets.as_ref().unwrap_or(after_nets), &mut truncated)
        .into_iter()
        .filter(|m| !was.is_some_and(|w| w.contains(&(m.moved.clone(), m.clash.item.clone()))))
        .collect();
    now.sort_by(|a, b| a.clash.gap.total_cmp(&b.clash.gap));
    // One per moved object and other object, the closest (a track's segments and a pad's or
    // via's layers are pieces of their own, in any order).
    let mut seen: BTreeSet<(DrcItem, DrcItem)> = BTreeSet::new();
    now.retain(|m| seen.insert((m.moved.clone(), m.clash.item.clone())));
    (now, truncated)
}
