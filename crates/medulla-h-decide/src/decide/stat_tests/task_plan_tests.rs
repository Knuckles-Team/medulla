//! Bounded explicit plans through the actual Decide handler.

use super::*;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::{
    decision::task_plan::TaskPlanProposal, decision::task_plan::TaskPlanTask,
    decision::task_plan::TASK_PLAN_POLICY_REVISION,
};

fn task_request(pin: &ComponentDependency) -> DecideRequest {
    let tasks: Vec<TaskPlanTask> = [
        ("a", vec![]),
        ("b", vec![]),
        ("c", vec!["b"]),
        ("join", vec!["a", "c"]),
    ]
    .into_iter()
    .map(|(id, deps)| TaskPlanTask {
        task_id: id.into(),
        prompt: format!("Analyze {id} with evidence"),
        depends_on: BoundedVec::new(deps.into_iter().map(str::to_string).collect()).unwrap(),
        call: None,
        aggregation: None,
        assignee: None,
        branch: None,
        join: None,
        decision: None,
        approval: None,
        duration_ms: None,
        priority_weight: None,
    })
    .collect();
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: "explicit-plan".into(),
            kind: QuestionKind::TaskPlan,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(
                tasks
                    .iter()
                    .map(|t| DeclaredOption {
                        option_id: t.task_id.clone(),
                        classification: BoundedVec::default(),
                        numbers: BoundedVec::default(),
                        texts: BoundedVec::default(),
                    })
                    .collect(),
            )
            .unwrap(),
        },
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![TypedParam {
            name: "task_plan".into(),
            value: TypedValue::TaskPlan(Box::new(TaskPlanProposal {
                tasks: BoundedVec::new(tasks).unwrap(),
                output_task_id: "join".into(),
                max_concurrency: 2,
                max_steps: 4,
                final_artifact: None,
            })),
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.3
// spec: MEDULLA-DECISION-ENGINE-R134.111.5
#[tokio::test]
async fn decide_routes_task_plan() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let batch = decide(&h, task_request(&pin)).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let plan = record.task_plan().unwrap();
    assert_eq!(
        plan.tasks
            .iter()
            .map(|t| t.task_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c", "join"]
    );
    assert_eq!(plan.policy_revision, TASK_PLAN_POLICY_REVISION);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    assert_eq!(
        record.resolution_kind,
        medulla_decision_types::decision::ResolutionKind::Constraint
    );
    assert_eq!(record.tenant_id, TENANT);
    assert_eq!(
        record.caller_principal,
        verified().principal_persistence_id()
    );
    assert!(record.premises.iter().any(|p| p.subject == "c"
        && p.fact.contains("dependencies=b")
        && p.class == medulla_decision_types::decision::PremiseClass::Claim));
    assert!(record.premises.iter().any(|p|p.subject==TASK_PLAN_POLICY_REVISION && matches!(&p.provenance, medulla_decision_types::decision::PremiseProvenance::Policy {policy_digest} if *policy_digest==plan.policy_digest)));
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(record),
        record.record_digest
    );
    let mut altered = record.clone();
    let Some(medulla_decision_types::decision::statistical::outcome::FamilyAnswer::TaskPlan(plan)) =
        altered.answer.as_mut()
    else {
        panic!("a task plan answer");
    };
    plan.max_concurrency = 3;
    assert_ne!(
        medulla_decision_types::decision::digest::statistical_record_digest(&altered),
        record.record_digest
    );
}

/// The decision seals an abstention from the `task_plan` family: the code, the
/// uncovered subtask in the detail and in a premise, and no plan.
async fn assert_abstained(h: &Harness, request: DecideRequest, code: &str, detail: &str) {
    use medulla_decision_types::decision::statistical::StatisticalOutcome;
    use medulla_decision_types::decision::AbstainReason;
    let batch = decide(h, request).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert!(record.task_plan().is_none());
    assert_eq!(
        record.resolution_kind,
        medulla_decision_types::decision::ResolutionKind::Abstention
    );
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("an abstention, got {:?}", record.outcome);
    };
    match &reasons.as_slice()[0] {
        AbstainReason::Family {
            family,
            code: raised,
            detail: named,
        } => {
            assert_eq!(*family, QuestionKind::TaskPlan);
            assert_eq!(raised.as_str(), code);
            assert_eq!(named, detail);
        }
        other => panic!("a family abstention, got {other:?}"),
    }
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact.contains(&format!("code={code}")) && p.fact.contains(detail)));
}

async fn assert_invalid(h: &Harness, request: DecideRequest, detail: &str) {
    let response =
        super::super::statistical::handle_decide(&h.state, 1, &verified(), request).await;
    assert_eq!(response.error.as_deref(), Some("PARAMETER_INVALID"));
    assert!(
        response
            .error_detail
            .as_deref()
            .is_some_and(|text| text.contains(detail)),
        "expected {detail:?}, got {:?}",
        response.error_detail
    );
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.4
#[tokio::test]
async fn invalid_task_requests_refuse() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let mut request = task_request(&pin);
    request.question.safety = QuestionSafety::Security;
    assert_invalid(&h, request, "ordinary safety").await;
    let mut request = task_request(&pin);
    request.tenant_id = "foreign-tenant".into();
    assert!(decide(&h, request)
        .await
        .unwrap_err()
        .starts_with("ACCESS_DENIED"));
    let mut request = task_request(&pin);
    request.head = Some(pin.clone());
    assert_invalid(&h, request, "no head").await;
    let mut request = task_request(&pin);
    request.belief_as_of = BoundedVec::new(vec![1]).unwrap();
    assert_invalid(&h, request, "belief slices").await;
    let mut request = task_request(&pin);
    request.candidates = CandidateSource::Declared {
        options: BoundedVec::default(),
    };
    assert_invalid(&h, request, "exactly the sorted task ids").await;
    let mut request = task_request(&pin);
    request.question.kind = QuestionKind::Route;
    assert_invalid(&h, request, "requires task_plan question").await;
    let mut request = task_request(&pin);
    let mut params = request.params.as_slice().to_vec();
    let TypedValue::TaskPlan(proposal) = &mut params[0].value else {
        unreachable!()
    };
    proposal.output_task_id = "b".into();
    request.params = BoundedVec::new(params).unwrap();
    assert_invalid(&h, request, "DISCONNECTED_OUTPUT").await;
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.5
#[tokio::test]
async fn task_plan_cannot_use_score_replay() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let batch = decide(&h, task_request(&pin)).await.unwrap();
    let mut record = batch.records.as_slice()[0].clone();
    record.question.kind = QuestionKind::Route;
    record.outcome = StatisticalOutcome::Abstained {
        reasons: BoundedVec::default(),
    };
    record.record_digest =
        medulla_decision_types::decision::digest::statistical_record_digest(&record);
    let ctx = super::super::stat_executor::ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: TENANT,
        now_ms: 1,
        server_secret: b"fixture",
        matched: &[],
        designation: None,
    };
    assert!(super::super::stat_replay::replay(&ctx, &record)
        .unwrap_err()
        .contains("evaluate-only"));
}

