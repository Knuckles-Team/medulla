//! The run outcome model: what a caller records about a delegated run so the
//! engine can credit each step (MEDULLA-DECISION-ENGINE-R100.102.1).
//!
//! A [`RunOutcome`] names the retrieval decisions the run used, the digest of
//! its answer, the rows the answer cited, whether it succeeded and one
//! [`StepRecord`] per step. The credit kernel and the receipt build on it;
//! this module is only the validated wire shape and its refusal codes.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::record::outcome::RefusalCode;
use medulla_types::contract::BoundedVec;

/// Most decisions one run may reference.
pub const MAX_RUN_DECISIONS: usize = 16;
/// Most rows one run may cite.
pub const MAX_RUN_CITED_ROWS: usize = 256;
/// The type bound on steps; `knob run_credit.max_steps` tightens it.
pub const MAX_RUN_STEPS_TYPE: usize = 1024;
/// Default of `knob run_credit.max_steps`.
pub const DEFAULT_RUN_MAX_STEPS: usize = 256;
/// Reward bound: `reward_q16` lies in `[-RUN_REWARD_BOUND, RUN_REWARD_BOUND]`.
pub const RUN_REWARD_BOUND: i32 = 65_536;

/// The run holds more steps than `knob run_credit.max_steps`.
pub const RUN_STEPS_TOO_MANY: RefusalCode = RefusalCode::new("RUN_STEPS_TOO_MANY");
/// Step indexes are not unique and ascending.
pub const RUN_STEP_ORDER_INVALID: RefusalCode = RefusalCode::new("RUN_STEP_ORDER_INVALID");
/// A step reward is outside `[-65536, 65536]`.
pub const RUN_REWARD_OUT_OF_RANGE: RefusalCode = RefusalCode::new("RUN_REWARD_OUT_OF_RANGE");

/// The refusal codes of the run outcome model, in the `RetrievalPlan` table.
pub const REFUSALS: &[RefusalCode] = &[
    RUN_REWARD_OUT_OF_RANGE,
    RUN_STEP_ORDER_INVALID,
    RUN_STEPS_TOO_MANY,
];

/// The `run_credit.*` knobs this model reads.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "run_credit.max_steps",
    unit: "steps",
    default: KnobValue::Integer(DEFAULT_RUN_MAX_STEPS as i64),
    min: KnobValue::Integer(16),
    max: KnobValue::Integer(MAX_RUN_STEPS_TYPE as i64),
    safety: KnobSafety::Tunable,
}];

/// Why a run outcome was refused: a registered code and a detail naming it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcomeRefusal {
    pub code: RefusalCode,
    pub detail: String,
}

impl RunOutcomeRefusal {
    fn new(code: RefusalCode, detail: String) -> Self {
        Self { code, detail }
    }
}

/// One step of a run: who acted, with which capability, and its reward.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StepRecord {
    pub step_idx: u32,
    pub agent_iri: String,
    pub capability_iri: String,
    /// Reward on the Q16 scale, an integer in `[-65536, 65536]`.
    pub reward_q16: i32,
}

/// What the caller records about one delegated run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RunOutcome {
    pub schema_version: u16,
    pub run_id: String,
    /// The retrieval decisions the run used (1 to 16).
    pub decision_ids: BoundedVec<String, MAX_RUN_DECISIONS>,
    /// SHA-256 of the answer: 64 lowercase hex characters.
    pub answer_digest: String,
    /// Row ids the answer cited (at most 256).
    #[serde(default)]
    pub cited_row_ids: BoundedVec<String, MAX_RUN_CITED_ROWS>,
    pub success: bool,
    #[serde(default)]
    pub steps: BoundedVec<StepRecord, MAX_RUN_STEPS_TYPE>,
}

/// Whether `text` is a SHA-256 digest: 64 lowercase hex characters.
pub fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn invalid(detail: &str) -> RunOutcomeRefusal {
    RunOutcomeRefusal::new(RefusalCode::new("INVALID_ARGUMENT"), detail.to_string())
}

