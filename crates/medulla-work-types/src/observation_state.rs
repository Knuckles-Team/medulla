//! Run-bound observation state (MEDULLA-DECISION-ENGINE-R816).
//!
//! A run publishes the digest of what it observed at one frame and revision
//! of one source; a later read of the same key answers with the stored record
//! while it is live. The tenant of a record is the verified request tenant,
//! never a request field.
use serde::{Deserialize, Serialize};

/// Longest `run_id` and `source`, in characters.
pub const MAX_KEY_CHARS: usize = 128;
/// How far from the authoritative clock `expires_at_ms` may lie: 24 hours.
pub const MAX_EXPIRY_SKEW_MS: u64 = 24 * 60 * 60 * 1000;
/// Graph node id prefix of one stored record.
pub const NODE_PREFIX: &str = "observation_state:";

/// One stored observation (MEDULLA-DECISION-ENGINE-R816).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ObservationRecord {
    pub run_id: String,
    pub tenant_id: String,
    pub source: String,
    pub frame: u64,
    pub revision: u64,
    /// 64 lowercase hex characters.
    pub payload_digest: String,
    pub expires_at_ms: u64,
}

/// What a caller sends to store one observation: the record without its
/// tenant, which the server takes from the verified context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ObservationInput {
    pub run_id: String,
    pub source: String,
    pub frame: u64,
    pub revision: u64,
    pub payload_digest: String,
    pub expires_at_ms: u64,
}

/// The key of one observation inside the caller's tenant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ObservationKey {
    pub run_id: String,
    pub source: String,
    pub frame: u64,
    pub revision: u64,
}

/// `Method::ObservationState` operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ObservationStateOp {
    Put { record: ObservationInput },
    Get { key: ObservationKey },
}

impl ObservationStateOp {
    /// Whether this operation changes durable state.
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Put { .. })
    }
}

fn check_text(name: &str, value: &str) -> Result<(), String> {
    let length = value.chars().count();
    if length == 0 || length > MAX_KEY_CHARS {
        return Err(format!(
            "{name} must be 1 to {MAX_KEY_CHARS} characters, got {length}"
        ));
    }
    Ok(())
}

impl ObservationKey {
    /// The key is well formed: `run_id` and `source` are 1 to 128 characters.
    pub fn check(&self) -> Result<(), String> {
        check_text("run_id", &self.run_id)?;
        check_text("source", &self.source)
    }

    /// The graph node id of this key inside `tenant`.
    pub fn node_id(&self, tenant: &str) -> String {
        format!(
            "{NODE_PREFIX}{tenant}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            self.run_id, self.source, self.frame, self.revision
        )
    }
}

impl ObservationInput {
    /// The key this input is stored under.
    pub fn key(&self) -> ObservationKey {
        ObservationKey {
            run_id: self.run_id.clone(),
            source: self.source.clone(),
            frame: self.frame,
            revision: self.revision,
        }
    }

    /// Shape and expiry window against the authoritative clock. The error is
    /// the detail of a `PARAMETER_INVALID` refusal.
    pub fn check(&self, now_ms: u64) -> Result<(), String> {
        self.key().check()?;
        let digest_ok = self.payload_digest.len() == 64
            && self
                .payload_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !digest_ok {
            return Err("payload_digest must be 64 lowercase hex characters".to_string());
        }
        if self.expires_at_ms.abs_diff(now_ms) > MAX_EXPIRY_SKEW_MS {
            return Err(format!(
                "expires_at_ms {} lies more than 24 hours from the authoritative clock {now_ms}",
                self.expires_at_ms
            ));
        }
        Ok(())
    }

    /// The record stored for `tenant`.
    pub fn into_record(self, tenant: &str) -> ObservationRecord {
        ObservationRecord {
            run_id: self.run_id,
            tenant_id: tenant.to_string(),
            source: self.source,
            frame: self.frame,
            revision: self.revision,
            payload_digest: self.payload_digest,
            expires_at_ms: self.expires_at_ms,
        }
    }
}

impl ObservationRecord {
    /// A record is live while the authoritative clock is before its expiry.
    pub fn is_live(&self, now_ms: u64) -> bool {
        now_ms < self.expires_at_ms
    }
}
