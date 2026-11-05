//! Deterministic document chunking (MEDULLA-REPO-INGEST-R652).
//!
//! One pass over the text splits it into sections at Markdown ATX headings
//! (`#` to `######`), then packs each section into windows of at most
//! [`ChunkingConfig::max_bytes`] UTF-8 bytes. A window ends at the last
//! paragraph break, else the last sentence end, else the last whitespace
//! before the limit. Consecutive windows of one section overlap by about
//! [`ChunkingConfig::overlap_bytes`]; windows never cross a heading. Every
//! chunk carries byte and character offsets into the source text, so a
//! citation can address the exact passage. The output depends only on the
//! text and the config: the same input always yields the same chunks.

use serde::{Deserialize, Serialize};

/// The chunker's version tag, recorded with every enrichment.
pub const CHUNKER_VERSION: &str = "eg-document/heading-paragraph-v1";

/// The largest window the chunker accepts.
pub const MAX_CHUNK_BYTES: usize = 64 * 1024;
/// The smallest window the chunker accepts.
pub const MIN_CHUNK_BYTES: usize = 64;

/// Window size and overlap, in UTF-8 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkingConfig {
    pub max_bytes: usize,
    pub overlap_bytes: usize,
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            max_bytes: 1_200,
            overlap_bytes: 150,
        }
    }
}

impl ChunkingConfig {
    /// Refuse a window outside the accepted range, or an overlap of half the
    /// window or more (which would make no forward progress worth the name).
    pub fn validate(&self) -> Result<(), String> {
        if !(MIN_CHUNK_BYTES..=MAX_CHUNK_BYTES).contains(&self.max_bytes) {
            return Err(format!(
                "CHUNK_CONFIG_INVALID: max_bytes must be in {MIN_CHUNK_BYTES}..={MAX_CHUNK_BYTES}"
            ));
        }
        if self.overlap_bytes * 2 >= self.max_bytes {
            return Err(
                "CHUNK_CONFIG_INVALID: overlap_bytes must be under half of max_bytes".into(),
            );
        }
        Ok(())
    }
}

/// One chunk: its position, its section and its offsets into the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkSpan {
    pub ordinal: u32,
    /// Headings enclosing the chunk, outermost first.
    pub heading_path: Vec<String>,
    pub start_byte: usize,
    pub end_byte: usize,
    /// Unicode scalar offsets, for citations.
    pub start_char: usize,
    pub end_char: usize,
}

/// An inline Markdown link's visible anchor and destination, addressed in the
/// original UTF-8 source by the anchor's half-open byte range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkAnchor {
    pub anchor: String,
    pub target: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

/// Extract inline Markdown links (`[anchor](target)`) in source order.
///
/// Repeated anchor text is retained as separate links. Escaped delimiters and
/// image syntax (`![alt](target)`) are not treated as links. Offsets select the
/// anchor text from the original source, including when it contains non-ASCII
/// characters. Reference-style links are outside this lightweight document pass.
pub fn extract_link_anchors(text: &str) -> Vec<LinkAnchor> {
    let bytes = text.as_bytes();
    let mut links = Vec::new();
    let mut anchor: Option<(usize, usize)> = None;
    let mut target: Option<(usize, usize, usize, usize)> = None;
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index = (index + 2).min(bytes.len());
            continue;
        }

        if let Some((anchor_start, bracket_depth)) = anchor {
            if bytes[index] == b'[' {
                anchor = Some((anchor_start, bracket_depth + 1));
            } else if bytes[index] == b']' {
                if bracket_depth > 1 {
                    anchor = Some((anchor_start, bracket_depth - 1));
                } else if bytes.get(index + 1) == Some(&b'(') {
                    target = Some((anchor_start, index, index + 2, 1));
                    anchor = None;
                    index += 1;
                } else {
                    anchor = None;
                }
            }
        } else if let Some((anchor_start, anchor_end, target_start, paren_depth)) = target {
            if bytes[index] == b'(' {
                target = Some((anchor_start, anchor_end, target_start, paren_depth + 1));
            } else if bytes[index] == b')' {
                if paren_depth > 1 {
                    target = Some((anchor_start, anchor_end, target_start, paren_depth - 1));
                } else {
                    if let (Some(anchor), Some(destination)) = (
                        text.get(anchor_start..anchor_end),
                        text.get(target_start..index),
                    ) {
                        if !anchor.is_empty() && !destination.is_empty() {
                            links.push(LinkAnchor {
                                anchor: anchor.to_owned(),
                                target: destination.to_owned(),
                                start_byte: anchor_start,
                                end_byte: anchor_end,
                            });
                        }
                    }
                    target = None;
                }
            }
        } else if bytes[index] == b'[' && (index == 0 || bytes[index - 1] != b'!') {
            anchor = Some((index + 1, 1));
        }
        index += 1;
    }

    links
}

