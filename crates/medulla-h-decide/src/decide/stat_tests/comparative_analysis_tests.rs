//! MEDULLA-DECISION-ENGINE-R139.3: a served `comparative_analysis_plan`
//! question returns a budgeted plan over the caller's visible candidates.
//! The engine never invokes a model, and another tenant's candidates never
//! enter a plan.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::super::candidates::library_candidates;
use super::*;
use medulla_decision_types::decision::comparative_analysis::{
    ComparativeAnalysisRequest, ComparativeAnalysisStage, ComparativeCandidate,
};
use medulla_decision_types::decision::ResolutionKind;
use medulla_program::{ModelCallRequest, ModelCallResponse, ModelTransport, ModelTransportError};

/// A model transport that counts every call it receives.
struct RecordingTransport(AtomicUsize);

impl ModelTransport for RecordingTransport {
    fn execute<'a>(
        &'a self,
        _request: &'a ModelCallRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ModelCallResponse, ModelTransportError>> + Send + 'a>>
    {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(ModelTransportError::Unavailable) })
    }
}

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn tool_scope() -> LibraryCandidateScope {
    LibraryCandidateScope {
        kinds: BoundedVec::new(vec![AgentComponentKind::Tool]).unwrap(),
        classification_under: None,
    }
}

fn q32(tenths: i64) -> QuantisedValue {
    QuantisedValue {
        scale: QuantScaleTag::Q32,
        value: (tenths << 32) / 10,
    }
}

fn candidate(id: &str, revision: u64, importance_tenths: i64) -> ComparativeCandidate {
    ComparativeCandidate {
        candidate_id: id.to_string(),
        candidate_revision: revision,
        score: q32(5),
        importance: q32(importance_tenths),
    }
}

