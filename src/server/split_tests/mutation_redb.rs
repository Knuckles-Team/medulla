//! Mutation-gateway tests that drive the concrete redb backend and the mining handler.
//!
//! They sit in the top server crate because they need the concrete persistence backend and
//! handler crates that `medulla-server-api` cannot depend on (R056.8).

use std::sync::Arc;

use medulla_capabilities::DurabilityDomain;
use medulla_core::isolation::IsolationLayer;
use medulla_graph_core::graph::GraphCore;
use medulla_persistence::redb_backend::RedbBackend;
use medulla_protocol_types::protocol::{GraphType, Method, ResultPayload};
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::mutation::*;

/// W1c: `check_graph_access_with_policy` (`src/server/access.rs`, the L-RLS-1
/// hardening) now mandates a provisioned identity for EVERY graph-scoped
/// access -- an empty, rule-less `IsolationLayer` (no registered agents) is
/// unconditionally denied, not just under-ruled. Every test below needs a
/// caller that actually clears that gate, so they provision "system-agent"
/// with `AgentRole::System` (the one role `IsolationLayer::check_access`
/// treats as unconditionally allowed, same mechanism
/// `unauthorized_actor_is_rejected_at_the_gateway` already relies on for its
/// registered identities) instead of an insufficient no-rules layer.
fn isolation_with_system_agent() -> IsolationLayer {
    let mut isolation = IsolationLayer::new();
    isolation.register_agent(medulla_types::acl::AgentIdentity {
        agent_id: "system-agent".to_string(),
        role: medulla_types::acl::AgentRole::System,
        teams: Vec::new(),
        roles: Vec::new(),
    });
    isolation
}

/// Common authenticated carrier for durable replay tests. Each attempt
/// supplies its own request ID, nonce, and stable idempotency key below.
#[cfg(feature = "redb")]
fn durable_replay_context<'a>(
    graph_name: &'a str,
    core: &'a Arc<GraphCore>,
    isolation: &'a IsolationLayer,
    persistence: &'a Arc<dyn PersistenceBackend>,
) -> MutationCtx<'a> {
    MutationCtx {
        anonymous: false,
        req_id: 0,
        caller: Some("system-agent"),
        attempt_nonce: None,
        idempotency_key: "",
        tenant_scope: "opaque-test-tenant",
        graph_name,
        graph_type: GraphType::Commons,
        owner: None,
        isolation,
        core,
        persistence: Some(persistence),
        #[cfg(feature = "streaming")]
        cdc: None,
        materialization_manifest: None,
        write_coalescer: None,
    }
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    medulla_kernel::kernel::persistence::temp_dir("medulla-mutation-gateway-test", tag)
}

/// (a) A GATEWAY_ROUTED, audited+CDC-emitting mutation (`AddNode`) produces a
/// durable record + an audit-chain entry + a CDC event -- all from the one
/// `commit_mutation` call, against a REAL `RedbBackend` (not a mock), so this
/// is exercising the actual durable-commit + audit-chain-append path, not just
/// asserting the gateway's own bookkeeping.
#[cfg(all(feature = "redb", feature = "security", feature = "streaming"))]
#[tokio::test(flavor = "multi_thread")]
async fn routed_mutation_produces_one_durable_record_one_audit_entry_and_one_cdc_event() {
    // Reads the ambient encryption env at its durable open, so the env must hold
    // still for this whole body. READ guard: it excludes only a key MUTATOR, never
    // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = temp_dir("durable-audit-cdc");
    let dir_s = dir.to_string_lossy().to_string();
    let backend = RedbBackend::open(dir_s, 64).expect("open redb backend");
    let persistence: Arc<dyn PersistenceBackend> = Arc::new(backend);

    let core = Arc::new(GraphCore::new());
    let isolation = isolation_with_system_agent();
    let cdc_hub = Arc::new(medulla_server_api::cdc::CdcHub::new());
    let graph_name = "g-medulla-p0-2-a";

    let method = Method::AddNode {
        node_id: "n1".to_string(),
        properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({"v": 1})).unwrap(),
    };
    let plan = MutationPlan::for_method(&method);
    assert!(plan.mutates, "AddNode must be classified as a mutation");
    assert!(plan.audited, "AddNode is policy-audited");
    assert!(plan.emits_cdc, "AddNode policy-emits CDC");
    assert_eq!(plan.durability_domain, DurabilityDomain::GraphRedb);

    let ctx = MutationCtx {
        anonymous: false,
        req_id: 1,
        caller: Some("system-agent"),
        attempt_nonce: None,
        idempotency_key: "test-idempotency",
        tenant_scope: "opaque-test-tenant",
        graph_name,
        graph_type: GraphType::Commons,
        owner: None,
        isolation: &isolation,
        core: &core,
        persistence: Some(&persistence),
        cdc: Some(&cdc_hub),
        materialization_manifest: None,
        write_coalescer: None,
    };

    let (node_id, props) = match &method {
        Method::AddNode {
            node_id,
            properties_msgpack,
        } => (node_id.clone(), properties_msgpack.clone()),
        _ => unreachable!(),
    };
    let resp = commit_mutation(&ctx, &plan, &method, move |core| {
        core.add_node(node_id, props);
        Ok(ResultPayload::String("ok".to_string()))
    })
    .await;
    assert!(
        resp.error.is_none(),
        "commit_mutation failed: {:?}",
        resp.error
    );

    // The write actually landed durably.
    let fname = medulla_kernel::persist::sanitize(graph_name);
    let read_back = persistence
        .read_node(&fname, "n1")
        .await
        .expect("read back the durably-committed node");
    assert!(
        read_back.is_some(),
        "AddNode did not durably commit via the gateway"
    );

    // Audit: the tamper-evident chain grew by exactly one entry for this graph.
    let report = persistence
        .audit_verify_blocking(&fname)
        .expect("audit_verify_blocking");
    assert!(report.ok, "audit chain broke: {report:?}");
    assert_eq!(
        report.entries, 1,
        "exactly one audited mutation (AddNode) went through the gateway"
    );

    // CDC: one AddNode event was emitted into the hub's feed for this graph.
    let events = cdc_hub.read(graph_name, 0, 100).events;
    assert_eq!(
        events.len(),
        1,
        "expected exactly one CDC event, got {events:?}"
    );
}

