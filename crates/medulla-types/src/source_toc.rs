//! Source table of contents (MEDULLA-DECISION-ENGINE-R153.1).
//!
//! A [`SourceToc`] maps the sections of one ingested source revision to line and
//! byte ranges. Lines are 1-based and inclusive. Byte offsets are zero-based with
//! an exclusive end. `line_index` holds the byte offset of every
//! `toc.line_index_stride`-th line, so a span read seeks to the nearest
//! checkpoint instead of scanning the whole source.

use crate::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};

/// Every refusal code of the `ReadSpan` family
/// (MEDULLA-DECISION-ENGINE-R153.4). Collected by
/// `medulla_decision_types::decision::codes::module_tables`.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("SPAN_INVERTED"),
    RefusalCode::new("SPAN_OUT_OF_BOUNDS"),
    RefusalCode::new("SPAN_TOO_LARGE"),
    RefusalCode::new("TOC_MISSING"),
];

/// Current wire schema version of [`SourceToc`].
pub const SOURCE_TOC_SCHEMA_VERSION: u32 = 1;

/// One section of a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TocEntry {
    /// Stable section id within the revision.
    pub section_id: String,
    /// Section title (heading text, or the document id for a source without headings).
    pub title: String,
    /// Heading level, 1 to 6; 0 for the whole-source entry.
    pub level: u8,
    /// First line, 1-based, inclusive.
    pub line_start: u64,
    /// Last line, 1-based, inclusive.
    pub line_end: u64,
    /// First byte, zero-based.
    pub byte_start: u64,
    /// End byte, zero-based, exclusive.
    pub byte_end: u64,
}

/// Why a table of contents is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceTocError {
    /// An entry's `line_end` is below its `line_start`.
    InvertedLines {
        /// The offending section.
        section_id: String,
    },
    /// An entry's `byte_end` is below its `byte_start`.
    InvertedBytes {
        /// The offending section.
        section_id: String,
    },
    /// An entry reaches past `total_lines`, or starts at line 0.
    LinesOutOfRange {
        /// The offending section.
        section_id: String,
    },
    /// The checkpoint stride is zero.
    ZeroStride,
    /// The schema version is not the current one.
    SchemaVersion(u32),
}

impl std::fmt::Display for SourceTocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvertedLines { section_id } => {
                write!(f, "toc entry {section_id}: line_end below line_start")
            }
            Self::InvertedBytes { section_id } => {
                write!(f, "toc entry {section_id}: byte_end below byte_start")
            }
            Self::LinesOutOfRange { section_id } => {
                write!(f, "toc entry {section_id}: lines outside 1..=total_lines")
            }
            Self::ZeroStride => write!(f, "toc line_index_stride must be at least 1"),
            Self::SchemaVersion(v) => write!(f, "unsupported toc schema version {v}"),
        }
    }
}

impl std::error::Error for SourceTocError {}

/// The table of contents of one source revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceToc {
    /// Document the table belongs to.
    pub document_id: String,
    /// Source revision the table was built from.
    pub revision: u64,
    /// SHA-256 hex digest of the source bytes.
    pub source_digest: String,
    /// Number of lines in the source.
    pub total_lines: u64,
    /// Sections in source order.
    pub entries: Vec<TocEntry>,
    /// Byte offset of lines 1, 1 + stride, 1 + 2 * stride, ...
    pub line_index: Vec<u64>,
    /// Lines between two `line_index` checkpoints, as built. Stored so a read
    /// stays correct after the `toc.line_index_stride` knob changes.
    pub line_index_stride: u64,
    /// Wire schema version.
    pub schema_version: u32,
}

impl SourceToc {
    /// Check every entry's ranges and the schema version.
    pub fn validate(&self) -> Result<(), SourceTocError> {
        if self.schema_version != SOURCE_TOC_SCHEMA_VERSION {
            return Err(SourceTocError::SchemaVersion(self.schema_version));
        }
        if self.line_index_stride == 0 {
            return Err(SourceTocError::ZeroStride);
        }
        for entry in &self.entries {
            if entry.line_end < entry.line_start {
                return Err(SourceTocError::InvertedLines {
                    section_id: entry.section_id.clone(),
                });
            }
            if entry.byte_end < entry.byte_start {
                return Err(SourceTocError::InvertedBytes {
                    section_id: entry.section_id.clone(),
                });
            }
            if entry.line_start == 0 || entry.line_end > self.total_lines {
                return Err(SourceTocError::LinesOutOfRange {
                    section_id: entry.section_id.clone(),
                });
            }
        }
        Ok(())
    }

    /// Decode and validate a table from JSON.
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        let toc: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        toc.validate().map_err(|e| e.to_string())?;
        Ok(toc)
    }
}

/// Node property naming the stored table of contents blob (SHA-256 hex).
pub const TOC_DIGEST_PROPERTY: &str = "toc_digest";
/// Node property naming the stored source text blob (SHA-256 hex).
pub const SOURCE_DIGEST_PROPERTY: &str = "source_digest";
/// Node property holding the revision the stored table and text belong to.
pub const SOURCE_REVISION_PROPERTY: &str = "source_revision";
/// Node property holding the line count of the stored text, so a span past the
/// end is refused before any blob is read (MEDULLA-DECISION-ENGINE-R153.4).
pub const SOURCE_TOTAL_LINES_PROPERTY: &str = "source_total_lines";
/// Record payload field that carries the text of a text source.
pub const SOURCE_TEXT_FIELD: &str = "source_text";

/// The answer of one `ReadSpan` (MEDULLA-DECISION-ENGINE-R153.3, R153.5).
///
/// Names the digests of the table and of the source it was built from, so a
/// caller can bind the span to both. Lines are 1-based and inclusive; the byte
/// range is zero-based with an exclusive end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadSpanResult {
    /// Document the span was read from.
    pub document_id: String,
    /// Source revision the table and text belong to.
    pub revision: u64,
    /// SHA-256 hex digest of the stored table of contents.
    pub toc_digest: String,
    /// SHA-256 hex digest of the stored source text.
    pub source_digest: String,
    /// First served line, 1-based.
    pub line_start: u64,
    /// Last served line, 1-based, inclusive.
    pub line_end: u64,
    /// First served byte, zero-based.
    pub byte_start: u64,
    /// End of the served bytes, zero-based, exclusive.
    pub byte_end: u64,
    /// The served text.
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SourceToc {
        SourceToc {
            document_id: "doc-1".into(),
            revision: 3,
            source_digest: "ab".repeat(32),
            total_lines: 10,
            entries: vec![TocEntry {
                section_id: "s1".into(),
                title: "Intro".into(),
                level: 1,
                line_start: 1,
                line_end: 10,
                byte_start: 0,
                byte_end: 120,
            }],
            line_index: vec![0],
            line_index_stride: 64,
            schema_version: SOURCE_TOC_SCHEMA_VERSION,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R153.1
    #[test]
    fn toc_round_trip() {
        let toc = sample();
        let bytes = serde_json::to_vec(&toc).unwrap();
        assert_eq!(SourceToc::from_json(&bytes).unwrap(), toc);

        let mut bad = sample();
        bad.entries[0].line_start = 5;
        bad.entries[0].line_end = 4;
        assert_eq!(
            bad.validate(),
            Err(SourceTocError::InvertedLines {
                section_id: "s1".into()
            })
        );
        let bytes = serde_json::to_vec(&bad).unwrap();
        assert!(SourceToc::from_json(&bytes).is_err());
    }
}
