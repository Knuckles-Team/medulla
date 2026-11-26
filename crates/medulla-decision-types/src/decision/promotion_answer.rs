//! The typed answer of a `promotion` question (MEDULLA-DECISION-ENGINE-R805.14).
//!
//! A promotion question decides whether a challenger revision takes the champion seat
//! (`promote`), whether the champion stays or is rolled back (`review`), or how a canary
//! proceeds (`canary`). The record seals this answer so a caller reads the verdict and the two
//! revisions instead of the premise facts. The answer is advisory: only a committed
//! `DecisionLog` record moves a pointer.

use serde::{Deserialize, Serialize};

/// What a promotion question decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PromotionAnswerVerdict {
    /// The challenger beats the champion on the held-out evidence (mode `promote`).
    Promote,
    /// Nothing moves: the evidence does not favour a change.
    Hold,
    /// The champion regressed; the seat returns to its parent (modes `review` and `canary`).
    Rollback,
    /// The canary may take more traffic (mode `canary`).
    Ramp,
    /// The question was refused; `reason` names the code.
    Abstain,
}

/// The typed answer of a `promotion` question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PromotionAnswer {
    pub verdict: PromotionAnswerVerdict,
    /// The revision that held the seat when the question was decided; absent when none did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub champion_ref: Option<String>,
    /// The challenger revision the question declared.
    pub challenger_ref: String,
    /// The refusal code when the verdict is `abstain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-DECISION-ENGINE-R805.14
    #[test]
    fn the_answer_round_trips_and_omits_an_absent_champion_and_reason() {
        let answer = PromotionAnswer {
            verdict: PromotionAnswerVerdict::Promote,
            champion_ref: None,
            challenger_ref: "program:b".to_string(),
            reason: None,
        };
        let text = serde_json::to_string(&answer).unwrap();
        assert_eq!(text, r#"{"verdict":"promote","challenger_ref":"program:b"}"#);
        assert_eq!(serde_json::from_str::<PromotionAnswer>(&text).unwrap(), answer);
        let refused = PromotionAnswer {
            verdict: PromotionAnswerVerdict::Abstain,
            champion_ref: Some("program:a".to_string()),
            challenger_ref: "program:b".to_string(),
            reason: Some("PROMOTION_UNEVALUATED".to_string()),
        };
        let text = serde_json::to_string(&refused).unwrap();
        assert_eq!(serde_json::from_str::<PromotionAnswer>(&text).unwrap(), refused);
        for (verdict, name) in [
            (PromotionAnswerVerdict::Hold, "hold"),
            (PromotionAnswerVerdict::Rollback, "rollback"),
            (PromotionAnswerVerdict::Ramp, "ramp"),
            (PromotionAnswerVerdict::Abstain, "abstain"),
        ] {
            assert_eq!(serde_json::to_string(&verdict).unwrap(), format!("\"{name}\""));
        }
    }
}
