//! The `context_search.*` knobs (MEDULLA-DECISION-ENGINE-R100.100.3).

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::DecisionPolicy;

const fn integer(
    name: &'static str,
    unit: &'static str,
    default: i64,
    min: i64,
    max: i64,
) -> KnobSpec {
    KnobSpec {
        name,
        unit,
        default: KnobValue::Integer(default),
        min: KnobValue::Integer(min),
        max: KnobValue::Integer(max),
        safety: KnobSafety::Tunable,
    }
}

/// Every `context_search.*` knob, in the field order of [`ContextSearchKnobs`].
pub const KNOBS: &[KnobSpec] = &[
    integer("context_search.context_bytes", "bytes", 160, 0, 4096),
    integer("context_search.ngram_n", "tokens", 2, 1, 8),
    integer("context_search.max_spans", "spans", 100, 1, 1000),
    integer("context_search.max_span_bytes", "bytes", 8192, 1024, 65536),
];

/// The resolved `context_search.*` values one search runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextSearchKnobs {
    pub context_bytes: u32,
    pub ngram_n: u32,
    pub max_spans: u32,
    pub max_span_bytes: u32,
}

impl ContextSearchKnobs {
    /// Each knob the pinned policy sets, or its default.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let read = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::Integer(value) => u32::try_from(value).unwrap_or(u32::MAX),
            _ => u32::MAX,
        };
        Self {
            context_bytes: read(0),
            ngram_n: read(1),
            max_spans: read(2),
            max_span_bytes: read(3),
        }
    }
}
