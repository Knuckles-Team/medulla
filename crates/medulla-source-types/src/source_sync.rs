//! The served `SourceSync` answer (MEDULLA-SRC-AUTH-R012.6, R012.8): how many
//! records each decision touched, the conflicts with both digests, and the
//! outbound changes the router queues as write-back proposals.

use serde::{Deserialize, Serialize};

/// One record both sides changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SyncConflictEntry {
    /// The source-side record key.
    pub record_key: String,
    /// The graph value digest.
    pub graph_digest: Option<String>,
    /// The source value digest.
    pub source_digest: Option<String>,
    /// The `vg:conflictRule` literal that was applied.
    pub rule: String,
}

/// One graph change to propose to the source. Sync never writes to a source;
/// the router queues each draft as one write-back proposal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OutboundDraft {
    /// The source-side record key; the proposal's external id.
    pub record_key: String,
    /// The digest of the outbound payload.
    pub payload_digest: String,
}

/// The reply of one `SourceSync`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SyncReport {
    /// The connection the sync ran against.
    pub connection_iri: String,
    /// The class whose records were synced.
    pub class_iri: String,
    /// Records that needed nothing.
    pub unchanged: u64,
    /// Records the source changed and the graph took.
    pub applied: u64,
    /// Records proposed outbound (graph-only changes and graph-wins conflicts).
    pub proposed: u64,
    /// Records both sides changed, with how each was resolved.
    pub conflicts: Vec<SyncConflictEntry>,
    /// The outbound changes, in record-key order.
    pub outbound: Vec<OutboundDraft>,
    /// Write-back proposal ids queued for them; empty until the router queues.
    #[serde(default)]
    pub queued: Vec<String>,
}
