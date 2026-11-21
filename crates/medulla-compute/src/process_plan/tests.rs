use medulla_decision_types::decision::process_plan::{BindingSource, BindingTarget, StepBinding};
use medulla_graph_core::graph::GraphView;
use medulla_types::process_import::{
    CanonicalStep, CanonicalWorkflow, ProcessSourceSystem, SourceTrace, StepKind,
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
    add_node(&mut view, WORKFLOW, serde_json::json!({ "type": "WorkflowPattern" }));
    // The catalog tool, the prompt, the decision and the lane with its agent kind.
    add_node(
        &mut view,
        "tool:mail.send",
        serde_json::json!({ "name": "send_mail", "ofConnector": "mail-mcp" }),
    );
    add_node(&mut view, "prompt:triage", serde_json::json!({ "name": "triage" }));
    add_node(&mut view, "decision:limits", serde_json::json!({ "name": "limits" }));
    add_node(&mut view, "kind:worker", serde_json::json!({ "name": "worker" }));
    add_node(&mut view, "lane:ops", serde_json::json!({ "name": "Operations" }));
    add_edge(&mut view, "lane:ops", "kind:worker", "hasAgentKind");

    // Steps: a tool and a prompt on one agent step, a prompt-only agent step, a decision
    // linked by `evaluatesDecision`, a decision named by its reference, two human steps,
    // a wait, a call and the anchors.
    step_node(&mut view, "s1", serde_json::json!({ "toolArguments": "{\"to\":\"{{owner}}\"}" }));
    add_edge(&mut view, &step_node_id(WORKFLOW, "s1"), "tool:mail.send", "usesTool");
    add_edge(&mut view, &step_node_id(WORKFLOW, "s1"), "prompt:triage", "hasPrompt");
    step_node(&mut view, "s2", serde_json::json!({}));
    add_edge(&mut view, &step_node_id(WORKFLOW, "s2"), "prompt:triage", "hasPrompt");
    step_node(&mut view, "s3", serde_json::json!({}));
    add_edge(&mut view, &step_node_id(WORKFLOW, "s3"), "decision:limits", "evaluatesDecision");
    step_node(&mut view, "s4", serde_json::json!({ "approverRole": "controller" }));
    step_node(&mut view, "s5", serde_json::json!({}));
    step_node(&mut view, "s6", serde_json::json!({}));
    step_node(&mut view, "s7", serde_json::json!({}));
    step_node(&mut view, "s8", serde_json::json!({}));
    step_node(&mut view, "s9", serde_json::json!({}));
    add_edge(&mut view, WORKFLOW, &step_node_id(WORKFLOW, "s9"), "hasStep");

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
        "a" => vec![assigned("send_mail", 1, 900_000), assigned("find_mail", 2, 800_000)],
        _ => vec![assigned("find_mail", 1, 500_000)],
    };
    let mut view = GraphView::default();
    add_node(&mut view, "tool:send", serde_json::json!({ "name": "send_mail", "ofConnector": "mail-mcp" }));
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
    let bound = ranked_bindings(&steps, &explicit, DEFAULT_MIN_TOOL_SCORE_MICROS, &relevance, &contract_of);
    assert_eq!(bound.len(), 1, "only a clears the floor; b is below it, c is bound, d is human");
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
    assert_eq!(ranked_bindings(&steps, &explicit, 400_000, &relevance, &contract_of).len(),
        2
    );
    // The contract is also found through a `hasContract` edge, and only for the serving connector.
    add_node(&mut view, "tool:read", serde_json::json!({ "name": "read_mail", "ofConnector": "mail-mcp" }));
    add_node(&mut view, "contract:read", serde_json::json!({ "name": "read" }));
    add_edge(&mut view, "tool:read", "contract:read", "hasContract");
    assert_eq!(
        registered_contract(&view, "mail-mcp", "read_mail").as_deref(),
        Some("contract:read")
    );
    assert_eq!(registered_contract(&view, "other-mcp", "read_mail"), None);
    assert_eq!(registered_contract(&view, "mail-mcp", "send_mail"), None);
}
