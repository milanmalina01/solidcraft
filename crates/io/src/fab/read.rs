//! Readers for what the writers here make, so the files can be checked against the board: the
//! part of Gerber they write (4.6 millimetre coordinates, round apertures, regions, linear
//! strokes, flashes, both polarities) and Excellon with decimal points and `G85` slots. Anything
//! else is refused, never guessed at.

use solidcraft_doc::ecad::copper::{hull_outline, stroke};
use solidcraft_geom::region::Region;

use super::drill::Hole;

/// The most objects one file may hold.
pub const MAX_OBJECTS: usize = 1 << 20;

/// What a Gerber file draws: its file function and the image.
pub struct Image {
    pub function: String,
    pub region: Region,
    pub objects: usize,
}

fn number(s: &str) -> Result<f64, String> {
    s.parse::<i64>().map(|v| v as f64 / 1e6).map_err(|_| format!("`{s}` is not a coordinate"))
}

/// The coordinate value after `key` in a statement such as `X100Y-200D01`.
fn field(s: &str, key: char) -> Option<&str> {
    let i = s.find(key)?;
    let rest = s.get(i + 1..)?;
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '+' || c == '.')).unwrap_or(rest.len());
    rest.get(..end)
}

/// Read a Gerber file this module wrote.
pub fn gerber(text: &str) -> Result<Image, String> {
    let mut function = String::new();
    let mut apertures: std::collections::BTreeMap<u32, f64> = std::collections::BTreeMap::new();
    let (mut current, mut at, mut dark, mut in_region) = (None::<u32>, [0.0, 0.0], true, false);
    let mut contour: Vec<[f64; 2]> = Vec::new();
    let mut batch: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut image = Region::new();
    let mut objects = 0usize;
    let flush = |image: &mut Region, batch: &mut Vec<Vec<[f64; 2]>>, dark: bool| {
        if batch.is_empty() {
            return;
        }
        let r = Region::polygons(batch.iter().map(Vec::as_slice));
        *image = if dark { image.union(&r) } else { image.difference(&r) };
        batch.clear();
    };
    let mut saw_format = false;
    for raw in text.split('*') {
        let s = raw.trim_matches(|c: char| c == '%' || c.is_whitespace());
        if s.is_empty() {
            continue;
        }
        if let Some(f) = s.strip_prefix("TF.FileFunction,") {
            function = f.to_string();
        } else if s.starts_with("TF.") || s.starts_with("G04") || s == "G01" || s == "MOMM" {
        } else if s == "FSLAX46Y46" {
            saw_format = true;
        } else if s.starts_with("FS") || s.starts_with("MOIN") {
            return Err(format!("unsupported format statement `{s}`"));
        } else if let Some(def) = s.strip_prefix("ADD") {
            let digits: String = def.chars().take_while(char::is_ascii_digit).collect();
            let d: u32 = digits.parse().map_err(|_| format!("bad aperture `{s}`"))?;
            let shape = def.get(digits.len()..).unwrap_or("");
            let size = shape.strip_prefix("C,").ok_or_else(|| format!("only round apertures are read: `{s}`"))?;
            let size: f64 = size.parse().map_err(|_| format!("bad aperture size `{s}`"))?;
            apertures.insert(d, size);
        } else if s == "LPD" || s == "LPC" {
            let want = s == "LPD";
            if want != dark {
                flush(&mut image, &mut batch, dark);
                dark = want;
            }
        } else if s == "G36" {
            in_region = true;
            contour.clear();
        } else if s == "G37" {
            if contour.len() >= 3 {
                batch.push(std::mem::take(&mut contour));
            }
            in_region = false;
        } else if s == "M02" {
            break;
        } else if let Some(d) = s.strip_prefix('D').and_then(|d| d.parse::<u32>().ok()).filter(|d| *d >= 10) {
            current = Some(d);
        } else if s.ends_with("D01") || s.ends_with("D02") || s.ends_with("D03") {
            if !saw_format {
                return Err("coordinates before the format statement".into());
            }
            objects += 1;
            if objects > MAX_OBJECTS {
                return Err(format!("more than {MAX_OBJECTS} objects"));
            }
            let x = field(s, 'X').map(number).transpose()?.unwrap_or(at[0]);
            let y = field(s, 'Y').map(number).transpose()?.unwrap_or(at[1]);
            let to = [x, y];
            let op = s.get(s.len() - 3..).unwrap_or("");
            if in_region {
                if op == "D02" && contour.len() >= 3 {
                    batch.push(std::mem::take(&mut contour));
                }
                if op == "D02" {
                    contour.clear();
                }
                contour.push(to);
            } else {
                let width = current.and_then(|d| apertures.get(&d).copied()).ok_or("a draw with no aperture")?;
                match op {
                    "D01" => batch.push(hull_outline(&stroke(at, to, width / 2.0), 0.0)),
                    "D03" => batch.push(hull_outline(&stroke(to, to, width / 2.0), 0.0)),
                    _ => {}
                }
            }
            at = to;
        } else {
            return Err(format!("unsupported statement `{s}`"));
        }
    }
    flush(&mut image, &mut batch, dark);
    Ok(Image { function, region: image, objects })
}

/// Read an Excellon file this module wrote: its holes.
pub fn excellon(text: &str) -> Result<Vec<Hole>, String> {
    let mut tools: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
    let mut current: Option<f64> = None;
    let mut out = Vec::new();
    let mut header = true;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if header {
            if line == "%" {
                header = false;
            } else if let Some(t) = line.strip_prefix('T').filter(|t| t.contains('C')) {
                let (n, d) = t.split_once('C').ok_or("bad tool")?;
                tools.insert(n.to_string(), d.parse().map_err(|_| format!("bad tool `{line}`"))?);
            }
            continue;
        }
        if let Some(t) = line.strip_prefix('T') {
            current = tools.get(t).copied();
            continue;
        }
        if line.starts_with('X') {
            let d = current.ok_or("a hole before a tool")?;
            let parse = |s: &str| -> Result<[f64; 2], String> {
                let x = field(s, 'X').ok_or("no X")?.parse::<f64>().map_err(|_| "bad X")?;
                let y = field(s, 'Y').ok_or("no Y")?.parse::<f64>().map_err(|_| "bad Y")?;
                Ok([x, y])
            };
            match line.split_once("G85") {
                Some((a, b)) => out.push(Hole { diameter: d, from: parse(a)?, to: Some(parse(b)?) }),
                None => out.push(Hole { diameter: d, from: parse(line)?, to: None }),
            }
            if out.len() > MAX_OBJECTS {
                return Err(format!("more than {MAX_OBJECTS} holes"));
            }
        }
    }
    Ok(out)
}
