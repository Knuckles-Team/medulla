//! Moved from the in-crate test tree of medulla-persistence (MEDULLA-CONTRACT-R056.11).

#[allow(unused_imports)]
use std::collections::{HashMap, HashSet};
#[allow(unused_imports)]
use std::sync::atomic::{AtomicU64, Ordering};
#[allow(unused_imports)]
use std::sync::Arc;
#[allow(unused_imports)]
use std::time::{Duration, Instant};
#[allow(unused_imports)]
use parking_lot::Mutex;
#[allow(unused_imports)]
use tokio::sync::RwLock;
#[allow(unused_imports)]
use medulla_core::registry::GraphHandle;
#[allow(unused_imports)]
use medulla_graph_core::graph::GraphCore;
#[allow(unused_imports)]
use medulla_server_api::api::persistence::PersistenceBackend;
#[allow(unused_imports)]
use medulla_server_api::state::ServerState;
#[allow(unused_imports)]
use medulla_persistence::cold_offload::*;

mod tracker_tests {
    use super::ColdTenantTracker;
    use std::time::Duration;

    /// A same-name recreate gets an independent access clock; the retired
    /// identity is removed before the new touch is recorded.
    #[test]
    fn access_recency_is_fenced_by_incarnation() {
        let tracker = ColdTenantTracker::new();
        tracker.touch_with_incarnation("tenant:recreate", "incarnation:old");
        assert!(tracker.is_cold_for("tenant:recreate", "incarnation:old", Duration::ZERO));

        tracker.touch_with_incarnation("tenant:recreate", "incarnation:new");
        assert!(!tracker.is_cold_for(
            "tenant:recreate",
            "incarnation:new",
            Duration::from_secs(3600)
        ));
        tracker.forget_incarnation("tenant:recreate", "incarnation:old");
        assert!(tracker.cold_graphs(Duration::from_secs(3600)).is_empty());
    }
}

// The admission tests below drive a REAL `RedbBackend` through
// `lazy_open`/`admit_capacity` (both `redb`-gated), so — like every other
// `ServerState`-constructing test module in this crate — that module only
// builds under the `redb` feature.  The focused gate tests above use a tiny
// backend double so every refusal branch remains deterministic.
#[cfg(feature = "redb")]
mod unsafe_eviction_tests {
    use super::offload_graph_core;
    use medulla_graph_core::graph::GraphCore;
    use medulla_server_api::api::persistence::PersistenceBackend;
    use std::sync::Arc;

    #[derive(Clone, Copy)]
    enum PresenceOutcome {
        All,
        False,
        WrongLength,
        Error,
    }

    struct StubBackend {
        outcome: PresenceOutcome,
    }

    #[async_trait::async_trait]
    impl PersistenceBackend for StubBackend {
        fn durable_node_presence(
            &self,
            _graph_fname: &str,
            node_ids: &[String],
        ) -> Result<Vec<bool>, String> {
            match self.outcome {
                PresenceOutcome::All => Ok(vec![true; node_ids.len()]),
                PresenceOutcome::False => Ok(vec![false; node_ids.len()]),
                PresenceOutcome::WrongLength => Ok(Vec::new()),
                PresenceOutcome::Error => Err("presence lookup failed".to_string()),
            }
        }

        fn shutdown(&self) {}
    }

    fn resident_core() -> (GraphCore, Vec<u8>) {
        let core = GraphCore::new();
        let properties = vec![0x91, 0xa1, 0x78];
        core.add_node("node-1".to_string(), properties.clone());
        (core, properties)
    }

    #[test]
    fn unsafe_presence_outcomes_retain_the_resident_projection() {
        for (label, outcome) in [
            ("false", PresenceOutcome::False),
            ("wrong-length", PresenceOutcome::WrongLength),
            ("error", PresenceOutcome::Error),
        ] {
            let (core, properties) = resident_core();
            let backend: Arc<dyn PersistenceBackend> = Arc::new(StubBackend { outcome });

            assert_eq!(
                offload_graph_core(&core, &backend, "tenant:unsafe"),
                None,
                "{label} durable presence must refuse eviction"
            );
            assert_eq!(core.node_count(), 1, "{label} path dropped a resident node");
            assert_eq!(core.get_node_properties("node-1"), Some(properties));
        }
    }

