//! MEDULLA-DECISION-ENGINE-R100.101.1: grounded topic coverage, and
//! MEDULLA-DECISION-ENGINE-R100.101.2: the soft-label unsupported-claim Brier
//! score (`weighted_brier`, below).
//!
//! A topic is covered when one of its mapped claims cites a source whose
//! support fraction `supporting / total` reaches
//! `knob grounded.min_topic_support`. A citation with fewer than
//! `knob grounded.min_valid_votes` votes refuses scoring. Coverage of a scope
//! is the summed covered count over the summed expected count of the pinned
//! query set, never an average of per-query ratios. All arithmetic is exact.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::statistical::grounded_eval::{
    CitationSupport, ClaimVotes, ExpectedTopicSet, GroundedClaimPrediction, GroundedKnobs,
    GroundedScope, QueryTopicCoverage, ScopeTopicCoverage, TopicSupport, WeightedBrier,
};

/// The `grounded_eval` family refusal table.
pub const REFUSALS: &[&str] = &[
    GROUNDED_QUERY_SET_EMPTY,
    GROUNDED_TOPIC_SET_EMPTY,
    GROUNDED_VOTES_TOO_FEW,
    GROUNDED_TOPIC_DUPLICATE,
    GROUNDED_CLAIM_SET_EMPTY,
    GROUNDED_PREDICTION_MISSING,
    GROUNDED_PREDICTION_EXTRA,
    GROUNDED_ANNOTATOR_DUPLICATE,
    GROUNDED_VOTES_ZERO,
    GROUNDED_PROBABILITY_OUT_OF_RANGE,
    GROUNDED_SCORER_SAW_LABELS,
    GROUNDED_REVISION_STALE,
    GROUNDED_DIGEST_MISMATCH,
];
pub const GROUNDED_QUERY_SET_EMPTY: &str = "GROUNDED_QUERY_SET_EMPTY";
pub const GROUNDED_TOPIC_SET_EMPTY: &str = "GROUNDED_TOPIC_SET_EMPTY";
pub const GROUNDED_VOTES_TOO_FEW: &str = "GROUNDED_VOTES_TOO_FEW";
pub const GROUNDED_TOPIC_DUPLICATE: &str = "GROUNDED_TOPIC_DUPLICATE";
pub const GROUNDED_CLAIM_SET_EMPTY: &str = "GROUNDED_CLAIM_SET_EMPTY";
pub const GROUNDED_PREDICTION_MISSING: &str = "GROUNDED_PREDICTION_MISSING";
pub const GROUNDED_PREDICTION_EXTRA: &str = "GROUNDED_PREDICTION_EXTRA";
pub const GROUNDED_ANNOTATOR_DUPLICATE: &str = "GROUNDED_ANNOTATOR_DUPLICATE";
pub const GROUNDED_VOTES_ZERO: &str = "GROUNDED_VOTES_ZERO";
pub const GROUNDED_PROBABILITY_OUT_OF_RANGE: &str = "GROUNDED_PROBABILITY_OUT_OF_RANGE";
/// The scorer's recorded inputs include the label set (R100.101.3).
pub const GROUNDED_SCORER_SAW_LABELS: &str = "GROUNDED_SCORER_SAW_LABELS";
/// A bound revision is older than the newest stored revision of its set
/// (R100.101.3).
pub const GROUNDED_REVISION_STALE: &str = "GROUNDED_REVISION_STALE";
/// A bound digest differs from the digest of its content or of the stored
/// revision (R100.101.3).
pub const GROUNDED_DIGEST_MISMATCH: &str = "GROUNDED_DIGEST_MISMATCH";

/// One million: the parts-per-million scale of probabilities and targets.
const PPM: u128 = 1_000_000;

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

/// `a / b` rounded half to even, for `b > 0`.
fn div_round_half_even(a: u128, b: u128) -> u128 {
    let (quotient, remainder) = (a / b, a % b);
    let twice = remainder * 2;
    if twice > b || (twice == b && quotient % 2 == 1) {
        quotient + 1
    } else {
        quotient
    }
}

