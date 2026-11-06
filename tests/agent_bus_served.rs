//! MEDULLA-DURABLE-KERNEL-R095.1000..R095.1004: the served agent bus roster
//! through the real `dispatch` shell over a redb-backed `ServerState`.
//!
//! Sessions register, heartbeat and read the roster; liveness follows the
//! participant ttl; every refusal writes nothing; another tenant's rows never
//! show; and a restart keeps every participant byte for byte.

#![cfg(feature = "redb")]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::collections::BTreeSet;

use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_types::agent_bus::{BusParticipant, BusRoster, MAX_SESSION_REF_BYTES};

const SECRET: &str = "agent-bus-served-secret";
const ENCRYPTION_KEY: &str = "agent-bus-served-encryption-key-0123456789";
const GRAPH: &str = "agentbus";
const TENANT: &str = "integration-test-tenant";
const NOW: u64 = 1_800_000_000_000;

fn participant(session_ref: &str, last_seen_ms: u64) -> BusParticipant {
    BusParticipant {
        session_ref: session_ref.to_string(),
        tenant: String::new(),
        host_ref: "host-a".to_string(),
        capabilities: ["code".to_string()].into(),
        last_seen_ms,
    }
}

fn register(session_ref: &str, last_seen_ms: u64) -> Method {
    Method::BusRegister {
        participant: participant(session_ref, last_seen_ms),
        principal: String::new(),
    }
}

fn heartbeat(session_ref: &str, now_ms: u64) -> Method {
    Method::BusHeartbeat {
        session_ref: session_ref.to_string(),
        now_ms,
        principal: String::new(),
    }
}

async fn send(state: &test_support::SharedState, id: u64, method: Method) -> Response {
    test_support::dispatch(state, test_support::request(SECRET, id, GRAPH, method)).await
}

async fn send_as(
    state: &test_support::SharedState,
    id: u64,
    context: medulla_types::acl::RequestContextClaims,
    method: Method,
) -> Response {
    test_support::dispatch(
        state,
        common::signed_request_with_context(SECRET, id, GRAPH, context, method),
    )
    .await
}

fn error_code(response: &Response) -> String {
    response.error.clone().unwrap_or_default()
}

fn decode_roster(response: Response) -> BusRoster {
    assert!(
        response.error.is_none(),
        "roster failed: {:?}",
        response.error
    );
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("typed roster"),
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes).expect("typed roster"),
        other => panic!("unexpected payload {other:?}"),
    }
}

async fn roster(state: &test_support::SharedState, id: u64, now_ms: u64) -> BusRoster {
    decode_roster(
        send(
            state,
            id,
            Method::BusRoster {
                tenant: TENANT.to_string(),
                now_ms,
            },
        )
        .await,
    )
}

/// The test isolation plus `root` as a second graph writer, so a foreign
/// heartbeat reaches the bus handler instead of the graph ACL.
fn isolation() -> medulla_isolation::isolation::IsolationLayer {
    let mut isolation = common::current_isolation();
    isolation.register_agent(medulla_types::acl::AgentIdentity {
        agent_id: "root".to_string(),
        role: medulla_types::acl::AgentRole::System,
        teams: Vec::new(),
        roles: Vec::new(),
    });
    isolation
}

async fn fresh() -> (
    std::path::PathBuf,
    test_support::SharedPersistence,
    test_support::SharedState,
) {
    test_support::durable_graph_state(
        SECRET,
        ENCRYPTION_KEY,
        "agent-bus-served",
        isolation(),
        GRAPH,
    )
    .await
}

// spec: MEDULLA-DURABLE-KERNEL-R095.1001
#[tokio::test(flavor = "multi_thread")]
async fn register_heartbeat_and_roster_mark_liveness() {
    let (_dir, _backend, state) = fresh().await;
    let ttl_ms = medulla_h_graph::graph_ops::bus::participant_ttl_s() * 1000;
    assert!(send(&state, 2, register("s-old", NOW))
        .await
        .error
        .is_none());
    assert!(send(&state, 3, register("s-new", NOW))
        .await
        .error
        .is_none());
    let later = NOW + ttl_ms + 1;
    assert!(send(&state, 4, heartbeat("s-new", later))
        .await
        .error
        .is_none());

    let rows = roster(&state, 5, later).await.participants;
    let live: Vec<(String, bool)> = rows
        .iter()
        .map(|row| (row.participant.session_ref.clone(), row.live))
        .collect();
    assert_eq!(
        live,
        vec![("s-new".to_string(), true), ("s-old".to_string(), false)]
    );
    assert_eq!(rows[0].participant.last_seen_ms, later);
    assert_eq!(rows[0].principal, common::TEST_AGENT);
}

