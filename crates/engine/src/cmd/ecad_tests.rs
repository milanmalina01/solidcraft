use std::sync::Arc;

use serde_json::{Value, json};

use crate::Session;

#[test]
fn ecad_inspect_reports_a_design_without_electronics() {
    let mut s = Session::default();
    let none = json!({"components": 0, "footprints": 0, "packages": 0, "symbols": 0});
    assert_eq!(s.execute("ecad.inspect", &json!({})).unwrap(), json!({"empty": true, "counts": none}));
}

#[test]
fn ecad_inspect_only_reads() {
    let spec = crate::find_command("ecad.inspect").unwrap();
    assert!(!spec.undoable && !spec.reads_model && spec.tab.is_empty());
    assert!(spec.contract.is_some() && spec.output.is_some());
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length": 10, "width": 10, "height": 10})).unwrap();
    let (doc, undo, revision, dirty) = (s.doc.clone(), s.undo.len(), s.revision, s.is_dirty());
    s.execute("ecad.inspect", &json!({})).unwrap();
    assert!(Arc::ptr_eq(&doc, &s.doc), "the document is not copied or changed");
    assert_eq!((s.undo.len(), s.revision, s.is_dirty()), (undo, revision, dirty));
}

#[test]
fn ecad_commands_are_listed_and_described() {
    let mut s = Session::default();
    let listed = s.execute("engine.commands", &json!({})).unwrap();
    assert!(listed.as_array().unwrap().iter().any(|c| c["id"] == "ecad.inspect"));
    let described = s.execute("engine.describe", &json!({"command": "ecad.inspect"})).unwrap();
    assert_eq!(described["input"]["type"], json!("object"));
    assert_eq!(described["output"]["properties"]["empty"]["type"], json!("boolean"));
}

