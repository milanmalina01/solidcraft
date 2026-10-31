//! Print pages of an electronics design: a cover (the title, the contents, the revision history
//! and a picture of the board), a page per section frame of the schematic with cross-references to
//! where each net continues, a block diagram of the frames, the parts list, the board (top and
//! bottom copper, assembly views and the layer stack-up) and an index of the nets, each page in a
//! border with zones and a title block. One display list per page
//! (`page`) feeds the writers: one PDF with every page, or an SVG or PNG per page; the parts list
//! also goes out as CSV.
//!
//! Everything is worked out from the design on each call and nothing is saved. Writers return
//! bytes; reading and writing files is the caller's (the browser build has no file system here).

mod blocks;
mod board;
mod border;
mod cover;
mod font;
mod nets;
pub mod page;
mod parts;
mod pdf;
mod png;
mod sections;
mod sheet;
mod svg;

use solidcraft_doc::ecad::Electronics;

pub use border::{nice_scale, scale_text};
pub use page::{Item, Layer, Link, Page, Rgb};
pub use png::{DPI, MAX_PIXELS, pixels};

/// The most pages one book has.
pub const MAX_PAGES: usize = 200;
/// The smallest text printed, in millimetres; smaller text is set at this size.
pub const MIN_TEXT: f64 = 1.6;
/// The resolution PNG pages get when none is given, in dots an inch.
pub const DEFAULT_DPI: f64 = 200.0;
/// How much larger a section must print on a portrait page before it gets one (orientation left
/// to the book).
pub const TURN_GAIN: f64 = 1.25;

/// What a page shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PageKind {
    /// The title, the contents, the revisions and the board's top side.
    Cover,
    Sections,
    BlockDiagram,
    Parts,
    BoardTop,
    BoardBottom,
    /// A page per inner copper layer.
    InnerLayers,
    AssemblyTop,
    AssemblyBottom,
    Stackup,
    /// Every named net and the pages it is on, at the end of the book: printed when a list of
    /// cross-references is cut short or power nets are cross-referenced.
    NetIndex,
}

impl PageKind {
    pub const ALL: [PageKind; 11] = [
        PageKind::Cover,
        PageKind::Sections,
        PageKind::BlockDiagram,
        PageKind::Parts,
        PageKind::BoardTop,
        PageKind::BoardBottom,
        PageKind::InnerLayers,
        PageKind::AssemblyTop,
        PageKind::AssemblyBottom,
        PageKind::Stackup,
        PageKind::NetIndex,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PageKind::Cover => "cover",
            PageKind::Sections => "sections",
            PageKind::BlockDiagram => "block_diagram",
            PageKind::Parts => "parts",
            PageKind::BoardTop => "board_top",
            PageKind::BoardBottom => "board_bottom",
            PageKind::InnerLayers => "inner_layers",
            PageKind::AssemblyTop => "assembly_top",
            PageKind::AssemblyBottom => "assembly_bottom",
            PageKind::Stackup => "stackup",
            PageKind::NetIndex => "net_index",
        }
    }

    /// Whether it is one of the board's pages.
    pub fn is_board(self) -> bool {
        matches!(
            self,
            PageKind::BoardTop | PageKind::BoardBottom | PageKind::InnerLayers | PageKind::AssemblyTop | PageKind::AssemblyBottom | PageKind::Stackup
        )
    }
}

/// A paper size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paper {
    A4,
    A3,
    Letter,
    Tabloid,
}

impl Paper {
    /// Width and height upright, in millimetres.
    pub fn size(self) -> [f64; 2] {
        match self {
            Paper::A4 => [210.0, 297.0],
            Paper::A3 => [297.0, 420.0],
            Paper::Letter => [215.9, 279.4],
            Paper::Tabloid => [279.4, 431.8],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Paper::A4 => "A4",
            Paper::A3 => "A3",
            Paper::Letter => "Letter",
            Paper::Tabloid => "Tabloid",
        }
    }

