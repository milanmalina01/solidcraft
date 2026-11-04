//! The board's manufacturing files (`solidcraft_io::fab`): Gerber X2 and Excellon drill files,
//! one zip for the fab with the assembly parts and placement lists beside it, the check that
//! reads them back and compares them with the board, and the placement list's rotation offsets.
//! Writing files changes nothing in the design.

use std::collections::BTreeMap;

use serde_json::Value;
use solidcraft_io::fab::{self, FabFile, Options, Verified};

use super::CommandSpec;
use super::ecad::{load_footprint, parse, reply, store_footprint};

use crate::params::bad;
use crate::schema::ecad_component::ComponentRef;
use crate::schema::ecad_fab::{
    CplRotationParams, CplRotationResult, FabExportParams, FabExportResult, FabFileOut, JlcExportParams, JlcExportResult, VerifyOutputsParams,
    VerifyOutputsResult,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.board.export.gerber", "Export Gerber", crate::checked!("ecad.board.export.gerber", export_gerber))
        .icon("export")
        .noundo()
        .doc_only()
        .contract(&EXPORT_CONTRACT)
        .output(output_schema_for::<FabExportResult>)
        .params(
            "path (a folder, made when missing), stem? (each file's name starts with it; the design's name) → {files: [{path, function, bytes}]}: Gerber X2 (RS-274X with file attributes, 4.6 mm) for each copper layer (.GTL, .G1…, .GBL), solder mask (.GTS/.GBS, negative: the pads grown 0.05 mm, open vias), paste (.GTP/.GBP), legend (.GTO/.GBO: footprints' silkscreen, the board's drawings, the names shown, text in the built-in font) and the profile (.GKO). Refused without an outline. Changes nothing in the design",
        ),
    CommandSpec::new("ecad.board.export.drill", "Export Drill", crate::checked!("ecad.board.export.drill", export_drill))
        .icon("export")
        .noundo()
        .doc_only()
        .contract(&EXPORT_CONTRACT)
        .output(output_schema_for::<FabExportResult>)
        .params(
            "path (a folder), stem? → {files}: Excellon, metric with decimal points: <stem>-PTH.drl (vias and plated pad holes) and, when the board has any, <stem>-NPTH.drl (mounting holes, unplated pads); a slot as G85. Changes nothing in the design",
        ),
    CommandSpec::new("ecad.board.export.jlc", "Export for JLCPCB", crate::checked!("ecad.board.export.jlc", export_jlc))
        .icon("export")
        .noundo()
        .doc_only()
        .contract(&JLC_CONTRACT)
        .output(output_schema_for::<JlcExportResult>)
        .params(
            "path (the .zip), stem?, rotations? {footprint: degrees} (in place of the library footprints' own, this export only) → {files, bom_rows, cpl_rows, missing_lcsc, verified}: the Gerber and drill files in one zip for the fab's upload page, and beside it <zip stem>-bom.csv (Comment, Designator, Footprint, LCSC Part #: one row per value, footprint and LCSC part) and <zip stem>-cpl.csv (Designator, Mid X, Mid Y, Layer, Rotation: each part at the middle of its footprint's pads, its side, its rotation plus its library footprint's offset from ecad.board.cpl_rotation; parts marked do-not-populate are in neither list). The files are read back and checked as ecad.board.verify_outputs does. Changes nothing in the design",
        ),
    CommandSpec::new("ecad.board.verify_outputs", "Verify Outputs", crate::checked!("ecad.board.verify_outputs", verify_outputs))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&VERIFY_CONTRACT)
        .output(output_schema_for::<VerifyOutputsResult>)
        .params(
            "path? (the .zip ecad.board.export.jlc wrote; left out, the files are made in memory) → {ok, copper_layers, expected_layers, pads, opens, shorts, plated: [board, files], unplated, problems}: the Gerber copper and the drill files read back; every two pads the board joins must be joined by the files' copper and plated holes, no two it keeps apart may be, and every layer and hole must be there. Read-only",
        ),
    CommandSpec::new("ecad.board.cpl_rotation", "Placement Rotation", crate::checked!("ecad.board.cpl_rotation", cpl_rotation))
        .icon("spin")
        .doc_only()
        .contract(&ROTATION_CONTRACT)
        .output(output_schema_for::<CplRotationResult>)
        .params(
            "footprint (a library footprint's name), offset? (degrees, -360 to 360, kept as -180 to 180; left out or 0 clears it) → {offsets}: the placement list's rotation offset for every placed copy of the footprint, kept with the library footprint so it travels with it (for a footprint drawn turned from the assembler's reference for the part). One undo step",
        ),
];

