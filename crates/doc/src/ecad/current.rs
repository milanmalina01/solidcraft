//! How wide a track must be to carry a current, from IPC-2221's published closed form of its
//! chart of a track's temperature rise:
//!
//!   I = k · ΔT^0.44 · A^0.725
//!
//! with I the current in amperes, ΔT the rise above the ambient temperature in °C, A the track's
//! cross-section in square mils, and k = 0.048 on an outer layer (cooled by the air) and 0.024 on
//! an inner one. Solved for the cross-section, A = (I / (k · ΔT^0.44))^(1/0.725), and the width
//! is A over the copper's thickness (1 oz/ft² is 0.0347 mm, 1.37 mil); solved for the rise at a
//! width, ΔT = (I / (k · A^0.725))^(1/0.44). The fit is published for up to 35 A, 0.5 to 3 oz
//! copper, a 10 to 100 °C rise and tracks up to 10 mm (400 mil) wide; outside that it is a rough
//! guess (`valid_range` false). It counts no planes or pours nearby, which carry heat away, so
//! its widths are on the safe side there.
//!
//! The track's DC resistance at its temperature (the ambient plus the rise) is R = ρ L / (w t),
//! with copper's resistivity ρ = 1.724e-8 Ω·m at 20 °C growing by α = 0.00393 of it per °C
//! (ρ(T) = ρ20 (1 + α (T − 20))); at a current I the track drops I R and loses I² R.
//!
//! The other formula ([`Fit::Ipc2152`]) is Brooks and Adam's fit to IPC-2152's external data
//! (2015, "Trace Currents and Temperatures Revisited"):
//!
//!   ΔT = 215.3 · I² · W^-1.15 · Th^-1
//!
//! with W the width and Th the copper's thickness in mils. IPC-2152 found that inner tracks cool
//! about as well as outer ones (IPC-2221 halves them without test data), so the fit serves both,
//! on the safe side inside; for wide outer tracks it allows about a quarter less current than
//! IPC-2221 (100 mil of 1 oz at 10 °C: 3.56 A against 4.68 A). It was fitted to the 2 and 3 oz
//! data, and the 1 oz curves were drawn from it; it is checked from 1 to 3 oz and up to a 100 °C
//! rise, and the calculator holds it to the same currents and widths as IPC-2221's range (up to
//! 35 A and 10 mm): `valid_range`. The design-quality checks use it (see `quality::pi_current`).

use std::ops::RangeInclusive;

use super::rules::NetClass;
use super::stackup::{COPPER_THICKNESS, OUTER_COPPER, StackKind, StackLayer};

/// 1 oz/ft² of copper, in millimetres.
pub const OZ_MM: f64 = 0.0347;
/// A mil (a thousandth of an inch), in millimetres.
const MIL_MM: f64 = 0.0254;
/// The fit's k on an outer and an inner layer.
pub const K_OUTER: f64 = 0.048;
pub const K_INNER: f64 = 0.024;
/// Copper's resistivity at 20 °C, in Ω·m, and how much of it it gains per °C.
pub const RESISTIVITY: f64 = 1.724e-8;
pub const ALPHA: f64 = 0.00393;
/// The rise and the ambient temperature when none is given, in °C.
pub const DEFAULT_RISE: f64 = 10.0;
pub const DEFAULT_AMBIENT: f64 = 25.0;
/// Which formula, as replies name it.
pub const FORMULA: &str = "IPC-2221 (I = k · ΔT^0.44 · A^0.725)";
pub const FORMULA_2152: &str = "IPC-2152 external fit (ΔT = 215.3 · I² · W^-1.15 · Th^-1, W and Th in mil; Brooks and Adam 2015)";
/// The IPC-2152 fit's constant and width exponent.
const FIT2152_K: f64 = 215.3;
const FIT2152_W: f64 = 1.15;
/// The copper (oz/ft²) and the highest rise (°C) the IPC-2152 fit is checked on.
pub const FIT2152_OZ: RangeInclusive<f64> = 1.0..=3.0;
pub const FIT2152_RISE: f64 = 100.0;

/// Which formula the calculator uses (see the module notes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fit {
    /// IPC-2221's chart, k 0.048 outside and 0.024 inside.
    #[default]
    Ipc2221,
    /// Brooks and Adam's fit to IPC-2152's external data, for every layer.
    Ipc2152,
}

impl Fit {
    pub fn name(self) -> &'static str {
        match self {
            Fit::Ipc2221 => "ipc2221",
            Fit::Ipc2152 => "ipc2152",
        }
    }

    /// As replies name it.
    pub fn formula(self) -> &'static str {
        match self {
            Fit::Ipc2221 => FORMULA,
            Fit::Ipc2152 => FORMULA_2152,
        }
    }
}