/// The canonical task-plan schema publishes once per tenant: a repeat publish
/// under its publish key replays the same component revision and digest, and
/// that pin routes a task-plan Decide.
// spec: MEDULLA-DECISION-ENGINE-R134.111.7
#[tokio::test]
async fn canonical_task_plan_schema_publishes_once_and_routes() {
    use medulla_decision_types::{
        decision::task_plan::task_plan_feature_schema,
        decision::task_plan::TASK_PLAN_FEATURE_SCHEMA_COMPONENT_ID,
    };
    let h = Harness::new().await;
    let body = task_plan_feature_schema().checked().unwrap();
    let encoded = encode_body(&body).unwrap();
    let key = format!("eg:task-plan-feature-schema:{}", encoded.content_digest);
    let publish = |nonce: u8| {
        let mut draft = test_component_draft(TENANT, TASK_PLAN_FEATURE_SCHEMA_COMPONENT_ID);
        draft.kind = AgentComponentKind::FeatureSchema;
        draft.summary = "task plan features".to_string();
        draft.content_digest = encoded.content_digest.clone();
        draft.attributes = encoded.attributes.clone();
        h.store
            .publish_component(AgentComponentPublishRequest {
                context: mutation_context(
                    &h.store,
                    TENANT,
                    &key,
                    nonce,
                    0,
                    "agent-component:publish",
                ),
                component: draft,
                evaluation_receipt_digest: None,
            })
            .unwrap()
            .result
            .component
            .definition_digest
    };
    let first = publish(201);
    let second = publish(202);
    assert_eq!(
        first, second,
        "a repeat publish must replay the same revision"
    );
    let pin = ComponentDependency {
        component_id: TASK_PLAN_FEATURE_SCHEMA_COMPONENT_ID.to_string(),
        kind: AgentComponentKind::FeatureSchema,
        definition_digest: first,
    };
    let batch = decide(&h, task_request(&pin)).await.unwrap();
    assert!(batch.records.as_slice()[0].task_plan().is_some());
}

fn with_call(request: &mut DecideRequest, reference: &str) {
    let mut params = request.params.as_slice().to_vec();
    let TypedValue::TaskPlan(proposal) = &mut params[0].value else {
        unreachable!()
    };
    let mut tasks = proposal.tasks.as_slice().to_vec();
    tasks[3].call = Some(medulla_decision_types::decision::task_plan::TaskPlanCall {
        server: "github".into(),
        tool: "create_issue".into(),
        arguments: serde_json::json!({
            "title": "Joined",
            "body": { "$ref": reference },
        }),
    });
    proposal.tasks = BoundedVec::new(tasks).unwrap();
    request.params = BoundedVec::new(params).unwrap();
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.12, MEDULLA-DECISION-ENGINE-R134.111.13
#[tokio::test]
async fn decide_routes_task_plan_with_calls() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let mut request = task_request(&pin);
    with_call(&mut request, "steps.c.output.summary");
    let batch = decide(&h, request).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let plan = record.task_plan().unwrap();
    let join = plan.tasks.iter().find(|t| t.task_id == "join").unwrap();
    let call = join.call.as_ref().unwrap();
    assert_eq!(
        (call.server.as_str(), call.tool.as_str()),
        ("github", "create_issue")
    );
    assert_eq!(
        call.arguments["body"],
        serde_json::json!({ "$ref": "steps.c.output.summary" })
    );
    assert!(plan
        .tasks
        .iter()
        .filter(|t| t.task_id != "join")
        .all(|t| t.call.is_none()));
    let mut request = task_request(&pin);
    with_call(&mut request, "steps.b.output");
    assert_invalid(
        &h,
        request,
        "BINDING_NOT_DEPENDENCY: task=join pointer=/body",
    )
    .await;
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.11
#[tokio::test]
async fn premises_name_calls_and_bindings() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let mut request = task_request(&pin);
    with_call(&mut request, "steps.c.output.summary");
    let batch = decide(&h, request).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let claims: Vec<&str> = record
        .premises
        .iter()
        .filter(|p| {
            p.subject == "join" && p.class == medulla_decision_types::decision::PremiseClass::Claim
        })
        .map(|p| p.fact.as_str())
        .collect();
    assert!(claims.contains(
        &"task_plan_call:server=github;tool=create_issue;bindings=steps.c.output.summary"
    ));
    assert!(claims.contains(
        &"task_plan_binding:pointer=/body;source_task=c;reference=steps.c.output.summary"
    ));
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(record),
        record.record_digest
    );
}

/// A task-plan request over a proposal given as JSON, with the declared
/// candidates the handler requires (the sorted task ids).
fn request_with(pin: &ComponentDependency, proposal: serde_json::Value) -> DecideRequest {
    let proposal: TaskPlanProposal = serde_json::from_value(proposal).unwrap();
    let mut request = task_request(pin);
    let mut ids: Vec<String> = proposal.tasks.iter().map(|t| t.task_id.clone()).collect();
    ids.sort();
    request.candidates = CandidateSource::Declared {
        options: BoundedVec::new(
            ids.into_iter()
                .map(|option_id| DeclaredOption {
                    option_id,
                    classification: BoundedVec::default(),
                    numbers: BoundedVec::default(),
                    texts: BoundedVec::default(),
                })
                .collect(),
        )
        .unwrap(),
    };
    request.params = BoundedVec::new(vec![TypedParam {
        name: "task_plan".into(),
        value: TypedValue::TaskPlan(Box::new(proposal)),
    }])
    .unwrap();
    request
}

fn step(id: &str, deps: &[&str], extra: serde_json::Value) -> serde_json::Value {
    let mut value = serde_json::json!({
        "task_id": id, "prompt": format!("Do {id}"), "depends_on": deps,
    });
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    value
}

fn control_proposal(join: bool, timeout_s: u64) -> serde_json::Value {
    let none = serde_json::json!({});
    let merge = if join {
        serde_json::json!({"join": {"branch_task_id": "fork"}})
    } else {
        none.clone()
    };
    serde_json::json!({
        "tasks": [
            step("start", &[], none.clone()),
            step("fork", &["start"], serde_json::json!({"branch": {
                "kind": "parallel",
                "arms": [{"target_task_id": "x"}, {"target_task_id": "y"}]}})),
            step("x", &["fork"], none.clone()),
            step("y", &["fork"], none.clone()),
            step("merge", &["x", "y"], merge),
            step("gate", &["merge"], serde_json::json!({"approval": {
                "approver_role": "reviewer", "timeout_s": timeout_s}})),
        ],
        "output_task_id": "gate",
        "max_concurrency": 4,
        "max_steps": 8
    })
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.16
#[tokio::test]
async fn control_refusals_name_the_task() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let ok = decide(&h, request_with(&pin, control_proposal(true, 3600)))
        .await
        .unwrap();
    let plan = ok.records.as_slice()[0].task_plan().unwrap();
    assert!(plan.tasks.iter().any(|t| t.branch.is_some()));
    assert!(ok.records.as_slice()[0]
        .premises
        .iter()
        .any(|p| p.subject == "gate" && p.fact.starts_with("task_plan_approval:")));
    assert_invalid(
        &h,
        request_with(&pin, control_proposal(false, 3600)),
        "BRANCH_JOIN_UNMATCHED: task=fork",
    )
    .await;
    assert_invalid(
        &h,
        request_with(&pin, control_proposal(true, 59)),
        "APPROVAL_TIMEOUT_OUT_OF_RANGE: task=gate",
    )
    .await;
    let mut conflict = control_proposal(true, 3600);
    conflict["tasks"][5]["join"] = serde_json::json!({"branch_task_id": "fork"});
    assert_invalid(
        &h,
        request_with(&pin, conflict),
        "STEP_KIND_CONFLICT: task=gate",
    )
    .await;
    let mut unknown_arm = control_proposal(true, 3600);
    unknown_arm["tasks"][1]["branch"]["arms"][1]["target_task_id"] = "ghost".into();
    assert_invalid(
        &h,
        request_with(&pin, unknown_arm),
        "BRANCH_ARM_UNKNOWN: task=fork",
    )
    .await;
    let mut no_default = control_proposal(true, 3600);
    no_default["tasks"][1]["branch"]["kind"] = "exclusive".into();
    assert_invalid(
        &h,
        request_with(&pin, no_default),
        "BRANCH_NO_DEFAULT: task=fork",
    )
    .await;
}

