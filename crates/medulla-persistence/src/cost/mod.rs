//! Per-tenant memory budget + autoscale signals (CONCEPT:MEDULLA-KG.compute.lane-v, Lane V —
//! cost/efficiency).
//!
//! The engine already bounds memory per *graph* (`MEDULLA_MAX_NODES_PER_GRAPH`
//! LRU eviction, KG-2.191) and can drop a cold *graph* entirely (hibernation,
//! KG-2.224). This module adds the *tenant* dimension and a memory-byte budget on top:
//!
//! * **Per-tenant memory budget.** A tenant owns one or more graphs (the tenant is
//!   derived from the graph name — the prefix before the first `:`, e.g. `agent` for
//!   `agent:planner`, or the whole name when there is no `:`). The enforcer tracks an
//!   approximate resident-RAM estimate per graph ([`GraphCore::memory_estimate`]) and
//!   sums it per tenant. When a tenant exceeds its byte budget, its COLDEST graphs
//!   (least node activity) are reclaimed — first by durability-gated LRU eviction
//!   (reusing [`crate::persist::evict_oversized_all`]'s per-graph path), then, if a
//!   graph is fully reducible, by hibernation ([`GraphCore::hibernate`]) — until the
//!   tenant is back under budget. Data is never lost: eviction is durability-gated and
//!   hibernation drops only RAM (the durable redb tier + the read-through seam stay
//!   intact, so an evicted/hibernated node rehydrates on access).
//!
//! * **Global ceiling + fair caps.** A process-wide memory ceiling caps total resident
//!   RAM; a fair per-tenant cap (`ceiling / max(active_tenants, 1)`) is the floor under
//!   each tenant's budget, so one hot tenant cannot consume the whole ceiling and
//!   starve others. A tenant's effective budget is `min(its configured budget, the
//!   fair share)` once the global ceiling is under pressure.
//!
//! * **Autoscale signals.** [`collect_resource_stats`] gathers a [`ResourceSnapshot`]
//!   (per-graph + per-tenant + process aggregate) for `ResourceStatsPage` and the
//!   Prometheus metrics endpoint — the signals an external autoscaler (agent-utilities
//!   OS-5.27) consumes.
//!
//! Pure-Rust: the enforcer is a periodic sweep over the registry reusing existing
//! evict/hibernate ops — no DataFusion / openraft / object_store. Pi-safe.

use std::cmp::Ordering as CmpOrdering;
#[cfg(test)]
use std::collections::BinaryHeap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use medulla_server_api::state::ServerState;
use medulla_protocol_types::result_contract::coordination::{GraphResourceStats, ResourceSnapshot};

mod resource_stats;

/// Derive the tenant a graph belongs to (CONCEPT:MEDULLA-KG.compute.lane-v). The convention across the
/// ecosystem is `tenant:scope` graph names (`agent:planner`, `team:research`,
/// `enterprise-acme:billing`); the tenant is the prefix before the FIRST `:`. A name
/// with no `:` (e.g. `__commons__`) is its own tenant. This is the one place the
/// mapping lives, so a future explicit tenant catalog only changes here.
pub fn tenant_of(graph_name: &str) -> &str {
    match graph_name.split_once(':') {
        Some((tenant, _)) => tenant,
        None => graph_name,
    }
}

/// Process-global cost bookkeeping (CONCEPT:MEDULLA-KG.compute.lane-v): which graphs the enforcer has
/// hibernated (so the snapshot reports resident-vs-hibernated accurately even though a
/// hibernated graph just looks like an empty one), and a monotonic eviction counter for
/// the rate signal. A singleton (like `metrics::SEEN_GRAPHS`) so it needs no ServerState
/// field / constructor churn — hibernation status + eviction totals are process-global.
#[derive(Default)]
struct CostState {
    /// Graphs the budget enforcer has hibernated (drives the resident/hibernated split).
    hibernated: Mutex<std::collections::HashSet<GraphIdentity>>,
    /// Total LRU nodes evicted by the budget enforcer (the rate is its delta over time).
    evicted_total: std::sync::atomic::AtomicU64,
    /// Total graphs hibernated by the budget enforcer.
    hibernated_total: std::sync::atomic::AtomicU64,
}

