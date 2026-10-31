use super::{GRID, Motion, Placement, flow, lattice, snap};
use crate::ecad::{Graphic, GraphicType, Pin, PinDirection, PinType};

fn pin(at: [f64; 2], direction: PinDirection) -> Pin {
    Pin { number: "1".into(), name: String::new(), name_hidden: false, electrical: PinType::Passive, at, direction, length: 2.54 }
}

fn near([a, b]: [f64; 2], [c, d]: [f64; 2]) -> bool {
    (a - c).abs() < 1e-9 && (b - d).abs() < 1e-9
}

fn placed(rotation: u16, mirror: bool) -> Placement {
    Placement { at: [0.0, 0.0], rotation, mirror }
}

fn graphic(kind: GraphicType) -> Graphic {
    Graphic {
        kind,
        layer: None,
        side: Default::default(),
        points: Vec::new(),
        from: None,
        to: None,
        center: None,
        radius: None,
        start: None,
        sweep: None,
        at: None,
        text: String::new(),
        height: None,
        rotation: 0.0,
        width: 0.15,
        fill: false,
    }
}

#[test]
fn a_turned_pin_lands_where_worked_out_by_hand() {
    // (2.54, 0) pointing left, on a part at (10, 20) turned a quarter: (10, 22.54), pointing down.
    let p = Placement { at: [10.0, 20.0], rotation: 90, mirror: false }.pin(&pin([2.54, 0.0], PinDirection::Left));
    assert!(near(p.at, [10.0, 22.54]), "{:?}", p.at);
    assert_eq!(p.direction, PinDirection::Down);
}

#[test]
fn a_mirrored_pin_lands_where_worked_out_by_hand() {
    // (−5.08, 0) pointing right, mirrored: (5.08, 0), pointing left.
    let p = placed(0, true).pin(&pin([-5.08, 0.0], PinDirection::Right));
    assert_eq!(p.at, [5.08, 0.0]);
    assert_eq!(p.direction, PinDirection::Left);
}

#[test]
fn all_eight_placements_of_a_pin() {
    use PinDirection::*;
    // A pin at (2.54, 1.27) pointing right. Mirrored first it is at (−2.54, 1.27) pointing left;
    // each quarter turn takes (x, y) to (−y, x).
    let cases = [
        (0, false, [2.54, 1.27], Right),
        (90, false, [-1.27, 2.54], Up),
        (180, false, [-2.54, -1.27], Left),
        (270, false, [1.27, -2.54], Down),
        (0, true, [-2.54, 1.27], Left),
        (90, true, [-1.27, -2.54], Down),
        (180, true, [2.54, -1.27], Right),
        (270, true, [1.27, 2.54], Up),
    ];
    for (rotation, mirror, at, direction) in cases {
        let p = placed(rotation, mirror).pin(&pin([2.54, 1.27], Right));
        assert_eq!((p.at, p.direction), (at, direction), "rotation {rotation}, mirror {mirror}");
    }
}

#[test]
fn quarter_turns_are_exact_and_compose_to_the_identity() {
    let q = [2.54, -7.62];
    assert_eq!(Placement { at: [0.0, 0.0], rotation: 0, mirror: false }.apply(q), q, "no turn, no mirror, at the origin");
    assert_eq!(Placement { at: [1.0, 2.0], rotation: 0, mirror: false }.apply(q), [3.54, -5.62]);
    let quarter = Motion { rotation: 90, ..Default::default() };
    let back = (0..4).fold(q, |p, _| quarter.point(p));
    assert_eq!(back, q, "four quarter turns");
    let mirror = Motion { mirror: true, about: [1.27, 0.0], ..Default::default() };
    assert_eq!(mirror.point(mirror.point(q)), q, "two mirrors");
    assert!(Motion::default().is_identity() && !quarter.is_identity());
    // The pin directions too.
    for d in [PinDirection::Right, PinDirection::Up, PinDirection::Left, PinDirection::Down] {
        assert_eq!(placed(0, false).direction(d), d);
        let turned = (0..4).fold(d, |d, _| placed(90, false).direction(d));
        assert_eq!(turned, d);
    }
}

