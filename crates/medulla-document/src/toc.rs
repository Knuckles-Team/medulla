//! Source table of contents build and span location (MEDULLA-DECISION-ENGINE-R153.2).
//!
//! [`build_toc`] makes one pass over the source (O(size)) and emits one entry per
//! ATX heading (`#` to `######`); a source without headings gets one entry that
//! covers every line. [`locate_span`] seeks to the nearest `line_index`
//! checkpoint at or before the first line and scans forward at most `stride`
//! lines plus the span, so a read costs O(stride + span).

use medulla_types::source_toc::{SourceToc, TocEntry, SOURCE_TOC_SCHEMA_VERSION};

use crate::header::content_hash;

/// Byte offset of the start of every line, plus the source length as a sentinel.
fn line_starts(source: &[u8]) -> Vec<u64> {
    let mut starts = vec![0u64];
    for (i, b) in source.iter().enumerate() {
        if *b == b'\n' && i + 1 < source.len() {
            starts.push((i + 1) as u64);
        }
    }
    starts
}

/// The heading level and title of an ATX heading line, if it is one.
fn atx_heading(line: &[u8]) -> Option<(u8, String)> {
    let text = std::str::from_utf8(line).ok()?;
    let text = text.trim_end_matches(['\n', '\r']);
    let indent = text.len() - text.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &text[indent..];
    let level = rest.bytes().take_while(|b| *b == b'#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let after = &rest[level..];
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return None;
    }
    let title = after.trim().trim_end_matches('#').trim_end().to_string();
    Some((level as u8, title))
}

/// Build the table of contents of one source revision. `stride` is the
/// `toc.line_index_stride` knob (lines per `line_index` checkpoint, ≥ 1).
pub fn build_toc(document_id: &str, revision: u64, source: &[u8], stride: u64) -> SourceToc {
    let stride = stride.max(1);
    let source_len = source.len() as u64;
    let starts = if source.is_empty() {
        Vec::new()
    } else {
        line_starts(source)
    };
    let total_lines = starts.len() as u64;
    let line_end_byte = |line: u64| -> u64 {
        // `line` is 1-based; its end is the next line's start or the source end.
        starts.get(line as usize).copied().unwrap_or(source_len)
    };

    let mut headings: Vec<(u64, u8, String)> = Vec::new();
    for (i, start) in starts.iter().enumerate() {
        let line_no = i as u64 + 1;
        let line = &source[*start as usize..line_end_byte(line_no) as usize];
        if let Some((level, title)) = atx_heading(line) {
            headings.push((line_no, level, title));
        }
    }

    let mut entries = Vec::new();
    if headings.is_empty() {
        if total_lines > 0 {
            entries.push(TocEntry {
                section_id: "s0".into(),
                title: document_id.to_string(),
                level: 0,
                line_start: 1,
                line_end: total_lines,
                byte_start: 0,
                byte_end: source_len,
            });
        }
    } else {
        if headings[0].0 > 1 {
            // Preamble before the first heading.
            let line_end = headings[0].0 - 1;
            entries.push(TocEntry {
                section_id: "s0".into(),
                title: document_id.to_string(),
                level: 0,
                line_start: 1,
                line_end,
                byte_start: 0,
                byte_end: line_end_byte(line_end),
            });
        }
        for (n, (line_start, level, title)) in headings.iter().enumerate() {
            let line_end = headings
                .get(n + 1)
                .map(|next| next.0 - 1)
                .unwrap_or(total_lines);
            entries.push(TocEntry {
                section_id: format!("s{}", n + 1),
                title: title.clone(),
                level: *level,
                line_start: *line_start,
                line_end,
                byte_start: starts[(*line_start - 1) as usize],
                byte_end: line_end_byte(line_end),
            });
        }
    }

    let line_index = starts.iter().step_by(stride as usize).copied().collect();
    SourceToc {
        document_id: document_id.to_string(),
        revision,
        source_digest: content_hash(source),
        total_lines,
        entries,
        line_index,
        line_index_stride: stride,
        schema_version: SOURCE_TOC_SCHEMA_VERSION,
    }
}

/// Find the byte range `[start, end)` of lines `line_start..=line_end` (1-based)
/// using the table's `line_index` checkpoints, scanning forward from the nearest
/// checkpoint at or before `line_start`. Returns `None` for an empty, inverted or
/// out-of-range span; the caller refuses those before reading.
pub fn locate_span(
    toc: &SourceToc,
    source: &[u8],
    stride: u64,
    line_start: u64,
    line_end: u64,
) -> Option<(u64, u64)> {
    let stride = stride.max(1);
    if line_start == 0 || line_end < line_start || line_end > toc.total_lines {
        return None;
    }
    let checkpoint = ((line_start - 1) / stride) as usize;
    let mut offset = *toc.line_index.get(checkpoint)? as usize;
    let mut line = checkpoint as u64 * stride + 1;
    // Advance to the end of line `target`, returning the next line's start.
    let advance_to = |target: u64, offset: &mut usize, line: &mut u64| {
        while *line < target {
            match source[*offset..].iter().position(|b| *b == b'\n') {
                Some(nl) => *offset += nl + 1,
                None => *offset = source.len(),
            }
            *line += 1;
        }
    };
    advance_to(line_start, &mut offset, &mut line);
    let start = offset;
    advance_to(line_end + 1, &mut offset, &mut line);
    Some((start as u64, offset as u64))
}

