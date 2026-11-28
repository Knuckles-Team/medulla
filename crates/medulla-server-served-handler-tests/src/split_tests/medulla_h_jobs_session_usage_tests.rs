//! MEDULLA-ORCH-VOCAB-R805: the served `GetSessionUsage` read returns the per-phase timings of
//! the committed run completions of one session, bound to the turn index of each run, and
//! another tenant's read of the same context id sees none of them.
//!
//! Every request goes through the real verified dispatch: a run is admitted with
//! `SubmitWorkItem`, claimed with `ClaimWorkItem` and committed with
//! `CommitWorkItemResult` carrying its `RunCompletionInput`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use medulla_identity_core_types::acl::RequestContextClaims;
use medulla_protocol_types::protocol::{GraphType, Method, Request, Response, ResultPayload};
use medulla_result_types::epistemic_operations::{ClaimWorkItemRequest, ClaimWorkItemResult};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_types::contract::Nonce;
use medulla_work_types::native_control::{SubmitWorkItemRequest, SubmitWorkItemResult};
use medulla_work_types::run_completion::{PhaseTiming, RunCompletionInput, TimingPhase};
use medulla_work_types::run_usage::{SessionTimingRecord, SessionUsage};
use serde::de::DeserializeOwned;
use tokio::sync::RwLock;

const TENANT: &str = "usage-tenant";
const OTHER_TENANT: &str = "usage-other-tenant";
const PRINCIPAL: &str = "usage-owner";
const GRAPH: &str = "usage-graph";
const CONTEXT: &str = "session-context";

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

struct Served {
    _dir: tempfile::TempDir,
    state: Arc<RwLock<ServerState>>,
}

impl Served {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut state = medulla_server::seams_install::new_state_for_test(
            "session-usage-fixture-secret", // sanitizer:ignore
            ServerState::test_isolation(PRINCIPAL),
        );
        state.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        let graphs = dir.path().join("graphs");
        std::fs::create_dir_all(&graphs).unwrap();
        state.persistence = Some(Arc::new(
            medulla_persistence::redb_backend::RedbBackend::open(
                graphs.to_string_lossy().into_owned(),
                64,
            )
            .expect("open the served graph backend"),
        ));
        if state.registry.get(GRAPH).is_none() {
            state
                .registry
                .create_graph(GRAPH, GraphType::Team, None)
                .unwrap();
        }
        Self {
            _dir: dir,
            state: Arc::new(RwLock::new(state)),
        }
    }

    /// One request as `PRINCIPAL` in `tenant`, through the verified dispatch.
    async fn call(&self, tenant: &str, key: &str, method: Method) -> Response {
        let id = next_id();
        let claims = RequestContextClaims {
            principal: PRINCIPAL.to_string(),
            agent_id: PRINCIPAL.to_string(),
            tenant: tenant.to_string(),
            audience: "medulla-test".to_string(),
            policy_version: "policy-test".to_string(),
            scopes: vec!["*".to_string()],
            ..RequestContextClaims::default()
        };
        let mut nonce = [0u8; 32];
        nonce[..8].copy_from_slice(&id.to_be_bytes());
        let context = VerifiedRequestContext::from_verified_claims_with_nonce(
            claims,
            key.to_string(),
            Some(Nonce::from_bytes(nonce)),
        );
        let request = Request {
            id,
            graph: GRAPH.to_string(),
            auth_token: String::new(),
            agent_id: Some(PRINCIPAL.to_string()),
            method,
            trace_id: None,
        };
        Box::pin(
            medulla_server_router::dispatch::request_boundary::dispatch_verified_request(
                &self.state,
                request,
                context,
            ),
        )
        .await
    }

    /// Submit, claim and commit one run of `CONTEXT` with `timings`.
    async fn commit_run(&self, run_id: &str, timings: Vec<PhaseTiming>) {
        let submit: SubmitWorkItemRequest = serde_json::from_value(serde_json::json!({
            "schema_version": "1", "run_id": run_id, "work_item_id": null,
            "context": {
                "schema_version": "2", "request_id": format!("request-{run_id}"),
                "subject_id": PRINCIPAL, "tenant_id": TENANT, "agent_id": PRINCIPAL,
                "scopes": ["work:*"], "audience": "medulla-test",
                "authentication_method": "local_process", "policy_version": "policy-test",
                "graph": GRAPH, "placement_epoch": null, "trace_id": format!("trace-{run_id}"),
                "issued_at_ms": 1, "expires_at_ms": u64::MAX >> 12
            },
            "idempotency_key": format!("submit-{run_id}"), "command_digest": "A".repeat(64),
            "kind": "failure_cluster", "priority": 2, "depends_on": [], "input_ref": "input",
            "policy_digest": "policy", "catalog_digest": "catalog", "model_digest": "model",
            "max_attempts": 3, "deadline_unix": null,
            "metadata": {"a2a_context_id": CONTEXT}, "provenance_refs": ["source"],
            "max_tenant_in_flight": 0
        }))
        .unwrap();
        let submitted: SubmitWorkItemResult = ok(
            "SubmitWorkItem",
            self.call(
                TENANT,
                &format!("submit-{run_id}"),
                Method::SubmitWorkItem { request: submit },
            )
            .await,
        );
        let claim: ClaimWorkItemRequest = serde_json::from_value(serde_json::json!({
            "schema_version": "1", "tenant_ref": TENANT,
            "work_item_id": submitted.work_item_id,
            "queue_ref": null, "resource_class": null, "fairness_group": null,
            "worker_ref": "usage-worker", "now_ms": now_ms(), "lease_ms": 600000,
            "max_tenant_in_flight": 64
        }))
        .unwrap();
        let held: ClaimWorkItemResult = ok(
            "ClaimWorkItem",
            self.call(
                TENANT,
                &format!("claim-{run_id}"),
                Method::ClaimWorkItem { request: claim },
            )
            .await,
        );
        assert!(held.claimed, "{held:?}");
        let committed = self
            .call(
                TENANT,
                &format!("finish-{run_id}"),
                Method::CommitWorkItemResult {
                    tenant: TENANT.into(),
                    work_item_id: submitted.work_item_id.clone(),
                    worker_id: "usage-worker".into(),
                    lease_epoch: held.lease_epoch.unwrap(),
                    fencing_token: held.fencing_token.unwrap(),
                    idempotency_key: format!("finish-{run_id}"),
                    outcome: "succeeded".into(),
                    result_ref: Some("answer:verified".into()),
                    error_ref: None,
                    retryable: false,
                    now_ms: now_ms(),
                    outcome_extension: None,
                    run_completion: Some(Box::new(RunCompletionInput {
                        schema_version: 1,
                        run_id: run_id.into(),
                        usage: None,
                        timings,
                    })),
                },
            )
            .await;
        assert!(committed.error.is_none(), "commit refused: {committed:?}");
    }

    async fn session_usage(&self, tenant: &str) -> SessionUsage {
        ok(
            "GetSessionUsage",
            self.call(
                tenant,
                &format!("usage-{tenant}-{}", next_id()),
                Method::GetSessionUsage {
                    tenant: tenant.into(),
                    context_id: CONTEXT.into(),
                },
            )
            .await,
        )
    }
}

