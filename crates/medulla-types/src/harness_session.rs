//! Harness session index records (MEDULLA-ORCH-VOCAB-R009.101.1, R009.101.2).
//!
//! The index is a projection over committed `RunTrace` nodes, not a second trace store: one
//! `HarnessSession` node per source session, one receipt node per accepted sync receipt and
//! one counter node per tenant. The connector side is `contracts/scion-harness-session-ingest.md`.

use serde::{Deserialize, Serialize};

use crate::refusal_code::RefusalCode;

/// The one manifest revision the engine reads.
pub const HARNESS_SESSION_MANIFEST_REVISION: u32 = 1;

/// A manifest revision, digest or adapter revision the engine does not read.
pub const SESSION_MANIFEST_UNSUPPORTED: &str = "SESSION_MANIFEST_UNSUPPORTED";
/// A record whose content differs from what the index already holds under its identity.
pub const SESSION_CONTENT_CHANGED: &str = "SESSION_CONTENT_CHANGED";
/// A run trace the caller's tenant does not hold, or a batch of another tenant.
pub const SESSION_TRACE_UNKNOWN: &str = "SESSION_TRACE_UNKNOWN";
/// A search cursor minted at another index revision, tenant or filter.
pub const SESSION_CURSOR_STALE: &str = "SESSION_CURSOR_STALE";

/// The refusal codes of the harness session index. The decision plane collects this table
/// with every other family table and refuses a code declared twice.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(SESSION_MANIFEST_UNSUPPORTED),
    RefusalCode::new(SESSION_CONTENT_CHANGED),
    RefusalCode::new(SESSION_TRACE_UNKNOWN),
    RefusalCode::new(SESSION_CURSOR_STALE),
];

/// Longest id, cursor and text field of a batch, in characters.
pub const MAX_ID_CHARS: usize = 256;
/// Longest `reason` of an unavailable session.
pub const MAX_REASON_CHARS: usize = 512;
/// Default and bounds of the knob `orchestration_limits.session_batch_max`.
pub const SESSION_BATCH_MAX_DEFAULT: u64 = 500;
/// Inclusive range of `orchestration_limits.session_batch_max`.
pub const SESSION_BATCH_MAX_RANGE: (u64, u64) = (1, 5_000);

/// The label of a session node.
pub const SESSION_LABEL: &str = "HarnessSession";
/// The label of a sync receipt node.
pub const RECEIPT_LABEL: &str = "HarnessSync";
/// The label of the per-tenant counter node.
pub const COUNTER_LABEL: &str = "HarnessSessionIndex";
/// The edge from a session node to its run trace.
pub const EDGE_INDEXES_RUN: &str = "indexesRun";

/// The id of the session node of `source_session_id` of `harness_id` in `tenant`.
pub fn session_node_id(tenant: &str, harness_id: &str, source_session_id: &str) -> String {
    format!("harnesssession:{tenant}:{harness_id}:{source_session_id}")
}

/// The id of the receipt node of `sync_receipt_id` in `tenant`.
pub fn receipt_node_id(tenant: &str, sync_receipt_id: &str) -> String {
    format!("harnesssync:{tenant}:{sync_receipt_id}")
}

/// The id of the counter node of `tenant`.
pub fn counter_node_id(tenant: &str) -> String {
    format!("harnesssessionindex:{tenant}")
}

/// One source session and the committed run trace it indexes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HarnessSessionRecord {
    pub harness_id: String,
    pub adapter_revision: u32,
    pub source_session_id: String,
    pub run_trace_id: String,
    pub run_trace_revision: u64,
    /// Lowercase hex SHA-256 of the run trace (`run_trace_digest`), no prefix.
    pub trace_digest: String,
    pub source_sequence: u64,
}

/// A source session the connector could not read, kept in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct UnavailableSession {
    pub harness_id: String,
    pub source_session_id: String,
    pub reason: String,
}

/// One batch of sessions of one manifest, with the sync receipt that vouches for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexHarnessSessionBatch {
    pub tenant_id: String,
    pub manifest_id: String,
    pub manifest_revision: u32,
    pub manifest_digest: String,
    pub declared_total: u64,
    pub sync_receipt_id: String,
    pub sync_receipt_digest: String,
    pub source_cursor: String,
    pub records: Vec<HarnessSessionRecord>,
    pub unavailable: Vec<UnavailableSession>,
}

/// What one batch changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IndexHarnessSessionBatchResult {
    /// Records written or replaced as indexed sessions.
    pub indexed: u32,
    /// Records and unavailable entries the index already held exactly.
    pub unchanged: u32,
    /// Unavailable entries written or replaced.
    pub unavailable: u32,
    /// The index revision after the batch.
    pub index_revision: u64,
}

