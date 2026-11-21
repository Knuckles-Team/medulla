//! The replay record of a stored program (MEDULLA-DECISION-ENGINE-R135.17.1).
//!
//! One row per run that looked a program up: whether the stored program ran to its end (`hit`),
//! ran until a step failed and the planner took over (`fallback`), or was refused (`abstain`).
//! The row stores the outcome the caller observed; it never re-runs the program.

use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};

/// Current wire schema version of [`ProgramReplay`].
pub const REPLAY_SCHEMA_VERSION: u32 = 1;

/// A replay record broke one of its rules (`StatisticalErrorCode::ParameterInvalid` leads
/// with this code).
pub const REPLAY_RECORD_INVALID: RefusalCode = RefusalCode::new("REPLAY_RECORD_INVALID");

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
}
