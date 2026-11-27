//! MEDULLA-DECISION-ENGINE-R651.11: appending a turn through the served
//! `MemoryRemember` enqueues one derive task per key on the durable queue
//! `memory.derive`. Ten turns of one session leave exactly one pending task per
//! key, the payload node holds the extended range and its due time, another
//! tenant's records and tasks never count, and a tenant at its pending limit
//! keeps its turn while the enqueue is skipped. The knobs are installed once,
//! before any served call.

#![cfg(all(feature = "redb", feature = "jobs"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use medulla_decision_types::claims::ObserverRef;
use medulla_h_graph::graph_ops::participant_derive::{payload_node_id, read_payload};
use medulla_protocol_types::protocol::{Method, Response};
use medulla_server_api::knob_config;
use medulla_server_ops::api::ingest::jobs_seam;
use medulla_types::contract::{ResourceId, TenantId};
use medulla_work_types::memory::{
    MemoryKind, MemoryRecord, MemoryRememberRequest, MemoryTier, MEMORY_RECORD_SCHEMA_VERSION,
};
use medulla_work_types::participant::ParticipantId;
use {
    medulla_decision_stats_records::decision::participant_derive::DeriveKey,
    medulla_decision_stats_records::decision::participant_derive::DerivePayload,
};

const SECRET: &str = "participant-derive-served-secret";
const ENCRYPTION_KEY: &str = "participant-derive-served-encryption-key-0123456789";
const GRAPH: &str = "derivegraph";
const QUEUE: &str = "memory.derive";
/// The registry's lowest legal pending limit.
const PENDING_LIMIT: u64 = 100;
const BATCH_TURNS: u64 = 3;
const DEBOUNCE_MS: u64 = 30_000;
const BASE_MS: i64 = 1_000_000;

fn member(slug: &str) -> ParticipantId {
    ParticipantId::new(slug).expect("a valid slug")
}

fn turn_record(tenant: &str, id: &str, session: &str, speaker: &str, ts: i64) -> MemoryRecord {
    MemoryRecord {
        memory_id: ResourceId::new(id).unwrap(),
        tenant: TenantId::new(tenant).unwrap(),
        kind: MemoryKind::ConversationTurn,
        tier: MemoryTier::Episodic,
        text: format!("{speaker} said {id}"),
        provenance_key: None,
        session_id: Some(ResourceId::new(session).unwrap()),
        speaker: Some(member(speaker)),
        timestamp_ms: ts,
        topics: vec![],
        pattern_key: None,
        importance: 1.0,
        access_count: 0,
        last_access_ms: 0,
        last_decay_ms: 0,
        forgotten: false,
        legal_hold: false,
        expires_at_ms: None,
        support_count: 1,
        token_estimate: 10,
        attachments: vec![],
        schema_version: MEMORY_RECORD_SCHEMA_VERSION,
    }
}

async fn send(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await
}

async fn append(
    state: &test_support::SharedState,
    n: u64,
    id: &str,
    session: &str,
    speaker: &str,
    ts: i64,
) {
    let record = turn_record(common::TEST_TENANT, id, session, speaker, ts);
    let method = Method::MemoryRemember {
        request: Box::new(MemoryRememberRequest {
            record,
            embedding: vec![1.0, 0.0, 0.0],
            episode: None,
        }),
    };
    let response = send(state, n, method).await;
    assert!(
        response.error.is_none(),
        "{id}: {:?}",
        response.refusal_text()
    );
}

fn key(session: &str, subject: &str, observer: ObserverRef) -> DeriveKey {
    DeriveKey {
        scope_id: String::new(),
        session_id: session.into(),
        subject: member(subject),
        observer,
    }
}

async fn payload_of(state: &test_support::SharedState, key: &DeriveKey) -> Option<DerivePayload> {
    let core = {
        let current = state.read().await;
        current.registry.get(GRAPH).expect("graph").core.clone()
    };
    read_payload(
        &core,
        common::TEST_TENANT,
        &payload_node_id(common::TEST_TENANT, key),
    )
}

/// Tasks of `tenant` waiting on the derive queue.
async fn pending(state: &test_support::SharedState, tenant: &str) -> u64 {
    let jobs = jobs_seam(state).await.expect("the jobs seam is installed");
    let persist_dir = state.read().await.persist_dir.clone();
    jobs.task_queue_load(&persist_dir, tenant, QUEUE)
        .expect("queue load")
        .0
}

