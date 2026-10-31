//! The design-quality checks, the approvals of every check (who made them: the user in the app's
//! windows, else an agent) and the `findings_delta` board edits give, on the test board
//! (`ecad::tests::board`: SIG on top over the GND planes on inner1 and inner2).

use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_doc::ecad::Actor;

use crate::Session;
use crate::cmd::ecad::tests::{board, run};

fn refused(s: &mut Session, id: &str, p: Value) -> String {
    let (before, undo) = (s.doc.clone(), s.undo.len());
    let Err(e) = s.execute(id, &p) else { panic!("{id} {p} must be refused") };
    assert!(Arc::ptr_eq(&before, &s.doc), "a refused {id} leaves the design as it was");
    assert_eq!(s.undo.len(), undo);
    e.to_string()
}

fn findings(s: &mut Session, p: Value) -> Vec<Value> {
    run(s, "ecad.quality.inspect", p)["findings"].as_array().cloned().unwrap_or_default()
}

/// The approved findings `ecad.quality.inspect` lists apart with `include_approved`.
fn approved(s: &mut Session, mut p: Value) -> Vec<Value> {
    p["include_approved"] = json!(true);
    run(s, "ecad.quality.inspect", p)["approved"].as_array().cloned().unwrap_or_default()
}

/// The OTHER net's track from J1.1 across the inner1 plane under SIG: it cuts the plane SIG's
/// return current runs in.
fn slot(s: &mut Session) -> Value {
    run(s, "ecad.board.track", json!({"layer": "inner1", "points": [[3, 7.27], [0, 7.27], [0, -4]], "width": 0.2}))
}

#[test]
fn a_return_path_over_a_slot_is_found_and_goes_with_it() {
    let mut s = board();
    assert!(findings(&mut s, json!({"codes": ["pi.return_path"]})).is_empty(), "SIG over a whole plane");
    let r = slot(&mut s);
    let track = r["created"][0]["id"].as_u64().unwrap();
    let found = findings(&mut s, json!({"codes": ["pi.return_path"]}));
    assert_eq!(found.len(), 1, "{found:?}");
    let f = &found[0];
    assert_eq!((f["class"].clone(), f["severity"].clone(), f["family"].clone()), (json!("critical"), json!("error"), json!("pi")));
    assert_eq!(f["key"], "pi.return_path|t8|z6", "keys name ids only: SIG's track 8 over pour 6");
    assert!(f["message"].as_str().unwrap().contains(&format!("track {track} (OTHER)")), "{f}");
    // The edit's reply says it, as the checks do.
    let introduced = r["findings_delta"]["introduced"].as_array().unwrap();
    assert!(introduced.iter().any(|i| i["key"] == "pi.return_path|t8|z6"), "{r}");
    // Filters: by family, net, layer and class; a family not built yet is said so.
    assert_eq!(findings(&mut s, json!({"families": ["pi"], "nets": ["SIG"], "class": "critical"})).len(), 1);
    assert!(findings(&mut s, json!({"nets": ["VCC"], "codes": ["pi.return_path"]})).is_empty());
    let summary = run(&mut s, "ecad.quality.inspect", json!({"summary": true}));
    assert!(summary.get("findings").is_none() && summary["summary"]["by_code"]["pi.return_path"] == 1, "{summary}");
    let r = run(&mut s, "ecad.board.delete", json!({"items": [track]}));
    assert_eq!(r["findings_delta"]["resolved"].as_u64().map(|n| n >= 1), Some(true), "{r}");
    assert!(findings(&mut s, json!({"codes": ["pi.return_path"]})).is_empty());
}

