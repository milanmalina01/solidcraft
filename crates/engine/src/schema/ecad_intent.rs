//! Contracts and result types for the commands in `cmd/ecad_intent.rs`. The records are the
//! design's own (`solidcraft_doc::ecad::intent`), read and written as they are saved; their
//! schemas are written out here.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solidcraft_doc::ecad::intent::{BoardFacts, Domain, LoopIntent, NetIntent, PairIntent, PartIntent};

/// The net roles, part roles, interfaces and loop kinds, as the records take them.
const NET_ROLES: &[&str] = &[
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
];
const PART_ROLES: &[&str] = &[
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
    "sensitive",
    "mechanical",
];
const INTERFACES: &[&str] = &["usb2_hs", "usb2_fs", "qspi", "spi", "i2c", "uart", "rs485", "swd", "jtag", "can", "i2s", "sdio"];
const LOOP_KINDS: &[&str] = &["buck_hot", "boost_hot", "gate", "decap", "crystal", "tvs", "sense"];
const SOURCES: &[&str] = &["declared", "inferred", "confirmed"];

fn names(list: &[&str]) -> Value {
    json!({"type": "string", "enum": list})
}

fn number(about: &str) -> Value {
    json!({"type": "number", "description": about})
}

fn text(about: &str) -> Value {
    json!({"type": "string", "description": about})
}

fn texts(about: &str) -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": about})
}

/// The fields every net, pair, part and loop record has besides its own.
fn common(props: &mut serde_json::Map<String, Value>) {
    props.insert("source".into(), json!({"type": "string", "enum": SOURCES, "description": "declared when left out"}));
    props.insert("by".into(), json!({"type": "string", "enum": ["user", "agent"], "description": "who made it; set by the engine"}));
    props.insert("confidence".into(), json!({"type": "string", "enum": ["high", "medium", "low"]}));
    props.insert("evidence".into(), texts("what it rests on (a datasheet section, a pin name), at most 8"));
    props.insert("note".into(), text("at most 256 characters"));
}

fn object(about: &str, required: &[&str], props: Value) -> Schema {
    let mut map = match props {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    common(&mut map);
    json_schema!({"type": "object", "description": about, "additionalProperties": false, "required": required, "properties": map})
}

/// A record read or written as the design saves it, with its schema written out.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Rec<T>(pub T);

impl JsonSchema for Rec<NetIntent> {
    fn schema_name() -> Cow<'static, str> {
        "NetIntent".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        object(
            "What a net is for.",
            &["net", "role"],
            json!({
                "net": text("the net's name"),
                "role": names(NET_ROLES),
                "current_a": number("the most current it carries, in A"),
                "voltage": {"type": "object", "additionalProperties": false, "required": ["nominal"], "properties": {"nominal": number("V"), "peak": number("V")}},
                "edge_ns": number("its edges' rise time, in ns"),
                "driver": names(&["standard", "high", "lvds", "serdes", "clock"]),
                "z_target": number("the impedance it is routed for, in ohms"),
                "z_tol": number("the tolerance, as a share (0.1 for 10 %)"),
                "interface": names(INTERFACES),
                "f_mhz": number("its frequency of concern, in MHz"),
                "external": {"type": "boolean", "description": "it leaves the board"},
                "domain": text("a domain's name, or the IC whose node it is"),
            }),
        )
    }
}

impl JsonSchema for Rec<PartIntent> {
    fn schema_name() -> Cow<'static, str> {
        "PartIntent".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        object(
            "What a part is for.",
            &["part", "role"],
            json!({
                "part": text("its designator"),
                "role": names(PART_ROLES),
                "of": texts("what it serves: pins (U1.VDD by name or U1.B1 by number), parts or nets"),
                "cable": {"type": "boolean", "description": "a connector: a cable plugs into it in the product"},
                "power_w": number("the power it dissipates, in W"),
                "loop": text("the loop it is a member of"),
            }),
        )
    }
}

impl JsonSchema for Rec<PairIntent> {
    fn schema_name() -> Cow<'static, str> {
        "PairIntent".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        object(
            "A differential pair.",
            &["name", "p", "n"],
            json!({
                "name": text("the pair's name"),
                "p": text("the positive net"),
                "n": text("the negative net"),
                "interface": names(INTERFACES),
                "skew_mm": number("the most length difference, in mm"),
            }),
        )
    }
}

impl JsonSchema for Rec<LoopIntent> {
    fn schema_name() -> Cow<'static, str> {
        "LoopIntent".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        object(
            "A current loop to keep small.",
            &["name", "kind", "members"],
            json!({
                "name": text("the loop's name"),
                "kind": names(LOOP_KINDS),
                "members": texts("its pins, such as C3.1 and U2.VIN (2 to 64)"),
                "reference_ratio": number("how its area may compare with a reference layout's"),
            }),
        )
    }
}

impl JsonSchema for Rec<Domain> {
    fn schema_name() -> Cow<'static, str> {
        "Domain".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "description": "A group of parts that share a kind of circuit.",
            "additionalProperties": false,
            "required": ["name", "kind"],
            "properties": {
                "name": text("its name"),
                "kind": names(&["analog", "digital", "power", "rf", "isolated"]),
                "parts": texts("designators"),
                "source": {"type": "string", "enum": SOURCES},
                "by": {"type": "string", "enum": ["user", "agent"]},
                "note": text("at most 256 characters"),
            }
        })
    }
}

