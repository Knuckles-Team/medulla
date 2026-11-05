//! MEDULLA-FEDERATED-QUERY-R088.2: served `Method::Uql` applies row visibility before
//! `LIMIT` and ranking trims, so rows hidden from the caller never displace visible rows,
//! and `WITH KNOWLEDGE` resolves over the same snapshot without changing the row set.
//!
//! The fixture is adversarial: every hidden row is a closer vector match than every
//! visible row, and `LIMIT 5` equals the visible-row count. If visibility ran after the
//! trim, the caller would receive fewer than five rows (or leak a hidden id).
// spec: MEDULLA-FEDERATED-QUERY-R088.2
#![cfg(all(feature = "query", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use serde_json::json;

use medulla_protocol_types::protocol::{Method, Request};
use medulla_types::acl::AgentRole;

const SECRET: &str = "uql-governed-summary-secret";

/// RBAC role every non-`System` registered peer needs on `__commons__` -- under
/// `feature = "security"` there is no pre-RBAC Commons-open-to-all fall-through for a
/// non-`System` identity (see `commons_isolation` below and
/// `advanced_crossmodal_roundtrip.rs`'s identical fixture for the full rationale).
const COMMONS_USER_ROLE: &str = "commons-user";

async fn state() -> test_support::SharedState {
    let (persist_dir, persistence) = common::tempdir_persistence();
    test_support::state_with(SECRET, commons_isolation(), persist_dir, persistence)
}

/// `common::current_isolation()` plus a `"commons-user"` RBAC role granted Read+Write on
/// `__commons__` -- the identical fixture shape `advanced_crossmodal_roundtrip.rs`'s
/// `commons_isolation` uses, needed for the same reason: `common::TEST_AGENT` is
/// `System`-role and bypasses RBAC, but the `alice`/`bob` peers this file registers
/// (plain `AgentRole::Agent`) do not, or every RPC they make is `ACCESS_DENIED` before
/// row-level RLS is ever reached.
fn commons_isolation() -> medulla_isolation::isolation::IsolationLayer {
    let mut isolation = common::current_isolation();
    use medulla_types::acl::{Grant, GrantEffect, RbacAction, ResourceSelector, Role};
    isolation.add_role(Role::new(COMMONS_USER_ROLE));
    let grant = |action: RbacAction| Grant {
        role: COMMONS_USER_ROLE.to_string(),
        resource: ResourceSelector::Graph("__commons__".to_string()),
        action,
        effect: GrantEffect::Allow,
    };
    isolation.add_grant(grant(RbacAction::Read));
    isolation.add_grant(grant(RbacAction::Write));

    // NE-065 migration: `root` is registered `AgentRole::Agent` (not `System`, see
    // `register_root_req`), so it needs an explicit grant of `RbacAction::Admin` on
    // the `"__admin__"` resource context `IsolationLayer::has_admin_capability`
    // checks -- exactly the capability `Method::RegisterIdentity` requires of its
    // actor, and nothing more.
    isolation.add_role(Role::new(common::ROOT_ADMIN_ROLE));
    isolation.add_grant(Grant {
        role: common::ROOT_ADMIN_ROLE.to_string(),
        resource: ResourceSelector::Graph("__admin__".to_string()),
        action: RbacAction::Admin,
        effect: GrantEffect::Allow,
    });
    isolation
}

fn req(id: u64, method: Method) -> Request {
    common::signed_request(SECRET, id, "__commons__", method)
}

fn req_as(id: u64, agent: &str, method: Method) -> Request {
    common::signed_request_as(SECRET, id, "__commons__", agent, method)
}

fn pack(v: serde_json::Value) -> Vec<u8> {
    rmp_serde::to_vec_named(&v).unwrap()
}

async fn ok(state: &test_support::SharedState, id: u64, method: Method) {
    let r = Box::pin(dispatch_req(state, req(id, method))).await;
    assert!(r.error.is_none(), "op {id} failed: {:?}", r.error);
}

fn register_identity_req(id: u64, actor: &str, agent_id: &str, role: AgentRole) -> Request {
    common::signed_register_identity_request(common::SignedRegisterIdentity {
        secret: SECRET,
        id,
        graph: "__commons__",
        actor,
        registered_agent: agent_id,
        role,
        teams: Vec::new(),
        roles: vec![COMMONS_USER_ROLE.to_string()],
    })
}

