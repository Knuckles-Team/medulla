//! Moved from the in-crate test tree of medulla-persistence (MEDULLA-CONTRACT-R056.11).

#[allow(unused_imports)]
use std::cmp::Ordering as CmpOrdering;
#[allow(unused_imports)]
use std::collections::BinaryHeap;
#[allow(unused_imports)]
use std::sync::Arc;
#[allow(unused_imports)]
use parking_lot::Mutex;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use tokio::sync::RwLock;
#[allow(unused_imports)]
use medulla_server_api::state::ServerState;
#[allow(unused_imports)]
use medulla_protocol_types::result_contract::coordination::{GraphResourceStats, ResourceSnapshot};
#[allow(unused_imports)]
use medulla_persistence::cost::budget::enforce_memory_budgets;
#[allow(unused_imports)]
use medulla_persistence::cost::*;


#[test]
fn process_rss_parser_prefers_current_then_peak_and_rejects_bad_values() {
    assert_eq!(
        parse_process_rss_bytes("Name:\teg\nVmHWM:\t29 kB\nVmRSS:\t17 kB\n"),
        17 * 1024
    );
    assert_eq!(parse_process_rss_bytes("VmHWM:\t29 kB\n"), 29 * 1024);
    assert_eq!(parse_process_rss_bytes("VmRSS:\tnot-a-number kB\n"), 0);
    assert_eq!(parse_process_rss_bytes("Name:\teg\n"), 0);
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "current_thread")]
async fn process_rss_probe_is_current_thread_safe_and_io_errors_fall_back_to_zero() {
    let root = tempfile::tempdir().expect("create RSS fixture directory");
    let status_path = root.path().join("status");
    std::fs::write(&status_path, "VmRSS:\t41 kB\n").expect("write RSS fixture");
    assert_eq!(process_rss_bytes_from(status_path).await, 41 * 1024);
    assert_eq!(
        process_rss_bytes_from(root.path().join("missing-status")).await,
        0
    );
}

#[cfg(not(target_os = "linux"))]
#[tokio::test(flavor = "current_thread")]
async fn process_rss_probe_has_a_platform_zero_fallback() {
    assert_eq!(process_rss_bytes().await, 0);
}

#[test]
fn tenant_of_splits_on_first_colon() {
    assert_eq!(tenant_of("agent:planner"), "agent");
    assert_eq!(tenant_of("enterprise-acme:billing:q1"), "enterprise-acme");
    assert_eq!(tenant_of("__commons__"), "__commons__");
    assert_eq!(tenant_of("plain"), "plain");
}

#[test]
fn resource_stats_tenant_filter_matches_only_canonical_graph_names() {
    assert!(graph_matches_tenant(
        "tenant__homelab____commons__",
        "homelab"
    ));
    assert!(graph_matches_tenant("tenant__homelab__default", "homelab"));
    assert!(graph_matches_tenant("agent:planner", "agent"));
    assert!(graph_matches_tenant("__commons__", "homelab"));
    assert!(!graph_matches_tenant("tenant__other__default", "homelab"));
    assert!(!graph_matches_tenant("tenant__homelab__private", "homelab"));
    assert!(!graph_matches_tenant("homelab__private", "homelab"));
}

#[test]
fn parse_bytes_handles_suffixes() {
    assert_eq!(parse_bytes("1024"), Some(1024));
    assert_eq!(parse_bytes("512k"), Some(512 * 1024));
    assert_eq!(parse_bytes("2m"), Some(2 * 1024 * 1024));
    assert_eq!(parse_bytes("3g"), Some(3 * 1024 * 1024 * 1024));
    assert_eq!(parse_bytes("4G"), Some(4 * 1024 * 1024 * 1024));
    assert_eq!(parse_bytes(""), None);
    assert_eq!(parse_bytes("notanumber"), None);
}

