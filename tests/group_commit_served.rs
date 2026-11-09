//! MEDULLA-ENGINE-PARITY-R001.2 / R001.3: the bounded enqueue wait of the
//! durable group-commit queue, observed through the persistence backend that
//! the served `AddNode` path calls (`record_durable`).
#![cfg(all(feature = "server", feature = "redb"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;
use std::time::Duration;

use medulla_persistence::redb_backend::commands::Cmd;
use medulla_persistence::redb_backend::{
    RedbBackend, RedbGroupCommitConfig, RedbGroupCommitTestControl, ReleaseOnDrop,
};
use medulla_protocol_types::protocol::{GraphType, Method};
use medulla_server_api::api::persistence::PersistenceBackend;
use tokio::sync::oneshot;

fn add_node(node_id: &str) -> Method {
    Method::AddNode {
        node_id: node_id.to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({})).unwrap(),
    }
}

fn mutation(graph: &str, node_id: &str) -> (Cmd, oneshot::Receiver<Result<(), String>>) {
    let (done, rx) = oneshot::channel();
    (
        Cmd::Mutation {
            graph: graph.to_string(),
            method: Box::new(add_node(node_id)),
            done,
        },
        rx,
    )
}

async fn wait_gate(entered: std::sync::mpsc::Receiver<()>) {
    tokio::task::spawn_blocking(move || {
        medulla_kernel::test_rendezvous::recv_within(
            &entered,
            "the writer entering the injected linger gate",
        );
    })
    .await
    .expect("gate waiter completed");
}

// spec: MEDULLA-ENGINE-PARITY-R001.2
#[tokio::test(flavor = "multi_thread")]
async fn full_queue_refuses_commit_queue_full() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = test_support::fresh_dir("group-commit-full");
    let (control, entered_rx, release_tx) = RedbGroupCommitTestControl::new();
    let backend = Arc::new(
        RedbBackend::open_with_group_commit_config(
            dir.to_string_lossy().to_string(),
            1,
            RedbGroupCommitConfig {
                linger: Duration::from_millis(2),
                shallow_threshold: 32,
                enqueue_timeout: Duration::from_millis(150),
                test_control: Some(control),
            },
        )
        .expect("open"),
    );
    let mut release = ReleaseOnDrop::new(release_tx);
    backend
        .register_graph("g", "g", GraphType::Global)
        .await
        .expect("register graph");
    let shard = backend.shard_for("g");

    // The writer takes the first command off the channel and parks at the gate;
    // the second fills the capacity-1 channel.
    let (first, first_rx) = mutation("g", "n0");
    shard.tx.send(first).expect("writer alive");
    wait_gate(entered_rx).await;
    let (second, second_rx) = mutation("g", "n1");
    shard.tx.send(second).expect("room for one queued command");

    let refused = backend.record_durable("g", &add_node("n2")).await;
    let message = refused.expect_err("a full queue must refuse");
    assert!(
        message.starts_with("COMMIT_QUEUE_FULL"),
        "refusal must carry the registered code: {message}"
    );
    assert_eq!(backend.commit_stats().refused_full(), 1);

    release.release().expect("writer still held");
    first_rx.await.expect("completion").expect("n0 commits");
    second_rx.await.expect("completion").expect("n1 commits");
    assert!(backend.read_node("g", "n0").await.unwrap().is_some());
    assert!(backend.read_node("g", "n1").await.unwrap().is_some());
    assert!(
        backend.read_node("g", "n2").await.unwrap().is_none(),
        "the refused write must leave the graph unchanged"
    );
    backend.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Verifies MEDULLA-ENGINE-PARITY-R001.2: Wiring and refusal — a request that
