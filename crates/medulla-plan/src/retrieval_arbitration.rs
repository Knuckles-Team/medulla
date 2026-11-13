//! The retrieval arbitration step (MEDULLA-FEDERATED-QUERY-R083.2): one pure function
//! that compares the model prior with the retrieved answer.
//!
//! No I/O and no clock. Support is classified first: a candidate is supported only
//! when it makes at least one claim, every claim carries a citation and the span
//! verifier accepts each citation on the request snapshot. Support precedes
//! confidence. The policy is deterministic and learns nothing.

use medulla_result_types::retrieval_arbitration::{
    ArbitrationChoice, ArbitrationVerdict, Candidate, Citation, SupportState,
};

/// What the span verifier says about one citation on the request snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanCheck {
    /// The span lies on character boundaries and its digest matches.
    Valid,
    /// The document is in the snapshot and the span or its digest does not hold.
    Invalid,
    /// The cited document is not in the caller's snapshot.
    OutsideSnapshot,
}

/// Checks one citation against the request snapshot. The served wiring implements
/// it over `VerifySpan` of MEDULLA-DECISION-ENGINE-R144.1; a test passes a stub.
pub trait SpanVerifier {
    fn check(&self, citation: &Citation) -> SpanCheck;
}

/// Classify a candidate's support. A candidate with no claims has nothing backing
/// it, so it is `Unsupported`. A missing or invalid citation gives `Unsupported`
/// even when another citation is outside the snapshot; otherwise a citation outside
/// the snapshot gives `Unknown`.
pub fn classify_support(candidate: &Candidate, verifier: &dyn SpanVerifier) -> SupportState {
    if candidate.claims.is_empty() {
        return SupportState::Unsupported;
    }
    let mut outside = false;
    for claim in &candidate.claims {
        let Some(citation) = &claim.citation else {
            return SupportState::Unsupported;
        };
        match verifier.check(citation) {
            SpanCheck::Valid => {}
            SpanCheck::Invalid => return SupportState::Unsupported,
            SpanCheck::OutsideSnapshot => outside = true,
        }
    }
    if outside {
        SupportState::Unknown
    } else {
        SupportState::Supported
    }
}

/// The id of one evidence span: `<document_id>:<start_byte>-<end_byte>`.
fn span_id(citation: &Citation) -> String {
    format!(
        "{}:{}-{}",
        citation.document_id, citation.start_byte, citation.end_byte
    )
}

fn supporting_spans(candidate: &Candidate) -> Vec<String> {
    candidate
        .claims
        .iter()
        .filter_map(|claim| claim.citation.as_ref().map(span_id))
        .collect()
}

