//! Silkscreen and pad-to-pour checks: what the fab prints, and how pours meet the pads soldered
//! to them.
//! - `silk.size`: a text on silk (a footprint's name or value, a drawing; hidden ones aside) lower
//!   than `min_height`, or whose stroke (a tenth of its height, as the board draws text) is
//!   thinner than the fab's least (`stroke_min`: 0.15 mm on the standard profile, 0.10 mm on the
//!   fine and HDI ones, unless the board sets it): the fab removes thinner strokes.
//! - `silk.clip`: a text that would lose stroke to the fab's clipping: what lands on exposed
//!   copper (pads, and open vias of `open_via` or more) and the legend clearance round it (the
//!   rules' `silk_to_pad`), on holes and `hole_margin` round them, within `edge_band` of the
//!   board's edge or off the board. Tented, plugged and filled vias are printed over.
//! - `silk.designator`: a footprint's name whose nearest body is another part's, or whose next
//!   body is less than `ratio` times as far as its own; a name over a body or a courtyard
//!   (hidden once the parts are on); a drawing's text on the bottom silk that isn't mirrored.
//!   Infos (cosmetic): a drawing's text turned 180° or 270°; names whose heights stray more than
//!   `height_tol` from the board's usual one (one finding for the board).
//! - `silk.polarity`: a part that can go on the wrong way (a diode's cathode or a part's
//!   positive pin, by the pin's name, or pin 1 of a part with more than two pads; a two-pad D or
//!   LED part whose pins name no cathode is not applicable) with no mark that tells its ends apart: a silk
//!   mark within `reach` of the pin, nearer it than the other end, not repeated across the part,
//!   at least `min_size` across, not under the body, and surviving the fab's clipping and its
//!   least stroke. Critical for symmetric pad patterns (a part that fits turned round) and on
//!   hand-assembled boards; an error there when there is no mark at all. Parts keyed by
//!   unplated pegs that don't fit turned round are left out, as are parts not fitted.
//! - `dfm.pad_to_pour`: the one pad-to-pour table: a through-hole pin relieved with fewer than
//!   `spokes` joined spokes, or joined solid to planes on two or more layers (or to a heavy
//!   pour); a chip of 0603 or smaller joined solid to a pour at one end only; a decoupling
//!   capacitor's pad relieved with no via of its net within `via_reach`. Reports the spokes'
//!   resistance (`sheet` per square, 1 oz). Pours with no net (shields) are left out; exposed
//!   pads are `pi.exposed_pad`'s, and the table's high-current row (spokes as wide together as
//!   the IPC-2152 width) is `pi.current`'s spoke sets, so a relief too narrow for its current is
//!   reported once.
//! - `dfm.tombstone`: a two-pad chip of 0402 or smaller (0603: info; 0201 and smaller: error)
//!   whose pads sink heat unequally: each pad with the same-layer copper joined to it within
//!   `window` of it (tracks touching it or such a track; a pour it is joined to solid, or its
//!   spokes), more than `ratio` times the other. Vias don't count, nor do boards for hand
//!   assembly, nor parts with both pads on one pour.
//!
//! The package of a two-pad chip comes from its footprint's name (an EIA code such as 0402),
//! else its fabrication outline, else its pads' span.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::super::airwires::BoardPad;
use super::super::board::{ViaProtection, footprint_box};
use super::super::checks::{Severity, gap};
use super::super::copper::{disc, drill_core, graphic_cores, inside, pad_core};
use super::super::drc::{Bound, Class, DrcItem};
use super::super::footprint::{Core, FootprintItem, PadKind, Side};
use super::super::graphics::{BoardLayer, Graphic, GraphicType, shapes_bbox};
use super::super::grid::{Grid, boxes_meet, grow};
use super::super::intent::PartRole;
use super::super::layers::{CopperLayer, DrawLayer};
use super::super::names;
use super::super::path;
use super::super::stackup::StackKind;
use super::catalog::{Code, Family, Param};
use super::geom::distance;
use super::{Cx, Draft, num};

const SIZE: &str = "silk.size";
const CLIP: &str = "silk.clip";
const DESIGNATOR: &str = "silk.designator";
const POLARITY: &str = "silk.polarity";
const PAD_TO_POUR: &str = "dfm.pad_to_pour";
const TOMBSTONE: &str = "dfm.tombstone";

/// A text's stroke as a share of its height, as the board draws text.
const STROKE_SHARE: f64 = 0.1;
/// The grid cell of the zones and bodies, in millimetres.
const CELL: f64 = 2.0;
/// 1 oz/ft² of copper, in millimetres.
const OZ: f64 = 0.0347;

const fn param(name: &'static str, default: f64, units: &'static str, min: f64, max: f64, source: &'static str, about: &'static str) -> Param {
    Param { name, default, units, source, min, max, verify: false, about }
}

const CODES: &[Code] = &[
    Code {
        code: SIZE,
        class: Class::Practical,
        severity: Severity::Warning,
        worst: None,
        needs: "geometry; profile: the fab's least legend height and stroke",
        section: "silkscreen-assembly#sizes",
        about: "a silk text lower than the least height, or with a stroke thinner than the fab prints",
        principle: "Text below the fab's least height or stroke comes out unreadable or not at all: the fab removes thinner strokes.",
        params: &[
            param("min_height", 1.0, "mm", 0.1, 10.0, "Eurocircuits; JLCPCB; Hausherr (IPC-7351 nominal 1.5 mm)", "the lowest a silk text may be"),
            param(
                "stroke_min",
                0.15,
                "mm",
                0.01,
                1.0,
                "JLCPCB and PCBWay 0.15 mm; Eurocircuits 0.10 mm (the fine and HDI profiles take 0.10 unless the board sets it)",
                "the thinnest stroke the fab prints; a text's stroke is a tenth of its height",
            ),
            param("ratio_min", 0.1, "ratio", 0.01, 1.0, "IPC-7351 via Hausherr", "the least stroke for a text's height"),
            param("ratio_max", 0.25, "ratio", 0.01, 1.0, "IPC-7351 via Hausherr", "the most stroke for a text's height"),
        ],
    },
    Code {
        code: CLIP,
        class: Class::Practical,
        severity: Severity::Warning,
        worst: None,
        needs: "geometry; profile: the legend clearance (the rules' silk_to_pad)",
        section: "silkscreen-assembly#clipping",
        about: "a silk text that loses stroke to the fab's clipping: on exposed copper, holes, open vias or the board's edge",
        principle: "Fabs remove legend that lands on exposed copper, holes and the board edge, so a designator printed there arrives with strokes missing.",
        params: &[
            param("hole_margin", 0.1, "mm", 0.0, 5.0, "Eurocircuits: legend 0.10 mm off holes", "the legend's clearance round a hole"),
            param(
                "open_via",
                0.4,
                "mm",
                0.0,
                10.0,
                "Eurocircuits: open vias over about 0.4 mm",
                "an open (untented) via this wide or wider clips the legend",
            ),
            param(
                "edge_band",
                0.25,
                "mm",
                0.0,
                5.0,
                "Eurocircuits: 0.25 mm from a routed edge (0.45 mm scored)",
                "the legend's clearance from the board's edge",
            ),
        ],
    },
    Code {
        code: DESIGNATOR,
        class: Class::Practical,
        severity: Severity::Warning,
        worst: None,
        needs: "geometry: the footprints' bodies (fabrication outline, else courtyard)",
        section: "silkscreen-assembly#designators",
        about: "a name nearer another part's body than its own's (or not clearly nearer), over a body or courtyard, bottom text not mirrored; turned or uneven text (info)",
        principle: "A designator is for people: it has to sit unambiguously next to its own part, stay visible once the parts are on, and read the same way across the board.",
        params: &[
            param(
                "ratio",
                1.4,
                "ratio",
                1.0,
                10.0,
                "the guide's designator rule",
                "the next body must be at least this many times as far from the name as its own",
            ),
            param(
                "height_tol",
                0.1,
                "ratio",
                0.0,
                1.0,
                "the guide: one height for the whole board",
                "how far a name's height may stray from the board's usual one",
            ),
            param("overlap", 0.05, "mm", 0.0, 5.0, "judgment", "a name reaching less than this over a body or courtyard only touches it"),
        ],
    },
    Code {
        code: POLARITY,
        class: Class::Practical,
        severity: Severity::Warning,
        worst: Some((Class::Critical, Severity::Error)),
        needs: "library: the cathode pin's name or pin 1, the silk graphics; intent: hand assembly",
        section: "silkscreen-assembly#polarity",
        about: "a part that can go on the wrong way with no silk mark near its pin 1 or cathode that tells its ends apart, survives clipping and shows once placed; critical for symmetric pad patterns and hand-assembled boards",
        principle: "A part that can be placed turned round needs a visible, unambiguous mark at pin 1 or the cathode, outside its body, or nobody can check it after placement.",
        params: &[
            param("reach", 1.0, "mm", 0.0, 10.0, "Hausherr", "how near pin 1 or the cathode the mark must be"),
            param("min_size", 0.5, "mm", 0.0, 10.0, "Hausherr: 0.5 mm on silk", "the smallest a mark may be across"),
        ],
    },
    Code {
        code: PAD_TO_POUR,
        class: Class::Practical,
        severity: Severity::Warning,
        worst: None,
        needs: "geometry and pours; library: through-hole pads, package sizes; intent: decoupling parts",
        section: "routing-dfm#pad-to-pour",
        about: "a pad joined to a pour against the pad-to-pour table: a through-hole pin with too few spokes or solid in two planes, a small chip solid at one end, a decoupling pad relieved with no via (spokes too narrow for a current are pi.current's)",
        principle: "How a pour meets a pad trades solderability against current and inductance: through-hole pins need reliefs, small chips need the same at both ends, decoupling needs a via and high current needs width.",
        params: &[
            param(
                "spokes",
                2.0,
                "spokes",
                1.0,
                8.0,
                "the pad-to-pour table: at least 2 joined spokes",
                "the fewest spokes a through-hole pin's relief may keep",
            ),
            param("via_reach", 0.5, "mm", 0.0, 10.0, "the pad-to-pour table", "a decoupling pad's via must be this close to a relieved pad"),
            param(
                "heavy",
                2.0,
                "oz",
                0.5,
                10.0,
                "judgment: 2 oz and heavier",
                "a through-hole pin solid in a pour on copper this heavy is hard to solder",
            ),
            param("sheet", 0.49, "mΩ/sq", 0.01, 10.0, "1 oz copper", "the copper's sheet resistance, for the spokes' resistance reported"),
        ],
    },
    Code {
        code: TOMBSTONE,
        class: Class::Practical,
        severity: Severity::Warning,
        worst: Some((Class::Practical, Severity::Error)),
        needs: "geometry and pours; library: the package size; intent: hand assembly",
        section: "routing-dfm#pad-to-pour",
        about: "a two-pad chip of 0402 or smaller whose pads sink heat unequally (0603: info; 0201 and smaller: error)",
        principle: "When one end of a small chip sits on much more copper, its solder melts later and surface tension stands the part on end.",
        params: &[
            param("ratio", 2.0, "ratio", 1.0, 100.0, "Nordson EFD; Reno", "the most one pad's copper (with the pad) may be of the other's"),
            param("window", 1.5, "mm", 0.1, 10.0, "Nordson EFD", "how far round a pad its copper counts"),
        ],
    },
];