#[test]
fn a_layer_change_needs_a_return_via_near_it() {
    let mut s = board();
    run(&mut s, "ecad.board.delete", json!({"items": [8]}));
    run(&mut s, "ecad.board.track", json!({"layer": "top", "points": [[-9.2, 0], [-3, 0]], "width": 0.2, "via": {}}));
    run(&mut s, "ecad.board.track", json!({"layer": "bottom", "points": [[-3, 0], [3, 0]], "width": 0.2, "via": {}}));
    run(&mut s, "ecad.board.track", json!({"layer": "top", "points": [[3, 0], [9.2, 0]], "width": 0.2}));
    // The via at (3, 0) has J1's through-hole GND pad 4.73 mm away, which joins the planes; the
    // one at (−3, 0) has nothing within 5 mm.
    let bad = findings(&mut s, json!({"codes": ["pi.layer_change"]}));
    assert_eq!(bad.len(), 1, "{bad:?}");
    assert!(bad[0]["at"] == json!([-3.0, 0.0]) && bad[0]["severity"] == "warning" && bad[0]["class"] == "practical", "{bad:?}");
    // A GND via within the distance the check allows (5 mm).
    run(&mut s, "ecad.board.via", json!({"at": [0, -2], "net": "GND"}));
    assert!(findings(&mut s, json!({"codes": ["pi.layer_change"]})).is_empty());
}

#[test]
fn approvals_keep_who_made_them_and_an_agent_waits_for_the_user_on_critical_ones() {
    let mut s = board();
    slot(&mut s);
    let critical = "pi.return_path|t8|z6".to_string();
    let practical = findings(&mut s, json!({"codes": ["pi.stitching"]}))[0]["key"].as_str().unwrap().to_string();
    assert_eq!(s.actor, Actor::Agent, "a program's calls are an agent's");
    let e = refused(&mut s, "ecad.quality.approve", json!({"keys": [critical]}));
    assert!(e.contains("needs a `note`"), "{e}");
    let e = refused(&mut s, "ecad.quality.approve", json!({"keys": ["pi.return_path|t8|z7"], "note": "x"}));
    assert!(e.contains("did you mean `pi.return_path|t8|z6`"), "{e}");
    let r = run(&mut s, "ecad.quality.approve", json!({"keys": [critical, practical], "note": "checked against the stack-up"}));
    assert_eq!(r["added"], json!([critical, practical]));
    assert_eq!(r["pending_user"], json!([critical]), "an agent's approval of a critical finding waits for the user");
    assert_eq!((r["counts"]["errors"].clone(), r["counts"]["pending_user"].clone()), (json!(1), json!(1)), "and still counts: {r}");
    let listed = findings(&mut s, json!({"codes": ["pi.return_path"]}));
    assert_eq!(
        (listed[0]["approved"].clone(), listed[0]["pending_user"].clone(), listed[0]["by"].clone()),
        (json!(false), json!(true), json!("agent")),
        "listed as open until the user confirms it"
    );
    assert!(approved(&mut s, json!({"codes": ["pi.return_path"]})).is_empty());
    assert_eq!(approved(&mut s, json!({"codes": ["pi.stitching"]}))[0]["by"], "agent", "a practical one is approved at once");
    // The user confirms it in the app; then it no longer counts, and an agent leaves it be.
    s.actor = Actor::User;
    let r = run(&mut s, "ecad.quality.approve", json!({"keys": [critical]}));
    assert_eq!((r["confirmed"].clone(), r["counts"]["errors"].clone()), (json!([critical]), json!(0)), "{r}");
    s.actor = Actor::Agent;
    let r = run(&mut s, "ecad.quality.approve", json!({"keys": [critical], "note": "another note"}));
    assert_eq!(r["kept"], json!([critical]));
    let r = run(&mut s, "ecad.quality.unapprove", json!({"all": true}));
    assert_eq!((r["removed"].clone(), r["kept"].clone()), (json!([practical]), json!([critical])), "an agent takes back only its own");
    let listed = approved(&mut s, json!({"codes": ["pi.return_path"]}));
    assert_eq!((listed[0]["by"].clone(), listed[0]["note"].clone()), (json!("user"), json!("checked against the stack-up")));
    assert!(findings(&mut s, json!({"codes": ["pi.return_path"]})).is_empty(), "approved by the user, no longer open");
    s.actor = Actor::User;
    let r = run(&mut s, "ecad.quality.unapprove", json!({"keys": [critical]}));
    assert_eq!(r["removed"], json!([critical]));
    let e = refused(&mut s, "ecad.quality.unapprove", json!({"keys": [critical]}));
    assert!(e.contains("not approved"), "{e}");
}