/// Regression: `commit_finalize` must refresh the per-graph `medulla_
/// graph_nodes`/`medulla_graph_edges` gauges. Before the fix, that
/// refresh existed only in the legacy non-gateway dispatch tail and Raft
/// snapshot-install -- NOT in `commit_finalize`, the universal gateway
/// essentially all current writes (including `AddNode`, GATEWAY_ROUTED) route
/// through -- so a graph mutated exclusively via gateway-routed writes never
/// refreshed its gauge and silently froze at its last snapshot-install value
/// (confirmed live: `/metrics` reported 56,882 nodes while a live `NodeCount`
/// RPC, a Cypher `count(n)`, and full label enumeration all independently
/// agreed on 25,122). This drives two REAL `AddNode` calls through
/// `commit_mutation` (never touching `set_graph_size` directly) and asserts
/// the rendered gauge tracks the actual count after each one.
#[cfg(all(feature = "redb", feature = "security", feature = "streaming"))]
#[tokio::test(flavor = "multi_thread")]
async fn commit_finalize_refreshes_the_graph_size_gauges() {
    // Reads the ambient encryption env at its durable open, so the env must hold
    // still for this whole body. READ guard: it excludes only a key MUTATOR, never
    // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = temp_dir("gauge-refresh");
    let dir_s = dir.to_string_lossy().to_string();
    let backend = RedbBackend::open(dir_s, 64).expect("open redb backend");
    let persistence: Arc<dyn PersistenceBackend> = Arc::new(backend);

    let core = Arc::new(GraphCore::new());
    let isolation = isolation_with_system_agent();
    let graph_name = "g-medulla-p0-2-gauge-refresh";

    fn gauge_value(metric: &str, graph_name: &str) -> Option<i64> {
        let rendered = medulla_kernel::metrics::render();
        let needle = format!("{metric}{{graph=\"{graph_name}\"}} ");
        rendered.lines().find_map(|line| {
            line.strip_prefix(&needle)
                .and_then(|rest| rest.trim().parse::<i64>().ok())
        })
    }

    async fn add_node(
        core: &Arc<GraphCore>,
        isolation: &IsolationLayer,
        persistence: &Arc<dyn PersistenceBackend>,
        graph_name: &'static str,
        node_id: &str,
    ) {
        let method = Method::AddNode {
            node_id: node_id.to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({"v": 1})).unwrap(),
        };
        let plan = MutationPlan::for_method(&method);
        let ctx = MutationCtx {
            anonymous: false,
            req_id: 1,
            caller: Some("system-agent"),
            attempt_nonce: None,
            // Each AddNode is a distinct caller operation. Reusing a stable
            // key for n1 and n2 correctly produces a kernel conflict.
            idempotency_key: node_id,
            tenant_scope: "opaque-test-tenant",
            graph_name,
            graph_type: GraphType::Commons,
            owner: None,
            isolation,
            core,
            persistence: Some(persistence),
            cdc: None,
            materialization_manifest: None,
            write_coalescer: None,
        };
        let node_id_owned = node_id.to_string();
        let props = match &method {
            Method::AddNode {
                properties_msgpack, ..
            } => properties_msgpack.clone(),
            _ => unreachable!(),
        };
        let resp = commit_mutation(&ctx, &plan, &method, move |core| {
            core.add_node(node_id_owned, props);
            Ok(ResultPayload::String("ok".to_string()))
        })
        .await;
        assert!(resp.error.is_none(), "AddNode failed: {:?}", resp.error);
    }

    // Before any write, either unset (no series yet) or zero -- never stale.
    add_node(&core, &isolation, &persistence, graph_name, "n1").await;
    assert_eq!(
        gauge_value("medulla_graph_nodes", graph_name),
        Some(1),
        "gauge must reflect the ACTUAL count (1) immediately after the first \
         gateway-routed AddNode, not a frozen/absent value"
    );

    add_node(&core, &isolation, &persistence, graph_name, "n2").await;
    assert_eq!(
        gauge_value("medulla_graph_nodes", graph_name),
        Some(2),
        "a SECOND gateway-routed AddNode must advance the gauge again -- this \
         is exactly what stayed frozen before the fix (measured live: the \
         gauge was unchanged across two scrapes ten minutes apart despite \
         graph_ops_total advancing by ~30k)"
    );
    assert_eq!(core.node_count(), 2, "sanity: the graph really has 2 nodes");
}

