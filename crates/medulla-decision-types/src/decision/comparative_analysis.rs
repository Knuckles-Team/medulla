//! The comparative-analysis plan model (MEDULLA-DECISION-ENGINE-R139.1).
//!
//! A budgeted discovery, synthesis and extraction plan over ranked
//! candidates of one unified query. The plan holds no generated text and no
//! receipt. Refusals are registered codes (R139.4) and premises are
//! `PremiseRef` values (R139.5), so this module declares neither. Digests are
//! SHA-256, 64 lowercase hex characters.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::numeric::QuantisedValue;
use super::policy::DecisionPolicy;
use super::record::outcome::RefusalCode;
use medulla_types::contract::BoundedVec;

/// Refusal code: a space, vector, result or candidate revision differs from the visible state.
pub const COMPARATIVE_STALE_INPUT: RefusalCode = RefusalCode::new("COMPARATIVE_STALE_INPUT");
/// Refusal code: a candidate is invisible, malformed or duplicate.
pub const COMPARATIVE_CANDIDATE_INVALID: RefusalCode =
    RefusalCode::new("COMPARATIVE_CANDIDATE_INVALID");
/// Refusal code: more candidates than the bound.
pub const COMPARATIVE_OVER_LIMIT: RefusalCode = RefusalCode::new("COMPARATIVE_OVER_LIMIT");
/// Refusal code: an importance outside [0, 1].
pub const COMPARATIVE_IMPORTANCE_INVALID: RefusalCode =
    RefusalCode::new("COMPARATIVE_IMPORTANCE_INVALID");
/// Refusal code: the extraction minimum is below the synthesis minimum.
pub const COMPARATIVE_THRESHOLD_ORDER: RefusalCode =
    RefusalCode::new("COMPARATIVE_THRESHOLD_ORDER");
/// Refusal code: a budget or ceiling is absent, zero where a call is needed, or above its knob maximum.
pub const COMPARATIVE_BUDGET_INVALID: RefusalCode = RefusalCode::new("COMPARATIVE_BUDGET_INVALID");
/// Refusal code: a stage cannot fit the remaining call ceiling.
pub const COMPARATIVE_STAGE_EXCEEDS_TOTAL: RefusalCode =
    RefusalCode::new("COMPARATIVE_STAGE_EXCEEDS_TOTAL");

/// The `REFUSALS` table of the `comparative_analysis_plan` family, registered
/// in `codes::FAMILY_TABLES` (MEDULLA-DECISION-ENGINE-R139.4).
pub const REFUSALS: &[RefusalCode] = &[
    COMPARATIVE_STALE_INPUT,
    COMPARATIVE_CANDIDATE_INVALID,
    COMPARATIVE_OVER_LIMIT,
    COMPARATIVE_IMPORTANCE_INVALID,
    COMPARATIVE_THRESHOLD_ORDER,
    COMPARATIVE_BUDGET_INVALID,
    COMPARATIVE_STAGE_EXCEEDS_TOTAL,
];

/// The `comparative.*` knobs (MEDULLA-DECISION-ENGINE-R139.2).
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "comparative.synthesis_importance_min",
        unit: "ratio",
        default: KnobValue::DecimalMicros(500_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "comparative.extraction_importance_min",
        unit: "ratio",
        default: KnobValue::DecimalMicros(800_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "comparative.synthesis_batch",
        unit: "candidates per call",
        default: KnobValue::Integer(32),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(64),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "comparative.total_call_ceiling_max",
        unit: "calls",
        default: KnobValue::Integer(66),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(130),
        safety: KnobSafety::Tunable,
    },
];

/// The resolved `comparative.*` knob values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComparativeKnobs {
    /// The least importance entering synthesis, in millionths.
    pub synthesis_importance_min_micros: i64,
    /// The least importance entering extraction, in millionths.
    pub extraction_importance_min_micros: i64,
    /// The most candidates one synthesis call takes.
    pub synthesis_batch: u32,
    /// The most calls a request's ceiling may name.
    pub total_call_ceiling_max: u32,
}

impl ComparativeKnobs {
    /// The values the policy in force resolves.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let micros = |spec: &KnobSpec| match policy.knob(spec) {
            KnobValue::DecimalMicros(v) => v,
            _ => 0,
        };
        let count = |spec: &KnobSpec| match policy.knob(spec) {
            KnobValue::Integer(v) => u32::try_from(v).unwrap_or(0),
            _ => 0,
        };
        Self {
            synthesis_importance_min_micros: micros(&KNOBS[0]),
            extraction_importance_min_micros: micros(&KNOBS[1]),
            synthesis_batch: count(&KNOBS[2]),
            total_call_ceiling_max: count(&KNOBS[3]),
        }
    }
}

/// The most candidates one comparative analysis plans over.
pub const MAX_COMPARATIVE_CANDIDATES: usize = 64;

/// One ranked candidate. `score` and `importance` are on scale Q32;
/// `importance` lies in [0, 1].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ComparativeCandidate {
    /// The candidate id.
    pub candidate_id: String,
    /// The candidate revision.
    pub candidate_revision: u64,
    /// The ranking score, Q32.
    pub score: QuantisedValue,
    /// The importance in [0, 1], Q32.
    pub importance: QuantisedValue,
}

/// The three stages of one comparative analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ComparativeAnalysisStage {
    /// Find the candidates worth comparing.
    Discovery,
    /// Synthesise across the important candidates.
    Synthesis,
    /// Extract from the most important candidates.
    Extraction,
}

