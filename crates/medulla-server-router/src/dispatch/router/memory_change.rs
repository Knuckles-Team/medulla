//! `Method::Consolidate` behind the memory-change admission gate
//! (MEDULLA-DECISION-ENGINE-R141.104): plan, then apply.
//!
//! `medulla-h-graph` never calls `medulla-h-decide`, so the router, which depends on
//! both, runs the gate between the two halves. The plan reads the target graph's
//! version and checks `require_admit_receipts` against the cluster and that head;
//! the apply refuses `STALE_VERSION` when the graph has moved since the plan, then
//! commits through the one graph path with the receipt digests recorded as the
//! premises `memory:receipt=<digest>` of the new semantic node. A refused gate
//! writes nothing.

use super::*;

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
    let DispatchCtx {
        state,
        req,
        verified_context,
        ..
    } = ctx;
    ControlFlow::Break(
        match admit(
            state,
            req,
            verified_context,
            &episodic_ids,
            semantic_props_msgpack,
            &admit_receipt_digests,
        )
        .await
        {
            Ok(props) => {
                dispatch_boxed(dispatch_graph_op(
                    state,
                    &req.graph,
                    req.id,
                    req.agent_id.as_deref(),
                    verified_context,
                    Method::Consolidate {
                        episodic_ids,
                        semantic_props_msgpack: props,
                        admit_receipt_digests,
                    },
                ))
                .await
            }
            Err(refusal) => Response::err(req.id, refusal),
        },
    )
}

/// The canonical (sorted, deduplicated) candidate ids of a consolidation.
fn canonical_cluster(episodic_ids: &[String]) -> Vec<String> {
    let mut cluster = episodic_ids.to_vec();
    cluster.sort();
    cluster.dedup();
    cluster
}

