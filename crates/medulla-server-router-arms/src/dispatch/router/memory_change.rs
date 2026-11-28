//! `Method::Consolidate` behind the memory-change admission gate
//! (MEDULLA-DECISION-ENGINE-R141.104): plan, then apply.
//!
//! `medulla-h-graph` never calls `medulla-h-decide`, so the router, which depends on
//! both, runs the gate before the change reaches the graph route: it derives the
//! candidate (semantic node id, cluster digest and stored head revision), checks
//! `require_admit_receipts`, and hands the method on with the receipt digests
//! recorded as the premises `memory:receipt=<digest>` of the new semantic node.
//! A refused gate writes nothing.

use super::*;

mod holder;

/// Hand `Consolidate` through the gate; every other method continues.
pub async fn dispatch_consolidate_method(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::Consolidate {
        episodic_ids,
        semantic_props_msgpack,
        admit_receipt_digests,
    } = method
    else {
        return ControlFlow::Continue(method);
    };
    ControlFlow::Break(
        handle_consolidate(
            ctx,
            episodic_ids,
            semantic_props_msgpack,
            admit_receipt_digests,
        )
        .await,
    )
}

/// Serve `Method::Consolidate`: gate, then the existing graph route.
async fn handle_consolidate(
    ctx: DispatchCtx<'_>,
    episodic_ids: Vec<String>,
    props_msgpack: Vec<u8>,
    admit_receipt_digests: Vec<String>,
) -> Response {
    use medulla_decision_statistical_types::decision::memory_benchmark::ChangeKind;
    use {
        medulla_h_decide_learning::decide::stat_memory_change::consolidate_candidate,
        medulla_h_decide_learning::decide::stat_memory_change::require_admit_receipts,
    };
    let DispatchCtx {
        state,
        req,
        verified_context,
        ..
    } = ctx;
    let mut props =
        medulla_types::msgpack::decode_property_object(&props_msgpack).unwrap_or_default();
    let admitted = async {
        let candidate =
            consolidate_candidate(state, verified_context, &req.graph, &episodic_ids, &props)
                .await?;
        require_admit_receipts(
            state,
            verified_context,
            ChangeKind::Consolidate,
            &candidate,
            &admit_receipt_digests,
        )
        .await
    }
    .await;
    let premises = match admitted {
        Ok(premises) => premises,
        Err(refusal) => return Response::err(req.id, refusal),
    };
    props.insert("premises".to_string(), serde_json::json!(premises));
    let props_msgpack = match rmp_serde::to_vec_named(&serde_json::Value::Object(props)) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Response::err(
                req.id,
                format!("INVALID_ARGUMENT: semantic props encoding failed: {error}"),
            )
        }
    };
    dispatch_boxed(dispatch_graph_op(
        state,
        &req.graph,
        req.id,
        req.agent_id.as_deref(),
        verified_context,
        Method::Consolidate {
            episodic_ids,
            semantic_props_msgpack: props_msgpack,
            admit_receipt_digests,
        },
    ))
    .await
}

/// Hand `SyncMemorySnapshot` through plan, gate and apply; every other method
/// continues (MEDULLA-DURABLE-KERNEL-R085.4, R863).
pub async fn dispatch_sync_memory_method(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let Method::SyncMemorySnapshot { request, .. } = method else {
        return ControlFlow::Continue(method);
    };
    ControlFlow::Break(handle_sync_memory(ctx, *request).await)
}

/// How many times a plan is made again when the head moved before its commit.
const SYNC_PLAN_ATTEMPTS: usize = 3;