/// Identity for process-global budget bookkeeping.  Name-only state survives
/// a delete/recreate and makes a fresh graph appear hibernated; the immutable
/// registry incarnation is therefore part of every state transition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GraphIdentity {
    name: String,
    incarnation_id: String,
}

fn cost_state() -> &'static CostState {
    use std::sync::OnceLock;
    static STATE: OnceLock<CostState> = OnceLock::new();
    STATE.get_or_init(CostState::default)
}

/// Mark one exact graph incarnation hibernated / resident in the process-global
/// bookkeeping.
pub fn note_hibernated(graph: &str, incarnation_id: &str, hibernated: bool) {
    let mut set = cost_state().hibernated.lock();
    let identity = GraphIdentity {
        name: graph.to_string(),
        incarnation_id: incarnation_id.to_string(),
    };
    if hibernated {
        set.insert(identity);
        cost_state()
            .hibernated_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    } else {
        set.remove(&identity);
    }
    let any_hibernated = set.iter().any(|entry| entry.name.as_str() == graph);
    medulla_kernel::metrics::set_graph_hibernated(graph, any_hibernated);
}

/// Is this exact graph incarnation currently hibernated by the enforcer?
pub fn is_hibernated(graph: &str, incarnation_id: &str) -> bool {
    cost_state().hibernated.lock().contains(&GraphIdentity {
        name: graph.to_string(),
        incarnation_id: incarnation_id.to_string(),
    })
}

/// Clear all cost bookkeeping for a deleted name.  The cold tracker calls this
/// from the lifecycle teardown path so no name-keyed state survives a recreate.
pub fn forget_graph(graph: &str) {
    cost_state()
        .hibernated
        .lock()
        .retain(|identity| identity.name.as_str() != graph);
    // Deletion/admission teardown must not recreate a label series after the
    // caller has dropped the graph's metrics.  `drop_graph` is idempotent and
    // also frees the bounded label slot.
    medulla_kernel::metrics::drop_graph(graph);
}

/// Clear one incarnation without touching a concurrently recreated identity.
pub fn forget_graph_incarnation(graph: &str, incarnation_id: &str) {
    let mut hibernated = cost_state().hibernated.lock();
    hibernated.remove(&GraphIdentity {
        name: graph.to_string(),
        incarnation_id: incarnation_id.to_string(),
    });
    if !hibernated
        .iter()
        .any(|identity| identity.name.as_str() == graph)
    {
        medulla_kernel::metrics::drop_graph(graph);
    }
}

// ── Configuration ─────────────────────────────────────────────────────────

/// Per-tenant memory budget configuration (CONCEPT:MEDULLA-KG.compute.lane-v), read once at startup.
#[derive(Debug, Clone, Copy)]
pub struct CostConfig {
    /// Process-wide resident-memory ceiling, bytes.
    pub global_ceiling_bytes: u64,
    /// Default per-tenant budget, bytes. A tenant may never exceed this; under global
    /// pressure its effective budget is further capped to the fair share.
    pub per_tenant_budget_bytes: u64,
    /// Sweep interval, seconds.
    pub interval_secs: u64,
}

/// Effective memory available to this process. The shared `medulla-resource` resolver
/// already intersects host RAM with cgroup v1/v2 and reserves RAM headroom, so
/// this module must not maintain a second parser.
fn system_memory_limit_bytes() -> Option<u64> {
    Some(medulla_kernel::autosize::detect_capacity().reserved_ram_bytes())
}

