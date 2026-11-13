//! The memory-change benchmark kernel (MEDULLA-DECISION-ENGINE-R141.101).
//!
//! Pure. Per query, `recall_ppm` is `|expected ∩ top_k| / |expected|` in parts
//! per million, rounded half to even. `recall_at_k` is the sum of the per-query
//! values divided by the query count, rounded half to even. An answer is
//! correct when the SHA-256 of its normalized text (NFC, lower case, collapsed
//! whitespace) equals the query's reference answer digest. The change is
//! admitted when the candidate reaches both minimums and neither metric falls
//! below the baseline by more than `memory_change.max_regression`.

use std::collections::BTreeSet;

use medulla_decision_types::decision::memory_benchmark::{
    BenchmarkKnobs, BenchmarkQuery, MemoryChangeBenchmarkRequest, RunResults, ACCURACY_BELOW_MIN,
    MEMORY_COVERAGE_INCOMPLETE, MEMORY_DATASET_EMPTY, MEMORY_EXPECTED_MISSING,
    MEMORY_METRIC_UNRESOLVED, MEMORY_REVISION_STALE, MEMORY_SNAPSHOT_MISMATCH, RECALL_BELOW_MIN,
    REGRESSION_ABOVE_MAX,
};
use medulla_decision_types::decision::record::outcome::RefusalCode;
use medulla_types::contract::Digest256;
use unicode_normalization::UnicodeNormalization;

const PPM: u128 = 1_000_000;

/// A benchmark that cannot be scored: the registered code and a detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchmarkRefusal {
    pub code: RefusalCode,
    pub detail: String,
}

fn refuse(code: RefusalCode, detail: String) -> BenchmarkRefusal {
    BenchmarkRefusal { code, detail }
}

/// The exact metrics of one run: numerators over positive denominators, and
/// the rounded values in parts per million.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunMetrics {
    /// Sum of the per-query `recall_ppm`.
    pub recall_ppm_sum: u64,
    pub correct: u64,
    pub queries: u64,
    pub recall_at_k_ppm: u64,
    pub answer_accuracy_ppm: u64,
}

/// Admit, or discard with every failing reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BenchmarkVerdict {
    Admit,
    Discard { reasons: Vec<RefusalCode> },
}

/// The scored benchmark: both runs' metrics, the verdict and the evidence
/// premises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchmarkOutcome {
    pub baseline: RunMetrics,
    pub candidate: RunMetrics,
    pub verdict: BenchmarkVerdict,
    pub premises: Vec<String>,
}

/// `numerator * PPM / denominator`, rounded half to even.
fn ratio_ppm(numerator: u128, denominator: u128) -> u128 {
    div_half_even(numerator * PPM, denominator)
}

/// `numerator / denominator`, rounded half to even.
fn div_half_even(numerator: u128, denominator: u128) -> u128 {
    let quotient = numerator / denominator;
    let twice_remainder = (numerator % denominator) * 2;
    match twice_remainder.cmp(&denominator) {
        std::cmp::Ordering::Less => quotient,
        std::cmp::Ordering::Greater => quotient + 1,
        std::cmp::Ordering::Equal => quotient + (quotient & 1),
    }
}

