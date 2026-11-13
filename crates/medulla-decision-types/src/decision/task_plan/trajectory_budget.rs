//! Complexity-gated K-trajectory decision (MEDULLA-DECISION-ENGINE-R140.100).
//!
//! The caller pins a scorer and the task features it observed. The engine
//! recomputes the scorer digest, refuses missing or stale features, scores in
//! 128-bit integer math and returns how many trajectories the branch may run:
//! one for a simple branch, a count in `[2, trajectory.max_k]` for a complex
//! one. The decision is pure; the served route reads the knobs and the clock.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use medulla_query_types::solve_scalar::digest_text;

use super::trajectory_refusal::{request_premise, TrajectoryRefusal};
use crate::decision::codes::PARAMETER_INVALID;
use crate::decision::knob_families::trajectory as knobs;
use crate::decision::policy::DecisionPolicy;
use crate::decision::record::outcome::RefusalCode;
use crate::decision::{PremiseClass, PremiseRef};

/// A scorer weight or feature is absent for a feature or weight the other names.
pub const TRAJECTORY_FEATURE_MISSING: RefusalCode = RefusalCode::new("TRAJECTORY_FEATURE_MISSING");
/// A feature was observed longer ago than `knob trajectory.feature_max_age`.
pub const TRAJECTORY_FEATURE_STALE: RefusalCode = RefusalCode::new("TRAJECTORY_FEATURE_STALE");
/// The scorer digest differs from the one the engine recomputes.
pub const TRAJECTORY_SCORER_UNPINNED: RefusalCode = RefusalCode::new("TRAJECTORY_SCORER_UNPINNED");
/// The request names no scorer.
pub const TRAJECTORY_POLICY_ABSENT: RefusalCode = RefusalCode::new("TRAJECTORY_POLICY_ABSENT");
/// The requested K is outside the range of the branch.
pub const TRAJECTORY_K_INVALID: RefusalCode = RefusalCode::new("TRAJECTORY_K_INVALID");

/// The refusal codes of the K-trajectory decision.
pub const REFUSALS: &[RefusalCode] = &[
    TRAJECTORY_FEATURE_MISSING,
    TRAJECTORY_FEATURE_STALE,
    TRAJECTORY_SCORER_UNPINNED,
    TRAJECTORY_POLICY_ABSENT,
    TRAJECTORY_K_INVALID,
];

/// Most features one request may carry.
pub const MAX_COMPLEXITY_FEATURES: usize = 64;

const SCORER_DIGEST_DOMAIN: &str = "eg/trajectory-scorer/v1";
const BUDGET_DIGEST_DOMAIN: &str = "eg/trajectory-budget/v1";
const MICRO: i128 = 1_000_000;

/// One observed task feature, valued in integer micro-units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ComplexityFeature {
    pub id: String,
    pub value: i64,
    pub unit: String,
    pub source_ref: String,
    pub source_revision: u64,
    pub source_digest: String,
    pub observed_at_ms: u64,
}

/// The weight a scorer gives one feature, in micro-units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FeatureWeight {
    pub feature_id: String,
    pub weight: i64,
}

/// A pinned scorer: one weight per feature and the digest the caller claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ComplexityScorer {
    pub id: String,
    pub revision: u32,
    pub digest: String,
    pub weights: Vec<FeatureWeight>,
}

/// The digest the engine recomputes for a scorer: SHA-256 over its id,
/// revision and weights in feature-id order, never over the claimed digest.
pub fn scorer_digest(scorer: &ComplexityScorer) -> String {
    let mut weights: Vec<(&str, i64)> = scorer
        .weights
        .iter()
        .map(|w| (w.feature_id.as_str(), w.weight))
        .collect();
    weights.sort_unstable();
    digest_text(
        SCORER_DIGEST_DOMAIN,
        &(&scorer.id, scorer.revision, weights),
    )
}

/// The question: how many trajectories may this task branch run?
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TrajectoryBudgetRequest {
    pub task_digest: String,
    pub features: Vec<ComplexityFeature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scorer: Option<ComplexityScorer>,
    /// The K the caller wants; absent asks for the largest permitted K.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_k: Option<u32>,
}

/// Which side of the threshold the score fell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum BudgetBranch {
    Simple,
    Complex,
}

