//! The design's electronics: what the Electronics workspace and the `ecad.*` commands edit.
//!
//! Each part (the library's symbols, footprints and 3D packages, the electronic components that use
//! them, the schematic, the board) comes with its own design-format change, and unknown keys are
//! refused rather than dropped, so a build that doesn't know a part refuses the design instead of
//! losing it on save.
//!
//! Conventions for everything saved here: lengths in millimetres whatever the design's units;
//! x right and y up, seen from the top of the board; angles in degrees, counter-clockwise.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub mod airwires;
pub mod board;
pub mod checks;
pub mod connect;
pub mod copper;
pub mod current;
pub mod drc;
pub mod erc;
pub mod fill;
pub mod follow;
pub mod footprint;
pub mod frame;
pub mod graphics;
mod grid;
pub mod impedance;
pub mod infer;
pub mod info;
pub mod intent;
pub mod layers;
pub mod names;
pub mod overbar;
pub mod path;
pub mod place;
pub mod pour;
pub mod profiles;
pub mod quality;
pub mod rework;
pub mod route;
pub mod rules;
pub mod schematic;
pub mod stackup;
pub mod stitch;
pub mod symbol;

pub use airwires::{Airwire, BoardNets, BoardPad, Placed};
pub use board::{Board, BoardFootprint, BoardGraphic, BoardKind, Hole, Track, Via, ViaKind, ViaProtection, ViaRole};
pub use checks::{Gap, Problem, Resolved, Severity};
pub use connect::{Connectivity, Net, NetPin, connectivity};
pub use fill::PourFill;
pub use footprint::{FootprintItem, NO_FOOTPRINT, Pad, PadKind, PadShape, Side};
pub use frame::Frame;
pub use graphics::{BoardLayer, Graphic, GraphicType};
pub use info::DesignInfo;
pub use intent::Intent;
pub use layers::{CopperLayer, DrawLayer};
pub use place::{Motion, Placement};
pub use pour::{Connect, Forbid, Hatch, Keepout, Pour, Thermal};
pub use rules::{DiffPair, NetClass, Rules};
pub use schematic::{
    Actor, Anchor, Approval, Detached, ItemKind, Label, LabelKind, Level, Library, Marker, MarkerKind, Part, Placeable, Schematic, Wire, WireEnd,
};
pub use stackup::{StackKind, StackLayer};
pub use symbol::{NO_SYMBOL, Pin, PinDirection, PinType, SymbolItem};

/// The most electronic components a design holds.
pub const MAX_COMPONENTS: usize = 4096;
/// The most symbols, footprints or 3D packages (each) a design's library holds.
pub const MAX_ITEMS: usize = 4096;
/// The most variants (footprint choices) an electronic component has.
pub const MAX_VARIANTS: usize = 64;
/// The longest name, value, manufacturer part number or manufacturer, in characters.
pub const MAX_TEXT: usize = 64;
/// The longest description, in characters.
pub const MAX_DESCRIPTION: usize = 256;
/// The longest designator prefix (`R`, `C`, `U`, `LED`…), in letters.
pub const MAX_PREFIX: usize = 8;
/// Ids stay below this, so every JSON reader keeps them exact.
pub const MAX_ID: u64 = 1 << 53;
/// The longest pin or pad number, in characters.
pub const MAX_NUMBER: usize = 16;
/// How far from the origin a symbol or footprint reaches, in millimetres.
pub const MAX_COORD: f64 = 1000.0;

/// Pin number to the pad numbers it connects to.
pub type PinMap = BTreeMap<String, Vec<String>>;

