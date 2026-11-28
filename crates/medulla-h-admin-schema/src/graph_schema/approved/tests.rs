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
use medulla_mutation_types::governed_change::{governed_approval_ref, SOURCE_AUTHORITY_KIND};
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::state::ServerState;
use medulla_server_auth::auth::dispatch_test_on_heap;

const SECRET: &str = "schema-approval-served-secret"; // sanitizer:ignore
const TENANT: &str = "tenant-shared";
const ADMIN: &str = "schema-approval-admin";
const PROPOSER: &str = "authority-proposer";
const APPROVER: &str = "authority-approver";
const GRAPH: &str = "drift-live";
const SOURCE: &str = "approved:container-manager-mcp";

/// The class the candidate's shapes target: the records the shadow ingest samples.
fn record_class() -> String {
    medulla_catalog_types::graph_schema::repair::repair_record_class("container-manager-mcp")
}
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
        source_authority: false,
    }
}

fn contract() -> RecordContract {
    contract_with("name")
}

/// The admin and the two governed-change principals, each a registered agent
/// (`ServerState::test_isolation` registers one).
fn test_isolation(
    clock: medulla_types::clock::ManualClock,
) -> medulla_isolation::isolation::IsolationLayer {
    let mut isolation =
        ServerState::test_isolation(ADMIN).with_governed_clock(std::sync::Arc::new(clock));
    for agent in [PROPOSER, APPROVER] {
        isolation.register_agent(medulla_identity_core_types::acl::AgentIdentity {
            agent_id: agent.into(),
            role: medulla_identity_core_types::acl::AgentRole::System,
            teams: Vec::new(),
            roles: Vec::new(),
        });
    }
    isolation
}