/// The maximum bounded workload of MEDULLA-DECISION-ENGINE-R134.111.6: 64
/// tasks, the most edges the concurrency bound allows and the full 64 KiB of
/// prompt text.
fn maximum_workload() -> serde_json::Value {
    let tasks: Vec<serde_json::Value> = (0..64)
        .map(|i| {
            let deps: Vec<String> = (0..i).rev().take(8).map(|d| format!("t{d:02}")).collect();
            let refs: Vec<&str> = deps.iter().map(String::as_str).collect();
            let mut task = step(&format!("t{i:02}"), &refs, serde_json::json!({}));
            task["prompt"] = "x".repeat(1024).into();
            task
        })
        .collect();
    serde_json::json!({
        "tasks": tasks, "output_task_id": "t63", "max_concurrency": 8, "max_steps": 64
    })
}

// spec: MEDULLA-DECISION-ENGINE-R134.111.6
#[tokio::test]
async fn maximum_bounded_plan_is_accepted() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let first = decide(&h, request_with(&pin, maximum_workload()))
        .await
        .unwrap();
    let second = decide(&h, request_with(&pin, maximum_workload()))
        .await
        .unwrap();
    let plan = first.records.as_slice()[0].task_plan().unwrap();
    assert_eq!(plan.total_tasks, 64);
    assert_eq!(
        plan.plan_digest,
        second.records.as_slice()[0]
            .task_plan()
            .unwrap()
            .plan_digest,
        "the digest of the maximum plan is stable"
    );
}

fn declared_ids(ids: &[&str]) -> CandidateSource {
    let mut sorted: Vec<&str> = ids.to_vec();
    sorted.sort();
    CandidateSource::Declared {
        options: BoundedVec::new(
            sorted
                .into_iter()
                .map(|id| DeclaredOption {
                    option_id: id.to_string(),
                    classification: BoundedVec::default(),
                    numbers: BoundedVec::default(),
                    texts: BoundedVec::default(),
                })
                .collect(),
        )
        .unwrap(),
    }
}

