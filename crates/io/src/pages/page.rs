//! The page display list every writer draws from: a paper size, layers of lines, filled areas,
//! rectangles, circles and text runs, clipped groups, and links to other pages. Everything is in
//! millimetres on the page, x right and y down from its top left corner.

/// A colour, 8 bits a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// `#rrggbb`.
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// Each channel from 0 to 1.
    pub fn unit(self) -> [f32; 3] {
        [f32::from(self.0) / 255.0, f32::from(self.1) / 255.0, f32::from(self.2) / 255.0]
    }

    /// Mixed with `other`: `t` 0 is this colour, 1 the other.
    pub fn mix(self, other: Rgb, t: f64) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round().clamp(0.0, 255.0) as u8;
        Rgb(m(self.0, other.0), m(self.1, other.1), m(self.2, other.2))
    }
}

/// How a line is drawn: round caps and joins, solid or dashed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub width: f64,
    pub color: Rgb,
    /// Dash and gap lengths; `None` for a solid line.
    pub dash: Option<[f64; 2]>,
}

impl Stroke {
    pub fn solid(width: f64, color: Rgb) -> Self {
        Stroke { width, color, dash: None }
    }

    pub fn dashed(width: f64, color: Rgb, dash: [f64; 2]) -> Self {
        Stroke { width, color, dash: Some(dash) }
    }
}

/// Where a text's `at` lies along it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Middle,
    End,
}

/// Where a text's `at` lies across it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Baseline {
    /// The top of the tallest letters.
    Top,
    /// Halfway up the capitals.
    Middle,
    Baseline,
    /// The bottom of the descenders (as the app sets a text's lower left).
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Bold,
}

/// A run of text on one line.
#[derive(Clone, Debug, PartialEq)]
pub struct Text {
    pub at: [f64; 2],
    pub text: String,
    /// The font size (an em), in millimetres.
    pub size: f64,
    pub align: Align,
    pub baseline: Baseline,
    /// Degrees counter-clockwise as the page is seen: 90 reads upwards.
    pub rotation: f64,
    pub weight: Weight,
    pub color: Rgb,
    /// Mirrored about `at` before the turn, as text on the bottom of a board seen from the top.
    pub mirror: bool,
}

impl Text {
    /// Level, regular text whose lower left (with the descenders) is `at`.
    pub fn new(at: [f64; 2], text: impl Into<String>, size: f64, color: Rgb) -> Self {
        Text {
            at,
            text: text.into(),
            size,
            align: Align::Start,
            baseline: Baseline::Baseline,
            rotation: 0.0,
            weight: Weight::Regular,
            color,
            mirror: false,
        }
    }

    pub fn align(mut self, align: Align, baseline: Baseline) -> Self {
        self.align = align;
        self.baseline = baseline;
        self
    }

    pub fn bold(mut self) -> Self {
        self.weight = Weight::Bold;
        self
    }

    pub fn turned(mut self, rotation: f64) -> Self {
        self.rotation = rotation;
        self
    }
}

/// One thing drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// Points joined in order; closed joins the last to the first.
    Line {
        points: Vec<[f64; 2]>,
        closed: bool,
        stroke: Stroke,
    },
    /// Closed rings filled even-odd (holes are rings inside rings), and maybe outlined.
    Area {
        rings: Vec<Vec<[f64; 2]>>,
        fill: Rgb,
        stroke: Option<Stroke>,
    },
    /// A rectangle (corners in any order), corners rounded by `radius`.
    Rect {
        rect: [[f64; 2]; 2],
        radius: f64,
        fill: Option<Rgb>,
        stroke: Option<Stroke>,
    },
    Circle {
        center: [f64; 2],
        radius: f64,
        fill: Option<Rgb>,
        stroke: Option<Stroke>,
    },
    Text(Text),
    /// Items drawn only inside `rect`.
    Clip {
        rect: [[f64; 2]; 2],
        items: Vec<Item>,
    },
}

/// A named group of items (an SVG group; the PDF and PNG draw them in order).
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: &'static str,
    pub items: Vec<Item>,
}

/// A clickable area that goes to another page (by index in the book), maybe to a point on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    pub rect: [[f64; 2]; 2],
    pub page: usize,
    pub at: Option<[f64; 2]>,
}

/// One page.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    /// Its bookmark and the title block's page title.
    pub title: String,
    /// Width and height, in millimetres.
    pub size: [f64; 2],
    pub background: Rgb,
    pub layers: Vec<Layer>,
    pub links: Vec<Link>,
}

impl Page {
    /// `items` drawn on the layer called `name`, made at the end if it is new.
    pub fn add(&mut self, name: &'static str, items: impl IntoIterator<Item = Item>) {
        match self.layers.iter_mut().find(|l| l.name == name) {
            Some(l) => l.items.extend(items),
            None => self.layers.push(Layer { name, items: items.into_iter().collect() }),
        }
    }

    /// How many items it draws, clipped groups' items included.
    pub fn item_count(&self) -> usize {
        fn count(items: &[Item]) -> usize {
            items.iter().map(|i| if let Item::Clip { items, .. } = i { 1 + count(items) } else { 1 }).sum()
        }
        self.layers.iter().map(|l| count(&l.items)).sum()
    }
}

/// A rounded rectangle's outline, corners as short arcs, clockwise on the page from the top left.
pub fn rounded_rect([a, b]: [[f64; 2]; 2], radius: f64) -> Vec<[f64; 2]> {
    let (x0, x1) = (a[0].min(b[0]), a[0].max(b[0]));
    let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
    // Not `clamp`: a box with no finite size would make its bound NaN, which panics.
    let room = ((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    let r = if room > 0.0 { radius.max(0.0).min(room) } else { 0.0 };
    if !(r > 1e-6) {
        return vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    }
    const STEPS: usize = 6;
    let mut out = Vec::with_capacity(4 * (STEPS + 1));
    // Corner centres and the angle each corner's arc starts at (y down: 180° is left, 270° up).
    for (cx, cy, start) in [(x0 + r, y0 + r, 180.0f64), (x1 - r, y0 + r, 270.0), (x1 - r, y1 - r, 0.0), (x0 + r, y1 - r, 90.0)] {
        for i in 0..=STEPS {
            let t = (start + 90.0 * i as f64 / STEPS as f64).to_radians();
            out.push([cx + r * t.cos(), cy + r * t.sin()]);
        }
    }
    out
}
