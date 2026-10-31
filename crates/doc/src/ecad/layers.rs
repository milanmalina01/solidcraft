//! The board's layers: its copper stack (top, the inner layers in order, bottom) and the layers
//! board drawings go on. Saved by name: `top`, `inner1` … `inner14`, `bottom`, and for drawings
//! also `silkscreen_top`, `silkscreen_bottom` and `documentation`.

use serde::{Deserialize, Serialize};

use super::footprint::Side;

/// The fewest and most copper layers a board has; always an even number.
pub const MIN_COPPER_LAYERS: u8 = 2;
pub const MAX_COPPER_LAYERS: u8 = 16;

/// A copper layer of the board, in stack order: the top, inner layer 1 under it, and so on down to
/// the bottom. Inner layer n exists when n ≤ the stack's copper layers − 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum CopperLayer {
    Top,
    /// 1 to 14.
    Inner(u8),
    Bottom,
}

impl CopperLayer {
    /// Its saved name: `top`, `inner1` … `inner14`, `bottom`.
    pub fn name(self) -> String {
        match self {
            CopperLayer::Top => "top".into(),
            CopperLayer::Inner(n) => format!("inner{n}"),
            CopperLayer::Bottom => "bottom".into(),
        }
    }

    /// The layer a saved name names; `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "top" => Some(CopperLayer::Top),
            "bottom" => Some(CopperLayer::Bottom),
            _ => {
                let n: u8 = s.strip_prefix("inner")?.parse().ok()?;
                // `inner01` is not a name.
                (s.len() == 5 + n.to_string().len() && (1..=MAX_COPPER_LAYERS - 2).contains(&n)).then_some(CopperLayer::Inner(n))
            }
        }
    }

    /// Whether a stack of `copper_layers` has it.
    pub fn exists_in(self, copper_layers: u8) -> bool {
        match self {
            CopperLayer::Top | CopperLayer::Bottom => true,
            CopperLayer::Inner(n) => n >= 1 && n <= copper_layers.saturating_sub(2),
        }
    }

    /// Its place in a stack of `copper_layers`, from 0 at the top.
    pub fn index(self, copper_layers: u8) -> usize {
        match self {
            CopperLayer::Top => 0,
            CopperLayer::Inner(n) => usize::from(n),
            CopperLayer::Bottom => usize::from(copper_layers.max(MIN_COPPER_LAYERS) - 1),
        }
    }

    /// The layer at `index` of a stack of `copper_layers`.
    pub fn at(index: usize, copper_layers: u8) -> Option<Self> {
        let last = usize::from(copper_layers.max(MIN_COPPER_LAYERS) - 1);
        match index {
            0 => Some(CopperLayer::Top),
            i if i == last => Some(CopperLayer::Bottom),
            i if i < last => u8::try_from(i).ok().map(CopperLayer::Inner),
            _ => None,
        }
    }

    /// The layer seen in its place once the board is turned over: top and bottom swap, inner
    /// layer n becomes inner layer (copper_layers − 1 − n).
    pub fn flipped(self, copper_layers: u8) -> Self {
        let last = usize::from(copper_layers.max(MIN_COPPER_LAYERS) - 1);
        CopperLayer::at(last.saturating_sub(self.index(copper_layers)), copper_layers).unwrap_or(self)
    }

    /// The board side of an outer layer; `None` for an inner one.
    pub fn side(self) -> Option<Side> {
        match self {
            CopperLayer::Top => Some(Side::Top),
            CopperLayer::Inner(_) => None,
            CopperLayer::Bottom => Some(Side::Bottom),
        }
    }

    pub fn is_inner(self) -> bool {
        matches!(self, CopperLayer::Inner(_))
    }
}

impl From<Side> for CopperLayer {
    fn from(side: Side) -> Self {
        match side {
            Side::Top => CopperLayer::Top,
            Side::Bottom => CopperLayer::Bottom,
        }
    }
}

impl From<CopperLayer> for String {
    fn from(l: CopperLayer) -> Self {
        l.name()
    }
}

impl TryFrom<String> for CopperLayer {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        CopperLayer::parse(&s)
            .ok_or_else(|| format!("unknown copper layer `{}` (top, inner1 … inner14, bottom)", s.chars().take(32).collect::<String>()))
    }
}

/// The copper layers of a stack of `copper_layers`, top to bottom.
pub fn stack(copper_layers: u8) -> Vec<CopperLayer> {
    let n = usize::from(copper_layers.clamp(MIN_COPPER_LAYERS, MAX_COPPER_LAYERS));
    (0..n).filter_map(|i| CopperLayer::at(i, copper_layers)).collect()
}

/// The board layer a board drawing is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum DrawLayer {
    SilkscreenTop,
    SilkscreenBottom,
    /// Notes and outlines for the fabrication and assembly drawings; never printed or etched.
    Documentation,
    /// Copper: it is etched, but joins nothing.
    Copper(CopperLayer),
}

impl DrawLayer {
    /// Its saved name: `silkscreen_top`, `silkscreen_bottom`, `documentation`, or the copper
    /// layer's.
    pub fn name(self) -> String {
        match self {
            DrawLayer::SilkscreenTop => "silkscreen_top".into(),
            DrawLayer::SilkscreenBottom => "silkscreen_bottom".into(),
            DrawLayer::Documentation => "documentation".into(),
            DrawLayer::Copper(l) => l.name(),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "silkscreen_top" => Some(DrawLayer::SilkscreenTop),
            "silkscreen_bottom" => Some(DrawLayer::SilkscreenBottom),
            "documentation" => Some(DrawLayer::Documentation),
            _ => CopperLayer::parse(s).map(DrawLayer::Copper),
        }
    }

    /// Whether it is read from the bottom of the board, so its text is made mirrored.
    pub fn is_bottom(self) -> bool {
        matches!(self, DrawLayer::SilkscreenBottom | DrawLayer::Copper(CopperLayer::Bottom))
    }

    /// The layer in its place once the board is turned over (see [`CopperLayer::flipped`]).
    pub fn flipped(self, copper_layers: u8) -> Self {
        match self {
            DrawLayer::SilkscreenTop => DrawLayer::SilkscreenBottom,
            DrawLayer::SilkscreenBottom => DrawLayer::SilkscreenTop,
            DrawLayer::Documentation => DrawLayer::Documentation,
            DrawLayer::Copper(l) => DrawLayer::Copper(l.flipped(copper_layers)),
        }
    }

    /// Whether a stack of `copper_layers` has it.
    pub fn exists_in(self, copper_layers: u8) -> bool {
        match self {
            DrawLayer::Copper(l) => l.exists_in(copper_layers),
            _ => true,
        }
    }
}

impl From<DrawLayer> for String {
    fn from(l: DrawLayer) -> Self {
        l.name()
    }
}

impl TryFrom<String> for DrawLayer {
    type Error = String;

    fn try_from(s: String) -> Result<Self, String> {
        DrawLayer::parse(&s).ok_or_else(|| {
            format!(
                "unknown board drawing layer `{}` (silkscreen_top, silkscreen_bottom, documentation, or a copper layer)",
                s.chars().take(32).collect::<String>()
            )
        })
    }
}