fn plan_request(
    pin: &ComponentDependency,
    name: &str,
    value: TypedValue,
    ids: &[&str],
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: "derived-plan".into(),
            kind: QuestionKind::TaskPlan,
            safety: QuestionSafety::Ordinary,
        },
        candidates: declared_ids(ids),
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![TypedParam {
            name: name.into(),
            value,
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn decompose_subtask(
    id: &str,
    deps: &[&str],
    schema: Option<&str>,
    specialists: &[(&str, i64)],
) -> medulla_decision_types::decision::task_plan::DecomposeSubtask {
    use medulla_decision_types::decision::task_plan::{DecomposeSubtask, SpecialistScore};
    DecomposeSubtask {
        id: id.into(),
        text: format!("Handle {id} with cited evidence"),
        depends_on: BoundedVec::new(deps.iter().map(|d| d.to_string()).collect()).unwrap(),
        answer_schema: schema.map(str::to_string),
        specialists: BoundedVec::new(
            specialists
                .iter()
                .map(|(id, micros)| SpecialistScore {
                    specialist_id: id.to_string(),
                    score_q32: ((i128::from(*micros) << 32) / 1_000_000) as i64,
                })
                .collect(),
        )
        .unwrap(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R134.3
#[tokio::test]
async fn decomposition_parameter_routes_through_decide() {
    use medulla_decision_types::decision::task_plan::{AggregationOp, DecomposeFacts};
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let facts = DecomposeFacts {
        output_task_id: "out".into(),
        subtasks: BoundedVec::new(vec![
            decompose_subtask(
                "a",
                &[],
                Some("verdict"),
                &[("zed", 700_000), ("alpha", 700_000)],
            ),
            decompose_subtask("b", &[], Some("verdict"), &[("beta", 600_000)]),
            decompose_subtask("out", &["a", "b"], None, &[("lead", 400_000)]),
        ])
        .unwrap(),
    };
    let request = plan_request(
        &pin,
        "decomposition",
        TypedValue::TaskDecomposition(Box::new(facts.clone())),
        &["a", "b", "out"],
    );
    let batch = decide(&h, request).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let plan = record.task_plan().unwrap();
    let task = |id: &str| plan.tasks.iter().find(|t| t.task_id == id).unwrap();
    assert_eq!(task("a").assignee.as_deref(), Some("alpha"));
    assert_eq!(task("b").assignee.as_deref(), Some("beta"));
    assert!(task("a").depends_on.is_empty() && task("b").depends_on.is_empty());
    assert_eq!(
        task("out").aggregation.as_ref().map(|a| a.op),
        Some(AggregationOp::MajorityVote)
    );
    assert_eq!((plan.max_concurrency, plan.max_steps), (2, 3));
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(record),
        record.record_digest
    );
    // A subtask no specialist qualifies for refuses, naming it.
    let mut weak = facts;
    weak.subtasks = BoundedVec::new(vec![
        decompose_subtask("a", &[], None, &[("alpha", 100_000)]),
        decompose_subtask("out", &["a"], None, &[("lead", 900_000)]),
    ])
    .unwrap();
    let request = plan_request(
        &pin,
        "decomposition",
        TypedValue::TaskDecomposition(Box::new(weak)),
        &["a", "out"],
    );
    assert_abstained(&h, request, "NO_SPECIALIST_FOR_SUBTASK", "a").await;
}

fn publish_skill(h: &Harness, tenant: &str, nonce: u8, id: &str, text: &str) {
    publish_skill_with_parent(h, tenant, nonce, id, text, None);
}

fn publish_skill_with_parent(
    h: &Harness,
    tenant: &str,
    nonce: u8,
    id: &str,
    text: &str,
    parent: Option<&str>,
) {
    let mut draft = test_component_draft(tenant, id);
    draft.kind = AgentComponentKind::Skill;
    draft.summary = text.to_string();
    if let Some(parent) = parent {
        draft.attributes.insert("parent".into(), parent.into());
    }
    h.store
        .publish_component(AgentComponentPublishRequest {
            context: mutation_context(
                &h.store,
                tenant,
                &format!("seed:{id}"),
                nonce,
                0,
                "agent-component:publish",
            ),
            component: draft,
            evaluation_receipt_digest: None,
        })
        .unwrap();
}

// spec: MEDULLA-DECISION-ENGINE-R134.4
#[tokio::test]
async fn skill_query_parameter_plans_over_the_visible_skills() {
    use medulla_decision_types::decision::task_plan::skill_plan::{SkillPlanQuery, SkillSubtask};
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    publish_skill(
        &h,
        TENANT,
        41,
        "skill-search",
        "search documents archive retrieval index",
    );
    publish_skill(
        &h,
        TENANT,
        42,
        "skill-report",
        "summarise findings report writing narrative",
    );
    // Another tenant's skill covers the physics subtask; this tenant cannot see it.
    publish_skill(
        &h,
        "foreign-tenant",
        43,
        "foreign-physics",
        "quantum chromodynamics lattice physics",
    );
    let subtask = |id: &str, text: &str, deps: &[&str]| SkillSubtask {
        id: id.into(),
        text: text.into(),
        depends_on: BoundedVec::new(deps.iter().map(|d| d.to_string()).collect()).unwrap(),
    };
    let query = |subtasks: Vec<SkillSubtask>| {
        TypedValue::SkillPlanQuery(Box::new(SkillPlanQuery {
            output_task_id: "z".into(),
            subtasks: BoundedVec::new(subtasks).unwrap(),
            outcome_pins: BoundedVec::default(),
        }))
    };
    let request = plan_request(
        &pin,
        "skill_query",
        query(vec![
            subtask("a", "search documents archive retrieval", &[]),
            subtask("q", "quantum chromodynamics lattice", &[]),
            subtask("z", "summarise findings report narrative", &["a", "q"]),
        ]),
        &["a", "q", "z"],
    );
    assert_abstained(&h, request, "NO_SKILL_FOR_SUBTASK", "q").await;
    let request = plan_request(
        &pin,
        "skill_query",
        query(vec![
            subtask("a", "search documents archive retrieval", &[]),
            subtask("z", "summarise findings report narrative", &["a"]),
        ]),
        &["a", "z"],
    );
    let batch = decide(&h, request).await.unwrap();
    let plan = batch.records.as_slice()[0].task_plan().unwrap();
    let assignee = |id: &str| {
        plan.tasks
            .iter()
            .find(|t| t.task_id == id)
            .and_then(|t| t.assignee.as_deref())
    };
    assert_eq!(assignee("a"), Some("skill-search"));
    assert_eq!(assignee("z"), Some("skill-report"));
    // A tenant with no published skill abstains with the empty-index code.
    let empty = Harness::new().await;
    let (pin, _) = publish_route_schema(&empty);
    let request = plan_request(
        &pin,
        "skill_query",
        query(vec![subtask(
            "z",
            "summarise findings report narrative",
            &[],
        )]),
        &["z"],
    );
    assert_abstained(
        &empty,
        request,
        "EMPTY_SKILL_INDEX",
        "no published skill is visible",
    )
    .await;
}

// spec: MEDULLA-DECISION-ENGINE-R134.7
#[tokio::test]
async fn premises_name_skill_and_flag() {
    use medulla_decision_types::decision::task_plan::skill_plan::{SkillPlanQuery, SkillSubtask};
    use medulla_decision_types::decision::task_plan::DecomposeFacts;
    use medulla_decision_types::decision::PremiseClass;
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    publish_skill(
        &h,
        TENANT,
        51,
        "skill-search",
        "search documents archive retrieval index",
    );
    publish_skill(
        &h,
        TENANT,
        52,
        "skill-report",
        "summarise findings report writing narrative",
    );
    let subtask = |id: &str, text: &str, deps: &[&str]| SkillSubtask {
        id: id.into(),
        text: text.into(),
        depends_on: BoundedVec::new(deps.iter().map(|d| d.to_string()).collect()).unwrap(),
    };
    let request = plan_request(
        &pin,
        "skill_query",
        TypedValue::SkillPlanQuery(Box::new(SkillPlanQuery {
            output_task_id: "z".into(),
            subtasks: BoundedVec::new(vec![
                subtask("a", "search documents archive retrieval", &[]),
                subtask("z", "summarise findings report narrative", &["a"]),
            ])
            .unwrap(),
            outcome_pins: BoundedVec::default(),
        })),
        &["a", "z"],
    );
    let batch = decide(&h, request).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let fact_of = |record: &StatisticalDecisionRecord, subject: &str, prefix: &str| {
        record
            .premises
            .iter()
            .find(|p| p.subject == subject && p.fact.starts_with(prefix))
            .map(|p| (p.fact.clone(), p.class))
    };
    // Skill-planned tasks carry their relevance at or above the floor (0.2 in Q32).
    let floor = (200_000i64 << 32) / 1_000_000;
    for (task, assignee, independent) in [("a", "skill-search", true), ("z", "skill-report", false)]
    {
        let (fact, class) = fact_of(record, task, "task_plan_assignment:").unwrap();
        assert_eq!(class, PremiseClass::Claim);
        assert!(fact.starts_with(&format!(
            "task_plan_assignment:assignee={assignee};relevance_q32="
        )));
        assert!(
            fact.ends_with(&format!(";independent={independent}")),
            "{fact}"
        );
        let q: i64 = fact
            .split("relevance_q32=")
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .and_then(|n| n.parse().ok())
            .unwrap();
        assert!(q >= floor && q < (1i64 << 32), "{fact}");
    }
    // No aggregation was declared, so no aggregation premise.
    assert!(fact_of(record, "z", "task_plan_aggregation:").is_none());
    // Every knob of the family is a premise.
    for knob in [
        "task_plan.specialist_min_score",
        "skill_plan.relevance_floor",
    ] {
        assert!(record
            .premises
            .iter()
            .any(|p| p.fact.starts_with(&format!("knob:{knob}="))));
    }
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(record),
        record.record_digest
    );
    assert_eq!(record.evidence_class, EvidenceClass::Claim);

    // A decomposition has no relevance and the engine chose the operator.
    let facts = DecomposeFacts {
        output_task_id: "out".into(),
        subtasks: BoundedVec::new(vec![
            decompose_subtask("a", &[], Some("verdict"), &[("alpha", 700_000)]),
            decompose_subtask("b", &[], Some("verdict"), &[("beta", 600_000)]),
            decompose_subtask("out", &["a", "b"], None, &[("lead", 400_000)]),
        ])
        .unwrap(),
    };
    let request = plan_request(
        &pin,
        "decomposition",
        TypedValue::TaskDecomposition(Box::new(facts)),
        &["a", "b", "out"],
    );
    let batch = decide(&h, request).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(
        fact_of(record, "a", "task_plan_assignment:").unwrap().0,
        "task_plan_assignment:assignee=alpha;independent=true"
    );
    assert_eq!(
        fact_of(record, "out", "task_plan_assignment:").unwrap().0,
        "task_plan_assignment:assignee=lead;independent=false"
    );
    let (fact, class) = fact_of(record, "out", "task_plan_aggregation:").unwrap();
    assert_eq!(fact, "task_plan_aggregation:op=majority_vote;default=true");
    assert_eq!(class, PremiseClass::Definition);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
}

fn two_proposers(pin: &ComponentDependency) -> DecideRequest {
    use medulla_decision_types::decision::task_plan::DecomposeFacts;
    let mut request = task_request(pin);
    let mut params = request.params.as_slice().to_vec();
    params.push(TypedParam {
        name: "decomposition".into(),
        value: TypedValue::TaskDecomposition(Box::new(DecomposeFacts {
            output_task_id: "out".into(),
            subtasks: BoundedVec::new(vec![decompose_subtask(
                "out",
                &[],
                None,
                &[("lead", 900_000)],
            )])
            .unwrap(),
        })),
    });
    request.params = BoundedVec::new(params).unwrap();
    request
}

// spec: MEDULLA-DECISION-ENGINE-R134.5
#[tokio::test]
async fn decide_routes_automatic_task_plan() {
    use medulla_decision_types::decision::task_plan::skill_plan::{SkillPlanQuery, SkillSubtask};
    use medulla_decision_types::decision::task_plan::DecomposeFacts;
    use medulla_decision_types::decision::{PremiseClass, ResolutionKind};
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    publish_skill(
        &h,
        TENANT,
        51,
        "skill-search",
        "search documents archive retrieval",
    );
    publish_skill(
        &h,
        "foreign-tenant",
        52,
        "foreign-report",
        "summarise findings report writing narrative",
    );
    // A decomposition request and a skill-query request each return the typed
    // plan a caller proposal returns: the same family, resolution and evidence.
    let facts = DecomposeFacts {
        output_task_id: "out".into(),
        subtasks: BoundedVec::new(vec![
            decompose_subtask("a", &[], None, &[("alpha", 700_000)]),
            decompose_subtask("out", &["a"], None, &[("lead", 900_000)]),
        ])
        .unwrap(),
    };
    let request = plan_request(
        &pin,
        "decomposition",
        TypedValue::TaskDecomposition(Box::new(facts)),
        &["a", "out"],
    );
    let batch = decide(&h, request).await.unwrap();
    let from_decomposition = &batch.records.as_slice()[0];
    let plan = from_decomposition.task_plan().unwrap();
    assert_eq!(
        plan.tasks
            .iter()
            .map(|t| t.task_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "out"]
    );
    let subtask = |id: &str, text: &str, deps: &[&str]| SkillSubtask {
        id: id.into(),
        text: text.into(),
        depends_on: BoundedVec::new(deps.iter().map(|d| d.to_string()).collect()).unwrap(),
    };
    let query = |subtasks: Vec<SkillSubtask>, out: &str| {
        TypedValue::SkillPlanQuery(Box::new(SkillPlanQuery {
            output_task_id: out.into(),
            subtasks: BoundedVec::new(subtasks).unwrap(),
            outcome_pins: BoundedVec::default(),
        }))
    };
    let request = plan_request(
        &pin,
        "skill_query",
        query(
            vec![subtask("a", "search documents archive retrieval", &[])],
            "a",
        ),
        &["a"],
    );
    let batch = decide(&h, request).await.unwrap();
    let from_skills = &batch.records.as_slice()[0];
    let plan = from_skills.task_plan().unwrap();
    assert_eq!(
        plan.tasks.as_slice()[0].assignee.as_deref(),
        Some("skill-search")
    );
    let caller = decide(&h, task_request(&pin)).await.unwrap();
    let caller = &caller.records.as_slice()[0];
    for record in [from_decomposition, from_skills, caller] {
        assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
        assert_eq!(record.evidence_class, EvidenceClass::Claim);
        assert!(record
            .premises
            .iter()
            .any(|p| p.class == PremiseClass::Claim));
    }
    // A cross-tenant skill never appears: the report subtask only the other
    // tenant can cover is uncovered here.
    let request = plan_request(
        &pin,
        "skill_query",
        query(
            vec![subtask("r", "summarise findings report narrative", &[])],
            "r",
        ),
        &["r"],
    );
    let batch = decide(&h, request).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert!(record.task_plan().is_none());
    assert!(!format!("{record:?}").contains("foreign-report"));
    // Zero or more than one proposer parameter refuses.
    assert_invalid(&h, two_proposers(&pin), "PLAN_PARAMETER_CONFLICT").await;
    let mut none = task_request(&pin);
    none.params = BoundedVec::default();
    assert_invalid(&h, none, "PLAN_PARAMETER_CONFLICT").await;
}

// spec: MEDULLA-DECISION-ENGINE-R134.6
#[tokio::test]
async fn uncovered_subtask_abstains() {
    use medulla_decision_types::decision::task_plan::skill_plan::{SkillPlanQuery, SkillSubtask};
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    publish_skill(
        &h,
        TENANT,
        61,
        "skill-search",
        "search documents archive retrieval",
    );
    // Only the other tenant can cover the physics subtask.
    publish_skill(
        &h,
        "foreign-tenant",
        62,
        "foreign-physics",
        "quantum chromodynamics lattice physics",
    );
    let subtask = |id: &str, text: &str, deps: &[&str]| SkillSubtask {
        id: id.into(),
        text: text.into(),
        depends_on: BoundedVec::new(deps.iter().map(|d| d.to_string()).collect()).unwrap(),
    };
    let request = plan_request(
        &pin,
        "skill_query",
        TypedValue::SkillPlanQuery(Box::new(SkillPlanQuery {
            output_task_id: "z".into(),
            subtasks: BoundedVec::new(vec![
                subtask("a", "search documents archive retrieval", &[]),
                subtask("m", "quantum chromodynamics lattice", &[]),
                subtask("q", "quantum chromodynamics lattice physics", &[]),
                subtask("z", "search documents archive", &["a", "m", "q"]),
            ])
            .unwrap(),
            outcome_pins: BoundedVec::default(),
        })),
        &["a", "m", "q", "z"],
    );
    // The first uncovered subtask is named; no plan is carried.
    assert_abstained(&h, request, "NO_SKILL_FOR_SUBTASK", "m").await;
    // A dependency cycle refuses with the renamed upper-snake planner code.
    let mut cyclic = task_request(&pin);
    let mut params = cyclic.params.as_slice().to_vec();
    let TypedValue::TaskPlan(proposal) = &mut params[0].value else {
        unreachable!()
    };
    let mut tasks = proposal.tasks.as_slice().to_vec();
    tasks[0].depends_on = BoundedVec::new(vec!["join".to_string()]).unwrap();
    proposal.tasks = BoundedVec::new(tasks).unwrap();
    cyclic.params = BoundedVec::new(params).unwrap();
    assert_invalid(&h, cyclic, "PLAN_CYCLE").await;
}

/// Log `base` as an earlier decision of the deciding principal that shares its
/// context, with one stored evaluation of `skill`.
fn log_skill_outcome(
    h: &Harness,
    base: &StatisticalDecisionRecord,
    id: &str,
    skill: &str,
    ok: bool,
) {
    use medulla_decision_types::decision::statistical::log::{
        DecisionLogEntry, DecisionOutcomeEvaluation, EntryInputs, RecordVisibility,
        StoredEvaluation, DECISION_LOG_SCHEMA_VERSION,
    };
    use medulla_persistence_records::decision_jobs::{encode_artifact, evaluation_key, record_key};
    let mut record = base.clone();
    record.record_id = format!("decision:{id}");
    record.caller_principal = verified().principal_persistence_id();
    let entry = DecisionLogEntry {
        schema_version: DECISION_LOG_SCHEMA_VERSION,
        visibility: RecordVisibility::Tenant,
        committed_by: record.caller_principal.clone(),
        committed_at_ms: record.created_at_ms,
        record: Box::new(record.clone()),
        inputs: EntryInputs::Inline,
    };
    let stored = StoredEvaluation {
        evaluation: DecisionOutcomeEvaluation {
            record_id: record.record_id.clone(),
            evaluation_id: format!("eval-{id}"),
            class: EvidenceClass::Observation,
            selected_agent: skill.to_string(),
            lease_holder: "worker".to_string(),
            fidelity: OutcomeFidelity::ToolCalls,
            success: Some(ok),
            backbone: None,
        },
        producer: "evaluator".to_string(),
        recorded_at_ms: record.created_at_ms,
    };
    h.store
        .put_decision_artifacts(
            TENANT,
            &[
                (
                    record_key(&record.record_id),
                    encode_artifact(&entry).unwrap(),
                ),
                (
                    evaluation_key(&record.record_id, &stored.evaluation.evaluation_id),
                    encode_artifact(&stored).unwrap(),
                ),
            ],
        )
        .unwrap();
}

// spec: MEDULLA-DECISION-ENGINE-R134.9
#[tokio::test]
async fn served_decide_returns_ranked_skill_tree() {
    use medulla_decision_types::decision::task_plan::skill_plan::{SkillPlanQuery, SkillSubtask};
    use medulla_decision_types::decision::task_plan::skill_tree::OutcomeRef;
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    publish_skill(
        &h,
        TENANT,
        41,
        "skill-search",
        "search documents archive retrieval index",
    );
    publish_skill_with_parent(
        &h,
        TENANT,
        42,
        "skill-index",
        "search documents index retrieval catalogue",
        Some("skill-search"),
    );
    publish_skill(
        &h,
        TENANT,
        43,
        "skill-report",
        "summarise findings report narrative",
    );
    // Another tenant's skill never enters this tenant's tree.
    publish_skill(
        &h,
        "foreign-tenant",
        44,
        "foreign-search",
        "search documents archive retrieval index",
    );
    let query = |pins: Vec<OutcomeRef>| {
        TypedValue::SkillPlanQuery(Box::new(SkillPlanQuery {
            output_task_id: "a".into(),
            subtasks: BoundedVec::new(vec![SkillSubtask {
                id: "a".into(),
                text: "search documents archive retrieval".into(),
                depends_on: BoundedVec::default(),
            }])
            .unwrap(),
            outcome_pins: BoundedVec::new(pins).unwrap(),
        }))
    };
    let ask = |pins: Vec<OutcomeRef>| plan_request(&pin, "skill_query", query(pins), &["a"]);
    let node = |record: &StatisticalDecisionRecord, skill: &str| {
        record
            .task_plan()
            .unwrap()
            .skill_trees
            .iter()
            .find(|tree| tree.task_id == "a")
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.skill_id == skill)
            .cloned()
    };

    // Relevance alone: every skill above the floor is a node, ranked, linked.
    let first = decide(&h, ask(vec![])).await.unwrap();
    let base = first.records.as_slice()[0].clone();
    let tree = &base.task_plan().unwrap().skill_trees.as_slice()[0];
    let ids: Vec<(&str, u16)> = tree
        .nodes
        .iter()
        .map(|n| (n.skill_id.as_str(), n.rank))
        .collect();
    assert_eq!(ids, vec![("skill-search", 1), ("skill-index", 2)]);
    assert_eq!(
        tree.nodes.as_slice()[1].parent_skill_id.as_deref(),
        Some("skill-search")
    );
    assert_eq!(tree.nodes.as_slice()[0].parent_skill_id, None);
    let context = base.inputs.context_digest.clone().unwrap();
    assert!(tree
        .nodes
        .iter()
        .all(|n| n.context_digest == context && n.prior_outcome_refs.is_empty()));
    assert!(base.premises.iter().any(|p| p.fact
        == format!(
            "skill_node:task=a;skill=skill-index;parent=skill-search;rank=2;score_q32={}",
            tree.nodes.as_slice()[1].score_q32
        )));
    assert!(base
        .premises
        .iter()
        .any(|p| p.fact.starts_with("knob:skill_tree.max_nodes=")));
    let again = decide(&h, ask(vec![])).await.unwrap();
    assert_eq!(
        again.records.as_slice()[0].task_plan(),
        base.task_plan(),
        "same tree, same plan digest"
    );
    let relevance = node(&base, "skill-index").unwrap().score_q32;

    // Below `skill_tree.min_outcomes` the outcomes are cited but do not move the score.
    for n in 0..4 {
        log_skill_outcome(&h, &base, &format!("h{n}"), "skill-index", true);
    }
    let four = decide(&h, ask(vec![])).await.unwrap();
    let four = node(&four.records.as_slice()[0], "skill-index").unwrap();
    assert_eq!(four.score_q32, relevance);
    assert_eq!(four.prior_outcome_refs.len(), 4);

    // At the support floor the outcome rate moves it: 5 of 5 adds weight * 1/2.
    log_skill_outcome(&h, &base, "h4", "skill-index", true);
    let five = decide(&h, ask(vec![])).await.unwrap();
    let five_record = five.records.as_slice()[0].clone();
    let five_node = node(&five_record, "skill-index").unwrap();
    assert_eq!(five_node.score_q32, relevance + (3 * (1_i64 << 32)) / 20);
    assert_eq!(five_node.prior_outcome_refs.len(), 5);
    // The sealed refs pin the stored outcome, and pinning them reproduces the tree.
    let pins: Vec<OutcomeRef> = five_node.prior_outcome_refs.iter().cloned().collect();
    assert!(pins
        .iter()
        .all(|r| r.outcome_revision == 1 && r.outcome_digest.starts_with("sha256:")));
    let pinned = decide(&h, ask(pins.clone())).await.unwrap();
    assert_eq!(
        node(&pinned.records.as_slice()[0], "skill-index"),
        Some(five_node.clone())
    );

    // A stale ref, a ref to no record, and an outcome of another context refuse.
    let mut stale = pins.clone();
    stale[0].outcome_digest = "sha256:stale".into();
    assert_invalid(&h, ask(stale), "STALE_OUTCOME_EVIDENCE").await;
    let mut missing = pins.clone();
    missing[0].record_id = "decision:none".into();
    assert_invalid(&h, ask(missing), "STALE_OUTCOME_EVIDENCE").await;
    let mut other = base.clone();
    other.inputs.context_digest = Some("another-context".into());
    log_skill_outcome(&h, &other, "other", "skill-index", true);
    let other_ref = OutcomeRef {
        record_id: "decision:other".into(),
        outcome_revision: 1,
        outcome_digest: "x".into(),
    };
    assert_invalid(&h, ask(vec![other_ref]), "STALE_OUTCOME_EVIDENCE").await;
    // The foreign tenant's skill never appears.
    assert!(node(&five_record, "foreign-search").is_none());
}

// spec: MEDULLA-DECISION-ENGINE-R134.9
#[tokio::test]
async fn served_decide_refuses_a_skill_parent_cycle_and_an_unknown_parent() {
    use medulla_decision_types::decision::task_plan::skill_plan::{SkillPlanQuery, SkillSubtask};
    let ask_over = |pin: &ComponentDependency| {
        plan_request(
            pin,
            "skill_query",
            TypedValue::SkillPlanQuery(Box::new(SkillPlanQuery {
                output_task_id: "a".into(),
                subtasks: BoundedVec::new(vec![SkillSubtask {
                    id: "a".into(),
                    text: "search documents retrieval".into(),
                    depends_on: BoundedVec::default(),
                }])
                .unwrap(),
                outcome_pins: BoundedVec::default(),
            })),
            &["a"],
        )
    };
    let cycle = Harness::new().await;
    let (pin, _) = publish_route_schema(&cycle);
    publish_skill_with_parent(
        &cycle,
        TENANT,
        51,
        "skill-a",
        "search documents retrieval",
        Some("skill-b"),
    );
    publish_skill_with_parent(
        &cycle,
        TENANT,
        52,
        "skill-b",
        "search documents index",
        Some("skill-a"),
    );
    assert_invalid(&cycle, ask_over(&pin), "SKILL_TREE_CYCLE").await;
    let unknown = Harness::new().await;
    let (pin, _) = publish_route_schema(&unknown);
    publish_skill_with_parent(
        &unknown,
        TENANT,
        53,
        "skill-a",
        "search documents retrieval",
        Some("skill-ghost"),
    );
    assert_invalid(&unknown, ask_over(&pin), "SKILL_PARENT_UNKNOWN").await;
}

/// A branched plan whose steps carry `(duration_ms, priority_weight)`.
fn weighted_proposal(weights: [(u64, u64); 5]) -> serde_json::Value {
    let w = |(duration, weight): (u64, u64)| serde_json::json!({"duration_ms": duration, "priority_weight": weight});
    serde_json::json!({
        "tasks": [
            step("a", &[], w(weights[0])),
            step("b", &[], w(weights[1])),
            step("c", &["a"], w(weights[2])),
            step("d", &["b"], w(weights[3])),
            step("join", &["c", "d"], w(weights[4])),
        ],
        "output_task_id": "join",
        "max_concurrency": 2,
        "max_steps": 8
    })
}

// spec: MEDULLA-DECISION-ENGINE-R134.15
#[tokio::test]
async fn critical_chain_picks_longest_weighted_path() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    // a,c,join = 10*100 + 20*100 + 1*100 = 3100; b,d,join = 5*300 + 10*100 + 100 = 2600.
    let heavy = weighted_proposal([(10, 100), (5, 300), (20, 100), (10, 100), (1, 100)]);
    let batch = decide(&h, request_with(&pin, heavy)).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let plan = record.task_plan().unwrap();
    let chain = plan.critical_chain.as_ref().expect("weights give a chain");
    assert_eq!(chain.critical_chain_step_ids, ["a", "c", "join"]);
    assert_eq!(chain.total_weight_units, 3100);
    assert_eq!(chain.total_weighted_duration_ms, 31);
    assert_eq!(chain.weight_scale, 100);
    assert_eq!(chain.policy_revision, TASK_PLAN_POLICY_REVISION);
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact == "critical_path_input:task=b;duration_ms=5;weight=300"));
    assert!(record.premises.iter().any(|p| p.fact.starts_with(
        "task_plan_critical_chain:steps=a,c,join;total_weighted_duration_ms=31;weight_scale=100;"
    )));
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(record),
        record.record_digest
    );
    // The plan digest covers the chain: the same plan without weights digests differently.
    let plain = decide(
        &h,
        request_with(&pin, {
            let mut p = weighted_proposal([(1, 1); 5]);
            for t in p["tasks"].as_array_mut().unwrap() {
                let o = t.as_object_mut().unwrap();
                o.remove("duration_ms");
                o.remove("priority_weight");
            }
            p
        }),
    )
    .await
    .unwrap();
    let unweighted = plain.records.as_slice()[0].task_plan().unwrap();
    assert!(unweighted.critical_chain.is_none());
    assert_ne!(unweighted.plan_digest, plan.plan_digest);

    // Equal totals choose the lexicographically smallest step sequence:
    // b,d,join = 5*400 + 10*100 + 100 = 3100 ties a,c,join.
    let tie = weighted_proposal([(10, 100), (5, 400), (20, 100), (10, 100), (1, 100)]);
    let batch = decide(&h, request_with(&pin, tie)).await.unwrap();
    let chain = batch.records.as_slice()[0]
        .task_plan()
        .unwrap()
        .critical_chain
        .clone()
        .unwrap();
    assert_eq!(chain.critical_chain_step_ids, ["a", "c", "join"]);
    assert_eq!(chain.total_weight_units, 3100);

    // Refusals name the code.
    let mut missing = weighted_proposal([(10, 100); 5]);
    missing["tasks"][2]
        .as_object_mut()
        .unwrap()
        .remove("priority_weight");
    assert_invalid(
        &h,
        request_with(&pin, missing),
        "CRITICAL_PATH_WEIGHT_MISSING: c must carry",
    )
    .await;
    let mut absent = weighted_proposal([(10, 100); 5]);
    absent["tasks"][3]
        .as_object_mut()
        .unwrap()
        .remove("duration_ms");
    absent["tasks"][3]
        .as_object_mut()
        .unwrap()
        .remove("priority_weight");
    assert_invalid(
        &h,
        request_with(&pin, absent),
        "CRITICAL_PATH_WEIGHT_MISSING: d must carry",
    )
    .await;
    assert_invalid(
        &h,
        request_with(
            &pin,
            weighted_proposal([(10, 100), (5, 400), (0, 100), (10, 100), (1, 100)]),
        ),
        "CRITICAL_PATH_INVALID_DURATION: c has a zero duration",
    )
    .await;
    let max = u32::MAX as u64;
    assert_invalid(
        &h,
        request_with(
            &pin,
            weighted_proposal([(max, max), (1, 1), (max, max), (1, 1), (1, 1)]),
        ),
        "CRITICAL_PATH_OVERFLOW: path total at c",
    )
    .await;

    // A caller of another tenant is refused and never reads this tenant's plan.
    let mut foreign = request_with(
        &pin,
        weighted_proposal([(10, 100), (5, 300), (20, 100), (10, 100), (1, 100)]),
    );
    foreign.tenant_id = "foreign-tenant".into();
    assert!(decide(&h, foreign)
        .await
        .unwrap_err()
        .starts_with("ACCESS_DENIED"));
}

