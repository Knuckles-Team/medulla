//! Attached-source conformance (MEDULLA-UNIFIED-DATA-PLANE-R022): the change-capture
//! replay of a recorded Debezium topic against the source's own row state.

use std::collections::BTreeMap;

use medulla_protocol_types::debezium_bridge::{DebeziumChangeEvent, DebeziumOp};
use medulla_protocol_types::protocol::Method;
use medulla_types::dialect_conformance::ConformanceOutcome;
use medulla_types::replay_conformance::compare_replay_to_snapshot;
use serde_json::Value;
use sha2::{Digest, Sha256};

const REPLAY_FIXTURE: &str = include_str!("../conformance/fixtures/replay_postgres16.jsonl");
/// The engine the fixture was recorded from.
const ENGINE: &str = "postgres";
const ENGINE_VERSION: &str = "16";
/// Rows the fixture's table holds before the mutations, and the mutations that follow.
const BASE_ROWS: usize = 1_000;
const MUTATIONS: usize = 200;

fn fixture_events() -> Vec<DebeziumChangeEvent> {
    REPLAY_FIXTURE
        .lines()
        .map(|line| serde_json::from_str(line).expect("a fixture line is a Debezium event"))
        .collect()
}

/// The row state the replay leaves: every event converted by the Debezium bridge
/// into a draft, the draft's graph operation applied in stream order.
fn replayed_state(events: &[DebeziumChangeEvent]) -> BTreeMap<String, Value> {
    let mut state = BTreeMap::new();
    for (ordinal, event) in events.iter().enumerate() {
        let draft = event
            .to_draft(format!("env-{ordinal}"), format!("batch-{ordinal}"))
            .expect("every fixture event carries the row its op needs");
        for operation in draft.mutation.operations {
            match operation.method {
                Method::AddNode {
                    node_id,
                    properties_msgpack,
                } => {
                    let row: Value =
                        rmp_serde::from_slice(&properties_msgpack).expect("a row decodes");
                    state.insert(node_id, row);
                }
                Method::RemoveNode { node_id } => {
                    state.remove(&node_id);
                }
                other => panic!("the bridge emits only node operations, got {other:?}"),
            }
        }
    }
    state
}

/// The source's own row state at the end of the stream: the recorded rows folded by
/// primary key without the bridge, as the source table would hold them.
fn snapshot_state(events: &[DebeziumChangeEvent]) -> BTreeMap<String, Value> {
    let mut table: BTreeMap<String, Value> = BTreeMap::new();
    for event in events {
        let row = event
            .after
            .as_ref()
            .or(event.before.as_ref())
            .expect("a row");
        let key = format!(
            "{}.{}:{}",
            event.source.db,
            event.source.table,
            row.get("id").expect("a primary key")
        );
        match event.op {
            DebeziumOp::Delete => {
                table.remove(&key);
            }
            _ => {
                table.insert(key, row.clone());
            }
        }
    }
    table
}

/// One SHA-256 (64 lowercase hex) over the rows sorted by primary key.
fn rows_digest(state: &BTreeMap<String, Value>) -> String {
    let mut hasher = Sha256::new();
    for (key, row) in state {
        hasher.update(key.as_bytes());
        hasher.update([0]);
        hasher.update(row.to_string().as_bytes());
        hasher.update([b'\n']);
    }
    hex::encode(hasher.finalize())
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R022.3.2
#[test]
fn replay_equals_snapshot() {
    let events = fixture_events();
    assert_eq!(events.len(), BASE_ROWS + MUTATIONS);
    let ops = |op: DebeziumOp| events[BASE_ROWS..].iter().filter(|e| e.op == op).count();
    assert!(ops(DebeziumOp::Create) > 0 && ops(DebeziumOp::Update) > 0);
    assert!(ops(DebeziumOp::Delete) > 0);

    let snapshot = snapshot_state(&events);
    let replayed = replayed_state(&events);
    let (replay_digest, snapshot_digest) = (rows_digest(&replayed), rows_digest(&snapshot));
    assert_eq!(replay_digest.len(), 64);
    assert!(replay_digest
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    assert_eq!(replay_digest, snapshot_digest);
    let entry = compare_replay_to_snapshot(ENGINE, ENGINE_VERSION, &replayed, &snapshot)
        .expect("a named engine version");
    assert_eq!(entry.outcome, ConformanceOutcome::Green);

    // A stream that lost one event no longer equals the source: a failing entry.
    let mut dropped = events.clone();
    dropped.remove(BASE_ROWS + 2);
    let short = replayed_state(&dropped);
    assert_ne!(rows_digest(&short), snapshot_digest);
    let failing = compare_replay_to_snapshot(ENGINE, ENGINE_VERSION, &short, &snapshot)
        .expect("a named engine version");
    assert_eq!(failing.outcome, ConformanceOutcome::Deviated);
    assert!(
        failing.validate().is_err(),
        "a deviation still needs a reviewer"
    );
}
