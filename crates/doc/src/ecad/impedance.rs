//! Characteristic impedance of a track over the board's stack-up, from published closed-form
//! approximations, and the width that gives an impedance (by bisection: every formula here falls
//! as the track widens). The reference plane is the copper layer next to the track (both
//! neighbours for a stripline), and solder mask is not counted.
//!
//! - Microstrip (an outer layer over the plane under it), IPC-2141:
//!   Z0 = 87 / √(εr + 1.41) · ln(5.98 h / (0.8 w + t)); in its range for 0.1 < w/h < 2 and
//!   1 < εr < 15. The effective εr given with it is Hammerstad's,
//!   (εr + 1)/2 + (εr − 1)/2 · (1 + 12 h/w)^−½.
//! - Stripline (an inner layer between the planes above and below it), IPC-2141 for a centred
//!   track: Z0 = 60 / √εr · ln(4 b / (0.67 π (0.8 w + t))), b the distance between the planes; in
//!   its range for w/(b − t) < 0.35 and t/b < 0.25. A track off the centre (dielectrics of
//!   thickness h1 above and h2 below) is taken as the harmonic mean of the centred striplines with
//!   b = 2 h1 + t and b = 2 h2 + t, which is the centred formula when h1 = h2; εr is the two
//!   dielectrics' average weighted by thickness. Once the track is so wide that either centred
//!   formula goes to 0 or below (the thin side first), the mean means nothing and there is no
//!   impedance.
//! - Coplanar waveguide with ground under it (an outer layer, a gap s to the copper on both
//!   sides), Wadell's conductor-backed formula with complete elliptic integrals K (worked out
//!   exactly by the arithmetic-geometric mean): k = w/(w + 2s), k3 = tanh(πw/4h)/tanh(π(w + 2s)/4h),
//!   q = K(k')/K(k) · K(k3)/K(k3'), εeff = (1 + εr q)/(1 + q),
//!   Z0 = 60π/√εeff / (K(k)/K(k') + K(k3)/K(k3')). The formula is worked out for copper of no
//!   thickness and publishes no range beyond that. Copper as thick as the gap is a good part of
//!   narrows the gap the field sees and lowers Z0, so this bound is our own, not a published
//!   one: in range while t ≤ 0.4 s (1 oz copper beside a gap of 0.1 mm or more, the usual
//!   coplanar lines); past it the Z0 given is a few ohms high or more.
//! - Edge-coupled differential pair, gap s between the two tracks, IPC-2141: on an outer layer
//!   Zdiff = 2 Z0 (1 − 0.48 e^(−0.96 s/h)) with Z0 the microstrip's; on an inner layer
//!   Zdiff = 2 Z0 (1 − 0.347 e^(−2.9 s/b)) with Z0 the stripline's; in range as that Z0.
//!
//! These are estimates good to a few per cent inside their ranges; a fab's field solver has the
//! last word.

use std::f64::consts::PI;

use super::layers::CopperLayer;
use super::stackup::{StackLayer, surroundings};

/// The narrowest and widest track the width search tries, in millimetres.
pub const WIDTH: std::ops::RangeInclusive<f64> = 0.01..=20.0;
/// The impedances a target may be, in ohms.
pub const TARGET: std::ops::RangeInclusive<f64> = 1.0..=500.0;

/// What kind of line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Microstrip,
    Stripline,
    Coplanar,
    Differential,
}

/// What a copper layer is made of and sits between, for the formulas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Medium {
    /// Top or bottom: a plane on one side only.
    pub outer: bool,
    /// The dielectric between the layer and its plane, in millimetres; for a stripline, `h1`
    /// above and `h2` below (`h2` is 0 on an outer layer).
    pub h1: f64,
    pub h2: f64,
    /// The copper's thickness.
    pub t: f64,
    pub er: f64,
}

/// Why the calculator gives no answer: in English through `Display`, and as a message key with
/// named arguments for the UI.
#[derive(Clone, Debug, PartialEq)]
pub enum ImpedanceError {
    /// The stack-up has no such copper layer.
    NoLayer(CopperLayer),
    /// The copper layer has no dielectric on either side.
    NoDielectric(CopperLayer),
    /// The kind of line doesn't run on this layer: a microstrip or a coplanar line on an inner
    /// layer, a stripline on an outer one.
    WrongLayer(LineKind),
    /// No dielectric, copper or εr to a plane.
    NoPlane,
    /// A coplanar line or a pair without a gap in range.
    Gap,
    /// The width out of [`WIDTH`].
    Width,
    /// The target out of [`TARGET`].
    Target,
    /// No impedance for a track that wide.
    TooWide,
    /// No impedance over this dielectric for any width.
    NoImpedance,
    /// The target is outside what the narrowest track (`low_width`, the highest impedance) to the
    /// widest one with an impedance (`high_width`) give.
    OutOfReach { target: f64, low_width: f64, low: f64, high_width: f64, high: f64 },
}

