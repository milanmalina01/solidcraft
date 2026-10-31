//! The board's stack-up: what it is made of, top to bottom. Each copper layer has a thickness,
//! between two copper layers is a dielectric (a core, or prepreg that bonds cores together) with
//! a thickness, a relative permittivity (εr) and optionally a loss tangent, and the outside may
//! have solder mask. Saved as a list, `stackup`, top first; without one the board has the default
//! stack-up (see [`default_stackup`]), worked out from its thickness and copper layers.

use serde::{Deserialize, Serialize};

use super::layers::{CopperLayer, MIN_COPPER_LAYERS};

/// Outer and inner copper when none is given, in millimetres (1 oz and ½ oz).
pub const OUTER_COPPER: f64 = 0.035;
pub const INNER_COPPER: f64 = 0.0175;
/// A dielectric's εr when none is given (FR-4 at a few GHz).
pub const DEFAULT_ER: f64 = 4.3;
/// Solder mask's εr when none is given.
pub const MASK_ER: f64 = 3.5;
/// The thinnest and thickest copper, dielectric and mask, in millimetres.
pub const COPPER_THICKNESS: std::ops::RangeInclusive<f64> = 0.005..=0.5;
pub const DIELECTRIC_THICKNESS: std::ops::RangeInclusive<f64> = 0.01..=5.0;
pub const MASK_THICKNESS: std::ops::RangeInclusive<f64> = 0.001..=0.2;
/// The εr and loss tangent a material may have.
pub const ER: std::ops::RangeInclusive<f64> = 1.0..=20.0;
pub const LOSS_TANGENT: std::ops::RangeInclusive<f64> = 0.0..=1.0;
/// How far a stack-up's total may be from the board's thickness, as a share of it.
pub const THICKNESS_SLACK: f64 = 0.1;

/// What a stack-up layer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackKind {
    Copper,
    /// A cured dielectric with copper on both sides.
    Core,
    /// The dielectric that bonds cores and outer copper.
    Prepreg,
    /// Solder mask, outside the outer copper.
    Mask,
}

/// One layer of the stack-up.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StackLayer {
    pub kind: StackKind,
    /// In millimetres.
    pub thickness: f64,
    /// Relative permittivity of a dielectric or the mask; left out, [`DEFAULT_ER`] (the mask's
    /// [`MASK_ER`]). Copper has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub er: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_tangent: Option<f64>,
}

impl StackLayer {
    pub fn copper(thickness: f64) -> Self {
        StackLayer { kind: StackKind::Copper, thickness, er: None, loss_tangent: None }
    }

    /// Its εr: the given one, or the default of its kind (1 for copper).
    pub fn er(&self) -> f64 {
        match (self.kind, self.er) {
            (StackKind::Copper, _) => 1.0,
            (_, Some(er)) => er,
            (StackKind::Mask, None) => MASK_ER,
            (_, None) => DEFAULT_ER,
        }
    }

    pub fn is_dielectric(&self) -> bool {
        matches!(self.kind, StackKind::Core | StackKind::Prepreg)
    }
}

/// The stack-up a board of `copper_layers` and `thickness` has without one of its own: outer
/// copper [`OUTER_COPPER`], inner [`INNER_COPPER`], the rest of the thickness shared evenly by
/// the dielectrics (at least the thinnest one), prepreg next to the outer copper and cores and
/// prepreg in turn inside (a 2-layer board is one core), no mask.
pub fn default_stackup(copper_layers: u8, thickness: f64) -> Vec<StackLayer> {
    let n = usize::from(copper_layers.max(MIN_COPPER_LAYERS));
    let copper = 2.0 * OUTER_COPPER + (n - 2) as f64 * INNER_COPPER;
    let each = ((thickness - copper) / (n - 1) as f64).max(*DIELECTRIC_THICKNESS.start());
    let mut out = Vec::with_capacity(2 * n - 1);
    for i in 0..n {
        out.push(StackLayer::copper(if i == 0 || i + 1 == n { OUTER_COPPER } else { INNER_COPPER }));
        if i + 1 < n {
            let kind = if n == 2 || i % 2 == 1 { StackKind::Core } else { StackKind::Prepreg };
            out.push(StackLayer { kind, thickness: each, er: None, loss_tangent: None });
        }
    }
    out
}

