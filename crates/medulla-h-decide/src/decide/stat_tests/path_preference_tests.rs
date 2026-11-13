//! Routing-path preference served through `Decide` (MEDULLA-DECISION-ENGINE-R159.3):
//! outcomes enter through `DecisionLog` `Evaluate`, and `Decide` ranks the declared
//! paths by their recent success rate, skips a path whose latest outcomes all
//! failed and never counts another tenant's outcomes.

use super::*;
use medulla_agent_types::agent_component::AgentComponentKind;
use medulla_decision_types::decision::path_success::RoutingPath;
use medulla_decision_types::decision::statistical::declared::{DeclaredNumber, DeclaredOption};
use medulla_decision_types::decision::statistical::log::{
    DecisionLogCommitted, DecisionLogOp, DecisionOutcomeEvaluation, NamedEvaluator,
};
use medulla_decision_types::decision::{DecisionPolicy, ResolutionKind};
use medulla_types::contract::Digest256;

const OTHER: &str = "tenant-other";
const QUESTION: &str = "route.tools";

fn path_id(method: &str) -> String {
    RoutingPath {
        connector_id: "conn:search".into(),
        method_id: method.into(),
        parameter_digest: Digest256::sha256(b"params"),
    }
    .path_id()
}

fn route_option(id: &str) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::new(vec![DeclaredNumber {
            key: "score".to_string(),
            q32: 0,
        }])
        .unwrap(),
        texts: BoundedVec::default(),
    }
}

fn exploring_policy() -> DecisionPolicy {
    ordinary_exploration_policy()
}

/// The schema and the exploring policy of `tenant`, as pins.
fn pins(h: &Harness, tenant: &str) -> (ComponentDependency, DecisionPolicyRef) {
    let body = FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![number_feature("score")]).unwrap(),
    };
    let encoded = encode_body(&body).unwrap();
    let mut schema = test_component_draft(tenant, &format!("schema-paths-{tenant}"));
    schema.kind = AgentComponentKind::FeatureSchema;
    schema.summary = "path features".to_string();
    schema.content_digest = encoded.content_digest;
    schema.attributes = encoded.attributes;
    let schema = h.commit_in(tenant, schema, None).unwrap();

    let policy = exploring_policy();
    let mut draft = test_component_draft(tenant, &format!("policy-paths-{tenant}"));
    draft.kind = AgentComponentKind::DecisionPolicy;
    draft.content_digest = medulla_decision_types::decision::digest::policy_digest(&policy);
    draft.attributes.insert(
        medulla_decision_types::decision::policy::DECISION_POLICY_ATTRIBUTE.to_string(),
        serde_json::to_string(&policy).unwrap(),
    );
    let component = h.commit_in(tenant, draft, None).unwrap();
    (schema, DecisionPolicyRef::Pinned { component })
}

fn context(who: &str, tenant: &str) -> VerifiedRequestContext {
    VerifiedRequestContext::verified_for_test_in_tenant(who, tenant)
}

/// One decision on `path` by `decider`, logged with a grant naming `evaluator`,
/// then one independent evaluation per outcome (oldest first).
async fn seed(
    h: &Harness,
    tenant: &str,
    pins: &(ComponentDependency, DecisionPolicyRef),
    path: &str,
    outcomes: &[bool],
) {
    let request = DecideRequest {
        tenant_id: tenant.to_string(),
        question: StatisticalQuestion {
            question_id: QUESTION.to_string(),
            kind: QuestionKind::Route,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![route_option(path)]).unwrap(),
        },
        feature_schema: pins.0.clone(),
        head: None,
        policy: pins.1.clone(),
        params: BoundedVec::default(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    };
    let decider = context("decider", tenant);
    let batch: DecisionBatch =
        decode(super::super::statistical::handle_decide(&h.state, 1, &decider, request).await)
            .unwrap();
    let record = batch.records.as_slice()[0].clone();
    assert!(matches!(
        record.outcome,
        StatisticalOutcome::Explored { .. }
    ));
    let evaluator = context("evaluator", tenant);
    let now = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let _: DecisionLogCommitted = decode(
        super::super::log::handle_decision_log(
            &h.state,
            9,
            &decider,
            DecisionLogOp::Commit {
                record: Box::new(record.clone()),
                evaluator: Some(NamedEvaluator {
                    principal: Some(evaluator.principal_persistence_id()),
                    role: None,
                    expires_at_ms: now + 3_600_000,
                }),
            },
        )
        .await,
    )
    .unwrap();
    for (i, success) in outcomes.iter().enumerate() {
        let _: medulla_decision_types::decision::statistical::log::StoredEvaluation = decode(
            super::super::log::handle_decision_log(
                &h.state,
                10,
                &evaluator,
                DecisionLogOp::Evaluate {
                    tenant_id: tenant.to_string(),
                    evaluation: DecisionOutcomeEvaluation {
                        record_id: record.record_id.clone(),
                        evaluation_id: format!("e-{i:03}"),
                        class: EvidenceClass::Observation,
                        selected_agent: "selected-agent".to_string(),
                        lease_holder: "lease-holder".to_string(),
                        fidelity: OutcomeFidelity::ToolCalls,
                        success: Some(*success),
                    },
                },
            )
            .await,
        )
        .unwrap();
    }
}

