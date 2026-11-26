//! The `episodes.*` knobs (MEDULLA-DECISION-ENGINE-R155.3).

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

const MINUTE_MS: u64 = 60_000;

/// The silence between two transcript turns that closes an episode.
pub const GAP: KnobSpec = KnobSpec {
    name: "episodes.gap",
    unit: "ms",
    default: KnobValue::DurationMs(30 * MINUTE_MS),
    min: KnobValue::DurationMs(MINUTE_MS),
    max: KnobValue::DurationMs(1_440 * MINUTE_MS),
    safety: KnobSafety::Tunable,
};

/// The episode knobs.
pub const KNOBS: &[KnobSpec] = &[GAP];

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R155.3
    #[test]
    fn gap_knob_is_bounded_and_tunable() {
        assert!(GAP.admits(&GAP.default));
        assert_eq!(GAP.default, KnobValue::DurationMs(1_800_000));
        assert_eq!(GAP.safety, KnobSafety::Tunable);
        assert!(!GAP.admits(&KnobValue::DurationMs(MINUTE_MS - 1)));
        assert!(!GAP.admits(&KnobValue::DurationMs(1_440 * MINUTE_MS + 1)));
    }
}
