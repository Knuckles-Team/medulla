//! The signed agent-turn envelope (MEDULLA-DURABLE-KERNEL-R750.750.1).
//!
//! The envelope carries the admitted tool subset and the harness decision
//! reference. The dispatch carrier wraps it with a detached signature that
//! covers every field, so a worker verifies the subset and the decision
//! before it claims the turn. Signing and verification live with the carrier
//! signing of `medulla-server-auth`; this module owns the shape, the bounds
//! and the canonical digest.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Most tool names one envelope may admit.
pub const MAX_ALLOWED_TOOLS: usize = 64;
/// Longest tool name one envelope may admit.
pub const MAX_TOOL_NAME_BYTES: usize = 256;
/// Longest decision record reference.
pub const MAX_DECISION_REF_BYTES: usize = 512;
/// Metadata key a `SubmitWorkItem` of kind `agent_turn` carries the carrier under.
pub const DISPATCH_CARRIER_METADATA_KEY: &str = "dispatch_carrier";

/// Why an envelope or carrier is refused. Each variant is a registered
/// `ServerErrorCode`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentTurnRefusal {
    /// A field is absent on a turn that declares a harness decision.
    #[error("ENVELOPE_FIELD_MISSING: {0}")]
    EnvelopeFieldMissing(&'static str),
    /// A field is present but outside its bound.
    #[error("INVALID_ARGUMENT: {0}")]
    FieldInvalid(&'static str),
    /// The signature does not verify over the current field values.
    #[error("CARRIER_SIGNATURE_INVALID: signature does not cover the envelope")]
    CarrierSignatureInvalid,
}

impl AgentTurnRefusal {
    /// The wire token this refusal answers with.
    pub fn code(&self) -> &'static str {
        match self {
            Self::EnvelopeFieldMissing(_) => "ENVELOPE_FIELD_MISSING",
            Self::FieldInvalid(_) => "INVALID_ARGUMENT",
            Self::CarrierSignatureInvalid => "CARRIER_SIGNATURE_INVALID",
        }
    }
}

/// One agent turn as admitted for dispatch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AgentTurnEnvelope {
    /// The run this turn belongs to.
    pub run_id: String,
    /// Whether the turn declares a harness decision; when true the three
    /// fields below are required.
    #[serde(default)]
    pub declares_harness_decision: bool,
    /// The admitted tool subset, at most [`MAX_ALLOWED_TOOLS`] names.
    #[serde(default)]
    pub allowed_tool_subset: Option<Vec<String>>,
    /// Reference to the decision record that admitted the subset.
    #[serde(default)]
    pub decision_record_ref: Option<String>,
    /// SHA-256 of the harness configuration, 64 lowercase hex characters.
    #[serde(default)]
    pub harness_config_digest: Option<String>,
}

/// An envelope with the detached signature of its dispatching signer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DispatchCarrier {
    /// The signed envelope.
    pub envelope: AgentTurnEnvelope,
    /// `signer_id:hex_hmac` over [`AgentTurnEnvelope::digest`].
    pub signature: String,
}

fn lower_hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl AgentTurnEnvelope {
    /// Check presence and bounds. A field that is present is always bounded;
    /// a field that is absent refuses only when the turn declares a decision.
    pub fn validate(&self) -> Result<(), AgentTurnRefusal> {
        if self.run_id.trim().is_empty() || self.run_id.len() > 512 {
            return Err(AgentTurnRefusal::FieldInvalid(
                "run_id is blank or too long",
            ));
        }
        if self.declares_harness_decision {
            for (name, absent) in [
                ("allowed_tool_subset", self.allowed_tool_subset.is_none()),
                ("decision_record_ref", self.decision_record_ref.is_none()),
                (
                    "harness_config_digest",
                    self.harness_config_digest.is_none(),
                ),
            ] {
                if absent {
                    return Err(AgentTurnRefusal::EnvelopeFieldMissing(name));
                }
            }
        }
        if let Some(tools) = &self.allowed_tool_subset {
            if tools.len() > MAX_ALLOWED_TOOLS
                || tools
                    .iter()
                    .any(|t| t.trim().is_empty() || t.len() > MAX_TOOL_NAME_BYTES)
            {
                return Err(AgentTurnRefusal::FieldInvalid(
                    "allowed_tool_subset exceeds 64 names or holds a blank name",
                ));
            }
        }
        if let Some(reference) = &self.decision_record_ref {
            if reference.trim().is_empty() || reference.len() > MAX_DECISION_REF_BYTES {
                return Err(AgentTurnRefusal::FieldInvalid(
                    "decision_record_ref is blank or too long",
                ));
            }
        }
        if let Some(digest) = &self.harness_config_digest {
            if !lower_hex64(digest) {
                return Err(AgentTurnRefusal::FieldInvalid(
                    "harness_config_digest must be 64 lowercase hex characters",
                ));
            }
        }
        Ok(())
    }

    /// Canonical digest over every field. Each field is length-prefixed and
    /// an absent optional is distinct from an empty one, so no change to any
    /// field keeps the digest.
    pub fn digest(&self) -> [u8; 32] {
        fn field(hasher: &mut Sha256, bytes: &[u8]) {
            hasher.update((bytes.len() as u64).to_be_bytes());
            hasher.update(bytes);
        }
        fn optional(hasher: &mut Sha256, value: Option<&str>) {
            match value {
                Some(v) => {
                    hasher.update([1]);
                    field(hasher, v.as_bytes());
                }
                None => hasher.update([0]),
            }
        }
        let mut hasher = Sha256::new();
        field(&mut hasher, b"medulla/agent-turn-envelope/v1");
        field(&mut hasher, self.run_id.as_bytes());
        hasher.update([u8::from(self.declares_harness_decision)]);
        match &self.allowed_tool_subset {
            Some(tools) => {
                hasher.update([1]);
                hasher.update((tools.len() as u64).to_be_bytes());
                for tool in tools {
                    field(&mut hasher, tool.as_bytes());
                }
            }
            None => hasher.update([0]),
        }
        optional(&mut hasher, self.decision_record_ref.as_deref());
        optional(&mut hasher, self.harness_config_digest.as_deref());
        hasher.finalize().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope() -> AgentTurnEnvelope {
        AgentTurnEnvelope {
            run_id: "run-1".into(),
            declares_harness_decision: true,
            allowed_tool_subset: Some(vec!["read".into(), "search".into()]),
            decision_record_ref: Some("decision:1".into()),
            harness_config_digest: Some("a".repeat(64)),
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R750.750.1
    #[test]
    fn every_field_changes_the_digest_and_missing_fields_refuse() {
        let base = envelope();
        base.validate().unwrap();
        let mut changed = vec![];
        let mut e = base.clone();
        e.allowed_tool_subset = Some(vec!["read".into()]);
        changed.push(e);
        let mut e = base.clone();
        e.decision_record_ref = Some("decision:2".into());
        changed.push(e);
        let mut e = base.clone();
        e.harness_config_digest = Some("b".repeat(64));
        changed.push(e);
        for other in changed {
            assert_ne!(other.digest(), base.digest());
        }
        for clear in 0..3 {
            let mut e = base.clone();
            match clear {
                0 => e.allowed_tool_subset = None,
                1 => e.decision_record_ref = None,
                _ => e.harness_config_digest = None,
            }
            assert_eq!(e.validate().unwrap_err().code(), "ENVELOPE_FIELD_MISSING");
        }
        let mut e = base;
        e.allowed_tool_subset = Some((0..65).map(|i| format!("t{i}")).collect());
        assert_eq!(e.validate().unwrap_err().code(), "INVALID_ARGUMENT");
    }
}
