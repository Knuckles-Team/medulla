//! Bookmark semantic search and authorized relations wire types
//! (MEDULLA-MEDIA-VOCAB-R001.102.1).
//!
//! `Method::BookmarkSemanticSearch` and `Method::BookmarkRelations` parse these types at their
//! handlers. Every response returns the [`BookmarkBinding`] of the one index revision and graph
//! snapshot it read. Collected by `medulla_decision_types::decision::codes::module_tables`
//! (MEDULLA-DECISION-ENGINE-R010.100), so no code here repeats a shared or foreign code.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::contract::Digest256;
use crate::refusal_code::RefusalCode;

/// `Fixed` ceiling of `max_hits`.
pub const MAX_HITS_CEILING: u32 = 100;
/// `Fixed` ceiling of `max_relationships`.
pub const MAX_RELATIONSHIPS_CEILING: u32 = 500;
/// `Fixed` ceiling of `max_response_bytes`.
pub const MAX_RESPONSE_BYTES_CEILING: u32 = 1_048_576;

/// The policy limit a request must state, or the request is refused.
pub const BOOKMARK_POLICY_INVALID: RefusalCode = RefusalCode::new("BOOKMARK_POLICY_INVALID");
/// A cap was reached; never answered as a truncated success.
pub const BOOKMARK_LIMIT_EXCEEDED: RefusalCode = RefusalCode::new("BOOKMARK_LIMIT_EXCEEDED");
/// The semantic index has no live generation, or the cursor names another one.
pub const BOOKMARK_INDEX_STALE: RefusalCode = RefusalCode::new("BOOKMARK_INDEX_STALE");
/// The source revision a bookmark carries does not match the request.
pub const BOOKMARK_REVISION_MISMATCH: RefusalCode =
    RefusalCode::new("BOOKMARK_REVISION_MISMATCH");
/// The serialized response is above `max_response_bytes`.
pub const BOOKMARK_RESPONSE_TOO_LARGE: RefusalCode =
    RefusalCode::new("BOOKMARK_RESPONSE_TOO_LARGE");

/// Every refusal code of the bookmark query family.
pub const REFUSALS: &[RefusalCode] = &[
    BOOKMARK_POLICY_INVALID,
    BOOKMARK_LIMIT_EXCEEDED,
    BOOKMARK_INDEX_STALE,
    BOOKMARK_REVISION_MISMATCH,
    BOOKMARK_RESPONSE_TOO_LARGE,
];

/// The three caps of one bookmark query. Each is positive and at most its ceiling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookmarkQueryLimitPolicy {
    pub max_hits: u32,
    pub max_relationships: u32,
    pub max_response_bytes: u32,
}

impl BookmarkQueryLimitPolicy {
    /// Refuse a zero or over-ceiling limit with `BOOKMARK_POLICY_INVALID`.
    pub fn validate(&self) -> Result<(), RefusalCode> {
        let within = |value: u32, ceiling: u32| (1..=ceiling).contains(&value);
        if within(self.max_hits, MAX_HITS_CEILING)
            && within(self.max_relationships, MAX_RELATIONSHIPS_CEILING)
            && within(self.max_response_bytes, MAX_RESPONSE_BYTES_CEILING)
        {
            Ok(())
        } else {
            Err(BOOKMARK_POLICY_INVALID)
        }
    }
}

/// One semantic search over the caller's bookmarks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookmarkSemanticSearchRequest {
    /// Non-empty and finite.
    pub query_embedding: Vec<f32>,
    /// Empty means every bookmark source of the caller.
    pub source_ids: Vec<String>,
    pub limit_policy: BookmarkQueryLimitPolicy,
    /// Absent on the first page.
    pub cursor: Option<String>,
}

impl BookmarkSemanticSearchRequest {
    /// Refuse an empty or non-finite embedding and an invalid policy.
    pub fn validate(&self) -> Result<(), RefusalCode> {
        self.limit_policy.validate()?;
        if self.query_embedding.is_empty() || self.query_embedding.iter().any(|v| !v.is_finite()) {
            return Err(BOOKMARK_POLICY_INVALID);
        }
        Ok(())
    }
}

/// One read of the authorized relations among bookmark nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookmarkRelationsRequest {
    /// Bookmark node ids, normally the hit ids of a search; non-empty, at most `max_hits`.
    pub node_ids: Vec<String>,
    pub limit_policy: BookmarkQueryLimitPolicy,
    pub cursor: Option<String>,
}

impl BookmarkRelationsRequest {
    /// Refuse an empty or oversized node list and an invalid policy.
    pub fn validate(&self) -> Result<(), RefusalCode> {
        self.limit_policy.validate()?;
        if self.node_ids.is_empty() {
            return Err(BOOKMARK_POLICY_INVALID);
        }
        if self.node_ids.len() > self.limit_policy.max_hits as usize {
            return Err(BOOKMARK_LIMIT_EXCEEDED);
        }
        Ok(())
    }
}

/// One search hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookmarkHit {
    pub id: String,
    pub score: f32,
    pub source_id: String,
    pub provenance: BTreeMap<String, String>,
    /// Every metadata key the bookmark node does not carry, sorted.
    pub unknown_keys: Vec<String>,
    /// Set on the last hit of a truncated page.
    pub cursor: Option<String>,
}

/// One authorized relation between two bookmark nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookmarkEdge {
    pub source_node_id: String,
    pub target_node_id: String,
    pub relationship: String,
    /// The mindmap id that drew the edge, `None` when unknown.
    pub provenance: Option<String>,
}

/// The evidence every response carries: what it read and under which revisions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookmarkBinding {
    pub source_ids: Vec<String>,
    pub ingestion_receipt_id: String,
    pub ingestion_receipt_digest: String,
    pub source_revision: String,
    /// Live generation of the semantic index, read once at the start of the call.
    pub index_revision: u64,
    pub query_digest: Digest256,
    /// `GraphCore::version()` of the caller's core, read once at the start of the call.
    pub graph_snapshot_revision: u64,
    /// `IsolationLayer::policy_revision()` read at the same moment.
    pub authorization_revision: u64,
    pub limit_policy_digest: Digest256,
}

/// The answer of `Method::BookmarkSemanticSearch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookmarkSearchResult {
    pub hits: Vec<BookmarkHit>,
    pub binding: BookmarkBinding,
}

/// The answer of `Method::BookmarkRelations`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookmarkRelationsResult {
    pub edges: Vec<BookmarkEdge>,
    pub binding: BookmarkBinding,
}