pub(super) const FAMILY: Family = Family { module: "silk_dfm", title: "silkscreen and pad-to-pour", codes: CODES, run };

fn run(cx: &mut Cx) {
    let silk = [SIZE, CLIP, DESIGNATOR].iter().any(|c| cx.wants(c));
    let labels = if silk { labels(cx) } else { Vec::new() };
    if cx.wants(SIZE) {
        size(cx, &labels);
    }
    if cx.wants(CLIP) {
        clip(cx, &labels);
    }
    if cx.wants(DESIGNATOR) {
        designators(cx, &labels);
    }
    if cx.wants(POLARITY) {
        polarity(cx);
    }
    if cx.wants(PAD_TO_POUR) {
        pad_to_pour(cx);
    }
    if cx.wants(TOMBSTONE) {
        tombstones(cx);
    }
}

/// What a text on silk is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Name,
    Value,
    Drawing,
}

/// A text the board prints on silk.
struct Label {
    kind: Kind,
    item: DrcItem,
    /// The footprint whose name or value it is, by index into the board's.
    owner: Option<usize>,
    text: String,
    side: Side,
    /// Its box's corners, counter-clockwise.
    corners: Vec<[f64; 2]>,
    height: f64,
    /// A drawing's turn and whether it is mirrored.
    rotation: f64,
    mirror: bool,
}

impl Label {
    fn bbox(&self) -> [[f64; 2]; 2] {
        super::super::bbox(self.corners.iter().copied()).unwrap_or([[0.0; 2]; 2])
    }

    fn centre(&self) -> [f64; 2] {
        let [a, b] = self.bbox();
        [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
    }

    /// `R1`'s name, `R1`'s value, drawing 12.
    fn says(&self) -> String {
        match (self.kind, &self.item) {
            (Kind::Name, i) => format!("{}'s name", i.reference()),
            (Kind::Value, i) => format!("{}'s value", i.reference()),
            (Kind::Drawing, i) => format!("{} (\"{}\")", i.reference(), self.text),
        }
    }
}

/// A footprint's courtyard box on the board (its courtyard graphics', else its whole box's).
fn courtyard_of(fp: &FootprintItem, placement: &super::super::place::Placement) -> Option<[[f64; 2]; 2]> {
    let local = shapes_bbox(fp.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Courtyard))).or_else(|| footprint_box(fp))?;
    Some(placement.bbox(local))
}

/// A footprint's body box on the board: its fabrication outline's, else its courtyard's.
fn body_of(fp: &FootprintItem, placement: &super::super::place::Placement) -> Option<[[f64; 2]; 2]> {
    match shapes_bbox(fp.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Fab) && g.kind != GraphicType::Text)) {
        Some(local) => Some(placement.bbox(local)),
        None => courtyard_of(fp, placement),
    }
}

/// The corners of a text box from its lower left `at`, `w` × `h`, unturned.
fn box_corners(at: [f64; 2], w: f64, h: f64) -> Vec<[f64; 2]> {
    vec![at, [at[0] + w, at[1]], [at[0] + w, at[1] + h], [at[0], at[1] + h]]
}

/// The texts the board prints on silk where the board shows them: footprints' shown names and
/// values (at their own spot, else their footprint's placeholder's, else round the courtyard on
/// the side with the most room, as the board view lays them out), and drawings' texts.
fn labels(cx: &mut Cx) -> Vec<Label> {
    let (ecad, board) = (cx.ecad, cx.board);
    if !cx.spend(board.footprints.len().saturating_mul(16).saturating_add(board.graphics.len())) {
        return Vec::new();
    }
    let mut out: Vec<(Label, Option<usize>)> = Vec::new();
    let mut boxes: Vec<([[f64; 2]; 2], Side)> = Vec::new();
    let mut sizes: Vec<[f64; 2]> = Vec::new();
    let mut across: Vec<[f64; 2]> = Vec::new();
    for (fi, f) in board.footprints.iter().enumerate() {
        let Some(placed) = ecad.placed(f) else { continue };
        let placement = f.placement();
        let side = if placement.mirror { Side::Bottom } else { Side::Top };
        let fp = placed.resolved.footprint;
        let court = courtyard_of(fp, &placement);
        let k = court.map(|c| {
            boxes.push((c, side));
            sizes.push([0.0; 2]);
            across.push([0.0; 2]);
            boxes.len().saturating_sub(1)
        });
        let item = DrcItem::Footprint { id: f.id, designator: placed.part.designator.clone() };
        for (kind, shown, own) in [(Kind::Name, f.show_name, f.name_at), (Kind::Value, f.show_value, f.value_at)] {
            if !shown {
                continue;
            }
            let (text, mark) = match kind {
                Kind::Value => (placed.part.value.clone().unwrap_or_else(|| placed.resolved.component.value.clone()), "{value}"),
                _ => (placed.part.designator.clone(), "{designator}"),
            };
            if text.is_empty() {
                continue;
            }
            let placeholder = fp.graphics.iter().find(|g| g.layer == Some(BoardLayer::Silkscreen) && g.kind == GraphicType::Text && g.text == mark);
            let height = board
                .name_height
                .or_else(|| placeholder.and_then(|g| g.height))
                .unwrap_or_else(|| court.map_or(names::MAX_AUTO_HEIGHT, names::auto_height));
            let spot = match (own, placeholder.and_then(|g| placement.graphic(g).at)) {
                (Some(o), _) => Some([f.at[0] + o[0], f.at[1] + o[1]]),
                (None, at) => at,
            };
            let width = names::text_width(height, text.chars().count());
            if spot.is_none()
                && let Some(k) = k
            {
                match sizes.get_mut(k) {
                    Some(main) if main[1] == 0.0 => *main = [width, height],
                    Some(_) => {
                        if let Some(a) = across.get_mut(k) {
                            *a = [width, height];
                        }
                    }
                    None => {}
                }
            }
            let at = spot.unwrap_or(f.at);
            let label = Label {
                kind,
                item: item.clone(),
                owner: Some(fi),
                text,
                side,
                corners: box_corners(at, width, height),
                height,
                rotation: 0.0,
                mirror: false,
            };
            out.push((label, if spot.is_none() { k } else { None }));
        }
    }
    if out.iter().any(|(_, k)| k.is_some()) {
        let around = names::sides(&boxes, &sizes, &across);
        for (label, k) in &mut out {
            let Some(k) = *k else { continue };
            let (Some((court, _)), Some(a)) = (boxes.get(k), around.get(k)) else { continue };
            let goes_across = across.get(k).is_some_and(|s| s[1] > 0.0);
            let a = if label.kind == Kind::Value && goes_across { a.opposite() } else { *a };
            let w = names::text_width(label.height, label.text.chars().count());
            let at = names::spot(*court, a, [w, label.height]);
            label.corners = box_corners(at, w, label.height);
        }
    }
    let mut labels: Vec<Label> = out.into_iter().map(|(l, _)| l).collect();
    for g in &board.graphics {
        let side = match g.layer {
            DrawLayer::SilkscreenTop => Side::Top,
            DrawLayer::SilkscreenBottom => Side::Bottom,
            _ => continue,
        };
        let t = &g.graphic;
        if t.kind != GraphicType::Text {
            continue;
        }
        let corners = t.text_box();
        if corners.len() < 4 {
            continue;
        }
        labels.push(Label {
            kind: Kind::Drawing,
            item: DrcItem::Graphic(g.id),
            owner: None,
            text: t.text.clone(),
            side,
            corners,
            height: t.height.unwrap_or(0.0),
            rotation: t.rotation,
            mirror: g.mirror,
        });
    }
    labels
}