/// Check the gate and answer the semantic props with the receipt premises added.
async fn admit(
    state: &Arc<RwLock<ServerState>>,
    req: &DispatchHeader,
    verified: &VerifiedRequestContext,
    episodic_ids: &[String],
    props_msgpack: Vec<u8>,
    digests: &[String],
) -> Result<Vec<u8>, String> {
    use medulla_decision_types::decision::memory_benchmark::ChangeKind;
    use medulla_h_decide::decide::stat_memory_change::{
        planned_head_revision, require_admit_receipts, MemoryChange,
    };
    let mut props = medulla_types::msgpack::decode_property_object(&props_msgpack)
        .unwrap_or_default();
    let candidate_digest = medulla_decision_types::decision::digest::digest_text_hex(
        "eg/memory-change-candidate/v1",
        &canonical_cluster(episodic_ids),
    );
    // The record a consolidation writes: the semantic id the caller names, else the
    // one its cluster derives.
    let record_id = match props.get("id").and_then(|id| id.as_str()) {
        Some(id) => id.to_string(),
        None => format!("consolidate:{candidate_digest}"),
    };
    let head = planned_head_revision(state, verified, &req.graph).await?;
    let store = state.write().await.ensure_agent_library()?;
    let premises = require_admit_receipts(
        &store,
        verified.tenant(),
        &MemoryChange {
            record_id: &record_id,
            kind: ChangeKind::Consolidate,
            candidate_digest: &candidate_digest,
            head_revision: head,
        },
        digests,
    )?;
    // The apply half: the graph must still stand at the planned version.
    if planned_head_revision(state, verified, &req.graph).await? != head {
        return Err("STALE_VERSION: the graph moved after the admission was planned".to_string());
    }
    props.insert("premises".to_string(), serde_json::json!(premises));
    rmp_serde::to_vec_named(&serde_json::Value::Object(props))
        .map_err(|error| format!("INVALID_ARGUMENT: semantic props encoding failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::memory_benchmark::{
        BenchmarkVerdictKind, ChangeKind, MemoryBenchmarkReceipt, RevisionRef, RunMetricsReceipt,
        MEMORY_BENCHMARK_SCHEMA_VERSION,
    };
    use medulla_h_decide::decide::stat_memory_change::benchmark_receipt_row;
    use medulla_h_decide::decide::stat_test_support::{Harness, TENANT};

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
                .create_graph("default", medulla_protocol_types::protocol::GraphType::Global, None)
                .expect("graph");
        }
        let verified = VerifiedRequestContext::verified_for_test_with_scopes(
            "decider",
            TENANT,
            &["kg:read", "kg:write"],
        );
        let req = DispatchHeader {
            id: 1,
            graph: "default".to_string(),
            agent_id: Some("decider".to_string()),
        };
        let digest = medulla_decision_types::decision::digest::digest_text_hex(
            "eg/memory-change-candidate/v1",
            &{
                let mut sorted = cluster();
                sorted.sort();
                sorted
            },
        );
        let record_id = format!("consolidate:{digest}");
        let head = planned(&h, &verified).await;
        let props = rmp_serde::to_vec_named(&serde_json::json!({"summary": "s"})).unwrap();
        let ask = |digests: Vec<String>, tenant_verified: &VerifiedRequestContext| {
            let h = &h;
            let req = &req;
            let props = props.clone();
            let tenant_verified = tenant_verified.clone();
            async move {
                admit(&h.state, req, &tenant_verified, &cluster(), props, &digests).await
            }
        };
        // No receipt, then an unknown digest: required.
        for digests in [Vec::new(), vec!["sha256:absent".to_string()]] {
            let refused = ask(digests, &verified).await.unwrap_err();
            assert!(refused.starts_with("MEMORY_RECEIPT_REQUIRED"), "{refused}");
        }
        // A discard receipt, a receipt bound to another candidate and one bound to another
        // head are each refused by their own code.
        let discard = receipt(&record_id, BenchmarkVerdictKind::Discard, &digest, head);
        let other = receipt(&record_id, BenchmarkVerdictKind::Admit, "sha256:other", head);
        let old = receipt(&record_id, BenchmarkVerdictKind::Admit, &digest, head + 7);
        let admit_ok = receipt(&record_id, BenchmarkVerdictKind::Admit, &digest, head);
        h.store
            .put_decision_artifacts(
                TENANT,
                &[
                    benchmark_receipt_row(&discard).unwrap(),
                    benchmark_receipt_row(&other).unwrap(),
                    benchmark_receipt_row(&old).unwrap(),
                    benchmark_receipt_row(&admit_ok).unwrap(),
                ],
            )
            .unwrap();
        for (bad, code) in [
            (&discard, "MEMORY_RECEIPT_NOT_ADMIT"),
            (&other, "MEMORY_RECEIPT_STALE"),
            (&old, "MEMORY_RECEIPT_STALE"),
        ] {
            let refused = ask(vec![bad.receipt_digest.clone()], &verified)
                .await
                .unwrap_err();
            assert!(refused.starts_with(code), "{refused}");
        }
        // The matching receipt admits and is recorded as a premise.
        let props = ask(vec![admit_ok.receipt_digest.clone()], &verified)
            .await
            .expect("an admit receipt bound to the cluster and head");
        let props = medulla_types::msgpack::decode_property_object(&props).unwrap();
        assert_eq!(
            props["premises"],
            serde_json::json!([format!("memory:receipt={}", admit_ok.receipt_digest)])
        );
        // Another tenant's caller reads the same digest as absent.
        let foreign = VerifiedRequestContext::verified_for_test_with_scopes(
            "decider",
            OTHER,
            &["kg:read", "kg:write"],
        );
        let refused = ask(vec![admit_ok.receipt_digest.clone()], &foreign)
            .await
            .unwrap_err();
        assert!(refused.starts_with("MEMORY_RECEIPT_REQUIRED"), "{refused}");
    }

    async fn planned(h: &Harness, verified: &VerifiedRequestContext) -> u64 {
        medulla_h_decide::decide::stat_memory_change::planned_head_revision(
            &h.state, verified, "default",
        )
        .await
        .expect("a readable graph")
    }
}