    /// The next size up in the same family, if any.
    pub fn larger(self) -> Option<Paper> {
        match self {
            Paper::A4 => Some(Paper::A3),
            Paper::Letter => Some(Paper::Tabloid),
            Paper::A3 | Paper::Tabloid => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Landscape,
    Portrait,
}

impl Orientation {
    pub fn name(self) -> &'static str {
        match self {
            Orientation::Landscape => "landscape",
            Orientation::Portrait => "portrait",
        }
    }
}

/// How the pages look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// White page, dark lines, a little colour: print-safe (the default).
    Paper,
    /// Black and greys only.
    Monochrome,
    /// The app's dark colours, for slides and READMEs.
    Dark,
}

/// The colours a style draws with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub background: Rgb,
    /// Symbol bodies, pins, titles.
    pub ink: Rgb,
    /// Values, pin numbers, captions.
    pub dim: Rgb,
    pub wire: Rgb,
    /// Rail and ground ports.
    pub power: Rgb,
    pub label: Rgb,
    /// Frames, zones and rules.
    pub zone: Rgb,
    /// Cross-references and links.
    pub link: Rgb,
    /// Symbol bodies' fill and table stripes.
    pub body: Rgb,
    pub warn: Rgb,
    pub copper_top: Rgb,
    pub copper_bottom: Rgb,
    pub silk: Rgb,
    pub board: Rgb,
    pub outline: Rgb,
    /// The stack-up's cross-section: core, prepreg, solder mask and copper foil.
    pub core: Rgb,
    pub prepreg: Rgb,
    pub mask: Rgb,
    pub foil: Rgb,
}

impl Style {
    pub fn name(self) -> &'static str {
        match self {
            Style::Paper => "paper",
            Style::Monochrome => "monochrome",
            Style::Dark => "dark",
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            Style::Paper => Palette {
                background: Rgb(255, 255, 255),
                ink: Rgb(34, 38, 46),
                dim: Rgb(98, 106, 118),
                wire: Rgb(24, 64, 120),
                power: Rgb(186, 52, 40),
                label: Rgb(24, 64, 120),
                zone: Rgb(150, 156, 166),
                link: Rgb(30, 110, 200),
                body: Rgb(246, 247, 250),
                warn: Rgb(196, 120, 0),
                copper_top: Rgb(200, 62, 52),
                copper_bottom: Rgb(46, 104, 184),
                silk: Rgb(34, 38, 46),
                board: Rgb(238, 241, 232),
                outline: Rgb(34, 38, 46),
                core: Rgb(224, 211, 166),
                prepreg: Rgb(210, 224, 176),
                mask: Rgb(128, 180, 148),
                foil: Rgb(196, 120, 60),
            },
            Style::Monochrome => Palette {
                background: Rgb(255, 255, 255),
                ink: Rgb(0, 0, 0),
                dim: Rgb(80, 80, 80),
                wire: Rgb(0, 0, 0),
                power: Rgb(0, 0, 0),
                label: Rgb(0, 0, 0),
                zone: Rgb(130, 130, 130),
                link: Rgb(0, 0, 0),
                body: Rgb(255, 255, 255),
                warn: Rgb(0, 0, 0),
                copper_top: Rgb(70, 70, 70),
                copper_bottom: Rgb(70, 70, 70),
                silk: Rgb(0, 0, 0),
                board: Rgb(245, 245, 245),
                outline: Rgb(0, 0, 0),
                core: Rgb(214, 214, 214),
                prepreg: Rgb(236, 236, 236),
                mask: Rgb(150, 150, 150),
                foil: Rgb(70, 70, 70),
            },
            Style::Dark => Palette {
                background: Rgb(46, 52, 64),
                ink: Rgb(222, 226, 232),
                dim: Rgb(148, 155, 168),
                wire: Rgb(58, 136, 238),
                power: Rgb(222, 150, 30),
                label: Rgb(143, 219, 154),
                zone: Rgb(110, 116, 128),
                link: Rgb(120, 178, 248),
                body: Rgb(54, 61, 75),
                warn: Rgb(222, 150, 30),
                copper_top: Rgb(226, 74, 66),
                copper_bottom: Rgb(86, 150, 226),
                silk: Rgb(236, 236, 230),
                board: Rgb(38, 64, 52),
                outline: Rgb(236, 236, 230),
                core: Rgb(150, 136, 92),
                prepreg: Rgb(126, 142, 96),
                mask: Rgb(52, 110, 74),
                foil: Rgb(196, 120, 60),
            },
        }
    }
}