/// The fix for a text: its name's height for the board, its value hidden, a drawing edited.
fn text_fix(d: Draft, l: &Label, height: f64) -> Draft {
    match (l.kind, &l.item) {
        // One name's own fix first; raising every name on a dense board crowds the rest, so
        // the board-wide one is a choice to make (no parameters).
        (Kind::Name, i) => d
            .fix(
                "ecad.board.edit",
                json!({"footprints": [{"footprint": i.reference(), "show_name": false}]}),
                format!("take {}'s name off the silk and leave it to the assembly drawing", i.reference()),
            )
            .fix(
                "ecad.board.names",
                Value::Null,
                format!("or, where the board has room, set every name's height to {} mm (ecad.board.names {{height}}); on a dense board that crowds and clips the rest", num(height)),
            ),
        (Kind::Value, i) => d.fix(
            "ecad.board.edit",
            json!({"footprints": [{"footprint": i.reference(), "show_value": false}]}),
            format!("take {}'s value off the silk: values go stale and add clutter", i.reference()),
        ),
        (Kind::Drawing, i) => d.fix("ecad.board.draw.edit", Value::Null, format!("make {} at least {} mm high", i.reference(), num(height))),
    }
}

/// The fab's least stroke: the board's own, else the profile's.
fn stroke_min(cx: &Cx) -> f64 {
    if cx.board.quality_params.contains_key(&format!("{SIZE}.stroke_min")) {
        return cx.param(SIZE, "stroke_min");
    }
    match cx.board.fab_profile.as_ref().map(|p| p.id.as_str()) {
        Some("fine" | "hdi") => 0.10,
        _ => cx.param(SIZE, "stroke_min"),
    }
}

fn size(cx: &mut Cx, labels: &[Label]) {
    let (min_height, ratio_min, ratio_max) = (cx.param(SIZE, "min_height"), cx.param(SIZE, "ratio_min"), cx.param(SIZE, "ratio_max"));
    let stroke_min = stroke_min(cx);
    let profile = cx.board.fab_profile.as_ref().map_or("no profile".to_string(), |p| format!("the {} profile", p.id));
    // The height that meets both.
    let need = (min_height.max(stroke_min / STROKE_SHARE) * 20.0).ceil() / 20.0;
    for l in labels {
        let h = l.height;
        if !(h.is_finite() && h > 0.0) {
            continue;
        }
        let stroke = h * STROKE_SHARE;
        let keys = [l.item.key(), format!("{:?}", l.kind).to_lowercase()];
        let what = l.says();
        let regime = format!("{profile}: text {} mm or higher, stroke {} mm or thicker", num(min_height), num(stroke_min));
        if h < min_height - 1e-9 || stroke < stroke_min - 1e-9 {
            let low = h < min_height - 1e-9;
            cx.add(SIZE, &keys, || {
                let d = if low {
                    Draft::new(l.centre(), vec![l.item.clone()], format!("{what} is {} mm high, lower than {} mm", num(h), num(min_height)))
                        .measure(h, min_height, "mm", Bound::Min)
                        .param("min_height")
                } else {
                    Draft::new(
                        l.centre(),
                        vec![l.item.clone()],
                        format!(
                            "{what}'s stroke is {} mm, thinner than the {} mm the fab prints: it needs {} mm of height",
                            num(stroke),
                            num(stroke_min),
                            num(need)
                        ),
                    )
                    .variant("stroke")
                    .measure(stroke, stroke_min, "mm", Bound::Min)
                    .param("stroke_min")
                };
                text_fix(d.arg("text", what.clone()).arg("need", num(need)).regime(regime), l, need)
            });
            continue;
        }
        let ratio = stroke / h;
        if ratio < ratio_min - 1e-6 || ratio > ratio_max + 1e-6 {
            let bound = if ratio < ratio_min { Bound::Min } else { Bound::Max };
            let limit = if ratio < ratio_min { ratio_min } else { ratio_max };
            cx.add(SIZE, &keys, || {
                Draft::new(
                    l.centre(),
                    vec![l.item.clone()],
                    format!("{what}'s stroke is {} of its height, outside {}–{}", num(ratio), num(ratio_min), num(ratio_max)),
                )
                .severity(Severity::Info)
                .class(Class::Cosmetic)
                .variant("ratio")
                .arg("text", what.clone())
                .measure(ratio, limit, "ratio", bound)
                .param(if ratio < ratio_min { "ratio_min" } else { "ratio_max" })
            });
        }
    }
}

/// Where the fab clips the legend on one side: exposed copper and holes, each with its margin.
struct Zones {
    list: Vec<(Core, f64, DrcItem)>,
    grid: Grid,
    /// The board's edge, and the band kept clear inside it.
    edge: Vec<[f64; 2]>,
    band: f64,
}

impl Zones {
    fn new(cx: &Cx, side: Side) -> Zones {
        let (board, nets) = (cx.board, cx.nets);
        let pad_margin = board.rules.silk_to_pad.max(0.0);
        let hole_margin = cx.param(CLIP, "hole_margin").max(0.0);
        let open_via = cx.param(CLIP, "open_via");
        let layer: CopperLayer = side.into();
        let mut list: Vec<(Core, f64, DrcItem)> = Vec::new();
        for p in &nets.pads {
            let item = DrcItem::Pad { footprint: p.footprint, number: p.pad.number.clone(), reference: p.reference() };
            if p.on_side(side) {
                list.push((pad_core(p), pad_margin, item.clone()));
            }
            if let Some(d) = drill_core(p) {
                list.push((d, hole_margin, item));
            }
        }
        for h in &board.holes {
            list.push((disc(h.at, h.drill / 2.0), hole_margin, DrcItem::Hole(h.id)));
        }
        for v in &board.vias {
            if v.protection == ViaProtection::Open && v.diameter >= open_via && v.on(layer, board.copper_layers) {
                list.push((disc(v.at, v.diameter / 2.0), pad_margin, DrcItem::Via(v.id)));
            }
        }
        let mut grid = Grid::new(CELL);
        for (i, (core, m, _)) in list.iter().enumerate() {
            grid.insert(i, grow(core.bbox(), *m));
        }
        let edge = if board.outline.is_empty() { Vec::new() } else { path::tessellate(&board.outline, &board.outline_curves, true, 0.01) };
        Zones { list, grid, edge, band: cx.param(CLIP, "edge_band").max(0.0) }
    }

    /// What clips the legend at `p`: a zone (by index), the edge (`None` inside `Some`), or nothing.
    fn at(&self, p: [f64; 2]) -> Option<Option<usize>> {
        let point = Core { points: vec![p], radius: 0.0 };
        for i in self.grid.near(p) {
            if let Some((core, m, _)) = self.list.get(i)
                && gap(core, &point) < *m - 1e-9
            {
                return Some(Some(i));
            }
        }
        // Off the board, or within the band inside its edge.
        if self.edge.len() >= 3 && (!inside(p, &self.edge) || edge_distance(p, &self.edge) < self.band - 1e-9) {
            return Some(None);
        }
        None
    }

    /// Whether a box is on the board and clear of the edge band (to skip the edge test there).
    fn edge_far(&self, bbox: [[f64; 2]; 2]) -> bool {
        let c = [(bbox[0][0] + bbox[1][0]) / 2.0, (bbox[0][1] + bbox[1][1]) / 2.0];
        let half = distance(bbox[0], bbox[1]) / 2.0;
        self.edge.len() < 3 || (inside(c, &self.edge) && edge_distance(c, &self.edge) > half + self.band + 0.01)
    }
}

/// How far `p` is from the closed outline `ring`.
fn edge_distance(p: [f64; 2], ring: &[[f64; 2]]) -> f64 {
    let n = ring.len();
    (0..n)
        .filter_map(|i| Some((*ring.get(i)?, *ring.get((i + 1) % n)?)))
        .map(|(a, b)| path::segment_distance(p, a, b, 0.0))
        .fold(f64::INFINITY, f64::min)
}

/// Points over a text's box, its outer half stroke left out, `step` apart.
fn box_points(l: &Label, step: f64) -> Vec<[f64; 2]> {
    let (Some(a), Some(b), Some(d)) = (l.corners.first(), l.corners.get(1), l.corners.get(3)) else { return Vec::new() };
    let u = [b[0] - a[0], b[1] - a[1]];
    let v = [d[0] - a[0], d[1] - a[1]];
    let (lu, lv) = (distance(*a, *b), distance(*a, *d));
    if !(lu > 0.0 && lv > 0.0) {
        return Vec::new();
    }
    let inset = l.height * STROKE_SHARE / 2.0;
    let (nu, nv) = (((lu / step).ceil() as usize).clamp(1, 400), ((lv / step).ceil() as usize).clamp(1, 400));
    let mut out = Vec::with_capacity((nu + 1) * (nv + 1));
    for i in 0..=nu {
        for j in 0..=nv {
            let (su, sv) = (i as f64 / nu as f64, j as f64 / nv as f64);
            // Inset: from `inset` to the length less it.
            let (tu, tv) = ((inset + su * (lu - 2.0 * inset).max(0.0)) / lu, (inset + sv * (lv - 2.0 * inset).max(0.0)) / lv);
            out.push([a[0] + u[0] * tu + v[0] * tv, a[1] + u[1] * tu + v[1] * tv]);
        }
    }
    out
}

