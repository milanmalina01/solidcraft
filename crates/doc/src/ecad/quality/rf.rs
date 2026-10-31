//! RF lines and antennas: the nets kept with the role rf (their records' `z_target`, else
//! `rf.line.target`) and the parts kept as antennas.
//! - `rf.line`: a chain of an RF net's tracks (joined end to end on one layer; a pad, a via or a
//!   branch ends it) longer than `fraction` of λg at the net's frequency: each track's impedance
//!   against the target (the net's `z_tol` is the error limit when it gives one), off by `warn` a
//!   warning, by `error` an error. It is worked out as a microstrip over the plane the track
//!   references, or as a stripline between two planes. With ground beside it on both sides within
//!   max(2W, 3h) (the gap measured) it is read both as a coplanar line with ground under it and as
//!   a microstrip that ground loads (the microstrip's impedance scaled by the coplanar model's drop
//!   from the gap max(2W, 3h) to the one measured, so the formulas' own disagreement is no
//!   finding), and the reading nearer the target counts. A track narrower than `min_width` on FR-4
//!   is a warning. A track whose width suits a microstrip and is off only because ground beside it
//!   loads it is `rf.coplanar_gap`'s.
//! - `rf.reference`: under RF tracks and pads, down to the plane they reference (the first layer
//!   whose planes cover `coverage` of them): another net's copper (an error), and a power plane
//!   as that reference (an error). Voids are no error here: a void under a pad or a matching
//!   part may be on purpose, and a void a track crosses is `pi.return_path`'s.
//! - `rf.antenna_keepout`: the antennas' keepouts. The library has no keepout data yet, so each
//!   antenna is an info to review (its board keepouts against its maker's drawing), and so is a
//!   module on a radio board with no pin on an RF net (its antenna may be on it); copper inside
//!   a board keepout over an antenna that the design rule check's `keepout` doesn't report (other
//!   parts' pads where the keepout lets parts in) is an error.
//! - `rf.coplanar_gap`: the copper right beside an RF track on its layer, within max(`widths`·W,
//!   `heights`·h): another net's (not ground, not the RF line's own; another RF net's too, the
//!   other side of a series part coupling past it) is an error; ground that loads a line whose
//!   width suits a microstrip (on a chain `rf.line` looks at), off its target by `warn` once
//!   loaded: a warning, an error past `error`.
//! - `rf.fence`: an RF track with ground beside it within `side`·h: the ground vias along each
//!   side (within `row` past the gap) no farther apart than `fraction` of λg at the frequency of
//!   concern (the intent's, else `harmonic` × the net's), each from the next as the crow flies
//!   (and the first and last from where the ground beside it starts and ends, along it), and one
//!   within `corner` of each bend and end of it (of a pad's edge, for one on a pad; an end
//!   that carries straight on into the next track of the line is none). Bends and ends inside a
//!   part of 9 pads or more are its maker's. A vendor note's 1.0–1.3 mm is an unverified
//!   reference only.
//!
//! An RF net's frequency is its record's `f_mhz`, else the top of the board's radio band; with
//! several bands it is the highest, assumed, and its findings are warnings at most until the net
//! says. λg = c/(f·√εr) with εr of the dielectric to its reference, a conservative figure.
//! Copper inside a keepout over an antenna (its zone) is the antenna's and isn't looked at, nor is
//! the copper beside a track inside the courtyard of a part it joins (the pinch round a pad).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::super::board::BoardFootprint;
use super::super::checks::{Severity, gap};
use super::super::copper::{disc, inside, pad_core, stroke};
use super::super::drc::{Bound, Class, DrcItem};
use super::super::footprint::{Core, PadKind};
use super::super::grid::{Grid, boxes_meet, grow};
use super::super::impedance::{self, LineKind, Medium};
use super::super::intent::{NetRole, PartRole};
use super::super::layers::CopperLayer;
use super::super::path;
use super::super::stackup::{DEFAULT_ER, surroundings};
use super::catalog::{Code, Family, Param};
use super::geom::{along, distance, track_pieces};
use super::roles::NetInfo;
use super::xtal::{Layered, Piece, box_core, copper_on, footprints, net_name, pad_item};
use super::{Cx, Draft, num};

const LINE: &str = "rf.line";
const REFERENCE: &str = "rf.reference";
const ANTENNA: &str = "rf.antenna_keepout";
const GAP: &str = "rf.coplanar_gap";
const FENCE: &str = "rf.fence";

/// The speed of light, in millimetres times megahertz.
const C_MM_MHZ: f64 = 299_792.458;
/// A dielectric of εr up to this is FR-4-like, for the width rule.
const FR4_ER: f64 = 5.5;
/// How far apart points along an RF track are looked at, at least, in millimetres.
const STEP: f64 = 0.1;
/// The most points one track is looked at.
const MAX_POINTS: usize = 2000;
/// How much larger than a via's or pad's copper and its plane's clearance a hole round it may be
/// and still be its antipad (as `pi.return_path`'s default).
const ANTIPAD_SLACK: f64 = 0.05;
/// How many halvings find where a ray from an RF track's edge first meets a piece of copper.
const RAY_STEPS: usize = 12;
/// How many steps out each side of a point is probed for a pour's fill.
const PROBES: usize = 48;
/// A turn sharper than this, in degrees, is a bend the fence needs a via at.
const BEND_DEG: f64 = 10.0;

const fn param(name: &'static str, default: f64, units: &'static str, min: f64, max: f64, source: &'static str, about: &'static str) -> Param {
    Param { name, default, units, source, min, max, verify: false, about }
}

const CODES: &[Code] = &[
    Code {
        code: LINE,
        class: Class::Critical,
        severity: Severity::Error,
        worst: None,
        needs: "intent: the RF nets (role rf, z_target), their f_mhz or the board's radio bands; geometry: the stack-up and a plane under the line",
        section: "rf-emc#rf-lines",
        about: "an RF track on a chain longer than λg/16 whose impedance (microstrip, coplanar with ground, or stripline) is off its target: a warning past warn, an error past error; narrower than min_width on FR-4 (a warning)",
        principle: "An RF line longer than a small part of a wavelength is a transmission line: off its impedance it reflects power and detunes the match.",
        params: &[
            param(
                "fraction",
                0.0625,
                "λg",
                0.01,
                1.0,
                "Silicon Labs Z-Wave 800 guide: λg/16",
                "chains longer than this share of λg at the net's frequency are lines",
            ),
            param("warn", 0.05, "ratio", 0.0, 1.0, "judgment", "an impedance off its target by more than this share is a warning"),
            param("error", 0.1, "ratio", 0.0, 1.0, "judgment", "an impedance off its target by more than this share is an error"),
            param("min_width", 0.2, "mm", 0.0, 10.0, "u-blox", "RF tracks on FR-4 narrower than this vary too much with the etch"),
            param("target", 50.0, "Ω", 1.0, 500.0, "judgment: the usual RF impedance", "the target of an RF net whose record gives no z_target"),
        ],
    },
    Code {
        code: REFERENCE,
        class: Class::Critical,
        severity: Severity::Error,
        worst: None,
        needs: "intent: the RF nets (role rf), the ground net; geometry: planes (pours) under the RF copper",
        section: "rf-emc#rf-lines",
        about: "another net's copper under an RF track or pad, between it and its reference plane or in it; a power plane as an RF line's reference",
        principle: "An RF line's field and return current fill the layers right under it: another net's copper there couples into the line and a power plane is a noisy, poorly decoupled reference.",
        params: &[
            param("coverage", 0.5, "ratio", 0.05, 1.0, "judgment", "the share of an RF track or pad a layer's planes must cover to be its reference"),
            param(
                "min_length",
                0.5,
                "mm",
                0.0,
                50.0,
                "judgment: shorter than λg/50 at a few GHz, where a via transition (rf.vias) is what matters",
                "a power plane under less of an RF track than this (off its own antipads) is an info, not an error",
            ),
        ],
    },
    Code {
        code: ANTENNA,
        class: Class::Critical,
        severity: Severity::Error,
        worst: None,
        needs: "library: the antenna's keepout polygons (none yet: the board's keepouts over it stand in); intent: the antenna part (role antenna)",
        section: "rf-emc#antennas",
        about: "a pad of another part inside a board keepout over an antenna that keeps copper out (an error); every antenna, and every module with no RF pin on a radio board, until its keepout comes from its library, an info to check its keepouts against its maker's drawing",
        principle: "An antenna radiates through the clearance its maker drew: copper or parts inside it detune it and soak up what it sends.",
        params: &[],
    },
    Code {
        code: GAP,
        class: Class::Critical,
        severity: Severity::Error,
        worst: None,
        needs: "intent: the RF nets (role rf, z_target), the ground net; geometry: the stack-up and a plane under the line",
        section: "rf-emc#rf-lines",
        about: "another net's copper as the nearest copper beside an RF track (an error); ground beside a line made for a microstrip that turns it into a coplanar line off its target (a warning, an error past error)",
        principle: "Copper beside an RF line within a few dielectric heights becomes part of the line: ground there must be a designed coplanar gap, and another net there couples straight in.",
        params: &[
            param(
                "heights",
                3.0,
                "h",
                0.0,
                20.0,
                "u-blox: max(2W, 3h)",
                "copper beside a line within this many dielectric heights of its edge loads it",
            ),
            param("widths", 2.0, "W", 0.0, 20.0, "u-blox: max(2W, 3h)", "copper beside a line within this many of its widths of its edge loads it"),
            param("warn", 0.05, "ratio", 0.0, 1.0, "judgment", "a coplanar impedance off its target by more than this share is a warning"),
            param("error", 0.1, "ratio", 0.0, 1.0, "judgment", "a coplanar impedance off its target by more than this share is an error"),
        ],
    },
    Code {
        code: FENCE,
        class: Class::Practical,
        severity: Severity::Warning,
        worst: None,
        needs: "intent: the RF nets (role rf), the ground net, the frequency of concern (the board's f_concern_mhz, the net's f_mhz or the radio bands)",
        section: "rf-emc#fences",
        about: "ground beside an RF line with its vias farther apart than λg/20 at the frequency of concern, or no ground via within corner of a bend, end or pad of the line",
        principle: "Ground beside an RF line is only ground where vias tie it to the plane: between vias farther apart than a small part of a wavelength it resonates and radiates.",
        params: &[
            param(
                "fraction",
                0.05,
                "λg",
                0.01,
                1.0,
                "λg/20 (a vendor note's 1.0–1.3 mm is unverified)",
                "the most a fence's vias may be apart, as a share of λg at the frequency of concern",
            ),
            param(
                "harmonic",
                2.0,
                "×",
                1.0,
                10.0,
                "judgment: the second harmonic",
                "the frequency of concern, as a multiple of the net's, when the board declares none",
            ),
            param("corner", 1.0, "mm", 0.0, 20.0, "judgment", "a ground via this close to each bend, end and pad of the line"),
            param("side", 3.0, "h", 0.5, 20.0, "judgment", "ground beside the line within this many dielectric heights is its coplanar side"),
            param("row", 1.0, "mm", 0.0, 20.0, "judgment", "vias this far past the gap's edge are the fence's"),
        ],
    },
];

