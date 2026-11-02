//! Manufacturing outputs of an electronics design's board (design stage S4): Gerber X2 for each
//! copper, mask, paste, legend layer and the profile ([`gerber`]), Excellon drill files
//! ([`drill`]), the assembly parts and placement lists ([`jlc`]), one zip of the fab files, and a
//! check that reads the written files back ([`read`]) and compares the copper they draw with the
//! board's connections ([`verify`]).

pub mod drill;
pub mod gerber;
pub mod jlc;
pub mod read;
pub mod text;

use std::io::Write as _;

use solidcraft_doc::ecad::Electronics;
use solidcraft_doc::ecad::airwires::BoardNets;

/// Solder mask grows past each pad by this when the caller gives nothing, in millimetres.
pub const MASK_MARGIN: f64 = 0.05;

/// One output file: its name, what it holds (the X2 file function, or `bom`/`cpl`), its bytes.
#[derive(Clone, Debug)]
pub struct FabFile {
    pub name: String,
    pub function: String,
    pub bytes: Vec<u8>,
}

impl FabFile {
    pub fn text(name: String, function: String, text: String) -> Self {
        FabFile { name, function, bytes: text.into_bytes() }
    }
}

/// How the files are made.
#[derive(Clone, Debug)]
pub struct Options {
    /// Each file's name starts with it: `lora-mini.GTL`.
    pub stem: String,
    pub mask_margin: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options { stem: "board".into(), mask_margin: MASK_MARGIN }
    }
}

/// Why the board can't be written.
fn ready(ecad: &Electronics) -> Result<(), String> {
    if ecad.board.outline.is_empty() {
        return Err("the board has no outline to make the files from: ecad.board.outline sets one".into());
    }
    Ok(())
}

/// The Gerber files.
pub fn gerbers(ecad: &Electronics, nets: &BoardNets, o: &Options) -> Result<Vec<FabFile>, String> {
    ready(ecad)?;
    let glyphs = text::Glyphs::new()?;
    Ok(gerber::files(ecad, nets, o, &glyphs))
}

/// The drill files.
pub fn drills(ecad: &Electronics, nets: &BoardNets, o: &Options) -> Result<Vec<FabFile>, String> {
    ready(ecad)?;
    Ok(drill::files(ecad, nets, o))
}

/// `files` in one zip, deflated, in the order given.
pub fn zip(files: &[FabFile]) -> Result<Vec<u8>, String> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for f in files {
        z.start_file(f.name.as_str(), options).map_err(|e| format!("{}: {e}", f.name))?;
        z.write_all(&f.bytes).map_err(|e| format!("{}: {e}", f.name))?;
    }
    Ok(z.finish().map_err(|e| e.to_string())?.into_inner())
}

/// The files of a zip, by name (at most `most` of them, each at most `limit` bytes).
pub fn unzip(bytes: &[u8], most: usize, limit: u64) -> Result<Vec<FabFile>, String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if z.len() > most {
        return Err(format!("the zip holds more than {most} files"));
    }
    let mut out = Vec::new();
    for i in 0..z.len() {
        let f = z.by_index(i).map_err(|e| e.to_string())?;
        let name = f.name().map_err(|e| e.to_string())?.to_string();
        if f.size() > limit {
            return Err(format!("{name} is larger than {limit} bytes"));
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(f, limit), &mut bytes).map_err(|e| e.to_string())?;
        out.push(FabFile { name, function: String::new(), bytes });
    }
    Ok(out)
}

/// What [`verify`] found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Verified {
    /// The copper layers read back, and how many the board has.
    pub copper_layers: usize,
    pub expected_layers: usize,
    /// The pads looked at.
    pub pads: usize,
    /// Pads joined on the board that the files leave apart, and pads apart on the board that the
    /// files join, each as `R1.2 / U1.5 (net)`.
    pub opens: Vec<String>,
    pub shorts: Vec<String>,
    /// Plated and unplated holes: on the board, in the files.
    pub plated: [usize; 2],
    pub unplated: [usize; 2],
    pub problems: Vec<String>,
}

impl Verified {
    pub fn ok(&self) -> bool {
        self.opens.is_empty()
            && self.shorts.is_empty()
            && self.problems.is_empty()
            && self.copper_layers == self.expected_layers
            && self.plated[0] == self.plated[1]
            && self.unplated[0] == self.unplated[1]
    }
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while let Some(&p) = parent.get(i) {
        if p == i {
            break;
        }
        let gp = parent.get(p).copied().unwrap_or(p);
        if let Some(slot) = parent.get_mut(i) {
            *slot = gp;
        }
        i = p;
    }
    i
}

fn join(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb
        && let Some(slot) = parent.get_mut(ra.max(rb))
    {
        *slot = ra.min(rb);
    }
}

/// The most pairs of opens or shorts listed.
const MAX_LISTED: usize = 50;