impl ChunkSpan {
    /// The chunk's text, borrowed from the source it was cut from.
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start_byte..self.end_byte]
    }
}

struct Section {
    heading_path: Vec<String>,
    start: usize,
    end: usize,
}

/// The heading level and title of an ATX heading line, if it is one.
fn heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start_matches(' ');
    let level = trimmed.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&level) || line.len() - trimmed.len() > 3 {
        return None;
    }
    let rest = &trimmed[level..];
    if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
        return None;
    }
    Some((level, rest.trim().trim_end_matches('#').trim_end()))
}

/// Split the text into heading-delimited sections. A heading line belongs to
/// the section it opens, so its title is part of that section's first chunk.
fn sections(text: &str) -> Vec<Section> {
    let mut out = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if let Some((level, title)) = heading(line.trim_end_matches(['\n', '\r'])) {
            if offset > start {
                out.push(Section {
                    heading_path: stack.iter().map(|(_, t)| t.clone()).collect(),
                    start,
                    end: offset,
                });
            }
            while stack.last().is_some_and(|(l, _)| *l >= level) {
                stack.pop();
            }
            stack.push((level, title.to_string()));
            start = offset;
        }
        offset += line.len();
    }
    if text.len() > start {
        out.push(Section {
            heading_path: stack.into_iter().map(|(_, t)| t).collect(),
            start,
            end: text.len(),
        });
    }
    out
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// The best cut in `text[from..limit]`: after the last paragraph break, else
/// after the last sentence end, else at the last whitespace, else `limit`.
/// The cut lies in the window's second half, so every window makes progress.
fn cut_point(text: &str, from: usize, limit: usize) -> usize {
    let floor = from + (limit - from) / 2;
    let window = &text.as_bytes()[floor..limit];
    if let Some(i) = window.windows(2).rposition(|w| w == b"\n\n") {
        return floor + i + 2;
    }
    if let Some(i) = window
        .windows(2)
        .rposition(|w| matches!(w[0], b'.' | b'!' | b'?') && w[1].is_ascii_whitespace())
    {
        return floor + i + 2;
    }
    if let Some(i) = window.iter().rposition(u8::is_ascii_whitespace) {
        return floor + i + 1;
    }
    limit
}

/// Counts characters up to a byte offset. Offsets move forward, and back by
/// at most one overlap, so the total work stays linear in the text.
struct CharCursor<'a> {
    text: &'a str,
    byte: usize,
    chars: usize,
}

impl CharCursor<'_> {
    fn at(&mut self, byte: usize) -> usize {
        if byte >= self.byte {
            self.chars += self.text[self.byte..byte].chars().count();
        } else {
            self.chars -= self.text[byte..self.byte].chars().count();
        }
        self.byte = byte;
        self.chars
    }
}

/// Trim surrounding whitespace from `text[start..end]`, returning the new
/// bounds, or `None` when nothing but whitespace remains.
fn trimmed(text: &str, start: usize, end: usize) -> Option<(usize, usize)> {
    let slice = &text[start..end];
    let lead = slice.len() - slice.trim_start().len();
    let body = slice.trim();
    (!body.is_empty()).then(|| (start + lead, start + lead + body.len()))
}