/// The decision: the score, the branch and the permitted and chosen K.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TrajectoryBudget {
    pub task_digest: String,
    pub scorer_digest: String,
    pub score_micro: i128,
    pub threshold_micro: i64,
    pub branch: BudgetBranch,
    pub k_min: u32,
    pub k_max: u32,
    pub k: u32,
    pub decision_digest: String,
}

/// The knobs the decision reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrajectoryKnobs {
    pub complexity_threshold: i64,
    pub max_k: u32,
    pub feature_max_age_s: u64,
}

impl TrajectoryKnobs {
    /// The knobs a pinned policy sets, each held inside its declared range.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        Self {
            complexity_threshold: knobs::read(policy, &knobs::COMPLEXITY_THRESHOLD),
            max_k: knobs::read(policy, &knobs::MAX_K) as u32,
            feature_max_age_s: knobs::read(policy, &knobs::FEATURE_MAX_AGE) as u64,
        }
    }
}

impl Default for TrajectoryKnobs {
    fn default() -> Self {
        Self {
            complexity_threshold: 1_000_000,
            max_k: 8,
            feature_max_age_s: 3600,
        }
    }
}

fn refuse(code: RefusalCode, detail: impl Into<String>) -> TrajectoryRefusal {
    TrajectoryRefusal::new(code, detail)
}

