//! MEDULLA-DECISION-ENGINE-R100.100.1: hits-at-k over pinned retrieval runs.
//!
//! `hits_at_k` counts the queries whose first `k` ranked ids include a
//! relevant id; the value is `numerator / denominator` in parts per million
//! rounded half to even. `evaluate_ranking` first checks that every input is
//! the one it names (digests recomputed from content, one corpus, one input
//! digest, one complete ranking per query) and refuses with a registered
//! `StatisticalErrorCode` otherwise; it never invents a metric.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_policy_types::decision::knob_families::ranking_eval::RankingEvalKnobs;
use medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode;
use {
    medulla_decision_policy_types::decision::statistical::ranking_eval::is_sha256_hex,
    medulla_decision_policy_types::decision::statistical::ranking_eval::AlgorithmRun,
    medulla_decision_policy_types::decision::statistical::ranking_eval::HitsAtK,
    medulla_decision_policy_types::decision::statistical::ranking_eval::LabelSet,
    medulla_decision_policy_types::decision::statistical::ranking_eval::QuerySet,
    medulla_decision_policy_types::decision::statistical::ranking_eval::RetrievalRankingRequest,
};

/// The `retrieval_eval` family refusal table.
pub const REFUSALS: &[StatisticalErrorCode] = &[
    StatisticalErrorCode::RankingQuerySetEmpty,
    StatisticalErrorCode::RankingQuerySetTooLarge,
    StatisticalErrorCode::RankingDigestMismatch,
    StatisticalErrorCode::RankingKOutOfRange,
    StatisticalErrorCode::RankingAlgorithmsTooFew,
    StatisticalErrorCode::RankingLabelsMissing,
    StatisticalErrorCode::RankingCorpusDrift,
    StatisticalErrorCode::RankingResultsIncomplete,
    StatisticalErrorCode::RankingSettingsMismatch,
];

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

/// `hits@k` of one run. The request must already have passed
/// [`check_request`]; a query the run did not rank counts as a miss.
pub fn hits_at_k(labels: &LabelSet, queries: &QuerySet, run: &AlgorithmRun, k: u32) -> HitsAtK {
    let mut relevant: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for pair in &labels.pairs {
        relevant
            .entry(pair.query_id.as_str())
            .or_default()
            .insert(pair.doc_id.as_str());
    }
    let ranked: BTreeMap<&str, &Vec<String>> = run
        .rankings
        .iter()
        .map(|ranking| (ranking.query_id.as_str(), &ranking.ranked_ids))
        .collect();
    let unique: BTreeSet<&str> = queries.query_ids.iter().map(String::as_str).collect();
    let hits = unique
        .iter()
        .filter(|query| {
            let (Some(relevant), Some(ids)) = (relevant.get(*query), ranked.get(*query)) else {
                return false;
            };
            ids.iter()
                .take(k as usize)
                .any(|id| relevant.contains(id.as_str()))
        })
        .count() as u64;
    let denominator = unique.len() as u64;
    let value_ppm = if denominator == 0 {
        0
    } else {
        let value = div_round_half_even(u128::from(hits) * 1_000_000, u128::from(denominator));
        u32::try_from(value).unwrap_or(1_000_000)
    };
    HitsAtK {
        numerator: hits,
        denominator,
        value_ppm,
    }
}

