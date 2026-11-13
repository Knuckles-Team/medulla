//! Feedback record model (MEDULLA-DECISION-ENGINE-R169.1).
//!
//! A checker reviews each round of an agent's output; the record keeps the
//! digests of what was produced, what the checker found and what the next
//! round was told. A continuation says whether the loop stops and why.

use serde::{Deserialize, Serialize};

use medulla_types::contract::Digest256;

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// The `feedback.*` knobs: the most rounds a feedback loop may run.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "feedback.max_rounds",
    unit: "rounds",
    default: KnobValue::Integer(5),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(20),
    safety: KnobSafety::Tunable,
}];

/// Why a feedback loop stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum StopReason {
    NoFindings,
    RoundBound,
}

/// One round of one feedback loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FeedbackRecord {
    pub loop_id: String,
    pub round: u32,
    pub output_digest: Digest256,
    pub checker_report_digest: Digest256,
    pub findings_digest: Digest256,
    pub finding_count: u32,
    pub next_instruction_digest: Digest256,
}

/// Whether the loop goes on after a round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Continuation {
    pub loop_id: String,
    pub round: u32,
    #[serde(default)]
    pub stop: Option<StopReason>,
}

/// The round bound in force: the request's bound, never above the resolved
/// `feedback.max_rounds` knob (MEDULLA-DECISION-ENGINE-R169.2).
pub fn effective_bound(request_bound: u32, knob_max_rounds: u32) -> u32 {
    request_bound.min(knob_max_rounds)
}

/// Whether the loop goes on after `round` (MEDULLA-DECISION-ENGINE-R169.2).
///
/// Zero findings stops with `NoFindings`. Otherwise a round at or above
/// `bound` stops with `RoundBound`. Otherwise the loop continues. `bound` is
/// [`effective_bound`] of the request and the knob.
pub fn next_round(loop_id: &str, finding_count: u32, round: u32, bound: u32) -> Continuation {
    let stop = if finding_count == 0 {
        Some(StopReason::NoFindings)
    } else if round >= bound {
        Some(StopReason::RoundBound)
    } else {
        None
    };
    Continuation {
        loop_id: loop_id.to_string(),
        round,
        stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R169.1
    #[test]
    fn records_round_trip() {
        let record = FeedbackRecord {
            loop_id: "loop:1".into(),
            round: 2,
            output_digest: Digest256::sha256(b"out"),
            checker_report_digest: Digest256::sha256(b"report"),
            findings_digest: Digest256::sha256(b"findings"),
            finding_count: 3,
            next_instruction_digest: Digest256::sha256(b"next"),
        };
        let text = serde_json::to_string(&record).unwrap();
        assert_eq!(
            serde_json::from_str::<FeedbackRecord>(&text).unwrap(),
            record
        );
        for stop in [
            None,
            Some(StopReason::NoFindings),
            Some(StopReason::RoundBound),
        ] {
            let continuation = Continuation {
                loop_id: "loop:1".into(),
                round: 2,
                stop,
            };
            let text = serde_json::to_string(&continuation).unwrap();
            assert_eq!(
                serde_json::from_str::<Continuation>(&text).unwrap(),
                continuation
            );
        }
        let text = serde_json::to_string(&StopReason::RoundBound).unwrap();
        assert_eq!(text, "\"round_bound\"");
        assert!(serde_json::from_str::<FeedbackRecord>(r#"{"loop_id":"l","round":-1}"#).is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R169.2
    #[test]
    fn next_round_rule() {
        // No findings stops, even at round zero and even past the bound.
        for round in [0, 1, 5, 9] {
            assert_eq!(
                next_round("loop:1", 0, round, 5).stop,
                Some(StopReason::NoFindings)
            );
        }
        // The bound edge: one below continues, at and above stop.
        assert_eq!(next_round("loop:1", 2, 4, 5).stop, None);
        assert_eq!(
            next_round("loop:1", 2, 5, 5).stop,
            Some(StopReason::RoundBound)
        );
        assert_eq!(
            next_round("loop:1", 2, 6, 5).stop,
            Some(StopReason::RoundBound)
        );
        // A continue keeps the loop and the round.
        let go = next_round("loop:7", 3, 1, 5);
        assert_eq!(
            (go.loop_id.as_str(), go.round, go.stop),
            ("loop:7", 1, None)
        );
        // The bound is the smaller of the request and the knob, and the knob
        // table admits its default and edges.
        assert_eq!(effective_bound(9, 5), 5);
        assert_eq!(effective_bound(3, 5), 3);
        let knob = &KNOBS[0];
        assert_eq!(knob.name, "feedback.max_rounds");
        assert!(knob.admits(&KnobValue::Integer(1)) && knob.admits(&KnobValue::Integer(20)));
        assert!(!knob.admits(&KnobValue::Integer(0)) && !knob.admits(&KnobValue::Integer(21)));
        assert!(knob.admits(&knob.default));
    }
}