impl ImpedanceError {
    /// The UI's message key.
    pub fn key(&self) -> &'static str {
        match self {
            ImpedanceError::NoLayer(_) => "electronics.impedance_err_no_layer",
            ImpedanceError::NoDielectric(_) => "electronics.impedance_err_no_dielectric",
            ImpedanceError::WrongLayer(LineKind::Microstrip) => "electronics.impedance_err_microstrip_inner",
            ImpedanceError::WrongLayer(LineKind::Coplanar) => "electronics.impedance_err_coplanar_inner",
            ImpedanceError::WrongLayer(_) => "electronics.impedance_err_stripline_outer",
            ImpedanceError::NoPlane => "electronics.impedance_err_no_plane",
            ImpedanceError::Gap => "electronics.impedance_err_gap",
            ImpedanceError::Width => "electronics.impedance_err_width",
            ImpedanceError::Target => "electronics.impedance_err_target",
            ImpedanceError::TooWide => "electronics.impedance_err_too_wide",
            ImpedanceError::NoImpedance => "electronics.impedance_err_none",
            ImpedanceError::OutOfReach { .. } => "electronics.impedance_err_out_of_reach",
        }
    }

    /// The message's arguments by name (a layer as the stack-up names it; the UI may word it
    /// from [`ImpedanceError::layer`]).
    pub fn args(&self) -> Vec<(&'static str, String)> {
        let n = |v: f64| format!("{v}");
        match self {
            ImpedanceError::NoLayer(l) | ImpedanceError::NoDielectric(l) => vec![("layer", l.name())],
            ImpedanceError::Gap => vec![("max", n(*WIDTH.end()))],
            ImpedanceError::Width => vec![("min", n(*WIDTH.start())), ("max", n(*WIDTH.end()))],
            ImpedanceError::Target => vec![("min", n(*TARGET.start())), ("max", n(*TARGET.end()))],
            ImpedanceError::OutOfReach { target, low_width, low, high_width, high } => vec![
                ("target", n(*target)),
                ("low", format!("{low:.1}")),
                ("low_width", n(*low_width)),
                ("high", format!("{high:.1}")),
                ("high_width", format!("{high_width:.3}")),
            ],
            _ => Vec::new(),
        }
    }

    /// The layer the message names, if any.
    pub fn layer(&self) -> Option<CopperLayer> {
        match self {
            ImpedanceError::NoLayer(l) | ImpedanceError::NoDielectric(l) => Some(*l),
            _ => None,
        }
    }
}

impl std::fmt::Display for ImpedanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImpedanceError::NoLayer(l) => write!(f, "the stack-up has no copper layer `{}`", l.name()),
            ImpedanceError::NoDielectric(l) => write!(f, "copper layer `{}` has no dielectric next to it", l.name()),
            ImpedanceError::WrongLayer(LineKind::Microstrip) => write!(f, "an inner layer has planes on both sides: use `stripline` there"),
            ImpedanceError::WrongLayer(LineKind::Coplanar) => {
                write!(f, "the coplanar waveguide here runs on an outer layer: use `stripline` on an inner one")
            }
            ImpedanceError::WrongLayer(_) => write!(f, "a stripline runs on an inner layer, between two planes: use `microstrip` on an outer one"),
            ImpedanceError::NoPlane => write!(f, "the stack-up gives no dielectric to a plane here"),
            ImpedanceError::Gap => write!(f, "give the `gap` (more than 0 and at most {} mm)", WIDTH.end()),
            ImpedanceError::Width => write!(f, "`width` must be {} to {} mm", WIDTH.start(), WIDTH.end()),
            ImpedanceError::Target => write!(f, "`target` must be {} to {} Ω", TARGET.start(), TARGET.end()),
            ImpedanceError::TooWide => write!(f, "the formula gives no impedance for a track that wide over that dielectric: make it narrower"),
            ImpedanceError::NoImpedance => write!(f, "the formula gives no impedance over this dielectric"),
            ImpedanceError::OutOfReach { target, low_width, low, high_width, high } => {
                write!(f, "{target} Ω is out of reach here: {low:.1} Ω (at {low_width} mm) to {high:.1} Ω (at {high_width:.3} mm)")
            }
        }
    }
}

impl From<ImpedanceError> for String {
    fn from(e: ImpedanceError) -> String {
        e.to_string()
    }
}

/// What a formula gives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Impedance {
    /// Ohms: single-ended, or differential for a pair.
    pub z0: f64,
    /// The effective εr, where the formula gives one.
    pub er_eff: Option<f64>,
    /// Inside the formula's published range (see the module notes).
    pub valid_range: bool,
    /// The first input outside it, when one is.
    pub out_of_range: Option<OutOfRange>,
    /// Which formula.
    pub formula: &'static str,
}