impl CostConfig {
    /// Resolve from the environment (CONCEPT:MEDULLA-KG.compute.lane-v). ONE knob is enough to turn it on
    /// with a sensible auto-sized default:
    ///
    /// * `MEDULLA_MEMORY_BUDGET` — the global resident-memory ceiling. Accepts a
    ///   plain byte count OR a `k`/`m`/`g` suffix (`512m`, `2g`). When UNSET it AUTO-SIZES
    ///   to 40% of the cgroup-aware effective RAM. Explicit values may lower the
    ///   automatic ceiling but are clamped to it, so an override cannot widen a
    ///   constrained process beyond its measured budget.
    /// * `MEDULLA_TENANT_BUDGET` — optional per-tenant budget override. Defaults
    ///   to the global ceiling (so a single-tenant box budgets against the ceiling); the
    ///   fair-share cap still applies under multi-tenant pressure regardless.
    /// * `MEDULLA_BUDGET_INTERVAL` — sweep cadence, seconds (default 15).
    pub fn from_env() -> Result<Self, String> {
        let automatic_ceiling = system_memory_limit_bytes()
            .map(|total| (total / 10 * 4).max(1))
            .unwrap_or(512 * 1024 * 1024 / 10 * 4);
        // A shared engine leaves most memory available to its host process,
        // filesystem cache, and peer services. Operators of a dedicated node
        // can explicitly raise these values, up to the cgroup-aware ceiling.
        let global_ceiling_bytes = bounded_bytes_env("MEDULLA_MEMORY_BUDGET", automatic_ceiling)?;
        let per_tenant_budget_bytes =
            bounded_bytes_env("MEDULLA_TENANT_BUDGET", global_ceiling_bytes)?;
        let interval_secs = match std::env::var("MEDULLA_BUDGET_INTERVAL") {
            Ok(value) => value
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|interval| (1..=3_600).contains(interval))
                .ok_or_else(|| "MEDULLA_BUDGET_INTERVAL must be between 1 and 3600".to_string())?,
            Err(std::env::VarError::NotPresent) => 15,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err("MEDULLA_BUDGET_INTERVAL is not valid Unicode".to_string())
            }
        };
        Ok(CostConfig {
            global_ceiling_bytes,
            per_tenant_budget_bytes,
            interval_secs,
        })
    }
}

/// Read a byte-count budget from `name`, clamped to `ceiling`. Absent means the
/// ceiling itself; a present-but-unparseable, zero or non-Unicode value is an
/// error, never a silent default. Both configured budgets are this one rule —
/// only their ceiling differs.
fn bounded_bytes_env(name: &str, ceiling: u64) -> Result<u64, String> {
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(ceiling),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!("{name} is not valid Unicode"))
        }
    };
    let requested = parse_bytes(&value)
        .filter(|parsed| *parsed > 0)
        .ok_or_else(|| format!("{name} must be positive"))?;
    let bounded = requested.min(ceiling);
    if bounded != requested {
        tracing::warn!(
            budget = name,
            requested,
            bounded,
            automatic = ceiling,
            "configured memory budget exceeds the cgroup-aware capacity; clamping"
        );
    }
    Ok(bounded)
}

/// Parse a byte count with an optional `k`/`m`/`g` (1024-based) suffix.
pub fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num, mult) = match s.chars().last().unwrap().to_ascii_lowercase() {
        'k' => (&s[..s.len() - 1], 1024),
        'm' => (&s[..s.len() - 1], 1024 * 1024),
        'g' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    num.trim()
        .parse::<u64>()
        .ok()
        .and_then(|value| value.checked_mul(mult))
}

// ── Resource snapshot (autoscale signals) ───────────────────────────────────

/// ResourceStats is a control-plane endpoint, not a whole-registry export.  Keep
/// both the default and the hard page bound finite so a legacy unit request can
/// never turn into an O(number-of-graphs) response allocation.
pub const DEFAULT_RESOURCE_STATS_LIMIT: usize = 128;
pub const MAX_RESOURCE_STATS_LIMIT: usize = 1024;
pub const MAX_RESOURCE_STATS_CURSOR_BYTES: usize = 1024;
/// A bounded rollup working set keeps a million-graph resident scan from
/// becoming a million-entry tenant map.  The response says when this cap was
/// reached instead of presenting a partial rollup as exact.
pub const MAX_RESOURCE_STATS_TENANTS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceStatsRequest {
    pub cursor: Option<String>,
    pub limit: usize,
    pub summary: bool,
}

impl ResourceStatsRequest {
    pub fn bounded_default() -> Self {
        Self {
            cursor: None,
            limit: DEFAULT_RESOURCE_STATS_LIMIT,
            summary: false,
        }
    }