/// (a2) A GATEWAY_ROUTED durable + audited BUT NON-CDC mutation (`Reinforce`)
/// commits durably, appends exactly ONE audit-chain entry, and leaves the CDC
/// feed UNTOUCHED. As of L3/MEDULLA-P0-6, `audit_line` is exhaustive over the full
/// durable-mutation surface, so `Reinforce` is now policy-`audited: true` (its
/// agent-memory write chains into the tamper-evident log); it stays
/// `emits_cdc: false`. This test now proves CDC -- NOT audit -- is the
/// policy-gated leg of the gateway: audit fires, CDC does not.
#[cfg(all(feature = "redb", feature = "security", feature = "streaming"))]
#[tokio::test(flavor = "multi_thread")]
async fn audited_mutation_writes_audit_but_cdc_stays_policy_gated() {
    // Reads the ambient encryption env at its durable open, so the env must hold
    // still for this whole body. READ guard: it excludes only a key MUTATOR, never
    // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = temp_dir("reinforce-audit-no-cdc");
    let dir_s = dir.to_string_lossy().to_string();
    let backend = RedbBackend::open(dir_s, 64).expect("open redb backend");
    let persistence: Arc<dyn PersistenceBackend> = Arc::new(backend);

    let core = Arc::new(GraphCore::new());
    core.add_node("n1".to_string(), Vec::new());
    let isolation = isolation_with_system_agent();
    let cdc_hub = Arc::new(medulla_server_api::cdc::CdcHub::new());
    let graph_name = "g-medulla-p0-2-a2";

    let method = Method::Reinforce {
        node_id: "n1".to_string(),
        now_ms: 1_000,
        weight: 1.0,
    };
    let plan = MutationPlan::for_method(&method);
    assert!(plan.mutates);
    assert!(
        plan.audited,
        "Reinforce is now policy-audited (L3/MEDULLA-P0-6)"
    );
    assert!(!plan.emits_cdc, "Reinforce policy-does-NOT-emit-CDC");
    assert_eq!(plan.durability_domain, DurabilityDomain::GraphRedb);

    let ctx = MutationCtx {
        anonymous: false,
        req_id: 2,
        caller: Some("system-agent"),
        attempt_nonce: None,
        idempotency_key: "test-idempotency",
        tenant_scope: "opaque-test-tenant",
        graph_name,
        graph_type: GraphType::Commons,
        owner: None,
        isolation: &isolation,
        core: &core,
        persistence: Some(&persistence),
        cdc: Some(&cdc_hub),
        materialization_manifest: None,
        write_coalescer: None,
    };
    let resp = commit_mutation(&ctx, &plan, &method, |core| {
        core.reinforce("n1", 1_000, 1.0);
        Ok(ResultPayload::Bool(true))
    })
    .await;
    assert!(resp.error.is_none());

    let fname = medulla_kernel::persist::sanitize(graph_name);
    let report = persistence
        .audit_verify_blocking(&fname)
        .expect("audit_verify_blocking");
    assert!(report.ok, "audit chain broke: {report:?}");
    assert_eq!(
        report.entries, 1,
        "Reinforce is now audited -- exactly one audit-chain entry"
    );
    assert_eq!(
        cdc_hub.read(graph_name, 0, 100).events.len(),
        0,
        "Reinforce must NOT emit a CDC event (CDC stays the policy-gated leg)"
    );
}

/// (a2b) W1c: drive one `method` through the REAL `commit_mutation` gateway
/// against a fresh `RedbBackend`, asserting the policy is `audited: true` +
/// `emits_cdc: true` and that exactly one audit-chain entry and one CDC marker
/// event actually land. Shared by every W1c admin/ledger-method case below (each
/// gets its own temp dir/backend/graph, so `report.entries`/the CDC feed start
/// clean per call).
#[cfg(all(feature = "redb", feature = "security", feature = "streaming"))]
async fn assert_w1c_method_audits_and_emits_one_cdc_marker<F>(tag: &str, method: Method, apply: F)
where
    F: FnOnce(&GraphCore) -> Result<ResultPayload, String>,
{
    let dir = temp_dir(&format!("w1c-{tag}"));
    let dir_s = dir.to_string_lossy().to_string();
    let backend = RedbBackend::open(dir_s, 64).expect("open redb backend");
    let persistence: Arc<dyn PersistenceBackend> = Arc::new(backend);
    let core = Arc::new(GraphCore::new());
    let isolation = isolation_with_system_agent();
    let cdc_hub = Arc::new(medulla_server_api::cdc::CdcHub::new());
    let graph_name = format!("g-w1c-{tag}");

    let plan = MutationPlan::for_method(&method);
    assert!(plan.mutates, "{tag}: must be classified as a mutation");
    assert!(plan.audited, "{tag}: W1c must now be policy-audited");
    assert!(plan.emits_cdc, "{tag}: W1c must now policy-emit CDC");
    assert_eq!(plan.durability_domain, DurabilityDomain::GraphRedb);

    let ctx = MutationCtx {
        anonymous: false,
        req_id: 1,
        caller: Some("system-agent"),
        attempt_nonce: None,
        idempotency_key: "test-idempotency",
        tenant_scope: "opaque-test-tenant",
        graph_name: &graph_name,
        graph_type: GraphType::Commons,
        owner: None,
        isolation: &isolation,
        core: &core,
        persistence: Some(&persistence),
        cdc: Some(&cdc_hub),
        materialization_manifest: None,
        write_coalescer: None,
    };
    let resp = commit_mutation(&ctx, &plan, &method, apply).await;
    assert!(
        resp.error.is_none(),
        "{tag}: commit_mutation failed: {:?}",
        resp.error
    );

    let fname = medulla_kernel::persist::sanitize(&graph_name);
    let report = persistence
        .audit_verify_blocking(&fname)
        .expect("audit_verify_blocking");
    assert!(report.ok, "{tag}: audit chain broke: {report:?}");
    assert_eq!(
        report.entries, 1,
        "{tag}: exactly one audit-chain entry must land"
    );

    let events = cdc_hub.read(&graph_name, 0, 100).events;
    assert_eq!(
        events.len(),
        1,
        "{tag}: exactly one CDC marker event, got {events:?}"
    );
}

