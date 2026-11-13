//! Distilled score ranking, served (MEDULLA-DECISION-ENGINE-R147.3, R147.4,
//! R147.5): teacher judgments are uploaded through `DecisionLog` `Learn`,
//! `Decide` ranks graph candidates from the stored scores alone, every
//! refusal precedes ranking and writes nothing, and the evidence names the
//! scorer.

use super::retrieval_tests::learn;
use super::*;
use medulla_decision_types::decision::statistical::distilled::{
    DistilledScorerManifest, DistilledUpload, TeacherJudgment,
};
use medulla_decision_types::decision::statistical::retrieval::{LearningRecorded, LearningWrite};
use medulla_persistence_records::decision_jobs::encode_artifact;
use medulla_types::contract::Digest256;

const GRAPH: &str = "kg-distilled-rank";

fn judgment(id: &str, score: u32, lo: u32) -> TeacherJudgment {
    TeacherJudgment {
        document_id: id.to_string(),
        score_q16: score,
        fidelity_lo_q16: lo,
        fidelity_hi_q16: 65_536,
    }
}

fn upload(revision: u64, scores: Vec<TeacherJudgment>) -> LearningWrite {
    LearningWrite::UploadDistilledScores {
        upload: Box::new(DistilledUpload {
            scorer_id: "scorer-1".to_string(),
            scorer_revision: revision,
            scores: BoundedVec::new(scores).unwrap(),
        }),
    }
}

async fn send(h: &Harness, write: LearningWrite) -> Result<DistilledScorerManifest, String> {
    match learn(h, "decider", write).await {
        Ok(LearningRecorded::DistilledScores(manifest)) => Ok(*manifest),
        Ok(_) => Err("not a distilled manifest".to_string()),
        Err(error) => Err(error),
    }
}

fn rows(h: &Harness, tenant: &str) -> Vec<(String, Vec<u8>)> {
    h.store
        .decision_artifacts_with_prefix(tenant, "distilled/", 10_000)
        .unwrap()
}

/// A harness with five `Tool` nodes in the graph and a pinned schema.
async fn fixture() -> (Harness, ComponentDependency) {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    {
        let mut guard = h.state.write().await;
        guard
            .registry
            .create_graph(
                GRAPH,
                medulla_protocol_types::protocol::GraphType::Team,
                None,
            )
            .unwrap();
        let core = guard.registry.get(GRAPH).unwrap().core.clone();
        for id in ["n-a", "n-b", "n-c", "n-d", "n-e"] {
            let props = serde_json::json!({"type": "Tool", "score": 0.5});
            core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
        }
    }
    let body = FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![number_feature("score")]).unwrap(),
    };
    let pin = h
        .publish_test_component(
            "schema-distilled",
            AgentComponentKind::FeatureSchema,
            "distilled features",
            Some(&body),
            None,
        )
        .unwrap();
    (h, pin)
}

