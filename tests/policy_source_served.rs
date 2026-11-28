//! Served policy source binding (MEDULLA-SRC-AUTH-R801).
//!
//! `Method::PolicySourceBind` is driven through the real request boundary: a
//! `DecisionPolicy` component is published and bound, a newer head is published,
//! the stale bind is refused with the activation node unchanged, and a restarted
//! engine reads the same activation. A policy of another tenant is not bound.
#![cfg(all(feature = "query", feature = "redb", feature = "security"))]

mod common;
#[path = "common/test_support.rs"]
mod test_support;

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_agent_types::agent_component::{AgentComponentKind, AgentComponentPublishRequest};
use medulla_decision_statistical_types::decision::digest::policy_part::policy_digest;
use medulla_decision_statistical_types::decision::policy::DECISION_POLICY_ATTRIBUTE;
use medulla_decision_statistical_types::decision::DecisionPolicy;
use medulla_persistence_records::agent_component::test_component_draft;
use medulla_persistence_records::agent_fixtures::mutation_context;
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_protocol_types::protocol::{Method, Response, ResultPayload};
use medulla_server_api::state::ServerState;
use medulla_types::policy_source::{PolicySourceBind, PolicySourceBinding};

const SECRET: &str = "policy-source-served-secret";
const GRAPH: &str = "__commons__";
const POLICY: &str = "policy-a";
const NODE: &str = "policy_source:policy-a";
const SOURCE: &str = "source-a";
const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

/// Publish `POLICY` for `tenant` at `version`, backed by the reviewed source,
/// and return the digest of its head (64 lowercase hex).
async fn publish(
    state: &Arc<RwLock<ServerState>>,
    tenant: &str,
    id: &str,
    version: &str,
    nonce: u8,
    expected_revision: u64,
) -> String {
    let store = state.write().await.ensure_agent_library().expect("library");
    let mut policy = DecisionPolicy::engine_default();
    // A distinct policy per publish gives each head its own content digest.
    policy.node_budget += u64::from(nonce);
    let mut draft = test_component_draft(tenant, id);
    draft.kind = AgentComponentKind::DecisionPolicy;
    draft.version = version.to_string();
    draft.content_digest = policy_digest(&policy);
    draft.content_ref = Some(format!("{SOURCE}@{REVISION}"));
    draft.attributes.insert(
        DECISION_POLICY_ATTRIBUTE.to_string(),
        serde_json::to_string(&policy).expect("policy json"),
    );
    let request = AgentComponentPublishRequest {
        context: mutation_context(
            &store,
            tenant,
            &format!("publish-{id}-{version}"),
            nonce,
            expected_revision,
            "agent-component:publish",
        ),
        component: draft,
        evaluation_receipt_digest: None,
        body: None,
    };
    let committed = store
        .publish_component(request, None)
        .expect("publish policy");
    committed
        .result
        .component
        .content_digest
        .trim_start_matches("sha256:")
        .to_string()
}

fn bind_request(policy: &str, version: &str, digest: &str) -> PolicySourceBind {
    PolicySourceBind {
        source_ref: SOURCE.to_string(),
        source_revision: REVISION.to_string(),
        source_digest: digest.to_string(),
        policy_iri: policy.to_string(),
        component_version: version.to_string(),
        approver: "reviewer".to_string(),
    }
}

async fn bind(state: &test_support::SharedState, id: u64, request: PolicySourceBind) -> Response {
    test_support::dispatch(
        state,
        common::signed_request_with_scopes(
            SECRET,
            id,
            GRAPH,
            &["kg:admin"],
            Method::PolicySourceBind { request },
        ),
    )
    .await
}

fn binding(response: Response) -> PolicySourceBinding {
    assert!(
        response.error.is_none(),
        "bind failed: {:?}",
        response.error
    );
    match response.result {
        Some(ResultPayload::Json(value)) => serde_json::from_value(value).expect("typed record"),
        other => panic!("unexpected payload {other:?}"),
    }
}