/// An input (a ratio of sizes, or εr) outside the range a formula holds in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutOfRange {
    /// Which: `w/h`, `er`, `w/(b-t)`, `t/b` or `t/s` (w the width, h the dielectric to the plane,
    /// b the distance between the planes, t the copper's thickness, s the gap).
    pub input: &'static str,
    pub value: f64,
    /// Where it holds: above `low` and up to `high` (either open when `None`).
    pub low: Option<f64>,
    pub high: Option<f64>,
}

/// The first of `tests` (input, value, low, high: in range for low < value < high) the value
/// is outside of.
fn first_out(tests: &[(&'static str, f64, Option<f64>, Option<f64>)]) -> Option<OutOfRange> {
    tests
        .iter()
        .find(|(_, v, lo, hi)| !(v.is_finite() && lo.is_none_or(|l| *v > l) && hi.is_none_or(|h| *v < h)))
        .map(|(input, value, low, high)| OutOfRange { input, value: *value, low: *low, high: *high })
}

/// What the stack-up `layers` (of a board with `copper_layers`) puts round `layer`; why not
/// when the layer isn't there or has no dielectric to a plane.
pub fn medium(layers: &[StackLayer], layer: CopperLayer, copper_layers: u8) -> Result<Medium, ImpedanceError> {
    let s = surroundings(layers, layer, copper_layers).ok_or(ImpedanceError::NoLayer(layer))?;
    let t = s.copper.thickness;
    match (s.above, s.below) {
        (None, Some(d)) | (Some(d), None) => Ok(Medium { outer: true, h1: d.thickness, h2: 0.0, t, er: d.er() }),
        (Some(a), Some(b)) => {
            let er = (a.er() * a.thickness + b.er() * b.thickness) / (a.thickness + b.thickness);
            Ok(Medium { outer: false, h1: a.thickness, h2: b.thickness, t, er })
        }
        (None, None) => Err(ImpedanceError::NoDielectric(layer)),
    }
}

fn microstrip(m: &Medium, w: f64) -> Impedance {
    let (h, t, er) = (m.h1, m.t, m.er);
    let z0 = 87.0 / (er + 1.41).sqrt() * (5.98 * h / (0.8 * w + t)).ln();
    let er_eff = (er + 1.0) / 2.0 + (er - 1.0) / 2.0 / (1.0 + 12.0 * h / w).sqrt();
    let out = first_out(&[("w/h", w / h, Some(0.1), Some(2.0)), ("er", er, Some(1.0), Some(15.0))]);
    Impedance { z0, er_eff: Some(er_eff), valid_range: out.is_none(), out_of_range: out, formula: "IPC-2141 microstrip" }
}

fn centred_stripline(b: f64, w: f64, t: f64, er: f64) -> f64 {
    60.0 / er.sqrt() * (4.0 * b / (0.67 * PI * (0.8 * w + t))).ln()
}

fn stripline(m: &Medium, w: f64) -> Impedance {
    let t = m.t;
    let b = m.h1 + m.h2 + t;
    let z1 = centred_stripline(2.0 * m.h1 + t, w, t, m.er);
    let z2 = centred_stripline(2.0 * m.h2 + t, w, t, m.er);
    // Past where either side's formula gives nothing, neither does their mean.
    let z0 = if z1 > 0.0 && z2 > 0.0 { 2.0 * z1 * z2 / (z1 + z2) } else { f64::NAN };
    let out = if b > t {
        first_out(&[("w/(b-t)", w / (b - t), None, Some(0.35)), ("t/b", t / b, None, Some(0.25))])
    } else {
        first_out(&[("t/b", t / b, None, Some(1.0))])
    };
    Impedance { z0, er_eff: Some(m.er), valid_range: out.is_none(), out_of_range: out, formula: "IPC-2141 stripline" }
}

/// The complete elliptic integral of the first kind K(k), by the arithmetic-geometric mean.
fn ellip_k(k: f64) -> f64 {
    let (mut a, mut g) = (1.0_f64, (1.0 - k * k).max(0.0).sqrt());
    for _ in 0..64 {
        if (a - g).abs() <= 1e-15 * a {
            break;
        }
        (a, g) = ((a + g) / 2.0, (a * g).sqrt());
    }
    PI / (2.0 * a)
}

/// K(k)/K(k').
fn k_ratio(k: f64) -> f64 {
    ellip_k(k) / ellip_k((1.0 - k * k).max(0.0).sqrt())
}

fn coplanar(m: &Medium, w: f64, s: f64) -> Impedance {
    let (h, er) = (m.h1, m.er);
    let k = w / (w + 2.0 * s);
    let k3 = (PI * w / (4.0 * h)).tanh() / (PI * (w + 2.0 * s) / (4.0 * h)).tanh();
    let (r, r3) = (k_ratio(k), k_ratio(k3));
    let q = r3 / r;
    let er_eff = (1.0 + er * q) / (1.0 + q);
    let z0 = 60.0 * PI / er_eff.sqrt() / (r + r3);
    // In range while t ≤ 0.4 s (see the module notes).
    let out = if m.t <= 0.4 * s { None } else { first_out(&[("t/s", m.t / s, None, Some(0.4))]) };
    Impedance { z0, er_eff: Some(er_eff), valid_range: out.is_none(), out_of_range: out, formula: "Wadell conductor-backed coplanar waveguide" }
}

fn differential(m: &Medium, w: f64, s: f64) -> Impedance {
    if m.outer {
        let single = microstrip(m, w);
        let z0 = 2.0 * single.z0 * (1.0 - 0.48 * (-0.96 * s / m.h1).exp());
        Impedance { z0, formula: "IPC-2141 edge-coupled microstrip", ..single }
    } else {
        let single = stripline(m, w);
        let b = m.h1 + m.h2 + m.t;
        let z0 = 2.0 * single.z0 * (1.0 - 0.347 * (-2.9 * s / b).exp());
        Impedance { z0, formula: "IPC-2141 edge-coupled stripline", ..single }
    }
}

/// Why a kind of line doesn't fit the layer, or a gap is missing.
fn check(m: &Medium, kind: LineKind, gap: Option<f64>) -> Result<f64, ImpedanceError> {
    match kind {
        LineKind::Microstrip | LineKind::Coplanar if !m.outer => return Err(ImpedanceError::WrongLayer(kind)),
        LineKind::Stripline if m.outer => return Err(ImpedanceError::WrongLayer(kind)),
        _ => {}
    }
    if !(m.h1 > 0.0 && m.t > 0.0 && m.er >= 1.0) {
        return Err(ImpedanceError::NoPlane);
    }
    match (kind, gap) {
        (LineKind::Coplanar | LineKind::Differential, Some(g)) if g.is_finite() && g > 0.0 && g <= *WIDTH.end() => Ok(g),
        (LineKind::Coplanar | LineKind::Differential, _) => Err(ImpedanceError::Gap),
        _ => Ok(0.0),
    }
}

/// The impedance of a line of `kind`, `width` wide (and `gap`, for a coplanar line or a pair)
/// in `m`; why not when the kind doesn't fit the layer or a size is out of range.
pub fn impedance(m: &Medium, kind: LineKind, width: f64, gap: Option<f64>) -> Result<Impedance, ImpedanceError> {
    let s = check(m, kind, gap)?;
    if !WIDTH.contains(&width) {
        return Err(ImpedanceError::Width);
    }
    let z = at(m, kind, width, s);
    if !(z.z0.is_finite() && z.z0 > 0.0) {
        return Err(ImpedanceError::TooWide);
    }
    Ok(z)
}

fn at(m: &Medium, kind: LineKind, w: f64, s: f64) -> Impedance {
    match kind {
        LineKind::Microstrip => microstrip(m, w),
        LineKind::Stripline => stripline(m, w),
        LineKind::Coplanar => coplanar(m, w, s),
        LineKind::Differential => differential(m, w, s),
    }
}

/// The width that gives `target` ohms for a line of `kind` (with `gap`) in `m`, and what it
/// gives; why not when the target is out of reach between the narrowest and widest track.
pub fn width_for(m: &Medium, kind: LineKind, target: f64, gap: Option<f64>) -> Result<(f64, Impedance), ImpedanceError> {
    let s = check(m, kind, gap)?;
    if !TARGET.contains(&target) {
        return Err(ImpedanceError::Target);
    }
    let z = |w: f64| at(m, kind, w, s).z0;
    let mut lo = *WIDTH.start();
    let mut hi = *WIDTH.end();
    // The widest track the formula still gives a positive impedance for.
    for _ in 0..64 {
        let v = z(hi);
        if v.is_finite() && v > 0.0 {
            break;
        }
        hi = lo + (hi - lo) / 2.0;
    }
    let (z_lo, z_hi) = (z(lo), z(hi));
    if !(z_lo.is_finite() && z_hi.is_finite() && z_hi > 0.0) {
        return Err(ImpedanceError::NoImpedance);
    }
    if target > z_lo || target < z_hi {
        return Err(ImpedanceError::OutOfReach { target, low_width: lo, low: z_lo, high_width: hi, high: z_hi });
    }
    for _ in 0..100 {
        let mid = (lo + hi) / 2.0;
        if z(mid) > target {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo < 1e-7 {
            break;
        }
    }
    let w = (lo + hi) / 2.0;
    Ok((w, at(m, kind, w, s)))
}