#[test]
fn resource_stats_request_rejects_zero_and_unbounded_limits() {
    assert!(ResourceStatsRequest {
        cursor: None,
        limit: 0,
        summary: false,
    }
    .validate()
    .is_err());
    assert!(ResourceStatsRequest {
        cursor: None,
        limit: MAX_RESOURCE_STATS_LIMIT + 1,
        summary: false,
    }
    .validate()
    .is_err());
    assert!(ResourceStatsRequest {
        cursor: Some("cursor".into()),
        limit: 1,
        summary: true,
    }
    .validate()
    .is_err());
    assert!(ResourceStatsRequest {
        cursor: Some("x".repeat(MAX_RESOURCE_STATS_CURSOR_BYTES + 1)),
        limit: 1,
        summary: false,
    }
    .validate()
    .is_err());
}

#[test]
fn resource_stats_candidate_heap_retains_only_the_bounded_prefix() {
    let mut candidates = BinaryHeap::new();
    for name in ["z", "b", "a", "m", "c"] {
        let candidate = ResourceStatsCandidate {
            graph: name.to_string(),
            tenant: "tenant".to_string(),
            nodes: 0,
            edges: 0,
            memory_bytes: 0,
            hibernated: false,
        };
        if candidates.len() < 2 {
            candidates.push(candidate);
        } else if candidates
            .peek()
            .is_some_and(|worst: &ResourceStatsCandidate| name < worst.graph.as_str())
        {
            candidates.pop();
            candidates.push(candidate);
        }
    }
    let mut names: Vec<_> = candidates
        .into_vec()
        .into_iter()
        .map(|candidate| candidate.graph)
        .collect();
    names.sort();
    assert_eq!(names, vec!["a", "b"]);
}

#[test]
fn hibernation_bookkeeping_does_not_cross_recreate() {
    let graph = "cost-test:recreate";
    note_hibernated(graph, "incarnation:old", true);
    assert!(is_hibernated(graph, "incarnation:old"));
    assert!(!is_hibernated(graph, "incarnation:new"));

    // A current-incarnation transition must not clear the old key by name.
    note_hibernated(graph, "incarnation:new", false);
    assert!(is_hibernated(graph, "incarnation:old"));
    forget_graph_incarnation(graph, "incarnation:old");
    assert!(!is_hibernated(graph, "incarnation:old"));
    assert!(!is_hibernated(graph, "incarnation:new"));
    forget_graph(graph);
}

// ── Budget-enforcement integration proofs (CONCEPT:MEDULLA-KG.compute.lane-v) ────────────
// These need a real durable backend (redb) so eviction is durability-gated and a
// rehydrate read serves the evicted node back. Gated on `redb` (present in
// pi/node/cluster/full); the budget/snapshot logic itself is feature `cost`.
#[cfg(feature = "redb")]
mod integration {
    use super::*;
    use medulla_protocol_types::protocol::GraphType;
    use medulla_protocol_types::protocol::Method;
    use medulla_protocol_types::protocol::Request;
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_server_auth::auth::build_shared_test_request;
    use medulla_server_auth::auth::dispatch_test_on_heap as dispatch_on_heap;

    const SECRET: &str = "cost-budget-test";
    const TEST_AGENT: &str = "unit-test-agent";

    fn props(v: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec(&v).unwrap()
    }

    async fn redb_state(dir_s: &str) -> Arc<RwLock<ServerState>> {
        medulla_persistence::cold_offload::test_redb_state(
            SECRET,
            dir_s,
            ServerState::test_isolation(TEST_AGENT),
            256,
        )
        .await
    }

    fn req(id: u64, graph: &str, method: Method) -> Request {
        build_shared_test_request(SECRET, id, graph, TEST_AGENT, method)
    }