/// The design's electronics, saved under the design-file key `ecad`; nothing is saved while it is
/// empty, so a design without electronics has no `ecad` key.
///
/// Its parts: the library (symbols, footprints and 3D packages, each its own item that many
/// electronic components can share), the electronic components that use them, the schematic,
/// the board, and the design's intent (what its nets and parts are for). The netlist, the
/// board's airwires and the 3D board are worked out from them, never saved twice.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Electronics {
    /// The library's symbols, in the order they were made.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<SymbolItem>,
    /// The library's footprints, in the order they were made.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub footprints: Vec<FootprintItem>,
    /// The library's 3D packages, in the order they were made.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<PackageItem>,
    /// The electronic components, in the order they were made.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<ElectronicComponent>,
    /// The id the next symbol, footprint, 3D package or electronic component gets (one id space
    /// for the four); ids are never reused.
    #[serde(skip_serializing_if = "is_zero")]
    pub next_id: u64,
    /// The schematic sheet; not saved while empty.
    #[serde(skip_serializing_if = "Schematic::is_empty")]
    pub schematic: Schematic,
    /// The circuit board; not saved while empty.
    #[serde(skip_serializing_if = "Board::is_empty")]
    pub board: Board,
    /// What the nets and parts are for (see [`intent`]); not saved while empty.
    #[serde(skip_serializing_if = "Intent::is_empty")]
    pub intent: Intent,
    /// What a print's title block says (see [`info`]); not saved while empty.
    #[serde(skip_serializing_if = "DesignInfo::is_empty")]
    pub info: DesignInfo,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// A part that goes on a circuit board (a resistor, a chip, a connector), with what a parts list
/// needs to buy it: the library symbol it shows on the schematic, and its variants, each a
/// footprint it can be built with. Not a SolidCraft component, which is a container of bodies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElectronicComponent {
    /// Never 0, and unique in the design.
    pub id: u64,
    /// Unique among the electronic components, such as `R0603` or `LM358`.
    pub name: String,
    /// The letters that start each placed copy's designator: `R` for R1, R2….
    pub prefix: String,
    /// The value printed on the schematic, such as `10k` or `100nF`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    /// The manufacturer part number.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mpn: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manufacturer: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The library symbol's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<u64>,
    /// The ways it can be built, the first one by default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Variant>,
}

/// One way to build an electronic component: a footprint, a 3D package on it, and which pads each
/// of the symbol's pins connects to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    /// Unique in the component, such as `SOT-23`.
    pub name: String,
    /// The library footprint's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footprint: Option<u64>,
    /// The library 3D package's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<u64>,
    /// Which pads each pin connects to, by number; without one, each pin connects to the pad with
    /// its number. A pin left out of the map connects to no pad.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_map: Option<PinMap>,
}

/// A 3D package in the design's library: the SolidCraft component `model`, its origin moved to
/// `translate` from the footprint's origin and turned `rotation` degrees about z. The board's top
/// is z = 0 and +z points away from it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageItem {
    /// Never 0; symbols, footprints, 3D packages and electronic components share one id space.
    pub id: u64,
    /// Unique among the 3D packages.
    pub name: String,
    /// The SolidCraft component's id (never 0, the design itself).
    pub model: u64,
    #[serde(default, skip_serializing_if = "is_origin")]
    pub translate: [f64; 3],
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub rotation: f64,
}

fn is_origin(v: &[f64; 3]) -> bool {
    *v == [0.0; 3]
}

fn is_zero_f(v: &f64) -> bool {
    *v == 0.0
}

impl PackageItem {
    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0 || self.id >= MAX_ID {
            return Err(format!("3D package `{}` has the id {}, which is out of range", self.name, self.id));
        }
        check_item_name("3D package", &self.name)?;
        self.validate_placement().map_err(|e| format!("3D package `{}`: {e}", self.name))
    }

    /// The model and where it sits, alone.
    pub fn validate_placement(&self) -> Result<(), String> {
        if self.model == 0 {
            return Err("a 3D package is a SolidCraft component, not the design itself".into());
        }
        if !self.translate.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORD) {
            return Err(format!("the 3D package's `translate` must be within {MAX_COORD} mm"));
        }
        if !(self.rotation.is_finite() && self.rotation.abs() <= 360.0) {
            return Err("the 3D package's `rotation` must be -360 to 360 degrees".into());
        }
        Ok(())
    }

    /// A point of the model, in the footprint's frame.
    pub fn place(&self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        let (s, c) = self.rotation.to_radians().sin_cos();
        [x * c - y * s + self.translate[0], x * s + y * c + self.translate[1], z + self.translate[2]]
    }
}