/// Registers `"root"` as an ordinary `AgentRole::Agent` identity carrying
/// [`common::ROOT_ADMIN_ROLE`] -- NOT `AgentRole::System` -- so it holds just enough
/// authority (`RbacAction::Admin` on `"__admin__"`, granted by [`commons_isolation`])
/// to go on and call `RegisterIdentity` itself for `alice`/`bob` below. See
/// `advanced_crossmodal_roundtrip.rs`'s identical helper for the full NE-065
/// rationale (a signer scoped to bootstrap itself must not also mint an independent
/// `System` identity for a third party, and self-registering `root` as `System`
/// cannot work either since `RegisterIdentity`'s actor must already hold admin
/// capability).
fn register_root_req(id: u64, actor: &str) -> Request {
    common::signed_register_identity_request(common::SignedRegisterIdentity {
        secret: SECRET,
        id,
        graph: "__commons__",
        actor,
        registered_agent: "root",
        role: AgentRole::Agent,
        teams: Vec::new(),
        roles: vec![common::ROOT_ADMIN_ROLE.to_string()],
    })
}

const PROBE: &str = "~[0.0, 1.0]";

/// Five `Robot` nodes private to `bob` whose embeddings are nearest the probe, and five
/// visible to `alice` (owned by her) whose embeddings are strictly farther. Then `alice`
/// and `bob` are registered, flipping the graph into row-level-security mode.
async fn seed(state: &test_support::SharedState) {
    let mut id = 1u64;
    for i in 0..5 {
        ok(
            state,
            id,
            Method::AddNode {
                node_id: format!("bob-{i}"),
                properties_msgpack: pack(json!({
                    "type": "Robot", "_owner": "bob", "_visibility": "private",
                })),
            },
        )
        .await;
        ok(
            state,
            id + 1,
            Method::AddEmbedding {
                node_id: format!("bob-{i}"),
                embedding: vec![0.0, 1.0 - 0.01 * i as f32],
            },
        )
        .await;
        id += 2;
    }
    for i in 0..5 {
        ok(
            state,
            id,
            Method::AddNode {
                node_id: format!("alice-{i}"),
                properties_msgpack: pack(json!({
                    "type": "Robot", "_owner": "alice", "_visibility": "private",
                })),
            },
        )
        .await;
        ok(
            state,
            id + 1,
            Method::AddEmbedding {
                node_id: format!("alice-{i}"),
                embedding: vec![0.9 - 0.05 * i as f32, 0.1 + 0.05 * i as f32],
            },
        )
        .await;
        id += 2;
    }
    let r = Box::pin(dispatch_req(
        state,
        register_root_req(900, common::TEST_AGENT),
    ))
    .await;
    assert!(r.error.is_none(), "root registration failed: {:?}", r.error);
    for (i, agent) in [(901u64, "alice"), (902, "bob")] {
        let r = Box::pin(dispatch_req(
            state,
            register_identity_req(i, "root", agent, AgentRole::Agent),
        ))
        .await;
        assert!(r.error.is_none(), "RegisterIdentity {agent}: {:?}", r.error);
    }
}

async fn dispatch_req(
    state: &test_support::SharedState,
    request: Request,
) -> medulla_protocol_types::protocol::Response {
    medulla_server_router::dispatch::request_boundary::dispatch(state, request).await
}

async fn uql_as(
    state: &test_support::SharedState,
    id: u64,
    agent: &str,
    text: &str,
) -> Vec<String> {
    let response = Box::pin(dispatch_req(
        state,
        req_as(id, agent, test_support::uql(text)),
    ))
    .await;
    assert!(
        response.error.is_none(),
        "UQL as {agent} failed: {:?}",
        response.error
    );
    test_support::uql_ids(&response)
}

// spec: MEDULLA-FEDERATED-QUERY-R088.2
#[tokio::test]
async fn hidden_rows_do_not_displace_visible_rows() {
    let state = state().await;
    seed(&state).await;

    let plain = format!("MATCH (:Robot) |> RANK BY {PROBE} |> LIMIT 5");
    let knowledge = format!("{plain} WITH KNOWLEDGE");

    // Sanity: bob (the owner of the closest rows) really does rank them first, so the
    // negative assertion for alice is not vacuous.
    let bob_ids = uql_as(&state, 10, "bob", &plain).await;
    assert_eq!(bob_ids.len(), 5, "bob sees five rows: {bob_ids:?}");
    assert!(
        bob_ids.iter().all(|id| id.starts_with("bob-")),
        "{bob_ids:?}"
    );

    let alice_ids = uql_as(&state, 11, "alice", &plain).await;
    assert_eq!(
        alice_ids.len(),
        5,
        "hidden rows must not consume the LIMIT budget: {alice_ids:?}"
    );
    assert!(
        alice_ids.iter().all(|id| id.starts_with("alice-")),
        "no hidden id may appear for alice: {alice_ids:?}"
    );

    // WITH KNOWLEDGE resolves over the same snapshot: same rows, same order.
    let alice_knowledge = uql_as(&state, 12, "alice", &knowledge).await;
    assert_eq!(
        alice_knowledge, alice_ids,
        "WITH KNOWLEDGE keeps the visible row order"
    );
}

