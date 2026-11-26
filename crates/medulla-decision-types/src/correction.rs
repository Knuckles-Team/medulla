//! Typed correction records for the Learn stage (MEDULLA-DECISION-ENGINE-R702.1).
//!
//! A [`Correction`] names the target it corrects, the value before and after,
//! who produced the original and who corrected it. Committing a correction
//! yields one [`CorrectionEffect`]: a rule candidate, an outcome tagged
//! `independent` and an eval case, all sharing the correction's `receipt_id`.
//! [`CorrectionOp`] is the body of `Method::CorrectionRecord`.

use serde::{Deserialize, Serialize};

use medulla_types::refusal_code::RefusalCode;

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};

/// The knob tables of the correction surface (MEDULLA-DECISION-ENGINE-R702.2):
/// `correction.nearest_k`, the most prior corrections `Nearest` returns.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "correction.nearest_k",
    unit: "results",
    default: KnobValue::Integer(8),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(16),
    safety: KnobSafety::Tunable,
}];

/// The verified role an actor needs to record a correction
/// (MEDULLA-DECISION-ENGINE-R702.4).
pub const CORRECTOR_ROLE: &str = "corrector";

/// The actor lacks the verified role `corrector`.
pub const CORRECTION_ROLE_REQUIRED: RefusalCode = RefusalCode::new("CORRECTION_ROLE_REQUIRED");
/// The actor of a correction is the producer of the corrected output: a
/// self-reported outcome never enters calibration.
pub const OUTCOME_SELF_REPORTED: RefusalCode = RefusalCode::new("OUTCOME_SELF_REPORTED");
/// No correction of that receipt id is visible to the caller; a correction of
/// another tenant reads as absent.
pub const CORRECTION_NOT_FOUND: RefusalCode = RefusalCode::new("CORRECTION_NOT_FOUND");

/// The `correction` table of registered refusal codes
/// (MEDULLA-DECISION-ENGINE-R702.4). `RULE_CONTRADICTS_INVARIANT` is the code
/// the promotion gate already registers (R129.5); the correction handler
/// reuses it and does not declare it twice.
pub const REFUSALS: &[RefusalCode] = &[
    CORRECTION_ROLE_REQUIRED,
    OUTCOME_SELF_REPORTED,
    CORRECTION_NOT_FOUND,
];

/// Upper bound on `Correction::before` and `Correction::after`, in bytes.
pub const CORRECTION_VALUE_MAX_BYTES: usize = 8 * 1024;

/// Where a correction came from (MEDULLA-DECISION-ENGINE-R702.7). Every source needs the same
/// verified role and yields the same three effects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CorrectionSource {
    /// A person corrected an output directly.
    #[default]
    Human,
    /// A review comment: `before` and `after` carry digests only, never the comment text.
    ReviewComment,
    /// A review rejection of the output.
    ReviewRejection,
}

impl CorrectionSource {
    /// Whether this is the default source; a human correction serializes without the field, so
    /// its receipt id is the one it had before sources existed.
    pub fn is_human(&self) -> bool {
        matches!(self, Self::Human)
    }