/// What the calculator takes: wider than the fit's range, which is flagged rather than refused.
/// Amperes.
pub const CURRENT: RangeInclusive<f64> = 0.001..=1000.0;
/// °C above the ambient.
pub const RISE: RangeInclusive<f64> = 0.1..=500.0;
/// °C.
pub const AMBIENT: RangeInclusive<f64> = -60.0..=200.0;
/// The copper's thickness in millimetres: what a stack-up's copper may be.
pub const COPPER: RangeInclusive<f64> = COPPER_THICKNESS;
/// The track's width and length, in millimetres.
pub const WIDTH: RangeInclusive<f64> = 0.01..=100.0;
pub const LENGTH: RangeInclusive<f64> = 0.001..=10_000.0;

/// The fit's published range: the most current, the copper in ounces, the rise and the widest
/// track (mm).
pub const FIT_CURRENT: f64 = 35.0;
pub const FIT_OZ: RangeInclusive<f64> = 0.5..=3.0;
pub const FIT_RISE: RangeInclusive<f64> = 10.0..=100.0;
pub const FIT_WIDTH: f64 = 10.0;

/// Why the calculator gives no answer: in English through `Display`, and as a message key with
/// named arguments for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurrentError {
    Current,
    Rise,
    Ambient,
    Copper,
    Width,
    Length,
    /// The numbers run out of range on the way (no finite answer).
    NoResult,
}

impl CurrentError {
    /// The UI's message key.
    pub fn key(&self) -> &'static str {
        match self {
            CurrentError::Current => "electronics.current_err_current",
            CurrentError::Rise => "electronics.current_err_rise",
            CurrentError::Ambient => "electronics.current_err_ambient",
            CurrentError::Copper => "electronics.current_err_copper",
            CurrentError::Width => "electronics.current_err_width",
            CurrentError::Length => "electronics.current_err_length",
            CurrentError::NoResult => "electronics.current_err_none",
        }
    }

    /// The range the message names.
    fn range(&self) -> Option<&'static RangeInclusive<f64>> {
        match self {
            CurrentError::Current => Some(&CURRENT),
            CurrentError::Rise => Some(&RISE),
            CurrentError::Ambient => Some(&AMBIENT),
            CurrentError::Copper => Some(&COPPER),
            CurrentError::Width => Some(&WIDTH),
            CurrentError::Length => Some(&LENGTH),
            CurrentError::NoResult => None,
        }
    }

    /// The message's arguments by name: the range's ends (and the copper's in ounces).
    pub fn args(&self) -> Vec<(&'static str, String)> {
        let Some(r) = self.range() else { return Vec::new() };
        let mut out = vec![("min", format!("{}", r.start())), ("max", format!("{}", r.end()))];
        if *self == CurrentError::Copper {
            out.push(("min_oz", format!("{:.2}", mm_to_oz(*r.start()))));
            out.push(("max_oz", format!("{:.1}", mm_to_oz(*r.end()))));
        }
        out
    }
}

impl std::fmt::Display for CurrentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Some(r) = self.range() else { return write!(f, "the formula gives no finite answer for these numbers") };
        let (what, unit) = match self {
            CurrentError::Current => ("`current`", "A"),
            CurrentError::Rise => ("`temp_rise`", "°C"),
            CurrentError::Ambient => ("`ambient`", "°C"),
            CurrentError::Copper => ("the copper's thickness", "mm"),
            CurrentError::Width => ("`width`", "mm"),
            _ => ("`length`", "mm"),
        };
        write!(f, "{what} must be {} to {} {unit}", r.start(), r.end())?;
        if *self == CurrentError::Copper {
            write!(f, " ({:.2} to {:.1} oz)", mm_to_oz(*r.start()), mm_to_oz(*r.end()))?;
        }
        Ok(())
    }
}

impl From<CurrentError> for String {
    fn from(e: CurrentError) -> String {
        e.to_string()
    }
}

/// Ounces of copper per square foot as millimetres of thickness, and back.
pub fn oz_to_mm(oz: f64) -> f64 {
    oz * OZ_MM
}

pub fn mm_to_oz(mm: f64) -> f64 {
    mm / OZ_MM
}

fn check(range: &RangeInclusive<f64>, v: f64, e: CurrentError) -> Result<f64, CurrentError> {
    if v.is_finite() && range.contains(&v) { Ok(v) } else { Err(e) }
}

fn finite(v: f64) -> Result<f64, CurrentError> {
    if v.is_finite() && v > 0.0 { Ok(v) } else { Err(CurrentError::NoResult) }
}

fn k(outer: bool) -> f64 {
    if outer { K_OUTER } else { K_INNER }
}

/// What the fit gives for one track: its width and rise (one given, the other worked out), on
/// copper `copper` mm thick on an outer or inner layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capacity {
    /// In millimetres.
    pub width: f64,
    /// °C above the ambient.
    pub rise: f64,
    pub copper: f64,
    pub outer: bool,
    /// Inside the fit's published range (see the module notes).
    pub valid_range: bool,
}

