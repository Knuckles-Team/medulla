//! Safe eviction and durable read-through through the served `EvictLRU` and
//! `GetNodeProperties` methods over a redb-backed `ServerState`
//! (MEDULLA-DURABLE-KERNEL-R086.2, R086.3, R086.5, R086.6).

#![cfg(feature = "redb")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use medulla_graph_core::read_through::NodeReadOutcome;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_server_api::api::persistence::PersistenceBackend;

const SECRET: &str = "safe-eviction-read-through-secret";
const KEY: &str = "safe-eviction-read-through-encryption-key";
const GRAPH: &str = "safeevict";
const OTHER: &str = "safeevictother";

async fn durable(
    prefix: &str,
    graph: &str,
) -> (
    std::path::PathBuf,
    test_support::SharedPersistence,
    test_support::SharedState,
) {
    test_support::durable_graph_state(SECRET, KEY, prefix, common::current_isolation(), graph).await
}

async fn attach_read_through(
    state: &test_support::SharedState,
    backend: &test_support::SharedPersistence,
) {
    let factory = Arc::new(
        medulla_persistence::read_through::BackendReadThroughFactory::new(backend.clone()),
    );
    state
        .write()
        .await
        .registry
        .set_read_through_factory(factory);
}

fn core(
    state: &test_support::SharedState,
    graph: &str,
) -> Arc<medulla_graph_core::graph::GraphCore> {
    state
        .try_read()
        .unwrap()
        .registry
        .get(graph)
        .unwrap()
        .core
        .clone()
}

async fn add(
    state: &test_support::SharedState,
    id: u64,
    graph: &str,
    node: &str,
    value: serde_json::Value,
) {
    test_support::call_ok(
        state,
        SECRET,
        id,
        graph,
        Method::AddNode {
            node_id: node.to_string(),
            properties_msgpack: test_support::json_bytes(value),
        },
    )
    .await;
}

async fn evict(
    state: &test_support::SharedState,
    id: u64,
    graph: &str,
    max_nodes: usize,
) -> Response {
    test_support::call_ok(state, SECRET, id, graph, Method::EvictLRU { max_nodes }).await
}

fn evicted_count(response: &Response) -> u64 {
    match &response.result {
        Some(ResultPayload::Json(value)) => value.as_u64().expect("count"),
        other => panic!("expected an eviction count, got {other:?}"),
    }
}

async fn get(state: &test_support::SharedState, id: u64, graph: &str, node: &str) -> Response {
    test_support::dispatch(
        state,
        test_support::request(
            SECRET,
            id,
            graph,
            Method::GetNodeProperties {
                node_id: node.to_string(),
            },
        ),
    )
    .await
}

fn props(response: &Response) -> Option<Vec<u8>> {
    assert!(
        response.error.is_none(),
        "read failed: {:?}",
        response.error
    );
    match &response.result {
        Some(ResultPayload::Raw(bytes)) => Some(bytes.clone()),
        Some(ResultPayload::Json(serde_json::Value::Null)) => None,
        other => panic!("unexpected read result {other:?}"),
    }
}

async fn close(
    dir: std::path::PathBuf,
    backend: test_support::SharedPersistence,
    state: test_support::SharedState,
) {
    backend.shutdown();
    state.write().await.persistence = None;
    drop(state);
    let _ = std::fs::remove_dir_all(dir);
}

// spec: MEDULLA-DURABLE-KERNEL-R086.2
#[test]
fn selected_residents_leave_durable_rows_unchanged() {
    test_support::on_engine(selected_residents_body);
}

