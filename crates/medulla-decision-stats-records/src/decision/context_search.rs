//! MEDULLA-DECISION-ENGINE-R100.100.3: the context-search offset kernel.
//!
//! Matching runs on NFC-normalized text, but every public offset addresses
//! the ORIGINAL bytes. The original is split before each character with
//! combining class 0; each segment is normalized on its own and its original
//! byte range is recorded in a map. A match in the normalized text is carried
//! back to the original by snapping outward to whole segments, so an offset
//! always lands on a character boundary of the text its digest names.
//! `validate_map` proves the map before any offset is trusted.

use std::collections::BTreeSet;

use medulla_decision_policy_types::decision::knob_families::context_search::ContextSearchKnobs;
use medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode;
use unicode_normalization::char::canonical_combining_class;
use unicode_normalization::UnicodeNormalization;
use {
    medulla_decision_policy_types::decision::statistical::context_search::sha256_hex,
    medulla_decision_policy_types::decision::statistical::context_search::text_digest,
    medulla_decision_policy_types::decision::statistical::context_search::ContextSpan,
};

/// The `context_search` family refusal table.
pub const REFUSALS: &[StatisticalErrorCode] = &[
    StatisticalErrorCode::ContextMapInvalid,
    StatisticalErrorCode::ContextQueryEmpty,
    StatisticalErrorCode::ContextSpanTooLarge,
];

/// One normalized segment and the original bytes it came from, half-open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentRange {
    pub norm_start: usize,
    pub norm_end: usize,
    pub orig_start: usize,
    pub orig_end: usize,
}

/// A document's normalized text with the map back to its original bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextIndex {
    pub normalized: String,
    pub map: Vec<SegmentRange>,
    /// SHA-256 (64 lowercase hex characters) of the normalized text plus the map.
    pub index_digest: String,
}

/// The start offsets of the segments of `text`: offset 0 and every character
/// with combining class 0 after it.
fn segment_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (offset, ch) in text.char_indices().skip(1) {
        if canonical_combining_class(ch) == 0 {
            starts.push(offset);
        }
    }
    starts
}

/// NFC of `text`, normalizing each segment on its own. The query side and the
/// document side use this one function, so the same characters always meet.
pub fn normalize(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let starts = segment_starts(text);
    let mut out = String::with_capacity(text.len());
    for (index, start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(text.len());
        out.extend(text[*start..end].nfc());
    }
    out
}

fn invalid(detail: impl std::fmt::Display) -> String {
    StatisticalErrorCode::ContextMapInvalid.refusal(detail)
}

/// Prove `map` against the texts: segments ordered and contiguous on both
/// sides, together covering both texts, every offset on a character boundary
/// and inside its text, and each segment's normalization equal to its
/// normalized range.
pub fn validate_map(original: &str, normalized: &str, map: &[SegmentRange]) -> Result<(), String> {
    if original.is_empty() != map.is_empty() {
        return Err(invalid("the map is empty exactly when the text is not"));
    }
    let (mut norm_at, mut orig_at) = (0usize, 0usize);
    for (index, entry) in map.iter().enumerate() {
        if entry.norm_start != norm_at || entry.orig_start != orig_at {
            return Err(invalid(format!("segment {index} is not contiguous")));
        }
        if entry.norm_end <= entry.norm_start
            || entry.orig_end <= entry.orig_start
            || entry.norm_end > normalized.len()
            || entry.orig_end > original.len()
        {
            return Err(invalid(format!("segment {index} is empty or out of range")));
        }
        if !normalized.is_char_boundary(entry.norm_start)
            || !normalized.is_char_boundary(entry.norm_end)
            || !original.is_char_boundary(entry.orig_start)
            || !original.is_char_boundary(entry.orig_end)
        {
            return Err(invalid(format!("segment {index} splits a character")));
        }
        let renormalized: String = original[entry.orig_start..entry.orig_end].nfc().collect();
        if renormalized != normalized[entry.norm_start..entry.norm_end] {
            return Err(invalid(format!(
                "segment {index} does not normalize to its range"
            )));
        }
        norm_at = entry.norm_end;
        orig_at = entry.orig_end;
    }
    if norm_at != normalized.len() || orig_at != original.len() {
        return Err(invalid("the map does not cover the text"));
    }
    Ok(())
}

fn index_digest(normalized: &str, map: &[SegmentRange]) -> String {
    let rows: String = map
        .iter()
        .map(|e| {
            format!(
                "{},{},{},{}\n",
                e.norm_start, e.norm_end, e.orig_start, e.orig_end
            )
        })
        .collect();
    sha256_hex(&[normalized.as_bytes(), &[0u8], rows.as_bytes()])
}

