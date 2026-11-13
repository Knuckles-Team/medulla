//! Served pinned retrieval ranking evaluation (MEDULLA-DECISION-ENGINE-R100.100.2):
//! `DecisionEval` `RetrievalRankingSubmit` scores two algorithm runs against
//! independently pinned labels, seals the receipt, and refuses every input the
//! kernel refuses without producing a receipt.

use super::*;
use medulla_decision_types::decision::statistical::ranking_eval::{
    AlgorithmRun, LabelPair, LabelSet, PinnedSet, QueryRanking, QuerySet, RetrievalRankingReceipt,
    RetrievalRankingRequest, RANKING_EVAL_SCHEMA_VERSION,
};
use medulla_decision_types::decision::DecisionJobStatusRequest;

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

/// Four queries `q1..q4`; each has the one relevant document `d<n>`. The
/// lexical run hits q1 and q2 within the first two ids; the vector run hits
/// all four.
fn request(key: &str) -> RetrievalRankingRequest {
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
        tenant_id: TENANT.to_string(),
        idempotency_key: key.into(),
        corpus: pinned("corpus", "ab".repeat(32)),
        labels,
        queries,
        k: 2,
        evaluator_revision: 7,
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
    let lexical = run(
        "lexical",
        vec![
            ranking("q1", &["d1", "x"]),
            ranking("q2", &["x", "d2"]),
            ranking("q3", &["x", "y", "d3"]),
            ranking("q4", &["x", "y"]),
        ],
    );
    let vector = run(
        "vector",
        vec![
            ranking("q1", &["x", "d1"]),
            ranking("q2", &["d2"]),
            ranking("q3", &["d3", "x"]),
            ranking("q4", &["d4"]),
        ],
    );
    request.algorithms = vec![lexical, vector];
    request
}

async fn submit_raw(
    h: &Harness,
    request: RetrievalRankingRequest,
) -> Result<DecisionJobRecord, String> {
    let op = DecisionEvalOp::RetrievalRankingSubmit {
        request: Box::new(request),
    };
    decode(super::super::jobs::handle_decision_eval(&h.state, 61, &verified(), op).await)
}

async fn submit(h: &Harness, request: RetrievalRankingRequest) -> DecisionJobRecord {
    submit_raw(h, request).await.unwrap()
}

async fn status(h: &Harness, job_id: &str) -> Option<DecisionJobRecord> {
    let op = DecisionEvalOp::Status {
        request: DecisionJobStatusRequest {
            tenant_id: TENANT.to_string(),
            job_id: job_id.to_string(),
        },
    };
    decode(super::super::jobs::handle_decision_eval(&h.state, 62, &verified(), op).await).unwrap()
}

fn receipt_of(job: &DecisionJobRecord) -> RetrievalRankingReceipt {
    let DecisionJobOutput::RankingEval { receipt } = succeeded(job).clone() else {
        panic!("ranking output")
    };
    *receipt
}

