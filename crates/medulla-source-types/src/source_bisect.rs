//! Bisect over the gate verdicts of a branch chain (MEDULLA-SCM-R009).
//!
//! `BisectStep` is a read: it names the changeset whose verdict the harness must record next,
//! or the culprit when the failing range is one changeset long. The engine writes nothing.

use serde::{Deserialize, Serialize};

/// `BisectStep` request. `good` is optional: absent, the last changeset below `bad` with a
/// passing verdict of `gate_id` is the lower bound.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BisectStepRequest {
    /// The repository graph.
    pub repo: String,
    /// The branch whose chain is searched.
    pub branch: String,
    /// The gate whose verdicts decide good and bad.
    pub gate_id: String,
    /// The branch commit id of the changeset the gate fails on.
    pub bad: String,
    /// The branch commit id of a changeset known to pass, when the caller knows one.
    #[serde(default)]
    pub good: Option<String>,
}

/// One changeset of the chain: its branch commit id and its changeset id (the verdict key).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BisectLink {
    /// The branch commit id of the changeset.
    pub commit: String,
    /// The changeset id, which is the key of its verdict.
    pub changeset: String,
}

/// Whether the answer asks for a verdict or names the culprit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum BisectOutcome {
    /// Run the gate on `link` and record its verdict, then ask again.
    NeedsVerdict,
    /// `link` is the first failing changeset: its predecessor passes and it fails.
    Culprit,
}

/// What `BisectStep` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BisectStepReceipt {
    /// Whether the answer asks for a verdict or names the culprit.
    pub outcome: BisectOutcome,
    /// The changeset the outcome is about.
    pub link: BisectLink,
    /// Changesets strictly between the known-good and known-bad bounds (`0` for a culprit).
    pub remaining: u64,
}

/// Why a bisect step produced no answer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BisectRefusal {
    /// No verdict covers a changeset the search needs.
    #[error("BISECT_VERDICT_COVERAGE: {0}")]
    VerdictCoverage(String),
    /// The named revision is not on the changeset chain.
    #[error("REVISION_NOT_ON_CHAIN: {0}")]
    RevisionNotOnChain(String),
    /// The request is malformed.
    #[error("INVALID_ARGUMENT: {0}")]
    InvalidArgument(String),
}

impl BisectRefusal {
    /// The stable refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::VerdictCoverage(_) => "BISECT_VERDICT_COVERAGE",
            Self::RevisionNotOnChain(_) => "REVISION_NOT_ON_CHAIN",
            Self::InvalidArgument(_) => "INVALID_ARGUMENT",
        }
    }
}
