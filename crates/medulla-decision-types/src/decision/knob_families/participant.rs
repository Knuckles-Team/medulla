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
    use crate::decision::knobs::KnobSafety;

    /// Verifies MEDULLA-DECISION-ENGINE-R179.8: Participant memory knobs.
    // spec: MEDULLA-DECISION-ENGINE-R179.8
    #[test]
    fn knob_defaults_are_inside_bounds() {
        let mut names = std::collections::BTreeSet::new();
        // The participant memory tables of R179.8 (memory, participant,
        // profile) and the topology caps; the full REGISTRY holds other
        // families' Fixed knobs, which R179.8 does not govern.
        let tables: [&[crate::decision::knobs::KnobSpec]; 4] = [
            super::super::memory::KNOBS,
            super::KNOBS,
            super::super::profile::KNOBS,
            super::super::topology::KNOBS,
        ];
        for spec in tables.iter().flat_map(|family| family.iter()) {
            assert!(spec.admits(&spec.default), "{} default", spec.name);
            assert!(spec.admits(&spec.min) && spec.admits(&spec.max));
            // R179.8: each entry is Tunable unless stated; R154.1 states the
            // two memory admission bounds Fixed.
            let fixed = matches!(
                spec.name,
                "memory.entry_max_bytes" | "memory.tenant_max_bytes"
            );
            let expected = if fixed {
                KnobSafety::Fixed
            } else {
                KnobSafety::Tunable
            };
            assert_eq!(spec.safety, expected, "{}", spec.name);
            assert!(names.insert(spec.name), "{} repeats", spec.name);
        }
        // 6 memory (4 of R179.8, 2 of R154.1), 6 participant, 2 profile, 3 topology.
        assert_eq!(names.len(), 17);
        for prefix in ["memory.", "participant.", "profile.", "topology."] {
            assert!(names.iter().any(|n| n.starts_with(prefix)));
        }
    }
}
