//! Feedback record model (MEDULLA-DECISION-ENGINE-R169.1).
//!
//! A checker reviews each round of an agent's output; the record keeps the
//! digests of what was produced, what the checker found and what the next
//! round was told. A continuation says whether the loop stops and why.

use serde::{Deserialize, Serialize};

use medulla_types::contract::Digest256;

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
}
