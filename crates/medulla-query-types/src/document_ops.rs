//! Document delete wire types (MEDULLA-UNIFIED-DATA-PLANE-R651.1).

use serde::{Deserialize, Serialize};

/// Delete one Document and everything derived from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DocumentDeleteRequest {
    pub tenant: String,
    pub document_id: String,
    /// A lower cascade cap than `knob document.max_cascade_nodes`; never raises it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_nodes: Option<u64>,
}

/// What one committed cascade deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DocumentDeleteReceipt {
    pub tenant: String,
    /// SHA-256 of the committed cascade plan, 64 lowercase hex.
    pub plan_digest: String,
    pub nodes_deleted: u64,
    pub edges_deleted: u64,
    /// Vector and lexical entries still draining after commit.
    pub index_pending: u64,
}

/// The content-free record a committed cascade leaves (MEDULLA-UNIFIED-DATA-PLANE-R651.5).
///
/// Written as one `DocumentDeletion` node in the same batch as the delete. It
/// mirrors [`DocumentDeleteReceipt`] and holds no document text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DocumentDeletion {
    pub document_id: String,
    pub plan_digest: String,
    pub nodes_deleted: u64,
    pub edges_deleted: u64,
    /// Commit time, milliseconds since the Unix epoch.
    pub deleted_at: u64,
    /// The verified principal that asked for the delete.
    pub actor: String,
}

/// Label of the deletion record node.
pub const DOCUMENT_DELETION_TYPE: &str = "DocumentDeletion";