/// NFC, lower case, collapsed whitespace, trimmed.
pub fn normalize_answer(text: &str) -> String {
    let folded: String = text.nfc().collect::<String>().to_lowercase();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The SHA-256 digest of the normalized answer, in lowercase hex.
pub fn answer_digest(text: &str) -> String {
    Digest256::sha256(normalize_answer(text).as_bytes()).to_hex()
}

fn check_dataset(queries: &[BenchmarkQuery]) -> Result<(), BenchmarkRefusal> {
    if queries.is_empty() {
        return Err(refuse(
            MEMORY_DATASET_EMPTY,
            "the dataset has no queries".to_string(),
        ));
    }
    if let Some(query) = queries
        .iter()
        .find(|query| query.expected_evidence_ids.is_empty())
    {
        return Err(refuse(
            MEMORY_EXPECTED_MISSING,
            format!("query {} has no expected evidence ids", query.query_id),
        ));
    }
    Ok(())
}

/// Score one run over the dataset's queries and the first `k` ids of each result.
pub fn score_run(
    queries: &[BenchmarkQuery],
    run: &RunResults,
    k: usize,
) -> Result<RunMetrics, BenchmarkRefusal> {
    check_dataset(queries)?;
    let mut recall_sum: u128 = 0;
    let mut correct: u64 = 0;
    for query in queries {
        let result = run
            .results
            .iter()
            .find(|result| result.query_id == query.query_id)
            .ok_or_else(|| {
                refuse(
                    MEMORY_COVERAGE_INCOMPLETE,
                    format!("the run has no result for query {}", query.query_id),
                )
            })?;
        let top: BTreeSet<&str> = result.top_ids.iter().take(k).map(String::as_str).collect();
        let expected: BTreeSet<&str> = query
            .expected_evidence_ids
            .iter()
            .map(String::as_str)
            .collect();
        let hits = expected.intersection(&top).count() as u128;
        recall_sum += ratio_ppm(hits, expected.len() as u128);
        if answer_digest(&result.answer_text) == query.reference_answer_digest {
            correct += 1;
        }
    }
    let count = queries.len() as u128;
    Ok(RunMetrics {
        recall_ppm_sum: recall_sum as u64,
        correct,
        queries: count as u64,
        recall_at_k_ppm: div_half_even(recall_sum, count) as u64,
        answer_accuracy_ppm: ratio_ppm(u128::from(correct), count) as u64,
    })
}

/// Score a benchmark request under the resolved knobs.
pub fn benchmark(
    request: &MemoryChangeBenchmarkRequest,
    knobs: Option<BenchmarkKnobs>,
) -> Result<BenchmarkOutcome, BenchmarkRefusal> {
    let knobs = knobs.ok_or_else(|| {
        refuse(
            MEMORY_METRIC_UNRESOLVED,
            "a memory_change knob does not resolve to its declared type".to_string(),
        )
    })?;
    if request.k == 0 || request.k > knobs.max_k {
        return Err(refuse(
            MEMORY_METRIC_UNRESOLVED,
            format!("k {} is outside 1..={}", request.k, knobs.max_k),
        ));
    }
    if request.candidate.revision <= request.baseline.revision {
        return Err(refuse(
            MEMORY_REVISION_STALE,
            "the candidate revision is not newer than the baseline revision".to_string(),
        ));
    }
    if request.baseline_results.revision != request.baseline
        || request.candidate_results.revision != request.candidate
    {
        return Err(refuse(
            MEMORY_SNAPSHOT_MISMATCH,
            "a run was measured on another snapshot than the request names".to_string(),
        ));
    }
    let queries = &request.dataset.queries;
    let k = request.k as usize;
    let baseline = score_run(queries, &request.baseline_results, k)?;
    let candidate = score_run(queries, &request.candidate_results, k)?;

    let mut reasons = Vec::new();
    if candidate.recall_at_k_ppm < knobs.min_recall_ppm {
        reasons.push(RECALL_BELOW_MIN);
    }
    if candidate.answer_accuracy_ppm < knobs.min_answer_accuracy_ppm {
        reasons.push(ACCURACY_BELOW_MIN);
    }
    let regressed = |base: u64, cand: u64| base.saturating_sub(cand) > knobs.max_regression_ppm;
    if regressed(baseline.recall_at_k_ppm, candidate.recall_at_k_ppm)
        || regressed(baseline.answer_accuracy_ppm, candidate.answer_accuracy_ppm)
    {
        reasons.push(REGRESSION_ABOVE_MAX);
    }
    let verdict = if reasons.is_empty() {
        BenchmarkVerdict::Admit
    } else {
        BenchmarkVerdict::Discard { reasons }
    };
    let premises = vec![
        format!("memory:dataset={}", request.dataset.digest),
        format!(
            "memory:recall_ppm={}/{}",
            candidate.recall_ppm_sum, candidate.queries
        ),
        format!(
            "memory:accuracy={}/{}",
            candidate.correct, candidate.queries
        ),
        format!("memory:baseline={}", request.baseline.digest),
        format!("memory:candidate={}", request.candidate.digest),
        format!("knob:memory_change.min_recall={}", knobs.min_recall_ppm),
        format!(
            "knob:memory_change.min_answer_accuracy={}",
            knobs.min_answer_accuracy_ppm
        ),
        format!(
            "knob:memory_change.max_regression={}",
            knobs.max_regression_ppm
        ),
        format!("knob:memory_change.max_k={}", knobs.max_k),
    ];
    Ok(BenchmarkOutcome {
        baseline,
        candidate,
        verdict,
        premises,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::memory_benchmark::{
        BenchmarkDataset, ChangeKind, QueryResult, RevisionRef, MEMORY_BENCHMARK_SCHEMA_VERSION,
    };
    use medulla_decision_types::decision::DecisionPolicyRef;

    fn rev(revision: u64) -> RevisionRef {
        RevisionRef {
            revision,
            digest: format!("{revision:064x}"),
        }
    }

    fn query(id: &str, expected: &[&str], answer: &str) -> BenchmarkQuery {
        BenchmarkQuery {
            query_id: id.to_string(),
            expected_evidence_ids: expected.iter().map(|s| s.to_string()).collect(),
            reference_answer_digest: answer_digest(answer),
        }
    }

    fn result(id: &str, top: &[&str], answer: &str) -> QueryResult {
        QueryResult {
            query_id: id.to_string(),
            top_ids: top.iter().map(|s| s.to_string()).collect(),
            answer_text: answer.to_string(),
        }
    }

    fn knobs(min_recall: u64, min_accuracy: u64, max_regression: u64) -> Option<BenchmarkKnobs> {
        Some(BenchmarkKnobs {
            min_recall_ppm: min_recall,
            min_answer_accuracy_ppm: min_accuracy,
            max_regression_ppm: max_regression,
            max_k: 100,
        })
    }

    fn request(
        baseline: Vec<QueryResult>,
        candidate: Vec<QueryResult>,
    ) -> MemoryChangeBenchmarkRequest {
        MemoryChangeBenchmarkRequest {
            schema_version: MEMORY_BENCHMARK_SCHEMA_VERSION,
            tenant_id: "tenant-a".to_string(),
            idempotency_key: "key-1".to_string(),
            record_id: "record-1".to_string(),
            change_kind: ChangeKind::Rewrite,
            baseline: rev(1),
            candidate: rev(2),
            dataset: BenchmarkDataset {
                id: "dataset".to_string(),
                revision: 1,
                digest: "d".repeat(64),
                queries: vec![
                    query("q1", &["a", "b", "c"], "Paris is the capital"),
                    query("q2", &["x"], "forty two"),
                    query("q3", &["m", "n"], "blue"),
                ],
            },
            k: 2,
            evaluator_revision: "eval-1".to_string(),
            policy: DecisionPolicyRef::Default,
            baseline_results: RunResults {
                revision: rev(1),
                results: baseline,
            },
            candidate_results: RunResults {
                revision: rev(2),
                results: candidate,
            },
        }
    }

    fn full() -> Vec<QueryResult> {
        vec![
            result("q1", &["a", "b"], "paris  IS the   capital"),
            result("q2", &["x"], "Forty Two"),
            result("q3", &["m", "n"], "Blue"),
        ]
    }

    // spec: MEDULLA-DECISION-ENGINE-R141.101
    #[test]
    fn recall_and_accuracy_use_exact_counts() {
        // q1 recalls 2 of 3 (666_667 ppm, half-even rounding of 666_666.67),
        // q2 and q3 recall everything: sum 2_666_667 over 3 queries.
        let outcome = benchmark(&request(full(), full()), knobs(800_000, 800_000, 0)).unwrap();
        assert_eq!(outcome.candidate.recall_ppm_sum, 2_666_667);
        assert_eq!(outcome.candidate.queries, 3);
        assert_eq!(outcome.candidate.recall_at_k_ppm, 888_889);
        assert_eq!(
            (
                outcome.candidate.correct,
                outcome.candidate.answer_accuracy_ppm
            ),
            (3, 1_000_000)
        );
        assert_eq!(outcome.verdict, BenchmarkVerdict::Admit);
        assert!(outcome
            .premises
            .contains(&"memory:recall_ppm=2666667/3".to_string()));
        assert!(outcome
            .premises
            .contains(&"memory:accuracy=3/3".to_string()));
        assert!(outcome
            .premises
            .contains(&format!("memory:baseline={}", rev(1).digest)));

        // Half to even: 1/2 -> 500_000 exactly; 1/8 -> 125_000; ties never drift.
        assert_eq!(ratio_ppm(1, 2), 500_000);
        assert_eq!(ratio_ppm(1, 3), 333_333);
        assert_eq!(ratio_ppm(5, 2_000_000), 2); // 2.5 rounds to the even 2
        assert_eq!(ratio_ppm(7, 2_000_000), 4); // 3.5 rounds to the even 4

        // Each failed metric blocks admission on its own.
        let low_recall = vec![
            result("q1", &["z"], "paris is the capital"),
            result("q2", &["x"], "forty two"),
            result("q3", &["m", "n"], "blue"),
        ];
        let by_recall =
            benchmark(&request(full(), low_recall), knobs(900_000, 0, 1_000_000)).unwrap();
        assert_eq!(
            by_recall.verdict,
            BenchmarkVerdict::Discard {
                reasons: vec![RECALL_BELOW_MIN]
            }
        );

        let wrong_answer = vec![
            result("q1", &["a", "b"], "paris is the capital"),
            result("q2", &["x"], "forty two"),
            result("q3", &["m", "n"], "red"),
        ];
        let by_accuracy = benchmark(
            &request(full(), wrong_answer.clone()),
            knobs(0, 900_000, 1_000_000),
        )
        .unwrap();
        assert_eq!(
            by_accuracy.verdict,
            BenchmarkVerdict::Discard {
                reasons: vec![ACCURACY_BELOW_MIN]
            }
        );

        // Mins reached, but a regression above the allowance blocks alone.
        let by_regression =
            benchmark(&request(full(), wrong_answer), knobs(0, 0, 100_000)).unwrap();
        assert_eq!(
            by_regression.verdict,
            BenchmarkVerdict::Discard {
                reasons: vec![REGRESSION_ABOVE_MAX]
            }
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R141.101
    #[test]
    fn each_refusal_carries_its_registered_code() {
        let code = |req: &MemoryChangeBenchmarkRequest, k| benchmark(req, k).unwrap_err().code;
        let mut empty = request(full(), full());
        empty.dataset.queries.clear();
        assert_eq!(code(&empty, knobs(0, 0, 0)), MEMORY_DATASET_EMPTY);

        let mut no_expected = request(full(), full());
        no_expected.dataset.queries[1].expected_evidence_ids.clear();
        assert_eq!(code(&no_expected, knobs(0, 0, 0)), MEMORY_EXPECTED_MISSING);

        let missing = request(full(), full()[..2].to_vec());
        assert_eq!(code(&missing, knobs(0, 0, 0)), MEMORY_COVERAGE_INCOMPLETE);

        let mut wrong_snapshot = request(full(), full());
        wrong_snapshot.candidate_results.revision = rev(9);
        assert_eq!(
            code(&wrong_snapshot, knobs(0, 0, 0)),
            MEMORY_SNAPSHOT_MISMATCH
        );

        assert_eq!(
            code(&request(full(), full()), None),
            MEMORY_METRIC_UNRESOLVED
        );
        let mut big_k = request(full(), full());
        big_k.k = 101;
        assert_eq!(code(&big_k, knobs(0, 0, 0)), MEMORY_METRIC_UNRESOLVED);

        let mut stale = request(full(), full());
        stale.candidate = rev(1);
        stale.candidate_results.revision = rev(1);
        assert_eq!(code(&stale, knobs(0, 0, 0)), MEMORY_REVISION_STALE);
    }
}