/// Serve `Method::SyncMemorySnapshot`: the graph handler plans the sync against
/// the stored rows, a change to existing content passes the admission gate, and
/// the plan commits through the graph gateway as one batch (head, revision and
/// the `memory_snapshot_synced` outbox intent) over the pre-image it was decided
/// against. The blob holder of the committed content is taken after the commit
/// by the router (`holder`), never by a graph-handler dependency on the blob store.
/// A client-supplied stamp is replaced; a refusal writes nothing.
async fn handle_sync_memory(
    ctx: DispatchCtx<'_>,
    request: medulla_mutation_types::memory_sync::SyncRequest,
) -> Response {
    use medulla_decision_statistical_types::decision::memory_benchmark::ChangeKind;
    use {
        medulla_h_decide_learning::decide::stat_memory_change::require_admit_receipts,
        medulla_h_decide_learning::decide::stat_memory_change::sync_candidate,
    };
    use {
        medulla_h_graph_gateway::graph_ops::memory_snapshot::check_doc_kind,
        medulla_h_graph_gateway::graph_ops::memory_snapshot::content_change,
        medulla_h_graph_gateway::graph_ops::memory_snapshot::decide_sync,
        medulla_h_graph_gateway::graph_ops::memory_snapshot::exceeds_capacity,
        medulla_h_graph_gateway::graph_ops::memory_snapshot::outcome_response,
        medulla_h_graph_gateway::graph_ops::memory_snapshot::stamp_for,
        medulla_h_graph_gateway::graph_ops::memory_snapshot::ContentChange,
    };
    let DispatchCtx {
        state,
        req,
        verified_context,
        ..
    } = ctx;
    if let Err(error) = request.validate() {
        return Response::err(req.id, error.code.as_str());
    }
    let stamp = stamp_for(
        req.agent_id.as_deref(),
        verified_context.tenant(),
        &req.graph,
    );
    let mut last = Response::err(
        req.id,
        "BUSY: the memory snapshot moved between plan and commit",
    );
    for _ in 0..SYNC_PLAN_ATTEMPTS {
        medulla_server_api::api::reentry::ensure_graph_resident(state, &req.graph).await;
        let core = {
            let s = state.read().await;
            match s.registry.get(&req.graph) {
                Some(entry) => entry.core.clone(),
                None => {
                    return Response::err(
                        req.id,
                        medulla_mutation_types::memory_sync::MemorySyncRefusal::NotAvailable
                            .as_str(),
                    )
                }
            }
        };
        // The pre-commit CAS step: a head whose content the CAS cannot serve reads
        // as the `Unavailable` standing.
        let head_content_held =
            match medulla_h_graph_gateway::graph_ops::memory_snapshot::head_content_digest(
                &core,
                &request.doc_id,
            ) {
                Some(digest) => holder::content_held(state, &digest).await,
                None => true,
            };
        let (outcome, snapshot) = decide_sync(&core, &request, &stamp, head_content_held);
        if let Err(refusal) = check_doc_kind(&request, &outcome, &snapshot) {
            return Response::err(req.id, refusal);
        }
        if snapshot.writes.is_empty() {
            return outcome_response(req.id, outcome, Vec::new());
        }
        // A write over the tenant's capacity is refused before any effect.
        if exceeds_capacity(
            &core,
            &request.doc_id,
            &outcome,
            &snapshot,
            medulla_server_api::knob_config::memory_sync_config().capacity_bytes,
        ) {
            return Response::err(
                req.id,
                medulla_mutation_types::memory_sync::MemorySyncRefusal::RetentionExhausted.as_str(),
            );
        }
        if let Some((change, digest, _)) = content_change(&request, &outcome, &snapshot) {
            let kind = match change {
                ContentChange::Rewrite => ChangeKind::Rewrite,
                ContentChange::Consolidate => ChangeKind::Consolidate,
            };
            let admitted = async {
                let candidate = sync_candidate(
                    state,
                    verified_context,
                    &req.graph,
                    request.doc_id.as_str(),
                    &format!("sha256:{}", digest.to_hex()),
                )
                .await?;
                require_admit_receipts(
                    state,
                    verified_context,
                    kind,
                    &candidate,
                    &request.admit_receipt_digests,
                )
                .await
            }
            .await;
            if let Err(refusal) = admitted {
                return Response::err(req.id, refusal);
            }
        }
        // The content is content-addressed and idempotent, so it reaches the CAS
        // before the commit: a storage failure refuses before anything commits.
        let staged = match holder::stage_content(state, &request, &snapshot, &outcome).await {
            Ok(staged) => staged,
            Err(error) => return Response::err(req.id, error),
        };
        let response = dispatch_boxed(dispatch_graph_op(
            state,
            &req.graph,
            req.id,
            req.agent_id.as_deref(),
            verified_context,
            Method::SyncMemorySnapshotStore {
                request: Box::new(request.clone()),
                plan: Box::new(snapshot),
            },
        ))
        .await;
        match response.error.as_deref() {
            None => {
                // A committed write is never reported as a failure: a holder that
                // could not be registered is a warning, and the consumer repairs it.
                let held = match &staged {
                    Some(staged) => {
                        holder::hold_staged(state, verified_context, req.id, &request, staged).await
                    }
                    None => Ok(()),
                };
                let warnings = match held {
                    Ok(()) => Vec::new(),
                    Err(error) => {
                        tracing::warn!(code = holder::HOLDER_PENDING, %error,
                            "memory snapshot holder not registered after commit");
                        vec![holder::HOLDER_PENDING.to_string()]
                    }
                };
                return outcome_response(req.id, outcome, warnings);
            }
            Some(error) if error.starts_with("BUSY") => last = response,
            Some(_) => return response,
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_h_decide::decide::stat_test_support::{Harness, TENANT};
    use medulla_h_decide_learning::decide::stat_memory_change::benchmark_receipt_row;
    use {
        medulla_decision_statistical_types::decision::memory_benchmark::BenchmarkVerdictKind,
        medulla_decision_statistical_types::decision::memory_benchmark::ChangeKind,
        medulla_decision_statistical_types::decision::memory_benchmark::MemoryBenchmarkReceipt,
        medulla_decision_statistical_types::decision::memory_benchmark::RevisionRef,
        medulla_decision_statistical_types::decision::memory_benchmark::RunMetricsReceipt,
        medulla_decision_statistical_types::decision::memory_benchmark::MEMORY_BENCHMARK_SCHEMA_VERSION,
    };

    const OTHER: &str = "tenant-other";

    fn cluster() -> Vec<String> {
        vec!["episode-b".to_string(), "episode-a".to_string()]
    }

    fn receipt(
        record_id: &str,
        verdict: BenchmarkVerdictKind,
        candidate: &str,
        head: u64,
    ) -> MemoryBenchmarkReceipt {
        let metrics = RunMetricsReceipt {
            recall_ppm_sum: 1_000_000,
            correct: 1,
            queries: 1,
            recall_at_k_ppm: 1_000_000,
            answer_accuracy_ppm: 1_000_000,
        };
        let mut receipt = MemoryBenchmarkReceipt {
            schema_version: MEMORY_BENCHMARK_SCHEMA_VERSION,
            tenant_id: TENANT.to_string(),
            record_id: record_id.to_string(),
            change_kind: ChangeKind::Consolidate,
            baseline: RevisionRef {
                revision: head,
                digest: "sha256:base".to_string(),
            },
            candidate: RevisionRef {
                revision: head + 1,
                digest: candidate.to_string(),
            },
            dataset_id: "dataset".to_string(),
            dataset_revision: 1,
            dataset_digest: "sha256:data".to_string(),
            k: 5,
            evaluator_revision: "e1".to_string(),
            policy_digest: "0".repeat(64),
            baseline_metrics: metrics.clone(),
            candidate_metrics: metrics,
            verdict,
            reason_codes: Vec::new(),
            premises: Vec::new(),
            receipt_digest: String::new(),
        };
        receipt.receipt_digest = receipt.compute_receipt_digest();
        receipt
    }

    fn props() -> Vec<u8> {
        rmp_serde::to_vec_named(&serde_json::json!({"summary": "s"})).unwrap()
    }

    async fn consolidate(
        h: &Harness,
        verified: &VerifiedRequestContext,
        digests: Vec<String>,
    ) -> Response {
        let req = DispatchHeader {
            id: 1,
            graph: "default".to_string(),
            agent_id: Some("decider".to_string()),
        };
        let ctx = DispatchCtx {
            state: &h.state,
            req: &req,
            verified_context: verified,
            state_machine_authorized: false,
            identity_bootstrap: false,
        };
        let method = Method::Consolidate {
            episodic_ids: cluster(),
            semantic_props_msgpack: props(),
            admit_receipt_digests: digests,
        };
        match dispatch_consolidate_method(ctx, method).await {
            ControlFlow::Break(response) => response,
            ControlFlow::Continue(_) => panic!("Consolidate is served by the gate arm"),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R141.104
    #[tokio::test]
    async fn consolidate_requires_a_current_admit_receipt() {
        let h = Harness::with_isolation(medulla_server_api::state::ServerState::test_isolation(
            "decider",
        ))
        .await;
        {
            let mut state = h.state.write().await;
            state
                .registry
                .create_graph(
                    "default",
                    medulla_protocol_types::protocol::GraphType::Global,
                    None,
                )
                .expect("graph");
        }
        let scopes = ["kg:read", "kg:write"];
        let verified =
            VerifiedRequestContext::verified_for_test_with_scopes("decider", TENANT, &scopes);
        let candidate =
            medulla_h_decide_learning::decide::stat_memory_change::consolidate_candidate(
                &h.state,
                &verified,
                "default",
                &cluster(),
                &medulla_types::msgpack::decode_property_object(&props()).unwrap(),
            )
            .await
            .expect("a candidate");
        assert_eq!(candidate.head_revision, 0);
        let (id, digest) = (&candidate.record_id, &candidate.candidate_digest);
        let discard = receipt(id, BenchmarkVerdictKind::Discard, digest, 0);
        let other = receipt(id, BenchmarkVerdictKind::Admit, "sha256:other", 0);
        let old = receipt(id, BenchmarkVerdictKind::Admit, digest, 7);
        let good = receipt(id, BenchmarkVerdictKind::Admit, digest, 0);
        h.store
            .put_decision_artifacts(
                TENANT,
                &[&discard, &other, &old, &good]
                    .map(|receipt| benchmark_receipt_row(receipt).unwrap()),
            )
            .unwrap();
        let version = |h: &Harness| {
            let h = h.state.clone();
            async move {
                h.read()
                    .await
                    .registry
                    .get("default")
                    .unwrap()
                    .core
                    .version()
            }
        };
        let before = version(&h).await;
        // No receipt, an unknown digest, a discard, another candidate and another head
        // each refuse by their own code and change nothing.
        for (digests, code) in [
            (Vec::new(), "MEMORY_RECEIPT_REQUIRED"),
            (vec!["sha256:absent".to_string()], "MEMORY_RECEIPT_REQUIRED"),
            (
                vec![discard.receipt_digest.clone()],
                "MEMORY_RECEIPT_NOT_ADMIT",
            ),
            (vec![other.receipt_digest.clone()], "MEMORY_RECEIPT_STALE"),
            (vec![old.receipt_digest.clone()], "MEMORY_RECEIPT_STALE"),
        ] {
            let refused = consolidate(&h, &verified, digests).await.error.unwrap();
            assert!(refused.starts_with(code), "{refused}");
        }
        assert_eq!(version(&h).await, before, "a refused gate writes nothing");
        // Another tenant's caller reads the same digest as absent.
        let foreign =
            VerifiedRequestContext::verified_for_test_with_scopes("decider", OTHER, &scopes);
        let refused = consolidate(&h, &foreign, vec![good.receipt_digest.clone()])
            .await
            .error
            .unwrap();
        assert!(refused.starts_with("MEMORY_RECEIPT_REQUIRED"), "{refused}");
        // The matching receipt commits one semantic node whose premises name it.
        let admitted = consolidate(&h, &verified, vec![good.receipt_digest.clone()]).await;
        let Some(ResultPayload::String(semantic)) = admitted.result else {
            panic!("admitted: {:?}", admitted.error)
        };
        assert_eq!(&semantic, id);
        let state = h.state.read().await;
        let core = &state.registry.get("default").unwrap().core;
        let node = medulla_types::msgpack::decode_property_object(
            &core.get_node_properties(&semantic).unwrap(),
        )
        .unwrap();
        assert_eq!(
            node["premises"],
            serde_json::json!([format!("memory:receipt={}", good.receipt_digest)])
        );
    }
}
