//! The design's intent (see `solidcraft_doc::ecad::intent`): read what is declared, what the
//! engine proposes from the schematic and the library (`solidcraft_doc::ecad::infer`) and what
//! still blocks the checks; declare records, accept proposals (one by one after a look, or the
//! low-risk ones in bulk) and take records away. Records keep who made them
//! (`Session::actor`): an agent leaves the user's records as they are, and says why (`note`) when
//! it gives a fact only the product tells or drops a blocking role the engine proposes. A change
//! is one undo step, all or none, and a call that changes nothing makes none.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use solidcraft_doc::ecad::infer::{self, Coverage, Inference, Record};
use solidcraft_doc::ecad::intent::{BoardFacts, Domain, Intent, LoopIntent, MAX_RECORDS, NetIntent, PairIntent, PartIntent, Source};
use solidcraft_doc::ecad::{Actor as DocActor, MAX_DESCRIPTION, check_text};

use super::CommandSpec;
use super::ecad::{closest, parse, reply};
use crate::params::bad;
use crate::schema::ecad_intent::{
    BlockingOut, CoverageOut, InspectParams, InspectResult, MissingOut, MoreOut, PowerPinsOut, ProposalKind, ProposalOut, Rec, SetParams, SetResult,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.intent.inspect", "Design Intent", crate::checked!("ecad.intent.inspect", inspect))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&INSPECT_CONTRACT)
        .output(output_schema_for::<InspectResult>)
        .params(
            "propose? (true); nets? [name]; parts? [designator]; roles? [role]; blocking? (only proposals that need a look); limit? (25) offset? → {coverage: {blocking: {total, confirmed, missing: [{key, role, needs, why}], missing_more?}, ic_power_pins: {total, assigned, proposed}, unknown?}, board?, nets, pairs, parts, loops, domains (the records kept), proposals: [{key, kind, role, blocking, confidence, needs?, evidence, record}], more: {records?, proposals?}}: what the design's nets and parts are for. The engine proposes roles from the schematic, the library and the board (ground and rail ports and names, pin types and names, prefixes, values, descriptions, placed distances): supplies with their voltage and the current their regulator is rated for, ground, decoupling of the nearest IC supply pin or power output, crystals and load capacitors, ESD protection of connector pins, connectors, regulators with their switch nodes (also an IC's own DC/DC by its DCC/SW/LX pin), inductors, capacitors, feedback and hot loops, chargers and linear regulators (power_ic), RF lines through their matching parts, pairs, buses and clocks; nothing is kept until accepted. Blocking roles (supplies, ground, clocks, RF, pairs, buses, decoupling, crystals, protection, connectors, antennas, regulators) need a look; a supply's current (not for one that only reaches supply-detect inputs), a connector's cable and the radio band are asked of the user. Read-only",
        ),
    CommandSpec::new("ecad.intent.set", "Set Design Intent", crate::checked!("ecad.intent.set", set))
        .icon("edit")
        .doc_only()
        .contract(&SET_CONTRACT)
        .output(output_schema_for::<SetResult>)
        .params(
            "nets? [{net, role, current_a?, voltage?: {nominal, peak?}, edge_ns?, driver?, z_target?, z_tol?, interface?, f_mhz?, external?, domain?, source? declared|confirmed, evidence?, note?}]; parts? [{part, role, of?, cable?, power_w?, loop?, …}]; pairs? [{name, p, n, interface?, skew_mm?, …}]; loops? [{name, kind, members, …}]; domains? [{name, kind, parts?}]; board? {product?, ipc_class?, enclosure?, f_concern_mhz?, bands?: [{name, lo_mhz, hi_mhz, rx?}], hand_assembly?, note?}; accept? [key] (proposals kept as proposed, confirmed); accept_low_risk? true (every non-blocking proposal with no record yet, as inferred; a regulator's capacitors, inductor, switch nodes and loops once the regulator is confirmed); remove? [key]; note? → {changed, removed, kept, coverage}: one undo step, all or none. A record replaces the one for its net, part, pair, loop or domain; board facts given replace those kept (give every band at once); names the schematic doesn't have are kept (and listed as unknown). An agent leaves the user's records as they are (kept); what only the product tells (a supply's current, a connector's cable, the radio bands) an agent gives as the user said it, with a note saying so, and so does a record that gives a subject proposed as blocking a role that isn't",
        ),
];

