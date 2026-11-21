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
}