impl Electronics {
    /// Whether nothing is saved for the electronics (no `ecad` key in the design file).
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether the design has electronics to show: library items, electronic components or
    /// anything on the sheet. The id counters alone, left after the last one is deleted, are none.
    pub fn has_content(&self) -> bool {
        !(self.components.is_empty() && self.symbols.is_empty() && self.footprints.is_empty() && self.packages.is_empty())
            || self.schematic.has_content()
            || self.board.has_content()
    }

    /// The electronic components by id, each with the symbol it shows, for checking and resolving
    /// the schematic.
    pub fn library(&self) -> Library<'_> {
        let symbols: BTreeMap<u64, &SymbolItem> = self.symbols.iter().map(|s| (s.id, s)).collect();
        let shown = |c: &ElectronicComponent| c.symbol.and_then(|id| symbols.get(&id).copied()).unwrap_or(&NO_SYMBOL);
        self.components.iter().map(|c| (c.id, Placeable { component: c, symbol: shown(c) })).collect()
    }

    /// The electronic component with this id.
    pub fn component(&self, id: u64) -> Option<&ElectronicComponent> {
        self.components.iter().find(|c| c.id == id)
    }

    /// The electronic component with this name (exactly).
    pub fn named(&self, name: &str) -> Option<&ElectronicComponent> {
        self.components.iter().find(|c| c.name == name)
    }

    pub fn symbol(&self, id: u64) -> Option<&SymbolItem> {
        self.symbols.iter().find(|s| s.id == id)
    }

    pub fn footprint(&self, id: u64) -> Option<&FootprintItem> {
        self.footprints.iter().find(|f| f.id == id)
    }

    pub fn package(&self, id: u64) -> Option<&PackageItem> {
        self.packages.iter().find(|p| p.id == id)
    }

    /// The symbol `c` shows; an empty one while it has none.
    pub fn symbol_of(&self, c: &ElectronicComponent) -> &SymbolItem {
        c.symbol.and_then(|id| self.symbol(id)).unwrap_or(&NO_SYMBOL)
    }

    /// `c` with its symbol and the variant named `variant` (the first one for `None`, or when it
    /// has no such variant) looked up.
    pub fn resolve<'a>(&'a self, c: &'a ElectronicComponent, variant: Option<&str>) -> Resolved<'a> {
        let v = match variant {
            Some(name) => c.variants.iter().find(|v| v.name == name),
            None => c.variants.first(),
        };
        self.resolve_variant(c, v)
    }

    /// `c` with its symbol and the variant `v` looked up.
    pub fn resolve_variant<'a>(&'a self, c: &'a ElectronicComponent, v: Option<&'a Variant>) -> Resolved<'a> {
        Resolved {
            component: c,
            symbol: self.symbol_of(c),
            variant: v,
            footprint: v.and_then(|v| v.footprint).and_then(|id| self.footprint(id)).unwrap_or(&NO_FOOTPRINT),
            package: v.and_then(|v| v.package).and_then(|id| self.package(id)),
        }
    }

    /// The electronic components that show the symbol `id`.
    pub fn symbol_users(&self, id: u64) -> impl Iterator<Item = &ElectronicComponent> {
        self.components.iter().filter(move |c| c.symbol == Some(id))
    }

    /// The variants (with their component) built on the footprint `id`.
    pub fn footprint_users(&self, id: u64) -> impl Iterator<Item = (&ElectronicComponent, &Variant)> {
        self.components.iter().flat_map(move |c| c.variants.iter().filter(move |v| v.footprint == Some(id)).map(move |v| (c, v)))
    }

    /// The variants (with their component) that use the 3D package `id`.
    pub fn package_users(&self, id: u64) -> impl Iterator<Item = (&ElectronicComponent, &Variant)> {
        self.components.iter().flat_map(move |c| c.variants.iter().filter(move |v| v.package == Some(id)).map(move |v| (c, v)))
    }

    /// Point the 3D packages modelled by a component `gone` says is going at `to` instead, or
    /// remove them (`None`) and the variants' references to them.
    pub fn repoint_packages(&mut self, gone: impl Fn(u64) -> bool, to: Option<u64>) {
        match to {
            Some(to) => {
                for p in self.packages.iter_mut().filter(|p| gone(p.model)) {
                    p.model = to;
                }
            }
            None => {
                let removed: BTreeSet<u64> = self.packages.iter().filter(|p| gone(p.model)).map(|p| p.id).collect();
                if removed.is_empty() {
                    return;
                }
                self.packages.retain(|p| !removed.contains(&p.id));
                for v in self.components.iter_mut().flat_map(|c| c.variants.iter_mut()) {
                    if v.package.is_some_and(|id| removed.contains(&id)) {
                        v.package = None;
                    }
                }
            }
        }
    }

    /// Every id in use: the library items' and the electronic components'.
    fn ids(&self) -> impl Iterator<Item = u64> + '_ {
        let symbols = self.symbols.iter().map(|s| s.id);
        let footprints = self.footprints.iter().map(|f| f.id);
        symbols.chain(footprints).chain(self.packages.iter().map(|p| p.id)).chain(self.components.iter().map(|c| c.id))
    }

    /// The id [`Electronics::take_id`] gives next, without taking it.
    pub fn new_id(&self) -> Result<u64, String> {
        let id = self.next_id.max(1).max(self.ids().map(|id| id.saturating_add(1)).max().unwrap_or(1));
        if id >= MAX_ID {
            return Err("no electronics ids are left in this design".into());
        }
        Ok(id)
    }

    /// A new id for a symbol, footprint, 3D package or electronic component, which nothing else
    /// has had in this design.
    pub fn take_id(&mut self) -> Result<u64, String> {
        let id = self.new_id()?;
        self.next_id = id + 1;
        Ok(id)
    }

    /// Everything the design file and the commands require: the caps; ids unique across the
    /// library and the components, names unique within each kind; each record's own rules;
    /// components' references to items the library has, and pin maps that name the referenced
    /// symbol's pins and footprint's pads; and the schematic's rules (see [`Schematic::validate`]).
    pub fn validate(&self) -> Result<(), String> {
        for (what, n, max) in [
            ("symbols", self.symbols.len(), MAX_ITEMS),
            ("footprints", self.footprints.len(), MAX_ITEMS),
            ("3D packages", self.packages.len(), MAX_ITEMS),
            ("electronic components", self.components.len(), MAX_COMPONENTS),
        ] {
            if n > max {
                return Err(format!("a design holds at most {max} {what}"));
            }
        }
        if self.next_id > MAX_ID {
            return Err(format!("the electronics id counter {} is out of range", self.next_id));
        }
        let mut ids = BTreeSet::new();
        for id in self.ids() {
            if !ids.insert(id) {
                return Err(format!("two electronics records have the id {id}"));
            }
        }
        fn unique<'a>(kind: &str, names: impl Iterator<Item = &'a str>) -> Result<(), String> {
            let mut seen = BTreeSet::new();
            for name in names {
                if !seen.insert(name) {
                    return Err(format!("two {kind} are named `{name}`"));
                }
            }
            Ok(())
        }
        for s in &self.symbols {
            s.validate()?;
        }
        for f in &self.footprints {
            f.validate()?;
        }
        for p in &self.packages {
            p.validate()?;
        }
        unique("symbols", self.symbols.iter().map(|s| s.name.as_str()))?;
        unique("footprints", self.footprints.iter().map(|f| f.name.as_str()))?;
        unique("3D packages", self.packages.iter().map(|p| p.name.as_str()))?;
        unique("electronic components", self.components.iter().map(|c| c.name.as_str()))?;
        for c in &self.components {
            c.validate()?;
            let at = |e: String| format!("electronic component `{}`: {e}", c.name);
            if let Some(id) = c.symbol
                && self.symbol(id).is_none()
            {
                return Err(at(format!("its symbol {id} is not in the library")));
            }
            for v in &c.variants {
                let at = |e: String| at(format!("variant `{}`: {e}", v.name));
                if let Some(id) = v.footprint
                    && self.footprint(id).is_none()
                {
                    return Err(at(format!("its footprint {id} is not in the library")));
                }
                if let Some(id) = v.package
                    && self.package(id).is_none()
                {
                    return Err(at(format!("its 3D package {id} is not in the library")));
                }
                self.resolve_variant(c, Some(v)).check_pin_map().map_err(at)?;
            }
        }
        self.schematic.validate(&self.library())?;
        self.board.validate(&self.schematic)?;
        self.intent.validate()?;
        self.info.validate()
    }

    /// What every command stores: the board's footprints of parts gone from the sheet removed,
    /// checked as the design file is (see [`Electronics::validate`]), then the sheet's saved points
    /// that lie on pins tied to them, and the id counter past every id in use, so an id is never
    /// handed out twice.
    pub fn settle(&mut self) -> Result<(), String> {
        self.board.forget_parts(&self.schematic);
        self.validate()?;
        self.next_id = self.next_id.max(self.ids().map(|id| id.saturating_add(1)).max().unwrap_or(0));
        let mut sheet = std::mem::take(&mut self.schematic);
        sheet.attach(&self.library());
        self.schematic = sheet;
        Ok(())
    }
}