/// Where a source text and its table are stored, content-addressed. `put`
/// returns the SHA-256 hex digest of the stored bytes.
pub trait TocSink {
    /// Store `bytes` and return their SHA-256 hex digest.
    fn put(&mut self, bytes: &[u8]) -> Result<String, String>;
}

/// The result of staging one text source: its table and the digests of the two
/// stored blobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedToc {
    /// The table that was stored.
    pub toc: SourceToc,
    /// SHA-256 hex digest of the stored table (its JSON bytes).
    pub toc_digest: String,
    /// SHA-256 hex digest of the stored source text.
    pub source_digest: String,
}

impl StagedToc {
    /// The node properties that bind a document node to its stored table, text
    /// and revision.
    pub fn node_properties(&self) -> [(&'static str, serde_json::Value); 4] {
        use medulla_types::source_toc::{
            SOURCE_DIGEST_PROPERTY, SOURCE_REVISION_PROPERTY, SOURCE_TOTAL_LINES_PROPERTY,
            TOC_DIGEST_PROPERTY,
        };
        [
            (TOC_DIGEST_PROPERTY, self.toc_digest.clone().into()),
            (SOURCE_DIGEST_PROPERTY, self.source_digest.clone().into()),
            (SOURCE_REVISION_PROPERTY, self.toc.revision.into()),
            (SOURCE_TOTAL_LINES_PROPERTY, self.toc.total_lines.into()),
        ]
    }
}

/// Build the table of one text source revision and store the source text and
/// the table (MEDULLA-DECISION-ENGINE-R153.3). The text is stored first so the
/// table's `source_digest` names a blob that exists.
pub fn build_and_store_toc(
    sink: &mut dyn TocSink,
    document_id: &str,
    revision: u64,
    source: &[u8],
    stride: u64,
) -> Result<StagedToc, String> {
    let toc = build_toc(document_id, revision, source, stride);
    let source_digest = sink.put(source)?;
    if source_digest != toc.source_digest {
        return Err(format!(
            "source text digest {source_digest} differs from the table digest {}",
            toc.source_digest
        ));
    }
    let bytes = serde_json::to_vec(&toc).map_err(|e| format!("toc encoding failed: {e}"))?;
    let toc_digest = sink.put(&bytes)?;
    Ok(StagedToc {
        toc,
        toc_digest,
        source_digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "preamble line\n# Title\nintro text\n\n## Part A\na1\na2\n### Deep\nd1\n## Part B\nb1\nlast line without newline";

    // spec: MEDULLA-DECISION-ENGINE-R153.2
    #[test]
    fn toc_offsets_slice_the_original_bytes() {
        let bytes = FIXTURE.as_bytes();
        let toc = build_toc("doc", 1, bytes, 16);
        toc.validate().unwrap();
        let lines: Vec<&str> = FIXTURE.split_inclusive('\n').collect();
        assert_eq!(toc.total_lines, lines.len() as u64);
        let titles: Vec<&str> = toc.entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["doc", "Title", "Part A", "Deep", "Part B"]);
        let mut rebuilt = Vec::new();
        for entry in &toc.entries {
            let slice = &bytes[entry.byte_start as usize..entry.byte_end as usize];
            let expected: String =
                lines[(entry.line_start - 1) as usize..entry.line_end as usize].concat();
            assert_eq!(slice, expected.as_bytes(), "entry {}", entry.section_id);
            rebuilt.extend_from_slice(slice);
        }
        assert_eq!(rebuilt, bytes);
        assert_eq!(toc.source_digest, content_hash(bytes));

        let plain = build_toc("plain", 1, b"a\nb\nc\n", 16);
        assert_eq!(plain.entries.len(), 1);
        assert_eq!(
            (plain.entries[0].line_start, plain.entries[0].line_end),
            (1, 3)
        );
        assert_eq!(plain.entries[0].byte_end, 6);
    }

    // spec: MEDULLA-DECISION-ENGINE-R153.2
    #[test]
    fn locate_span_matches_a_full_scan() {
        let source: String = (1..=500)
            .map(|i| format!("line {i} {}\n", "x".repeat(i % 7)))
            .collect();
        let bytes = source.as_bytes();
        let lines: Vec<&str> = source.split_inclusive('\n').collect();
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |bound: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % bound
        };
        for stride in [16u64, 64, 1024] {
            let toc = build_toc("doc", 1, bytes, stride);
            for _ in 0..200 {
                let a = next(500) + 1;
                let b = a + next(501 - a);
                let (start, end) = locate_span(&toc, bytes, stride, a, b).unwrap();
                let expected: String = lines[(a - 1) as usize..b as usize].concat();
                assert_eq!(&bytes[start as usize..end as usize], expected.as_bytes());
            }
            assert!(locate_span(&toc, bytes, stride, 0, 1).is_none());
            assert!(locate_span(&toc, bytes, stride, 5, 4).is_none());
            assert!(locate_span(&toc, bytes, stride, 1, 501).is_none());
        }
    }
}
