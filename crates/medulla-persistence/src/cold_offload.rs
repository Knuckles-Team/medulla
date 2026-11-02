//! Cold-tenant whole-graph offload (CONCEPT:MEDULLA-KG.sharding.medulla-r6, M3).
//!
//! ## What it solves
//!
//! KG-2.191 read-through bounds RAM at NODE granularity (an evicted node serves from
//! redb). KG-2.234's budget enforcer reclaims the COLDEST graphs only once a tenant is
//! over its memory budget. Neither bounds RAM for the long tail of tenants that are simply
//! IDLE: with 100M tenants, a graph untouched for hours should not keep its whole in-RAM
//! topology/props/vectors resident just because the box is under its global budget.
//!
//! This adds time-windowed WHOLE-graph offload: a graph not accessed for longer than a
//! window is hibernated (its in-RAM state dropped via [`GraphCore::hibernate`], the SAME
//! primitive the budget enforcer uses) while its durable redb rows are retained, so reads
//! serve through the existing KG-2.191 node read-through and a full topology rebuild
//! rehydrates from the durable dump on demand. It complements — does not replace — the
//! budget-pressure path: this is proactive (idle-driven), that is reactive (budget-driven).
//!
//! Offload is durability-gated: every resident node must be confirmed in the
//! authoritative backend before the in-RAM core is dropped. An offloaded graph is
//! therefore never lost, only evicted.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::RwLock;

use medulla_core::registry::GraphHandle;
use medulla_graph_core::graph::GraphCore;
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::state::ServerState;

/// Identity for one access/offload tracker entry.  The name is only a routing
/// key; the incarnation is the authority boundary.  A same-name recreate must
/// never inherit the prior incarnation's recency or offloaded mark.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TrackerKey {
    graph: String,
    incarnation_id: Option<String>,
}

/// Typed access to the type-erased cold-tenant tracker held by `ServerState`
/// (MEDULLA-CONTRACT-R056.8).
pub trait ColdTrackerStateExt {
    /// The installed tracker, if startup (or `ensure_cold_tracker`) created one.
    fn cold_tracker(&self) -> Option<Arc<ColdTenantTracker>>;
    /// Install the tracker.
    fn set_cold_tracker(&mut self, tracker: Arc<ColdTenantTracker>);
    /// The installed tracker, creating an empty one when none is installed yet.
    fn ensure_cold_tracker(&mut self) -> Arc<ColdTenantTracker>;
}

impl ColdTrackerStateExt for ServerState {
    fn cold_tracker(&self) -> Option<Arc<ColdTenantTracker>> {
        self.cold_tracker
            .as_ref()
            .and_then(|h| Arc::clone(h).downcast::<ColdTenantTracker>().ok())
    }

    fn set_cold_tracker(&mut self, tracker: Arc<ColdTenantTracker>) {
        self.cold_tracker = Some(tracker);
    }

    fn ensure_cold_tracker(&mut self) -> Arc<ColdTenantTracker> {
        if let Some(existing) = self.cold_tracker() {
            return existing;
        }
        let tracker = Arc::new(ColdTenantTracker::default());
        self.set_cold_tracker(Arc::clone(&tracker));
        tracker
    }
}

/// Per-incarnation last-access tracker + offload bookkeeping
/// (CONCEPT:MEDULLA-KG.sharding.medulla-r6). The engine calls
/// [`touch_with_incarnation`](ColdTenantTracker::touch_with_incarnation) on every
/// read/write of a graph; the periodic [`offload_cold_tenants`] sweep reads it
/// to pick idle graphs. Cheap relaxed atomics + two mutex-guarded maps stay off
/// the per-op graph hot path (a touch is one map upsert).
#[derive(Default)]
pub struct ColdTenantTracker {
    last_access: Mutex<HashMap<TrackerKey, Instant>>,
    /// Graphs currently offloaded (drives the offloaded-vs-resident split + avoids
    /// re-offloading a still-cold graph every sweep).
    offloaded: Mutex<HashSet<TrackerKey>>,
    offloaded_total: AtomicU64,
}

