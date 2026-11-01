//! Served acceptance of the statistical release (§11.2), end to end through
//! the handlers: publish a feature schema, decide without a head (abstain),
//! fit a head, refuse its publish without a passed receipt, evaluate it,
//! publish it with the receipt, and decide again (abstain on synthetic
//! calibration). Plus the negative
//! fixtures that need the served path: exploration on a security question,
//! a foreign tenant, an idempotency conflict, graph candidates.

#[cfg(feature = "redb")]
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_protocol_types::protocol::ResultPayload;
use medulla_agent_types::{
    agent_component::AgentComponentKind, agent_component::AgentComponentPublishRequest,
    agent_component::ComponentDependency,
};
use medulla_decision_types::decision::statistical::body::encode_body;
use medulla_decision_types::decision::statistical::dataset::OutcomeFidelity;
use medulla_decision_types::decision::statistical::FeatureMatrixRef;
use medulla_decision_types::decision::statistical::StatisticalDecisionRecord;
use medulla_decision_types::decision::EvidenceClass;
use medulla_decision_types::{
    decision::jobs::DatasetSource, decision::jobs::DecisionJobOutput,
    decision::jobs::DecisionJobState, decision::jobs::EvalCandidate, decision::jobs::LabelRegime,
    decision::jobs::OpeEstimatorKind, decision::jobs::OptimiserSpec, decision::jobs::RecordWindow,
};
use medulla_decision_types::{
    decision::statistical::dataset::ItemLabel, decision::statistical::dataset::LabelSource,
    decision::statistical::dataset::LabelledDataset, decision::statistical::dataset::LabelledItem,
    decision::statistical::dataset::LABELLED_DATASET_SCHEMA_VERSION,
};
use medulla_decision_types::{
    decision::statistical::features::FeatureKind,
    decision::statistical::features::FeatureSchemaBody,
    decision::statistical::features::FeatureSpec, decision::statistical::features::MissingValue,
    decision::statistical::features::FEATURE_SCHEMA_VERSION,
};
use medulla_decision_types::{
    decision::statistical::log::DecisionLogCommitted, decision::statistical::log::DecisionLogEntry,
    decision::statistical::log::DecisionLogOp,
    decision::statistical::log::DecisionOutcomeEvaluation, decision::statistical::log::EntryInputs,
    decision::statistical::log::OutcomeAggregate,
    decision::statistical::log::OutcomeAggregateRequest,
    decision::statistical::log::StoredEvaluation,
};
use medulla_decision_types::{
    decision::statistical::CandidateSource, decision::statistical::DecideRequest,
    decision::statistical::DecisionBatch, decision::statistical::QuestionKind,
    decision::statistical::QuestionSafety, decision::statistical::StatisticalOutcome,
    decision::statistical::StatisticalQuestion, decision::statistical::TypedParam,
    decision::statistical::TypedValue,
};
use medulla_decision_types::{
    decision::ColdStart, decision::DecisionEvalOp, decision::DecisionEvalRequest,
    decision::DecisionFitOp, decision::DecisionFitRequest, decision::DecisionJobRecord,
    decision::DecisionPolicyRef, decision::ExplorationBudget, decision::HeadKind,
    decision::LibraryCandidateScope, decision::QuantScaleTag, decision::QuantisedValue,
    decision::UnitRationalWire,
};
use medulla_types::contract::BoundedVec;

use medulla_server_api::auth::VerifiedRequestContext;
use medulla_persistence_records::agent_component::test_component_draft;
use medulla_persistence_records::agent_fixtures::mutation_context;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_api::state::ServerState;
use super::stat_test_support::*;

fn verified() -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_in_tenant("decider", TENANT)
}

fn rational(n: u64, d: u64) -> UnitRationalWire {
    UnitRationalWire::new(n, d).unwrap()
}

fn ordinary_exploration_policy() -> medulla_decision_types::decision::DecisionPolicy {
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.cold_start = ColdStart::Explore {
        budget: ExplorationBudget {
            fraction: rational(1, 1),
            spend_at_risk_micros: 1,
            questions: BoundedVec::new(vec!["route.tools".to_string()]).unwrap(),
        },
    };
    policy
}

fn fit_optimiser() -> OptimiserSpec {
    OptimiserSpec {
        max_iterations: 60,
        tolerance: QuantisedValue {
            scale: QuantScaleTag::Q32,
            value: 1 << 12,
        },
        seed: 0,
    }
}

