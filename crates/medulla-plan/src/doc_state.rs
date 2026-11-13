//! Reusable compressed document state with decoding-free reranking
//! (MEDULLA-FEDERATED-QUERY-R081.101.2 and .3).
//!
//! `build_document_state` runs once per immutable content digest, at the ingest
//! worker's document commit. `rerank_with_state` scores a query projection against
//! the stored projections by max-over-segments dot product. The [`StateEncoder`]
//! trait has no decode or generate method, so a rerank can make no decoder call.
//! Nothing here writes: query time never alters a state.

use std::collections::BTreeMap;

use medulla_result_types::retrieval_round::{sha256_hex_of, StateResult, StateScoreRow};
use medulla_result_types::retrieval_verdict::{
    RetrievalRefusal, RETRIEVAL_STATE_INVALID, RETRIEVAL_STATE_STALE,
};
use medulla_types::retrieval_round::{DocumentState, SegmentBoundary};

/// Revision of the deterministic built-in encoder; it names the encoder and its dimension.
pub const BUILTIN_ENCODER_REVISION: &str = "builtin:hash-segments:v1:d32";
/// Revision of the max-over-segments int8 dot-product scorer.
pub const SCORER_REVISION: &str = "state-max-dot:v1";
/// Width of one key (and one value) vector of the built-in encoder.
pub const BUILTIN_DIM: usize = 32;

/// The server-bound port that turns text into int8 projections. It has no decode
/// or generate method by construction.
pub trait StateEncoder: Send + Sync {
    /// The revision stored with every state this encoder builds.
    fn revision(&self) -> String;
    /// Width of one key vector; a segment projection is key then value.
    fn dim(&self) -> usize;
    /// The projection of one segment: key vector then value vector.
    fn encode_segment(&self, text: &str) -> Vec<i8>;
    /// The query projection, one key-space vector of width `dim`.
    fn encode_query(&self, query: &str) -> Vec<i8>;
}

/// The deterministic test and default encoder: signed hashed bag of tokens.
#[derive(Debug, Clone, Copy, Default)]
pub struct HashSegmentsEncoder;

fn tokens(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
}

fn hashed(text: &str, salt: &[u8]) -> Vec<i8> {
    let mut acc = [0i32; BUILTIN_DIM];
    for token in tokens(text) {
        let digest = sha256_hex_of(&[salt, token.as_bytes()]);
        let byte = |at: usize| u8::from_str_radix(&digest[at..at + 2], 16).unwrap_or(0);
        let bucket = usize::from(byte(0)) % BUILTIN_DIM;
        acc[bucket] += if byte(2) & 1 == 0 { 1 } else { -1 };
    }
    acc.iter()
        .map(|v| i8::try_from((*v).clamp(-127, 127)).unwrap_or(0))
        .collect()
}

impl StateEncoder for HashSegmentsEncoder {
    fn revision(&self) -> String {
        BUILTIN_ENCODER_REVISION.to_string()
    }
    fn dim(&self) -> usize {
        BUILTIN_DIM
    }
    fn encode_segment(&self, text: &str) -> Vec<i8> {
        let mut projection = hashed(text, b"k");
        projection.extend(hashed(text, b"v"));
        projection
    }
    fn encode_query(&self, query: &str) -> Vec<i8> {
        hashed(query, b"k")
    }
}

/// SHA-256 hex of the document text.
pub fn content_digest(text: &str) -> String {
    sha256_hex_of(&[text.as_bytes()])
}

/// The segment policy a state records, from the segment size.
pub fn segment_policy(segment_bytes: usize) -> String {
    format!("segment_bytes={segment_bytes}")
}

/// Ordered byte ranges covering `text` exactly, cut at char boundaries once a
/// segment holds at least `segment_bytes` bytes.
pub fn segment_boundaries(text: &str, segment_bytes: usize) -> Vec<SegmentBoundary> {
    let size = segment_bytes.max(1);
    let mut out = Vec::new();
    let mut start = 0usize;
    for (index, ch) in text.char_indices() {
        let end = index + ch.len_utf8();
        if end - start >= size {
            out.push(SegmentBoundary {
                start: start as u64,
                end: end as u64,
            });
            start = end;
        }
    }
    if start < text.len() {
        out.push(SegmentBoundary {
            start: start as u64,
            end: text.len() as u64,
        });
    }
    out
}

