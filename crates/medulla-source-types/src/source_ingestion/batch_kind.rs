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
}

/// One variant per non-record kind. Element payloads stay opaque JSON here;
/// the row that owns each kind types its elements at admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SourceBatchExtension {
    Access(BoundedVec<SourceJson, MAX_ACCESS_SNAPSHOTS>),
    Groups(BoundedVec<SourceJson, MAX_SOURCE_GROUPS>),
    Enumeration(BoundedVec<SourceEntityRef, MAX_ENUMERATION_IDS>),
    Containers(BoundedVec<SourceJson, MAX_CONTAINERS>),
    Proposals(BoundedVec<SourceJson, MAX_PROPOSALS>),
    Facts(BoundedVec<SourceJson, MAX_FACTS>),
    Transcripts(BoundedVec<SourceJson, MAX_TRANSCRIPT_MESSAGES>),
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
