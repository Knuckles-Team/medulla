//! The tenant's revision challengers as a paged read
//! (MEDULLA-DECISION-ENGINE-R817): one stored row per revision of a promotable
//! subject, carrying its stage, digest and evaluation receipt.

use serde::{Deserialize, Serialize};

use super::promotion::RevisionStage;

/// The schema version of [`RevisionProposalPage`].
pub const REVISION_PROPOSAL_PAGE_SCHEMA_VERSION: u16 = 1;

/// The store key prefix of revision rows; a key is
/// `revision:<created_ms, 20 digits>:<id>`, so key order is creation order.
pub const REVISION_KEY_PREFIX: &str = "revision:";

/// One stored revision of a subject.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RevisionRow {
    pub id: String,
    /// The subject kind segment of the champion pointer key.
    pub subject_kind: String,
    /// The subject reference (the rest of the pointer key).
    pub subject_ref: String,
    pub stage: RevisionStage,
    pub digest: String,
    pub evaluation_receipt_id: Option<String>,
    pub created_ms: u64,
}

impl RevisionRow {
    /// The row's store key.
    pub fn key(&self) -> String {
        format!("{REVISION_KEY_PREFIX}{:020}:{}", self.created_ms, self.id)
    }

    /// The subject kind and reference of a `champion:<kind>:<ref>` pointer key.
    pub fn subject_of(pointer_key: &str) -> Option<(String, String)> {
        let rest = pointer_key.strip_prefix("champion:")?;
        let (kind, subject) = rest.split_once(':')?;
        Some((kind.to_string(), subject.to_string()))
    }
}

/// `Method::RevisionProposalList`: the request carries no tenant; the tenant
/// is the verified request's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RevisionProposalListRequest {
    /// The `next_cursor` of the previous page.
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// One page of revision rows in creation order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RevisionProposalPage {
    pub schema_version: u16,
    pub rows: Vec<RevisionRow>,
    pub next_cursor: Option<String>,
}

/// The cursor of `row`: its `<created_ms>:<id>` key tail.
pub fn cursor_of(row: &RevisionRow) -> String {
    format!("{:020}:{}", row.created_ms, row.id)
}

/// The store key a cursor resumes after, or `None` for a malformed cursor.
pub fn key_after_cursor(cursor: &str) -> Option<String> {
    let (ms, id) = cursor.split_once(':')?;
    let valid = ms.len() == 20 && ms.bytes().all(|b| b.is_ascii_digit()) && !id.is_empty();
    valid.then(|| format!("{REVISION_KEY_PREFIX}{cursor}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, created_ms: u64) -> RevisionRow {
        RevisionRow {
            id: id.into(),
            subject_kind: "program".into(),
            subject_ref: "p1".into(),
            stage: RevisionStage::Draft,
            digest: "sha256:aa".into(),
            evaluation_receipt_id: None,
            created_ms,
        }
    }

    #[test]
    fn cursor_resumes_after_the_row_it_names() {
        let first = row("a", 5);
        let key = key_after_cursor(&cursor_of(&first)).expect("valid cursor");
        assert_eq!(key, first.key());
    }

    #[test]
    fn malformed_cursors_are_rejected() {
        for bad in [
            "",
            "x",
            "12:a",
            "0000000000000000000x:a",
            "00000000000000000005:",
        ] {
            assert_eq!(key_after_cursor(bad), None, "{bad}");
        }
    }

    #[test]
    fn subject_is_read_from_the_pointer_key() {
        assert_eq!(
            RevisionRow::subject_of("champion:definition:agent:x:y"),
            Some(("definition".into(), "agent:x:y".into()))
        );
        assert_eq!(RevisionRow::subject_of("pointer:z"), None);
    }
}