    /// The stable name stored on the correction node.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::ReviewComment => "review_comment",
            Self::ReviewRejection => "review_rejection",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Correction {
    pub correction_id: String,
    pub target: String,
    pub before: String,
    pub after: String,
    pub produced_by: String,
    pub actor: String,
    pub at_ms: u64,
    /// Where the correction came from; `human` when absent.
    #[serde(default, skip_serializing_if = "CorrectionSource::is_human")]
    pub source: CorrectionSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CorrectionShapeError {
    #[error("correction field `{0}` exceeds {CORRECTION_VALUE_MAX_BYTES} bytes")]
    ValueTooLarge(&'static str),
    /// A review comment carries digests only: the field holds text, not `sha256:` plus 64
    /// lowercase hex (MEDULLA-DECISION-ENGINE-R702.7).
    #[error("review comment field `{0}` must be a sha256 digest, never the comment text")]
    CommentNotDigest(&'static str),
}

/// Whether `text` is `sha256:` plus 64 lowercase hex characters.
fn is_digest_text(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
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
        if self.source == CorrectionSource::ReviewComment {
            for (name, value) in [("before", &self.before), ("after", &self.after)] {
                if !is_digest_text(value) {
                    return Err(CorrectionShapeError::CommentNotDigest(name));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RuleCandidate {
    pub rule_id: String,
    pub text_digest: String,
}

/// The only outcome tag a correction may carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum OutcomeTag {
    Independent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CorrectionOutcome {
    pub outcome_id: String,
    pub tag: OutcomeTag,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct EvalCase {
    pub case_id: String,
    pub input_digest: String,
    pub expected_digest: String,
}

/// The three effects of one committed correction; every effect shares `receipt_id`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CorrectionEffect {
    pub receipt_id: String,
    pub rule_candidate: RuleCandidate,
    pub outcome: CorrectionOutcome,
    pub eval_case: EvalCase,
}

/// One node a committed correction writes: its id, class and properties.
#[derive(Clone, Debug, PartialEq)]
pub struct CorrectionNode {
    pub id: String,
    pub class: &'static str,
    pub properties: serde_json::Map<String, serde_json::Value>,
}

/// One edge a committed correction writes, from the correction node to an effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorrectionEdge {
    pub source: String,
    pub target: String,
    pub relationship: &'static str,
}

/// The plan of one correction: the effect and every node and edge that holds it.
/// `nodes` lists the correction node first, then the rule candidate, the
/// outcome and the eval case; the handler (R702.3) turns it into one batch.
#[derive(Clone, Debug, PartialEq)]
pub struct CorrectionCommit {
    pub effect: CorrectionEffect,
    pub nodes: Vec<CorrectionNode>,
    pub edges: Vec<CorrectionEdge>,
}

/// Body of `Method::CorrectionRecord`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum CorrectionOp {
    /// Record one correction and commit its three effects atomically.
    Record { correction: Correction },
    /// Nearest prior corrections to `target`, tenant-filtered.
    Nearest { target: String },
    /// Every node sharing `receipt_id`.
    Get { receipt_id: String },
}

impl CorrectionOp {
    /// Whether the op writes: `Record` does, `Nearest` and `Get` do not.
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Record { .. })
    }
}

/// What `CorrectionOp::Record` answers once its batch committed
/// (MEDULLA-DECISION-ENGINE-R702.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CorrectionRecorded {
    /// The receipt id every node of the correction shares.
    pub receipt_id: String,
    /// The rule candidate, outcome and eval case the correction yielded.
    pub effect: CorrectionEffect,
}

/// What `CorrectionOp::Nearest` answers: the node ids of the nearest prior
/// corrections of the caller's tenant, best first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CorrectionNearest {
    pub node_ids: Vec<String>,
}

/// What `CorrectionOp::Get` answers: the correction and its three effects,
/// read back from the four nodes that share one receipt id
/// (MEDULLA-DECISION-ENGINE-R702.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CorrectionRead {
    pub receipt_id: String,
    pub correction: Correction,
    pub effect: CorrectionEffect,
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
            source: CorrectionSource::Human,
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

    // spec: MEDULLA-DECISION-ENGINE-R702.7
    #[test]
    fn a_review_comment_carries_digests_and_a_human_correction_keeps_its_bytes() {
        // A human correction serializes without a source, so its receipt id does not move.
        let human = sample_correction();
        assert!(!serde_json::to_string(&human).unwrap().contains("source"));
        // A review source names itself.
        let mut rejection = sample_correction();
        rejection.source = CorrectionSource::ReviewRejection;
        let json = serde_json::to_string(&rejection).unwrap();
        assert!(json.contains(r#""source":"review_rejection""#));
        assert_eq!(serde_json::from_str::<Correction>(&json).unwrap(), rejection);
        assert!(rejection.validate().is_ok());
        // A review comment holds digests, never text.
        let mut comment = sample_correction();
        comment.source = CorrectionSource::ReviewComment;
        assert_eq!(
            comment.validate(),
            Err(CorrectionShapeError::CommentNotDigest("before"))
        );
        comment.before = format!("sha256:{}", "a".repeat(64));
        assert_eq!(
            comment.validate(),
            Err(CorrectionShapeError::CommentNotDigest("after"))
        );
        comment.after = format!("sha256:{}", "b".repeat(64));
        assert!(comment.validate().is_ok());
    }
}
