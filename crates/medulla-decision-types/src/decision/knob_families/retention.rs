//! The `retention.*` knobs (MEDULLA-DECISION-ENGINE-R066.4).

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

const DAY_MS: u64 = 86_400_000;

/// Index of `retention.graph_summary_expire_after` in [`KNOBS`].
pub const GRAPH_SUMMARY_EXPIRE_AFTER: usize = 0;

/// Every `retention.*` knob. Expiry deletes a log entry, which cannot be
/// undone, so the knob is `Fixed`: a knob-set revision never moves it; only a
/// pinned policy sets it.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "retention.graph_summary_expire_after",
    unit: "d",
    default: KnobValue::DurationMs(90 * DAY_MS),
    min: KnobValue::DurationMs(DAY_MS),
    max: KnobValue::DurationMs(3650 * DAY_MS),
    safety: KnobSafety::Fixed,
}];