pub(super) const FAMILY: Family = Family { module: "rf", title: "RF lines and antennas", codes: CODES, run };

/// An antenna on the board and the keepouts over it (by index into the board's keepouts).
struct Antenna<'a> {
    designator: &'a str,
    footprint: &'a BoardFootprint,
    keepouts: Vec<usize>,
}

/// What lies first beside an RF track at a point, on one side, and how far from its edge.
#[derive(Clone, Debug)]
enum Hit {
    /// Ground: how far, its net, and what copper it is.
    Ground(f64, usize, DrcItem),
    /// Another net's copper.
    Foreign(f64, usize, DrcItem),
}

/// A point along an RF track: where, the track's direction there, how far along it.
#[derive(Clone, Copy)]
struct Sample {
    at: [f64; 2],
    dir: [f64; 2],
    t: f64,
}

/// The plane an RF track or pad references one way from its layer.
#[derive(Clone, Copy, Debug)]
struct Ref {
    layer: CopperLayer,
    h: f64,
    er: f64,
    net: usize,
}

/// An RF track as the line checks see it.
struct Line {
    ti: usize,
    id: u64,
    net: usize,
    layer: CopperLayer,
    width: f64,
    length: f64,
    /// Its points, those inside an antenna's zone or a courtyard of a part it joins left out.
    samples: Vec<Sample>,
    /// Each sample's first copper on its left and right within `reach` of its edge.
    sides: Vec<[Option<Hit>; 2]>,
    /// The planes it references (two for a stripline).
    refs: Vec<Ref>,
    reach: f64,
}

impl Line {
    /// Its dielectric height to the nearer reference.
    fn h(&self) -> Option<f64> {
        self.refs.iter().map(|r| r.h).filter(|h| *h > 0.0).min_by(f64::total_cmp)
    }

    /// The εr of the dielectrics to its references, the largest.
    fn er(&self) -> f64 {
        self.refs.iter().map(|r| r.er).fold(f64::NAN, f64::max).max(1.0)
    }

    /// Ground on both sides within `within` of its edge at half its samples or more, and the
    /// median of the two sides' mean gaps there: a coplanar line.
    fn coplanar(&self, within: f64) -> Option<(f64, DrcItem)> {
        let mut gaps: Vec<(f64, DrcItem)> = Vec::new();
        for [l, r] in &self.sides {
            if let (Some(Hit::Ground(a, _, item)), Some(Hit::Ground(b, _, _))) = (l, r)
                && *a <= within
                && *b <= within
            {
                gaps.push(((a + b) / 2.0, item.clone()));
            }
        }
        if self.sides.is_empty() || gaps.len() * 2 < self.sides.len() {
            return None;
        }
        gaps.sort_by(|a, b| a.0.total_cmp(&b.0));
        gaps.get(gaps.len() / 2).cloned()
    }

    /// The share of its samples with ground on side `k` (0 left, 1 right) within `within`, and
    /// the median gap there.
    fn side_ground(&self, k: usize, within: f64) -> (f64, f64) {
        let mut gaps: Vec<f64> = self
            .sides
            .iter()
            .filter_map(|s| match s.get(k) {
                Some(Some(Hit::Ground(d, _, _))) if *d <= within => Some(*d),
                _ => None,
            })
            .collect();
        if self.sides.is_empty() {
            return (0.0, 0.0);
        }
        gaps.sort_by(f64::total_cmp);
        (gaps.len() as f64 / self.sides.len() as f64, gaps.get(gaps.len() / 2).copied().unwrap_or(0.0))
    }

    /// The ground net beside it most often.
    fn ground(&self) -> Option<usize> {
        let mut by: BTreeMap<usize, usize> = BTreeMap::new();
        for hit in self.sides.iter().flatten().flatten() {
            if let Hit::Ground(_, n, _) = hit {
                *by.entry(*n).or_default() += 1;
            }
        }
        by.into_iter().max_by_key(|(_, c)| *c).map(|(n, _)| n)
    }

    /// The point `t` along it.
    fn point(&self, board: &super::super::board::Board, t: f64) -> [f64; 2] {
        let Some(track) = board.tracks.get(self.ti) else { return [0.0, 0.0] };
        let mut walked = 0.0;
        for (a, b) in track_pieces(&track.points, &track.curves) {
            let len = distance(a, b);
            if walked + len >= t && len > 0.0 {
                let s = ((t - walked) / len).clamp(0.0, 1.0);
                return [a[0] + (b[0] - a[0]) * s, a[1] + (b[1] - a[1]) * s];
            }
            walked += len;
        }
        track.points.last().copied().unwrap_or([0.0, 0.0])
    }
}

fn run(cx: &mut Cx) {
    let roles = cx.roles;
    let antennas = antennas(cx);
    if cx.wants(ANTENNA) {
        antenna_keepouts(cx, &antennas);
    }
    let rf: Vec<usize> = roles.nets.iter().filter(|n| n.has_role(NetRole::Rf)).map(|n| n.index).collect();
    let wanted: Vec<&'static str> = [LINE, REFERENCE, GAP, FENCE].into_iter().filter(|c| cx.wants(c)).collect();
    if wanted.is_empty() {
        return;
    }
    if rf.is_empty() {
        let radio = roles.intent.board.bands.as_ref().is_some_and(|b| !b.is_empty())
            || [PartRole::Antenna, PartRole::RfIc, PartRole::Module].into_iter().any(|r| roles.parts(r).next().is_some())
            || looks_radio(cx);
        if radio {
            for code in wanted {
                cx.not_applicable(code, "intent: the RF nets (role rf)");
            }
        }
        return;
    }
    if roles.grounds.is_empty() {
        let wanted: Vec<_> = [REFERENCE, GAP, FENCE].into_iter().filter(|c| cx.wants(c)).collect();
        for code in wanted {
            cx.not_applicable(code, "intent: the ground net (a net record with role ground)");
        }
    }
    let board = cx.board;
    // The antennas' zones: the outlines of the keepouts over them.
    let zones: Vec<Vec<[f64; 2]>> = antennas
        .iter()
        .flat_map(|a| a.keepouts.iter())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|k| board.keepouts.get(*k))
        .map(|k| path::tessellate(&k.outline, &k.curves, true, 0.05))
        .collect();
    let mut cache: BTreeMap<CopperLayer, Option<Layered>> = BTreeMap::new();
    if cx.wants(REFERENCE) && !roles.grounds.is_empty() {
        references(cx, &rf, &zones, &mut cache);
    }
    if cx.spent() {
        return;
    }
    if !(cx.wants(LINE) || cx.wants(GAP) || cx.wants(FENCE)) {
        return;
    }
    let mut lines: Vec<Line> = Vec::new();
    for (ti, t) in board.tracks.iter().enumerate() {
        let Some(net) = cx.nets.track_nets.get(ti).copied().flatten().filter(|n| rf.contains(n)) else { continue };
        let Some(copper) = copper_on(cx, &mut cache, t.layer) else { return };
        let Some(line) = line(cx, ti, net, &zones, copper) else { return };
        lines.push(line);
    }
    if cx.wants(LINE) || cx.wants(GAP) {
        impedances(cx, &lines);
    }
    if cx.wants(GAP) && !roles.grounds.is_empty() {
        neighbours(cx, &lines);
    }
    if cx.wants(FENCE) && !roles.grounds.is_empty() {
        fences(cx, &lines, &zones);
    }
}