impl RunOutcome {
    /// The validating check under `max_steps` (`knob run_credit.max_steps`).
    pub fn check(&self, max_steps: usize) -> Result<(), RunOutcomeRefusal> {
        if self.run_id.is_empty() {
            return Err(invalid("a run outcome names its run"));
        }
        if self.decision_ids.is_empty() || self.decision_ids.iter().any(String::is_empty) {
            return Err(invalid("a run outcome names 1 to 16 decisions"));
        }
        if !is_sha256_hex(&self.answer_digest) {
            return Err(invalid("answer_digest is 64 lowercase hex characters"));
        }
        let unique: BTreeSet<&str> = self.cited_row_ids.iter().map(String::as_str).collect();
        if unique.len() != self.cited_row_ids.len() || unique.contains("") {
            return Err(invalid("cited row ids are non-empty and unique"));
        }
        let limit = max_steps.min(MAX_RUN_STEPS_TYPE);
        if self.steps.len() > limit {
            return Err(RunOutcomeRefusal::new(
                RUN_STEPS_TOO_MANY,
                format!(
                    "run {} holds {} steps; the limit is {limit}",
                    self.run_id,
                    self.steps.len()
                ),
            ));
        }
        for pair in self.steps.windows(2) {
            if pair[0].step_idx >= pair[1].step_idx {
                return Err(RunOutcomeRefusal::new(
                    RUN_STEP_ORDER_INVALID,
                    format!(
                        "run {} step {} does not ascend past step {}",
                        self.run_id, pair[1].step_idx, pair[0].step_idx
                    ),
                ));
            }
        }
        for step in self.steps.iter() {
            if step.agent_iri.is_empty() || step.capability_iri.is_empty() {
                return Err(invalid("a step names its agent and capability"));
            }
            if !(-RUN_REWARD_BOUND..=RUN_REWARD_BOUND).contains(&step.reward_q16) {
                return Err(RunOutcomeRefusal::new(
                    RUN_REWARD_OUT_OF_RANGE,
                    format!(
                        "run {} step {} reward {} is outside [-65536, 65536]",
                        self.run_id, step.step_idx, step.reward_q16
                    ),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::codes;

    fn step(idx: u32, reward: i32) -> StepRecord {
        StepRecord {
            step_idx: idx,
            agent_iri: "urn:agent:a".to_string(),
            capability_iri: "urn:cap:c".to_string(),
            reward_q16: reward,
        }
    }

    fn outcome(steps: Vec<StepRecord>) -> RunOutcome {
        RunOutcome {
            schema_version: 1,
            run_id: "run-1".to_string(),
            decision_ids: BoundedVec::new(vec!["d1".to_string(), "d2".to_string()]).unwrap(),
            answer_digest: "ab".repeat(32),
            cited_row_ids: BoundedVec::new(vec!["r1".to_string(), "r2".to_string()]).unwrap(),
            success: true,
            steps: BoundedVec::new(steps).unwrap(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R100.102.1
    #[test]
    fn run_outcome_round_trips() {
        let run = outcome(vec![step(0, 65_536), step(3, -65_536)]);
        let text = serde_json::to_string(&run).unwrap();
        assert_eq!(serde_json::from_str::<RunOutcome>(&text).unwrap(), run);
        assert!(run.check(DEFAULT_RUN_MAX_STEPS).is_ok());

        let many = outcome((0..257).map(|i| step(i, 0)).collect());
        let refusal = many.check(DEFAULT_RUN_MAX_STEPS).unwrap_err();
        assert_eq!(refusal.code, RUN_STEPS_TOO_MANY);
        assert!(many.check(1024).is_ok());
        // The type bound is 1024 even when the knob is raised past it.
        assert!(BoundedVec::<StepRecord, MAX_RUN_STEPS_TYPE>::new(
            (0..1025).map(|i| step(i, 0)).collect()
        )
        .is_err());
        let unknown = text.replace("\"success\"", "\"extra\":1,\"success\"");
        assert!(serde_json::from_str::<RunOutcome>(&unknown).is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R100.102.1
    #[test]
    fn steps_ascend_and_rewards_stay_in_range() {
        let tied = outcome(vec![step(1, 0), step(1, 0)]);
        assert_eq!(tied.check(256).unwrap_err().code, RUN_STEP_ORDER_INVALID);
        let falling = outcome(vec![step(2, 0), step(1, 0)]);
        assert_eq!(falling.check(256).unwrap_err().code, RUN_STEP_ORDER_INVALID);
        let high = outcome(vec![step(0, 65_537)]);
        assert_eq!(high.check(256).unwrap_err().code, RUN_REWARD_OUT_OF_RANGE);
        let low = outcome(vec![step(0, -65_537)]);
        assert_eq!(low.check(256).unwrap_err().code, RUN_REWARD_OUT_OF_RANGE);
        let mut bad = outcome(vec![]);
        bad.answer_digest = "AB".repeat(32);
        assert!(bad.check(256).is_err());
        let mut none = outcome(vec![]);
        none.decision_ids = BoundedVec::default();
        assert!(none.check(256).is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R100.102.1
    #[test]
    fn refusals_and_knob_are_registered() {
        for code in REFUSALS {
            assert_eq!(codes::resolve(code.as_str()), Some(*code));
        }
        let spec = &KNOBS[0];
        assert!(spec.admits(&spec.default));
        assert!(!spec.admits(&KnobValue::Integer(15)));
        assert!(!spec.admits(&KnobValue::Integer(1025)));
    }
}