/// Chunk `text` under `config`. Refuses an invalid config; an empty or
/// whitespace-only text yields no chunks.
pub fn chunk_text(text: &str, config: &ChunkingConfig) -> Result<Vec<ChunkSpan>, String> {
    config.validate()?;
    let mut cursor = CharCursor {
        text,
        byte: 0,
        chars: 0,
    };
    let mut chunks = Vec::new();
    for section in sections(text) {
        let mut from = section.start;
        while from < section.end {
            let limit = floor_boundary(text, (from + config.max_bytes).min(section.end));
            let cut = if limit == section.end {
                limit
            } else {
                floor_boundary(text, cut_point(text, from, limit))
            };
            if let Some((start, end)) = trimmed(text, from, cut) {
                let start_char = cursor.at(start);
                let end_char = cursor.at(end);
                chunks.push(ChunkSpan {
                    ordinal: chunks.len() as u32,
                    heading_path: section.heading_path.clone(),
                    start_byte: start,
                    end_byte: end,
                    start_char,
                    end_char,
                });
            }
            if cut >= section.end {
                break;
            }
            from = next_start(text, from, cut, config.overlap_bytes);
        }
    }
    Ok(chunks)
}

/// Where the next window starts: `overlap` bytes before `cut`, moved forward
/// to a word start, and always after `from`.
fn next_start(text: &str, from: usize, cut: usize, overlap: usize) -> usize {
    if overlap == 0 {
        return cut;
    }
    let back = floor_boundary(text, cut.saturating_sub(overlap).max(from + 1));
    let word = text.as_bytes()[back..cut]
        .iter()
        .position(u8::is_ascii_whitespace)
        .map_or(back, |i| back + i + 1);
    if word > from && word < cut {
        word
    } else {
        cut
    }
}

/// How a document marks its sections (MEDULLA-REPO-INGEST-R652.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionSource {
    /// Markdown ATX headings, `#` to `######`.
    Markdown,
    /// Form-feed (`\x0c`) page breaks.
    Paged,
}

/// One node of a document's section tree (MEDULLA-REPO-INGEST-R652.1).
/// Spans are byte offsets into the source text, `span_start..span_end`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionNode {
    pub section_id: u32,
    pub parent_id: Option<u32>,
    pub title: String,
    pub span_start: usize,
    pub span_end: usize,
    pub depth: u32,
}

/// The outcome of [`verify_section_tree`] (MEDULLA-REPO-INGEST-R652.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionTreeCheck {
    Valid,
    Overlap,
    Gap,
    OutOfBounds,
}

/// The processing evidence recorded for one document (MEDULLA-REPO-INGEST-R652.1).
/// `coverage_milli` is the share of the text covered by leaf sections, in
/// thousandths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessedDocument {
    pub document_id: String,
    pub content_digest: String,
    pub chunk_count: u32,
    pub section_count: u32,
    pub coverage_milli: u32,
    pub tree_check: SectionTreeCheck,
}

struct OpenSection {
    index: usize,
    level: usize,
    has_child: bool,
}

/// Close every open section deeper than or at `level`, ending them at `end`.
fn close_sections(
    nodes: &mut [SectionNode],
    open: &mut Vec<OpenSection>,
    level: usize,
    end: usize,
) {
    while open.last().is_some_and(|o| o.level >= level) {
        let closed = open.pop().expect("checked non-empty");
        nodes[closed.index].span_end = end;
    }
}

/// Push a node under the innermost open section. When that parent gets its
/// first child after some text of its own, the text becomes an intro leaf, so
/// the leaves always cover the whole parent span.
fn open_section(
    nodes: &mut Vec<SectionNode>,
    open: &mut Vec<OpenSection>,
    title: &str,
    level: usize,
    start: usize,
) {
    let parent = open.last_mut().map(|p| {
        let first_child = !p.has_child;
        p.has_child = true;
        (p.index, first_child)
    });
    if let Some((parent_index, true)) = parent {
        let parent_node = nodes[parent_index].clone();
        if start > parent_node.span_start {
            nodes.push(SectionNode {
                section_id: nodes.len() as u32,
                parent_id: Some(parent_node.section_id),
                title: parent_node.title,
                span_start: parent_node.span_start,
                span_end: start,
                depth: parent_node.depth + 1,
            });
        }
    }
    let (parent_id, depth) = match open.last() {
        Some(p) => (Some(nodes[p.index].section_id), nodes[p.index].depth + 1),
        None => (None, 1),
    };
    let index = nodes.len();
    nodes.push(SectionNode {
        section_id: index as u32,
        parent_id,
        title: title.to_string(),
        span_start: start,
        span_end: start,
        depth,
    });
    open.push(OpenSection {
        index,
        level,
        has_child: false,
    });
}