const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();
const SET_CONTRACT: TypedContract<SetParams> = TypedContract::new();

/// The most records, proposals and missing items `ecad.intent.inspect` lists by default.
const DEFAULT_LIMIT: usize = 25;
/// The most it lists when asked.
const MAX_LIMIT: usize = 4096;

fn record_value(r: &Record) -> Value {
    let v = match r {
        Record::Net(x) => serde_json::to_value(x),
        Record::Part(x) => serde_json::to_value(x),
        Record::Pair(x) => serde_json::to_value(x),
        Record::Loop(x) => serde_json::to_value(x),
        Record::Board(x) => serde_json::to_value(x),
    };
    v.unwrap_or(Value::Null)
}

fn kind_of(r: &Record) -> ProposalKind {
    match r {
        Record::Net(_) => ProposalKind::Net,
        Record::Part(_) => ProposalKind::Part,
        Record::Pair(_) => ProposalKind::Pair,
        Record::Loop(_) => ProposalKind::Loop,
        Record::Board(_) => ProposalKind::Board,
    }
}

pub(super) fn coverage_out(c: &Coverage, limit: usize) -> CoverageOut {
    let missing: Vec<MissingOut> = c
        .missing
        .iter()
        .take(limit)
        .map(|m| MissingOut {
            key: m.key.clone(),
            role: m.role.to_string(),
            needs: m.needs.iter().map(|n| n.to_string()).collect(),
            why: m.why.to_string(),
        })
        .collect();
    CoverageOut {
        blocking: BlockingOut {
            total: c.blocking_total,
            confirmed: c.blocking_confirmed,
            missing_more: c.missing.len().saturating_sub(missing.len()),
            missing,
        },
        ic_power_pins: PowerPinsOut { total: c.ic_power_pins, assigned: c.ic_power_pins_assigned, proposed: c.ic_power_pins_proposed },
        unknown: c.unknown.iter().take(limit).cloned().collect(),
    }
}

/// A list a call names, within its cap.
fn capped<'a>(list: &'a Option<Vec<String>>, what: &str, cmd: &str) -> Result<Option<BTreeSet<&'a str>>> {
    match list {
        Some(l) if l.len() > MAX_LIMIT => Err(bad(cmd, format!("`{what}` names at most {MAX_LIMIT}"))),
        Some(l) => Ok(Some(l.iter().map(String::as_str).collect())),
        None => Ok(None),
    }
}

fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.intent.inspect";
    let p: InspectParams = parse(CMD, p)?;
    let limit = p.limit.unwrap_or(DEFAULT_LIMIT);
    if limit > MAX_LIMIT {
        return Err(bad(CMD, format!("`limit` is at most {MAX_LIMIT}")));
    }
    let offset = p.offset.unwrap_or(0);
    let nets = capped(&p.nets, "nets", CMD)?;
    let parts = capped(&p.parts, "parts", CMD)?;
    let roles = capped(&p.roles, "roles", CMD)?;
    let ecad = &s.doc.ecad;
    let inference = infer::infer(ecad);
    if let Some(roles) = &roles {
        let known: BTreeSet<&str> = inference.proposals.iter().map(|x| x.record.role()).chain(KNOWN_ROLES.iter().copied()).collect();
        for r in roles {
            if !known.contains(r) {
                let mut msg = format!("`roles`: there is no role `{r}`");
                if let Some(near) = closest(r, known.iter().copied()) {
                    msg.push_str(&format!(" (did you mean `{near}`?)"));
                }
                return Err(bad(CMD, msg));
            }
        }
    }
    let intent = &ecad.intent;
    let cov = infer::coverage(ecad, intent, &inference);
    // A subject is wanted when the filters leave it in: no filter of its kind, or named there.
    let any_subject = nets.is_some() || parts.is_some();
    let net_ok = |n: &str| if any_subject { nets.as_ref().is_some_and(|x| x.contains(n)) } else { true };
    let part_ok = |n: &str| if any_subject { parts.as_ref().is_some_and(|x| x.contains(n)) } else { true };
    let role_ok = |r: &str| roles.as_ref().is_none_or(|x| x.contains(r));
    let mut more = MoreOut::default();
    let mut window = |n: usize| -> (usize, usize) {
        let take = n.saturating_sub(offset).min(limit);
        more.records += n.saturating_sub(offset).saturating_sub(take);
        (offset, take)
    };
    let pick_nets: Vec<&NetIntent> = intent.nets.iter().filter(|r| net_ok(&r.net) && role_ok(r.role.name())).collect();
    let (skip, take) = window(pick_nets.len());
    let out_nets = pick_nets.into_iter().skip(skip).take(take).cloned().map(Rec).collect();
    let pick_parts: Vec<&PartIntent> = intent.parts.iter().filter(|r| part_ok(&r.part) && role_ok(r.role.name())).collect();
    let (skip, take) = window(pick_parts.len());
    let out_parts = pick_parts.into_iter().skip(skip).take(take).cloned().map(Rec).collect();
    let pick_pairs: Vec<_> = intent.pairs.iter().filter(|r| (net_ok(&r.p) || net_ok(&r.n)) && role_ok("pair")).collect();
    let (skip, take) = window(pick_pairs.len());
    let out_pairs = pick_pairs.into_iter().skip(skip).take(take).cloned().map(Rec).collect();
    let pick_loops: Vec<_> = intent.loops.iter().filter(|r| !any_subject && role_ok(r.kind.name())).collect();
    let (skip, take) = window(pick_loops.len());
    let out_loops = pick_loops.into_iter().skip(skip).take(take).cloned().map(Rec).collect();
    let pick_domains: Vec<_> = intent.domains.iter().filter(|_| !any_subject && roles.is_none()).collect();
    let (skip, take) = window(pick_domains.len());
    let out_domains = pick_domains.into_iter().skip(skip).take(take).cloned().map(Rec).collect();
    let mut proposals = Vec::new();
    if p.propose.unwrap_or(true) {
        let kept = intent.keys();
        let wanted = |x: &&infer::Proposal| {
            !kept.contains(&x.key)
                && (!p.blocking.unwrap_or(false) || x.blocking)
                && role_ok(x.record.role())
                && match &x.record {
                    Record::Net(r) => net_ok(&r.net),
                    Record::Part(r) => part_ok(&r.part),
                    Record::Pair(r) => net_ok(&r.p) || net_ok(&r.n),
                    Record::Loop(_) | Record::Board(_) => !any_subject,
                }
        };
        let mut list: Vec<&infer::Proposal> = inference.proposals.iter().filter(wanted).collect();
        list.sort_by_key(|x| !x.blocking);
        more.proposals = list.len().saturating_sub(offset).saturating_sub(limit);
        proposals = list
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|x| ProposalOut {
                key: x.key.clone(),
                kind: kind_of(&x.record),
                role: x.record.role().to_string(),
                blocking: x.blocking,
                confidence: x.confidence.name().to_string(),
                needs: x.needs.iter().map(|n| n.to_string()).collect(),
                evidence: x.evidence.clone(),
                record: record_value(&x.record),
            })
            .collect();
    }
    reply(InspectResult {
        coverage: coverage_out(&cov, limit),
        board: (!intent.board.is_empty()).then(|| Rec(intent.board.clone())),
        nets: out_nets,
        pairs: out_pairs,
        parts: out_parts,
        loops: out_loops,
        domains: out_domains,
        proposals,
        more,
        param_warnings: Vec::new(),
    })
}