    #[test]
    fn complete_presence_is_the_only_path_that_hibernates() {
        let (core, _) = resident_core();
        let backend: Arc<dyn PersistenceBackend> = Arc::new(StubBackend {
            outcome: PresenceOutcome::All,
        });

        assert_eq!(
            offload_graph_core(&core, &backend, "tenant:durable"),
            Some(1)
        );
        assert_eq!(core.node_count(), 0);
        assert_eq!(core.get_node_properties("node-1"), None);
    }
}

#[cfg(feature = "redb")]
mod admission_tests {
    //! Bounded hot-context cache + admission-control proofs over a REAL redb
    //! backend (CONCEPT:MEDULLA-KG.sharding.lazy-graph-catalog, DIST-P2-3) — the durable tier a lazily-opened
    //! graph rehydrates from, and the tier that makes eviction loss-free.
    use super::*;
    use medulla_persistence::read_through::BackendGraphMaterializer;
    use medulla_persistence::read_through::BackendReadThroughFactory;
    use medulla_persistence::redb_backend::RedbBackend;
    use medulla_isolation::isolation::IsolationLayer;
    use medulla_protocol_types::protocol::GraphType;
    use medulla_protocol_types::protocol::Method;
    use medulla_protocol_types::protocol::Request;
    use medulla_server_api::api::persistence::PersistenceBackend;
    use medulla_server_auth::auth::build_current_test_request;
    use medulla_server_auth::auth::dispatch_test_on_heap as dispatch_on_heap;
    use medulla_server_api::state::ServerState;
    #[cfg(feature = "security")]
    use medulla_types::acl::AgentIdentity;
    use medulla_types::acl::AgentRole;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use tokio::sync::{RwLock, Semaphore};

    fn props(v: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec(&v).unwrap()
    }

    const SECRET: &str = "lazy-lifecycle-test";
    const TEST_AGENT: &str = "unit-test-agent";
    static NONCE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    #[cfg(feature = "security")]
    fn rls_isolation() -> IsolationLayer {
        use medulla_types::acl::Grant;
        use medulla_types::acl::GrantEffect;
        use medulla_types::acl::RbacAction;
        use medulla_types::acl::ResourceSelector;
        use medulla_types::acl::Role;

        let mut isolation = ServerState::test_isolation(TEST_AGENT);
        isolation.register_agent(AgentIdentity {
            agent_id: "alice".to_string(),
            role: AgentRole::Agent,
            teams: Vec::new(),
            roles: vec!["commons-rw".to_string()],
        });
        isolation.register_agent(AgentIdentity {
            agent_id: "bob".to_string(),
            role: AgentRole::Agent,
            teams: Vec::new(),
            roles: vec!["commons-rw".to_string()],
        });
        isolation.add_role(Role::new("commons-rw"));
        for action in [RbacAction::Read, RbacAction::Write] {
            isolation.add_grant(Grant {
                role: "commons-rw".to_string(),
                resource: ResourceSelector::Graph("__commons__".to_string()),
                action,
                effect: GrantEffect::Allow,
            });
        }
        isolation
    }

    async fn state_with_backend(
        dir_s: &str,
        backend: Arc<dyn PersistenceBackend>,
        isolation: IsolationLayer,
    ) -> Arc<RwLock<ServerState>> {
        super::test_state_with_backend(SECRET, dir_s, backend, isolation).await
    }

    async fn redb_state(dir_s: &str) -> Arc<RwLock<ServerState>> {
        super::test_redb_state(SECRET, dir_s, ServerState::test_isolation(TEST_AGENT), 64).await
    }

    fn req(id: u64, graph: &str, method: Method) -> Request {
        req_as_tenant(id, graph, TEST_AGENT, "tenant-shared", method)
    }

    fn req_as_tenant(
        id: u64,
        graph: &str,
        agent_id: &str,
        tenant: &str,
        method: Method,
    ) -> Request {
        build_current_test_request(SECRET, tenant, id, graph, agent_id, method)
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
    }