/// The antennas kept and placed, with the keepouts that meet their courtyards.
fn antennas<'a>(cx: &Cx<'a>) -> Vec<Antenna<'a>> {
    let (board, roles) = (cx.board, cx.roles);
    let fps = footprints(cx);
    let mut out = Vec::new();
    for a in roles.parts(PartRole::Antenna) {
        let Some((designator, (fp, yard))) = fps.get_key_value(a.part.as_str()) else { continue };
        let corners = box_core(*yard).points;
        let centre = [(yard[0][0] + yard[1][0]) / 2.0, (yard[0][1] + yard[1][1]) / 2.0];
        let keepouts = board
            .keepouts
            .iter()
            .enumerate()
            .filter(|(_, k)| {
                let ring = path::tessellate(&k.outline, &k.curves, true, 0.05);
                super::super::bbox(ring.iter().copied()).is_some_and(|b| boxes_meet(b, *yard))
                    && (inside(centre, &ring)
                        || corners.iter().any(|c| inside(*c, &ring))
                        || ring.iter().any(|p| p[0] >= yard[0][0] && p[0] <= yard[1][0] && p[1] >= yard[0][1] && p[1] <= yard[1][1]))
            })
            .map(|(i, _)| i)
            .collect();
        out.push(Antenna { designator, footprint: fp, keepouts });
    }
    out
}

/// Whether the design looks like it has a radio though its intent says nothing of one: a net
/// named with an `RF` or `ANT` word (`LORA_RF`, `ANT_FEED`), or a part named `ANT…` or `AE…`.
fn looks_radio(cx: &Cx) -> bool {
    let words = |name: &str| name.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w.eq_ignore_ascii_case("rf") || w.eq_ignore_ascii_case("ant"));
    let part = |d: &str| {
        let prefix: String = d.chars().take_while(char::is_ascii_alphabetic).collect::<String>().to_ascii_uppercase();
        prefix == "ANT" || prefix == "AE"
    };
    cx.nets.names.iter().any(|n| words(n)) || cx.ecad.schematic.parts.iter().any(|p| part(&p.designator))
}

fn in_zone(zones: &[Vec<[f64; 2]>], p: [f64; 2]) -> bool {
    zones.iter().any(|z| inside(p, z))
}

fn antenna_keepouts(cx: &mut Cx, antennas: &[Antenna]) {
    let (board, nets, roles) = (cx.board, cx.nets, cx.roles);
    let radio = roles.intent.board.bands.as_ref().is_some_and(|b| !b.is_empty());
    // A module on a radio board with no pin on an RF net may carry its antenna on it.
    let mut modules = 0usize;
    if radio {
        let rf: BTreeSet<usize> = roles.nets.iter().filter(|n| n.has_role(NetRole::Rf)).map(|n| n.index).collect();
        let fps = footprints(cx);
        for m in roles.parts(PartRole::Module) {
            let Some((designator, (fp, _))) = fps.get_key_value(m.part.as_str()) else { continue };
            if nets.pads.iter().any(|p| p.footprint == fp.id && p.net.is_some_and(|n| rf.contains(&n))) {
                continue;
            }
            modules += 1;
            let item = DrcItem::Footprint { id: fp.id, designator: designator.to_string() };
            cx.add(ANTENNA, &[item.key(), "unverified".into()], || {
                Draft::new(
                    fp.at,
                    vec![item.clone()],
                    format!("module {designator} has no pin on an RF net, so it may carry its own antenna, and its library has no keepout: if it does, draw its maker's antenna keepout as board keepouts"),
                )
                .variant("module")
                .severity(Severity::Info)
                .review()
                .fix("ecad.board.keepout", Value::Null, "draw the module maker's antenna keepout as board keepouts, layer by layer, as its drawing says")
            });
        }
    }
    if antennas.is_empty() {
        if (radio || looks_radio(cx)) && modules == 0 {
            cx.not_applicable(ANTENNA, "intent: the antenna part (role antenna)");
        }
        return;
    }
    for a in antennas {
        let item = DrcItem::Footprint { id: a.footprint.id, designator: a.designator.to_string() };
        let own: BTreeSet<usize> = nets.pads.iter().filter(|p| p.footprint == a.footprint.id).filter_map(|p| p.net).collect();
        let names: Vec<String> = a.keepouts.iter().filter_map(|k| board.keepouts.get(*k)).map(|k| k.id.to_string()).collect();
        let covered: BTreeSet<CopperLayer> =
            cx.geo.layers.iter().copied().filter(|l| a.keepouts.iter().filter_map(|k| board.keepouts.get(*k)).any(|k| k.on(*l))).collect();
        let bare: Vec<String> = cx.geo.layers.iter().filter(|l| !covered.contains(l)).map(|l| l.name()).collect();
        let at = a.footprint.at;
        let says = if names.is_empty() {
            format!(
                "antenna {} has no keepout from its library, and no keepout on the board covers it: draw its maker's keepout on every layer it names",
                a.designator
            )
        } else if bare.is_empty() {
            format!(
                "antenna {} has no keepout from its library: check keepout{} {} against its maker's drawing",
                a.designator,
                if names.len() == 1 { "" } else { "s" },
                names.join(", ")
            )
        } else {
            format!(
                "antenna {} has no keepout from its library: check keepout{} {} against its maker's drawing; none covers it on {}",
                a.designator,
                if names.len() == 1 { "" } else { "s" },
                names.join(", "),
                bare.join(", ")
            )
        };
        let variant = if names.is_empty() { "none" } else { "unverified" };
        cx.add(ANTENNA, &[item.key(), "unverified".into()], || {
            Draft::new(at, vec![item.clone()], says).variant(variant).severity(Severity::Info).review().arg("keepouts", names.join(", ")).fix(
                "ecad.board.keepout",
                Value::Null,
                "draw the antenna maker's keepout as board keepouts, layer by layer, as its drawing says",
            )
        });
        // Other parts' pads in a keepout that keeps copper out but lets parts in (the design rule
        // check reports footprints in one that keeps parts out, and tracks and vias).
        for k in a.keepouts.iter().filter_map(|k| board.keepouts.get(*k)) {
            let keeps_copper = k.forbid.tracks || k.forbid.vias || k.forbid.pours;
            if !keeps_copper || k.forbid.components {
                continue;
            }
            let ring = path::tessellate(&k.outline, &k.curves, true, 0.05);
            let Some(kbox) = super::super::bbox(ring.iter().copied()) else { continue };
            if !cx.spend(nets.pads.len().saturating_mul(ring.len().max(1)).saturating_add(1)) {
                return;
            }
            let n = ring.len();
            for p in nets.pads.iter() {
                if p.footprint == a.footprint.id || p.net.is_some_and(|x| own.contains(&x)) || p.pad.kind == PadKind::Hole {
                    continue;
                }
                let layers: Vec<CopperLayer> = cx.geo.layers.iter().copied().filter(|l| p.on(*l) && k.on(*l)).collect();
                if layers.is_empty() || !boxes_meet(p.bbox(), kbox) {
                    continue;
                }
                let core = pad_core(p);
                let meets = inside(p.at(), &ring)
                    || (0..n).filter_map(|i| Some((*ring.get(i)?, *ring.get((i + 1) % n)?))).any(|(u, v)| gap(&core, &stroke(u, v, 0.0)) < 0.0);
                if !meets {
                    continue;
                }
                let pad = pad_item(p);
                let other = p.net.map(|x| net_name(cx, x)).unwrap_or_else(|| "no net".into());
                let on = layers.iter().map(|l| l.name()).collect::<Vec<_>>().join(", ");
                let kitem = DrcItem::Keepout(k.id);
                let at = p.at();
                cx.add(ANTENNA, &[item.key(), kitem.key(), pad.key()], || {
                    Draft::new(
                        at,
                        vec![item.clone(), pad.clone()],
                        format!("pad {} ({other}) is inside keepout {} over antenna {} on {on}", p.reference(), k.id, a.designator),
                    )
                    .nets(vec![other.clone()])
                    .arg("other", other.clone())
                    .arg("keepout", k.id.to_string())
                    .arg("under", on.clone())
                    .fix("ecad.board.move", Value::Null, format!("move {} out of the antenna's keepout", p.designator))
                });
            }
        }
    }
}

/// The layers going away from `layer`, each way (one way from an outer layer).
fn ways(layers: &[CopperLayer], layer: CopperLayer) -> Vec<Vec<CopperLayer>> {
    let Some(i) = layers.iter().position(|l| *l == layer) else { return Vec::new() };
    let up: Vec<CopperLayer> = layers.get(..i).map(|s| s.iter().rev().copied().collect()).unwrap_or_default();
    let down: Vec<CopperLayer> = layers.get(i + 1..).map(<[CopperLayer]>::to_vec).unwrap_or_default();
    [up, down].into_iter().filter(|w| !w.is_empty()).collect()
}

/// Walking `way` from `layer`: the first layer whose planes (not of `own`) cover `share` of
/// `points` (those over `own`'s antipads there counting neither way), the net that covers most
/// of them there, and the layers before it.
fn walk(cx: &Cx, layer: CopperLayer, way: &[CopperLayer], points: &[[f64; 2]], own: usize, share: f64) -> (Option<Ref>, Vec<CopperLayer>) {
    let (geo, nets) = (cx.geo, cx.nets);
    let mut before = Vec::new();
    for l in way {
        let Some(plane) = geo.plane(*l).filter(|p| !p.is_empty()) else {
            before.push(*l);
            continue;
        };
        let at = plane.nets_at(nets, points);
        let skip = geo.own_antipads(*l, points, Some(own), ANTIPAD_SLACK);
        let mut by: BTreeMap<usize, usize> = BTreeMap::new();
        let mut counted = 0usize;
        for (n, skipped) in at.into_iter().zip(skip) {
            if skipped {
                continue;
            }
            counted += 1;
            if let Some(n) = n.filter(|n| *n != own) {
                *by.entry(n).or_default() += 1;
            }
        }
        let on: usize = by.values().sum();
        if counted > 0 && on as f64 >= share * counted as f64 {
            let net = by.iter().max_by_key(|(_, c)| **c).map(|(n, _)| *n);
            if let Some(net) = net {
                return (Some(Ref { layer: *l, h: geo.height(layer, *l), er: geo.er(layer, *l), net }), before);
            }
        }
        before.push(*l);
    }
    (None, before)
}

