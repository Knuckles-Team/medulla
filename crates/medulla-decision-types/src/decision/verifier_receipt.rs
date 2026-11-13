//! Verifier receipt model (MEDULLA-DECISION-ENGINE-R138.100.1).
//!
//! A receipt records that generated code was attacked and retested under a
//! named policy: one case per attack, each in a closed state. Pure data; the
//! check kernel lives in `medulla-decision-stats`.

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;

/// The most cases one receipt carries.
pub const MAX_VERIFIER_CASES: usize = 256;

/// Where one attack case stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum VerifierCaseState {
    /// The attack found a defect that is not fixed.
    Open,
    /// A fix was proposed and has not been retested.
    Fixed,
    /// The fix was retested against the proposed artifact and held.
    Retested,
    /// The retest failed.
    Failed,
}

/// One attack case and its outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct VerifierCase {
    pub case_id: String,
    pub kind: String,
    pub state: VerifierCaseState,
    /// The artifact the retest ran against, 64 lowercase hex characters;
    /// present once the case was retested.
    #[serde(default)]
    pub retest_artifact_digest: Option<String>,
}

/// The receipt of one verification of one artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct VerifierReceipt {
    /// The artifact verified, 64 lowercase hex characters.
    pub artifact_digest: String,
    /// The test set the artifact was verified against, 64 lowercase hex characters.
    pub tests_digest: String,
    pub attack_policy_revision: String,
    pub verifier_revision: String,
    pub cases: BoundedVec<VerifierCase, MAX_VERIFIER_CASES>,
}
