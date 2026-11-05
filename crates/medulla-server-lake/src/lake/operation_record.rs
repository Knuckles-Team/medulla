//! Lake operation evidence (MEDULLA-FEDERATED-QUERY-R079.5).
//!
//! One [`LakeOperationRecord`] per publication, schema change or time-travel read,
//! kept in a bounded ring on the [`super::LakeManager`] next to the lineage and
//! audit rings. A record correlates the committed graph version, the source log
//! position, the schema version and the published Iceberg artifacts (locations and
//! SHA-256 digests). A failed publication writes a `Refused` record and never a
//! `Committed` one. Records carry no credentials and no row data.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Bound on the in-memory operation-record ring (oldest records drop first).
pub const OPERATION_RING_CAP: usize = 500;

/// How an operation ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationOutcome {
    /// The snapshot or schema change is visible.
    Committed,
    /// The operation was refused; no new snapshot is visible.
    Refused,
    /// A time-travel read resolved an existing snapshot.
    Read,
}

/// Evidence of one lake operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LakeOperationRecord {
    /// Owning tenant of the table (`None` for an engine-internal table).
    pub tenant: Option<String>,
    /// `namespace.table`.
    pub table: String,
    /// The committed graph version the snapshot is bound to.
    pub graph_version: Option<u64>,
    /// The source log position the snapshot is bound to.
    pub source_lsn: Option<u64>,
    /// The Iceberg schema id in effect for the operation.
    pub schema_version: Option<i32>,
    /// `CREATE`, `APPEND`, `OVERWRITE`, `TRUNCATE`, `SCHEMA_ADD_COLUMN` or `TIME_TRAVEL_READ`.
    pub change_kind: String,
    /// Reader compatibility of the change (`backward-compatible` for every
    /// permitted change; `refused` for a refusal).
    pub compatibility: String,
    /// The Iceberg snapshot id the operation published or resolved.
    pub snapshot_id: Option<i64>,
    /// The Iceberg table format version.
    pub format_version: u32,
    /// Location of the manifest file.
    pub manifest_location: Option<String>,
    /// Location of the manifest list.
    pub manifest_list_location: Option<String>,
    /// Location of the table metadata file.
    pub metadata_location: Option<String>,
    /// SHA-256 (64 lowercase hex) of the manifest file.
    pub manifest_digest: Option<String>,
    /// SHA-256 (64 lowercase hex) of the manifest list.
    pub manifest_list_digest: Option<String>,
    /// SHA-256 (64 lowercase hex) of the metadata file.
    pub metadata_digest: Option<String>,
    /// How the operation ended.
    pub outcome: OperationOutcome,
    /// Why the operation was refused.
    pub refusal_reason: Option<String>,
}

/// Iceberg table format version this tier publishes.
pub const FORMAT_VERSION: u32 = 2;

/// SHA-256 of `bytes` as 64 lowercase hex characters.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

impl LakeOperationRecord {
    /// A record with every optional field empty.
    pub fn new(
        table: String,
        tenant: Option<String>,
        change_kind: &str,
        outcome: OperationOutcome,
    ) -> Self {
        LakeOperationRecord {
            tenant,
            table,
            graph_version: None,
            source_lsn: None,
            schema_version: None,
            change_kind: change_kind.to_string(),
            compatibility: "backward-compatible".to_string(),
            snapshot_id: None,
            format_version: FORMAT_VERSION,
            manifest_location: None,
            manifest_list_location: None,
            metadata_location: None,
            manifest_digest: None,
            manifest_list_digest: None,
            metadata_digest: None,
            outcome,
            refusal_reason: None,
        }
    }

    /// A refusal record: no snapshot, no digests, the reason kept.
    pub fn refused(
        table: String,
        tenant: Option<String>,
        change_kind: &str,
        reason: String,
    ) -> Self {
        let mut record = Self::new(table, tenant, change_kind, OperationOutcome::Refused);
        record.compatibility = "refused".to_string();
        record.refusal_reason = Some(reason);
        record
    }
}