impl ElectronicComponent {
    /// The record's own rules: a name, a designator prefix and texts within their lengths, and
    /// variants with names unique in the component.
    pub fn validate(&self) -> Result<(), String> {
        if self.id == 0 || self.id >= MAX_ID {
            return Err(format!("electronic component `{}` has the id {}, which is out of range", self.name, self.id));
        }
        check_name(&self.name)?;
        check_prefix(&self.prefix)?;
        for (what, text, max) in [
            ("value", &self.value, MAX_TEXT),
            ("mpn", &self.mpn, MAX_TEXT),
            ("manufacturer", &self.manufacturer, MAX_TEXT),
            ("description", &self.description, MAX_DESCRIPTION),
        ] {
            check_text(what, text, max)?;
        }
        if self.variants.len() > MAX_VARIANTS {
            return Err(format!("electronic component `{}` has more than {MAX_VARIANTS} variants", self.name));
        }
        let mut names = BTreeSet::new();
        for v in &self.variants {
            check_item_name("variant", &v.name).map_err(|e| format!("electronic component `{}`: {e}", self.name))?;
            if !names.insert(v.name.as_str()) {
                return Err(format!("electronic component `{}` has two variants named `{}`", self.name, v.name));
            }
        }
        Ok(())
    }

    /// The variant named `name`.
    pub fn variant(&self, name: &str) -> Option<&Variant> {
        self.variants.iter().find(|v| v.name == name)
    }
}

