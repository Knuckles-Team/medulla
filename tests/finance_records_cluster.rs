//! MEDULLA-FINANCE-PRIMITIVES-R004.3.2.9: the replicated `FinanceRecordPut`.
//!
//! Three in-process Raft nodes serve the signed `dispatch` surface. A valid put
//! on the leader is checked there, proposed as the sealed
//! `NativeMutationCommand::FinanceRecordPut` and applied once on every node. A
//! derived-kind put refuses `RECORD_DERIVED_READONLY` and a stale
//! `expected_head` answers `RECORD_HEAD_CONFLICT`; both are refused by the
//! leader's checks, so the leader's log does not grow and no node changes.
#![cfg(all(
    feature = "harness",
    feature = "security",
    feature = "finance",
    feature = "shacl"
))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use openraft::async_runtime::watch::WatchReceiver;
use tokio::sync::RwLock;

use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
use medulla_raft::raft::config::RaftClusterConfig;
use medulla_raft::raft::node::{self, StartedNode};
use medulla_raft::raft::NodeId;
use medulla_raft_xshard::harness::cluster::fixture;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::state::ServerState;
use medulla_types::wire::{FinanceRecord, FinanceRecordKind, FINANCE_V1_SCHEMA_VERSION};

const SECRET: &str = "medulla-fin-r004-3-2-9-cluster-secret";
const GRAPH: &str = "fin-r004-3-2-9";
/// The verified tenant of every signed test request (`tests/common/mod.rs`).
const TENANT: &str = "integration-test-tenant";

type Shared = Arc<RwLock<ServerState>>;

async fn make_state(dir: &str) -> Shared {
    let backend: Arc<dyn PersistenceBackend> = fixture::open_backend(dir).expect("open redb");
    backend
        .register_graph("__commons__", "__commons__", GraphType::Commons)
        .await
        .expect("register mandatory commons graph");
    let state = medulla_raft_xshard::harness_support::make_state_with_cdc(
        dir,
        backend,
        common::current_isolation(),
        SECRET,
    )
    .await;
    medulla_server::seams_install::install_seams_on(&state).await;
    state
}

fn cluster_cfg(node_id: NodeId, ports: &[u16]) -> RaftClusterConfig {
    medulla_raft_xshard::harness::trace_capture::init();
    medulla_raft_xshard::harness::cluster_slots::claim();
    let peers = fixture::peer_map(ports);
    let bind_addr = peers.get(&node_id).unwrap().addr.clone();
    RaftClusterConfig {
        node_id,
        cluster_id: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .to_string(),
        peers: peers.clone(),
        bind_addr,
        advertised_client_addr: format!("tcp://127.0.0.1:{}", 31_000 + node_id),
        advertised_tls_server_name: None,
        advertised_certificate_id: None,
        advertised_certificate_rotation_epoch: 0,
        advertised_certificate_not_before_ms: None,
        advertised_certificate_not_after_ms: None,
        is_bootstrap: peers.keys().next() == Some(&node_id),
        groups: 1,
        transport_secret: Some(
            medulla_raft::raft::config::RaftTransportSecret::from_material(&[0x5b; 32]).unwrap(),
        ),
    }
}

fn free_ports(n: usize) -> Vec<u16> {
    let listeners: Vec<_> = (0..n)
        .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    listeners
        .iter()
        .map(|listener| listener.local_addr().unwrap().port())
        .collect()
}

async fn wait_until<F, Fut>(timeout: Duration, mut pred: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if pred().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    false
}

