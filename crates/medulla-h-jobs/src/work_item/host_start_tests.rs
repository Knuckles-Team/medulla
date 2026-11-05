//! Signed dispatch to native provenance. These tests need the redb server build;
//! they do not qualify deployment identity, current sessions, or host launch.
use super::*;
use medulla_persistence::redb_backend::RedbBackend;
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::dispatch_test_on_heap;
use medulla_server_auth::auth::scoped_test_request_on;
use medulla_server_auth::auth::ScopedTestCaller;
use medulla_work_types::control_lease::ControlLeaseIssued;
use medulla_work_types::host_start::{
    HostStartBinding, HostStartInterfaces, IssueHostStartGrantRequest,
};
use tokio::sync::RwLock;

// spec: MEDULLA-DURABLE-KERNEL-R030.1.3
const SECRET: &str = "host-start-contract-test-secret";
const ISSUER: &str = "host-start-test-issuer";
const TENANT: &str = "tenant-shared";
const GRAPH: &str = "host-start-test";

async fn call(
    state: &Arc<RwLock<ServerState>>,
    id: u64,
    key: &str,
    scopes: &[&str],
    method: Method,
) -> Response {
    let request = scoped_test_request_on(
        SECRET,
        id,
        GRAPH,
        method,
        ScopedTestCaller {
            principal: ISSUER,
            tenant: TENANT,
            scopes,
            nonce: &format!("host-start-nonce-{id}"),
            idempotency_key: key,
        },
    );
    dispatch_test_on_heap(state, request).await
}

fn decode(response: Response) -> ControlLeaseIssued {
    assert!(response.error.is_none(), "{:?}", response.error);
    match response.result.unwrap() {
        medulla_protocol_types::protocol::ResultPayload::Json(value) => {
            serde_json::from_value(value).unwrap()
        }
        medulla_protocol_types::protocol::ResultPayload::Raw(bytes) => {
            rmp_serde::from_slice(&bytes).unwrap()
        }
        other => panic!("unexpected host-start payload {other:?}"),
    }
}

async fn fixture() -> (
    tempfile::TempDir,
    Arc<RwLock<ServerState>>,
    Arc<RedbBackend>,
    IssueHostStartGrantRequest,
) {
    let dir = tempfile::tempdir().unwrap();
    let mut state = medulla_server::seams_install::new_state_for_test(
        SECRET,
        ServerState::test_isolation(ISSUER),
    );
    state.persist_dir = Some(dir.path().to_string_lossy().into_owned());
    let backend =
        RedbBackend::open(dir.path().join("graphs").to_string_lossy().into_owned(), 64).unwrap();
    let backend = Arc::new(backend);
    state.persistence = Some(backend.clone());
    let state = Arc::new(RwLock::new(state));
    let create = scoped_test_request_on(
        SECRET,
        1,
        "__commons__",
        Method::CreateGraph {
            graph_name: GRAPH.into(),
            graph_type: medulla_protocol_types::protocol::GraphType::Global,
        },
        ScopedTestCaller {
            principal: ISSUER,
            tenant: TENANT,
            scopes: &["*"],
            nonce: "host-create",
            idempotency_key: "host-create",
        },
    );
    let created = dispatch_test_on_heap(&state, create).await;
    assert!(created.error.is_none(), "{:?}", created.error);
    let now = medulla_server_api::txn::now_ms();
    let request = IssueHostStartGrantRequest {
        tenant: TENANT.into(),
        lease_id: "host-grant".into(),
        binding: HostStartBinding {
            principal_ref: "beneficiary".into(),
            session_ref: "opaque-session".into(),
            workspace_ref: "workspace".into(),
            configuration_digest: "a".repeat(64),
            interfaces: HostStartInterfaces::Terminal,
            web_logs: false,
            policy_revision: "policy-1".into(),
            session_expires_at_ms: now + 60_000,
        },
        expires_at_ms: now + 60_000,
        idempotency_key: "host-issue".into(),
    };
    (dir, state, backend, request)
}

#[tokio::test]
async fn signed_issuer_is_stamped_and_retry_retains_attribution() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (_dir, state, backend, request) = fixture().await;
    let before = authority_snapshot(&backend).await;
    let mut denied_request = request.clone();
    denied_request.idempotency_key = "denied-issue".into();
    let denied = call(
        &state,
        2,
        "denied-issue",
        &["lease:write"],
        Method::IssueHostStartGrant {
            request: denied_request,
        },
    )
    .await;
    assert!(denied.result.is_none(), "denial must not produce a result");
    let error = denied
        .error
        .as_deref()
        .expect("generic lease writer must be refused");
    // Public dispatch intentionally strips private policy details.
    assert_eq!(error, "ACCESS_DENIED");
    assert_eq!(authority_snapshot(&backend).await, before);
    let issued = decode(
        call(
            &state,
            3,
            "host-issue",
            &["host:issue-start"],
            Method::IssueHostStartGrant {
                request: request.clone(),
            },
        )
        .await,
    );
    let retry = decode(
        call(
            &state,
            4,
            "host-issue",
            &["host:issue-start"],
            Method::IssueHostStartGrant { request },
        )
        .await,
    );
    assert_eq!(issued, retry);
    let provenance = issued.lease.unwrap().host_start_provenance.unwrap();
    assert_eq!(
        provenance.issuer_ref,
        medulla_server_api::batch_compile::principal_fingerprint(ISSUER).unwrap()
    );
    assert_ne!(provenance.issuer_ref, ISSUER);
}

