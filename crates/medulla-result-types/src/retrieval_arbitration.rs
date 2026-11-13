//! Typed arbitration values for the retrieval boundary.
//!
//! MEDULLA-FEDERATED-QUERY-R083.1 defines the value shared by a future planner
//! arbitration step and its caller: both the model prior and the retrieved
//! answer remain present in every verdict.  This module deliberately owns only
//! the serde data model.  Support classification, confidence comparison and
//! planner wiring belong to later R083 children.

use serde::{Deserialize, Serialize};

/// The grounding of a candidate answer: parametric or retrieved.
///
/// This is closed so a caller cannot silently introduce a third provenance
/// class that the R083 arbitration policy does not understand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerGrounding {
    /// An answer produced without retrieved evidence.
    Parametric,
    /// An answer conditioned on retrieved evidence.
    Retrieved,
}

/// Refusal code for a candidate that names no grounding.
pub const CANDIDATE_GROUNDING_REQUIRED: &str = "CANDIDATE_GROUNDING_REQUIRED";

/// Why a candidate was refused before arbitration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArbitrationRefusal {
    pub code: &'static str,
    pub detail: String,
}

/// Whether the cited evidence backs a candidate's claims.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportState {
    Supported,
    Unsupported,
    #[default]
    Unknown,
}

/// A byte span of one stored document, bound to the digest it was read at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Citation {
    pub document_id: String,
    pub start_byte: u64,
    pub end_byte: u64,
    pub expected_digest: String,
}

/// One claim of a candidate answer and the evidence span that backs it, if any.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub text: String,
    #[serde(default)]
    pub citation: Option<Citation>,
}

/// One answer considered by the retrieval arbitration policy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    /// The candidate answer text carried through arbitration.
    pub answer: String,
    /// The producer's confidence in this candidate, before arbitration.
    pub confidence: f64,
    /// Whether the candidate is the model prior or retrieved evidence path.
    pub grounding: AnswerGrounding,
    /// The claims the answer makes, each with the span that backs it.
    #[serde(default)]
    pub claims: Vec<Claim>,
    /// Whether the cited evidence supports the claims.
    #[serde(default)]
    pub support_state: SupportState,
}

impl Candidate {
    /// Decode a candidate from its wire value. A value that names no `grounding`
    /// refuses `CANDIDATE_GROUNDING_REQUIRED`; any other malformed value refuses
    /// with the decoder's reason.
    pub fn from_value(value: &serde_json::Value) -> Result<Self, ArbitrationRefusal> {
        let named = value
            .get("grounding")
            .is_some_and(|grounding| !grounding.is_null());
        if !named {
            return Err(ArbitrationRefusal {
                code: CANDIDATE_GROUNDING_REQUIRED,
                detail: "a candidate names whether it is parametric or retrieved".into(),
            });
        }
        serde_json::from_value(value.clone()).map_err(|error| ArbitrationRefusal {
            code: "INVALID_ARGUMENT",
            detail: error.to_string(),
        })
    }
}

/// The closed set of outcomes from comparing the prior and retrieved answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArbitrationChoice {
    Prior,
    Retrieved,
    Abstain,
}