#[test]
fn an_approval_whose_finding_is_gone_is_stale() {
    let mut s = board();
    run(&mut s, "ecad.board.delete", json!({"items": [12]}));
    let key = findings(&mut s, json!({"codes": ["pi.stitching"]}))[0]["key"].as_str().unwrap().to_string();
    s.actor = Actor::User;
    run(&mut s, "ecad.quality.approve", json!({"keys": [key]}));
    s.actor = Actor::Agent;
    run(&mut s, "ecad.board.stitch", json!({"net": "GND", "pattern": "grid", "under_parts": true}));
    assert!(findings(&mut s, json!({"codes": ["pi.stitching"]})).is_empty() && approved(&mut s, json!({"codes": ["pi.stitching"]})).is_empty());
    let q = run(&mut s, "ecad.quality.inspect", json!({}));
    assert_eq!((q["counts"]["stale"].clone(), q["stale"].clone()), (json!(1), json!([key])), "{q}");
    let r = run(&mut s, "ecad.quality.unapprove", json!({"stale": true}));
    assert_eq!(r["removed"], json!([key]), "even the user's, which approves nothing now");
}

#[test]
fn design_rule_and_electrical_approvals_follow_the_same_rules() {
    let mut s = board();
    // An unrouted connection is critical: the agent's approval waits; the user's counts.
    let drc = run(&mut s, "ecad.drc.inspect", json!({"codes": ["unrouted"]}));
    let key = drc["findings"][0]["key"].as_str().unwrap().to_string();
    assert_eq!(drc["findings"][0]["class"], "critical");
    let r = run(&mut s, "ecad.drc.approve", json!({"keys": [key], "note": "routed by hand later"}));
    assert_eq!(r["pending_user"], json!([key]));
    s.actor = Actor::User;
    let r = run(&mut s, "ecad.drc.approve", json!({"keys": [key]}));
    assert_eq!(r["confirmed"], json!([key]));
    s.actor = Actor::Agent;
    let r = run(&mut s, "ecad.drc.unapprove", json!({"keys": [key]}));
    assert!(r["removed"].as_array().is_none_or(Vec::is_empty) && r["kept"] == json!([key]), "{r}");
    // A schematic finding the same way: J1.1 is the only pin on OTHER.
    let erc = run(&mut s, "ecad.erc.inspect", json!({}));
    let single = erc["findings"].as_array().unwrap().iter().find(|f| f["code"] == "net_single_pin").unwrap()["key"].clone();
    let e = refused(&mut s, "ecad.erc.approve", json!({"keys": [single]}));
    assert!(e.contains("needs a `note`"), "{e}");
    let r = run(&mut s, "ecad.erc.approve", json!({"keys": [single], "note": "a test point"}));
    assert!(r["added"] == json!([single]) && r["pending_user"].as_array().is_none_or(Vec::is_empty), "not a critical finding: {r}");
    s.actor = Actor::User;
    let r = run(&mut s, "ecad.erc.approve", json!({"keys": [single]}));
    assert_eq!(r["confirmed"], json!([single]));
    let saved = &s.doc.ecad.schematic.approvals;
    assert_eq!(saved.iter().find(|a| json!(a.key) == single).and_then(|a| a.by), Some(Actor::User), "saved with who made it");
}