/// Normalize `original`, build and validate its map and digest the result.
pub fn index_text(original: &str) -> Result<ContextIndex, String> {
    let starts = segment_starts(original);
    let mut normalized = String::with_capacity(original.len());
    let mut map = Vec::new();
    if !original.is_empty() {
        for (index, start) in starts.iter().enumerate() {
            let end = starts.get(index + 1).copied().unwrap_or(original.len());
            let norm_start = normalized.len();
            normalized.extend(original[*start..end].nfc());
            map.push(SegmentRange {
                norm_start,
                norm_end: normalized.len(),
                orig_start: *start,
                orig_end: end,
            });
        }
    }
    validate_map(original, &normalized, &map)?;
    let index_digest = index_digest(&normalized, &map);
    Ok(ContextIndex {
        normalized,
        map,
        index_digest,
    })
}

/// The n-grams of `query`: its whitespace-separated tokens, normalized, in
/// windows of `ngram_n` joined with single spaces. A query shorter than
/// `ngram_n` is one gram of all its tokens. An empty query refuses.
pub fn query_ngrams(query: &str, ngram_n: u32) -> Result<Vec<String>, String> {
    let tokens: Vec<String> = query.split_whitespace().map(normalize).collect();
    if tokens.is_empty() {
        return Err(StatisticalErrorCode::ContextQueryEmpty.refusal("the query has no tokens"));
    }
    let width = (ngram_n.max(1) as usize).min(tokens.len());
    let grams: BTreeSet<String> = tokens.windows(width).map(|w| w.join(" ")).collect();
    Ok(grams.into_iter().collect())
}

