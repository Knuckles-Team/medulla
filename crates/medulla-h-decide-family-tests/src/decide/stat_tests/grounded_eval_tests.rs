//! Served grounded evaluation (MEDULLA-DECISION-ENGINE-R100.101.3):
//! `DecisionEval` `GroundedAnswerSubmit` scores pooled topic coverage and the
//! soft-label unsupported-claim Brier score from non-unanimous annotator
//! votes, enforces scorer isolation, pins every bound revision and seals the
//! receipt.

use medulla_decision_policy_types::decision::statistical::ranking_eval::PinnedSet;
use medulla_decision_types::decision::jobs::DecisionJobStatusRequest;
use medulla_h_decide_stat_tests::decide::stat_tests::*;
use medulla_persistence_records::decision_jobs::grounded_pin_key;
use {
    medulla_decision_program_types::decision::statistical::grounded_eval::CitationSupport,
    medulla_decision_program_types::decision::statistical::grounded_eval::ClaimSupport,
    medulla_decision_program_types::decision::statistical::grounded_eval::ClaimVotes,
    medulla_decision_program_types::decision::statistical::grounded_eval::ExpectedTopicSet,
    medulla_decision_program_types::decision::statistical::grounded_eval::GroundedAnswerRequest,
    medulla_decision_program_types::decision::statistical::grounded_eval::GroundedClaimPrediction,
    medulla_decision_program_types::decision::statistical::grounded_eval::GroundedEvalReceipt,
    medulla_decision_program_types::decision::statistical::grounded_eval::GroundedScope,
    medulla_decision_program_types::decision::statistical::grounded_eval::GroundedScorerRecord,
    medulla_decision_program_types::decision::statistical::grounded_eval::TopicSupport,
    medulla_decision_program_types::decision::statistical::grounded_eval::GROUNDED_EVAL_SCHEMA_VERSION,
};

fn pin(id: &str, digest: String) -> PinnedSet {
    PinnedSet {
        id: id.into(),
        revision: 1,
        digest,
    }
}

fn support(query: &str, topic: &str, claim: &str, supporting: u32, total: u32) -> TopicSupport {
    TopicSupport {
        query_id: query.into(),
        topic_id: topic.into(),
        claims: vec![ClaimSupport {
            claim_id: claim.into(),
            citations: vec![CitationSupport {
                citation_id: format!("{claim}-cite"),
                supporting,
                total,
            }],
        }],
    }
}

fn prediction(claim: &str, p_ppm: u32) -> GroundedClaimPrediction {
    GroundedClaimPrediction {
        claim_id: claim.into(),
        answer_digest: format!("answer-{claim}"),
        input_digest: format!("input-{claim}"),
        scorer_id: "detector".into(),
        scorer_revision: 4,
        scorer_artifact_digest: "ee".repeat(32),
        p_ppm,
    }
}

fn votes(claim: &str, annotators: &[&str], unsupported: u32, supported: u32) -> ClaimVotes {
    ClaimVotes {
        claim_id: claim.into(),
        annotators: annotators.iter().map(|a| (*a).into()).collect(),
        unsupported_votes: unsupported,
        supported_votes: supported,
    }
}

/// Recompute every content-bound digest after the content changed.
fn rebind(request: &mut GroundedAnswerRequest) {
    request.queries.digest = request.queries_content_digest();
    request.claims.digest = request.claims_content_digest();
    request.citations.digest = request.citations_content_digest();
    request.answers.digest = request.answers_content_digest();
    request.labels.digest = request.labels_content_digest();
}

