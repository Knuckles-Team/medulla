//! MEDULLA-CQ-ORCH-R010.3: the served `ArchitectureView` read through the real `dispatch`
//! shell over a redb-backed `ServerState`.
//!
//! The view is derived from the dispatching graph only: another graph's repository is
//! unknown, a stated revision that differs from the repository snapshot digest refuses
//! `VIEW_STALE_REVISION`, and a read commits nothing.

#![cfg(all(feature = "redb", feature = "mining"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_catalog_types::architecture_view::{ArchitectureViewResponse, ViewScope};
use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};

const SECRET: &str = "architecture-view-served-secret";
const ENCRYPTION_KEY: &str = "architecture-view-served-key-0123456789";
const GRAPH: &str = "view-home";
const OTHER_GRAPH: &str = "view-other";
const REPOSITORY: &str = "repo";

async fn send(state: &test_support::SharedState, id: u64, graph: &str, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, graph, method)).await
}

fn payload(response: Response) -> ArchitectureViewResponse {
    assert!(response.error.is_none(), "refused: {:?}", response.error);
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("typed answer"),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed answer"),
        other => panic!("unexpected payload {other:?}"),
    }
}

async fn add_node(
    state: &test_support::SharedState,
    id: u64,
    graph: &str,
    node_id: &str,
    properties: serde_json::Value,
) {
    let response = send(
        state,
        id,
        graph,
        Method::AddNode {
            node_id: node_id.to_string(),
            properties_msgpack: test_support::json_bytes(properties),
        },
    )
    .await;
    assert!(
        response.error.is_none(),
        "AddNode {node_id}: {:?}",
        response.error
    );
}

/// One package with a manifest and two files of `repo`.
async fn ingest_repository(state: &test_support::SharedState, graph: &str, first_id: u64) -> u64 {
    let mut id = first_id;
    for (node, kind, file) in [
        ("m-orders", "FILE", "services/orders/Cargo.toml"),
        ("s-orders", "SYMBOL", "services/orders/src/main.rs"),
    ] {
        add_node(
            state,
            id,
            graph,
            node,
            serde_json::json!({"type": kind, "file": file, "repository_id": REPOSITORY}),
        )
        .await;
        id += 1;
    }
    id
}

fn view_request(revision: Option<String>) -> Method {
    Method::ArchitectureView {
        scope: ViewScope {
            repository_id: REPOSITORY.to_string(),
            revision,
        },
    }
}

fn error_text(response: &Response) -> String {
    format!(
        "{} {}",
        response.error.clone().unwrap_or_default(),
        response.error_detail.clone().unwrap_or_default()
    )
}

async fn state_with_two_graphs() -> test_support::SharedState {
    let (_dir, _backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "architecture-view-served",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    test_support::call_ok(
        &state,
        SECRET,
        2,
        OTHER_GRAPH,
        Method::CreateGraph {
            graph_name: OTHER_GRAPH.to_string(),
            graph_type: GraphType::Global,
        },
    )
    .await;
    state
}

// spec: MEDULLA-CQ-ORCH-R010.3
#[tokio::test(flavor = "multi_thread")]
async fn served_operation_returns_view() {
    let state = state_with_two_graphs().await;
    let next = ingest_repository(&state, GRAPH, 3).await;
    let answer = payload(send(&state, next, GRAPH, view_request(None)).await);
    assert_eq!(answer.view.containers.len(), 1, "{:?}", answer.view);
    assert_eq!(answer.view.containers[0].name, "orders");
    assert!(answer.view.containers[0]
        .support
        .contains(&"services/orders/Cargo.toml".to_string()));
    let revision = answer.revision.clone().expect("a committed snapshot digest");
    assert_eq!(revision.len(), 64);
    assert_eq!(answer.digest.len(), 64);

    // A stated revision equal to the digest returns the same view.
    let again = payload(send(&state, next + 1, GRAPH, view_request(Some(revision.clone()))).await);
    assert_eq!(again.view, answer.view);
    assert_eq!(again.revision, Some(revision));
}

// spec: MEDULLA-CQ-ORCH-R010.3
#[tokio::test(flavor = "multi_thread")]
async fn other_tenant_repository_is_unknown() {
    let state = state_with_two_graphs().await;
    let next = ingest_repository(&state, OTHER_GRAPH, 3).await;
    let response = send(&state, next, GRAPH, view_request(None)).await;
    assert!(
        error_text(&response).contains("VIEW_UNKNOWN_REPOSITORY"),
        "{:?}",
        response.error
    );
    assert!(response.result.is_none(), "no partial view");
}

// spec: MEDULLA-CQ-ORCH-R010.3
#[tokio::test(flavor = "multi_thread")]
async fn stale_revision_compares_latest_committed_snapshot() {
    let state = state_with_two_graphs().await;
    let next = ingest_repository(&state, GRAPH, 3).await;
    let first = payload(send(&state, next, GRAPH, view_request(None)).await)
        .revision
        .expect("first snapshot digest");

    // A second committed ingest changes the snapshot digest.
    add_node(
        &state,
        next + 1,
        GRAPH,
        "s-orders-lib",
        serde_json::json!({
            "type": "SYMBOL",
            "file": "services/orders/src/lib.rs",
            "repository_id": REPOSITORY,
        }),
    )
    .await;
    let stale = send(&state, next + 2, GRAPH, view_request(Some(first.clone()))).await;
    assert!(
        error_text(&stale).contains("VIEW_STALE_REVISION"),
        "{:?}",
        stale.error
    );
    assert!(stale.result.is_none(), "no view for a stale revision");

    let newest = payload(send(&state, next + 3, GRAPH, view_request(None)).await)
        .revision
        .expect("second snapshot digest");
    assert_ne!(newest, first);
    let current = payload(send(&state, next + 4, GRAPH, view_request(Some(newest.clone()))).await);
    assert_eq!(current.revision, Some(newest));
}
