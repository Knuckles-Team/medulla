//! MEDULLA-FEDERATED-QUERY-R720.5: the `DECAY` and `CLUSTER BY EMBEDDING` refusals through
//! the served `Method::Uql` path — each names its registered code and returns no rows.
#![cfg(all(feature = "query", feature = "redb", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{Method, Response};

const SECRET: &str = "uql-single-plan-all-modalities-secret";

fn state() -> test_support::SharedState {
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    )
}

async fn add_note(state: &test_support::SharedState, id: u64, node: &str) {
    let response = test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::AddNode {
                node_id: node.to_string(),
                properties_msgpack: test_support::json_bytes(serde_json::json!({"type": "Note"})),
            },
        ),
    )
    .await;
    assert!(
        response.error.is_none(),
        "AddNode {node} failed: {:?}",
        response.error
    );
}

async fn uql(state: &test_support::SharedState, id: u64, text: &str) -> Response {
    test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::Uql {
                text: text.to_string(),
                params: Default::default(),
                prepare: false,
                foreign_execution: None,
            },
        ),
    )
    .await
}

fn assert_refused(response: Response, code: &str) {
    let error = response.error.unwrap_or_else(|| panic!("expected {code}"));
    assert!(error.contains(code), "expected {code}, got {error}");
    assert!(
        response.result.is_none(),
        "a refused program returns no rows"
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R720.5
#[tokio::test]
async fn decay_and_cluster_refusals() {
    let state = state();
    add_note(&state, 1, "n1").await;
    add_note(&state, 2, "n2").await;
    assert_refused(
        uql(&state, 3, "MATCH (:Note) |> DECAY HALF_LIFE 60").await,
        "UQL_E_DECAY_NO_TIME",
    );
    assert_refused(
        uql(&state, 4, "MATCH (:Note) |> CLUSTER BY EMBEDDING K 2").await,
        "UQL_E_CLUSTER_NO_VECTOR",
    );
}