/// One served UQL answer: the row ids and the `(snapshot_revision,
/// knowledge_revision)` header.
async fn uql_header_as(
    state: &test_support::SharedState,
    request: Request,
) -> (Vec<String>, u64, u64, Vec<bool>) {
    use medulla_types::wire::UqlResult;
    let response = Box::pin(dispatch_req(state, request)).await;
    assert!(response.error.is_none(), "UQL failed: {:?}", response.error);
    let Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) =
        response.result.as_ref()
    else {
        panic!("expected Raw result, got {:?}", response.result);
    };
    match rmp_serde::from_slice(bytes).unwrap() {
        UqlResult::Rows {
            rows,
            snapshot_revision,
            knowledge_revision,
            ..
        }
        | UqlResult::Profile {
            rows,
            snapshot_revision,
            knowledge_revision,
            ..
        } => (
            rows.iter().map(|row| row.id.clone()).collect(),
            snapshot_revision,
            knowledge_revision,
            rows.iter().map(|row| row.knowledge.is_some()).collect(),
        ),
        UqlResult::Explain { .. } => panic!("an EXPLAIN result has no rows"),
    }
}

/// The knowledge revision the header must carry for a snapshot revision.
fn expected_knowledge_revision(snapshot_revision: u64) -> u64 {
    if cfg!(feature = "epistemic") {
        snapshot_revision
    } else {
        0
    }
}

// spec: MEDULLA-FEDERATED-QUERY-R088.3
#[tokio::test]
async fn rows_and_header_share_one_snapshot() {
    let state = state().await;
    let add = |node: &str| Method::AddNode {
        node_id: node.to_string(),
        properties_msgpack: pack(json!({ "type": "Probe" })),
    };
    ok(&state, 1, add("probe-0")).await;
    let text = "MATCH (:Probe) WITH KNOWLEDGE";

    let (first_rows, first_rev, first_k, first_knowledge) =
        uql_header_as(&state, req(10, test_support::uql(text))).await;
    assert_eq!(first_rows, vec!["probe-0".to_string()]);
    assert!(first_rev > 0, "the header names a real snapshot revision");
    assert_eq!(first_k, expected_knowledge_revision(first_rev));
    assert!(first_knowledge.iter().all(|k| *k), "{first_knowledge:?}");

    // A repeat read with no write in between names the same snapshot.
    let (again_rows, again_rev, _, _) =
        uql_header_as(&state, req(11, test_support::uql(text))).await;
    assert_eq!((again_rows, again_rev), (first_rows.clone(), first_rev));

    // A write between two reads moves the header together with the rows.
    ok(&state, 2, add("probe-1")).await;
    let (second_rows, second_rev, second_k, second_knowledge) =
        uql_header_as(&state, req(12, test_support::uql(text))).await;
    let mut sorted = second_rows.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["probe-0".to_string(), "probe-1".to_string()]);
    assert!(
        second_rev > first_rev,
        "the header must name the newer snapshot the rows came from: {first_rev} -> {second_rev}"
    );
    assert_eq!(second_k, expected_knowledge_revision(second_rev));
    assert_eq!(second_knowledge.len(), second_rows.len());
    assert!(second_knowledge.iter().all(|k| *k));
}

// spec: MEDULLA-FEDERATED-QUERY-R088.6
#[tokio::test]
async fn governed_rows_carry_knowledge_and_header() {
    let state = state().await;
    seed(&state).await;

    let plain = format!("MATCH (:Robot) |> RANK BY {PROBE} |> LIMIT 5");
    let knowledge = format!("{plain} WITH KNOWLEDGE");

    let (plain_rows, plain_rev, _, _) =
        uql_header_as(&state, req_as(20, "alice", test_support::uql(&plain))).await;
    let (rows, rev, k_rev, annotated) =
        uql_header_as(&state, req_as(21, "alice", test_support::uql(&knowledge))).await;

    assert_eq!(rows.len(), 5, "{rows:?}");
    assert!(
        rows.iter().all(|id| id.starts_with("alice-")),
        "hidden rows must be absent: {rows:?}"
    );
    assert_eq!(rows, plain_rows, "WITH KNOWLEDGE keeps the row order");
    assert!(annotated.iter().all(|k| *k), "every row carries knowledge");
    assert_eq!(rev, plain_rev, "the header names the snapshot of the rows");
    assert_eq!(k_rev, expected_knowledge_revision(rev));

    // The second principal sees its own rows under the same snapshot header.
    let (bob_rows, bob_rev, _, _) =
        uql_header_as(&state, req_as(22, "bob", test_support::uql(&knowledge))).await;
    assert!(
        bob_rows.iter().all(|id| id.starts_with("bob-")),
        "{bob_rows:?}"
    );
    assert_eq!(bob_rev, rev);
}
