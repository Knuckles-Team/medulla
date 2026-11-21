//! Private graph-ref cache tests for the analytics job handler.

use super::*;
use medulla_kernel::lock_recovery::WriteRecovery;

#[cfg(test)]
mod resolve_core_ref_tests {
    use super::*;
    use medulla_core::registry::GraphRegistry;
    use medulla_protocol_types::protocol::GraphType;

    /// A unique-enough name per call so parallel `cargo test` threads sharing
    /// the ONE process-wide `opaque_graph_ref_index()` singleton never collide
    /// on the same digest.
    fn unique_name(label: &str) -> String {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("w1b-resolve-core-ref-test-{label}-{n}-{nanos}")
    }

    #[test]
    fn resolves_every_resident_graph_by_its_opaque_ref_after_a_cold_scan() {
        let mut registry = GraphRegistry::new();
        let a = unique_name("a");
        let b = unique_name("b");
        registry.create_graph(&a, GraphType::Agent, None).unwrap();
        registry.create_graph(&b, GraphType::Team, None).unwrap();

        let ref_a = native_opaque_ref("graph", &a);
        let ref_b = native_opaque_ref("graph", &b);

        let (name, kind, _core) = resolve_opaque_graph_ref(&registry, &ref_a).unwrap();
        assert_eq!(name, a);
        assert_eq!(kind, GraphType::Agent);

        let (name, kind, _core) = resolve_opaque_graph_ref(&registry, &ref_b).unwrap();
        assert_eq!(name, b);
        assert_eq!(kind, GraphType::Team);

        assert!(resolve_opaque_graph_ref(&registry, "eg:graph:not-a-real-digest").is_none());
    }

    #[test]
    fn a_warm_cache_hit_still_returns_the_live_registry_entry() {
        let mut registry = GraphRegistry::new();
        let name = unique_name("warm");
        registry
            .create_graph(&name, GraphType::Agent, None)
            .unwrap();
        let graph_ref = native_opaque_ref("graph", &name);

        // Cold lookup: falls back to the full scan and populates the index.
        let first = resolve_opaque_graph_ref(&registry, &graph_ref).unwrap();
        assert_eq!(first.0, name);

        // Warm lookup: same digest, same registry -- must take the cache-hit
        // path and still resolve to the live entry.
        let second = resolve_opaque_graph_ref(&registry, &graph_ref).unwrap();
        assert_eq!(second.0, name);
        assert_eq!(second.1, GraphType::Agent);
    }

    #[test]
    fn a_poisoned_cache_entry_can_only_waste_a_rescan_never_resolve_the_wrong_graph() {
        let mut registry = GraphRegistry::new();
        let real_name = unique_name("real");
        registry
            .create_graph(&real_name, GraphType::Agent, None)
            .unwrap();
        let real_ref = native_opaque_ref("graph", &real_name);

        // Directly poison the shared process-wide index with a WRONG name for
        // this exact digest, simulating a stale or corrupted cache entry --
        // e.g. left over from a deleted graph whose name got reused for a
        // digest collision class this test forces by hand.
        opaque_graph_ref_index()
            .write_recovering("opaque graph-ref cache")
            .insert(real_ref.clone(), "not-the-real-graph-name".to_string());

        // The live registry has no graph by that poisoned name, so the digest
        // re-verification must reject the cache hit and fall back to a fresh
        // scan -- resolving to the REAL graph, never the poisoned name and
        // never `None`.
        let resolved = resolve_opaque_graph_ref(&registry, &real_ref);
        assert_eq!(resolved.map(|(name, _, _)| name), Some(real_name));
    }

    #[test]
    fn a_deleted_graphs_stale_cache_entry_resolves_to_none_not_a_wrong_graph() {
        let mut registry = GraphRegistry::new();
        let name = unique_name("deleted");
        registry
            .create_graph(&name, GraphType::Agent, None)
            .unwrap();
        let graph_ref = native_opaque_ref("graph", &name);

        // Populate the cache while the graph is still live.
        assert!(resolve_opaque_graph_ref(&registry, &graph_ref).is_some());

        registry.delete_graph(&name).unwrap();

        // The index may still hold `graph_ref -> name`, but the live registry
        // no longer backs it -- must resolve to `None`, never a stale core.
        assert!(resolve_opaque_graph_ref(&registry, &graph_ref).is_none());
    }
}