/// The title block's fields as printed (a field left empty prints as a dash), and the revision
/// history for the cover, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TitleInfo {
    pub title: String,
    pub revision: String,
    pub date: String,
    pub author: String,
    pub company: String,
    pub notes: String,
    pub revisions: Vec<solidcraft_doc::ecad::info::Revision>,
}

/// What to print.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// The kinds of page, in this order whatever the order given; empty for all that apply.
    pub pages: Vec<PageKind>,
    /// The frames that get a page of their own (by id); `None` for all.
    pub frames: Option<Vec<u64>>,
    /// `None` picks one (see [`compose`]).
    pub paper: Option<Paper>,
    /// `None`: landscape, but a section page turns to portrait when that prints it at least
    /// [`TURN_GAIN`] times larger (a tall frame).
    pub orientation: Option<Orientation>,
    pub style: Style,
    /// Cross-reference the supply nets too (ground never is: the net index lists it).
    pub power_xrefs: bool,
    /// The design file's name, for the title block.
    pub file_name: String,
    pub info: TitleInfo,
}

/// One page as listed for a check before writing.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Its number in the whole book: a part of the book keeps the whole book's page numbers.
    pub number: usize,
    pub kind: PageKind,
    /// The frame a section page shows.
    pub frame: Option<u64>,
    pub title: String,
    pub paper: Paper,
    pub orientation: Orientation,
    /// The drawing's scale, as the title block states it.
    pub scale: Option<String>,
}

/// The pages, ready for the writers.
#[derive(Clone, Debug, PartialEq)]
pub struct Book {
    pub title: String,
    pub author: String,
    pub pages: Vec<Page>,
    pub entries: Vec<Entry>,
    pub warnings: Vec<String>,
    pub paper: Paper,
}

/// The built-in font's file.
fn font_data() -> Result<solidcraft_sketch::FontData, String> {
    solidcraft_sketch::font_data(None).map_err(|e| format!("the page font: {e}"))
}

/// The pages `o` asks for. Without a paper size: A4 when every section page fits A4 at a scale
/// that prints the sheet's 1.27 mm text at 1.6 mm or more, else A3; one size for the whole book,
/// so it prints on one tray.
pub fn compose(ecad: &Electronics, o: &Options) -> Result<Book, String> {
    let data = font_data()?;
    let font = font::Font::new(data.bytes())?;
    sections::compose(ecad, o, &font)
}

/// The book as one PDF.
pub fn write_pdf(book: &Book) -> Result<Vec<u8>, String> {
    let data = font_data()?;
    let font = font::Font::new(data.bytes())?;
    pdf::write(book, &font, data.bytes())
}

/// Page `i` as SVG; `file_of(k)` names page `k`'s file, for links between pages.
pub fn write_svg(book: &Book, i: usize, file_of: &dyn Fn(usize) -> String) -> Result<String, String> {
    let data = font_data()?;
    let font = font::Font::new(data.bytes())?;
    let page = book.pages.get(i).ok_or_else(|| format!("the book has no page {}", i + 1))?;
    Ok(svg::write(page, &font, file_of))
}

/// Page `i` as PNG at `dpi`.
pub fn write_png(book: &Book, i: usize, dpi: f64) -> Result<Vec<u8>, String> {
    let data = font_data()?;
    let font = font::Font::new(data.bytes())?;
    let page = book.pages.get(i).ok_or_else(|| format!("the book has no page {}", i + 1))?;
    png::write(page, &font, dpi)
}

/// Page `i` drawn at `dpi` as opaque RGBA rows (top first) with its size in pixels, as the PNG
/// writer draws it: for a preview on screen.
pub fn write_rgba(book: &Book, i: usize, dpi: f64) -> Result<([u32; 2], Vec<u8>), String> {
    let data = font_data()?;
    let font = font::Font::new(data.bytes())?;
    let page = book.pages.get(i).ok_or_else(|| format!("the book has no page {}", i + 1))?;
    png::rgba(page, &font, dpi)
}

/// The parts list as CSV: the same rows as its pages.
pub fn parts_csv(ecad: &Electronics) -> String {
    parts::csv(ecad)
}

/// How many rows the parts list has (the CSV's lines after its header).
pub fn parts_rows(ecad: &Electronics) -> usize {
    parts::rows(ecad).len()
}

#[cfg(test)]
mod tests;