fn number_feature(key: &str) -> FeatureSpec {
    FeatureSpec {
        name: key.to_string(),
        kind: FeatureKind::Number {
            key: key.to_string(),
        },
        missing: MissingValue::Abstain,
    }
}

async fn decide(h: &Harness, request: DecideRequest) -> Result<DecisionBatch, String> {
    decode(super::statistical::handle_decide(&h.state, 1, &verified(), request).await)
}

async fn decide_and_assert_abstains(
    h: &Harness,
    schema_pin: &ComponentDependency,
    head_pin: Option<ComponentDependency>,
) -> DecisionBatch {
    let batch = decide(
        h,
        request(
            schema_pin,
            head_pin,
            DecisionPolicyRef::Default,
            QuestionSafety::Ordinary,
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        batch.records.as_slice()[0].outcome,
        StatisticalOutcome::Abstained { .. }
    ));
    batch
}

async fn receipt_timeline_first_page(
    h: &Harness,
    req_id: u64,
) -> medulla_decision_types::decision::DecisionReceiptTimelinePage {
    decode(
        super::jobs::handle_decision_eval(
            &h.state,
            req_id,
            &verified(),
            DecisionEvalOp::Timeline {
                request: medulla_decision_types::decision::DecisionReceiptTimelineRequest {
                    tenant_id: TENANT.to_string(),
                    after: None,
                    limit: 1,
                },
            },
        )
        .await,
    )
    .unwrap()
}

/// Gold items that are noisy copies of the live matrix, gold = the option the
/// query matches best (row 0 after sorting by id).
fn dataset(schema_digest: &str, ids: &[String], values: &[i64], n: usize) -> LabelledDataset {
    let items = (0..n)
        .map(|i| {
            let jitter = ((i % 7) as i64 - 3) << 24;
            LabelledItem {
                item_id: format!("gold-{i:04}"),
                recorded_at_ms: 1_000,
                class_key: "route".to_string(),
                candidate_ids: BoundedVec::new(ids.to_vec()).unwrap(),
                features: BoundedVec::new(values.iter().map(|v| v + jitter).collect()).unwrap(),
                label: ItemLabel::Gold {
                    acceptable: BoundedVec::new(vec![ids[0].clone()]).unwrap(),
                    source: LabelSource::SyntheticConstruction,
                },
                audit_inclusion: None,
            }
        })
        .collect();
    LabelledDataset {
        schema_version: LABELLED_DATASET_SCHEMA_VERSION,
        feature_schema_digest: schema_digest.to_string(),
        feature_names: BoundedVec::new(vec!["text".to_string(), "coverage".to_string()]).unwrap(),
        scale: QuantScaleTag::Q32,
        items: BoundedVec::new(items).unwrap(),
        synthetic: true,
    }
}

fn window() -> RecordWindow {
    RecordWindow {
        from_ms: 0,
        to_ms: u64::MAX,
    }
}

fn succeeded(job: &DecisionJobRecord) -> &DecisionJobOutput {
    match &job.state {
        DecisionJobState::Succeeded { output } => output.as_ref(),
        other => panic!("job did not succeed: {other:?}"),
    }
}