/// A chain of `n` steps, each depending on the one before.
fn chain_proposal(n: usize) -> serde_json::Value {
    let none = serde_json::json!({});
    let ids: Vec<String> = (0..n).map(|i| format!("s{i:02}")).collect();
    let tasks: Vec<serde_json::Value> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let deps: Vec<&str> = if i == 0 {
                vec![]
            } else {
                vec![ids[i - 1].as_str()]
            };
            step(id, &deps, none.clone())
        })
        .collect();
    serde_json::json!({
        "tasks": tasks, "output_task_id": ids[n - 1], "max_concurrency": 1, "max_steps": 16
    })
}

fn checkpoint_of(
    plan: &medulla_decision_types::decision::task_plan::TaskPlan,
    index: u32,
    consumed: u64,
    reserved: u64,
    remaining: u64,
) -> medulla_decision_types::decision::task_plan::HorizonCheckpoint {
    let partition = plan.horizons.as_ref().unwrap();
    medulla_decision_types::decision::task_plan::HorizonCheckpoint {
        plan_digest: partition.plan_digest.clone(),
        horizon_index: index,
        consumed,
        reserved,
        remaining,
        ceiling: partition.credit_ceiling,
        rule: "consumed+reserved+remaining==ceiling".into(),
    }
}

async fn log_checkpoint(
    h: &Harness,
    who: &str,
    tenant: &str,
    plan: &medulla_decision_types::decision::task_plan::TaskPlan,
    checkpoint: medulla_decision_types::decision::task_plan::HorizonCheckpoint,
) -> Result<medulla_decision_types::decision::program_store::RunRecordStored, String> {
    let op = DecisionLogOp::RunRecord {
        tenant_id: tenant.to_string(),
        record: medulla_decision_types::decision::program_store::RunRecord::HorizonCheckpoint {
            plan: Box::new(plan.clone()),
            policy: DecisionPolicyRef::Default,
            checkpoint,
        },
    };
    decode(super::log_tests::log_op(h, who, op).await)
}