/// The soft-label target `q` of one claim in parts per million:
/// `unsupported_votes / v` rounded half to even. `None` when `v` is zero.
pub fn target_ppm(votes: &ClaimVotes) -> Option<u32> {
    let valid = u128::from(votes.valid_votes());
    if valid == 0 {
        return None;
    }
    let q = div_round_half_even(u128::from(votes.unsupported_votes) * PPM, valid);
    u32::try_from(q).ok()
}

/// Score the detector's unsupported-claim predictions for the required claim
/// set `claims` against the annotators' soft labels. The numerator is the sum
/// of `v * (p - q)^2` in parts per million squared in 128-bit integers, the
/// denominator the sum of `v`, and the score `numerator / (denominator *
/// 10^12)` rounded half to even to `digits` decimal digits. Each required
/// claim needs exactly one prediction and at least one valid vote; labelled
/// votes never stand in for a prediction.
pub fn weighted_brier(
    claims: &[String],
    predictions: &[GroundedClaimPrediction],
    votes: &[ClaimVotes],
    digits: u32,
) -> Result<WeightedBrier, GroundedRefusal> {
    if claims.is_empty() {
        return Err(refuse(
            GROUNDED_CLAIM_SET_EMPTY,
            "the required claim set is empty".to_string(),
        ));
    }
    let required: BTreeSet<&str> = claims.iter().map(String::as_str).collect();
    let mut predicted: BTreeMap<&str, &GroundedClaimPrediction> = BTreeMap::new();
    for prediction in predictions {
        let claim = prediction.claim_id.as_str();
        if !required.contains(claim) || predicted.insert(claim, prediction).is_some() {
            return Err(refuse(GROUNDED_PREDICTION_EXTRA, format!("claim={claim}")));
        }
        if prediction.p_ppm > 1_000_000 {
            return Err(refuse(
                GROUNDED_PROBABILITY_OUT_OF_RANGE,
                format!("claim={claim} p_ppm={}", prediction.p_ppm),
            ));
        }
    }
    let mut annotated: BTreeMap<&str, &ClaimVotes> = BTreeMap::new();
    for claim_votes in votes {
        let mut seen = BTreeSet::new();
        for annotator in &claim_votes.annotators {
            if !seen.insert(annotator.as_str()) {
                return Err(refuse(
                    GROUNDED_ANNOTATOR_DUPLICATE,
                    format!("claim={} annotator={annotator}", claim_votes.claim_id),
                ));
            }
        }
        annotated.insert(claim_votes.claim_id.as_str(), claim_votes);
    }
    let (mut numerator, mut denominator) = (0u128, 0u64);
    for claim in &required {
        let Some(prediction) = predicted.get(claim) else {
            return Err(refuse(
                GROUNDED_PREDICTION_MISSING,
                format!("claim={claim}"),
            ));
        };
        let Some((claim_votes, q)) = annotated
            .get(claim)
            .and_then(|claim_votes| target_ppm(claim_votes).map(|q| (*claim_votes, q)))
        else {
            return Err(refuse(GROUNDED_VOTES_ZERO, format!("claim={claim}")));
        };
        let gap = i128::from(prediction.p_ppm) - i128::from(q);
        numerator += u128::from(claim_votes.valid_votes()) * (gap * gap) as u128;
        denominator += claim_votes.valid_votes();
    }
    let scale = 10u128.pow(digits);
    let score_scaled = div_round_half_even(numerator * scale, u128::from(denominator) * PPM * PPM);
    Ok(WeightedBrier {
        numerator,
        denominator,
        digits,
        score_scaled: u64::try_from(score_scaled).unwrap_or(u64::MAX),
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

    // spec: MEDULLA-DECISION-ENGINE-R100.101.1
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
        assert_eq!(REFUSALS.len(), 13);
    }

    fn prediction(claim: &str, p_ppm: u32) -> GroundedClaimPrediction {
        GroundedClaimPrediction {
            claim_id: claim.into(),
            answer_digest: "sha256:answer".into(),
            input_digest: "sha256:input".into(),
            scorer_id: "detector".into(),
            scorer_revision: 1,
            scorer_artifact_digest: "sha256:artifact".into(),
            p_ppm,
        }
    }

    fn votes(claim: &str, unsupported: u32, supported: u32) -> ClaimVotes {
        ClaimVotes {
            claim_id: claim.into(),
            annotators: (0..unsupported + supported)
                .map(|n| format!("annotator-{n}"))
                .collect(),
            unsupported_votes: unsupported,
            supported_votes: supported,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R100.101.2
    #[test]
    fn brier_changes_by_the_formula_when_only_p_varies() {
        // Labels fixed: A is 3 of 4 unsupported (q = 750000, v = 4), B is 0 of
        // 2 (q = 0, v = 2).
        let claims = vec!["A".to_string(), "B".to_string()];
        let labels = vec![votes("A", 3, 1), votes("B", 0, 2)];
        let score = |a: u32, b: u32, digits: u32| {
            weighted_brier(
                &claims,
                &[prediction("A", a), prediction("B", b)],
                &labels,
                digits,
            )
            .unwrap()
        };
        let perfect = score(750_000, 0, 6);
        assert_eq!((perfect.numerator, perfect.denominator), (0, 6));
        assert_eq!(perfect.score_scaled, 0);
        // 4 * 250000^2 + 2 * 100000^2 = 2.7e11; 2.7e11 / (6 * 1e12) = 0.045.
        let near = score(500_000, 100_000, 6);
        assert_eq!((near.numerator, near.denominator), (270_000_000_000, 6));
        assert_eq!(near.score_scaled, 45_000);
        assert_eq!(score(500_000, 100_000, 3).score_scaled, 45);
        // 0.045 at two digits is an exact tie: half to even gives 0.04.
        assert_eq!(score(500_000, 100_000, 2).score_scaled, 4);
        // Only p moves: A predicted 0 against q = 750000 -> 4 * 750000^2 / 6e12.
        let far = score(0, 0, 6);
        assert_eq!(far.numerator, 2_250_000_000_000);
        assert_eq!(far.score_scaled, 375_000);
        // Labels never replace predictions: the same labels, one claim away.
        assert_eq!(far.denominator, perfect.denominator);

        // The target rounds half to even: 1 of 128 is 7812.5 ppm -> 7812.
        assert_eq!(target_ppm(&votes("C", 1, 127)), Some(7_812));
        assert_eq!(target_ppm(&votes("C", 3, 125)), Some(23_438));
        assert_eq!(target_ppm(&votes("C", 0, 0)), None);

        let code =
            |claims: &[String], predictions: &[GroundedClaimPrediction], labels: &[ClaimVotes]| {
                weighted_brier(claims, predictions, labels, 6)
                    .unwrap_err()
                    .code
            };
        let a = vec!["A".to_string()];
        assert_eq!(code(&[], &[], &[]), GROUNDED_CLAIM_SET_EMPTY);
        assert_eq!(
            code(&a, &[], &[votes("A", 1, 1)]),
            GROUNDED_PREDICTION_MISSING
        );
        assert_eq!(
            code(
                &a,
                &[prediction("A", 0), prediction("Z", 0)],
                &[votes("A", 1, 1)]
            ),
            GROUNDED_PREDICTION_EXTRA
        );
        assert_eq!(
            code(
                &a,
                &[prediction("A", 0), prediction("A", 1)],
                &[votes("A", 1, 1)]
            ),
            GROUNDED_PREDICTION_EXTRA
        );
        let mut doubled = votes("A", 1, 1);
        doubled.annotators = vec!["same".into(), "same".into()];
        assert_eq!(
            code(&a, &[prediction("A", 0)], &[doubled]),
            GROUNDED_ANNOTATOR_DUPLICATE
        );
        assert_eq!(
            code(&a, &[prediction("A", 0)], &[votes("A", 0, 0)]),
            GROUNDED_VOTES_ZERO
        );
        assert_eq!(code(&a, &[prediction("A", 0)], &[]), GROUNDED_VOTES_ZERO);
        assert_eq!(
            code(&a, &[prediction("A", 1_000_001)], &[votes("A", 1, 1)]),
            GROUNDED_PROBABILITY_OUT_OF_RANGE
        );
        assert_eq!(REFUSALS.len(), 13);
    }
}