impl JsonSchema for Rec<BoardFacts> {
    fn schema_name() -> Cow<'static, str> {
        "BoardFacts".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "description": "Facts about the whole board; each left out until known.",
            "additionalProperties": false,
            "properties": {
                "product": names(&["prototype", "production"]),
                "ipc_class": {"type": "integer", "enum": [2, 3]},
                "enclosure": names(&["none", "plastic", "metal"]),
                "f_concern_mhz": number("the highest frequency of concern for emissions, in MHz"),
                "bands": {
                    "type": "array",
                    "description": "the radio bands it works in; [] for none",
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["name", "lo_mhz", "hi_mhz"],
                        "properties": {"name": text("such as EU868"), "lo_mhz": number("MHz"), "hi_mhz": number("MHz"), "rx": {"type": "boolean"}}
                    }
                },
                "hand_assembly": {"type": "boolean"},
                "by": {"type": "string", "enum": ["user", "agent"]},
                "note": text("at most 256 characters"),
            }
        })
    }
}

/// `ecad.intent.inspect`: what is declared, what the engine proposes, and what blocks the
/// checks. Read-only.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct InspectParams {
    /// List the engine's proposals for what has no record yet; left out, true.
    pub propose: Option<bool>,
    /// Only records and proposals of these nets (by name) or parts (by designator).
    pub nets: Option<Vec<String>>,
    pub parts: Option<Vec<String>>,
    /// Only these roles (`power`, `decoupling`, `pair`, `buck_hot`…).
    pub roles: Option<Vec<String>>,
    /// Only proposals that need a look (blocking ones); left out, false.
    pub blocking: Option<bool>,
    /// The most records, proposals and missing items to list, each (25 when left out, at most
    /// 4096), after `offset` of them.
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// A blocking role still to settle.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MissingOut {
    /// `net|+3V3`, `part|C5`, `pair|USB`, `board`: what `ecad.intent.set {accept}` takes.
    pub key: String,
    pub role: String,
    /// `confirm` (a look at the proposal and its evidence), or the fields still to give
    /// (`current_a`, `voltage`, `cable`, `bands`).
    pub needs: Vec<String>,
    pub why: String,
}

/// How much of the blocking intent is settled.
#[derive(Debug, Serialize, JsonSchema)]
pub struct BlockingOut {
    pub total: usize,
    pub confirmed: usize,
    /// At most `limit`; `missing_more` counts the rest.
    pub missing: Vec<MissingOut>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub missing_more: usize,
}

/// The ICs' supply pins: how many, how many a kept decoupling or bulk record names, and how many
/// a proposal names.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PowerPinsOut {
    pub total: usize,
    pub assigned: usize,
    pub proposed: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CoverageOut {
    pub blocking: BlockingOut,
    pub ic_power_pins: PowerPinsOut,
    /// Records naming nets or parts the schematic doesn't have (`net|X`, `part|R9`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<String>,
}

/// What a proposal is about.
#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    Net,
    Part,
    Pair,
    Loop,
    Board,
}

/// One of the engine's proposals.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProposalOut {
    /// What `ecad.intent.set {accept: [key]}` takes.
    pub key: String,
    pub kind: ProposalKind,
    pub role: String,
    /// It needs a look before it is accepted (`accept_low_risk` leaves it).
    pub blocking: bool,
    pub confidence: String,
    /// What the files can't tell: asked of the user.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    pub evidence: Vec<String>,
    /// The record as it would be kept.
    pub record: Value,
}

/// How many were left out past `limit`.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct MoreOut {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub records: usize,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub proposals: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// What `ecad.intent.inspect` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct InspectResult {
    pub coverage: CoverageOut,
    /// The board's facts, when any are set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub board: Option<Rec<BoardFacts>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nets: Vec<Rec<NetIntent>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<Rec<PairIntent>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Rec<PartIntent>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loops: Vec<Rec<LoopIntent>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<Rec<Domain>>,
    /// Blocking ones first, then by kind (nets, pairs, parts, loops).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<ProposalOut>,
    pub more: MoreOut,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// `ecad.intent.set`: declare records, accept proposals, take records away; one undo step, all
/// or none.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SetParams {
    /// Records to keep as given (`declared` unless `source` says `confirmed`), each in place of
    /// the one for its net, part, pair, loop or domain.
    pub nets: Option<Vec<Rec<NetIntent>>>,
    pub parts: Option<Vec<Rec<PartIntent>>>,
    pub pairs: Option<Vec<Rec<PairIntent>>>,
    pub loops: Option<Vec<Rec<LoopIntent>>>,
    pub domains: Option<Vec<Rec<Domain>>>,
    /// Board facts to set (those given replace the ones kept).
    pub board: Option<Rec<BoardFacts>>,
    /// Proposals to keep as proposed, confirmed (keys as `ecad.intent.inspect` gives them).
    pub accept: Option<Vec<String>>,
    /// Keep every proposal that isn't blocking and has no record yet, as `inferred`.
    pub accept_low_risk: Option<bool>,
    /// Records to take away, by key (`net|X`, `part|C5`, `pair|USB`, `loop|U3.buck_hot`,
    /// `domain|RF`, `board`).
    pub remove: Option<Vec<String>>,
    /// Why (kept on records accepted or declared without a note of their own).
    pub note: Option<String>,
}

/// What `ecad.intent.set` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct SetResult {
    /// Keys of the records made or changed.
    pub changed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// The user's records an agent's call named, left as they are (an agent changes or removes
    /// only its own and the engine's).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<String>,
    pub coverage: CoverageOut,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