/// Build the section tree of `text` in one pass (MEDULLA-REPO-INGEST-R652.2).
/// Nodes come in pre-order with `section_id` equal to their index. Children
/// nest inside their parent's span, siblings do not overlap and the leaves
/// cover the text. Text before the first heading is a top-level untitled
/// section. An empty text has no sections.
pub fn build_section_tree(text: &str, kind: SectionSource) -> Vec<SectionNode> {
    let mut nodes = Vec::new();
    if text.is_empty() {
        return nodes;
    }
    let mut open: Vec<OpenSection> = Vec::new();
    match kind {
        SectionSource::Markdown => {
            let mut offset = 0;
            for line in text.split_inclusive('\n') {
                if let Some((level, title)) = heading(line.trim_end_matches(['\n', '\r'])) {
                    close_sections(&mut nodes, &mut open, level, offset);
                    open_section(&mut nodes, &mut open, title, level, offset);
                } else if open.is_empty() {
                    // Preamble before the first heading: deeper than any
                    // heading level, so the first heading closes it rather
                    // than nesting under it.
                    open_section(&mut nodes, &mut open, "", 7, offset);
                }
                offset += line.len();
            }
        }
        SectionSource::Paged => {
            let mut offset = 0;
            for (page, body) in text.split_inclusive('\x0c').enumerate() {
                close_sections(&mut nodes, &mut open, 1, offset);
                open_section(
                    &mut nodes,
                    &mut open,
                    &format!("page {}", page + 1),
                    1,
                    offset,
                );
                offset += body.len();
            }
        }
    }
    close_sections(&mut nodes, &mut open, 0, text.len());
    nodes
}

/// Verify the section-tree rule over `nodes` for a text of `text_len` bytes
/// (MEDULLA-REPO-INGEST-R652.2): every span lies in the text, each child lies
/// in its parent, siblings are ordered and disjoint, and the leaves, read in
/// order, cover `0..text_len` with no gap and no overlap. O(n).
pub fn verify_section_tree(nodes: &[SectionNode], text_len: usize) -> SectionTreeCheck {
    let mut last_child_end: Vec<Option<usize>> = vec![None; nodes.len()];
    let mut top_end: Option<usize> = None;
    let mut has_child = vec![false; nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        if node.section_id as usize != index
            || node.span_start > node.span_end
            || node.span_end > text_len
        {
            return SectionTreeCheck::OutOfBounds;
        }
        let previous_end = match node.parent_id {
            Some(parent) => {
                let parent = parent as usize;
                if parent >= index {
                    return SectionTreeCheck::OutOfBounds;
                }
                let p = &nodes[parent];
                if node.span_start < p.span_start || node.span_end > p.span_end {
                    return SectionTreeCheck::Overlap;
                }
                has_child[parent] = true;
                &mut last_child_end[parent]
            }
            None => &mut top_end,
        };
        if previous_end.is_some_and(|end| node.span_start < end) {
            return SectionTreeCheck::Overlap;
        }
        *previous_end = Some(node.span_end);
    }
    let mut covered = 0;
    for (index, node) in nodes.iter().enumerate() {
        if has_child[index] {
            continue;
        }
        if node.span_start > covered {
            return SectionTreeCheck::Gap;
        }
        if node.span_start < covered {
            return SectionTreeCheck::Overlap;
        }
        covered = node.span_end;
    }
    if covered != text_len {
        return SectionTreeCheck::Gap;
    }
    SectionTreeCheck::Valid
}

