//! The model capacity question (MEDULLA-DECISION-ENGINE-R825).
//!
//! A `Decide` over `QuestionKind::ModelCapacity` reads the recorded latency
//! and outcome observations of one model endpoint, which the caller sends in
//! the request, and answers the concurrency limit the admission controllers
//! allow. The engine reads no usage ledger and no decision log.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;

/// JSON text parameter: the recorded observations, an array of
/// [`CapacityObservation`].
pub const PARAM_OBSERVATIONS: &str = "capacity_observations";
/// JSON text parameter: the [`CapacityBounds`].
pub const PARAM_BOUNDS: &str = "capacity_bounds";
/// The one option a model capacity question declares.
pub const CAPACITY_OPTION: &str = "capacity";
/// Most observations one question may carry.
pub const MAX_OBSERVATIONS: usize = 1_000;

/// Whether one call through the endpoint succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityOutcome {
    Success,
    Failure,
}

/// One recorded call: when it finished, how long it took and how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityObservation {
    pub at_ms: u64,
    pub latency_ms: u64,
    pub outcome: CapacityOutcome,
}

/// The declared limits the controllers run inside.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityBounds {
    pub group: String,
    pub initial: u32,
    pub min: u32,
    pub ceiling: u32,
    pub cell_ceiling: u32,
    pub group_slots: u32,
    pub now_ms: u64,
}

/// The endpoint circuit breaker's state at `now_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakerKind {
    Closed,
    HalfOpen,
    Open,
}

/// The answer, carried as the JSON of the `capacity:<json>` option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityAnswer {
    /// The adaptive concurrency limit.
    pub limit: u32,
    /// The limit narrowed by the breaker: open admits none, half-open one.
    pub effective_limit: u32,
    pub breaker: BreakerKind,
    /// Slots of the GPU group granted to this endpoint.
    pub granted: u32,
    pub group_slots_free: u32,
}

/// The decision-policy knobs of model capacity.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "capacity.latency_target_ms",
        unit: "ms",
        default: KnobValue::DurationMs(2_000),
        min: KnobValue::DurationMs(50),
        max: KnobValue::DurationMs(60_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "capacity.breaker_failures",
        unit: "count",
        default: KnobValue::Integer(5),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(100),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "capacity.breaker_cooldown_ms",
        unit: "ms",
        default: KnobValue::DurationMs(30_000),
        min: KnobValue::DurationMs(1_000),
        max: KnobValue::DurationMs(600_000),
        safety: KnobSafety::Tunable,
    },
];

/// The latency the controller steers toward, in milliseconds.
pub fn latency_target_ms(policy: &DecisionPolicy) -> u64 {
    match policy.knob(&KNOBS[0]) {
        KnobValue::DurationMs(ms) => ms,
        _ => 2_000,
    }
}

/// Consecutive failures that open the breaker.
pub fn breaker_failures(policy: &DecisionPolicy) -> u32 {
    match policy.knob(&KNOBS[1]) {
        KnobValue::Integer(count) => u32::try_from(count).unwrap_or(5),
        _ => 5,
    }
}

/// Milliseconds an open breaker waits before it half-opens.
pub fn breaker_cooldown_ms(policy: &DecisionPolicy) -> u64 {
    match policy.knob(&KNOBS[2]) {
        KnobValue::DurationMs(ms) => ms,
        _ => 30_000,
    }
}