const EXPORT_CONTRACT: TypedContract<FabExportParams> = TypedContract::new();
const JLC_CONTRACT: TypedContract<JlcExportParams> = TypedContract::new();
const VERIFY_CONTRACT: TypedContract<VerifyOutputsParams> = TypedContract::new();
const ROTATION_CONTRACT: TypedContract<CplRotationParams> = TypedContract::new();

/// The longest path accepted.
const MAX_PATH: usize = 4096;
/// The most files, and bytes per file, a zip read back may hold.
const MAX_ZIP_FILES: usize = 64;
const MAX_FILE_BYTES: u64 = 256 << 20;
/// The most rotation offsets a board keeps.
const MAX_OFFSETS: usize = 4096;

fn path_of(cmd: &str, path: &str) -> Result<String> {
    let path = path.trim().to_string();
    if path.is_empty() || path.chars().count() > MAX_PATH || path.contains('\0') {
        return Err(bad(cmd, format!("`path` must be a path of 1 to {MAX_PATH} characters")));
    }
    Ok(path)
}

/// The design's name as a file name stem.
fn stem_of(s: &Session, given: Option<String>, cmd: &str) -> Result<String> {
    let raw = given.unwrap_or_else(|| s.doc.name.clone());
    if raw.chars().count() > 64 {
        return Err(bad(cmd, "`stem` must be at most 64 characters"));
    }
    let out: String = raw.trim().chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    Ok(if out.trim_matches('-').is_empty() { "board".into() } else { out })
}

fn write(path: &str, bytes: &[u8]) -> Result<u64> {
    solidcraft_io::vfs::write(path, bytes).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    Ok(bytes.len() as u64)
}

fn write_all(folder: &str, files: &[FabFile]) -> Result<Vec<FabFileOut>> {
    solidcraft_io::vfs::create_dir_all(folder).map_err(|e| EngineError::Other(format!("{folder}: {e}")))?;
    let base = folder.trim_end_matches(['/', '\\']);
    files
        .iter()
        .map(|f| {
            let path = format!("{base}/{}", f.name);
            let bytes = write(&path, &f.bytes)?;
            Ok(FabFileOut { path, function: f.function.clone(), bytes })
        })
        .collect()
}

fn fab_files(s: &Session, stem: String, cmd: &str) -> Result<Vec<FabFile>> {
    let nets = s.board_nets();
    let o = Options { stem, ..Options::default() };
    let mut files = fab::gerbers(&s.doc.ecad, &nets, &o).map_err(|e| bad(cmd, e))?;
    files.extend(fab::drills(&s.doc.ecad, &nets, &o).map_err(|e| bad(cmd, e))?);
    Ok(files)
}

fn export_gerber(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.export.gerber";
    let p: FabExportParams = parse(CMD, p)?;
    let path = path_of(CMD, &p.path)?;
    let stem = stem_of(s, p.stem, CMD)?;
    let nets = s.board_nets();
    let files = fab::gerbers(&s.doc.ecad, &nets, &Options { stem, ..Options::default() }).map_err(|e| bad(CMD, e))?;
    reply(FabExportResult { files: write_all(&path, &files)? })
}

fn export_drill(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.export.drill";
    let p: FabExportParams = parse(CMD, p)?;
    let path = path_of(CMD, &p.path)?;
    let stem = stem_of(s, p.stem, CMD)?;
    let nets = s.board_nets();
    let files = fab::drills(&s.doc.ecad, &nets, &Options { stem, ..Options::default() }).map_err(|e| bad(CMD, e))?;
    reply(FabExportResult { files: write_all(&path, &files)? })
}

fn verified_out(v: &Verified) -> VerifyOutputsResult {
    VerifyOutputsResult {
        ok: v.ok(),
        copper_layers: v.copper_layers,
        expected_layers: v.expected_layers,
        pads: v.pads,
        opens: v.opens.clone(),
        shorts: v.shorts.clone(),
        plated: v.plated,
        unplated: v.unplated,
        problems: v.problems.clone(),
    }
}