fn clip(cx: &mut Cx, labels: &[Label]) {
    if labels.is_empty() {
        return;
    }
    let zones = [Zones::new(cx, Side::Top), Zones::new(cx, Side::Bottom)];
    for l in labels {
        let Some(z) = zones.get(usize::from(l.side == Side::Bottom)) else { continue };
        let step = (l.height * STROKE_SHARE).clamp(0.02, 0.1);
        let pts = box_points(l, step);
        if pts.is_empty() {
            continue;
        }
        let near_edge = !z.edge_far(l.bbox());
        let cost = pts.len().saturating_mul(if near_edge { z.edge.len().saturating_add(4) } else { 4 });
        if !cx.spend(cost) {
            return;
        }
        let near = z.grid.near_box(l.bbox(), z.list.len());
        if near.is_empty() && !near_edge {
            continue;
        }
        let mut clipped = 0usize;
        // What clips most, by zone (the edge as `None`).
        let mut by: BTreeMap<Option<usize>, usize> = BTreeMap::new();
        for p in &pts {
            let hit = if near_edge { z.at(*p) } else { z.at_zone(*p) };
            if let Some(what) = hit {
                clipped += 1;
                *by.entry(what).or_default() += 1;
            }
        }
        if clipped == 0 {
            continue;
        }
        let share = clipped as f64 / pts.len() as f64;
        let worst = by.iter().max_by_key(|(_, n)| **n).and_then(|(w, _)| *w);
        let clipper = worst.and_then(|i| z.list.get(i)).map(|(_, _, item)| item.clone());
        let on = match &clipper {
            Some(item @ DrcItem::Pad { .. }) => format!("the mask opening of pad {}", item.reference()),
            Some(item @ DrcItem::Via(_)) => format!("open {}", item.reference()),
            Some(item) => item.reference(),
            None => "the board's edge (off the board or within its edge band)".into(),
        };
        let mut keys = vec![l.item.key(), format!("{:?}", l.kind).to_lowercase()];
        let mut items = vec![l.item.clone()];
        if let Some(c) = &clipper {
            keys.push(c.key());
            items.push(c.clone());
        }
        let what = l.says();
        cx.add(CLIP, &keys, || {
            let d = Draft::new(
                l.centre(),
                items,
                format!("{what} loses {} % of its box to {on}: the fab clips the legend there", num((share * 100.0).round())),
            )
            .measure(share, 0.0, "ratio", Bound::Max)
            .param("")
            .arg("text", what.clone())
            .arg("on", on.clone())
            .arg("percent", num((share * 100.0).round()));
            match (l.kind, &l.item) {
                (Kind::Drawing, i) => {
                    d.fix("ecad.board.draw.edit", Value::Null, format!("move {} off exposed copper, holes and the edge", i.reference()))
                }
                (kind, i) => {
                    let field = if kind == Kind::Name { "name_at" } else { "value_at" };
                    d.fix(
                        "ecad.board.edit",
                        Value::Null,
                        format!("move {}'s text off exposed copper, holes and the edge ({field}), or hide it", i.reference()),
                    )
                }
            }
        });
    }
}

impl Zones {
    /// [`Zones::at`] without the edge.
    fn at_zone(&self, p: [f64; 2]) -> Option<Option<usize>> {
        let point = Core { points: vec![p], radius: 0.0 };
        self.grid.near(p).find(|i| self.list.get(*i).is_some_and(|(core, m, _)| gap(core, &point) < *m - 1e-9)).map(Some)
    }
}

/// How far `p` is from the box `[lo, hi]` (0 inside).
fn box_distance(p: [f64; 2], [lo, hi]: [[f64; 2]; 2]) -> f64 {
    let dx = (lo[0] - p[0]).max(p[0] - hi[0]).max(0.0);
    let dy = (lo[1] - p[1]).max(p[1] - hi[1]).max(0.0);
    dx.hypot(dy)
}

/// How far two boxes overlap along each axis (negative: apart).
fn overlap([a0, a1]: [[f64; 2]; 2], [b0, b1]: [[f64; 2]; 2]) -> f64 {
    let x = a1[0].min(b1[0]) - a0[0].max(b0[0]);
    let y = a1[1].min(b1[1]) - a0[1].max(b0[1]);
    x.min(y)
}

/// A footprint's body and courtyard on the board, its side and designator.
struct Body {
    index: usize,
    item: DrcItem,
    body: [[f64; 2]; 2],
    court: Option<[[f64; 2]; 2]>,
    side: Side,
}

fn bodies(cx: &Cx) -> Vec<Body> {
    let (ecad, board) = (cx.ecad, cx.board);
    let mut out = Vec::new();
    for (i, f) in board.footprints.iter().enumerate() {
        let Some(placed) = ecad.placed(f) else { continue };
        let placement = f.placement();
        let fp = placed.resolved.footprint;
        let Some(body) = body_of(fp, &placement) else { continue };
        out.push(Body {
            index: i,
            item: DrcItem::Footprint { id: f.id, designator: placed.part.designator.clone() },
            body,
            court: courtyard_of(fp, &placement),
            side: if placement.mirror { Side::Bottom } else { Side::Top },
        });
    }
    out
}

fn designators(cx: &mut Cx, labels: &[Label]) {
    let (ratio, height_tol, touch) = (cx.param(DESIGNATOR, "ratio"), cx.param(DESIGNATOR, "height_tol"), cx.param(DESIGNATOR, "overlap"));
    let bodies = bodies(cx);
    if !cx.spend(bodies.len().saturating_add(labels.len()).saturating_mul(8)) {
        return;
    }
    let mut grid = Grid::new(CELL);
    for (i, b) in bodies.iter().enumerate() {
        grid.insert(
            i,
            b.court
                .map_or(b.body, |c| [[c[0][0].min(b.body[0][0]), c[0][1].min(b.body[0][1])], [c[1][0].max(b.body[1][0]), c[1][1].max(b.body[1][1])]]),
        );
    }
    let by_index: BTreeMap<usize, usize> = bodies.iter().enumerate().map(|(k, b)| (b.index, k)).collect();
    for l in labels.iter().filter(|l| l.kind == Kind::Name) {
        let Some(own) = l.owner.and_then(|o| by_index.get(&o)).and_then(|k| bodies.get(*k)) else { continue };
        let c = l.centre();
        let lb = l.bbox();
        let d_own = box_distance(c, own.body);
        let what = l.says();
        // Over a body or a courtyard: hidden once the parts are on.
        let near = grid.near_box(lb, bodies.len());
        if !cx.spend(near.len().saturating_add(1)) {
            return;
        }
        let over = near
            .iter()
            .filter_map(|k| bodies.get(*k))
            .filter(|b| b.side == l.side)
            .filter_map(|b| {
                let on_body = overlap(lb, b.body);
                let on_court = b.court.map_or(f64::NEG_INFINITY, |c| overlap(lb, c));
                let o = on_body.max(on_court);
                (o > touch).then_some((b, o, on_body > touch))
            })
            .max_by(|x, y| (x.0.index != own.index).cmp(&(y.0.index != own.index)).then(x.1.total_cmp(&y.1)));
        if let Some((b, o, on_body)) = over {
            let whose = if b.index == own.index { "its own part's".to_string() } else { format!("{}'s", b.item.reference()) };
            let part = if on_body { "body" } else { "courtyard" };
            let keys = [l.item.key(), "hidden".into()];
            let items = if b.index == own.index { vec![l.item.clone()] } else { vec![l.item.clone(), b.item.clone()] };
            cx.add(DESIGNATOR, &keys, || {
                Draft::new(c, items, format!("{what} lies {} mm over {whose} {part}: hidden once the parts are on", num(o)))
                    .variant("hidden")
                    .measure(o, touch, "mm", Bound::Max)
                    .param("overlap")
                    .arg("text", what.clone())
                    .arg("whose", whose.clone())
                    .arg("part", part)
                    .fix("ecad.board.edit", Value::Null, format!("move {what} next to its part, clear of bodies and courtyards (name_at)"))
            });
            continue;
        }
        if d_own <= 1e-9 {
            continue;
        }
        // The nearest other body.
        let look = grow([c, c], d_own * ratio + 0.01);
        let other = grid
            .near_box(look, bodies.len())
            .into_iter()
            .filter_map(|k| bodies.get(k))
            .filter(|b| b.side == l.side && b.index != own.index)
            .map(|b| (b, box_distance(c, b.body)))
            .min_by(|x, y| x.1.total_cmp(&y.1));
        let Some((b, d)) = other else { continue };
        if d >= ratio * d_own - 1e-9 {
            continue;
        }
        let keys = [l.item.key(), "ambiguous".into()];
        let nearer = d < d_own;
        cx.add(DESIGNATOR, &keys, || {
            let how = if nearer {
                format!("nearer {}'s body ({} mm) than its own ({} mm)", b.item.reference(), num(d), num(d_own))
            } else {
                format!("{} mm from its own body and {} mm from {}'s, less than {}× as far", num(d_own), num(d), b.item.reference(), num(ratio))
            };
            Draft::new(c, vec![l.item.clone(), b.item.clone()], format!("{what} is {how}: which part it names is unclear"))
                .variant("ambiguous")
                .measure(d / d_own, ratio, "ratio", Bound::Min)
                .param("ratio")
                .arg("text", what.clone())
                .arg("own", num(d_own))
                .arg("other", num(d))
                .fix(
                    "ecad.board.edit",
                    Value::Null,
                    format!("move {what} closer to its own part than to {} (name_at), or hide it", b.item.reference()),
                )
        });
    }
    // Drawings: mirrored on the bottom, read from the bottom or the right.
    for l in labels.iter().filter(|l| l.kind == Kind::Drawing) {
        let what = l.says();
        if l.side == Side::Bottom && !l.mirror {
            cx.add(DESIGNATOR, &[l.item.key(), "mirror".into()], || {
                Draft::new(l.centre(), vec![l.item.clone()], format!("{what} is on the bottom silk but not mirrored: it reads backwards from below"))
                    .variant("mirror")
                    .arg("text", what.clone())
                    .fix("ecad.board.draw.edit", Value::Null, "mirror the text")
            });
        }
        let turn = l.rotation.rem_euclid(360.0);
        if (turn - 180.0).abs() < 1.0 || (turn - 270.0).abs() < 1.0 {
            cx.add(DESIGNATOR, &[l.item.key(), "rotation".into()], || {
                Draft::new(l.centre(), vec![l.item.clone()], format!("{what} is turned {}°: it reads upside down", num(turn.round())))
                    .variant("rotation")
                    .severity(Severity::Info)
                    .class(Class::Cosmetic)
                    .arg("text", what.clone())
                    .arg("turn", num(turn.round()))
                    .fix("ecad.board.draw.edit", Value::Null, "turn the text to 0° or 90°, so it reads from the bottom or the right")
            });
        }
    }
    // One height for the board.
    let mut heights: Vec<f64> = labels.iter().filter(|l| l.kind == Kind::Name).map(|l| l.height).filter(|h| h.is_finite()).collect();
    heights.sort_by(f64::total_cmp);
    let Some(usual) = heights.get(heights.len() / 2).copied() else { return };
    if usual <= 0.0 {
        return;
    }
    let off: Vec<&Label> = labels.iter().filter(|l| l.kind == Kind::Name && ((l.height - usual) / usual).abs() > height_tol + 1e-9).collect();
    let Some(worst) = off.iter().map(|l| ((l.height - usual) / usual).abs()).max_by(f64::total_cmp) else { return };
    let items: Vec<DrcItem> = off.iter().map(|l| l.item.clone()).collect();
    let at = off.first().map_or([0.0, 0.0], |l| l.centre());
    let count = off.len();
    cx.add(DESIGNATOR, &["height".to_string()], || {
        Draft::new(
            at,
            items,
            format!("{count} names stray more than {} % from the board's usual name height of {} mm", num(height_tol * 100.0), num(usual)),
        )
        .variant("height")
        .severity(Severity::Info)
        .class(Class::Cosmetic)
        .measure(worst, height_tol, "ratio", Bound::Max)
        .param("height_tol")
        .arg("count", count.to_string())
        .arg("height", num(usual))
        .fix(
            "ecad.board.names",
            Value::Null,
            format!("give every name one height, {} mm (ecad.board.names {{height}}), where the board has room", num(usual)),
        )
    });
}

