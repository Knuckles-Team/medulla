//! The identity an index owns: who built it, from what, under which policies.
//!
//! The identity is persisted in the index manifest when the index is built, and the
//! compaction worker reads it from there (MEDULLA-FEDERATED-QUERY-R040.5.6). No caller
//! passes it to the worker.

use serde::{Deserialize, Serialize};

/// Tenant, snapshot, source, revision, text digest and policies of one built index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexIdentity {
    /// Tenant that owns the indexed source.
    pub tenant: String,
    /// Snapshot of the source the index was built from.
    pub snapshot: String,
    /// Source the indexed text came from.
    pub source_id: String,
    /// Source revision the index reflects.
    pub revision: u64,
    /// Digest of the indexed source text. The text itself is never copied.
    pub text_digest: [u8; 32],
    /// Encoding policy the vectors were produced under.
    pub encoding_policy: String,
    /// Build policy the index was built under.
    pub build_policy: String,
    /// Source generation the build read; always positive.
    pub generation: u64,
    /// Byte ceiling the compact artifact must stay under.
    pub ceiling_bytes: u64,
}
