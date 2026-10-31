//! Contracts and result types for the commands in `cmd/ecad_frame.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ecad_schematic::nullable;

/// A frame: by its id, or by its title (exactly).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum FrameRef {
    Id(u64),
    Title(String),
}

impl<'de> Deserialize<'de> for FrameRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::String(s) => Ok(FrameRef::Title(s)),
            v => v.as_u64().map(FrameRef::Id).ok_or_else(|| serde::de::Error::custom("must be a frame's id or its title")),
        }
    }
}

/// `ecad.schematic.frame`: a new section frame.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FrameParams {
    /// Such as `Power` or `LoRa radio`; 1 to 64 characters.
    pub title: String,
    /// Two opposite corners, in sheet millimetres.
    pub rect: [[f64; 2]; 2],
    /// Print order (lower first; also which frame wins where frames overlap). Left out: after
    /// the frames that have one, by id.
    pub order: Option<u32>,
}

/// `ecad.schematic.frame.edit`: change a frame.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FrameEditParams {
    pub frame: FrameRef,
    pub title: Option<String>,
    pub rect: Option<[[f64; 2]; 2]>,
    /// `null` takes the order away.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<u32>")]
    pub order: Option<Option<u32>>,
}

/// A frame as replies give it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct FrameOut {
    pub id: u64,
    pub title: String,
    /// Lower left and upper right, in sheet millimetres.
    pub rect: [[f64; 2]; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
    /// The parts that belong to it (their symbol's middle is inside it, and no frame before it
    /// in print order holds it), by designator.
    pub parts: Vec<String>,
}

/// What the frame commands return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FrameResult {
    /// The frame made or changed.
    pub frame: FrameOut,
    /// Every frame, in print order.
    pub frames: Vec<FrameOut>,
    /// Parts in no frame, by designator.
    pub outside: Vec<String>,
    /// Pairs of frames that overlap, by title, and which one the parts in both belong to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlaps: Vec<String>,
    /// In the warn parameter-check mode: what the call's parameters got wrong.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
