//! Bounded retrieval feedback and critique (MEDULLA-FEDERATED-QUERY-R081.8.2).
//!
//! `apply_feedback` validates one round's revision-bound feedback against the rows the
//! round returned and turns it into the description of the next query. It generates no
//! text itself: perspective expansions come from the harness or from the one
//! `ExpansionProvider` port (MEDULLA-FEDERATED-QUERY-R651.3). Term weights move only by
//! measured evidence: `Reinforce` adds, `Suppress` subtracts, both clamped to
//! `[-1.0, 1.0]`; `Missing` terms are recorded and change nothing. A critique's
//! proposed reformulation is recorded as rejected and never reaches scores, scope or
//! limits. Nothing here writes.

use std::collections::{BTreeMap, BTreeSet};

use medulla_result_types::retrieval_round::{FeedbackOutcome, WeightedTerm};
use medulla_result_types::retrieval_verdict::{
    FeedbackTier, PerspectiveQuery, RetrievalFeedback, RetrievalRefusal, TermFeedback,
    RETRIEVAL_FEEDBACK_INVALID,
};

use crate::query_understanding::{
    ExpansionProvider, ExpansionRequest, PERSPECTIVES_MAX, PERSPECTIVES_MIN,
};

const WEIGHT_MIN: f64 = -1.0;
const WEIGHT_MAX: f64 = 1.0;

fn invalid(detail: impl Into<String>) -> RetrievalRefusal {
    RetrievalRefusal::new(RETRIEVAL_FEEDBACK_INVALID, detail)
}