/// (a2c) W1c: the 9 durable admin/ledger methods (`FromMsgpack`/`ClearLedger`/
/// `ApplyLedger`/`CompactNodesByType`/`RunDatalogReasoning`/`Reconcile`/
/// `ApplyMutation`/`ApplyMultisigMutation`/`IcvConfigure`) were GATEWAY_ROUTED
/// and GraphRedb-durable but `audited: false, emits_cdc: false` -- invisible to
/// both the tamper-evident audit chain and the CDC feed despite committing
/// durably. Each now audits + emits CDC, consistent with the rest of the
/// durable mutation surface (`audit::audit_line` / `cdc::emit_for_method`).
/// 7 of the 9 don't map onto a single node/edge row, so each emits ONE
/// reserved-marker `UpdateNode` CDC event (the same shape `ApplyChangeEnvelope`/
/// `ServedModality` already use); those 7 are exercised here.
#[cfg(all(feature = "redb", feature = "security", feature = "streaming"))]
#[tokio::test(flavor = "multi_thread")]
async fn w1c_marker_admin_ledger_methods_now_audit_and_emit_cdc() {
    // Reads the ambient encryption env at its durable open, so the env must hold
    // still for this whole body. READ guard: it excludes only a key MUTATOR, never
    // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    assert_w1c_method_audits_and_emits_one_cdc_marker(
        "clear-ledger",
        Method::ClearLedger,
        |core| {
            core.clear_ledger();
            Ok(ResultPayload::String("ok".to_string()))
        },
    )
    .await;

    assert_w1c_method_audits_and_emits_one_cdc_marker(
        "apply-ledger",
        Method::ApplyLedger {
            transactions: Vec::new(),
        },
        |core| {
            core.apply_ledger(Vec::new())
                .map(|()| ResultPayload::String("ok".to_string()))
        },
    )
    .await;

    assert_w1c_method_audits_and_emits_one_cdc_marker(
        "compact-nodes-by-type",
        Method::CompactNodesByType {
            node_type: "x".into(),
            threshold: 1,
        },
        |core| {
            let removed = core.compact_nodes_by_type("x", 1);
            Ok(ResultPayload::Json(
                serde_json::json!({ "removed_nodes": removed }),
            ))
        },
    )
    .await;

    assert_w1c_method_audits_and_emits_one_cdc_marker(
        "apply-mutation",
        Method::ApplyMutation {
            event_type: "w1c_test_event".into(),
            query: "SELECT 1".into(),
        },
        |_core| Ok(ResultPayload::String("ok".to_string())),
    )
    .await;

    assert_w1c_method_audits_and_emits_one_cdc_marker(
        "apply-multisig-mutation",
        Method::ApplyMultisigMutation {
            signatures: vec!["sig1".into(), "sig2".into()],
            threshold: 2,
            mutation_type: "policy_update".into(),
            query: "UPDATE policy SET x = 1".into(),
        },
        |_core| Ok(ResultPayload::Bool(true)),
    )
    .await;

    assert_w1c_method_audits_and_emits_one_cdc_marker(
        "icv-configure",
        Method::IcvConfigure {
            graph: None,
            mode: "enforce".into(),
            shapes: "@prefix sh: <http://www.w3.org/ns/shacl#> .".into(),
        },
        |_core| Ok(ResultPayload::Bool(true)),
    )
    .await;

    assert_w1c_method_audits_and_emits_one_cdc_marker(
        "run-datalog-reasoning",
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
        |_core| {
            Ok(ResultPayload::Json(serde_json::json!({
                "inferred_count": 0,
                "inferred_triples": Vec::<()>::new(),
            })))
        },
    )
    .await;
}

/// (a2d) W1c: `FromMsgpack`/`Reconcile` both replace the graph's ENTIRE
/// node/edge content with an imported/merged authoritative image
/// (`GraphCore::from_msgpack`) -- the same "whole graph replaced" shape as
/// `ClearGraph`, so their CDC signal is a feed RESET (`CdcHub::reset_graph`),
/// not a marker event. Proven by seeding one real CDC event first (a durable
/// `AddNode`) and observing the feed empty out afterward, while the audit
/// chain still grows by one entry for the `FromMsgpack`/`Reconcile` call
/// itself.
#[cfg(all(feature = "redb", feature = "security", feature = "streaming"))]
#[tokio::test(flavor = "multi_thread")]
async fn w1c_from_msgpack_and_reconcile_reset_the_cdc_feed_and_audit() {
    // Reads the ambient encryption env at its durable open, so the env must hold
    // still for this whole body. READ guard: it excludes only a key MUTATOR, never
    // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    for (tag, build_method) in [
        (
            "from-msgpack",
            (|msgpack: Vec<u8>| Method::FromMsgpack { msgpack }) as fn(Vec<u8>) -> Method,
        ),
        (
            "reconcile",
            (|msgpack: Vec<u8>| Method::Reconcile {
                graph_name: "other-graph".to_string(),
                msgpack,
            }) as fn(Vec<u8>) -> Method,
        ),
    ] {
        let dir = temp_dir(&format!("w1c-{tag}"));
        let dir_s = dir.to_string_lossy().to_string();
        let backend = RedbBackend::open(dir_s, 64).expect("open redb backend");
        let persistence: Arc<dyn PersistenceBackend> = Arc::new(backend);
        let core = Arc::new(GraphCore::new());
        let isolation = isolation_with_system_agent();
        let cdc_hub = Arc::new(medulla_server_api::cdc::CdcHub::new());
        let graph_name = format!("g-w1c-{tag}");

        let ctx = MutationCtx {
            anonymous: false,
            req_id: 1,
            caller: Some("system-agent"),
            attempt_nonce: None,
            idempotency_key: "w1c-replace",
            tenant_scope: "opaque-test-tenant",
            graph_name: &graph_name,
            graph_type: GraphType::Commons,
            owner: None,
            isolation: &isolation,
            core: &core,
            persistence: Some(&persistence),
            cdc: Some(&cdc_hub),
            materialization_manifest: None,
            write_coalescer: None,
        };

        // Seed one durable AddNode -- one audit entry, one CDC event.
        let seed = Method::AddNode {
            node_id: "seed".to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({"v": 1})).unwrap(),
        };
        let seed_plan = MutationPlan::for_method(&seed);
        let seed_ctx = MutationCtx {
            idempotency_key: "w1c-seed",
            ..ctx
        };
        let seed_resp = commit_mutation(&seed_ctx, &seed_plan, &seed, |core| {
            core.add_node("seed".to_string(), Vec::new());
            Ok(ResultPayload::String("ok".to_string()))
        })
        .await;
        assert!(seed_resp.error.is_none(), "{tag}: seed AddNode failed");
        assert_eq!(
            cdc_hub.read(&graph_name, 0, 100).events.len(),
            1,
            "{tag}: seed AddNode must emit exactly one CDC event"
        );

        // FromMsgpack/Reconcile: replace with a (valid, empty) authoritative
        // snapshot of the SAME core -- policy-audited + policy-CDC-emitting.
        let msgpack = core.to_msgpack().expect("to_msgpack");
        let method = build_method(msgpack.clone());
        let plan = MutationPlan::for_method(&method);
        assert!(plan.audited, "{tag}: W1c must now be policy-audited");
        assert!(plan.emits_cdc, "{tag}: W1c must now policy-emit CDC");

        let resp = commit_mutation(&ctx, &plan, &method, move |core| {
            core.from_msgpack(&msgpack)
                .map(|()| ResultPayload::String("ok".to_string()))
        })
        .await;
        assert!(
            resp.error.is_none(),
            "{tag}: commit_mutation failed: {:?}",
            resp.error
        );

        let fname = medulla_kernel::persist::sanitize(&graph_name);
        let report = persistence
            .audit_verify_blocking(&fname)
            .expect("audit_verify_blocking");
        assert!(report.ok, "{tag}: audit chain broke: {report:?}");
        assert_eq!(
            report.entries, 2,
            "{tag}: seed AddNode + {tag} == exactly two audit-chain entries"
        );

        assert_eq!(
            cdc_hub.read(&graph_name, 0, 100).events.len(),
            0,
            "{tag}: the CDC feed must be RESET (empty) after the whole-graph replace"
        );
    }
}

