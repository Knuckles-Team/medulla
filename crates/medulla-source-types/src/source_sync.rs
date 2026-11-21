//! The served `SourceSync` answer (MEDULLA-SRC-AUTH-R012.6, R012.8): how many
//! records each decision touched, the conflicts with both digests, and the
//! outbound changes the router queues as write-back proposals.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Why a sync lists a record among its conflicts (MEDULLA-TYPED-PACKS-R110.6). A kind
/// set at construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SyncConflictReason {
    /// Both sides changed the record since the last sync.
    BothChanged,
    /// Only the graph changed, but the source store holds a newer version.
    SourceVersionAhead,
}

/// One source type the sync left out, with the first record key that carried it and
/// how many records carried it (MEDULLA-TYPED-PACKS-R110.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SyncSkippedType {
    /// The source type name no catalog class carries.
    pub type_name: String,
    /// The first record key (in key order) that carried the type.
    pub first_key: String,
    /// The records that carried the type.
    pub count: u64,
}

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
    /// Why the record is listed.
    pub reason: SyncConflictReason,
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
    /// The write-back queue the proposals were stored under; the id an
    /// approver names to approve and release them. `None` until the router
    /// queues, and when nothing is proposed.
    #[serde(default)]
    pub queue_id: Option<String>,
    /// Source types the sync left out, sorted by type name (MEDULLA-TYPED-PACKS-R110.6).
    #[serde(default)]
    pub skipped: Vec<SyncSkippedType>,
    /// Accepted source records per source type (MEDULLA-TYPED-PACKS-R110.6).
    #[serde(default)]
    pub by_type: BTreeMap<String, u64>,
}
