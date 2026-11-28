//! Replay limits of a sealed harness configuration (MEDULLA-DECISION-ENGINE-R128.13.2.2).
//!
//! The limits are decision knobs read from the pinned policy. A workload above
//! any limit refuses with `HARNESS_REPLAY_LIMIT` before the replay starts:
//! nothing is re-derived, read or written.

use super::HarnessConfigEvaluationEvidence;
use medulla_decision_statistical_types::decision::policy::DecisionPolicy;
use {
    medulla_decision_statistical_types::decision::knobs::KnobSafety,
    medulla_decision_statistical_types::decision::knobs::KnobSpec,
    medulla_decision_statistical_types::decision::knobs::KnobValue,
};

/// Refusal code of a replay workload above a limit.
pub const HARNESS_REPLAY_LIMIT: &str = "HARNESS_REPLAY_LIMIT";

/// Refusal code of a record whose stored answer or premises do not
/// re-derive (a forged answer, a changed premise, or evidence that does not
/// belong to a harness_config question).
pub const HARNESS_REPLAY_MISMATCH: &str = "HARNESS_REPLAY_MISMATCH";

const fn limit_knob(
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

/// The `harness.replay_*` knobs, in the field order of [`HarnessReplayLimits`].
pub const KNOBS: &[KnobSpec] = &[
    limit_knob("harness.replay_max_premises", "count", 1024, 16, 8192),
    limit_knob("harness.replay_max_candidates", "count", 256, 8, 2048),
    limit_knob(
        "harness.replay_max_bytes",
        "bytes",
        8 * 1024 * 1024,
        64 * 1024,
        16 * 1024 * 1024,
    ),
];

/// The three limits one replay workload is held to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessReplayLimits {
    pub max_premises: u64,
    pub max_candidates: u64,
    pub max_bytes: u64,
}

fn over(what: &str, got: u64, limit: u64) -> String {
    format!("{HARNESS_REPLAY_LIMIT}: {what} {got} exceeds {limit}")
}

impl HarnessReplayLimits {
    /// The limits the pinned policy sets, each knob or its default.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let read = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::Integer(value) => u64::try_from(value).unwrap_or(0),
            _ => 0,
        };
        Self {
            max_premises: read(0),
            max_candidates: read(1),
            max_bytes: read(2),
        }
    }

    /// Refuse a workload above any limit. `premises` is the stored premise
    /// count, `candidates` the declared option count and `bytes` the encoded
    /// size of the stored evidence.
    pub fn check(
        &self,
        premises: usize,
        evidence: &HarnessConfigEvaluationEvidence,
        bytes: usize,
    ) -> Result<(), String> {
        let premises = premises as u64;
        if premises > self.max_premises {
            return Err(over("replay premises", premises, self.max_premises));
        }
        let candidates = evidence.declared_options.as_slice().len() as u64;
        if candidates > self.max_candidates {
            return Err(over("replay candidates", candidates, self.max_candidates));
        }
        let bytes = bytes as u64;
        if bytes > self.max_bytes {
            return Err(over("replay bytes", bytes, self.max_bytes));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