/// Decide the K budget at `now_ms`, with the premises the decision rests on.
pub fn decide_budget(
    request: &TrajectoryBudgetRequest,
    knobs: &TrajectoryKnobs,
    now_ms: u64,
) -> Result<(TrajectoryBudget, Vec<PremiseRef>), TrajectoryRefusal> {
    if request.task_digest.is_empty() || request.features.len() > MAX_COMPLEXITY_FEATURES {
        return Err(refuse(
            PARAMETER_INVALID,
            "a budget request names its task and at most 64 features",
        ));
    }
    let Some(scorer) = &request.scorer else {
        return Err(refuse(
            TRAJECTORY_POLICY_ABSENT,
            "the request names no complexity scorer",
        ));
    };
    let recomputed = scorer_digest(scorer);
    if scorer.digest != recomputed {
        return Err(refuse(
            TRAJECTORY_SCORER_UNPINNED,
            format!(
                "scorer {} claims {} but hashes to {recomputed}",
                scorer.id, scorer.digest
            ),
        ));
    }
    let mut features: BTreeMap<&str, &ComplexityFeature> = BTreeMap::new();
    for feature in &request.features {
        if features.insert(feature.id.as_str(), feature).is_some() {
            return Err(refuse(
                PARAMETER_INVALID,
                format!("feature {} is named twice", feature.id),
            ));
        }
    }
    let mut weights: BTreeMap<&str, i64> = BTreeMap::new();
    for weight in &scorer.weights {
        if weights
            .insert(weight.feature_id.as_str(), weight.weight)
            .is_some()
        {
            return Err(refuse(
                PARAMETER_INVALID,
                format!("scorer weight {} is named twice", weight.feature_id),
            ));
        }
    }
    if let Some(id) = weights.keys().find(|id| !features.contains_key(*id)) {
        return Err(refuse(
            TRAJECTORY_FEATURE_MISSING,
            format!("the scorer weighs feature {id}, which the request lacks"),
        ));
    }
    if let Some(id) = features.keys().find(|id| !weights.contains_key(*id)) {
        return Err(refuse(
            TRAJECTORY_FEATURE_MISSING,
            format!("feature {id} has no scorer weight"),
        ));
    }
    let max_age_ms = u128::from(knobs.feature_max_age_s) * 1000;
    if let Some(stale) = features
        .values()
        .find(|f| u128::from(now_ms.saturating_sub(f.observed_at_ms)) > max_age_ms)
    {
        return Err(refuse(
            TRAJECTORY_FEATURE_STALE,
            format!(
                "feature {} was observed at {} ms; the limit is {} s",
                stale.id, stale.observed_at_ms, knobs.feature_max_age_s
            ),
        ));
    }
    let sum: i128 = features
        .iter()
        .map(|(id, feature)| i128::from(feature.value) * i128::from(weights[id]))
        .sum();
    let score = sum / MICRO;
    let branch = if score >= i128::from(knobs.complexity_threshold) {
        BudgetBranch::Complex
    } else {
        BudgetBranch::Simple
    };
    let (k_min, k_max) = match branch {
        BudgetBranch::Simple => (1, 1),
        BudgetBranch::Complex => (2, knobs.max_k),
    };
    let k = match request.requested_k {
        None => k_max,
        Some(k) if (k_min..=k_max).contains(&k) => k,
        Some(k) => {
            return Err(refuse(
                TRAJECTORY_K_INVALID,
                format!("K {k} is outside [{k_min}, {k_max}] for a {branch:?} branch"),
            ))
        }
    };
    let mut budget = TrajectoryBudget {
        task_digest: request.task_digest.clone(),
        scorer_digest: recomputed,
        score_micro: score,
        threshold_micro: knobs.complexity_threshold,
        branch,
        k_min,
        k_max,
        k,
        decision_digest: String::new(),
    };
    budget.decision_digest = digest_text(BUDGET_DIGEST_DOMAIN, &budget);
    let mut premises = vec![
        request_premise(
            "trajectory:task",
            format!("trajectory:task={}", request.task_digest),
            PremiseClass::Claim,
            "params.trajectory.task_digest",
        ),
        request_premise(
            "trajectory:scorer",
            format!(
                "trajectory:scorer={}@{};digest={}",
                scorer.id, scorer.revision, budget.scorer_digest
            ),
            PremiseClass::Claim,
            "params.trajectory.scorer",
        ),
    ];
    premises.extend(features.values().map(|f| {
        request_premise(
            &format!("trajectory:feature:{}", f.id),
            format!(
                "trajectory:feature:{}={}{};source={}@{};digest={};observed_at_ms={}",
                f.id,
                f.value,
                f.unit,
                f.source_ref,
                f.source_revision,
                f.source_digest,
                f.observed_at_ms
            ),
            PremiseClass::Claim,
            "params.trajectory.features",
        )
    }));
    premises.push(request_premise(
        "trajectory:score",
        format!(
            "trajectory:score={};threshold={};branch={:?};k={}",
            budget.score_micro, budget.threshold_micro, budget.branch, budget.k
        ),
        PremiseClass::Claim,
        "params.trajectory.features",
    ));
    Ok((budget, premises))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 10_000_000;

    fn feature(id: &str, value: i64, observed_at_ms: u64) -> ComplexityFeature {
        ComplexityFeature {
            id: id.into(),
            value,
            unit: "micro".into(),
            source_ref: format!("source:{id}"),
            source_revision: 3,
            source_digest: format!("sha256:{id}"),
            observed_at_ms,
        }
    }

    fn scorer(weights: &[(&str, i64)]) -> ComplexityScorer {
        let mut scorer = ComplexityScorer {
            id: "complexity".into(),
            revision: 1,
            digest: String::new(),
            weights: weights
                .iter()
                .map(|(id, weight)| FeatureWeight {
                    feature_id: (*id).into(),
                    weight: *weight,
                })
                .collect(),
        };
        scorer.digest = scorer_digest(&scorer);
        scorer
    }

    fn request(values: &[(&str, i64, i64)]) -> TrajectoryBudgetRequest {
        TrajectoryBudgetRequest {
            task_digest: "sha256:task".into(),
            features: values
                .iter()
                .map(|(id, value, _)| feature(id, *value, NOW))
                .collect(),
            scorer: Some(scorer(
                &values
                    .iter()
                    .map(|(id, _, weight)| (*id, *weight))
                    .collect::<Vec<_>>(),
            )),
            requested_k: None,
        }
    }

    fn code(result: Result<(TrajectoryBudget, Vec<PremiseRef>), TrajectoryRefusal>) -> RefusalCode {
        result.unwrap_err().code
    }

    // spec: MEDULLA-DECISION-ENGINE-R140.100
    #[test]
    fn score_and_branch_golden_vectors() {
        let knobs = TrajectoryKnobs::default();
        // 2_000_000 * 250_000 / 1e6 + 1_500_000 * 1_000_000 / 1e6 = 500_000 + 1_500_000.
        let complex = request(&[
            ("depth", 2_000_000, 250_000),
            ("fanout", 1_500_000, 1_000_000),
        ]);
        let (budget, premises) = decide_budget(&complex, &knobs, NOW).unwrap();
        assert_eq!(budget.score_micro, 2_000_000);
        assert_eq!(budget.branch, BudgetBranch::Complex);
        assert_eq!((budget.k_min, budget.k_max, budget.k), (2, 8, 8));
        assert_eq!(premises.first().unwrap().subject, "trajectory:task");
        assert!(premises
            .iter()
            .any(|p| p.subject == "trajectory:feature:depth"));
        assert_eq!(premises.last().unwrap().subject, "trajectory:score");
        // The threshold edge: exactly the threshold is complex, one below is simple.
        let at = request(&[("depth", 1_000_000, 1_000_000)]);
        let (edge, _) = decide_budget(&at, &knobs, NOW).unwrap();
        assert_eq!(
            (edge.score_micro, edge.branch, edge.k),
            (1_000_000, BudgetBranch::Complex, 8)
        );
        let below = request(&[("depth", 999_999, 1_000_000)]);
        let (simple, _) = decide_budget(&below, &knobs, NOW).unwrap();
        assert_eq!(
            (simple.score_micro, simple.branch, simple.k),
            (999_999, BudgetBranch::Simple, 1)
        );
        // The same request decides to the same digest; a lower max_k narrows K.
        assert_eq!(budget, decide_budget(&complex, &knobs, NOW).unwrap().0);
        let narrow = TrajectoryKnobs { max_k: 3, ..knobs };
        let mut asked = complex.clone();
        asked.requested_k = Some(3);
        assert_eq!(decide_budget(&asked, &narrow, NOW).unwrap().0.k, 3);
        assert_ne!(
            decide_budget(&complex, &narrow, NOW)
                .unwrap()
                .0
                .decision_digest,
            budget.decision_digest
        );
        // 128-bit math: a product beyond i64 does not overflow.
        let huge = request(&[("depth", i64::MAX, i64::MAX)]);
        let (big, _) = decide_budget(&huge, &knobs, NOW).unwrap();
        assert_eq!(
            big.score_micro,
            (i128::from(i64::MAX) * i128::from(i64::MAX)) / 1_000_000
        );
        // Refusals.
        let mut asked = complex.clone();
        asked.requested_k = Some(1);
        assert_eq!(
            code(decide_budget(&asked, &knobs, NOW)),
            TRAJECTORY_K_INVALID
        );
        asked.requested_k = Some(9);
        assert_eq!(
            code(decide_budget(&asked, &knobs, NOW)),
            TRAJECTORY_K_INVALID
        );
        let mut simple_k = below.clone();
        simple_k.requested_k = Some(2);
        assert_eq!(
            code(decide_budget(&simple_k, &knobs, NOW)),
            TRAJECTORY_K_INVALID
        );
        let mut none = complex.clone();
        none.scorer = None;
        assert_eq!(
            code(decide_budget(&none, &knobs, NOW)),
            TRAJECTORY_POLICY_ABSENT
        );
        let mut forged = complex.clone();
        forged.scorer.as_mut().unwrap().weights[0].weight += 1;
        assert_eq!(
            code(decide_budget(&forged, &knobs, NOW)),
            TRAJECTORY_SCORER_UNPINNED
        );
        let mut lacking = complex.clone();
        lacking.features.pop();
        assert_eq!(
            code(decide_budget(&lacking, &knobs, NOW)),
            TRAJECTORY_FEATURE_MISSING
        );
        let mut unweighted = complex.clone();
        unweighted.features.push(feature("extra", 1, NOW));
        assert_eq!(
            code(decide_budget(&unweighted, &knobs, NOW)),
            TRAJECTORY_FEATURE_MISSING
        );
        let mut stale = complex.clone();
        stale.features[0].observed_at_ms = NOW - 3_600_001;
        assert_eq!(
            code(decide_budget(&stale, &knobs, NOW)),
            TRAJECTORY_FEATURE_STALE
        );
        stale.features[0].observed_at_ms = NOW - 3_600_000;
        assert!(decide_budget(&stale, &knobs, NOW).is_ok());
        let mut twice = complex.clone();
        twice.features.push(feature("depth", 1, NOW));
        assert_eq!(code(decide_budget(&twice, &knobs, NOW)), PARAMETER_INVALID);
    }
}