/// The frequency of an RF net in MHz, how it is known, and whether it is assumed (see the module
/// notes).
fn carrier(cx: &Cx, info: &NetInfo) -> Option<(f64, String, bool)> {
    if let Some(f) = info.f_mhz().filter(|f| f.is_finite() && *f > 0.0) {
        return Some((f, format!("{} MHz, its record's", num(f)), false));
    }
    let bands = cx.roles.intent.board.bands.as_ref()?;
    let top = bands.iter().filter(|b| b.hi_mhz.is_finite() && b.hi_mhz > 0.0).max_by(|a, b| a.hi_mhz.total_cmp(&b.hi_mhz))?;
    if bands.len() > 1 {
        Some((
            top.hi_mhz,
            format!("assumed {} MHz, the top of the highest band ({}): give {}'s f_mhz to confirm", num(top.hi_mhz), top.name, info.name),
            true,
        ))
    } else {
        Some((top.hi_mhz, format!("{} MHz, the top of the {} band", num(top.hi_mhz), top.name), false))
    }
}

/// The footprints a net has a pad in, by id, with their courtyards.
fn joined_yards(cx: &Cx, net: usize) -> Vec<[[f64; 2]; 2]> {
    let ids: BTreeSet<u64> = cx.nets.pads.iter().filter(|p| p.net == Some(net)).map(|p| p.footprint).collect();
    footprints(cx).into_values().filter(|(f, _)| ids.contains(&f.id)).map(|(_, y)| y).collect()
}

fn in_box([[x0, y0], [x1, y1]]: [[f64; 2]; 2], p: [f64; 2]) -> bool {
    p[0] >= x0 && p[0] <= x1 && p[1] >= y0 && p[1] <= y1
}

/// An RF track as the line checks see it (see [`Line`]); `None` once the budget is spent.
fn line(cx: &mut Cx, ti: usize, net: usize, zones: &[Vec<[f64; 2]>], copper: &Layered) -> Option<Line> {
    let (board, nets, geo, roles) = (cx.board, cx.nets, cx.geo, cx.roles);
    let t = board.tracks.get(ti)?;
    let length = path::length(&t.points, &t.curves, false);
    let step = STEP.max(length / MAX_POINTS as f64);
    let yards = joined_yards(cx, net);
    let mut all: Vec<Sample> = Vec::new();
    let mut walked = 0.0;
    for (a, b) in track_pieces(&t.points, &t.curves) {
        let len = distance(a, b);
        if len <= 1e-9 {
            continue;
        }
        let dir = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        all.extend(along(a, b, step).into_iter().map(|at| Sample { at, dir, t: walked + distance(a, at) }));
        walked += len;
    }
    if !cx.spend(all.len().saturating_add(1)) {
        return None;
    }
    // The references, from the centreline between the parts it joins (all of it outside the
    // antenna's zone, for a track inside their courtyards).
    let between: Vec<[f64; 2]> = all.iter().map(|s| s.at).filter(|p| !in_zone(zones, *p) && !yards.iter().any(|y| in_box(*y, *p))).collect();
    let centre = if between.is_empty() { all.iter().map(|s| s.at).filter(|p| !in_zone(zones, *p)).collect() } else { between };
    let share = super::param_value(board, REFERENCE, "coverage");
    let refs: Vec<Ref> = ways(&geo.layers, t.layer).iter().filter_map(|w| walk(cx, t.layer, w, &centre, net, share).0).collect();
    let h = refs.iter().map(|r| r.h).filter(|h| *h > 0.0).min_by(f64::total_cmp);
    let h_reach = h.unwrap_or_else(|| geo.neighbours(t.layer).first().map_or(0.2, |(_, d)| *d));
    let (widths, heights) = (super::param_value(board, GAP, "widths"), super::param_value(board, GAP, "heights"));
    let side_h = super::param_value(board, FENCE, "side");
    let reach = (widths * t.width).max(heights * h_reach).max(side_h * h_reach).max(0.05);
    let samples: Vec<Sample> = all.into_iter().filter(|s| !in_zone(zones, s.at) && !yards.iter().any(|y| in_box(*y, s.at))).collect();
    // Each sample's first copper outward on each side, from the track's edge, other than its own
    // net's (another RF net's is no ground either): a piece where a ray from the edge first meets it (a search along
    // the ray), a fill at the first of the probes along it inside it; the nearer, another net's
    // before ground's at the same distance.
    let dstep = (reach / PROBES as f64).max(0.005);
    let k_max = ((reach / dstep).ceil() as usize).clamp(1, PROBES * 4);
    let half = t.width / 2.0;
    let fills: Vec<usize> = copper.fills.iter().copied().filter(|f| nets.fills.get(*f).and_then(|fill| fill.net).is_some_and(|n| n != net)).collect();
    let mut probes: Vec<[f64; 2]> = Vec::new();
    if !fills.is_empty() {
        let cost = samples.len().saturating_mul(2 * k_max).saturating_mul(fills.len());
        if !cx.spend(cost.saturating_add(1)) {
            return None;
        }
        probes.reserve(samples.len().saturating_mul(2 * k_max));
        for s in &samples {
            let n = [-s.dir[1], s.dir[0]];
            for side in [1.0, -1.0] {
                for k in 1..=k_max {
                    let d = half + k as f64 * dstep;
                    probes.push([s.at[0] + n[0] * side * d, s.at[1] + n[1] * side * d]);
                }
            }
        }
    }
    let mut in_fill: Vec<Option<(usize, u64)>> = vec![None; probes.len()];
    for f in &fills {
        let Some(fill) = nets.fills.get(*f) else { continue };
        let Some(fnet) = fill.net else { continue };
        for (slot, found) in in_fill.iter_mut().zip(fill.region.locate(&probes)) {
            if slot.is_none() && found.is_some() {
                *slot = Some((fnet, fill.pour));
            }
        }
    }
    let mut sides: Vec<[Option<Hit>; 2]> = Vec::with_capacity(samples.len());
    for (si, s) in samples.iter().enumerate() {
        let n = [-s.dir[1], s.dir[0]];
        let mut both: [Option<Hit>; 2] = [None, None];
        for (side, slot) in both.iter_mut().enumerate() {
            let sign = if side == 0 { 1.0 } else { -1.0 };
            let dir = [n[0] * sign, n[1] * sign];
            let from = [s.at[0] + dir[0] * half, s.at[1] + dir[1] * half];
            let to = [from[0] + dir[0] * reach, from[1] + dir[1] * reach];
            let span = [[from[0].min(to[0]), from[1].min(to[1])], [from[0].max(to[0]), from[1].max(to[1])]];
            let near: Vec<&Piece> = copper.near(span).filter(|p| !p.edge && p.net.is_some_and(|m| m != net)).collect();
            if !cx.spend(near.len().saturating_mul(RAY_STEPS + 1).saturating_add(1)) {
                return None;
            }
            // The nearest: how far, whether another net's (not ground), its net and copper.
            let mut best: Option<(f64, bool, usize, DrcItem)> = None;
            let mut keep = |d: f64, m: usize, item: &DrcItem| {
                let foreign = !roles.grounds.contains(&m);
                let better = best.as_ref().is_none_or(|(bd, bf, _, _)| d < *bd - dstep || ((d - *bd).abs() <= dstep && foreign && !*bf));
                if better {
                    best = Some((d, foreign, m, item.clone()));
                }
            };
            for p in near {
                if let (Some(m), Some(d)) = (p.net, first_hit(from, dir, reach, &p.core)) {
                    keep(d, m, &p.item);
                }
            }
            for k in 1..=k_max {
                let idx = (si * 2 + side) * k_max + k.saturating_sub(1);
                if let Some(Some((fnet, pour))) = in_fill.get(idx) {
                    keep(k as f64 * dstep, *fnet, &DrcItem::Pour(*pour));
                    break;
                }
            }
            *slot = best.map(|(d, foreign, m, item)| if foreign { Hit::Foreign(d, m, item) } else { Hit::Ground(d, m, item) });
        }
        sides.push(both);
    }
    Some(Line { ti, id: t.id, net, layer: t.layer, width: t.width, length, samples, sides, refs, reach })
}