/// Whether `text` is 64 lowercase hex characters.
pub fn is_hex64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl IndexHarnessSessionBatch {
    /// The bounds of a batch: ids, cursor and reason lengths and the batch size, each refused
    /// `PARAMETER_INVALID`. Digests and the manifest revision are checked by the handler as
    /// `SESSION_MANIFEST_UNSUPPORTED`.
    pub fn check_bounds(&self, batch_max: usize) -> Result<(), String> {
        if self.records.len() + self.unavailable.len() > batch_max {
            return Err(format!(
                "PARAMETER_INVALID: a batch holds at most {batch_max} sessions"
            ));
        }
        let too_long = |text: &str, max: usize| text.chars().count() > max;
        let mut ids = vec![
            &self.tenant_id,
            &self.manifest_id,
            &self.sync_receipt_id,
            &self.source_cursor,
        ];
        for record in &self.records {
            ids.extend([
                &record.harness_id,
                &record.source_session_id,
                &record.run_trace_id,
            ]);
        }
        for entry in &self.unavailable {
            ids.extend([&entry.harness_id, &entry.source_session_id]);
            if too_long(&entry.reason, MAX_REASON_CHARS) {
                return Err(format!(
                    "PARAMETER_INVALID: a reason holds at most {MAX_REASON_CHARS} characters"
                ));
            }
        }
        if ids
            .iter()
            .any(|id| id.is_empty() || too_long(id, MAX_ID_CHARS))
        {
            return Err(format!(
                "PARAMETER_INVALID: ids and the cursor are 1 to {MAX_ID_CHARS} characters"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch() -> IndexHarnessSessionBatch {
        IndexHarnessSessionBatch {
            tenant_id: "t1".into(),
            manifest_id: "manifest-1".into(),
            manifest_revision: 1,
            manifest_digest: "ab".repeat(32),
            declared_total: 3,
            sync_receipt_id: "receipt-1".into(),
            sync_receipt_digest: "cd".repeat(32),
            source_cursor: "cursor-9".into(),
            records: vec![HarnessSessionRecord {
                harness_id: "claude-code".into(),
                adapter_revision: 2,
                source_session_id: "s-1".into(),
                run_trace_id: "runtrace:s-1".into(),
                run_trace_revision: 1,
                trace_digest: "ef".repeat(32),
                source_sequence: 7,
            }],
            unavailable: vec![UnavailableSession {
                harness_id: "claude-code".into(),
                source_session_id: "s-2".into(),
                reason: "transcript unreadable".into(),
            }],
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.101.1
    #[test]
    fn batch_round_trips_against_vectors() {
        // The committed wire vector: the field names and shapes a connector sends.
        let vector = serde_json::json!({
            "tenant_id": "t1",
            "manifest_id": "manifest-1",
            "manifest_revision": 1,
            "manifest_digest": "ab".repeat(32),
            "declared_total": 3,
            "sync_receipt_id": "receipt-1",
            "sync_receipt_digest": "cd".repeat(32),
            "source_cursor": "cursor-9",
            "records": [{
                "harness_id": "claude-code",
                "adapter_revision": 2,
                "source_session_id": "s-1",
                "run_trace_id": "runtrace:s-1",
                "run_trace_revision": 1,
                "trace_digest": "ef".repeat(32),
                "source_sequence": 7,
            }],
            "unavailable": [{
                "harness_id": "claude-code",
                "source_session_id": "s-2",
                "reason": "transcript unreadable",
            }],
        });
        let decoded: IndexHarnessSessionBatch = serde_json::from_value(vector.clone()).unwrap();
        assert_eq!(decoded, batch());
        assert_eq!(serde_json::to_value(&decoded).unwrap(), vector);
        let result = IndexHarnessSessionBatchResult {
            indexed: 1,
            unchanged: 0,
            unavailable: 1,
            index_revision: 4,
        };
        let wire = serde_json::to_value(&result).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"indexed": 1, "unchanged": 0, "unavailable": 1, "index_revision": 4})
        );
        assert!(serde_json::from_value::<IndexHarnessSessionBatch>(
            serde_json::json!({"tenant_id": "t1", "extra": 1})
        )
        .is_err());
    }

    // spec: MEDULLA-ORCH-VOCAB-R009.101.1
    #[test]
    fn bounds_refuse_an_oversized_or_unnamed_batch() {
        assert!(batch().check_bounds(500).is_ok());
        assert!(batch()
            .check_bounds(1)
            .unwrap_err()
            .starts_with("PARAMETER_INVALID"));
        let mut long = batch();
        long.source_cursor = "x".repeat(257);
        assert!(long.check_bounds(500).is_err());
        let mut reason = batch();
        reason.unavailable[0].reason = "r".repeat(513);
        assert!(reason.check_bounds(500).is_err());
        assert_eq!(session_node_id("t1", "h", "s"), "harnesssession:t1:h:s");
    }
}
