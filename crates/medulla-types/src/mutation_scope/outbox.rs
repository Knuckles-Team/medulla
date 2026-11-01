use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::MutationScopeIdentity;

/// A projection/index/CDC/audit/lineage notification to publish after commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MutationOutboxIntent {
    pub topic: String,
    pub key: String,
    #[cfg_attr(feature = "contract-schema", schemars(with = "Vec<u8>"))]
    #[serde(with = "serde_bytes")]
    pub payload: Vec<u8>,
    pub headers: BTreeMap<String, String>,
}

/// Exact authoritative version transition committed with a durable record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CommittedVersion {
    Graph { source: u64, target: u64 },
    Native { source: u64, target: u64 },
    None,
}

impl CommittedVersion {
    pub fn checked_graph(source: u64) -> Result<Self, String> {
        Ok(Self::Graph {
            source,
            target: source
                .checked_add(1)
                .ok_or_else(|| "graph mutation version overflow".to_string())?,
        })
    }

    pub fn checked_native(source: u64) -> Result<Self, String> {
        Ok(Self::Native {
            source,
            target: source
                .checked_add(1)
                .ok_or_else(|| "native mutation version overflow".to_string())?,
        })
    }

    pub fn target(self) -> Option<u64> {
        match self {
            Self::Graph { target, .. } | Self::Native { target, .. } => Some(target),
            Self::None => None,
        }
    }
}

/// One durable outbox row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MutationOutboxRecord {
    pub schema_version: u16,
    pub batch_id: String,
    pub ordinal: u32,
    pub identity: MutationScopeIdentity,
    pub committed_version: CommittedVersion,
    /// The scope version after the batch committed. Legacy rows may omit
    /// this field and are rejected when their original order is unverifiable.
    #[serde(default)]
    pub commit_sequence: Option<u64>,
    pub intent: MutationOutboxIntent,
    pub created_at_ms: u64,
}

/// Durable lease over an outbox row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct MutationOutboxLease {
    pub record: MutationOutboxRecord,
    pub consumer: String,
    pub lease_epoch: u64,
    /// EXCLUSIVE upper bound, mirroring `medulla_transaction::outbox::rows::
    /// OutboxDelivery::lease_until_ms` (the durable row this lease is
    /// issued over): valid while `now_ms < lease_until_ms`, expired once
    /// `now_ms >= lease_until_ms`. `now_ms == lease_until_ms` is already
    /// expired, not still valid.
    pub lease_until_ms: u64,
    pub attempt: u32,
}

/// Monotonic projection watermark updated in the same transaction as outbox ack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationProjectionCursor {
    pub schema_version: u16,
    pub projection: String,
    pub identity: MutationScopeIdentity,
    pub batch_id: String,
    pub outbox_ordinal: u32,
    pub committed_version: CommittedVersion,
    pub advanced_at_ms: u64,
}

impl MutationOutboxRecord {
    pub fn validate_identity(&self) -> Result<(), String> {
        self.identity.validate_digest()
    }
}

impl MutationProjectionCursor {
    pub fn validate_identity(&self) -> Result<(), String> {
        self.identity.validate_digest()
    }
}