/// (a5) L11 rollout batch 2, family representative: a message-broker method
/// (`DeclareExchange`, `DurabilityDomain::Outbox`) routed through the SAME
/// `commit_mutation` gateway produces the policy-declared durable/audit effect
/// (audited, no CDC) against a REAL `RedbBackend` — proving the Outbox
/// durability domain commits through the identical `record_durable` path as
/// the GraphRedb-domain methods above (the backend does not branch on
/// `DurabilityDomain`, only on whether it's `None`).
#[cfg(all(
    feature = "redb",
    feature = "broker",
    feature = "security",
    feature = "streaming"
))]
#[tokio::test(flavor = "multi_thread")]
async fn broker_family_routed_mutation_is_audited_with_no_cdc() {
    // Reads the ambient encryption env at its durable open, so the env must hold
    // still for this whole body. READ guard: it excludes only a key MUTATOR, never
    // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = temp_dir("broker-declare-exchange");
    let dir_s = dir.to_string_lossy().to_string();
    let backend = RedbBackend::open(dir_s, 64).expect("open redb backend");
    let persistence: Arc<dyn PersistenceBackend> = Arc::new(backend);

    let core = Arc::new(GraphCore::new());
    let isolation = isolation_with_system_agent();
    let cdc_hub = Arc::new(medulla_server_api::cdc::CdcHub::new());
    let graph_name = "g-medulla-p0-2-l11-broker-a";

    let method = Method::DeclareExchange {
        exchange: "orders".to_string(),
        kind: "direct".to_string(),
    };
    let plan = MutationPlan::for_method(&method);
    assert!(plan.mutates);
    assert_eq!(plan.durability_domain, DurabilityDomain::Outbox);
    assert!(plan.audited, "DeclareExchange is policy-audited");
    assert!(!plan.emits_cdc, "DeclareExchange policy-does-NOT-emit-CDC");

    let ctx = MutationCtx {
        anonymous: false,
        req_id: 10,
        caller: Some("system-agent"),
        attempt_nonce: None,
        idempotency_key: "test-idempotency",
        tenant_scope: "opaque-test-tenant",
        graph_name,
        graph_type: GraphType::Commons,
        owner: None,
        isolation: &isolation,
        core: &core,
        persistence: Some(&persistence),
        cdc: Some(&cdc_hub),
        materialization_manifest: None,
        write_coalescer: None,
    };

    let resp = commit_mutation(&ctx, &plan, &method, move |core| {
        let Some(k) = medulla_core::broker::ExchangeKind::parse("direct") else {
            return Err("bad kind".to_string());
        };
        medulla_core::broker::declare_exchange(core, "orders", k)
            .map(|()| ResultPayload::String("ok".to_string()))
    })
    .await;
    assert!(
        resp.error.is_none(),
        "commit_mutation failed: {:?}",
        resp.error
    );

    let fname = medulla_kernel::persist::sanitize(graph_name);
    let report = persistence
        .audit_verify_blocking(&fname)
        .expect("audit_verify_blocking");
    assert!(report.ok, "audit chain broke: {report:?}");
    assert_eq!(
        report.entries, 1,
        "exactly one audited mutation (DeclareExchange) went through the gateway"
    );
    assert_eq!(
        cdc_hub.read(graph_name, 0, 100).events.len(),
        0,
        "DeclareExchange must NOT emit a CDC event (Outbox methods never do)"
    );
}