/// Two queries, three topics (`t1`, `t3` covered, `t2` not) and three claims
/// with non-unanimous annotator votes.
fn request(key: &str) -> GroundedAnswerRequest {
    let mut request = GroundedAnswerRequest {
        schema_version: GROUNDED_EVAL_SCHEMA_VERSION,
        tenant_id: TENANT.to_string(),
        idempotency_key: key.into(),
        corpus: pin("corpus", "ab".repeat(32)),
        queries: pin("queries", String::new()),
        answers: pin("answers", String::new()),
        claims: pin("claims", String::new()),
        citations: pin("citations", String::new()),
        labels: pin("labels", String::new()),
        scope: GroundedScope::LocalMultiDocument,
        evaluator_revision: 3,
        policy: DecisionPolicyRef::Default,
        expected: vec![
            ExpectedTopicSet {
                query_id: "q1".into(),
                topic_ids: vec!["t1".into(), "t2".into()],
            },
            ExpectedTopicSet {
                query_id: "q2".into(),
                topic_ids: vec!["t3".into()],
            },
        ],
        support: vec![
            support("q1", "t1", "c1", 2, 3),
            support("q1", "t2", "c2", 1, 3),
            support("q2", "t3", "c3", 3, 4),
        ],
        claim_ids: vec!["c1".into(), "c2".into(), "c3".into()],
        predictions: vec![
            prediction("c1", 100_000),
            prediction("c2", 800_000),
            prediction("c3", 500_000),
        ],
        votes: vec![
            votes("c1", &["a1", "a2", "a3"], 1, 2),
            votes("c2", &["a1", "a2", "a3"], 2, 1),
            votes("c3", &["a1", "a2", "a3", "a4"], 2, 2),
        ],
        scorer: GroundedScorerRecord {
            scorer_id: "detector".into(),
            scorer_revision: 4,
            scorer_artifact_digest: "ee".repeat(32),
            inputs_read: vec!["ff".repeat(32)],
        },
    };
    rebind(&mut request);
    request
}

async fn submit_raw(
    h: &Harness,
    request: GroundedAnswerRequest,
) -> Result<DecisionJobRecord, String> {
    let op = DecisionEvalOp::GroundedAnswerSubmit {
        request: Box::new(request),
    };
    decode(
        medulla_h_decide::decide::jobs::handle_decision_eval(&h.state, 71, &verified(), op).await,
    )
}

async fn submit(h: &Harness, request: GroundedAnswerRequest) -> DecisionJobRecord {
    submit_raw(h, request).await.unwrap()
}

async fn status(h: &Harness, job_id: &str) -> Option<DecisionJobRecord> {
    let op = DecisionEvalOp::Status {
        request: DecisionJobStatusRequest {
            tenant_id: TENANT.to_string(),
            job_id: job_id.to_string(),
        },
    };
    decode(
        medulla_h_decide::decide::jobs::handle_decision_eval(&h.state, 72, &verified(), op).await,
    )
    .unwrap()
}

fn receipt_of(job: &DecisionJobRecord) -> GroundedEvalReceipt {
    let DecisionJobOutput::GroundedEval { receipt } = succeeded(job).clone() else {
        panic!("grounded output")
    };
    *receipt
}

