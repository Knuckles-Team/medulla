//! Served acceptance of the orchestration vocabulary consumed by assembly
//! (MEDULLA-ORCH-VOCAB-R007): the `:hasStep` steps of a request's tasks reach
//! the decision inputs in step order, and a `:precedes` cycle refuses.

use super::assemble_tests::{assemble, harness, request, seed, Harness};
use medulla_types::ontology_iri;

const CORE: &str = ontology_iri!("core#");
const TASK: &str = "eg:task/research";

/// Add `(subject, predicate, object)` edges of the core vocabulary to the
/// request graph, creating the graph when it does not exist yet.
async fn state_edges(h: &Harness, edges: &[(&str, &str, &str)]) {
    let mut state = h.state.write().await;
    if state.registry.get("default").is_none() {
        state
            .registry
            .create_graph(
                "default",
                medulla_protocol_types::protocol::GraphType::Global,
                None,
            )
            .expect("create the request graph");
    }
    let core = &state.registry.get("default").expect("request graph").core;
    let empty = rmp_serde::to_vec_named(&serde_json::json!({})).unwrap();
    for (subject, predicate, object) in edges {
        for node in [subject, object] {
            core.add_node(format!("<{node}>"), empty.clone());
        }
        core.add_edge(
            format!("<{subject}>"),
            format!("<{object}>"),
            rmp_serde::to_vec_named(&serde_json::json!({
                "relationship": format!("{CORE}{predicate}"),
            }))
            .unwrap(),
        )
        .expect("add the step edge");
    }
}

fn asking(tasks: &[&str]) -> medulla_decision_types::decision::AssemblyRequest {
    let mut asked = request(Vec::new());
    asked.requirements.tasks =
        medulla_types::contract::BoundedVec::new(tasks.iter().map(|t| t.to_string()).collect())
            .unwrap();
    asked
}

// spec: MEDULLA-ORCH-VOCAB-R007.1
/// Verifies MEDULLA-ORCH-VOCAB-R007.1: the steps of a task, in `:precedes`
/// order, are recorded in the inputs with their capabilities.
#[tokio::test]
async fn assemble_expands_has_step_into_tasks() {
    let h = harness().await;
    seed(&h.store, None);
    state_edges(
        &h,
        &[
            (TASK, "hasStep", "urn:step:report"),
            (TASK, "hasStep", "urn:step:gather"),
            ("urn:step:gather", "precedes", "urn:step:report"),
            (
                "urn:step:gather",
                "requiresCapability",
                "eg:capability/retrieval",
            ),
        ],
    )
    .await;
    let result = assemble(&h, asking(&[TASK])).await.expect("assembles");
    let steps: Vec<(&str, &str, u16, Vec<&str>)> = result
        .record
        .inputs
        .steps
        .iter()
        .map(|s| {
            (
                s.task.as_str(),
                s.step.as_str(),
                s.position,
                s.capabilities.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        steps,
        vec![
            (TASK, "urn:step:gather", 0, vec!["eg:capability/retrieval"]),
            (TASK, "urn:step:report", 1, vec![]),
        ]
    );
    // A request with no task step keeps empty steps.
    let plain = assemble(&h, asking(&[])).await.expect("assembles");
    assert!(plain.record.inputs.steps.is_empty());
}

// spec: MEDULLA-ORCH-VOCAB-R007.1
/// Verifies MEDULLA-ORCH-VOCAB-R007.1: a `:precedes` cycle refuses
/// `STEP_ORDER_CYCLE` and nothing is answered.
#[tokio::test]
async fn step_cycle_refuses_step_order_cycle() {
    let h = harness().await;
    seed(&h.store, None);
    state_edges(
        &h,
        &[
            (TASK, "hasStep", "urn:step:a"),
            (TASK, "hasStep", "urn:step:b"),
            ("urn:step:a", "precedes", "urn:step:b"),
            ("urn:step:b", "precedes", "urn:step:a"),
        ],
    )
    .await;
    let error = assemble(&h, asking(&[TASK])).await.expect_err("refuses");
    assert!(error.starts_with("STEP_ORDER_CYCLE: "), "{error}");
}

// spec: MEDULLA-ORCH-VOCAB-R007.4
/// Verifies MEDULLA-ORCH-VOCAB-R007.4: a slot that requires an agent kind excludes an
/// executor-only candidate, accepts a verifier, and refuses `NO_CANDIDATE_OF_KIND` when no
/// visible candidate carries the kind.
#[tokio::test]
async fn assemble_enforces_slot_agent_kind() {
    use super::super::candidates::{filter_by_slot_kind, library_candidates, NO_CANDIDATE_OF_KIND};
    use super::assemble_tests::publish_with;
    use medulla_agent_types::agent_component::{AgentComponentKind, A2A_AGENT_KIND_ATTRIBUTE};
    use medulla_agent_types::agent_fixtures::topology::slot;
    use medulla_agent_types::agent_topology::SlotRole;

    let verifier = format!("{CORE}agentKindVerifier");
    let executor = format!("{CORE}agentKindExecutor");
    let h = harness().await;
    let store = &h.store;
    for (nonce, id, kind) in [(1u8, "agent-exec", &executor), (2, "agent-ver", &verifier)] {
        publish_with(
            store,
            nonce,
            (id, AgentComponentKind::Tool),
            None,
            |draft| {
                draft
                    .attributes
                    .insert(A2A_AGENT_KIND_ATTRIBUTE.to_string(), kind.clone());
            },
        );
    }
    publish_with(
        store,
        3,
        ("agent-plain", AgentComponentKind::Tool),
        None,
        |_| {},
    );
    let read = library_candidates(store, super::assemble_tests::TENANT, &request(Vec::new()).candidates)
        .expect("candidates read");
    let mut verifier_slot = slot("check", SlotRole::Verifier, (1, 1), Some(100));
    verifier_slot.required_agent_kind = Some(verifier.clone());
    let kept = filter_by_slot_kind(&[verifier_slot.clone()], &read.entries, &read.views)
        .expect("a verifier is visible");
    assert_eq!(
        kept.iter().map(|view| view.id.as_str()).collect::<Vec<_>>(),
        vec!["agent-ver"]
    );
    // An unconstrained slot keeps current behaviour: every candidate stays.
    let open = slot("work", SlotRole::Child, (1, 1), Some(100));
    let all = filter_by_slot_kind(&[open], &read.entries, &read.views).expect("unconstrained");
    assert_eq!(all.len(), read.views.len());
    // No visible candidate of the kind abstains by name.
    let mut missing = verifier_slot;
    missing.required_agent_kind = Some(format!("{CORE}agentKindPlanner"));
    let refusal = filter_by_slot_kind(&[missing], &read.entries, &read.views)
        .expect_err("no planner is visible");
    assert!(refusal.to_string().starts_with(NO_CANDIDATE_OF_KIND));
}
