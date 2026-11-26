//! The replay record of a stored program (MEDULLA-DECISION-ENGINE-R135.17.1).
//!
//! One row per run that looked a program up: whether the stored program ran to its end (`hit`),
//! ran until a step failed and the planner took over (`fallback`), or was refused (`abstain`).
//! The row stores the outcome the caller observed; it never re-runs the program.

use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::record::outcome::DecisionRefusal;
use crate::decision::DecisionPolicy;

/// Current wire schema version of [`ProgramReplay`].
pub const REPLAY_SCHEMA_VERSION: u32 = 1;

/// A replay record broke one of its rules (`StatisticalErrorCode::ParameterInvalid` leads
/// with this code).
pub const REPLAY_RECORD_INVALID: RefusalCode = RefusalCode::new("REPLAY_RECORD_INVALID");

/// Abstention: the program's recent replays fell back too often to be reused
/// (MEDULLA-DECISION-ENGINE-R135.17.2).
pub const PROGRAM_DEMOTED: RefusalCode = RefusalCode::new("PROGRAM_DEMOTED");

/// The newest replays a lookup counts.
pub const REPLAY_WINDOW: KnobSpec = KnobSpec {
    name: "program.replay_window",
    unit: "replays",
    default: KnobValue::Integer(20),
    min: KnobValue::Integer(5),
    max: KnobValue::Integer(200),
    safety: KnobSafety::Tunable,
};
/// The fewest counted replays a demotion needs.
pub const DEMOTE_MIN_REPLAYS: KnobSpec = KnobSpec {
    name: "program.demote_min_replays",
    unit: "replays",
    default: KnobValue::Integer(5),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(100),
    safety: KnobSafety::Tunable,
};
/// The fallback share, in millionths of the counted replays, that demotes a program.
pub const DEMOTE_FALLBACK_RATE: KnobSpec = KnobSpec {
    name: "program.demote_fallback_rate",
    unit: "millionths",
    default: KnobValue::DecimalMicros(500_000),
    min: KnobValue::DecimalMicros(100_000),
    max: KnobValue::DecimalMicros(1_000_000),
    safety: KnobSafety::Tunable,
};

/// The most replay rows one lookup reads.
pub const MAX_REPLAY_ROWS: usize = 4096;

/// The key prefix of every replay row of `task_signature`.
pub fn replay_prefix(task_signature: &str) -> String {
    format!("run_record/program_replay/{task_signature}/")
}

/// The resolved values of the replay knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayKnobs {
    pub window: usize,
    pub demote_min_replays: usize,
    pub demote_fallback_rate_micros: i64,
}

impl ReplayKnobs {
    /// The values `policy` resolves, each clamped to its knob range.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let value = |spec: &KnobSpec| match policy.knob(spec) {
            KnobValue::Integer(v) | KnobValue::DecimalMicros(v) => v,
            KnobValue::DurationMs(v) => i64::try_from(v).unwrap_or(i64::MAX),
        };
        Self {
            window: usize::try_from(value(&REPLAY_WINDOW)).unwrap_or(5).clamp(5, 200),
            demote_min_replays: usize::try_from(value(&DEMOTE_MIN_REPLAYS))
                .unwrap_or(1)
                .clamp(1, 100),
            demote_fallback_rate_micros: value(&DEMOTE_FALLBACK_RATE).clamp(100_000, 1_000_000),
        }
    }
}

/// What the recent replays of one program say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayVerdict {
    /// The replays counted: the newest window of `hit` and `fallback` rows of the program.
    pub replays: usize,
    /// How many of them fell back.
    pub fallbacks: usize,
    /// Whether the program is demoted.
    pub demoted: bool,
}

/// The verdict over `rows` (the replay rows of one task signature) for the program
/// `program_digest`. Rows of another program and `abstain` rows are not counted; the newest
/// `window` rows count, newest by `recorded_at_ms` and then by `run_id`. The program is demoted
/// when the count reaches `demote_min_replays` and the fallbacks reach the rate of the count.
pub fn replay_verdict(
    rows: &[StoredReplay],
    program_digest: &str,
    knobs: &ReplayKnobs,
) -> ReplayVerdict {
    let mut counted: Vec<&StoredReplay> = rows
        .iter()
        .filter(|row| {
            row.replay.program_digest.as_deref() == Some(program_digest)
                && matches!(
                    row.replay.outcome,
                    ReplayOutcome::Hit | ReplayOutcome::Fallback
                )
        })
        .collect();
    counted.sort_by(|a, b| {
        b.recorded_at_ms
            .cmp(&a.recorded_at_ms)
            .then_with(|| b.replay.run_id.cmp(&a.replay.run_id))
    });
    counted.truncate(knobs.window);
    let replays = counted.len();
    let fallbacks = counted
        .iter()
        .filter(|row| row.replay.outcome == ReplayOutcome::Fallback)
        .count();
    let demoted = replays >= knobs.demote_min_replays
        && (fallbacks as i128) * 1_000_000 >= i128::from(knobs.demote_fallback_rate_micros) * (replays as i128);
    ReplayVerdict {
        replays,
        fallbacks,
        demoted,
    }
}

