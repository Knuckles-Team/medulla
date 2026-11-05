//! The typed source-reliability read contract (MEDULLA-DECISION-ENGINE-R118.1.2.1).
//!
//! Every reader of a subject's learned reliability gets one of three answers:
//! the visible log holds no outcome of it, it holds fewer outcomes than the
//! `reputation.min_support` knob, or it holds enough for an estimate. A sparse
//! subject is never reported as absent, so a reader cannot fall back to a
//! prior where the log has spoken but not loudly enough.

use serde::{Deserialize, Serialize};

use super::record::outcome::RefusalCode;

/// A visible subject has fewer independent outcomes than `reputation.min_support`.
pub const REPUTATION_INSUFFICIENT_HISTORY: RefusalCode =
    RefusalCode::new("REPUTATION_INSUFFICIENT_HISTORY");

/// The `reputation` family's own refusal codes.
pub const REFUSALS: &[RefusalCode] = &[REPUTATION_INSUFFICIENT_HISTORY];

/// What the caller's visible decision log says about one subject's reliability.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SourceReliability {
    /// The visible log holds no outcome of the subject.
    Absent,
    /// The subject has `trials` visible outcomes, fewer than `min_support`.
    InsufficientHistory { trials: u64, min_support: u64 },
    /// The learned success probability and its credible interval.
    Estimated {
        mean: f64,
        lower: f64,
        upper: f64,
        trials: u64,
    },
}

impl SourceReliability {
    /// The refusal code a reader reports, `None` unless the history is too thin.
    pub fn refusal(&self) -> Option<RefusalCode> {
        matches!(self, Self::InsufficientHistory { .. }).then_some(REPUTATION_INSUFFICIENT_HISTORY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R118.1.2.1
    #[test]
    fn source_reliability_round_trips() {
        let cases = [
            SourceReliability::Absent,
            SourceReliability::InsufficientHistory {
                trials: 3,
                min_support: 10,
            },
            SourceReliability::Estimated {
                mean: 0.75,
                lower: 0.5,
                upper: 0.9,
                trials: 40,
            },
        ];
        for case in cases {
            let wire = serde_json::to_string(&case).unwrap();
            let back: SourceReliability = serde_json::from_str(&wire).unwrap();
            assert_eq!(back, case, "{wire}");
        }
        assert_eq!(
            serde_json::to_value(SourceReliability::Absent).unwrap(),
            serde_json::json!({"state": "absent"})
        );
        assert_eq!(
            SourceReliability::InsufficientHistory {
                trials: 1,
                min_support: 10
            }
            .refusal()
            .map(RefusalCode::as_str),
            Some("REPUTATION_INSUFFICIENT_HISTORY")
        );
        assert!(SourceReliability::Absent.refusal().is_none());
        assert!(serde_json::from_str::<SourceReliability>(r#"{"state":"absent","x":1}"#).is_err());
        let codes = super::super::codes::SHARED;
        assert!(REFUSALS.iter().all(|c| !codes.contains(c)));
    }
}
