//! The `paper_admission` family through the actual Decide handler
//! (MEDULLA-DECISION-ENGINE-R031.100): staged papers admit on the pinned head's
//! calibrated relevance over two features (BM25 of the summary against the
//! platform goals, similarity to the nearest content-graph row), a paper
//! relevant to only one is not admitted, an admitted paper carries its claim
//! ids, a repeated content digest and a moved graph are refused.

use super::scorer_tests::evaluated;
use super::*;
use medulla_decision_types::decision::knobs::KnobValue;
use medulla_decision_types::decision::paper_admission::{
    PARAM_GOALS, PARAM_GRAPH_VERSION, TEXT_CLAIM_IDS, TEXT_CONTENT_DIGEST,
};
use medulla_decision_types::decision::statistical::features::{
    FeatureKind, FeatureSpec, MissingValue,
};
use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
use medulla_decision_types::decision::AbstainReason;

const STAGED: &str = "staged-papers";
const CONTENT: &str = "platform-content";
const GOALS: &str = "graph retrieval calibration";
const CONTENT_PARAM: &str = "platform.graph";
const SUMMARY_KEY: &str = "paper.summary";

fn schema_body() -> FeatureSchemaBody {
    FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![
            FeatureSpec {
                name: "goals".to_string(),
                kind: FeatureKind::TextBm25 {
                    key: SUMMARY_KEY.to_string(),
                    param: PARAM_GOALS.to_string(),
                },
                missing: MissingValue::Abstain,
            },
            FeatureSpec {
                name: "nn".to_string(),
                kind: FeatureKind::NearestNeighbourSimilarity {
                    graph_param: CONTENT_PARAM.to_string(),
                },
                missing: MissingValue::Abstain,
            },
        ])
        .unwrap(),
    }
}

