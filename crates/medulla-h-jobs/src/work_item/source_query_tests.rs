//! Signed dispatch of `ApproveSourceQuery` (MEDULLA-FEDERATED-QUERY-R085.2). These tests need
//! the redb server build with `federation-sql`; no external SQL runs during approval.
use super::*;
use medulla_catalog_types::connector_pack::{
    ConnectorPackSourceApprovalResult, PackHeadRef, PackSourceApproval, PackSourceKind,
    PackSourceRegistration, CONNECTOR_PACK_SCHEMA_VERSION,
};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_persistence_records::agent_library::AgentLibraryStateExt;
use medulla_result_types::source_query::{
    SourceQueryDialect, SourceQueryLimits, SourceQueryParameter, SourceQueryRequest,
    SourceQueryValue,
};
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::dispatch_test_on_heap;
use medulla_server_auth::auth::scoped_test_request_on;
use medulla_server_auth::auth::ScopedTestCaller;
use medulla_types::contract::{Digest256, ResourceId};
use medulla_work_types::source_query_approval::{
    ApproveSourceQueryRequest, SOURCE_QUERY_APPROVAL_KIND,
};
use tokio::sync::RwLock;

const SECRET: &str = "source-query-approval-test-secret";
const ISSUER: &str = "source-query-test-issuer";
const TENANT: &str = "tenant-shared";
const GRAPH: &str = "source-query-test";

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
            nonce: &format!("source-query-nonce-{id}"),
            idempotency_key: key,
        },
    );
    dispatch_test_on_heap(state, request).await
}

async fn fixture(
    with_agent_library: bool,
) -> (tempfile::TempDir, Arc<RwLock<ServerState>>, Arc<RedbBackend>) {
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
    if with_agent_library {
        state.ensure_agent_library().expect("agent library opens");
    }
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
            nonce: "source-query-create",
            idempotency_key: "source-query-create",
        },
    );
    let created = dispatch_test_on_heap(&state, create).await;
    assert!(created.error.is_none(), "{:?}", created.error);
    (dir, state, backend)
}

fn approval() -> ConnectorPackSourceApprovalResult {
    let connector = ResourceId::new("connector").expect("connector id");
    ConnectorPackSourceApprovalResult {
        schema_version: CONNECTOR_PACK_SCHEMA_VERSION,
        tenant_id: TENANT.into(),
        connector: connector.clone(),
        head: PackHeadRef {
            binding_revision: 1,
            pack_digest: Digest256::sha256(b"head"),
        },
        registration: PackSourceRegistration {
            connector,
            contract_iri: "ac:contract".into(),
            resource_uri: "mcp://connector/read".into(),
            source_kind: PackSourceKind::McpTool,
            approval: PackSourceApproval::Approved,
            class_iri: "ac:Issue".into(),
            source_id: "connector.issue.read".into(),
            operation: "read".into(),
            entity: "issue".into(),
            key_field: "id".into(),
        },
        contract_digest: Digest256::sha256(b"contract"),
        batch_id: "batch".into(),
        committed_version: 1,
    }
}

fn request() -> ApproveSourceQueryRequest {
    ApproveSourceQueryRequest {
        tenant: TENANT.into(),
        query: SourceQueryRequest {
            schema_version: 1,
            source_ref: "unregistered-source".into(),
            source_revision: "1".into(),
            schema_digest: Digest256::sha256(b"contract").to_hex(),
            snapshot_ref: Digest256::sha256(b"head").to_hex(),
            dialect: SourceQueryDialect::Postgres,
            sql: "SELECT id FROM issues WHERE state = $1".into(),
            parameters: vec![SourceQueryParameter {
                position: 1,
                value: SourceQueryValue::Text("open".into()),
            }],
            limits: SourceQueryLimits {
                max_rows: 10,
                max_bytes: 1_048_576,
                chunk_rows: 5,
                chunk_bytes: 65_536,
                queue_depth: 2,
                queue_bytes: 1_048_576,
                deadline_ms: 10_000,
                max_resident_bytes: 8_388_608,
            },
            requested_expires_at_ms: medulla_server_api::txn::now_ms() + 60_000,
            idempotency_key: "body-key".into(),
        },
        admission: None,
    }
}

// proves R085.2: Behavior: issue approval through a dedicated native issuer
// spec: MEDULLA-FEDERATED-QUERY-R085.2
#[tokio::test]
async fn approval_requires_authority_and_native_commit() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (_dir, state, backend) = fixture(true).await;
    let before = backend.read_mutation_graph_version(GRAPH).await.unwrap();

    // An unregistered source refuses `SOURCE_QUERY_UNAUTHORIZED` and writes nothing.
    let refused = call(
        &state,
        2,
        "approve-1",
        &["source:approve-query"],
        Method::ApproveSourceQuery {
            request: Box::new(request()),
            source_approval: Box::new(approval()),
        },
    )
    .await;
    assert!(refused.result.is_none(), "an unregistered source has no result");
    let error = refused.error.as_deref().expect("approval is refused");
    assert!(error.contains("SOURCE_QUERY_UNAUTHORIZED"), "{error}");
    assert!(
        !error.contains("SELECT") && !error.contains("open"),
        "a refusal never carries SQL text or a parameter value: {error}"
    );

    // The generic issuer stays closed to the approval kind.
    let generic = call(
        &state,
        3,
        "generic-1",
        &["lease:write"],
        Method::IssueControlLease {
            request: medulla_work_types::control_lease::IssueControlLeaseRequest {
                tenant: TENANT.into(),
                lease_id: "generic-approval".into(),
                kind: SOURCE_QUERY_APPROVAL_KIND.into(),
                grant: serde_json::Map::new(),
                pending_call_registration: None,
                issued_at_ms: medulla_server_api::txn::now_ms(),
                expires_at_ms: medulla_server_api::txn::now_ms() + 1_000,
                hard_expires_at_ms: medulla_server_api::txn::now_ms() + 1_000,
                idempotency_key: "generic-1".into(),
            },
        },
    )
    .await;
    assert!(generic.result.is_none());
    assert_eq!(generic.error.as_deref(), Some("ACCESS_DENIED"));
    assert!(backend
        .read_control_lease(GRAPH, TENANT, "generic-approval")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        backend.read_mutation_graph_version(GRAPH).await.unwrap(),
        before,
        "a refused approval commits nothing"
    );
}

// spec: MEDULLA-FEDERATED-QUERY-R085.2
#[tokio::test]
async fn approval_without_the_agent_library_cannot_verify() {
    let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let (_dir, state, backend) = fixture(false).await;
    let before = backend.read_mutation_graph_version(GRAPH).await.unwrap();
    let refused = call(
        &state,
        2,
        "approve-2",
        &["source:approve-query"],
        Method::ApproveSourceQuery {
            request: Box::new(request()),
            source_approval: Box::new(approval()),
        },
    )
    .await;
    let error = refused.error.as_deref().expect("approval is refused");
    assert!(
        error.contains("SOURCE_QUERY_VERIFICATION_UNAVAILABLE"),
        "{error}"
    );
    assert_eq!(
        backend.read_mutation_graph_version(GRAPH).await.unwrap(),
        before
    );
}