/// (a6) State-backed maintenance operations use the same authoritative
/// MutationBatch boundary as ordinary graph writes. `TouchNodes` remains
/// intentionally unaudited and CDC-silent, but its resulting graph image and
/// terminal result must survive restart.
#[cfg(all(feature = "redb", feature = "security"))]
#[tokio::test(flavor = "multi_thread")]
async fn touch_nodes_commits_authoritative_state_without_audit_or_cdc() {
    // Reads the ambient encryption env at its durable open, so the env must hold
    // still for this whole body. READ guard: it excludes only a key MUTATOR, never
    // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = temp_dir("touch-nodes-state-backed");
    let dir_s = dir.to_string_lossy().to_string();
    let backend = RedbBackend::open(dir_s, 64).expect("open redb backend");
    let persistence: Arc<dyn PersistenceBackend> = Arc::new(backend);

    let core = Arc::new(GraphCore::new());
    core.add_node(
        "n1".to_string(),
        rmp_serde::to_vec_named(&serde_json::json!({})).expect("encode seed node"),
    );
    let isolation = isolation_with_system_agent();
    let graph_name = "g-medulla-p0-2-l11-none-durability-a";

    let method = Method::TouchNodes {
        node_ids: vec!["n1".to_string()],
    };
    let plan = MutationPlan::for_method(&method);
    assert!(plan.mutates);
    assert_eq!(plan.durability_domain, DurabilityDomain::GraphRedb);
    assert!(!plan.audited);
    assert!(!plan.emits_cdc);

    let ctx = MutationCtx {
        anonymous: false,
        req_id: 11,
        caller: Some("system-agent"),
        attempt_nonce: None,
        idempotency_key: "test-idempotency",
        tenant_scope: "opaque-test-tenant",
        graph_name,
        graph_type: GraphType::Commons,
        owner: None,
        isolation: &isolation,
        core: &core,
        persistence: Some(&persistence),
        #[cfg(feature = "streaming")]
        cdc: None,
        materialization_manifest: None,
        write_coalescer: None,
    };

    let resp = commit_mutation(&ctx, &plan, &method, move |core| {
        let now = 1_000u64;
        let touched = core.touch_nodes(&["n1".to_string()], now);
        Ok(ResultPayload::Count(touched as u64))
    })
    .await;
    assert!(
        resp.error.is_none(),
        "commit_mutation failed: {:?}",
        resp.error
    );

    let fname = medulla_kernel::persist::sanitize(graph_name);
    let batch_id = medulla_server_api::mutation_batch::opaque_idempotency_key_for_context(
        "rpc",
        "opaque-test-tenant",
        graph_name,
        Some("system-agent"),
        "test-idempotency",
    );
    let record = persistence
        .read_mutation_batch(&fname, &batch_id)
        .await
        .expect("read mutation batch")
        .expect("TouchNodes batch is durable");
    assert_eq!(
        record.batch.identity.tenant().as_str(),
        medulla_storage::GRAPH_SHARD_TENANT
    );
    let (snapshot, version) = persistence
        .read_authoritative_graph_snapshot(&fname)
        .await
        .expect("read authoritative graph")
        .expect("TouchNodes graph image is durable");
    assert!(version > 0);
    let node = snapshot
        .nodes
        .iter()
        .find(|(id, _)| id == "n1")
        .expect("durable node");
    let properties: serde_json::Value =
        medulla_types::msgpack::decode_property_value(node.1.as_slice())
            .expect("decode durable node");
    assert_eq!(properties["last_access"], serde_json::json!(1_000));
    assert_eq!(properties["confidence"], serde_json::json!(1.0));

    let report = persistence
        .audit_verify_blocking(&fname)
        .expect("audit_verify_blocking");
    assert!(report.ok, "audit chain broke: {report:?}");
    assert_eq!(
        report.entries, 0,
        "TouchNodes is state-backed but intentionally unaudited"
    );
}