    pub fn validate(self) -> Result<Self, String> {
        if self.limit == 0 || self.limit > MAX_RESOURCE_STATS_LIMIT {
            return Err(format!(
                "ResourceStats limit must be between 1 and {MAX_RESOURCE_STATS_LIMIT}"
            ));
        }
        if let Some(cursor) = &self.cursor {
            if cursor.is_empty()
                || cursor.len() > MAX_RESOURCE_STATS_CURSOR_BYTES
                || cursor.bytes().any(|byte| byte == 0)
            {
                return Err(format!(
                    "ResourceStats cursor must be non-empty, NUL-free, and at most {MAX_RESOURCE_STATS_CURSOR_BYTES} bytes"
                ));
            }
        }
        if self.summary {
            // A cursor has no meaning when no detail page is emitted.  Rejecting
            // the combination also prevents callers from mistaking a summary's
            // aggregate counts for the page after a cursor.
            if self.cursor.is_some() {
                return Err("ResourceStats summary cannot be combined with cursor".to_string());
            }
            // Summary mode intentionally does not need a page-sized candidate
            // heap. Keep the validated finite limit in the response metadata.
        }
        Ok(self)
    }
}

impl Default for ResourceStatsRequest {
    fn default() -> Self {
        Self::bounded_default()
    }
}

/// Parse current process RSS in bytes, falling back to peak RSS when a platform
/// omits `VmRSS`.
pub fn parse_process_rss_bytes(status: &str) -> u64 {
    for key in ["VmRSS:", "VmHWM:"] {
        for line in status.lines() {
            let Some(rest) = line.strip_prefix(key) else {
                continue;
            };
            if let Some(kb) = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u64>().ok())
            {
                return kb.saturating_mul(1024);
            }
        }
    }
    0
}

#[cfg(target_os = "linux")]
pub async fn process_rss_bytes_from(status_path: std::path::PathBuf) -> u64 {
    // `/proc/self/status` is small, but opening it can still block on filesystem
    // machinery. Keep the complete, owned probe on Tokio's bounded blocking pool;
    // a missing procfs or a failed worker preserves the metric's documented zero
    // fallback rather than failing the ResourceStats request.
    ::tokio::task::spawn_blocking(move || {
        std::fs::read_to_string(status_path)
            .map(|status| parse_process_rss_bytes(&status))
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// Current process RSS in bytes, falling back to peak RSS when Linux omits
/// `VmRSS`. Returns `0` when procfs is unavailable or on another platform.
async fn process_rss_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        process_rss_bytes_from(std::path::PathBuf::from("/proc/self/status")).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// A page candidate carries only the bounded response fields.  Ordering the
/// heap by graph name lets the registry scan retain the lexicographically first
/// `limit` visible names without ever materializing an N-graph vector.
#[derive(Debug, Eq, PartialEq)]
pub struct ResourceStatsCandidate {
    pub graph: String,
    pub tenant: String,
    pub nodes: u64,
    pub edges: u64,
    pub memory_bytes: u64,
    pub hibernated: bool,
}

impl Ord for ResourceStatsCandidate {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        // BinaryHeap is a max-heap; the largest retained name is the item that
        // should be evicted when a smaller name arrives.
        self.graph.cmp(&other.graph)
    }
}

impl PartialOrd for ResourceStatsCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl ResourceStatsCandidate {
    fn into_stats(self) -> GraphResourceStats {
        GraphResourceStats {
            graph: self.graph,
            tenant: self.tenant,
            nodes: self.nodes,
            edges: self.edges,
            memory_bytes: self.memory_bytes,
            hibernated: self.hibernated,
        }
    }
}

fn canonical_tenant_slug(tenant: &str) -> String {
    let mut slug = String::with_capacity(tenant.len());
    for ch in tenant.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-') {
            slug.push(ch.to_ascii_lowercase());
        } else {
            slug.push('_');
        }
    }
    slug.trim_matches('_').to_string()
}

struct TenantMatcher {
    tenant: String,
    prefix: String,
}