/// A caller can mark its inline labels Human and non-synthetic, but the
/// server has no independent gold-set authority for those bytes. Even with
/// disjoint fit/eval items, this path cannot publish a threshold claim.
async fn inline_human_labels_cannot_publish_threshold(
    h: &Harness,
    schema_pin: &ComponentDependency,
    heldout: LabelledDataset,
) {
    let mut training = heldout.clone();
    training.items = BoundedVec::new(
        training
            .items
            .as_slice()
            .iter()
            .cloned()
            .map(|mut item| {
                item.item_id = format!("train-{}", item.item_id);
                item
            })
            .collect(),
    )
    .unwrap();
    assert!(training
        .items
        .iter()
        .zip(heldout.items.iter())
        .all(|(train, eval)| train.item_id != eval.item_id));
    let fit_gold = super::stat_jobs::dataset_digest(&training).unwrap();
    let fit: DecisionJobRecord = decode(
        super::jobs::handle_decision_fit(
            &h.state,
            57,
            &verified(),
            DecisionFitOp::Submit {
                request: Box::new(DecisionFitRequest {
                    tenant_id: TENANT.to_string(),
                    idempotency_key: "fit-real-calibration".to_string(),
                    head_kind: HeadKind::ListwiseLogistic,
                    feature_schema: schema_pin.clone(),
                    policy: DecisionPolicyRef::Default,
                    label_regime: LabelRegime::FullLabel {
                        gold_set_digest: fit_gold,
                    },
                    window: window(),
                    optimiser: fit_optimiser(),
                    source: DatasetSource::Inline {
                        dataset: Box::new(training),
                    },
                }),
            },
        )
        .await,
    )
    .unwrap();
    let DecisionJobOutput::Fit {
        draft_sha256,
        draft_length,
        draft,
        ..
    } = succeeded(&fit)
    else {
        panic!("inline fit output")
    };
    assert!(draft.synthetic);
    assert!(draft.calibration.as_ref().is_some_and(|cal| cal.synthetic));
    let heldout_gold = super::stat_jobs::dataset_digest(&heldout).unwrap();
    let evaluated: DecisionJobRecord = decode(
        super::jobs::handle_decision_eval(
            &h.state,
            58,
            &verified(),
            DecisionEvalOp::Submit {
                request: Box::new(DecisionEvalRequest {
                    tenant_id: TENANT.to_string(),
                    idempotency_key: "eval-real-heldout".to_string(),
                    candidate: EvalCandidate::DraftArtifact {
                        sha256: draft_sha256.clone(),
                        length: *draft_length,
                    },
                    policy: DecisionPolicyRef::Default,
                    estimators: BoundedVec::new(vec![OpeEstimatorKind::Ips]).unwrap(),
                    gold_set_digest: Some(heldout_gold),
                    window: window(),
                    source: DatasetSource::Inline {
                        dataset: Box::new(heldout),
                    },
                    mode: medulla_decision_types::decision::EvalMode::OffPolicy,
                }),
            },
        )
        .await,
    )
    .unwrap();
    let DecisionJobOutput::Eval { receipt } = succeeded(&evaluated) else {
        panic!("inline evaluation output")
    };
    assert!(receipt.synthetic);
    assert!(receipt.calibration.as_ref().is_some_and(|cal| {
        cal.synthetic
            && cal.method == medulla_decision_types::decision::CalibrationMethod::Conformal
    }));
    let timeline: medulla_decision_types::decision::DecisionReceiptTimelinePage = decode(
        super::jobs::handle_decision_eval(
            &h.state,
            59,
            &verified(),
            DecisionEvalOp::Timeline {
                request: medulla_decision_types::decision::DecisionReceiptTimelineRequest {
                    tenant_id: TENANT.to_string(),
                    after: None,
                    limit: 10,
                },
            },
        )
        .await,
    )
    .unwrap();
    assert!(timeline.entries.is_empty());
    assert_eq!(timeline.next_after, None);
    assert!(h
        .store
        .decision_artifact(
            TENANT,
            &medulla_persistence_records::decision_jobs::receipt_threshold_key(
                &receipt.receipt_digest,
            ),
        )
        .unwrap()
        .is_none());
}

/// What the route fixture publishes and reads back.
struct RouteFixture {
    schema_pin: ComponentDependency,
    schema_digest: String,
    ids: Vec<String>,
    values: Vec<i64>,
}

