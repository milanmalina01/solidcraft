//! Copper pours and keepouts: areas of the board drawn as closed outlines (corners and edge
//! sweeps, as the board outline). A pour fills its area on one copper layer with copper of its
//! net, round everything else (see [`super::fill`]); only the outline and its settings are saved,
//! never the fill. A keepout is an area where tracks, vias, pours or components are not
//! allowed, on some copper layers or all of them.

use serde::{Deserialize, Serialize};

use super::layers::CopperLayer;
use super::rules::check_net_name;

/// The most pours and keepouts a board holds (each).
pub const MAX_POURS: usize = 512;
pub const MAX_KEEPOUTS: usize = 512;
/// The thinnest copper a pour keeps when none is given, in millimetres.
pub const DEFAULT_MIN_WIDTH: f64 = 0.2;
/// The largest pour size (clearance, widths, gaps), in millimetres.
pub const MAX_POUR_SIZE: f64 = 10.0;
/// The highest priority.
pub const MAX_PRIORITY: u32 = 1000;
/// The most hatch lines a pour has each way.
pub const MAX_HATCH_LINES: f64 = 4096.0;

/// How a pour joins the pads of its net.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Connect {
    /// Round each pad a gap, crossed by spokes: the pad heats up for soldering.
    #[default]
    Thermal,
    /// Straight into the pad.
    Solid,
    /// Not at all: the pads are cleared like another net's.
    None,
}

/// A thermal connection's spokes and the gap round the pad, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Thermal {
    pub spoke_width: f64,
    pub gap: f64,
    /// 2 (along the pad's width) or 4.
    pub spokes: u8,
}

impl Default for Thermal {
    fn default() -> Self {
        Thermal { spoke_width: 0.3, gap: 0.3, spokes: 4 }
    }
}

impl Thermal {
    pub fn is_default(&self) -> bool {
        *self == Thermal::default()
    }
}

/// A hatched fill: lines `width` wide every `pitch` both ways, and a border as wide.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hatch {
    pub width: f64,
    pub pitch: f64,
}

fn default_min_width() -> f64 {
    DEFAULT_MIN_WIDTH
}

fn is_default_min_width(v: &f64) -> bool {
    *v == DEFAULT_MIN_WIDTH
}

fn is_thermal(c: &Connect) -> bool {
    *c == Connect::Thermal
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// A copper pour (see the module notes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pour {
    pub id: u64,
    pub layer: CopperLayer,
    /// The schematic net it is on, by name; without one it joins nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
    /// Its outline's corners, closed back to the first.
    pub outline: Vec<[f64; 2]>,
    /// Each edge's sweep in degrees, the closing edge last; empty while all are straight.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<f64>,
    /// The gap to other nets' copper, in millimetres; without one, its net class's. Set, it takes
    /// the place of its net class's clearance for the fill (the design rule check holds the fill
    /// to it too): the larger of this and the other copper's class clearance applies, and against
    /// another pour's fill the larger of the two pours'.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance: Option<f64>,
    /// The thinnest copper the fill keeps: narrower necks and slivers go.
    #[serde(default = "default_min_width", skip_serializing_if = "is_default_min_width")]
    pub min_width: f64,
    #[serde(default, skip_serializing_if = "is_thermal")]
    pub connect: Connect,
    #[serde(default, skip_serializing_if = "Thermal::is_default")]
    pub thermal: Thermal,
    /// Where pours on one layer overlap, the higher priority fills first and the other keeps
    /// clear of it (equal priorities: the older pour first).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub priority: u32,
    /// Fill islands that touch no copper of the pour's net go.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub remove_islands: bool,
    /// A hatched fill in place of solid copper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hatch: Option<Hatch>,
}

/// What a keepout forbids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Forbid {
    pub tracks: bool,
    pub vias: bool,
    pub pours: bool,
    /// Footprints, by their courtyard.
    pub components: bool,
}

impl Default for Forbid {
    fn default() -> Self {
        Forbid { tracks: true, vias: true, pours: true, components: true }
    }
}

impl Forbid {
    pub fn is_all(&self) -> bool {
        *self == Forbid::default()
    }
}

/// A keepout (see the module notes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keepout {
    pub id: u64,
    /// The copper layers it keeps clear; empty is every layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<CopperLayer>,
    pub outline: Vec<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<f64>,
    #[serde(default, skip_serializing_if = "Forbid::is_all")]
    pub forbid: Forbid,
    /// The board marking whose layer mark it clears pours round (see [`super::marking`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marking: Option<u64>,
}

impl Keepout {
    /// Whether it covers copper layer `layer`.
    pub fn on(&self, layer: CopperLayer) -> bool {
        self.layers.is_empty() || self.layers.contains(&layer)
    }
}

