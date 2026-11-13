//! The `indexless.*` knobs (MEDULLA-DECISION-ENGINE-R144.7): the tenant
//! ceiling an indexless retrieval budget is clamped to, read from the pinned
//! decision policy.

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// Most rows one indexless request may visit.
pub const MAX_ROWS: KnobSpec = KnobSpec {
    name: "indexless.max_rows",
    unit: "rows",
    default: KnobValue::Integer(10_000),
    min: KnobValue::Integer(100),
    max: KnobValue::Integer(1_000_000),
    safety: KnobSafety::Tunable,
};

/// Most visits one indexless request may make.
pub const MAX_VISITS: KnobSpec = KnobSpec {
    name: "indexless.max_visits",
    unit: "visits",
    default: KnobValue::Integer(100_000),
    min: KnobValue::Integer(1_000),
    max: KnobValue::Integer(10_000_000),
    safety: KnobSafety::Tunable,
};

/// Most cost units one indexless request may spend.
pub const MAX_COST_UNITS: KnobSpec = KnobSpec {
    name: "indexless.max_cost_units",
    unit: "cost units",
    default: KnobValue::Integer(1_000_000),
    min: KnobValue::Integer(10_000),
    max: KnobValue::Integer(100_000_000),
    safety: KnobSafety::Tunable,
};

/// Every `indexless.*` knob, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[MAX_ROWS, MAX_VISITS, MAX_COST_UNITS];
