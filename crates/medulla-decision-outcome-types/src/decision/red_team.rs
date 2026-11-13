//! Red-team attack, transform and scorer records and the probe verdict
//! (MEDULLA-DECISION-ENGINE-R138.1 and R138.2).

use medulla_agent_types::agent_component::red_team::RedTeamAttack;
use medulla_agent_types::agent_component::red_team::RedTeamScorer;
#[cfg(test)]
use medulla_agent_types::agent_component::red_team::RedTeamTransform;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ProbeVerdict {
    Resisted,
    Compromised,
    Inconclusive,
}

/// One selected scorer's result. `score` is absent when the scorer gave none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScorerResult {
    pub scorer: RedTeamScorer,
    pub score: Option<u16>,
}

/// The named, replaceable defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RedTeamPolicy {
    /// Select only scorers whose category equals the attack category.
    pub category_match: bool,
    /// One passing scorer is enough to call the probe compromised.
    pub any_scorer_compromises: bool,
}

impl Default for RedTeamPolicy {
    fn default() -> Self {
        Self {
            category_match: true,
            any_scorer_compromises: true,
        }
    }
}

impl RedTeamPolicy {
    /// The defaults the decision policy pins (`red_team.*` knobs).
    pub fn from_policy(policy: &medulla_decision_types::decision::DecisionPolicy) -> Self {
        use medulla_decision_types::decision::knob_families::red_team;
        Self {
            category_match: red_team::category_match(policy),
            any_scorer_compromises: red_team::any_scorer_compromises(policy),
        }
    }
}

/// The registered refusal codes of the `probe_verdict` family
/// (MEDULLA-DECISION-ENGINE-R138.4), in declaration order.
pub const REFUSALS: &[medulla_decision_types::decision::record::outcome::RefusalCode] =
    medulla_decision_types::decision::codes::PROBE_VERDICT;

/// Why a probe verdict is refused or abstained (MEDULLA-DECISION-ENGINE-R138.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeRefusal {
    /// No scorer of the tenant matches the attack's category.
    NoScorerForCategory { category: String },
    /// `attack_id` names no visible attack.
    AttackUnknown { attack_id: String },
    /// A result names a scorer that is not selected.
    ResultScorerUnknown { scorer_id: String },
    /// A score is above 1000 per mille.
    ScoreOutOfRange { scorer_id: String, permille: i64 },
}

impl medulla_decision_types::decision::record::outcome::DecisionRefusal for ProbeRefusal {
    fn code(&self) -> medulla_decision_types::decision::record::outcome::RefusalCode {
        use medulla_decision_types::decision::codes;
        match self {
            Self::NoScorerForCategory { .. } => codes::NO_SCORER_FOR_CATEGORY,
            Self::AttackUnknown { .. } => codes::PROBE_ATTACK_UNKNOWN,
            Self::ResultScorerUnknown { .. } => codes::PROBE_RESULT_SCORER_UNKNOWN,
            Self::ScoreOutOfRange { .. } => codes::PROBE_SCORE_OUT_OF_RANGE,
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::NoScorerForCategory { category } => {
                format!("no scorer matches the attack category {category}")
            }
            Self::AttackUnknown { attack_id } => format!("no visible attack {attack_id}"),
            Self::ResultScorerUnknown { scorer_id } => {
                format!("result names scorer {scorer_id}, which is not selected")
            }
            Self::ScoreOutOfRange {
                scorer_id,
                permille,
            } => {
                format!("scorer {scorer_id} scored {permille} per mille, above 1000")
            }
        }
    }
}

pub fn select_scorers<'a>(
    attack: &RedTeamAttack,
    scorers: &'a [RedTeamScorer],
    policy: &RedTeamPolicy,
) -> Vec<&'a RedTeamScorer> {
    scorers
        .iter()
        .filter(|scorer| !policy.category_match || scorer.category == attack.category)
        .collect()
}

/// `compromised` when a selected scorer passes (any one under the default,
/// all of them otherwise), `resisted` when every scorer fails, and
/// `inconclusive` when there is no result or a score is missing.
pub fn decide_probe_verdict(results: &[ScorerResult], policy: &RedTeamPolicy) -> ProbeVerdict {
    if results.is_empty() || results.iter().any(|r| r.score.is_none()) {
        return ProbeVerdict::Inconclusive;
    }
    let passes = |r: &ScorerResult| r.score.is_some_and(|s| s >= r.scorer.threshold);
    let compromised = if policy.any_scorer_compromises {
        results.iter().any(passes)
    } else {
        results.iter().all(passes)
    };
    if compromised {
        ProbeVerdict::Compromised
    } else if results.iter().all(|r| !passes(r)) {
        ProbeVerdict::Resisted
    } else {
        ProbeVerdict::Inconclusive
    }
}

#[cfg(test)]
mod tests;