/// Whether a track of `width` mm on `copper` mm carrying `current` A at a `rise` °C is inside
/// the fit's published range.
pub fn in_range(current: f64, rise: f64, copper: f64, width: f64) -> bool {
    let oz = mm_to_oz(copper);
    let slack = 1e-6;
    current <= FIT_CURRENT && oz >= FIT_OZ.start() - slack && oz <= FIT_OZ.end() + slack && FIT_RISE.contains(&rise) && width <= FIT_WIDTH
}

/// The width the fit asks for, unchecked: in millimetres.
fn width_at(current: f64, rise: f64, copper: f64, outer: bool) -> f64 {
    let area = (current / (k(outer) * rise.powf(0.44))).powf(1.0 / 0.725);
    area / (copper / MIL_MM) * MIL_MM
}

/// The narrowest track that carries `current` A with a rise of `rise` °C on copper `copper` mm
/// thick (an outer layer's when `outer`); why not when a number is out of range.
pub fn width_for(current: f64, rise: f64, copper: f64, outer: bool) -> Result<Capacity, CurrentError> {
    check(&CURRENT, current, CurrentError::Current)?;
    check(&RISE, rise, CurrentError::Rise)?;
    check(&COPPER, copper, CurrentError::Copper)?;
    let width = finite(width_at(current, rise, copper, outer))?;
    Ok(Capacity { width, rise, copper, outer, valid_range: in_range(current, rise, copper, width) })
}

/// The rise of a track `width` mm wide on copper `copper` mm thick carrying `current` A; why not
/// when a number is out of range.
pub fn rise_for(current: f64, width: f64, copper: f64, outer: bool) -> Result<Capacity, CurrentError> {
    check(&CURRENT, current, CurrentError::Current)?;
    check(&WIDTH, width, CurrentError::Width)?;
    check(&COPPER, copper, CurrentError::Copper)?;
    let area = (width / MIL_MM) * (copper / MIL_MM);
    let rise = finite((current / (k(outer) * area.powf(0.725))).powf(1.0 / 0.44))?;
    Ok(Capacity { width, rise, copper, outer, valid_range: in_range(current, rise, copper, width) })
}

/// The width the same rise asks for on the other kind of layer, or on copper of another
/// thickness: unchecked against the ranges (the rise may be one worked out), finite or none.
pub fn width_like(c: &Capacity, current: f64, copper: f64, outer: bool) -> Option<Capacity> {
    let width = finite(width_at(current, c.rise, copper, outer)).ok()?;
    Some(Capacity { width, rise: c.rise, copper, outer, valid_range: in_range(current, c.rise, copper, width) })
}

/// The rise (°C) the IPC-2152 fit gives a track `width` mm wide on copper `copper` mm thick
/// carrying `current` A, unchecked against the ranges: finite, or none.
pub fn rise_2152(current: f64, width: f64, copper: f64) -> Option<f64> {
    if !(current.is_finite() && width > 0.0 && copper > 0.0) {
        return None;
    }
    let rise = FIT2152_K * current * current * (width / MIL_MM).powf(-FIT2152_W) / (copper / MIL_MM);
    rise.is_finite().then_some(rise)
}

/// The width (mm) the IPC-2152 fit asks for to carry `current` A with a rise of `rise` °C on
/// copper `copper` mm thick, unchecked against the ranges: finite and above 0, or none.
pub fn width_2152(current: f64, rise: f64, copper: f64) -> Option<f64> {
    if !(current.is_finite() && rise > 0.0 && copper > 0.0) {
        return None;
    }
    let mils = (FIT2152_K * current * current / (rise * copper / MIL_MM)).powf(1.0 / FIT2152_W);
    finite(mils * MIL_MM).ok()
}

/// Whether a rise of `rise` °C on copper `copper` mm thick is inside the range the IPC-2152 fit
/// is checked on.
pub fn in_range_2152(rise: f64, copper: f64) -> bool {
    let oz = mm_to_oz(copper);
    let slack = 1e-6;
    oz >= FIT2152_OZ.start() - slack && oz <= FIT2152_OZ.end() + slack && rise <= FIT2152_RISE
}

/// Whether the IPC-2152 fit's answer for `current` A on a track `width` mm wide with a rise of
/// `rise` °C on copper `copper` mm thick is inside its checked range and the calculator's span.
fn in_span_2152(current: f64, rise: f64, copper: f64, width: f64) -> bool {
    in_range_2152(rise, copper) && current <= FIT_CURRENT && width <= FIT_WIDTH
}