/// The deepest section containing byte `offset`: the leaf a chunk starting
/// there belongs to (MEDULLA-REPO-INGEST-R652.2).
pub fn section_for(nodes: &[SectionNode], offset: usize) -> Option<u32> {
    nodes
        .iter()
        .filter(|n| n.span_start <= offset && offset < n.span_end)
        .max_by_key(|n| n.depth)
        .map(|n| n.section_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every chunk's char offsets select exactly its byte-offset text.
    fn assert_char_offsets(text: &str, chunks: &[ChunkSpan]) {
        for chunk in chunks {
            let by_chars: String = text
                .chars()
                .skip(chunk.start_char)
                .take(chunk.end_char - chunk.start_char)
                .collect();
            assert_eq!(by_chars, chunk.text(text));
        }
    }

    fn config(max_bytes: usize, overlap_bytes: usize) -> ChunkingConfig {
        ChunkingConfig {
            max_bytes,
            overlap_bytes,
        }
    }

    #[test]
    fn extracts_repeated_and_non_ascii_link_anchors_with_byte_offsets() {
        let text = "[Read](one) and [Read](two), then [café 🧠](three).";
        let links = extract_link_anchors(text);

        assert_eq!(
            links
                .iter()
                .map(|link| (link.anchor.as_str(), link.target.as_str()))
                .collect::<Vec<_>>(),
            vec![("Read", "one"), ("Read", "two"), ("café 🧠", "three")]
        );
        for link in &links {
            assert_eq!(
                text.get(link.start_byte..link.end_byte),
                Some(link.anchor.as_str())
            );
        }
        let non_ascii = &links[2];
        assert_eq!(non_ascii.start_byte, text.find("café 🧠").unwrap());
        assert_eq!(non_ascii.end_byte - non_ascii.start_byte, "café 🧠".len());
        assert_ne!(
            non_ascii.end_byte - non_ascii.start_byte,
            "café 🧠".chars().count()
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R651.7.2
    #[test]
    fn chunks_respect_headings_size_and_offsets() {
        let text = "# Title\n\nIntro paragraph one.\n\n## Part A\n\nAlpha text here.\n\n## Part B\n\nBeta.\n";
        let chunks = chunk_text(text, &ChunkingConfig::default()).unwrap();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].heading_path, vec!["Title"]);
        assert_eq!(chunks[1].heading_path, vec!["Title", "Part A"]);
        assert_eq!(chunks[2].heading_path, vec!["Title", "Part B"]);
        assert!(chunks[1].text(text).starts_with("## Part A"));
        assert_char_offsets(text, &chunks);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R651.7.2
    #[test]
    fn long_sections_split_with_overlap_under_the_limit() {
        let sentence = "Ünïcode sentence number with some words in it. ";
        let text = sentence.repeat(200);
        let chunks = chunk_text(&text, &config(300, 60)).unwrap();
        assert!(chunks.len() > 20);
        for pair in chunks.windows(2) {
            assert!(pair[0].end_byte - pair[0].start_byte <= 300);
            assert!(
                pair[1].start_byte < pair[0].end_byte,
                "consecutive windows overlap"
            );
            assert!(pair[1].start_byte > pair[0].start_byte, "windows advance");
        }
        assert_eq!(chunks.last().unwrap().end_byte, text.trim_end().len());
        assert_char_offsets(&text, &chunks);
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R651.7.2
    #[test]
    fn chunking_is_deterministic_and_paragraph_aware() {
        let text = format!("{}\n\n{}", "a".repeat(500), "b ".repeat(400));
        let first = chunk_text(&text, &config(800, 100)).unwrap();
        assert_eq!(first, chunk_text(&text, &config(800, 100)).unwrap());
        assert_eq!(
            first[0].text(&text),
            "a".repeat(500),
            "cut at the paragraph break"
        );
    }

    // spec: MEDULLA-UNIFIED-DATA-PLANE-R651.7.4
    #[test]
    fn invalid_configs_and_blank_text() {
        assert!(chunk_text("x", &config(10, 0)).is_err());
        assert!(chunk_text("x", &config(200, 100)).is_err());
        assert!(chunk_text(" \n\n ", &ChunkingConfig::default())
            .unwrap()
            .is_empty());
        let unbroken = "x".repeat(5_000);
        let chunks = chunk_text(&unbroken, &config(1_000, 0)).unwrap();
        assert_eq!(chunks.len(), 5, "a text with no break cuts at the limit");
    }
}