#[test]
fn only_the_user_lowers_a_critical_code_and_agents_keep_the_users_settings() {
    let mut s = board();
    slot(&mut s);
    let e = refused(&mut s, "ecad.quality.rules", json!({"rules": {"pi.return_path": "ignore"}}));
    assert!(e.contains("only the user lowers"), "{e}");
    let e = refused(&mut s, "ecad.quality.rules", json!({"rules": {"pi.retrun_path": "warning"}}));
    assert!(e.contains("did you mean `pi.return_path`"), "{e}");
    // A practical code an agent may set; a parameter needs a note and keeps who set it.
    run(&mut s, "ecad.quality.rules", json!({"rules": {"pi.stitching": "info"}}));
    let e = refused(&mut s, "ecad.quality.rules", json!({"params": {"pi.stitching": {"fraction": 0.2}}}));
    assert!(e.contains("give a `note`"), "{e}");
    let e = refused(&mut s, "ecad.quality.rules", json!({"params": {"pi.stitching": {"fraction": 5}}, "note": "x"}));
    assert!(e.contains("0.01 to 1"), "{e}");
    run(&mut s, "ecad.quality.rules", json!({"params": {"pi.stitching": {"fraction": 0.2}}, "note": "the product's own limit"}));
    assert_eq!(s.doc.ecad.board.quality_params["pi.stitching.fraction"].by, Some(Actor::Agent));
    s.actor = Actor::User;
    let r = run(
        &mut s,
        "ecad.quality.rules",
        json!({"rules": {"pi.return_path": "warning"}, "params": {"pi.stitching": {"fraction": 0.15}}, "note": "mine"}),
    );
    assert_eq!(r["rules"]["pi.return_path"], "warning");
    assert_eq!(findings(&mut s, json!({"codes": ["pi.return_path"]}))[0]["severity"], "warning", "the level rewrites the severity");
    s.actor = Actor::Agent;
    let r = run(&mut s, "ecad.quality.rules", json!({"params": {"pi.stitching": {"fraction": 0.3}}, "note": "again"}));
    assert_eq!((r["kept"].clone(), r["params"]["pi.stitching.fraction"].clone()), (json!(["pi.stitching.fraction"]), json!(0.15)));
    // Raising a critical code back is anyone's.
    run(&mut s, "ecad.quality.rules", json!({"rules": {"pi.return_path": null}}));
    assert_eq!(findings(&mut s, json!({"codes": ["pi.return_path"]}))[0]["severity"], "error");
    // With neither, the catalogue: every code with its section and parameters.
    let all = run(&mut s, "ecad.quality.rules", json!({}));
    let codes = all["codes"].as_array().unwrap();
    assert!(codes.iter().all(|c| c["section"].as_str().is_some_and(|x| !x.is_empty())), "{all}");
    assert!(codes.iter().any(|c| c["code"] == "pi.stitching" && c["params"].as_array().is_some_and(|p| p.iter().any(|x| x["name"] == "fraction"))));
}

#[test]
fn board_edits_report_the_findings_they_change_when_asked() {
    let mut s = board();
    assert!(s.board_delta, "programs get it by default");
    let r = slot(&mut s);
    assert!(r["findings_delta"]["introduced"].as_array().is_some_and(|i| !i.is_empty()), "{r}");
    let r = run(&mut s, "ecad.board.via", json!({"at": [-5, 5], "net": "GND", "findings_delta": false}));
    assert!(r.get("findings_delta").is_none(), "{r}");
    let e = refused(&mut s, "ecad.board.via", json!({"at": [-5, 5], "net": "GND", "findings_delta": "yes"}));
    assert!(e.contains("`findings_delta` is true or false"), "{e}");
    // The app's own windows turn it off; a call can still ask.
    s.board_delta = false;
    let r = run(&mut s, "ecad.board.via", json!({"at": [-7, 5], "net": "GND"}));
    assert!(r.get("findings_delta").is_none());
    let r = run(&mut s, "ecad.board.via", json!({"at": [-9, 5], "net": "GND", "findings_delta": true}));
    let delta = &r["findings_delta"];
    assert!(delta["introduced"].is_array() && delta["resolved"].is_u64(), "{r}");
    assert!(delta["introduced"].as_array().unwrap().iter().all(|f| f["code"] != "clearance"), "a via clear of everything: {r}");
    // Only board edits take it; a read or a schematic edit has no such key.
    s.board_delta = true;
    let r = run(&mut s, "ecad.schematic.move", json!({"items": ["R2"], "by": [0, 5.08]}));
    assert!(r.get("findings_delta").is_none());
    let r = run(&mut s, "ecad.board.inspect", json!({}));
    assert!(r.get("findings_delta").is_none());
    // An edit that changes nothing reports nothing.
    let r = run(&mut s, "ecad.board.move", json!({"items": ["R1"], "by": [0, 0]}));
    assert!(r.get("findings_delta").is_none(), "{r}");
}