/// Whether a pin's name says it is a diode's cathode.
fn is_cathode(name: &str) -> bool {
    ["k", "cathode", "kathode"].iter().any(|n| name.trim().eq_ignore_ascii_case(n))
}

/// Whether a pin's name says it is a polarised part's positive pin.
fn is_positive(name: &str) -> bool {
    ["+", "positive"].iter().any(|n| name.trim().eq_ignore_ascii_case(n))
}

fn is_negative(name: &str) -> bool {
    ["-", "negative"].iter().any(|n| name.trim().eq_ignore_ascii_case(n))
}

/// A diode's anode, by its full name (a switch's `A` is no anode).
fn is_anode(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case("anode")
}

/// Each pad's image (holes too) when the footprint is turned half round about its pads' box's
/// centre, by index into its pads; `None` when some pad lands on no pad alike: the part doesn't
/// fit turned round.
fn half_turn(fp: &FootprintItem) -> Option<Vec<usize>> {
    let [lo, hi] = super::super::bbox(fp.pads.iter().flat_map(|p| p.bbox()))?;
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    fp.pads
        .iter()
        .map(|p| {
            let q = [2.0 * c[0] - p.at[0], 2.0 * c[1] - p.at[1]];
            fp.pads.iter().position(|o| {
                distance(o.at, q) <= 0.02
                    && o.kind == p.kind
                    && (o.size[0] - p.size[0]).abs() <= 0.02
                    && (o.size[1] - p.size[1]).abs() <= 0.02
                    && ((o.rotation - p.rotation).rem_euclid(180.0)).min(180.0 - (o.rotation - p.rotation).rem_euclid(180.0)) <= 0.5
            })
        })
        .collect()
}

/// Whether turning the part half round changes nothing electrically: each pad lands on a pad of
/// the same pin (by name) or the same net, but for at most one pair of pins swapped whose names
/// say no polarity (a crystal's two terminals, a switch's A and B). `pin` names a pad's pin
/// (its number when the symbol has none), `net` its net on the board.
fn turns_freely(fp: &FootprintItem, map: &[usize], pin: &dyn Fn(&str) -> String, net: &dyn Fn(&str) -> Option<usize>) -> bool {
    let mut swaps: BTreeSet<(String, String)> = BTreeSet::new();
    for (p, o) in fp.pads.iter().zip(map.iter().filter_map(|i| fp.pads.get(*i))) {
        if p.kind == PadKind::Hole {
            continue;
        }
        let (a, b) = (pin(&p.number), pin(&o.number));
        if a == b || net(&p.number).is_some_and(|n| net(&o.number) == Some(n)) {
            continue;
        }
        if [&a, &b].iter().any(|n| is_cathode(n) || is_positive(n) || is_negative(n) || is_anode(n)) {
            return false;
        }
        swaps.insert(if a < b { (a, b) } else { (b, a) });
    }
    swaps.len() <= 1
}

/// A silk drawing that may be a polarity mark: its strokes on the board, box, centre, how wide
/// across, and its stroke (0 when filled).
struct Mark {
    cores: Vec<Core>,
    bbox: [[f64; 2]; 2],
    centre: [f64; 2],
    size: f64,
    stroke: f64,
}

/// `g` (in the footprint's frame when `place` is a placement) as a mark.
fn mark_of(g: &Graphic, place: &dyn Fn([f64; 2]) -> [f64; 2]) -> Option<Mark> {
    let cores = match (g.kind, g.fill, g.center, g.radius) {
        (GraphicType::Circle, true, Some(c), Some(r)) => vec![disc(place(c), r + g.width / 2.0)],
        _ => graphic_cores(g, place),
    };
    let bbox = super::super::bbox(cores.iter().flat_map(|c| c.bbox()))?;
    let centre = [(bbox[0][0] + bbox[1][0]) / 2.0, (bbox[0][1] + bbox[1][1]) / 2.0];
    let size = (bbox[1][0] - bbox[0][0]).max(bbox[1][1] - bbox[0][1]);
    let filled = g.fill && matches!(g.kind, GraphicType::Circle | GraphicType::Rect | GraphicType::Polygon);
    let stroke = if g.kind == GraphicType::Text {
        g.height.unwrap_or(0.0) * STROKE_SHARE
    } else if filled {
        0.0
    } else {
        g.width
    };
    Some(Mark { cores, bbox, centre, size, stroke })
}

/// Points along a mark's strokes (its cores' corners and edges), at most `step` apart.
fn mark_points(m: &Mark, step: f64) -> Vec<[f64; 2]> {
    let mut out = Vec::new();
    for c in &m.cores {
        let n = c.points.len();
        if n == 1 {
            out.extend(c.points.iter().copied());
            continue;
        }
        for i in 0..n {
            let (Some(a), Some(b)) = (c.points.get(i), c.points.get((i + 1) % n)) else { continue };
            out.extend(super::geom::along(*a, *b, step));
        }
    }
    out
}

