//! Contracts and result types for the command in `cmd/ecad_board_marking.rs`: a board marking.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ecad_board::BoardResult;
use super::ecad_component::Side;

/// What a board marking holds; each left out takes the usual (title, revision and date shown, the
/// rest off, 1.5 mm text). The app keeps one in its preferences and passes it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct MarkingTemplateParam {
    pub title: Option<bool>,
    pub revision: Option<bool>,
    pub date: Option<bool>,
    /// A pattern such as `MM-{project}-{number}` (`{project}`/`{title}`, `{number}`, `{revision}`,
    /// `{date}`, `{company}`, `{author}`); at most 64 characters.
    pub part_number: Option<String>,
    /// An empty box to write a serial number in.
    pub serial_box: Option<bool>,
    /// A QR code of the part number and revision.
    pub qr: Option<bool>,
    /// A short "made by" text.
    pub made_by: Option<String>,
    /// The text the fab replaces with its order number, as its order form asks.
    pub order_placeholder: Option<String>,
    /// Text height in millimetres, 0.5 to 5.
    pub height: Option<f64>,
}

/// `ecad.board.marking`: stamp the board's identity on its silkscreen, in one step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct MarkingParams {
    pub template: Option<MarkingTemplateParam>,
    pub side: Side,
    /// The block's top left, in millimetres.
    pub at: [f64; 2],
    /// The part number's `{number}`.
    pub number: Option<String>,
    /// A mark in copper on each layer ("TOP"/"BOT", or "1".."n"), each in an area pours keep out of.
    pub layer_marks: Option<bool>,
    /// Work it out and say what it would make, changing nothing.
    pub dry_run: Option<bool>,
}

/// What `ecad.board.marking` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct MarkingResult {
    /// The marking's id (0 on a dry run).
    pub marking: u64,
    /// The lines the block prints, top first.
    pub lines: Vec<String>,
    pub part_number: String,
    /// How many drawings and pour keepouts it makes.
    pub graphics: usize,
    pub keepouts: usize,
    /// The box round what it draws, in millimetres.
    pub bbox: [[f64; 2]; 2],
    /// The design rule check's warnings on what it draws (silkscreen over a pad).
    pub warnings: Vec<String>,
    #[serde(flatten)]
    pub board: BoardResult,
}
