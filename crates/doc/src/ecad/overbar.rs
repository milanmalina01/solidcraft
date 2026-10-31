//! Overbar markup in names: `~{text}` is `text` with a bar over it, as datasheets mark an
//! active-low signal (`~{CE}`, `~{WR}/RD`). A name keeps the markup as written, so matching, the
//! netlist and search use the raw name; only drawing reads it, and a drawn name's width leaves the
//! markup out. A `~{` with no `}` bars the rest of the name; a `~` not followed by `{` is an
//! ordinary character, and so is a `{` or `~` inside a bar.

use std::borrow::Cow;
use std::ops::Range;

/// What a place that cannot draw a bar puts after each barred run instead (a macron: `CE¯`).
pub const MARKER: char = '\u{af}';

/// A name as drawn: its text without the markup, and the runs of it (in characters) under a bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Barred {
    pub text: String,
    pub bars: Vec<Range<usize>>,
}

/// Whether `name` has any overbar markup.
pub fn has_markup(name: &str) -> bool {
    name.contains("~{")
}

/// `name` split into its drawn text and its bars.
pub fn parse(name: &str) -> Barred {
    let mut out = Barred { text: String::with_capacity(name.len()), bars: Vec::new() };
    let mut count = 0usize;
    let mut open: Option<usize> = None;
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, open) {
            ('~', None) if chars.peek() == Some(&'{') => {
                chars.next();
                open = Some(count);
            }
            ('}', Some(start)) => {
                if count > start {
                    out.bars.push(start..count);
                }
                open = None;
            }
            _ => {
                out.text.push(c);
                count += 1;
            }
        }
    }
    if let Some(start) = open
        && count > start
    {
        out.bars.push(start..count);
    }
    out
}

/// `name` without its markup: what is drawn, and what a width is measured on.
pub fn plain(name: &str) -> Cow<'_, str> {
    if has_markup(name) { Cow::Owned(parse(name).text) } else { Cow::Borrowed(name) }
}

/// The first `n` letters of `b` written with their markup again (a bar cut short ends where
/// they do).
pub fn cut(b: &Barred, n: usize) -> String {
    let mut out = String::with_capacity(b.text.len() + 3 * b.bars.len());
    for (i, c) in b.text.chars().take(n).enumerate() {
        let bar = b.bars.iter().find(|r| r.contains(&i));
        if bar.is_some_and(|r| r.start == i) {
            out.push_str("~{");
        }
        out.push(c);
        if bar.is_some_and(|r| r.end == i + 1 || i + 1 == n) {
            out.push('}');
        }
    }
    out
}

/// `name` as plain text for a place that cannot draw a bar: [`MARKER`] after each barred run
/// (`~{CE}` → `CE¯`).
pub fn marked(name: &str) -> Cow<'_, str> {
    if !has_markup(name) {
        return Cow::Borrowed(name);
    }
    let b = parse(name);
    let mut out = String::with_capacity(b.text.len() + 2 * b.bars.len());
    for (i, c) in b.text.chars().enumerate() {
        out.push(c);
        if b.bars.iter().any(|r| r.end == i + 1) {
            out.push(MARKER);
        }
    }
    Cow::Owned(out)
}