/// A pour size: finite, more than 0 (or 0 too, unless `open`) and at most [`MAX_POUR_SIZE`].
fn size_ok(v: f64, open: bool) -> bool {
    let low = if open { v > 0.0 } else { v >= 0.0 };
    v.is_finite() && low && v <= MAX_POUR_SIZE
}

/// A pour setting out of range (see [`Pour::check_settings`]), in English through `Display`, and
/// as a message key for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadSetting {
    Clearance,
    MinWidth,
    SpokeWidth,
    ThermalGap,
    Spokes,
    Priority,
    HatchWidth,
    HatchPitch,
    /// Too many hatch lines across the pour.
    HatchLines,
}

impl BadSetting {
    /// The UI's message key (its arguments are `max`, see [`BadSetting::max`]).
    pub fn key(self) -> &'static str {
        match self {
            BadSetting::Clearance => "electronics.board_pour_bad_clearance",
            BadSetting::MinWidth => "electronics.board_pour_bad_min_width",
            BadSetting::SpokeWidth => "electronics.board_pour_bad_spoke_width",
            BadSetting::ThermalGap => "electronics.board_pour_bad_thermal_gap",
            BadSetting::Spokes => "electronics.board_pour_bad_spokes",
            BadSetting::Priority => "electronics.board_pour_bad_priority",
            BadSetting::HatchWidth => "electronics.board_pour_bad_hatch_width",
            BadSetting::HatchPitch => "electronics.board_pour_bad_hatch_pitch",
            BadSetting::HatchLines => "electronics.board_pour_bad_hatch_lines",
        }
    }

    /// The most the setting may be (lines, for the hatch's lines; nothing for the spokes).
    pub fn max(self) -> f64 {
        match self {
            BadSetting::Spokes => 4.0,
            BadSetting::Priority => f64::from(MAX_PRIORITY),
            BadSetting::HatchPitch => 10.0 * MAX_POUR_SIZE,
            BadSetting::HatchLines => MAX_HATCH_LINES,
            _ => MAX_POUR_SIZE,
        }
    }
}

impl std::fmt::Display for BadSetting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let max = self.max();
        match self {
            BadSetting::Clearance => write!(f, "`clearance` must be 0 to {max} mm"),
            BadSetting::MinWidth => write!(f, "`min_width` must be 0 to {max} mm"),
            BadSetting::SpokeWidth => write!(f, "the thermal `spoke_width` must be more than 0 to {max} mm"),
            BadSetting::ThermalGap => write!(f, "the thermal `gap` must be more than 0 to {max} mm"),
            BadSetting::Spokes => write!(f, "a thermal connection has 2 or 4 `spokes`"),
            BadSetting::Priority => write!(f, "`priority` must be 0 to {max}"),
            BadSetting::HatchWidth => write!(f, "the hatch `width` must be more than 0 to {max} mm"),
            BadSetting::HatchPitch => write!(f, "the hatch `pitch` must be more than its width and at most {max} mm"),
            BadSetting::HatchLines => write!(f, "the hatch would have more than {max} lines across the pour: make its `pitch` wider"),
        }
    }
}

impl Pour {
    /// Its settings in range (the outline and layer are the board's to check): a net name's
    /// form, sizes, 2 or 4 spokes, the priority, a hatch's pitch wider than its lines and no
    /// more than [`MAX_HATCH_LINES`] lines across the outline's box (`span`, its larger side).
    pub fn check_settings(&self, span: f64) -> Result<(), String> {
        if let Some(n) = &self.net {
            check_net_name(n)?;
        }
        match self.bad_setting(span) {
            Some(b) => Err(b.to_string()),
            None => Ok(()),
        }
    }

    /// The first of its settings out of range (as [`Pour::check_settings`], the net's name
    /// apart).
    pub fn bad_setting(&self, span: f64) -> Option<BadSetting> {
        if self.clearance.is_some_and(|c| !size_ok(c, false)) {
            return Some(BadSetting::Clearance);
        }
        if !size_ok(self.min_width, false) {
            return Some(BadSetting::MinWidth);
        }
        if !size_ok(self.thermal.spoke_width, true) {
            return Some(BadSetting::SpokeWidth);
        }
        if !size_ok(self.thermal.gap, true) {
            return Some(BadSetting::ThermalGap);
        }
        if !matches!(self.thermal.spokes, 2 | 4) {
            return Some(BadSetting::Spokes);
        }
        if self.priority > MAX_PRIORITY {
            return Some(BadSetting::Priority);
        }
        if let Some(h) = self.hatch {
            if !size_ok(h.width, true) {
                return Some(BadSetting::HatchWidth);
            }
            if !(h.pitch.is_finite() && h.pitch > h.width && h.pitch <= 10.0 * MAX_POUR_SIZE) {
                return Some(BadSetting::HatchPitch);
            }
            if span / h.pitch > MAX_HATCH_LINES {
                return Some(BadSetting::HatchLines);
            }
        }
        None
    }
}
