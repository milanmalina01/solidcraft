//! Print pages of the electronics design (`solidcraft_io::pages`): a cover, a page per section
//! frame of the schematic with cross-references, a block diagram, the parts list and the board,
//! written as one PDF, an SVG or PNG per page, or the parts list as CSV; and the title block's
//! fields and the revision history (`ecad.design.info`), saved with the design. Exporting changes
//! nothing in the design.

use serde_json::Value;
use solidcraft_doc::ecad::DesignInfo;
use solidcraft_doc::ecad::info::{MAX_REVISIONS, Revision};
use solidcraft_doc::metadata;
use solidcraft_io::pages::{self, Book, MAX_PIXELS, Options, TitleInfo};

use super::CommandSpec;
use super::ecad::{commit, parse, reply};
use crate::params::bad;
use crate::schema::ecad_export::{
    ExportParams, ExportResult, FileOut, FrameRef, InfoInspectParams, InfoOut, InfoParams, InfoResult, InspectParams, InspectResult, Orientation,
    PageKind, PageOut, PagesFormat, Paper, RevisionOut, Style,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("ecad.design.info", "Design Info", crate::checked!("ecad.design.info", info))
        .icon("settings")
        .doc_only()
        .contract(&INFO_CONTRACT)
        .output(output_schema_for::<InfoResult>)
        .params(
            "title?, revision?, date? (as it should print; never the clock), author?, company?, notes? (short); each given one is set, \"\" clears; revisions? [{rev, date?, by?, note?}] (the whole history, oldest first, at most 50; replaces the saved one, [] clears; the cover prints it newest first, or one row from revision, date and author while it is empty) → {info (saved), printed (an empty field falls back to the design properties project, revision, author, organization; the title to the design's name), changed}",
        ),
    CommandSpec::new("ecad.design.info.inspect", "Inspect Design Info", crate::checked!("ecad.design.info.inspect", info_inspect))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&INFO_INSPECT_CONTRACT)
        .output(output_schema_for::<InfoResult>)
        .params("No parameters → {info, printed}: the title block's fields as saved and as printed. Read-only"),
    CommandSpec::new("ecad.export.pages", "Export Pages", crate::checked!("ecad.export.pages", export))
        .icon("export")
        .noundo()
        .doc_only()
        .contract(&EXPORT_CONTRACT)
        .output(output_schema_for::<ExportResult>)
        .params(
            "pages? [cover|sections|block_diagram|parts|board_top|board_bottom|inner_layers|assembly_top|assembly_bottom|stackup|net_index] (left out: all that apply; cover: the title, notes, revision, date, author, the contents, the revision history and a picture of the board's top side; overview is taken as cover; a sheet with no frames prints its sections as tiles; net_index: every named net's pages and zones, at the end, printed when a cross-reference list is cut short or power_xrefs is on), frames? [id|title] (what prints keeps its page numbers and cross-references in the whole book), paper? a4|a3|letter|tabloid (left out: A4 when every frame fits it at full-size text, else A3, whichever frames print), orientation? landscape|portrait (left out: landscape, a tall section portrait when that prints it 1.25× larger), style? paper|monochrome|dark, power_xrefs? (cross-reference rails too; ground never is, the net index lists it; a list names at most 3 pages, else 2 and `+N more` linking to the net index), format: pdf|svg|png|csv (csv: the parts list; the page parameters do not apply), dpi? (png, 36 to 1200, 200; a page is at most 80 million pixels), path (pdf: a .pdf file; csv: a .csv file; svg/png: a folder, or a name stem: out/lora gives out/lora-01-cover.svg …) → {files: [{path, pages (numbers in the whole book), bytes, rows? (csv: the parts list's rows)}], warnings}. Changes nothing in the design",
        ),
    CommandSpec::new("ecad.export.pages.inspect", "Inspect Pages", crate::checked!("ecad.export.pages.inspect", inspect))
        .icon("inspect")
        .noundo()
        .doc_only()
        .contract(&INSPECT_CONTRACT)
        .output(output_schema_for::<InspectResult>)
        .params(
            "As ecad.export.pages without path (format? left out: pdf; csv lists the parts list's pages) → {pages: [{page, kind, title, frame?, paper, orientation, size, scale?, pixels?}], count, paper, style, warnings (for png also pages past the pixel cap, with the dpi that fits)}: what an export would make. Read-only",
        ),
];