/// Every role name a record may have, for `roles` filters.
const KNOWN_ROLES: &[&str] = &[
    "power",
    "ground",
    "rf",
    "diff",
    "clock",
    "strobe",
    "fast",
    "interface",
    "analog",
    "sensitive",
    "slow",
    "static",
    "hv",
    "sw_node",
    "fb",
    "sense",
    "pair",
    "board",
    "decoupling",
    "bulk",
    "crystal",
    "load_cap",
    "tvs",
    "esd_array",
    "series_term",
    "parallel_term",
    "power_ic",
    "smps_ic",
    "smps_in_cap",
    "smps_out_cap",
    "smps_inductor",
    "smps_diode",
    "fb_divider",
    "sense_resistor",
    "ferrite",
    "filter",
    "connector",
    "antenna",
    "rf_ic",
    "rf_match",
    "module",
    "test_point",
    "debug",
    "fiducial",
    "mounting",
    "hot",
    "mechanical",
    "buck_hot",
    "boost_hot",
    "gate",
    "decap",
    "tvs",
    "sense",
];

/// A list of records with an index by subject, so each record a call keeps costs one lookup.
struct Indexed<'a, T> {
    list: &'a mut Vec<T>,
    at: BTreeMap<String, usize>,
}

impl<'a, T> Indexed<'a, T> {
    fn new(list: &'a mut Vec<T>, subject: impl Fn(&T) -> &str) -> Self {
        let at = list.iter().enumerate().map(|(i, x)| (subject(x).to_string(), i)).collect();
        Indexed { list, at }
    }

    /// `item` in place of the record for `subject`, or added.
    fn put(&mut self, subject: &str, item: T) {
        match self.at.get(subject).and_then(|i| self.list.get_mut(*i)) {
            Some(x) => *x = item,
            None => {
                self.at.insert(subject.to_string(), self.list.len());
                self.list.push(item);
            }
        }
    }
}

/// The intent's lists, indexed (see [`Indexed`]).
struct Lists<'a> {
    nets: Indexed<'a, NetIntent>,
    parts: Indexed<'a, PartIntent>,
    pairs: Indexed<'a, PairIntent>,
    loops: Indexed<'a, LoopIntent>,
    domains: Indexed<'a, Domain>,
    board: &'a mut BoardFacts,
}

impl<'a> Lists<'a> {
    fn new(intent: &'a mut Intent) -> Self {
        let Intent { board, nets, pairs, parts, loops, domains } = intent;
        Lists {
            nets: Indexed::new(nets, |r| r.net.as_str()),
            parts: Indexed::new(parts, |r| r.part.as_str()),
            pairs: Indexed::new(pairs, |r| r.name.as_str()),
            loops: Indexed::new(loops, |r| r.name.as_str()),
            domains: Indexed::new(domains, |r| r.name.as_str()),
            board,
        }
    }

    /// Keep `r` (in place of the record for its subject).
    fn keep(&mut self, r: Record) {
        match r {
            Record::Net(x) => {
                let k = x.net.clone();
                self.nets.put(&k, x);
            }
            Record::Part(x) => {
                let k = x.part.clone();
                self.parts.put(&k, x);
            }
            Record::Pair(x) => {
                let k = x.name.clone();
                self.pairs.put(&k, x);
            }
            Record::Loop(x) => {
                let k = x.name.clone();
                self.loops.put(&k, x);
            }
            Record::Board(x) => merge_board(self.board, x),
        }
    }
}

/// The facts `given` sets, over those kept.
fn merge_board(kept: &mut BoardFacts, given: BoardFacts) {
    let BoardFacts { product, ipc_class, enclosure, f_concern_mhz, bands, hand_assembly, by, note } = given;
    kept.product = product.or(kept.product);
    kept.ipc_class = ipc_class.or(kept.ipc_class);
    kept.enclosure = enclosure.or(kept.enclosure);
    kept.f_concern_mhz = f_concern_mhz.or(kept.f_concern_mhz);
    if bands.is_some() {
        kept.bands = bands;
    }
    kept.hand_assembly = hand_assembly.or(kept.hand_assembly);
    kept.by = by.or(kept.by);
    if !note.is_empty() {
        kept.note = note;
    }
}

/// A declared record's source: declared, or confirmed when the caller says so; never inferred
/// (that is what accepting a proposal unlooked at gives).
fn declared(source: Source, what: &str, cmd: &str) -> Result<Source> {
    match source {
        Source::Inferred => {
            Err(bad(cmd, format!("{what}: `source` inferred is for proposals accepted with accept_low_risk; give declared or confirmed")))
        }
        s => Ok(s),
    }
}

