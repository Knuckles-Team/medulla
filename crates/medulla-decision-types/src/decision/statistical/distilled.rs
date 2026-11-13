//! The distilled score model (MEDULLA-DECISION-ENGINE-R147.1).
//!
//! A scorer is distilled offline from a teacher's judgments and stored as
//! integer scores, so ranking reads the stored scores and makes no model call.

use serde::{Deserialize, Serialize};

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use medulla_types::contract::Digest256;

/// Refusal code: a score is out of range or its fidelity bounds are inverted.
pub const DISTILLED_SCORE_INVALID: &str = "DISTILLED_SCORE_INVALID";

/// The largest Q16 score.
pub const Q16_ONE: u32 = 65_536;

/// The `distilled.*` knobs.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "distilled.max_top_k",
    unit: "items",
    default: KnobValue::Integer(100),
    min: KnobValue::Integer(1),
    max: KnobValue::Integer(1000),
    safety: KnobSafety::Tunable,
}];

/// One teacher judgment of one document, with its fidelity bounds, all Q16.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TeacherJudgment {
    pub document_id: String,
    pub score_q16: u32,
    pub fidelity_lo_q16: u32,
    pub fidelity_hi_q16: u32,
}

impl TeacherJudgment {
    /// Every value lies in `[0, 65536]` and the low bound is not above the high.
    pub fn checked(self) -> Result<Self, &'static str> {
        let in_range = [self.score_q16, self.fidelity_lo_q16, self.fidelity_hi_q16]
            .iter()
            .all(|value| *value <= Q16_ONE);
        if !in_range || self.fidelity_lo_q16 > self.fidelity_hi_q16 {
            return Err(DISTILLED_SCORE_INVALID);
        }
        Ok(self)
    }
}

/// The persisted identity of one distilled scorer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DistilledScorerManifest {
    pub scorer_id: String,
    pub scorer_revision: u64,
    pub score_count: u64,
    pub score_digest: Digest256,
    pub persisted_at_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn judgment(score: u32, lo: u32, hi: u32) -> TeacherJudgment {
        TeacherJudgment {
            document_id: "doc:1".into(),
            score_q16: score,
            fidelity_lo_q16: lo,
            fidelity_hi_q16: hi,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R147.1
    #[test]
    fn distilled_scores_round_trip() {
        let good = judgment(40_000, 30_000, 50_000).checked().unwrap();
        let text = serde_json::to_string(&good).unwrap();
        assert_eq!(
            serde_json::from_str::<TeacherJudgment>(&text).unwrap(),
            good
        );
        let manifest = DistilledScorerManifest {
            scorer_id: "scorer:1".into(),
            scorer_revision: 3,
            score_count: 1,
            score_digest: Digest256::sha256(text.as_bytes()),
            persisted_at_ms: 1_700_000_000_000,
        };
        let wire = serde_json::to_string(&manifest).unwrap();
        assert_eq!(
            serde_json::from_str::<DistilledScorerManifest>(&wire).unwrap(),
            manifest
        );
        // Inverted bounds and out-of-range values refuse with the stable code.
        assert_eq!(
            judgment(1, 50_000, 40_000).checked().unwrap_err(),
            "DISTILLED_SCORE_INVALID"
        );
        assert!(judgment(65_537, 0, 1).checked().is_err());
        assert!(judgment(1, 0, 65_537).checked().is_err());
        assert!(judgment(65_536, 0, 65_536).checked().is_ok());
        // A negative score is not representable: the wire refuses it.
        assert!(serde_json::from_str::<TeacherJudgment>(
            r#"{"document_id":"d","score_q16":-1,"fidelity_lo_q16":0,"fidelity_hi_q16":1}"#
        )
        .is_err());
        assert!(KNOBS[0].admits(&KNOBS[0].default));
    }
}
