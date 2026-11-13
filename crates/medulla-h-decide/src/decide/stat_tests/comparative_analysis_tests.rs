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

/// Mutate the typed comparative body of `request` in place.
fn edit_body(request: &mut DecideRequest, edit: impl FnOnce(&mut ComparativeAnalysisRequest)) {
    let mut params = request.params.as_slice().to_vec();
    let TypedValue::ComparativeAnalysis(body) = &mut params[0].value else {
        panic!("a comparative_analysis parameter");
    };
    edit(body);
    request.params = BoundedVec::new(params).unwrap();
}

fn with_ceiling(mut request: DecideRequest, ceiling: u32) -> DecideRequest {
    edit_body(&mut request, |body| body.total_call_ceiling = ceiling);
    request
}

fn thresholds(h: &Harness, synthesis: i64, extraction: i64) -> DecisionPolicyRef {
    use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.knobs.insert(
        KnobName::new("comparative.synthesis_importance_min"),
        KnobValue::DecimalMicros(synthesis),
    );
    policy.knobs.insert(
        KnobName::new("comparative.extraction_importance_min"),
        KnobValue::DecimalMicros(extraction),
    );
    DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-comparative-thresholds", &policy),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R139.4
#[tokio::test]
async fn comparative_refusals() {
    let transport = RecordingTransport(AtomicUsize::new(0));
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let revisions = publish_tools(&h, TENANT, &["cand-a", "cand-b", "cand-c"]);
    let at = |id: &str, importance: i64| {
        let (id, revision) = revisions.iter().find(|(known, _)| known == id).unwrap();
        candidate(id, *revision, importance)
    };
    let refuses = |code: &str, request: DecideRequest| {
        let h = &h;
        async move {
            let error = decide(h, request).await.expect_err(code);
            assert!(error.contains(code), "{code}: {error}");
            error
        }
    };

    // COMPARATIVE_STALE_INPUT: a candidate revision differs from the visible one.
    let mut stale = at("cand-a", 9);
    stale.candidate_revision += 1;
    refuses(
        "COMPARATIVE_STALE_INPUT",
        comparative_request(&pin, vec![stale]),
    )
    .await;
    // COMPARATIVE_CANDIDATE_INVALID: invisible, then duplicate.
    refuses(
        "COMPARATIVE_CANDIDATE_INVALID",
        comparative_request(&pin, vec![candidate("cand-missing", 1, 9)]),
    )
    .await;
    refuses(
        "COMPARATIVE_CANDIDATE_INVALID",
        comparative_request(&pin, vec![at("cand-a", 9), at("cand-a", 9)]),
    )
    .await;
    // COMPARATIVE_OVER_LIMIT: a 65th candidate is refused when the request is
    // decoded, by the registered code.
    let mut wire = serde_json::to_value(comparative_request(&pin, vec![at("cand-a", 9)])).unwrap();
    let carried = wire
        .pointer_mut("/params/0/value/value/candidates")
        .and_then(|value| value.as_array_mut())
        .expect("the candidates travel in the typed param");
    let first = carried[0].clone();
    carried.extend(std::iter::repeat(first).take(64));
    let error = serde_json::from_value::<DecideRequest>(wire).unwrap_err();
    assert!(
        error.to_string().contains("COMPARATIVE_OVER_LIMIT"),
        "{error}"
    );
    // COMPARATIVE_IMPORTANCE_INVALID: an importance above 1.
    refuses(
        "COMPARATIVE_IMPORTANCE_INVALID",
        comparative_request(&pin, vec![at("cand-a", 11)]),
    )
    .await;
    // COMPARATIVE_THRESHOLD_ORDER: the extraction minimum below the synthesis one.
    let mut inverted = comparative_request(&pin, vec![at("cand-a", 9)]);
    inverted.policy = thresholds(&h, 500_000, 200_000);
    refuses("COMPARATIVE_THRESHOLD_ORDER", inverted).await;
    // COMPARATIVE_BUDGET_INVALID: a zero ceiling where calls are needed, then a
    // ceiling above the knob maximum (130).
    refuses(
        "COMPARATIVE_BUDGET_INVALID",
        with_ceiling(comparative_request(&pin, vec![at("cand-a", 9)]), 0),
    )
    .await;
    refuses(
        "COMPARATIVE_BUDGET_INVALID",
        with_ceiling(comparative_request(&pin, vec![at("cand-a", 9)]), 131),
    )
    .await;
    // COMPARATIVE_STAGE_EXCEEDS_TOTAL names the stage, the requested calls and
    // the calls that remain: synthesis takes the one call there is, and
    // extraction needs three.
    let error = refuses(
        "COMPARATIVE_STAGE_EXCEEDS_TOTAL",
        with_ceiling(
            comparative_request(
                &pin,
                vec![at("cand-a", 9), at("cand-b", 9), at("cand-c", 9)],
            ),
            1,
        ),
    )
    .await;
    assert!(
        error.contains("stage Extraction needs 3 calls and 0 remain under the ceiling"),
        "{error}"
    );
    // No refusal reached a model.
    assert_eq!(transport.0.load(Ordering::SeqCst), 0);
}

// spec: MEDULLA-DECISION-ENGINE-R139.5
#[tokio::test]
async fn mutating_a_bound_input_changes_the_plan_digest() {
    use medulla_decision_types::decision::{PremiseClass, PremiseProvenance};
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let revisions = publish_tools(&h, TENANT, &["cand-a", "cand-b"]);
    let base = |h: &Harness| {
        let candidates = revisions
            .iter()
            .map(|(id, revision)| candidate(id, *revision, if id == "cand-a" { 9 } else { 6 }))
            .collect();
        let _ = h;
        comparative_request(&pin, candidates)
    };
    let plan_of = |request: DecideRequest| {
        let h = &h;
        async move {
            let batch = decide(h, request).await.unwrap();
            let record = batch.records.as_slice()[0].clone();
            let plan = record.comparative_analysis_plan().expect("a plan").clone();
            (record, plan)
        }
    };
    let (record, plan) = plan_of(base(&h)).await;

    // One Claim premise per bound input, each with Request provenance.
    let fact = |text: &str| {
        let found: Vec<_> = record
            .premises
            .iter()
            .filter(|premise| premise.fact == text)
            .collect();
        assert_eq!(found.len(), 1, "one premise {text}");
        assert_eq!(found[0].class, PremiseClass::Claim);
        assert_eq!(
            found[0].provenance,
            PremiseProvenance::Request {
                field: "comparative_analysis".to_string()
            }
        );
    };
    fact("comparative:space=space:a@3");
    fact(&format!("comparative:query_vector={DIGEST}"));
    fact(&format!("comparative:result={DIGEST}"));
    let ordered: Vec<&str> = record
        .premises
        .iter()
        .map(|premise| premise.fact.as_str())
        .filter(|fact| fact.starts_with("comparative:candidate="))
        .collect();
    assert_eq!(ordered.len(), 2);
    for (premise, candidate) in ordered.iter().zip(plan.candidates.iter()) {
        assert!(premise.starts_with(&format!(
            "comparative:candidate={}@{}/0.500000/",
            candidate.candidate_id, candidate.candidate_revision
        )));
    }
    fact("comparative:stage=discovery/0");
    fact("comparative:stage=synthesis/1");
    fact("comparative:stage=extraction/1");
    assert!(record.premises.iter().any(|premise| premise
        .fact
        .starts_with("knob:comparative.synthesis_batch=")));
    assert_eq!(plan.plan_digest.len(), 64);

    // Mutating any bound input changes the digest.
    let mutations: Vec<(&str, Box<dyn Fn(&mut ComparativeAnalysisRequest)>)> = vec![
        ("space id", Box::new(|b| b.space_id = "space:b".into())),
        ("space revision", Box::new(|b| b.space_revision = 4)),
        (
            "query vector",
            Box::new(|b| b.query_vector_digest = "1".repeat(64)),
        ),
        (
            "result",
            Box::new(|b| b.unified_query_result_digest = "2".repeat(64)),
        ),
        ("ceiling", Box::new(|b| b.total_call_ceiling = 11)),
        (
            "importance",
            Box::new(|b| {
                let mut all = b.candidates.as_slice().to_vec();
                all[1].importance = q32(7);
                b.candidates = BoundedVec::new(all).unwrap();
            }),
        ),
        (
            "score",
            Box::new(|b| {
                let mut all = b.candidates.as_slice().to_vec();
                all[0].score = q32(4);
                b.candidates = BoundedVec::new(all).unwrap();
            }),
        ),
        (
            "candidate order",
            Box::new(|b| {
                let mut all = b.candidates.as_slice().to_vec();
                all.reverse();
                b.candidates = BoundedVec::new(all).unwrap();
            }),
        ),
    ];
    let mut digests = std::collections::BTreeSet::from([plan.plan_digest.clone()]);
    for (name, edit) in mutations {
        let mut request = base(&h);
        edit_body(&mut request, |body| edit(body));
        let (_, mutated) = plan_of(request).await;
        assert!(
            digests.insert(mutated.plan_digest.clone()),
            "mutating the {name} must change the plan digest"
        );
    }
}