fn check_offsets(cmd: &str, offsets: &BTreeMap<String, f64>) -> Result<()> {
    if offsets.len() > MAX_OFFSETS {
        return Err(bad(cmd, format!("at most {MAX_OFFSETS} rotation offsets")));
    }
    for (k, v) in offsets {
        if !(v.is_finite() && (-360.0..=360.0).contains(v)) || k.chars().count() > 128 {
            return Err(bad(cmd, format!("the rotation offset of `{k}` must be -360 to 360 degrees")));
        }
    }
    Ok(())
}

fn export_jlc(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.export.jlc";
    let p: JlcExportParams = parse(CMD, p)?;
    let path = path_of(CMD, &p.path)?;
    let lower = path.to_ascii_lowercase();
    let Some(base) = lower.strip_suffix(".zip").map(|b| path.get(..b.len()).unwrap_or(b).to_string()) else {
        return Err(bad(CMD, format!("`path` must name a .zip file (got `{path}`)")));
    };
    let offsets = p.rotations.unwrap_or_default();
    check_offsets(CMD, &offsets)?;
    if s.doc.ecad.board.footprints.is_empty() {
        return Err(bad(CMD, "the board has no footprints to assemble"));
    }
    let stem = stem_of(s, p.stem, CMD)?;
    let files = fab_files(s, stem, CMD)?;
    let zip = fab::zip(&files).map_err(EngineError::Other)?;
    let bom = fab::jlc::bom(&s.doc.ecad);
    let cpl = fab::jlc::cpl(&s.doc.ecad, &offsets);
    let missing_lcsc: Vec<String> = bom.iter().filter(|r| r.lcsc.trim().is_empty()).flat_map(|r| r.designators.clone()).collect();
    if let Some(dir) = path.rfind(['/', '\\']).and_then(|i| path.get(..i)).filter(|d| !d.is_empty()) {
        solidcraft_io::vfs::create_dir_all(dir).map_err(|e| EngineError::Other(format!("{dir}: {e}")))?;
    }
    let (bom_path, cpl_path) = (format!("{base}-bom.csv"), format!("{base}-cpl.csv"));
    let out = vec![
        FabFileOut { bytes: write(&path, &zip)?, path: path.clone(), function: "zip".into() },
        FabFileOut { bytes: write(&bom_path, fab::jlc::bom_csv(&bom).as_bytes())?, path: bom_path, function: "bom".into() },
        FabFileOut { bytes: write(&cpl_path, fab::jlc::cpl_csv(&cpl).as_bytes())?, path: cpl_path, function: "cpl".into() },
    ];
    let nets = s.board_nets();
    let verified = fab::verify(&s.doc.ecad, &nets, &files);
    reply(JlcExportResult { files: out, bom_rows: bom.len(), cpl_rows: cpl.len(), missing_lcsc, verified: verified_out(&verified) })
}

fn verify_outputs(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.verify_outputs";
    let p: VerifyOutputsParams = parse(CMD, p)?;
    let files = match p.path {
        Some(path) => {
            let path = path_of(CMD, &path)?;
            if !path.to_ascii_lowercase().ends_with(".zip") {
                return Err(bad(CMD, "`path` must be the .zip ecad.board.export.jlc wrote; leave it out to check files made in memory"));
            }
            let bytes = solidcraft_io::vfs::read(&path, (MAX_FILE_BYTES * 4) as usize).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
            fab::unzip(&bytes, MAX_ZIP_FILES, MAX_FILE_BYTES).map_err(|e| bad(CMD, format!("{path}: {e}")))?
        }
        None => fab_files(s, "board".into(), CMD)?,
    };
    let nets = s.board_nets();
    reply(verified_out(&fab::verify(&s.doc.ecad, &nets, &files)))
}

fn cpl_rotation(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.board.cpl_rotation";
    let p: CplRotationParams = parse(CMD, p)?;
    let offset = p.offset.unwrap_or(0.0);
    let mut one = BTreeMap::new();
    one.insert(p.footprint.trim().to_string(), offset);
    check_offsets(CMD, &one)?;
    let mut fp = load_footprint(s, &ComponentRef::Name(p.footprint.trim().to_string()), CMD)?;
    fp.assembly_rotation = offset.rem_euclid(360.0);
    if fp.assembly_rotation > 180.0 {
        fp.assembly_rotation -= 360.0;
    }
    store_footprint(s, fp, &[], false, CMD)?;
    let offsets = s.doc.ecad.footprints.iter().filter(|f| f.assembly_rotation != 0.0).map(|f| (f.name.clone(), f.assembly_rotation)).collect();
    reply(CplRotationResult { offsets })
}
