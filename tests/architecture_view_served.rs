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

/// One ArchiMate diagram whose only application component is `Billing`.
const DIAGRAM: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<model xmlns="http://www.opengroup.org/xsd/archimate/3.0/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" identifier="id-model">
  <name xml:lang="en">Estate</name>
  <elements>
    <element identifier="id-proc" xsi:type="BusinessProcess"><name xml:lang="en">Order to cash</name></element>
    <element identifier="id-app" xsi:type="ApplicationComponent"><name xml:lang="en">Billing</name></element>
    <element identifier="id-cap" xsi:type="Capability"><name xml:lang="en">Invoicing</name></element>
  </elements>
  <relationships>
    <relationship identifier="id-r1" source="id-app" target="id-proc" xsi:type="Serving"/>
  </relationships>
  <views><diagrams><view identifier="id-view" xsi:type="Diagram"><name xml:lang="en">Main</name>
    <node identifier="n1" elementRef="id-proc" xsi:type="Element" x="10" y="20" w="120" h="55"/>
    <node identifier="n2" elementRef="id-app" xsi:type="Element" x="200" y="20" w="120" h="55"/>
    <connection identifier="c1" relationshipRef="id-r1" xsi:type="Relationship" source="n2" target="n1"/>
  </view></diagrams></views>
</model>"##;

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

// spec: MEDULLA-CQ-ORCH-R010.6
#[tokio::test(flavor = "multi_thread")]
async fn served_view_over_fixture_repository() {
    let state = state_with_two_graphs().await;
    let mut id = ingest_repository(&state, GRAPH, 3).await;

    // A second package and two calls from `orders` into it.
    for (node, kind, file) in [
        ("m-ledger", "FILE", "services/ledger/Cargo.toml"),
        ("s-ledger", "SYMBOL", "services/ledger/src/main.rs"),
        ("s-ledger-lib", "SYMBOL", "services/ledger/src/lib.rs"),
    ] {
        add_node(
            &state,
            id,
            GRAPH,
            node,
            serde_json::json!({"type": kind, "file": file, "repository_id": REPOSITORY}),
        )
        .await;
        id += 1;
    }
    for target in ["s-ledger", "s-ledger-lib"] {
        let response = send(
            &state,
            id,
            GRAPH,
            Method::AddEdge {
                source_id: "s-orders".to_string(),
                target_id: target.to_string(),
                properties_msgpack: test_support::json_bytes(
                    serde_json::json!({"relationship": "calls"}),
                ),
            },
        )
        .await;
        assert!(response.error.is_none(), "AddEdge {target}: {:?}", response.error);
        id += 1;
    }

    // One ArchiMate diagram in the same graph.
    let imported = send(
        &state,
        id,
        GRAPH,
        Method::ArchitectureModelImport {
            document: DIAGRAM.to_string(),
            expected_version: Some(1),
        },
    )
    .await;
    assert!(imported.error.is_none(), "import: {:?}", imported.error);
    id += 1;

    let answer = payload(send(&state, id, GRAPH, view_request(None)).await);
    let names: Vec<&str> = answer.view.containers.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Billing", "ledger", "orders"], "{:?}", answer.view);
    let billing = &answer.view.containers[0];
    assert!(billing.support.is_empty(), "a diagram-only container cites no file");
    let orders = &answer.view.containers[2];
    assert_eq!(
        orders.support,
        ["services/orders/Cargo.toml", "services/orders/src/main.rs"]
    );
    assert_eq!(answer.view.edges.len(), 1, "{:?}", answer.view.edges);
    let edge = &answer.view.edges[0];
    assert_eq!(
        (edge.from.as_str(), edge.to.as_str(), edge.kind.as_str(), edge.call_count),
        ("orders", "ledger", "calls", 2)
    );
}