#[test]
fn a_moved_placement_places_its_symbol_where_moving_each_point_would() {
    // Moving a part (mirror and turn about a pivot, then shift) must equal moving each of its
    // symbol's points: so a mirror flips the part's own turn.
    let points = [[2.54, 1.27], [-5.08, 0.0], [0.0, 3.81]];
    for rotation in [0, 90, 180, 270] {
        for mirror in [false, true] {
            let p = Placement { at: [12.7, -5.08], rotation, mirror };
            for m_rotation in [0, 90, 180, 270] {
                for m_mirror in [false, true] {
                    let m = Motion { about: [2.54, 7.62], mirror: m_mirror, rotation: m_rotation, by: [5.08, -2.54] };
                    let moved = m.placement(&p);
                    for q in points {
                        assert!(near(moved.apply(q), m.point(p.apply(q))), "{p:?} {m:?} {q:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn a_mirrored_arc_runs_the_other_way() {
    // An arc from 0° sweeping +90°, mirrored: from 180°, sweeping −90°, about the mirrored centre.
    let mut arc = graphic(GraphicType::Arc);
    (arc.center, arc.radius, arc.start, arc.sweep) = (Some([1.0, 0.0]), Some(1.0), Some(0.0), Some(90.0));
    let m = placed(0, true).graphic(&arc);
    assert_eq!((m.center, m.start, m.sweep), (Some([-1.0, 0.0]), Some(180.0), Some(-90.0)));
    let (a, b) = (arc.arc_points(8), m.arc_points(8));
    for (p, q) in a.iter().zip(&b) {
        assert!(near(placed(0, true).apply(*p), *q), "the same points: {p:?} → {q:?}");
    }
    // Turned as well: the start turns with the part.
    let t = placed(90, true).graphic(&arc);
    assert_eq!((t.start, t.sweep), (Some(270.0), Some(-90.0)));
    // Lines, rectangles and circles move point by point.
    let mut rect = graphic(GraphicType::Rect);
    (rect.from, rect.to) = (Some([-2.54, -1.0]), Some([2.54, 1.0]));
    let r = placed(90, false).graphic(&rect);
    assert_eq!((r.from, r.to), (Some([1.0, -2.54]), Some([-1.0, 2.54])));
}

#[test]
fn text_stays_readable_and_covers_the_same_box() {
    let mut text = graphic(GraphicType::Text);
    (text.at, text.text, text.height) = (Some([1.0, 0.0]), "R".into(), Some(1.0));
    // Each case: the turn and mirror, then where the readable text starts and its turn.
    for (rotation, mirror, at, turn) in [
        (0, false, [1.0, 0.0], 0.0),
        (180, false, [-1.6, -1.0], 0.0),
        (0, true, [-1.6, 0.0], 0.0),
        (90, false, [0.0, 1.0], 90.0),
        (270, false, [1.0, -1.6], 90.0),
        (90, true, [0.0, -1.6], 90.0),
    ] {
        let p = placed(rotation, mirror);
        let t = p.graphic(&text);
        assert!(t.at.is_some_and(|a| near(a, at)) && (t.rotation - turn).abs() < 1e-9, "{rotation} {mirror}: {:?} {}", t.at, t.rotation);
        let boxed = |corners: Vec<[f64; 2]>| crate::ecad::bbox(corners).unwrap();
        let want = p.bbox(boxed(text.text_box()));
        let got = boxed(t.text_box());
        assert!(near(want[0], got[0]) && near(want[1], got[1]), "{rotation} {mirror}: {want:?} vs {got:?}");
    }
}

#[test]
fn points_snap_to_the_lattice() {
    assert_eq!(snap(1.000_04), 1.0);
    assert_eq!(snap(1.000_06), 1.0001);
    assert_eq!(snap(-0.000_01).to_string(), "0", "never -0");
    assert_eq!(lattice([2.54, -1.27]), [25_400, -12_700]);
    assert_eq!(lattice([0.1 + 0.2, 0.0]), lattice([0.3, 0.0]), "nearly equal points meet");
}

#[test]
fn the_flow_layout_fills_rows_on_the_grid() {
    let resistor = [[-5.08, -1.143], [5.08, 1.143]];
    assert_eq!(flow(None, &[resistor, resistor]), [[5.08, -2.54], [25.4, -2.54]], "left to right under the origin");
    // 50.8 mm wide parts: four fit in a 254 mm row, the fifth starts the next one, 10.16 mm under
    // the first row's bottom (−5.08).
    let wide = [[-25.4, -2.54], [25.4, 2.54]];
    let o = flow(None, &[wide; 5]);
    assert_eq!(o.iter().map(|p| p[1]).collect::<Vec<_>>(), [-2.54, -2.54, -2.54, -2.54, -17.78]);
    assert_eq!(o.first().map(|p| p[0]), o.last().map(|p| p[0]), "the new row starts at the left");
    // Under what the sheet holds, from its left edge on the grid.
    let o = flow(Some([[-10.0, -20.0], [30.0, 5.0]]), &[resistor]);
    assert_eq!(o, [[-5.08, -33.02]]);
    for p in flow(Some([[-3.3, 7.7], [1.0, 9.0]]), &[resistor, wide, resistor]) {
        for v in p {
            assert!(((v / GRID) - (v / GRID).round()).abs() < 1e-9, "{v} is on the grid");
        }
    }
}