/// [`width_for`] by the formula `fit` (the IPC-2152 fit takes no account of `outer`).
pub fn width_for_fit(fit: Fit, current: f64, rise: f64, copper: f64, outer: bool) -> Result<Capacity, CurrentError> {
    if fit == Fit::Ipc2221 {
        return width_for(current, rise, copper, outer);
    }
    check(&CURRENT, current, CurrentError::Current)?;
    check(&RISE, rise, CurrentError::Rise)?;
    check(&COPPER, copper, CurrentError::Copper)?;
    let width = width_2152(current, rise, copper).ok_or(CurrentError::NoResult)?;
    Ok(Capacity { width, rise, copper, outer, valid_range: in_span_2152(current, rise, copper, width) })
}

/// [`rise_for`] by the formula `fit`.
pub fn rise_for_fit(fit: Fit, current: f64, width: f64, copper: f64, outer: bool) -> Result<Capacity, CurrentError> {
    if fit == Fit::Ipc2221 {
        return rise_for(current, width, copper, outer);
    }
    check(&CURRENT, current, CurrentError::Current)?;
    check(&WIDTH, width, CurrentError::Width)?;
    check(&COPPER, copper, CurrentError::Copper)?;
    let rise = rise_2152(current, width, copper).filter(|r| *r > 0.0).ok_or(CurrentError::NoResult)?;
    Ok(Capacity { width, rise, copper, outer, valid_range: in_span_2152(current, rise, copper, width) })
}

/// [`width_like`] by the formula `fit`.
pub fn width_like_fit(fit: Fit, c: &Capacity, current: f64, copper: f64, outer: bool) -> Option<Capacity> {
    if fit == Fit::Ipc2221 {
        return width_like(c, current, copper, outer);
    }
    let width = width_2152(current, c.rise, copper)?;
    Some(Capacity { width, rise: c.rise, copper, outer, valid_range: in_span_2152(current, c.rise, copper, width) })
}

/// A track's resistance, what it drops and loses at its current, and its temperature.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loss {
    /// Ohms.
    pub resistance: f64,
    /// Volts.
    pub voltage_drop: f64,
    /// Watts.
    pub power: f64,
}

/// Copper's resistivity at `temperature` °C, in Ω·m.
pub fn resistivity(temperature: f64) -> f64 {
    RESISTIVITY * (1.0 + ALPHA * (temperature - 20.0))
}

/// The DC resistance of a track `width` mm wide (any width, as one worked out may be wider than
/// [`WIDTH`]), `length` mm long, on copper `copper` mm thick at `temperature` °C, and what it
/// drops and loses at `current` A; why not when a number is out of range.
pub fn loss(current: f64, width: f64, copper: f64, length: f64, temperature: f64) -> Result<Loss, CurrentError> {
    check(&CURRENT, current, CurrentError::Current)?;
    finite(width)?;
    check(&COPPER, copper, CurrentError::Copper)?;
    check(&LENGTH, length, CurrentError::Length)?;
    if !temperature.is_finite() {
        return Err(CurrentError::NoResult);
    }
    // Millimetres to metres: L / (w t) gains a factor of 1000.
    let resistance = finite(resistivity(temperature) * length / (width * copper) * 1000.0)?;
    Ok(Loss { resistance, voltage_drop: current * resistance, power: current * current * resistance })
}

/// The copper a stack-up gives the calculator, in millimetres: the thinner of the two outer
/// layers (a width enough there is enough on the other), and the thinnest inner one (the outer
/// one's on a board with no inner layers).
pub fn stack_copper(layers: &[StackLayer]) -> (f64, f64) {
    let copper: Vec<f64> = layers.iter().filter(|l| l.kind == StackKind::Copper).map(|l| l.thickness).collect();
    let outer = match (copper.first(), copper.last()) {
        (Some(a), Some(b)) => a.min(*b),
        _ => OUTER_COPPER,
    };
    let inner = copper.get(1..copper.len().saturating_sub(1)).unwrap_or_default().iter().copied().reduce(f64::min).unwrap_or(outer);
    (outer, inner)
}

/// What a net class with a current asks for: the widths on outer and inner layers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClassWidths {
    pub current: f64,
    pub rise: f64,
    pub outer: Capacity,
    pub inner: Capacity,
}

/// The widths `class` asks for at its current and rise, on its own copper or that of the
/// stack-up `layers`; `None` without a current (or with numbers out of range).
pub fn class_widths(class: &NetClass, layers: &[StackLayer]) -> Option<ClassWidths> {
    let current = class.current?;
    let rise = class.temp_rise.unwrap_or(DEFAULT_RISE);
    let (outer, inner) = match class.copper_oz {
        Some(oz) => (oz_to_mm(oz), oz_to_mm(oz)),
        None => stack_copper(layers),
    };
    Some(ClassWidths { current, rise, outer: width_for(current, rise, outer, true).ok()?, inner: width_for(current, rise, inner, false).ok()? })
}