fn polarity(cx: &mut Cx) {
    let (ecad, board, nets) = (cx.ecad, cx.board, cx.nets);
    let (reach, min_size) = (cx.param(POLARITY, "reach"), cx.param(POLARITY, "min_size"));
    let thinnest = stroke_min(cx);
    let hand = cx.roles.intent.board.hand_assembly == Some(true);
    let mut zones: Option<[Zones; 2]> = None;
    for f in &board.footprints {
        let Some(placed) = ecad.placed(f) else { continue };
        if placed.part.dnp {
            continue;
        }
        let r = &placed.resolved;
        let fp = r.footprint;
        let copper: Vec<&super::super::footprint::Pad> = fp.pads.iter().filter(|p| p.kind != PadKind::Hole).collect();
        let named = |pin: &str, is: fn(&str) -> bool| r.symbol.pins.iter().any(|p| p.number == pin && is(&p.name));
        let pads_of = |is: fn(&str) -> bool| -> Vec<&str> {
            r.connections().into_iter().filter(|(pin, _)| named(pin, is)).flat_map(|(_, pads)| pads).collect()
        };
        let (cathodes, positives) = (pads_of(is_cathode), pads_of(is_positive));
        let (target, what) = match (cathodes.first(), positives.first()) {
            (Some(c), _) => (fp.pad(c), "cathode"),
            (None, Some(c)) => (fp.pad(c), "positive pin"),
            (None, None) if copper.len() > 2 => (["1", "A1"].iter().find_map(|n| fp.pad(n)), "pin 1"),
            (None, None) => (None, ""),
        };
        let Some(target) = target else {
            // A diode whose pins don't say which is its cathode, a part with no pad 1: the
            // library has to say.
            let prefix: String = placed.part.designator.chars().take_while(char::is_ascii_alphabetic).collect::<String>().to_ascii_uppercase();
            if copper.len() == 2 && matches!(prefix.as_str(), "D" | "LED") {
                cx.not_applicable(POLARITY, format!("library: which pin of {} is the cathode (a pin named K)", placed.part.designator));
            } else if copper.len() > 2 {
                cx.not_applicable(POLARITY, format!("library: which pad of {} is pin 1 (none is numbered 1 or A1)", placed.part.designator));
            }
            continue;
        };
        let map = half_turn(fp);
        let sym = map.is_some();
        // Keyed by pegs that don't fit turned round: it can't go on the wrong way.
        if !sym && fp.pads.iter().any(|p| p.kind == PadKind::Hole) {
            continue;
        }
        // Turned round, it is the same circuit (a crystal's terminals, a switch's two sides):
        // no way is wrong.
        if let Some(map) = &map {
            let pin = |pad: &str| -> String {
                let number = r.connections().into_iter().find(|(_, pads)| pads.contains(&pad)).map(|(pin, _)| pin);
                number
                    .and_then(|n| {
                        r.symbol.pins.iter().find(|p| p.number == n).map(|p| p.name.clone()).filter(|x| !x.is_empty()).or(Some(n.to_string()))
                    })
                    .unwrap_or_else(|| pad.to_string())
            };
            let net = |pad: &str| nets.pads.iter().find(|p| p.footprint == f.id && p.pad.number == pad).and_then(|p| p.net);
            if !cx.spend(fp.pads.len().saturating_mul(fp.pads.len()).saturating_add(1)) {
                return;
            }
            if turns_freely(fp, map, &pin, &net) {
                continue;
            }
        }
        if !cx.spend(fp.graphics.len().saturating_add(board.graphics.len()).saturating_add(fp.pads.len()).saturating_mul(8)) {
            return;
        }
        let placement = f.placement();
        let side = if placement.mirror { Side::Bottom } else { Side::Top };
        let critical = sym || hand;
        // Pin 1 (or the cathode), the end across from it, and the part's centre, on the board.
        let Some([lo, hi]) = super::super::bbox(copper.iter().flat_map(|p| p.bbox())) else { continue };
        let centre_local = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        let across = [2.0 * centre_local[0] - target.at[0], 2.0 * centre_local[1] - target.at[1]];
        let opposite = copper.iter().filter(|p| p.number != target.number).min_by(|a, b| distance(a.at, across).total_cmp(&distance(b.at, across)));
        let Some(pin) = nets.pads.iter().find(|p| p.footprint == f.id && p.pad.number == target.number) else { continue };
        let pin_core = pad_core(pin);
        let opp_core = opposite.and_then(|o| nets.pads.iter().find(|p| p.footprint == f.id && p.pad.number == o.number)).map(pad_core);
        let centre = placement.apply(centre_local);
        let body = body_of(fp, &placement);
        let mut marks: Vec<Mark> = fp
            .graphics
            .iter()
            .filter(|g| g.layer == Some(BoardLayer::Silkscreen) && g.side == Side::Top)
            .filter(|g| !(g.kind == GraphicType::Text && g.text.starts_with('{')))
            .filter_map(|g| mark_of(&placement.graphic(g), &|p| p))
            .collect();
        let layer = if side == Side::Top { DrawLayer::SilkscreenTop } else { DrawLayer::SilkscreenBottom };
        let look = grow(core_box(&pin_core), reach + 1.0);
        for g in board.graphics.iter().filter(|g| g.layer == layer) {
            if let Some(m) = mark_of(&g.graphic, &|p| p)
                && boxes_meet(m.bbox, look)
            {
                marks.push(m);
            }
        }
        let gap_to = |m: &Mark, core: &Core| m.cores.iter().map(|c| gap(c, core)).fold(f64::INFINITY, f64::min);
        // Marks that tell the ends apart: near pin 1, nearer it than the other end, not centred
        // on the part, not repeated across it.
        let telling: Vec<&Mark> = marks
            .iter()
            .filter(|m| gap_to(m, &pin_core) <= reach)
            .filter(|m| opp_core.as_ref().is_none_or(|o| gap_to(m, &pin_core) < gap_to(m, o) - 1e-6))
            .filter(|m| distance(m.centre, centre) > 0.2)
            .filter(|m| {
                let mirrored = [2.0 * centre[0] - m.centre[0], 2.0 * centre[1] - m.centre[1]];
                !marks.iter().any(|o| distance(o.centre, mirrored) <= 0.25 && (o.size - m.size).abs() <= 0.25 * m.size.max(1e-9))
            })
            .collect();
        let item = DrcItem::Footprint { id: f.id, designator: placed.part.designator.clone() };
        let pad_ref = pin.reference();
        let class = if critical { Class::Critical } else { Class::Practical };
        let why_class = match (sym, hand) {
            (true, _) => "its pads fit it turned round",
            (false, true) => "the board is hand-assembled",
            _ => "it can be placed turned round",
        };
        let regime = format!("{}: {why_class}", class.name());
        if telling.is_empty() {
            let severity = if critical { Severity::Error } else { Severity::Warning };
            let designator = placed.part.designator.clone();
            cx.add(POLARITY, &[item.key()], || {
                Draft::new(pin.at(), vec![item.clone()], format!("{designator} has no silk mark within {} mm of its {what} ({pad_ref}) that tells its ends apart", num(reach)))
                    .class(class)
                    .severity(severity)
                    .param("reach")
                    .regime(regime)
                    .arg("pin", what)
                    .arg("pad", pad_ref.clone())
                    .arg("reach", num(reach))
                    .fix(
                        "ecad.footprint.set_graphics",
                        Value::Null,
                        format!("add a mark of {} mm or more to {}'s silk within {} mm of {what}, outside the body, nowhere else alike (a dot, a bar, or K at a cathode)", num(min_size), fp.name, num(reach)),
                    )
            });
            continue;
        }
        let zone = zones.get_or_insert_with(|| [Zones::new(cx, Side::Top), Zones::new(cx, Side::Bottom)]);
        let Some(z) = zone.get(usize::from(side == Side::Bottom)) else { continue };
        // Each telling mark's faults: too small, under the body, clipped or too thin to print.
        let mut best: Option<(usize, Vec<String>, f64)> = None;
        for m in &telling {
            let mut faults = Vec::new();
            if m.size < min_size - 1e-9 {
                faults.push(format!("{} mm across, below {} mm", num(m.size), num(min_size)));
            }
            if body.is_some_and(|b| m.bbox[0][0] >= b[0][0] && m.bbox[0][1] >= b[0][1] && m.bbox[1][0] <= b[1][0] && m.bbox[1][1] <= b[1][1]) {
                faults.push("under the body, hidden once placed".into());
            }
            if m.stroke > 0.0 && m.stroke < thinnest - 1e-9 {
                faults.push(format!("its stroke {} mm is thinner than the {} mm the fab prints", num(m.stroke), num(thinnest)));
            } else {
                let pts = mark_points(m, 0.05);
                let lost = pts.iter().filter(|p| z.at(**p).is_some()).count();
                if !pts.is_empty() && lost * 2 > pts.len() {
                    faults.push(format!("{} % of it is clipped by the fab", num((lost as f64 / pts.len() as f64 * 100.0).round())));
                }
            }
            if best.as_ref().is_none_or(|(n, _, _)| faults.len() < *n) {
                best = Some((faults.len(), faults, m.size));
            }
        }
        let Some((n, faults, mark_size)) = best else { continue };
        if n == 0 {
            continue;
        }
        let designator = placed.part.designator.clone();
        let why = faults.join("; ");
        cx.add(POLARITY, &[item.key()], || {
            let mut d = Draft::new(pin.at(), vec![item.clone()], format!("{designator}'s polarity mark at its {what} ({pad_ref}) is weak: {why}"))
                .class(class)
                .severity(Severity::Warning)
                .variant("weak")
                .regime(regime)
                .arg("pin", what)
                .arg("pad", pad_ref.clone())
                .arg("why", why.clone())
                .fix(
                    "ecad.footprint.set_graphics",
                    Value::Null,
                    format!("make {}'s mark at {what} at least {} mm, outside the body, clear of pads", fp.name, num(min_size)),
                );
            if mark_size < min_size {
                d = d.measure(mark_size, min_size, "mm", Bound::Min).param("min_size");
            } else {
                d = d.param("");
            }
            d
        });
    }
}

fn core_box(c: &Core) -> [[f64; 2]; 2] {
    c.bbox()
}

/// The EIA size of a two-pad chip, as its code's place in [`SIZES`]: from the footprint's name,
/// else its fabrication outline's longer side, else its pads' span.
fn chip_size(fp: &FootprintItem) -> Option<usize> {
    let copper: Vec<_> = fp.pads.iter().filter(|p| p.kind != PadKind::Hole).collect();
    if copper.len() != 2 || copper.iter().any(|p| p.kind != PadKind::Smd) {
        return None;
    }
    // The name's digit runs, as an EIA code.
    let mut run = String::new();
    for ch in fp.name.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() {
            run.push(ch);
            continue;
        }
        if let Some(k) = SIZES.iter().position(|(code, _, _)| *code == run) {
            return Some(k);
        }
        run.clear();
    }
    let long = |[lo, hi]: [[f64; 2]; 2]| (hi[0] - lo[0]).max(hi[1] - lo[1]);
    if let Some(b) = shapes_bbox(fp.graphics.iter().filter(|g| g.layer == Some(BoardLayer::Fab) && g.kind != GraphicType::Text)) {
        let l = long(b);
        return SIZES.iter().position(|(_, body, _)| l <= *body).or(Some(SIZES.len()));
    }
    let b = super::super::bbox(copper.iter().flat_map(|p| p.bbox()))?;
    let l = long(b);
    SIZES.iter().position(|(_, _, span)| l <= *span).or(Some(SIZES.len()))
}

/// EIA chip codes, smallest first: the code, the longest body and the longest pads' span taken for it.
const SIZES: [(&str, f64, f64); 4] = [("01005", 0.5, 0.75), ("0201", 0.8, 1.15), ("0402", 1.2, 1.8), ("0603", 1.9, 2.7)];

fn size_name(k: usize) -> &'static str {
    SIZES.get(k).map_or("0805 or larger", |(c, _, _)| c)
}

/// Each copper layer's copper thickness, top to bottom (the stack-up's, else 1 oz).
fn copper_by_layer(cx: &Cx) -> BTreeMap<CopperLayer, f64> {
    let geo = cx.geo;
    let thick: Vec<f64> = geo.stackup.iter().filter(|l| l.kind == StackKind::Copper).map(|l| l.thickness).collect();
    geo.layers.iter().enumerate().map(|(i, l)| (*l, thick.get(i).copied().unwrap_or(OZ))).collect()
}

/// A pad's joins to pours: by pour fill, its layer, solid or relieved, the spokes asked and
/// joined, and the spokes' width and length.
#[derive(Clone, Copy)]
struct Join {
    fill: usize,
    pour: u64,
    layer: CopperLayer,
    solid: bool,
    connected: u8,
    spoke_width: f64,
    spoke_length: f64,
}

