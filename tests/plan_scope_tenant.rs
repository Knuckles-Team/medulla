//! MEDULLA-FEDERATED-QUERY-R078.3: a served planned statement reaches execution
//! only with the caller's scope injected into its plan — the executed plan is
//! never the caller's raw binding, and a statement whose scope cannot bind is
//! refused before execution.
#![cfg(all(
    feature = "query",
    feature = "redb",
    feature = "security",
    feature = "federation"
))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::wire::UqlResult;

const SECRET: &str = "plan-scope-tenant-secret";

fn state() -> test_support::SharedState {
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    )
}

async fn add_doc(state: &test_support::SharedState, id: u64, node: &str) {
    let props = serde_json::json!({
        "type": "Doc",
        "node_type": "Doc",
    });
    let response = test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::AddNode {
                node_id: node.to_string(),
                properties_msgpack: test_support::json_bytes(props),
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

fn body(response: &Response) -> UqlResult {
    match &response.result {
        Some(ResultPayload::Raw(bytes)) => {
            rmp_serde::from_slice(bytes).expect("Uql result payload")
        }
        other => panic!(
            "expected a Raw Uql result, got {other:?} / error: {:?}",
            response.error
        ),
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R078.3
#[tokio::test]
async fn planned_statement_executes_only_with_predicate() {
    let state = state();
    add_doc(&state, 1, "d1").await;
    add_doc(&state, 2, "d2").await;

    // The planned statement reaches execution WITH the scope: the server pin
    // over the executed plan differs from the digest of the caller's own
    // unscoped binding of the same text — `inject_scope` ran after binding
    // and before execution.
    let text = "EXPLAIN MATCH (:Doc)";
    let explain = uql(&state, 3, text).await;
    assert!(
        explain.error.is_none(),
        "EXPLAIN refused: {:?}",
        explain.error
    );
    let UqlResult::Explain {
        plan_digest: Some(served_digest),
        ..
    } = body(&explain)
    else {
        panic!("the served EXPLAIN carries the plan pin: {explain:?}")
    };
    let params = std::collections::BTreeMap::new();
    let statement = medulla_uql::parse_statement(text, &params).expect("parse explain");
    let raw_binding = medulla_plan::uql::serve::binding_plan(&statement);
    let raw_digest =
        medulla_server_ops::api::query_txn::query_support::canonical_plan_digest(&raw_binding)
            .expect("raw plan digest");
    assert_ne!(
        served_digest, raw_digest,
        "the executed plan carries the injected scope, not the raw binding"
    );

    // And the statement executes — carried by the predicate, never refused by
    // it: the scope rides the plan and filters no row this slice.
    let run = uql(&state, 4, "MATCH (:Doc)").await;
    assert!(
        run.error.is_none(),
        "a scoped statement executes: {:?}",
        run.error
    );
    let UqlResult::Rows { rows, .. } = body(&run) else {
        panic!("rows were expected: {run:?}")
    };
    assert_eq!(rows.len(), 2, "both rows return: {run:?}");
}
