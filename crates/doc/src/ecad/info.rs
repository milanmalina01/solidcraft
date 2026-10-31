//! What a print's title block and cover say about the design: its title, revision, date, author,
//! company, a short note and the revision history, each the user's own text (the dates too:
//! nothing here reads the clock). A field left empty falls back to the design's properties where
//! it has one (project, revision, author, organization), then to nothing.

use serde::{Deserialize, Serialize};

use super::{MAX_DESCRIPTION, MAX_TEXT, check_text};

/// The most revisions the history keeps.
pub const MAX_REVISIONS: usize = 50;

/// The title block's fields, saved under `ecad.info`; not saved while all are empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DesignInfo {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub revision: String,
    /// As the user writes it (`2026-10-29`, `29 Oct 2026`…).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub date: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub author: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub company: String,
    /// A short note printed in the title block.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// The revision history, oldest first (the cover prints it newest first).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub revisions: Vec<Revision>,
}

/// One revision of the design: its name (`A`, `1.1`), when, by whom and what changed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub rev: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub date: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl Revision {
    /// A name, then each field within its length ([`MAX_TEXT`], the note [`MAX_DESCRIPTION`]), on
    /// one line.
    pub fn validate(&self) -> Result<(), String> {
        if self.rev.trim().is_empty() {
            return Err("a revision needs `rev`, its name (such as A or 1.1)".into());
        }
        for (what, text, max) in
            [("rev", &self.rev, MAX_TEXT), ("date", &self.date, MAX_TEXT), ("by", &self.by, MAX_TEXT), ("note", &self.note, MAX_DESCRIPTION)]
        {
            check_text(what, text, max)?;
        }
        Ok(())
    }
}

impl DesignInfo {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Each field within its length ([`MAX_TEXT`], the note [`MAX_DESCRIPTION`]), on one line, and
    /// at most [`MAX_REVISIONS`] revisions, each valid.
    pub fn validate(&self) -> Result<(), String> {
        for (what, text, max) in [
            ("title", &self.title, MAX_TEXT),
            ("revision", &self.revision, MAX_TEXT),
            ("date", &self.date, MAX_TEXT),
            ("author", &self.author, MAX_TEXT),
            ("company", &self.company, MAX_TEXT),
            ("notes", &self.notes, MAX_DESCRIPTION),
        ] {
            check_text(what, text, max)?;
        }
        if self.revisions.len() > MAX_REVISIONS {
            return Err(format!("`revisions` holds at most {MAX_REVISIONS} revisions (got {})", self.revisions.len()));
        }
        for (k, r) in self.revisions.iter().enumerate() {
            r.validate().map_err(|e| format!("`revisions[{k}]`: {e}"))?;
        }
        Ok(())
    }
}
