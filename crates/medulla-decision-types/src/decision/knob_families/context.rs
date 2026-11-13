//! The `context.*` knobs (MEDULLA-DECISION-ENGINE-R137.3): the over-window
//! fraction, the recursive parallel-map limits and the default window.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;

/// The `context.*` knobs, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "context.over_window_fraction",
        unit: "fraction",
        default: KnobValue::DecimalMicros(800_000),
        min: KnobValue::DecimalMicros(100_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "context.max_depth",
        unit: "levels",
        default: KnobValue::Integer(3),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(8),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "context.max_items",
        unit: "items",
        default: KnobValue::Integer(1000),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(100_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "context.max_model_calls",
        unit: "calls",
        default: KnobValue::Integer(1000),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(100_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "context.max_cost",
        unit: "tokens",
        default: KnobValue::Integer(10_000_000),
        min: KnobValue::Integer(1000),
        max: KnobValue::Integer(1_000_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "context.default_window_tokens",
        unit: "tokens",
        default: KnobValue::Integer(128_000),
        min: KnobValue::Integer(1024),
        max: KnobValue::Integer(10_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "context.replacement_tolerance",
        unit: "score fraction",
        default: KnobValue::DecimalMicros(20_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(200_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "context.solver_node_budget",
        unit: "nodes",
        default: KnobValue::Integer(100_000),
        min: KnobValue::Integer(1000),
        max: KnobValue::Integer(5_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "context.value_floor_micro_per_token",
        unit: "micro-units",
        default: KnobValue::Integer(0),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "context.band_low",
        unit: "fraction",
        default: KnobValue::DecimalMicros(250_000),
        min: KnobValue::DecimalMicros(50_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "context.band_medium",
        unit: "fraction",
        default: KnobValue::DecimalMicros(500_000),
        min: KnobValue::DecimalMicros(50_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "context.band_high",
        unit: "fraction",
        default: KnobValue::DecimalMicros(1_000_000),
        min: KnobValue::DecimalMicros(50_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
];

/// The integer value of the knob at `index` in [`KNOBS`].
pub fn integer(policy: &DecisionPolicy, index: usize) -> u64 {
    match (policy.knob(&KNOBS[index]), KNOBS[index].default) {
        (KnobValue::Integer(v), _) => u64::try_from(v).unwrap_or(0),
        (_, KnobValue::Integer(v)) => u64::try_from(v).unwrap_or(0),
        _ => 0,
    }
}

/// The over-window fraction of `policy` as a numerator over one million.
pub fn over_window_fraction_micros(policy: &DecisionPolicy) -> u64 {
    match policy.knob(&KNOBS[0]) {
        KnobValue::DecimalMicros(v) => u64::try_from(v).unwrap_or(800_000),
        _ => 800_000,
    }
}

/// The replacement tolerance of `policy` in micro units of score.
pub fn replacement_tolerance_micros(policy: &DecisionPolicy) -> u64 {
    match policy.knob(&KNOBS[6]) {
        KnobValue::DecimalMicros(v) => u64::try_from(v).unwrap_or(20_000),
        _ => 20_000,
    }
}

/// The solver node budget of `policy` for a context package (R137.9).
pub fn solver_node_budget(policy: &DecisionPolicy) -> u64 {
    integer(policy, 7)
}

/// The least value per token a chosen package candidate must carry, in micro
/// units, under `policy` (R137.9).
pub fn value_floor_micro_per_token(policy: &DecisionPolicy) -> u64 {
    integer(policy, 8)
}

/// The fraction of the remaining tokens a task of complexity `band` may take
/// (`low`, `medium` or `high`), as a numerator over one million; `None` for any
/// other band name (R137.16).
pub fn band_fraction_micros(policy: &DecisionPolicy, band: &str) -> Option<u64> {
    let index = match band {
        "low" => 9,
        "medium" => 10,
        "high" => 11,
        _ => return None,
    };
    match (policy.knob(&KNOBS[index]), KNOBS[index].default) {
        (KnobValue::DecimalMicros(v), _) | (_, KnobValue::DecimalMicros(v)) => {
            u64::try_from(v).ok()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::knobs::KnobName;

    // spec: MEDULLA-DECISION-ENGINE-R137.3
    #[test]
    fn context_knobs_read_defaults_and_pinned_values() {
        let default = DecisionPolicy::engine_default();
        assert_eq!(over_window_fraction_micros(&default), 800_000);
        assert_eq!(integer(&default, 1), 3);
        assert_eq!(integer(&default, 5), 128_000);
        let mut pinned = DecisionPolicy::engine_default();
        pinned.knobs.insert(
            KnobName::new("context.default_window_tokens"),
            KnobValue::Integer(4096),
        );
        assert_eq!(integer(&pinned, 5), 4096);
        assert_eq!(solver_node_budget(&default), 100_000);
        assert_eq!(value_floor_micro_per_token(&default), 0);
        assert_eq!(KNOBS[7].name, "context.solver_node_budget");
        assert_eq!(KNOBS[8].name, "context.value_floor_micro_per_token");
        assert_eq!(band_fraction_micros(&default, "low"), Some(250_000));
        assert_eq!(band_fraction_micros(&default, "medium"), Some(500_000));
        assert_eq!(band_fraction_micros(&default, "high"), Some(1_000_000));
        assert_eq!(band_fraction_micros(&default, "extreme"), None);
        for spec in KNOBS {
            assert!(spec.admits(&spec.default));
        }
    }
}