#[test]
fn the_board_check_lists_its_gates_in_order_and_names_what_got_worse() {
    let mut s = board();
    let (doc, undo) = (s.doc.clone(), s.undo.len());
    let c = run(&mut s, "ecad.board.check", json!({}));
    assert!(Arc::ptr_eq(&doc, &s.doc) && s.undo.len() == undo, "the check only reads");
    let gates: Vec<&str> = c["gates"].as_array().unwrap().iter().filter_map(|g| g["gate"].as_str()).collect();
    assert_eq!(gates, ["completeness", "connectivity", "fab", "critical", "intent", "practical", "cosmetic"]);
    assert_eq!((c["stage"].clone(), c["done"].clone()), (json!("route"), json!(false)), "four connections are still to route");
    assert_eq!(c["gates"][0]["status"], "fail");
    assert!(c.get("regressions").is_none(), "the first check is the baseline");
    slot(&mut s);
    let c = run(&mut s, "ecad.board.check", json!({}));
    assert!(c["regressions"].as_array().is_some_and(|r| !r.is_empty()), "the slot made the critical findings worse: {c}");
    // The user's checks are kept apart from an agent's: the user's first is a baseline too.
    s.actor = Actor::User;
    let c = run(&mut s, "ecad.board.check", json!({}));
    assert!(c.get("regressions").is_none(), "{c}");
    // A rule change makes the next check a new baseline.
    s.actor = Actor::Agent;
    run(&mut s, "ecad.board.rules", json!({"clearance": 0.25}));
    let c = run(&mut s, "ecad.board.check", json!({}));
    assert_eq!(c["rules_changed"], true, "{c}");
    let e = refused(&mut s, "ecad.board.check", json!({"stage": "ship"}));
    assert!(e.contains("`stage`"), "{e}");
}

#[test]
fn a_fab_profile_sets_the_rules_it_needs_and_never_loosens_the_users() {
    let mut s = board();
    let p = run(&mut s, "ecad.board.profile.inspect", json!({}));
    let ids: Vec<&str> = p["profiles"].as_array().unwrap().iter().filter_map(|x| x["id"].as_str()).collect();
    assert_eq!(ids, ["standard", "fine", "hdi"]);
    assert_eq!(p["current"]["id"], "standard", "the board's");
    run(&mut s, "ecad.board.rules", json!({"min_drill": 0.4}));
    let r = run(&mut s, "ecad.board.profile", json!({"fab": "hdi"}));
    assert_eq!(r["profile"]["id"], "hdi");
    assert!(r["kept"].as_array().is_some_and(|k| k.contains(&json!({"field": "min_drill", "value": 0.4}))), "{r}");
    assert!(r["rules_set"].as_array().is_some_and(|k| !k.is_empty()), "the rules that had the old profile's figures follow: {r}");
    let rules = run(&mut s, "ecad.board.rules.inspect", json!({}))["rules"].clone();
    assert_eq!(rules["min_drill"], 0.4, "a finer profile never loosens a rule set on the board: {rules}");
    run(&mut s, "ecad.board.profile", json!({"fab": null}));
    let c = run(&mut s, "ecad.board.check", json!({}));
    let fab = c["gates"].as_array().unwrap().iter().find(|g| g["gate"] == "fab").unwrap().clone();
    assert_ne!(fab["status"], "pass", "with no profile the check says so: {fab}");
    let e = refused(&mut s, "ecad.board.profile", json!({"fab": "cheap"}));
    assert!(e.contains("standard"), "{e}");
}