/// (a7) L11 rollout batch 3, RUNTIME-CONDITIONAL family representative:
/// `MineAssociate` proves `commit_conditional_mutation` resolves the
/// durability/audit/authz gateway from the request's OWN `writeback` field,
/// not from `policy()`'s conservative upper bound:
///   * `writeback: true`  -> Write access required, one audited durable entry.
///   * `writeback: false` -> Read access SUFFICES (an agent with only Read
///     succeeds), and NOTHING durable/audited is produced -- proving a
///     read-only call is never incorrectly gated behind Write or persisted as
///     a phantom mutation (the bug this function exists to prevent).
#[cfg(all(feature = "redb", feature = "mining", feature = "security"))]
#[tokio::test(flavor = "multi_thread")]
async fn mining_family_writeback_gates_durability_and_authz() {
    // Reads the ambient encryption env at its durable open, so the env must hold
    // still for this whole body. READ guard: it excludes only a key MUTATOR, never
    // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = temp_dir("mine-associate-conditional");
    let dir_s = dir.to_string_lossy().to_string();
    let backend = RedbBackend::open(dir_s, 64).expect("open redb backend");
    let persistence: Arc<dyn PersistenceBackend> = Arc::new(backend);

    let core = Arc::new(GraphCore::new());
    let graph_name = "g-medulla-p0-2-l11-mining-a";

    // This test's job is the DURABILITY/AUDIT half of the runtime-conditional
    // gateway (below). The Read-vs-Write ACL distinction is already covered
    // structurally by `commit_conditional_mutation`'s own code path (the
    // `mutates_now == false` branch calls `check_graph_access(.., AccessLevel::
    // Read)`; `true` goes through `commit_mutation`, which uses `AccessLevel::
    // Write`) and by `unauthorized_actor_is_rejected_at_the_gateway` proving the
    // Write gate itself denies an unauthorized caller.
    let isolation = isolation_with_system_agent();

    let transactions = vec![
        vec!["a".to_string(), "b".to_string()],
        vec!["a".to_string(), "b".to_string()],
        vec!["a".to_string(), "b".to_string()],
    ];

    // ── writeback: false -- a plain read: no durability, no audit ──
    let method_ro = Method::MineAssociate {
        transactions: transactions.clone(),
        source: None,
        min_support: 0.5,
        min_confidence: 0.5,
        algorithm: medulla_protocol_types::protocol::MineAlgorithm::default(),
        writeback: false,
        #[cfg(feature = "epistemic")]
        as_claim: false,
    };
    let plan_ro = MutationPlan::for_method(&method_ro);
    let ctx_ro = MutationCtx {
        anonymous: false,
        req_id: 20,
        caller: Some("system-agent"),
        attempt_nonce: None,
        idempotency_key: "test-idempotency",
        tenant_scope: "opaque-test-tenant",
        graph_name,
        graph_type: GraphType::Commons,
        owner: None,
        isolation: &isolation,
        core: &core,
        persistence: Some(&persistence),
        #[cfg(feature = "streaming")]
        cdc: None,
        materialization_manifest: None,
        write_coalescer: None,
    };
    let resp = commit_conditional_mutation(&ctx_ro, &plan_ro, &method_ro, false, |core| {
        let resp = crate::server::handlers::mining::handle_associate(
            20,
            core,
            crate::server::handlers::mining::AssociationRequest {
                transactions: transactions.clone(),
                source: None,
                min_support: 0.5,
                min_confidence: 0.5,
                algorithm: medulla_protocol_types::protocol::MineAlgorithm::default(),
                writeback: crate::server::handlers::mining::WritebackOptions {
                    enabled: false,
                    #[cfg(feature = "epistemic")]
                    as_claim: false,
                },
            },
        );
        match resp.error {
            Some(e) => Err(e),
            None => Ok(resp.result.unwrap()),
        }
    })
    .await;
    assert!(
        resp.error.is_none(),
        "read-only call failed: {:?}",
        resp.error
    );

    let fname = medulla_kernel::persist::sanitize(graph_name);
    let report = persistence
        .audit_verify_blocking(&fname)
        .expect("audit_verify_blocking");
    assert!(report.ok);
    assert_eq!(
        report.entries, 0,
        "writeback:false must produce NO durable/audit record"
    );

    // ── writeback: true -- a real mutation: durable + audited ──
    let method_rw = Method::MineAssociate {
        transactions: transactions.clone(),
        source: None,
        min_support: 0.5,
        min_confidence: 0.5,
        algorithm: medulla_protocol_types::protocol::MineAlgorithm::default(),
        writeback: true,
        #[cfg(feature = "epistemic")]
        as_claim: false,
    };
    let plan_rw = MutationPlan::for_method(&method_rw);
    assert!(plan_rw.audited, "MineAssociate is policy-audited");
    assert!(!plan_rw.emits_cdc, "MineAssociate policy-does-NOT-emit-CDC");
    let ctx_rw = MutationCtx {
        anonymous: false,
        req_id: 21,
        caller: Some("system-agent"),
        attempt_nonce: None,
        idempotency_key: "test-idempotency",
        tenant_scope: "opaque-test-tenant",
        graph_name,
        graph_type: GraphType::Commons,
        owner: None,
        isolation: &isolation,
        core: &core,
        persistence: Some(&persistence),
        #[cfg(feature = "streaming")]
        cdc: None,
        materialization_manifest: None,
        write_coalescer: None,
    };
    let resp = commit_conditional_mutation(&ctx_rw, &plan_rw, &method_rw, true, |core| {
        let resp = crate::server::handlers::mining::handle_associate(
            21,
            core,
            crate::server::handlers::mining::AssociationRequest {
                transactions: transactions.clone(),
                source: None,
                min_support: 0.5,
                min_confidence: 0.5,
                algorithm: medulla_protocol_types::protocol::MineAlgorithm::default(),
                writeback: crate::server::handlers::mining::WritebackOptions {
                    enabled: true,
                    #[cfg(feature = "epistemic")]
                    as_claim: false,
                },
            },
        );
        match resp.error {
            Some(e) => Err(e),
            None => Ok(resp.result.unwrap()),
        }
    })
    .await;
    assert!(
        resp.error.is_none(),
        "writeback call failed: {:?}",
        resp.error
    );

    let report = persistence
        .audit_verify_blocking(&fname)
        .expect("audit_verify_blocking");
    assert!(report.ok);
    assert_eq!(
        report.entries, 1,
        "writeback:true must produce exactly one audited durable record"
    );
}

