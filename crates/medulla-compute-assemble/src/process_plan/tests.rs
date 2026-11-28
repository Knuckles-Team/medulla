use medulla_graph_core::graph::GraphView;
use medulla_types::process_import::{
    CanonicalStep, CanonicalWorkflow, ProcessSourceSystem, SourceTrace, StepKind,
};
use {
    medulla_decision_statistical_types::decision::process_plan::BindingSource,
    medulla_decision_statistical_types::decision::process_plan::BindingTarget,
    medulla_decision_statistical_types::decision::process_plan::StepBinding,
};

use super::bind::{explicit_bindings, registered_contract, resolve_bindings, step_node_id};
use super::rank::ranked_bindings;
use medulla_decision_types::decision::tool_relevance::AssignedTool;

/// The default of `knob process_plan.min_tool_score`, in millionths.
const DEFAULT_MIN_TOOL_SCORE_MICROS: u64 = 600_000;

const WORKFLOW: &str = "workflow:invoice:v1";

fn blob(value: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).unwrap()
}

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

fn workflow(steps: Vec<CanonicalStep>) -> CanonicalWorkflow {
    CanonicalWorkflow {
        schema_version: 1,
        schema: "medulla.process.canonical/v1".to_string(),
        name: "invoice".to_string(),
        steps,
        flows: Vec::new(),
        digest: "d".to_string(),
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

fn add_node(view: &mut GraphView, id: &str, properties: serde_json::Value) {
    view.overlay_add_node(id.to_string(), blob(properties));
}

fn add_edge(view: &mut GraphView, source: &str, target: &str, relationship: &str) {
    assert!(view.overlay_add_edge(
        source.to_string(),
        target.to_string(),
        blob(serde_json::json!({ "relationship": relationship })),
    ));
}

fn step_node(view: &mut GraphView, id: &str, extra: serde_json::Value) {
    let mut properties = serde_json::json!({ "type": "WorkflowStep", "stepId": id });
    if let (Some(base), Some(more)) = (properties.as_object_mut(), extra.as_object()) {
        base.extend(more.clone());
    }
    add_node(view, &step_node_id(WORKFLOW, id), properties);
}

// spec: MEDULLA-CQ-ORCH-R704.2
#[test]
fn explicit_bindings_follow_graph_links() {
    let mut view = GraphView::default();
    add_node(
        &mut view,
        WORKFLOW,
        serde_json::json!({ "type": "WorkflowPattern" }),
    );
    // The catalog tool, the prompt, the decision and the lane with its agent kind.
    add_node(
        &mut view,
        "tool:mail.send",
        serde_json::json!({ "name": "send_mail", "ofConnector": "mail-mcp" }),
    );
    add_node(
        &mut view,
        "prompt:triage",
        serde_json::json!({ "name": "triage" }),
    );
    add_node(
        &mut view,
        "decision:limits",
        serde_json::json!({ "name": "limits" }),
    );
    add_node(
        &mut view,
        "kind:worker",
        serde_json::json!({ "name": "worker" }),
    );
    add_node(
        &mut view,
        "lane:ops",
        serde_json::json!({ "name": "Operations" }),
    );
    add_edge(&mut view, "lane:ops", "kind:worker", "hasAgentKind");

    // Steps: a tool and a prompt on one agent step, a prompt-only agent step, a decision
    // linked by `evaluatesDecision`, a decision named by its reference, two human steps,
    // a wait, a call and the anchors.
    step_node(
        &mut view,
        "s1",
        serde_json::json!({ "toolArguments": "{\"to\":\"{{owner}}\"}" }),
    );
    add_edge(
        &mut view,
        &step_node_id(WORKFLOW, "s1"),
        "tool:mail.send",
        "usesTool",
    );
    add_edge(
        &mut view,
        &step_node_id(WORKFLOW, "s1"),
        "prompt:triage",
        "hasPrompt",
    );
    step_node(&mut view, "s2", serde_json::json!({}));
    add_edge(
        &mut view,
        &step_node_id(WORKFLOW, "s2"),
        "prompt:triage",
        "hasPrompt",
    );
    step_node(&mut view, "s3", serde_json::json!({}));
    add_edge(
        &mut view,
        &step_node_id(WORKFLOW, "s3"),
        "decision:limits",
        "evaluatesDecision",
    );
    step_node(
        &mut view,
        "s4",
        serde_json::json!({ "approverRole": "controller" }),
    );
    step_node(&mut view, "s5", serde_json::json!({}));
    step_node(&mut view, "s6", serde_json::json!({}));
    step_node(&mut view, "s7", serde_json::json!({}));
    step_node(&mut view, "s8", serde_json::json!({}));
    step_node(&mut view, "s9", serde_json::json!({}));
    add_edge(
        &mut view,
        WORKFLOW,
        &step_node_id(WORKFLOW, "s9"),
        "hasStep",
    );

    let workflow = workflow(vec![
        step("s0", StepKind::Start, None, None),
        step("s1", StepKind::Agent, Some("Operations"), None),
        step("s2", StepKind::Agent, Some("Operations"), None),
        step("s3", StepKind::Decision, None, None),
        step("s4", StepKind::Human, Some("Clerk"), None),
        step("s5", StepKind::Human, Some("Clerk"), None),
        step("s6", StepKind::Wait, None, Some("payment-received")),
        step("s7", StepKind::Call, None, Some("workflow:payment")),
        step("s8", StepKind::Decision, None, Some("decision:fees")),
        step("s9", StepKind::End, None, None),
    ]);
    let bindings = explicit_bindings(&view, WORKFLOW, &workflow);
    let target = |id: &str| {
        &bindings
            .iter()
            .find(|binding| binding.step_id == id)
            .unwrap_or_else(|| panic!("{id} is bound"))
            .target
    };
    assert_eq!(
        target("s1"),
        &BindingTarget::Tool {
            server: "mail-mcp".to_string(),
            tool: "send_mail".to_string(),
            contract_iri: "tool:mail.send".to_string(),
        }
    );
    assert_eq!(
        target("s2"),
        &BindingTarget::Prompt {
            prompt_iri: "prompt:triage".to_string(),
            agent_kind: "worker".to_string(),
        }
    );
    assert_eq!(
        target("s3"),
        &BindingTarget::Decision {
            decision_iri: "decision:limits".to_string()
        }
    );
    assert_eq!(
        target("s4"),
        &BindingTarget::Approval {
            approver_role: "controller".to_string()
        }
    );
    // Without `approverRole` the step's role is the approver.
    assert_eq!(
        target("s5"),
        &BindingTarget::Approval {
            approver_role: "Clerk".to_string()
        }
    );
    assert_eq!(
        target("s6"),
        &BindingTarget::Approval {
            approver_role: "signal:payment-received".to_string()
        }
    );
    assert_eq!(
        target("s8"),
        &BindingTarget::Decision {
            decision_iri: "decision:fees".to_string()
        }
    );
    // A call inlines its workflow and the anchors run nothing: none is bound.
    for id in ["s0", "s7", "s9"] {
        assert!(bindings.iter().all(|binding| binding.step_id != id), "{id}");
    }
    assert!(bindings
        .iter()
        .all(|binding| binding.source == BindingSource::Explicit));
    let _: &[StepBinding] = &bindings;

    // The prompt the tool step also names is recorded as a premise, with its arguments.
    let resolved = resolve_bindings(&view, WORKFLOW, &workflow);
    assert_eq!(resolved.bindings, bindings);
    assert!(resolved
        .premises
        .contains(&"binding_superseded:step=s1;kept=tool;dropped=prompt".to_string()));
    assert!(resolved
        .premises
        .iter()
        .any(|premise| premise.starts_with("tool_arguments:step=s1;")));
    // Equal views give equal bindings.
    assert_eq!(explicit_bindings(&view, WORKFLOW, &workflow), bindings);
}

fn assigned(tool: &str, rank: u32, relevance_micros: i64) -> AssignedTool {
    AssignedTool {
        tool_id: tool.to_string(),
        server_id: "mail-mcp".to_string(),
        lexical_rank: Some(rank),
        embedding_rank: None,
        fused_score_q32: 0,
        relevance_q32: (relevance_micros << 32) / 1_000_000,
        rank,
    }
}

// proves R704.3: Ranked fallback for unbound steps
// spec: MEDULLA-CQ-ORCH-R704.3
#[test]
fn ranked_fallback_respects_min_score() {
    let steps = vec![
        step("a", StepKind::Agent, Some("clerk"), None),
        step("b", StepKind::Agent, None, None),
        step("c", StepKind::Agent, None, None),
        step("d", StepKind::Human, None, None),
    ];
    let explicit = vec![StepBinding {
        step_id: "c".to_string(),
        target: BindingTarget::Approval {
            approver_role: "x".to_string(),
        },
        source: BindingSource::Explicit,
    }];
    // Step a: the top tool by rank scores 0.8; step b: the best tool scores 0.5, below 0.6.
    let relevance = |label: &str, _lane: &str| match label {
        // send_mail ranks first but has no registered contract, so it is not ranked.
        "a" => vec![
            assigned("send_mail", 1, 900_000),
            assigned("find_mail", 2, 800_000),
        ],
        _ => vec![assigned("find_mail", 1, 500_000)],
    };
    let mut view = GraphView::default();
    add_node(
        &mut view,
        "tool:send",
        serde_json::json!({ "name": "send_mail", "ofConnector": "mail-mcp" }),
    );
    add_node(
        &mut view,
        "tool:find",
        serde_json::json!({ "name": "find_mail", "ofConnector": "mail-mcp", "contractIri": "urn:contract:find-mail" }),
    );
    let contract_of = |server: &str, tool: &str| registered_contract(&view, server, tool);
    let steps: Vec<CanonicalStep> = steps
        .into_iter()
        .map(|mut s| {
            s.label = s.id.clone();
            s
        })
        .collect();
    let bound = ranked_bindings(
        &steps,
        &explicit,
        DEFAULT_MIN_TOOL_SCORE_MICROS,
        &relevance,
        &contract_of,
    );
    assert_eq!(
        bound.len(),
        1,
        "only a clears the floor; b is below it, c is bound, d is human"
    );
    assert_eq!(bound[0].step_id, "a");
    assert_eq!(
        bound[0].target,
        BindingTarget::Tool {
            server: "mail-mcp".to_string(),
            tool: "find_mail".to_string(),
            contract_iri: "urn:contract:find-mail".to_string()
        }
    );
    assert!(matches!(bound[0].source, BindingSource::Ranked { score_q32 } if score_q32 > 0));
    // A lower floor binds b as well.
    assert_eq!(
        ranked_bindings(&steps, &explicit, 400_000, &relevance, &contract_of).len(),
        2
    );
    // The contract is also found through a `hasContract` edge, and only for the serving connector.
    add_node(
        &mut view,
        "tool:read",
        serde_json::json!({ "name": "read_mail", "ofConnector": "mail-mcp" }),
    );
    add_node(
        &mut view,
        "contract:read",
        serde_json::json!({ "name": "read" }),
    );
    add_edge(&mut view, "tool:read", "contract:read", "hasContract");
    assert_eq!(
        registered_contract(&view, "mail-mcp", "read_mail").as_deref(),
        Some("contract:read")
    );
    assert_eq!(registered_contract(&view, "other-mcp", "read_mail"), None);
    assert_eq!(registered_contract(&view, "mail-mcp", "send_mail"), None);
}

fn flow(
    source: &str,
    target: &str,
    condition: Option<&str>,
    default: bool,
) -> medulla_types::process_import::CanonicalFlow {
    medulla_types::process_import::CanonicalFlow {
        source: source.to_string(),
        target: target.to_string(),
        condition: condition.map(str::to_string),
        default,
    }
}

fn gated(id: &str, kind: StepKind, gate: medulla_types::process_import::GateKind) -> CanonicalStep {
    CanonicalStep {
        gate: Some(gate),
        ..step(id, kind, None, None)
    }
}

fn bound(step_id: &str, target: BindingTarget) -> StepBinding {
    StepBinding {
        step_id: step_id.to_string(),
        target,
        source: BindingSource::Explicit,
    }
}

fn no_callee(_: &str) -> Option<super::plan::Callee> {
    None
}

/// The sealed plan of a workflow: its proposal through `plan_explicit_tasks`.
fn sealed(
    workflow: &CanonicalWorkflow,
    bindings: &[StepBinding],
    callee: super::plan::CalleeOf<'_>,
    max_call_depth: u32,
) -> Result<
    medulla_decision_program_types::decision::task_plan::TaskPlan,
    medulla_decision_statistical_types::decision::process_plan::ProcessPlanRefusal,
> {
    let drafted =
        super::plan::plan_from_workflow(workflow, bindings, callee, max_call_depth, 86_400)?;
    medulla_decision_program_types::decision::task_plan::plan_explicit_tasks(&drafted.proposal)
        .map_err(|message| super::plan::engine_refusal(&message))
}

/// The invoice process: a start, a human check, a decision, an XOR split with two arms (one
/// through a human approval), an XOR join, an agent post, an AND fork of two agent steps, an
/// AND join and an end.
fn invoice() -> (CanonicalWorkflow, Vec<StepBinding>) {
    use medulla_types::process_import::GateKind::{And, Xor};
    let mut wf = workflow(vec![
        step("s1", StepKind::Start, None, None),
        step("s2", StepKind::Human, Some("clerk"), None),
        step("s3", StepKind::Decision, None, Some("dmn:risk")),
        gated("s4", StepKind::Branch, Xor),
        step("s5", StepKind::Human, Some("manager"), None),
        gated("s6", StepKind::Join, Xor),
        step("s7", StepKind::Agent, None, None),
        gated("s8", StepKind::Fork, And),
        step("s9", StepKind::Agent, None, None),
        step("s10", StepKind::Agent, None, None),
        gated("s11", StepKind::Join, And),
        step("s12", StepKind::End, None, None),
    ]);
    wf.flows = vec![
        flow("s1", "s2", None, false),
        flow("s2", "s3", None, false),
        flow("s3", "s4", None, false),
        flow("s4", "s5", Some("amount > 1000"), false),
        flow("s4", "s6", None, true),
        flow("s5", "s6", None, false),
        flow("s6", "s7", None, false),
        flow("s7", "s8", None, false),
        flow("s8", "s9", None, false),
        flow("s8", "s10", None, false),
        flow("s9", "s11", None, false),
        flow("s10", "s11", None, false),
        flow("s11", "s12", None, false),
    ];
    let prompt = |iri: &str| BindingTarget::Prompt {
        prompt_iri: iri.to_string(),
        agent_kind: "writer".to_string(),
    };
    let bindings = vec![
        bound(
            "s2",
            BindingTarget::Approval {
                approver_role: "clerk".to_string(),
            },
        ),
        bound(
            "s3",
            BindingTarget::Decision {
                decision_iri: "dmn:risk".to_string(),
            },
        ),
        bound(
            "s5",
            BindingTarget::Approval {
                approver_role: "manager".to_string(),
            },
        ),
        bound(
            "s7",
            BindingTarget::Tool {
                server: "ledger".to_string(),
                tool: "post".to_string(),
                contract_iri: "urn:contract:post".to_string(),
            },
        ),
        bound("s9", prompt("urn:prompt:a")),
        bound("s10", prompt("urn:prompt:b")),
    ];
    (wf, bindings)
}

// spec: MEDULLA-CQ-ORCH-R704.4
#[test]
fn workflow_maps_to_canonical_plan() {
    use {
        medulla_decision_program_types::decision::task_plan::BranchCondition,
        medulla_decision_program_types::decision::task_plan::BranchKind,
    };
    let (wf, bindings) = invoice();
    let plan = sealed(&wf, &bindings, &no_callee, 4).unwrap();
    let again = sealed(&wf, &bindings, &no_callee, 4).unwrap();
    assert_eq!(
        plan.plan_digest, again.plan_digest,
        "equal digest on a second run"
    );
    // start and end add no task.
    assert_eq!(plan.tasks.len(), 10);
    assert_eq!(plan.output_task_id, "s11");
    let task = |id: &str| plan.tasks.iter().find(|t| t.task_id == id).unwrap();
    // A tool step, a prompt step, a decision step and two approval waits.
    assert_eq!(task("s7").call.as_ref().unwrap().tool, "post");
    assert!(task("s9").call.is_none() && task("s9").branch.is_none());
    assert_eq!(
        task("s3").decision.as_ref().unwrap().decision_iri,
        "dmn:risk"
    );
    assert_eq!(task("s2").approval.as_ref().unwrap().approver_role, "clerk");
    assert_eq!(
        task("s5").approval.as_ref().unwrap().approver_role,
        "manager"
    );
    assert_eq!(task("s2").depends_on.len(), 0);
    // The XOR split takes one arm: the conditioned arm is a typed decision condition whose
    // IRI carries the flow condition text, and the default flow is the default arm.
    let branch = task("s4").branch.as_ref().unwrap();
    assert_eq!(branch.kind, BranchKind::Exclusive);
    assert_eq!(branch.default_arm.as_deref(), Some("s6"));
    assert_eq!(branch.arms.len(), 1);
    match branch.arms.as_slice()[0].condition.as_ref().unwrap() {
        BranchCondition::Decision(d) => {
            assert_eq!(d.decision_iri, "flowcondition:d:f4");
        }
        other => panic!("{other:?}"),
    }
    // The joins find the nearest dominating split of their gate.
    assert_eq!(task("s6").join.as_ref().unwrap().branch_task_id, "s4");
    assert_eq!(task("s11").join.as_ref().unwrap().branch_task_id, "s8");
    assert_eq!(
        task("s8").branch.as_ref().unwrap().kind,
        BranchKind::Parallel
    );
    // The flow condition is also returned as a decision question for the caller to register.
    let drafted = super::plan::plan_from_workflow(&wf, &bindings, &no_callee, 4, 86_400).unwrap();
    assert_eq!(
        drafted.questions,
        vec![super::plan::ConditionQuestion {
            decision_iri: "flowcondition:d:f4".to_string(),
            text: "amount > 1000".to_string(),
        }]
    );
}

// spec: MEDULLA-CQ-ORCH-R704.4
#[test]
fn plan_refuses_loops_unbound_steps_and_deep_calls() {
    use {
        medulla_decision_statistical_types::decision::process_plan::PROCESS_PLAN_CALL_DEPTH,
        medulla_decision_statistical_types::decision::process_plan::PROCESS_PLAN_LOOP,
        medulla_decision_statistical_types::decision::process_plan::PROCESS_STEP_UNBOUND,
        medulla_decision_statistical_types::decision::process_plan::WORKFLOW_UNKNOWN,
    };
    let (wf, bindings) = invoice();
    // An agent step with no binding is named.
    let partial: Vec<StepBinding> = bindings
        .iter()
        .filter(|b| b.step_id != "s9")
        .cloned()
        .collect();
    let refused =
        super::plan::plan_from_workflow(&wf, &partial, &no_callee, 4, 86_400).unwrap_err();
    assert_eq!(refused.code, PROCESS_STEP_UNBOUND);
    assert_eq!(refused.steps, vec!["s9".to_string()]);
    // A flow back into the XOR split is a loop.
    let mut looped = wf.clone();
    looped.flows.push(flow("s7", "s4", None, false));
    let refused =
        super::plan::plan_from_workflow(&looped, &bindings, &no_callee, 4, 86_400).unwrap_err();
    assert_eq!(refused.code, PROCESS_PLAN_LOOP);
    assert!(
        refused.steps.contains(&"s4".to_string()),
        "{:?}",
        refused.steps
    );
    // A call step: unknown callee, then inlined, then too deep.
    let mut caller = workflow(vec![
        step("s1", StepKind::Start, None, None),
        step("s2", StepKind::Call, None, Some("workflow:inner")),
        step("s3", StepKind::Agent, None, None),
        step("s4", StepKind::End, None, None),
    ]);
    caller.flows = vec![
        flow("s1", "s2", None, false),
        flow("s2", "s3", None, false),
        flow("s3", "s4", None, false),
    ];
    let post = bound(
        "s3",
        BindingTarget::Tool {
            server: "ledger".to_string(),
            tool: "post".to_string(),
            contract_iri: "urn:contract:post".to_string(),
        },
    );
    let refused = super::plan::plan_from_workflow(
        &caller,
        std::slice::from_ref(&post),
        &no_callee,
        4,
        86_400,
    )
    .unwrap_err();
    assert_eq!(refused.code, WORKFLOW_UNKNOWN);
    assert_eq!(refused.steps, vec!["s2".to_string()]);
    let mut inner = workflow(vec![
        step("s1", StepKind::Start, None, None),
        step("s2", StepKind::Human, Some("clerk"), None),
        step("s3", StepKind::End, None, None),
    ]);
    inner.flows = vec![flow("s1", "s2", None, false), flow("s2", "s3", None, false)];
    let callee = |reference: &str| {
        (reference == "workflow:inner").then(|| super::plan::Callee {
            workflow: inner.clone(),
            bindings: vec![bound(
                "s2",
                BindingTarget::Approval {
                    approver_role: "clerk".to_string(),
                },
            )],
        })
    };
    let plan = sealed(&caller, std::slice::from_ref(&post), &callee, 4).unwrap();
    let ids: Vec<&str> = plan.tasks.iter().map(|t| t.task_id.as_str()).collect();
    assert_eq!(ids, vec!["s2:s2", "s3"]);
    assert_eq!(
        plan.tasks.as_slice()[1].depends_on.as_slice().to_vec(),
        vec!["s2:s2".to_string()]
    );
    let refused =
        super::plan::plan_from_workflow(&caller, &[post], &callee, 0, 86_400).unwrap_err();
    assert_eq!(refused.code, PROCESS_PLAN_CALL_DEPTH);
    assert_eq!(refused.steps, vec!["s2".to_string()]);
}

// spec: MEDULLA-CQ-ORCH-R704.4
#[test]
fn several_boundaries_on_one_activity_split_normal_and_trigger_arms() {
    use {
        medulla_decision_program_types::decision::task_plan::BranchCondition,
        medulla_decision_program_types::decision::task_plan::BranchKind,
    };
    let mut wf = workflow(vec![
        step("s1", StepKind::Start, None, None),
        step("s2", StepKind::Agent, None, None),
        step("s3", StepKind::Boundary, None, Some("timer")),
        step("s4", StepKind::Boundary, None, Some("error")),
        step("s5", StepKind::Agent, None, None),
        step("s6", StepKind::Agent, None, None),
        step("s7", StepKind::Agent, None, None),
        step("s8", StepKind::End, None, None),
    ]);
    wf.flows = vec![
        flow("s1", "s2", None, false),
        flow("s2", "s3", None, false),
        flow("s2", "s4", None, false),
        flow("s2", "s5", None, false),
        flow("s3", "s6", None, false),
        flow("s4", "s7", None, false),
        flow("s5", "s8", None, false),
        flow("s6", "s8", None, false),
        flow("s7", "s8", None, false),
    ];
    let prompt = |step_id: &str| {
        bound(
            step_id,
            BindingTarget::Prompt {
                prompt_iri: format!("urn:prompt:{step_id}"),
                agent_kind: "writer".to_string(),
            },
        )
    };
    let bindings: Vec<StepBinding> = ["s2", "s5", "s6", "s7"].into_iter().map(prompt).collect();
    let drafted = super::plan::plan_from_workflow(&wf, &bindings, &no_callee, 4, 86_400).unwrap();
    let task = |id: &str| {
        drafted
            .proposal
            .tasks
            .iter()
            .find(|t| t.task_id == id)
            .unwrap_or_else(|| panic!("task {id}"))
    };
    // The first boundary by id carries the activity's normal arm and its own trigger arm.
    let first = task("s3").branch.as_ref().unwrap();
    assert_eq!(first.kind, BranchKind::Exclusive);
    let arms = first.arms.as_slice();
    assert_eq!(arms.len(), 2);
    assert_eq!(arms[0].target_task_id, "s5");
    assert!(arms[0].condition.is_none(), "the normal arm has no trigger");
    assert_eq!(arms[1].target_task_id, "s6");
    match arms[1].condition.as_ref().unwrap() {
        BranchCondition::Decision(d) => assert_eq!(d.decision_iri, "flowcondition:d:s3:trigger"),
        other => panic!("{other:?}"),
    }
    // The later boundary is a trigger-only branch.
    let later = task("s4").branch.as_ref().unwrap();
    let arms = later.arms.as_slice();
    assert_eq!(arms.len(), 1);
    assert_eq!(arms[0].target_task_id, "s7");
    match arms[0].condition.as_ref().unwrap() {
        BranchCondition::Decision(d) => assert_eq!(d.decision_iri, "flowcondition:d:s4:trigger"),
        other => panic!("{other:?}"),
    }
}

// spec: MEDULLA-CQ-ORCH-R704.9
#[test]
fn plan_code_names_no_source_format() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![
        root.join("src/process_plan.rs"),
        root.join("../medulla-h-decide-learning/src/decide/family/process_plan.rs"),
    ];
    for entry in std::fs::read_dir(root.join("src/process_plan")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name != "tests.rs") {
            files.push(path);
        }
    }
    assert!(files.len() >= 5, "{files:?}");
    for file in files {
        let source = std::fs::read_to_string(&file).unwrap();
        for format in ["Bpmn", "Dmn", "Epc", "Aris"] {
            assert!(
                !source.contains(format),
                "{} names the source format {format}",
                file.display()
            );
        }
    }
}