// MEDULLA-DURABLE-KERNEL-R082.2: exercise the same typed governor used by Submit.
#[cfg(test)]
mod mine_associate_submission_tests {
    use super::*;
    use medulla_server_api::authority_context::VerifiedRequestContext;

    fn authority() -> CarrierAuthority {
        CarrierAuthority::from_verified(&VerifiedRequestContext::from_verified_claims(
            medulla_identity_types::acl::RequestContextClaims {
                principal: "analytics-test".into(),
                agent_id: "analytics-test".into(),
                tenant: "analytics-test-tenant".into(),
                audience: "medulla".into(),
                policy_version: "test".into(),
                ..Default::default()
            },
            "analytics-test-key".into(),
        ))
        .unwrap()
    }

    fn govern(kind: JobKind) -> Result<(JobKind, String, String, serde_json::Value), Response> {
        govern_job_kind(
            &authority(),
            82,
            kind,
            JobGovernance {
                policy_fingerprint: "policy:test",
                purpose: "",
                score_rows: None,
                required_capabilities: &mut Vec::new(),
            },
        )
    }

    fn mining_kind(support: f64, confidence: f64, algorithm: &str) -> JobKind {
        JobKind::MineAssociate {
            transactions: vec![vec!["eg:item:a".into(), "eg:item:b".into()]],
            min_support: support,
            min_confidence: confidence,
            algorithm: algorithm.into(),
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R082.2
    #[test]
    fn mine_associate_invalid_parameters_have_typed_bounded_refusals() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.01, 1.01] {
            for (support, confidence) in [(invalid, 0.5), (0.1, invalid)] {
                let response = govern(mining_kind(support, confidence, "fpgrowth")).unwrap_err();
                assert_eq!(response.id, 82);
                assert_eq!(response.error.as_deref(), Some("INVALID_ARGUMENT"));
                assert_eq!(
                    response.error_detail.as_deref(),
                    Some("association job thresholds are invalid")
                );
                assert!(response.result.is_none());
            }
        }
        for algorithm in ["", "unknown", "fpgrowth\nprivate-input", "private-input"] {
            let response = govern(mining_kind(0.1, 0.5, algorithm)).unwrap_err();
            assert_eq!(response.error.as_deref(), Some("INVALID_ARGUMENT"));
            assert_eq!(
                response.error_detail.as_deref(),
                Some("unsupported association job algorithm")
            );
        }
    }

    // spec: MEDULLA-PLACEMENT-R003.3
    #[test]
    fn mine_associate_accepts_thirty_one_distinct_items_and_refuses_thirty_two() {
        for count in [31, 32] {
            let transactions = vec![(0..count).map(|i| format!("item:{i}")).collect()];
            let outcome = govern(JobKind::MineAssociate {
                transactions,
                min_support: 0.1,
                min_confidence: 0.5,
                algorithm: "fpgrowth".into(),
            });
            if count == 31 {
                let (kind, _, _, _) = outcome.expect("the distinct-item cap is inclusive");
                match kind {
                    JobKind::MineAssociate { transactions, .. } => {
                        assert_eq!(transactions[0].len(), 31);
                        assert!(transactions[0]
                            .iter()
                            .all(|item| item.starts_with("eg:analytics_item:")));
                        assert_eq!(
                            transactions
                                .iter()
                                .flatten()
                                .collect::<std::collections::BTreeSet<_>>()
                                .len(),
                            31
                        );
                    }
                    _ => panic!("expected association job"),
                }
            } else {
                let response = outcome.expect_err("one item above the cap must be refused");
                assert_eq!(response.id, 82);
                assert_eq!(response.error.as_deref(), Some("INVALID_ARGUMENT"));
                assert_eq!(
                    response.error_detail.as_deref(),
                    Some("association job exceeds the distinct-item limit")
                );
                assert!(response.result.is_none());
            }
        }
    }