const INFO_CONTRACT: TypedContract<InfoParams> = TypedContract::new();
const INFO_INSPECT_CONTRACT: TypedContract<InfoInspectParams> = TypedContract::new();
const EXPORT_CONTRACT: TypedContract<ExportParams> = TypedContract::new();
const INSPECT_CONTRACT: TypedContract<InspectParams> = TypedContract::new();

/// The most pixels all the pages of one PNG export add up to.
const MAX_PNG_TOTAL: u64 = 1_000_000_000;
/// The longest path accepted.
const MAX_PATH: usize = 4096;

fn revisions_out(list: &[Revision]) -> Vec<RevisionOut> {
    list.iter().map(|r| RevisionOut { rev: r.rev.clone(), date: r.date.clone(), by: r.by.clone(), note: r.note.clone() }).collect()
}

fn info_out(i: &DesignInfo) -> InfoOut {
    InfoOut {
        title: i.title.clone(),
        revision: i.revision.clone(),
        date: i.date.clone(),
        author: i.author.clone(),
        company: i.company.clone(),
        notes: i.notes.clone(),
        revisions: revisions_out(&i.revisions),
    }
}

/// The title block's fields as printed: the saved ones, an empty one from the design's
/// properties, the title from the design's name at last.
pub fn title_info(s: &Session) -> TitleInfo {
    let i = &s.doc.ecad.info;
    let m = &s.doc.metadata;
    let or = |own: &str, key: &str| if own.is_empty() { m.text(key).unwrap_or_default().to_string() } else { own.to_string() };
    let mut title = or(&i.title, metadata::PROJECT);
    if title.is_empty() {
        title = s.doc.name.clone();
    }
    TitleInfo {
        title,
        revision: or(&i.revision, metadata::REVISION),
        date: i.date.clone(),
        author: or(&i.author, metadata::AUTHOR),
        company: or(&i.company, metadata::ORGANIZATION),
        notes: i.notes.clone(),
        revisions: i.revisions.clone(),
    }
}

fn info_result(s: &Session, changed: Vec<String>) -> Result<Value> {
    let p = title_info(s);
    let printed = InfoOut {
        title: p.title,
        revision: p.revision,
        date: p.date,
        author: p.author,
        company: p.company,
        notes: p.notes,
        revisions: revisions_out(&p.revisions),
    };
    reply(InfoResult { info: info_out(&s.doc.ecad.info), printed, changed, param_warnings: Vec::new() })
}

fn info(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.design.info";
    let p: InfoParams = parse(CMD, p)?;
    let mut next = s.doc.ecad.clone();
    let mut changed = Vec::new();
    {
        let i = &mut next.info;
        for (name, field, value) in [
            ("title", &mut i.title, p.title),
            ("revision", &mut i.revision, p.revision),
            ("date", &mut i.date, p.date),
            ("author", &mut i.author, p.author),
            ("company", &mut i.company, p.company),
            ("notes", &mut i.notes, p.notes),
        ] {
            if let Some(v) = value {
                let v = v.trim().to_string();
                if *field != v {
                    *field = v;
                    changed.push(name.to_string());
                }
            }
        }
        if let Some(list) = p.revisions {
            if list.len() > MAX_REVISIONS {
                return Err(bad(CMD, format!("`revisions` holds at most {MAX_REVISIONS} revisions (got {})", list.len())));
            }
            let trim = |v: Option<String>| v.map(|v| v.trim().to_string()).unwrap_or_default();
            let list: Vec<Revision> = list
                .into_iter()
                .map(|r| Revision { rev: r.rev.trim().to_string(), date: trim(r.date), by: trim(r.by), note: trim(r.note) })
                .collect();
            if i.revisions != list {
                i.revisions = list;
                changed.push("revisions".to_string());
            }
        }
        i.validate().map_err(|e| bad(CMD, e))?;
    }
    // The board's markings print the title, revision and date: made again, in this same step.
    if !next.board.markings.is_empty() && changed.iter().any(|c| matches!(c.as_str(), "title" | "revision" | "date" | "company" | "author")) {
        let f = super::ecad_board_marking::fields(s, &next);
        solidcraft_doc::ecad::marking::refresh(&mut next, &f).map_err(|e| bad(CMD, e))?;
    }
    commit(s, next, CMD)?;
    info_result(s, changed)
}

