//! Document delete wire types (MEDULLA-UNIFIED-DATA-PLANE-R651.1).

use serde::{Deserialize, Serialize};

/// Delete one Document and everything derived from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentDeleteRequest {
    pub tenant: String,
    pub document_id: String,
}

/// What one committed cascade deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentDeleteReceipt {
    pub tenant: String,
    /// SHA-256 of the committed cascade plan, 64 lowercase hex.
    pub plan_digest: String,
    pub nodes_deleted: u64,
    pub edges_deleted: u64,
    /// Vector and lexical entries still draining after commit.
    pub index_pending: u64,
}
