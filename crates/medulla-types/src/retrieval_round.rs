//! The reusable compressed document state (MEDULLA-FEDERATED-QUERY-R081.101.1).
//!
//! A `DocumentState` is built once per immutable document content and encoder
//! revision. It never depends on a query or on candidate order. The segment
//! boundaries are ordered byte ranges that cover the text exactly, and each
//! segment carries one int8 projection (key vector then value vector).

use serde::{Deserialize, Serialize};

/// Most segments one document state may hold.
pub const MAX_STATE_SEGMENTS: usize = 4096;
/// Most projection rows (vectors) one document state may hold.
pub const MAX_STATE_ROWS: usize = 1_048_576;
/// Largest encoded state value, in bytes.
pub const MAX_STATE_VALUE_BYTES: usize = 16 * 1024 * 1024;

/// One ordered byte range of the document text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentBoundary {
    pub start: u64,
    pub end: u64,
}

/// The query-independent state of one document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentState {
    pub document_id: String,
    pub content_digest: String,
    pub encoder_revision: String,
    pub policy_revision: String,
    pub segments: Vec<SegmentBoundary>,
    /// One int8 projection per segment: the key vector then the value vector.
    pub projections: Vec<Vec<i8>>,
}

/// Why a state is not usable. `Invalid` maps to `RETRIEVAL_STATE_INVALID`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentStateError {
    /// Segments do not cover the text exactly, or a bound is exceeded.
    Invalid(String),
    /// The stored value could not be encoded or decoded.
    Codec(String),
}

impl DocumentState {
    /// The store key: `(document_id, content_digest, encoder_revision, policy_revision)`.
    pub fn key(&self) -> String {
        state_key(
            &self.document_id,
            &self.content_digest,
            &self.encoder_revision,
            &self.policy_revision,
        )
    }

    /// Check that the segments are ordered, non-empty ranges covering exactly
    /// `text_len` bytes, one projection per segment, and every bound holds.
    pub fn check_coverage(&self, text_len: u64) -> Result<(), DocumentStateError> {
        let invalid = |detail: &str| Err(DocumentStateError::Invalid(detail.to_string()));
        if self.segments.len() > MAX_STATE_SEGMENTS {
            return invalid("more segments than the bound");
        }
        if self.projections.len() != self.segments.len() {
            return invalid("one projection per segment is required");
        }
        let rows: usize = self.projections.iter().map(Vec::len).sum();
        if rows > MAX_STATE_ROWS {
            return invalid("more projection rows than the bound");
        }
        let mut cursor = 0u64;
        for segment in &self.segments {
            if segment.start != cursor || segment.end <= segment.start {
                return invalid("segments are not ordered and gap-free");
            }
            cursor = segment.end;
        }
        if cursor != text_len {
            return invalid("segments do not cover the text exactly");
        }
        Ok(())
    }

    /// The MessagePack value stored under [`Self::key`]; refuses a value over the bound.
    pub fn encode(&self) -> Result<Vec<u8>, DocumentStateError> {
        let bytes =
            rmp_serde::to_vec_named(self).map_err(|e| DocumentStateError::Codec(e.to_string()))?;
        if bytes.len() > MAX_STATE_VALUE_BYTES {
            return Err(DocumentStateError::Invalid(
                "encoded state exceeds the value bound".to_string(),
            ));
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DocumentStateError> {
        rmp_serde::from_slice(bytes).map_err(|e| DocumentStateError::Codec(e.to_string()))
    }
}

/// Length-prefixed join of the four key parts, so no two tuples collide.
pub fn state_key(
    document_id: &str,
    content_digest: &str,
    encoder_revision: &str,
    policy_revision: &str,
) -> String {
    [
        document_id,
        content_digest,
        encoder_revision,
        policy_revision,
    ]
    .iter()
    .map(|part| format!("{}:{part}", part.len()))
    .collect::<Vec<_>>()
    .join("|")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> DocumentState {
        DocumentState {
            document_id: "doc-1".into(),
            content_digest: "abc".into(),
            encoder_revision: "enc-1".into(),
            policy_revision: "512".into(),
            segments: vec![
                SegmentBoundary { start: 0, end: 5 },
                SegmentBoundary { start: 5, end: 9 },
            ],
            projections: vec![vec![1, -2, 3, 4], vec![-5, 6, -7, 8]],
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R081.101.1
    #[test]
    fn document_state_round_trips_and_checks_coverage() {
        let s = state();
        let bytes = s.encode().unwrap();
        assert_eq!(DocumentState::decode(&bytes).unwrap(), s);
        assert!(s.check_coverage(9).is_ok());
        // a short, long, gapped, empty-range or projection-less state is invalid
        assert!(matches!(
            s.check_coverage(10),
            Err(DocumentStateError::Invalid(_))
        ));
        assert!(s.check_coverage(8).is_err());
        let mut gap = state();
        gap.segments[1].start = 6;
        assert!(gap.check_coverage(9).is_err());
        let mut empty = state();
        empty.segments[0].end = 0;
        assert!(empty.check_coverage(9).is_err());
        let mut missing = state();
        missing.projections.pop();
        assert!(missing.check_coverage(9).is_err());
        // the key separates its parts
        assert_ne!(
            state_key("a", "bc", "d", "e"),
            state_key("ab", "c", "d", "e")
        );
        assert_eq!(s.key(), state_key("doc-1", "abc", "enc-1", "512"));
    }
}