fn info_inspect(s: &mut Session, p: &Value) -> Result<Value> {
    let _: InfoInspectParams = parse("ecad.design.info.inspect", p)?;
    info_result(s, Vec::new())
}

fn kind_in(k: PageKind) -> pages::PageKind {
    match k {
        PageKind::Cover => pages::PageKind::Cover,
        PageKind::Sections => pages::PageKind::Sections,
        PageKind::BlockDiagram => pages::PageKind::BlockDiagram,
        PageKind::Parts => pages::PageKind::Parts,
        PageKind::BoardTop => pages::PageKind::BoardTop,
        PageKind::BoardBottom => pages::PageKind::BoardBottom,
        PageKind::InnerLayers => pages::PageKind::InnerLayers,
        PageKind::AssemblyTop => pages::PageKind::AssemblyTop,
        PageKind::AssemblyBottom => pages::PageKind::AssemblyBottom,
        PageKind::Stackup => pages::PageKind::Stackup,
        PageKind::NetIndex => pages::PageKind::NetIndex,
    }
}

fn kind_out(k: pages::PageKind) -> PageKind {
    match k {
        pages::PageKind::Cover => PageKind::Cover,
        pages::PageKind::Sections => PageKind::Sections,
        pages::PageKind::BlockDiagram => PageKind::BlockDiagram,
        pages::PageKind::Parts => PageKind::Parts,
        pages::PageKind::BoardTop => PageKind::BoardTop,
        pages::PageKind::BoardBottom => PageKind::BoardBottom,
        pages::PageKind::InnerLayers => PageKind::InnerLayers,
        pages::PageKind::AssemblyTop => PageKind::AssemblyTop,
        pages::PageKind::AssemblyBottom => PageKind::AssemblyBottom,
        pages::PageKind::Stackup => PageKind::Stackup,
        pages::PageKind::NetIndex => PageKind::NetIndex,
    }
}

fn paper_in(p: Paper) -> pages::Paper {
    match p {
        Paper::A4 => pages::Paper::A4,
        Paper::A3 => pages::Paper::A3,
        Paper::Letter => pages::Paper::Letter,
        Paper::Tabloid => pages::Paper::Tabloid,
    }
}

fn paper_out(p: pages::Paper) -> Paper {
    match p {
        pages::Paper::A4 => Paper::A4,
        pages::Paper::A3 => Paper::A3,
        pages::Paper::Letter => Paper::Letter,
        pages::Paper::Tabloid => Paper::Tabloid,
    }
}

fn style_in(s: Style) -> pages::Style {
    match s {
        Style::Paper => pages::Style::Paper,
        Style::Monochrome => pages::Style::Monochrome,
        Style::Dark => pages::Style::Dark,
    }
}

/// The design file's name, as the title block prints it.
fn file_name(s: &Session) -> String {
    let named = s.path.as_deref().and_then(|p| std::path::Path::new(p).file_name()).and_then(|n| n.to_str()).map(str::to_string);
    named.unwrap_or_else(|| format!("{}.{}", s.doc.name, solidcraft_io::DESIGN_EXT))
}

/// The design's name as a file name stem: letters, digits, `-` and `_`.
fn stem(s: &Session) -> String {
    let name = file_name(s);
    let base = name.rsplit_once('.').map_or(name.as_str(), |(b, _)| b);
    let out: String = base.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    if out.trim_matches('-').is_empty() { "pages".into() } else { out }
}

