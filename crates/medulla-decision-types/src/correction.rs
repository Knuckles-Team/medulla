//! Typed correction records for the Learn stage (MEDULLA-DECISION-ENGINE-R702.1).
//!
//! A [`Correction`] names the target it corrects, the value before and after,
//! who produced the original and who corrected it. Committing a correction
//! yields one [`CorrectionEffect`]: a rule candidate, an outcome tagged
//! `independent` and an eval case, all sharing the correction's `receipt_id`.
//! [`CorrectionOp`] is the body of `Method::CorrectionRecord`.

use serde::{Deserialize, Serialize};

/// Upper bound on `Correction::before` and `Correction::after`, in bytes.
pub const CORRECTION_VALUE_MAX_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Correction {
    pub correction_id: String,
    pub target: String,
    pub before: String,
    pub after: String,
    pub produced_by: String,
    pub actor: String,
    pub at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CorrectionShapeError {
    #[error("correction field `{0}` exceeds {CORRECTION_VALUE_MAX_BYTES} bytes")]
    ValueTooLarge(&'static str),
}

impl Correction {
    /// Checks the 8 KiB bound on `before` and `after`.
    pub fn validate(&self) -> Result<(), CorrectionShapeError> {
        if self.before.len() > CORRECTION_VALUE_MAX_BYTES {
            return Err(CorrectionShapeError::ValueTooLarge("before"));
        }
        if self.after.len() > CORRECTION_VALUE_MAX_BYTES {
            return Err(CorrectionShapeError::ValueTooLarge("after"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleCandidate {
    pub rule_id: String,
    pub text_digest: String,
}

/// The only outcome tag a correction may carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeTag {
    Independent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectionOutcome {
    pub outcome_id: String,
    pub tag: OutcomeTag,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalCase {
    pub case_id: String,
    pub input_digest: String,
    pub expected_digest: String,
}

/// The three effects of one committed correction; every effect shares `receipt_id`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectionEffect {
    pub receipt_id: String,
    pub rule_candidate: RuleCandidate,
    pub outcome: CorrectionOutcome,
    pub eval_case: EvalCase,
}

/// Body of `Method::CorrectionRecord`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum CorrectionOp {
    /// Record one correction and commit its three effects atomically.
    Record { correction: Correction },
    /// Nearest prior corrections to `target`, tenant-filtered.
    Nearest { target: String },
    /// Every node sharing `receipt_id`.
    Get { receipt_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_correction() -> Correction {
        Correction {
            correction_id: "c1".into(),
            target: "decision:7".into(),
            before: "a".into(),
            after: "b".into(),
            produced_by: "agent:x".into(),
            actor: "user:y".into(),
            at_ms: 42,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R702.1
    #[test]
    fn correction_types_round_trip_with_golden_vector() {
        let op = CorrectionOp::Record {
            correction: sample_correction(),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(
            json,
            r#"{"op":"record","correction":{"correction_id":"c1","target":"decision:7","before":"a","after":"b","produced_by":"agent:x","actor":"user:y","at_ms":42}}"#
        );
        assert_eq!(serde_json::from_str::<CorrectionOp>(&json).unwrap(), op);

        let effect = CorrectionEffect {
            receipt_id: "r".repeat(64),
            rule_candidate: RuleCandidate {
                rule_id: "rule:1".into(),
                text_digest: "d".into(),
            },
            outcome: CorrectionOutcome {
                outcome_id: "o1".into(),
                tag: OutcomeTag::Independent,
                value: 1.0,
            },
            eval_case: EvalCase {
                case_id: "e1".into(),
                input_digest: "i".into(),
                expected_digest: "x".into(),
            },
        };
        let json = serde_json::to_string(&effect).unwrap();
        assert!(json.contains(r#""tag":"independent""#));
        assert_eq!(
            serde_json::from_str::<CorrectionEffect>(&json).unwrap(),
            effect
        );

        // Unknown fields and other outcome tags are refused.
        assert!(serde_json::from_str::<Correction>(
            r#"{"correction_id":"c","target":"t","before":"","after":"","produced_by":"p","actor":"a","at_ms":0,"extra":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<OutcomeTag>(r#""self_reported""#).is_err());

        // The 8 KiB bound.
        let mut big = sample_correction();
        assert!(big.validate().is_ok());
        big.after = "z".repeat(CORRECTION_VALUE_MAX_BYTES + 1);
        assert_eq!(
            big.validate(),
            Err(CorrectionShapeError::ValueTooLarge("after"))
        );
    }
}