async fn leader(nodes: &BTreeMap<NodeId, StartedNode>) -> NodeId {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        for node in nodes.values() {
            if node.handle.raft.current_leader().await == Some(node.handle.node_id) {
                return node.handle.node_id;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("a leader must be elected");
}

fn last_log_index(node: &StartedNode) -> Option<u64> {
    node.handle.raft.metrics().borrow_watched().last_log_index
}

/// The stored record head on one node, `None` when the node has no record.
async fn stored_head(state: &Shared, node_id: &str) -> Option<u64> {
    let current = state.read().await;
    let core = current.registry.get(GRAPH)?.core.clone();
    let blob = core.get_node_properties(node_id)?;
    let props = medulla_types::msgpack::decode_property_object(&blob).ok()?;
    props.get("finance_head").and_then(|v| v.as_u64())
}

fn account(record_id: &str) -> FinanceRecord {
    FinanceRecord {
        record_id: record_id.to_string(),
        tenant: TENANT.to_string(),
        fields: [
            ("accountId", "\"acct-1\""),
            ("baseCurrency", "\"USD\""),
            ("accountKind", "\"taxable\""),
        ]
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect(),
    }
}

fn position(record_id: &str) -> FinanceRecord {
    FinanceRecord {
        record_id: record_id.to_string(),
        tenant: TENANT.to_string(),
        fields: [
            ("positionAccount", "<http://example.org/markets#acct1>"),
            ("positionInstrument", "<http://example.org/markets#btc>"),
            ("positionQuantity", "\"10\"^^xsd:decimal"),
            ("costBasisMethod", "\"fifo\""),
        ]
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect(),
    }
}

fn put(kind: FinanceRecordKind, record: FinanceRecord, expected_head: Option<u64>) -> Method {
    Method::FinanceRecordPut {
        kind,
        schema_version: FINANCE_V1_SCHEMA_VERSION.to_string(),
        record,
        expected_head,
    }
}

fn refused_with(response: &Response, code: &str) {
    assert!(
        response.result.is_none(),
        "expected a refusal: {response:?}"
    );
    assert_eq!(response.error.as_deref(), Some(code), "{response:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
// spec: MEDULLA-FINANCE-PRIMITIVES-R004.3.2.9
async fn replicated_put_checks_on_leader_and_applies_once() {
    common::configure_authority();
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let root = std::env::temp_dir().join(format!("medulla-fin-cluster-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let ports = free_ports(3);
    let mut states = Vec::new();
    let mut nodes: BTreeMap<NodeId, StartedNode> = BTreeMap::new();
    for id in 1..=3u64 {
        let dir = root.join(format!("node{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        let state = make_state(&dir.to_string_lossy()).await;
        let started = node::start(cluster_cfg(id, &ports), state.clone())
            .await
            .expect("start raft node");
        states.push(state);
        nodes.insert(id, started);
    }
    let leader_id = leader(&nodes).await;
    let leader_state = states[(leader_id - 1) as usize].clone();
    let mut next_id = 10u64;
    let mut call = |method: Method| {
        next_id += 1;
        test_support::request(SECRET, next_id, GRAPH, method)
    };

    let created = test_support::dispatch(
        &leader_state,
        call(Method::CreateGraph {
            graph_name: GRAPH.to_string(),
            graph_type: GraphType::Global,
        }),
    )
    .await;
    assert!(created.error.is_none(), "CreateGraph: {created:?}");

    // A valid put is checked on the leader, then applied once on every node.
    let account_node = FinanceRecordKind::Account.node_id("rec_1");
    let first = test_support::dispatch(
        &leader_state,
        call(put(FinanceRecordKind::Account, account("rec_1"), None)),
    )
    .await;
    assert!(
        matches!(first.result, Some(ResultPayload::Count(1))),
        "{first:?}"
    );
    for state in &states {
        let state = state.clone();
        let node = account_node.clone();
        assert!(
            wait_until(Duration::from_secs(10), || {
                let state = state.clone();
                let node = node.clone();
                async move { stored_head(&state, &node).await == Some(1) }
            })
            .await,
            "the put must apply on every node"
        );
    }

    // Refusals on the leader propose nothing: the leader's log does not grow.
    let leader_node = nodes.get(&leader_id).unwrap();
    let before = last_log_index(leader_node);

    let derived = test_support::dispatch(
        &leader_state,
        call(put(FinanceRecordKind::Position, position("rec_1"), None)),
    )
    .await;
    refused_with(&derived, "RECORD_DERIVED_READONLY");

    let stale = test_support::dispatch(
        &leader_state,
        call(put(FinanceRecordKind::Account, account("rec_1"), None)),
    )
    .await;
    refused_with(&stale, "RECORD_HEAD_CONFLICT");

    assert_eq!(
        last_log_index(leader_node),
        before,
        "a refused put must propose nothing"
    );
    let position_node = FinanceRecordKind::Position.node_id("rec_1");
    for state in &states {
        assert_eq!(stored_head(state, &account_node).await, Some(1));
        assert_eq!(stored_head(state, &position_node).await, None);
    }

    for (_, node) in nodes {
        node.multi.stop_listener();
        let _ = node.handle.raft.shutdown().await;
    }
    let _ = std::fs::remove_dir_all(&root);
}