// spec: MEDULLA-DECISION-ENGINE-R134.11
#[tokio::test]
async fn long_plan_splits_and_checkpoints_reconcile() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    // A short plan stays one horizon.
    let short = decide(&h, request_with(&pin, chain_proposal(3)))
        .await
        .unwrap();
    let short_plan = short.records.as_slice()[0].task_plan().unwrap();
    let partition = short_plan.horizons.as_ref().unwrap();
    assert_eq!(partition.horizons.len(), 1);
    assert_eq!(partition.horizons[0].step_ids, ["s00", "s01", "s02"]);
    assert_eq!(partition.credit_ceiling, 200_000);

    // A ten-step chain splits at layer boundaries: eight steps, then two.
    let long = decide(&h, request_with(&pin, chain_proposal(10)))
        .await
        .unwrap();
    let record = &long.records.as_slice()[0];
    let plan = record.task_plan().unwrap().clone();
    let partition = plan.horizons.as_ref().unwrap();
    assert_eq!(partition.horizons.len(), 2);
    assert_eq!(partition.horizons[0].step_ids.len(), 8);
    assert_eq!(partition.horizons[1].step_ids, ["s08", "s09"]);
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact == "horizon:index=1;steps=s08,s09"));
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(record),
        record.record_digest
    );

    // Horizon 1 cannot be stored before horizon 0.
    let early = checkpoint_of(&plan, 1, 10, 5, 199_985);
    let err = log_checkpoint(&h, "runner", TENANT, &plan, early.clone())
        .await
        .unwrap_err();
    assert!(err.starts_with("HORIZON_CHECKPOINT_MISSING"), "{err}");

    // A balanced checkpoint is stored and reconciles exactly; a repeat replays.
    let first = checkpoint_of(&plan, 0, 120_000, 30_000, 50_000);
    let stored = log_checkpoint(&h, "runner", TENANT, &plan, first.clone())
        .await
        .unwrap();
    assert!(!stored.replayed);
    let medulla_decision_types::decision::program_store::StoredRun::HorizonCheckpoint {
        checkpoint,
    } = &stored.stored
    else {
        panic!("a checkpoint record");
    };
    assert_eq!(
        checkpoint.consumed + checkpoint.reserved + checkpoint.remaining,
        checkpoint.ceiling
    );
    assert_eq!(checkpoint, &first);
    assert!(
        log_checkpoint(&h, "runner", TENANT, &plan, first.clone())
            .await
            .unwrap()
            .replayed
    );
    // With horizon 0 stored, horizon 1 stores.
    assert!(log_checkpoint(&h, "runner", TENANT, &plan, early)
        .await
        .is_ok());

    // Refusals store nothing and name their code.
    let mut bad_rule = checkpoint_of(&plan, 0, 1, 1, 199_998);
    bad_rule.rule = "consumed==ceiling".into();
    let mut off_by_one = checkpoint_of(&plan, 0, 1, 1, 199_999);
    off_by_one.horizon_index = 0;
    let over = checkpoint_of(&plan, 0, 150_000, 60_000, 0);
    let mut overflow = checkpoint_of(&plan, 0, u64::MAX, 1, 0);
    overflow.ceiling = partition.credit_ceiling;
    let mut stale = checkpoint_of(&plan, 0, 1, 1, 199_998);
    stale.plan_digest = "sha256:other-plan".into();
    let no_horizon = checkpoint_of(&plan, 2, 1, 1, 199_998);
    for (checkpoint, code) in [
        (bad_rule, "HORIZON_BUDGET_RULE_INVALID"),
        (off_by_one, "HORIZON_BUDGET_RULE_INVALID"),
        (over, "HORIZON_OVER_CREDIT"),
        (overflow, "HORIZON_BUDGET_RULE_INVALID"),
        (stale, "HORIZON_PLAN_STALE"),
        (no_horizon, "HORIZON_PLAN_STALE"),
    ] {
        let err = log_checkpoint(&h, "runner", TENANT, &plan, checkpoint)
            .await
            .unwrap_err();
        assert!(err.starts_with(code), "{code}: {err}");
    }
    // A plan whose horizons were altered is stale.
    let mut altered = plan.clone();
    altered.horizons.as_mut().unwrap().horizons[0]
        .step_ids
        .pop();
    let err = log_checkpoint(
        &h,
        "runner",
        TENANT,
        &altered,
        checkpoint_of(&plan, 0, 1, 1, 199_998),
    )
    .await
    .unwrap_err();
    assert!(err.starts_with("HORIZON_PLAN_STALE"), "{err}");

    // A plan with more steps than a horizon holds and no layer boundary refuses.
    // plan_explicit_tasks cannot derive one (every step must feed the output), so
    // the partition's own refusal is exercised on a hand-built flat body.
    let none = serde_json::json!({});
    let tight = medulla_decision_types::decision::task_plan::HorizonKnobs {
        max_steps: 1,
        credit_ceiling: 1_000,
    };
    let one_layer = medulla_decision_types::decision::task_plan::plan_explicit_tasks(
        &serde_json::from_value(serde_json::json!({
            "tasks": [step("only", &[], none.clone())],
            "output_task_id": "only", "max_concurrency": 1, "max_steps": 1
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        medulla_decision_types::decision::task_plan::partition_horizons(&one_layer, &tight).is_ok()
    );
    let mut flat = plan.clone();
    flat.tasks = BoundedVec::new(
        flat.tasks
            .iter()
            .cloned()
            .map(|mut t| {
                t.depends_on = BoundedVec::default();
                t
            })
            .collect(),
    )
    .unwrap();
    let default_knobs = medulla_decision_types::decision::task_plan::HorizonKnobs {
        max_steps: 8,
        credit_ceiling: 200_000,
    };
    let err =
        medulla_decision_types::decision::task_plan::partition_horizons(&flat, &default_knobs)
            .unwrap_err();
    assert!(err.starts_with("HORIZON_UNSPLITTABLE"), "{err}");
    let two = medulla_decision_types::decision::task_plan::plan_explicit_tasks(
        &serde_json::from_value(chain_proposal(2)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        medulla_decision_types::decision::task_plan::partition_horizons(&two, &tight)
            .unwrap()
            .horizons
            .len(),
        2
    );

    // Another tenant's op is refused; it never reads or satisfies this tenant's checkpoints.
    let err = log_checkpoint(&h, "runner", "foreign-tenant", &plan, first)
        .await
        .unwrap_err();
    assert!(err.starts_with("ACCESS_DENIED"), "{err}");
}
