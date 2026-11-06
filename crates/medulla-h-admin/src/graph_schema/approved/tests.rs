//! EH-403 served acceptance: `GraphSchema.AttachApproved` through the real
//! signed dispatch, over a real redb authority holding the approval lease.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use medulla_catalog_types::{
    graph_schema::approval::approved_candidate_digest,
    graph_schema::approval::SCHEMA_APPROVAL_ACTION,
    graph_schema::approval::SCHEMA_APPROVAL_LEASE_KIND,
};
use medulla_catalog_types::{
    graph_schema::repair::FieldContract, graph_schema::repair::JsonType,
    graph_schema::repair::RecordContract,
};
use medulla_catalog_types::{
    graph_schema::GraphSchemaCommitted, graph_schema::GraphSchemaSourcesView,
    graph_schema::SchemaSourceOriginView,
};
use medulla_work_types::{
    control_lease::ControlLeaseTarget, control_lease::IssueControlLeaseRequest,
    control_lease::TransitionControlLeaseRequest,
};

use super::*;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::dispatch_test_on_heap;

const SECRET: &str = "schema-approval-served-secret"; // sanitizer:ignore
const TENANT: &str = "tenant-shared";
const ADMIN: &str = "schema-approval-admin";
const GRAPH: &str = "drift-live";
const SOURCE: &str = "approved:container-manager-mcp";
fn contract_with(field: &str) -> RecordContract {
    RecordContract {
        fields: [(
            field.to_string(),
            FieldContract {
                required: true,
                types: vec![JsonType::String],
            },
        )]
        .into_iter()
        .collect(),
    }
}

fn contract() -> RecordContract {
    contract_with("name")
}

struct Served {
    _dir: tempfile::TempDir,
    _env: medulla_kernel::crypto::TestEnvReadGuard,
    state: Arc<RwLock<ServerState>>,
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Served {
    async fn new() -> Self {
        let env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir = tempfile::tempdir().unwrap();
        let mut state = medulla_server::seams_install::new_state_for_test(
            SECRET,
            ServerState::test_isolation(ADMIN),
        );
        state.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        let backend = medulla_persistence::redb_backend::RedbBackend::open(
            dir.path().join("graphs").to_string_lossy().into_owned(),
            64,
        )
        .expect("open the served graph backend");
        state.persistence = Some(Arc::new(backend));
        let served = Self {
            _dir: dir,
            _env: env,
            state: Arc::new(RwLock::new(state)),
        };
        let created = served
            .call(
                "__commons__",
                "create",
                Method::CreateGraph {
                    graph_name: GRAPH.to_string(),
                    graph_type: medulla_protocol_types::protocol::GraphType::Global,
                },
            )
            .await;
        assert!(created.error.is_none(), "{:?}", created.error);
        served
    }

    async fn call(&self, graph: &str, key: &str, method: Method) -> Response {
        let id = next_id();
        let request = medulla_server_auth::auth::scoped_test_request_on(
            SECRET,
            id,
            graph,
            method,
            medulla_server_auth::auth::ScopedTestCaller {
                principal: ADMIN,
                tenant: TENANT,
                scopes: &["*"],
                nonce: &format!("schema-approval-nonce-{id}"),
                idempotency_key: key,
            },
        );
        dispatch_test_on_heap(&self.state, request).await
    }

    /// Issue an approval lease whose grant names `candidate`, and approve it
    /// (`active -> consumed`) unless `approve` is false.
    async fn approval(&self, lease_id: &str, candidate: &str, approve: bool) {
        let now = medulla_server_api::txn::now_ms();
        let Value::Object(grant) = json!({
            "kind": SCHEMA_APPROVAL_ACTION,
            "target": SOURCE,
            "candidate_digest": candidate,
        }) else {
            unreachable!("the grant literal is an object")
        };
        let issue = IssueControlLeaseRequest {
            pending_call_registration: None,
            tenant: TENANT.to_string(),
            lease_id: lease_id.to_string(),
            kind: SCHEMA_APPROVAL_LEASE_KIND.to_string(),
            grant,
            issued_at_ms: now,
            expires_at_ms: now + 600_000,
            hard_expires_at_ms: now + 600_000,
            idempotency_key: format!("issue:{lease_id}"),
        };
        let method = Method::IssueControlLease { request: issue };
        let issued = self.call(GRAPH, &format!("issue:{lease_id}"), method).await;
        assert!(issued.error.is_none(), "{:?}", issued.error);
        if !approve {
            return;
        }
        let consume = TransitionControlLeaseRequest {
            pending_call_consumption: None,
            tenant: TENANT.to_string(),
            lease_id: lease_id.to_string(),
            expected_revision: 1,
            to: ControlLeaseTarget::Consumed,
            idempotency_key: format!("approve:{lease_id}"),
        };
        let method = Method::TransitionControlLease { request: consume };
        let approved = self
            .call(GRAPH, &format!("approve:{lease_id}"), method)
            .await;
        assert!(approved.error.is_none(), "{:?}", approved.error);
    }

    async fn attach_approved(&self, lease_id: &str) -> Response {
        self.attach_expecting(lease_id, None).await
    }

    async fn attach_expecting(&self, lease_id: &str, expected_digest: Option<String>) -> Response {
        let op = GraphSchemaOp::AttachApproved {
            source_id: SOURCE.to_string(),
            contract: contract(),
            approval_lease_id: lease_id.to_string(),
            expected_digest,
            if_composed_digest: None,
        };
        let method = Method::GraphSchema { op: Box::new(op) };
        self.call(GRAPH, &format!("attach:{lease_id}"), method)
            .await
    }

    /// Write one node whose properties are `record`.
    async fn node(&self, node_id: &str, record: Value) {
        let method = Method::AddNode {
            node_id: node_id.to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&record).unwrap(),
        };
        let added = self.call(GRAPH, &format!("node:{node_id}"), method).await;
        assert!(added.error.is_none(), "{:?}", added.error);
    }

