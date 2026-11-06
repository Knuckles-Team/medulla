//! Bolt listener round trip over a real TCP socket (MEDULLA-DURABLE-KERNEL-R860.2).
//!
//! Starts `medulla_wire::bolt_wire::serve` (the function `spawn_bolt_listener` runs)
//! over an in-process `ServerState`, authenticates with the in-tree Bolt client using
//! the `epistemic` scheme and a signed session credential, then runs an openCypher
//! match through `RUN` + `PULL` and reads the records.

#![cfg(feature = "bolt-wire")]

#[path = "common/bolt_client.rs"]
mod bolt_client;
mod common;
#[path = "common/test_support.rs"]
mod test_support;

use bolt_client::{BoltClient, BoltReply};
use medulla_protocol_types::protocol::Method;
use medulla_wire::bolt_wire;
use medulla_wire::bolt_wire::packstream::PackValue;

const AUTH_SECRET: &str = "test";
const USER: &str = "tester";

async fn spawn_bolt(state: test_support::SharedState) -> String {
    let addr = test_support::ephemeral_listener_addr().await;
    let serve_addr = addr.clone();
    tokio::spawn(async move {
        let _ = bolt_wire::serve(&serve_addr, state).await;
    });
    test_support::wait_for_listener_ready(&addr).await;
    addr
}

// spec: MEDULLA-DURABLE-KERNEL-R860.2
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn run_returns_records() {
    let persistence = test_support::durable_persistence("bolt-roundtrip-recovery-key");
    let state = test_support::seeded_wire_state(
        AUTH_SECRET,
        USER,
        Some(test_support::sql_test_persist_dir("bolt")),
        persistence,
    );
    let addr = spawn_bolt(state).await;

    let mut client = BoltClient::connect(&addr).await.expect("bolt connect");
    let session = common::signed_request_as(AUTH_SECRET, 1, "__commons__", USER, Method::Health);
    match client.hello(&session).await.expect("hello") {
        BoltReply::Success(_) => {}
        other => panic!("HELLO with a signed session credential must succeed: {other:?}"),
    }

    let records = client
        .run_pull("MATCH (n) RETURN n.rank AS rank")
        .await
        .expect("run/pull io")
        .unwrap_or_else(|failure| panic!("RUN failed: {failure:?}"));
    let mut ranks: Vec<i64> = records
        .iter()
        .filter_map(|row| match row.first() {
            Some(PackValue::Int(rank)) => Some(*rank),
            _ => None,
        })
        .collect();
    ranks.sort();
    assert_eq!(
        records.len(),
        3,
        "three seeded nodes stream back: {records:?}"
    );
    assert_eq!(ranks, vec![1, 2, 3], "records carry the seeded properties");

    // A forged credential is refused before any query runs.
    let mut forged = BoltClient::connect(&addr).await.expect("bolt connect");
    let mut bad = session.clone();
    bad.auth_token = "eg2.forged".to_string();
    match forged.hello(&bad).await.expect("hello io") {
        BoltReply::Failure(_) => {}
        other => panic!("a forged credential must be refused: {other:?}"),
    }
}
