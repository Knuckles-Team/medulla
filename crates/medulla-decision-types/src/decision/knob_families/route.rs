//! The `route.*` knobs (MEDULLA-DECISION-ENGINE-R035.750.1, R035.750.2):
//! thresholds the route pipeline's rule stages read from the pinned decision
//! policy.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// Largest turn, in tokens, the fast path answers (R035.750.1).
pub const FAST_PATH_MAX_TOKENS: KnobSpec = KnobSpec {
    name: "route.fast_path_max_tokens",
    unit: "tokens",
    default: KnobValue::Integer(256),
    min: KnobValue::Integer(0),
    max: KnobValue::Integer(4096),
    safety: KnobSafety::Tunable,
};

/// Fewest judgements an option needs before team reuse may pick it (R035.750.2).
pub const TEAM_REUSE_MIN_SAMPLES: KnobSpec = KnobSpec {
    name: "route.team_reuse_min_samples",
    unit: "judgements",
    default: KnobValue::Integer(5),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(1000),
    safety: KnobSafety::Tunable,
};

/// Lowest success rate an option needs before team reuse may pick it, as a
/// fraction in micros (R035.750.2).
pub const TEAM_REUSE_MIN_RATE: KnobSpec = KnobSpec {
    name: "route.team_reuse_min_rate",
    unit: "fraction",
    default: KnobValue::DecimalMicros(800_000),
    min: KnobValue::DecimalMicros(0),
    max: KnobValue::DecimalMicros(1_000_000),
    safety: KnobSafety::Tunable,
};

/// Every `route.*` knob, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[
    FAST_PATH_MAX_TOKENS,
    TEAM_REUSE_MIN_SAMPLES,
    TEAM_REUSE_MIN_RATE,
];

/// The fast-path bound the pinned policy sets: the knob, or its default.
pub fn fast_path_max_tokens(policy: &crate::decision::policy::DecisionPolicy) -> i64 {
    match policy.knob(&FAST_PATH_MAX_TOKENS) {
        KnobValue::Integer(tokens) => tokens.clamp(0, 4096),
        _ => 256,
    }
}

/// The team-reuse floors the pinned policy sets: judgements and rate in micros,
/// each the knob or its default.
pub fn team_reuse_floors(policy: &crate::decision::policy::DecisionPolicy) -> (u64, u64) {
    let samples = match policy.knob(&TEAM_REUSE_MIN_SAMPLES) {
        KnobValue::Integer(samples) => samples.clamp(1, 1000) as u64,
        _ => 5,
    };
    let rate = match policy.knob(&TEAM_REUSE_MIN_RATE) {
        KnobValue::DecimalMicros(micros) => micros.clamp(0, 1_000_000) as u64,
        _ => 800_000,
    };
    (samples, rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::knobs::KnobName;

    // spec: MEDULLA-DECISION-ENGINE-R035.750.1
    #[test]
    fn the_fast_path_knob_is_declared_and_defaults_inside_its_bounds() {
        let spec = &KNOBS[0];
        assert_eq!(
            spec.knob_name(),
            KnobName::new("route.fast_path_max_tokens")
        );
        assert!(spec.admits(&spec.default));
        assert!(!spec.admits(&KnobValue::Integer(4097)));
        assert!(!spec.admits(&KnobValue::Integer(-1)));
        let policy = crate::decision::policy::DecisionPolicy::engine_default();
        assert_eq!(fast_path_max_tokens(&policy), 256);
    }

    // spec: MEDULLA-DECISION-ENGINE-R035.750.2
    #[test]
    fn the_team_reuse_knobs_are_declared_and_default_inside_their_bounds() {
        for spec in [&TEAM_REUSE_MIN_SAMPLES, &TEAM_REUSE_MIN_RATE] {
            assert!(spec.admits(&spec.default));
        }
        assert!(!TEAM_REUSE_MIN_SAMPLES.admits(&KnobValue::Integer(0)));
        assert!(!TEAM_REUSE_MIN_SAMPLES.admits(&KnobValue::Integer(1001)));
        assert!(!TEAM_REUSE_MIN_RATE.admits(&KnobValue::DecimalMicros(1_000_001)));
        let policy = crate::decision::policy::DecisionPolicy::engine_default();
        assert_eq!(team_reuse_floors(&policy), (5, 800_000));
    }
}
