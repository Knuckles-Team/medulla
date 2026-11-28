//! Curriculum promotion: the wire model (MEDULLA-DECISION-ENGINE-R751.750).
//!
//! A curriculum is an ordered list of stages, each with a horizon and an
//! evaluation window. `Decide` answers whether a stage's recent independent
//! outcomes clear its promotion threshold. Nothing here reads a clock or a
//! store.

use serde::{Deserialize, Serialize};

use medulla_decision_statistical_types::decision::numeric::QuantisedValue;
use medulla_decision_statistical_types::decision::policy::DecisionPolicy;
use medulla_types::refusal_code::RefusalCode;
use {
    medulla_decision_statistical_types::decision::knobs::KnobSafety,
    medulla_decision_statistical_types::decision::knobs::KnobSpec,
    medulla_decision_statistical_types::decision::knobs::KnobValue,
};

/// One stage of a curriculum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CurriculumStage {
    pub stage_id: String,
    /// The longest trajectory, in steps, the stage trains on.
    pub horizon_steps: u32,
    /// How many of the latest independently evaluated runs the stage is judged on.
    pub window_runs: u32,
}

/// When a stage is promoted and to which stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PromotionPolicy {
    /// The success rate that promotes, a ratio in `[0, 1]` on `Q32`.
    pub promote_threshold: QuantisedValue,
    pub next_stage_id: String,
}

/// The `curriculum.*` knobs (MEDULLA-DECISION-ENGINE-R751.754): the number of
/// latest independently evaluated runs a stage is judged on.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "curriculum.window_runs",
        unit: "runs",
        default: KnobValue::Integer(20),
        min: KnobValue::Integer(5),
        max: KnobValue::Integer(200),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "curriculum.promote_floor",
        unit: "ratio",
        default: KnobValue::DecimalMicros(500_000),
        min: KnobValue::DecimalMicros(500_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
];

/// The evaluation window, in runs, the policy in force sets.
pub fn window_runs(policy: &DecisionPolicy) -> u32 {
    match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(value) => u32::try_from(value).unwrap_or(20).clamp(5, 200),
        _ => 20,
    }
}

/// The body of a curriculum promotion question: the stage and its policy. The
/// stage's outcomes are read from the decision log under the option id
/// `stage_id`, never carried in the request.
/// `knob curriculum.promote_floor` in millionths: the lowest threshold a
/// stage policy may set.
pub fn promote_floor_micros(policy: &DecisionPolicy) -> i64 {
    match policy.knob(&KNOBS[1]) {
        KnobValue::DecimalMicros(value) => value.clamp(500_000, 1_000_000),
        _ => 500_000,
    }
}

/// Refusal code: fewer evaluations than the promotion window.
pub const CURRICULUM_WINDOW_INCOMPLETE: RefusalCode =
    RefusalCode::new("CURRICULUM_WINDOW_INCOMPLETE");
/// Refusal code: a promotion threshold under `knob curriculum.promote_floor`.
pub const CURRICULUM_THRESHOLD_BELOW_FLOOR: RefusalCode =
    RefusalCode::new("CURRICULUM_THRESHOLD_BELOW_FLOOR");

/// The `REFUSALS` table of the `curriculum_promotion` family, registered in
/// `code_registry::FAMILY_TABLES` (MEDULLA-DECISION-ENGINE-R751.752).
pub const REFUSALS: &[RefusalCode] = &[
    CURRICULUM_WINDOW_INCOMPLETE,
    CURRICULUM_THRESHOLD_BELOW_FLOOR,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CurriculumRequest {
    /// The stage being judged: a configuration record, not a lifecycle state.
    pub judged_stage: CurriculumStage,
    pub policy: PromotionPolicy,
}

/// Whether a stage is promoted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PromotionVerdict {
    Promote { next_stage_id: String },
    Hold,
}

/// The verdict and the window it rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PromotionReading {
    pub verdict: PromotionVerdict,
    pub successes: u32,
    pub runs: u32,
    /// The window length the policy in force required.
    pub window_runs: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_statistical_types::decision::numeric::QuantScaleTag;

    // spec: MEDULLA-DECISION-ENGINE-R751.750
    #[test]
    fn curriculum_types_round_trip() {
        let stage = CurriculumStage {
            stage_id: "short".into(),
            horizon_steps: 8,
            window_runs: 20,
        };
        let policy = PromotionPolicy {
            promote_threshold: QuantisedValue {
                scale: QuantScaleTag::Q32,
                value: 3 << 30,
            },
            next_stage_id: "long".into(),
        };
        let text = serde_json::to_string(&stage).unwrap();
        assert_eq!(
            serde_json::from_str::<CurriculumStage>(&text).unwrap(),
            stage
        );
        let text = serde_json::to_string(&policy).unwrap();
        assert_eq!(
            serde_json::from_str::<PromotionPolicy>(&text).unwrap(),
            policy
        );
        assert_eq!(
            serde_json::to_string(&medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind::CurriculumPromotion).unwrap(),
            "\"curriculum_promotion\""
        );
    }
}
