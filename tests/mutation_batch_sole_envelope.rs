//! Every store member commits through the envelope with an idempotent terminal outcome
//! (MEDULLA-DURABLE-KERNEL-R043.100.5).
//!
//! For each member of the durable store (the graph shard, the key-value store, the user-table
//! store and the series store) a second request that carries the first request's idempotency
//! key under a fresh nonce returns the first request's terminal outcome and writes nothing. The
//! same holds after the engine restarts on the same directory. Each member is read before and
//! after the replay: an unchanged read proves nothing was written.
#![cfg(all(feature = "server", feature = "security", feature = "redb", feature = "kv", feature = "tsdb"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_protocol_types::protocol::{GraphType, Method, Request, Response};

const SECRET: &str = "sole-envelope-replay-secret";
const GRAPH: &str = "soleenvelope";

fn signed(id: u64, method: Method, nonce: &str, key: &str) -> Request {
    common::signed_request_with_key(SECRET, id, GRAPH, method, nonce, key)
}

async fn send(state: &test_support::SharedState, request: Request) -> Response {
    Box::pin(test_support::dispatch(state, request)).await
}

/// The outcome of a response as comparable text: its result and its error.
fn outcome(response: &Response) -> String {
    format!("{:?}|{:?}", response.result, response.error)
}

/// Sends `method` under `key` and then again under the same key with a fresh nonce; the replay
/// must answer the first request's outcome.
async fn assert_replay_returns_terminal_outcome(
    state: &test_support::SharedState,
    label: &str,
    method: Method,
    read: Method,
    id: u64,
) {
    let key = format!("sole-envelope-{label}-key");
    let first = send(state, signed(id, method.clone(), &format!("{label}-nonce-1"), &key)).await;
    assert!(first.error.is_none(), "{label} first request: {:?}", first.error);
    let before = outcome(&send(state, signed(id + 1, read.clone(), &format!("{label}-read-1"), &format!("{label}-read-key-1"))).await);
    let replay = send(state, signed(id, method, &format!("{label}-nonce-2"), &key)).await;
    assert!(replay.error.is_none(), "{label} replay: {:?}", replay.error);
    assert_eq!(
        outcome(&replay),
        outcome(&first),
        "{label}: the fresh-envelope replay returns the first terminal outcome"
    );
    let after = outcome(&send(state, signed(id + 1, read, &format!("{label}-read-2"), &format!("{label}-read-key-2"))).await);
    assert_eq!(before, after, "{label}: the replay wrote nothing");
}

fn points() -> Vec<u8> {
    rmp_serde::to_vec(&vec![(1_000_000_000i64, vec![1.0f64]), (2_000_000_000i64, vec![2.0f64])])
        .expect("points")
}

fn members() -> Vec<(&'static str, Method, Method)> {
    vec![
        (
            "applymutation",
            Method::AddNode {
                node_id: "sole-node".to_string(),
                properties_msgpack: test_support::json_bytes(serde_json::json!({"type": "Doc"})),
            },
            Method::GetNodeProperties {
                node_id: "sole-node".to_string(),
            },
        ),
        (
            "kvput",
            Method::KvPut {
                namespace: "sole".to_string(),
                key: "k".to_string(),
                value: b"v".to_vec(),
                shared: false,
            },
            Method::KvGet {
                namespace: "sole".to_string(),
                key: "k".to_string(),
                shared: false,
            },
        ),
        (
            "sqlinsert",
            Method::Sql {
                query: "INSERT INTO sole_t (id) VALUES (1)".to_string(),
                params_msgpack: Vec::new(),
            },
            Method::Sql {
                query: "SELECT count(*) FROM sole_t".to_string(),
                params_msgpack: Vec::new(),
            },
        ),
        (
            "tsappend",
            Method::TsAppend {
                series_id: "sole-series".to_string(),
                n_fields: 1,
                bucket_ns: 60_000_000_000,
                field_names: vec!["value".to_string()],
                points_msgpack: points(),
            },
            Method::TsRange {
                series_id: "sole-series".to_string(),
                from: 0,
                to: 10_000_000_000,
            },
        ),
    ]
}

async fn run_members(state: &test_support::SharedState, round: &str, base: u64) {
    for (index, (label, method, read)) in members().into_iter().enumerate() {
        assert_replay_returns_terminal_outcome(
            state,
            &format!("{label}-{round}"),
            method,
            read,
            base + 10 * index as u64,
        )
        .await;
    }
}

// spec: MEDULLA-DURABLE-KERNEL-R043.100.5
#[tokio::test]
async fn every_store_member_replays_its_terminal_outcome_across_restart() {
    let dir = test_support::fresh_dir("medulla-sole-envelope");
    let dir_s = dir.to_string_lossy().to_string();
    test_support::provision_encryption_key_once("sole-envelope-encryption-key");
    let series = test_support::temporary_series("sole-envelope");

    let backend = test_support::open_redb_backend(dir_s.clone()).unwrap();
    let state = test_support::state_with_tsdb(
        SECRET,
        common::current_isolation(),
        Some(dir_s.clone()),
        Some(backend.clone()),
        Some(series.clone()),
    );
    let created = send(
        &state,
        signed(
            1,
            Method::CreateGraph {
                graph_name: GRAPH.to_string(),
                graph_type: GraphType::Global,
            },
            "sole-create-nonce",
            "sole-create-key",
        ),
    )
    .await;
    assert!(created.error.is_none(), "CreateGraph: {:?}", created.error);
    let table = send(
        &state,
        signed(
            2,
            Method::Sql {
                query: "CREATE TABLE sole_t (id BIGINT PRIMARY KEY)".to_string(),
                params_msgpack: Vec::new(),
            },
            "sole-table-nonce",
            "sole-table-key",
        ),
    )
    .await;
    assert!(table.error.is_none(), "CREATE TABLE: {:?}", table.error);

    run_members(&state, "live", 100).await;

    // Restart on the same directory and replay again: the terminal outcomes are durable.
    drop(state);
    let backend = test_support::reopen_after_sole_reference(
        backend,
        || test_support::open_redb_backend(dir_s.clone()),
        "reopen the durable backend for the sole-envelope replay",
    )
    .await;
    let state = test_support::state_with_tsdb(
        SECRET,
        common::current_isolation(),
        Some(dir_s.clone()),
        Some(backend.clone()),
        Some(series),
    );
    backend
        .load_catalog(&state)
        .await
        .expect("recover the durable catalog");
    // The first-round keys are replayed after the restart under yet another fresh nonce.
    for (index, (label, method, read)) in members().into_iter().enumerate() {
        let key = format!("sole-envelope-{label}-live-key");
        let id = 500 + 10 * index as u64;
        let before = outcome(&send(&state, signed(id + 1, read.clone(), &format!("{label}-r-1"), &format!("{label}-rk-1"))).await);
        let replay = send(&state, signed(id, method, &format!("{label}-restart-nonce"), &key)).await;
        assert!(replay.error.is_none(), "{label} replay after restart: {:?}", replay.error);
        let after = outcome(&send(&state, signed(id + 1, read, &format!("{label}-r-2"), &format!("{label}-rk-2"))).await);
        assert_eq!(before, after, "{label}: the replay after restart wrote nothing");
    }
}