/// How far along the ray from `from` in the direction `dir` (of unit length), up to `reach`,
/// the copper `core` is first met: the shortest stretch of the ray that meets it, found by
/// halving ([`RAY_STEPS`] times); `None` when the whole ray misses it.
fn first_hit(from: [f64; 2], dir: [f64; 2], reach: f64, core: &Core) -> Option<f64> {
    let at = |d: f64| [from[0] + dir[0] * d, from[1] + dir[1] * d];
    if gap(&stroke(from, at(reach), 0.0), core) > 0.0 {
        return None;
    }
    if gap(&disc(from, 0.0), core) <= 0.0 {
        return Some(0.0);
    }
    let (mut lo, mut hi) = (0.0, reach);
    for _ in 0..RAY_STEPS {
        let mid = (lo + hi) / 2.0;
        if gap(&stroke(from, at(mid), 0.0), core) <= 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(hi)
}

/// The chains of the RF tracks `lines` (by index into them): tracks of a net on one layer joined
/// end to end, where nothing else meets (no pad, no via, no third track).
fn chains(cx: &mut Cx, lines: &[Line]) -> Vec<Vec<usize>> {
    let (board, nets) = (cx.board, cx.nets);
    let n = lines.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        let mut hops = 0usize;
        while let Some(p) = parent.get(i).copied() {
            if p == i || hops > parent.len() {
                break;
            }
            i = p;
            hops += 1;
        }
        i
    }
    let alone = || -> Vec<Vec<usize>> { (0..n).map(|i| vec![i]).collect() };
    let tol = 0.01;
    // Every line's ends and straight pieces, by line, each with a grid.
    let mut ends: Vec<(usize, [f64; 2])> = Vec::new();
    let mut end_grid = Grid::new(0.5);
    let mut pieces: Vec<(usize, [f64; 2], [f64; 2])> = Vec::new();
    let mut piece_grid = Grid::new(0.5);
    for (i, l) in lines.iter().enumerate() {
        for p in track_ends(board, l.ti) {
            end_grid.insert(ends.len(), [p, p]);
            ends.push((i, p));
        }
        for (u, v) in board.tracks.get(l.ti).map(|t| track_pieces(&t.points, &t.curves)).unwrap_or_default() {
            piece_grid.insert(pieces.len(), grow([[u[0].min(v[0]), u[1].min(v[1])], [u[0].max(v[0]), u[1].max(v[1])]], tol));
            pieces.push((i, u, v));
        }
    }
    if !cx.spend(ends.len().saturating_add(pieces.len()).saturating_add(1)) {
        return alone();
    }
    for (i, p) in ends.iter().copied() {
        let Some(a) = lines.get(i) else { continue };
        let same = |k: usize| lines.get(k).is_some_and(|l| l.net == a.net && l.layer == a.layer);
        let probe = [[p[0] - tol, p[1] - tol], [p[0] + tol, p[1] + tol]];
        let meeting: Vec<usize> = end_grid
            .near_box(probe, ends.len())
            .into_iter()
            .filter_map(|e| ends.get(e).copied())
            .filter(|(k, q)| same(*k) && distance(p, *q) <= tol)
            .map(|(k, _)| k)
            .collect();
        if !cx.spend(meeting.len().saturating_add(1)) {
            return alone();
        }
        // Exactly two ends meet there, of two lines, on no other line, pad or via.
        let [x, y] = meeting.as_slice() else { continue };
        let j = if *x == i { *y } else { *x };
        if j <= i {
            continue;
        }
        let near = piece_grid.near_box(probe, pieces.len());
        if !cx.spend(near.len().saturating_add(nets.pads.len()).saturating_add(board.vias.len()).saturating_add(1)) {
            return alone();
        }
        let through = near
            .into_iter()
            .filter_map(|k| pieces.get(k))
            .any(|(k, u, v)| *k != i && *k != j && same(*k) && path::segment_distance(p, *u, *v, 0.0) <= tol);
        let pad = nets.pads.iter().any(|q| q.on(a.layer) && q.contains(p));
        let via = board.vias.iter().any(|v| v.on(a.layer, board.copper_layers) && distance(v.at, p) <= v.diameter / 2.0);
        if !through && !pad && !via {
            let (ra, rb) = (root(&mut parent, i), root(&mut parent, j));
            if let Some(slot) = parent.get_mut(ra) {
                *slot = rb;
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        groups.entry(root(&mut parent, i)).or_default().push(i);
    }
    groups.into_values().collect()
}

/// A track's two ends (none for a track with no points).
fn track_ends(board: &super::super::board::Board, ti: usize) -> Vec<[f64; 2]> {
    board.tracks.get(ti).map(|t| [t.points.first().copied(), t.points.last().copied()].into_iter().flatten().collect()).unwrap_or_default()
}

/// The medium of a line over its references: one plane (a microstrip, or a coplanar line on an
/// outer layer), two (a stripline).
fn medium(cx: &Cx, l: &Line) -> Option<(Medium, bool)> {
    let t = surroundings(&cx.geo.stackup, l.layer, cx.board.copper_layers).map_or(0.035, |s| s.copper.thickness);
    match l.refs.as_slice() {
        [r] => Some((Medium { outer: true, h1: r.h, h2: 0.0, t, er: r.er }, false)),
        [a, b] => {
            let (h1, h2) = (a.h, b.h);
            let er = if h1 + h2 > 0.0 { (a.er * h1 + b.er * h2) / (h1 + h2) } else { DEFAULT_ER };
            Some((Medium { outer: false, h1, h2, t, er }, true))
        }
        _ => None,
    }
}

/// A line's reference planes, as findings say them.
fn refs_say(cx: &Cx, l: &Line) -> String {
    l.refs
        .iter()
        .map(|r| format!("{} ({}) at h {} mm, εr {}", r.layer.name(), net_name(cx, r.net), num(r.h), num((r.er * 100.0).round() / 100.0)))
        .collect::<Vec<_>>()
        .join(" and ")
}

fn impedances(cx: &mut Cx, lines: &[Line]) {
    let (board, roles) = (cx.board, cx.roles);
    let p = |code: &str, name: &str| super::param_value(board, code, name);
    let (fraction, warn, error, min_width, default_target) =
        (p(LINE, "fraction"), p(LINE, "warn"), p(LINE, "error"), p(LINE, "min_width"), p(LINE, "target"));
    let (gap_warn, gap_error, widths, heights) = (p(GAP, "warn"), p(GAP, "error"), p(GAP, "widths"), p(GAP, "heights"));
    let mut no_freq: BTreeSet<usize> = BTreeSet::new();
    for chain in chains(cx, lines) {
        let members: Vec<&Line> = chain.iter().filter_map(|i| lines.get(*i)).collect();
        let Some(first) = members.first() else { continue };
        let Some(info) = roles.net(first.net) else { continue };
        let Some((f, f_says, assumed)) = carrier(cx, info) else {
            if no_freq.insert(first.net) {
                for code in [LINE, GAP] {
                    cx.not_applicable(code, format!("intent: f_mhz of {} or the board's radio bands", info.name));
                }
            }
            continue;
        };
        let er = members.iter().map(|l| l.er()).fold(1.0, f64::max);
        let er = if members.iter().all(|l| l.refs.is_empty()) { DEFAULT_ER } else { er };
        let lambda = C_MM_MHZ / f / er.sqrt();
        let long = members.iter().map(|l| l.length).sum::<f64>();
        if long <= fraction * lambda {
            continue;
        }
        let target = info.record.and_then(|r| r.z_target).filter(|z| z.is_finite() && *z > 0.0).unwrap_or(default_target);
        // A tolerance the net declares is its error limit.
        let (warn, error) = match info.record.and_then(|r| r.z_tol).filter(|t| t.is_finite() && *t > 0.0) {
            Some(t) => (warn.min(t), t),
            None => (warn, error),
        };
        let name = info.name.to_string();
        for l in &members {
            if cx.spent() {
                return;
            }
            let at = l.samples.get(l.samples.len() / 2).map_or_else(|| l.point(board, l.length / 2.0), |s| s.at);
            let chain_says = format!("{f_says}; chain {} mm, λg/{} {} mm", num(long), num((1.0 / fraction).round()), num(fraction * lambda));
            // Too narrow for FR-4's etch.
            if l.width < min_width && l.er() <= FR4_ER {
                let regime = chain_says.clone();
                cx.add(LINE, &[DrcItem::Track(l.id).key(), "width".into()], || {
                    Draft::new(
                        at,
                        vec![DrcItem::Track(l.id)],
                        format!("RF track {} ({name}) is {} mm wide; RF lines on FR-4 stay {} mm or wider", l.id, num(l.width), num(min_width)),
                    )
                    .layer(l.layer)
                    .nets(vec![name.clone()])
                    .variant("width")
                    .measure(l.width, min_width, "mm", Bound::Min)
                    .param("min_width")
                    .severity(Severity::Warning)
                    .regime(regime)
                    .arg("net", name.clone())
                    .fix(
                        "ecad.board.impedance.inspect",
                        json!({"layer": l.layer.name(), "kind": "microstrip", "target": target}),
                        "the width for its impedance",
                    )
                });
            }
            let Some((m, strip)) = medium(cx, l) else {
                cx.not_applicable(LINE, format!("geometry: a plane under RF track {} ({})", l.id, l.layer.name()));
                continue;
            };
            let kind = if strip { LineKind::Stripline } else { LineKind::Microstrip };
            let Ok(plain) = impedance::impedance(&m, kind, l.width, None) else { continue };
            let h = l.h().unwrap_or(m.h1);
            let within = (widths * l.width).max(heights * h);
            let coplanar = if strip { None } else { l.coplanar(within) };
            let dev = |z: f64| (z - target).abs() / target;
            let refs = refs_say(cx, l);
            // With ground beside it, two readings: a coplanar line designed with that gap (the
            // coplanar model), or a microstrip the ground loads (the microstrip's figure scaled
            // by the coplanar model's own drop from the gap where loading starts, `within`, to
            // this one: so the two formulas' disagreement is no finding).
            let mut ground: Option<(f64, DrcItem)> = None;
            let mut model = if strip { "a stripline".to_string() } else { "a microstrip".to_string() };
            let mut z0 = plain;
            let mut loaded: Option<f64> = None;
            if let Some((s, item)) = &coplanar
                && let (Ok(cp), Ok(open)) = (
                    impedance::impedance(&m, LineKind::Coplanar, l.width, Some(*s)),
                    impedance::impedance(&m, LineKind::Coplanar, l.width, Some(within.max(*s))),
                )
                && open.z0 > 0.0
            {
                let est = plain.z0 * (cp.z0 / open.z0).min(1.0);
                ground = Some((*s, item.clone()));
                loaded = Some(est);
                if dev(cp.z0) <= dev(est) {
                    model = format!("a coplanar line with ground {} mm beside it", num(*s));
                    z0 = cp;
                } else {
                    model = format!("a microstrip loaded by ground {} mm beside it", num(*s));
                    z0.z0 = est;
                }
            }
            let off = dev(z0.z0);
            let range = if z0.valid_range { String::new() } else { format!("; outside the {} formula's range", z0.formula) };
            if off <= warn {
                continue;
            }
            if let (Some((s, item)), Some(est)) = (ground.clone(), loaded)
                && dev(plain.z0) <= warn
            {
                // The width suits a microstrip: the ground beside it is the fault.
                let off = dev(est);
                if off <= gap_warn {
                    continue;
                }
                let severity = if off > gap_error && !assumed { Severity::Error } else { Severity::Warning };
                let regime = format!("{chain_says}; reference {refs}{range}");
                let zp = plain.z0;
                cx.add(GAP, &[DrcItem::Track(l.id).key(), "gap".into()], || {
                    Draft::new(
                        at,
                        vec![DrcItem::Track(l.id), item.clone()],
                        format!(
                            "ground {} mm beside RF track {} ({name}) loads it: a microstrip of {} Ω, it is about {} Ω with that gap, {} % off {} Ω; open the gap to {} mm or make it a designed coplanar line",
                            num(s),
                            l.id,
                            num(zp.round()),
                            num(est.round()),
                            num((off * 100.0).round()),
                            num(target),
                            num(within)
                        ),
                    )
                    .layer(l.layer)
                    .nets(vec![name.clone()])
                    .variant("gap")
                    .measure(s, within, "mm", Bound::Min)
                    .param("heights")
                    .severity(severity)
                    .regime(regime)
                    .arg("net", name.clone())
                    .arg("z0", num(est.round()))
                    .arg("microstrip", num(zp.round()))
                    .arg("target", num(target))
                    .arg("percent", num((off * 100.0).round()))
                    .fix("ecad.board.impedance.inspect", json!({"layer": l.layer.name(), "kind": "coplanar", "target": target, "gap": s}), "the width that makes it a coplanar line of its target with this gap")
                    .fix("ecad.board.pour.edit", Value::Null, format!("or open the ground's clearance round it to {} mm (pour clearance)", num(within)))
                });
                continue;
            }
            let severity = if off > error && !assumed { Severity::Error } else { Severity::Warning };
            let kind_name = match (strip, &ground) {
                (true, _) => "stripline",
                (false, Some(_)) => "coplanar",
                (false, None) => "microstrip",
            };
            let regime = format!("{chain_says}; reference {refs}{range}");
            let gap_param = ground.as_ref().map(|(s, _)| *s);
            let z = z0.z0;
            cx.add(LINE, &[DrcItem::Track(l.id).key()], || {
                let mut fix = json!({"layer": l.layer.name(), "kind": kind_name, "target": target});
                if let (Some(s), Some(obj)) = (gap_param, fix.as_object_mut()) {
                    obj.insert("gap".into(), json!(s));
                }
                Draft::new(
                    at,
                    vec![DrcItem::Track(l.id)],
                    format!(
                        "RF track {} ({name}, {} mm wide) is {} Ω as {model}, {} % off its {} Ω, on a chain longer than λg/{}",
                        l.id,
                        num(l.width),
                        num(z.round()),
                        num((off * 100.0).round()),
                        num(target),
                        num((1.0 / fraction).round())
                    ),
                )
                .layer(l.layer)
                .nets(vec![name.clone()])
                .measure(off, if severity == Severity::Error { error } else { warn }, "ratio", Bound::Max)
                .param(if severity == Severity::Error { "error" } else { "warn" })
                .severity(severity)
                .regime(regime)
                .arg("net", name.clone())
                .arg("z0", num(z.round()))
                .arg("model", model.clone())
                .arg("target", num(target))
                .arg("percent", num((off * 100.0).round()))
                .fix("ecad.board.impedance.inspect", fix, "the width for its target impedance over this stack-up")
            });
        }
    }
}

/// Another net's copper as the nearest copper beside an RF track (`rf.coplanar_gap`).
fn neighbours(cx: &mut Cx, lines: &[Line]) {
    let (board, roles) = (cx.board, cx.roles);
    let (widths, heights) = (super::param_value(board, GAP, "widths"), super::param_value(board, GAP, "heights"));
    for l in lines {
        let h = l.h().unwrap_or(0.0);
        let within = (widths * l.width).max(heights * h);
        let mut by: BTreeMap<usize, (f64, DrcItem, [f64; 2])> = BTreeMap::new();
        for (s, sides) in l.samples.iter().zip(&l.sides) {
            for hit in sides.iter().flatten() {
                if let Hit::Foreign(d, n, item) = hit
                    && *d <= within
                    && by.get(n).is_none_or(|(old, _, _)| *d < *old)
                {
                    by.insert(*n, (*d, item.clone(), s.at));
                }
            }
        }
        let Some(info) = roles.net(l.net) else { continue };
        let name = info.name.to_string();
        for (n, (d, item, at)) in by {
            let other = net_name(cx, n);
            let regime = if h > 0.0 { format!("h {} mm to {}", num(h), refs_say(cx, l)) } else { "no plane under it".to_string() };
            cx.add(GAP, &[DrcItem::Track(l.id).key(), item.key()], || {
                Draft::new(
                    at,
                    vec![DrcItem::Track(l.id), item.clone()],
                    format!(
                        "{} ({other}) is the nearest copper beside RF track {} ({name}, {}), {} mm from its edge, within max(2W, 3h) = {} mm: an RF line's neighbour on its layer must be ground",
                        item.reference(),
                        l.id,
                        l.layer.name(),
                        num(d),
                        num(within)
                    ),
                )
                .layer(l.layer)
                .nets(vec![name.clone(), other.clone()])
                .measure(d, within, "mm", Bound::Min)
                .param("heights")
                .regime(regime)
                .arg("net", name.clone())
                .arg("other", other.clone())
                .fix("ecad.board.net.inspect", json!({"nets": [other], "metrics": ["layers"]}), format!("move {other} away from {name}, or put ground between"))
            });
        }
    }
}

/// Another net's copper under RF tracks and pads, and power planes as their reference. Each point
/// of the RF copper is walked down (and up, from an inner layer) to the first plane under it, its
/// reference there: other nets' copper on the way is under it, and a plane of another net than
/// ground as that reference is a power reference (for `coverage` of the points; under less of
/// them, it is other copper under it). A point over the net's own antipad counts neither way, and
/// a void on the way (declared or not) only moves the reference down.
fn references(cx: &mut Cx, rf: &[usize], zones: &[Vec<[f64; 2]>], cache: &mut BTreeMap<CopperLayer, Option<Layered>>) {
    let (board, nets, geo, roles) = (cx.board, cx.nets, cx.geo, cx.roles);
    let share = super::param_value(board, REFERENCE, "coverage");
    let min_length = super::param_value(board, REFERENCE, "min_length");
    // The RF copper: tracks, and surface-mount pads on top or bottom, as points with the radius
    // of copper round each.
    struct Item {
        item: DrcItem,
        net: usize,
        layer: CopperLayer,
        radius: f64,
        points: Vec<[f64; 2]>,
    }
    let mut items: Vec<Item> = Vec::new();
    for (ti, t) in board.tracks.iter().enumerate() {
        let Some(net) = nets.track_nets.get(ti).copied().flatten().filter(|n| rf.contains(n)) else { continue };
        let points: Vec<[f64; 2]> = track_pieces(&t.points, &t.curves).into_iter().flat_map(|(a, b)| along(a, b, STEP.max(t.width / 2.0))).collect();
        items.push(Item { item: DrcItem::Track(t.id), net, layer: t.layer, radius: t.width / 2.0, points });
    }
    for p in nets.pads.iter().filter(|p| p.net.is_some_and(|n| rf.contains(&n)) && p.pad.kind == PadKind::Smd) {
        let Some(net) = p.net else { continue };
        let layer = if p.top { CopperLayer::Top } else { CopperLayer::Bottom };
        let [[x0, y0], [x1, y1]] = p.bbox();
        let mut points = Vec::new();
        for i in 0..5 {
            for j in 0..5 {
                let q = [x0 + (x1 - x0) * (f64::from(i) + 0.5) / 5.0, y0 + (y1 - y0) * (f64::from(j) + 0.5) / 5.0];
                if p.contains(q) {
                    points.push(q);
                }
            }
        }
        points.push(p.at());
        items.push(Item { item: pad_item(p), net, layer, radius: 0.0, points });
    }
    let mut no_plane: BTreeSet<CopperLayer> = BTreeSet::new();
    for it in &items {
        if cx.spent() {
            return;
        }
        // Inside an antenna's zone: the antenna's.
        let outside: Vec<[f64; 2]> = it.points.iter().copied().filter(|p| !in_zone(zones, *p)).collect();
        if outside.len() * 2 < it.points.len() || outside.is_empty() {
            continue;
        }
        if !cx.spend(outside.len().saturating_mul(geo.layers.len()).saturating_mul(4).saturating_add(1)) {
            return;
        }
        let Some(info) = roles.net(it.net) else { continue };
        let name = info.name.to_string();
        // Each other net under it: the layers, and its first piece; the planes its points
        // reference, by layer and net, with how many points; the points that count, each way.
        let mut under: BTreeMap<usize, (Vec<CopperLayer>, DrcItem)> = BTreeMap::new();
        let mut planes: BTreeMap<(CopperLayer, usize), usize> = BTreeMap::new();
        let mut counted: Vec<usize> = Vec::new();
        let mut found_any = false;
        for way in ways(&geo.layers, it.layer) {
            let mut open = vec![true; outside.len()];
            let mut count = outside.len();
            for layer in way {
                if !open.iter().any(|o| *o) {
                    break;
                }
                let at = geo.plane(layer).map_or_else(|| vec![None; outside.len()], |pl| pl.nets_at(nets, &outside));
                let skip = geo.own_antipads(layer, &outside, Some(it.net), ANTIPAD_SLACK);
                let Some(copper) = copper_on(cx, cache, layer) else { return };
                for (k, p) in outside.iter().enumerate() {
                    let Some(slot) = open.get_mut(k) else { continue };
                    if !*slot {
                        continue;
                    }
                    if skip.get(k).copied().unwrap_or(false) {
                        // Over its own antipad: neither way.
                        *slot = false;
                        count = count.saturating_sub(1);
                        continue;
                    }
                    let here = disc(*p, it.radius);
                    let r = it.radius;
                    for q in copper.near([[p[0] - r, p[1] - r], [p[0] + r, p[1] + r]]) {
                        let Some(n) = q.net.filter(|n| *n != it.net && !roles.grounds.contains(n)) else { continue };
                        if q.edge || gap(&q.core, &here) >= 0.0 {
                            continue;
                        }
                        let e = under.entry(n).or_insert_with(|| (Vec::new(), q.item.clone()));
                        if !e.0.contains(&layer) {
                            e.0.push(layer);
                        }
                    }
                    if let Some(n) = at.get(k).copied().flatten().filter(|n| *n != it.net) {
                        *slot = false;
                        found_any = true;
                        *planes.entry((layer, n)).or_default() += 1;
                    }
                }
            }
            counted.push(count);
        }
        let at = it.points.get(it.points.len() / 2).copied().unwrap_or([0.0, 0.0]);
        let most = counted.iter().copied().max().unwrap_or(0).max(1);
        // The references (ground or not) as the regime says them.
        let reference: Vec<String> = planes
            .iter()
            .filter(|(_, c)| **c as f64 >= share * most as f64)
            .map(|((l, n), _)| format!("reference {} ({}) at h {} mm", l.name(), net_name(cx, *n), num(geo.height(it.layer, *l))))
            .collect();
        if !found_any && under.is_empty() {
            if no_plane.insert(it.layer) {
                cx.not_applicable(REFERENCE, format!("geometry: a ground plane (pour) under the RF copper on {}", it.layer.name()));
            }
            continue;
        }
        for ((layer, n), c) in &planes {
            if roles.grounds.contains(n) {
                continue;
            }
            let pours: Vec<u64> = geo
                .plane(*layer)
                .map(|pl| pl.fills.iter().filter(|(_, fnet)| fnet == n).filter_map(|(f, _)| nets.fills.get(*f)).map(|f| f.pour).collect())
                .unwrap_or_default();
            if (*c as f64) < share * most as f64 {
                // Under part of it only: other copper under it.
                let obj = pours.first().map_or(DrcItem::Pour(0), |p| DrcItem::Pour(*p));
                let e = under.entry(*n).or_insert_with(|| (Vec::new(), obj));
                if !e.0.contains(layer) {
                    e.0.push(*layer);
                }
                continue;
            }
            let plane = net_name(cx, *n);
            // A track's points are at most a step apart: how much of it the plane is under.
            let spacing = if matches!(it.item, DrcItem::Track(_)) { STEP.max(it.radius) } else { f64::INFINITY };
            let length = *c as f64 * spacing;
            let short = length < min_length;
            let mut keys = vec![it.item.key(), "power".to_string(), layer.name()];
            keys.extend(pours.iter().map(|p| DrcItem::Pour(*p).key()));
            let regime = format!("reference {} ({plane}) at h {} mm", layer.name(), num(geo.height(it.layer, *layer)));
            let layer = *layer;
            cx.add(REFERENCE, &keys, || {
                let mut d = Draft::new(
                    at,
                    vec![it.item.clone()],
                    if short {
                        format!("{} ({name}, {}) has the {plane} plane on {} under only about {} mm of it off its own antipads: too little to be its reference; check the via transition", it.item.reference(), it.layer.name(), layer.name(), num((length * 100.0).round() / 100.0))
                    } else {
                        format!("{} ({name}, {}) references the {plane} plane on {}: RF copper needs ground as its reference", it.item.reference(), it.layer.name(), layer.name())
                    },
                );
                if short {
                    d = d.severity(Severity::Info).class(Class::Practical).review().arg("length", num((length * 100.0).round() / 100.0));
                }
                d.layer(it.layer)
                .nets(vec![name.clone(), plane.clone()])
                .variant(if short { "power_short" } else { "power" })
                .regime(regime)
                .arg("net", name.clone())
                .arg("plane", plane.clone())
                .arg("under", layer.name())
                .fix("ecad.board.net.inspect", json!({"nets": [name], "metrics": ["reference"]}), format!("route {name} with a ground plane next to its layer, or make the {} plane ground under it", layer.name()))
            });
        }
        for (n, (layers, obj)) in under {
            let other = net_name(cx, n);
            let on = layers.iter().map(|l| l.name()).collect::<Vec<_>>().join(", ");
            let regime = if reference.is_empty() { "no plane under most of it".to_string() } else { reference.join("; ") };
            cx.add(REFERENCE, &[it.item.key(), other.clone()], || {
                Draft::new(
                    at,
                    vec![it.item.clone(), obj.clone()],
                    format!(
                        "{} of {other} on {on} lies under {} of RF net {name} ({}): keep the layers under RF copper clear of other nets",
                        obj.reference(),
                        it.item.reference(),
                        it.layer.name()
                    ),
                )
                .layer(it.layer)
                .nets(vec![name.clone(), other.clone()])
                .regime(regime)
                .arg("net", name.clone())
                .arg("other", other.clone())
                .arg("under", on.clone())
                .fix(
                    "ecad.board.net.inspect",
                    json!({"nets": [other], "metrics": ["layers"]}),
                    format!("move {other}'s copper out from under {name} on {on}"),
                )
            });
        }
    }
}

/// Where along a track a point lies: the distance along it to the nearest point, and how far to
/// the side (positive on its left).
fn locate_on(board: &super::super::board::Board, ti: usize, p: [f64; 2]) -> Option<(f64, f64)> {
    let t = board.tracks.get(ti)?;
    let mut walked = 0.0;
    let mut best: Option<(f64, f64, f64)> = None;
    for (a, b) in track_pieces(&t.points, &t.curves) {
        let len = distance(a, b);
        if len <= 1e-9 {
            continue;
        }
        let (q, s) = path::project(p, a, b, 0.0);
        let d = distance(p, q);
        let side = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        if best.is_none_or(|(_, _, bd)| d < bd) {
            best = Some((walked + s * len, if side >= 0.0 { d } else { -d }, d));
        }
        walked += len;
    }
    best.map(|(t, off, _)| (t, off))
}

/// The turn between two directions, in degrees: 0 straight on, 180 straight back; 0 when either
/// has no length.
fn turn_deg(u: [f64; 2], v: [f64; 2]) -> f64 {
    let (lu, lv) = (u[0].hypot(u[1]), v[0].hypot(v[1]));
    if lu <= 1e-9 || lv <= 1e-9 {
        return 0.0;
    }
    ((u[0] * v[0] + u[1] * v[1]) / (lu * lv)).clamp(-1.0, 1.0).acos().to_degrees()
}

fn fences(cx: &mut Cx, lines: &[Line], zones: &[Vec<[f64; 2]>]) {
    let (board, nets, roles) = (cx.board, cx.nets, cx.roles);
    let p = |name: &str| super::param_value(board, FENCE, name);
    let (fraction, harmonic, corner, side_h, row) = (p("fraction"), p("harmonic"), p("corner"), p("side"), p("row"));
    let layers = board.copper_layers;
    let f_concern = roles.intent.board.f_concern_mhz.filter(|f| f.is_finite() && *f > 0.0);
    // Footprints of 9 pads or more (ICs, modules): their pins' fences are their makers'.
    let mut pad_count: BTreeMap<u64, usize> = BTreeMap::new();
    for q in &nets.pads {
        *pad_count.entry(q.footprint).or_default() += 1;
    }
    let big: Vec<[[f64; 2]; 2]> =
        footprints(cx).into_values().filter(|(f, _)| pad_count.get(&f.id).is_some_and(|c| *c >= 9)).map(|(_, y)| y).collect();
    let mut no_freq: BTreeSet<usize> = BTreeSet::new();
    for l in lines {
        let Some(h) = l.h() else { continue };
        let within = side_h * h;
        let sides: Vec<(usize, f64)> =
            (0..2).map(|k| l.side_ground(k, within)).enumerate().filter(|(_, (share, _))| *share >= 0.5).map(|(k, (_, g))| (k, g)).collect();
        if sides.is_empty() {
            continue;
        }
        let Some(info) = roles.net(l.net) else { continue };
        let (f, f_says, assumed) = match (f_concern, carrier(cx, info)) {
            (Some(f), _) => (f, format!("the board's frequency of concern, {} MHz", num(f)), false),
            (None, Some((c, says, assumed))) => (harmonic * c, format!("{} × {says}", num(harmonic)), assumed),
            (None, None) => {
                if no_freq.insert(l.net) {
                    cx.not_applicable(FENCE, format!("intent: f_mhz of {}, the board's f_concern_mhz or its radio bands", info.name));
                }
                continue;
            }
        };
        let lambda = C_MM_MHZ / f / l.er().sqrt();
        let limit = fraction * lambda;
        let name = info.name.to_string();
        // The ground vias beside it: where along it, and which side.
        if !cx.spend(board.vias.len().saturating_add(1)) {
            return;
        }
        let Some(tbox) = board.tracks.get(l.ti).and_then(|t| path::bbox(&t.points, &t.curves, false)) else { continue };
        let reach = l.width / 2.0 + l.reach + row + corner;
        let mut fence: Vec<(f64, f64, [f64; 2])> = Vec::new();
        let mut ground_vias: Vec<[f64; 2]> = Vec::new();
        for (vi, v) in board.vias.iter().enumerate() {
            if !nets.via_nets.get(vi).copied().flatten().is_some_and(|n| roles.grounds.contains(&n)) || !v.on(l.layer, layers) {
                continue;
            }
            if !boxes_meet(grow(tbox, reach), [v.at, v.at]) {
                continue;
            }
            ground_vias.push(v.at);
            if let Some((t, off)) = locate_on(board, l.ti, v.at) {
                fence.push((t, off, v.at));
            }
        }
        // The pitch along each side with ground beside it: from each via to the next along the
        // line, as far apart as they are (round a bend the inner row's vias are farther apart
        // along the line than along the ground's edge).
        let mut worst: Option<(f64, [f64; 2])> = None;
        for (k, g) in &sides {
            let sign = if *k == 0 { 1.0 } else { -1.0 };
            let mut row_vias: Vec<(f64, [f64; 2])> = fence
                .iter()
                .filter(|(t, off, _)| {
                    let o = off * sign;
                    o >= l.width / 2.0 && o <= l.width / 2.0 + g + row && *t >= -corner && *t <= l.length + corner
                })
                .map(|(t, _, at)| (*t, *at))
                .collect();
            row_vias.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut gaps: Vec<(f64, [f64; 2])> = row_vias
                .windows(2)
                .filter_map(|w| Some((w.first()?.1, w.get(1)?.1)))
                .map(|(a, b)| (distance(a, b), [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]))
                .collect();
            // The ground beside it on this side before its first via and past its last.
            let run: Vec<f64> = l
                .samples
                .iter()
                .zip(&l.sides)
                .filter(|(_, hits)| matches!(hits.get(*k), Some(Some(Hit::Ground(d, _, _))) if *d <= within))
                .map(|(s, _)| s.t)
                .collect();
            let (Some(first), Some(last)) = (run.iter().copied().reduce(f64::min), run.iter().copied().reduce(f64::max)) else { continue };
            match (row_vias.first(), row_vias.last()) {
                (Some((a, _)), Some((b, _))) => {
                    if *a > first {
                        gaps.push((a - first, l.point(board, (first + a) / 2.0)));
                    }
                    if last > *b {
                        gaps.push((last - b, l.point(board, (last + b) / 2.0)));
                    }
                }
                _ => gaps.push((last - first, l.point(board, (first + last) / 2.0))),
            }
            if let Some((pitch, mid)) = gaps.into_iter().max_by(|a, b| a.0.total_cmp(&b.0))
                && pitch > limit
                && worst.is_none_or(|(w, _)| pitch > w)
            {
                worst = Some((pitch, mid));
            }
        }
        let regime = format!(
            "{f_says}; εr {}, λg {} mm{}",
            num((l.er() * 100.0).round() / 100.0),
            num(lambda.round()),
            if assumed { "; frequency assumed" } else { "" }
        );
        let g_net = l.ground().or_else(|| roles.grounds.first().copied()).map(|g| net_name(cx, g)).unwrap_or_default();
        if let Some((pitch, at)) = worst {
            let regime = regime.clone();
            let g_net = g_net.clone();
            cx.add(FENCE, &[DrcItem::Track(l.id).key(), "pitch".into()], || {
                Draft::new(
                    at,
                    vec![DrcItem::Track(l.id)],
                    format!(
                        "the ground beside RF track {} ({name}) has vias up to {} mm apart; λg/{} at {} MHz is {} mm (a vendor note's 1.0–1.3 mm is an unverified reference)",
                        l.id,
                        num(pitch),
                        num((1.0 / fraction).round()),
                        num(f.round()),
                        num(limit)
                    ),
                )
                .layer(l.layer)
                .nets(vec![name.clone()])
                .measure(pitch, limit, "mm", Bound::Max)
                .param("fraction")
                .regime(regime)
                .arg("net", name.clone())
                .arg("f_mhz", num(f.round()))
                .arg("over", num((1.0 / fraction).round()))
                .fix("ecad.board.stitch", json!({"net": g_net, "pattern": "fence", "tracks": [l.id]}), "fence the line: vias along both sides at the pitch, and at its bends, pads and ends")
            });
        }
        // A via near each bend, end and pad.
        let Some(t) = board.tracks.get(l.ti) else { continue };
        let line = path::tessellate(&t.points, &t.curves, false, 0.01);
        // Each spot, and the pad it is on: a via near the pad's edge serves it.
        let mut spots: Vec<([f64; 2], Option<Core>)> = Vec::new();
        // An end is a spot unless it carries on straight into another track of the line.
        let end_spot = |end: Option<&[f64; 2]>, next: Option<&[f64; 2]>| -> Option<([f64; 2], Option<Core>)> {
            let (e, q) = (*end?, *next?);
            let out = [e[0] - q[0], e[1] - q[1]];
            let straight = lines.iter().filter(|o| o.ti != l.ti && o.net == l.net && o.layer == l.layer).any(|o| {
                let Some(ot) = board.tracks.get(o.ti) else { return false };
                let pts = &ot.points;
                [(pts.first(), pts.get(1)), (pts.last(), pts.len().checked_sub(2).and_then(|i| pts.get(i)))]
                    .into_iter()
                    .filter_map(|(a, b)| Some((*a?, *b?)))
                    .any(|(a, b)| distance(a, e) <= 0.01 && turn_deg(out, [b[0] - a[0], b[1] - a[1]]) <= BEND_DEG)
            });
            let pad = nets.pads.iter().find(|q| q.on(l.layer) && q.contains(e)).map(pad_core);
            (pad.is_some() || !straight).then_some((e, pad))
        };
        spots.extend(end_spot(line.first(), line.get(1)));
        for w in line.windows(3) {
            let (Some(a), Some(m), Some(c)) = (w.first(), w.get(1), w.get(2)) else { continue };
            let (u, v) = ([m[0] - a[0], m[1] - a[1]], [c[0] - m[0], c[1] - m[1]]);
            if turn_deg(u, v) > BEND_DEG {
                // A jog inside a pad is the pad's.
                spots.push((*m, nets.pads.iter().find(|q| q.on(l.layer) && q.contains(*m)).map(pad_core)));
            }
        }
        spots.extend(end_spot(line.last(), line.len().checked_sub(2).and_then(|i| line.get(i))));
        // Spots closer together than half the reach (an arc's many corners) count once.
        let mut bare: Vec<[f64; 2]> = Vec::new();
        for (s, pad) in spots {
            let served = |v: &[f64; 2]| distance(*v, s) <= corner || pad.as_ref().is_some_and(|c| gap(c, &disc(*v, 0.0)) <= corner);
            if in_zone(zones, s) || big.iter().any(|y| in_box(*y, s)) || ground_vias.iter().any(served) {
                continue;
            }
            if !bare.iter().any(|b| distance(*b, s) <= corner / 2.0) {
                bare.push(s);
            }
        }
        if let Some(first) = bare.first().copied() {
            let count = bare.len();
            cx.add(FENCE, &[DrcItem::Track(l.id).key(), "points".into()], || {
                Draft::new(
                    first,
                    vec![DrcItem::Track(l.id)],
                    format!(
                        "{count} bend{}, end{} or pad{} of RF track {} ({name}) with ground beside it {} no ground via within {} mm",
                        if count == 1 { "" } else { "s" },
                        if count == 1 { "" } else { "s" },
                        if count == 1 { "" } else { "s" },
                        l.id,
                        if count == 1 { "has" } else { "have" },
                        num(corner)
                    ),
                )
                .layer(l.layer)
                .nets(vec![name.clone()])
                .variant("points")
                .measure(count as f64, 0.0, "places", Bound::Max)
                .param("")
                .regime(regime)
                .arg("net", name.clone())
                .arg("count", count.to_string())
                .arg("reach", num(corner))
                .fix(
                    "ecad.board.stitch",
                    json!({"net": g_net, "pattern": "fence", "tracks": [l.id]}),
                    "fence the line: it puts a ground via beside every bend, tee, pad and end",
                )
            });
        }
    }
}