    // spec: MEDULLA-PLACEMENT-R003.5
    #[test]
    fn mine_associate_accepts_plain_item_references_and_stores_opaque_refs() {
        let (kind, _, _, _) = govern(JobKind::MineAssociate {
            transactions: vec![vec!["raw-label".into()]],
            min_support: 0.1,
            min_confidence: 0.5,
            algorithm: "fpgrowth".into(),
        })
        .expect("plain item labels are mapped before durable submission");
        match kind {
            JobKind::MineAssociate { transactions, .. } => {
                assert_eq!(transactions.len(), 1);
                assert_eq!(transactions[0].len(), 1);
                assert!(transactions[0][0].starts_with("eg:analytics_item:"));
                assert_ne!(transactions[0][0], "raw-label");
            }
            _ => panic!("expected association job"),
        }
    }
}

// MEDULLA-DURABLE-KERNEL-R082.1: a submitted MineAssociate job completes on the
// colocated executor with no external worker connected.
#[cfg(test)]
mod colocated_executor_tests {
    use medulla_persistence::redb_backend::RedbBackend;
    use medulla_protocol_types::protocol::{GraphType, Method, Response, ResultPayload};
    use medulla_server_api::state::ServerState;
    use medulla_server_auth::auth::{
        dispatch_test_on_heap, scoped_test_request_on, ScopedTestCaller,
    };
    use medulla_work_types::jobs::{JobKind, JobOp, SubmitJobSpec};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::RwLock;

    const SECRET: &str = "colocated-executor-test-secret";
    const ISSUER: &str = "colocated-executor-test-issuer";
    const TENANT: &str = "tenant-colocated-executor";
    const GRAPH: &str = "colocated-executor-test";

