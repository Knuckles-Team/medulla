//! Coding-agent composition inputs and plan (MEDULLA-DECISION-ENGINE-R036.101).
//!
//! The caller hands `AgentAssemble` facts it has already verified: one coding
//! agent, the discovery receipt that proves the agent exists, and the ordered
//! skill transformations to run on it. Medulla chooses only from what is
//! supplied and never scans a path. The plan binds the agent, the skills in
//! order and the artifact contract digest the last skill must produce.

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;

/// A coding agent the caller discovered and verified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct VerifiedAgentFact {
    pub agent_id: String,
    pub revision: String,
    pub capabilities: BoundedVec<String, 64>,
    /// Digest of the discovery receipt that proves the agent; empty means the
    /// caller holds no receipt.
    pub discovery_receipt_digest: String,
    /// Seconds since the discovery that the receipt records.
    pub discovery_age_s: u64,
}

/// One skill the plan runs, with the digests of its input and output schemas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillTransformation {
    pub skill_id: String,
    pub version: String,
    pub input_schema_digest: String,
    pub output_schema_digest: String,
}

/// The composition question: one agent, skills in order, and the artifact
/// contract the last skill must produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CodingAgentRequest {
    pub agent: VerifiedAgentFact,
    pub skills: BoundedVec<SkillTransformation, 32>,
    pub artifact_contract_digest: String,
}

/// The composed plan: the agent, the skills in the order they run and the
/// artifact contract digest they produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CodingAgentPlan {
    pub agent_id: String,
    pub agent_revision: String,
    pub discovery_receipt_digest: String,
    pub skills: BoundedVec<SkillTransformation, 32>,
    pub artifact_contract_digest: String,
}

/// The assistant-profile constraint a decision was taken under, recorded in
/// the inputs so a replay reads the same allowlist
/// (MEDULLA-DECISION-ENGINE-R036.107).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProfileConstraint {
    pub profile_id: String,
    pub revision: u64,
    /// The tool candidates the allowlist removed before the solve.
    pub removed: BoundedVec<String, 64>,
    /// The capabilities the removed tools declared, so an uncovered required
    /// capability that only they could cover is reported as the profile's.
    pub blocked_capabilities: BoundedVec<String, 64>,
}