fn comparative_request(
    pin: &ComponentDependency,
    candidates: Vec<ComparativeCandidate>,
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: "comparative.test".into(),
            kind: QuestionKind::ComparativeAnalysisPlan,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::AgentLibrary {
            scope: tool_scope(),
        },
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![TypedParam {
            name: "comparative_analysis".into(),
            value: TypedValue::ComparativeAnalysis(Box::new(ComparativeAnalysisRequest {
                space_id: "space:a".into(),
                space_revision: 3,
                query_vector_digest: DIGEST.into(),
                unified_query_result_digest: DIGEST.into(),
                candidates: BoundedVec::new(candidates).unwrap(),
                total_call_ceiling: 10,
            })),
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

/// Publish three tools into `tenant` and return their visible revisions.
fn publish_tools(h: &Harness, tenant: &str, ids: &[&str]) -> Vec<(String, u64)> {
    for id in ids {
        h.commit_in(tenant, test_component_draft(tenant, id), None)
            .unwrap();
    }
    library_candidates(&h.store, tenant, &tool_scope())
        .unwrap()
        .entries
        .iter()
        .filter(|entry| ids.contains(&entry.component_id.as_str()))
        .map(|entry| (entry.component_id.clone(), entry.entry_revision))
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R139.3
#[tokio::test]
async fn served_decide_returns_budgeted_plan() {
    let transport = RecordingTransport(AtomicUsize::new(0));
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let revisions = publish_tools(&h, TENANT, &["cand-a", "cand-b", "cand-c"]);
    let importance = |id: &str| match id {
        "cand-a" => 9,
        "cand-b" => 6,
        _ => 1,
    };
    let candidates = revisions
        .iter()
        .map(|(id, revision)| candidate(id, *revision, importance(id)))
        .collect();
    let batch = decide(&h, comparative_request(&pin, candidates))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let plan = record.comparative_analysis_plan().expect("a plan answer");
    assert_eq!(plan.tenant_id, TENANT);
    assert_eq!(plan.space_revision, 3);
    assert_eq!(plan.total_call_ceiling, 10);
    assert_eq!(
        plan.candidates
            .iter()
            .map(|c| c.candidate_id.as_str())
            .collect::<Vec<_>>(),
        ["cand-a", "cand-b", "cand-c"]
    );
    // Default thresholds: synthesis takes importance >= 0.5 (two candidates,
    // one call), extraction takes importance >= 0.8 (one candidate).
    let budgets: Vec<_> = plan
        .stage_budgets
        .iter()
        .map(|b| (b.stage, b.max_calls))
        .collect();
    assert_eq!(
        budgets,
        [
            (ComparativeAnalysisStage::Discovery, 0),
            (ComparativeAnalysisStage::Synthesis, 1),
            (ComparativeAnalysisStage::Extraction, 1),
        ]
    );
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    assert_eq!(record.tenant_id, TENANT);
    assert_eq!(
        record.caller_principal,
        verified().principal_persistence_id()
    );
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(record),
        record.record_digest
    );
    // The engine holds no model path: the discovery stage budgets no calls and
    // the transport the test holds never receives one.
    assert_eq!(transport.0.load(Ordering::SeqCst), 0);
}

// spec: MEDULLA-DECISION-ENGINE-R139.3
#[tokio::test]
async fn other_tenant_candidates_never_enter_the_plan() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let own = publish_tools(&h, TENANT, &["cand-own"]);
    let foreign = publish_tools(&h, "tenant-other", &["cand-foreign"]);
    // The caller's own candidate plans.
    let ok = decide(
        &h,
        comparative_request(&pin, vec![candidate(&own[0].0, own[0].1, 9)]),
    )
    .await
    .unwrap();
    let plan = ok.records.as_slice()[0]
        .comparative_analysis_plan()
        .expect("a plan answer");
    assert!(plan.candidates.iter().all(|c| c.candidate_id == "cand-own"));
    // Another tenant's candidate, at its true revision, refuses with the same
    // code as an id that does not exist: the refusal never confirms the row.
    let foreign_error = decide(
        &h,
        comparative_request(&pin, vec![candidate(&foreign[0].0, foreign[0].1, 9)]),
    )
    .await
    .unwrap_err();
    let missing_error = decide(
        &h,
        comparative_request(&pin, vec![candidate("cand-missing", 1, 9)]),
    )
    .await
    .unwrap_err();
    assert!(foreign_error.contains("COMPARATIVE_CANDIDATE_INVALID"));
    assert!(missing_error.contains("COMPARATIVE_CANDIDATE_INVALID"));
    assert_eq!(
        foreign_error.replace("cand-foreign", "X"),
        missing_error.replace("cand-missing", "X")
    );
    // A stale revision of a visible candidate refuses as stale.
    let stale = decide(
        &h,
        comparative_request(&pin, vec![candidate(&own[0].0, own[0].1 + 1, 9)]),
    )
    .await
    .unwrap_err();
    assert!(stale.contains("COMPARATIVE_STALE_INPUT"));
}

// spec: MEDULLA-DECISION-ENGINE-R139.3
#[tokio::test]
async fn decide_returns_plan_without_model_execution() {
    let transport = RecordingTransport(AtomicUsize::new(0));
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let revisions = publish_tools(&h, TENANT, &["cand-a"]);
    let request = comparative_request(&pin, vec![candidate(&revisions[0].0, revisions[0].1, 9)]);
    let batch = decide(&h, request).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let plan = record.comparative_analysis_plan().expect("a plan answer");
    // The plan pins the caller's inputs and the policy in force.
    assert_eq!(plan.space_id, "space:a");
    assert_eq!(plan.space_revision, 3);
    assert_eq!(plan.query_vector_digest, DIGEST);
    assert_eq!(plan.unified_query_result_digest, DIGEST);
    assert_eq!(plan.policy_digest, record.inputs.policy_digest);
    assert_eq!(plan.plan_digest.len(), 64);
    // Discovery budgets zero calls, and no model call is made.
    let discovery = plan
        .stage_budgets
        .iter()
        .find(|b| b.stage == ComparativeAnalysisStage::Discovery)
        .unwrap();
    assert_eq!(discovery.max_calls, 0);
    assert_eq!(transport.0.load(Ordering::SeqCst), 0);
}