#[allow(clippy::too_many_arguments)]
fn options(
    s: &Session,
    cmd: &str,
    kinds: Option<Vec<PageKind>>,
    frames: Option<Vec<FrameRef>>,
    paper: Option<Paper>,
    orientation: Option<Orientation>,
    style: Option<Style>,
    power_xrefs: Option<bool>,
) -> Result<Options> {
    let kinds = kinds.unwrap_or_default();
    if kinds.len() > 64 {
        return Err(bad(cmd, "`pages` names at most 64 kinds of page"));
    }
    let frames = match frames {
        None => None,
        Some(f) => {
            if f.is_empty() || f.len() > solidcraft_doc::ecad::frame::MAX_FRAMES {
                return Err(bad(cmd, format!("`frames` names 1 to {} frames by id or title", solidcraft_doc::ecad::frame::MAX_FRAMES)));
            }
            let sheet = &s.doc.ecad.schematic;
            let mut ids = Vec::with_capacity(f.len());
            for r in &f {
                let found = match r {
                    FrameRef::Id(id) => sheet.frames.iter().find(|x| x.id == *id),
                    FrameRef::Title(t) => {
                        sheet.frames.iter().find(|x| x.title == *t).or_else(|| sheet.frames.iter().find(|x| x.title.eq_ignore_ascii_case(t)))
                    }
                };
                let Some(found) = found else {
                    let known: Vec<String> = sheet.frames.iter().map(|x| format!("{} ({})", x.id, x.title)).collect();
                    let hint = if known.is_empty() { "it has none".to_string() } else { format!("it has {}", known.join(", ")) };
                    let what = match r {
                        FrameRef::Id(id) => id.to_string(),
                        FrameRef::Title(t) => format!("`{t}`"),
                    };
                    return Err(bad(cmd, format!("`frames`: the schematic has no frame {what} ({hint})")));
                };
                ids.push(found.id);
            }
            Some(ids)
        }
    };
    let mut pages: Vec<pages::PageKind> = kinds.into_iter().map(kind_in).collect();
    pages.sort();
    pages.dedup();
    Ok(Options {
        pages,
        frames,
        paper: paper.map(paper_in),
        orientation: orientation.map(|o| match o {
            Orientation::Portrait => pages::Orientation::Portrait,
            Orientation::Landscape => pages::Orientation::Landscape,
        }),
        style: style.map_or(pages::Style::Paper, style_in),
        power_xrefs: power_xrefs.unwrap_or(false),
        file_name: file_name(s),
        info: title_info(s),
    })
}

fn dpi(d: Option<f64>, cmd: &str) -> Result<f64> {
    let d = d.unwrap_or(pages::DEFAULT_DPI);
    if !(d.is_finite() && pages::DPI.contains(&d)) {
        return Err(bad(cmd, format!("`dpi` must be {} to {}", pages::DPI.start(), pages::DPI.end())));
    }
    Ok(d)
}

/// Why a PNG export of `book` at `dpi` is past the pixel caps, if it is: the first page too
/// large, or all of them together, with the resolution that would fit.
fn pixel_problem(book: &Book, dpi: f64) -> Option<String> {
    let mut total = 0u64;
    for (i, p) in book.pages.iter().enumerate() {
        let (w, h) = pages::pixels(p, dpi);
        let n = u64::from(w) * u64::from(h);
        if n > MAX_PIXELS {
            let most = (dpi * (MAX_PIXELS as f64 / n as f64).sqrt()).floor();
            let number = book.entries.get(i).map_or(i + 1, |e| e.number);
            return Some(format!("page {number} would be {w} × {h} pixels, more than {MAX_PIXELS}: `dpi` {most} or lower fits"));
        }
        total = total.saturating_add(n);
    }
    if total > MAX_PNG_TOTAL {
        let most = (dpi * (MAX_PNG_TOTAL as f64 / total as f64).sqrt()).floor();
        return Some(format!(
            "the pages would be {total} pixels together, more than {MAX_PNG_TOTAL}: `dpi` {most} or lower fits, or export fewer pages"
        ));
    }
    None
}

