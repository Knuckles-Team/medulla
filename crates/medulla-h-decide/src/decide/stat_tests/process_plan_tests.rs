//! MEDULLA-CQ-ORCH-R704.5: a served `ProcessPlan` question reads the stored canonical workflow
//! in the graph its `graph` param names, binds each step from the graph, and seals the task
//! plan. Nothing is written.

use super::kernel_family_support::kernel_request;
use super::path_preference_tests::pins;
use super::*;
use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
use medulla_types::process_import::{
    CanonicalFlow, CanonicalStep, CanonicalWorkflow, ProcessSourceSystem, SourceTrace, StepKind,
};

const GRAPH: &str = "kg-workflows";

fn step(id: &str, kind: StepKind, role: Option<&str>, reference: Option<&str>) -> CanonicalStep {
    CanonicalStep {
        id: id.to_string(),
        kind,
        gate: None,
        label: id.to_string(),
        role: role.map(str::to_string),
        reference: reference.map(str::to_string),
    }
}

/// start, a human check by `clerk`, a decision `dmn:risk`, end.
fn check_workflow() -> CanonicalWorkflow {
    let flow = |source: &str, target: &str| CanonicalFlow {
        source: source.to_string(),
        target: target.to_string(),
        condition: None,
        default: false,
    };
    CanonicalWorkflow {
        schema_version: 1,
        schema: "medulla.process.canonical/v1".to_string(),
        name: "check".to_string(),
        steps: vec![
            step("s1", StepKind::Start, None, None),
            step("s2", StepKind::Human, Some("clerk"), None),
            step("s3", StepKind::Decision, None, Some("dmn:risk")),
            step("s4", StepKind::End, None, None),
        ],
        flows: vec![flow("s1", "s2"), flow("s2", "s3"), flow("s3", "s4")],
        digest: "digest".to_string(),
        source: SourceTrace {
            schema_version: 1,
            system: ProcessSourceSystem::Bpmn20,
            model_id: "model".to_string(),
            elements: Default::default(),
        },
        annex: Vec::new(),
        losses: Vec::new(),
    }
}

async fn seed_workflows(h: &Harness, workflows: &[(&str, u32, CanonicalWorkflow)]) {
    let core = {
        let mut guard = h.state.write().await;
        guard
            .registry
            .create_graph(
                GRAPH,
                medulla_protocol_types::protocol::GraphType::Team,
                None,
            )
            .unwrap();
        guard.registry.get(GRAPH).unwrap().core.clone()
    };
    for (workflow_id, version, workflow) in workflows {
        let properties = serde_json::json!({
            "type": "WorkflowPattern",
            "tenant_id": TENANT,
            "version": version,
            "workflowDigest": medulla_types::process_import::stored_workflow_digest(&workflow.digest),
            "canonical": serde_json::to_string(workflow).unwrap(),
        });
        core.add_node(
            medulla_types::process_import::workflow_node_id(workflow_id, *version),
            rmp_serde::to_vec_named(&properties).unwrap(),
        );
    }
    core.mark_dirty();
}

fn plan_request(
    schema: &ComponentDependency,
    workflow_id: &str,
    version: u32,
) -> DecideRequest {
    let mut request = kernel_request(
        schema,
        QuestionKind::ProcessPlan,
        "process_plan",
        TypedValue::Text(
            serde_json::json!({ "workflow_id": workflow_id, "version": version }).to_string(),
        ),
        &["process_plan"],
    );
    request.params = BoundedVec::new(vec![
        TypedParam {
            name: "graph".to_string(),
            value: TypedValue::Text(GRAPH.to_string()),
        },
        TypedParam {
            name: "process_plan".to_string(),
            value: TypedValue::Text(
                serde_json::json!({ "workflow_id": workflow_id, "version": version }).to_string(),
            ),
        },
    ])
    .unwrap();
    request
}

// spec: MEDULLA-CQ-ORCH-R704.5
#[tokio::test]
async fn decide_routes_process_plan() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pins = pins(&h, TENANT);
    seed_workflows(&h, &[("process:check", 1, check_workflow())]).await;

    let batch = decide(&h, plan_request(&pins.0, "process:check", 1))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let plan = record.task_plan().expect("the sealed record holds the plan");
    let ids: Vec<&str> = plan.tasks.iter().map(|t| t.task_id.as_str()).collect();
    assert_eq!(ids, ["s2", "s3"], "start and end add no task");
    assert_eq!(plan.output_task_id, "s3");
    assert_eq!(
        plan.tasks.as_slice()[0].approval.as_ref().unwrap().approver_role,
        "clerk"
    );
    assert_eq!(
        plan.tasks.as_slice()[1].decision.as_ref().unwrap().decision_iri,
        "dmn:risk"
    );

    // The same workflow again gives the same plan digest.
    let again = decide(&h, plan_request(&pins.0, "process:check", 1))
        .await
        .unwrap();
    assert_eq!(
        again.records.as_slice()[0].task_plan().unwrap().plan_digest,
        plan.plan_digest
    );

    // A workflow or version that is not stored is the family's abstention.
    let missing = decide(&h, plan_request(&pins.0, "process:check", 2))
        .await
        .unwrap();
    let outcome = format!("{:?}", missing.records.as_slice()[0].outcome);
    assert!(outcome.contains("WORKFLOW_UNKNOWN"), "{outcome}");
    assert!(missing.records.as_slice()[0].task_plan().is_none());
}