/// The refusal a demoted program answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramDemoted {
    pub replays: usize,
    pub fallbacks: usize,
}

impl DecisionRefusal for ProgramDemoted {
    fn code(&self) -> RefusalCode {
        PROGRAM_DEMOTED
    }

    fn detail(&self) -> String {
        format!(
            "{} of the last {} replays fell back",
            self.fallbacks, self.replays
        )
    }
}

/// How one replay of a stored program ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ReplayOutcome {
    /// The program ran to its end.
    Hit,
    /// The program started and a step failed; the planner took over.
    Fallback,
    /// The lookup was refused; no program ran.
    Abstain,
}

/// One replay of a stored program, as the caller reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProgramReplay {
    /// Wire schema version.
    pub schema_version: u32,
    /// The run that replayed.
    pub run_id: String,
    /// The task signature the lookup named.
    pub task_signature: String,
    /// The digest of the committed `Decide` record that answered the lookup.
    pub decision_record_digest: String,
    /// How the replay ended.
    pub outcome: ReplayOutcome,
    /// The program that ran; absent for an abstention.
    #[serde(default)]
    pub program_digest: Option<String>,
    /// The step that failed, for a fallback.
    #[serde(default)]
    pub failed_step: Option<u32>,
    /// The refusal code, for an abstention or a fallback with no failing step.
    #[serde(default)]
    pub refusal_code: Option<String>,
}

/// The stored row: the replay, the engine time it was recorded and who committed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StoredReplay {
    /// The replay as committed.
    pub replay: ProgramReplay,
    /// The engine time of the commit.
    pub recorded_at_ms: u64,
    /// The committing principal.
    pub principal: String,
}

/// The decision-artifact key of the replay of `run_id` against `task_signature`.
pub fn replay_key(task_signature: &str, run_id: &str) -> String {
    format!("run_record/program_replay/{task_signature}/{run_id}")
}