/// Arbitrate between the model `prior` and the `retrieved` answer.
///
/// Exactly one supported candidate wins whatever the confidence. Two supported
/// candidates: the higher confidence wins only when it exceeds the other by more than
/// `margin`, otherwise the verdict is `Abstain`. No supported candidate gives
/// `Abstain`. Both candidates stay in the verdict with their computed support state.
pub fn arbitrate(
    mut prior: Candidate,
    mut retrieved: Candidate,
    margin: f64,
    query_digest: &str,
    graph_snapshot_revision: &str,
    verifier: &dyn SpanVerifier,
) -> ArbitrationVerdict {
    prior.support_state = classify_support(&prior, verifier);
    retrieved.support_state = classify_support(&retrieved, verifier);
    let prior_ok = prior.support_state == SupportState::Supported;
    let retrieved_ok = retrieved.support_state == SupportState::Supported;
    let choice = match (prior_ok, retrieved_ok) {
        (true, false) => ArbitrationChoice::Prior,
        (false, true) => ArbitrationChoice::Retrieved,
        (false, false) => ArbitrationChoice::Abstain,
        (true, true) => {
            if prior.confidence - retrieved.confidence > margin {
                ArbitrationChoice::Prior
            } else if retrieved.confidence - prior.confidence > margin {
                ArbitrationChoice::Retrieved
            } else {
                ArbitrationChoice::Abstain
            }
        }
    };
    let supporting_span_ids = match choice {
        ArbitrationChoice::Prior => supporting_spans(&prior),
        ArbitrationChoice::Retrieved => supporting_spans(&retrieved),
        ArbitrationChoice::Abstain => Vec::new(),
    };
    ArbitrationVerdict {
        choice,
        margin,
        prior,
        retrieved,
        query_digest: query_digest.to_owned(),
        graph_snapshot_revision: graph_snapshot_revision.to_owned(),
        supporting_span_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_result_types::retrieval_arbitration::{AnswerGrounding, Claim};

    /// Stub: documents named `outside-*` are not in the snapshot, `bad-*` fail the
    /// digest, everything else verifies.
    struct Stub;
    impl SpanVerifier for Stub {
        fn check(&self, citation: &Citation) -> SpanCheck {
            if citation.document_id.starts_with("outside-") {
                SpanCheck::OutsideSnapshot
            } else if citation.document_id.starts_with("bad-") {
                SpanCheck::Invalid
            } else {
                SpanCheck::Valid
            }
        }
    }

    fn cite(document: &str) -> Option<Citation> {
        Some(Citation {
            document_id: document.into(),
            start_byte: 0,
            end_byte: 4,
            expected_digest: "00".repeat(32),
        })
    }

    fn candidate(confidence: f64, grounding: AnswerGrounding, docs: &[Option<&str>]) -> Candidate {
        Candidate {
            answer: format!("{grounding:?}"),
            confidence,
            grounding,
            claims: docs
                .iter()
                .map(|doc| Claim {
                    text: "claim".into(),
                    citation: doc.and_then(cite),
                })
                .collect(),
            support_state: SupportState::Unknown,
        }
    }

    fn run(prior: Candidate, retrieved: Candidate, margin: f64) -> ArbitrationVerdict {
        arbitrate(prior, retrieved, margin, "q-digest", "rev-1", &Stub)
    }

    // spec: MEDULLA-FEDERATED-QUERY-R083.2
    #[test]
    fn support_precedence_precedes_confidence_margin() {
        let prior = candidate(0.95, AnswerGrounding::Parametric, &[None]);
        let retrieved = candidate(0.70, AnswerGrounding::Retrieved, &[Some("doc-a")]);
        let verdict = run(prior, retrieved, 0.10);
        assert_eq!(verdict.choice, ArbitrationChoice::Retrieved);
        assert_eq!(verdict.prior.support_state, SupportState::Unsupported);
        assert_eq!(verdict.retrieved.support_state, SupportState::Supported);
        assert_eq!(verdict.supporting_span_ids, vec!["doc-a:0-4".to_owned()]);

        // Both supported: the 0.25 gap exceeds the 0.10 margin.
        let prior = candidate(0.95, AnswerGrounding::Parametric, &[Some("doc-p")]);
        let retrieved = candidate(0.70, AnswerGrounding::Retrieved, &[Some("doc-r")]);
        assert_eq!(run(prior, retrieved, 0.10).choice, ArbitrationChoice::Prior);

        // A gap inside the margin abstains, and so does a gap equal to it.
        let prior = candidate(0.75, AnswerGrounding::Parametric, &[Some("doc-p")]);
        let retrieved = candidate(0.70, AnswerGrounding::Retrieved, &[Some("doc-r")]);
        let verdict = run(prior.clone(), retrieved.clone(), 0.10);
        assert_eq!(verdict.choice, ArbitrationChoice::Abstain);
        assert!(verdict.supporting_span_ids.is_empty());
        assert_eq!(verdict.margin, 0.10);
        assert_eq!(verdict.prior.answer, prior.answer);
        assert_eq!(verdict.retrieved.answer, retrieved.answer);
        let tied = candidate(0.80, AnswerGrounding::Retrieved, &[Some("doc-r")]);
        let low = candidate(0.50, AnswerGrounding::Parametric, &[Some("doc-p")]);
        assert_eq!(run(low, tied, 0.30).choice, ArbitrationChoice::Abstain);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R083.2
    #[test]
    fn support_classification_distinguishes_unsupported_from_unknown() {
        let missing = candidate(0.9, AnswerGrounding::Retrieved, &[Some("doc-a"), None]);
        assert_eq!(classify_support(&missing, &Stub), SupportState::Unsupported);
        let invalid = candidate(0.9, AnswerGrounding::Retrieved, &[Some("bad-a")]);
        assert_eq!(classify_support(&invalid, &Stub), SupportState::Unsupported);
        let outside = candidate(0.9, AnswerGrounding::Retrieved, &[Some("outside-a")]);
        assert_eq!(classify_support(&outside, &Stub), SupportState::Unknown);
        let mixed = candidate(
            0.9,
            AnswerGrounding::Retrieved,
            &[Some("outside-a"), Some("bad-b")],
        );
        assert_eq!(classify_support(&mixed, &Stub), SupportState::Unsupported);
        let no_claims = candidate(0.9, AnswerGrounding::Retrieved, &[]);
        assert_eq!(
            classify_support(&no_claims, &Stub),
            SupportState::Unsupported
        );
        let good = candidate(
            0.9,
            AnswerGrounding::Retrieved,
            &[Some("doc-a"), Some("doc-b")],
        );
        assert_eq!(classify_support(&good, &Stub), SupportState::Supported);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R083.2
    #[test]
    fn no_supported_candidate_abstains_and_keeps_both() {
        let prior = candidate(0.99, AnswerGrounding::Parametric, &[Some("outside-p")]);
        let retrieved = candidate(0.99, AnswerGrounding::Retrieved, &[Some("bad-r")]);
        let verdict = run(prior, retrieved, 0.10);
        assert_eq!(verdict.choice, ArbitrationChoice::Abstain);
        assert_eq!(verdict.prior.support_state, SupportState::Unknown);
        assert_eq!(verdict.retrieved.support_state, SupportState::Unsupported);
        assert_eq!(verdict.query_digest, "q-digest");
        assert_eq!(verdict.graph_snapshot_revision, "rev-1");
    }

    // spec: MEDULLA-FEDERATED-QUERY-R083.2
    #[test]
    fn margin_picks_or_abstains() {
        let supported = |confidence, grounding| candidate(confidence, grounding, &[Some("doc-a")]);
        // The gap must exceed the margin: 0.20 > 0.10 picks, 0.10 == 0.10 abstains.
        let picked = run(
            supported(0.60, AnswerGrounding::Parametric),
            supported(0.80, AnswerGrounding::Retrieved),
            0.10,
        );
        assert_eq!(picked.choice, ArbitrationChoice::Retrieved);
        let abstained = run(
            supported(0.50, AnswerGrounding::Parametric),
            supported(0.75, AnswerGrounding::Retrieved),
            0.25,
        );
        assert_eq!(abstained.choice, ArbitrationChoice::Abstain);
        // Identical confidence abstains at a zero margin.
        let equal = run(
            supported(0.70, AnswerGrounding::Parametric),
            supported(0.70, AnswerGrounding::Retrieved),
            0.0,
        );
        assert_eq!(equal.choice, ArbitrationChoice::Abstain);
    }
}