fn ok<T: DeserializeOwned>(label: &str, response: Response) -> T {
    assert!(
        response.error.is_none(),
        "{label} was refused: {response:?}"
    );
    match response.result {
        Some(ResultPayload::Raw(bytes)) => rmp_serde::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("{label} result does not decode: {error}")),
        Some(ResultPayload::Json(value)) => serde_json::from_value(value)
            .unwrap_or_else(|error| panic!("{label} result does not decode: {error}")),
        other => panic!("{label} answered an unexpected payload shape: {other:?}"),
    }
}

// proves R805: Per-turn timing observations in the session usage result
// spec: MEDULLA-ORCH-VOCAB-R805
#[tokio::test]
async fn timing_binds_to_committed_turn_receipts() {
    let served = Served::new();
    served
        .commit_run(
            "run-timed",
            vec![
                // Declared out of phase order: the read emits them in phase order.
                PhaseTiming {
                    phase: TimingPhase::Run,
                    duration_ms: 900,
                },
                PhaseTiming {
                    phase: TimingPhase::Request,
                    duration_ms: 40,
                },
            ],
        )
        .await;
    served.commit_run("run-untimed", Vec::new()).await;

    let usage = served.session_usage(TENANT).await;
    assert_eq!(usage.expected_runs, 2, "{usage:?}");
    assert_eq!(
        usage.turn_timings,
        vec![
            SessionTimingRecord {
                turn_index: 0,
                run_id: "run-timed".into(),
                phase: TimingPhase::Request,
                duration_ms: 40,
            },
            SessionTimingRecord {
                turn_index: 0,
                run_id: "run-timed".into(),
                phase: TimingPhase::Run,
                duration_ms: 900,
            },
        ],
        "the timed run is turn 0; the untimed run takes turn 1 and has no record"
    );

    // Another tenant reading the same context id sees none of them.
    let foreign = served.session_usage(OTHER_TENANT).await;
    assert!(foreign.turn_timings.is_empty(), "{foreign:?}");
    assert_eq!(foreign.expected_runs, 0, "{foreign:?}");
}