/// Refuse a PNG export whose pages are past the pixel caps.
fn check_pixels(book: &Book, dpi: f64, cmd: &str) -> Result<()> {
    match pixel_problem(book, dpi) {
        Some(why) => Err(bad(cmd, why)),
        None => Ok(()),
    }
}

/// A page's file name part: its number in the whole book in two digits and its title in lower
/// case, such as `03-lora-radio`.
fn page_slug(number: usize, title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(40).collect();
    format!("{number:02}-{}", slug.trim_end_matches('-'))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "ecad.export.pages";
    let p: ExportParams = parse(CMD, p)?;
    let path = p.path.trim().to_string();
    if path.is_empty() || path.chars().count() > MAX_PATH || path.contains('\0') {
        return Err(bad(CMD, format!("`path` must be a file or folder path of 1 to {MAX_PATH} characters")));
    }
    let given = (p.pages.is_some(), p.frames.is_some());
    let o = options(s, CMD, p.pages, p.frames, p.paper, p.orientation, p.style, p.power_xrefs)?;
    let lower = path.to_ascii_lowercase();
    let write = |path: &str, bytes: &[u8]| -> Result<u64> {
        solidcraft_io::vfs::write(path, bytes).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
        Ok(bytes.len() as u64)
    };
    let mut files = Vec::new();
    let mut param_warnings = Vec::new();
    let warnings = match p.format {
        PagesFormat::Csv => {
            // The parts list's rows, whatever the pages would look like.
            let ignored: Vec<&str> = [
                ("pages", given.0),
                ("frames", given.1),
                ("paper", p.paper.is_some()),
                ("orientation", p.orientation.is_some()),
                ("style", p.style.is_some()),
                ("power_xrefs", p.power_xrefs.is_some()),
                ("dpi", p.dpi.is_some()),
            ]
            .into_iter()
            .filter_map(|(name, given)| given.then_some(name))
            .collect();
            if !ignored.is_empty() {
                let verb = if ignored.len() == 1 { "does" } else { "do" };
                param_warnings.push(format!(
                    "a CSV export is the parts list only: {} {verb} not apply",
                    ignored.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")
                ));
            }
            if !lower.ends_with(".csv") {
                return Err(bad(CMD, format!("`path` must name a .csv file for the parts list (got `{path}`)")));
            }
            if s.doc.ecad.schematic.parts.is_empty() {
                return Err(bad(CMD, "the schematic has no parts, so the parts list is empty"));
            }
            let text = pages::parts_csv(&s.doc.ecad);
            let bytes = write(&path, text.as_bytes())?;
            let rows = pages::parts_rows(&s.doc.ecad);
            files.push(FileOut { path, pages: Vec::new(), bytes, rows: Some(rows) });
            Vec::new()
        }
        PagesFormat::Pdf => {
            if !lower.ends_with(".pdf") {
                return Err(bad(CMD, format!("`path` must name a .pdf file (got `{path}`)")));
            }
            let book = pages::compose(&s.doc.ecad, &o).map_err(|e| bad(CMD, e))?;
            let bytes = pages::write_pdf(&book).map_err(EngineError::Other)?;
            let n = write(&path, &bytes)?;
            files.push(FileOut { path, pages: book.entries.iter().map(|e| e.number).collect(), bytes: n, rows: None });
            book.warnings
        }
        PagesFormat::Svg | PagesFormat::Png => {
            let ext = if p.format == PagesFormat::Svg { "svg" } else { "png" };
            let dpi = dpi(p.dpi, CMD)?;
            let book = pages::compose(&s.doc.ecad, &o).map_err(|e| bad(CMD, e))?;
            if p.format == PagesFormat::Png {
                check_pixels(&book, dpi, CMD)?;
            }
            // A folder (one that exists, or a path ending in a separator), else a name stem.
            let folder = path.ends_with('/') || path.ends_with('\\') || (solidcraft_io::vfs::exists(&path) && !solidcraft_io::vfs::is_file(&path));
            let base = if folder {
                solidcraft_io::vfs::create_dir_all(&path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
                format!("{}/{}", path.trim_end_matches(['/', '\\']), stem(s))
            } else {
                let dot = format!(".{ext}");
                if lower.ends_with(&dot) { path.get(..path.len() - dot.len()).unwrap_or(&path).to_string() } else { path.clone() }
            };
            let number = |i: usize| book.entries.get(i).map_or(i + 1, |e| e.number);
            let names: Vec<String> =
                book.pages.iter().enumerate().map(|(i, page)| format!("{base}-{}.{ext}", page_slug(number(i), &page.title))).collect();
            let leaf = |k: usize| names.get(k).map(|n| n.rsplit(['/', '\\']).next().unwrap_or(n).to_string()).unwrap_or_default();
            for (i, name) in names.iter().enumerate() {
                let bytes = if p.format == PagesFormat::Svg {
                    pages::write_svg(&book, i, &leaf).map_err(EngineError::Other)?.into_bytes()
                } else {
                    pages::write_png(&book, i, dpi).map_err(EngineError::Other)?
                };
                let n = write(name, &bytes)?;
                files.push(FileOut { path: name.clone(), pages: vec![number(i)], bytes: n, rows: None });
            }
            book.warnings
        }
    };
    reply(ExportResult { files, warnings, param_warnings })
}

fn inspect(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, out) = composed(s, p)?;
    reply(out)
}

