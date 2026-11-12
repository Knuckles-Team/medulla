//! Independent ingest readback receipt (MEDULLA-REPO-INGEST-R002.100).
//!
//! After a `SourceIngestionReceipt` or `ChangeEnvelopeCommit`, a second read
//! under the caller principal (not the writer principal) compares id, content
//! digest and revision of each result. `Matched` only when every id matches.

use serde::{Deserialize, Serialize};

use crate::contract::Digest256;

/// What the writer's receipt promised for one result id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReadbackExpectation {
    pub id: String,
    pub content_digest: Digest256,
    pub revision: u64,
}

/// `Method::VerifyIngestReadback` request: the write receipt's digest and
/// result ids, and the graph revision the write committed at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct VerifyIngestReadbackRequest {
    pub write_receipt_digest: Digest256,
    pub expected_graph_revision: u64,
    pub results: Vec<ReadbackExpectation>,
}

/// Refusal code: a result id exists but the caller may not read it.
pub const READBACK_DENIED: &str = "READBACK_DENIED";
/// Refusal code: a result id is absent from the caller's view.
pub const READBACK_PARTIAL: &str = "READBACK_PARTIAL";
/// Refusal code: the graph has not reached the write's revision.
pub const READBACK_STALE: &str = "READBACK_STALE";
/// Refusal code: a result id's content digest or revision differs.
pub const READBACK_MISMATCH: &str = "READBACK_MISMATCH";

/// Outcome of one readback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ReadbackVerdict {
    Matched,
    Mismatched,
}

/// The `:ReadbackReceipt` node payload. `grant_revision` is the graph revision
/// at which the caller's read authority was evaluated; `graph_revision` is the
/// revision after the reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReadbackReceipt {
    pub write_receipt_digest: Digest256,
    pub principal: String,
    pub grant_revision: u64,
    pub graph_revision: u64,
    pub matched_ids: Vec<String>,
    pub mismatched_ids: Vec<String>,
    pub verdict: ReadbackVerdict,
}

impl ReadbackReceipt {
    /// The verdict a set of compared ids implies: `Matched` only when at least
    /// one id was compared and none mismatched.
    pub fn verdict_for(matched: &[String], mismatched: &[String]) -> ReadbackVerdict {
        if mismatched.is_empty() && !matched.is_empty() {
            ReadbackVerdict::Matched
        } else {
            ReadbackVerdict::Mismatched
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_is_matched_only_when_every_id_matches() {
        let ids = vec!["a".to_string()];
        assert_eq!(
            ReadbackReceipt::verdict_for(&ids, &[]),
            ReadbackVerdict::Matched
        );
        assert_eq!(
            ReadbackReceipt::verdict_for(&ids, &ids),
            ReadbackVerdict::Mismatched
        );
        assert_eq!(
            ReadbackReceipt::verdict_for(&[], &[]),
            ReadbackVerdict::Mismatched
        );
    }
}