/// Fit, evaluate and publish a head over the two paper features. The labelled
/// items hold four candidates: relevant to both, to the goals only, to the
/// graph only, and to neither; only the first is acceptable.
async fn published_head(h: &Harness) -> (ComponentDependency, ComponentDependency) {
    let body = schema_body();
    let schema_pin = h
        .publish_test_component(
            "schema-paper",
            AgentComponentKind::FeatureSchema,
            "paper features",
            Some(&body),
            None,
        )
        .unwrap();
    let digest = encode_body(&body).unwrap().content_digest;
    let ids: Vec<String> = ["both", "goals", "graph", "none"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let (high, low) = (2_i64 << 32, 0_i64);
    let unit = 1_i64 << 32;
    let mut data = dataset(
        &digest,
        &ids,
        &[high, unit, high, low, low, unit, low, low],
        400,
    );
    data.feature_names = BoundedVec::new(vec!["goals".to_string(), "nn".to_string()]).unwrap();
    let gold = super::super::stat_jobs::dataset_digest(&data).unwrap();
    let request = DecisionFitRequest {
        tenant_id: TENANT.to_string(),
        idempotency_key: "fit-paper".to_string(),
        head_kind: HeadKind::ListwiseLogistic,
        feature_schema: schema_pin.clone(),
        policy: DecisionPolicyRef::Default,
        label_regime: LabelRegime::FullLabel {
            gold_set_digest: gold,
        },
        window: window(),
        optimiser: fit_optimiser(),
        source: DatasetSource::Inline {
            dataset: Box::new(data.clone()),
        },
        penalty: None,
        features: Default::default(),
        parent_head: None,
        calibration_method: None,
    };
    let job: DecisionJobRecord = decode(
        super::super::jobs::handle_decision_fit(
            &h.state,
            2,
            &verified(),
            DecisionFitOp::Submit {
                request: Box::new(request),
            },
        )
        .await,
    )
    .unwrap();
    let DecisionJobOutput::Fit {
        draft,
        draft_sha256,
        draft_length,
        ..
    } = succeeded(&job).clone()
    else {
        panic!("fit output")
    };
    let receipt = evaluated(h, 3, (draft_sha256, draft_length), data).await;
    assert!(
        receipt.passed,
        "gates: {:?}",
        receipt.failed_gates.as_slice()
    );
    let head_pin = h
        .publish_test_component(
            "head-paper",
            AgentComponentKind::DecisionHead,
            "paper relevance head",
            Some(draft.as_ref()),
            Some(receipt.receipt_digest.clone()),
        )
        .unwrap();
    (schema_pin, head_pin)
}

fn advisory_policy(h: &Harness, relevance_min_micros: Option<i64>) -> DecisionPolicyRef {
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.cold_start = ColdStart::AdvisoryUncalibrated;
    if let Some(floor) = relevance_min_micros {
        let spec = &medulla_decision_types::decision::paper_admission::KNOBS[0];
        policy
            .knobs
            .insert(spec.knob_name(), KnobValue::DecimalMicros(floor));
    }
    DecisionPolicyRef::Pinned {
        component: h.publish_policy(
            &format!("policy-paper-{}", relevance_min_micros.unwrap_or(-1)),
            &policy,
        ),
    }
}

/// One staged paper node: its summary, content digest and claim ids, with a
/// stored vector.
fn paper(
    core: &medulla_graph_core::graph::GraphCore,
    id: &str,
    summary: &str,
    digest: &str,
    claims: &str,
    vector: Vec<f32>,
) {
    let props = serde_json::json!({
        "type": "Paper",
        SUMMARY_KEY: summary,
        TEXT_CONTENT_DIGEST: digest,
        TEXT_CLAIM_IDS: claims,
    });
    core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
    core.semantic_store
        .write()
        .add_embedding(id.to_string(), vector)
        .unwrap();
}

/// The staged-papers graph and the platform content graph, both in one space.
/// Returns the staged graph's version.
async fn graphs(h: &Harness, papers: &[(&str, &str, &str, &str, Vec<f32>)]) -> u64 {
    let staged = super::learning_tests::graph_with(h, STAGED, "paper-m", 3, Vec::new()).await;
    super::learning_tests::graph_with(
        h,
        CONTENT,
        "paper-m",
        3,
        vec![
            ("content-a".to_string(), vec![1.0, 0.0, 0.0]),
            ("content-b".to_string(), vec![0.9, 0.1, 0.0]),
        ],
    )
    .await;
    for (id, summary, digest, claims, vector) in papers {
        paper(&staged, id, summary, digest, claims, vector.clone());
    }
    staged.version()
}

fn papers_request(
    schema: &ComponentDependency,
    head: &ComponentDependency,
    policy: DecisionPolicyRef,
    graph_version: u64,
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "paper.admission".to_string(),
            kind: QuestionKind::PaperAdmission,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Graph {
            graph: STAGED.to_string(),
            plan: Box::new(medulla_types::wire::Plan::new(vec![
                medulla_types::wire::Op::Scan {
                    label: "Paper".to_string(),
                },
            ])),
        },
        feature_schema: schema.clone(),
        head: Some(head.clone()),
        policy,
        params: BoundedVec::new(vec![
            TypedParam {
                name: PARAM_GRAPH_VERSION.to_string(),
                value: TypedValue::Int(i64::try_from(graph_version).unwrap()),
            },
            TypedParam {
                name: PARAM_GOALS.to_string(),
                value: TypedValue::Text(GOALS.to_string()),
            },
            TypedParam {
                name: CONTENT_PARAM.to_string(),
                value: TypedValue::Text(CONTENT.to_string()),
            },
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn admitted(record: &StatisticalDecisionRecord) -> Vec<String> {
    match &record.outcome {
        StatisticalOutcome::Advisory { scores, .. } => {
            scores.iter().map(|s| s.option_id.clone()).collect()
        }
        other => panic!("advisory outcome, got {other:?}"),
    }
}

fn refusal_code(record: &StatisticalDecisionRecord) -> String {
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("abstained outcome, got {:?}", record.outcome)
    };
    match &reasons.as_slice()[0] {
        AbstainReason::Family { code, .. } => code.as_str().to_string(),
        other => panic!("family refusal, got {other:?}"),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R031.100
#[tokio::test]
async fn only_relevant_papers_are_admitted_with_claims() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let (schema, head) = published_head(&h).await;
    let papers = [
        (
            "paper-both",
            "graph retrieval calibration of graph embeddings for retrieval",
            "sha256:both",
            "claim-1,claim-2",
            vec![1.0, 0.05, 0.0],
        ),
        (
            "paper-goals",
            "calibration of graph retrieval scores in retrieval studies",
            "sha256:goals",
            "claim-3",
            vec![0.0, 0.0, 1.0],
        ),
        (
            "paper-graph",
            "baking sourdough bread with rye flour and patience",
            "sha256:graph",
            "claim-4",
            vec![0.95, 0.05, 0.0],
        ),
    ];
    let version = graphs(&h, &papers).await;

    // The paper relevant to both goals and graph is admitted with its claims;
    // the one relevant to only the goals and the one relevant to only the
    // graph are not.
    let batch = decide(
        &h,
        papers_request(&schema, &head, advisory_policy(&h, None), version),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::PaperAdmission);
    assert_eq!(admitted(record), ["paper-both"]);
    let facts: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
    for wanted in [
        "relevance:paper-both=",
        "bm25:platform.goals:paper-both=",
        "nearest_neighbour_similarity:paper-both=",
        "paper.claim_ids:paper-both=claim-1,claim-2",
    ] {
        assert!(
            facts.iter().any(|fact| fact.starts_with(wanted)),
            "premise {wanted} in {facts:?}"
        );
    }
    assert!(
        !facts
            .iter()
            .any(|fact| fact.contains("paper-goals") || fact.contains("paper-graph")),
        "no premise for a paper that is not admitted: {facts:?}"
    );
    assert_eq!(record.evidence_class, EvidenceClass::Observation);

    // Below the threshold: a floor of 1.0 admits nothing, not even the paper
    // that was admitted at the default floor.
    let strict = decide(
        &h,
        papers_request(
            &schema,
            &head,
            advisory_policy(&h, Some(1_000_000)),
            version,
        ),
    )
    .await
    .unwrap();
    assert!(admitted(&strict.records.as_slice()[0]).is_empty());

    // A stale snapshot: the graph moved after the papers were staged.
    let stale = decide(
        &h,
        papers_request(&schema, &head, advisory_policy(&h, None), version - 1),
    )
    .await
    .unwrap();
    assert_eq!(
        refusal_code(&stale.records.as_slice()[0]),
        "PAPER_SNAPSHOT_STALE"
    );

    // A repeated content digest is refused as a duplicate.
    {
        let guard = h.state.read().await;
        let staged = guard.registry.get(STAGED).unwrap().core.clone();
        drop(guard);
        paper(
            &staged,
            "paper-copy",
            "graph retrieval calibration of graph embeddings for retrieval",
            "sha256:both",
            "claim-1,claim-2",
            vec![1.0, 0.05, 0.0],
        );
        let moved = staged.version();
        let duplicate = decide(
            &h,
            papers_request(&schema, &head, advisory_policy(&h, None), moved),
        )
        .await
        .unwrap();
        assert_eq!(
            refusal_code(&duplicate.records.as_slice()[0]),
            "PAPER_DUPLICATE"
        );
    }
}