fn refusal_of(job: &DecisionJobRecord) -> &str {
    match &job.state {
        DecisionJobState::Failed { code } => code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R100.100.2
#[tokio::test]
async fn ranking_evaluation_is_sealed_and_readable() {
    let h = Harness::new().await;
    let request = request("rank-1");
    let job = submit(&h, request.clone()).await;
    let receipt = receipt_of(&job);

    // Exact counts: lexical 2 of 4, vector 4 of 4, in request order.
    assert_eq!(receipt.scores.len(), 2);
    assert_eq!(
        (
            receipt.scores[0].algorithm_id.as_str(),
            receipt.scores[0].numerator,
            receipt.scores[0].denominator,
            receipt.scores[0].value_ppm
        ),
        ("lexical", 2, 4, 500_000)
    );
    assert_eq!(
        (
            receipt.scores[1].algorithm_id.as_str(),
            receipt.scores[1].numerator,
            receipt.scores[1].denominator,
            receipt.scores[1].value_ppm
        ),
        ("vector", 4, 4, 1_000_000)
    );

    // Exact digests: every bound input and the receipt itself.
    assert_eq!(receipt.input_digest, request.input_digest());
    assert_eq!(receipt.corpus_digest, request.corpus.digest);
    assert_eq!(receipt.labels_digest, request.labels.content_digest());
    assert_eq!(receipt.queries_digest, request.queries.content_digest());
    assert_eq!((receipt.k, receipt.evaluator_revision), (2, 7));
    assert_eq!(receipt.receipt_digest, receipt.digest());
    assert_ne!(
        receipt.scores[0].result_digest,
        receipt.scores[1].result_digest
    );

    // The evidence premises name the inputs, the runs, the counts and the knobs.
    let settings = "cd".repeat(32);
    assert_eq!(
        receipt.premises,
        vec![
            format!("corpus:{}", request.corpus.digest),
            format!("labels:{}", request.labels.content_digest()),
            format!("queries:{}", request.queries.content_digest()),
            format!("algorithm:lexical@1:{settings}"),
            format!("algorithm:vector@1:{settings}"),
            "hits_at_k:lexical=2/4".to_string(),
            "hits_at_k:vector=4/4".to_string(),
            "knob:ranking_eval.max_k=1000".to_string(),
            "knob:ranking_eval.min_algorithms=2".to_string(),
            "knob:ranking_eval.max_queries=10000".to_string(),
        ]
    );

    // Read back through Status: the stored job equals the submit result.
    let stored = status(&h, &job.job_id).await.expect("stored job");
    assert_eq!(stored, job);
    assert_eq!(receipt_of(&stored), receipt);

    // An equal replay returns the stored job; the same key with new content
    // is the typed conflict.
    assert_eq!(submit(&h, request.clone()).await, job);
    let mut changed = request.clone();
    changed.k = 3;
    let conflict = submit_raw(&h, changed).await.unwrap_err();
    assert!(conflict.starts_with("IDEMPOTENCY_CONFLICT"), "{conflict}");

    // A foreign tenant is refused before anything is read or written.
    let mut foreign = request.clone();
    foreign.tenant_id = "tenant-other".to_string();
    foreign.idempotency_key = "rank-foreign".to_string();
    let denied = submit_raw(&h, foreign).await.unwrap_err();
    assert!(denied.starts_with("ACCESS_DENIED"), "{denied}");

    // Each refusal code of the kernel, with no receipt.
    refuses_each_unscorable_input_without_a_receipt(&h).await;
}

/// Every input the kernel refuses is a refused job with no receipt.
async fn refuses_each_unscorable_input_without_a_receipt(h: &Harness) {
    let mut cases: Vec<(&str, RetrievalRankingRequest)> = Vec::new();

    let mut empty = request("empty");
    empty.queries.query_ids.clear();
    cases.push(("RANKING_QUERY_SET_EMPTY", empty));

    let mut digest = request("digest");
    digest.labels.pinned.digest = "ef".repeat(32);
    cases.push(("RANKING_DIGEST_MISMATCH", digest));

    let mut k = request("k");
    k.k = 0;
    cases.push(("RANKING_K_OUT_OF_RANGE", k));

    let mut few = request("few");
    few.algorithms.truncate(1);
    cases.push(("RANKING_ALGORITHMS_TOO_FEW", few));

    let mut labels = request("labels");
    labels.labels.pairs.retain(|pair| pair.query_id != "q4");
    labels.labels.pinned.digest = labels.labels.content_digest();
    cases.push(("RANKING_LABELS_MISSING", labels));

    let mut drift = request("drift");
    drift.algorithms[1].corpus_digest = "12".repeat(32);
    cases.push(("RANKING_CORPUS_DRIFT", drift));

    let mut incomplete = request("incomplete");
    incomplete.algorithms[0].rankings.pop();
    cases.push(("RANKING_RESULTS_INCOMPLETE", incomplete));

    let mut settings = request("settings");
    settings.algorithms[0].input_digest = "34".repeat(32);
    cases.push(("RANKING_SETTINGS_MISMATCH", settings));

    for (code, request) in cases {
        let job = submit(h, request).await;
        assert!(
            refusal_of(&job).starts_with(code),
            "{code}: got {}",
            refusal_of(&job)
        );
        // The refused job carries no receipt, and reads back as refused.
        let stored = status(h, &job.job_id).await.expect("stored job");
        assert!(matches!(stored.state, DecisionJobState::Failed { .. }));
    }
}