fn refusal_of(job: &DecisionJobRecord) -> &str {
    match &job.outcome {
        DecisionJobOutcome::Failed { code } => code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

async fn assert_refused(h: &Harness, code: &str, request: GroundedAnswerRequest) {
    let job = submit(h, request).await;
    assert!(
        refusal_of(&job).starts_with(code),
        "{code}: got {}",
        refusal_of(&job)
    );
    let stored = status(h, &job.job_id).await.expect("stored job");
    assert!(matches!(stored.outcome, DecisionJobOutcome::Failed { .. }));
}

// spec: MEDULLA-DECISION-ENGINE-R100.101.3, MEDULLA-DECISION-ENGINE-R100.101.2
#[tokio::test]
async fn grounded_evaluation_is_sealed_with_scorer_isolation() {
    let h = Harness::new().await;
    let request = request("g-1");
    let job = submit(&h, request.clone()).await;
    let receipt = receipt_of(&job);

    // Pooled coverage: covered 2 of 3 (never the mean of 1/2 and 1/1).
    assert_eq!(receipt.scope, GroundedScope::LocalMultiDocument);
    assert_eq!(
        (receipt.coverage.covered, receipt.coverage.expected),
        (2, 3)
    );
    assert_eq!(receipt.coverage.queries[0].covered_topic_ids, vec!["t1"]);
    assert_eq!(
        receipt.coverage.queries[0].expected_topic_ids,
        vec!["t1", "t2"]
    );
    assert_eq!(receipt.coverage.queries[1].covered_topic_ids, vec!["t3"]);

    // Brier from the soft labels q = 333333, 666667, 500000 ppm and the
    // predictions 100000, 800000, 500000 ppm, weighted by 3, 3 and 4 votes.
    let expected_numerator: u128 = [
        (3u128, 100_000i128, 333_333i128),
        (3, 800_000, 666_667),
        (4, 500_000, 500_000),
    ]
    .iter()
    .map(|(v, p, q)| v * ((p - q) * (p - q)) as u128)
    .sum();
    assert_eq!(expected_numerator, 216_665_933_334);
    assert_eq!(receipt.brier.numerator, expected_numerator);
    assert_eq!(receipt.brier.denominator, 10);
    assert_eq!(receipt.brier.digits, 6);
    assert_eq!(receipt.brier.score_scaled, 21_667);

    // Every binding reads back with its role, revision and digest.
    let roles: Vec<(&str, &str, u32, &str)> = receipt
        .bindings
        .iter()
        .map(|b| {
            (
                b.role.as_str(),
                b.pinned.id.as_str(),
                b.pinned.revision,
                b.pinned.digest.as_str(),
            )
        })
        .collect();
    assert_eq!(
        roles,
        vec![
            ("corpus", "corpus", 1, request.corpus.digest.as_str()),
            ("queries", "queries", 1, request.queries.digest.as_str()),
            ("answers", "answers", 1, request.answers.digest.as_str()),
            ("claims", "claims", 1, request.claims.digest.as_str()),
            (
                "citations",
                "citations",
                1,
                request.citations.digest.as_str()
            ),
            ("labels", "labels", 1, request.labels.digest.as_str()),
        ]
    );
    assert_eq!(receipt.evaluator_revision, 3);
    assert!(!receipt.policy_digest.is_empty());
    assert_eq!(
        (receipt.scorer_id.as_str(), receipt.scorer_revision),
        ("detector", 4)
    );
    assert_eq!(receipt.scorer_artifact_digest, "ee".repeat(32));
    assert_eq!(receipt.scorer_inputs, vec!["ff".repeat(32)]);
    assert_eq!(receipt.receipt_digest, receipt.digest());
    assert_eq!(
        receipt.premises,
        vec![
            "grounded:scope=local_multi_document".to_string(),
            "grounded:coverage=2/3".to_string(),
            "grounded:brier=216665933334/10".to_string(),
            "grounded:scorer=detector@4".to_string(),
            "knob:grounded.score_digits=6".to_string(),
            "knob:grounded.min_topic_support=0.500000".to_string(),
            "knob:grounded.min_valid_votes=3".to_string(),
        ]
    );

    // Status reads the same job; every bound digest is stored under its pin.
    let stored = status(&h, &job.job_id).await.expect("stored job");
    assert_eq!(stored, job);
    for (_, pinned) in request.bindings() {
        let row = h
            .store
            .decision_artifact(TENANT, &grounded_pin_key(&pinned.id, pinned.revision))
            .unwrap();
        assert_eq!(row.as_deref(), Some(pinned.digest.as_bytes()));
    }

    // An equal replay returns the stored job; the same key with new content
    // is the typed conflict.
    assert_eq!(submit(&h, request.clone()).await, job);
    let mut changed = request.clone();
    changed.evaluator_revision = 4;
    let conflict = submit_raw(&h, changed).await.unwrap_err();
    assert!(conflict.starts_with("IDEMPOTENCY_CONFLICT"), "{conflict}");

    // Scorer isolation: a scorer that read the label set is refused.
    let mut saw = request.clone();
    saw.idempotency_key = "g-saw".into();
    saw.scorer.inputs_read.push(request.labels.digest.clone());
    assert_refused(&h, "GROUNDED_SCORER_SAW_LABELS", saw).await;

    // Digest binding: a pin that is not the digest of its content, a malformed
    // digest, and a revision stored with another digest are all refused.
    let mut tampered = request.clone();
    tampered.idempotency_key = "g-tamper".into();
    tampered.labels.digest = "12".repeat(32);
    assert_refused(&h, "GROUNDED_DIGEST_MISMATCH", tampered).await;
    let mut malformed = request.clone();
    malformed.idempotency_key = "g-malformed".into();
    malformed.corpus.digest = "NOT-HEX".into();
    assert_refused(&h, "GROUNDED_DIGEST_MISMATCH", malformed).await;
    let mut restated = request.clone();
    restated.idempotency_key = "g-restated".into();
    restated.votes[0] = votes("c1", &["a1", "a2", "a3"], 0, 3);
    rebind(&mut restated);
    assert_refused(&h, "GROUNDED_DIGEST_MISMATCH", restated).await;
    let mut other_corpus = request.clone();
    other_corpus.idempotency_key = "g-corpus".into();
    other_corpus.corpus.digest = "cd".repeat(32);
    assert_refused(&h, "GROUNDED_DIGEST_MISMATCH", other_corpus).await;

    // Staleness: once labels revision 2 is stored, revision 1 is stale.
    let mut next = request.clone();
    next.idempotency_key = "g-rev2".into();
    next.labels.revision = 2;
    receipt_of(&submit(&h, next).await);
    let mut stale = request.clone();
    stale.idempotency_key = "g-stale".into();
    assert_refused(&h, "GROUNDED_REVISION_STALE", stale).await;

    // A foreign tenant is refused before anything is read or written.
    let mut foreign = request.clone();
    foreign.tenant_id = "tenant-other".into();
    foreign.idempotency_key = "g-foreign".into();
    let denied = submit_raw(&h, foreign).await.unwrap_err();
    assert!(denied.starts_with("ACCESS_DENIED"), "{denied}");

    // Each kernel refusal code, with no receipt. A fresh store: a revision the
    // evaluation above already accepted pins its digest, so a changed set at
    // that revision would be refused for its digest before the kernel runs.
    refuses_each_unscorable_input_without_a_receipt(&Harness::new().await).await;
}

/// Every input the kernels refuse is a refused job with no receipt.
async fn refuses_each_unscorable_input_without_a_receipt(h: &Harness) {
    let mutate = |key: &str, change: &dyn Fn(&mut GroundedAnswerRequest)| {
        let mut request = request(key);
        change(&mut request);
        rebind(&mut request);
        request
    };
    let cases: Vec<(&str, GroundedAnswerRequest)> = vec![
        (
            "GROUNDED_QUERY_SET_EMPTY",
            mutate("e1", &|r| r.expected.clear()),
        ),
        (
            "GROUNDED_TOPIC_SET_EMPTY",
            mutate("e2", &|r| r.expected[0].topic_ids.clear()),
        ),
        (
            "GROUNDED_VOTES_TOO_FEW",
            mutate("e3", &|r| r.support[0] = support("q1", "t1", "c1", 1, 2)),
        ),
        (
            "GROUNDED_TOPIC_DUPLICATE",
            mutate("e4", &|r| {
                r.expected[0].topic_ids = vec!["t1".into(), "t1".into()]
            }),
        ),
        (
            "GROUNDED_CLAIM_SET_EMPTY",
            mutate("e5", &|r| r.claim_ids.clear()),
        ),
        (
            "GROUNDED_PREDICTION_MISSING",
            mutate("e6", &|r| {
                r.predictions.pop();
            }),
        ),
        (
            "GROUNDED_PREDICTION_EXTRA",
            mutate("e7", &|r| r.predictions.push(prediction("c9", 1))),
        ),
        (
            "GROUNDED_ANNOTATOR_DUPLICATE",
            mutate("e8", &|r| {
                r.votes[0] = votes("c1", &["a1", "a1", "a2"], 1, 2)
            }),
        ),
        (
            "GROUNDED_VOTES_ZERO",
            mutate("e9", &|r| r.votes[2] = votes("c3", &[], 0, 0)),
        ),
        (
            "GROUNDED_PROBABILITY_OUT_OF_RANGE",
            mutate("e10", &|r| r.predictions[0].p_ppm = 1_000_001),
        ),
    ];
    for (code, request) in cases {
        assert_refused(h, code, request).await;
    }
}