fn flow(source: &str, target: &str) -> CanonicalFlow {
    CanonicalFlow {
        source: source.to_string(),
        target: target.to_string(),
        condition: None,
        default: false,
    }
}

fn workflow_of(steps: Vec<CanonicalStep>, flows: Vec<CanonicalFlow>) -> CanonicalWorkflow {
    CanonicalWorkflow {
        steps,
        flows,
        ..check_workflow()
    }
}

// spec: MEDULLA-CQ-ORCH-R704.6
#[tokio::test]
async fn process_plan_refusals_name_the_step() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pins = pins(&h, TENANT);
    // An agent step no link binds and no tool ranks.
    let unbound = workflow_of(
        vec![
            step("s1", StepKind::Start, None, None),
            step("s2", StepKind::Agent, None, None),
            step("s3", StepKind::End, None, None),
        ],
        vec![flow("s1", "s2"), flow("s2", "s3")],
    );
    // A cycle through an exclusive gate.
    let mut gate = step("s2", StepKind::Branch, None, None);
    gate.gate = Some(medulla_types::process_import::GateKind::Xor);
    let looped = workflow_of(
        vec![
            step("s1", StepKind::Start, None, None),
            gate,
            step("s3", StepKind::Human, Some("clerk"), None),
        ],
        vec![flow("s1", "s2"), flow("s2", "s3"), flow("s3", "s2")],
    );
    // A call to itself, which runs past the depth knob, and a call to nothing.
    let calling = |reference: &str| {
        workflow_of(
            vec![
                step("s1", StepKind::Start, None, None),
                step("s2", StepKind::Call, None, Some(reference)),
                step("s3", StepKind::End, None, None),
            ],
            vec![flow("s1", "s2"), flow("s2", "s3")],
        )
    };
    // More steps than a task plan holds.
    let mut steps = vec![step("s1", StepKind::Start, None, None)];
    let mut flows = Vec::new();
    for n in 2..=67 {
        steps.push(step(&format!("s{n}"), StepKind::Human, Some("clerk"), None));
        flows.push(flow(&format!("s{}", n - 1), &format!("s{n}")));
    }
    steps.push(step("s68", StepKind::End, None, None));
    flows.push(flow("s67", "s68"));
    seed_workflows(
        &h,
        &[
            ("process:unbound", 1, unbound),
            ("process:loop", 1, looped),
            ("process:self", 1, calling("process:self")),
            ("process:orphan", 1, calling("process:none")),
            ("process:large", 1, workflow_of(steps, flows)),
        ],
    )
    .await;

    for (workflow_id, expected) in [
        ("process:unbound", "PROCESS_STEP_UNBOUND"),
        ("process:loop", "PROCESS_PLAN_LOOP"),
        ("process:self", "PROCESS_PLAN_CALL_DEPTH"),
        ("process:orphan", "WORKFLOW_UNKNOWN"),
        ("process:large", "PROCESS_PLAN_TOO_LARGE"),
        ("process:missing", "WORKFLOW_UNKNOWN"),
    ] {
        let batch = decide(&h, plan_request(&pins.0, workflow_id, 1)).await.unwrap();
        let record = &batch.records.as_slice()[0];
        let outcome = format!("{:?}", record.outcome);
        assert!(outcome.contains(expected), "{workflow_id}: {outcome}");
        assert!(record.task_plan().is_none(), "{workflow_id}");
        let named = match workflow_id {
            "process:unbound" => Some("steps=s2"),
            "process:loop" => Some("steps=s2"),
            "process:self" => Some("steps=s2:s2:s2:s2:s2"),
            "process:orphan" => Some("steps=s2"),
            _ => None,
        };
        if let Some(named) = named {
            assert!(outcome.contains(named), "{workflow_id}: {outcome}");
        }
    }
}

// spec: MEDULLA-CQ-ORCH-R704.7
#[tokio::test]
async fn premises_cite_each_binding() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let pins = pins(&h, TENANT);
    seed_workflows(&h, &[("process:check", 1, check_workflow())]).await;
    let batch = decide(&h, plan_request(&pins.0, "process:check", 1))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let facts: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
    for expected in [
        "process_plan_workflow:id=process:check;version=1;workflow_digest=digest",
        "binding:step=s2;target=approval:clerk;source=explicit;score_q32=none",
        "binding:step=s3;target=decision:dmn:risk;source=explicit;score_q32=none",
    ] {
        assert!(facts.contains(&expected), "{expected} in {facts:?}");
    }
    // Every binding came from the graph's own links, so the record is an observation, and the
    // knobs the decision read are cited (R179.5).
    assert_eq!(record.evidence_class, EvidenceClass::Observation);
    assert!(
        facts.iter().any(|f| f.starts_with("knob:process_plan.")),
        "{facts:?}"
    );
}
