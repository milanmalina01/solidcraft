//! One page as an SVG file: millimetre user units, a group per layer, text as text in the page
//! font (named, with fallbacks), clipped groups, and links to the other pages' files.

use std::fmt::Write;

use super::font::{Font, SVG_FAMILY};
use super::page::{Item, Page, Stroke, Text, Weight, rounded_rect};

/// The SVG of `page`; `file_of(i)` names page `i`'s file for the links.
pub fn write(page: &Page, font: &Font, file_of: &dyn Fn(usize) -> String) -> String {
    let [w, h] = page.size;
    let mut out = String::with_capacity(64 * 1024);
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{}mm" height="{}mm" viewBox="0 0 {} {}" font-family="{SVG_FAMILY}" stroke-linecap="round" stroke-linejoin="round">"#,
        num(w),
        num(h),
        num(w),
        num(h)
    );
    let _ = writeln!(out, "<title>{}</title>", escape(&page.title));
    let _ = writeln!(out, r#"<rect width="{}" height="{}" fill="{}"/>"#, num(w), num(h), page.background.hex());
    let mut clips = 0usize;
    for l in &page.layers {
        let _ = writeln!(out, r#"<g id="{}">"#, l.name);
        items(&mut out, &l.items, font, &mut clips);
        out.push_str("</g>\n");
    }
    if !page.links.is_empty() {
        out.push_str("<g id=\"links\">\n");
        for link in &page.links {
            let [[x0, y0], [x1, y1]] = link.rect;
            let _ = writeln!(
                out,
                r#"<a xlink:href="{0}" href="{0}"><rect x="{1}" y="{2}" width="{3}" height="{4}" fill="transparent"/></a>"#,
                escape(&file_of(link.page)),
                num(x0.min(x1)),
                num(y0.min(y1)),
                num((x1 - x0).abs()),
                num((y1 - y0).abs())
            );
        }
        out.push_str("</g>\n");
    }
    out.push_str("</svg>\n");
    out
}

/// A number with at most three decimals and no trailing zeros.
fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn stroke_attrs(s: &Stroke) -> String {
    let mut a = format!(r#" stroke="{}" stroke-width="{}""#, s.color.hex(), num(s.width));
    if let Some([on, off]) = s.dash {
        let _ = write!(a, r#" stroke-dasharray="{} {}""#, num(on), num(off));
    }
    a
}

fn points(p: &[[f64; 2]]) -> String {
    let mut out = String::with_capacity(p.len() * 12);
    for (i, q) in p.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{},{}", num(q[0]), num(q[1]));
    }
    out
}

fn items(out: &mut String, list: &[Item], font: &Font, clips: &mut usize) {
    for i in list {
        match i {
            Item::Line { points: p, closed, stroke } => {
                if p.len() < 2 {
                    continue;
                }
                let tag = if *closed { "polygon" } else { "polyline" };
                let _ = writeln!(out, r#"<{tag} points="{}" fill="none"{}/>"#, points(p), stroke_attrs(stroke));
            }
            Item::Area { rings, fill, stroke } => {
                let mut d = String::new();
                for r in rings.iter().filter(|r| r.len() >= 3) {
                    for (k, q) in r.iter().enumerate() {
                        let _ = write!(d, "{}{},{} ", if k == 0 { 'M' } else { 'L' }, num(q[0]), num(q[1]));
                    }
                    d.push('Z');
                }
                let s = stroke.as_ref().map(stroke_attrs).unwrap_or_default();
                let _ = writeln!(out, r#"<path d="{d}" fill="{}" fill-rule="evenodd"{s}/>"#, fill.hex());
            }
            Item::Rect { rect, radius, fill, stroke } => {
                let fill = fill.map_or("none".to_string(), |f| f.hex());
                let s = stroke.as_ref().map(stroke_attrs).unwrap_or_default();
                if *radius > 0.0 {
                    let _ = writeln!(out, r#"<polygon points="{}" fill="{fill}"{s}/>"#, points(&rounded_rect(*rect, *radius)));
                } else {
                    let [a, b] = *rect;
                    let _ = writeln!(
                        out,
                        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{fill}"{s}/>"#,
                        num(a[0].min(b[0])),
                        num(a[1].min(b[1])),
                        num((b[0] - a[0]).abs()),
                        num((b[1] - a[1]).abs())
                    );
                }
            }
            Item::Circle { center, radius, fill, stroke } => {
                let fill = fill.map_or("none".to_string(), |f| f.hex());
                let s = stroke.as_ref().map(stroke_attrs).unwrap_or_default();
                let _ = writeln!(out, r#"<circle cx="{}" cy="{}" r="{}" fill="{fill}"{s}/>"#, num(center[0]), num(center[1]), num(*radius));
            }
            Item::Text(t) => text(out, t, font),
            Item::Clip { rect: [a, b], items: inner } => {
                *clips += 1;
                let id = format!("clip{}", *clips);
                let _ = writeln!(
                    out,
                    r#"<clipPath id="{id}"><rect x="{}" y="{}" width="{}" height="{}"/></clipPath><g clip-path="url(#{id})">"#,
                    num(a[0].min(b[0])),
                    num(a[1].min(b[1])),
                    num((b[0] - a[0]).abs()),
                    num((b[1] - a[1]).abs())
                );
                items(out, inner, font, clips);
                out.push_str("</g>\n");
            }
        }
    }
}

fn text(out: &mut String, t: &Text, font: &Font) {
    if t.text.is_empty() || !(t.size > 0.0) {
        return;
    }
    let (o, along, down) = font.placement(t);
    let weight = if t.weight == Weight::Bold { r#" font-weight="bold""# } else { "" };
    let _ = writeln!(
        out,
        r#"<text transform="matrix({} {} {} {} {} {})" font-size="{}" fill="{}"{weight} xml:space="preserve">{}</text>"#,
        num(along[0]),
        num(along[1]),
        num(down[0]),
        num(down[1]),
        num(o[0]),
        num(o[1]),
        num(t.size),
        t.color.hex(),
        escape(&t.text)
    );
}
