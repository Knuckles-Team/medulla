//! `MutationPlan` + the single commit gateway (CONCEPT:MEDULLA-P0-2).
//!
//! ## Why this exists
//!
//! `medulla-capabilities` (CONCEPT:MEDULLA-P0-1) is a machine-checked, exhaustive `MethodPolicy`
//! table over every `Method` variant — but on its own it is a one-way AUDIT: it
//! cross-checks the existing hand-rolled classifiers (`access.rs::requires_write`,
//! `mutation_apply::is_durable_mutation`, `audit.rs::audit_line`, `cdc.rs::emit_for_method`)
//! against its declared policy, without either side CONSUMING the other. Handlers
//! still mutate `medulla-core` directly (`src/server/handlers/graph_ops.rs`'s
//! `g.add_node(...)` etc.), and the dispatch shell (`src/server/dispatch.rs`)
//! separately re-derives whether to durably commit / CDC-emit a given method from its
//! own classifiers.
//!
//! This module is the other direction: [`MutationPlan`] is POPULATED FROM
//! `medulla_capabilities::policy` (never re-hardcoded), and [`commit_mutation`] is the
//! ONE place a routed mutation's authz + durable write + CDC emission happen
//! together, driven by that plan. The invariant this buys: for a method in
//! [`plan::GATEWAY_ROUTED`], mutation + durability + audit + CDC happen together, in one
//! call, declared by policy — never scattered back across the dispatch shell.
//!
//! ## Scope (read before assuming more is covered)
//!
//! [`plan::GATEWAY_ROUTED`] methods are wired through this gateway. MEDULLA-P0-2 started with
//! 7 (`AddNode`/`RemoveNode`/`AddEdge`/`RemoveEdge` + `CreateSummaryNode`/
//! `Consolidate`/`Reinforce`); the L11 rollout expanded it across the full routed
//! graph surface, adding:
//!   * the rest of the plain graph-core family (CAS/claim/edge-temporal/scene/
//!     trajectory/embedding/ledger/lifecycle ops — all handled in `graph_ops.rs`
//!     already, same shape as the original 7);
//!   * the message-broker/stream family (`Outbox` durability domain, behind
//!     `feature = "broker"` — proves the gateway is domain-agnostic: `commit_mutation`
//!     doesn't branch on `DurabilityDomain`, only on whether it's `None`);
//!   * `IcvConfigure` (validated graph control state carried in the staged
//!     `GraphCore`, so policy and data share the graph-scoped durable commit);
//!   * BOTH runtime-conditional families whose REAL `mutates` answer is a
//!     per-invocation `writeback: bool` field, not `policy()`'s conservative
//!     upper bound: `GraphLearnFit`/`GraphLearnPredict` and every writeback-capable
//!     `Mine*` — routed via [`commit_conditional_mutation`], NOT `commit_mutation`,
//!     so a `writeback: false` call is correctly treated as a plain read (Read
//!     ACL, no durability/audit/CDC) instead of being incorrectly gated behind
//!     Write access or persisted as a phantom mutation.
//!
//! Coverage is machine-partitioned by
//! `tests::gateway_routed_set_matches_mutating_policy_surface`: graph-scoped
//! mutations use this gateway; WorkItem, OCC/2PC, lifecycle, and non-graph stores
//! identify their native atomic coordinator. `OPEN_NOT_JUSTIFIED` is asserted
//! empty, so a new mutating method cannot silently bypass both categories.
//!
//! `src/server/dispatch.rs` wires `medulla_h_graph::graph_ops::try_handle_gateway` ahead
//! of the generic handler chain. A routed method therefore reaches exactly one
//! mutation kernel. The five coalescable structural writes route through
//! [`commit_coalescable_mutation`] (L18, see its doc) instead of [`commit_mutation`],
//! so routed structural writes retain batching without a second durability path.
//!
//! ## What this gateway does NOT reimplement
//!
//! Audit-chain emission (the tamper-evident hash chain, `audit.rs` +
//! `redb_store::append_audit_entry`) is stateful PER GRAPH (each entry chains off
//! the previous one's hash) and already lives inside the durable-commit path
//! ([`medulla_server::api::persistence::PersistenceBackend::record_durable`] → the redb backend). Reimplementing
//! that chaining here would risk a second, diverging chain. Instead,
//! Compact row commits and staged-state MutationBatch commits both append the audit
//! entry inside their authoritative redb transaction. `plan.audited`
//! (sourced from `medulla_capabilities::policy`) is the assertable, tested FACT that this
//! delegation actually happens for the methods policy says should be audited (see
//! `tests::routed_mutation_produces_one_durable_record_one_audit_entry_and_one_cdc_event`,
//! which uses a REAL `RedbBackend` and reads the audit chain back).
//!
//! The per-graph write-coalescer's batching is genuinely live for the routed set
//! (L18/MEDULLA-P0-6, rewritten): the five coalescable structural writes
//! (`AddNode`/`RemoveNode`/`AddEdge`/`RemoveEdge`/`CompareAndSetNodeFields`) are routed through
//! [`commit_coalescable_mutation`] instead of [`commit_mutation`] (see
//! `medulla_h_graph::graph_ops::try_handle_gateway`'s dedicated arms). That function
//! hands the WHOLE prepare→durable-commit→RAM-publish sequence — not just the RAM
//! apply — to the per-graph `medulla_server::routed_write_coalescer` worker, which runs a
//! flushed batch's sequences back-to-back inside ONE `mutation_batch::lock_graph`
//! acquisition instead of one per op (`stats().batches() < stats().ops()` under
//! concurrent load). [`commit_mutation_body`] is the single, shared implementation
//! of that sequence — called once per op by both the ordinary single-call path
//! ([`commit_entry::commit_mutation_inner`]) and the worker, so there is exactly one copy of the
//! durability/audit/CDC kernel regardless of which lock-hold granularity wraps it.
//! The non-coalescable routed memory ops (`CreateSummaryNode`/`Consolidate`/
//! `Reinforce`) and every other routed method keep going through
//! [`commit_mutation`]/[`commit_entry::commit_mutation_inner`] unchanged, one lock acquisition per
//! op. With Raft active, dispatch reaches the consensus barrier before this local
//! gateway; each committed ordinary Raft method is then staged and committed through
//! the same state-backed MutationBatch authority on every replica.
//!
//! See [`commit_coalescable_mutation`]'s doc for exactly why batching the RAM
//! publish ALONE (an earlier version of this fix) is unsafe, and why the durable
//! commit must move into the SAME lock-held sequence rather than staying outside it.

#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
use medulla_graph_core::graph::GraphCore;
#[cfg(test)]
use medulla_isolation::isolation::IsolationLayer;
#[cfg(test)]
use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::ResultPayload;

mod coalescer;
mod commit_encode;
mod commit_entry;
mod commit_paths;
mod commit_replay;
mod conditional;
mod context;
pub mod plan;

use plan::consensus_apply_is_authorized;
#[cfg(test)]
pub use plan::GATEWAY_ROUTED;
#[cfg(feature = "raft")]
pub use plan::LOCAL_ONLY_CLUSTER_REFUSAL;
#[cfg(any(feature = "raft", test, feature = "test-support"))]
pub use plan::{cluster_mutation_route, ClusterMutationRoute};
pub use plan::{is_gateway_routed, method_variant_name, GatewayAuthzCtx, MutationPlan};

pub use context::MutationCtx;
use context::{advance_authoritative_manifest, idempotency_store, response_dedup_key};
pub use context::{durable_receipt_method, LifecycleAttempt};

pub use coalescer::commit_coalescable_mutation;
pub use coalescer::{apply_coalescable_write, is_coalescable_structural_write};

pub use commit_entry::commit_mutation;
use commit_entry::{
    commit_finalize, commit_mutation_body, commit_prepare, CommitFinalizeOptions, CommitPrep,
};

pub(super) use commit_encode::publish_committed_row_delta;
use commit_encode::{
    commit_mutation_body_commit_staged, commit_staged_with_receipt,
    diff_and_serialize_staged_mutation, prepublish_success, preserves_node_derived_indexes,
    StagedMutation, StagedReceipt,
};
use commit_paths::{
    commit_mutation_body_prepublish_fast_path, commit_mutation_body_staged_path,
    resolve_authoritative_base_snapshot,
};
use commit_replay::{
    commit_mutation_body_replay_response, commit_row_replay_probe, commit_staged_replay_probe,
    compile_batch_and_encode_result, compile_batch_context, DurableBatchAttempt,
    DurableBatchTarget,
};