async fn node(state: &test_support::SharedState, id: u64) -> Option<Vec<u8>> {
    let response = test_support::dispatch(
        state,
        test_support::request(
            SECRET,
            id,
            GRAPH,
            Method::GetNodeProperties {
                node_id: NODE.to_string(),
            },
        ),
    )
    .await;
    assert!(response.error.is_none(), "GetNodeProperties: {response:?}");
    format!("{:?}", response.result).into_bytes().into()
}

// spec: MEDULLA-SRC-AUTH-R801
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_review_leaves_policy_unchanged() {
    test_support::provision_encryption_key_once("policy-source-served-encryption-key");
    let dir = test_support::fresh_dir("medulla-policy-source");
    let dir_s = dir.to_string_lossy().to_string();
    let (backend, state) = test_support::redb_state_at(SECRET, common::current_isolation(), &dir_s);

    let digest = publish(&state, common::TEST_TENANT, POLICY, "1.0.0", 1, 0).await;
    let bound = binding(bind(&state, 1, bind_request(POLICY, "1.0.0", &digest)).await);
    assert_eq!(bound.policy_iri, POLICY);
    assert_eq!(bound.source_digest, digest);
    let active = node(&state, 2).await;

    // A newer head leaves the reviewed version behind: the stale bind is refused
    // and the activation node is byte-for-byte what it was.
    let newer = publish(&state, common::TEST_TENANT, POLICY, "2.0.0", 2, 1).await;
    let stale = bind(&state, 3, bind_request(POLICY, "1.0.0", &newer)).await;
    let error = stale.error.expect("the stale review is refused");
    assert!(
        error.starts_with("POLICY_SOURCE_STALE"),
        "unexpected refusal: {error}"
    );
    assert_eq!(
        node(&state, 4).await,
        active,
        "a refusal leaves the activation unchanged"
    );

    // A digest the head does not hold is a mismatch, and writes nothing either.
    let mismatch = bind(&state, 5, bind_request(POLICY, "2.0.0", &digest)).await;
    let error = mismatch.error.expect("the digest mismatch is refused");
    assert!(
        error.starts_with("POLICY_SOURCE_DIGEST_MISMATCH"),
        "unexpected refusal: {error}"
    );
    assert_eq!(node(&state, 6).await, active);

    // A policy published for another tenant is no policy of this caller.
    publish(&state, "other-tenant", "policy-b", "1.0.0", 3, 0).await;
    let foreign = bind(&state, 7, bind_request("policy-b", "1.0.0", &digest)).await;
    let error = foreign.error.expect("a foreign tenant's policy is refused");
    assert!(
        error.starts_with("INVALID_ARGUMENT"),
        "unexpected refusal: {error}"
    );

    // The current review binds the newer head, replacing the one record.
    let rebound = binding(bind(&state, 8, bind_request(POLICY, "2.0.0", &newer)).await);
    assert_eq!(rebound.component_version, "2.0.0");
    assert_eq!(rebound.source_digest, newer);
    let activated = node(&state, 9).await;
    assert_ne!(activated, active);

    // Restart: the same activation is read back from the durable tier.
    backend.shutdown();
    state.write().await.persistence = None;
    drop(state);
    drop(backend);
    let reopened = test_support::reopen_with_bounded_retry(
        || test_support::open_redb_backend(dir_s.clone()),
        "reopen durable tier",
    )
    .await;
    let restarted = test_support::state_with(
        SECRET,
        common::current_isolation(),
        Some(dir_s.clone()),
        Some(reopened.clone()),
    );
    reopened
        .load_all(&restarted)
        .await
        .expect("reload durable graphs");
    assert_eq!(
        node(&restarted, 20).await,
        activated,
        "the activation survives a restart"
    );

    reopened.shutdown();
    restarted.write().await.persistence = None;
    drop(restarted);
    let _ = std::fs::remove_dir_all(&dir);
}
