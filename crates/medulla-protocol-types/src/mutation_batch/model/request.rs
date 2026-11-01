use serde::{Deserialize, Serialize};

use crate::protocol::Method;
use medulla_mutation_types::{mutation_scope::DurabilityDomain, mutation_scope::MutationSurface};

/// Admission capabilities that materially change persistence semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum MutationCapability {
    /// Reserved for authenticated first-boot/system-recovery operations.
    UnversionedSystemMutation,
}

/// Exact commit boundaries used by the black-box release certification harness.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationCommitPhase {
    BeforeRows,
    AfterRowsBeforeMetadata,
    BeforeCommit,
    AfterCommitBeforeAck,
}

/// One ordered engine operation in a [`MutationBatch`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MutationOperation {
    pub ordinal: u32,
    pub surface: MutationSurface,
    /// Required authoritative domain.
    pub domain: DurabilityDomain,
    pub method: Method,
}
