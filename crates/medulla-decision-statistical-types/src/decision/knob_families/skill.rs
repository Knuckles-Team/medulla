//! The `skill.*` knobs (MEDULLA-DECISION-ENGINE-R703.2, R703.3): how skills are
//! blocked and how close two skills must be before a merge is proposed. They
//! are decision-plane thresholds, pinned through the decision policy and
//! sealed with its digest.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;

/// Most skills compared against each other in one block.
pub const MAX_BLOCK: KnobSpec = KnobSpec {
    name: "skill.max_block",
    unit: "skills",
    default: KnobValue::Integer(256),
    min: KnobValue::Integer(8),
    max: KnobValue::Integer(4096),
    safety: KnobSafety::Tunable,
};

/// Smallest Jaccard overlap of declared capability ids, in ppm.
pub const MERGE_JACCARD_MIN: KnobSpec = KnobSpec {
    name: "skill.merge_jaccard_min",
    unit: "ppm",
    default: KnobValue::Integer(600_000),
    min: KnobValue::Integer(0),
    max: KnobValue::Integer(1_000_000),
    safety: KnobSafety::Tunable,
};

/// Smallest cosine similarity of stored embeddings, in ppm.
pub const MERGE_COSINE_MIN: KnobSpec = KnobSpec {
    name: "skill.merge_cosine_min",
    unit: "ppm",
    default: KnobValue::Integer(900_000),
    min: KnobValue::Integer(0),
    max: KnobValue::Integer(1_000_000),
    safety: KnobSafety::Tunable,
};

/// Fewest uses before a term no capability resolves is a neologism
/// (MEDULLA-DECISION-ENGINE-R703.3).
pub const NEOLOGISM_MIN_USES: KnobSpec = KnobSpec {
    name: "skill.neologism_min_uses",
    unit: "uses",
    default: KnobValue::Integer(3),
    min: KnobValue::Integer(2),
    max: KnobValue::Integer(1000),
    safety: KnobSafety::Tunable,
};

/// Every `skill.*` knob, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[
    MAX_BLOCK,
    MERGE_JACCARD_MIN,
    MERGE_COSINE_MIN,
    NEOLOGISM_MIN_USES,
];

/// The pinned (or default) value of an integer knob.
pub fn integer(policy: &DecisionPolicy, spec: &KnobSpec) -> u64 {
    match policy.knob(spec) {
        KnobValue::Integer(value) => u64::try_from(value).unwrap_or(0),
        _ => u64::MAX,
    }
}