impl Variant {
    /// Keep the pin map in step when a pin is renumbered (`to` is `Some`) or deleted (`None`).
    pub fn pin_renamed(&mut self, from: &str, to: Option<&str>) {
        if let Some(map) = &mut self.pin_map
            && let Some(pads) = map.remove(from)
            && let Some(to) = to
        {
            map.insert(to.to_string(), pads);
        }
    }

    /// Keep the pin map in step when a pad is renumbered (`to` is `Some`) or deleted (`None`).
    pub fn pad_renamed(&mut self, from: &str, to: Option<&str>) {
        if let Some(map) = &mut self.pin_map {
            for pads in map.values_mut() {
                match to {
                    Some(to) => {
                        for pad in pads.iter_mut() {
                            if pad.as_str() == from {
                                *pad = to.to_string();
                            }
                        }
                    }
                    None => pads.retain(|pad| pad.as_str() != from),
                }
            }
        }
    }
}

/// A pin or pad number: 1 to [`MAX_NUMBER`] characters, no surrounding spaces, no control
/// characters.
pub fn check_number(what: &str, number: &str) -> Result<(), String> {
    if number.is_empty() || number.trim() != number {
        return Err(format!("a {what} number must not be empty or start or end with a space (got `{number}`)"));
    }
    check_text(&format!("{what} number"), number, MAX_NUMBER)
}

/// A point within [`MAX_COORD`] of the origin on both axes.
pub fn check_point(what: &str, [x, y]: [f64; 2]) -> Result<(), String> {
    if !(x.is_finite() && y.is_finite() && x.abs() <= MAX_COORD && y.abs() <= MAX_COORD) {
        return Err(format!("{what} must be within {MAX_COORD} mm of the origin"));
    }
    Ok(())
}