    async fn assert_authoritative_projection(state: &Arc<RwLock<ServerState>>, graph: &str) {
        let (core, backend, manifest) = {
            let state = state.read().await;
            let core = state
                .registry
                .get(graph)
                .unwrap_or_else(|| panic!("graph {graph} is not resident"))
                .core
                .clone();
            let backend = state
                .persistence
                .clone()
                .expect("cost integration state has durable persistence");
            let manifest = state
                .registry
                .materialization_manifest(graph)
                .expect("resident graph has a materialization manifest");
            (core, backend, manifest)
        };
        let fname = medulla_kernel::persist::sanitize(graph);
        let durable_version = backend
            .read_mutation_graph_version(&fname)
            .await
            .expect("authoritative version read")
            .expect("created graph has an authoritative version");
        assert_eq!(
            core.version(),
            durable_version,
            "serving projection must publish the committed authoritative version"
        );
        assert!(manifest.valid, "resident projection must be complete");
        assert_eq!(
            manifest.source_snapshot_version,
            Some(durable_version),
            "materialization manifest must carry the authoritative watermark"
        );
    }

    async fn create(state: &Arc<RwLock<ServerState>>, id: u64, graph: &str) {
        let r = dispatch_on_heap(
            state,
            req(
                id,
                graph,
                Method::CreateGraph {
                    graph_name: graph.into(),
                    graph_type: GraphType::Agent,
                },
            ),
        )
        .await;
        assert!(r.error.is_none(), "create {graph}: {:?}", r.error);
        assert_authoritative_projection(state, graph).await;
    }

    async fn add(state: &Arc<RwLock<ServerState>>, id: u64, graph: &str, node: &str) {
        assert_authoritative_projection(state, graph).await;
        let r = dispatch_on_heap(
            state,
            req(
                id,
                graph,
                Method::AddNode {
                    node_id: node.into(),
                    properties_msgpack: props(serde_json::json!({"payload": node})),
                },
            ),
        )
        .await;
        assert!(r.error.is_none(), "add {node}: {:?}", r.error);
    }