impl TenantMatcher {
    pub fn new(tenant: &str) -> Self {
        let tenant = tenant.trim().to_string();
        let slug = canonical_tenant_slug(&tenant);
        Self {
            prefix: format!("tenant__{slug}__"),
            tenant,
        }
    }

    fn matches(&self, graph_name: &str) -> bool {
        if self.tenant.is_empty() {
            return false;
        }
        if graph_name == "__commons__" || graph_name == self.tenant {
            return true;
        }

        // Preserve the legacy `tenant:scope` convention used by engine-owned
        // graphs, while also recognizing the agent-utilities tenant graph
        // convention (`tenant__<slug>__<base>`).  Only the two bases that the
        // engine's authoritative tenant auto-grant recognizes are admitted
        // here; a guessed custom suffix must still prove access through ACL.
        tenant_of(graph_name) == self.tenant
            || (graph_name.starts_with(&self.prefix)
                && (graph_name.ends_with("__default") || graph_name.ends_with("____commons__")))
    }
}

pub fn graph_matches_tenant(graph_name: &str, verified_tenant: &str) -> bool {
    TenantMatcher::new(verified_tenant).matches(graph_name)
}

/// Gather a [`ResourceSnapshot`] for an already-verified caller.  Tenant and
/// graph ACL checks happen before a graph contributes to *any* count, cursor,
/// candidate heap, or metric.  The raw two-argument function below remains only
/// as a bounded in-process compatibility helper for existing Rust callers/tests;
/// the served RPC always uses this authority-bearing path.
pub async fn collect_resource_stats_authorized(
    state: &Arc<RwLock<ServerState>>,
    authority: &medulla_server_api::carrier::GraphReadAuthority,
    verified_tenant: &str,
    request: ResourceStatsRequest,
) -> Result<ResourceSnapshot, String> {
    let actor = authority.verified_actor()?.to_string();
    let admin = authority
        .carrier()
        .is_some_and(|carrier| carrier.is_admin());
    collect_resource_stats_inner(
        state,
        Some((actor.as_str(), verified_tenant, admin)),
        request,
    )
    .await
}

/// Backward-compatible in-process helper.  It has an explicit finite default
/// and is not used by the authenticated RPC dispatcher; callers serving a
/// request must use [`collect_resource_stats_authorized`].
pub async fn collect_resource_stats(
    state: &Arc<RwLock<ServerState>>,
) -> Result<ResourceSnapshot, String> {
    collect_resource_stats_inner(state, None, ResourceStatsRequest::bounded_default()).await
}