#[test]
fn a_design_without_electronics_saves_and_reopens_without_them() {
    let dir = std::env::temp_dir().join(format!("solidcraft-ecad-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("board.sc");
    let mut s = Session::default();
    s.execute("solid.box", &json!({"length": 10, "width": 10, "height": 10})).unwrap();
    let before = s.execute("ecad.inspect", &json!({})).unwrap();
    s.execute("file.save", &json!({"path": path, "format": "json"})).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(saved.get("ecad").is_none(), "empty electronics are not saved");
    s.execute("file.new", &json!({})).unwrap();
    s.execute("doc.open", &json!({"path": path})).unwrap();
    assert!(!s.is_dirty());
    assert_eq!(s.execute("ecad.inspect", &json!({})).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

pub(crate) fn run(s: &mut Session, id: &str, p: Value) -> Value {
    s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"))
}

/// The steps [`board`] is built with: the library (an 0603 resistor and capacitor, a two-pin
/// header, a spare test point, a 3D package on the SolidCraft component Body), a schematic
/// (R1 and R2 on SIG, J1, J2 and C1, GND and VCC ports, OTHER on J1.1, the frame Main) and a
/// 30 x 20 mm four-layer board: GND pours 6 and 7 on inner1 and inner2, the SIG track 8 on
/// top, GND vias 9 and 10, keepout 11, hole 12 and the silkscreen text 13 (footprints 1 to 5).
pub(crate) fn board_steps() -> Vec<(&'static str, Value)> {
    vec![
        ("file.new", json!({"kind": "electronics_design"})),
        ("ecad.footprint.generate", json!({"footprint": "R0603", "package": "chip", "size": "0603"})),
        ("ecad.symbol.generate", json!({"symbol": "RES", "style": "resistor"})),
        ("ecad.component.create", json!({"name": "R0603", "prefix": "R", "value": "10k", "symbol": "RES", "footprint": "R0603"})),
        ("ecad.symbol.generate", json!({"symbol": "CAP", "style": "capacitor"})),
        ("ecad.component.create", json!({"name": "C0603", "prefix": "C", "value": "100nF", "symbol": "CAP", "footprint": "R0603"})),
        ("ecad.footprint.generate", json!({"footprint": "HDR2", "package": "header", "pins": 2})),
        ("ecad.symbol.generate", json!({"symbol": "CONN2", "style": "box", "pins": [{"number": "1", "name": "A"}, {"number": "2", "name": "B"}]})),
        ("ecad.component.create", json!({"name": "CONN2", "prefix": "J", "symbol": "CONN2", "footprint": "HDR2"})),
        ("ecad.symbol.generate", json!({"symbol": "SPARE", "style": "test_point"})),
        ("ecad.component.variant.add", json!({"component": "CONN2", "name": "Alt", "footprint": "HDR2"})),
        ("component.create", json!({"name": "Body"})),
        ("solid.box", json!({"length": 1.6, "width": 0.8, "height": 0.45, "corner": [-0.8, -0.4, 0]})),
        ("component.activate", json!({"component": 0})),
        ("ecad.package.create", json!({"name": "PKG", "model": "Body"})),
        (
            "ecad.schematic.place",
            json!({"parts": [{"component": "R0603", "at": [0, 0]}, {"component": "R0603", "at": [20.32, 0]}, {"component": "CONN2", "at": [40.64, 0]}, {"component": "CONN2", "at": [40.64, 20.32]}, {"component": "C0603", "at": [0, 20.32]}]}),
        ),
        ("ecad.schematic.wire", json!({"wires": [{"from": "R1.2", "to": "R2.1"}]})),
        ("ecad.schematic.label", json!({"labels": [{"name": "SIG", "at": "R1.2"}]})),
        (
            "ecad.schematic.power",
            json!({"ports": [{"name": "GND", "at": "R1.1"}, {"name": "GND", "at": "R2.2"}, {"name": "GND", "at": "J1.2"}, {"name": "GND", "at": "J2.2"}, {"name": "GND", "at": "C1.2"}, {"name": "VCC", "at": "J2.1"}, {"name": "VCC", "at": "C1.1"}]}),
        ),
        ("ecad.schematic.label", json!({"labels": [{"name": "OTHER", "at": "J1.1"}]})),
        ("ecad.schematic.frame", json!({"title": "Main", "rect": [[-20, -20], [60, 40]]})),
        ("ecad.design.info", json!({"title": "Test board", "revision": "A", "date": "2026-10-30", "author": "Tests"})),
        ("ecad.board.outline", json!({"rect": [30, 20]})),
        ("ecad.board.stack", json!({"copper_layers": 4})),
        ("ecad.board.sync", json!({})),
        ("ecad.board.move", json!({"items": ["R1"], "to": [-10, 0]})),
        ("ecad.board.move", json!({"items": ["R2"], "to": [10, 0]})),
        ("ecad.board.move", json!({"items": ["J1"], "to": [3, 6]})),
        ("ecad.board.move", json!({"items": ["J2"], "to": [-10, -7]})),
        ("ecad.board.move", json!({"items": ["C1"], "to": [12, -7]})),
        (
            "ecad.intent.set",
            json!({"nets": [{"net": "SIG", "role": "fast", "edge_ns": 1.0}, {"net": "GND", "role": "ground"}, {"net": "VCC", "role": "power", "voltage": {"nominal": 3.3}, "current_a": 0.1, "note": "a test board"}], "board": {"f_concern_mhz": 2400}}),
        ),
        ("ecad.board.profile", json!({"fab": "standard"})),
        ("ecad.board.netclass", json!({"name": "Power", "nets": ["VCC"], "track_width": 0.4})),
        ("ecad.board.pour", json!({"layer": "inner1", "net": "GND", "rect": [30, 20], "remove_islands": false})),
        ("ecad.board.pour", json!({"layer": "inner2", "net": "GND", "rect": [30, 20], "remove_islands": false})),
        ("ecad.board.track", json!({"layer": "top", "points": [[-9.2, 0], [9.2, 0]], "width": 0.2})),
        ("ecad.board.via", json!({"at": [-13, -8], "net": "GND"})),
        ("ecad.board.via", json!({"at": [13, 8], "net": "GND"})),
        ("ecad.board.keepout", json!({"rect": [2, 2], "center": [0, -6]})),
        ("ecad.board.hole", json!({"at": [-13, 8]})),
        ("ecad.board.draw.text", json!({"layer": "silkscreen_top", "at": [-14, -9.5], "text": "TEST", "height": 1})),
    ]
}

/// A small electronics design with something of every kind on it (see [`board_steps`]).
pub(crate) fn board() -> Session {
    let mut s = Session::default();
    for (id, p) in board_steps() {
        run(&mut s, id, p);
    }
    s
}

/// The first finding's key of the electrical rules check, the design rule check and the quality
/// checks on `s`.
fn first_keys(s: &mut Session) -> [String; 3] {
    let key = |s: &mut Session, id: &str| run(s, id, json!({}))["findings"][0]["key"].as_str().map(str::to_string).unwrap_or_default();
    [key(s, "ecad.erc.inspect"), key(s, "ecad.drc.inspect"), key(s, "ecad.quality.inspect")]
}

/// One call of each electronics command that works on [`board`] (but `ecad.export.pages`, which
/// writes a file, and `ecad.view.board_3d`, which is to come). `keys` are [`first_keys`].
fn calls(keys: &[String; 3]) -> Vec<(&'static str, Value)> {
    vec![
        ("ecad.inspect", json!({})),
        ("ecad.view.schematic", json!({})),
        ("ecad.view.board", json!({})),
        ("ecad.component.create", json!({"name": "L0603", "prefix": "L", "value": "10u", "symbol": "RES", "footprint": "R0603"})),
        ("ecad.component.edit", json!({"component": "R0603", "value": "4k7", "mpn": "X1"})),
        ("ecad.component.copy", json!({"component": "R0603", "name": "R0603 copy"})),
        ("ecad.component.delete", json!({"components": ["R0603"], "with_parts": true})),
        ("ecad.component.inspect", json!({"component": "R0603"})),
        ("ecad.component.map_pins", json!({"component": "R0603", "map": {"1": ["2"], "2": ["1"]}})),
        ("ecad.component.variant.add", json!({"component": "R0603", "name": "Alt2", "footprint": "HDR2", "package": "PKG"})),
        ("ecad.component.variant.edit", json!({"component": "R0603", "variant": "R0603", "name": "Std"})),
        ("ecad.component.variant.delete", json!({"component": "CONN2", "variant": "Alt"})),
        ("ecad.symbol.create", json!({"name": "NEW", "description": "a new symbol"})),
        ("ecad.symbol.edit", json!({"symbol": "RES", "description": "resistor"})),
        ("ecad.symbol.copy", json!({"symbol": "RES", "name": "RES2"})),
        ("ecad.symbol.delete", json!({"symbol": "SPARE"})),
        ("ecad.symbol.inspect", json!({"symbol": "RES"})),
        ("ecad.footprint.create", json!({"name": "NEWFP", "component": "CONN2", "variant": "Alt3"})),
        ("ecad.footprint.edit", json!({"footprint": "R0603", "description": "chip"})),
        ("ecad.footprint.copy", json!({"footprint": "R0603", "name": "R0603b"})),
        ("ecad.footprint.delete", json!({"footprint": "HDR2", "force": true})),
        ("ecad.footprint.inspect", json!({"footprint": "R0603"})),
        ("ecad.library.inspect", json!({})),
        (
            "ecad.library.import",
            json!({"items": {"symbols": [{"id": 1, "name": "IMP", "pins": [{"number": "1", "at": [0, 0], "direction": "right", "length": 2.54}]}]}}),
        ),
        ("ecad.pin.add", json!({"symbol": "CONN2", "pins": [{"number": 3, "name": "C", "at": [-7.62, -2.54], "direction": "right"}]})),
        ("ecad.pin.edit", json!({"symbol": "CONN2", "pin": 2, "name": "BB"})),
        ("ecad.pin.delete", json!({"symbol": "CONN2", "pins": [2]})),
        ("ecad.pad.add", json!({"footprint": "HDR2", "pads": [{"number": 3, "at": [0, 5.08], "size": [1.7, 1.7], "shape": "round", "drill": 1.0}]})),
        ("ecad.pad.add.through_hole", json!({"footprint": "HDR2", "pads": [{"number": 3, "at": [0, 5.08], "size": [1.7, 1.7]}]})),
        ("ecad.pad.add.hole", json!({"footprint": "HDR2", "pads": [{"number": "H", "at": [0, 8], "size": [1, 1], "shape": "round"}]})),
        ("ecad.pad.edit", json!({"footprint": "R0603", "pad": 1, "size": [0.8, 0.9]})),
        ("ecad.pad.delete", json!({"footprint": "HDR2", "pads": [2]})),
        ("ecad.symbol.set_graphics", json!({"symbol": "CONN2", "graphics": [{"type": "rect", "from": [-5.08, -2.54], "to": [5.08, 2.54]}]})),
        (
            "ecad.footprint.set_graphics",
            json!({"footprint": "R0603", "layer": "silkscreen", "graphics": [{"type": "line", "points": [[-0.5, 0.8], [0.5, 0.8]]}]}),
        ),
        ("ecad.footprint.courtyard", json!({"footprint": "R0603", "margin": 0.3})),
        ("ecad.draw.line", json!({"symbol": "CONN2", "points": [[0, 0], [2.54, 0]]})),
        ("ecad.draw.rect", json!({"footprint": "HDR2", "layer": "fab", "from": [-1, -2], "to": [1, 2]})),
        ("ecad.draw.circle", json!({"footprint": "HDR2", "layer": "silkscreen", "center": [-2, 2], "radius": 0.2, "fill": true})),
        ("ecad.draw.text", json!({"symbol": "CONN2", "at": [0, 3], "text": "{value}", "height": 1.27})),
        ("ecad.footprint.generate", json!({"footprint": "SOIC8", "package": "soic", "pins": 8})),
        ("ecad.symbol.generate", json!({"symbol": "LED", "style": "led"})),
        ("ecad.package.create", json!({"name": "PKG2", "model": "Body", "component": "R0603"})),
        ("ecad.package.edit", json!({"package": "PKG", "rotation": 90})),
        ("ecad.package.delete", json!({"package": "PKG"})),
        ("ecad.schematic.place", json!({"parts": [{"component": "R0603", "at": [20.32, 20.32]}]})),
        ("ecad.schematic.edit", json!({"parts": [{"part": "R1", "value": "1k", "dnp": true}]})),
        ("ecad.schematic.move", json!({"items": ["R2"], "by": [0, 5.08]})),
        ("ecad.schematic.duplicate", json!({"items": ["R1"], "by": [0, -20.32]})),
        ("ecad.schematic.delete", json!({"items": ["C1"]})),
        ("ecad.schematic.annotate", json!({"order": "position", "keep_existing": false})),
        ("ecad.schematic.inspect", json!({"pin": "R1.2"})),
        ("ecad.schematic.add", json!({"parts": [{"component": "C0603", "at": [-20.32, 0]}]})),
        ("ecad.schematic.wire", json!({"wires": [{"from": "J1.2", "to": [45.72, -10.16]}]})),
        ("ecad.schematic.connect", json!({"nets": [{"pins": ["J1.1", "J2.1"], "by": "labels", "name": "LINK"}]})),
        ("ecad.schematic.label", json!({"labels": [{"name": "OUT", "at": "R2.2"}]})),
        ("ecad.schematic.power", json!({"ports": [{"name": "+3V3", "at": "J1.1"}]})),
        ("ecad.schematic.marker", json!({"markers": [{"at": "J1.1"}]})),
        ("ecad.schematic.marker.power_flag", json!({"markers": [{"at": "J2.1"}]})),
        ("ecad.schematic.wire.edit", json!({"wire": 6, "via": [[10.16, 2.54]]})),
        ("ecad.schematic.net.rename", json!({"net": "SIG", "to": "SIGNAL"})),
        ("ecad.netlist.inspect", json!({"net": "GND"})),
        ("ecad.netlist.export", json!({})),
        ("ecad.erc.inspect", json!({"severity": "warning", "include_approved": true})),
        ("ecad.erc.approve", json!({"keys": [keys[0]], "note": "a test"})),
        ("ecad.erc.unapprove", json!({"stale": true})),
        ("ecad.erc.rules", json!({"rules": {"pin_unconnected": "warning"}})),
        ("ecad.schematic.frame", json!({"title": "Power", "rect": [[-30, 30], [0, 50]], "order": 2})),
        ("ecad.schematic.frame.edit", json!({"frame": "Main", "title": "Main sheet"})),
        ("ecad.design.info", json!({"company": "Example", "revisions": [{"rev": "A", "date": "2026-10-30", "note": "first"}]})),
        ("ecad.design.info.inspect", json!({})),
        ("ecad.export.pages.inspect", json!({"format": "pdf"})),
        ("ecad.board.outline", json!({"rect": [32, 22], "corner_radius": 1})),
        ("ecad.board.sync", json!({})),
        ("ecad.board.move", json!({"items": ["C1"], "by": [0, 1]})),
        ("ecad.board.rotate", json!({"items": ["C1"]})),
        ("ecad.board.flip", json!({"items": ["C1"]})),
        ("ecad.board.delete", json!({"items": [13]})),
        ("ecad.board.track", json!({"layer": "top", "points": [[11.2, -7], [6, -7]], "width": 0.3})),
        ("ecad.board.track.edit", json!({"tracks": [8], "width": 0.25})),
        ("ecad.board.via", json!({"at": [0, 4], "net": "GND"})),
        ("ecad.board.inspect", json!({})),
        ("ecad.board.stack", json!({"copper_layers": 6})),
        ("ecad.board.hole", json!({"at": [13, -2], "drill": 2.2})),
        ("ecad.board.hole.edit", json!({"holes": [12], "drill": 3.0})),
        ("ecad.board.edit", json!({"footprints": [{"footprint": "R1", "name_at": [0, 1.5]}]})),
        ("ecad.board.names", json!({"height": 0.8})),
        ("ecad.board.track.reshape", json!({"track": 8, "points": [[-9.2, 0], [0, 1], [9.2, 0]]})),
        ("ecad.board.track.split", json!({"track": 8, "segment": 0, "at": [0, 0]})),
        ("ecad.board.track.miter", json!({"track": 8, "radius": 0.5})),
        ("ecad.board.unroute", json!({"items": [8]})),
        ("ecad.board.draw.line", json!({"layer": "silkscreen_top", "points": [[-14, 9], [-10, 9]]})),
        ("ecad.board.draw.rect", json!({"layer": "documentation", "from": [-5, -5], "to": [5, 5]})),
        ("ecad.board.draw.circle", json!({"layer": "silkscreen_bottom", "center": [0, 8], "radius": 0.5})),
        ("ecad.board.draw.text", json!({"layer": "silkscreen_bottom", "at": [5, 8], "text": "REV A", "height": 1.5})),
        ("ecad.board.draw.edit", json!({"graphic": 13, "height": 1.5})),
        ("ecad.board.pour", json!({"layer": "bottom", "net": "GND", "rect": [30, 20]})),
        ("ecad.board.pour.edit", json!({"pour": 6, "clearance": 0.3})),
        ("ecad.board.pour.inspect", json!({"pour": 6, "polygons": true})),
        ("ecad.board.keepout", json!({"layers": ["top"], "circle": {"center": [5, -5], "radius": 1}})),
        ("ecad.board.keepout.edit", json!({"keepout": 11, "forbid": {"pours": false}})),
        ("ecad.board.pour.connect", json!({"footprint": "J1", "pads": ["2"], "connect": "solid"})),
        ("ecad.board.via.edit", json!({"vias": [9], "diameter": 0.7})),
        ("ecad.board.rules", json!({"clearance": 0.25})),
        ("ecad.board.netclass", json!({"name": "Signal", "nets": ["SIG"], "track_width": 0.2})),
        ("ecad.board.netclass.delete", json!({"name": "Power"})),
        ("ecad.board.rules.inspect", json!({})),
        ("ecad.board.impedance.inspect", json!({"layer": "top", "kind": "microstrip", "width": 0.3})),
        ("ecad.board.current.inspect", json!({"current": 1})),
        ("ecad.board.stitch", json!({"net": "GND", "pattern": "grid", "under_parts": true})),
        ("ecad.board.stitch.remove", json!({"net": "GND"})),
        ("ecad.board.ground.inspect", json!({"map": 4})),
        ("ecad.drc.inspect", json!({"limit": 50})),
        ("ecad.drc.approve", json!({"keys": [keys[1]], "note": "a test"})),
        ("ecad.drc.unapprove", json!({"all": true})),
        ("ecad.drc.rules", json!({"rules": {"silk_over_pad": "ignore"}})),
        ("ecad.board.check", json!({"stage": "route"})),
        ("ecad.board.profile", json!({"fab": "fine"})),
        ("ecad.board.profile.inspect", json!({"fab": "hdi"})),
        ("ecad.intent.inspect", json!({"propose": true})),
        ("ecad.intent.set", json!({"nets": [{"net": "OTHER", "role": "slow"}]})),
        ("ecad.quality.inspect", json!({"families": ["pi"]})),
        ("ecad.quality.rules", json!({"rules": {"pi.stitching": "info"}})),
        ("ecad.quality.approve", json!({"keys": [keys[2]], "note": "a test"})),
        ("ecad.quality.unapprove", json!({"stale": true})),
        ("ecad.board.net.inspect", json!({"nets": ["SIG"]})),
        ("ecad.board.loop.inspect", json!({})),
    ]
}

/// Every electronics command's input and output schemas, as `engine.describe` gives them.
fn schemas(s: &mut Session, id: &str) -> (Value, Value) {
    let d = run(s, "engine.describe", json!({"command": id}));
    (d["input"].clone(), d["output"].clone())
}

fn ecad_ids() -> Vec<&'static str> {
    crate::command_specs().iter().map(|c| c.id).filter(|id| id.starts_with("ecad.")).collect()
}

#[test]
fn every_electronics_command_has_its_schemas_and_a_call_here() {
    let mut s = board();
    let keys = first_keys(&mut s);
    assert!(keys.iter().all(|k| !k.is_empty()), "the board has a finding of each check: {keys:?}");
    let called: Vec<&str> = calls(&keys).iter().map(|(id, _)| *id).chain(["ecad.export.pages", "ecad.view.board_3d"]).collect();
    for id in ecad_ids() {
        let spec = crate::find_command(id).unwrap();
        assert!(spec.contract.is_some() && spec.output.is_some(), "{id} has an input and an output schema");
        let (input, output) = schemas(&mut s, id);
        assert_eq!((input["type"].clone(), output["type"].clone()), (json!("object"), json!("object")), "{id}");
        if id.ends_with(".inspect") {
            assert!(!spec.undoable, "{id} only reads");
            assert!(crate::control_pending::command_reads_only(id), "{id}");
        }
        assert!(called.contains(&id), "{id} has no call in `calls`: add one, so the schema, undo and hostile tests cover it");
    }
}

#[test]
fn each_call_matches_its_schemas_and_is_one_undo_step() {
    let fixture = board();
    let mut probe = fixture.scratch();
    let keys = first_keys(&mut probe);
    for (id, p) in calls(&keys) {
        let (_, output) = schemas(&mut probe, id);
        let spec = crate::find_command(id).unwrap();
        // Strict refuses a key the command doesn't take and changes nothing.
        let mut s = fixture.scratch();
        s.param_check = crate::schema::ParamCheck::Strict;
        let mut typo = p.clone();
        typo["bogus"] = json!(1);
        let e = s.execute(id, &typo).expect_err("refused").to_string();
        assert!(e.contains("unknown key `bogus`"), "{id}: {e}");
        assert!(Arc::ptr_eq(&s.doc, &fixture.doc), "{id}: refused, nothing changes");
        // Warn runs it and says so; the reply matches the output schema.
        let mut s = fixture.scratch();
        s.param_check = crate::schema::ParamCheck::Warn;
        let before = s.doc.clone();
        let r = run(&mut s, id, typo);
        assert!(crate::schema::tests::conforms(&output, &output, &r), "{id}: {r} doesn't match {output}");
        assert!(r["param_warnings"].as_array().is_some_and(|w| !w.is_empty()), "{id}: {r}");
        let after = s.doc.clone();
        if Arc::ptr_eq(&before, &after) || *before == *after {
            assert_eq!(s.undo.len(), 0, "{id}: a call that changes nothing makes no undo step");
            continue;
        }
        assert!(spec.undoable && !id.ends_with(".inspect"), "{id} changed the design");
        assert_eq!(s.undo.len(), 1, "{id}: one call, one undo step");
        run(&mut s, "edit.undo", json!({}));
        assert!(*s.doc == *before, "{id}: undo brings the design back");
        run(&mut s, "edit.redo", json!({}));
        assert!(*s.doc == *after, "{id}: redo makes it again");
        assert!(s.doc.ecad.validate().is_ok(), "{id} left a design the file would refuse");
    }
}

/// `p` with the value at each place in it (each key, and each key of an object in it or the
/// first in a list in it, two levels down) replaced by `junk`.
fn spoiled(p: &Value, junk: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let Some(o) = p.as_object() else { return out };
    for (k, v) in o {
        let mut q = p.clone();
        q[k] = junk.clone();
        out.push(q);
        let inner = match v {
            Value::Object(_) => Some(v.clone()),
            Value::Array(a) => a.first().filter(|f| f.is_object()).cloned(),
            _ => None,
        };
        for spoilt in inner.iter().flat_map(|i| spoiled(i, junk)) {
            let mut q = p.clone();
            match &mut q[k] {
                Value::Array(a) => {
                    if let Some(first) = a.first_mut() {
                        *first = spoilt;
                    }
                }
                slot => *slot = spoilt,
            }
            out.push(q);
        }
    }
    out
}

/// Every electronics command, every parameter its schema names, every hostile value, on a board
/// that has something of every kind: alone, in place of each value of a call that works (two
/// levels down), and as the whole parameters. No panic escapes, and the design stays one the
/// file takes.
#[test]
fn hostile_electronics_params_never_panic() {
    let mut probe = board();
    let keys = first_keys(&mut probe);
    let good: std::collections::BTreeMap<&str, Value> = calls(&keys).into_iter().collect();
    let jobs: Vec<(&str, Vec<String>)> = ecad_ids()
        .into_iter()
        .map(|id| {
            let (input, _) = schemas(&mut probe, id);
            let names = input["properties"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
            (id, names)
        })
        .collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let failures: Vec<String> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let fixture = board();
                    let mut failures = Vec::new();
                    while let Some((id, names)) = jobs.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                        let mut try_one = |p: &Value| {
                            let mut s = fixture.scratch();
                            match s.execute(id, p) {
                                Err(crate::EngineError::Internal(_, msg)) => failures.push(format!("{id} {p}: {msg}")),
                                _ if s.doc.ecad.validate().is_err() => failures.push(format!("{id} {p} left a design the file would refuse")),
                                _ => {}
                            }
                        };
                        for junk in crate::tests::hostile_values() {
                            try_one(&junk);
                            for k in names.iter().filter(|k| k.as_str() != "path") {
                                try_one(&Value::Object([(k.clone(), junk.clone())].into_iter().collect()));
                            }
                            for p in good.get(id).map(|p| spoiled(p, &junk)).unwrap_or_default() {
                                try_one(&p);
                            }
                        }
                    }
                    failures
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap()).collect()
    });
    assert!(failures.is_empty(), "{} hostile calls failed:\n{}", failures.len(), failures.join("\n"));
}