impl ColdTenantTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `graph` was just accessed (read or write). Clears any offloaded mark —
    /// a graph that is touched again has rehydrated and is resident.
    pub fn touch(&self, graph: &str) {
        self.touch_key(graph, None);
    }

    /// Record an access for an exact registry incarnation.  Served dispatches
    /// use this form; the name-only [`Self::touch`] remains as a compatibility
    /// adapter for embedded/test callers that do not expose lifecycle ids.
    pub fn touch_with_incarnation(&self, graph: &str, incarnation_id: &str) {
        self.touch_key(graph, Some(incarnation_id.to_string()));
    }

    fn touch_key(&self, graph: &str, incarnation_id: Option<String>) {
        let key = TrackerKey {
            graph: graph.to_string(),
            incarnation_id,
        };
        let mut access = self.last_access.lock();
        // One name has at most one current registry incarnation.  Drop any
        // retired access clocks before recording the new one so a recreate
        // cannot inherit or expose stale recency through the name-only view.
        access.retain(|entry, _| entry.graph.as_str() != graph);
        access.insert(key, Instant::now());
        drop(access);
        let mut off = self.offloaded.lock();
        // A touch rehydrates the current identity; retired offload marks are
        // never meaningful for the same name after that transition.
        off.retain(|entry| entry.graph.as_str() != graph);
    }

    /// Graphs whose last access is older than `idle_window` (the offload candidates). A
    /// graph never touched is NOT a candidate (no access timestamp = nothing tracked yet).
    pub fn cold_graphs(&self, idle_window: Duration) -> Vec<String> {
        let now = Instant::now();
        let mut cold = HashSet::new();
        for (key, timestamp) in self.last_access.lock().iter() {
            if now.duration_since(*timestamp) >= idle_window {
                cold.insert(key.graph.clone());
            }
        }
        cold.into_iter().collect()
    }

    fn mark_offloaded(&self, handle: &GraphHandle) {
        let key = TrackerKey {
            graph: handle.name.clone(),
            incarnation_id: Some(handle.incarnation_id.clone()),
        };
        if self.offloaded.lock().insert(key) {
            self.offloaded_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Is this graph currently marked offloaded?
    pub fn is_offloaded(&self, graph: &str) -> bool {
        self.offloaded
            .lock()
            .iter()
            .any(|entry| entry.graph.as_str() == graph)
    }

    /// Is this exact graph incarnation currently marked offloaded?
    pub fn is_offloaded_for(&self, graph: &str, incarnation_id: &str) -> bool {
        self.offloaded.lock().contains(&TrackerKey {
            graph: graph.to_string(),
            incarnation_id: Some(incarnation_id.to_string()),
        })
    }

    /// Is this exact graph incarnation older than the idle window?  A legacy
    /// wildcard touch is accepted only as a compatibility fallback when no
    /// exact touch exists for the incarnation.
    pub fn is_cold_for(&self, graph: &str, incarnation_id: &str, idle_window: Duration) -> bool {
        let now = Instant::now();
        let access = self.last_access.lock();
        let exact = access.get(&TrackerKey {
            graph: graph.to_string(),
            incarnation_id: Some(incarnation_id.to_string()),
        });
        let legacy = access.get(&TrackerKey {
            graph: graph.to_string(),
            incarnation_id: None,
        });
        exact
            .or(legacy)
            .is_some_and(|timestamp| now.duration_since(*timestamp) >= idle_window)
    }

    /// Cumulative graphs offloaded by the cold sweep.
    pub fn offloaded_total(&self) -> u64 {
        self.offloaded_total.load(Ordering::Relaxed)
    }

    /// Forget a graph entirely (e.g. on `DeleteGraph`) so its access timestamp + offload
    /// mark don't leak across a same-name recreate.
    pub fn forget(&self, graph: &str) {
        self.last_access
            .lock()
            .retain(|key, _| key.graph.as_str() != graph);
        self.offloaded
            .lock()
            .retain(|key| key.graph.as_str() != graph);
        #[cfg(feature = "cost")]
        crate::cost::forget_graph(graph);
    }

    /// Forget only one incarnation, preserving access state for a concurrently
    /// recreated graph with the same name.
    pub fn forget_incarnation(&self, graph: &str, incarnation_id: &str) {
        let matches = |key: &TrackerKey| {
            key.graph.as_str() == graph && key.incarnation_id.as_deref() == Some(incarnation_id)
        };
        self.last_access.lock().retain(|key, _| !matches(key));
        self.offloaded.lock().retain(|key| !matches(key));
        #[cfg(feature = "cost")]
        crate::cost::forget_graph_incarnation(graph, incarnation_id);
    }

    /// Pick the COLDEST (least-recently-touched) graph among `candidates`
    /// (CONCEPT:MEDULLA-KG.sharding.lazy-graph-catalog — bounded hot-context cache admission control, DIST-P2-3).
    /// A candidate this tracker has no access record for sorts FIRST (treated as
    /// infinitely cold — e.g. a graph that was just lazily opened and has not yet
    /// been `touch`ed on the dispatch path), ahead of one that has been genuinely
    /// active. `None` when `candidates` is empty.
    pub fn coldest(&self, candidates: impl Iterator<Item = String>) -> Option<String> {
        let last_access = self.last_access.lock();
        candidates.min_by_key(|graph| {
            // Use the newest touch for the currently named graph.  Older
            // incarnations can remain in the map briefly during a lifecycle
            // race, but must not make a freshly active recreate look cold.
            last_access
                .iter()
                .filter(|(key, _)| key.graph.as_str() == graph.as_str())
                .map(|(_, timestamp)| *timestamp)
                .max()
        })
    }
}

/// Offload one resident graph's whole in-RAM state, durability-gated (CONCEPT:MEDULLA-KG.sharding.medulla-r6).
/// Returns the freed node count, or `None` when it was NOT safe to drop (durability could
/// not be confirmed — the graph stays resident, no loss). Reuses [`GraphCore::hibernate`]
/// (KG-2.224) so reads serve via the KG-2.191 read-through afterward.
pub fn offload_graph_core(
    core: &GraphCore,
    backend: &Arc<dyn PersistenceBackend>,
    graph_name: &str,
) -> Option<usize> {
    if core.node_count() == 0 {
        return Some(0); // already empty / hibernated
    }
    // The topology is the authoritative resident-node set.  `get_nodes()` is
    // the property projection and may omit a topology resident while a
    // partially materialized/recovered core is being reconciled.  Never let a
    // durability proof silently skip such an id: hibernating in that state
    // would discard the topology-only projection and make the graph appear
    // complete in RAM after the durable check.
    let node_ids = core.node_ids();
    let property_ids: HashSet<String> = core
        .get_nodes()
        .into_iter()
        .map(|(node_id, _)| node_id)
        .collect();
    if node_ids.len() != core.node_count()
        || property_ids.len() != node_ids.len()
        || node_ids
            .iter()
            .any(|node_id| !property_ids.contains(node_id))
    {
        return None;
    }
    let fname = medulla_kernel::persist::sanitize(graph_name);
    let presence = backend.durable_node_presence(&fname, &node_ids).ok()?;
    if presence.len() != node_ids.len() || presence.iter().any(|present| !present) {
        return None;
    }
    Some(core.hibernate())
}

/// Sweep the registry and offload every graph idle longer than `idle_window`
/// (CONCEPT:MEDULLA-KG.sharding.medulla-r6). Returns the number of graphs offloaded. The shared `__commons__`
/// graph is never offloaded (every agent needs it hot). No-op when `idle_window` is huge
/// / nothing is cold. Pure-Rust; reuses the hibernate + read-through path, so it never
/// loses data.
pub async fn offload_cold_tenants(
    state: &Arc<RwLock<ServerState>>,
    tracker: &ColdTenantTracker,
    idle_window: Duration,
) -> u64 {
    let cold: HashSet<String> = tracker.cold_graphs(idle_window).into_iter().collect();
    if cold.is_empty() {
        return 0;
    }

    // Snapshot generation-safe resident handles + durable authority under a
    // read lock.  The handle, rather than only `(name, Arc<GraphCore>)`, is the
    // fence used when the durable check publishes its eviction.
    let (entries, persistence) = {
        let s = state.read().await;
        let entries: Vec<GraphHandle> = s
            .registry
            .all_entries()
            .iter()
            .filter_map(|e| s.registry.handle(&e.name))
            .collect();
        (entries, s.persistence.clone())
    };
    let Some(_persistence) = persistence else {
        return 0;
    };

    let mut offloaded = 0u64;
    for handle in &entries {
        if offload_one_cold_tenant(state, tracker, &cold, idle_window, handle).await {
            offloaded += 1;
        }
    }
    offloaded
}

/// One candidate's worth of [`offload_cold_tenants`]: still cold and current, durably
/// confirmed empty-able, and actually freed. Returns whether it was offloaded (the
/// registry's residency entry evicted). Split out so the sweep loop stays a single
/// early-return dispatch instead of nested conditionals four deep.
async fn offload_one_cold_tenant(
    state: &Arc<RwLock<ServerState>>,
    tracker: &ColdTenantTracker,
    cold: &HashSet<String>,
    idle_window: Duration,
    handle: &GraphHandle,
) -> bool {
    let name = &handle.name;
    if name.as_str() == "__commons__"
        || !cold.contains(name)
        || !tracker.is_cold_for(name, &handle.incarnation_id, idle_window)
        || tracker.is_offloaded_for(name, &handle.incarnation_id)
    {
        return false;
    }
    // Lifecycle work and ordinary writes share this lane.  This prevents
    // a delete/recreate or concurrent write from changing the durable
    // authority between the presence check and hibernation.
    let _lifecycle_guard = medulla_server_api::routed_write_coalescer::lock_graph(name).await;
    let persistence = {
        let s = state.read().await;
        if !s.registry.is_current_handle(handle) {
            return false;
        }
        s.persistence.clone()
    };
    let Some(persistence) = persistence else {
        return false;
    };
    if handle.core.node_count() == 0 {
        return false; // nothing resident to drop
    }
    let Some(freed) = offload_graph_core(&handle.core, &persistence, name) else {
        return false;
    };
    if freed == 0 {
        return false;
    }
    // U-148 / BUG-130: `offload_graph_core` only drops this graph's RAM
    // (`GraphCore::hibernate`) — it never removes the registry's residency
    // entry. Left as-is, `is_resident(name)` stays `true` with zero
    // topology, so the lazy-open dispatch path never rehydrates it and
    // every whole-graph read (Cypher, counts, traversal) observes a
    // permanently empty snapshot. Mirror `admit_capacity`'s already-correct
    // pattern: transition the now durability-confirmed-empty entry to
    // catalog-only so the NEXT access takes the existing bounded durable
    // lazy-open instead.
    let mut s = state.write().await;
    if s.registry.evict_resident_if_current(handle) {
        tracker.mark_offloaded(handle);
        true
    } else {
        false
    }
}

// ── Bounded hot-context cache: admission control + lazy open (CONCEPT:MEDULLA-KG.sharding.lazy-graph-catalog,
// DIST-P2-3) ─────────────────────────────────────────────────────────────────────────
//
// The registry's catalog (`GraphRegistry::register_catalog_only`/`open_lazy`) lets
// the engine KNOW about millions of graphs at the cost of a metadata row each. This
// section bounds how many of them are simultaneously RESIDENT (a real `GraphCore`):
// a cap on hot contexts, enforced by evicting the coldest resident graph — by this
// tracker's last-access recency, reusing the SAME durability-gated `offload_graph_core`
// hibernate path R6 cold-offload uses — before a new one is admitted. `__commons__`
// is never evicted.

pub const DEFAULT_PRODUCTION_MAX_RESIDENT_GRAPHS: usize = 1024;
pub const DEFAULT_PRODUCTION_LAZY_OPEN_PAGE_SIZE: usize = 4096;

/// The configured finite cap on resident hot-context graphs.
pub fn max_resident_graphs() -> usize {
    medulla_server_api::state::positive_runtime_limit_from_env(
        "MEDULLA_MAX_RESIDENT_GRAPHS",
        DEFAULT_PRODUCTION_MAX_RESIDENT_GRAPHS,
    )
}

/// Admit a new/lazily-opened graph into the bounded hot-context cache
/// (CONCEPT:MEDULLA-KG.sharding.lazy-graph-catalog): while the resident set is AT `cap`, evict the
/// COLDEST resident graph (`__commons__` and `incoming` itself excluded) —
/// durability-gated via `offload_graph_core` (the same no-loss hibernate path the
/// R6 idle sweep and the byte-budget enforcer use) — REMOVING it from the resident
/// map entirely (`GraphRegistry::evict_resident`, not merely `hibernate`, so the
/// freed memory includes the `GraphCore` structures themselves, not just its
/// content). The catalog row survives, so the evicted graph re-opens on its next
/// access. Takes the cap as an explicit parameter so tests can exercise a tight cap
/// deterministically without mutating global process environment.
pub fn admit_capacity(
    s: &mut ServerState,
    tracker: &ColdTenantTracker,
    incoming: &str,
    cap: usize,
) {
    let cap = cap.max(1);
    while s.registry.resident_len() >= cap {
        let Some(persistence) = s.persistence.clone() else {
            return;
        };
        let candidate_names: Vec<String> = s
            .registry
            .all_entries()
            .iter()
            .filter(|e| e.name != "__commons__" && e.name != incoming)
            .map(|e| e.name.clone())
            .collect();
        let victim = match tracker.coldest(candidate_names.into_iter()) {
            Some(v) => v,
            None => return, // nothing evictable (only __commons__/incoming resident)
        };
        let handle = match s.registry.handle(&victim) {
            Some(handle) => handle,
            None => return, // race: victim already gone
        };
        if offload_graph_core(&handle.core, &persistence, &victim).is_none() {
            return;
        }
        if !s.registry.evict_resident_if_current(&handle) {
            return;
        }
        tracker.forget(&victim);
    }
}

/// The configured positive page size for a paged lazy-open.
/// [`medulla_core::registry::GraphRegistry::open_lazy_paged`]: the graph becomes resident
/// after one bounded page (nodes+edges combined, per page), instead of after a full
/// rehydrate. Operations receive explicit partial-materialization state until all
/// pages and maintained indexes are valid — the concrete fix for the honest limitation
/// `docs/architecture/epistemic-os-hardening.md` names as open ledger item L38
/// ("first access to a lazily-opened graph still fully rehydrates it"). The rest of
/// the graph's material is paged in by [`page_in_remaining`] as a background task;
/// no incomplete whole-graph result is served in the interim.
pub fn lazy_open_page_size() -> usize {
    medulla_server_api::state::positive_runtime_limit_from_env(
        "MEDULLA_LAZY_OPEN_PAGE_SIZE",
        DEFAULT_PRODUCTION_LAZY_OPEN_PAGE_SIZE,
    )
}

/// Lazily materialize a catalog-known graph's resident `GraphCore` on first access
/// (CONCEPT:MEDULLA-KG.sharding.lazy-graph-catalog). A no-op if the graph is ALREADY resident (the common
/// case on the hot path — checked by the caller BEFORE taking the write lock this
/// function needs, so an already-resident graph never pays for one) or genuinely
/// unknown (neither cataloged nor resident — the caller's subsequent registry
/// lookup then reports "not found", unchanged from pre-DIST-P2-3 behavior).
/// Applies bounded-cache admission (`admit_capacity`) BEFORE materializing so the
/// resident set never exceeds `cap`. Returns `true` when the graph is resident
/// afterward.
///
/// `page_size` (CONCEPT:MEDULLA-KG.sharding.paged-lazy-open, L38 "paged adjacency") — resolved by the
/// caller via [`lazy_open_page_size`], same convention as `cap`/[`max_resident_graphs`]
/// (an explicit parameter, not read from the environment HERE, so tests can exercise a
/// specific page size deterministically without mutating global process env state).
/// The positive page size materializes only one bounded page inline and spawns
/// [`page_in_remaining`] to fetch the rest off the request path. The graph is resident
/// but explicitly partial on return; a paged open never blocks the triggering request
/// on a full rehydrate of a very large graph.
///
/// Gated on `redb`: the ONLY caller (`dispatch.rs`'s durable-read path) invokes this
/// from inside its own `#[cfg(feature = "redb")]` block, and the body reads
/// `ServerState::cold_tracker`, which only exists on a `redb` build.
#[cfg(feature = "redb")]
pub async fn lazy_open(
    state: &Arc<RwLock<ServerState>>,
    graph_name: &str,
    cap: usize,
    page_size: usize,
) -> bool {
    // The same striped per-graph lane used by Create/Delete/MutationBatch fences
    // lifecycle work without serializing unrelated graphs. Durable page reads run
    // outside the server-wide state lock.
    let _lifecycle_guard = medulla_server_api::routed_write_coalescer::lock_graph(graph_name).await;
    let ticket = {
        let s = state.read().await;
        if s.registry.is_resident(graph_name) {
            return true;
        }
        if !s.registry.exists(graph_name) {
            return false;
        }
        match s.registry.prepare_lazy_open(graph_name) {
            Some(ticket) => ticket,
            None => return false,
        }
    };

    let page_size = page_size.max(1);
    let fetch_ticket = ticket.clone();
    let page =
        match tokio::task::spawn_blocking(move || fetch_ticket.materialize_page(None, page_size))
            .await
        {
            Ok(page) => page,
            Err(_) => return false,
        };
    let mut s = state.write().await;
    let tracker = s.ensure_cold_tracker();
    admit_capacity(&mut s, &tracker, graph_name, cap);
    let outcome = s.registry.publish_lazy_first_page(&ticket, page);
    if !outcome.resident {
        return false;
    }
    if let Some(cursor) = outcome.cursor {
        drop(s);
        tokio::spawn(page_in_remaining(state.clone(), ticket, cursor, page_size));
    }
    true
}

/// Background continuation of a paged [`lazy_open`] (CONCEPT:MEDULLA-KG.sharding.paged-lazy-open, L38): keeps
/// calling [`medulla_core::registry::GraphRegistry::page_in`] until the cursor is exhausted
/// (the graph's ENTIRE durable material has landed — the "no full rehydrate on first
/// touch, load on demand" property, just spread across bounded off-request-path
/// steps instead of one inline blocking fetch). Stops early, as a benign no-op, if the
/// graph is evicted (cold-offloaded) or deleted out from under it before paging
/// finishes — there is nothing left to continue into, and the NEXT lazy-open (paged
/// or not) starts a fresh rehydrate from the durable tier regardless. Never awaited by
/// a caller; entirely off the request path.
///
async fn page_in_remaining(
    state: Arc<RwLock<ServerState>>,
    ticket: medulla_core::registry::LazyOpenTicket,
    mut cursor: medulla_core::registry::MaterializeCursor,
    page_size: usize,
) {
    loop {
        let _lifecycle_guard = medulla_server_api::routed_write_coalescer::lock_graph(ticket.name()).await;
        if ticket.is_cancelled() {
            return;
        }
        let fetch_ticket = ticket.clone();
        let fetch_cursor = cursor;
        let page = match tokio::task::spawn_blocking(move || {
            fetch_ticket.materialize_page(Some(fetch_cursor), page_size)
        })
        .await
        {
            Ok(Some(page)) => page,
            _ => return,
        };
        let (handle, manifest) = {
            let s = state.read().await;
            let Some(handle) = s.registry.handle(ticket.name()) else {
                return;
            };
            let Some(manifest) = s.registry.materialization_handle(ticket.name()) else {
                return;
            };
            (handle, manifest)
        };
        let next = medulla_core::registry::GraphRegistry::apply_lazy_page_to_handle(
            &ticket, &handle, &manifest, page,
        );
        match next {
            Some(c) => cursor = c,
            None => {
                // `None` is either complete or a failed/mixed source snapshot.
                // A failed partial core is evicted so the next access restarts
                // from page zero; it is never advertised as complete.
                let mut s = state.write().await;
                if s.registry
                    .materialization_manifest(ticket.name())
                    .is_some_and(|manifest| {
                        manifest.phase == medulla_core::registry::MaterializationPhase::Failed
                    })
                {
                    s.registry.evict_resident_if_current(&handle);
                }
                return;
            }
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub async fn test_state_with_backend(
    secret: &str,
    dir_s: &str,
    backend: Arc<dyn PersistenceBackend>,
    isolation: medulla_isolation::isolation::IsolationLayer,
) -> Arc<RwLock<ServerState>> {
    use crate::read_through::BackendGraphMaterializer;
    use crate::read_through::BackendReadThroughFactory;

    let mut state = ServerState::new_for_test(secret, isolation);
    state.persist_dir = Some(dir_s.to_string());
    state.persistence = Some(backend.clone());
    state.max_in_flight = Arc::new(tokio::sync::Semaphore::new(64));
    state.read_admission = Arc::new(tokio::sync::Semaphore::new(64));
    state.per_graph_inflight_limit = 32;
    let state = Arc::new(RwLock::new(state));
    {
        let mut s = state.write().await;
        let rt_factory = Arc::new(BackendReadThroughFactory::new(backend.clone()));
        s.registry.set_read_through_factory(rt_factory);
        let materializer = Arc::new(BackendGraphMaterializer::new(backend));
        s.registry.set_materializer(materializer);
    }
    state
}

#[cfg(any(test, feature = "test-support"))]
pub async fn test_redb_state(
    secret: &str,
    dir_s: &str,
    isolation: medulla_isolation::isolation::IsolationLayer,
    page_size: usize,
) -> Arc<RwLock<ServerState>> {
    use crate::redb_backend::RedbBackend;

    let backend: Arc<dyn PersistenceBackend> =
        Arc::new(RedbBackend::open(dir_s.to_string(), page_size).expect("open"));
    test_state_with_backend(secret, dir_s, backend, isolation).await
}