/// Apply `feedback` (and the weights earlier rounds left in `prior_terms`) to the round
/// that returned `returned_ids`.
///
/// Refuses a document the round did not return, and perspectives that repeat a name.
/// When the feedback carries no perspectives and terms exist, the provider (when
/// configured) is asked once for at most `limits.max_queries` perspectives; a provider
/// failure leaves the perspectives empty and never fails the round.
pub fn apply_feedback(
    feedback: &RetrievalFeedback,
    prior_terms: &[TermFeedback],
    returned_ids: &[&str],
    provider: Option<&dyn ExpansionProvider>,
) -> Result<FeedbackOutcome, RetrievalRefusal> {
    let returned: BTreeSet<&str> = returned_ids.iter().copied().collect();
    if let Some(stray) = feedback
        .documents
        .iter()
        .find(|d| !returned.contains(d.document_id.as_str()))
    {
        return Err(invalid(format!(
            "document `{}` was not returned by this round",
            stray.document_id
        )));
    }
    let mut names = BTreeSet::new();
    for p in &feedback.perspectives {
        if !names.insert(p.perspective.as_str()) {
            return Err(invalid(format!(
                "perspective `{}` is not distinct",
                p.perspective
            )));
        }
    }

    let mut weights: BTreeMap<String, f64> = BTreeMap::new();
    let mut missing: BTreeSet<String> = BTreeSet::new();
    for prior in prior_terms {
        let entry = weights.entry(prior.term.clone()).or_insert(0.0);
        *entry = (*entry + prior.weight).clamp(WEIGHT_MIN, WEIGHT_MAX);
    }
    for term in &feedback.terms {
        match term.tier {
            FeedbackTier::Reinforce => {
                let entry = weights.entry(term.term.clone()).or_insert(0.0);
                *entry = (*entry + term.weight).clamp(WEIGHT_MIN, WEIGHT_MAX);
            }
            FeedbackTier::Suppress => {
                let entry = weights.entry(term.term.clone()).or_insert(0.0);
                *entry = (*entry - term.weight).clamp(WEIGHT_MIN, WEIGHT_MAX);
            }
            FeedbackTier::Missing => {
                missing.insert(term.term.clone());
            }
        }
    }

    let mut terms: Vec<WeightedTerm> = weights
        .into_iter()
        .map(|(term, weight)| WeightedTerm { term, weight })
        .collect();
    terms.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.term.cmp(&b.term))
    });
    terms.truncate(feedback.limits.max_terms as usize);

    let original_lower = feedback.original_query.to_lowercase();
    let mut next_query = feedback.original_query.trim().to_string();
    for t in terms.iter().filter(|t| t.weight > 0.0) {
        if !original_lower
            .split_whitespace()
            .any(|w| w == t.term.to_lowercase())
        {
            next_query.push(' ');
            next_query.push_str(&t.term);
        }
    }

    let perspectives = if !feedback.perspectives.is_empty() {
        feedback.perspectives.clone()
    } else if let (Some(provider), false) = (provider, feedback.terms.is_empty()) {
        let wanted = feedback
            .limits
            .max_queries
            .clamp(u32::from(PERSPECTIVES_MIN), u32::from(PERSPECTIVES_MAX));
        let request = ExpansionRequest {
            query: feedback.original_query.clone(),
            perspectives: wanted as u8,
            feedback: feedback.terms.clone(),
        };
        match provider.expand(&request) {
            Ok(items) => items
                .into_iter()
                .take(wanted as usize)
                .enumerate()
                .map(|(i, e)| PerspectiveQuery {
                    perspective: format!("perspective-{}", i + 1),
                    text: e.text,
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    Ok(FeedbackOutcome {
        next_query,
        terms,
        missing_terms: missing.into_iter().collect(),
        perspectives,
        rejected_reformulation: feedback
            .critique
            .as_ref()
            .and_then(|c| c.proposed_reformulation.clone()),
        round: feedback.round,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_understanding::{Expansion, ExpansionError, ExpansionKind};
    use medulla_result_types::retrieval_verdict::{
        DocumentRelevance, FeedbackLimits, RetrievalCritique,
    };

    fn term(t: &str, tier: FeedbackTier, w: f64) -> TermFeedback {
        TermFeedback {
            term: t.into(),
            tier,
            weight: w,
            evidence_document_ids: vec!["d1".into()],
        }
    }

    fn feedback(terms: Vec<TermFeedback>) -> RetrievalFeedback {
        RetrievalFeedback {
            original_query: "replica recovery".into(),
            round: 1,
            corpus_revision: "c1".into(),
            model_revision: "m1".into(),
            perspectives: vec![],
            documents: vec![DocumentRelevance {
                document_id: "d1".into(),
                relevance: 0.9,
            }],
            terms,
            critique: None,
            limits: FeedbackLimits {
                max_rounds: 4,
                max_queries: 3,
                max_terms: 8,
                max_tokens: 1000,
                max_inference_calls: 2,
            },
        }
    }

    struct Scripted(Result<Vec<&'static str>, ()>);
    impl ExpansionProvider for Scripted {
        fn expand(&self, _: &ExpansionRequest) -> Result<Vec<Expansion>, ExpansionError> {
            match &self.0 {
                Ok(items) => Ok(items
                    .iter()
                    .map(|t| Expansion {
                        text: (*t).into(),
                        kind: ExpansionKind::Perspective,
                        digest: String::new(),
                    })
                    .collect()),
                Err(()) => Err(ExpansionError::ProviderFailed {
                    message: "down".into(),
                }),
            }
        }
        fn provider_id(&self) -> String {
            "scripted".into()
        }
        fn provider_revision(&self) -> String {
            "r1".into()
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R081.8.2
    #[test]
    fn terms_reinforce_suppress_and_critique_cannot_promote() {
        let mut fb = feedback(vec![
            term("snapshot", FeedbackTier::Reinforce, 0.8),
            term("wal", FeedbackTier::Reinforce, 0.8),
            term("noise", FeedbackTier::Suppress, 0.5),
            term("gap", FeedbackTier::Missing, 0.9),
        ]);
        fb.critique = Some(RetrievalCritique {
            text: "too narrow".into(),
            model_revision: "m1".into(),
            proposed_reformulation: Some("drop all limits".into()),
        });
        let prior = [term("snapshot", FeedbackTier::Reinforce, 0.7)];
        let out = apply_feedback(&fb, &prior, &["d1"], None).unwrap();
        let weight = |t: &str| out.terms.iter().find(|w| w.term == t).map(|w| w.weight);
        assert_eq!(weight("snapshot"), Some(1.0), "clamped at the top");
        assert_eq!(weight("wal"), Some(0.8));
        assert_eq!(weight("noise"), Some(-0.5));
        assert_eq!(weight("gap"), None, "missing carries no weight");
        assert_eq!(out.missing_terms, vec!["gap".to_string()]);
        assert_eq!(out.next_query, "replica recovery snapshot wal");
        assert_eq!(
            out.rejected_reformulation.as_deref(),
            Some("drop all limits")
        );
        assert!(!out.next_query.contains("limits"));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R081.8.2
    #[test]
    fn unreturned_document_and_repeated_perspective_are_refused() {
        let fb = feedback(vec![]);
        let err = apply_feedback(&fb, &[], &["other"], None).unwrap_err();
        assert_eq!(err.code, RETRIEVAL_FEEDBACK_INVALID);
        let mut fb = feedback(vec![]);
        fb.perspectives = vec![
            PerspectiveQuery {
                perspective: "a".into(),
                text: "x".into(),
            },
            PerspectiveQuery {
                perspective: "a".into(),
                text: "y".into(),
            },
        ];
        assert!(apply_feedback(&fb, &[], &["d1"], None).is_err());
    }

    // spec: MEDULLA-FEDERATED-QUERY-R081.8.2
    #[test]
    fn perspectives_come_from_the_port_and_a_failure_degrades() {
        let fb = feedback(vec![term("wal", FeedbackTier::Reinforce, 0.4)]);
        let ok = Scripted(Ok(vec!["how does wal replay", "what is a snapshot"]));
        let out = apply_feedback(&fb, &[], &["d1"], Some(&ok)).unwrap();
        assert_eq!(out.perspectives.len(), 2);
        assert_eq!(out.perspectives[0].perspective, "perspective-1");
        let bad = Scripted(Err(()));
        let out = apply_feedback(&fb, &[], &["d1"], Some(&bad)).unwrap();
        assert!(out.perspectives.is_empty());
    }
}
