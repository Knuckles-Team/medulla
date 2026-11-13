//! The `ranking_eval.*` knobs (MEDULLA-DECISION-ENGINE-R100.100.1).

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::DecisionPolicy;

/// Every `ranking_eval.*` knob, in the field order of [`RankingEvalKnobs`].
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "ranking_eval.max_k",
        unit: "items",
        default: KnobValue::Integer(1000),
        min: KnobValue::Integer(10),
        max: KnobValue::Integer(10000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "ranking_eval.min_algorithms",
        unit: "algorithms",
        default: KnobValue::Integer(2),
        min: KnobValue::Integer(2),
        max: KnobValue::Integer(16),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "ranking_eval.max_queries",
        unit: "queries",
        default: KnobValue::Integer(10000),
        min: KnobValue::Integer(100),
        max: KnobValue::Integer(100000),
        safety: KnobSafety::Tunable,
    },
];

/// The resolved `ranking_eval.*` values one evaluation runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RankingEvalKnobs {
    pub max_k: u32,
    pub min_algorithms: u32,
    pub max_queries: u32,
}

impl RankingEvalKnobs {
    /// Each knob the pinned policy sets, or its default.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let read = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::Integer(value) => u32::try_from(value).unwrap_or(u32::MAX),
            _ => u32::MAX,
        };
        Self {
            max_k: read(0),
            min_algorithms: read(1),
            max_queries: read(2),
        }
    }
}