    /// Run `ValidateRepair`, which stores the shadow-ingest report of the candidate.
    async fn shadow_validate(&self, key: &str) {
        let op = GraphSchemaOp::ValidateRepair {
            source_id: SOURCE.to_string(),
            contract: contract(),
            if_composed_digest: None,
        };
        let method = Method::GraphSchema { op: Box::new(op) };
        let validated = self.call(GRAPH, key, method).await;
        assert!(validated.error.is_none(), "{:?}", validated.error);
    }

    /// A clean current node plus a stored clean report: the state `AttachApproved` needs.
    async fn clean_shadow(&self) {
        self.node("person-1", json!({"name": "alice"})).await;
        self.shadow_validate("shadow-clean").await;
    }
}

fn decode<T: DeserializeOwned>(response: Response) -> T {
    assert!(response.error.is_none(), "{:?}", response.error);
    match response.result.expect("a committed schema op answers") {
        ResultPayload::Raw(bytes) => rmp_serde::from_slice(&bytes).unwrap(),
        ResultPayload::Json(value) => serde_json::from_value(value).unwrap(),
        other => panic!("unexpected GraphSchema payload {other:?}"),
    }
}

fn refusal(response: &Response) -> &str {
    assert!(response.result.is_none(), "{:?}", response.result);
    response
        .error
        .as_deref()
        .expect("the attach must be refused")
}

fn candidate() -> String {
    approved_candidate_digest(SOURCE, &contract())
}

#[tokio::test]
async fn an_approved_candidate_attaches_under_its_approval_and_records_it() {
    let served = Served::new().await;
    served.clean_shadow().await;
    served
        .approval("action_approval:ok", &candidate(), true)
        .await;
    let committed: GraphSchemaCommitted =
        decode(served.attach_approved("action_approval:ok").await);
    assert!(committed.changed);

    let listed = served.call(GRAPH, "list", Method::GraphSchemaList).await;
    let listed: GraphSchemaSourcesView = decode(listed);
    let source = listed
        .dynamic_sources
        .iter()
        .find(|source| source.source_id == SOURCE)
        .expect("the approved source is attached");
    assert!(
        matches!(&source.origin, SchemaSourceOriginView::Approved { name, approval_lease_id, .. }
            if name == "container-manager-mcp" && approval_lease_id == "action_approval:ok"),
        "{:?}",
        source.origin
    );
    assert!(
        matches!(&source.origin, SchemaSourceOriginView::Approved { shadow_report_digest: Some(digest), .. } if digest.len() == 64),
        "the attach records the shadow report digest as provenance"
    );
}

#[tokio::test]
async fn no_approval_a_pending_approval_or_another_candidates_approval_is_refused() {
    let served = Served::new().await;
    let missing = served.attach_approved("action_approval:absent").await;
    assert!(refusal(&missing).starts_with("SCHEMA_APPROVAL_REQUIRED"));

    served
        .approval("action_approval:pending", &candidate(), false)
        .await;
    let pending = served.attach_approved("action_approval:pending").await;
    assert_eq!(refusal(&pending), "SCHEMA_APPROVAL_REQUIRED");
    assert!(pending
        .error_detail
        .as_deref()
        .is_some_and(|detail| detail.contains("still pending")));

    let other = approved_candidate_digest(SOURCE, &contract_with("title"));
    served.approval("action_approval:other", &other, true).await;
    let mismatched = served.attach_approved("action_approval:other").await;
    assert!(refusal(&mismatched).starts_with("SCHEMA_APPROVAL_MISMATCH"));

    let listed: GraphSchemaSourcesView = decode(
        served
            .call(GRAPH, "list-after", Method::GraphSchemaList)
            .await,
    );
    assert!(listed
        .dynamic_sources
        .iter()
        .all(|source| source.source_id != SOURCE));
}

