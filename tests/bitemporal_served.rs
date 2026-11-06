//! MEDULLA-DURABLE-KERNEL-R080.2: served `SupersedeEdge` refuses a missing or
//! closed prior edge with its registered code and leaves history unchanged.
//! MEDULLA-DURABLE-KERNEL-R078: the engine stamps transaction time and refuses an
//! inverted valid interval.
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
/// Verifies MEDULLA-DURABLE-KERNEL-R080.2: Supersession refuses a missing or closed prior edge.
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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn edge_blob(value: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&value).unwrap()
}

fn stamps(blob: &[u8]) -> serde_json::Value {
    medulla_types::msgpack::decode_property_value(blob).unwrap()
}

// spec: MEDULLA-DURABLE-KERNEL-R078
#[tokio::test]
async fn engine_stamps_tx_time_and_refuses_an_inverted_interval() {
    let state = state();
    let graph = "bitemporal-r078";
    assert!(create_graph(&state, 1, graph).await.error.is_none());
    add_node(&state, 2, graph, "a").await;
    add_node(&state, 3, graph, "b").await;

    let before = now_ms();
    for (id, props) in [
        (
            4,
            serde_json::json!({"relationship": "LIKES", "valid_from": 100, "tx_from": 1}),
        ),
        (
            5,
            serde_json::json!({"relationship": "KNOWS", "tx_from": 1}),
        ),
    ] {
        let added = call(
            &state,
            id,
            graph,
            Method::AddEdge {
                source_id: "a".to_string(),
                target_id: "b".to_string(),
                properties_msgpack: edge_blob(props),
            },
        )
        .await;
        assert!(added.error.is_none(), "AddEdge: {added:?}");
    }
    // The caller's tx_now is replaced by the engine clock before the method is logged.
    let invalidated = call(
        &state,
        6,
        graph,
        Method::InvalidateEdge {
            source_id: "a".to_string(),
            target_id: "b".to_string(),
            relationship: "LIKES".to_string(),
            invalid_at: 500,
            tx_now: 7,
        },
    )
    .await;
    assert!(
        invalidated.error.is_none(),
        "InvalidateEdge: {invalidated:?}"
    );
    let after = now_ms();

    let (blobs, ledger, edge_count) = edge_snapshot(&state, graph).await;
    let likes = stamps(&blobs[0]);
    let knows = stamps(&blobs[1]);
    let within = |value: &serde_json::Value| {
        let t = value.as_u64().expect("stamp is a number");
        (before..=after).contains(&t)
    };
    assert_eq!(likes["valid_from"], 100, "caller valid_from is kept");
    assert!(within(&likes["tx_from"]), "engine tx_from: {likes}");
    assert_eq!(likes["valid_until"], 500);
    assert!(within(&likes["tx_to"]), "engine tx_to: {likes}");
    assert_eq!(
        knows["valid_from"], knows["tx_from"],
        "valid_from defaults to tx_now"
    );
    assert!(within(&knows["tx_from"]), "engine tx_from: {knows}");

    // An inverted interval refuses and writes nothing.
    let inverted = call(
        &state,
        7,
        graph,
        Method::AddEdge {
            source_id: "a".to_string(),
            target_id: "b".to_string(),
            properties_msgpack: edge_blob(serde_json::json!({
                "relationship": "TRUSTS", "valid_from": 900, "valid_until": 800,
            })),
        },
    )
    .await;
    assert!(
        inverted
            .refusal_text()
            .is_some_and(|text| text.starts_with("VALIDITY_INVERTED")),
        "inverted AddEdge: {inverted:?}"
    );
    let too_early = call(
        &state,
        8,
        graph,
        Method::InvalidateEdge {
            source_id: "a".to_string(),
            target_id: "b".to_string(),
            relationship: "KNOWS".to_string(),
            invalid_at: 0,
            tx_now: 0,
        },
    )
    .await;
    assert!(
        too_early
            .refusal_text()
            .is_some_and(|text| text.starts_with("VALIDITY_INVERTED")),
        "inverted InvalidateEdge: {too_early:?}"
    );
    let (_, ledger_after, count_after) = edge_snapshot(&state, graph).await;
    assert_eq!((ledger_after, count_after), (ledger.clone(), edge_count));

    // Replaying the logged methods over a fresh core reproduces every stamp.
    let replayed = medulla_graph_core::graph::GraphCore::new();
    for node in ["a", "b"] {
        replayed.add_node(node.to_string(), edge_blob(serde_json::json!({})));
    }
    for entry in &ledger {
        let parts: Vec<&str> = entry.split('|').collect();
        match parts.as_slice() {
            ["ADD_EDGE", source, target, hex] => medulla_server_api::mutation_apply::apply(
                &replayed,
                &Method::AddEdge {
                    source_id: source.to_string(),
                    target_id: target.to_string(),
                    properties_msgpack: hex::decode(hex).unwrap(),
                },
            ),
            ["INVALIDATE_EDGE", source, target, relationship, invalid_at, tx_now] => {
                medulla_server_api::mutation_apply::apply(
                    &replayed,
                    &Method::InvalidateEdge {
                        source_id: source.to_string(),
                        target_id: target.to_string(),
                        relationship: relationship.to_string(),
                        invalid_at: invalid_at.parse().unwrap(),
                        tx_now: tx_now.parse().unwrap(),
                    },
                )
            }
            _ => {}
        }
    }
    assert_eq!(replayed.get_edge_properties("a", "b"), blobs);
}
