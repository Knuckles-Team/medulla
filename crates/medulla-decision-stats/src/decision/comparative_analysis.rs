//! The comparative-analysis planner (MEDULLA-DECISION-ENGINE-R139.2).
//!
//! Pure: from the caller's ranked candidates and the resolved `comparative.*`
//! knobs it budgets three stages. Discovery makes no model call and builds no
//! embedding (its budget is 0): the plan only reuses the caller's pinned
//! vector digest and result digest. Synthesis takes every candidate whose
//! importance reaches the synthesis minimum; extraction takes every candidate
//! that reaches the extraction minimum. The planner never clamps and never
//! returns a partial plan: a configuration it cannot honour is a typed
//! refusal naming the registered code.

use medulla_decision_types::decision::comparative_analysis::{
    ComparativeAnalysisPlan, ComparativeAnalysisRequest, ComparativeAnalysisStage,
    ComparativeKnobs, ComparativeStageBudget, COMPARATIVE_BUDGET_INVALID,
    COMPARATIVE_IMPORTANCE_INVALID, COMPARATIVE_STAGE_EXCEEDS_TOTAL, COMPARATIVE_THRESHOLD_ORDER,
};
use medulla_decision_types::decision::record::outcome::RefusalCode;
use medulla_decision_types::decision::{QuantScaleTag, QuantisedValue};
use medulla_query_types::solve_scalar::digest_text;
use medulla_types::contract::BoundedVec;

/// The domain of a plan digest.
pub const PLAN_DIGEST_DOMAIN: &str = "eg/comparative-analysis-plan/v1";

/// The plan schema version.
pub const PLAN_SCHEMA_VERSION: u32 = 1;

/// Why the planner refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanRefusal {
    /// A candidate importance lies outside [0, 1].
    ImportanceInvalid { candidate_id: String },
    /// The extraction minimum is below the synthesis minimum.
    ThresholdOrder,
    /// The ceiling is zero where a call is needed, or above its knob maximum,
    /// or a knob resolved to nothing usable.
    BudgetInvalid { detail: String },
    /// A stage needs more calls than the ceiling has left.
    StageExceedsTotal {
        stage: ComparativeAnalysisStage,
        requested: u32,
        remaining: u32,
    },
}

impl PlanRefusal {
    /// The registered refusal code.
    pub fn code(&self) -> RefusalCode {
        match self {
            Self::ImportanceInvalid { .. } => COMPARATIVE_IMPORTANCE_INVALID,
            Self::ThresholdOrder => COMPARATIVE_THRESHOLD_ORDER,
            Self::BudgetInvalid { .. } => COMPARATIVE_BUDGET_INVALID,
            Self::StageExceedsTotal { .. } => COMPARATIVE_STAGE_EXCEEDS_TOTAL,
        }
    }

    /// The refusal detail a caller sees after the code.
    pub fn detail(&self) -> String {
        match self {
            Self::ImportanceInvalid { candidate_id } => {
                format!("candidate {candidate_id} has an importance outside [0, 1]")
            }
            Self::ThresholdOrder => {
                "the extraction minimum is below the synthesis minimum".to_string()
            }
            Self::BudgetInvalid { detail } => detail.clone(),
            Self::StageExceedsTotal {
                stage,
                requested,
                remaining,
            } => format!(
                "stage {stage:?} needs {requested} calls and {remaining} remain under the ceiling"
            ),
        }
    }
}

const Q32_ONE: i128 = 1 << 32;

/// A millionths ratio as Q32, rounded down.
fn micros_to_q32(micros: i64) -> i64 {
    ((i128::from(micros) << 32) / 1_000_000) as i64
}

/// Whether `importance` (Q32) is at least `micros` millionths, exactly.
fn reaches(importance_q32: i64, micros: i64) -> bool {
    i128::from(importance_q32) * 1_000_000 >= i128::from(micros) << 32
}

fn q32(value: i64) -> QuantisedValue {
    QuantisedValue {
        scale: QuantScaleTag::Q32,
        value,
    }
}