impl ProgramReplay {
    /// Check the rules of the outcome; the error names the broken rule and leads with
    /// [`REPLAY_RECORD_INVALID`].
    pub fn validate(&self) -> Result<(), String> {
        let broken = |rule: &str| Err(format!("{}: {rule}", REPLAY_RECORD_INVALID.as_str()));
        let present = |value: &Option<String>| value.as_deref().is_some_and(|v| !v.trim().is_empty());
        if self.run_id.trim().is_empty()
            || self.task_signature.trim().is_empty()
            || self.decision_record_digest.trim().is_empty()
        {
            return broken("run_id, task_signature and decision_record_digest are required");
        }
        match self.outcome {
            ReplayOutcome::Hit => {
                if !present(&self.program_digest) || self.failed_step.is_some() {
                    return broken("a hit names its program_digest and no failed_step");
                }
            }
            ReplayOutcome::Fallback => {
                if !present(&self.program_digest) {
                    return broken("a fallback names its program_digest");
                }
                if self.failed_step.is_none() && !present(&self.refusal_code) {
                    return broken("a fallback names a failed_step or a refusal_code");
                }
            }
            ReplayOutcome::Abstain => {
                if !present(&self.refusal_code) || self.program_digest.is_some() {
                    return broken("an abstain names its refusal_code and no program_digest");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replay(outcome: ReplayOutcome) -> ProgramReplay {
        ProgramReplay {
            schema_version: REPLAY_SCHEMA_VERSION,
            run_id: "run-1".into(),
            task_signature: "sig".into(),
            decision_record_digest: "ab".repeat(32),
            outcome,
            program_digest: None,
            failed_step: None,
            refusal_code: None,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R135.17.1
    #[test]
    fn each_outcome_needs_its_fields() {
        // hit
        let mut hit = replay(ReplayOutcome::Hit);
        assert!(hit.validate().unwrap_err().starts_with("REPLAY_RECORD_INVALID"));
        hit.program_digest = Some("cd".repeat(32));
        assert!(hit.validate().is_ok());
        hit.failed_step = Some(2);
        assert!(hit.validate().is_err());
        // fallback
        let mut fallback = replay(ReplayOutcome::Fallback);
        fallback.failed_step = Some(1);
        assert!(fallback.validate().is_err(), "no program_digest");
        fallback.program_digest = Some("cd".repeat(32));
        assert!(fallback.validate().is_ok());
        fallback.failed_step = None;
        assert!(fallback.validate().is_err(), "no step and no code");
        fallback.refusal_code = Some("STEP_REFUSED".into());
        assert!(fallback.validate().is_ok());
        // abstain
        let mut abstain = replay(ReplayOutcome::Abstain);
        assert!(abstain.validate().is_err());
        abstain.refusal_code = Some("NO_PROGRAM".into());
        assert!(abstain.validate().is_ok());
        abstain.program_digest = Some("cd".repeat(32));
        assert!(abstain.validate().is_err());
        // identity fields
        for blank in 0..3 {
            let mut broken = replay(ReplayOutcome::Abstain);
            broken.refusal_code = Some("NO_PROGRAM".into());
            match blank {
                0 => broken.run_id = " ".into(),
                1 => broken.task_signature.clear(),
                _ => broken.decision_record_digest.clear(),
            }
            assert!(broken.validate().is_err());
        }
        assert_eq!(replay_key("sig", "run-1"), "run_record/program_replay/sig/run-1");
        let wire = serde_json::to_value(replay(ReplayOutcome::Hit)).unwrap();
        assert_eq!(wire["outcome"], "hit");
    }

    fn stored(run: &str, at: u64, outcome: ReplayOutcome, program: &str) -> StoredReplay {
        let mut replay = replay(outcome);
        replay.run_id = run.into();
        replay.program_digest = Some(program.into());
        replay.failed_step = (outcome == ReplayOutcome::Fallback).then_some(1);
        StoredReplay {
            replay,
            recorded_at_ms: at,
            principal: "p".into(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R135.17.2
    #[test]
    fn window_and_rate_decide_demotion() {
        let knobs = ReplayKnobs {
            window: 5,
            demote_min_replays: 5,
            demote_fallback_rate_micros: 500_000,
        };
        // Five fallbacks demote; four do not reach the minimum count.
        let five: Vec<StoredReplay> = (0..5)
            .map(|i| stored(&format!("r{i}"), i, ReplayOutcome::Fallback, "p"))
            .collect();
        assert!(replay_verdict(&five, "p", &knobs).demoted);
        assert!(!replay_verdict(&five[..4], "p", &knobs).demoted);
        // Another program's rows and abstentions are not counted.
        assert_eq!(replay_verdict(&five, "other", &knobs).replays, 0);
        // Only the newest `window` rows count: old fallbacks fall out of the window.
        let mut rows: Vec<StoredReplay> = (0..5)
            .map(|i| stored(&format!("old{i}"), i, ReplayOutcome::Fallback, "p"))
            .collect();
        rows.extend((0..5).map(|i| stored(&format!("new{i}"), 100 + i, ReplayOutcome::Hit, "p")));
        let verdict = replay_verdict(&rows, "p", &knobs);
        assert_eq!((verdict.replays, verdict.fallbacks, verdict.demoted), (5, 0, false));
        // The rate: 2 fallbacks of 5 is 400000 < 500000; 3 of 5 is 600000.
        let mixed = |fallbacks: usize| -> Vec<StoredReplay> {
            (0..5)
                .map(|i| {
                    let outcome = if i < fallbacks {
                        ReplayOutcome::Fallback
                    } else {
                        ReplayOutcome::Hit
                    };
                    stored(&format!("m{i}"), i as u64, outcome, "p")
                })
                .collect()
        };
        assert!(!replay_verdict(&mixed(2), "p", &knobs).demoted);
        assert!(replay_verdict(&mixed(3), "p", &knobs).demoted);
        // Equal times order by run id, newest (largest) first.
        let tied = vec![
            stored("a", 7, ReplayOutcome::Fallback, "p"),
            stored("b", 7, ReplayOutcome::Hit, "p"),
        ];
        let one = ReplayKnobs { window: 1, demote_min_replays: 1, ..knobs };
        assert!(!replay_verdict(&tied, "p", &one).demoted);
        // The knobs' bounds.
        let policy = DecisionPolicy::engine_default();
        let defaults = ReplayKnobs::from_policy(&policy);
        assert_eq!((defaults.window, defaults.demote_min_replays), (20, 5));
        assert_eq!(defaults.demote_fallback_rate_micros, 500_000);
    }
}