async fn selected_residents_body() {
    let (dir, backend, state) = durable("medulla-evict-durable", GRAPH).await;
    attach_read_through(&state, &backend).await;
    let ids: Vec<String> = (0..8).map(|i| format!("n{i}")).collect();
    for (i, node) in ids.iter().enumerate() {
        add(
            &state,
            10 + i as u64,
            GRAPH,
            node,
            serde_json::json!({"type": "Row", "i": i}),
        )
        .await;
    }
    let fname = medulla_kernel::persist::sanitize(GRAPH);
    let mut before = Vec::new();
    for node in &ids {
        before.push(backend.read_node(&fname, node).await.expect("durable read"));
    }
    assert!(before.iter().all(Option::is_some), "every row is durable");
    let version = core(&state, GRAPH).version();

    let response = evict(&state, 30, GRAPH, 3).await;
    assert_eq!(evicted_count(&response), 5);
    assert!(
        response.error_detail.is_none(),
        "every candidate was confirmed"
    );
    assert_eq!(core(&state, GRAPH).node_count(), 3);

    let mut after = Vec::new();
    for node in &ids {
        after.push(backend.read_node(&fname, node).await.expect("durable read"));
    }
    assert_eq!(before, after, "eviction leaves the durable rows unchanged");
    assert_eq!(
        core(&state, GRAPH).version(),
        version,
        "eviction dirties nothing"
    );
    close(dir, backend, state).await;
}

// spec: MEDULLA-DURABLE-KERNEL-R086.3
#[test]
fn evicted_blob_reads_without_resident_growth() {
    test_support::on_engine(evicted_blob_reads_body);
}

async fn evicted_blob_reads_body() {
    let (dir, backend, state) = durable("medulla-evict-readthrough", GRAPH).await;
    attach_read_through(&state, &backend).await;
    let ids: Vec<String> = (0..101).map(|i| format!("r{i:03}")).collect();
    for (i, node) in ids.iter().enumerate() {
        add(
            &state,
            100 + i as u64,
            GRAPH,
            node,
            serde_json::json!({"type": "Row", "i": i}),
        )
        .await;
    }
    let mut served = Vec::new();
    for (i, node) in ids.iter().enumerate() {
        served.push(props(&get(&state, 300 + i as u64, GRAPH, node).await));
    }
    assert_eq!(evicted_count(&evict(&state, 500, GRAPH, 1).await), 100);
    let resident = core(&state, GRAPH).node_count();
    assert_eq!(resident, 1);

    for (i, node) in ids.iter().enumerate() {
        let read = props(&get(&state, 600 + i as u64, GRAPH, node).await);
        assert_eq!(
            read, served[i],
            "evicted node {node} reads its durable bytes"
        );
    }
    assert_eq!(
        core(&state, GRAPH).node_count(),
        resident,
        "reads never repopulate RAM"
    );

    struct Down;
    impl medulla_graph_core::read_through::ReadThrough for Down {
        fn read_node_blob(&self, _node_id: &str) -> NodeReadOutcome {
            NodeReadOutcome::Unavailable("injected durable fault".to_string())
        }
    }
    core(&state, GRAPH).set_read_through(Arc::new(Down));
    let refused = get(&state, 900, GRAPH, &ids[0]).await;
    assert_eq!(refused.error.as_deref(), Some("DURABLE_READ_UNAVAILABLE"));
    assert!(
        refused.result.is_none(),
        "a durable fault never answers empty"
    );
    close(dir, backend, state).await;
}

// spec: MEDULLA-DURABLE-KERNEL-R086.5
#[test]
fn served_evicted_reads_preserve_scope() {
    test_support::on_engine(served_scope_body);
}