/// Durable facade replay is decided by the kernel: a fresh nonce and request
/// id replay the stable caller key without invoking the effect again, while
/// the original nonce and a changed payload under that key are refused.
#[cfg(feature = "redb")]
#[tokio::test(flavor = "multi_thread")]
async fn durable_facade_replay_uses_stable_key_and_kernel_nonce_authority() {
    // Reads the ambient encryption env at its durable open, so the env must hold
    // still for this whole body. READ guard: it excludes only a key MUTATOR, never
    // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = temp_dir("stable-kernel-replay");
    let dir_s = dir.to_string_lossy().to_string();
    let persistence: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir_s.clone(), 64).expect("open redb backend"));
    let core = Arc::new(GraphCore::new());
    let isolation = isolation_with_system_agent();
    let graph_name = "g-medulla-stable-kernel-replay";
    let method = Method::RemoveNode {
        node_id: "n1".to_string(),
    };
    let plan = MutationPlan::for_method(&method);
    let apply_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let first = MutationCtx {
        req_id: 4,
        attempt_nonce: Some(medulla_types::contract::Nonce::from_bytes([1; 32])),
        idempotency_key: "caller-stable-remove",
        ..durable_replay_context(graph_name, &core, &isolation, &persistence)
    };
    let count = Arc::clone(&apply_count);
    let response = commit_mutation(&first, &plan, &method, move |core| {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        core.remove_node("n1".to_string());
        Ok(ResultPayload::String("ok".to_string()))
    })
    .await;
    assert!(response.error.is_none(), "{:?}", response.error);
    let _ = first;
    drop(persistence);
    drop(core);

    let persistence: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir_s, 64).expect("reopen redb backend"));
    let core = Arc::new(GraphCore::new());
    let retry = MutationCtx {
        req_id: 5,
        attempt_nonce: Some(medulla_types::contract::Nonce::from_bytes([2; 32])),
        idempotency_key: "caller-stable-remove",
        ..durable_replay_context(graph_name, &core, &isolation, &persistence)
    };
    let count = Arc::clone(&apply_count);
    let response = commit_mutation(&retry, &plan, &method, move |_| {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ResultPayload::String("duplicate".to_string()))
    })
    .await;
    assert!(response.error.is_none(), "{:?}", response.error);
    assert_eq!(format!("{:?}", response.result), "Some(String(\"ok\"))");
    assert_eq!(apply_count.load(std::sync::atomic::Ordering::SeqCst), 1);

    let consumed = MutationCtx {
        anonymous: false,
        req_id: 6,
        attempt_nonce: Some(medulla_types::contract::Nonce::from_bytes([1; 32])),
        ..retry
    };
    let response = commit_mutation(&consumed, &plan, &method, |_| {
        panic!("consumed nonce must be rejected before apply")
    })
    .await;
    assert!(response
        .error
        .as_deref()
        .is_some_and(|e| e == "REPLAY_NONCE_CONSUMED"));

    let changed = Method::RemoveNode {
        node_id: "different-node".to_string(),
    };
    let changed_plan = MutationPlan::for_method(&changed);
    let conflict = MutationCtx {
        anonymous: false,
        req_id: 7,
        attempt_nonce: Some(medulla_types::contract::Nonce::from_bytes([3; 32])),
        ..consumed
    };
    let response = commit_mutation(&conflict, &changed_plan, &changed, |_| {
        panic!("idempotency conflict must be rejected before apply")
    })
    .await;
    assert!(response
        .error
        .as_deref()
        .is_some_and(|e| e.contains("IDEMPOTENCY_CONFLICT")));
    assert_eq!(apply_count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// The async query facade uses the same durable replay authority without
/// rerunning a staged query to discover whether its key committed before a
/// lost acknowledgement.
#[cfg(all(feature = "redb", feature = "cypher"))]
#[tokio::test(flavor = "multi_thread")]
async fn staged_query_replay_probes_kernel_before_running_the_handler() {
    // Reads the ambient encryption env at its durable open, so the env must hold
    // still for this whole body. READ guard: it excludes only a key MUTATOR, never
    // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
    let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
    let dir = temp_dir("staged-query-kernel-replay");
    let dir_s = dir.to_string_lossy().to_string();
    let persistence: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir_s.clone(), 64).expect("open redb backend"));
    let core = Arc::new(GraphCore::new());
    let isolation = isolation_with_system_agent();
    let graph_name = "g-medulla-staged-query-replay";
    let method = Method::CypherQuery {
        query: "CREATE (n:ReplayProbe {id: 'n1'})".to_string(),
        mode: medulla_protocol_types::protocol::CypherMode::Write,
    };
    let plan = MutationPlan::for_method(&method);
    let apply_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let first = MutationCtx {
        req_id: 40,
        attempt_nonce: Some(medulla_types::contract::Nonce::from_bytes([11; 32])),
        idempotency_key: "caller-stable-cypher",
        ..durable_replay_context(graph_name, &core, &isolation, &persistence)
    };
    let count = Arc::clone(&apply_count);
    let response =
        commit_conditional_mutation_async(&first, &plan, &method, true, move |staged| async move {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            staged.add_node("n1".to_string(), Vec::new());
            Ok(ResultPayload::String("created".to_string()))
        })
        .await;
    assert!(response.error.is_none(), "{:?}", response.error);
    let _ = first;
    drop(persistence);
    drop(core);

    let persistence: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir_s, 64).expect("reopen redb backend"));
    let core = Arc::new(GraphCore::new());
    let retry = MutationCtx {
        req_id: 41,
        attempt_nonce: Some(medulla_types::contract::Nonce::from_bytes([12; 32])),
        idempotency_key: "caller-stable-cypher",
        ..durable_replay_context(graph_name, &core, &isolation, &persistence)
    };
    let response = commit_conditional_mutation_async(&retry, &plan, &method, true, |_| async {
        panic!("stable replay must not rerun staged query")
    })
    .await;
    assert!(response.error.is_none(), "{:?}", response.error);
    assert_eq!(
        format!("{:?}", response.result),
        "Some(String(\"created\"))"
    );
    assert_eq!(apply_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(core.has_node("n1"));

    let consumed = MutationCtx {
        anonymous: false,
        req_id: 42,
        attempt_nonce: Some(medulla_types::contract::Nonce::from_bytes([11; 32])),
        ..retry
    };
    let response = commit_conditional_mutation_async(&consumed, &plan, &method, true, |_| async {
        panic!("consumed nonce must not rerun staged query")
    })
    .await;
    assert!(response
        .error
        .as_deref()
        .is_some_and(|error| error == "REPLAY_NONCE_CONSUMED"));

    let changed = Method::CypherQuery {
        query: "CREATE (n:ReplayProbe {id: 'n2'})".to_string(),
        mode: medulla_protocol_types::protocol::CypherMode::Write,
    };
    let changed_plan = MutationPlan::for_method(&changed);
    let conflict = MutationCtx {
        anonymous: false,
        req_id: 43,
        attempt_nonce: Some(medulla_types::contract::Nonce::from_bytes([13; 32])),
        ..consumed
    };
    let response =
        commit_conditional_mutation_async(&conflict, &changed_plan, &changed, true, |_| async {
            panic!("conflict must not rerun staged query")
        })
        .await;
    assert!(response
        .error
        .as_deref()
        .is_some_and(|error| error.contains("IDEMPOTENCY_CONFLICT")));
    assert!(!core.has_node("n2"));
}