// spec: MEDULLA-DECISION-ENGINE-R651.11
#[tokio::test(flavor = "multi_thread")]
async fn turn_append_enqueues_one_ordered_job_per_key() {
    let args = vec![
        format!("memory.derive_max_pending={PENDING_LIMIT}"),
        format!("memory.derive_batch_turns={BATCH_TURNS}"),
        format!("memory.derive_debounce={DEBOUNCE_MS}"),
    ];
    knob_config::install(knob_config::load_knobs(&args, |_| None).expect("knobs in range"));

    let (dir, backend, state) = test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "participant-derive-served",
        common::current_isolation(),
        GRAPH,
    )
    .await;
    let tenant = common::TEST_TENANT;

    // Ten turns of one session, alternating two speakers.
    let mut call = 2;
    for i in 0..10i64 {
        let speaker = if i % 2 == 0 { "alice" } else { "bob" };
        append(&state, call, &format!("t{i}"), "s1", speaker, BASE_MS + i).await;
        call += 1;
    }
    // Two speakers, each with the system view and its own view: four keys, one pending task
    // each, however many turns were appended.
    assert_eq!(pending(&state, tenant).await, 4);
    let alice_system = payload_of(&state, &key("s1", "alice", ObserverRef::System))
        .await
        .expect("alice's system key has a payload");
    assert_eq!(alice_system.first_turn_id, "t0");
    assert_eq!(alice_system.last_turn_id, "t8");
    assert_eq!(alice_system.turns, 5);
    // The range holds at least `batch_turns` turns, so it is due at its newest turn.
    assert_eq!(alice_system.due_at_ms, (BASE_MS + 8) as u64);
    let alice_own = payload_of(
        &state,
        &key("s1", "alice", ObserverRef::Participant(member("alice"))),
    )
    .await
    .expect("alice's own key has a payload");
    assert_ne!(alice_own.task_id, alice_system.task_id);
    // Bob never observes alice (the default is self only): no such key.
    assert!(payload_of(
        &state,
        &key("s1", "alice", ObserverRef::Participant(member("bob")))
    )
    .await
    .is_none());

    // Appending an id already stored changes nothing.
    append(&state, call, "t0", "s1", "alice", BASE_MS).await;
    call += 1;
    assert_eq!(
        payload_of(&state, &key("s1", "alice", ObserverRef::System)).await,
        Some(alice_system.clone())
    );

    // A short range waits out the debounce.
    append(&state, call, "u0", "s2", "alice", BASE_MS).await;
    call += 1;
    let short = payload_of(&state, &key("s2", "alice", ObserverRef::System))
        .await
        .expect("a payload");
    assert_eq!(short.turns, 1);
    assert_eq!(short.due_at_ms, BASE_MS as u64 + DEBOUNCE_MS);
    assert_eq!(pending(&state, tenant).await, 6);

    // A later turn extends the pending range and adds no second task.
    append(&state, call, "t10", "s1", "alice", BASE_MS + 10).await;
    call += 1;
    assert_eq!(pending(&state, tenant).await, 6);
    let extended = payload_of(&state, &key("s1", "alice", ObserverRef::System))
        .await
        .expect("a payload");
    assert_eq!(extended.turns, 6);
    assert_eq!(extended.last_turn_id, "t10");
    assert_eq!(extended.task_id, alice_system.task_id);
    // Another tenant's tasks never count against ours.
    assert_eq!(pending(&state, "another-tenant").await, 0);

    // Fill the tenant to its limit with one-turn sessions (two keys each).
    let mut n = 0u64;
    while pending(&state, tenant).await < PENDING_LIMIT {
        append(
            &state,
            call,
            &format!("b{n}"),
            &format!("b-{n}"),
            "alice",
            BASE_MS,
        )
        .await;
        call += 1;
        n += 1;
    }
    assert_eq!(pending(&state, tenant).await, PENDING_LIMIT);
    // One more turn: it commits, the enqueue is skipped, no second task appears.
    append(&state, call, "over", "s-over", "alice", BASE_MS).await;
    let core = {
        let current = state.read().await;
        current.registry.get(GRAPH).expect("graph").core.clone()
    };
    assert!(
        core.get_node_properties(&medulla_h_graph::graph_ops::memory_surface::node_id(
            tenant, "over"
        ))
        .is_some(),
        "the turn is stored"
    );
    assert_eq!(pending(&state, tenant).await, PENDING_LIMIT);
    assert!(
        payload_of(&state, &key("s-over", "alice", ObserverRef::System))
            .await
            .is_none()
    );

    backend.shutdown();
    state.write().await.persistence = None;
    drop(state);
    let _ = std::fs::remove_dir_all(dir);
}
