//! Chunk-encoding evidence and the served document-vector read/write types
//! (MEDULLA-FEDERATED-QUERY-R039.2.5, R039.2.6, R039.2.7).
//!
//! They live below the protocol crate because `Method::EncodeDocument` and
//! `Method::EncodedDocumentSearch` carry or return them. The recipe itself is
//! `medulla_core::compute::chunk_windows`; the served handler in `medulla-h-query`
//! runs it.

use serde::{Deserialize, Serialize};

use crate::contract::Digest256;

/// The receipt of one document encoding: every chunk, the single stored vector and
/// the policy and generation they were published under. It is stored beside the
/// vector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EncodingReceipt {
    /// SHA-256 of the document's token ids, little-endian `u32`s in order.
    pub document_digest: String,
    /// Half-open `[start, end)` token range of every chunk, in ordinal order.
    pub token_ranges: Vec<[u64; 2]>,
    /// `local_reset` or `absolute_offset`.
    pub position_mode: String,
    /// The model revision that encoded each chunk, one per range.
    pub chunk_model_revisions: Vec<String>,
    /// SHA-256 of the stored vector, little-endian `f32`s in order.
    pub aggregate_digest: String,
    /// SHA-256 of the canonical policy record the document was encoded under.
    pub policy_digest: String,
    /// The vector generation the document was published into.
    pub index_generation: u64,
    /// Wall-clock milliseconds the encoding took, as measured.
    pub measured_cost_ms: u64,
}

impl EncodingReceipt {
    /// Every digest is 64 lowercase hex characters and every chunk has a range and a
    /// model revision.
    pub fn validate(&self) -> Result<(), String> {
        for (name, digest) in [
            ("document_digest", &self.document_digest),
            ("aggregate_digest", &self.aggregate_digest),
            ("policy_digest", &self.policy_digest),
        ] {
            Digest256::parse(digest).map_err(|error| format!("{name}: {error}"))?;
        }
        if self.token_ranges.len() != self.chunk_model_revisions.len() {
            return Err("every chunk range needs one model revision".to_string());
        }
        if self.token_ranges.iter().any(|[start, end]| start >= end) {
            return Err("a chunk range must be non-empty".to_string());
        }
        Ok(())
    }
}

/// The SHA-256 of `bytes` as 64 lowercase hex characters.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Digest256::sha256(bytes).to_hex()
}

/// A served `EncodeDocument` answer: the receipt and the one document vector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EncodedDocument {
    pub document_id: String,
    pub receipt: EncodingReceipt,
    pub vector: Vec<f32>,
}

/// A served `EncodedDocumentSearch` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EncodedSearchRequest {
    pub query_embedding: Vec<f32>,
    pub k: u32,
    /// The policy digest to rank under. A digest with no vector generation under it
    /// refuses `ENCODE_POLICY_STALE` before ranking.
    pub policy_digest: String,
}

/// One ranked document of an encoded-document search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EncodedHit {
    pub document_id: String,
    pub score: f32,
    pub receipt: EncodingReceipt,
}

/// A served `EncodedDocumentSearch` answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EncodedSearchResult {
    pub hits: Vec<EncodedHit>,
    /// The policy digest the ranking ran under.
    pub policy_digest: String,
    /// The vector generation the ranking read.
    pub index_generation: u64,
}

/// MEDULLA-FEDERATED-QUERY-R039: why a ranked answer holds fewer than the `k` rows
/// asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ShortfallReason {
    /// Every eligible candidate was ranked and fewer than `k` exist.
    EligibleExhausted,
    /// Some eligible candidate holds no embedding, so it could not be ranked.
    MissingEmbedding,
    /// `k` exceeded the plan's result-row budget and was capped to it.
    BudgetExhausted,
}

/// MEDULLA-FEDERATED-QUERY-R039: the typed answer of a ranked step that could not
/// return `k` rows. The planner never returns fewer than `k` rows without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Shortfall {
    /// The `k` the plan asked for.
    pub requested: u64,
    /// The rows the answer holds.
    pub found: u64,
    /// Why the answer is short.
    pub reason: ShortfallReason,
}

/// MEDULLA-FEDERATED-QUERY-R039: the answer of a `UnifiedQuery` whose plan sets
/// `report_shortfall`: the ranked rows and, when fewer than `k` survive, the
/// [`Shortfall`] that names why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RankedRows {
    /// The ranked rows, in the plan's final order.
    pub rows: Vec<crate::unified_row::UnifiedRow>,
    /// The retrieval evidence of the request (MEDULLA-FEDERATED-QUERY-R040.1.5); absent
    /// when the request declared nothing the receipt reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<RetrievalReceipt>,
    /// Present when the answer holds fewer rows than the plan asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortfall: Option<Shortfall>,
}

/// MEDULLA-FEDERATED-QUERY-R040.1.5: what one request's hard constraints did. The binding
/// (digest and revision) is fixed when the constraints compile and stays identical across
/// replenishment and fusion; it holds ids and numbers only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TraceConstraint {
    /// SHA-256 of the canonical JSON of the declared constraints.
    pub constraint_digest: String,
    /// The pinned graph revision the constraints declared (`0`: no pin).
    pub graph_revision: u64,
    /// How many candidates the caller was authorized to see and the constraints excluded.
    pub authorized_exclusion_count: u64,
    /// One entry per declared predicate, entity predicates first, in declaration order.
    pub per_predicate: Vec<PredicateOutcome>,
}

/// MEDULLA-FEDERATED-QUERY-R040.1.5: the outcome of one declared predicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PredicateOutcome {
    /// `entity[i]` or `relationship[i]`: the predicate's kind and declaration index.
    pub predicate_id: String,
    /// How many authorized candidates this predicate alone admitted.
    pub outcome: u64,
}

/// MEDULLA-FEDERATED-QUERY-R040.1.5: the evidence a ranked `UnifiedQuery` answer carries.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RetrievalReceipt {
    /// Present when the request declared hard constraints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraint: Option<TraceConstraint>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> EncodingReceipt {
        EncodingReceipt {
            document_digest: sha256_hex(b"d"),
            token_ranges: vec![[0, 4], [2, 6]],
            position_mode: "local_reset".into(),
            chunk_model_revisions: vec!["m".into(), "m".into()],
            aggregate_digest: sha256_hex(b"a"),
            policy_digest: sha256_hex(b"p"),
            index_generation: 3,
            measured_cost_ms: 1,
        }
    }

    #[test]
    fn receipt_round_trips_and_refuses_invalid() {
        let good = receipt();
        good.validate().unwrap();
        let wire = serde_json::to_string(&good).unwrap();
        assert_eq!(
            serde_json::from_str::<EncodingReceipt>(&wire).unwrap(),
            good
        );
        let mut bad = receipt();
        bad.document_digest = "ABC".into();
        assert!(bad.validate().is_err());
        let mut short = receipt();
        short.chunk_model_revisions.pop();
        assert!(short.validate().is_err());
        let mut empty = receipt();
        empty.token_ranges[0] = [3, 3];
        assert!(empty.validate().is_err());
    }
}
