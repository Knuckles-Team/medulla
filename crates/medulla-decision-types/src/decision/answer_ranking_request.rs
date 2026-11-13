//! The request side of verifier-free answer ranking
//! (MEDULLA-DECISION-ENGINE-R134.104, R134.106).
//!
//! These types live below the ranking itself so a `Decide` request can carry
//! them as `TypedValue::AnswerRanking`; the ranking and its refusals stay in
//! `medulla-decision-outcome-types`.

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;

pub const MAX_ANSWER_CANDIDATES: usize = 16;
/// Every unordered pair of the largest candidate set.
pub const MAX_PAIRWISE_COMPARISONS: usize = MAX_ANSWER_CANDIDATES * (MAX_ANSWER_CANDIDATES - 1) / 2;

/// Which side of one comparison the caller preferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PairwisePreference {
    First,
    Second,
    Tie,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PairwiseComparison {
    pub first: String,
    pub second: String,
    pub preference: PairwisePreference,
}

/// How equal scores are ordered. The only rule is the lowest candidate id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AnswerTieRule {
    LowestCandidateId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AnswerRankingRequest {
    pub candidate_ids: BoundedVec<String, MAX_ANSWER_CANDIDATES>,
    pub comparisons: BoundedVec<PairwiseComparison, MAX_PAIRWISE_COMPARISONS>,
    /// A named verifier is refused: this ranking is verifier-free by contract.
    #[serde(default)]
    pub verifier: Option<String>,
    pub retain_top_k: u16,
    pub max_revisions: u16,
}