/// Build the query-independent state of one document. Pure and deterministic: the
/// same content, encoder and policy always give the same state.
pub fn build_document_state(
    encoder: &dyn StateEncoder,
    document_id: &str,
    text: &str,
    segment_bytes: usize,
) -> Result<DocumentState, RetrievalRefusal> {
    let segments = segment_boundaries(text, segment_bytes);
    let projections = segments
        .iter()
        .map(|s| encoder.encode_segment(&text[s.start as usize..s.end as usize]))
        .collect();
    let state = DocumentState {
        document_id: document_id.to_string(),
        content_digest: content_digest(text),
        encoder_revision: encoder.revision(),
        policy_revision: segment_policy(segment_bytes),
        segments,
        projections,
    };
    state
        .check_coverage(text.len() as u64)
        .map_err(|e| RetrievalRefusal::new(RETRIEVAL_STATE_INVALID, format!("{e:?}")))?;
    Ok(state)
}

/// The digest of one stored state: every field, in order.
pub fn state_digest(state: &DocumentState) -> String {
    let mut parts: Vec<Vec<u8>> = vec![
        state.document_id.as_bytes().to_vec(),
        state.content_digest.as_bytes().to_vec(),
        state.encoder_revision.as_bytes().to_vec(),
        state.policy_revision.as_bytes().to_vec(),
    ];
    for (segment, projection) in state.segments.iter().zip(&state.projections) {
        parts.push(segment.start.to_le_bytes().to_vec());
        parts.push(segment.end.to_le_bytes().to_vec());
        parts.push(projection.iter().map(|v| *v as u8).collect());
    }
    sha256_hex_of(&parts)
}

/// Where a rerank reads stored states from; the store is the only writer.
pub trait StateSource {
    fn get(
        &self,
        document_id: &str,
        content_digest: &str,
        encoder_revision: &str,
        policy_revision: &str,
    ) -> Result<Option<DocumentState>, String>;
}

/// One candidate: its id and its current text.
pub struct StateCandidate<'a> {
    pub id: &'a str,
    pub text: Option<&'a str>,
}

fn dot(a: &[i8], b: &[i8]) -> i64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| i64::from(*x) * i64::from(*y))
        .sum()
}