/// Check every pinned input of `request` against its content and the knobs.
pub fn check_request(
    request: &RetrievalRankingRequest,
    knobs: &RankingEvalKnobs,
) -> Result<(), String> {
    let queries = &request.queries;
    if queries.query_ids.is_empty() {
        return Err(StatisticalErrorCode::RankingQuerySetEmpty.refusal("no queries named"));
    }
    if queries.query_ids.len() > knobs.max_queries as usize {
        return Err(
            StatisticalErrorCode::RankingQuerySetTooLarge.refusal(format!(
                "{} queries, limit {}",
                queries.query_ids.len(),
                knobs.max_queries
            )),
        );
    }
    if request.k == 0 || request.k > knobs.max_k {
        return Err(StatisticalErrorCode::RankingKOutOfRange
            .refusal(format!("k={} limit {}", request.k, knobs.max_k)));
    }
    if request.algorithms.len() < knobs.min_algorithms as usize {
        return Err(
            StatisticalErrorCode::RankingAlgorithmsTooFew.refusal(format!(
                "{} runs, need {}",
                request.algorithms.len(),
                knobs.min_algorithms
            )),
        );
    }
    let mismatch = |what: &str| Err(StatisticalErrorCode::RankingDigestMismatch.refusal(what));
    if !is_sha256_hex(&request.corpus.digest) {
        return mismatch("corpus digest is not 64 lowercase hex characters");
    }
    if request.labels.pinned.digest != request.labels.content_digest() {
        return mismatch("labels digest does not match the label pairs");
    }
    if request.queries.pinned.digest != request.queries.content_digest() {
        return mismatch("queries digest does not match the query ids");
    }
    let labelled: BTreeSet<&str> = request
        .labels
        .pairs
        .iter()
        .map(|pair| pair.query_id.as_str())
        .collect();
    if let Some(query) = queries
        .query_ids
        .iter()
        .find(|query| !labelled.contains(query.as_str()))
    {
        return Err(StatisticalErrorCode::RankingLabelsMissing.refusal(format!("query={query}")));
    }
    let input_digest = request.input_digest();
    let expected: BTreeSet<&str> = queries.query_ids.iter().map(String::as_str).collect();
    for run in &request.algorithms {
        if run.corpus_digest != request.corpus.digest {
            return Err(StatisticalErrorCode::RankingCorpusDrift
                .refusal(format!("algorithm={}", run.algorithm_id)));
        }
        if run.input_digest != input_digest {
            return Err(StatisticalErrorCode::RankingSettingsMismatch
                .refusal(format!("algorithm={}", run.algorithm_id)));
        }
        let ranked: BTreeSet<&str> = run
            .rankings
            .iter()
            .map(|ranking| ranking.query_id.as_str())
            .collect();
        if ranked != expected || run.rankings.len() != expected.len() {
            return Err(StatisticalErrorCode::RankingResultsIncomplete
                .refusal(format!("algorithm={}", run.algorithm_id)));
        }
    }
    Ok(())
}

