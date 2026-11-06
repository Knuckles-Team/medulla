//! MEDULLA-DECISION-ENGINE-R100.101.1: grounded topic coverage.
//!
//! A topic is covered when one of its mapped claims cites a source whose
//! support fraction `supporting / total` reaches
//! `knob grounded.min_topic_support`. A citation with fewer than
//! `knob grounded.min_valid_votes` votes refuses scoring. Coverage of a scope
//! is the summed covered count over the summed expected count of the pinned
//! query set, never an average of per-query ratios. All arithmetic is exact.

use std::collections::BTreeSet;

use medulla_decision_types::decision::statistical::grounded_eval::{
    CitationSupport, ExpectedTopicSet, GroundedKnobs, GroundedScope, QueryTopicCoverage,
    ScopeTopicCoverage, TopicSupport,
};

/// The `grounded_eval` family refusal table.
pub const REFUSALS: &[&str] = &[
    GROUNDED_QUERY_SET_EMPTY,
    GROUNDED_TOPIC_SET_EMPTY,
    GROUNDED_VOTES_TOO_FEW,
    GROUNDED_TOPIC_DUPLICATE,
];
pub const GROUNDED_QUERY_SET_EMPTY: &str = "GROUNDED_QUERY_SET_EMPTY";
pub const GROUNDED_TOPIC_SET_EMPTY: &str = "GROUNDED_TOPIC_SET_EMPTY";
pub const GROUNDED_VOTES_TOO_FEW: &str = "GROUNDED_VOTES_TOO_FEW";
pub const GROUNDED_TOPIC_DUPLICATE: &str = "GROUNDED_TOPIC_DUPLICATE";

/// A refused coverage scoring: the code and a detail naming the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundedRefusal {
    pub code: &'static str,
    pub detail: String,
}

fn refuse(code: &'static str, detail: String) -> GroundedRefusal {
    GroundedRefusal { code, detail }
}

fn supports(citation: &CitationSupport, knobs: &GroundedKnobs) -> Result<bool, GroundedRefusal> {
    if citation.total < knobs.min_valid_votes || citation.supporting > citation.total {
        return Err(refuse(
            GROUNDED_VOTES_TOO_FEW,
            format!(
                "citation={} total={} min={}",
                citation.citation_id, citation.total, knobs.min_valid_votes
            ),
        ));
    }
    let fraction = knobs.min_topic_support;
    Ok(
        u128::from(citation.supporting) * u128::from(fraction.denominator())
            >= u128::from(citation.total) * u128::from(fraction.numerator()),
    )
}

fn topic_covered(
    query_id: &str,
    topic_id: &str,
    support: &[TopicSupport],
    knobs: &GroundedKnobs,
) -> Result<bool, GroundedRefusal> {
    let mut covered = false;
    for topic in support
        .iter()
        .filter(|t| t.query_id == query_id && t.topic_id == topic_id)
    {
        for citation in topic.claims.iter().flat_map(|c| &c.citations) {
            covered |= supports(citation, knobs)?;
        }
    }
    Ok(covered)
}

/// Score the pinned query set of one scope.
pub fn topic_coverage(
    scope: GroundedScope,
    expected: &[ExpectedTopicSet],
    support: &[TopicSupport],
    knobs: &GroundedKnobs,
) -> Result<ScopeTopicCoverage, GroundedRefusal> {
    if expected.is_empty() {
        return Err(refuse(GROUNDED_QUERY_SET_EMPTY, format!("scope={scope:?}")));
    }
    let mut queries = Vec::with_capacity(expected.len());
    let (mut covered_sum, mut expected_sum) = (0u64, 0u64);
    for set in expected {
        if set.topic_ids.is_empty() {
            return Err(refuse(
                GROUNDED_TOPIC_SET_EMPTY,
                format!("query={}", set.query_id),
            ));
        }
        let mut seen = BTreeSet::new();
        let mut covered = Vec::new();
        for topic in &set.topic_ids {
            if !seen.insert(topic.as_str()) {
                return Err(refuse(
                    GROUNDED_TOPIC_DUPLICATE,
                    format!("query={} topic={topic}", set.query_id),
                ));
            }
            if topic_covered(&set.query_id, topic, support, knobs)? {
                covered.push(topic.clone());
            }
        }
        covered_sum += covered.len() as u64;
        expected_sum += set.topic_ids.len() as u64;
        queries.push(QueryTopicCoverage {
            query_id: set.query_id.clone(),
            expected_topic_ids: set.topic_ids.clone(),
            covered_topic_ids: covered,
        });
    }
    Ok(ScopeTopicCoverage {
        scope,
        queries,
        covered: covered_sum,
        expected: expected_sum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::statistical::grounded_eval::ClaimSupport;

    fn set(query: &str, topics: &[&str]) -> ExpectedTopicSet {
        ExpectedTopicSet {
            query_id: query.into(),
            topic_ids: topics.iter().map(|t| (*t).into()).collect(),
        }
    }

    fn support(query: &str, topic: &str, supporting: u32, total: u32) -> TopicSupport {
        TopicSupport {
            query_id: query.into(),
            topic_id: topic.into(),
            claims: vec![ClaimSupport {
                claim_id: format!("{query}-{topic}-claim"),
                citations: vec![CitationSupport {
                    citation_id: format!("{query}-{topic}-cite"),
                    supporting,
                    total,
                }],
            }],
        }
    }

    #[test]
    fn coverage_sums_numerators_before_dividing() {
        let knobs = GroundedKnobs::default();
        // q1 covers 1 of 1; q2 covers 1 of 4. Pooled 2/5, not mean(1, 1/4).
        let expected = vec![set("q1", &["a"]), set("q2", &["a", "b", "c", "d"])];
        let evidence = vec![
            support("q1", "a", 3, 3),
            support("q2", "a", 2, 4),
            support("q2", "b", 1, 4),
        ];
        let got =
            topic_coverage(GroundedScope::GlobalCorpus, &expected, &evidence, &knobs).unwrap();
        assert_eq!((got.covered, got.expected), (2, 5));
        assert_eq!(got.queries[0].covered_topic_ids, ["a"]);
        assert_eq!(got.queries[1].covered_topic_ids, ["a"]);
        assert_eq!(got.queries[1].expected_topic_ids.len(), 4);

        let code = |e: &[ExpectedTopicSet], s: &[TopicSupport]| {
            topic_coverage(GroundedScope::SingleDocument, e, s, &knobs)
                .unwrap_err()
                .code
        };
        assert_eq!(code(&[], &[]), GROUNDED_QUERY_SET_EMPTY);
        assert_eq!(code(&[set("q", &[])], &[]), GROUNDED_TOPIC_SET_EMPTY);
        assert_eq!(
            code(&[set("q", &["a", "a"])], &[]),
            GROUNDED_TOPIC_DUPLICATE
        );
        assert_eq!(
            code(&[set("q", &["a"])], &[support("q", "a", 2, 2)]),
            GROUNDED_VOTES_TOO_FEW
        );
        assert_eq!(REFUSALS.len(), 4);
    }
}