async fn authority_snapshot(backend: &RedbBackend) -> serde_json::Value {
    let version = backend.read_mutation_graph_version(GRAPH).await.unwrap();
    let own = backend
        .read_control_lease(GRAPH, TENANT, "host-grant")
        .await
        .unwrap();
    let foreign = backend
        .read_control_lease(GRAPH, "foreign-tenant", "host-grant")
        .await
        .unwrap();
    serde_json::to_value((version, own, foreign)).unwrap()
}

#[tokio::test]
async fn signed_wrong_tenant_and_forged_provenance_refuse_without_mutation() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (_dir, state, backend, request) = fixture().await;
    let before = authority_snapshot(&backend).await;
    assert!(before[1].is_null() && before[2].is_null());

    let mut foreign = request.clone();
    foreign.tenant = "foreign-tenant".into();
    foreign.idempotency_key = "foreign-issue".into();
    let mut forged = request.lease_at(medulla_server_api::txn::now_ms()).unwrap();
    forged.idempotency_key = "forged-issue".into();
    forged.grant.insert(
        "host_start_provenance".into(),
        serde_json::json!({"schema_version": 1, "issuer_ref": "forged-issuer"}),
    );
    for (id, key, method, scenario) in [
        (
            20,
            "foreign-issue",
            Method::IssueHostStartGrant { request: foreign },
            "foreign body tenant",
        ),
        (
            21,
            "forged-issue",
            Method::IssueControlLease { request: forged },
            "forged generic host-start grant",
        ),
    ] {
        let denied = call(
            &state,
            id,
            key,
            &["host:issue-start", "lease:write"],
            method,
        )
        .await;
        assert!(denied.result.is_none(), "refusal must not return a grant");
        let error = denied
            .error
            .as_deref()
            .expect("signed invalid authority must refuse");
        assert_eq!(error, "ACCESS_DENIED", "{scenario}");
        assert_eq!(authority_snapshot(&backend).await, before);
    }
    let issued = decode(
        call(
            &state,
            22,
            "host-issue",
            &["host:issue-start"],
            Method::IssueHostStartGrant { request },
        )
        .await,
    );
    assert_eq!(
        issued.outcome,
        medulla_work_types::control_lease::ControlLeaseIssueOutcome::Issued
    );
    assert_eq!(
        issued
            .lease
            .unwrap()
            .host_start_provenance
            .unwrap()
            .issuer_ref,
        medulla_server_api::batch_compile::principal_fingerprint(ISSUER).unwrap()
    );
}

fn payload_json(response: Response) -> serde_json::Value {
    assert!(response.error.is_none(), "{:?}", response.error);
    match response.result.unwrap() {
        medulla_protocol_types::protocol::ResultPayload::Json(value) => value,
        medulla_protocol_types::protocol::ResultPayload::Raw(bytes) => {
            rmp_serde::from_slice(&bytes).unwrap()
        }
        other => panic!("unexpected payload {other:?}"),
    }
}

/// The installed Python client (`ControlLeaseClient` / `_control_lease_view`)
/// accepts exactly these view keys; the arbora consumer reads through it.
const CLIENT_VIEW_FIELDS: &[&str] = &[
    "expires_at_ms",
    "grant",
    "hard_expires_at_ms",
    "host_start_provenance",
    "issued_at_ms",
    "kind",
    "lease_id",
    "revision",
    "status",
];

// spec: MEDULLA-DURABLE-KERNEL-R030.1.4
#[tokio::test]
async fn signed_dispatch_issues_and_arbora_client_reads_the_grant() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (_dir, state, _backend, request) = fixture().await;
    let issued = payload_json(
        call(
            &state,
            30,
            "host-issue",
            &["host:issue-start"],
            Method::IssueHostStartGrant {
                request: request.clone(),
            },
        )
        .await,
    );
    let typed: ControlLeaseIssued = serde_json::from_value(issued.clone()).unwrap();
    assert!(typed.lease.is_some(), "issue must return the lease view");
    let read = payload_json(
        call(
            &state,
            31,
            "host-read",
            &["lease:read"],
            Method::GetControlLease {
                tenant: TENANT.into(),
                lease_id: "host-grant".into(),
            },
        )
        .await,
    );
    let view = if read.get("lease_id").is_some() {
        read.clone()
    } else {
        read.get("lease").cloned().unwrap_or(read.clone())
    };
    let keys: Vec<&str> = {
        let mut keys: Vec<&str> = view
            .as_object()
            .expect("lease view is an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    };
    assert_eq!(keys, CLIENT_VIEW_FIELDS, "client view contract drifted");
    assert_eq!(
        view["kind"],
        medulla_work_types::host_start::HOST_START_KIND
    );
    let binding: HostStartBinding = serde_json::from_value(view["grant"].clone())
        .expect("grant carries exactly the Rust HostStartBinding fields");
    assert_eq!(binding, request.binding);
    assert_eq!(
        view["host_start_provenance"]["issuer_ref"],
        serde_json::json!(
            medulla_server_api::batch_compile::principal_fingerprint(ISSUER).unwrap()
        )
    );
    assert_eq!(view, issued["lease"]);
}
