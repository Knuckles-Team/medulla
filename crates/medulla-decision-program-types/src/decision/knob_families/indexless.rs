//! The `indexless.*` knobs: the tenant ceiling an indexless retrieval budget
//! is clamped to (MEDULLA-DECISION-ENGINE-R144.7) and the per-unit operation
//! costs (MEDULLA-DECISION-ENGINE-R144.1), read from the decision policy.

use {
    medulla_decision_statistical_types::decision::knobs::KnobSafety,
    medulla_decision_statistical_types::decision::knobs::KnobSpec,
    medulla_decision_statistical_types::decision::knobs::KnobValue,
};

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

/// Cost units one `RawRead` spends per document (MEDULLA-DECISION-ENGINE-R144.1).
pub const READ_COST: KnobSpec = KnobSpec {
    name: "indexless.read_cost",
    unit: "cost units per document",
    default: KnobValue::Integer(1),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(100),
    safety: KnobSafety::Tunable,
};

/// Cost units one `CorpusScan` spends per row (MEDULLA-DECISION-ENGINE-R144.1).
pub const SCAN_COST: KnobSpec = KnobSpec {
    name: "indexless.scan_cost",
    unit: "cost units per row",
    default: KnobValue::Integer(1),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(100),
    safety: KnobSafety::Tunable,
};

/// Cost units one `VerifySpan` spends per span (MEDULLA-DECISION-ENGINE-R144.1).
pub const VERIFY_COST: KnobSpec = KnobSpec {
    name: "indexless.verify_cost",
    unit: "cost units per span",
    default: KnobValue::Integer(2),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(100),
    safety: KnobSafety::Tunable,
};

/// Every `indexless.*` knob, each `Tunable`.
pub const KNOBS: &[KnobSpec] = &[
    MAX_ROWS,
    MAX_VISITS,
    MAX_COST_UNITS,
    READ_COST,
    SCAN_COST,
    VERIFY_COST,
];