/// Every pad's joins (by index into the pads), from the fills' reliefs; pours with no net left out.
fn joins(cx: &Cx) -> BTreeMap<usize, Vec<Join>> {
    let (board, nets) = (cx.board, cx.nets);
    let mut out: BTreeMap<usize, Vec<Join>> = BTreeMap::new();
    for (fi, f) in nets.fills.iter().enumerate() {
        if f.net.is_none() {
            continue;
        }
        let gap = board.pours.iter().find(|p| p.id == f.pour).map_or(0.3, |p| p.thermal.gap);
        for r in &f.reliefs {
            if !r.inside || r.connected == 0 {
                continue;
            }
            out.entry(r.pad).or_default().push(Join {
                fill: fi,
                pour: f.pour,
                layer: f.layer,
                solid: r.solid,
                connected: r.connected,
                spoke_width: r.spoke_width,
                spoke_length: gap,
            });
        }
    }
    out
}

fn pad_item(p: &BoardPad) -> DrcItem {
    DrcItem::Pad { footprint: p.footprint, number: p.pad.number.clone(), reference: p.reference() }
}

fn pad_to_pour(cx: &mut Cx) {
    let (ecad, board, nets, roles) = (cx.ecad, cx.board, cx.nets, cx.roles);
    let (spokes_min, via_reach, heavy, sheet) =
        (cx.param(PAD_TO_POUR, "spokes"), cx.param(PAD_TO_POUR, "via_reach"), cx.param(PAD_TO_POUR, "heavy"), cx.param(PAD_TO_POUR, "sheet"));
    if !cx.spend(nets.fills.iter().map(|f| f.reliefs.len()).fold(nets.pads.len(), usize::saturating_add)) {
        return;
    }
    let joins = joins(cx);
    let copper = copper_by_layer(cx);
    let decoupling: BTreeSet<&str> = roles.parts(PartRole::Decoupling).map(|p| p.part.as_str()).collect();
    // A relief's spokes' resistance, in mΩ.
    let spoke_r = |j: &Join| {
        let t = copper.get(&j.layer).copied().unwrap_or(OZ).max(1e-6);
        sheet * (OZ / t) * j.spoke_length / j.spoke_width.max(1e-6) / f64::from(j.connected.max(1))
    };
    let by_fp: BTreeMap<u64, &super::super::board::BoardFootprint> = board.footprints.iter().map(|f| (f.id, f)).collect();
    let mut chips: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (pi, p) in nets.pads.iter().enumerate() {
        let Some(list) = joins.get(&pi) else { continue };
        let tht = p.top && p.bottom && p.pad.drill.is_some();
        let net_name = p.net.and_then(|n| nets.name(n)).unwrap_or("?").to_string();
        let item = pad_item(p);
        if tht {
            for j in list.iter().filter(|j| !j.solid && f64::from(j.connected) < spokes_min - 1e-9) {
                let r = spoke_r(j);
                let (j, item, net_name) = (*j, item.clone(), net_name.clone());
                cx.add(PAD_TO_POUR, &[item.key(), format!("z{}", j.pour), "spokes".into()], || {
                    Draft::new(
                        p.at(),
                        vec![item.clone(), DrcItem::Pour(j.pour)],
                        format!(
                            "through-hole pad {} joins the {net_name} pour on {} by {} spoke{} (≈ {} mΩ): at least {} keep it soldered and joined",
                            p.reference(),
                            j.layer.name(),
                            j.connected,
                            if j.connected == 1 { "" } else { "s" },
                            num(r),
                            num(spokes_min)
                        ),
                    )
                    .layer(j.layer)
                    .nets(vec![net_name.clone()])
                    .variant("spokes")
                    .measure(f64::from(j.connected), spokes_min, "spokes", Bound::Min)
                    .param("spokes")
                    .arg("milliohm", num(r))
                    .fix(
                        "ecad.board.pour.edit",
                        Value::Null,
                        format!("give the {net_name} pour on {} narrower spokes or a smaller gap so more of them reach the fill", j.layer.name()),
                    )
                });
            }
            let solid: Vec<&Join> = list.iter().filter(|j| j.solid).collect();
            let layers: BTreeSet<CopperLayer> = solid.iter().map(|j| j.layer).collect();
            let heavy_layer = solid.iter().find(|j| copper.get(&j.layer).is_some_and(|t| *t >= heavy * OZ - 1e-9)).map(|j| j.layer);
            if layers.len() >= 2 || heavy_layer.is_some() {
                let on: Vec<String> = layers.iter().map(|l| l.name()).collect();
                let why = match heavy_layer {
                    Some(l) if layers.len() < 2 => format!("a pour of {} oz or more on {}", num(heavy), l.name()),
                    _ => format!("planes on {}", on.join(", ")),
                };
                let fp_ref = p.designator.clone();
                let (item, net_name) = (item.clone(), net_name.clone());
                cx.add(PAD_TO_POUR, &[item.key(), "solid".into()], || {
                    Draft::new(
                        p.at(),
                        vec![item.clone()],
                        format!("through-hole pad {} joins {why} solid: they sink the soldering heat, so the joint is hard to make", p.reference()),
                    )
                    .nets(vec![net_name.clone()])
                    .variant("solid")
                    .measure(layers.len() as f64, 1.0, "planes", Bound::Max)
                    .param("")
                    .arg("why", why.clone())
                    .fix(
                        "ecad.board.pour.connect",
                        json!({"footprint": fp_ref, "pads": [p.pad.number], "connect": "thermal"}),
                        format!("relieve pad {} with thermal spokes", p.reference()),
                    )
                });
            }
            continue;
        }
        // Decoupling: a relief wants a via at the pad.
        if decoupling.contains(p.designator.as_str()) {
            let layer: CopperLayer = if p.top { CopperLayer::Top } else { CopperLayer::Bottom };
            if let Some(j) = list.iter().find(|j| !j.solid && j.layer == layer) {
                let core = pad_core(p);
                let near = board.vias.iter().enumerate().any(|(vi, v)| {
                    nets.via_nets.get(vi).copied().flatten() == p.net
                        && v.on(layer, board.copper_layers)
                        && gap(&core, &disc(v.at, v.diameter / 2.0)) <= via_reach + 1e-9
                });
                if !near {
                    let (j, item, net_name) = (*j, item.clone(), net_name.clone());
                    cx.add(PAD_TO_POUR, &[item.key(), "decoupling".into()], || {
                        Draft::new(p.at(), vec![item.clone(), DrcItem::Pour(j.pour)], format!("decoupling pad {} joins the {net_name} pour on {} through a relief with no {net_name} via within {} mm: the spokes add inductance", p.reference(), j.layer.name(), num(via_reach)))
                            .layer(j.layer)
                            .nets(vec![net_name.clone()])
                            .variant("decoupling")
                            .param("via_reach")
                            .arg("reach", num(via_reach))
                            .fix("ecad.board.via", Value::Null, format!("place a {net_name} via at pad {}, beside it or in it filled and capped (a relief is fine then)", p.reference()))
                    });
                }
            }
        }
        // A chip's ends: collected per footprint.
        if let Some(f) = by_fp.get(&p.footprint)
            && let Some(placed) = ecad.placed(f)
            && chip_size(placed.resolved.footprint).is_some_and(|k| k <= 3)
        {
            chips.entry(p.footprint).or_default().push(pi);
        }
    }
    // Chips of 0603 and smaller: solid at one end only.
    for (fid, pads) in chips {
        let solid_pours = |pi: usize| -> BTreeSet<u64> { joins.get(&pi).into_iter().flatten().filter(|j| j.solid).map(|j| j.pour).collect() };
        let Some(f) = by_fp.get(&fid) else { continue };
        let all: Vec<usize> = nets.pads.iter().enumerate().filter(|(_, p)| p.footprint == fid && (p.top || p.bottom)).map(|(i, _)| i).collect();
        let (Some(a), Some(b)) = (all.first().copied(), all.get(1).copied()) else { continue };
        let (sa, sb) = (solid_pours(a), solid_pours(b));
        if sa.is_empty() == sb.is_empty() || pads.is_empty() {
            continue;
        }
        let (solid, other) = if sa.is_empty() { (b, a) } else { (a, b) };
        let (Some(ps), Some(po)) = (nets.pads.get(solid), nets.pads.get(other)) else { continue };
        let Some(placed) = ecad.placed(f) else { continue };
        let code = chip_size(placed.resolved.footprint).map_or("", size_name);
        let item = DrcItem::Footprint { id: f.id, designator: placed.part.designator.clone() };
        let designator = placed.part.designator.clone();
        let net_name = ps.net.and_then(|n| nets.name(n)).unwrap_or("?").to_string();
        cx.add(PAD_TO_POUR, &[item.key(), "chip".into()], || {
            Draft::new(
                ps.at(),
                vec![item.clone(), pad_item(ps)],
                format!(
                    "{code} chip {designator} has pad {} joined solid to the {net_name} pour and pad {} not: unequal heat at the two ends",
                    ps.reference(),
                    po.reference()
                ),
            )
            .nets(vec![net_name.clone()])
            .variant("chip")
            .param("")
            .arg("size", code)
            .arg("pad", ps.reference())
            .fix(
                "ecad.board.pour.connect",
                json!({"footprint": designator, "pads": [ps.pad.number], "connect": "thermal"}),
                format!("relieve pad {} so both ends heat alike", ps.reference()),
            )
        });
    }
}

/// How far the point `q` is outside the copper `c` (0 inside it): `gap` for a point, without
/// its general case's cost, for lattices of many points.
fn point_gap(c: &Core, q: [f64; 2]) -> f64 {
    let n = c.points.len();
    let d = match (c.points.first(), n) {
        (None, _) => f64::INFINITY,
        (Some(p), 1) => distance(*p, q),
        (Some(_), _) if n >= 3 && inside(q, &c.points) => 0.0,
        _ => (0..n)
            .filter(|i| n >= 3 || *i + 1 < n)
            .filter_map(|i| Some((*c.points.get(i)?, *c.points.get((i + 1) % n)?)))
            .map(|(a, b)| path::segment_distance(q, a, b, 0.0))
            .fold(f64::INFINITY, f64::min),
    };
    d - c.radius
}