/// Publish three tools and the route feature schema, then decide without a
/// head under the default (deterministic-only) policy: that abstains, records
/// the matrix exactly, and returns it for fixtures shaped like it.
async fn route_fixture(h: &Harness) -> RouteFixture {
    h.publish(
        "tool-a-search",
        AgentComponentKind::Tool,
        "web search engine for pages",
        None::<&()>,
        None,
    )
    .unwrap();
    h.publish(
        "tool-b-files",
        AgentComponentKind::Tool,
        "write files to disk",
        None::<&()>,
        None,
    )
    .unwrap();
    h.publish(
        "tool-c-mail",
        AgentComponentKind::Tool,
        "send an email message",
        None::<&()>,
        None,
    )
    .unwrap();
    let (schema_pin, schema_digest) = publish_route_schema(h);

    // No head under the default (deterministic-only) policy: abstain, with the
    // matrix recorded exactly and the record digest reproducible.
    let batch = decide_and_assert_abstains(h, &schema_pin, None).await;
    let record = &batch.records.as_slice()[0];
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(record),
        record.record_digest
    );
    let medulla_decision_types::decision::statistical::FeatureMatrixRef::Inline {
        candidate_ids,
        values,
        ..
    } = &record.inputs.feature_matrix
    else {
        panic!("inline matrix")
    };
    let ids: Vec<String> = candidate_ids.iter().cloned().collect();
    assert_eq!(ids[0], "tool-a-search");

    RouteFixture {
        schema_pin,
        schema_digest,
        ids,
        values: values.as_slice().to_vec(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R122
#[tokio::test]
async fn fit_evaluate_publish_and_decide_end_to_end() {
    let h = Harness::new().await;
    let RouteFixture {
        schema_pin,
        schema_digest,
        ids,
        values,
    } = route_fixture(&h).await;
    // Fit on 400 gold items shaped like the live matrix.
    let data = dataset(&schema_digest, &ids, &values, 400);
    let gold = super::stat_jobs::dataset_digest(&data).unwrap();
    let fit_request = DecisionFitRequest {
        tenant_id: TENANT.to_string(),
        idempotency_key: "fit-1".to_string(),
        head_kind: HeadKind::ListwiseLogistic,
        feature_schema: schema_pin.clone(),
        policy: DecisionPolicyRef::Default,
        label_regime: LabelRegime::FullLabel {
            gold_set_digest: gold.clone(),
        },
        window: window(),
        optimiser: fit_optimiser(),
        source: DatasetSource::Inline {
            dataset: Box::new(data.clone()),
        },
    };
    let fit_op = || DecisionFitOp::Submit {
        request: Box::new(fit_request.clone()),
    };
    let job: DecisionJobRecord =
        decode(super::jobs::handle_decision_fit(&h.state, 2, &verified(), fit_op()).await).unwrap();
    let DecisionJobOutput::Fit {
        draft,
        draft_sha256,
        draft_length,
        ..
    } = succeeded(&job).clone()
    else {
        panic!("fit output")
    };
    let replay: DecisionJobRecord =
        decode(super::jobs::handle_decision_fit(&h.state, 3, &verified(), fit_op()).await).unwrap();
    assert_eq!(replay, job, "a retried submit is a replay");
    let mut conflicting = fit_request.clone();
    conflicting.optimiser.max_iterations = 10;
    let conflict = decode::<DecisionJobRecord>(
        super::jobs::handle_decision_fit(
            &h.state,
            4,
            &verified(),
            DecisionFitOp::Submit {
                request: Box::new(conflicting),
            },
        )
        .await,
    );
    assert!(conflict.unwrap_err().starts_with("IDEMPOTENCY_CONFLICT"));

    // Promotion needs a passed receipt naming exactly these bytes (EH-040).
    let refused = h.publish(
        "head-route",
        AgentComponentKind::DecisionHead,
        "route head",
        Some(draft.as_ref()),
        Some(format!("sha256:{}", "0".repeat(64))),
    );
    assert!(refused
        .unwrap_err()
        .starts_with("EVALUATION_RECEIPT_MISMATCH"));

    let eval = DecisionEvalRequest {
        tenant_id: TENANT.to_string(),
        idempotency_key: "eval-1".to_string(),
        candidate: EvalCandidate::DraftArtifact {
            sha256: draft_sha256,
            length: draft_length,
        },
        policy: DecisionPolicyRef::Default,
        estimators: BoundedVec::new(vec![OpeEstimatorKind::Ips]).unwrap(),
        gold_set_digest: Some(gold),
        window: window(),
        source: DatasetSource::Inline {
            dataset: Box::new(data),
        },
        mode: medulla_decision_types::decision::EvalMode::OffPolicy,
    };
    let job: DecisionJobRecord = decode(
        super::jobs::handle_decision_eval(
            &h.state,
            5,
            &verified(),
            DecisionEvalOp::Submit {
                request: Box::new(eval),
            },
        )
        .await,
    )
    .unwrap();
    let DecisionJobOutput::Eval { receipt } = succeeded(&job).clone() else {
        panic!("eval output")
    };
    let receipt = *receipt;
    assert!(
        receipt.passed,
        "gates: {:?}",
        receipt.failed_gates.as_slice()
    );
    let read: Option<medulla_decision_types::decision::DecisionEvalReceipt> = decode(
        super::jobs::handle_decision_eval(
            &h.state,
            51,
            &verified(),
            DecisionEvalOp::Receipt {
                request: medulla_decision_types::decision::DecisionReceiptGetRequest {
                    tenant_id: TENANT.to_string(),
                    receipt_digest: receipt.receipt_digest.clone(),
                },
            },
        )
        .await,
    )
    .unwrap();
    assert_eq!(read.as_ref(), Some(&receipt));
    let page: medulla_decision_types::decision::DecisionReceiptPage = decode(
        super::jobs::handle_decision_eval(
            &h.state,
            52,
            &verified(),
            DecisionEvalOp::Receipts {
                request: medulla_decision_types::decision::DecisionReceiptListRequest {
                    tenant_id: TENANT.to_string(),
                    after: None,
                    limit: 1,
                },
            },
        )
        .await,
    )
    .unwrap();
    assert_eq!(page.receipts.as_slice(), std::slice::from_ref(&receipt));
    assert_eq!(page.next_after, None);
    let timeline = receipt_timeline_first_page(&h, 54).await;
    // This fixture labels by synthetic construction, so it must not enter
    // the real-world calibrated timeline even though it has metrics.
    assert!(receipt.synthetic);
    assert!(timeline.entries.is_empty());
    assert_eq!(timeline.next_after, None);
    // A caller-authored Human label and synthetic=false flag do not prove
    // independent gold-set provenance or unlock the real-world timeline.
    let mut human_data = dataset(&schema_digest, &ids, &values, 400);
    human_data.synthetic = false;
    human_data.items = BoundedVec::new(
        human_data
            .items
            .as_slice()
            .iter()
            .cloned()
            .map(|mut item| {
                if let ItemLabel::Gold { source, .. } = &mut item.label {
                    *source = LabelSource::Human;
                }
                item
            })
            .collect(),
    )
    .unwrap();
    let human_gold = super::stat_jobs::dataset_digest(&human_data).unwrap();
    let heldout_data = human_data.clone();
    let human_eval = DecisionEvalRequest {
        tenant_id: TENANT.to_string(),
        idempotency_key: "eval-human-1".to_string(),
        candidate: EvalCandidate::DraftArtifact {
            sha256: receipt.head_digest.clone(),
            length: draft_length,
        },
        policy: DecisionPolicyRef::Default,
        estimators: BoundedVec::new(vec![OpeEstimatorKind::Ips]).unwrap(),
        gold_set_digest: Some(human_gold),
        window: window(),
        source: DatasetSource::Inline {
            dataset: Box::new(human_data),
        },
        mode: medulla_decision_types::decision::EvalMode::OffPolicy,
    };
    let human_job: DecisionJobRecord = decode(
        super::jobs::handle_decision_eval(
            &h.state,
            55,
            &verified(),
            DecisionEvalOp::Submit {
                request: Box::new(human_eval),
            },
        )
        .await,
    )
    .unwrap();
    let DecisionJobOutput::Eval {
        receipt: human_receipt,
    } = succeeded(&human_job).clone()
    else {
        panic!("human eval output")
    };
    assert!(human_receipt.synthetic);
    assert!(human_receipt.metrics.is_some());
    let timeline = receipt_timeline_first_page(&h, 56).await;
    assert!(timeline.entries.is_empty());
    assert_eq!(timeline.next_after, None);
    // The fitted calibration and caller-authored evaluation labels both lack
    // independently verified provenance.
    assert!(human_receipt
        .calibration
        .as_ref()
        .is_some_and(|c| c.synthetic));
    assert!(super::stat_jobs::threshold_assessment(
        &human_receipt,
        &super::stat_support::default_statistical_policy(),
    )
    .is_none());
    assert!(h
        .store
        .decision_artifact(
            TENANT,
            &medulla_persistence_records::decision_jobs::receipt_threshold_key(
                &human_receipt.receipt_digest,
            ),
        )
        .unwrap()
        .is_none());
    inline_human_labels_cannot_publish_threshold(&h, &schema_pin, heldout_data).await;
    let foreign = decode::<Option<medulla_decision_types::decision::DecisionEvalReceipt>>(
        super::jobs::handle_decision_eval(
            &h.state,
            53,
            &verified(),
            DecisionEvalOp::Receipt {
                request: medulla_decision_types::decision::DecisionReceiptGetRequest {
                    tenant_id: "foreign".into(),
                    receipt_digest: receipt.receipt_digest.clone(),
                },
            },
        )
        .await,
    );
    assert!(foreign.unwrap_err().starts_with("ACCESS_DENIED"));
    let head_pin = h
        .publish(
            "head-route",
            AgentComponentKind::DecisionHead,
            "route head",
            Some(draft.as_ref()),
            Some(receipt.receipt_digest.clone()),
        )
        .unwrap();

    // An inline-fitted head is synthetic even when its caller claims Human
    // labels. Its apparent calibration cannot authorize an ordinary Act.
    let batch = decide_and_assert_abstains(&h, &schema_pin, Some(head_pin.clone())).await;
    let record = &batch.records.as_slice()[0];
    assert!(record.calibration.is_none());
    assert!(record.audit.is_none());
    assert!(record.logging_propensities.is_empty());
    assert!(
        record.synthetic_evidence,
        "a head fitted on synthetic data says so"
    );

    // The log and retention tests still need an executed decision. Explicit
    // ordinary-question exploration supplies one without claiming risk-bound
    // Act from the synthetic calibration.
    let policy = ordinary_exploration_policy();
    let policy_pin = h.publish_policy("policy-inline-explore", &policy);
    let explored = decide(
        &h,
        request(
            &schema_pin,
            Some(head_pin),
            DecisionPolicyRef::Pinned {
                component: policy_pin,
            },
            QuestionSafety::Ordinary,
        ),
    )
    .await
    .unwrap();
    let record = &explored.records.as_slice()[0];
    assert!(matches!(
        record.outcome,
        StatisticalOutcome::Explored { .. }
    ));
    assert_eq!(record.logging_propensities.len(), 3);

    decision_log_round_trip(&h, record.clone()).await;
    // Library inputs remain inline and continue to train.
    fit_from_the_engine_log(&h, &schema_pin, record).await;
    log_tests::mixed_origin_aggregates_exclude_synthetic(&h, &schema_pin, record).await;
}

// spec: MEDULLA-DECISION-ENGINE-R020
#[tokio::test]
async fn exploration_on_a_security_question_is_refused_by_the_served_path() {
    let h = Harness::new().await;
    h.publish(
        "tool-a",
        AgentComponentKind::Tool,
        "web search",
        None::<&()>,
        None,
    )
    .unwrap();
    let schema_pin = h
        .publish(
            "schema",
            AgentComponentKind::FeatureSchema,
            "features",
            Some(&schema()),
            None,
        )
        .unwrap();
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.cold_start = ColdStart::Explore {
        budget: ExplorationBudget {
            fraction: rational(1, 2),
            spend_at_risk_micros: 1,
            questions: BoundedVec::new(vec!["route.tools".to_string()]).unwrap(),
        },
    };
    let policy_pin = h.publish_policy("policy-explore", &policy);
    let pinned = DecisionPolicyRef::Pinned {
        component: policy_pin,
    };
    let error = decide(
        &h,
        request(&schema_pin, None, pinned.clone(), QuestionSafety::Security),
    )
    .await
    .unwrap_err();
    assert!(error.starts_with("EXPLORATION_FORBIDDEN"), "{error}");
    let batch = decide(
        &h,
        request(&schema_pin, None, pinned, QuestionSafety::Ordinary),
    )
    .await
    .unwrap();
    let exploration = batch.records.as_slice()[0]
        .inputs
        .exploration
        .as_ref()
        .expect("the draw is committed");
    assert!(
        exploration.seed_commitment.starts_with("sha256:") && exploration.revealed_seed.is_none()
    );
}

#[tokio::test]
async fn foreign_tenants_and_graph_candidates_are_refused() {
    let h = Harness::new().await;
    let schema_pin = h
        .publish(
            "schema",
            AgentComponentKind::FeatureSchema,
            "features",
            Some(&schema()),
            None,
        )
        .unwrap();
    let mut foreign = request(
        &schema_pin,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    foreign.tenant_id = "tenant-other".to_string();
    assert!(decide(&h, foreign)
        .await
        .unwrap_err()
        .starts_with("ACCESS_DENIED"));
    let mut graph = request(
        &schema_pin,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    graph.candidates = CandidateSource::Graph {
        graph: "kg".to_string(),
        plan: Box::new(medulla_types::wire::Plan::new(Vec::new())),
    };
    assert!(decide(&h, graph)
        .await
        .unwrap_err()
        .starts_with("CANDIDATE_PLAN_REFUSED"));
    let mut wrong_pin = request(
        &schema_pin,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    wrong_pin.feature_schema.definition_digest = format!("sha256:{}", "f".repeat(64));
    assert!(decide(&h, wrong_pin)
        .await
        .unwrap_err()
        .starts_with("COMPONENT_PIN_MISMATCH"));
}

/// The executor's visibility step runs before the candidate source is even
/// read: a request naming another tenant is refused `ACCESS_DENIED` whether
/// its `CandidateSource` is the agent library or an RLS-filtered graph plan --
/// a foreign tenant never reaches the graph ACL that would otherwise refuse
/// the plan itself.
// spec: MEDULLA-DECISION-ENGINE-R057
#[tokio::test]
async fn graph_sourced_candidates_from_a_foreign_tenant_are_refused() {
    let h = Harness::new().await;
    let schema_pin = h
        .publish(
            "schema-foreign-graph",
            AgentComponentKind::FeatureSchema,
            "features",
            Some(&schema()),
            None,
        )
        .unwrap();
    let mut foreign_graph = request(
        &schema_pin,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    foreign_graph.tenant_id = "tenant-other".to_string();
    foreign_graph.candidates = CandidateSource::Graph {
        graph: "kg".to_string(),
        plan: Box::new(medulla_types::wire::Plan::new(Vec::new())),
    };
    let error = decide(&h, foreign_graph).await.unwrap_err();
    assert!(
        error.starts_with("ACCESS_DENIED"),
        "a foreign tenant's graph-sourced request must be refused before the \
         plan is ever read: {error}"
    );
}

/// EH-059: graph candidates are the rows a SELECT-only plan returns over the
/// caller's RLS-filtered snapshot; a ranking stage is refused and an
/// unregistered caller is refused by the graph ACL before any row is read.
#[cfg(feature = "query")]
#[tokio::test]
async fn graph_candidates_are_read_through_acl_rls_and_a_select_only_plan() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    {
        let mut guard = h.state.write().await;
        guard
            .registry
            .create_graph("kg-decide", medulla_protocol_types::protocol::GraphType::Team, None)
            .unwrap();
        let core = guard.registry.get("kg-decide").unwrap().core.clone();
        for (id, score, summary) in [
            ("n-a", 0.9, "web search engine"),
            ("n-b", 0.1, "file writer"),
        ] {
            let props = serde_json::json!({"type": "Tool", "score": score, "summary": summary});
            core.add_node(id.to_string(), rmp_serde::to_vec_named(&props).unwrap());
        }
    }
    let body = FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![
            number_feature("score"),
            medulla_decision_types::decision_fixtures::decision::summary_text_feature(),
        ])
        .unwrap(),
    };
    let schema_pin = h
        .publish(
            "schema-graph",
            AgentComponentKind::FeatureSchema,
            "graph features",
            Some(&body),
            None,
        )
        .unwrap();
    let graph_request = |plan: medulla_types::wire::Plan| {
        let mut request = request(
            &schema_pin,
            None,
            DecisionPolicyRef::Default,
            QuestionSafety::Ordinary,
        );
        request.candidates = CandidateSource::Graph {
            graph: "kg-decide".to_string(),
            plan: Box::new(plan),
        };
        request
    };
    let select = medulla_types::wire::Plan::new(vec![medulla_types::wire::Op::Scan {
        label: "Tool".to_string(),
    }]);
    let batch = decide(&h, graph_request(select.clone())).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let FeatureMatrixRef::Inline {
        candidate_ids,
        values,
        ..
    } = &record.inputs.feature_matrix
    else {
        panic!("inline matrix")
    };
    assert_eq!(
        candidate_ids.as_slice(),
        ["n-a".to_string(), "n-b".to_string()]
    );
    assert_eq!(values.len(), 4, "two visible rows, two features");
    assert!(matches!(
        record.candidate_source,
        medulla_decision_types::decision::CandidateSourceRecord::Graph { .. }
    ));

    let ranked = medulla_types::wire::Plan::new(vec![
        medulla_types::wire::Op::Scan {
            label: "Tool".to_string(),
        },
        medulla_types::wire::Op::RankText {
            query: "web".to_string(),
        },
    ]);
    let refused = decide(&h, graph_request(ranked)).await.unwrap_err();
    assert!(refused.starts_with("CANDIDATE_PLAN_REFUSED"), "{refused}");

    let stranger = VerifiedRequestContext::verified_for_test_in_tenant("stranger", TENANT);
    let denied = decode::<DecisionBatch>(
        super::statistical::handle_decide(&h.state, 3, &stranger, graph_request(select)).await,
    );
    assert!(denied.unwrap_err().starts_with("ACCESS_DENIED"));
}

/// EH-059/EH-060: a graph-sourced record is logged under `RecordVisibility::
/// Principal` -- the same per-principal visibility a caller-`Declared` record
/// gets, never the whole-tenant visibility an `AgentLibrary` record gets.
/// Committing it from the graph candidates above, the committing principal's
/// own `DecisionLog.get` sees it; a different reader in the SAME tenant does
/// not -- proving the row's RLS-routed read is finer than tenant scope.
#[cfg(feature = "query")]
// spec: MEDULLA-DECISION-ENGINE-R058
#[tokio::test]
async fn graph_sourced_records_are_principal_scoped_not_tenant_scoped_on_log_read() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let record = graph_log_record(&h).await;
    assert!(matches!(
        record.candidate_source,
        medulla_decision_types::decision::CandidateSourceRecord::Graph { .. }
    ));
    let commit = DecisionLogOp::Commit {
        record: Box::new(record.clone()),
        evaluator: None,
    };
    let committed: DecisionLogCommitted = decode(log_op(&h, "decider", commit).await).unwrap();

    let get = |record_id: String| DecisionLogOp::Get {
        tenant_id: TENANT.to_string(),
        record_id,
    };
    let mine: Option<DecisionLogEntry> =
        decode(log_op(&h, "decider", get(committed.record_id.clone())).await).unwrap();
    assert!(
        mine.is_some(),
        "the committing principal sees its own record"
    );

    let theirs: Option<DecisionLogEntry> =
        decode(log_op(&h, "another-reader", get(committed.record_id.clone())).await).unwrap();
    assert!(
        theirs.is_none(),
        "a graph-sourced record is visible to its committing principal only, \
         not to another reader in the same tenant"
    );
    #[cfg(feature = "blob")]
    {
        retention_compacts_and_verifies(&h, &record).await;
        retention_retires(&h, &record).await;
    }
}

#[cfg(feature = "query")]
async fn graph_log_record(h: &Harness) -> StatisticalDecisionRecord {
    {
        let mut guard = h.state.write().await;
        guard
            .registry
            .create_graph("kg-decide-log", medulla_protocol_types::protocol::GraphType::Team, None)
            .unwrap();
        let core = guard.registry.get("kg-decide-log").unwrap().core.clone();
        let props = serde_json::json!({"type": "Tool", "score": 0.9, "summary": "web search"});
        core.add_node("n-a".to_string(), rmp_serde::to_vec_named(&props).unwrap());
    }
    let body = FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![
            number_feature("score"),
            medulla_decision_types::decision_fixtures::decision::summary_text_feature(),
        ])
        .unwrap(),
    };
    let schema_pin = h
        .publish(
            "schema-graph-log",
            AgentComponentKind::FeatureSchema,
            "graph features",
            Some(&body),
            None,
        )
        .unwrap();
    let mut graph_request = request(
        &schema_pin,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    graph_request.candidates = CandidateSource::Graph {
        graph: "kg-decide-log".to_string(),
        plan: Box::new(medulla_types::wire::Plan::new(vec![
            medulla_types::wire::Op::Scan {
                label: "Tool".to_string(),
            },
        ])),
    };
    let batch = decide(h, graph_request).await.unwrap();
    batch.records.as_slice()[0].clone()
}

mod retention_tests;

mod log_tests;
use log_tests::{decision_log_round_trip, fit_from_the_engine_log, log_op};
#[cfg(all(feature = "blob", feature = "query"))]
use log_tests::{retention_compacts_and_verifies, retention_retires};

mod classes_tests;

mod tool_subset_tests;


mod consumer_tests;

#[cfg(feature = "query")]
mod retrieval_tests;

#[cfg(feature = "query")]
mod learning_tests;

#[cfg(feature = "query")]
mod evaluator_tests;
// EH-523: the served per-component slate split.
mod attribution_tests;
// EH-525: the learned reputation relation and SOURCE RELIABILITY.
#[cfg(feature = "query")]
mod reputation_tests;

mod nl_tests;

mod pretool_risk_tests;

mod connector_triage_tests;

mod connector_toolchoice_tests;

mod connector_writeback_tests;

mod scorer_tests;
// EH-528: replay evaluation (its validation kernels are finance's).
#[cfg(feature = "finance")]
mod replay_tests;

mod task_plan_tests;
