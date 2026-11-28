//! The `recommend.*` knobs of the item-recommendation family
//! (MEDULLA-DECISION-ENGINE-R175.2).

use medulla_decision_statistical_types::decision::DecisionPolicy;
use {
    medulla_decision_statistical_types::decision::knobs::KnobSafety,
    medulla_decision_statistical_types::decision::knobs::KnobSpec,
    medulla_decision_statistical_types::decision::knobs::KnobValue,
};

/// Every `recommend.*` knob, in the field order of [`RecommendKnobs`].
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "recommend.levels",
        unit: "levels",
        default: KnobValue::Integer(4),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(4),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "recommend.codes_per_level",
        unit: "codes",
        default: KnobValue::Integer(256),
        min: KnobValue::Integer(2),
        max: KnobValue::Integer(256),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "recommend.kmeans_iters",
        unit: "iterations",
        default: KnobValue::Integer(20),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(100),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "recommend.pause_alpha",
        unit: "unitless",
        default: KnobValue::DecimalMicros(500_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "recommend.nearest_pull",
        unit: "unitless",
        default: KnobValue::DecimalMicros(250_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
];

/// The resolved `recommend.*` values one recommendation runs under.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecommendKnobs {
    pub levels: usize,
    pub codes_per_level: usize,
    pub kmeans_iters: usize,
    pub pause_alpha: f64,
    pub nearest_pull: f64,
}

impl RecommendKnobs {
    /// Each knob the pinned policy sets, or its default.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let integer = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::Integer(value) => usize::try_from(value).unwrap_or(0),
            _ => 0,
        };
        let decimal = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::DecimalMicros(value) => value as f64 / 1_000_000.0,
            _ => 0.0,
        };
        Self {
            levels: integer(0),
            codes_per_level: integer(1),
            kmeans_iters: integer(2),
            pause_alpha: decimal(3),
            nearest_pull: decimal(4),
        }
    }
}
