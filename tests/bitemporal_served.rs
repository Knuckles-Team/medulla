//! MEDULLA-DURABLE-KERNEL-R080.2: served `SupersedeEdge` refuses a missing or
//! closed prior edge with its registered code and leaves history unchanged.
#![cfg(all(feature = "server", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, Response};

const SECRET: &str = "bitemporal-served-secret";

fn state() -> test_support::SharedState {
    test_support::durable_state(SECRET, common::current_isolation())
}

async fn call(state: &test_support::SharedState, id: u64, graph: &str, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, graph, method)).await
}

async fn create_graph(state: &test_support::SharedState, id: u64, name: &str) -> Response {
    call(
        state,
        id,
        name,
        Method::CreateGraph {
            graph_name: name.to_string(),
            graph_type: GraphType::Global,
        },
    )
    .await
}

fn supersede_edge_method() -> Method {
    Method::SupersedeEdge {
        source_id: "a".to_string(),
        target_id: "b".to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
            "relationship": "DISLIKES",
            "valid_from": 200,
            "tx_from": 250,
            "supersedes": "a:b:LIKES",
        }))
        .unwrap(),
        prior_source: "a".to_string(),
        prior_target: "b".to_string(),
        prior_relationship: "LIKES".to_string(),
        valid_at: 200,
        tx_now: 250,
    }
}

async fn edge_snapshot(
    state: &test_support::SharedState,
    graph: &str,
) -> (Vec<Vec<u8>>, Vec<String>, usize) {
    let core = state
        .read()
        .await
        .registry
        .get(graph)
        .expect("test graph must remain resident")
        .core
        .clone();
    (
        core.get_edge_properties("a", "b"),
        core.get_ledger(),
        core.edge_count(),
    )
}

async fn add_node(state: &test_support::SharedState, id: u64, graph: &str, node_id: &str) {
    let response = call(
        state,
        id,
        graph,
        Method::AddNode {
            node_id: node_id.to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({})).unwrap(),
        },
    )
    .await;
    assert!(response.error.is_none(), "AddNode {node_id}: {response:?}");
}

// spec: MEDULLA-DURABLE-KERNEL-R080.2
#[tokio::test]
async fn second_supersession_refuses_with_edge_already_closed_and_history_is_unchanged() {
    let state = state();
    let graph = "bitemporal-r0802";
    assert!(create_graph(&state, 1, graph).await.error.is_none());
    add_node(&state, 2, graph, "a").await;
    add_node(&state, 3, graph, "b").await;

    let missing = call(&state, 4, graph, supersede_edge_method()).await;
    assert!(
        missing
            .refusal_text()
            .is_some_and(|text| text.starts_with("EDGE_NOT_FOUND")),
        "missing prior must refuse with EDGE_NOT_FOUND: {missing:?}"
    );
    let (empty, _, empty_count) = edge_snapshot(&state, graph).await;
    assert!(empty.is_empty(), "no successor may be added on refusal");
    assert_eq!(empty_count, 0);

    let prior = call(
        &state,
        5,
        graph,
        Method::AddEdge {
            source_id: "a".to_string(),
            target_id: "b".to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({
                "relationship": "LIKES",
                "valid_from": 100,
                "tx_from": 100,
            }))
            .unwrap(),
        },
    )
    .await;
    assert!(prior.error.is_none(), "AddEdge: {prior:?}");

    let first = call(&state, 6, graph, supersede_edge_method()).await;
    assert!(first.error.is_none(), "first SupersedeEdge: {first:?}");
    let after_first = edge_snapshot(&state, graph).await;
    assert_eq!(after_first.0.len(), 2, "prior stays beside the successor");

    let second = call(&state, 7, graph, supersede_edge_method()).await;
    assert!(
        second
            .refusal_text()
            .is_some_and(|text| text.starts_with("EDGE_ALREADY_CLOSED")),
        "closed prior must refuse with EDGE_ALREADY_CLOSED: {second:?}"
    );
    assert_eq!(edge_snapshot(&state, graph).await, after_first);
}