/// The call budget of one stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ComparativeStageBudget {
    /// The stage.
    pub stage: ComparativeAnalysisStage,
    /// The most model calls the stage may make.
    pub max_calls: u32,
}

/// One comparative-analysis question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ComparativeAnalysisRequest {
    /// The space queried.
    pub space_id: String,
    /// The space revision read.
    pub space_revision: u64,
    /// SHA-256 of the query vector.
    pub query_vector_digest: String,
    /// SHA-256 of the unified query result.
    pub unified_query_result_digest: String,
    /// The ranked candidates.
    pub candidates: BoundedVec<ComparativeCandidate, MAX_COMPARATIVE_CANDIDATES>,
    /// The most model calls across every stage.
    pub total_call_ceiling: u32,
}

/// The budgeted plan `Decide` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ComparativeAnalysisPlan {
    /// The plan schema version.
    pub schema_version: u32,
    /// SHA-256 of the plan.
    pub plan_digest: String,
    /// The tenant the plan was made for.
    pub tenant_id: String,
    /// The space queried.
    pub space_id: String,
    /// The space revision read.
    pub space_revision: u64,
    /// SHA-256 of the query vector.
    pub query_vector_digest: String,
    /// SHA-256 of the unified query result.
    pub unified_query_result_digest: String,
    /// The candidates in plan order.
    pub candidates: BoundedVec<ComparativeCandidate, MAX_COMPARATIVE_CANDIDATES>,
    /// SHA-256 of the policy applied.
    pub policy_digest: String,
    /// The least importance a candidate needs to enter synthesis, Q32.
    pub synthesis_importance_threshold: QuantisedValue,
    /// The least importance a candidate needs to enter extraction, Q32.
    pub extraction_importance_threshold: QuantisedValue,
    /// One budget per stage.
    pub stage_budgets: BoundedVec<ComparativeStageBudget, 3>,
    /// The most model calls across every stage.
    pub total_call_ceiling: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::statistical::QuestionKind;
    use crate::decision::QuantScaleTag;

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn q32(value: i64) -> QuantisedValue {
        QuantisedValue {
            scale: QuantScaleTag::Q32,
            value,
        }
    }

    fn candidate(index: usize) -> ComparativeCandidate {
        ComparativeCandidate {
            candidate_id: format!("c{index}"),
            candidate_revision: 1,
            score: q32(1 << 31),
            importance: q32(1 << 30),
        }
    }

    fn round_trip<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
        value: &T,
    ) {
        let wire = serde_json::to_string(value).unwrap();
        assert_eq!(&serde_json::from_str::<T>(&wire).unwrap(), value);
    }

    #[test]
    fn comparative_types_round_trip() {
        let candidates = BoundedVec::new((0..64).map(candidate).collect()).unwrap();
        let request = ComparativeAnalysisRequest {
            space_id: "space:a".into(),
            space_revision: 7,
            query_vector_digest: DIGEST.into(),
            unified_query_result_digest: DIGEST.into(),
            candidates: candidates.clone(),
            total_call_ceiling: 12,
        };
        round_trip(&request);
        round_trip(&candidate(0));
        for stage in [
            ComparativeAnalysisStage::Discovery,
            ComparativeAnalysisStage::Synthesis,
            ComparativeAnalysisStage::Extraction,
        ] {
            round_trip(&ComparativeStageBudget {
                stage,
                max_calls: 4,
            });
        }
        assert_eq!(
            serde_json::to_string(&ComparativeAnalysisStage::Synthesis).unwrap(),
            "\"synthesis\""
        );
        let plan = ComparativeAnalysisPlan {
            schema_version: 1,
            plan_digest: DIGEST.into(),
            tenant_id: "tenant:a".into(),
            space_id: request.space_id.clone(),
            space_revision: 7,
            query_vector_digest: DIGEST.into(),
            unified_query_result_digest: DIGEST.into(),
            candidates,
            policy_digest: DIGEST.into(),
            synthesis_importance_threshold: q32(1 << 31),
            extraction_importance_threshold: q32(3 << 30),
            stage_budgets: BoundedVec::new(vec![
                ComparativeStageBudget {
                    stage: ComparativeAnalysisStage::Discovery,
                    max_calls: 4,
                },
                ComparativeStageBudget {
                    stage: ComparativeAnalysisStage::Synthesis,
                    max_calls: 4,
                },
                ComparativeStageBudget {
                    stage: ComparativeAnalysisStage::Extraction,
                    max_calls: 4,
                },
            ])
            .unwrap(),
            total_call_ceiling: 12,
        };
        round_trip(&plan);
        assert_eq!(
            serde_json::to_string(&QuestionKind::ComparativeAnalysisPlan).unwrap(),
            "\"comparative_analysis_plan\""
        );
        round_trip(&QuestionKind::ComparativeAnalysisPlan);

        // A 65th candidate is refused by construction and on the wire.
        assert!(
            BoundedVec::<ComparativeCandidate, MAX_COMPARATIVE_CANDIDATES>::new(
                (0..65).map(candidate).collect()
            )
            .is_err()
        );
        let mut wire = serde_json::to_value(&request).unwrap();
        wire["candidates"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::to_value(candidate(64)).unwrap());
        assert!(serde_json::from_value::<ComparativeAnalysisRequest>(wire).is_err());
    }
}
