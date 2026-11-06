//! Independent ingest readback receipt (MEDULLA-REPO-INGEST-R002.100).
//!
//! After a `SourceIngestionReceipt` or `ChangeEnvelopeCommit`, a second read
//! under the caller principal (not the writer principal) compares id, content
//! digest and revision of each result. `Matched` only when every id matches.

use serde::{Deserialize, Serialize};

use crate::contract::Digest256;

/// Outcome of one readback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadbackVerdict {
    Matched,
    Mismatched,
}

/// The `:ReadbackReceipt` node payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
