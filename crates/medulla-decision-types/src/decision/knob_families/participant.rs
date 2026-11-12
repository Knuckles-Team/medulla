//! The `participant.*` knobs (MEDULLA-DECISION-ENGINE-R179.8).

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// Every `participant.*` knob, each `Tunable` unless stated.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "participant.card_token_budget",
        unit: "tokens",
        default: KnobValue::Integer(400),
        min: KnobValue::Integer(64),
        max: KnobValue::Integer(4000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "participant.card_recency_half_life",
        unit: "ms",
        default: KnobValue::DurationMs(2592000000),
        min: KnobValue::DurationMs(86400000),
        max: KnobValue::DurationMs(31536000000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "participant.ask_max_claims",
        unit: "claims",
        default: KnobValue::Integer(20),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(100),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "participant.ask_question_max_chars",
        unit: "characters",
        default: KnobValue::Integer(500),
        min: KnobValue::Integer(50),
        max: KnobValue::Integer(2000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "participant.scope_max_sessions",
        unit: "sessions",
        default: KnobValue::Integer(500),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(10000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "participant.events_pull_max",
        unit: "events",
        default: KnobValue::Integer(100),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(1000),
        safety: KnobSafety::Tunable,
    },
];

#[cfg(test)]
mod tests {
    use crate::decision::knob_families::REGISTRY;
    use crate::decision::knobs::KnobSafety;

    /// Verifies MEDULLA-DECISION-ENGINE-R179.8: Participant memory knobs.
    // spec: MEDULLA-DECISION-ENGINE-R179.8
    #[test]
    fn knob_defaults_are_inside_bounds() {
        let mut names = std::collections::BTreeSet::new();
        for spec in REGISTRY.iter().flat_map(|family| family.iter()) {
            assert!(spec.admits(&spec.default), "{} default", spec.name);
            assert!(spec.admits(&spec.min) && spec.admits(&spec.max));
            assert_eq!(spec.safety, KnobSafety::Tunable, "{}", spec.name);
            assert!(names.insert(spec.name), "{} repeats", spec.name);
        }
        assert_eq!(names.len(), 15);
        for prefix in ["memory.", "participant.", "profile.", "topology."] {
            assert!(names.iter().any(|n| n.starts_with(prefix)));
        }
    }
}