async fn served_scope_body() {
    let (dir, backend, state) = durable("medulla-evict-scope", GRAPH).await;
    test_support::call_ok(
        &state,
        SECRET,
        2,
        OTHER,
        Method::CreateGraph {
            graph_name: OTHER.to_string(),
            graph_type: medulla_protocol_types::protocol::GraphType::Global,
        },
    )
    .await;
    attach_read_through(&state, &backend).await;
    add(
        &state,
        10,
        GRAPH,
        "shared",
        serde_json::json!({"type": "Row", "graph": "one"}),
    )
    .await;
    add(
        &state,
        11,
        OTHER,
        "shared",
        serde_json::json!({"type": "Row", "graph": "two"}),
    )
    .await;
    add(
        &state,
        12,
        GRAPH,
        "hidden",
        serde_json::json!({"type": "Row", "_visibility": "private", "_owner": "someone-else"}),
    )
    .await;
    add(
        &state,
        13,
        GRAPH,
        "keep",
        serde_json::json!({"type": "Row"}),
    )
    .await;
    add(
        &state,
        14,
        OTHER,
        "keep",
        serde_json::json!({"type": "Row"}),
    )
    .await;

    let one = props(&get(&state, 20, GRAPH, "shared").await);
    let two = props(&get(&state, 21, OTHER, "shared").await);
    let hidden_before = props(&get(&state, 22, GRAPH, "hidden").await);
    let absent = props(&get(&state, 23, GRAPH, "never-written").await);
    assert_ne!(one, two, "equal ids hold distinct rows per graph");

    evict(&state, 30, GRAPH, 1).await;
    evict(&state, 31, OTHER, 1).await;

    assert_eq!(props(&get(&state, 40, GRAPH, "shared").await), one);
    assert_eq!(props(&get(&state, 41, OTHER, "shared").await), two);
    let hidden_after = props(&get(&state, 42, GRAPH, "hidden").await);
    assert_eq!(
        hidden_after, hidden_before,
        "the fallback re-checks row visibility"
    );
    if hidden_before.is_none() {
        assert_eq!(
            hidden_after, absent,
            "a hidden row answers like an absent row"
        );
    }
    close(dir, backend, state).await;
}

/// A persistence tier whose presence answer is a chosen fault; every other
/// method keeps the trait's fail-closed default.
struct PresenceFault(fn(usize) -> Result<Vec<bool>, String>);

#[async_trait::async_trait]
impl PersistenceBackend for PresenceFault {
    async fn load_all(&self, _state: &test_support::SharedState) -> Result<usize, String> {
        Ok(0)
    }

    async fn record_durable(
        &self,
        _graph_fname: &str,
        _method: &medulla_protocol_types::protocol::Method,
    ) -> Result<(), String> {
        Ok(())
    }

    fn durable_node_presence(
        &self,
        _graph_fname: &str,
        node_ids: &[String],
    ) -> Result<Vec<bool>, String> {
        (self.0)(node_ids.len())
    }

    fn shutdown(&self) {}
}

// spec: MEDULLA-DURABLE-KERNEL-R086.6
#[test]
fn production_eviction_retains_unconfirmed_candidates() {
    test_support::on_engine(unconfirmed_candidates_body);
}

async fn unconfirmed_candidates_body() {
    let faults: [Option<fn(usize) -> Result<Vec<bool>, String>>; 4] = [
        None,
        Some(|_| Err("injected presence fault".to_string())),
        Some(|n| Ok(vec![true; n.saturating_sub(1)])),
        Some(|n| Ok(vec![false; n])),
    ];
    for (case, fault) in faults.into_iter().enumerate() {
        let persistence: Option<test_support::SharedPersistence> =
            fault.map(|f| Arc::new(PresenceFault(f)) as test_support::SharedPersistence);
        let state =
            test_support::state_with(SECRET, common::current_isolation(), None, persistence);
        let base = case as u64 * 100;
        test_support::call_ok(
            &state,
            SECRET,
            base + 1,
            GRAPH,
            Method::CreateGraph {
                graph_name: GRAPH.to_string(),
                graph_type: medulla_protocol_types::protocol::GraphType::Global,
            },
        )
        .await;
        let graph = core(&state, GRAPH);
        for i in 0..4 {
            graph.add_node(
                format!("u{i}"),
                test_support::json_bytes(serde_json::json!({"type": "Row", "i": i})),
            );
        }
        let resident_before = graph.node_ids();
        let response = evict(&state, base + 10, GRAPH, 1).await;
        assert_eq!(
            evicted_count(&response),
            0,
            "case {case}: nothing confirmed, nothing evicted"
        );
        assert_eq!(
            response.error_detail.as_deref(),
            Some("EVICTION_UNCONFIRMED"),
            "case {case}: candidates remain, the cap is not reported reached"
        );
        let mut resident_after = graph.node_ids();
        let mut expected = resident_before;
        resident_after.sort();
        expected.sort();
        assert_eq!(
            resident_after, expected,
            "case {case}: every candidate stays resident"
        );
    }
}
