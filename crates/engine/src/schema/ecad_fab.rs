//! Contracts and result types for the commands in `cmd/ecad_fab.rs`: the board's manufacturing
//! files and the check of them.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `ecad.board.export.gerber` and `ecad.board.export.drill`: write the files into a folder.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct FabExportParams {
    /// A folder; it is made when missing.
    pub path: String,
    /// What each file's name starts with; the design's name when left out.
    pub stem: Option<String>,
}

/// `ecad.board.export.jlc`: the fab zip, the parts list and the placement list.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct JlcExportParams {
    /// The zip (`out/board.zip`); the parts and placement lists go beside it
    /// (`out/board-bom.csv`, `out/board-cpl.csv`).
    pub path: String,
    pub stem: Option<String>,
    /// Rotation offsets by library footprint name, in degrees, for this export only, in place of
    /// the library footprints' own.
    pub rotations: Option<BTreeMap<String, f64>>,
}

/// `ecad.board.verify_outputs`: read the files back and compare them with the board.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct VerifyOutputsParams {
    /// A zip or a folder of the files; left out, they are made in memory as an export would.
    pub path: Option<String>,
}

/// `ecad.board.cpl_rotation`: set or clear a library footprint's rotation offset for the
/// placement list.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CplRotationParams {
    /// The library footprint's name.
    pub footprint: String,
    /// Degrees added to each placed copy's rotation, -360 to 360 (kept as -180 to 180); left
    /// out or 0 clears it. Kept with the library footprint, so it travels with it.
    pub offset: Option<f64>,
}

/// One written file.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FabFileOut {
    pub path: String,
    /// The Gerber X2 file function (`Copper,L1,Top`), `Plated,1,2,PTH` for a drill file, or
    /// `bom`, `cpl`, `zip`.
    pub function: String,
    pub bytes: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FabExportResult {
    pub files: Vec<FabFileOut>,
}

/// What reading the files back found.
#[derive(Debug, Serialize, JsonSchema)]
pub struct VerifyOutputsResult {
    /// Every check passed.
    pub ok: bool,
    pub copper_layers: usize,
    pub expected_layers: usize,
    pub pads: usize,
    /// Pads the board joins that the files leave apart (`R1.2 (GND) / U1.5 (GND)`), at most 50.
    pub opens: Vec<String>,
    /// Pads the board keeps apart that the files join, at most 50.
    pub shorts: Vec<String>,
    /// Holes on the board and in the files.
    pub plated: [usize; 2],
    pub unplated: [usize; 2],
    pub problems: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct JlcExportResult {
    pub files: Vec<FabFileOut>,
    pub bom_rows: usize,
    pub cpl_rows: usize,
    /// Designators whose component has no LCSC part number.
    pub missing_lcsc: Vec<String>,
    pub verified: VerifyOutputsResult,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CplRotationResult {
    /// Every library footprint's offset after the change, by name (those with none left out).
    pub offsets: BTreeMap<String, f64>,
}