/// Who made each record kept, by key (`None`: not known).
fn authors(intent: &Intent) -> BTreeMap<String, Option<DocActor>> {
    let nets = intent.nets.iter().map(|r| (format!("net|{}", r.net), r.by));
    let parts = intent.parts.iter().map(|r| (format!("part|{}", r.part), r.by));
    let pairs = intent.pairs.iter().map(|r| (format!("pair|{}", r.name), r.by));
    let loops = intent.loops.iter().map(|r| (format!("loop|{}", r.name), r.by));
    let domains = intent.domains.iter().map(|r| (format!("domain|{}", r.name), r.by));
    let board = (!intent.board.is_empty()).then(|| ("board".to_string(), intent.board.by));
    nets.chain(parts).chain(pairs).chain(loops).chain(domains).chain(board).collect()
}

/// What an agent may not do without saying why (`note`): give a fact only the product tells
/// (a supply's current, a connector's cable, the radio bands) other than the files propose it,
/// or give a subject the engine proposes as blocking a role that isn't. The error, or `None`.
fn needs_note(r: &Record, proposed: Option<&infer::Proposal>, what: &str) -> Option<String> {
    let blocking_dropped = proposed.is_some_and(|x| x.blocking)
        && match r {
            Record::Net(n) => !n.role.blocking(),
            Record::Part(p) => !p.role.blocking(),
            _ => false,
        };
    if blocking_dropped {
        return Some(format!(
            "{what}: the engine proposes a blocking role here; an agent's record that gives it one that isn't needs a `note` saying why"
        ));
    }
    let fact = match (r, proposed.map(|x| &x.record)) {
        (Record::Net(n), Some(Record::Net(x))) => (n.current_a.is_some() && n.current_a != x.current_a).then_some("current_a"),
        (Record::Net(n), _) => n.current_a.is_some().then_some("current_a"),
        (Record::Part(p), Some(Record::Part(x))) => (p.cable.is_some() && p.cable != x.cable).then_some("cable"),
        (Record::Part(p), _) => p.cable.is_some().then_some("cable"),
        (Record::Board(b), Some(Record::Board(x))) => {
            (b.bands.is_some() && (b.bands != x.bands || proposed.is_some_and(|p| !p.needs.is_empty()))).then_some("bands")
        }
        (Record::Board(b), _) => b.bands.as_ref().is_some_and(|b| !b.is_empty()).then_some("bands"),
        _ => None,
    };
    fact.map(|f| {
        format!(
            "{what}: `{f}` is the user's to tell (the files don't); an agent gives it as the user said, with a `note` saying so (\"the user said …\")"
        )
    })
}