struct Served {
    _dir: tempfile::TempDir,
    _env: medulla_kernel::crypto::TestEnvReadGuard,
    state: Arc<RwLock<ServerState>>,
    /// The governed clock of the served layer: tests move it, never sleep.
    clock: medulla_types::clock::ManualClock,
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Served {
    async fn new() -> Self {
        let env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir = tempfile::tempdir().unwrap();
        let clock = medulla_types::clock::ManualClock::new(
            medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
        );
        let mut state = medulla_server::seams_install::new_state_for_test(
            SECRET,
            test_isolation(clock.clone()),
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
            clock,
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
        self.call_as(ADMIN, &["*"], graph, key, method).await
    }

    async fn call_as(
        &self,
        principal: &str,
        scopes: &[&str],
        graph: &str,
        key: &str,
        method: Method,
    ) -> Response {
        self.call_in_tenant(TENANT, principal, scopes, graph, key, method)
            .await
    }

    /// One signed call from `principal` verified in `tenant`.
    async fn call_in_tenant(
        &self,
        tenant: &str,
        principal: &str,
        scopes: &[&str],
        graph: &str,
        key: &str,
        method: Method,
    ) -> Response {
        let id = next_id();
        let request = medulla_server_auth::auth::scoped_test_request_on(
            SECRET,
            id,
            graph,
            method,
            medulla_server_auth::auth::ScopedTestCaller {
                principal,
                tenant,
                scopes,
                nonce: &format!("schema-approval-nonce-{id}"),
                idempotency_key: key,
            },
        );
        dispatch_test_on_heap(&self.state, request).await
    }

    /// Propose a source-authority change for `digest` as one principal and
    /// approve it as another. The approved change is what an authority attach
    /// names, as `governed_approval_ref(change_id)`; no lease is issued.
    async fn govern_authority(&self, change_id: &str, digest: &str) {
        self.govern_authority_for(change_id, digest, 600_000).await;
    }

    /// `govern_authority` with an approval window of `window_ms`.
    async fn govern_authority_for(&self, change_id: &str, digest: &str, window_ms: u64) {
        use medulla_mutation_types::governed_change::{
            GovernedApproval, GovernedChangeOp, GovernedProposal,
        };
        let propose = Method::GovernedChange {
            op: GovernedChangeOp::Propose {
                request: GovernedProposal {
                    change_id: change_id.to_string(),
                    kind: SOURCE_AUTHORITY_KIND.to_string(),
                    target: SOURCE.to_string(),
                    digest: digest.to_string(),
                    justification: "authority candidate".to_string(),
                    proposal_ttl_ms: 600_000,
                    window_ms,
                },
            },
            actor: None,
        };
        let proposed = self
            .call_as(
                PROPOSER,
                &["governance:propose"],
                GRAPH,
                &format!("propose:{change_id}"),
                propose,
            )
            .await;
        assert!(proposed.error.is_none(), "{:?}", proposed.error);
        let approve = Method::GovernedChange {
            op: GovernedChangeOp::Approve {
                request: GovernedApproval {
                    change_id: change_id.to_string(),
                    digest: digest.to_string(),
                },
            },
            actor: None,
        };
        let approved = self
            .call_as(
                APPROVER,
                &["governance:approve-source-authority", "governance:read"],
                GRAPH,
                &format!("approve:{change_id}"),
                approve,
            )
            .await;
        assert!(approved.error.is_none(), "{:?}", approved.error);
    }

    /// The tenant's view of one control lease (`None` when none is visible).
    async fn lease(
        &self,
        lease_id: &str,
    ) -> Option<medulla_work_types::control_lease::ControlLeaseView> {
        let response = self
            .call(
                GRAPH,
                &format!("get:{lease_id}"),
                Method::GetControlLease {
                    tenant: TENANT.to_string(),
                    lease_id: lease_id.to_string(),
                },
            )
            .await;
        decode(response)
    }

    /// Issue an approval lease whose grant names `candidate`, and approve it
    /// (`active -> consumed`) unless `approve` is false.
    async fn approval(&self, lease_id: &str, candidate: &str, approve: bool) {
        self.approval_of(lease_id, SCHEMA_APPROVAL_ACTION, candidate, approve)
            .await;
    }

    /// `approval` for a grant naming `action`.
    async fn approval_of(&self, lease_id: &str, action: &str, candidate: &str, approve: bool) {
        let now = medulla_server_api::txn::now_ms();
        let Value::Object(grant) = json!({
            "kind": action,
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
            decided_by: String::new(),
            idempotency_key: format!("approve:{lease_id}"),
        };
        let method = Method::TransitionControlLease { request: consume };
        // The approvals queue's decider is a principal other than the executing caller.
        let approved = self
            .call_as(
                APPROVER,
                &["*"],
                GRAPH,
                &format!("approve:{lease_id}"),
                method,
            )
            .await;
        assert!(approved.error.is_none(), "{:?}", approved.error);
    }

    async fn attach_approved(&self, lease_id: &str) -> Response {
        self.attach_expecting(lease_id, None).await
    }

    async fn attach_expecting(&self, lease_id: &str, expected_digest: Option<String>) -> Response {
        self.attach_keyed(&format!("attach:{lease_id}"), lease_id, expected_digest)
            .await
    }

    /// One `AttachApproved` under its own idempotency key: a repeated key is a
    /// replay of the first answer, not a second attach.
    async fn attach_keyed(
        &self,
        key: &str,
        lease_id: &str,
        expected_digest: Option<String>,
    ) -> Response {
        self.attach_contract(key, lease_id, contract(), expected_digest)
            .await
    }

    async fn attach_contract(
        &self,
        key: &str,
        lease_id: &str,
        contract: RecordContract,
        expected_digest: Option<String>,
    ) -> Response {
        let op = GraphSchemaOp::AttachApproved {
            source_id: SOURCE.to_string(),
            contract,
            approval_lease_id: lease_id.to_string(),
            expected_digest,
            if_composed_digest: None,
        };
        let method = Method::GraphSchema { op: Box::new(op) };
        self.call(GRAPH, key, method).await
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
        self.node("person-1", json!({"type": record_class(), "name": "alice"}))
            .await;
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

// spec: MEDULLA-TYPED-PACKS-R806
#[tokio::test]
async fn an_approval_decided_by_the_executing_caller_is_refused_as_self_approved() {
    let served = Served::new().await;
    served.clean_shadow().await;
    served
        .approval("action_approval:own", &candidate(), false)
        .await;
    // The executing caller (ADMIN) decides its own approval. A decider the client
    // names in the request is overwritten by the engine with the verified caller.
    let consume = TransitionControlLeaseRequest {
        pending_call_consumption: None,
        tenant: TENANT.to_string(),
        lease_id: "action_approval:own".to_string(),
        expected_revision: 1,
        to: ControlLeaseTarget::Consumed,
        decided_by: "principal:sha256:forged-other-decider".to_string(),
        idempotency_key: "approve:own".to_string(),
    };
    let decided = served
        .call(
            GRAPH,
            "approve:own",
            Method::TransitionControlLease { request: consume },
        )
        .await;
    assert!(decided.error.is_none(), "{:?}", decided.error);
    let refused = served.attach_approved("action_approval:own").await;
    assert!(
        refusal(&refused).starts_with("REPAIR_SELF_APPROVED"),
        "{:?}",
        refused.error
    );
    let listed: GraphSchemaSourcesView = decode(
        served
            .call(GRAPH, "list-own", Method::GraphSchemaList)
            .await,
    );
    assert!(listed
        .dynamic_sources
        .iter()
        .all(|source| source.source_id != SOURCE));
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
    // A node outside the contract's target class is not sampled at all.
    served
        .node(
            "bookkeeping",
            json!({"type": "OntologyEdit", "title": "no name"}),
        )
        .await;
    served.shadow_validate("shadow-untargeted").await;
    let untargeted = served.attach_approved("action_approval:shadow").await;
    assert!(refusal(&untargeted).starts_with("SCHEMA_REPAIR_SHADOW_MISSING"));

    served
        .node(
            "person-bad",
            json!({"type": record_class(), "title": "no name"}),
        )
        .await;
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

    // A clean report over current data lets the same approval attach. The
    // approval lease is issued first: a control row, never ingested as data.
    let clean = Served::new().await;
    clean
        .approval("action_approval:shadow", &candidate(), true)
        .await;
    clean.clean_shadow().await;
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
    // The same digest again, as a new request, is a no-op.
    let again: GraphSchemaCommitted = decode(
        served
            .attach_keyed(
                "attach-again:action_approval:digest",
                "action_approval:digest",
                Some(digest.clone()),
            )
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

// spec: MEDULLA-SRC-AUTH-R003.3
#[tokio::test]
async fn authority_candidate_needs_matching_action_and_digest() {
    let served = Served::new().await;
    served.clean_shadow().await;
    let mut authority = contract();
    authority.source_authority = true;
    let digest = approved_candidate_digest(SOURCE, &authority);
    assert_ne!(digest, candidate(), "the typed flag changes the digest");

    // An ordinary schema-approval grant does not approve an authority candidate.
    served
        .approval("action_approval:ordinary", &digest, true)
        .await;
    let wrong_action = served
        .attach_contract(
            "attach:ordinary",
            "action_approval:ordinary",
            authority.clone(),
            None,
        )
        .await;
    assert!(refusal(&wrong_action).starts_with("SCHEMA_APPROVAL_MISMATCH"));

    // A source-authority grant does not approve an ordinary candidate.
    served
        .approval_of(
            "action_approval:authority",
            SOURCE_AUTHORITY_KIND,
            &digest,
            true,
        )
        .await;
    let ordinary = served
        .attach_contract(
            "attach:plain",
            "action_approval:authority",
            contract(),
            None,
        )
        .await;
    assert!(refusal(&ordinary).starts_with("SCHEMA_APPROVAL_MISMATCH"));

    // A reference that names no governed change is refused, and so is a
    // governed reference to a change the ledger does not hold.
    served
        .approval_of(
            "action_approval:unbound",
            SOURCE_AUTHORITY_KIND,
            &digest,
            false,
        )
        .await;
    let unbound = served
        .attach_contract(
            "attach:unbound",
            "action_approval:unbound",
            authority.clone(),
            None,
        )
        .await;
    assert!(refusal(&unbound).starts_with("SCHEMA_APPROVAL_MISMATCH"));
    let unknown = served
        .attach_contract(
            "attach:unknown",
            &governed_approval_ref("change-unknown"),
            authority.clone(),
            None,
        )
        .await;
    assert!(refusal(&unknown).starts_with("SCHEMA_APPROVAL_REQUIRED"));

    // The approved change in the governed ledger admits the attach: the
    // matching kind, source and digest are all it needs.
    served.govern_authority("change-authority", &digest).await;
    let approval_ref = governed_approval_ref("change-authority");
    let committed: GraphSchemaCommitted = decode(
        served
            .attach_contract("attach:authority", &approval_ref, authority, None)
            .await,
    );
    assert!(committed.changed);
}

// spec: MEDULLA-SRC-AUTH-R003.3
#[tokio::test]
async fn an_approved_authority_attach_is_single_use() {
    let served = Served::new().await;
    served.clean_shadow().await;
    let mut authority = contract();
    authority.source_authority = true;
    let digest = approved_candidate_digest(SOURCE, &authority);
    served.govern_authority("change-single-use", &digest).await;
    let approval_ref = governed_approval_ref("change-single-use");

    // The approval is the ledger's alone: it issued no lease.
    assert!(served.lease(&approval_ref).await.is_none());

    // An attach that fails spends nothing.
    let failed = served
        .attach_contract(
            "attach:failed",
            &approval_ref,
            authority.clone(),
            Some("0".repeat(64)),
        )
        .await;
    assert!(refusal(&failed).starts_with("SCHEMA_DIGEST_MISMATCH"));

    let first: GraphSchemaCommitted = decode(
        served
            .attach_contract("attach:first", &approval_ref, authority.clone(), None)
            .await,
    );
    assert!(first.changed);

    // A different request under the same approval is refused as already used.
    let second = served
        .attach_contract("attach:second", &approval_ref, authority, None)
        .await;
    let text = refusal(&second);
    assert!(text.starts_with("SCHEMA_APPROVAL_REQUIRED"), "{text}");
    assert!(text.contains("already used"), "{text}");
}

// spec: MEDULLA-SRC-AUTH-R003.3
#[tokio::test]
async fn a_retried_authority_attach_replays_its_receipt() {
    let served = Served::new().await;
    served.clean_shadow().await;
    let mut authority = contract();
    authority.source_authority = true;
    let digest = approved_candidate_digest(SOURCE, &authority);
    served.govern_authority("change-retry", &digest).await;
    let approval_ref = governed_approval_ref("change-retry");

    let first: GraphSchemaCommitted = decode(
        served
            .attach_contract("attach:retry", &approval_ref, authority.clone(), None)
            .await,
    );
    assert!(first.changed);

    // The same request key answers the original receipt, not "already used".
    let replay: GraphSchemaCommitted = decode(
        served
            .attach_contract("attach:retry", &approval_ref, authority.clone(), None)
            .await,
    );
    assert_eq!(replay, first);

    // Another key for the same approval is a different request and is refused.
    let other = served
        .attach_contract("attach:other", &approval_ref, authority, None)
        .await;
    assert!(refusal(&other).contains("already used"));
}

/// An authority attach under `reference` sent by a second verified tenant.
async fn foreign_attach(
    served: &Served,
    contract: RecordContract,
    reference: String,
    key: &str,
) -> Response {
    let op = GraphSchemaOp::AttachApproved {
        source_id: SOURCE.to_string(),
        contract,
        approval_lease_id: reference,
        expected_digest: None,
        if_composed_digest: None,
    };
    let method = Method::GraphSchema { op: Box::new(op) };
    served
        .call_in_tenant("tenant-foreign", ADMIN, &["*"], GRAPH, key, method)
        .await
}

// spec: MEDULLA-SRC-AUTH-R003.3
#[tokio::test]
async fn another_tenants_approved_change_answers_like_an_unknown_change() {
    let served = Served::new().await;
    served.clean_shadow().await;
    let mut authority = contract();
    authority.source_authority = true;
    let digest = approved_candidate_digest(SOURCE, &authority);
    served.govern_authority("change-tenant", &digest).await;
    let approval_ref = governed_approval_ref("change-tenant");

    // The proposing tenant's change, attached by another tenant, is refused
    // exactly as a change id the ledger does not hold.
    let foreign = foreign_attach(
        &served,
        authority.clone(),
        approval_ref.clone(),
        "foreign:held",
    )
    .await;
    let unknown = foreign_attach(
        &served,
        authority.clone(),
        governed_approval_ref("change-never-proposed"),
        "foreign:unknown",
    )
    .await;
    assert!(refusal(&foreign).starts_with("SCHEMA_APPROVAL_REQUIRED"));
    assert_eq!(foreign.error, unknown.error);
    assert_eq!(foreign.error_detail, unknown.error_detail);

    // The refusal spent nothing: the proposing tenant still attaches.
    let committed: GraphSchemaCommitted = decode(
        served
            .attach_contract("attach:tenant", &approval_ref, authority, None)
            .await,
    );
    assert!(committed.changed);
}

// spec: MEDULLA-SRC-AUTH-R003.3
#[tokio::test]
async fn a_retry_after_the_approval_window_closed_still_replays() {
    let served = Served::new().await;
    served.clean_shadow().await;
    let mut authority = contract();
    authority.source_authority = true;
    let digest = approved_candidate_digest(SOURCE, &authority);
    served
        .govern_authority_for("change-window", &digest, 1_000)
        .await;
    let approval_ref = governed_approval_ref("change-window");

    let first: GraphSchemaCommitted = decode(
        served
            .attach_contract("attach:window", &approval_ref, authority.clone(), None)
            .await,
    );
    assert!(first.changed);

    // The controlled clock moves past the 1 s window; no wall-clock wait.
    served.clock.advance_ms(1_300);

    // The window is closed: a new request is refused, the same key replays.
    let late = served
        .attach_contract("attach:late", &approval_ref, authority.clone(), None)
        .await;
    assert!(refusal(&late).starts_with("SCHEMA_APPROVAL_REQUIRED"));
    let replay: GraphSchemaCommitted = decode(
        served
            .attach_contract("attach:window", &approval_ref, authority, None)
            .await,
    );
    assert_eq!(replay, first);
}

/// A governed `Get` or `List` read by `principal` verified in `tenant`.
async fn governed_read(
    served: &Served,
    tenant: &str,
    key: &str,
    op: medulla_mutation_types::governed_change::GovernedChangeOp,
) -> Response {
    let method = Method::GovernedChange { op, actor: None };
    served
        .call_in_tenant(tenant, APPROVER, &["governance:read"], GRAPH, key, method)
        .await
}

// spec: MEDULLA-SRC-AUTH-R003.3
#[tokio::test]
async fn governed_ledger_reads_are_scoped_to_the_proposing_tenant() {
    use medulla_mutation_types::governed_change::GovernedChangeOp;
    let served = Served::new().await;
    served.govern_authority("change-read", "digest-read").await;

    // The proposing tenant reads its change by id and in the list.
    let own = governed_read(
        &served,
        TENANT,
        "read:own",
        GovernedChangeOp::Get {
            change_id: "change-read".to_string(),
        },
    )
    .await;
    assert!(own.error.is_none(), "{:?}", own.error);
    assert!(format!("{:?}", own.result).contains("change-read"));
    let own_list = governed_read(&served, TENANT, "list:own", GovernedChangeOp::List).await;
    assert!(format!("{:?}", own_list.result).contains("change-read"));

    // Another tenant's Get answers byte-identically to an unknown id.
    let foreign = governed_read(
        &served,
        "tenant-foreign",
        "read:foreign",
        GovernedChangeOp::Get {
            change_id: "change-read".to_string(),
        },
    )
    .await;
    let unknown = governed_read(
        &served,
        "tenant-foreign",
        "read:unknown",
        GovernedChangeOp::Get {
            change_id: "change-never-proposed".to_string(),
        },
    )
    .await;
    assert_eq!(
        format!("{:?}", foreign.result),
        format!("{:?}", unknown.result)
    );
    assert_eq!(foreign.error, unknown.error);
    assert_eq!(foreign.error_detail, unknown.error_detail);
    assert!(!format!("{:?}", foreign.result).contains("change-read"));

    // Another tenant's List holds none of this tenant's changes.
    let foreign_list = governed_read(
        &served,
        "tenant-foreign",
        "list:foreign",
        GovernedChangeOp::List,
    )
    .await;
    assert!(foreign_list.error.is_none(), "{:?}", foreign_list.error);
    assert!(!format!("{:?}", foreign_list.result).contains("change-read"));
}

// spec: MEDULLA-SRC-AUTH-R003.3
#[tokio::test]
async fn a_same_key_retry_with_a_different_body_is_refused() {
    let served = Served::new().await;
    served.clean_shadow().await;
    let mut authority = contract();
    authority.source_authority = true;
    let digest = approved_candidate_digest(SOURCE, &authority);
    served.govern_authority("change-body", &digest).await;
    let approval_ref = governed_approval_ref("change-body");

    let first: GraphSchemaCommitted = decode(
        served
            .attach_contract("attach:body", &approval_ref, authority.clone(), None)
            .await,
    );
    assert!(first.changed);

    // The same key with another candidate is a different operation: the commit
    // idempotency compares the body digest and refuses, never replays.
    let mut other = contract_with("other_field");
    other.source_authority = true;
    let different = served
        .attach_contract("attach:body", &approval_ref, other, None)
        .await;
    assert!(
        refusal(&different).contains("IDEMPOTENCY_CONFLICT"),
        "{:?}",
        different.error
    );
}