/// Read `files` back and compare the copper they draw with the board: every two pads the board
/// joins must be joined by the files' copper and plated holes, and no two it keeps apart may be;
/// and the files must hold every copper layer and every hole.
pub fn verify(ecad: &Electronics, nets: &BoardNets, files: &[FabFile]) -> Verified {
    let n = usize::from(ecad.board.copper_layers);
    let mut v = Verified { expected_layers: n, ..Verified::default() };
    let mut layers: Vec<Option<solidcraft_geom::region::Region>> = vec![None; n];
    let (mut plated, mut unplated): (Vec<drill::Hole>, Vec<drill::Hole>) = (Vec::new(), Vec::new());
    for f in files {
        let Ok(text) = std::str::from_utf8(&f.bytes) else { continue };
        let lower = f.name.to_ascii_lowercase();
        if lower.ends_with(".drl") {
            match read::excellon(text) {
                Ok(h) if text.contains("NonPlated") => unplated.extend(h),
                Ok(h) => plated.extend(h),
                Err(e) => v.problems.push(format!("{}: {e}", f.name)),
            }
            continue;
        }
        if !text.contains("%TF.FileFunction,Copper,") {
            continue;
        }
        match read::gerber(text) {
            Ok(img) => {
                let index = img.function.strip_prefix("Copper,L").and_then(|r| r.split(',').next()).and_then(|i| i.parse::<usize>().ok());
                match index.and_then(|i| i.checked_sub(1)).and_then(|i| layers.get_mut(i)) {
                    Some(slot) => *slot = Some(img.region),
                    None => v.problems.push(format!("{}: copper layer `{}` the board doesn't have", f.name, img.function)),
                }
            }
            Err(e) => v.problems.push(format!("{}: {e}", f.name)),
        }
    }
    v.copper_layers = layers.iter().filter(|l| l.is_some()).count();
    let (board_plated, board_unplated) = drill::holes(ecad, nets);
    v.plated = [board_plated.len(), plated.len()];
    v.unplated = [board_unplated.len(), unplated.len()];
    // Islands of each layer as nodes, joined by the plated holes.
    let shapes: Vec<Vec<solidcraft_geom::region::Region>> = layers.iter().map(|l| l.as_ref().map(|r| r.split()).unwrap_or_default()).collect();
    let mut base = Vec::with_capacity(n);
    let mut total = 0usize;
    for s in &shapes {
        base.push(total);
        total += s.len();
    }
    let mut parent: Vec<usize> = (0..total).collect();
    let locate = |layer: usize, p: [f64; 2]| -> Option<usize> {
        let r = layers.get(layer)?.as_ref()?;
        let shape = r.locate(&[p]).first().copied().flatten()?;
        Some(base.get(layer)? + shape)
    };
    for h in &plated {
        let at: Vec<usize> = (0..n).filter_map(|l| locate(l, h.from)).collect();
        for w in at.windows(2) {
            if let [a, b] = w {
                join(&mut parent, *a, *b);
            }
        }
    }
    let stack = ecad.board.stack();
    let mut node: Vec<Option<usize>> = Vec::with_capacity(nets.pads.len());
    for p in &nets.pads {
        let at = p.at();
        let mut first = None;
        for (l, layer) in stack.iter().enumerate() {
            if !p.on(*layer) {
                continue;
            }
            if let Some(k) = locate(l, at) {
                match first {
                    Some(f) => join(&mut parent, f, k),
                    None => first = Some(k),
                }
            }
        }
        node.push(first);
    }
    let mut seen: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    let mut by_component: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    let name = |k: usize| -> String {
        let Some(p) = nets.pads.get(k) else { return String::new() };
        let net = p.net.and_then(|i| nets.names.get(i)).map_or("no net", String::as_str);
        format!("{} ({net})", p.reference())
    };
    for (k, p) in nets.pads.iter().enumerate() {
        if !(p.top || p.bottom) {
            continue;
        }
        v.pads += 1;
        let island = nets.pad_islands.get(k).copied();
        let comp = node.get(k).copied().flatten().map(|c| find(&mut parent, c));
        let Some(comp) = comp else {
            if v.opens.len() < MAX_LISTED {
                v.opens.push(format!("{}: no copper under it in the files", name(k)));
            }
            continue;
        };
        {
            let island = island.unwrap_or(usize::MAX - k);
            match seen.get(&island) {
                Some(&other) if node.get(other).copied().flatten().map(|c| find(&mut parent, c)) != Some(comp) => {
                    if v.opens.len() < MAX_LISTED {
                        v.opens.push(format!("{} / {}", name(other), name(k)));
                    }
                }
                Some(_) => {}
                None => {
                    seen.insert(island, k);
                }
            }
        }
        match by_component.get(&comp) {
            Some(&other) if nets.pad_islands.get(other).copied() != island => {
                if v.shorts.len() < MAX_LISTED {
                    v.shorts.push(format!("{} / {}", name(other), name(k)));
                }
            }
            Some(_) => {}
            None => {
                by_component.insert(comp, k);
            }
        }
    }
    v
}
