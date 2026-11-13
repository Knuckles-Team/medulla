//! Process remediation verdict (MEDULLA-DECISION-ENGINE-R820).
//!
//! A run-bound process is `zombie` when it made no progress for
//! `process.zombie_after` and holds no child output, `hung` when it ran without
//! progress for `process.hung_after`, and `progressing` otherwise. The clock is
//! the engine's own; the evidence carries only when it was observed. Evidence
//! older than `process.evidence_max_age`, or observed in the future, is
//! refused. Advisory; `Decide` writes nothing.

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use super::record::outcome::RefusalCode;

/// Text parameter: the run the question is about.
pub const PARAM_RUN_ID: &str = "run_id";
/// Text parameter: the run the evidence was recorded for.
pub const PARAM_EVIDENCE_RUN_ID: &str = "evidence_run_id";
/// Integer parameter: when the evidence was observed, epoch milliseconds.
pub const PARAM_OBSERVED_AT_MS: &str = "observed_at_ms";
/// Integer parameter: when the process last made progress, epoch milliseconds.
pub const PARAM_LAST_PROGRESS_MS: &str = "last_progress_ms";
/// Integer parameter: the process's cpu time in milliseconds.
pub const PARAM_CPU_TIME_MS: &str = "cpu_time_ms";
/// Integer parameter: the bytes of output the process produced.
pub const PARAM_OUTPUT_BYTES: &str = "output_bytes";
/// Boolean parameter: whether the process holds child output.
pub const PARAM_HOLDS_CHILD_OUTPUT: &str = "holds_child_output";
/// The one option a process remediation question declares.
pub const REMEDIATION_OPTION: &str = "remediation";

/// The decision-policy knobs of process remediation (DE-R059.1-KNOBS).
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "process.zombie_after",
        unit: "seconds",
        default: KnobValue::Integer(600),
        min: KnobValue::Integer(60),
        max: KnobValue::Integer(86_400),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "process.hung_after",
        unit: "seconds",
        default: KnobValue::Integer(1_800),
        min: KnobValue::Integer(60),
        max: KnobValue::Integer(86_400),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "process.evidence_max_age",
        unit: "seconds",
        default: KnobValue::Integer(300),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(86_400),
        safety: KnobSafety::Tunable,
    },
];

fn seconds(policy: &DecisionPolicy, index: usize) -> u64 {
    match policy.knob(&KNOBS[index]) {
        KnobValue::Integer(v) => u64::try_from(v).unwrap_or(0),
        _ => 0,
    }
}

/// The family's refusal table, registered in `codes::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("LIVENESS_EVIDENCE_STALE"),
    RefusalCode::new("LIVENESS_EVIDENCE_MISMATCH"),
];

/// `LIVENESS_EVIDENCE_STALE`.
pub const EVIDENCE_STALE: RefusalCode = REFUSALS[0];
/// `LIVENESS_EVIDENCE_MISMATCH`.
pub const EVIDENCE_MISMATCH: RefusalCode = REFUSALS[1];

/// The three verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remediation {
    Zombie,
    Hung,
    Progressing,
}

impl Remediation {
    /// The option id the verdict is answered as.
    pub fn option_id(self) -> &'static str {
        match self {
            Self::Zombie => "zombie",
            Self::Hung => "hung",
            Self::Progressing => "progressing",
        }
    }
}

/// The liveness evidence of one run.
#[derive(Debug, Clone, Copy)]
pub struct Liveness {
    pub observed_at_ms: u64,
    pub last_progress_ms: u64,
    pub holds_child_output: bool,
}

/// Why a verdict cannot be reached; the code and the detail of the refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivenessRefusal {
    pub code: RefusalCode,
    pub detail: String,
}

/// The verdict at the engine's `now_ms`.
pub fn verdict(
    now_ms: u64,
    evidence: Liveness,
    policy: &DecisionPolicy,
) -> Result<Remediation, LivenessRefusal> {
    let max_age_ms = seconds(policy, 2).saturating_mul(1_000);
    if evidence.observed_at_ms > now_ms
        || now_ms - evidence.observed_at_ms > max_age_ms
        || evidence.last_progress_ms > evidence.observed_at_ms
    {
        return Err(LivenessRefusal {
            code: EVIDENCE_STALE,
            detail: format!(
                "evidence observed at {} cannot describe the run at {now_ms}",
                evidence.observed_at_ms
            ),
        });
    }
    let idle_ms = now_ms - evidence.last_progress_ms;
    if idle_ms >= seconds(policy, 0).saturating_mul(1_000) && !evidence.holds_child_output {
        Ok(Remediation::Zombie)
    } else if idle_ms >= seconds(policy, 1).saturating_mul(1_000) {
        Ok(Remediation::Hung)
    } else {
        Ok(Remediation::Progressing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(idle_s: u64, child: bool) -> Liveness {
        Liveness {
            observed_at_ms: 10_000_000,
            last_progress_ms: 10_000_000 - idle_s * 1_000,
            holds_child_output: child,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R820
    #[test]
    fn thresholds_decide_the_verdict() {
        let policy = DecisionPolicy::engine_default();
        let now = 10_000_000;
        assert_eq!(
            verdict(now, at(599, false), &policy),
            Ok(Remediation::Progressing)
        );
        assert_eq!(
            verdict(now, at(600, false), &policy),
            Ok(Remediation::Zombie)
        );
        assert_eq!(
            verdict(now, at(600, true), &policy),
            Ok(Remediation::Progressing)
        );
        assert_eq!(
            verdict(now, at(1_800, true), &policy),
            Ok(Remediation::Hung)
        );
        assert_eq!(
            verdict(now + 301_000, at(0, false), &policy)
                .unwrap_err()
                .code,
            EVIDENCE_STALE
        );
        assert_eq!(
            verdict(now - 1, at(0, false), &policy).unwrap_err().code,
            EVIDENCE_STALE
        );
    }
}