// Each conditional-commit entry point is re-exported exactly where its callers
// are compiled: the Mine*/GraphLearn*/ML gateways (whose tests are gated on
// `mining`), and the query (SQL/Cypher/GraphQL, incl. Bolt) and RDF gateways.
#[cfg(any(
    feature = "mining",
    feature = "graphlearn",
    feature = "ml-pipeline",
    feature = "modality-serving"
))]
pub use conditional::commit_conditional_mutation;
#[cfg(any(
    feature = "query",
    feature = "cypher",
    feature = "graphql",
    feature = "rdf"
))]
pub use conditional::commit_conditional_mutation_async;
#[cfg(any(feature = "query", feature = "cypher", feature = "graphql"))]
pub use conditional::is_query_native_coordinator;
pub use conditional::{is_query_gateway_method, is_rdf_gateway_method};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validated_public_batch_has_a_deterministic_prepublish_result() {
        let core = GraphCore::new();
        let operations = rmp_serde::to_vec_named(&serde_json::json!([
            {"op": "add_node", "id": "a", "properties": {"text": "alpha"}},
            {"op": "add_embedding", "id": "a", "embedding": [1.0, 0.0]}
        ]))
        .unwrap();
        let method = Method::BatchUpdate {
            operations_msgpack: operations,
        };
        let Some(ResultPayload::Json(result)) = prepublish_success(&core, &method) else {
            panic!("validated BatchUpdate must use compact authoritative rows");
        };
        assert_eq!(result["added_nodes"], 1);
        assert_eq!(result["added_embeddings"], 1);

        let malformed = Method::BatchUpdate {
            operations_msgpack: vec![0xc1],
        };
        assert!(prepublish_success(&core, &malformed).is_none());
    }

    /// (b) An unauthorized actor is rejected AT THE GATEWAY, before `apply` ever
    /// runs (proven by asserting the graph is left untouched).
    #[tokio::test(flavor = "multi_thread")]
    async fn unauthorized_actor_is_rejected_at_the_gateway() {
        let core = Arc::new(GraphCore::new());
        let mut isolation = IsolationLayer::new();
        // Registering ANY identity flips `has_rules()` on, switching graph-
        // targeted dispatch into enforcing mode.
        isolation.register_agent(medulla_types::acl::AgentIdentity {
            agent_id: "owner".to_string(),
            role: medulla_types::acl::AgentRole::Agent,
            teams: Vec::new(),
            roles: Vec::new(),
        });
        isolation.register_agent(medulla_types::acl::AgentIdentity {
            agent_id: "intruder".to_string(),
            role: medulla_types::acl::AgentRole::Agent,
            teams: Vec::new(),
            roles: Vec::new(),
        });

        let method = Method::AddNode {
            node_id: "n1".to_string(),
            properties_msgpack: Vec::new(),
        };
        let plan = MutationPlan::for_method(&method);

        let ctx = MutationCtx {
            anonymous: false,
            req_id: 3,
            caller: Some("intruder"),
            attempt_nonce: None,
            idempotency_key: "test-idempotency",
            tenant_scope: "opaque-test-tenant",
            graph_name: "agent:owner",
            graph_type: medulla_protocol_types::protocol::GraphType::Agent,
            owner: Some("owner"),
            isolation: &isolation,
            core: &core,
            persistence: None,
            #[cfg(feature = "streaming")]
            cdc: None,
            materialization_manifest: None,
            write_coalescer: None,
        };

        let mut apply_ran = false;
        let resp = commit_mutation(&ctx, &plan, &method, |core| {
            apply_ran = true;
            core.add_node("n1".to_string(), Vec::new());
            Ok(ResultPayload::String("ok".to_string()))
        })
        .await;

        assert!(
            resp.error.is_some(),
            "expected ACCESS_DENIED, got {:?}",
            resp
        );
        assert!(resp.error.unwrap().contains("ACCESS_DENIED"));
        assert!(!apply_ran, "apply must never run for a denied caller");
        assert!(!core.has_node("n1"), "the graph must be untouched");
    }

    /// (d) Bypass guard, part 1: `MutationPlan` never diverges from
    /// `medulla_capabilities::policy` for any [`GATEWAY_ROUTED`] method -- if someone
    /// edits `MutationPlan::for_method` to hardcode a field instead of reading it
    /// off `policy()`, this catches the divergence.
    #[test]
    fn routed_methods_plan_is_never_hardcoded_relative_to_policy() {
        let samples: Vec<Method> = vec![
            Method::AddNode {
                node_id: "x".into(),
                properties_msgpack: Vec::new(),
            },
            Method::CreateNodeIfAbsent {
                node_id: "x".into(),
                properties_msgpack: Vec::new(),
            },
            Method::RemoveNode {
                node_id: "x".into(),
            },
            Method::AddEdge {
                source_id: "a".into(),
                target_id: "b".into(),
                properties_msgpack: Vec::new(),
            },
            Method::RemoveEdge {
                source_id: "a".into(),
                target_id: "b".into(),
            },
            Method::CreateSummaryNode {
                level: 1,
                child_ids: vec!["a".into()],
                props_msgpack: Vec::new(),
            },
            Method::Consolidate {
                episodic_ids: vec!["a".into()],
                semantic_props_msgpack: Vec::new(),
            },
            Method::Reinforce {
                node_id: "x".into(),
                now_ms: 0,
                weight: 0.0,
            },
            // ── L11 rollout batch 2: graph-core family ──
            Method::CompareAndSetNodeFields {
                node_id: "x".into(),
                conditions_msgpack: Vec::new(),
                updates_msgpack: Vec::new(),
            },
            Method::ClaimNext {
                label: "x".into(),
                updates_msgpack: Vec::new(),
            },
            Method::DecayNode {
                node_id: "x".into(),
                now_ms: 0,
                half_life_ms: 1,
            },
            Method::DecayMemories {
                now_ms: 0,
                half_life_ms: 1,
                ids: vec!["x".into()],
            },
            Method::EvictBelow {
                ids: vec!["x".into()],
                threshold: 0.0,
                delete: false,
            },
            Method::Maintain {
                ids: vec!["x".into()],
                now_ms: 0,
                half_life_ms: 1,
                evict_threshold: 0.0,
                delete: false,
            },
            Method::AddSceneObject {
                pose_msgpack: Vec::new(),
                parent: None,
            },
            Method::SetPose {
                node_id: "x".into(),
                pose_msgpack: Vec::new(),
            },
            Method::Reparent {
                node_id: "x".into(),
                new_parent: None,
            },
            Method::StartTrajectory {
                props_msgpack: Vec::new(),
            },
            Method::AppendStep {
                traj_id: "x".into(),
                action_msgpack: Vec::new(),
                reward: 0.0,
                state_ref: None,
                next_state_ref: None,
                t: 0,
            },
            Method::AddEmbedding {
                node_id: "x".into(),
                embedding: vec![0.0],
            },
            Method::InvalidateEdge {
                source_id: "a".into(),
                target_id: "b".into(),
                relationship: "r".into(),
                invalid_at: 0,
                tx_now: 0,
            },
            Method::SupersedeEdge {
                source_id: "a".into(),
                target_id: "b".into(),
                properties_msgpack: Vec::new(),
                prior_source: "a".into(),
                prior_target: "b".into(),
                prior_relationship: "r".into(),
                valid_at: 0,
                tx_now: 0,
            },
            Method::ClearGraph,
            Method::EvictLRU { max_nodes: 1 },
            Method::DecaySweep {
                half_life_secs: 1.0,
                floor: 0.0,
                prune: false,
            },
            Method::TouchNodes {
                node_ids: vec!["x".into()],
            },
            Method::FromMsgpack {
                msgpack: Vec::new(),
            },
            Method::Reconcile {
                graph_name: "g".into(),
                msgpack: Vec::new(),
            },
            Method::ApplyMutation {
                event_type: "e".into(),
                query: "q".into(),
            },
            #[cfg(feature = "shacl")]
            Method::IcvConfigure {
                graph: None,
                mode: "enforce".into(),
                shapes: "@prefix sh: <http://www.w3.org/ns/shacl#> .".into(),
            },
            #[cfg(feature = "reasoning")]
            Method::RunDatalogReasoning {
                subclass_relations: Vec::new(),
                subproperty_relations: Vec::new(),
                symmetric_properties: Vec::new(),
                transitive_properties: Vec::new(),
                inverse_properties: Vec::new(),
                domain_rules: Vec::new(),
                range_rules: Vec::new(),
                property_chains: Vec::new(),
            },
            Method::PruneByLifecycle {
                max_age_secs: 1,
                min_score: 0.0,
            },
            Method::BatchUpdate {
                operations_msgpack: Vec::new(),
            },
            Method::ClearLedger,
            Method::ApplyLedger {
                transactions: Vec::new(),
            },
            Method::CompactNodesByType {
                node_type: "x".into(),
                threshold: 1,
            },
            // ── L11 rollout batch 2: message-broker / stream family ──
            #[cfg(feature = "broker")]
            Method::DeclareExchange {
                exchange: "x".into(),
                kind: "direct".into(),
            },
            #[cfg(feature = "broker")]
            Method::DeleteExchange {
                exchange: "x".into(),
            },
            #[cfg(feature = "broker")]
            Method::BindQueue {
                exchange: "x".into(),
                queue: "q".into(),
                routing_key: "k".into(),
            },
            #[cfg(feature = "broker")]
            Method::UnbindQueue {
                exchange: "x".into(),
                queue: "q".into(),
                routing_key: "k".into(),
            },
            #[cfg(feature = "broker")]
            Method::Publish {
                exchange: "x".into(),
                routing_key: "k".into(),
                payload: Vec::new(),
            },
            #[cfg(feature = "broker")]
            Method::DeclareQueue {
                queue: "q".into(),
                dl_exchange: None,
                dl_routing_key: None,
                max_delivery_count: None,
                message_ttl_ms: None,
                queue_expiry_ms: None,
                max_priority: None,
            },
            #[cfg(feature = "broker")]
            Method::PublishEx {
                exchange: "x".into(),
                routing_key: "k".into(),
                payload: Vec::new(),
                priority: 0,
                delay_ms: None,
                ttl_ms: None,
                now_ms: None,
            },
            #[cfg(feature = "broker")]
            Method::BrokerConsume {
                queue: "q".into(),
                group: "g".into(),
                consumer: "c".into(),
                now_ms: 0,
                lease_ms: 0,
                prefetch: 0,
            },
            #[cfg(feature = "broker")]
            Method::BrokerAck {
                queue: "q".into(),
                node_id: "x".into(),
            },
            #[cfg(feature = "broker")]
            Method::BrokerReject {
                queue: "q".into(),
                node_id: "x".into(),
                requeue: false,
                now_ms: 0,
            },
            #[cfg(feature = "broker")]
            Method::SweepExpired { now_ms: 0 },
            #[cfg(feature = "broker")]
            Method::StreamDeclare {
                stream: "s".into(),
                max_messages: None,
                max_age_ms: None,
            },
            #[cfg(feature = "broker")]
            Method::StreamPublish {
                stream: "s".into(),
                payload: Vec::new(),
                now_ms: 0,
            },
            #[cfg(feature = "broker")]
            Method::StreamTrim {
                stream: "s".into(),
                now_ms: 0,
            },
            #[cfg(feature = "broker")]
            Method::StreamCommitOffset {
                stream: "s".into(),
                group: "g".into(),
                offset: 0,
            },
            #[cfg(feature = "broker")]
            Method::PublishConfirmed {
                exchange: "x".into(),
                routing_key: "k".into(),
                payload: Vec::new(),
                priority: 0,
                delay_ms: None,
                ttl_ms: None,
                now_ms: None,
            },
            #[cfg(feature = "broker")]
            Method::PublishIdempotent {
                exchange: "x".into(),
                routing_key: "k".into(),
                payload: Vec::new(),
                producer_id: None,
                seq: 0,
                priority: 0,
                delay_ms: None,
                ttl_ms: None,
                now_ms: None,
            },
            #[cfg(feature = "broker")]
            Method::BrokerAckTag {
                delivery_tag: 0,
                consumer: "c".into(),
            },
            #[cfg(feature = "broker")]
            Method::BrokerNackTag {
                delivery_tag: 0,
                consumer: "c".into(),
                requeue: false,
                now_ms: 0,
            },
            #[cfg(feature = "broker")]
            Method::BrokerRenewTag {
                delivery_tag: 0,
                consumer: "c".into(),
                now_ms: 0,
                lease_ms: 1,
            },
            // ── L11 rollout batch 3: runtime-conditional graph-learning family ──
            #[cfg(feature = "graphlearn")]
            Method::GraphLearnFit {
                source: medulla_protocol_types::protocol::GraphSource {
                    node_label: "x".into(),
                    direction: "any".into(),
                    relation: None,
                    limit: 0,
                },
                params: medulla_protocol_types::protocol::GraphLearnParams {
                    basis: "chebyshev".into(),
                    degree: 3,
                    hidden: 0,
                    epochs: 1,
                    lr: 0.1,
                    neg_ratio: 1.0,
                    seed: 0,
                    alpha: 0.5,
                },
                writeback: false,
            },
            #[cfg(feature = "graphlearn")]
            Method::GraphLearnPredict {
                model: serde_json::Value::Null,
                source: medulla_protocol_types::protocol::GraphSource {
                    node_label: "x".into(),
                    direction: "any".into(),
                    relation: None,
                    limit: 0,
                },
                candidate_pairs: Vec::new(),
                top_k: 10,
                writeback: false,
            },
            // ── L11 rollout batch 3: runtime-conditional data-mining family.
            // `MineAssociate` is a SPOT CHECK, not full coverage of all 17: every
            // Mine* variant's `policy()` arm (`crates/medulla-capabilities/src/lib.rs`)
            // is one of 4 groups that share the IDENTICAL `MethodPolicy` value, and
            // `MutationPlan::for_method`'s call site has no per-variant branching at
            // all (it is `medulla_capabilities::policy(method)` verbatim for every Method
            // in existence) -- so one sample exercises the same code path the other
            // 16 would. Full name-level coverage (every Mine* name really is a
            // mutating, policy-known method) is still asserted for ALL 17 by
            // `gateway_routed_set_matches_mutating_policy_surface` below, which
            // needs no `Method` construction (it reads
            // `medulla_capabilities::method_policy_entries()` by name). The runtime-conditional (`writeback`) gateway PATH itself is
            // proven end-to-end (WAL + audit, no CDC, Read-vs-Write ACL both ways)
            // by `mining_family_writeback_gates_durability_and_authz` below.
            #[cfg(feature = "mining")]
            Method::MineAssociate {
                transactions: vec![vec!["a".into(), "b".into()]],
                source: None,
                min_support: 0.1,
                min_confidence: 0.5,
                algorithm: medulla_protocol_types::protocol::MineAlgorithm::default(),
                writeback: false,
                #[cfg(feature = "epistemic")]
                as_claim: false,
            },
        ];
        // NOTE: this crate's default/test feature set always includes `broker` +
        // `reasoning` + `graphlearn` + `mining` (`default = ["graph", "algorithms",
        // "metrics", "full"]`), so every sample above is unconditionally constructed
        // in the build this test actually runs under; the `#[cfg]`s only keep the
        // file correct (it still compiles) for a slimmer, non-default feature
        // selection. `samples` is intentionally a SUBSET of `GATEWAY_ROUTED` now
        // (see the Mine* comment above) -- every sample must still be routed +
        // policy-consistent, but not every routed name needs a hand-built sample.
        assert!(samples.len() <= GATEWAY_ROUTED.len());
        for m in &samples {
            assert!(is_gateway_routed(m), "{m:?} must be in GATEWAY_ROUTED");
            let plan = MutationPlan::for_method(m);
            let p = medulla_capabilities::policy(m);
            assert_eq!(plan.mutates, p.mutates, "{}", plan.method_name);
            assert_eq!(
                plan.durability_domain, p.durability_domain,
                "{}",
                plan.method_name
            );
            assert_eq!(plan.authz_action, p.authz_action, "{}", plan.method_name);
            assert_eq!(plan.idempotent, p.idempotent, "{}", plan.method_name);
            assert_eq!(plan.audited, p.audited, "{}", plan.method_name);
            assert_eq!(plan.emits_cdc, p.emits_cdc, "{}", plan.method_name);
            assert_eq!(
                plan.txn_participation, p.txn_participation,
                "{}",
                plan.method_name
            );
        }
    }

    /// L11 rollout: every mutating method NOT in [`GATEWAY_ROUTED`] falls into
    /// EXACTLY one of two documented buckets -- there is no silent third category.
    ///
    /// **NON_GATEWAY_COORDINATED** -- does not fit `commit_mutation`'s
    /// `(ctx, plan, method, apply)` shape for a real, load-bearing architectural
    /// reason (a different commit protocol entirely, a non-graph-scoped store
    /// with no `GraphCore`/`graph_name` in scope, a cross-shard/cluster-wide op,
    /// or a registry-lifecycle op that runs before/across a single graph's core
    /// exists). Each entry cites the native MutationBatch, saga, translation, or
    /// explicitly non-authoritative staging mechanism that owns the operation.
    ///
    /// **OPEN_NOT_JUSTIFIED** -- reserved for a graph-scoped, structurally routable
    /// mutation that has no coordinator yet. It is intentionally empty and asserted
    /// empty below; adding such a method is therefore a visible test-gated regression.
    const NON_GATEWAY_COORDINATED: &[(&str, &str)] = &[
        // ── OCC/2PC multi-op transaction protocol (src/server/handlers/txn.rs) ──
        // `Commit` applies a DYNAMIC list of already-staged Methods through one or
        // more child MutationBatches, while a multi-graph commit routes through 2PC
        // (`CrossShardCoordinator`), and a cross-modal commit lands
        // graph+vector+blob+series in ONE redb `WriteTransaction`
        // (`commit_cross_modal_txn`) -- none of that is a single `(ctx, plan,
        // method, apply)` call the gateway models. The Txn* STAGE ops don't even
        // mutate durable state at stage time (only `Commit` does); they self-route
        // in `dispatch.rs` BEFORE `dispatch_graph_op` entirely (resolved from
        // `open_txns`, not `req.graph`).
        ("BeginTxn", "encrypted Raft-native OCC staging authority"),
        ("TxnAddNode", "replicated OCC staging; named Commit receipt and graph MutationBatch own publication authority"),
        ("TxnRemoveNode", "ephemeral OCC staging; named Commit receipt and graph MutationBatch own authority"),
        ("TxnAddEdge", "ephemeral OCC staging; named Commit receipt and graph MutationBatch own authority"),
        ("TxnRemoveEdge", "ephemeral OCC staging; named Commit receipt and graph MutationBatch own authority"),
        ("TxnCas", "ephemeral OCC staging; named Commit receipt and graph MutationBatch own authority"),
        ("TxnAddEmbedding", "ephemeral OCC staging; named Commit receipt and cross-modal MutationBatch own authority"),
        ("TxnBlobRef", "ephemeral OCC staging; named Commit receipt and cross-modal MutationBatch own authority"),
        ("TxnAddMeasurement", "ephemeral OCC staging; named Commit receipt and cross-modal MutationBatch own authority"),
        ("TxnAxiom", "ephemeral OCC staging; named Commit receipt and cross-modal MutationBatch own authority"),
        ("TxnConstruct", "ephemeral OCC staging; named Commit receipt and cross-modal MutationBatch own authority"),
        ("TxnPlanWriteback", "ephemeral OCC staging; named Commit receipt and cross-modal MutationBatch own authority"),
        ("TxnMaterializeBelief", "ephemeral OCC staging; named Commit receipt and cross-modal MutationBatch own authority"),
        ("Commit", "named control-plane receipt coordinates graph/cross-modal/2PC child MutationBatches"),
        ("Rollback", "replicated removal of encrypted OCC staging authority"),
        // ── RF-ADR-009 / RF-ADR-010: the decision and connector-pack surfaces ──
        // Each commits through the agent-library owner's own medulla-transaction
        // write, not a `(ctx, plan, method, apply)` graph mutation, and each
        // has process-local ordering only (see `plan::LOCAL_ONLY_METHODS`).
        ("ConnectorPack", "native MutationBatch in agent_library.redb: pack head, members, component revisions, body holders, import record, receipt and outbox in one WTX; local-only authority"),
        ("WriteBack", "native MutationBatch in agent_library.redb: immutable source change set plus append-only attempt and reconciliation receipts; local-only authority"),
        ("DecisionCommit", "native MutationBatch in agent_library.redb: one DecisionRecord component revision, receipt and outbox in one WTX after re-derivation and catalog compare-and-set; local-only authority"),
        ("DecisionFit", "native MutationBatch in agent_library.redb: the terminal decision job row and its draft head body in one control-owner WTX; local-only authority"),
        ("DecisionEval", "native MutationBatch in agent_library.redb: the terminal evaluation job row and its receipt in one control-owner WTX; local-only authority"),
        ("DecisionLog", "native MutationBatch in agent_library.redb: one verify-replayed statistical record or one outcome evaluation per control-owner WTX; local-only authority"),
        ("MutationOutbox", "owner-local outbox ledger rewind: bounded medulla-transaction transactions with a durable control cursor; local-only authority"),
        ("EdgeIndex", "tenant SQL owner maintenance writes: edge-index registration, generation parts and fenced drop, one WTX each; local-only authority"),
        ("ClaimWorkItem", "dedicated engine-native MutationBatch lease transition in mutation_batch.rs/redb_store.rs"),
        ("KgDelegate", "authenticated Agent Library admission lowers to the native WorkItem command-log transaction"),
        ("SubmitWorkItem", "dedicated engine-native atomic WorkItem command-log admission in mutation_batch.rs/redb_store.rs"),
        ("SubmitWorkItems", "dedicated engine-native bounded atomic WorkItem admission batch in mutation_batch.rs/redb_store.rs"),
        ("AcquireCapacity", "dedicated engine-native capacity-cell CAS/lease transaction in dispatch.rs/redb_store/capacity_lease.rs"),
        ("RenewCapacity", "dedicated engine-native fenced capacity renewal transaction in dispatch.rs/redb_store/capacity_lease.rs"),
        ("ReleaseCapacity", "dedicated engine-native fenced capacity release transaction in dispatch.rs/redb_store/capacity_lease.rs"),
        ("ReclaimExpiredCapacity", "dedicated engine-native bounded expiry reclaim transaction in dispatch.rs/redb_store/capacity_lease.rs"),
        ("UpdateCapacityCell", "dedicated engine-native controller epoch-CAS transaction in dispatch.rs/redb_store/capacity_lease.rs"),
        ("ThrottleCapacityCell", "EH-406: dedicated engine-native AIMD step on one cell's throttle in redb_store/capacity_lease/throttle.rs, one admitted group like UpdateCapacityCell"),
        ("RenewWorkItemLease", "dedicated engine-native MutationBatch lease-fence transition in mutation_batch.rs/redb_store.rs"),
        ("CommitWorkItemResult", "dedicated engine-native MutationBatch result/dependency transition in mutation_batch.rs/redb_store.rs"),
        ("CancelWorkItem", "dedicated engine-native MutationBatch pending-cancellation transition in mutation_batch.rs/redb_store.rs"),
        ("DeferWorkItem", "dedicated engine-native MutationBatch fenced deferral transition in mutation_batch.rs/redb_store.rs"),
        ("PublishCurrentPlan", "dedicated engine-native MutationBatch owner/fence/revision current-plan publication in store_native.rs"),
        ("PublishRunCheckpoint", "dedicated engine-native MutationBatch owner/fence/CAS checkpoint publication in store_native.rs"),
        ("CasWorkItemMetadata", "dedicated engine-native MutationBatch scheduling-metadata CAS transition in mutation_batch.rs/redb_store.rs (BUG-111)"),
        ("IssueHostStartGrant", "dedicated issuer-bound host-start record in protected ControlLease storage"),
        ("IssueControlLease", "dedicated engine-native MutationBatch control-lease issue (absence-guarded) in the WorkItem kernel, redb_store/work_item/control_lease.rs"),
        ("TransitionControlLease", "dedicated engine-native MutationBatch control-lease revision-CAS transition in the WorkItem kernel, redb_store/work_item/control_lease.rs"),
        ("PolicyEvolutionStore", "EH-346 engine-internal create-only content-addressed policy-evolution record row in the WorkItem kernel, redb_store/work_item/policy_record.rs"),
        ("GapUpsert", "dedicated engine-native MutationBatch canonical-Gap upsert with its native WorkItem admission in one transaction, redb_store/work_item/market/upsert.rs"),
        ("GapTransition", "dedicated engine-native MutationBatch Gap lifecycle revision-CAS transition in the WorkItem kernel, redb_store/work_item/market/lifecycle.rs"),
        ("GapSettle", "dedicated engine-native MutationBatch Gap evidence write-back from the stored WorkItem row, redb_store/work_item/market/lifecycle.rs"),
        ("WorkOfferPut", "dedicated engine-native MutationBatch work-offer version-CAS write on the Gap row, redb_store/work_item/market/lifecycle.rs"),
        ("RetireSealedRecord", "dedicated engine-native MutationBatch digest-checked sealed-record retirement (tombstone in place) in the WorkItem kernel, redb_store/work_item/sealed_record.rs"),
        ("ReserveWorkItemResources", "dedicated engine-native MutationBatch host-reservation transaction in mutation_batch.rs/redb_store.rs"),
        ("ReleaseWorkItemResources", "dedicated engine-native MutationBatch reservation-release transaction in mutation_batch.rs/redb_store.rs"),
        ("ReclaimWorkItemResources", "dedicated engine-native MutationBatch reservation-reclaim transaction in mutation_batch.rs/redb_store.rs"),
        ("UpdateResourceHost", "dedicated engine-native MutationBatch host-telemetry transaction in mutation_batch.rs/redb_store.rs"),
        // ── Non-graph-scoped stores: no GraphCore/graph_name in scope at all;
        // each self-routes in dispatch.rs BEFORE the per-graph chain, exactly
        // like Txn*, with its OWN dedicated redb file + commit path. ──
        ("BlobBegin", "native MutationBatch in blob.redb: durable upload cursor + status/fence/idempotency/outbox in one WTX"),
        ("BlobChunkPut", "native MutationBatch in blob.redb: chunk + upload cursor + coordinator metadata in one WTX"),
        ("BlobCommit", "native MutationBatch in blob.redb: manifest/cursor transition + coordinator metadata in one WTX"),
        ("BlobGc", "native MutationBatch in blob.redb: GC rows + exact result/coordinator metadata in one WTX"),
        ("BlobRef", "native MutationBatch in blob.redb: refcount + coordinator metadata in one WTX"),
        ("BlobUnref", "native MutationBatch in blob.redb: refcount + coordinator metadata in one WTX"),
        ("KvPut", "native MutationBatch in kv.redb: KV row + status/fence/idempotency/outbox in one WTX"),
        ("KvDelete", "native MutationBatch in kv.redb: KV row + status/fence/idempotency/outbox in one WTX"),
        ("KvCas", "native MutationBatch in kv.redb: CAS decision/row + exact result/coordinator metadata in one WTX"),
        ("TsAppend", "native MutationBatch in series.redb: series rows/projection + coordinator metadata in one WTX"),
        ("TsEvict", "self-routes via dispatch.rs's tsdb block to timeseries.rs, like TsAppend above; one series.redb WTX via SeriesStore::evict_before_scoped -- no medulla_transaction idempotency batch, because retention is content-idempotent (re-evicting an already-past cutoff is a safe no-op), unlike TsAppend"),
        ("TsDeleteSeries", "self-routes via dispatch.rs's tsdb block to timeseries.rs, like TsAppend above; one series.redb WTX via SeriesStore::delete_scoped -- no medulla_transaction idempotency batch, because deletion is content-idempotent (re-deleting an already-gone series is a safe no-op), unlike TsAppend"),
        ("TsDefineSeries", "native MutationBatch in series.redb, like TsAppend: the derived points plus the definition/checkpoint in the series metadata in one WTX (EH-524)"),
        #[cfg(feature = "jobs")]
        ("AnalyticsJob", "native MutationBatch in jobs.redb; asynchronous claim writeback uses a staged graph MutationBatch"),
        // `Statechart` self-routes in dispatch.rs BEFORE dispatch_graph_op (see the
        // `Method::Statechart` arm there and `handlers::statechart` module docs) --
        // it never reaches this gateway's `try_handle_gateway`/`commit_mutation` at
        // all. `medulla-statechart`'s `StatechartStore::instantiate`/`send_event` commit
        // through `medulla-transaction` (the SAME universal MutationBatch/OCC
        // primitive `medulla-jobs` uses for `AnalyticsJob`, per medulla-statechart/Cargo.toml)
        // against their OWN `statecharts.redb`, keyed by def_id/instance_id, not a
        // graph -- structurally identical to `AnalyticsJob` above, just gated
        // `statechart` instead of `jobs`. Note: this is `medulla-transaction` (a
        // generic per-store OCC/durable-commit primitive shared by jobs/kv/blob/
        // series/statecharts), NOT this module's `GATEWAY_ROUTED`/`commit_mutation`
        // -- the two are easily conflated by name but are different mechanisms.
        #[cfg(feature = "statechart")]
        ("Statechart", "native MutationBatch in statecharts.redb; instance define/instantiate/send_event commit status/version/fence/idempotency/outbox in one WTX via medulla-transaction, exactly like AnalyticsJob in jobs.redb"),
        // RF-020. `AgentLibrary` self-routes in router.rs's
        // `dispatch_agent_library_methods` BEFORE the per-graph chain and owns
        // its own `agent_library.redb`. `AgentLibraryStore` commits publish and
        // retire through `medulla-transaction`'s `MutationKernel` -- the same
        // per-store OCC primitive `AnalyticsJob` and `Statechart` use -- so the
        // revision row, kernel receipt and outbox event land in one WTX.
        ("AgentLibrary", "native MutationBatch in agent_library.redb: entry revision + head + kernel receipt/outbox in one WTX via medulla-transaction, exactly like AnalyticsJob in jobs.redb"),
        // RF-ADR-008. Published into the SAME `agent_library.redb` owner as
        // `AgentLibrary` -- an agent graph is a composition of agent entries,
        // not a separate entity family, so a second physical store would split
        // one authority in two (RF-RULING-004).
        ("AgentGraph", "native MutationBatch in agent_library.redb: graph revision + head + kernel receipt/outbox in one WTX via medulla-transaction, exactly like AgentLibrary alongside it"),
        ("AgentComponent", "native MutationBatch in agent_library.redb: component revision + head + kernel receipt/outbox in one WTX via medulla-transaction, alongside the agents and graphs that reference it"),
        ("AgentTemplate", "native MutationBatch in agent_library.redb: template revision + head + kernel receipt/outbox in one WTX via medulla-transaction, alongside the components, agents and graphs its base is assembled from"),
        // RF-019. Commits into the semantic index's OWN `OwnerLayout::SemanticIndex`
        // owner through `medulla-transaction`, never a graph shard: one WTX carries the
        // stage transition, its artifact, the successor intent, the generation
        // checkpoint and the outbox lease acknowledgement together, which is what
        // makes "a stage cannot complete before its predecessor" a durable property
        // rather than an ordering convention.
        ("SemanticIndex", "native MutationBatch in the OwnerLayout::SemanticIndex owner: binding/stage row + artifact + successor intent + lease ack in one WTX via medulla-transaction, exactly like AnalyticsJob in jobs.redb"),
        ("ImportSqliteFile", "native SQL-catalog MutationBatch: all imported tables + exact result/coordinator metadata in one WTX"),
        ("SqlSourceBatch", "native SQL owner MutationBatch: typed source rows, provider checkpoint, committed epoch, terminal result, replay and outbox in one WTX; LocalOnly authority refuses active Raft"),
        // ── Process-global registries on ServerState: opaque control-redb sagas,
        // no GraphCore/graph_name; dispatched directly in the top-level match. ──
        ("CreateChannel", "opaque prepared/committed session-control MutationBatch"),
        ("JoinChannel", "opaque prepared/committed session-control MutationBatch"),
        ("LeaveChannel", "opaque prepared/committed session-control MutationBatch"),
        ("CloseChannel", "opaque prepared/committed session-control MutationBatch"),
        ("SendMessage", "request-scoped opaque session-control saga; a committed retry never duplicates delivery"),
        ("RegisterIdentity", "native rbac.redb MutationBatch shares the RBAC snapshot WTX"),
        ("RbacAdmin", "native rbac.redb MutationBatch shares the RBAC snapshot WTX"),
        ("RbacElevation", "EH-404: native rbac.redb MutationBatch; the elevation ledger is part of the RBAC policy image and shares its snapshot WTX"),
        ("GovernedChange", "EH-560: native rbac.redb MutationBatch; the governed ledger is part of the RBAC policy image and shares its snapshot WTX"),
        ("Identity", "native rbac.redb MutationBatch; the identity store and its RBAC projection are part of the RBAC policy image and share its snapshot WTX"),
        ("AuditAppend", "self-admits a maintenance group directly against the named graph's own shard (shard::admit_maintenance + ShardWrite), writing the audit-chain entry and its request-id/op idempotency row in one WTX outside the generic (ctx, plan, method, apply) shape, exactly like EdgeIndex's tenant-owner maintenance writes above"),
        ("RegisterForeignSource", "opaque prepared/committed session-control MutationBatch"),
        ("RegisterUdf", "opaque prepared/committed session-control MutationBatch"),
        ("RegisterContinuousQuery", "opaque prepared/committed session-control MutationBatch"),
        ("DropContinuousQuery", "opaque prepared/committed session-control MutationBatch"),
        ("RegisterTrigger", "opaque prepared/committed session-control MutationBatch"),
        ("DropTrigger", "opaque prepared/committed session-control MutationBatch"),
        ("CepSubscribe", "opaque prepared/committed session-control MutationBatch"),
        ("CepUnsubscribe", "opaque prepared/committed session-control MutationBatch"),
        // ── Cluster-wide / cross-shard admin ops: operate across the WHOLE
        // registry/cluster under a durable control-plane saga
        // (medulla_h_admin::admin::try_handle / medulla_h_txn::dist_compute::try_handle), not
        // a single resolved graph's core. ──
        ("Reshard", "prepared/committed admin MutationBatch saga around cluster-wide resharding"),
        ("CatalogAssign", "prepared/committed admin MutationBatch saga around the durable tenant catalog"),
        ("CatalogReassign", "prepared/committed admin MutationBatch saga around the durable tenant catalog"),
        ("CatalogRemove", "prepared/committed admin MutationBatch saga around the durable tenant catalog"),
        ("RebalanceExecute", "prepared/committed admin MutationBatch saga around cluster-wide rebalance"),
        ("Restore", "prepared/committed admin MutationBatch saga around online restore/PITR"),
        ("RaftAddLearner", "leader-only openraft add_learner via medulla_h_admin::raft_admin::try_handle against MultiRaft directly -- no GraphCore/graph_name in scope, cluster-wide like Reshard/CatalogAssign above"),
        ("RaftChangeMembership", "leader-only openraft change_membership via medulla_h_admin::raft_admin::try_handle against MultiRaft directly -- no GraphCore/graph_name in scope, cluster-wide like Reshard/CatalogAssign above"),
        // W2.5 fleet server registry: self-translates into `Method::AddNode` against
        // `__commons__` from its own top-level dispatch.rs arm (see the
        // `Method::RegisterServer` match), exactly like `ApplyMultisigMutation` above
        // translates into `Method::ApplyMutation` -- by the time a mutation happens the
        // method value has already become `AddNode`, so `RegisterServer` itself never
        // reaches `commit_mutation` directly. The durable write / audit line / CDC event
        // are AddNode's (already gateway-routed); `RegisterServer`'s OWN `audit_line`/
        // `emit_for_method` marker arms (mirroring `ApplyMultisigMutation`'s) are
        // defense-in-depth only, unreachable via this single-node delegation path.
        ("RegisterServer", "validates + computes the lease fields then TRANSLATES into a Method::AddNode dispatched through the ordinary dispatch_graph_op path against __commons__ (which IS gateway-routed) -- see dispatch.rs; by the time a mutation happens the method value has already become AddNode, so this variant itself never reaches commit_mutation directly"),
        ("FleetCatalog", "EH-345: each write op TRANSLATES into exactly one Method::CreateNodeIfAbsent or Method::CompareAndSetNodeFields against __commons__ through the ordinary dispatch_graph_op path (which IS gateway-routed), exactly like RegisterServer above; list/lookup are reads"),
        ("PolicyEvolution", "EH-346/EH-347: each write op TRANSLATES into exactly one Method::CreateNodeIfAbsent of an immutable content-addressed record in the request graph through the ordinary dispatch_graph_op path (which IS gateway-routed), exactly like FleetCatalog above; get is a read"),
        ("TelemetryDerive", "EH-408/EH-409: reads stored telemetry and the request graph's declared individuals, then TRANSLATES every derived fact into exactly one Method::BatchUpdate against the request graph through the ordinary dispatch_graph_op path (which IS gateway-routed), exactly like FleetCatalog above"),
        ("PlacementAdmin", "raft-replicated placement-catalog admin op (Assign/Move/AbortMove); MultiRaft::placement_assign / TenantManager::move_partition / abort_move commit through the DEFAULT group's own client_write / commit_placement to the __placement_catalog__ control graph, not this gateway's per-graph MutationBatch"),
        ("CreateMatView", "prepared/committed control-plane MutationBatch saga around the durable cross-shard view row"),
        ("RefreshMatView", "prepared/committed control-plane MutationBatch saga around the durable cross-shard view row"),
        ("PlanMatViewDefine", "prepared/committed control-plane MutationBatch saga around the durable plan definition"),
        ("PlanMatViewRefresh", "prepared/committed control-plane MutationBatch saga around derived-cache refresh"),
        ("PlanMatViewDrop", "prepared/committed control-plane MutationBatch saga around durable definition removal"),
        // ── Registry-lifecycle ops: run BEFORE/ACROSS a single graph's core
        // exists (or spans many), so there is no one EXISTING graph to route
        // "through". ──
        ("CreateGraph", "native lifecycle MutationBatch commits graph identity before registry publication"),
        ("DeleteGraph", "native lifecycle MutationBatch commits purge before registry eviction"),
        ("MultiGraphBatchUpdate", "cluster placement fanout emits one typed graph command per child; standalone mode uses a durable parent saga"),
        ("ApplyChangeEnvelope", "governed envelope coordinator commits typed graph/object/provenance rows, cursor, version, and outbox through one native MutationBatch"),
        ("ApplyChangeEnvelopes", "batch envelope coordinator groups envelopes by graph and commits each graph's page as one coalesced native MutationBatch transaction; fans out per graph like MultiGraphBatchUpdate"),
        ("ApplyChangeEnvelopeDraft", "client draft compiled at the request boundary into ApplyChangeEnvelope under the verified context, then committed by that same native coordinator"),
        ("ApplyChangeEnvelopeDrafts", "client draft batch compiled at the request boundary into ApplyChangeEnvelopes under the verified context, then committed by that same native coordinator"),
        ("SourceIngest", "RF-ADR-009 stages raw CAS and an authoritative Connector Manifest mapping, then delegates its sole canonical graph/provenance/cursor commit to ApplyChangeEnvelope"),
        ("IndexRepository", "EH-280 runtime-conditional: an unscoped batch is a stateless parse; a scoped batch lowers its :Blob/:FileVersion/:Branch projection and delegates its sole graph commit to ApplyChangeEnvelope, exactly like SourceIngest"),
        ("EnrichDocument", "MEDULLA-UNIFIED-DATA-PLANE-R651.7 lowers chunk nodes, chunkOf edges, vectors and one enrichment record of a held Document and delegates its sole graph commit to ApplyChangeEnvelope, exactly like IndexRepository"),
        ("RecomputeMaterialization", "fenced reasoning-projection coordinator resolves authoritative graph provenance and fsyncs its projection watermark"),
        // ── Server lifecycle: TxnParticipation::None, not a graph mutation. ──
        ("Shutdown", "server-lifecycle control-plane action, not a graph mutation"),
        // ── Translates away before it could ever reach the gateway. ──
        (
            "ApplyMultisigMutation",
            "validates the multisig threshold then TRANSLATES into a Method::ApplyMutation \
             dispatched through the ordinary dispatch_graph_op path (which IS gateway-routed) -- \
             see dispatch.rs; by the time a mutation happens the method value has already become \
             ApplyMutation, so this variant itself never reaches commit_mutation directly",
        ),
        // ── Native WorkItem claim capabilities: dedicated private ledger, dispatch.rs's
        // own block just above the reservation-read guard ("Native WorkItem claim
        // capabilities use a dedicated private ledger and never enter MutationBatch/
        // result/outbox/CDC projections"). See access.rs's REASON_NATIVE_CAPABILITY_LEDGER
        // for the read-side twin (VerifyWorkItemClaimCapability). ──
        (
            "MintWorkItemClaimCapability",
            "dispatch.rs's dedicated WorkItem-claim-capability block routes this to \
             redb_store::work_item_capability::mint_work_item_claim_capability against its own \
             private ledger, keyed by an AuthenticatedAuthority derived from the verified request \
             context -- never enters MutationBatch/result/outbox/CDC projections, so it does not \
             fit commit_mutation's (ctx, plan, method, apply) shape at all",
        ),
        // ── DevelopmentLane*'s 6 write methods now route through the explicit
        // `medulla_h_jobs::development_lane::try_handle` owner, which commits through
        // `PersistenceBackend::commit_development_lane` -> the redb writer thread ->
        // `redb_store::development_lane::commit_development_lane` -- a self-contained
        // begin_write()/commit() against the native `development_lane_*` tables. Same posture
        // as `MintWorkItemClaimCapability` above: no MutationBatch/result/outbox/CDC
        // projection, so it does not fit commit_mutation's (ctx, plan, method, apply) shape.
        // Formerly NOT_YET_AUDITED-adjacent placeholder text (push/medulla-merge-artifacts, commit
        // 174c381) said "no dispatch.rs wire-routing arm exists yet" -- that arm now exists;
        // see access.rs's REASON_NATIVE_DEVELOPMENT_LANE_READ for the read-side twin
        // (DevelopmentLaneStatus/QueryDevelopmentLane). ──
        ("ReserveDevelopmentLane", "medulla_h_jobs::development_lane::try_handle routes this to PersistenceBackend::commit_development_lane -> redb_store::development_lane::commit_development_lane, a self-contained redb transaction against the native development_lane_* tables -- never enters MutationBatch/result/outbox/CDC projections, same posture as MintWorkItemClaimCapability above"),
        ("RenewDevelopmentLane", "medulla_h_jobs::development_lane::try_handle routes this to PersistenceBackend::commit_development_lane -> redb_store::development_lane::commit_development_lane, a self-contained redb transaction against the native development_lane_* tables -- never enters MutationBatch/result/outbox/CDC projections, same posture as MintWorkItemClaimCapability above"),
        ("ObserveDevelopmentLane", "medulla_h_jobs::development_lane::try_handle routes this to PersistenceBackend::commit_development_lane -> redb_store::development_lane::commit_development_lane, a self-contained redb transaction against the native development_lane_* tables -- never enters MutationBatch/result/outbox/CDC projections, same posture as MintWorkItemClaimCapability above"),
        ("FinishDevelopmentLane", "medulla_h_jobs::development_lane::try_handle routes this to PersistenceBackend::commit_development_lane -> redb_store::development_lane::commit_development_lane, a self-contained redb transaction against the native development_lane_* tables -- never enters MutationBatch/result/outbox/CDC projections, same posture as MintWorkItemClaimCapability above"),
        ("CleanupDevelopmentLane", "medulla_h_jobs::development_lane::try_handle routes this to PersistenceBackend::commit_development_lane -> redb_store::development_lane::commit_development_lane, a self-contained redb transaction against the native development_lane_* tables -- never enters MutationBatch/result/outbox/CDC projections, same posture as MintWorkItemClaimCapability above"),
        ("UpdateDevelopmentLaneQuota", "medulla_h_jobs::development_lane::try_handle routes this to PersistenceBackend::commit_development_lane -> redb_store::development_lane::commit_development_lane, a self-contained redb transaction against the native development_lane_* tables -- never enters MutationBatch/result/outbox/CDC projections, same posture as MintWorkItemClaimCapability above"),
    ];

    /// Graph-scoped methods requiring a coordinator outside this gateway. The set is
    /// **EMPTY**: all former entries now have an owning mutation kernel --
    ///   - `Sql`/`CypherQuery`/`GraphQl` are now routed via
    ///     `commit_conditional_mutation_async` at the query dispatch site (the async
    ///     twin built for exactly the `state`/`rls`-needing surfaces);
    ///   - `AddTriples`/`RemoveTriples`/`DropNamedGraph` are routed the same way at
    ///     the RDF dispatch site, with multi-valued literals included in the staged
    ///     authoritative image;
    ///   - `RunRules` was AUDITED to be genuinely read-only, so the fix was the
    ///     policy (`medulla_capabilities::policy(RunRules).mutates` : true -> false), not
    ///     a route -- it left the mutating surface entirely.
    ///
    /// Kept as an explicit (empty) const, and asserted empty below, so "nothing is
    /// silently deferred" stays a machine-checked invariant: any future mutating
    /// method that is routable-but-unrouted must be listed here (a hard, visible
    /// admission), never dropped into the untracked void.
    const OPEN_NOT_JUSTIFIED: &[(&str, &str)] = &[];

    /// (d) Bypass guard, part 2: the migration surface is machine-visible. Every
    /// name in [`GATEWAY_ROUTED`] really exists in `medulla_capabilities::method_policy_entries()`
    /// and really is `mutates == true` (catches a rename/typo silently un-routing
    /// a method); the COMPLEMENT (every other mutating method) must fall ENTIRELY
    /// into [`NON_GATEWAY_COORDINATED`] or [`OPEN_NOT_JUSTIFIED`] -- an undocumented name in
    /// the complement is a hard test failure (a silent skip), not a warning.
    #[test]
    fn gateway_routed_set_matches_mutating_policy_surface() {
        use std::collections::BTreeSet;

        let all_mutating: BTreeSet<&'static str> = medulla_capabilities::method_policy_entries()
            .filter(|(_, p, _)| p.mutates)
            .map(|(name, _, _)| name)
            .collect();

        for routed in GATEWAY_ROUTED {
            assert!(
                all_mutating.contains(routed),
                "GATEWAY_ROUTED name '{routed}' is not a mutating method in \
                 medulla_capabilities::method_policy_entries() (renamed/typo'd?)"
            );
        }

        let routed_set: BTreeSet<&'static str> = GATEWAY_ROUTED.iter().copied().collect();
        let not_yet_migrated: Vec<&'static str> =
            all_mutating.difference(&routed_set).copied().collect();

        let coordinated: std::collections::HashMap<&'static str, &'static str> =
            NON_GATEWAY_COORDINATED.iter().copied().collect();
        let open: std::collections::HashMap<&'static str, &'static str> =
            OPEN_NOT_JUSTIFIED.iter().copied().collect();

        // Every NON_GATEWAY_COORDINATED / OPEN_NOT_JUSTIFIED entry must be a real,
        // currently-non-routed, mutating method name -- catches a stale doc entry
        // (a name that got routed, renamed, or was never mutating) as loudly as an
        // undocumented one.
        for name in coordinated.keys().chain(open.keys()) {
            assert!(
                not_yet_migrated.contains(name),
                "'{name}' is documented in NON_GATEWAY_COORDINATED/OPEN_NOT_JUSTIFIED but is NOT in the \
                 current non-gateway set (already routed, renamed, or never a mutating method?) -- stale entry"
            );
        }

        let undocumented: Vec<&&'static str> = not_yet_migrated
            .iter()
            .filter(|name| !coordinated.contains_key(*name) && !open.contains_key(*name))
            .collect();
        assert!(
            undocumented.is_empty(),
            "UNDOCUMENTED non-gateway entries (silently deferred, not allowed): {undocumented:?} -- \
             add each to NON_GATEWAY_COORDINATED (native coordinator) or OPEN_NOT_JUSTIFIED (honest \
             remainder) in medulla_server::mutation::tests"
        );
        assert_eq!(
            not_yet_migrated.len(),
            coordinated.len() + open.len(),
            "NON_GATEWAY_COORDINATED + OPEN_NOT_JUSTIFIED must exactly partition the non-gateway set"
        );
        // L11 close-out invariant: the OPEN (routable-but-unrouted) bucket is EMPTY.
        // Every non-routed mutating method has a concrete native coordinator,
        // translation, or pre-commit staging protocol; none is a durability bypass.
        assert!(
            open.is_empty(),
            "OPEN_NOT_JUSTIFIED must be empty (all routable methods are owned); entries: {:?}",
            open.keys().collect::<Vec<_>>()
        );

        println!(
            "MEDULLA-P0-2/L11 gateway migration surface: {} routed / {} mutating total; \
             {} non-gateway = {} natively coordinated + \
             {} open-not-justified (MUST be 0)",
            routed_set.len(),
            all_mutating.len(),
            not_yet_migrated.len(),
            coordinated.len(),
            open.len(),
        );
    }

    /// `cluster_mutation_route` must classify each `SELF_ROUTED_ADMIN_METHODS`
    /// entry as `SelfRoutedAdmin`, never `ConsensusNative` -- the exact bug
    /// (`CLUSTER_MUTATION_UNAVAILABLE: no bounded native command exists`) that
    /// made `Method::RaftAddLearner`/`RaftChangeMembership` unreachable through
    /// dispatch even though `medulla_h_admin::raft_admin::try_handle` fully implements
    /// them.
    #[test]
    fn self_routed_admin_methods_bypass_the_native_mutation_router() {
        assert_eq!(
            cluster_mutation_route(&Method::RaftAddLearner {
                group: None,
                node_id: 2,
                addr: "127.0.0.1:1".to_string(),
            }),
            ClusterMutationRoute::SelfRoutedAdmin,
        );
        assert_eq!(
            cluster_mutation_route(&Method::RaftChangeMembership {
                group: None,
                voters: vec![1, 2],
            }),
            ClusterMutationRoute::SelfRoutedAdmin,
        );
    }

    #[test]
    fn clustered_mutations_are_consensus_typed() {
        assert_eq!(
            cluster_mutation_route(&Method::AddNode {
                node_id: "opaque-node".to_string(),
                properties_msgpack: Vec::new(),
            }),
            ClusterMutationRoute::ConsensusGraph,
        );
        assert_eq!(
            cluster_mutation_route(&Method::CreateGraph {
                graph_name: "opaque-graph".to_string(),
                graph_type: medulla_protocol_types::protocol::GraphType::Global,
            }),
            ClusterMutationRoute::ConsensusNative,
        );
        assert_eq!(
            cluster_mutation_route(&Method::ApplyMutation {
                event_type: "opaque-event".to_string(),
                query: "opaque-operation".to_string(),
            }),
            ClusterMutationRoute::ConsensusNative,
        );
        #[cfg(feature = "sparql-http")]
        assert_eq!(
            cluster_mutation_route(&Method::ApplyMutation {
                event_type: medulla_server_api::api::dispatch_support::SPARQL_HTTP_UPDATE_EVENT.to_string(),
                query: "CLEAR DEFAULT".to_string(),
            }),
            ClusterMutationRoute::ConsensusFanout,
        );
        assert_eq!(
            cluster_mutation_route(&Method::MultiGraphBatchUpdate {
                batches_msgpack: Vec::new(),
            }),
            ClusterMutationRoute::ConsensusFanout,
        );
        assert_eq!(
            cluster_mutation_route(&Method::Shutdown),
            ClusterMutationRoute::VolatileControl,
        );
        assert_eq!(
            cluster_mutation_route(&Method::PlacementRoute {
                request: medulla_result_types::epistemic_operations::PlacementRouteRequest {
                    schema_version:
                        medulla_result_types::epistemic_operations::PlacementRouteRequestSchemaVersion::V1,
                    tenant_ref: "opaque-tenant".to_string(),
                    partition_ref: "opaque-partition".to_string(),
                    client_epoch: 0,
                },
            }),
            ClusterMutationRoute::ReadOnly,
        );
    }

    /// MEDULLA-TYPED-PACKS-R012: `ConnectorPack` and its four agent-layer write
    /// methods -- the exact set named in the requirement, not the broader
    /// decision/outbox half of [`LOCAL_ONLY_METHODS`] -- are `LocalOnly` and
    /// refuse in clustered mode until a separately specified clustered writer
    /// exists. This is the method-inventory test the requirement calls for:
    /// it enumerates exactly these five by name and proves each one routes
    /// `LocalOnly`, the classification `propose_native_mutation` consults
    /// to refuse with `LOCAL_ONLY_CLUSTER_REFUSAL` before placement, proposal,
    /// saga or store -- the same classifier
    /// `source_batch_is_a_local_only_cluster_route` already relies on as its
    /// clustered-mode proof for `SqlSourceBatch`.
    // spec: MEDULLA-TYPED-PACKS-R012
    #[test]
    fn pack_and_agent_layer_write_methods_are_the_local_only_five() {
        // Only the mutating ops are LocalOnly: the capability policy is
        // runtime-conditional, so Status/Current reads classify ReadOnly.
        let ctx = medulla_agent_types::agent_fixtures::agent_library::mutation_context;
        let methods: [(&str, Method); 5] = [
            (
                "ConnectorPack",
                Method::ConnectorPack {
                    op: Box::new(medulla_catalog_types::connector_pack::ConnectorPackOp::Unbind {
                        request: medulla_catalog_types::connector_pack::ConnectorPackUnbindRequest {
                            context: ctx(),
                            connector: medulla_catalog_types::catalog_fixtures::contract_pack::connector(),
                        },
                    }),
                },
            ),
            (
                "AgentComponent",
                Method::AgentComponent {
                    op: medulla_agent_types::agent_component::AgentComponentOp::Retire {
                        request: medulla_agent_types::agent_component::AgentComponentRetireRequest {
                            context: ctx(),
                            component_id: "component-a".to_string(),
                        },
                    },
                },
            ),
            (
                "AgentGraph",
                Method::AgentGraph {
                    op: medulla_agent_types::agent_graph::AgentGraphOp::Retire {
                        request: medulla_agent_types::agent_graph::AgentGraphRetireRequest {
                            context: ctx(),
                            graph_id: "graph-a".to_string(),
                        },
                    },
                },
            ),
            (
                "AgentLibrary",
                Method::AgentLibrary {
                    op: medulla_agent_types::agent_library::AgentLibraryOp::Retire {
                        request: medulla_agent_types::agent_library::AgentLibraryRetireRequest {
                            context: ctx(),
                            agent_id: "agent-a".to_string(),
                        },
                    },
                },
            ),
            (
                "AgentTemplate",
                Method::AgentTemplate {
                    op: medulla_agent_types::agent_template::AgentTemplateOp::Retire {
                        request: medulla_agent_types::agent_template::AgentTemplateRetireRequest {
                            context: ctx(),
                            template_id: "template-a".to_string(),
                        },
                    },
                },
            ),
        ];
        for (name, method) in &methods {
            assert_eq!(
                cluster_mutation_route(method),
                ClusterMutationRoute::LocalOnly,
                "{name} must route LocalOnly"
            );
            assert!(
                plan::LOCAL_ONLY_METHODS.contains(name),
                "{name} must be named in LOCAL_ONLY_METHODS"
            );
        }
    }
}

/// Shared conversion tail for gateway closures that wrap a handler `Response`: a handler
/// `Response` carries its own refusal/result, so folding it into the
/// `FnOnce(&GraphCore) -> Result<ResultPayload, String>` shape the conditional commit
/// expects is identical work at every call site.
pub fn response_to_gateway_result(
    resp: medulla_protocol_types::protocol::Response,
) -> Result<ResultPayload, String> {
    // Keep the detail with the code: the gateway re-wraps this text through
    // `Response::err`, which would otherwise answer with a bare code.
    match resp.refusal_text() {
        Some(refusal) => Err(refusal),
        None => Ok(resp
            .result
            .unwrap_or(ResultPayload::Json(serde_json::Value::Null))),
    }
}
