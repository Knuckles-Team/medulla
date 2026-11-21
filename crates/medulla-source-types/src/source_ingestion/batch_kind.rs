//! Batch kind and typed batch extension of `SourceIngest`
//! (MEDULLA-INGEST-WORKER-R902.3.1).
//!
//! `Records` is the classic record batch. Every other kind carries its payload
//! in exactly one matching [`SourceBatchExtension`] variant and no records,
//! relationships, withdrawals or live ids.

use medulla_types::contract::{BoundedVec, ServerErrorCode};
use serde::{Deserialize, Serialize};

use super::{SourceEntityRef, SourceIngestionBatch, SourceJson};

pub const MAX_ACCESS_SNAPSHOTS: usize = 1_024;
pub const MAX_SOURCE_GROUPS: usize = 64;
pub const MAX_ENUMERATION_IDS: usize = 16_384;
pub const MAX_CONTAINERS: usize = 1_024;
pub const MAX_PROPOSALS: usize = 64;
pub const MAX_FACTS: usize = 1_024;
pub const MAX_TRANSCRIPT_MESSAGES: usize = 1_024;
pub const MAX_SKILL_DOCUMENTS: usize = 64;
/// Most action records one `action_records` batch carries (MEDULLA-DECISION-ENGINE-R869.2).
pub const MAX_ACTION_RECORDS: usize = 256;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SourceBatchKind {
    #[default]
    Records,
    Access,
    Groups,
    Enumeration,
    Containers,
    Proposals,
    Facts,
    Transcripts,
    SkillDocuments,
    ActionRecords,
}

/// One page of an identifier-only enumeration run (MEDULLA-INGEST-WORKER-R903.2).
/// Pages of one `run_id` are numbered from zero; the page flagged `last_page`
/// closes the run. `complete` is the provider's statement that the run listed
/// every identifier, and `ids_digest` (required on the last page) is the digest
/// of the sorted identifier digests of the whole run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EnumerationPage {
    pub run_id: String,
    pub page: u32,
    pub last_page: bool,
    pub complete: bool,
    #[serde(default)]
    pub ids_digest: Option<String>,
    pub ids: BoundedVec<SourceEntityRef, MAX_ENUMERATION_IDS>,
}

/// One skill document of a `skill_documents` batch (MEDULLA-REPO-INGEST-R002.1.6): the
/// directory it belongs to and its text. A skill document is a typed item of its own
/// batch kind, never a mapped record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillDocumentItem {
    pub directory_ref: String,
    pub text: String,
}

/// One variant per non-record kind. Element payloads stay opaque JSON here;
/// the row that owns each kind types its elements at admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SourceBatchExtension {
    Access(BoundedVec<SourceJson, MAX_ACCESS_SNAPSHOTS>),
    Groups(BoundedVec<SourceJson, MAX_SOURCE_GROUPS>),
    Enumeration(EnumerationPage),
    Containers(BoundedVec<SourceJson, MAX_CONTAINERS>),
    Proposals(BoundedVec<SourceJson, MAX_PROPOSALS>),
    Facts(BoundedVec<SourceJson, MAX_FACTS>),
    Transcripts(BoundedVec<SourceJson, MAX_TRANSCRIPT_MESSAGES>),
    SkillDocuments(BoundedVec<SourceJson, MAX_SKILL_DOCUMENTS>),
    ActionRecords(BoundedVec<SourceJson, MAX_ACTION_RECORDS>),
}

impl SourceBatchExtension {
    pub fn kind(&self) -> SourceBatchKind {
        match self {
            Self::Access(_) => SourceBatchKind::Access,
            Self::Groups(_) => SourceBatchKind::Groups,
            Self::Enumeration(_) => SourceBatchKind::Enumeration,
            Self::Containers(_) => SourceBatchKind::Containers,
            Self::Proposals(_) => SourceBatchKind::Proposals,
            Self::Facts(_) => SourceBatchKind::Facts,
            Self::Transcripts(_) => SourceBatchKind::Transcripts,
            Self::SkillDocuments(_) => SourceBatchKind::SkillDocuments,
            Self::ActionRecords(_) => SourceBatchKind::ActionRecords,
        }
    }
}

fn mismatch(detail: &str) -> String {
    format!(
        "{}: {detail}",
        ServerErrorCode::SourceBatchKindMismatch.as_str()
    )
}

/// `Records` takes no extension; every other kind takes its matching variant
/// and nothing else.
pub(super) fn validate_kind(batch: &SourceIngestionBatch) -> Result<(), String> {
    let extension_kind = batch.extension.as_ref().map(SourceBatchExtension::kind);
    if batch.kind == SourceBatchKind::Records {
        return extension_kind.map_or(Ok(()), |_| {
            Err(mismatch("records batch carries an extension"))
        });
    }
    if extension_kind != Some(batch.kind) {
        return Err(mismatch("batch kind needs its matching extension"));
    }
    let live_ids = batch
        .authoritative_live_ids
        .as_ref()
        .map_or(0, |ids| ids.len());
    let carried = batch.records.len() + batch.relationships.len() + batch.withdrawals.len();
    if carried + live_ids > 0 {
        return Err(mismatch("non-record batch carries record fields"));
    }
    Ok(())
}