// spec: MEDULLA-CQ-ORCH-R704.9
#[test]
fn equal_canonical_digest_gives_equal_plan_digest() {
    let (bpmn, bindings) = invoice();
    // The same canonical steps, flows and digest from another source system, with another
    // model id and an annex the digest does not cover.
    let mut epc = bpmn.clone();
    epc.source.system = ProcessSourceSystem::ArisEpc;
    epc.source.model_id = "aris-model".to_string();
    epc.annex = vec![medulla_types::process_import::AnnexEntry {
        schema_version: 1,
        after: Some("s2".to_string()),
        before: None,
        construct: "epc-attachment".to_string(),
        element_id: "attachment-1".to_string(),
        label: "Policy document".to_string(),
        type_num: None,
    }];
    assert_eq!(bpmn.digest, epc.digest);
    let first = sealed(&bpmn, &bindings, &no_callee, 4).unwrap();
    let second = sealed(&epc, &bindings, &no_callee, 4).unwrap();
    assert_eq!(first.plan_digest, second.plan_digest);
    assert_eq!(first.input_digest, second.input_digest);
}

// spec: MEDULLA-CQ-ORCH-R704.4
#[test]
fn approval_timeout_is_the_knob_value() {
    let (wf, bindings) = invoice();
    let drafted = super::plan::plan_from_workflow(&wf, &bindings, &no_callee, 4, 3_600).unwrap();
    let approvals: Vec<u64> = drafted
        .proposal
        .tasks
        .iter()
        .filter_map(|t| t.approval.as_ref().map(|a| a.timeout_s))
        .collect();
    assert_eq!(approvals, vec![3_600, 3_600]);
}
