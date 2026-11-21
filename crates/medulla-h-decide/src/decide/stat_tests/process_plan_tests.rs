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

async fn seed_workflow(h: &Harness, workflow_id: &str, version: u32, workflow: &CanonicalWorkflow) {
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
    let properties = serde_json::json!({
        "type": "WorkflowPattern",
        "tenant_id": TENANT,
        "version": version,
        "workflowDigest": workflow.digest,
        "canonical": serde_json::to_string(workflow).unwrap(),
    });
    core.add_node(
        medulla_types::process_import::workflow_node_id(workflow_id, version),
        rmp_serde::to_vec_named(&properties).unwrap(),
    );
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
    seed_workflow(&h, "process:check", 1, &check_workflow()).await;

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