#[tokio::test]
async fn a_generic_attach_cannot_write_the_approved_namespace() {
    let served = Served::new().await;
    let op = GraphSchemaOp::Attach {
        source_id: SOURCE.to_string(),
        shapes_ttl: Some("@prefix sh: <http://www.w3.org/ns/shacl#> .".to_string()),
        ontology_ttl: None,
        if_composed_digest: None,
    };
    let response = served
        .call(GRAPH, "generic", Method::GraphSchema { op: Box::new(op) })
        .await;
    assert!(refusal(&response).contains("SCHEMA_SOURCE_RESERVED"));
}

#[tokio::test]
async fn validate_repair_renders_and_validates_without_attaching() {
    let served = Served::new().await;
    let op = GraphSchemaOp::ValidateRepair {
        source_id: SOURCE.to_string(),
        contract: contract(),
        if_composed_digest: None,
    };
    let method = Method::GraphSchema { op: Box::new(op) };
    let validated: GraphSchemaCommitted = decode(served.call(GRAPH, "validate", method).await);
    assert!(!validated.changed);
    let listed: GraphSchemaSourcesView = decode(
        served
            .call(GRAPH, "list-validate", Method::GraphSchemaList)
            .await,
    );
    assert!(listed
        .dynamic_sources
        .iter()
        .all(|source| source.source_id != SOURCE));
}

// spec: MEDULLA-DECISION-ENGINE-R102.2
#[tokio::test]
async fn schema_repair_requires_real_shadow_ingest_before_activation() {
    // No report: refused before anything attaches.
    let served = Served::new().await;
    served
        .approval("action_approval:shadow", &candidate(), true)
        .await;
    let missing = served.attach_approved("action_approval:shadow").await;
    assert!(refusal(&missing).starts_with("SCHEMA_REPAIR_SHADOW_MISSING"));

    // A report over an empty graph ingested nothing, which is still missing.
    served.shadow_validate("shadow-empty").await;
    let empty = served.attach_approved("action_approval:shadow").await;
    assert!(refusal(&empty).starts_with("SCHEMA_REPAIR_SHADOW_MISSING"));

    // A current node that breaks the contract is a violation.
    served.node("person-bad", json!({"title": "no name"})).await;
    served.shadow_validate("shadow-violating").await;
    let violating = served.attach_approved("action_approval:shadow").await;
    assert!(refusal(&violating).starts_with("SCHEMA_REPAIR_SHADOW_VIOLATIONS"));
    let listed: GraphSchemaSourcesView = decode(
        served
            .call(GRAPH, "list-shadow", Method::GraphSchemaList)
            .await,
    );
    assert!(listed
        .dynamic_sources
        .iter()
        .all(|source| source.source_id != SOURCE));

    // A clean report over current data lets the same approval attach.
    let clean = Served::new().await;
    clean.clean_shadow().await;
    clean
        .approval("action_approval:shadow", &candidate(), true)
        .await;
    let committed: GraphSchemaCommitted =
        decode(clean.attach_approved("action_approval:shadow").await);
    assert!(committed.changed);
}

/// Verifies MEDULLA-TYPED-PACKS-R808: Operator-selected schema import with digest check and readback
// spec: MEDULLA-TYPED-PACKS-R808
#[tokio::test]
async fn expected_digest_is_checked_before_the_attach_and_reads_back() {
    let served = Served::new().await;
    served.clean_shadow().await;
    let rendered = super::source(
        &GraphSchemaOp::ValidateRepair {
            source_id: SOURCE.to_string(),
            contract: contract(),
            if_composed_digest: None,
        },
        None,
    )
    .unwrap();
    let digest = rendered.shapes_sha256.unwrap().to_hex();

    // The candidate is unknown when no lease of the tenant covers it.
    let unknown = served
        .attach_expecting("action_approval:absent", Some(digest.clone()))
        .await;
    assert!(refusal(&unknown).starts_with("SCHEMA_CANDIDATE_UNKNOWN"));

    served
        .approval("action_approval:digest", &candidate(), true)
        .await;
    let wrong = served
        .attach_expecting("action_approval:digest", Some("0".repeat(64)))
        .await;
    assert!(refusal(&wrong).starts_with("SCHEMA_DIGEST_MISMATCH"));

    let attached: GraphSchemaCommitted = decode(
        served
            .attach_expecting("action_approval:digest", Some(digest.clone()))
            .await,
    );
    assert!(attached.changed);
    // The same digest again is a no-op.
    let again: GraphSchemaCommitted = decode(
        served
            .attach_expecting("action_approval:digest", Some(digest.clone()))
            .await,
    );
    assert!(!again.changed);

    let listed: GraphSchemaSourcesView = decode(
        served
            .call(GRAPH, "list-digest", Method::GraphSchemaList)
            .await,
    );
    let stored = listed
        .dynamic_sources
        .iter()
        .find(|source| source.source_id == SOURCE)
        .expect("the selected candidate is attached");
    assert_eq!(stored.shapes_sha256.as_deref(), Some(digest.as_str()));
}
