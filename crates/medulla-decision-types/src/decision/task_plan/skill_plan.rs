//! The data types of planning over the skill library (MEDULLA-DECISION-ENGINE-R134.4).
//!
//! The behaviour, `plan_over_skills`, lives in `medulla-decision-stats` beside
//! the candidate-local BM25 it scores with; that crate sits above this one.

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;

use crate::decision::knob_families::task_plan::{
    SKILL_BM25_SATURATION, SKILL_RELEVANCE_FLOOR, SKILL_SPLIT_PROMPT_BYTES,
};
use crate::decision::knobs::KnobValue;
use crate::decision::policy::DecisionPolicy;

/// A subtask has no visible skill at or above `skill_plan.relevance_floor`.
pub const NO_SKILL_FOR_SUBTASK: &str = "NO_SKILL_FOR_SUBTASK";
/// The tenant sees no published skill at all.
pub const EMPTY_SKILL_INDEX: &str = "EMPTY_SKILL_INDEX";

/// One atomic subtask the harness produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillSubtask {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub depends_on: BoundedVec<String, 64>,
}

/// The subtasks to cover with skills. The output task is one of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SkillPlanQuery {
    pub output_task_id: String,
    pub subtasks: BoundedVec<SkillSubtask, 64>,
}

/// The knobs skill planning reads, resolved from the pinned decision policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillPlanKnobs {
    /// `skill_plan.bm25_saturation` in millionths of a BM25 unit.
    pub bm25_saturation_micros: i64,
    /// `skill_plan.relevance_floor` in millionths.
    pub relevance_floor_micros: i64,
    /// `skill_plan.split_prompt_bytes`.
    pub split_prompt_bytes: usize,
}

impl SkillPlanKnobs {
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let micros = |spec, fallback| match policy.knob(spec) {
            KnobValue::DecimalMicros(value) => value,
            _ => fallback,
        };
        let bytes = match policy.knob(&SKILL_SPLIT_PROMPT_BYTES) {
            KnobValue::Integer(value) => usize::try_from(value).unwrap_or(4096),
            _ => 4096,
        };
        Self {
            bm25_saturation_micros: micros(&SKILL_BM25_SATURATION, 2_000_000),
            relevance_floor_micros: micros(&SKILL_RELEVANCE_FLOOR, 200_000),
            split_prompt_bytes: bytes,
        }
    }
}