/// Snap `offset` down to the nearest character boundary of `text`.
fn snap_down(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Snap `offset` up to the nearest character boundary of `text`.
fn snap_up(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

/// Every match of `grams` in one document, as spans over its original text in
/// match order. A span longer than `knobs.max_span_bytes` refuses.
pub fn search_document(
    document_id: &str,
    document_revision: u64,
    original: &str,
    grams: &[String],
    knobs: &ContextSearchKnobs,
) -> Result<Vec<ContextSpan>, String> {
    let index = index_text(original)?;
    let digest = text_digest(original);
    let mut matches: BTreeSet<(usize, usize)> = BTreeSet::new();
    for gram in grams.iter().filter(|gram| !gram.is_empty()) {
        for (norm_start, found) in index.normalized.match_indices(gram.as_str()) {
            let norm_end = norm_start + found.len();
            let first = index.map.partition_point(|e| e.norm_end <= norm_start);
            let last = index.map.partition_point(|e| e.norm_end < norm_end);
            matches.insert((index.map[first].orig_start, index.map[last].orig_end));
        }
    }
    let mut spans = Vec::with_capacity(matches.len());
    for (match_start, match_end) in matches {
        let context_start = snap_down(
            original,
            match_start.saturating_sub(knobs.context_bytes as usize),
        );
        let context_end = snap_up(
            original,
            match_end.saturating_add(knobs.context_bytes as usize),
        );
        if context_end - context_start > knobs.max_span_bytes as usize {
            return Err(StatisticalErrorCode::ContextSpanTooLarge.refusal(format!(
                "document={document_id} span={} limit {}",
                context_end - context_start,
                knobs.max_span_bytes
            )));
        }
        spans.push(ContextSpan {
            document_id: document_id.to_string(),
            document_revision,
            text_digest: digest.clone(),
            match_start: match_start as u64,
            match_end: match_end as u64,
            context_start: context_start as u64,
            context_end: context_end as u64,
            context_text: original[context_start..context_end].to_string(),
        });
    }
    Ok(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOBS: ContextSearchKnobs = ContextSearchKnobs {
        context_bytes: 160,
        ngram_n: 2,
        max_spans: 100,
        max_span_bytes: 8192,
    };

    fn spans(text: &str, query: &str, n: u32, knobs: &ContextSearchKnobs) -> Vec<ContextSpan> {
        let grams = query_ngrams(query, n).unwrap();
        search_document("doc", 7, text, &grams, knobs).unwrap()
    }

    fn matched<'a>(text: &'a str, span: &ContextSpan) -> &'a str {
        &text[span.match_start as usize..span.match_end as usize]
    }

    fn code(error: String) -> String {
        error.split(':').next().unwrap().to_string()
    }

    // spec: MEDULLA-DECISION-ENGINE-R100.100.3
    #[test]
    fn offsets_slice_the_original_bytes_for_composed_and_decomposed_text() {
        let tight = ContextSearchKnobs {
            context_bytes: 0,
            ..KNOBS
        };
        // Composed text, decomposed query: both normalize to the same bytes.
        let composed = "un caf\u{e9} noir, un th\u{e9} vert";
        let found = spans(composed, "cafe\u{301}", 1, &tight);
        assert_eq!(found.len(), 1);
        assert_eq!(matched(composed, &found[0]), "caf\u{e9}");
        assert_eq!(found[0].text_digest, text_digest(composed));
        assert_eq!(found[0].document_revision, 7);

        // Decomposed text, composed query: the original offsets cover the
        // three-byte decomposed form (the combining accent is two bytes).
        let decomposed = "un cafe\u{301} noir";
        let found = spans(decomposed, "caf\u{e9}", 1, &tight);
        assert_eq!(found.len(), 1);
        assert_eq!(matched(decomposed, &found[0]), "cafe\u{301}");
        assert_eq!(found[0].match_end - found[0].match_start, 6);

        // A bigram across multibyte text; the second match is found too.
        let text = "\u{65e5}\u{672c}\u{8a9e} na\u{ef}ve caf\u{e9} et na\u{ef}ve caf\u{e9}";
        let found = spans(text, "na\u{ef}ve caf\u{e9}", 2, &tight);
        assert_eq!(found.len(), 2);
        for span in &found {
            assert_eq!(matched(text, span), "na\u{ef}ve caf\u{e9}");
        }
        assert!(found[0].match_start < found[1].match_start);

        // Context extends the match by `context_bytes`, then snaps to character
        // boundaries: 1 byte either side of a match between two 3-byte
        // characters widens to the whole characters.
        let wide = "\u{65e5}x\u{672c}";
        let one = ContextSearchKnobs {
            context_bytes: 1,
            ..KNOBS
        };
        let found = spans(wide, "x", 1, &one);
        assert_eq!(matched(wide, &found[0]), "x");
        assert_eq!(found[0].context_start, 0);
        assert_eq!(found[0].context_end as usize, wide.len());
        assert_eq!(found[0].context_text, wide);
        let three = ContextSearchKnobs {
            context_bytes: 160,
            ..KNOBS
        };
        let found = spans(composed, "caf\u{e9}", 1, &three);
        assert_eq!(
            (found[0].context_start, found[0].context_end as usize),
            (0, composed.len())
        );

        // A span longer than the knob refuses; an empty query refuses.
        let small = ContextSearchKnobs {
            max_span_bytes: 4,
            ..KNOBS
        };
        let grams = query_ngrams("caf\u{e9}", 1).unwrap();
        let refused = search_document("doc", 1, composed, &grams, &small).unwrap_err();
        assert_eq!(code(refused), "CONTEXT_SPAN_TOO_LARGE");
        assert_eq!(
            code(query_ngrams("  \t ", 2).unwrap_err()),
            "CONTEXT_QUERY_EMPTY"
        );

        // The index digest pins the normalized text and the map.
        let a = index_text(decomposed).unwrap();
        let b = index_text("un caf\u{e9} noir").unwrap();
        assert_eq!(a.normalized, b.normalized);
        assert_ne!(a.index_digest, b.index_digest, "the maps differ");
        assert_eq!(a.index_digest.len(), 64);

        // A corrupt map is refused: a gap, a split character, a wrong range,
        // a range past the text, and a map that stops short.
        let index = index_text(composed).unwrap();
        let corrupt = |edit: &dyn Fn(&mut Vec<SegmentRange>)| {
            let mut map = index.map.clone();
            edit(&mut map);
            code(validate_map(composed, &index.normalized, &map).unwrap_err())
        };
        assert!(validate_map(composed, &index.normalized, &index.map).is_ok());
        let e_acute = composed.find('\u{e9}').unwrap();
        let at = index
            .map
            .iter()
            .position(|e| e.orig_start == e_acute)
            .unwrap();
        assert_eq!(corrupt(&|m| m[at].orig_end += 1), "CONTEXT_MAP_INVALID");
        assert_eq!(corrupt(&|m| m[at].orig_start += 1), "CONTEXT_MAP_INVALID");
        assert_eq!(corrupt(&|m| m[at].norm_end += 1), "CONTEXT_MAP_INVALID");
        // Consistent on both neighbours, but the shared edge lies inside the
        // two-byte character.
        assert_eq!(
            corrupt(&|m| {
                m[at].orig_end = e_acute + 1;
                m[at + 1].orig_start = e_acute + 1;
            }),
            "CONTEXT_MAP_INVALID"
        );
        assert_eq!(corrupt(&|m| m[0].orig_end = 10_000), "CONTEXT_MAP_INVALID");
        assert_eq!(corrupt(&|m| m.swap(1, 2)), "CONTEXT_MAP_INVALID");
        assert_eq!(
            corrupt(&|m| {
                m.pop();
            }),
            "CONTEXT_MAP_INVALID"
        );
        assert_eq!(corrupt(&|m| m.clear()), "CONTEXT_MAP_INVALID");
        assert_eq!(REFUSALS.len(), 3);
    }
}