/// The total thickness of `layers`.
pub fn total(layers: &[StackLayer]) -> f64 {
    layers.iter().map(|l| l.thickness).sum()
}

/// A stack-up fits a board of `copper_layers`: mask (optional) on the outside only, then copper
/// and dielectrics in turn, starting and ending with copper, `copper_layers` copper layers; each
/// value in its range. Its total against the board's thickness is checked by the board.
pub fn check_stackup(layers: &[StackLayer], copper_layers: u8) -> Result<(), String> {
    let at = |i: usize, e: String| format!("`stackup[{i}]`: {e}");
    let inner: &[StackLayer] = {
        let start = usize::from(layers.first().is_some_and(|l| l.kind == StackKind::Mask));
        let end = layers.len().saturating_sub(usize::from(layers.len() > start && layers.last().is_some_and(|l| l.kind == StackKind::Mask)));
        layers.get(start..end).unwrap_or_default()
    };
    let copper = layers.iter().filter(|l| l.kind == StackKind::Copper).count();
    if copper != usize::from(copper_layers) {
        return Err(format!("the stack-up has {copper} copper layers; the board has {copper_layers}"));
    }
    for (i, l) in inner.iter().enumerate() {
        let want_copper = i % 2 == 0;
        if want_copper != (l.kind == StackKind::Copper) || l.kind == StackKind::Mask {
            return Err("a stack-up runs copper, dielectric (core or prepreg), copper … copper, with solder mask only outside".into());
        }
    }
    if inner.len().is_multiple_of(2) {
        return Err("a stack-up starts and ends with copper (solder mask may go outside it)".into());
    }
    for (i, l) in layers.iter().enumerate() {
        let (range, what) = match l.kind {
            StackKind::Copper => (COPPER_THICKNESS, "copper"),
            StackKind::Core | StackKind::Prepreg => (DIELECTRIC_THICKNESS, "a dielectric"),
            StackKind::Mask => (MASK_THICKNESS, "solder mask"),
        };
        if !range.contains(&l.thickness) {
            return Err(at(i, format!("{what} is {} to {} mm thick", range.start(), range.end())));
        }
        if l.kind == StackKind::Copper && (l.er.is_some() || l.loss_tangent.is_some()) {
            return Err(at(i, "copper has no `er` or `loss_tangent`".into()));
        }
        if let Some(er) = l.er
            && !ER.contains(&er)
        {
            return Err(at(i, format!("`er` must be {} to {}", ER.start(), ER.end())));
        }
        if let Some(t) = l.loss_tangent
            && !LOSS_TANGENT.contains(&t)
        {
            return Err(at(i, format!("`loss_tangent` must be {} to {}", LOSS_TANGENT.start(), LOSS_TANGENT.end())));
        }
    }
    Ok(())
}

/// Where a copper layer sits in a stack-up: its copper, and the dielectrics right above and
/// below it (`None` outside the outer copper).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surroundings {
    pub copper: StackLayer,
    pub above: Option<StackLayer>,
    pub below: Option<StackLayer>,
}

/// The copper layer `layer`'s place in `layers` (a stack-up of `copper_layers`); `None` when it
/// has no such layer.
pub fn surroundings(layers: &[StackLayer], layer: CopperLayer, copper_layers: u8) -> Option<Surroundings> {
    // An inner layer past the stack would take the place of a later one (inner1 of a 2-layer
    // board is the bottom's place).
    if !layer.exists_in(copper_layers) {
        return None;
    }
    let want = layer.index(copper_layers);
    let (i, copper) = layers.iter().enumerate().filter(|(_, l)| l.kind == StackKind::Copper).nth(want)?;
    let dielectric = |j: Option<usize>| j.and_then(|j| layers.get(j)).copied().filter(StackLayer::is_dielectric);
    Some(Surroundings { copper: *copper, above: dielectric(i.checked_sub(1)), below: dielectric(i.checked_add(1)) })
}
