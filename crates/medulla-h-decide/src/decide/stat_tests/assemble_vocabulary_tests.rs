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
