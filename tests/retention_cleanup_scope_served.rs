//! Served scope gate of the retention cleanup step (MEDULLA-DURABLE-KERNEL-R700.4).
//!
//! `Method::RetentionCleanupRun` is driven through the real request boundary with a
//! verified context that lacks `retention:run`. The generic method gate refuses it
//! as `ACCESS_DENIED` from the policy row of the method, and the graph is unchanged.
#![cfg(all(feature = "server", feature = "security", feature = "streaming"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_mutation_types::retention_cleanup::RetentionCleanupRunRequest;
use medulla_protocol_types::protocol::{Method, Response};

const SECRET: &str = "r700-4-retention-cleanup-scope-secret";
const GRAPH: &str = "__commons__";

async fn call(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await
}

async fn node_listing(state: &test_support::SharedState, id: u64) -> String {
    let response = call(
        state,
        id,
        Method::GetNodesByLabel {
            label: String::new(),
            after: None,
            limit: 100,
        },
    )
    .await;
    assert!(response.error.is_none(), "GetNodesByLabel: {response:?}");
    format!("{:?}", response.result)
}

// spec: MEDULLA-DURABLE-KERNEL-R700.4
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retention_run_without_the_scope_is_access_denied() {
    let state = test_support::durable_state(SECRET, common::current_isolation());
    let added = call(
        &state,
        1,
        Method::AddNode {
            node_id: "doc:held".to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({"type": "Doc"}))
                .unwrap(),
        },
    )
    .await;
    assert!(added.error.is_none(), "AddNode: {added:?}");
    let before = node_listing(&state, 2).await;

    for (id, dry_run) in [(3_u64, false), (4, true)] {
        let refused = test_support::dispatch(
            &state,
            common::signed_request_with_scopes(
                SECRET,
                id,
                GRAPH,
                &["graph:read"],
                Method::RetentionCleanupRun {
                    request: RetentionCleanupRunRequest {
                        graph: GRAPH.to_string(),
                        dry_run,
                        cursor: None,
                    },
                },
            ),
        )
        .await;
        assert!(refused.result.is_none(), "an unscoped caller ran a cleanup step");
        let error = refused.error.expect("an unscoped caller is refused");
        assert!(error.contains("ACCESS_DENIED"), "unexpected refusal: {error}");
    }

    assert_eq!(node_listing(&state, 5).await, before, "a refusal leaves the graph unchanged");
}