fn set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.intent.set";
    let p: SetParams = parse(CMD, p)?;
    for (what, n) in [
        ("nets", p.nets.as_ref().map_or(0, Vec::len)),
        ("parts", p.parts.as_ref().map_or(0, Vec::len)),
        ("pairs", p.pairs.as_ref().map_or(0, Vec::len)),
        ("loops", p.loops.as_ref().map_or(0, Vec::len)),
        ("domains", p.domains.as_ref().map_or(0, Vec::len)),
        ("accept", p.accept.as_ref().map_or(0, Vec::len)),
        ("remove", p.remove.as_ref().map_or(0, Vec::len)),
    ] {
        if n > MAX_RECORDS {
            return Err(bad(CMD, format!("`{what}` lists at most {MAX_RECORDS}")));
        }
    }
    let note = p.note.unwrap_or_default().trim().to_string();
    check_text("note", &note, MAX_DESCRIPTION).map_err(|e| bad(CMD, e))?;
    let actor = s.actor;
    let agent = actor == DocActor::Agent;
    let old = s.doc.ecad.intent.clone();
    let by = authors(&old);
    // An agent leaves the user's records as they are (as the user's approvals).
    let users = |key: &str| agent && by.get(key).is_some_and(|b| *b == Some(DocActor::User));
    let mut intent = old.clone();
    let mut changed: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    let mut gone: BTreeSet<String> = BTreeSet::new();
    for (i, key) in p.remove.iter().flatten().enumerate() {
        if !by.contains_key(key.as_str()) {
            let mut msg = format!("`remove[{i}]`: no record has the key `{key}`");
            if let Some(near) = closest(key, by.keys().map(String::as_str)) {
                msg.push_str(&format!(" (did you mean `{near}`?)"));
            }
            return Err(bad(CMD, msg));
        }
        if users(key) {
            kept.push(key.clone());
        } else if gone.insert(key.clone()) {
            removed.push(key.clone());
        }
    }
    if !gone.is_empty() {
        intent.nets.retain(|r| !gone.contains(&format!("net|{}", r.net)));
        intent.parts.retain(|r| !gone.contains(&format!("part|{}", r.part)));
        intent.pairs.retain(|r| !gone.contains(&format!("pair|{}", r.name)));
        intent.loops.retain(|r| !gone.contains(&format!("loop|{}", r.name)));
        intent.domains.retain(|r| !gone.contains(&format!("domain|{}", r.name)));
        if gone.contains("board") {
            intent.board = BoardFacts::default();
        }
    }
    let with_note = |n: &mut String| {
        if n.is_empty() {
            n.clone_from(&note);
        }
    };
    let declaring = p.nets.is_some() || p.parts.is_some() || p.board.is_some();
    let accept = p.accept.unwrap_or_default();
    let low_risk = p.accept_low_risk.unwrap_or(false);
    let inference: Option<Inference> = (declaring || !accept.is_empty() || low_risk).then(|| infer::infer(&s.doc.ecad));
    let proposal = |key: &str| inference.as_ref().and_then(|x| x.get(key));
    // An agent's record that needs a note and has none is refused.
    let check = |r: &Record, n: &str, what: &str| -> Result<()> {
        match needs_note(r, proposal(&r.key()), what) {
            Some(e) if agent && n.is_empty() => Err(bad(CMD, e)),
            _ => Ok(()),
        }
    };
    let mut lists = Lists::new(&mut intent);
    for (i, Rec(mut r)) in p.nets.into_iter().flatten().enumerate() {
        r.source = declared(r.source, &format!("`nets[{i}]`"), CMD)?;
        (r.by, r.confidence) = (Some(actor), None);
        with_note(&mut r.note);
        let key = format!("net|{}", r.net);
        if users(&key) {
            kept.push(key);
            continue;
        }
        let note = r.note.clone();
        let r = Record::Net(r);
        check(&r, &note, &format!("`nets[{i}]`"))?;
        changed.push(key);
        lists.keep(r);
    }
    for (i, Rec(mut r)) in p.parts.into_iter().flatten().enumerate() {
        r.source = declared(r.source, &format!("`parts[{i}]`"), CMD)?;
        (r.by, r.confidence) = (Some(actor), None);
        with_note(&mut r.note);
        let key = format!("part|{}", r.part);
        if users(&key) {
            kept.push(key);
            continue;
        }
        let note = r.note.clone();
        let r = Record::Part(r);
        check(&r, &note, &format!("`parts[{i}]`"))?;
        changed.push(key);
        lists.keep(r);
    }
    for (i, Rec(mut r)) in p.pairs.into_iter().flatten().enumerate() {
        r.source = declared(r.source, &format!("`pairs[{i}]`"), CMD)?;
        (r.by, r.confidence) = (Some(actor), None);
        with_note(&mut r.note);
        let key = format!("pair|{}", r.name);
        if users(&key) {
            kept.push(key);
            continue;
        }
        changed.push(key);
        lists.keep(Record::Pair(r));
    }
    for (i, Rec(mut r)) in p.loops.into_iter().flatten().enumerate() {
        r.source = declared(r.source, &format!("`loops[{i}]`"), CMD)?;
        (r.by, r.confidence) = (Some(actor), None);
        with_note(&mut r.note);
        let key = format!("loop|{}", r.name);
        if users(&key) {
            kept.push(key);
            continue;
        }
        changed.push(key);
        lists.keep(Record::Loop(r));
    }
    for (i, Rec(mut r)) in p.domains.into_iter().flatten().enumerate() {
        r.source = declared(r.source, &format!("`domains[{i}]`"), CMD)?;
        r.by = Some(actor);
        with_note(&mut r.note);
        let key = format!("domain|{}", r.name);
        if users(&key) {
            kept.push(key);
            continue;
        }
        changed.push(key);
        let k = r.name.clone();
        lists.domains.put(&k, r);
    }
    if let Some(Rec(mut b)) = p.board {
        b.by = Some(actor);
        with_note(&mut b.note);
        if users("board") {
            kept.push("board".into());
        } else {
            let note = b.note.clone();
            let r = Record::Board(b);
            check(&r, &note, "`board`")?;
            changed.push("board".into());
            lists.keep(r);
        }
    }
    if let Some(inference) = &inference {
        let mut seen = BTreeSet::new();
        for (i, key) in accept.iter().enumerate() {
            if !seen.insert(key.as_str()) {
                continue;
            }
            let Some(x) = inference.get(key) else {
                let mut msg = format!("`accept[{i}]`: no proposal has the key `{key}`");
                if let Some(near) = closest(key, inference.proposals.iter().map(|x| x.key.as_str())) {
                    msg.push_str(&format!(" (did you mean `{near}`?)"));
                } else {
                    msg.push_str(" (ecad.intent.inspect lists the proposals and their keys)");
                }
                return Err(bad(CMD, msg));
            };
            if matches!(x.record, Record::Board(_)) && !x.needs.is_empty() {
                // The bands proposed (a 2.4 GHz radio's), and the region's the user names.
                let proposed = match &x.record {
                    Record::Board(b) => b
                        .bands
                        .iter()
                        .flatten()
                        .map(|b| format!("{{name: \"{}\", lo_mhz: {}, hi_mhz: {}, rx: true}}", b.name, b.lo_mhz, b.hi_mhz))
                        .collect(),
                    _ => Vec::new(),
                };
                let mut bands = proposed;
                bands.push("{name: \"EU868\", lo_mhz: 863, hi_mhz: 870, rx: true} (or US915: 902-928)".to_string());
                return Err(bad(
                    CMD,
                    format!(
                        "`accept[{i}]`: the board's {} can't come from the files: ask the user, then give them all in `board` with a `note` (such as bands: [{}])",
                        x.needs.join(", "),
                        bands.join(", ")
                    ),
                ));
            }
            if users(key) {
                kept.push(key.clone());
                continue;
            }
            let mut r = x.record.clone().accepted(Source::Confirmed, actor);
            note_record(&mut r, &note);
            changed.push(key.clone());
            lists.keep(r);
        }
    }
    drop(lists);
    // The low-risk proposals with no record yet, those this call kept included.
    if let Some(inference) = inference.as_ref().filter(|_| low_risk) {
        let low: Vec<Record> = inference
            .low_risk(&intent)
            .into_iter()
            .map(|x| {
                let mut r = x.record.clone().accepted(Source::Inferred, actor);
                note_record(&mut r, &note);
                r
            })
            .collect();
        let mut lists = Lists::new(&mut intent);
        for r in low {
            changed.push(r.key());
            lists.keep(r);
        }
    }
    intent.validate().map_err(|e| bad(CMD, e))?;
    if intent != old {
        s.doc_mut().ecad.intent = intent;
    }
    let ecad = &s.doc.ecad;
    let cov = infer::coverage(ecad, &ecad.intent, &infer::infer(ecad));
    changed.sort();
    changed.dedup();
    reply(SetResult { changed, removed, kept, coverage: coverage_out(&cov, DEFAULT_LIMIT), param_warnings: Vec::new() })
}

/// The call's note on a record that has none of its own.
fn note_record(r: &mut Record, note: &str) {
    if note.is_empty() {
        return;
    }
    let slot = match r {
        Record::Net(x) => &mut x.note,
        Record::Part(x) => &mut x.note,
        Record::Pair(x) => &mut x.note,
        Record::Loop(x) => &mut x.note,
        Record::Board(x) => &mut x.note,
    };
    if slot.is_empty() {
        *slot = note.to_string();
    }
}

#[cfg(test)]
#[path = "ecad_intent_tests.rs"]
mod tests;
