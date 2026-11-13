//! MEDULLA-FEDERATED-QUERY-R065: prove the DAG EXPLAIN/PROFILE contract through
//! the served `Method::Uql` path, rather than only through `medulla-plan`.
// spec: MEDULLA-FEDERATED-QUERY-R065
#![cfg(all(feature = "query", feature = "redb", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::wire::UqlResult;

const SECRET: &str = "uql-dag-served-test-secret";

fn state() -> test_support::SharedState {
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with(
        SECRET,
        common::current_isolation(),
        persist_dir,
        persistence,
    )
}

async fn add_doc(state: &test_support::SharedState, id: u64, node: &str, year: i64) {
    let response = test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::AddNode {
                node_id: node.to_string(),
                properties_msgpack: test_support::json_bytes(
                    serde_json::json!({"type": "Doc", "year": year}),
                ),
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

async fn add_cites(state: &test_support::SharedState, id: u64, source: &str, target: &str) {
    let response = test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::AddEdge {
                source_id: source.to_string(),
                target_id: target.to_string(),
                properties_msgpack: test_support::json_bytes(
                    serde_json::json!({"relationship": "CITES"}),
                ),
            },
        ),
    )
    .await;
    assert!(
        response.error.is_none(),
        "AddEdge {source}->{target} failed: {:?}",
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

fn decode(response: Response) -> UqlResult {
    assert!(response.error.is_none(), "UQL failed: {:?}", response.error);
    match response.result {
        Some(ResultPayload::Raw(bytes)) => {
            rmp_serde::from_slice(&bytes).expect("decode served UQL result")
        }
        other => panic!("expected Raw UQL result, got {other:?}"),
    }
}

/// The source-level DAG checks already cover materialization and execution. This test
/// closes the served seam by sending the same named-subplan program through `Method::Uql`.
#[tokio::test]
async fn served_dag_explain_and_profile_report_each_node() {
    let state = state();
    for (id, node, year) in [
        (1, "d1", 2020),
        (2, "d2", 2021),
        (3, "d3", 2022),
        (4, "d4", 2023),
    ] {
        add_doc(&state, id, node, year).await;
    }
    add_cites(&state, 5, "d1", "d2").await;
    add_cites(&state, 6, "d2", "d3").await;

    let program = "LET docs = MATCH (:Doc); LET cited = FROM docs |> TRAVERSE CITES; \
                   JOIN docs, cited |> RERANK MENTIONS |> RETURN mentions";

    let explain = decode(uql(&state, 7, &format!("EXPLAIN {program}")).await);
    let UqlResult::Explain {
        canonical,
        optimized,
        stages,
        incremental,
        incremental_note,
        ..
    } = explain
    else {
        panic!("expected served EXPLAIN result")
    };
    assert_eq!(canonical, optimized, "served DAG runs as written");
    assert_eq!(stages.len(), 4, "one served EXPLAIN stage per DAG node");
    assert!(stages[0].stage.starts_with("#0 MATCH (:Doc)"));
    assert!(stages[2].stage.starts_with("#2 <- #0,#1 RERANK MENTIONS"));
    assert!(stages
        .iter()
        .all(|stage| stage.rows.is_none() && stage.micros.is_none()));
    assert!(!incremental);
    assert!(incremental_note.contains("no cost"));
    assert!(incremental_note.contains("not incrementally maintained"));

    let profile = decode(uql(&state, 8, &format!("PROFILE {program}")).await);
    let UqlResult::Profile {
        rows,
        columns,
        stages,
        ..
    } = profile
    else {
        panic!("expected served PROFILE result")
    };
    assert_eq!(columns, vec!["mentions"]);
    assert_eq!(stages.len(), 4, "one served PROFILE stage per DAG node");
    assert_eq!(
        stages.iter().map(|stage| stage.rows).collect::<Vec<_>>(),
        vec![Some(4), Some(2), Some(2), Some(2)]
    );
    assert!(stages.iter().all(|stage| stage.micros.is_some()));
    assert_eq!(rows.len(), 2, "served DAG result keeps the joined rows");
    assert!(rows.iter().all(|row| row.channels == vec![Some(1.0)]));
}

// spec: MEDULLA-DURABLE-KERNEL-R079
async fn add_edge_with(
    state: &test_support::SharedState,
    id: u64,
    source: &str,
    target: &str,
    properties: serde_json::Value,
) {
    let response = test_support::dispatch(
        state,
        test_support::commons_request(
            SECRET,
            id,
            Method::AddEdge {
                source_id: source.to_string(),
                target_id: target.to_string(),
                properties_msgpack: test_support::json_bytes(properties),
            },
        ),
    )
    .await;
    assert!(response.error.is_none(), "AddEdge: {:?}", response.error);
}

async fn reached_ids(state: &test_support::SharedState, id: u64, text: &str) -> Vec<String> {
    let UqlResult::Rows { rows, .. } = decode(uql(state, id, text).await) else {
        panic!("expected served rows")
    };
    let mut ids: Vec<String> = rows.into_iter().map(|row| row.id).collect();
    ids.sort();
    ids
}

/// Versions written through the served `SupersedeEdge` answer correctly at two
/// instants on both axes: a traversal follows only the edge valid then, and the
/// instant before the first version reaches nothing.
// spec: MEDULLA-DURABLE-KERNEL-R079
#[tokio::test]
async fn as_of_returns_the_right_version_at_two_instants_on_both_axes() {
    let state = state();
    // `d1` (year 2020) seeds the walk; `d2` and `d3` are the two targets.
    for (id, node, year) in [(1, "d1", 2020), (2, "d2", 2021), (3, "d3", 2022)] {
        add_doc(&state, id, node, year).await;
    }
    // The engine stamps transaction time with its own clock
    // (MEDULLA-DURABLE-KERNEL-R078), so the transaction-axis instants are read
    // from that clock around each write rather than chosen by the caller.
    add_edge_with(
        &state,
        4,
        "d1",
        "d2",
        serde_json::json!({"relationship": "CITES", "valid_from": 100, "tx_from": 100}),
    )
    .await;
    let between_writes = engine_instant_after_a_tick().await;
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let superseded = test_support::dispatch(
        &state,
        test_support::commons_request(
            SECRET,
            5,
            Method::SupersedeEdge {
                source_id: "d1".to_string(),
                target_id: "d3".to_string(),
                properties_msgpack: test_support::json_bytes(
                    serde_json::json!({"relationship": "CITES"}),
                ),
                prior_source: "d1".to_string(),
                prior_target: "d2".to_string(),
                prior_relationship: "CITES".to_string(),
                valid_at: 200,
                tx_now: 250,
            },
        ),
    )
    .await;
    assert!(superseded.error.is_none(), "{:?}", superseded.error);
    let after_supersession = engine_instant_after_a_tick().await;

    let walk = |pins: &str| format!("MATCH (:Doc) WHERE year = 2020 |> {pins}TRAVERSE CITES");
    // Valid axis.
    assert_eq!(
        reached_ids(&state, 6, &walk("AS OF @150 |> ")).await,
        ["d2"]
    );
    assert_eq!(
        reached_ids(&state, 7, &walk("AS OF @250 |> ")).await,
        ["d3"]
    );
    assert!(reached_ids(&state, 8, &walk("AS OF @50 |> "))
        .await
        .is_empty());
    // Transaction axis.
    assert_eq!(
        reached_ids(&state, 9, &walk(&format!("AS OF TX @{between_writes} |> "))).await,
        ["d2"]
    );
    assert_eq!(
        reached_ids(
            &state,
            10,
            &walk(&format!("AS OF TX @{after_supersession} |> "))
        )
        .await,
        ["d3"]
    );
    // Both axes: what was believed after the supersession and valid at 150 is nothing.
    assert!(reached_ids(
        &state,
        11,
        &walk(&format!("AS OF @150 |> AS OF TX @{after_supersession} |> "))
    )
    .await
    .is_empty());
}

/// An engine-clock instant (epoch milliseconds) strictly after every write that
/// returned before this call: wait out the current millisecond, then read it.
async fn engine_instant_after_a_tick() -> u64 {
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .as_millis() as u64
}