/// Check `request` and score every run, in request order.
pub fn evaluate_ranking(
    request: &RetrievalRankingRequest,
    knobs: &RankingEvalKnobs,
) -> Result<Vec<(String, HitsAtK)>, String> {
    check_request(request, knobs)?;
    Ok(request
        .algorithms
        .iter()
        .map(|run| {
            (
                run.algorithm_id.clone(),
                hits_at_k(&request.labels, &request.queries, run, request.k),
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use {
        medulla_decision_policy_types::decision::statistical::ranking_eval::LabelPair,
        medulla_decision_policy_types::decision::statistical::ranking_eval::PinnedSet,
        medulla_decision_policy_types::decision::statistical::ranking_eval::QueryRanking,
        medulla_decision_policy_types::decision::statistical::ranking_eval::RANKING_EVAL_SCHEMA_VERSION,
    };

    const KNOBS: RankingEvalKnobs = RankingEvalKnobs {
        max_k: 1000,
        min_algorithms: 2,
        max_queries: 10_000,
    };

    fn pinned(id: &str, digest: String) -> PinnedSet {
        PinnedSet {
            id: id.into(),
            revision: 1,
            digest,
        }
    }

    fn ranking(query: &str, ids: &[&str]) -> QueryRanking {
        QueryRanking {
            query_id: query.into(),
            ranked_ids: ids.iter().map(|id| (*id).into()).collect(),
        }
    }

    /// Four queries; each has one relevant document `d<n>`.
    fn request() -> RetrievalRankingRequest {
        let query_ids: Vec<String> = ["q1", "q2", "q3", "q4"].map(String::from).to_vec();
        let labels = LabelSet {
            pinned: pinned("labels", String::new()),
            pairs: query_ids
                .iter()
                .enumerate()
                .map(|(n, query)| LabelPair {
                    query_id: query.clone(),
                    doc_id: format!("d{}", n + 1),
                })
                .collect(),
        };
        let queries = QuerySet {
            pinned: pinned("queries", String::new()),
            query_ids,
        };
        let mut request = RetrievalRankingRequest {
            schema_version: RANKING_EVAL_SCHEMA_VERSION,
            tenant_id: "tenant".into(),
            idempotency_key: "key".into(),
            corpus: pinned("corpus", "ab".repeat(32)),
            labels,
            queries,
            k: 2,
            evaluator_revision: 1,
            algorithms: Vec::new(),
        };
        request.labels.pinned.digest = request.labels.content_digest();
        request.queries.pinned.digest = request.queries.content_digest();
        let run = |id: &str, rankings: Vec<QueryRanking>| AlgorithmRun {
            algorithm_id: id.into(),
            version: "1".into(),
            settings_digest: "cd".repeat(32),
            corpus_digest: request.corpus.digest.clone(),
            input_digest: request.input_digest(),
            rankings,
        };
        // "lexical": hits q1 (rank 1) and q2 (rank 2); misses q3 (rank 3) and q4.
        let lexical = run(
            "lexical",
            vec![
                ranking("q1", &["d1", "x"]),
                ranking("q2", &["x", "d2"]),
                ranking("q3", &["x", "y", "d3"]),
                ranking("q4", &["x", "y"]),
            ],
        );
        // "vector": hits all four within the first two.
        let vector = run(
            "vector",
            vec![
                ranking("q1", &["x", "d1"]),
                ranking("q2", &["d2"]),
                ranking("q3", &["d3", "x"]),
                ranking("q4", &["d4", "x"]),
            ],
        );
        request.algorithms = vec![lexical, vector];
        request
    }

    fn code(request: &RetrievalRankingRequest, knobs: &RankingEvalKnobs) -> String {
        let error = check_request(request, knobs).unwrap_err();
        error.split(':').next().unwrap().to_string()
    }

    // spec: MEDULLA-DECISION-ENGINE-R100.100.1
    #[test]
    fn hits_at_k_counts_queries_with_a_relevant_top_k_item() {
        let request = request();
        let scored = evaluate_ranking(&request, &KNOBS).unwrap();
        assert_eq!(scored[0].0, "lexical");
        assert_eq!(
            scored[0].1,
            HitsAtK {
                numerator: 2,
                denominator: 4,
                value_ppm: 500_000
            }
        );
        assert_eq!(scored[1].0, "vector");
        assert_eq!(
            scored[1].1,
            HitsAtK {
                numerator: 4,
                denominator: 4,
                value_ppm: 1_000_000
            }
        );
        // Widening k to 3 lets the lexical run hit q3 as well.
        let wider = hits_at_k(&request.labels, &request.queries, &request.algorithms[0], 3);
        assert_eq!((wider.numerator, wider.denominator), (3, 4));
        assert_eq!(wider.value_ppm, 750_000);
        // 1 of 3 queries is 333333.33 ppm, rounded to 333333; 2 of 3 to 666667.
        let mut three = request.clone();
        three.queries.query_ids.truncate(3);
        let one = hits_at_k(&three.labels, &three.queries, &three.algorithms[0], 1);
        assert_eq!(
            (one.numerator, one.denominator, one.value_ppm),
            (1, 3, 333_333)
        );
        let two = hits_at_k(&three.labels, &three.queries, &three.algorithms[0], 2);
        assert_eq!(
            (two.numerator, two.denominator, two.value_ppm),
            (2, 3, 666_667)
        );

        let mut empty = request.clone();
        empty.queries.query_ids.clear();
        assert_eq!(code(&empty, &KNOBS), "RANKING_QUERY_SET_EMPTY");
        let small = RankingEvalKnobs {
            max_queries: 3,
            ..KNOBS
        };
        assert_eq!(code(&request, &small), "RANKING_QUERY_SET_TOO_LARGE");
        for k in [0, 1001] {
            let mut bad = request.clone();
            bad.k = k;
            assert_eq!(code(&bad, &KNOBS), "RANKING_K_OUT_OF_RANGE");
        }
        let mut one_run = request.clone();
        one_run.algorithms.truncate(1);
        assert_eq!(code(&one_run, &KNOBS), "RANKING_ALGORITHMS_TOO_FEW");
        let mut tampered = request.clone();
        tampered.labels.pairs[0].doc_id = "other".into();
        assert_eq!(code(&tampered, &KNOBS), "RANKING_DIGEST_MISMATCH");
        let mut tampered = request.clone();
        tampered.queries.query_ids.swap(0, 1);
        assert!(
            check_request(&tampered, &KNOBS).is_ok(),
            "the query digest is order-independent"
        );
        tampered.queries.query_ids[0] = "q9".into();
        assert_eq!(code(&tampered, &KNOBS), "RANKING_DIGEST_MISMATCH");
        let mut bad_corpus = request.clone();
        bad_corpus.corpus.digest = "ABC".into();
        assert_eq!(code(&bad_corpus, &KNOBS), "RANKING_DIGEST_MISMATCH");
        let mut unlabelled = request.clone();
        unlabelled.labels.pairs.pop();
        unlabelled.labels.pinned.digest = unlabelled.labels.content_digest();
        assert_eq!(code(&unlabelled, &KNOBS), "RANKING_LABELS_MISSING");
        let mut drift = request.clone();
        drift.algorithms[1].corpus_digest = "ef".repeat(32);
        assert_eq!(code(&drift, &KNOBS), "RANKING_CORPUS_DRIFT");
        let mut incomplete = request.clone();
        incomplete.algorithms[0].rankings.pop();
        assert_eq!(code(&incomplete, &KNOBS), "RANKING_RESULTS_INCOMPLETE");
        let mut settings = request.clone();
        settings.algorithms[0].input_digest = "00".repeat(32);
        assert_eq!(code(&settings, &KNOBS), "RANKING_SETTINGS_MISMATCH");
        assert_eq!(REFUSALS.len(), 9);
    }
}
