//! A blueprint optimization proposal, stored with no authority
//! (MEDULLA-TYPED-PACKS-R094.6).
//!
//! A proposal pins the revision it was derived from, the digest of the payload it proposes
//! and the evidence that compares the two. It carries no publication authority and no
//! repository write grant: adopting it is a normal publication through the revision
//! lifecycle, which records the source revision. A proposal that supplies its own approval,
//! actor scope or permission is refused.

use serde::{Deserialize, Serialize};

use super::validate_text;
use crate::agent_library::AgentLibraryMutationContext;

/// Refusal: a proposal supplied its own approval, actor scope or permission.
pub const PROPOSAL_SELF_APPROVED: &str = "PROPOSAL_SELF_APPROVED";

/// Largest comparison evidence a proposal carries.
pub const MAX_PROPOSAL_COMPARISON_BYTES: usize = 16 * 1024;

/// The proposal as a tool or optimizer submits it.
///
/// `approval`, `actor_scope` and `permission` exist on the wire only so that a proposal which
/// tries to grant itself authority is refused by name; none is ever stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BlueprintProposal {
    /// The revision the proposal was derived from, at least 1.
    pub source_revision: u64,
    /// `sha256:<hex>` of the proposed payload.
    pub payload_digest: String,
    /// The evidence comparing the proposed payload with the source revision.
    pub comparison: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
}

/// Store one proposal against a component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BlueprintProposalRequest {
    pub context: AgentLibraryMutationContext,
    pub component_id: String,
    pub proposal: BlueprintProposal,
}

/// The answer to a store: `recorded` is false when the same proposal was already held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BlueprintProposalResult {
    pub component_id: String,
    pub source_revision: u64,
    pub payload_digest: String,
    pub recorded: bool,
}

fn is_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

impl BlueprintProposal {
    /// The table key of this proposal: one row per component, source revision and payload.
    pub fn key(&self, component_id: &str) -> String {
        format!(
            "blueprint-proposal:{component_id}:{}:{}",
            self.source_revision, self.payload_digest
        )
    }

    /// The stored form: exactly the three evidence fields, never an authority field.
    pub fn stored_form(&self) -> (u64, &str, &str) {
        (
            self.source_revision,
            self.payload_digest.as_str(),
            self.comparison.as_str(),
        )
    }

    /// Refuse a self-approving or malformed proposal before anything is read or written.
    pub fn validate(&self) -> Result<(), String> {
        if self.approval.is_some() || self.actor_scope.is_some() || self.permission.is_some() {
            return Err(format!(
                "{PROPOSAL_SELF_APPROVED}: a proposal carries no approval, actor scope or \
                 permission; adoption goes through the normal publication boundary"
            ));
        }
        if self.source_revision == 0 {
            return Err(
                "INVALID_ARGUMENT: a proposal names the source revision it was derived from (at \
                 least 1)"
                    .to_string(),
            );
        }
        if !is_sha256_digest(&self.payload_digest) {
            return Err(
                "INVALID_ARGUMENT: payload_digest is sha256:<64 lowercase hex characters>"
                    .to_string(),
            );
        }
        validate_text("comparison", &self.comparison)?;
        if self.comparison.len() > MAX_PROPOSAL_COMPARISON_BYTES {
            return Err(format!(
                "INVALID_ARGUMENT: comparison is at most {MAX_PROPOSAL_COMPARISON_BYTES} bytes"
            ));
        }
        Ok(())
    }
}

impl BlueprintProposalRequest {
    pub fn validate(&self) -> Result<(), String> {
        self.context.validate()?;
        validate_text("component_id", &self.component_id)?;
        self.proposal.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal() -> BlueprintProposal {
        BlueprintProposal {
            source_revision: 1,
            payload_digest: format!("sha256:{}", "a".repeat(64)),
            comparison: "scored 0.82 against 0.78 on the held-out set".to_string(),
            approval: None,
            actor_scope: None,
            permission: None,
        }
    }

    #[test]
    fn self_approval_fields_refuse_by_name() {
        assert!(proposal().validate().is_ok());
        for mutate in [
            |p: &mut BlueprintProposal| p.approval = Some("approved".into()),
            |p: &mut BlueprintProposal| p.actor_scope = Some("admin".into()),
            |p: &mut BlueprintProposal| p.permission = Some("agent:component-write".into()),
        ] {
            let mut p = proposal();
            mutate(&mut p);
            assert!(p
                .validate()
                .unwrap_err()
                .starts_with(PROPOSAL_SELF_APPROVED));
        }
    }
}