/// Score `query` against each candidate's stored state. Refuses
/// `RETRIEVAL_STATE_STALE` for a missing state or a candidate with no current
/// text, and `RETRIEVAL_STATE_INVALID` for a state that does not cover its text.
/// Result order is by score then id, so candidate order never matters.
pub fn rerank_with_state(
    encoder: &dyn StateEncoder,
    source: &dyn StateSource,
    policy_revision: &str,
    query: &str,
    candidates: &[StateCandidate<'_>],
) -> Result<StateResult, RetrievalRefusal> {
    let query_projection = encoder.encode_query(query);
    let query_tokens = tokens(query).count() as u64;
    let revision = encoder.revision();
    let mut by_id: BTreeMap<&str, &StateCandidate<'_>> = BTreeMap::new();
    for candidate in candidates {
        by_id.entry(candidate.id).or_insert(candidate);
    }
    let mut scores = Vec::new();
    let (mut baseline_ops, mut state_ops) = (0u64, 0u64);
    for (id, candidate) in by_id {
        let stale = |detail: String| RetrievalRefusal::new(RETRIEVAL_STATE_STALE, detail);
        let Some(text) = candidate.text else {
            return Err(stale(format!("document {id} has no current text")));
        };
        let digest = content_digest(text);
        let state = source
            .get(id, &digest, &revision, policy_revision)
            .map_err(|e| RetrievalRefusal::new(RETRIEVAL_STATE_INVALID, e))?
            .ok_or_else(|| {
                stale(format!(
                    "no state for document {id} at this content and encoder"
                ))
            })?;
        if state.document_id != id
            || state.content_digest != digest
            || state.encoder_revision != revision
        {
            return Err(stale(format!("state of document {id} does not match")));
        }
        state
            .check_coverage(text.len() as u64)
            .map_err(|e| RetrievalRefusal::new(RETRIEVAL_STATE_INVALID, format!("{e:?}")))?;
        let dim = encoder.dim();
        let mut best = i64::MIN;
        for projection in &state.projections {
            if projection.len() < dim {
                return Err(RetrievalRefusal::new(
                    RETRIEVAL_STATE_INVALID,
                    "projection narrower than the encoder",
                ));
            }
            best = best.max(dot(&query_projection, &projection[..dim]));
            state_ops += dim as u64;
        }
        baseline_ops += query_tokens * tokens(text).count() as u64;
        scores.push(StateScoreRow {
            document_id: id.to_string(),
            document_digest: digest,
            state_digest: state_digest(&state),
            state_revision: state.encoder_revision.clone(),
            segment_policy: state.policy_revision.clone(),
            score: if best == i64::MIN { 0 } else { best },
            scorer_revision: SCORER_REVISION.to_string(),
        });
    }
    scores.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.document_id.cmp(&b.document_id))
    });
    let result_digest = sha256_hex_of(
        &scores
            .iter()
            .map(|s| format!("{}|{}|{}", s.document_id, s.state_digest, s.score))
            .collect::<Vec<_>>(),
    );
    Ok(StateResult {
        scores,
        baseline_ops,
        state_ops,
        decoder_calls: 0,
        result_digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Mem(HashMap<String, DocumentState>);
    impl StateSource for Mem {
        fn get(&self, d: &str, c: &str, e: &str, p: &str) -> Result<Option<DocumentState>, String> {
            Ok(self
                .0
                .get(&medulla_types::retrieval_round::state_key(d, c, e, p))
                .cloned())
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R081.101.2
    #[test]
    fn state_digest_is_stable_for_same_content() {
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        let a = build_document_state(&HashSegmentsEncoder, "d", text, 16).unwrap();
        let b = build_document_state(&HashSegmentsEncoder, "d", text, 16).unwrap();
        assert_eq!(a, b);
        assert_eq!(state_digest(&a), state_digest(&b));
        assert!(a.segments.len() > 1);
        assert!(a.check_coverage(text.len() as u64).is_ok());
        // other content, policy or id is another state
        let other = build_document_state(&HashSegmentsEncoder, "d", "alpha beta", 16).unwrap();
        assert_ne!(state_digest(&a), state_digest(&other));
        let wide = build_document_state(&HashSegmentsEncoder, "d", text, 64).unwrap();
        assert_ne!(state_digest(&a), state_digest(&wide));
        // multi-byte text is cut on char boundaries and still covers exactly
        let utf = "héllo wörld ünïcode tëxt";
        let s = build_document_state(&HashSegmentsEncoder, "u", utf, 3).unwrap();
        assert!(s.check_coverage(utf.len() as u64).is_ok());
        // the empty document has no segments and covers zero bytes
        let empty = build_document_state(&HashSegmentsEncoder, "e", "", 16).unwrap();
        assert!(empty.segments.is_empty());
    }

    #[test]
    fn rerank_is_order_independent_and_refuses_missing_state() {
        let docs = [
            ("a", "red apple pie"),
            ("b", "blue ocean wave"),
            ("c", "red red red"),
        ];
        let mut mem = Mem::default();
        for (id, text) in docs {
            let s = build_document_state(&HashSegmentsEncoder, id, text, 8).unwrap();
            mem.0.insert(s.key(), s);
        }
        let cands = |order: &[usize]| -> Vec<StateCandidate<'static>> {
            order
                .iter()
                .map(|i| StateCandidate {
                    id: docs[*i].0,
                    text: Some(docs[*i].1),
                })
                .collect()
        };
        let policy = segment_policy(8);
        let one = rerank_with_state(
            &HashSegmentsEncoder,
            &mem,
            &policy,
            "red",
            &cands(&[0, 1, 2]),
        )
        .unwrap();
        let two = rerank_with_state(
            &HashSegmentsEncoder,
            &mem,
            &policy,
            "red",
            &cands(&[2, 1, 0]),
        )
        .unwrap();
        assert_eq!(one, two);
        assert_eq!(one.decoder_calls, 0);
        assert!(one.state_ops > 0 && one.baseline_ops > 0);
        // stale: changed content has no state
        let mut stale = cands(&[0]);
        stale[0].text = Some("red apple tart");
        let err =
            rerank_with_state(&HashSegmentsEncoder, &mem, &policy, "red", &stale).unwrap_err();
        assert_eq!(err.code, RETRIEVAL_STATE_STALE);
        let none = [StateCandidate {
            id: "a",
            text: None,
        }];
        assert_eq!(
            rerank_with_state(&HashSegmentsEncoder, &mem, &policy, "red", &none)
                .unwrap_err()
                .code,
            RETRIEVAL_STATE_STALE
        );
    }
}
