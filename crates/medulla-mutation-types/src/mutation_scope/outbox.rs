use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{DurabilityDomain, MutationScope, MutationScopeIdentity};

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

/// First product on-disk/wire schema of every mutation batch and record. Any
/// other schema is incompatible and is rejected by current-only readers.
pub const MUTATION_BATCH_VERSION: u16 = 1;
/// Most headers one outbox intent may carry.
pub const MAX_MUTATION_OUTBOX_HEADERS: usize = 100_000;
/// Largest byte total one mutation write may account.
pub const MAX_MUTATION_WRITE_BYTES: usize = 64 * 1024 * 1024;

/// Add `added` bytes to a write's running `total`, refusing past the budget.
pub fn account_write_bytes(total: &mut usize, added: usize) -> Result<(), String> {
    *total = total
        .checked_add(added)
        .filter(|count| *count <= MAX_MUTATION_WRITE_BYTES)
        .ok_or_else(|| "mutation write exceeds its byte budget".to_string())?;
    Ok(())
}

/// Refuse an empty, padded or control-character mutation text field.
pub fn validate_required(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty() || value.trim() != value || value.chars().any(char::is_control) {
        return Err(format!("{label} is invalid"));
    }
    Ok(())
}

/// Refuse a record whose schema is not [`MUTATION_BATCH_VERSION`].
pub fn validate_record_schema(version: u16, record_name: &str) -> Result<(), String> {
    if version != MUTATION_BATCH_VERSION {
        return Err(format!(
            "unsupported mutation {record_name} version {version} (expected {MUTATION_BATCH_VERSION})"
        ));
    }
    Ok(())
}

/// Refuse a committed version that does not match its typed scope.
pub fn validate_committed_scope(
    scope: &MutationScope,
    committed: CommittedVersion,
    record_name: &str,
) -> Result<(), String> {
    let valid = match (scope, committed) {
        (MutationScope::Graph { .. }, CommittedVersion::Graph { source, target }) => {
            source.checked_add(1) == Some(target)
        }
        (MutationScope::Native { .. }, CommittedVersion::Native { source, target }) => {
            source.checked_add(1) == Some(target)
        }
        (MutationScope::Native { domain, .. }, CommittedVersion::None) => matches!(
            domain,
            DurabilityDomain::ControlPlane | DurabilityDomain::Lifecycle
        ),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "mutation {record_name} committed version does not match its typed scope"
        ))
    }
}

impl MutationOutboxRecord {
    pub fn validate(&self) -> Result<(), String> {
        validate_record_schema(self.schema_version, "outbox")?;
        self.validate_identity()?;
        validate_required(&self.batch_id, "mutation outbox batch_id")?;
        validate_required(&self.intent.topic, "mutation outbox topic")?;
        validate_required(&self.intent.key, "mutation outbox key")?;
        validate_committed_scope(self.identity.scope(), self.committed_version, "outbox")
    }

    pub fn validate_write_budget(&self) -> Result<(), String> {
        self.validate()?;
        let mut bytes = 0usize;
        for size in [
            self.batch_id.len(),
            self.intent.topic.len(),
            self.intent.key.len(),
            self.intent.payload.len(),
        ] {
            account_write_bytes(&mut bytes, size)?;
        }
        if self.intent.headers.len() > MAX_MUTATION_OUTBOX_HEADERS {
            return Err("mutation write exceeds its header budget".to_string());
        }
        for (key, value) in &self.intent.headers {
            account_write_bytes(&mut bytes, key.len())?;
            account_write_bytes(&mut bytes, value.len())?;
        }
        Ok(())
    }
}

impl MutationProjectionCursor {
    pub fn validate(&self) -> Result<(), String> {
        validate_record_schema(self.schema_version, "projection cursor")?;
        self.validate_identity()?;
        validate_required(&self.projection, "mutation projection")?;
        validate_required(&self.batch_id, "mutation projection batch_id")?;
        validate_committed_scope(
            self.identity.scope(),
            self.committed_version,
            "projection cursor",
        )
    }
}