/// A typed arbitration result retaining both candidates for every outcome.
///
/// The `margin` is carried even when the choice is `Abstain`, so a later
/// planner or evidence record can explain the policy threshold it applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArbitrationVerdict {
    pub choice: ArbitrationChoice,
    pub margin: f64,
    pub prior: Candidate,
    pub retrieved: Candidate,
    /// Digest of the query the verdict answers.
    pub query_digest: String,
    /// The graph snapshot revision the retrieved candidate was read at.
    pub graph_snapshot_revision: String,
    /// Ids of the evidence spans that support the chosen answer, in order.
    #[serde(default)]
    pub supporting_span_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(answer: &str, grounding: AnswerGrounding, cited: bool) -> Candidate {
        Candidate {
            answer: answer.to_owned(),
            confidence: 0.9,
            grounding,
            claims: vec![Claim {
                text: format!("{answer} claim"),
                citation: cited.then(|| Citation {
                    document_id: "doc-1".into(),
                    start_byte: 4,
                    end_byte: 19,
                    expected_digest: "ab".repeat(32),
                }),
            }],
            support_state: if cited {
                SupportState::Supported
            } else {
                SupportState::Unsupported
            },
        }
    }

    #[test]
    fn verdict_carries_both_candidates_and_support() {
        let verdict = ArbitrationVerdict {
            choice: ArbitrationChoice::Abstain,
            margin: 0.10,
            prior: candidate("prior answer", AnswerGrounding::Parametric, false),
            retrieved: candidate("retrieved answer", AnswerGrounding::Retrieved, true),
            query_digest: "cd".repeat(32),
            graph_snapshot_revision: "rev-7".into(),
            supporting_span_ids: vec!["span-2".into(), "span-1".into()],
        };

        let encoded = serde_json::to_string(&verdict).expect("verdict must serialize");
        assert!(encoded.contains("\"parametric\"") && encoded.contains("\"retrieved\""));
        assert!(encoded.contains("\"grounding\"") && !encoded.contains("evidence_class"));
        assert!(encoded.contains("\"supported\"") && encoded.contains("\"unsupported\""));

        let decoded: ArbitrationVerdict =
            serde_json::from_str(&encoded).expect("verdict must round-trip");
        assert_eq!(decoded, verdict);
        assert_eq!(decoded.prior.answer, "prior answer");
        assert_eq!(decoded.retrieved.answer, "retrieved answer");
        // Order of the supporting spans is part of the verdict.
        assert_eq!(decoded.supporting_span_ids, ["span-2", "span-1"]);
        let cited = decoded.retrieved.claims[0]
            .citation
            .as_ref()
            .expect("citation");
        assert_eq!((cited.start_byte, cited.end_byte), (4, 19));

        // Support defaults to unknown; claims default to none.
        let bare = Candidate::from_value(&serde_json::json!({
            "answer": "a", "confidence": 0.5, "grounding": "retrieved"
        }))
        .expect("grounded candidate");
        assert_eq!(bare.support_state, SupportState::Unknown);
        assert!(bare.claims.is_empty());
        // A closed support state rejects an unknown value.
        assert!(serde_json::from_str::<SupportState>("\"maybe\"").is_err());
    }

    #[test]
    fn candidate_without_grounding_is_refused() {
        for value in [
            serde_json::json!({"answer": "a", "confidence": 0.5}),
            serde_json::json!({"answer": "a", "confidence": 0.5, "grounding": null}),
            // The retired field name does not stand in for grounding.
            serde_json::json!({"answer": "a", "confidence": 0.5, "evidence_class": "retrieved"}),
        ] {
            let refused = Candidate::from_value(&value).expect_err("no grounding");
            assert_eq!(refused.code, CANDIDATE_GROUNDING_REQUIRED);
        }
        let bad = Candidate::from_value(
            &serde_json::json!({"answer": "a", "confidence": 0.5, "grounding": "claim"}),
        )
        .expect_err("unknown grounding");
        assert_eq!(bad.code, "INVALID_ARGUMENT");
    }

    // spec: MEDULLA-DECISION-ENGINE-R010.100.3
    #[test]
    fn answer_grounding_round_trip() {
        for (value, wire) in [
            (AnswerGrounding::Parametric, "\"parametric\""),
            (AnswerGrounding::Retrieved, "\"retrieved\""),
        ] {
            let encoded = serde_json::to_string(&value).expect("grounding must serialize");
            assert_eq!(encoded, wire);
            let decoded: AnswerGrounding =
                serde_json::from_str(&encoded).expect("grounding must round-trip");
            assert_eq!(decoded, value);
        }
        assert!(serde_json::from_str::<AnswerGrounding>("\"claim\"").is_err());
    }
}
