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
use medulla_query_types::wire::UqlResult;

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
                foreign_executions: Default::default(),
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

// spec: MEDULLA-FEDERATED-QUERY-R078.4
#[tokio::test]
async fn unverified_tenant_refused_zero_reads() {
    use medulla_server_api::api::reentry::TryHandleContext;

    let state = state();
    add_doc(&state, 1, "d1").await;

    // The served handler is entered with no verified read authority: the
    // caller has no verified tenant, so the statement is refused before any
    // leg binds or any read runs — never answered by post-filtering rows.
    let rls = std::sync::Arc::new(common::current_isolation());
    let response = medulla_h_query::query::try_handle(
        &state,
        TryHandleContext {
            req_id: 5,
            graph_name: "__commons__",
            read_authority: None,
            caller: "",
        },
        std::sync::Arc::new(medulla_graph_core::graph::GraphCore::new()),
        Method::Uql {
            text: "MATCH (:Doc)".to_string(),
            params: Default::default(),
            prepare: false,
            foreign_executions: Default::default(),
        },
        &rls,
    )
    .await
    .unwrap_or_else(|method| panic!("the query family handles Uql, not {method:?}"));
    assert_eq!(
        response.error.as_deref(),
        Some("PLAN_SCOPE_UNVERIFIED"),
        "an unverified caller is refused with the registered code: {response:?}"
    );
    assert!(
        response.result.is_none(),
        "no rows, no snapshot header, no source coverage: {response:?}"
    );

    // The same text from a verified caller (the served dispatch) still runs.
    let verified = uql(&state, 6, "MATCH (:Doc)").await;
    assert!(verified.error.is_none(), "{:?}", verified.error);
}

// spec: MEDULLA-FEDERATED-QUERY-R078.5
#[tokio::test]
async fn trace_and_explain_show_scope() {
    let state = state();
    add_doc(&state, 1, "d1").await;

    let explain = uql(&state, 2, "EXPLAIN MATCH (:Doc)").await;
    assert!(explain.error.is_none(), "{:?}", explain.error);
    let UqlResult::Explain {
        plan_scope_tenant,
        plan_scope_predicates,
        ..
    } = body(&explain)
    else {
        panic!("EXPLAIN result expected: {explain:?}")
    };
    assert_eq!(plan_scope_tenant.as_deref(), Some("__commons__"));
    assert_eq!(
        plan_scope_predicates.len(),
        2,
        "the tenant equality and the clearance-label set: {plan_scope_predicates:?}"
    );
    assert!(plan_scope_predicates[0].contains("__commons__"));

    let profile = uql(&state, 3, "PROFILE MATCH (:Doc)").await;
    assert!(profile.error.is_none(), "{:?}", profile.error);
    let UqlResult::Profile {
        plan_scope_tenant: traced_tenant,
        plan_scope_predicates: traced_predicates,
        ..
    } = body(&profile)
    else {
        panic!("PROFILE result expected: {profile:?}")
    };
    assert_eq!(traced_tenant.as_deref(), Some("__commons__"));
    assert_eq!(traced_predicates, plan_scope_predicates);
}

async fn send_in(
    state: &test_support::SharedState,
    graph: &str,
    id: u64,
    method: Method,
) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, graph, method)).await
}

async fn seed_vector_doc(
    state: &test_support::SharedState,
    graph: &str,
    id: u64,
    node: &str,
    embedding: Vec<f32>,
) {
    let props = serde_json::json!({ "type": "Doc", "node_type": "Doc" });
    let added = send_in(
        state,
        graph,
        id,
        Method::AddNode {
            node_id: node.to_string(),
            properties_msgpack: test_support::json_bytes(props),
        },
    )
    .await;
    assert!(added.error.is_none(), "AddNode {node}: {:?}", added.error);
    let embedded = send_in(
        state,
        graph,
        id + 1,
        Method::AddEmbedding {
            node_id: node.to_string(),
            embedding,
        },
    )
    .await;
    assert!(
        embedded.error.is_none(),
        "AddEmbedding {node}: {:?}",
        embedded.error
    );
}

fn row_ids(response: &Response) -> Vec<String> {
    let UqlResult::Rows { rows, .. } = body(response) else {
        panic!("rows were expected: {response:?}")
    };
    let mut ids: Vec<String> = rows.into_iter().map(|row| row.id).collect();
    ids.sort();
    ids
}

// spec: MEDULLA-FEDERATED-QUERY-R078.6
#[tokio::test]
async fn only_own_rows_return() {
    let state = state();
    // Two tenants hold the same label and near-identical vectors; each statement is
    // answered with its own tenant's rows only, on the graph scan and on the vector leaf.
    seed_vector_doc(&state, "plan-scope-a", 10, "a-1", vec![1.0, 0.0]).await;
    seed_vector_doc(&state, "plan-scope-a", 20, "a-2", vec![0.9, 0.1]).await;
    seed_vector_doc(&state, "plan-scope-b", 30, "b-1", vec![1.0, 0.0]).await;
    seed_vector_doc(&state, "plan-scope-b", 40, "b-2", vec![0.95, 0.05]).await;
    seed_vector_doc(&state, "plan-scope-b", 50, "b-3", vec![0.0, 1.0]).await;

    for (graph, expected) in [
        ("plan-scope-a", vec!["a-1", "a-2"]),
        ("plan-scope-b", vec!["b-1", "b-2", "b-3"]),
    ] {
        let scan = send_in(
            &state,
            graph,
            60,
            test_support::uql("MATCH (:Doc)"),
        )
        .await;
        assert_eq!(row_ids(&scan), expected, "graph scan of {graph}: {scan:?}");

        let ranked = send_in(
            &state,
            graph,
            61,
            test_support::uql("MATCH (:Doc) |> RANK BY ~[1.0,0.0] |> LIMIT 10"),
        )
        .await;
        assert_eq!(
            row_ids(&ranked),
            expected,
            "vector leaf of {graph}: {ranked:?}"
        );

        let explain = send_in(
            &state,
            graph,
            62,
            test_support::uql("EXPLAIN MATCH (:Doc) |> RANK BY ~[1.0,0.0] |> LIMIT 10"),
        )
        .await;
        let UqlResult::Explain {
            plan_scope_tenant, ..
        } = body(&explain)
        else {
            panic!("EXPLAIN result expected: {explain:?}")
        };
        assert_eq!(plan_scope_tenant.as_deref(), Some(graph));
    }
}