/// `successes` successes then failures out of ten, or any exact sequence.
fn run(successes: usize, failures: usize) -> Vec<bool> {
    std::iter::repeat_n(true, successes)
        .chain(std::iter::repeat_n(false, failures))
        .collect()
}

async fn prefer(
    h: &Harness,
    pins: &(ComponentDependency, DecisionPolicyRef),
    paths: &[&str],
) -> StatisticalDecisionRecord {
    let mut sorted: Vec<&str> = paths.to_vec();
    sorted.sort_unstable();
    let request = DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "path.preference".to_string(),
            kind: QuestionKind::PathPreference,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(sorted.iter().map(|p| route_option(p)).collect()).unwrap(),
        },
        feature_schema: pins.0.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::default(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    };
    decide(h, request).await.unwrap().records.as_slice()[0].clone()
}

fn ranked(record: &StatisticalDecisionRecord) -> Vec<(String, i64)> {
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected an advisory answer, got {:?}", record.outcome);
    };
    scores
        .iter()
        .map(|s| (s.option_id.clone(), s.score.value))
        .collect()
}

fn q32_of(micros: i64) -> i64 {
    (i128::from(micros << 32) / 1_000_000) as i64
}

/// Verifies MEDULLA-DECISION-ENGINE-R159.3: the higher success rate is preferred,
/// a path whose latest three outcomes failed is skipped, and a path below the
/// minimum attempts is not ranked.
// spec: MEDULLA-DECISION-ENGINE-R159.3
#[tokio::test]
async fn served_decide_prefers_the_higher_success_path() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    let (a, b, c, e, thin) = (
        path_id("a"),
        path_id("b"),
        path_id("c"),
        path_id("e"),
        path_id("thin"),
    );
    // A: 9 of 10, its one failure in the middle. B: 3 of 10, the latest three failed.
    let mut a_outcomes = run(4, 1);
    a_outcomes.extend(run(5, 0));
    seed(&h, TENANT, &pins, &a, &a_outcomes).await;
    seed(&h, TENANT, &pins, &b, &run(3, 7)).await;
    // C: 5 of 10 with a late success. E: 3 of 10, its three successes the latest.
    let mut c_outcomes = run(0, 5);
    c_outcomes.extend(run(5, 0));
    seed(&h, TENANT, &pins, &c, &c_outcomes).await;
    let mut e_outcomes = run(0, 7);
    e_outcomes.extend(run(3, 0));
    seed(&h, TENANT, &pins, &e, &e_outcomes).await;
    // Nine attempts: below path.min_samples.
    seed(&h, TENANT, &pins, &thin, &run(9, 0)).await;

    let record = prefer(&h, &pins, &[&a, &b, &c, &e, &thin]).await;
    assert_eq!(record.question.kind, QuestionKind::PathPreference);
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(
        ranked(&record),
        vec![
            (a.clone(), q32_of(900_000)),
            (c.clone(), q32_of(500_000)),
            (e.clone(), q32_of(300_000)),
        ],
        "B is skipped after three failures, the thin path is not eligible"
    );

    // An undeclared path is never ranked, and Decide wrote no outcome.
    let record = prefer(&h, &pins, &[&a, &path_id("never-used")]).await;
    assert_eq!(ranked(&record), vec![(a.clone(), q32_of(900_000))]);

    // A non-path option id is refused.
    let mut bad = DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "path.preference".to_string(),
            kind: QuestionKind::PathPreference,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![route_option("not-a-path")]).unwrap(),
        },
        feature_schema: pins.0.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::default(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    };
    assert!(decide(&h, bad.clone())
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));
    bad.tenant_id = OTHER.to_string();
    let other = context("decider", OTHER);
    let error = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(&h.state, 1, &other, bad.clone()).await,
    )
    .unwrap_err();
    assert!(error.starts_with("COMPONENT_PIN_MISMATCH"), "{error}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R159.3: another tenant's outcomes never count.
// spec: MEDULLA-DECISION-ENGINE-R159.3
#[tokio::test]
async fn other_tenant_outcomes_never_count() {
    let h = Harness::new().await;
    let mine = pins(&h, TENANT);
    let theirs = pins(&h, OTHER);
    let (a, f) = (path_id("a"), path_id("f"));
    let mut a_outcomes = run(4, 1);
    a_outcomes.extend(run(5, 0));
    seed(&h, TENANT, &mine, &a, &a_outcomes).await;
    // The other tenant measures the same path A as always failing, and a path F
    // as always succeeding.
    seed(&h, OTHER, &theirs, &a, &run(0, 10)).await;
    seed(&h, OTHER, &theirs, &f, &run(10, 0)).await;

    let record = prefer(&h, &mine, &[&a, &f]).await;
    assert_eq!(
        ranked(&record),
        vec![(a, q32_of(900_000))],
        "A keeps this tenant's 9 of 10; F is unmeasured here"
    );
}
