//! Retention-cleanup run request, report and audit record
//! (MEDULLA-DURABLE-KERNEL-R700.1).
//!
//! `RetentionPolicy` (`retention_policy.rs`) owns validation. These are the
//! wire records the `RetentionCleanupRun` handler and the maintenance tick
//! exchange. No policy means no sweep: the default age is unset.

use serde::{Deserialize, Serialize};

/// The default number of candidates one cleanup step removes.
pub const DEFAULT_RETENTION_BATCH: u32 = 1_000;

/// One `RetentionCleanupRun` call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionCleanupRunRequest {
    pub graph: String,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub cursor: Option<String>,
}

/// The outcome of one cleanup step.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionCleanupReport {
    pub scanned: u64,
    pub removed: u64,
    pub held_live_reference: u64,
    pub held_legal_hold: u64,
    pub next_cursor: Option<String>,
}

/// The durable audit record of one cleanup step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionCleanupRecord {
    pub graph: String,
    pub cursor_start: Option<String>,
    pub cursor_end: Option<String>,
    pub scanned: u64,
    pub removed: u64,
    pub held_live_reference: u64,
    pub held_legal_hold: u64,
    pub policy_digest: String,
    pub at_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DURABLE-KERNEL-R700.1
    #[test]
    fn report_and_record_round_trip() {
        let report = RetentionCleanupReport {
            scanned: 1_000,
            removed: 990,
            held_live_reference: 7,
            held_legal_hold: 3,
            next_cursor: Some("node-1000".to_owned()),
        };
        let packed = rmp_serde::to_vec_named(&report).expect("report encodes");
        assert_eq!(
            rmp_serde::from_slice::<RetentionCleanupReport>(&packed).expect("report decodes"),
            report
        );

        let record = RetentionCleanupRecord {
            graph: "main".to_owned(),
            cursor_start: None,
            cursor_end: Some("node-1000".to_owned()),
            scanned: 1_000,
            removed: 990,
            held_live_reference: 7,
            held_legal_hold: 3,
            policy_digest: "ab".repeat(32),
            at_ms: 1_793_000_000_000,
        };
        let packed = rmp_serde::to_vec_named(&record).expect("record encodes");
        assert_eq!(
            rmp_serde::from_slice::<RetentionCleanupRecord>(&packed).expect("record decodes"),
            record
        );

        let request = RetentionCleanupRunRequest {
            graph: "main".to_owned(),
            dry_run: true,
            cursor: None,
        };
        let packed = rmp_serde::to_vec_named(&request).expect("request encodes");
        assert_eq!(
            rmp_serde::from_slice::<RetentionCleanupRunRequest>(&packed).expect("request decodes"),
            request
        );
        assert_eq!(DEFAULT_RETENTION_BATCH, 1_000);
    }
}