// spec: MEDULLA-DURABLE-KERNEL-R095.1002
#[tokio::test(flavor = "multi_thread")]
async fn bus_methods_listed_and_reach_the_gateway() {
    let listed: BTreeSet<&str> = medulla_capabilities::method_policy_entries()
        .map(|(name, _, _)| name)
        .filter(|name| name.starts_with("Bus"))
        .collect();
    assert_eq!(
        listed,
        BTreeSet::from(["BusHeartbeat", "BusRegister", "BusRoster"])
    );
    for (name, policy, _) in medulla_capabilities::method_policy_entries() {
        match name {
            "BusRegister" | "BusHeartbeat" => assert_eq!(policy.authz_action, "bus:write"),
            "BusRoster" => assert_eq!(policy.authz_action, "bus:read"),
            _ => {}
        }
    }
    assert!(medulla_server_mutation::mutation::is_gateway_routed(
        &heartbeat("s", NOW)
    ));

    let (_dir, _backend, state) = fresh().await;
    assert!(send(&state, 2, register("s-1", NOW)).await.error.is_none());
    let version = |state: &test_support::SharedState| {
        state
            .try_read()
            .unwrap()
            .registry
            .get(GRAPH)
            .unwrap()
            .core
            .version()
    };
    let before = version(&state);
    let response = send(&state, 3, heartbeat("s-1", NOW + 1)).await;
    assert!(response.error.is_none(), "{:?}", response.error);
    assert!(matches!(response.result, Some(ResultPayload::Bool(true))));
    assert_eq!(
        version(&state),
        before + 1,
        "one heartbeat commits one batch"
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R095.1003
#[tokio::test(flavor = "multi_thread")]
async fn bus_refusals_write_nothing() {
    let (_dir, _backend, state) = fresh().await;
    assert!(send(&state, 2, register("s-1", NOW)).await.error.is_none());
    let baseline = roster(&state, 3, NOW).await;

    // No bus scope.
    let mut narrow = common::context_for(common::TEST_AGENT);
    narrow.scopes = vec!["node:read".to_string()];
    let refused = send_as(&state, 4, narrow.clone(), register("s-2", NOW)).await;
    assert!(error_code(&refused).starts_with("BUS_SCOPE_REQUIRED"));
    let refused = send_as(
        &state,
        5,
        narrow,
        Method::BusRoster {
            tenant: TENANT.to_string(),
            now_ms: NOW,
        },
    )
    .await;
    assert!(error_code(&refused).starts_with("BUS_SCOPE_REQUIRED"));

    // Over the bounds.
    let long = "x".repeat(MAX_SESSION_REF_BYTES + 1);
    let refused = send(&state, 6, register(&long, NOW)).await;
    assert!(error_code(&refused).starts_with("BUS_FIELD_TOO_LARGE"));
    let mut wide = participant("s-3", NOW);
    wide.capabilities = (0..65).map(|i| format!("c{i}")).collect();
    let refused = send(
        &state,
        7,
        Method::BusRegister {
            participant: wide,
            principal: String::new(),
        },
    )
    .await;
    assert!(error_code(&refused).starts_with("BUS_FIELD_TOO_LARGE"));

    // A heartbeat naming another principal's session.
    // A signed request's principal is its signer, so the other principal is
    // a second provisioned signer.
    let other = common::context_for("root");
    let refused = send_as(&state, 8, other, heartbeat("s-1", NOW + 5)).await;
    assert!(
        error_code(&refused).starts_with("BUS_SESSION_FOREIGN"),
        "{refused:?}"
    );

    // Another tenant sees none of these rows.
    let mut foreign = common::context_for(common::TEST_AGENT);
    foreign.tenant = "another-tenant".to_string();
    let foreign_roster = send_as(
        &state,
        9,
        foreign,
        Method::BusRoster {
            tenant: TENANT.to_string(),
            now_ms: NOW,
        },
    )
    .await;
    if foreign_roster.error.is_none() {
        assert!(decode_roster(foreign_roster).participants.is_empty());
    }

    assert_eq!(
        roster(&state, 10, NOW).await,
        baseline,
        "refusals wrote nothing"
    );

    // The roster bound: BUS_ROSTER_FULL is asserted against the Fixed constant.
    assert_eq!(
        medulla_types::agent_bus::MAX_PARTICIPANTS_PER_TENANT,
        10_000
    );
}

// spec: MEDULLA-DURABLE-KERNEL-R095.1000
// spec: MEDULLA-DURABLE-KERNEL-R095.1004
#[tokio::test(flavor = "multi_thread")]
async fn roster_survives_restart() {
    let (dir, backend, state) = fresh().await;
    let dir_s = dir.to_string_lossy().to_string();
    for i in 0..100u64 {
        let session = format!("s-{:03}", 99 - i);
        let response = send(&state, 10 + i, register(&session, NOW + i)).await;
        assert!(response.error.is_none(), "{:?}", response.error);
    }
    let before = roster(&state, 200, NOW + 100).await;
    assert_eq!(before.participants.len(), 100);
    let order: Vec<&str> = before
        .participants
        .iter()
        .map(|row| row.participant.session_ref.as_str())
        .collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted, "roster is session_ref ascending");

    state.write().await.persistence = None;
    drop(state);
    let reopened = test_support::reopen_after_sole_reference(
        backend,
        || test_support::open_redb_backend(dir_s.clone()),
        "reopen agent bus backend",
    )
    .await;
    let restarted = test_support::state_with(
        SECRET,
        isolation(),
        Some(dir_s.clone()),
        Some(reopened.clone()),
    );
    // A restarted engine recovers its durable graphs before it serves.
    reopened
        .load_all(&restarted)
        .await
        .expect("reload durable graphs");
    let after = roster(&restarted, 201, NOW + 100).await;
    assert_eq!(
        rmp_serde::to_vec_named(&after).unwrap(),
        rmp_serde::to_vec_named(&before).unwrap(),
        "the roster survives restart byte for byte"
    );
}