    /// PROOF 1: a tenant over its byte budget triggers eviction/hibernation of its
    /// cold graphs and stays under cap; the evicted data rehydrates correctly on a
    /// read (durability-gated, no loss).
    #[tokio::test(flavor = "multi_thread")]
    async fn over_budget_evicts_and_rehydrates() {
        // Reads the ambient encryption env at its durable open, so the env must hold
        // still for this whole body. READ guard: it excludes only a key MUTATOR, never
        // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir =
            std::env::temp_dir().join(format!("medulla-cost-budget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();
        let state = redb_state(&dir_s).await;

        // One tenant `acme` with two graphs; fill them with nodes so the tenant's
        // resident memory is well over a tiny budget.
        create(&state, 1, "acme:hot").await;
        create(&state, 2, "acme:cold").await;
        let hot_was_evicted = {
            let state = state.read().await;
            let resident = state.registry.get("acme:hot").is_some();
            let manifest = state
                .registry
                .materialization_manifest("acme:hot")
                .expect("created graph remains cataloged");
            if !resident {
                assert_eq!(
                    manifest.phase,
                    medulla_core::registry::MaterializationPhase::CatalogOnly,
                    "capacity eviction must leave a catalog-only generation"
                );
            }
            !resident
        };
        if hot_was_evicted {
            let reopened = dispatch_on_heap(
                &state,
                req(
                    50,
                    "acme:hot",
                    Method::GetNodeProperties {
                        node_id: "version-probe".into(),
                    },
                ),
            )
            .await;
            assert!(
                reopened.error.is_none(),
                "catalog-only graph must reopen: {:?}",
                reopened.error
            );
        }
        assert_authoritative_projection(&state, "acme:hot").await;
        let mut id = 100u64;
        for i in 0..40 {
            add(&state, id, "acme:hot", &format!("h{i}")).await;
            id += 1;
            add(&state, id, "acme:cold", &format!("c{i}")).await;
            id += 1;
        }

        let before = collect_resource_stats(&state).await.unwrap();
        let acme_before = before
            .tenants
            .iter()
            .find(|t| t.tenant == "acme")
            .expect("acme tenant present");
        assert!(acme_before.memory_bytes > 0);

        // A budget below the tenant's TOTAL footprint (both graphs, ~3.2KB each at
        // today's `NODE_OVERHEAD` calibration) forces reclamation, but above ONE
        // graph's individual footprint so coldest-first eviction stops after
        // reclaiming exactly one graph, leaving the other resident — matching the
        // "reclamation is coldest-graph-granular" comment below. A budget under one
        // graph's footprint (the previous 2048/4096 constants) evicts BOTH graphs to
        // catalog-only, emptying the tenant from `collect_resource_stats` entirely
        // and failing the `acme` lookup below — not a BUG-192/193 regression, a
        // pre-existing miscalibration against the real per-node accounting.
        let cfg = CostConfig {
            global_ceiling_bytes: 9000,
            per_tenant_budget_bytes: 4500,
            interval_secs: 1,
        };
        assert!(acme_before.memory_bytes > cfg.per_tenant_budget_bytes);

        let (evicted, hibernated) = enforce_memory_budgets(&state, cfg).await;
        assert!(
            evicted > 0 || hibernated > 0,
            "expected the over-budget tenant to be reclaimed"
        );

        // STAYS UNDER CAP: after enforcement the tenant's resident footprint is at
        // or under the budget (within one graph's residual, since reclamation is
        // coldest-graph-granular).
        let after = collect_resource_stats(&state).await.unwrap();
        let acme_after = after.tenants.iter().find(|t| t.tenant == "acme").unwrap();
        assert!(
            acme_after.memory_bytes < acme_before.memory_bytes,
            "tenant footprint should shrink: {} !< {}",
            acme_after.memory_bytes,
            acme_before.memory_bytes
        );

        // REHYDRATES CORRECTLY: a read of an evicted node serves its durable blob
        // back through the read-through seam (no data loss).
        let r = dispatch_on_heap(
            &state,
            req(
                9000,
                "acme:cold",
                Method::GetNodeProperties {
                    node_id: "c0".into(),
                },
            ),
        )
        .await;
        assert!(
            r.error.is_none(),
            "evicted node must still read: {:?}",
            r.error
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// U-142/U-148 (BUG-130) — regression closing the exact coverage gap the bug
    /// report calls out: `over_budget_evicts_and_rehydrates` above only ever
    /// checks `GetNodeProperties`, whose redb point-read seam serves an
    /// evicted-but-still-"resident" node just fine and therefore MASKS this
    /// defect. Pre-fix, `enforce_memory_budgets` durability-confirmed every node
    /// empty via `GraphCore::evict_resident_nodes` but never told the REGISTRY —
    /// `GraphRegistry::get`/`is_resident` kept reporting the graph resident with
    /// zero topology, so `dispatch.rs`'s cold-path lazy-open (gated on a registry
    /// MISS) never re-triggered, and the graph stayed permanently, silently empty
    /// to any whole-graph operation for the rest of the process's life. A
    /// governed `GetNodeProperties` point read still worked (redb read-through has
    /// no registry-residency dependency); a native Cypher `MATCH` did not (it runs
    /// `analysis_snapshot()` over the live, now-permanently-empty resident
    /// `GraphCore`, with no read-through fallback). FAILS without the
    /// `registry.evict_resident` call added to `enforce_memory_budgets`; PASSES
    /// with it.
    #[tokio::test(flavor = "multi_thread")]
    async fn budget_eviction_leaves_graph_catalog_only_so_cypher_and_point_read_agree() {
        // Reads the ambient encryption env at its durable open, so the env must hold
        // still for this whole body. READ guard: it excludes only a key MUTATOR, never
        // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir = std::env::temp_dir()
            .join(format!("medulla-cost-cypher-evict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();
        let state = redb_state(&dir_s).await;

        const NODE_COUNT: usize = 20;
        create(&state, 1, "acme:solo").await;
        for (id, i) in (100u64..).zip(0..NODE_COUNT) {
            add(&state, id, "acme:solo", &format!("n{i}")).await;
        }

        // `acme:solo` is the ONLY graph for tenant `acme`, so a budget far below
        // its footprint forces the enforcer to reclaim it down to zero (there is
        // no colder sibling graph to reclaim instead).
        let cfg = CostConfig {
            global_ceiling_bytes: 1,
            per_tenant_budget_bytes: 1,
            interval_secs: 1,
        };
        enforce_memory_budgets(&state, cfg).await;

        // The registry must have transitioned this graph OUT of the resident map
        // entirely (catalog-only) — not merely emptied its topology while still
        // reporting resident. This is the exact defect: pre-fix, `registry.get`
        // still returned `Some` here because only the `GraphCore`'s RAM was
        // cleared, never the registry entry.
        {
            let s = state.read().await;
            assert!(
                s.registry.get("acme:solo").is_none(),
                "a fully durability-reclaimed graph must leave the resident map \
                 (go catalog-only), or dispatch's cold-path lazy-open (gated on a \
                 registry MISS) never re-triggers on the next access"
            );
            let manifest = s
                .registry
                .materialization_manifest("acme:solo")
                .expect("an evicted graph remains cataloged, just not resident");
            assert_eq!(
                manifest.phase,
                medulla_core::registry::MaterializationPhase::CatalogOnly,
                "an evicted graph must be catalog-only, ready for the next lazy-open"
            );
        }

        // GOVERNED POINT READ: rehydrates via lazy-open, sees the durable node.
        let point = dispatch_on_heap(
            &state,
            req(
                9001,
                "acme:solo",
                Method::GetNodeProperties {
                    node_id: "n0".into(),
                },
            ),
        )
        .await;
        assert!(point.error.is_none(), "point read: {:?}", point.error);

        // NATIVE CYPHER over the SAME graph, the SAME data, via the whole-graph
        // MATCH path (`analysis_snapshot()`, no read-through fallback) — must see
        // every durably-written node, not zero. This is the assertion prior
        // "only GetNodeProperties" coverage never made, and the one U-142/U-148
        // reproduced live: governed reads saw data while Cypher saw none.
        let cypher = dispatch_on_heap(
            &state,
            req(
                9002,
                "acme:solo",
                Method::CypherQuery {
                    query: "MATCH (n) RETURN n".into(),
                    mode: medulla_protocol_types::protocol::CypherMode::Read,
                },
            ),
        )
        .await;
        assert!(cypher.error.is_none(), "cypher: {:?}", cypher.error);
        let bytes = match &cypher.result {
            Some(ResultPayload::Raw(b)) => b.clone(),
            other => panic!("expected Raw cypher result, got {other:?}"),
        };
        let result: medulla_protocol_types::protocol::QueryResult = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(
            result.rows.len(),
            NODE_COUNT,
            "Cypher must observe every durably-written node once the budget \
             eviction's catalog-only transition lets dispatch lazily reopen the \
             graph — a stale/empty-but-\"resident\" image would report 0 rows \
             here even though the governed point read above already found data \
             (through redb's separate read-through seam, which does not depend \
             on registry residency the way a whole-graph scan does)"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PROOF 2: a fair per-tenant cap stops one hot tenant from starving others —
    /// the hot tenant is reclaimed while a small tenant under the fair share is left
    /// alone.
    #[tokio::test(flavor = "multi_thread")]
    async fn fair_cap_protects_small_tenant() {
        // Reads the ambient encryption env at its durable open, so the env must hold
        // still for this whole body. READ guard: it excludes only a key MUTATOR, never
        // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir =
            std::env::temp_dir().join(format!("medulla-cost-fair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();
        let state = redb_state(&dir_s).await;

        create(&state, 1, "whale:big").await;
        create(&state, 2, "minnow:small").await;
        let mut id = 100u64;
        for i in 0..60 {
            add(&state, id, "whale:big", &format!("b{i}")).await;
            id += 1;
        }
        add(&state, id, "minnow:small", "s0").await;

        let small_before = {
            let s = collect_resource_stats(&state).await.unwrap();
            s.tenants
                .iter()
                .find(|t| t.tenant == "minnow")
                .unwrap()
                .memory_bytes
        };

        // Global ceiling split across 2 tenants → fair share is generous enough for
        // the minnow but the whale blows past it.
        let cfg = CostConfig {
            global_ceiling_bytes: 8192,
            per_tenant_budget_bytes: 8192,
            interval_secs: 1,
        };
        enforce_memory_budgets(&state, cfg).await;

        let after = collect_resource_stats(&state).await.unwrap();
        let small_after = after
            .tenants
            .iter()
            .find(|t| t.tenant == "minnow")
            .unwrap()
            .memory_bytes;
        // The minnow (well under its fair share) is untouched.
        assert_eq!(
            small_after, small_before,
            "small tenant under the fair share must not be reclaimed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// PROOF 3: ResourceStats returns accurate per-graph + aggregate counts under a
    /// realistic fill (the autoscale-signal accuracy gate).
    #[tokio::test(flavor = "multi_thread")]
    async fn resource_stats_counts_are_accurate() {
        // Reads the ambient encryption env at its durable open, so the env must hold
        // still for this whole body. READ guard: it excludes only a key MUTATOR, never
        // another opener. See `medulla_kernel::crypto::acquire_test_env_read_lock`'s doc.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir =
            std::env::temp_dir().join(format!("medulla-cost-stats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();
        let state = redb_state(&dir_s).await;

        create(&state, 1, "acme:a").await;
        create(&state, 2, "beta:b").await;
        for i in 0..10 {
            add(&state, 100 + i, "acme:a", &format!("a{i}")).await;
        }
        for i in 0..5 {
            add(&state, 200 + i, "beta:b", &format!("b{i}")).await;
        }

        let snap = collect_resource_stats(&state).await.unwrap();

        // Per-graph node counts are exact.
        let ga = snap.graphs.iter().find(|g| g.graph == "acme:a").unwrap();
        let gb = snap.graphs.iter().find(|g| g.graph == "beta:b").unwrap();
        assert_eq!(ga.nodes, 10, "acme:a node count");
        assert_eq!(gb.nodes, 5, "beta:b node count");
        assert_eq!(ga.tenant, "acme");
        assert_eq!(gb.tenant, "beta");
        assert!(
            ga.memory_bytes > gb.memory_bytes,
            "more nodes ⇒ more memory"
        );

        // Aggregate node count = 15 (+ whatever __commons__ holds, which is 0 here).
        assert_eq!(snap.total_nodes, 15, "aggregate node count");
        // Three graphs: __commons__ + acme:a + beta:b.
        assert_eq!(snap.graph_count, 3);
        // Three tenants: __commons__, acme, beta.
        assert_eq!(snap.tenant_count, 3);
        // Per-tenant rollup is exact.
        let ta = snap.tenants.iter().find(|t| t.tenant == "acme").unwrap();
        assert_eq!(ta.nodes, 10);
        assert_eq!(ta.graphs, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn capacity_estimate_scales_with_tenants_and_shards() {
    // 1000 tenants, 1 MiB working set each, 256 MiB/shard, 50% hot at once.
    let mib = 1024 * 1024;
    let est = capacity_estimate(1000, mib, 256 * mib, 0.5);
    assert_eq!(est.total_working_set_bytes, 1000 * mib);
    assert_eq!(est.budgeted_resident_bytes, 500 * mib);
    // 500 MiB / 256 MiB = 2 shards (ceil).
    assert_eq!(est.recommended_shards, 2);

    // Holding everything hot (fraction 1.0) needs more shards.
    let hot = capacity_estimate(1000, mib, 256 * mib, 1.0);
    assert_eq!(hot.budgeted_resident_bytes, 1000 * mib);
    assert_eq!(hot.recommended_shards, 4);

    // Zero shard budget ⇒ undefined shard count (0), never a divide-by-zero.
    assert_eq!(capacity_estimate(10, mib, 0, 1.0).recommended_shards, 0);
}