fn tombstones(cx: &mut Cx) {
    let (ecad, board, nets) = (cx.ecad, cx.board, cx.nets);
    if cx.roles.intent.board.hand_assembly == Some(true) {
        cx.not_applicable(TOMBSTONE, "intent: the board is hand-assembled (hand_assembly)");
        return;
    }
    let (ratio_max, window) = (cx.param(TOMBSTONE, "ratio"), cx.param(TOMBSTONE, "window"));
    let step = 0.05_f64;
    let joins = joins(cx);
    let fills = &nets.fills;
    // Each net's track pieces by layer, made once.
    let mut by_net: BTreeMap<(usize, CopperLayer), Vec<Core>> = BTreeMap::new();
    if !cx.spend(board.tracks.len().saturating_add(1)) {
        return;
    }
    for (ti, t) in board.tracks.iter().enumerate() {
        if let Some(n) = nets.track_nets.get(ti).copied().flatten() {
            by_net.entry((n, t.layer)).or_default().extend(super::super::copper::track_cores(t));
        }
    }
    let mut pads_of: BTreeMap<u64, Vec<(usize, &BoardPad)>> = BTreeMap::new();
    for (i, p) in nets.pads.iter().enumerate().filter(|(_, p)| p.top || p.bottom) {
        pads_of.entry(p.footprint).or_default().push((i, p));
    }
    // One pad's copper looked at: the points round it, which of them its joined copper covers,
    // the fills joined solid to it, and its relief's spokes' area.
    struct Look {
        pts: Vec<[f64; 2]>,
        on: Vec<bool>,
        solid: Vec<usize>,
        spokes: f64,
        own: f64,
    }
    let mut chips: Vec<(&super::super::board::BoardFootprint, usize, [(usize, &BoardPad); 2], [Look; 2])> = Vec::new();
    for f in &board.footprints {
        let Some(placed) = ecad.placed(f) else { continue };
        if placed.part.dnp {
            continue;
        }
        let Some(k) = chip_size(placed.resolved.footprint) else { continue };
        if k > 3 {
            continue;
        }
        let pads = pads_of.get(&f.id).map_or(&[][..], Vec::as_slice);
        let (Some((ia, a)), Some((ib, b))) = (pads.first().copied(), pads.get(1).copied()) else { continue };
        let ja: BTreeSet<u64> = joins.get(&ia).into_iter().flatten().map(|j| j.pour).collect();
        let jb: BTreeSet<u64> = joins.get(&ib).into_iter().flatten().map(|j| j.pour).collect();
        // Both pads on one pour: alike.
        if ja.intersection(&jb).next().is_some() {
            continue;
        }
        let mut looks: Vec<Look> = Vec::with_capacity(2);
        for (pi, p) in [(ia, a), (ib, b)] {
            let layer: CopperLayer = if p.top { CopperLayer::Top } else { CopperLayer::Bottom };
            let core = pad_core(p);
            let around = grow(core.bbox(), window);
            let (nx, ny) = (((around[1][0] - around[0][0]) / step).ceil() as usize, ((around[1][1] - around[0][1]) / step).ceil() as usize);
            let count = nx.saturating_add(1).saturating_mul(ny.saturating_add(1));
            // Its net's tracks on the layer near it that join it (touching it, or a track that
            // does), and the fills joined solid to it.
            let near: Vec<&Core> =
                p.net.and_then(|n| by_net.get(&(n, layer))).into_iter().flatten().filter(|c| boxes_meet(c.bbox(), around)).collect();
            if !cx.spend(near.len().saturating_add(1).saturating_mul(near.len().saturating_add(1))) {
                return;
            }
            let mut joined: Vec<bool> = near.iter().map(|c| gap(c, &core) <= 1e-9).collect();
            let mut open: Vec<usize> = (0..near.len()).filter(|i| joined.get(*i).copied().unwrap_or(false)).collect();
            while let Some(i) = open.pop() {
                let Some(a) = near.get(i) else { continue };
                for (j, b) in near.iter().enumerate() {
                    if let Some(slot) = joined.get_mut(j)
                        && !*slot
                        && gap(a, b) <= 1e-9
                    {
                        *slot = true;
                        open.push(j);
                    }
                }
            }
            let tracks: Vec<&Core> = near.iter().zip(&joined).filter(|(_, j)| **j).map(|(c, _)| *c).collect();
            let solid: Vec<usize> = joins.get(&pi).into_iter().flatten().filter(|j| j.solid && j.layer == layer).map(|j| j.fill).collect();
            if !cx.spend(count.saturating_mul(tracks.len().saturating_add(solid.len()).saturating_add(2))) {
                return;
            }
            let mut pts = Vec::with_capacity(count.min(1 << 20));
            for i in 0..=nx.min(1000) {
                for j in 0..=ny.min(1000) {
                    let q = [around[0][0] + i as f64 * step, around[0][1] + j as f64 * step];
                    let g = point_gap(&core, q);
                    if g > 0.0 && g <= window {
                        pts.push(q);
                    }
                }
            }
            let mut on = vec![false; pts.len()];
            for t in &tracks {
                let reach = grow(t.bbox(), 1e-9);
                for (slot, q) in on.iter_mut().zip(&pts) {
                    if !*slot && boxes_meet(reach, [*q, *q]) && point_gap(t, *q) <= 0.0 {
                        *slot = true;
                    }
                }
            }
            // A relief's spokes sink through their width only.
            let spokes: f64 = joins
                .get(&pi)
                .into_iter()
                .flatten()
                .filter(|j| !j.solid && j.layer == layer)
                .map(|j| f64::from(j.connected) * j.spoke_width * j.spoke_length)
                .sum();
            looks.push(Look { pts, on, solid, spokes, own: core.area() });
        }
        let (Some(lb), Some(la)) = (looks.pop(), looks.pop()) else { continue };
        chips.push((f, k, [(ia, a), (ib, b)], [la, lb]));
    }
    // The fills joined solid, each asked once for every point round every pad joined to it (a
    // plane's outline is long: asking it pad by pad is what costs).
    let mut asks: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for (c, (_, _, _, looks)) in chips.iter().enumerate() {
        for (side, l) in looks.iter().enumerate() {
            for fi in &l.solid {
                asks.entry(*fi).or_default().push((c, side));
            }
        }
    }
    for (fi, who) in asks {
        let Some(fill) = fills.get(fi) else { continue };
        let pts: Vec<[f64; 2]> = who.iter().filter_map(|(c, side)| chips.get(*c)?.3.get(*side)).flat_map(|l| l.pts.iter().copied()).collect();
        if !cx.spend(pts.len().saturating_add(fill.region.shapes().len())) {
            return;
        }
        let mut hits = fill.region.locate(&pts).into_iter();
        for (c, side) in who {
            let Some(l) = chips.get_mut(c).and_then(|x| x.3.get_mut(side)) else { continue };
            for slot in l.on.iter_mut() {
                if hits.next().flatten().is_some() {
                    *slot = true;
                }
            }
        }
    }
    for (f, k, [(_, a), (_, b)], looks) in chips {
        let Some(placed) = ecad.placed(f) else { continue };
        let mut heat = [0.0_f64; 2];
        let mut parts = [(0.0_f64, 0.0_f64); 2];
        for (slot, l) in looks.iter().enumerate() {
            let area = l.on.iter().filter(|s| **s).count() as f64 * step * step + l.spokes;
            if let (Some(h), Some(pp)) = (heat.get_mut(slot), parts.get_mut(slot)) {
                *h = l.own + area;
                *pp = (l.own, area);
            }
        }
        let [ha, hb] = heat;
        let (big, small) = (ha.max(hb), ha.min(hb));
        if small <= 0.0 {
            continue;
        }
        let ratio = big / small;
        if ratio <= ratio_max + 1e-9 {
            continue;
        }
        let (heavy, light) = if ha >= hb { (a, b) } else { (b, a) };
        let [pa, pb] = parts;
        let (hp, lp) = if ha >= hb { (pa, pb) } else { (pb, pa) };
        let code = size_name(k);
        let (class_sev, note) = match k {
            0 | 1 => (Severity::Error, ""),
            3 => (Severity::Info, ": at 0603 a note"),
            _ => (Severity::Warning, ""),
        };
        let item = DrcItem::Footprint { id: f.id, designator: placed.part.designator.clone() };
        let designator = placed.part.designator.clone();
        cx.add(TOMBSTONE, &[item.key()], || {
            Draft::new(
                heavy.at(),
                vec![item.clone(), pad_item(heavy)],
                format!(
                    "{code} chip {designator}: pad {} has {} mm² of copper within {} mm and pad {} has {} mm² (with the pads), {}× the heat sinking{note}",
                    heavy.reference(),
                    num(((hp.0 + hp.1) * 100.0).round() / 100.0),
                    num(window),
                    light.reference(),
                    num(((lp.0 + lp.1) * 100.0).round() / 100.0),
                    num((ratio * 10.0).round() / 10.0)
                ),
            )
            .severity(class_sev)
            .measure(ratio, ratio_max, "ratio", Bound::Max)
            .param("ratio")
            .regime(format!("{code} chip"))
            .arg("size", code)
            .arg("pad", heavy.reference())
            .arg("other", light.reference())
            .fix(
                "ecad.board.track.edit",
                Value::Null,
                format!("neck the copper at pad {} (a narrow track into the pour or a relief), or give pad {} as much", heavy.reference(), light.reference()),
            )
        });
    }
}