fn rank_request(pin: &ComponentDependency, scorer: Option<(u64, u32)>) -> DecideRequest {
    let mut request = request(
        pin,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    request.question.kind = QuestionKind::Rank;
    request.candidates = CandidateSource::Graph {
        graph: GRAPH.to_string(),
        plan: Box::new(medulla_types::wire::Plan::new(vec![
            medulla_types::wire::Op::Scan {
                label: "Tool".to_string(),
            },
        ])),
    };
    let mut params = Vec::new();
    if let Some((revision, top_k)) = scorer {
        params.push(TypedParam {
            name: "distilled_scorer".to_string(),
            value: TypedValue::DistilledScorer {
                scorer_id: "scorer-1".to_string(),
                scorer_revision: revision,
                top_k,
            },
        });
    }
    request.params = BoundedVec::new(params).unwrap();
    request
}

fn ranked(batch: &DecisionBatch) -> Vec<String> {
    let StatisticalOutcome::Advisory { scores, .. } = &batch.records.as_slice()[0].outcome else {
        panic!("an advisory ranking")
    };
    scores.iter().map(|s| s.option_id.clone()).collect()
}

fn premise_facts(batch: &DecisionBatch) -> Vec<String> {
    batch.records.as_slice()[0]
        .premises
        .iter()
        .map(|p| p.fact.clone())
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R147.3
#[tokio::test]
async fn stored_scores_rank_without_model_calls() {
    let (h, pin) = fixture().await;
    let first = vec![judgment("n-a", 100, 40_000), judgment("n-b", 900, 40_000)];
    let m1 = send(&h, upload(1, first.clone())).await.unwrap();
    assert_eq!(m1.score_count, 2);
    let rest = vec![
        judgment("n-c", 900, 40_000),
        judgment("n-d", 500, 40_000),
        judgment("n-e", 50, 40_000),
        // Repeating a stored judgment exactly is idempotent.
        judgment("n-a", 100, 40_000),
    ];
    let m2 = send(&h, upload(1, rest)).await.unwrap();
    assert_eq!(m2.score_count, 5);

    // The digest chain: sha256(previous digest bytes ++ batch digest bytes).
    let batch_digest = |judgments: &[TeacherJudgment]| {
        Digest256::sha256(&encode_artifact(&judgments.to_vec()).unwrap())
    };
    let chain = |previous: [u8; 32], batch: Digest256| {
        let mut bytes = previous.to_vec();
        bytes.extend_from_slice(batch.as_bytes());
        Digest256::sha256(&bytes)
    };
    let expected1 = chain([0u8; 32], batch_digest(&first));
    assert_eq!(m1.score_digest, expected1);
    let new_in_second = [
        judgment("n-c", 900, 40_000),
        judgment("n-d", 500, 40_000),
        judgment("n-e", 50, 40_000),
    ];
    assert_eq!(
        m2.score_digest,
        chain(*expected1.as_bytes(), batch_digest(&new_in_second))
    );

    // An exact replay of a stored batch changes nothing.
    let before = rows(&h, TENANT);
    assert_eq!(send(&h, upload(1, first)).await.unwrap(), m2);
    assert_eq!(rows(&h, TENANT), before);

    // Ranking: score descending, the n-b/n-c tie by id, top 3.
    let batch = decide(&h, rank_request(&pin, Some((1, 3)))).await.unwrap();
    assert_eq!(ranked(&batch), ["n-b", "n-c", "n-d"]);
    let facts = premise_facts(&batch);
    assert!(
        facts.contains(&"distilled:model_calls=0".to_string()),
        "{facts:?}"
    );
    assert!(facts.contains(&format!("distilled:digest={}", m2.score_digest.to_hex())));
    // Ranking writes nothing.
    assert_eq!(rows(&h, TENANT), before);

    // Another tenant has no scores, and its decide is refused for this carrier.
    assert!(rows(&h, "other-tenant").is_empty());
    let mut foreign = rank_request(&pin, Some((1, 3)));
    foreign.tenant_id = "other-tenant".to_string();
    let denied = decide(&h, foreign).await.unwrap_err();
    assert!(denied.starts_with("ACCESS_DENIED"), "{denied}");
    let op = DecisionLogOp::Learn {
        tenant_id: "other-tenant".to_string(),
        write: upload(1, vec![judgment("n-a", 1, 40_000)]),
    };
    assert!(decode::<LearningRecorded>(log_op(&h, "decider", op).await).is_err());
    assert!(rows(&h, "other-tenant").is_empty());
}

// spec: MEDULLA-DECISION-ENGINE-R147.4
#[tokio::test]
async fn missing_score_refuses() {
    let (h, pin) = fixture().await;
    // Nothing stored: the scorer is empty.
    let empty = decide(&h, rank_request(&pin, Some((1, 3))))
        .await
        .unwrap_err();
    assert!(empty.contains("DISTILLED_SCORER_EMPTY"), "{empty}");

    let four = vec![
        judgment("n-a", 100, 40_000),
        judgment("n-b", 900, 40_000),
        judgment("n-c", 700, 40_000),
        judgment("n-d", 500, 40_000),
    ];
    send(&h, upload(1, four)).await.unwrap();
    let stored = rows(&h, TENANT);
    let missing = decide(&h, rank_request(&pin, Some((1, 3))))
        .await
        .unwrap_err();
    assert!(missing.contains("DISTILLED_SCORE_MISSING"), "{missing}");
    assert!(missing.contains("n-e"), "{missing}");

    // A different revision than the stored one is stale.
    let stale = decide(&h, rank_request(&pin, Some((2, 3))))
        .await
        .unwrap_err();
    assert!(stale.contains("SCORER_REVISION_STALE"), "{stale}");
    // top_k 0 and above the knob refuse.
    for top_k in [0, 101] {
        let bad = decide(&h, rank_request(&pin, Some((1, top_k))))
            .await
            .unwrap_err();
        assert!(bad.contains("TOP_K_INVALID"), "{bad}");
    }

    // A used score under the fidelity floor (0.5 = 32768 Q16) refuses.
    send(&h, upload(1, vec![judgment("n-e", 10, 1_000)]))
        .await
        .unwrap();
    let low = decide(&h, rank_request(&pin, Some((1, 3))))
        .await
        .unwrap_err();
    assert!(low.contains("DISTILLED_FIDELITY_BELOW_FLOOR"), "{low}");
    assert!(low.contains("n-e"), "{low}");

    // Upload refusals: a different score for a stored document, an inverted
    // bound, and an older revision after a newer one.
    let after_upload = rows(&h, TENANT);
    let conflict = send(&h, upload(1, vec![judgment("n-a", 101, 40_000)]))
        .await
        .unwrap_err();
    assert!(conflict.contains("IDEMPOTENCY_CONFLICT"), "{conflict}");
    let mut inverted = judgment("n-z", 1, 50_000);
    inverted.fidelity_hi_q16 = 40_000;
    let invalid = send(&h, upload(1, vec![inverted])).await.unwrap_err();
    assert!(invalid.contains("DISTILLED_SCORE_INVALID"), "{invalid}");
    // Every refusal wrote nothing: the manifest and batches are unchanged.
    assert_eq!(rows(&h, TENANT), after_upload);
    send(&h, upload(2, vec![judgment("n-a", 1, 40_000)]))
        .await
        .unwrap();
    let after_newer = rows(&h, TENANT);
    let older = send(&h, upload(1, vec![judgment("n-z", 1, 40_000)]))
        .await
        .unwrap_err();
    assert!(older.contains("SCORER_REVISION_STALE"), "{older}");
    assert_eq!(rows(&h, TENANT), after_newer);
    assert_ne!(after_upload, after_newer);
    assert!(stored.len() < after_upload.len());

    // Another tenant sees none of it.
    assert!(rows(&h, "other-tenant").is_empty());
}

// spec: MEDULLA-DECISION-ENGINE-R147.5
#[tokio::test]
async fn distilled_evidence_binds_scorer() {
    let (h, pin) = fixture().await;
    let all = vec![
        judgment("n-a", 100, 40_000),
        judgment("n-b", 900, 36_000),
        judgment("n-c", 700, 50_000),
        judgment("n-d", 500, 40_000),
        judgment("n-e", 50, 45_000),
    ];
    let manifest = send(&h, upload(1, all)).await.unwrap();
    let batch = decide(&h, rank_request(&pin, Some((1, 2)))).await.unwrap();
    assert_eq!(ranked(&batch), ["n-b", "n-c"]);
    let facts = premise_facts(&batch);
    for expected in [
        "distilled:scorer=scorer-1@1".to_string(),
        format!("distilled:digest={}", manifest.score_digest.to_hex()),
        "distilled:score_count=5".to_string(),
        format!("distilled:persisted_at_ms={}", manifest.persisted_at_ms),
        "distilled:fidelity_lo_min_q16=36000".to_string(),
        "distilled:model_calls=0".to_string(),
        "knob:distilled.max_top_k=100".to_string(),
        "knob:distilled.max_scores_per_write=4096".to_string(),
        "knob:distilled.min_fidelity=0.500000".to_string(),
    ] {
        assert!(facts.contains(&expected), "{expected} in {facts:?}");
    }
    // Stored scores are claims by the scorer principal.
    for premise in batch.records.as_slice()[0]
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("distilled:"))
    {
        assert_eq!(
            premise.class,
            medulla_decision_types::decision::PremiseClass::Claim
        );
    }
    // Another tenant's rows carry no scorer.
    assert!(rows(&h, "other-tenant").is_empty());
}