    fn next_nonce() -> u64 {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    async fn call(state: &Arc<RwLock<ServerState>>, id: u64, method: Method) -> Response {
        let request = scoped_test_request_on(
            SECRET,
            id,
            GRAPH,
            method,
            ScopedTestCaller {
                principal: ISSUER,
                tenant: TENANT,
                scopes: &["*"],
                nonce: &format!("colocated-executor-nonce-{id}-{}", next_nonce()),
                idempotency_key: &format!("colocated-executor-key-{id}-{}", next_nonce()),
            },
        );
        dispatch_test_on_heap(state, request).await
    }

    fn json_of(response: Response) -> serde_json::Value {
        assert!(response.error.is_none(), "{:?}", response.error);
        match response.result.expect("a result payload") {
            ResultPayload::Json(value) => value,
            ResultPayload::Raw(bytes) => rmp_serde::from_slice(&bytes).unwrap(),
            other => panic!("unexpected payload {other:?}"),
        }
    }

    fn submit_spec() -> SubmitJobSpec {
        SubmitJobSpec {
            graph: GRAPH.to_string(),
            tenant: String::new(),
            actor: String::new(),
            purpose: "colocated-executor".to_string(),
            priority: 0,
            deadline_unix_ms: None,
            quota_cpu_ms: None,
            memory_bytes: None,
            io_bytes: None,
            output_bytes: None,
            worker_pool: String::new(),
            worker_region: String::new(),
            required_capabilities: Vec::new(),
            max_attempts: 1,
            backoff_ms: 0,
            kind: JobKind::MineAssociate {
                transactions: vec![
                    vec!["item-a".to_string(), "item-b".to_string()],
                    vec!["item-a".to_string(), "item-b".to_string()],
                    vec!["item-a".to_string(), "item-c".to_string()],
                ],
                min_support: 0.3,
                min_confidence: 0.5,
                algorithm: "fpgrowth".to_string(),
            },
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R082.1
    #[tokio::test]
    async fn mine_associate_job_succeeds_on_the_colocated_executor() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = medulla_server::seams_install::new_state_for_test(
            SECRET,
            ServerState::test_isolation(ISSUER),
        );
        server.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        let backend =
            RedbBackend::open(dir.path().join("graphs").to_string_lossy().into_owned(), 64)
                .unwrap();
        server.persistence = Some(Arc::new(backend));
        let state = Arc::new(RwLock::new(server));

        let created = call(
            &state,
            1,
            Method::CreateGraph {
                graph_name: GRAPH.into(),
                graph_type: GraphType::Global,
            },
        )
        .await;
        assert!(created.error.is_none(), "{:?}", created.error);

        let submitted = json_of(
            call(
                &state,
                2,
                Method::AnalyticsJob {
                    op: JobOp::Submit(submit_spec()),
                },
            )
            .await,
        );
        let job_id = submitted
            .get("job_id")
            .and_then(serde_json::Value::as_str)
            .expect("submitted job id")
            .to_string();

        let mut succeeded = false;
        for id in 10..210 {
            let status = json_of(
                call(
                    &state,
                    id,
                    Method::AnalyticsJob {
                        op: JobOp::Status {
                            job_id: job_id.clone(),
                        },
                    },
                )
                .await,
            );
            let job_state = status.get("state").expect("job state");
            if job_state.get("Succeeded").is_some() {
                assert!(status.get("output").is_some_and(|value| !value.is_null()));
                succeeded = true;
                break;
            }
            assert!(
                job_state.get("Failed").is_none() && job_state.get("Cancelled").is_none(),
                "the colocated executor terminated the job unsuccessfully: {status}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(succeeded, "the colocated executor did not finish the job");
    }

    /// A kernel that runs far longer than any test waits: every subset of 20 items is
    /// frequent, so the rule enumeration is exponential. Only a stop signal ends it.
    fn long_kernel() -> JobKind {
        let items: Vec<String> = (0..20).map(|i| format!("item-{i:02}")).collect();
        JobKind::MineAssociate {
            transactions: vec![items.clone(), items.clone(), items],
            min_support: 0.1,
            min_confidence: 0.0,
            algorithm: "fpgrowth".to_string(),
        }
    }

    fn unix_now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    async fn prepared_state() -> Arc<RwLock<ServerState>> {
        let dir = tempfile::tempdir().unwrap();
        let mut server = medulla_server::seams_install::new_state_for_test(
            SECRET,
            ServerState::test_isolation(ISSUER),
        );
        server.persist_dir = Some(dir.path().to_string_lossy().into_owned());
        let backend =
            RedbBackend::open(dir.path().join("graphs").to_string_lossy().into_owned(), 64)
                .unwrap();
        server.persistence = Some(Arc::new(backend));
        // The job store and the pool live for the process; keep the directory with them.
        std::mem::forget(dir);
        let state = Arc::new(RwLock::new(server));
        let created = call(
            &state,
            1,
            Method::CreateGraph {
                graph_name: GRAPH.into(),
                graph_type: GraphType::Global,
            },
        )
        .await;
        assert!(created.error.is_none(), "{:?}", created.error);
        state
    }

    async fn status_of(state: &Arc<RwLock<ServerState>>, job_id: &str) -> serde_json::Value {
        json_of(
            call(
                state,
                next_nonce() + 1_000,
                Method::AnalyticsJob {
                    op: JobOp::Status {
                        job_id: job_id.to_string(),
                    },
                },
            )
            .await,
        )
    }

    async fn submit_long(state: &Arc<RwLock<ServerState>>, spec: SubmitJobSpec) -> String {
        let submitted = json_of(
            call(
                state,
                next_nonce() + 1_000,
                Method::AnalyticsJob {
                    op: JobOp::Submit(spec),
                },
            )
            .await,
        );
        submitted
            .get("job_id")
            .and_then(serde_json::Value::as_str)
            .expect("submitted job id")
            .to_string()
    }

    /// Poll until `done` holds of the job state, within `limit`.
    async fn wait_for(
        state: &Arc<RwLock<ServerState>>,
        job_id: &str,
        limit: Duration,
        done: impl Fn(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        let began = std::time::Instant::now();
        loop {
            let status = status_of(state, job_id).await;
            if done(status.get("state").expect("job state")) {
                return status;
            }
            assert!(
                began.elapsed() < limit,
                "the job did not reach the expected state in time: {status}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn call_scoped(
        state: &Arc<RwLock<ServerState>>,
        method: Method,
        scopes: &[&str],
    ) -> Response {
        let id = next_nonce() + 5_000;
        let request = scoped_test_request_on(
            SECRET,
            id,
            GRAPH,
            method,
            ScopedTestCaller {
                principal: ISSUER,
                tenant: TENANT,
                scopes,
                nonce: &format!("remote-worker-nonce-{id}-{}", next_nonce()),
                idempotency_key: &format!("remote-worker-key-{id}-{}", next_nonce()),
            },
        );
        dispatch_test_on_heap(state, request).await
    }

    async fn job_op(state: &Arc<RwLock<ServerState>>, op: JobOp) -> serde_json::Value {
        json_of(call_scoped(state, Method::AnalyticsJob { op }, &["*"]).await)
    }

    fn state_name(job: &serde_json::Value) -> String {
        match job.get("state").expect("job state") {
            serde_json::Value::String(name) => name.clone(),
            other => other
                .as_object()
                .and_then(|object| object.keys().next().cloned())
                .expect("a tagged job state"),
        }
    }

    // spec: MEDULLA-DURABLE-KERNEL-R082.10
    #[tokio::test]
    async fn remote_worker_drives_the_full_sequence_and_scope_is_enforced() {
        let state = prepared_state().await;
        // A capability no colocated executor serves, so only the remote worker claims the jobs.
        let capability = format!("remote-worker-{}", next_nonce());
        let spec = || SubmitJobSpec {
            required_capabilities: vec![capability.clone()],
            ..submit_spec()
        };
        let claim_op = |capabilities: Vec<String>| JobOp::WorkerClaim {
            worker_instance: "remote-slot".to_string(),
            capabilities,
            lease_ms: 60_000,
        };

        // The claim bound: 64 capabilities of at most 128 characters, no control characters.
        let too_many: Vec<String> = (0..65).map(|index| format!("cap-{index}")).collect();
        let refused = call_scoped(
            &state,
            Method::AnalyticsJob {
                op: claim_op(too_many),
            },
            &["*"],
        )
        .await;
        assert!(
            refused.error.as_deref().is_some_and(|error| error.contains("capabilities are invalid")),
            "{:?}",
            refused.error
        );
        for invalid in ["x".repeat(129), "bad\ncapability".to_string(), String::new()] {
            let refused = call_scoped(
                &state,
                Method::AnalyticsJob {
                    op: claim_op(vec![invalid]),
                },
                &["*"],
            )
            .await;
            assert!(refused.error.is_some(), "an invalid capability is refused");
        }

        // The service principal needs `analytics:worker`: a caller without it is denied.
        let denied = call_scoped(
            &state,
            Method::AnalyticsJob {
                op: claim_op(vec![capability.clone()]),
            },
            &["kg:read", "kg:write"],
        )
        .await;
        assert!(
            denied.error.as_deref().is_some_and(|error| error.contains("ACCESS_DENIED")),
            "{:?}",
            denied.error
        );

        // A cancelled run: claim, renew, checkpoint, cancel, then the worker confirms.
        let submitted = job_op(&state, JobOp::Submit(spec())).await;
        let job_id = submitted["job_id"].as_str().expect("job id").to_string();
        let claimed = job_op(&state, claim_op(vec![capability.clone()])).await;
        assert_eq!(claimed["job"]["job_id"], job_id);
        assert_eq!(state_name(&claimed["job"]), "Running");
        let epoch = claimed["lease"]["epoch"].as_u64().expect("lease epoch");
        let renewed = job_op(
            &state,
            JobOp::WorkerRenew {
                job_id: job_id.clone(),
                worker_instance: "remote-slot".to_string(),
                lease_epoch: epoch,
                lease_ms: 60_000,
            },
        )
        .await;
        assert_eq!(state_name(&renewed), "Running");
        let checkpointed = job_op(
            &state,
            JobOp::WorkerCheckpoint {
                job_id: job_id.clone(),
                worker_instance: "remote-slot".to_string(),
                lease_epoch: epoch,
                progress: 0.5,
                label: "half".to_string(),
                state_ref: None,
            },
        )
        .await;
        assert_eq!(state_name(&checkpointed), "Running");
        let requested = job_op(&state, JobOp::Cancel { job_id: job_id.clone() }).await;
        assert_eq!(state_name(&requested), "CancelRequested");
        let cancelled = job_op(
            &state,
            JobOp::WorkerCancel {
                job_id: job_id.clone(),
                worker_instance: "remote-slot".to_string(),
                lease_epoch: epoch,
            },
        )
        .await;
        assert_eq!(state_name(&cancelled), "Cancelled");

        // A failed run with one attempt ends Failed through the server-owned retry policy.
        let failing = job_op(&state, JobOp::Submit(spec())).await;
        let failing_id = failing["job_id"].as_str().expect("job id").to_string();
        let claim = job_op(&state, claim_op(vec![capability.clone()])).await;
        assert_eq!(claim["job"]["job_id"], failing_id);
        let failed = job_op(
            &state,
            JobOp::WorkerFail {
                job_id: failing_id.clone(),
                worker_instance: "remote-slot".to_string(),
                lease_epoch: claim["lease"]["epoch"].as_u64().expect("lease epoch"),
                reason_code: "kernel_failure".to_string(),
            },
        )
        .await;
        assert_eq!(state_name(&failed), "Failed");
    }

    // spec: MEDULLA-DURABLE-KERNEL-R082.7
    #[tokio::test]
    async fn cancel_deadline_and_cpu_budget_each_stop_the_kernel_with_their_reason() {
        let state = prepared_state().await;
        let limit = Duration::from_secs(60);

        // The status tick follows the claim's lease: a third of it, never above 5 s.
        assert_eq!(super::super::executor_claim::status_tick_ms(60_000), 5_000);
        assert_eq!(super::super::executor_claim::status_tick_ms(9_000), 3_000);
        assert_eq!(super::super::executor_claim::status_tick_ms(1), 1);

        // A cancel request stops the running kernel and the job ends `Cancelled`.
        let job_id = submit_long(
            &state,
            SubmitJobSpec {
                kind: long_kernel(),
                ..submit_spec()
            },
        )
        .await;
        wait_for(&state, &job_id, limit, |job| job.get("Running").is_some()).await;
        let _cancel_reply = json_of(
            call(
                &state,
                next_nonce() + 1_000,
                Method::AnalyticsJob {
                    op: JobOp::Cancel {
                        job_id: job_id.clone(),
                    },
                },
            )
            .await,
        );
        let status = wait_for(&state, &job_id, limit, |job| job.get("Cancelled").is_some()).await;
        assert!(status["lease"].is_null(), "no lease outlives the stop: {status}");

        // A passed deadline ends the job `Failed` with `deadline_exceeded`.
        let job_id = submit_long(
            &state,
            SubmitJobSpec {
                kind: long_kernel(),
                deadline_unix_ms: Some(unix_now_ms() + 1_000),
                ..submit_spec()
            },
        )
        .await;
        let status = wait_for(&state, &job_id, limit, |job| job.get("Failed").is_some()).await;
        assert_eq!(status["state"]["Failed"]["reason"], "deadline_exceeded");
        assert!(status["lease"].is_null(), "no lease outlives the stop: {status}");

        // An exhausted CPU budget ends it `Failed` with `cpu_budget_exceeded`.
        let job_id = submit_long(
            &state,
            SubmitJobSpec {
                kind: long_kernel(),
                quota_cpu_ms: Some(200),
                ..submit_spec()
            },
        )
        .await;
        let status = wait_for(&state, &job_id, limit, |job| job.get("Failed").is_some()).await;
        assert_eq!(status["state"]["Failed"]["reason"], "cpu_budget_exceeded");
        assert!(status["lease"].is_null(), "no lease outlives the stop: {status}");
    }

    // spec: MEDULLA-DURABLE-KERNEL-R082.8
    #[test]
    fn kernel_failed_twice_obeys_backoff_attempt_count_and_leaks_no_payload() {
        use super::super::executor_claim::{fail_attempt, FAILURE_CODES};
        use medulla_jobs::model::{AlgoVersion, InputSnapshotHandle, JobPolicy};
        use medulla_jobs::store::{SubmitSpec, TenantJobQuota};

        assert_eq!(
            FAILURE_CODES,
            [
                "invalid_payload",
                "kernel_failure",
                "deadline_exceeded",
                "cpu_budget_exceeded",
                "worker_execution_failed",
                "downtime_budget_exhausted",
                "downtime_body_missing",
            ]
        );

        let dir = tempfile::tempdir().unwrap();
        let store = super::super::core::open_job_store(dir.path()).expect("job store opens");
        let secret_label = "secret-payload-label";
        let payload = rmp_serde::to_vec_named(&JobKind::MineAssociate {
            transactions: vec![vec![secret_label.to_string(), "other".to_string()]],
            min_support: 0.5,
            min_confidence: 0.5,
            algorithm: "fpgrowth".to_string(),
        })
        .unwrap();
        let job = store
            .submit(SubmitSpec {
                input_snapshot: InputSnapshotHandle::new("eg:graph:r082-8", 1).with_dataset(
                    "eg:job_input:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                ),
                policy: JobPolicy {
                    tenant: "tenant-r082-8".into(),
                    actor: "actor".into(),
                    purpose: "retry".into(),
                    ..JobPolicy::default()
                },
                algo: AlgoVersion {
                    family: "mining.association".into(),
                    algorithm: "fpgrowth".into(),
                    params_digest: "params".into(),
                    code_version: "test".into(),
                    env_version: "test".into(),
                },
                input_payload: Some(payload),
                max_attempts: 3,
                backoff_ms: 1_000,
            })
            .unwrap();
        let claim_at = |at: i64| {
            store
                .claim_next("worker-ref", &[], at, 60_000, TenantJobQuota::default())
                .unwrap()
        };

        // Attempt 1 fails at t0: the job is claimable again only after `backoff_ms`.
        let t0 = 1_000_000_i64;
        let first = claim_at(t0).expect("attempt 1 is claimed");
        assert_eq!(first.job.job_id, job.job_id);
        assert!(fail_attempt(
            &store,
            &job.job_id,
            &first.lease.worker_ref,
            first.lease.epoch,
            "kernel_failure",
            t0
        )
        .is_ok());
        assert!(claim_at(t0 + 999).is_none(), "backoff holds the retry back");

        // Attempt 2 fails at t1.
        let t1 = t0 + 1_000;
        let second = claim_at(t1).expect("attempt 2 is claimed after the backoff");
        let failed_again = fail_attempt(
            &store,
            &job.job_id,
            &second.lease.worker_ref,
            second.lease.epoch,
            "kernel_failure",
            t1,
        )
        .unwrap();
        assert!(
            matches!(
                failed_again.state,
                medulla_jobs::model::JobState::Submitted { .. }
            ),
            "two failures of three attempts leave the job retryable"
        );
        assert_eq!(failed_again.not_before_ms, t1 + 1_000);
        assert!(claim_at(t1 + 999).is_none());

        // Attempt 3 is the last: its failure ends the job with the code only.
        let t2 = t1 + 1_000;
        let third = claim_at(t2).expect("attempt 3 is claimed after the backoff");
        let ended = fail_attempt(
            &store,
            &job.job_id,
            &third.lease.worker_ref,
            third.lease.epoch,
            "kernel_failure",
            t2,
        )
        .unwrap();
        let medulla_jobs::model::JobState::Failed { reason, .. } = &ended.state else {
            panic!("attempts stop at max_attempts: {:?}", ended.state)
        };
        assert_eq!(reason, "kernel_failure");
        assert!(FAILURE_CODES.contains(&reason.as_str()));
        assert!(!reason.contains(secret_label));
        assert!(claim_at(t2 + 10_000).is_none(), "a failed job is never claimed again");

        // A code outside the closed set is refused before any write.
        assert!(fail_attempt(&store, &job.job_id, "worker-ref", 1, "lease_lost", t2).is_err());
        assert!(fail_attempt(&store, &job.job_id, "worker-ref", 1, secret_label, t2).is_err());
    }
}

/// The staged association result of the job plane (MEDULLA-DURABLE-KERNEL-R082.5).
#[cfg(test)]
mod association_result_tests {
    use super::*;
    use sha2::{Digest, Sha256};

    // spec: MEDULLA-DURABLE-KERNEL-R082.5
    #[test]
    fn association_result_has_the_eleven_column_shape_and_recomputed_digest() {
        let dataset_ref = native_opaque_ref("dataset", "association-result-input");
        let input_digest = hex::encode(Sha256::digest(b"association-input"));
        let job = medulla_jobs::AnalyticsJob {
            job_id: "association-result-job".to_string(),
            input_snapshot: medulla_jobs::model::InputSnapshotHandle::new(
                native_opaque_ref("graph", "association-result"),
                7,
            )
            .with_dataset(dataset_ref.clone(), input_digest.clone()),
            policy: medulla_jobs::model::JobPolicy {
                policy_fingerprint: "policy-fingerprint-1".to_string(),
                ..medulla_jobs::model::JobPolicy::default()
            },
            algo: medulla_jobs::model::AlgoVersion {
                family: "mining.association".to_string(),
                algorithm: "fpgrowth".to_string(),
                params_digest: "params-digest-1".to_string(),
                code_version: "code-version-1".to_string(),
                env_version: "env-version-1".to_string(),
            },
            input_payload: None,
            retry: medulla_jobs::model::RetryPolicy::default(),
            state: medulla_jobs::model::JobState::Submitted { checkpoint: None },
            lease_epoch: 0,
            last_worker_ref: String::new(),
            output: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let rules = vec![
            medulla_mining::mining::association::LabeledRule {
                antecedent: vec!["a".to_string()],
                consequent: vec!["b".to_string()],
                support: 0.5,
                confidence: 0.8,
                lift: 1.6,
            },
            medulla_mining::mining::association::LabeledRule {
                antecedent: vec!["b".to_string(), "c".to_string()],
                consequent: vec!["d".to_string()],
                support: 0.25,
                confidence: 0.5,
                lift: 2.0,
            },
        ];

        let result = typed_association_result(&job, &rules).expect("the staged result builds");

        // The eleven columns, in order.
        let columns: Vec<&str> = result
            .schema
            .iter()
            .map(|column| column.name.as_str())
            .collect();
        assert_eq!(
            columns,
            [
                "id",
                "kind",
                "confidence",
                "evidence_refs",
                "source_refs",
                "proof_ids",
                "contradiction_ids",
                "antecedent",
                "consequent",
                "support",
                "lift",
            ]
        );
        assert_eq!(result.rows.len(), 2);

        // Evidence and source refs are the input dataset ref.
        assert_eq!(result.evidence_refs, vec![dataset_ref.clone()]);
        for row in &result.rows {
            assert_eq!(row["evidence_refs"], serde_json::json!([dataset_ref]));
            assert_eq!(row["source_refs"], serde_json::json!([dataset_ref]));
            assert_eq!(row["kind"], serde_json::json!("association_rule"));
            assert_eq!(row["proof_ids"], serde_json::json!([]));
            assert_eq!(row["contradiction_ids"], serde_json::json!([]));
        }

        // The result ref is `eg:knowledge_batch:` plus the SHA-256 over the schema and the rows,
        // recomputed here independently of the producer.
        let mut hasher = Sha256::new();
        hasher.update(b"eg-jobs.knowledge-batch.v1\0");
        hasher.update(rmp_serde::to_vec_named(&result.schema).unwrap());
        hasher.update([0]);
        hasher.update(rmp_serde::to_vec_named(&result.rows).unwrap());
        let digest = hex::encode(hasher.finalize());
        assert_eq!(digest.len(), 64);
        assert!(digest
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')));
        assert_eq!(result.content_digest, digest);
        assert_eq!(result.dataset_ref, format!("eg:knowledge_batch:{digest}"));
        result.validate().expect("the content identity holds");

        // Uncertainty is the mean of one minus confidence; calibration is the min and max
        // of support times confidence.
        let uncertainty = result.uncertainty.expect("uncertainty is set");
        assert!((uncertainty - 0.35).abs() < 1e-12, "{uncertainty}");
        let (low, high) = result.calibration.expect("calibration is set");
        assert!((low - 0.125).abs() < 1e-12, "{low}");
        assert!((high - 0.4).abs() < 1e-12, "{high}");

        // The reproducibility block.
        let reproducibility = &result.reproducibility;
        assert_eq!(reproducibility.input_dataset_ref, dataset_ref);
        assert_eq!(reproducibility.input_content_digest, input_digest);
        assert_eq!(reproducibility.input_snapshot_version, 7);
        assert_eq!(reproducibility.algorithm_ref, "mining.association:fpgrowth");
        assert_eq!(reproducibility.params_digest, "params-digest-1");
        assert_eq!(reproducibility.implementation_version, "code-version-1");
        assert_eq!(reproducibility.environment_version, "env-version-1");
        assert_eq!(reproducibility.policy_fingerprint, "policy-fingerprint-1");
    }
}