/// The smallest box around `points`: `[[x0, y0], [x1, y1]]`, `None` for no points.
pub fn bbox(points: impl IntoIterator<Item = [f64; 2]>) -> Option<[[f64; 2]; 2]> {
    points.into_iter().fold(None, |acc, [x, y]| match acc {
        None => Some([[x, y], [x, y]]),
        Some([[x0, y0], [x1, y1]]) => Some([[x0.min(x), y0.min(y)], [x1.max(x), y1.max(y)]]),
    })
}

/// An electronic component's name: 1 to [`MAX_TEXT`] characters, no surrounding spaces and no
/// line breaks or other control characters.
pub fn check_name(name: &str) -> Result<(), String> {
    check_item_name("electronic component", name)
}

/// The name of a `kind` of record (`symbol`, `footprint`, `variant`…), by the rules of
/// [`check_name`].
pub fn check_item_name(kind: &str, name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err(format!("a {kind} needs a name"));
    }
    if name.trim() != name {
        return Err(format!("the name `{name}` starts or ends with a space"));
    }
    check_text("name", name, MAX_TEXT)
}

/// A designator prefix: 1 to [`MAX_PREFIX`] ASCII letters.
pub fn check_prefix(prefix: &str) -> Result<(), String> {
    if prefix.is_empty() || prefix.len() > MAX_PREFIX || !prefix.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(format!("the designator prefix `{prefix}` must be 1 to {MAX_PREFIX} letters, such as R, C, U or LED"));
    }
    Ok(())
}

/// A text of at most `max` characters, with no line breaks or other control characters.
pub fn check_text(what: &str, text: &str, max: usize) -> Result<(), String> {
    if text.chars().count() > max {
        return Err(format!("`{what}` is longer than {max} characters"));
    }
    if text.chars().any(char::is_control) {
        return Err(format!("`{what}` has a line break or another control character"));
    }
    Ok(())
}

/// What the file holds under `ecad`: an object with these keys and no others.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Saved {
    symbols: Vec<SymbolItem>,
    footprints: Vec<FootprintItem>,
    packages: Vec<PackageItem>,
    components: Vec<ElectronicComponent>,
    next_id: u64,
    #[serde(deserialize_with = "object")]
    schematic: Schematic,
    #[serde(deserialize_with = "object")]
    board: Board,
    #[serde(deserialize_with = "object")]
    intent: Intent,
    #[serde(deserialize_with = "object")]
    info: DesignInfo,
}

/// An object only, as [`Electronics`] is read: a derived reader would also take a list.
fn object<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(d: D) -> Result<T, D::Error> {
    let object = serde_json::Map::<String, serde_json::Value>::deserialize(d)?;
    serde_json::from_value(serde_json::Value::Object(object)).map_err(serde::de::Error::custom)
}

/// An object only (a derived reader would also take a list, as the fields in order), with only
/// the keys this build knows: a newer build's parts are refused, never dropped. The records are
/// checked as the commands check them, and the id counters never fall behind an id in use.
impl<'de> Deserialize<'de> for Electronics {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let object = serde_json::Map::<String, serde_json::Value>::deserialize(d)?;
        let saved: Saved = serde_json::from_value(serde_json::Value::Object(object)).map_err(D::Error::custom)?;
        let mut out = Electronics {
            symbols: saved.symbols,
            footprints: saved.footprints,
            packages: saved.packages,
            components: saved.components,
            next_id: saved.next_id,
            schematic: saved.schematic,
            board: saved.board,
            intent: saved.intent,
            info: saved.info,
        };
        let sheet = &mut out.schematic;
        if sheet.has_content() {
            sheet.next_id = sheet.next_id.max(sheet.floor());
        }
        let board = &mut out.board;
        if board.has_content() {
            board.next_id = board.next_id.max(board.floor());
        }
        out.validate().map_err(D::Error::custom)?;
        let floor = out.ids().map(|id| id + 1).max().unwrap_or(0);
        out.next_id = out.next_id.max(floor);
        Ok(out)
    }
}

#[cfg(test)]
#[path = "ecad_tests.rs"]
mod tests;