/// cannot enqueue while the queue is held full fails alone with
/// `COMMIT_QUEUE_FULL`; the requests already in the held batch still commit,
/// and nothing that was enqueued is dropped.
// spec: MEDULLA-ENGINE-PARITY-R001.2
#[tokio::test(flavor = "multi_thread")]
async fn one_failure_does_not_fail_batch() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = test_support::fresh_dir("group-commit-failure-isolation");
    let (control, entered_rx, release_tx) = RedbGroupCommitTestControl::new();
    let backend = Arc::new(
        RedbBackend::open_with_group_commit_config(
            dir.to_string_lossy().to_string(),
            1,
            RedbGroupCommitConfig {
                linger: Duration::from_millis(2),
                shallow_threshold: 32,
                enqueue_timeout: Duration::from_millis(150),
                test_control: Some(control),
            },
        )
        .expect("open"),
    );
    let mut release = ReleaseOnDrop::new(release_tx);
    backend
        .register_graph("g", "g", GraphType::Global)
        .await
        .expect("register graph");
    let shard = backend.shard_for("g");
    let stats = shard.stats.clone();

    // The writer takes the first command off the channel and parks at the gate;
    // the second fills the capacity-1 channel.
    let (first, first_rx) = mutation("g", "ok-a");
    shard.tx.send(first).expect("writer alive");
    wait_gate(entered_rx).await;
    let (second, second_rx) = mutation("g", "ok-b");
    shard.tx.send(second).expect("room for one queued command");

    // The third request cannot enqueue while the queue is held full: it fails
    // alone, and the batch around it is untouched.
    let refused = backend.record_durable("g", &add_node("failed")).await;
    let message = refused.expect_err("the held-full queue must refuse the one request");
    assert!(
        message.starts_with("COMMIT_QUEUE_FULL"),
        "refusal must carry the registered code: {message}"
    );
    assert_eq!(backend.commit_stats().refused_full(), 1);

    release.release().expect("writer still held");
    first_rx.await.expect("completion").expect("ok-a commits");
    second_rx.await.expect("completion").expect("ok-b commits");
    assert!(backend.read_node("g", "ok-a").await.unwrap().is_some());
    assert!(backend.read_node("g", "ok-b").await.unwrap().is_some());
    assert!(
        backend.read_node("g", "failed").await.unwrap().is_none(),
        "the failed request changes nothing"
    );
    assert_eq!(
        stats.ops(),
        2,
        "the batch around the failure commits intact"
    );
    backend.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Verifies MEDULLA-ENGINE-PARITY-R001.3: Hot-path proof
// spec: MEDULLA-ENGINE-PARITY-R001.3
#[tokio::test(flavor = "multi_thread")]
async fn sixty_four_writers_share_one_fsync() {
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = test_support::fresh_dir("group-commit-64");
    let (control, entered_rx, release_tx) = RedbGroupCommitTestControl::new();
    let backend = Arc::new(
        RedbBackend::open_with_group_commit_config(
            dir.to_string_lossy().to_string(),
            4096,
            RedbGroupCommitConfig {
                linger: Duration::from_millis(2),
                shallow_threshold: 256,
                enqueue_timeout: Duration::from_secs(5),
                test_control: Some(control),
            },
        )
        .expect("open"),
    );
    let mut release = ReleaseOnDrop::new(release_tx);
    backend
        .register_graph("g", "g", GraphType::Global)
        .await
        .expect("register graph");
    let shard = backend.shard_for("g");
    let stats = shard.stats.clone();

    let mut receivers = Vec::new();
    let (first, first_rx) = mutation("g", "w0");
    shard.tx.send(first).expect("writer alive");
    receivers.push(first_rx);
    wait_gate(entered_rx).await;
    for i in 1..64 {
        let (cmd, rx) = mutation("g", &format!("w{i}"));
        shard.tx.send(cmd).expect("writer alive");
        receivers.push(rx);
    }
    release.release().expect("writer still held");
    for rx in receivers {
        rx.await.expect("own receipt").expect("each write commits");
    }
    assert_eq!(stats.ops(), 64, "every write is accounted for once");
    assert_eq!(stats.commits(), 1, "64 writers share one fsync");
    backend.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