async fn collect_resource_stats_inner(
    state: &Arc<RwLock<ServerState>>,
    authorization: Option<(&str, &str, bool)>,
    request: ResourceStatsRequest,
) -> Result<ResourceSnapshot, String> {
    let request = request.validate()?;
    let config = CostConfig::from_env()?;
    let capacity = medulla_kernel::autosize::detect_capacity();
    let (configured_max_inflight, permits_available) = {
        let current = state.read().await;
        let permits = current.max_in_flight.available_permits();
        let automatic_max = capacity.max_inflight();
        let max = std::env::var("MEDULLA_MAX_INFLIGHT")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|&value| value > 0)
            .map(|value| medulla_kernel::autosize::bound_explicit(value, automatic_max))
            .unwrap_or(automatic_max);
        (max, permits)
    };
    let in_flight = configured_max_inflight.saturating_sub(permits_available) as u64;
    let scan = resource_stats::scan_registry(state, authorization, &request, &config).await?;

    let rss = process_rss_bytes().await;
    let (coalescer_queue_depth, coalescer_queue_bytes, coalescer_operations_total) =
        medulla_kernel::write_coalescer::global_stats();
    medulla_kernel::metrics::set_resource_stats(
        capacity.reserved_cpus().min(i64::MAX as usize) as i64,
        capacity.reserved_ram_bytes().min(i64::MAX as u64) as i64,
        rss.min(i64::MAX as u64) as i64,
        coalescer_queue_depth.min(i64::MAX as u64) as i64,
        coalescer_queue_bytes.min(i64::MAX as u64) as i64,
        coalescer_operations_total.min(i64::MAX as u64) as i64,
    );

    let counters = cost_state();
    Ok(ResourceSnapshot {
        limit: request.limit as u64,
        cursor: request.cursor,
        next_cursor: scan.next_cursor,
        has_more: scan.has_more,
        summary: request.summary,
        total_memory_bytes: scan.total_memory_bytes,
        process_rss_bytes: rss,
        global_ceiling_bytes: config.global_ceiling_bytes,
        total_nodes: scan.total_nodes,
        total_edges: scan.total_edges,
        graph_count: scan.resident_graphs.saturating_add(scan.hibernated_graphs),
        tenant_count: scan.tenants.len() as u64,
        resident_graphs: scan.resident_graphs,
        hibernated_graphs: scan.hibernated_graphs,
        effective_cpu_cores: capacity.reserved_cpus() as u64,
        effective_memory_limit_bytes: capacity.reserved_ram_bytes(),
        tenant_count_truncated: scan.tenant_count_truncated,
        in_flight,
        inflight_permits_available: permits_available as u64,
        budget_evictions_total: counters
            .evicted_total
            .load(std::sync::atomic::Ordering::Relaxed),
        budget_hibernations_total: counters
            .hibernated_total
            .load(std::sync::atomic::Ordering::Relaxed),
        coalescer_queue_depth,
        coalescer_queue_bytes,
        coalescer_operations_total,
        tenants: if request.summary {
            Vec::new()
        } else {
            scan.tenants
        },
        graphs: if request.summary {
            Vec::new()
        } else {
            scan.graphs
        },
    })
}
/// Per-tenant memory-budget enforcement — the only part of this model that
/// mutates engine state.
pub mod budget;
pub use budget::enforce_memory_budgets;

// ── Cost model / capacity planning (CONCEPT:MEDULLA-KG.compute.lane-v) ───────────────────────

/// A capacity estimate for a target tenant count + average working set
/// (CONCEPT:MEDULLA-KG.compute.lane-v). Maps a workload onto a resource footprint so an operator can pick
/// a deployment tier (Pi → cluster) and size the shard count. See `docs/cost-model.md`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CapacityEstimate {
    pub tenants: u64,
    /// Average resident working-set bytes per tenant the estimate assumed.
    pub avg_working_set_bytes: u64,
    /// Total resident RAM a single engine would need to hold ALL working sets hot.
    pub total_working_set_bytes: u64,
    /// Resident RAM the engine actually needs given the configured budget (the budget
    /// enforcer evicts/hibernates cold tenants past this — so a box only needs to hold
    /// the budgeted resident fraction hot, the rest serve read-through from the durable
    /// tier).
    pub budgeted_resident_bytes: u64,
    /// How many shards of `per_shard_budget_bytes` it takes to hold the budgeted resident
    /// set (the autoscale target).
    pub recommended_shards: u64,
}

/// Estimate the resource footprint for `tenants` tenants each with an `avg_working_set`
/// (CONCEPT:MEDULLA-KG.compute.lane-v). `global_budget` is the per-shard resident-memory ceiling, and
/// `resident_fraction` (0.0–1.0) is the share of tenants expected hot at once (the rest
/// hibernate / read-through). Pure arithmetic — no engine state — so it is a planning
/// helper callable offline (and the doc's worked examples derive from it).
pub fn capacity_estimate(
    tenants: u64,
    avg_working_set_bytes: u64,
    per_shard_budget_bytes: u64,
    resident_fraction: f64,
) -> CapacityEstimate {
    let resident_fraction = resident_fraction.clamp(0.0, 1.0);
    let total = tenants.saturating_mul(avg_working_set_bytes);
    let budgeted_resident = ((total as f64) * resident_fraction) as u64;
    let recommended_shards = if per_shard_budget_bytes == 0 {
        0
    } else {
        budgeted_resident.div_ceil(per_shard_budget_bytes).max(1)
    };
    CapacityEstimate {
        tenants,
        avg_working_set_bytes,
        total_working_set_bytes: total,
        budgeted_resident_bytes: budgeted_resident,
        recommended_shards,
    }
}