/// The pages `ecad.export.pages.inspect` lists for its parameters `p`, with the book they make,
/// so a preview can draw any of them; its reply as the command gives it. Reads the design only.
pub fn pages_book(s: &Session, p: &Value) -> Result<(Book, Value)> {
    let (book, out) = composed(s, p)?;
    Ok((book, reply(out)?))
}

fn composed(s: &Session, p: &Value) -> Result<(Book, InspectResult)> {
    const CMD: &str = "ecad.export.pages.inspect";
    let p: InspectParams = parse(CMD, p)?;
    let csv = p.format == Some(PagesFormat::Csv);
    // A CSV holds the parts list's rows: its pages are what it is.
    let kinds = if csv { Some(vec![PageKind::Parts]) } else { p.pages };
    let o = options(s, CMD, kinds, p.frames, p.paper, p.orientation, p.style, p.power_xrefs)?;
    let png = p.format == Some(PagesFormat::Png);
    let dpi = dpi(p.dpi, CMD)?;
    let mut book = pages::compose(&s.doc.ecad, &o).map_err(|e| bad(CMD, e))?;
    if png && let Some(why) = pixel_problem(&book, dpi) {
        book.warnings.push(format!("as PNG {why}"));
    }
    let list = book
        .entries
        .iter()
        .zip(&book.pages)
        .map(|(e, page)| PageOut {
            page: e.number,
            kind: kind_out(e.kind),
            title: e.title.clone(),
            frame: e.frame,
            paper: paper_out(e.paper),
            orientation: if e.orientation == pages::Orientation::Portrait { Orientation::Portrait } else { Orientation::Landscape },
            size: page.size,
            scale: e.scale.clone(),
            pixels: png.then(|| {
                let (w, h) = pages::pixels(page, dpi);
                [w, h]
            }),
        })
        .collect();
    let style = match o.style {
        pages::Style::Paper => Style::Paper,
        pages::Style::Monochrome => Style::Monochrome,
        pages::Style::Dark => Style::Dark,
    };
    let out = InspectResult {
        count: book.pages.len(),
        pages: list,
        paper: paper_out(book.paper),
        style,
        warnings: book.warnings.clone(),
        param_warnings: Vec::new(),
    };
    Ok((book, out))
}

#[cfg(test)]
#[path = "ecad_export_tests.rs"]
mod tests;