    async fn add_node(state: &Arc<RwLock<ServerState>>, id: u64, graph: &str, node: &str) {
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

    #[cfg(feature = "security")]
    async fn add_rls_node(
        state: &Arc<RwLock<ServerState>>,
        id: u64,
        node: &str,
        properties: serde_json::Value,
    ) {
        let r = dispatch_on_heap(
            state,
            req_as_tenant(
                id,
                "__commons__",
                TEST_AGENT,
                "tenant-shared",
                Method::AddNode {
                    node_id: node.to_string(),
                    properties_msgpack: props(properties),
                },
            ),
        )
        .await;
        assert!(r.error.is_none(), "add {node}: {:?}", r.error);
    }

    #[cfg(feature = "security")]
    async fn add_rls_edge(state: &Arc<RwLock<ServerState>>, id: u64, source: &str, target: &str) {
        let r = dispatch_on_heap(
            state,
            req_as_tenant(
                id,
                "__commons__",
                TEST_AGENT,
                "tenant-shared",
                Method::AddEdge {
                    source_id: source.to_string(),
                    target_id: target.to_string(),
                    properties_msgpack: props(serde_json::json!({"relationship": "RELATED_TO"})),
                },
            ),
        )
        .await;
        assert!(
            r.error.is_none(),
            "add edge {source}->{target}: {:?}",
            r.error
        );
    }

    #[cfg(feature = "security")]
    fn raw_result(response: &medulla_protocol_types::protocol::Response) -> Vec<u8> {
        match &response.result {
            Some(medulla_protocol_types::protocol::ResultPayload::Raw(bytes)) => bytes.clone(),
            other => panic!("expected a raw result, got {other:?}"),
        }
    }

    /// N graphs are registered (catalog rows) but only M<N are ever accessed —
    /// the untouched majority never materializes a resident `GraphCore`. Accessing
    /// a cold one lazily opens it (byte-identical data) without disturbing the
    /// rest.
    #[tokio::test(flavor = "multi_thread")]
    async fn n_registered_m_resident_lazy_open_on_access() {
        // Holds the encryption env still for this test's whole body: it opens a
        // durable store, so a concurrent key mutation would break its canary
        // check. A READ guard, so these tests still run concurrently with each
        // other -- only a key MUTATOR is excluded.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir = std::env::temp_dir().join(format!("medulla-lazy-n-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();
        let state = redb_state(&dir_s).await;

        // Register 20 graphs, but only touch 3 of them.
        let n = 20;
        for i in 0..n {
            create(&state, i as u64, &format!("tenant:{i}")).await;
        }
        for i in 0..n {
            add_node(&state, 1000 + i as u64, &format!("tenant:{i}"), "seed").await;
        }
        // Checkpoint so every graph is durable, then evict ALL of them back to
        // catalog-only (simulating cold tenants that have aged out of RAM),
        // leaving the catalog fully populated but nothing but __commons__ resident.
        {
            let mut s = state.write().await;
            for i in 0..n {
                s.registry.evict_resident(&format!("tenant:{i}"));
            }
        }

        assert_eq!(
            state.read().await.registry.catalog_len(),
            n + 1,
            "catalog knows about every registered graph"
        );
        assert_eq!(
            state.read().await.registry.resident_len(),
            1,
            "only __commons__ resident after eviction — catalog rows, not resident cores"
        );

        // Access exactly 3 cold graphs through the real dispatch path.
        for i in [2, 9, 17] {
            let r = dispatch_on_heap(
                &state,
                req(
                    5000 + i,
                    &format!("tenant:{i}"),
                    Method::GetNodeProperties {
                        node_id: "seed".into(),
                    },
                ),
            )
            .await;
            assert!(r.error.is_none(), "lazy open tenant:{i}: {:?}", r.error);
        }

        assert_eq!(
            state.read().await.registry.resident_len(),
            4,
            "__commons__ + exactly the 3 accessed graphs are resident"
        );
        assert!(state.read().await.registry.is_resident("tenant:2"));
        assert!(!state.read().await.registry.is_resident("tenant:3"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An evicted graph's data is fully intact after it lazily re-opens — the
    /// durability-gated eviction never loses anything.
    #[tokio::test(flavor = "multi_thread")]
    async fn evicted_graph_lazy_reopens_with_data_intact() {
        // Holds the encryption env still for this test's whole body: it opens a
        // durable store, so a concurrent key mutation would break its canary
        // check. A READ guard, so these tests still run concurrently with each
        // other -- only a key MUTATOR is excluded.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir = std::env::temp_dir().join(format!("medulla-lazy-reopen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();
        let state = redb_state(&dir_s).await;

        create(&state, 1, "acme:cold").await;
        for i in 0..5 {
            add_node(&state, 100 + i, "acme:cold", &format!("n{i}")).await;
        }
        {
            let mut s = state.write().await;
            assert!(s.registry.evict_resident("acme:cold"));
        }
        assert!(!state.read().await.registry.is_resident("acme:cold"));

        // Access it — lazy_open re-materializes it from the durable tier.
        let opened = lazy_open(
            &state,
            "acme:cold",
            DEFAULT_PRODUCTION_MAX_RESIDENT_GRAPHS,
            DEFAULT_PRODUCTION_LAZY_OPEN_PAGE_SIZE,
        )
        .await;
        assert!(opened);
        let core = {
            let s = state.read().await;
            s.registry.get("acme:cold").unwrap().core.clone()
        };
        assert_eq!(core.node_count(), 5, "all 5 nodes survived the round-trip");
        for i in 0..5 {
            assert_eq!(
                core.get_node_properties(&format!("n{i}")),
                Some(props(serde_json::json!({"payload": format!("n{i}")})))
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CONCEPT:MEDULLA-KG.sharding.paged-lazy-open (L38 "paged adjacency") — the literal "no full rehydrate"
    /// proof: `open_lazy_paged` with a small `page_size` over a REAL redb-backed
    /// `RedbBackend::read_graph_material_page_blocking` materializes ONLY that page's
    /// worth of nodes immediately, not the whole graph, and repeated deterministic
    /// `page_in` calls land the rest one bounded batch at a time.
    #[tokio::test(flavor = "multi_thread")]
    async fn open_lazy_paged_materializes_one_page_then_page_in_completes_deterministically() {
        // Holds the encryption env still for this test's whole body: it opens a
        // durable store, so a concurrent key mutation would break its canary
        // check. A READ guard, so these tests still run concurrently with each
        // other -- only a key MUTATOR is excluded.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir =
            std::env::temp_dir().join(format!("medulla-lazy-paged-manual-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();
        let state = redb_state(&dir_s).await;

        create(&state, 1, "paged:manual").await;
        let total_nodes = 20u64;
        for i in 0..total_nodes {
            add_node(&state, 100 + i, "paged:manual", &format!("n{i}")).await;
        }
        {
            let mut s = state.write().await;
            assert!(s.registry.evict_resident("paged:manual"));
        }
        assert!(!state.read().await.registry.is_resident("paged:manual"));

        let page_size = 4usize;
        let mut s = state.write().await;
        let outcome = s.registry.open_lazy_paged("paged:manual", page_size);
        assert!(outcome.resident, "paged open reports resident immediately");
        let node_count_after_first_page = s.registry.get("paged:manual").unwrap().core.node_count();
        assert!(
            node_count_after_first_page < total_nodes as usize,
            "the FIRST page must NOT fully rehydrate the graph (got {node_count_after_first_page} \
             of {total_nodes} — a full rehydrate here would defeat the whole point of L38)"
        );
        assert!(
            node_count_after_first_page > 0,
            "the first page still materializes SOMETHING (resident ⇒ queryable right away)"
        );
        assert!(
            outcome.cursor.is_some(),
            "more material remains ⇒ a resume cursor must be returned"
        );

        // Drive page_in to completion — deterministic, no background task / timing.
        let mut cursor = outcome.cursor;
        let mut iterations = 0;
        while let Some(c) = cursor {
            cursor = s.registry.page_in("paged:manual", c, page_size);
            iterations += 1;
            assert!(iterations < 1000, "runaway paging — cursor never exhausted");
        }
        assert!(
            iterations > 1,
            "a {total_nodes}-node graph at page_size={page_size} needs more than one page_in call"
        );

        let final_count = s.registry.get("paged:manual").unwrap().core.node_count();
        assert_eq!(
            final_count, total_nodes as usize,
            "page_in eventually lands EVERY node — no data lost across the paged sequence"
        );
        for i in 0..total_nodes {
            assert_eq!(
                s.registry
                    .get("paged:manual")
                    .unwrap()
                    .core
                    .get_node_properties(&format!("n{i}")),
                Some(props(serde_json::json!({"payload": format!("n{i}")}))),
                "node n{i} survived the paged round-trip byte-identical to a full rehydrate"
            );
        }
        drop(s);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CONCEPT:MEDULLA-KG.sharding.paged-lazy-open (L38) — `lazy_open`'s wiring: with `page_size > 0` it
    /// returns resident/queryable immediately (without inline-blocking on a full
    /// rehydrate), and the spawned [`page_in_remaining`] background task eventually
    /// lands the rest of a graph bigger than one page.
    #[tokio::test(flavor = "multi_thread")]
    async fn paged_lazy_open_wiring_is_resident_immediately_then_completes_in_background() {
        // Holds the encryption env still for this test's whole body: it opens a
        // durable store, so a concurrent key mutation would break its canary
        // check. A READ guard, so these tests still run concurrently with each
        // other -- only a key MUTATOR is excluded.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir =
            std::env::temp_dir().join(format!("medulla-lazy-paged-bg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();
        let state = redb_state(&dir_s).await;

        create(&state, 1, "paged:big").await;
        let total_nodes = 25u64;
        for i in 0..total_nodes {
            add_node(&state, 100 + i, "paged:big", &format!("n{i}")).await;
        }
        {
            let mut s = state.write().await;
            assert!(s.registry.evict_resident("paged:big"));
        }
        assert!(!state.read().await.registry.is_resident("paged:big"));

        // page_size well under total_nodes forces a background continuation.
        let opened = lazy_open(
            &state,
            "paged:big",
            DEFAULT_PRODUCTION_MAX_RESIDENT_GRAPHS,
            5,
        )
        .await;
        assert!(opened, "paged lazy-open must report resident immediately");
        assert!(
            state.read().await.registry.is_resident("paged:big"),
            "graph is queryable right away, even though not fully paged in yet"
        );

        // The background page_in_remaining task eventually lands every node — poll
        // with a bounded deadline rather than a fixed sleep (deterministic pass/fail,
        // no race with the tokio scheduler's timing).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let count = {
                let s = state.read().await;
                s.registry.get("paged:big").map(|e| e.core.node_count())
            };
            if count == Some(total_nodes as usize) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "background page-in did not complete within the deadline (last count: {count:?})"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // Byte-identical to a full rehydrate: every node's properties survived.
        let core = state
            .read()
            .await
            .registry
            .get("paged:big")
            .unwrap()
            .core
            .clone();
        for i in 0..total_nodes {
            assert_eq!(
                core.get_node_properties(&format!("n{i}")),
                Some(props(serde_json::json!({"payload": format!("n{i}")})))
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Startup with many persisted graphs (`load_catalog`) hydrates NONE of them —
    /// only the catalog populates; every graph materializes on first access.
    #[tokio::test(flavor = "multi_thread")]
    async fn lazy_startup_hydrates_nothing_until_accessed() {
        // Held for the whole test: it opens `redb_state` (write side), shuts it down,
        // then opens a SECOND `RedbBackend` on the same dir (reload side) — both must
        // resolve the same `MEDULLA_ENCRYPTION_KEY` cipher. See
        // `medulla_kernel::crypto::acquire_test_env_lock`'s doc for the full mechanism.
        #[cfg(feature = "security")]
        let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
        let dir = std::env::temp_dir().join(format!("medulla-lazy-boot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();

        // ── write side: create + populate 8 graphs, then shut down ──
        {
            let state = redb_state(&dir_s).await;
            for i in 0..8 {
                create(&state, i as u64, &format!("boot:{i}")).await;
                add_node(&state, 100 + i as u64, &format!("boot:{i}"), "n").await;
            }
            let backend = state.read().await.persistence.clone().unwrap();
            backend.shutdown();
        }

        // ── reload side: fresh backend + fresh empty state, CATALOG-ONLY load ──
        let backend2: Arc<dyn PersistenceBackend> =
            Arc::new(RedbBackend::open(dir_s.clone(), 64).expect("reopen"));
        // RBAC (CONCEPT:MEDULLA-KG.compute.feature) is mandatory for every non-System
        // identity, so the reload fixture uses the same registered System agent
        // as the write-side state.
        let mut state2 = ServerState::new_for_test(SECRET, ServerState::test_isolation(TEST_AGENT));
        state2.persist_dir = Some(dir_s.clone());
        state2.persistence = Some(backend2.clone());
        state2.max_in_flight = Arc::new(Semaphore::new(64));
        state2.read_admission = Arc::new(Semaphore::new(64));
        state2.per_graph_inflight_limit = 32;
        let state2 = Arc::new(RwLock::new(state2));
        {
            let mut s = state2.write().await;
            let rt_factory = Arc::new(BackendReadThroughFactory::new(backend2.clone()));
            s.registry.set_read_through_factory(rt_factory);
            let materializer = Arc::new(BackendGraphMaterializer::new(backend2.clone()));
            s.registry.set_materializer(materializer);
        }

        let loaded = backend2.load_catalog(&state2).await.unwrap();
        // Only the 8 `boot:N` graphs are durably cataloged (a `graph_meta` row is
        // written on `create`/first write — see `read_all_graph_meta`): the
        // write-side setup above never creates OR writes to "__commons__", so it
        // has no durable footprint to catalog. `__commons__`'s residency below
        // comes from `GraphRegistry::new()` unconditionally pre-creating it
        // in-memory (every fresh registry's baseline, independent of `load_catalog`
        // — see `resident_len() == 1` next), NOT from the durable catalog scan.
        assert_eq!(loaded, 8, "boot:0..7 cataloged");
        assert_eq!(
            state2.read().await.registry.resident_len(),
            1,
            "catalog-only load hydrates NOTHING but the pre-created __commons__"
        );
        for i in 0..8 {
            assert!(state2.read().await.registry.exists(&format!("boot:{i}")));
            assert!(!state2
                .read()
                .await
                .registry
                .is_resident(&format!("boot:{i}")));
        }

        // Touch ONE — it lazily hydrates with its data intact; the rest stay cold.
        let r = dispatch_on_heap(
            &state2,
            req(
                9000,
                "boot:3",
                Method::GetNodeProperties {
                    node_id: "n".into(),
                },
            ),
        )
        .await;
        assert!(r.error.is_none());
        assert!(state2.read().await.registry.is_resident("boot:3"));
        assert!(!state2.read().await.registry.is_resident("boot:4"));

        backend2.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// NE-105 regression: a durable commons graph must not remain the empty
    /// bootstrap resident after restart. Point reads use the durable read-through
    /// seam on a RAM miss, while Cypher/RLS executes against the resident snapshot;
    /// catalog recovery therefore has to demote the synthetic placeholder so both
    /// surfaces hydrate the same committed graph before authorization filtering.
    #[cfg(all(feature = "security", feature = "cypher"))]
    #[tokio::test(flavor = "multi_thread")]
    async fn restart_rehydrates_commons_before_point_and_cypher_rls_reads() {
        let _env_lock = medulla_kernel::crypto::acquire_test_env_lock().await;
        let dir = std::env::temp_dir().join(format!(
            "medulla-ne105-restart-{}-{}",
            std::process::id(),
            NONCE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_s = dir.to_string_lossy().into_owned();

        let backend1: Arc<dyn PersistenceBackend> =
            Arc::new(RedbBackend::open(dir_s.clone(), 64).expect("open write side"));
        let state1 = state_with_backend(&dir_s, backend1.clone(), rls_isolation()).await;

        let public_id = "ne105-public";
        let shared_id = "ne105-shared";
        let private_id = "ne105-private";
        add_rls_node(
            &state1,
            1,
            public_id,
            serde_json::json!({
                "node_type": "Doc",
                "tenant_id": "tenant-shared",
                "name": "public",
                "_shared_scope": "org"
            }),
        )
        .await;
        add_rls_node(
            &state1,
            2,
            shared_id,
            serde_json::json!({
                "node_type": "Doc",
                "tenant_id": "tenant-shared",
                "name": "shared",
                "_owner_id": "alice",
                "_shared_scope": "org"
            }),
        )
        .await;
        add_rls_node(
            &state1,
            3,
            private_id,
            serde_json::json!({
                "node_type": "Doc",
                "tenant_id": "tenant-shared",
                "name": "private",
                "_owner_id": "alice",
                "_shared_scope": "private"
            }),
        )
        .await;
        add_rls_edge(&state1, 4, shared_id, public_id).await;
        assert!(state1
            .read()
            .await
            .registry
            .get("__commons__")
            .unwrap()
            .core
            .has_edge(shared_id, public_id));

        backend1.shutdown();
        drop(state1);
        drop(backend1);

        let backend2: Arc<dyn PersistenceBackend> =
            Arc::new(RedbBackend::open(dir_s.clone(), 64).expect("reopen read side"));
        let state2 = state_with_backend(&dir_s, backend2.clone(), rls_isolation()).await;
        let loaded = backend2.load_catalog(&state2).await.expect("load catalog");
        assert_eq!(loaded, 1, "the commons metadata row must be recovered");
        assert!(
            !state2.read().await.registry.is_resident("__commons__"),
            "restart must not leave the synthetic empty commons core resident"
        );
        assert_eq!(
            state2
                .read()
                .await
                .registry
                .materialization_manifest("__commons__")
                .unwrap()
                .phase,
            medulla_core::registry::MaterializationPhase::CatalogOnly
        );

        // Native point reads and the Cypher projection both hydrate the same
        // durable graph. Alice owns the private row; Bob is a same-tenant peer.
        for (id, node) in [(10, public_id), (11, shared_id), (12, private_id)] {
            let response = dispatch_on_heap(
                &state2,
                req_as_tenant(
                    id,
                    "__commons__",
                    "alice",
                    "tenant-shared",
                    Method::GetNodeProperties {
                        node_id: node.to_string(),
                    },
                ),
            )
            .await;
            assert!(
                response.error.is_none(),
                "alice point read failed: {response:?}"
            );
            assert!(!raw_result(&response).is_empty(), "alice must see {node}");
        }

        let bob_private = dispatch_on_heap(
            &state2,
            req_as_tenant(
                13,
                "__commons__",
                "bob",
                "tenant-shared",
                Method::GetNodeProperties {
                    node_id: private_id.to_string(),
                },
            ),
        )
        .await;
        assert!(bob_private.error.is_none());
        assert!(
            matches!(
                bob_private.result,
                Some(medulla_protocol_types::protocol::ResultPayload::Json(
                    serde_json::Value::Null
                ))
            ),
            "same-tenant peer must not point-read the private row: {bob_private:?}"
        );
        for (id, node) in [(14, public_id), (15, shared_id)] {
            let response = dispatch_on_heap(
                &state2,
                req_as_tenant(
                    id,
                    "__commons__",
                    "bob",
                    "tenant-shared",
                    Method::GetNodeProperties {
                        node_id: node.to_string(),
                    },
                ),
            )
            .await;
            assert!(
                response.error.is_none(),
                "bob point read failed: {response:?}"
            );
            assert!(!raw_result(&response).is_empty(), "bob must see {node}");
        }

        let query_state = Arc::clone(&state2);
        let query = move |id, agent| {
            let state = Arc::clone(&query_state);
            async move {
                dispatch_on_heap(
                    &state,
                    req_as_tenant(
                        id,
                        "__commons__",
                        agent,
                        "tenant-shared",
                        Method::CypherQuery {
                            query: "MATCH (n:Doc) RETURN n".to_string(),
                            mode: medulla_protocol_types::protocol::CypherMode::Read,
                        },
                    ),
                )
                .await
            }
        };
        let alice_rows = query(16, "alice").await;
        assert!(
            alice_rows.error.is_none(),
            "alice Cypher failed: {alice_rows:?}"
        );
        let alice_bytes = raw_result(&alice_rows);
        let alice_text = String::from_utf8_lossy(&alice_bytes);
        for node in [public_id, shared_id, private_id] {
            assert!(
                alice_text.contains(node),
                "owner Cypher omitted {node}: {alice_text}"
            );
        }

        let bob_rows = query(17, "bob").await;
        assert!(bob_rows.error.is_none(), "bob Cypher failed: {bob_rows:?}");
        let bob_bytes = raw_result(&bob_rows);
        let bob_text = String::from_utf8_lossy(&bob_bytes);
        assert!(bob_text.contains(public_id));
        assert!(bob_text.contains(shared_id));
        assert!(
            !bob_text.contains(private_id),
            "RLS leaked private row: {bob_text}"
        );

        let edge_rows = dispatch_on_heap(
            &state2,
            req_as_tenant(
                18,
                "__commons__",
                "bob",
                "tenant-shared",
                Method::CypherQuery {
                    query: "MATCH (a:Doc)-[:RELATED_TO]->(b:Doc) RETURN a, b".to_string(),
                    mode: medulla_protocol_types::protocol::CypherMode::Read,
                },
            ),
        )
        .await;
        assert!(
            edge_rows.error.is_none(),
            "edge Cypher failed: {edge_rows:?}"
        );
        let edge_bytes = raw_result(&edge_rows);
        let edge_text = String::from_utf8_lossy(&edge_bytes);
        assert!(edge_text.contains(shared_id));
        assert!(edge_text.contains(public_id));

        let cross_tenant = dispatch_on_heap(
            &state2,
            req_as_tenant(
                19,
                "__commons__",
                "bob",
                "tenant-foreign",
                Method::CypherQuery {
                    query: "MATCH (n:Doc) RETURN n".to_string(),
                    mode: medulla_protocol_types::protocol::CypherMode::Read,
                },
            ),
        )
        .await;
        assert!(
            cross_tenant.error.is_some(),
            "cross-tenant request must fail closed: {cross_tenant:?}"
        );

        backend2.shutdown();
        drop(state2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Quota enforcement: with a tight resident cap, accessing more cold graphs
    /// than the cap never grows the resident set past it — the coldest is evicted
    /// to admit each new one.
    #[tokio::test(flavor = "multi_thread")]
    async fn quota_bounds_resident_count_under_churn() {
        // Holds the encryption env still for this test's whole body: it opens a
        // durable store, so a concurrent key mutation would break its canary
        // check. A READ guard, so these tests still run concurrently with each
        // other -- only a key MUTATOR is excluded.
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir = std::env::temp_dir().join(format!("medulla-lazy-quota-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();
        let state = redb_state(&dir_s).await;

        let n = 10;
        for i in 0..n {
            create(&state, i as u64, &format!("quota:{i}")).await;
            add_node(&state, 100 + i as u64, &format!("quota:{i}"), "n").await;
        }
        {
            let mut s = state.write().await;
            for i in 0..n {
                s.registry.evict_resident(&format!("quota:{i}"));
            }
        }
        assert_eq!(state.read().await.registry.resident_len(), 1);

        // A cap of 4 (including the pinned __commons__): access all 10 cold graphs
        // one at a time through `lazy_open`, enforcing the cap on every admission.
        let cap = 4usize;
        for i in 0..n {
            let name = format!("quota:{i}");
            let opened =
                lazy_open(&state, &name, cap, DEFAULT_PRODUCTION_LAZY_OPEN_PAGE_SIZE).await;
            assert!(opened, "quota:{i} must lazily open");
            let resident = state.read().await.registry.resident_len();
            assert!(
                resident <= cap,
                "resident count {resident} exceeds cap {cap} after opening quota:{i}"
            );
            // Touch it so it isn't immediately picked as "coldest" by an artifact
            // of insertion order alone (mirrors the dispatch path's cold_tracker.touch).
            state.write().await.ensure_cold_tracker().touch(&name);
        }

        // The cap held throughout — never more than `cap` resident graphs at once,
        // even after opening far more than `cap` distinct cold graphs.
        assert!(state.read().await.registry.resident_len() <= cap);
        // The MOST RECENTLY opened graph must still be resident (LRU protects it).
        assert!(state
            .read()
            .await
            .registry
            .is_resident(&format!("quota:{}", n - 1)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pins [`offload_cold_tenants`]'s R6 idle sweep end to end: every resident graph
    /// touched at least `idle_window` ago is durability-gated and evicted to
    /// catalog-only (never merely hibernated — `is_resident` must go false, not just
    /// `node_count`), `__commons__` is NEVER swept even though it is just as cold, and
    /// the tracker's offload bookkeeping (`offloaded_total`, `is_offloaded_for`)
    /// reflects exactly what was evicted. `Duration::ZERO` makes "touched, then never
    /// touched again" cold without a real sleep — any elapsed time already satisfies
    /// `>= idle_window`.
    #[tokio::test(flavor = "multi_thread")]
    async fn cold_sweep_evicts_idle_graphs_but_never_commons() {
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir = std::env::temp_dir().join(format!("medulla-cold-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir_s = dir.to_string_lossy().to_string();
        let state = redb_state(&dir_s).await;

        create(&state, 1, "cold:a").await;
        create(&state, 2, "cold:b").await;
        add_node(&state, 10, "cold:a", "seed").await;
        add_node(&state, 11, "cold:b", "seed").await;
        assert!(state.read().await.registry.is_resident("cold:a"));
        assert!(state.read().await.registry.is_resident("cold:b"));
        assert!(state.read().await.registry.is_resident("__commons__"));

        let tracker = state.write().await.ensure_cold_tracker();
        let offloaded =
            super::offload_cold_tenants(&state, &tracker, std::time::Duration::ZERO).await;

        assert_eq!(offloaded, 2, "both idle graphs offloaded, commons excluded");
        assert!(
            !state.read().await.registry.is_resident("cold:a"),
            "cold:a evicted to catalog-only, not merely hibernated"
        );
        assert!(
            !state.read().await.registry.is_resident("cold:b"),
            "cold:b evicted to catalog-only, not merely hibernated"
        );
        assert!(
            state.read().await.registry.is_resident("__commons__"),
            "__commons__ is never offloaded by the idle sweep, however cold"
        );
        assert_eq!(tracker.offloaded_total(), 2);
        assert!(tracker.is_offloaded("cold:a"));
        assert!(tracker.is_offloaded("cold:b"));

        // A repeat sweep offloads nothing more: both are already catalog-only (no
        // resident core to drop) and already marked offloaded.
        let offloaded_again =
            super::offload_cold_tenants(&state, &tracker, std::time::Duration::ZERO).await;
        assert_eq!(
            offloaded_again, 0,
            "nothing left to offload on a repeat sweep"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
