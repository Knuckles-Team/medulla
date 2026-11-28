//! The `retrieval.*` decision knobs (MEDULLA-DECISION-ENGINE-R821).

use medulla_decision_statistical_types::decision::policy::DecisionPolicy;
use {
    medulla_decision_statistical_types::decision::knobs::KnobSafety,
    medulla_decision_statistical_types::decision::knobs::KnobSpec,
    medulla_decision_statistical_types::decision::knobs::KnobValue,
};

/// Index of `retrieval.adopt_margin` in [`KNOBS`].
pub const ADOPT_MARGIN: usize = 0;

/// Every `retrieval.*` decision knob. The adoption margin is `Tunable`: the
/// bar is a ratio of score, stored as micros.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "retrieval.adopt_margin",
    unit: "ratio",
    default: KnobValue::DecimalMicros(20_000),
    min: KnobValue::DecimalMicros(0),
    max: KnobValue::DecimalMicros(500_000),
    safety: KnobSafety::Tunable,
}];

/// The margin, in micros, a retrieval strategy must beat its champion by.
pub fn adopt_margin_micros(policy: &DecisionPolicy) -> i64 {
    match policy.knob(&KNOBS[ADOPT_MARGIN]) {
        KnobValue::DecimalMicros(micros) => micros,
        _ => i64::MAX,
    }
}
