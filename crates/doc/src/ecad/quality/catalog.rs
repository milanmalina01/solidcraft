//! The design-quality checks' catalogue: every code with its family, class, usual severity, what
//! it needs, the guide section that owns its principle, what it finds, the principle in one plain
//! sentence, and its parameters with their defaults and sources. It is the one source the engine,
//! the app and the guide read, so a code's `about` and `section` are never written twice.
//!
//! Each family module declares its own rows (its `CODES`) and the function that checks them, as a
//! [`Family`]; [`FAMILIES`] lists the modules once. Adding codes to a family touches only its own
//! module.

use super::super::checks::Severity;
use super::super::drc::{self, Class};
use super::{Cx, esd, pair, pi_current, pi_decap, pi_return, pi_smps, rf, silk_dfm, xtal};

/// A threshold a check measures against: its default, its units, where the default comes from,
/// and the range a board may set it to (`ecad.quality.rules`).
#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub default: f64,
    /// `mm`, `ratio`, `GHz`…
    pub units: &'static str,
    /// Where the default comes from: a source, or `judgment` for a default the agent or user
    /// overrides from a datasheet.
    pub source: &'static str,
    pub min: f64,
    pub max: f64,
    /// The default rests on public summaries of a standard: the code's findings from it are
    /// warnings at most.
    pub verify: bool,
    pub about: &'static str,
}

/// One code of the catalogue.
#[derive(Clone, Copy, Debug)]
pub struct Code {
    /// `pi.return_path`: the family's prefix, a dot and the check's name.
    pub code: &'static str,
    /// What leaving one of its findings usually costs (a finding may say more or less).
    pub class: Class,
    /// The most severe its findings usually are.
    pub severity: Severity,
    /// The class and severity its findings may rise to when that is more than `class` and
    /// `severity` (a crystal's keepout is critical for a watch crystal, a layer change at T2 an
    /// error): what only the user may lower (see [`Code::worst_class`]).
    pub worst: Option<(Class, Severity)>,
    /// What it needs to run: `geometry` (the board and its stack-up), `profile`, `library`, or
    /// `intent: …` with the roles it reads.
    pub needs: &'static str,
    /// The guide section that owns its principle: `power-ground#return-paths` is
    /// `reference/power-ground.md`, heading "Return paths".
    pub section: &'static str,
    /// What it finds, in one line.
    pub about: &'static str,
    /// Why, in one plain sentence.
    pub principle: &'static str,
    pub params: &'static [Param],
}

impl Code {
    /// Its family: the part of the code before the dot (`pi`, `si`, `rf`, `esd`, `silk`, `dfm`).
    pub fn family(&self) -> &'static str {
        self.code.split_once('.').map_or(self.code, |(f, _)| f)
    }

    pub fn param(&self, name: &str) -> Option<&'static Param> {
        self.params.iter().find(|p| p.name == name)
    }

    /// The highest class a finding of it may have.
    pub fn worst_class(&self) -> Class {
        self.worst.map_or(self.class, |(c, _)| c.min(self.class))
    }

    /// The most severe a finding of it may be.
    pub fn worst_severity(&self) -> Severity {
        self.worst.map_or(self.severity, |(_, s)| s.min(self.severity))
    }
}

/// A module of checks: its codes and the function that checks them (see [`super::Cx`]).
#[derive(Clone, Copy)]
pub struct Family {
    /// The module's name: `pi_return`.
    pub module: &'static str,
    /// What it checks, in a few words, for the list of checks not built yet.
    pub title: &'static str,
    pub codes: &'static [Code],
    pub run: fn(&mut Cx),
}

/// Every family, in the order their checks run and their findings are listed.
pub const FAMILIES: &[Family] = &[
    pi_return::FAMILY,
    pi_decap::FAMILY,
    pi_smps::FAMILY,
    pi_current::FAMILY,
    esd::FAMILY,
    xtal::FAMILY,
    rf::FAMILY,
    pair::FAMILY,
    silk_dfm::FAMILY,
];

/// Every code, family by family.
pub fn codes() -> impl Iterator<Item = &'static Code> {
    FAMILIES.iter().flat_map(|f| f.codes.iter())
}

/// The code `code`.
pub fn find(code: &str) -> Option<&'static Code> {
    codes().find(|c| c.code == code)
}

/// The families: the codes' prefixes, each once, in catalogue order.
pub fn families() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for c in codes() {
        if !out.contains(&c.family()) {
            out.push(c.family());
        }
    }
    out
}

/// The families with no code yet, by title: what the board check lists as not built.
pub fn not_built() -> Vec<&'static str> {
    FAMILIES.iter().filter(|f| f.codes.is_empty()).map(|f| f.title).collect()
}

/// A heading's anchor as the guide links it: lower case, spaces as `-`, other punctuation
/// dropped (`Pad to pour` → `pad-to-pour`, `2. Critical loops` → `2-critical-loops`).
pub fn anchor(heading: &str) -> String {
    let mut out = String::new();
    for ch in heading.trim().chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if (ch == ' ' || ch == '-') && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// The sections every quality code and design rule check code names that the guide's reference
/// files lack, as `code → file#anchor`, then the links between those files that lead nowhere, as
/// `file → link`. `files` holds each reference file's name without `reference/` and `.md`
/// (`power-ground`) and its text; a section resolves when the file has a `#`-heading whose
/// [`anchor`] is the part after `#`. The guide's test (`quality_codes_match_skill_sections`)
/// calls this with the files it ships, so a renamed heading fails there.
pub fn section_misses(files: &[(&str, &str)]) -> Vec<String> {
    let resolves = |section: &str| {
        let Some((file, want)) = section.split_once('#') else { return false };
        files
            .iter()
            .filter(|(name, _)| *name == file)
            .any(|(_, text)| text.lines().filter_map(|l| l.strip_prefix('#')).any(|h| anchor(h.trim_start_matches('#')) == want))
    };
    let quality = codes().map(|c| (c.code, c.section));
    let drc = drc::RULES.iter().map(|r| (r.code, r.section));
    let mut misses: Vec<String> = quality.chain(drc).filter(|(_, s)| !resolves(s)).map(|(code, s)| format!("{code} → {s}")).collect();
    misses.extend(link_misses(files, &resolves));
    misses
}

/// The links in the reference files whose heading isn't there: `power-ground.md#decoupling` or
/// `power-ground#decoupling` (with or without `reference/`), and `#stages` for the file's own,
/// in backticks or as a Markdown link's target. Code blocks are skipped, and so is a `name#x`
/// without `.md` whose name is none of the files.
fn link_misses(files: &[(&str, &str)], resolves: &dyn Fn(&str) -> bool) -> Vec<String> {
    let named = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let mut out = Vec::new();
    for (name, text) in files {
        let mut fenced = false;
        for line in text.lines() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
                continue;
            }
            if fenced {
                continue;
            }
            let spans = line.split('`').skip(1).step_by(2);
            let targets = line.split("](").skip(1).filter_map(|rest| rest.split(')').next());
            for link in spans.chain(targets).map(str::trim) {
                let Some((file, want)) = link.strip_prefix("reference/").unwrap_or(link).split_once('#') else { continue };
                let (file, md) = file.strip_suffix(".md").map_or((file, false), |f| (f, true));
                if !named(want) || !(file.is_empty() || named(file)) {
                    continue;
                }
                let file = if file.is_empty() { *name } else { file };
                if (md || files.iter().any(|(n, _)| *n == file)) && !resolves(&format!("{file}#{want}")) {
                    out.push(format!("{name} → {link}"));
                }
            }
        }
    }
    out
}