/// Plan one comparative analysis.
pub fn plan_comparative_analysis(
    tenant_id: &str,
    policy_digest: &str,
    request: &ComparativeAnalysisRequest,
    knobs: &ComparativeKnobs,
) -> Result<ComparativeAnalysisPlan, PlanRefusal> {
    if knobs.extraction_importance_min_micros < knobs.synthesis_importance_min_micros {
        return Err(PlanRefusal::ThresholdOrder);
    }
    if knobs.synthesis_batch == 0 {
        return Err(PlanRefusal::BudgetInvalid {
            detail: "comparative.synthesis_batch resolved to zero".to_string(),
        });
    }
    if let Some(bad) = request.candidates.iter().find(|candidate| {
        candidate.importance.scale != QuantScaleTag::Q32
            || i128::from(candidate.importance.value) < 0
            || i128::from(candidate.importance.value) > Q32_ONE
    }) {
        return Err(PlanRefusal::ImportanceInvalid {
            candidate_id: bad.candidate_id.clone(),
        });
    }
    let count = |micros: i64| {
        let reached = request
            .candidates
            .iter()
            .filter(|candidate| reaches(candidate.importance.value, micros))
            .count();
        u32::try_from(reached).unwrap_or(u32::MAX)
    };
    let synthesis_candidates = count(knobs.synthesis_importance_min_micros);
    let extraction_candidates = count(knobs.extraction_importance_min_micros);
    let synthesis_calls = match synthesis_candidates {
        0 => 0,
        n if n <= knobs.synthesis_batch => 1,
        _ => 2,
    };
    let stages = [
        (ComparativeAnalysisStage::Discovery, 0),
        (ComparativeAnalysisStage::Synthesis, synthesis_calls),
        (ComparativeAnalysisStage::Extraction, extraction_candidates),
    ];
    let needed: u32 = stages.iter().map(|(_, calls)| *calls).sum();
    if request.total_call_ceiling > knobs.total_call_ceiling_max {
        return Err(PlanRefusal::BudgetInvalid {
            detail: format!(
                "total_call_ceiling {} is above comparative.total_call_ceiling_max {}",
                request.total_call_ceiling, knobs.total_call_ceiling_max
            ),
        });
    }
    if request.total_call_ceiling == 0 && needed > 0 {
        return Err(PlanRefusal::BudgetInvalid {
            detail: "total_call_ceiling is zero where calls are needed".to_string(),
        });
    }
    let mut remaining = request.total_call_ceiling;
    for (stage, calls) in stages {
        if calls > remaining {
            return Err(PlanRefusal::StageExceedsTotal {
                stage,
                requested: calls,
                remaining,
            });
        }
        remaining -= calls;
    }
    let mut plan = ComparativeAnalysisPlan {
        schema_version: PLAN_SCHEMA_VERSION,
        plan_digest: String::new(),
        tenant_id: tenant_id.to_string(),
        space_id: request.space_id.clone(),
        space_revision: request.space_revision,
        query_vector_digest: request.query_vector_digest.clone(),
        unified_query_result_digest: request.unified_query_result_digest.clone(),
        candidates: request.candidates.clone(),
        policy_digest: policy_digest.to_string(),
        synthesis_importance_threshold: q32(micros_to_q32(knobs.synthesis_importance_min_micros)),
        extraction_importance_threshold: q32(micros_to_q32(knobs.extraction_importance_min_micros)),
        stage_budgets: BoundedVec::new(
            stages
                .iter()
                .map(|(stage, max_calls)| ComparativeStageBudget {
                    stage: *stage,
                    max_calls: *max_calls,
                })
                .collect(),
        )
        .expect("three stages are inside the bound"),
        total_call_ceiling: request.total_call_ceiling,
    };
    let digest = digest_text(PLAN_DIGEST_DOMAIN, &plan);
    plan.plan_digest = digest.trim_start_matches("sha256:").to_string();
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::comparative_analysis::ComparativeCandidate;
    use medulla_decision_types::decision::DecisionPolicy;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn candidate(id: &str, importance_micros: i64) -> ComparativeCandidate {
        ComparativeCandidate {
            candidate_id: id.to_string(),
            candidate_revision: 1,
            score: q32(1 << 31),
            // The least Q32 value that reaches `importance_micros`.
            importance: q32((((i128::from(importance_micros) << 32) + 999_999) / 1_000_000) as i64),
        }
    }

    fn request(candidates: Vec<ComparativeCandidate>, ceiling: u32) -> ComparativeAnalysisRequest {
        ComparativeAnalysisRequest {
            space_id: "space:a".to_string(),
            space_revision: 4,
            query_vector_digest: DIGEST.to_string(),
            unified_query_result_digest: DIGEST.to_string(),
            candidates: BoundedVec::new(candidates).unwrap(),
            total_call_ceiling: ceiling,
        }
    }

    fn knobs() -> ComparativeKnobs {
        ComparativeKnobs::from_policy(&DecisionPolicy::engine_default())
    }

    fn calls(plan: &ComparativeAnalysisPlan) -> Vec<u32> {
        plan.stage_budgets.iter().map(|b| b.max_calls).collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R139.2
    #[test]
    fn thresholds_inclusive_and_budgets() {
        let k = knobs();
        assert_eq!(
            (
                k.synthesis_importance_min_micros,
                k.extraction_importance_min_micros,
                k.synthesis_batch,
                k.total_call_ceiling_max
            ),
            (500_000, 800_000, 32, 66)
        );
        // Inclusive 0.50 and 0.80: the exact minimum qualifies, a hair under does not.
        let plan = plan_comparative_analysis(
            "t",
            DIGEST,
            &request(
                vec![
                    candidate("a", 500_000),
                    candidate("b", 499_999),
                    candidate("c", 800_000),
                    candidate("d", 799_999),
                ],
                66,
            ),
            &k,
        )
        .unwrap();
        // Synthesis: a, c, d qualify (3 <= 32) so one call. Extraction: c only.
        assert_eq!(calls(&plan), vec![0, 1, 1]);
        // No qualifying synthesis candidate: no call.
        let none = plan_comparative_analysis("t", DIGEST, &request(vec![candidate("a", 1)], 5), &k)
            .unwrap();
        assert_eq!(calls(&none), vec![0, 0, 0]);
        // 32 qualify: one call. 33 qualify: two calls.
        let many = |n: usize| {
            (0..n)
                .map(|i| candidate(&format!("c{i:02}"), 500_000))
                .collect::<Vec<_>>()
        };
        let one = plan_comparative_analysis("t", DIGEST, &request(many(32), 5), &k).unwrap();
        assert_eq!(calls(&one), vec![0, 1, 0]);
        let two = plan_comparative_analysis("t", DIGEST, &request(many(33), 5), &k).unwrap();
        assert_eq!(calls(&two), vec![0, 2, 0]);
        // The stage sum may equal the ceiling and may not exceed it.
        let exact = plan_comparative_analysis(
            "t",
            DIGEST,
            &request(vec![candidate("a", 900_000), candidate("b", 900_000)], 3),
            &k,
        )
        .unwrap();
        assert_eq!(calls(&exact), vec![0, 1, 2]);
        let over = plan_comparative_analysis(
            "t",
            DIGEST,
            &request(vec![candidate("a", 900_000), candidate("b", 900_000)], 2),
            &k,
        )
        .unwrap_err();
        assert_eq!(
            over,
            PlanRefusal::StageExceedsTotal {
                stage: ComparativeAnalysisStage::Extraction,
                requested: 2,
                remaining: 1
            }
        );
        assert_eq!(over.code(), COMPARATIVE_STAGE_EXCEEDS_TOTAL);
        // Ties keep the request order, and discovery's budget is always 0.
        assert_eq!(
            exact
                .candidates
                .iter()
                .map(|c| c.candidate_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(exact
            .stage_budgets
            .iter()
            .all(|b| { b.stage != ComparativeAnalysisStage::Discovery || b.max_calls == 0 }));
        // The ceiling above its knob maximum, and a zero ceiling that needs calls.
        for ceiling in [67, 0] {
            let refused = plan_comparative_analysis(
                "t",
                DIGEST,
                &request(vec![candidate("a", 900_000)], ceiling),
                &k,
            )
            .unwrap_err();
            assert_eq!(refused.code(), COMPARATIVE_BUDGET_INVALID);
        }
        // The extraction minimum may not fall below the synthesis minimum.
        let mut inverted = k;
        inverted.extraction_importance_min_micros = 400_000;
        let refused =
            plan_comparative_analysis("t", DIGEST, &request(vec![], 5), &inverted).unwrap_err();
        assert_eq!(refused.code(), COMPARATIVE_THRESHOLD_ORDER);
        // An importance outside [0, 1] refuses.
        let mut loud = candidate("x", 500_000);
        loud.importance = q32((1 << 32) + 1);
        let refused =
            plan_comparative_analysis("t", DIGEST, &request(vec![loud], 5), &k).unwrap_err();
        assert_eq!(refused.code(), COMPARATIVE_IMPORTANCE_INVALID);
    }

    // spec: MEDULLA-DECISION-ENGINE-R139.2
    #[test]
    fn planner_replays_a_golden_vector() {
        let k = knobs();
        let asked = request(
            vec![
                candidate("alpha", 950_000),
                candidate("beta", 600_000),
                candidate("gamma", 100_000),
            ],
            10,
        );
        let first = plan_comparative_analysis("tenant-a", DIGEST, &asked, &k).unwrap();
        let again = plan_comparative_analysis("tenant-a", DIGEST, &asked, &k).unwrap();
        assert_eq!(
            serde_json::to_vec(&first).unwrap(),
            serde_json::to_vec(&again).unwrap(),
            "byte-identical output"
        );
        assert_eq!(first.plan_digest.len(), 64);
        assert_eq!(calls(&first), vec![0, 1, 1]);
        assert_eq!(first.synthesis_importance_threshold.value, 1 << 31);
        assert_eq!(first.extraction_importance_threshold.value, 3_435_973_836);
        // Every bound input moves the digest.
        let other_tenant = plan_comparative_analysis("tenant-b", DIGEST, &asked, &k).unwrap();
        assert_ne!(first.plan_digest, other_tenant.plan_digest);
        let mut moved = asked.clone();
        moved.space_revision += 1;
        let moved = plan_comparative_analysis("tenant-a", DIGEST, &moved, &k).unwrap();
        assert_ne!(first.plan_digest, moved.plan_digest);
    }
}
