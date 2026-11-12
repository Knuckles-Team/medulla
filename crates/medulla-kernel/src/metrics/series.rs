//! The kernel's Prometheus series as one owned value (MEDULLA-SDLC-R023).
//!
//! `KernelMetrics::new` builds a fresh registry and registers every series on
//! it, so two values share nothing; the process holds one, reached through
//! `metrics::process()`, which the exposition endpoint renders.

use parking_lot::Mutex;
use prometheus::{
    Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Opts,
    Registry,
};
use std::collections::HashSet;

/// UDS round-trips are sub-millisecond; analytics can run for seconds —
/// span both regimes. The tail (60/120/300/600/1800/3600) keeps a request
/// anywhere up to `MEDULLA_DISPATCH_DEADLINE_SECS`'s ceiling
/// observable instead of collapsing into the `+Inf` catch-all — a 30s-max
/// histogram could not distinguish an 82-request population that maxed
/// out at 31s from one that maxed out at 590s (D-EIMG-7).
const DURATION_BUCKETS: &[f64] = &[
    0.0001, 0.00025, 0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5,
    5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0,
];

fn counter_vec(registry: &Registry, name: &str, help: &str, labels: &[&str]) -> IntCounterVec {
    let m = IntCounterVec::new(Opts::new(name, help), labels).expect("valid metric");
    registry
        .register(Box::new(m.clone()))
        .expect("unique metric");
    m
}

fn gauge_vec(registry: &Registry, name: &str, help: &str, labels: &[&str]) -> IntGaugeVec {
    let m = IntGaugeVec::new(Opts::new(name, help), labels).expect("valid metric");
    registry
        .register(Box::new(m.clone()))
        .expect("unique metric");
    m
}

fn gauge(registry: &Registry, name: &str, help: &str) -> IntGauge {
    let m = IntGauge::new(name, help).expect("valid metric");
    registry
        .register(Box::new(m.clone()))
        .expect("unique metric");
    m
}

fn counter(registry: &Registry, name: &str, help: &str) -> IntCounter {
    let m = IntCounter::new(name, help).expect("valid metric");
    registry
        .register(Box::new(m.clone()))
        .expect("unique metric");
    m
}

// CONCEPT:MEDULLA-OS.observability.write-lock-gap-histogram — labelled-histogram helper for the write-lock-gap series.
// Mirrors `counter_vec`/`gauge_vec` (register-on-build), with a caller-supplied
// bucket layout so the lock-gap metrics can span sub-ms .. tens-of-seconds.
fn histogram_vec(
    registry: &Registry,
    name: &str,
    help: &str,
    labels: &[&str],
    buckets: Vec<f64>,
) -> HistogramVec {
    let m = HistogramVec::new(HistogramOpts::new(name, help).buckets(buckets), labels)
        .expect("valid metric");
    registry
        .register(Box::new(m.clone()))
        .expect("unique metric");
    m
}

// CONCEPT:MEDULLA-OS.observability.write-lock-gap-histogram — write-lock-gap bucket layout. The starvation we measure
// (semantic_search 0.02s idle → 14s under the `__commons__` firehose) spans
// five orders of magnitude: an uncontended batch holds the topology lock for
// tens of microseconds, while a starved writer can wait tens of seconds behind
// the ingestion stream. An exponential ladder from 0.1ms to ~90s resolves both
// the fast tail and the pathological wait without unbounded cardinality.
fn lock_gap_buckets() -> Vec<f64> {
    prometheus::exponential_buckets(0.0001, 2.5, 16).expect("valid bucket spec")
}

/// Every kernel series, on its own registry.
pub struct KernelMetrics {
    pub(super) registry: Registry,
    pub(super) seen_graphs: Mutex<HashSet<String>>,
    pub(super) requests_total: IntCounterVec,
    pub(super) request_duration: HistogramVec,
    pub(super) in_flight: IntGauge,
    pub(super) permits_available: IntGauge,
    pub(super) busy_rejections: IntCounter,
    pub(super) dispatch_deadline_exceeded: IntCounter,
    pub(super) asset_usage_dropped: IntCounter,
    pub(super) read_reserved_admitted: IntCounter,
    // ── W2.4 engine-native QoS lanes (CONCEPT:MEDULLA-KG.coordination.backpressure-busy-signal) ──
    // Per-admission-class observability: the first-class "did the noisy-neighbor SLO
    // hold" telemetry. `class` ∈ {ingest,hydration,orch,interactive} (bounded, 4);
    // `reason` ∈ {quota,fair_share,backpressure,rate_limited} (bounded, 4). Zero unless
    // the QoS gate is enabled (`MEDULLA_QOS`).
    pub(super) qos_admitted: IntCounterVec,
    pub(super) qos_shed: IntCounterVec,
    pub(super) qos_class_in_flight: IntGaugeVec,
    pub(super) qos_class_latency: HistogramVec,
    // MEDULLA-DURABLE-KERNEL-R084.5: one count per resolved stale envelope.
    // `strategy` ∈ {refuse,prefer_newer,field_union}; `winner` ∈
    // {existing,incoming,merged} (bounded, 3 × 3).
    pub(super) write_conflicts: IntCounterVec,
    pub(super) graph_ops: IntCounterVec,
    pub(super) graph_nodes: IntGaugeVec,
    pub(super) graph_edges: IntGaugeVec,
    pub(super) checkpoint_duration: Histogram,
    pub(super) checkpoint_last_success: IntGauge,
    pub(super) analytics_jobs_ready: IntGauge,
    pub(super) analytics_jobs_active: IntGauge,
    pub(super) analytics_jobs_publishing: IntGauge,
    pub(super) auth_failures: IntCounter,
    pub(super) access_denied: IntCounter,
    // CA-11 CDC-to-Kafka sink backpressure (feature `cdc-kafka`, DEC-CA-03).
    // W05: "paused broker doesn't block commits" — these two observe that
    // without ever gating the write path on the broker being reachable.
    pub(super) cdc_kafka_sink_lag: IntGauge,
    pub(super) cdc_kafka_sink_send_failures: IntCounter,
    // Per-graph write-coalescer observability (CONCEPT:MEDULLA-KG.sharding.per-graph-write-coalescer). The win is
    // visible as ops/batch > 1: BATCHES counts topology-lock acquisitions on the
    // coalesced path, OPS counts the writes those acquisitions applied, so
    // (OPS / BATCHES) is the lock-acquisitions-saved ratio per graph.
    pub(super) write_batches: IntCounterVec,
    pub(super) write_batched_ops: IntCounterVec,
    // ── Write-lock-gap histograms (CONCEPT:MEDULLA-OS.observability.write-lock-gap-histogram) ──
    // The contention this measures: under the `__commons__` ingestion firehose a
    // read (semantic_search) goes 0.02s idle → 14s, because writers serialize on
    // the per-graph topology write lock and a reader/late-writer queues behind the
    // batch in flight. WAIT is the starvation cost (enqueue → lock acquired); HOLD
    // is the contention cost (how long each batch keeps the lock). (HOLD × batch
    // rate) is the fraction of wall-clock the lock is unavailable to readers.
    pub(super) write_lock_wait: HistogramVec,
    pub(super) write_lock_hold: HistogramVec,
    // ── Global dispatch state-lock wait (D-EIMG-2) ──
    // The two histograms above cover the PER-GRAPH topology lock, reached only via the
    // write coalescer. They say nothing about the single process-wide
    // `Arc<RwLock<ServerState>>` that EVERY dispatched method acquires before it can
    // do anything at all. That global lock is the suspected cause of the load-dependent
    // latency tail seen live (`HasNode` — a hash lookup — at a 3.992s mean; bimodal
    // `GetNodeProperties`/`GetNeighbors` with a small population of 0.25–30s calls
    // against a >99% sub-0.0001s body). Without this histogram that hypothesis cannot
    // be confirmed or refuted from the outside: per-method latency alone cannot
    // separate "the work is slow" from "the work waited to start".
    //
    // `mode` (read|write) is the ONLY label, deliberately: a `method` label would
    // multiply ~200 methods by 16 buckets on the hottest path in the process. Total
    // wait split by read/write is what distinguishes contention from expensive work,
    // which is the open question; per-method attribution can be layered on later if
    // this confirms contention.
    pub(super) dispatch_lock_wait: HistogramVec,
    // EH-290 commit_ops decomposition: see CommitPhaseTimer for phases.
    pub(super) commit_ops_phase: HistogramVec,
    // EH-384 background node-payload scrub: rows opened, unreadable rows
    // found (by closed `UnsealFailure` code), completed full-file cycles.
    pub(super) storage_scrub_rows: IntCounter,
    pub(super) storage_scrub_unreadable: IntCounterVec,
    pub(super) storage_scrub_cycles: IntCounter,
    // MEDULLA-DURABLE-KERNEL-R006.2 shard-open phase split.
    pub(super) store_open_phase: HistogramVec,
    pub(super) store_open_scan: IntCounterVec,
    pub(super) commit_ops_bytes: HistogramVec,
    // ── RLS projection-cache hit/miss + miss cost (D-EGP-1, t1-grounding-0802) ──
    // `GraphReadAuthority::project_core` (`server/access.rs`) caches its expensive
    // per-(actor, GraphCore::version()) materialization in `rls_projection_cache`
    // (D-OP-1 / D-OB-20: cold miss measured 1.3-1.5s vs 7.57us warm — a ~190,000x
    // ratio). The lock-contention hypothesis for grounding's 40-56s compile latency
    // was REFUTED by `DISPATCH_LOCK_WAIT` (uncontended: 308 acquisitions totalled
    // 0.000155s). The prime remaining suspect is that the cache THRASHES under
    // interleaved reads/writes on a shared daemon — every committed write
    // invalidates the cache for every actor (the version bump), so a compile
    // spanning tens of seconds crosses several invalidation boundaries. There was
    // NO hit/miss telemetry to confirm or refute this; these two series are it.
    //
    // `result` (hit|miss) is the ONLY label — mirrors `DISPATCH_LOCK_WAIT`'s
    // `mode`-only cardinality discipline on the same hot path (every RLS-active
    // read goes through this). Per-actor/per-graph attribution is deliberately
    // NOT added here for the same reason.
    pub(super) projection_cache_result: IntCounterVec,
    pub(super) projection_cache_miss_build: Histogram,
    // ── Cost / efficiency autoscale signals (CONCEPT:MEDULLA-KG.compute.lane-v, Lane V) ──
    pub(super) graph_memory_bytes: IntGaugeVec,
    pub(super) graph_hibernated: IntGaugeVec,
    // ResourceStats publishes these process-wide signals without a graph
    // label.  They are intentionally aggregate-only so a telemetry scan
    // cannot create a metric series for a graph the caller was not allowed
    // to enumerate.
    pub(super) effective_cpu_cores: IntGauge,
    pub(super) effective_memory_limit_bytes: IntGauge,
    pub(super) process_rss_bytes: IntGauge,
    pub(super) coalescer_queue_depth: IntGauge,
    pub(super) coalescer_queue_bytes: IntGauge,
    pub(super) coalescer_operations_total: IntGauge,
    pub(super) budget_evictions: IntCounter,
    pub(super) budget_hibernations: IntCounter,
    // CONCEPT:MEDULLA-OS.observability.slow-query-descriptor — slow-query counter. Increments once per query whose
    // end-to-end execution met/exceeded MEDULLA_SLOW_QUERY_MS; stays at
    // zero unless the slow-query threshold is configured.
    pub(super) slow_queries: IntCounter,
    // ── Background interval-loop observability (daemon-consolidation design, Phase 2) ──
    // The engine's own hardcoded `tokio::time::interval` sweeps (lake materialize,
    // checkpoint, decay, memory-cap, memory-budget, cold-tenant offload, txn-TTL) each
    // funnel their tick through ONE call (`loop_tick`) so they show up on the SAME
    // telemetry plane as everything else, without touching any loop's cadence or logic.
    // The `loop` label is a fixed, small set of hardcoded names (bounded cardinality —
    // no `graph_label`-style overflow guard needed).
    pub(super) loop_ticks_total: IntCounterVec,
    pub(super) loop_tick_duration: HistogramVec,
    pub(super) loop_last_run: IntGaugeVec,
    // CONCEPT:MEDULLA-KG.epistemic.truth-maintenance — the "give staleness a consumer"
    // SURPASS gap-closure: a live gauge (the CURRENT stale count, refreshed from
    // the durable per-graph reasoning projection after committed invalidations)
    // plus a cumulative counter (how many staling EVENTS have fired, distinct from
    // the gauge because a materialization can go stale, get recomputed back to
    // fresh, and go stale again — the counter never decreases, the gauge does).
    pub(super) epistemic_materializations_stale: IntGauge,
    pub(super) epistemic_materializations_staled_total: IntCounter,
    // ADR-5 (W2.2) statechart migration phase 1: the dual-write DIVERGENCE ALARM.
    // Incremented whenever a lifecycle statechart MIRROR disagrees with the
    // authoritative (redb-row / Python-enum) next state — the loud, queryable signal
    // that a chart definition is not yet a faithful replica and the authority flip
    // (phase 2) must not proceed. The `machine` label is a fixed small set
    // (`work_item`, `loop`) so the series is bounded.
    pub(super) statechart_divergence_total: IntCounterVec,
    pub(super) decision_abstentions: IntCounterVec,
    pub(super) decision_head_drift: IntGaugeVec,
    pub(super) decision_solver_nodes_ratio: Histogram,
    pub(super) decision_stage_seconds: HistogramVec,
    pub(super) decisions: IntCounterVec,
    pub(super) decision_refusals: IntCounterVec,
    pub(super) decision_exploration: IntCounterVec,
    pub(super) decision_evaluations: IntCounterVec,
    pub(super) decision_threshold_assessments: IntCounterVec,
    pub(super) decision_latency: HistogramVec,
    pub(super) outbox_dead_lettered_total: IntCounterVec,
    pub(super) outbox_head_age_ms: IntGaugeVec,
    pub(super) outbox_head_attempt: IntGaugeVec,
    pub(super) view_changes_applied: IntCounterVec,
    pub(super) view_demotions: IntCounterVec,
    pub(super) view_shadow_checks: IntCounterVec,
    pub(super) view_state_bytes: IntGaugeVec,
    pub(super) view_resident_bytes: IntGauge,
    pub(super) work_item_uncertain_effects_total: IntCounterVec,
    pub(super) work_item_uncertain_open: IntGauge,
    pub(super) store_migration_steps_total: IntCounterVec,
}

impl KernelMetrics {
    /// Build every series on a fresh registry.
    pub fn new() -> Self {
        let registry = Registry::new();
        Self {
            seen_graphs: Mutex::new(HashSet::new()),
            requests_total: counter_vec(&registry,
            "medulla_requests_total",
            "Requests dispatched, by protocol operation",
            &["op"],
        ),
            request_duration: {
            let m = HistogramVec::new(
                HistogramOpts::new(
                    "medulla_request_duration_seconds",
                    "Dispatch latency, by protocol operation",
                )
                .buckets(DURATION_BUCKETS.to_vec()),
                &["op"],
            )
            .expect("valid metric");
            registry
                .register(Box::new(m.clone()))
                .expect("unique metric");
            m
        },
            in_flight: gauge(&registry,
            "medulla_in_flight_requests",
            "Connection requests currently holding an admission permit",
        ),
            permits_available: gauge(&registry,
            "medulla_inflight_permits_available",
            "Admission-semaphore permits still available \
             (MEDULLA_MAX_INFLIGHT minus in-flight; admission is \
             try-acquire, so nothing ever queues — excess load is shed as BUSY)",
        ),
            busy_rejections: counter(&registry,
            "medulla_busy_rejections_total",
            "Requests shed with BUSY because the admission semaphore was exhausted",
        ),
            dispatch_deadline_exceeded: counter(&registry,
            "medulla_dispatch_deadline_exceeded_total",
            "Requests abandoned because their dispatch exceeded the hard per-request \
             deadline (CONCEPT:MEDULLA-KG.coordination.backpressure-busy-signal). Non-zero means a \
             served request stalled long enough to strand its admission permits — the \
             failure mode that once left every permit held and shed ALL later traffic",
        ),
            asset_usage_dropped: counter(&registry,
            "medulla_asset_usage_dropped_total",
            "Asset usage points that could not be recorded after a served query \
             (MEDULLA-UNIFIED-DATA-PLANE-R764.3.1); the query itself still succeeded",
        ),
            read_reserved_admitted: counter(&registry,
            "medulla_read_reserved_admitted_total",
            "Read/query requests admitted via the RESERVED read lane (CONCEPT:MEDULLA-KG.coordination.reserved-read-lane) \
             after the global pool / per-graph cap was saturated by writes — each is an \
             interactive read that would otherwise have been shed BUSY behind ingestion",
        ),
            qos_admitted: counter_vec(&registry,
            "medulla_qos_admitted_total",
            "Requests admitted by the QoS admission gate, by priority class (W2.4)",
            &["class"],
        ),
            qos_shed: counter_vec(&registry,
            "medulla_qos_shed_total",
            "Requests shed by the QoS admission gate, by priority class and shed reason \
             (W2.4). The lowest class is shed first under pressure",
            &["class", "reason"],
        ),
            qos_class_in_flight: gauge_vec(&registry,
            "medulla_qos_class_in_flight",
            "Requests currently holding a QoS admission permit, by priority class (W2.4) — \
             the per-class queue depth",
            &["class"],
        ),
            qos_class_latency: {
            let m = HistogramVec::new(
                HistogramOpts::new(
                    "medulla_qos_class_dispatch_seconds",
                    "End-to-end dispatch latency of a QoS-admitted request, by priority \
                     class (W2.4) — interactive p99 must hold under an ingest flood",
                )
                .buckets(DURATION_BUCKETS.to_vec()),
                &["class"],
            )
            .expect("valid metric");
            registry
                .register(Box::new(m.clone()))
                .expect("unique metric");
            m
        },
            write_conflicts: counter_vec(&registry,
            "medulla_write_conflicts_total",
            "Stale change-envelope write conflicts, by merge strategy and winner",
            &["strategy", "winner"],
        ),
            graph_ops: counter_vec(&registry,
            "medulla_graph_ops_total",
            "Graph-targeted operations admitted past the ACL, by graph (bounded cardinality)",
            &["graph"],
        ),
            graph_nodes: gauge_vec(&registry,
            "medulla_graph_nodes",
            "Node count per graph, updated on mutation",
            &["graph"],
        ),
            graph_edges: gauge_vec(&registry,
            "medulla_graph_edges",
            "Edge count per graph, updated on mutation",
            &["graph"],
        ),
            checkpoint_duration: {
            let m = Histogram::with_opts(
                HistogramOpts::new(
                    "medulla_checkpoint_duration_seconds",
                    "Wall-clock duration of a full registry checkpoint",
                )
                .buckets(vec![0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 15.0, 60.0]),
            )
            .expect("valid metric");
            registry
                .register(Box::new(m.clone()))
                .expect("unique metric");
            m
        },
            checkpoint_last_success: gauge(&registry,
            "medulla_checkpoint_last_success_timestamp_seconds",
            "Unix timestamp of the last successful checkpoint",
        ),
            analytics_jobs_ready: gauge(&registry,
            "medulla_analytics_jobs_ready",
            "Durable analytics jobs eligible for a remote worker lease",
        ),
            analytics_jobs_active: gauge(&registry,
            "medulla_analytics_jobs_active",
            "Durable analytics jobs with a currently live worker lease",
        ),
            analytics_jobs_publishing: gauge(&registry,
            "medulla_analytics_jobs_publishing",
            "Durable analytics jobs with a staged result awaiting publication",
        ),
            auth_failures: counter(&registry,
            "medulla_auth_failures_total",
            "Requests rejected by HMAC authentication",
        ),
            access_denied: counter(&registry,
            "medulla_access_denied_total",
            "Graph operations denied by the isolation ACL",
        ),
            cdc_kafka_sink_lag: gauge(&registry,
            "medulla_cdc_kafka_sink_lag",
            "CA-11 CDC-to-Kafka sink: events queued locally but not yet \
             broker-acknowledged (rdkafka in-flight count)",
        ),
            cdc_kafka_sink_send_failures: counter(&registry,
            "medulla_cdc_kafka_sink_send_failures_total",
            "CA-11 CDC-to-Kafka sink: emit() calls that failed (queue-full or \
             broker error) — the CDC ring still has the event, never lost, \
             just not (yet) forwarded to medulla.cdc.<graph>",
        ),
            write_batches: counter_vec(&registry,
            "medulla_write_batches_total",
            "Coalesced write batches committed (= topology-lock acquisitions on the \
             coalesced path), by graph",
            &["graph"],
        ),
            write_batched_ops: counter_vec(&registry,
            "medulla_write_batched_ops_total",
            "Single-op writes applied via the coalescer, by graph. Divide by \
             write_batches_total for the average batch size (ops per lock acquisition)",
            &["graph"],
        ),
            write_lock_wait: histogram_vec(&registry,
            "medulla_write_lock_wait_seconds",
            "Time a coalesced write waited from enqueue to acquiring the per-graph \
             topology write lock (the starvation cost), by graph",
            &["graph"],
            lock_gap_buckets(),
        ),
            write_lock_hold: histogram_vec(&registry,
            "medulla_write_lock_hold_seconds",
            "Time the per-graph topology write lock was held to apply one coalesced \
             batch (the contention cost — readers/other writers block for this), by graph",
            &["graph"],
            lock_gap_buckets(),
        ),
            dispatch_lock_wait: histogram_vec(&registry,
            "medulla_dispatch_lock_wait_seconds",
            "Time a dispatched request blocked acquiring the process-wide ServerState \
             lock (the global dispatch serialization point), by acquisition mode",
            &["mode"],
            lock_gap_buckets(),
        ),
            commit_ops_phase: histogram_vec(&registry,
            "medulla_commit_ops_phase_seconds",
            "Wall-clock duration of one commit_ops drain, decomposed by phase \
             (commit_ops|acquire_txn|bind_graphs|admit_group|apply_writes|\
             lane_link_validation|ledger_finish|durability_commit)",
            &["phase"],
            DURATION_BUCKETS.to_vec(),
        ),
            storage_scrub_rows: counter(&registry,
            "medulla_storage_scrub_rows_total",
            "Node payloads opened by the background storage scrub",
        ),
            storage_scrub_unreadable: counter_vec(&registry,
            "medulla_storage_scrub_unreadable_total",
            "Unreadable node rows found by the background storage scrub, by cause",
            &["cause"],
        ),
            storage_scrub_cycles: counter(&registry,
            "medulla_storage_scrub_cycles_total",
            "Completed full walks of a shard file by the background storage scrub",
        ),
            store_open_phase: histogram_vec(&registry,
            "medulla_store_open_phase_seconds",
            "Wall-clock duration of one kernel-owned store open, by phase \
             (file_open|table_validation|recovery|bind|ledger|total)",
            &["phase"],
            DURATION_BUCKETS.to_vec(),
        ),
            store_open_scan: counter_vec(&registry,
            "medulla_store_open_recovery_scan_total",
            "Store opens by recovery-scan outcome (ran|skipped_clean_marker|created)",
            &["outcome"],
        ),
            commit_ops_bytes: histogram_vec(&registry,
            "medulla_commit_ops_bytes",
            "Bytes accounted per commit_ops drain, by kind (logical|physical_delta)",
            &["kind"],
            prometheus::exponential_buckets(64.0, 4.0, 11).expect("valid bucket spec"),
        ),
            projection_cache_result: counter_vec(&registry,
            "medulla_projection_cache_result_total",
            "RLS projection-cache (project_core) lookups, by result: hit (fast path, \
             microseconds) or miss (cold/stale, triggers a full O(V log V + E log E + \
             V*d) rebuild)",
            &["result"],
        ),
            projection_cache_miss_build: {
            let m = Histogram::with_opts(
                HistogramOpts::new(
                    "medulla_projection_cache_miss_build_seconds",
                    "Wall-clock cost of rebuilding the RLS projection on a cache miss \
                     (the O(V log V + E log E + V*d) materialization project_core's cache \
                     exists to amortize)",
                )
                .buckets(lock_gap_buckets()),
            )
            .expect("valid metric");
            registry
                .register(Box::new(m.clone()))
                .expect("unique metric");
            m
        },
            graph_memory_bytes: gauge_vec(&registry,
            "medulla_graph_memory_bytes",
            "Approximate resident RAM per graph (node/edge property blobs + topology \
             overhead + embedding vectors), updated by the budget sweep",
            &["graph"],
        ),
            graph_hibernated: gauge_vec(&registry,
            "medulla_graph_hibernated",
            "1 if the graph is hibernated (in-RAM state dropped, durable in redb), 0 if \
             resident — by graph",
            &["graph"],
        ),
            effective_cpu_cores: gauge(&registry,
            "medulla_effective_cpu_cores",
            "Effective cgroup-aware CPU lanes after automatic headroom",
        ),
            effective_memory_limit_bytes: gauge(&registry,
            "medulla_effective_memory_limit_bytes",
            "Effective cgroup-aware RAM after automatic headroom",
        ),
            process_rss_bytes: gauge(&registry,
            "medulla_process_rss_bytes",
            "Current process RSS in bytes",
        ),
            coalescer_queue_depth: gauge(&registry,
            "medulla_write_coalescer_queue_depth",
            "Structural writes waiting in bounded per-graph coalescer queues",
        ),
            coalescer_queue_bytes: gauge(&registry,
            "medulla_write_coalescer_queue_bytes",
            "Approximate bytes held by bounded per-graph coalescer queues",
        ),
            coalescer_operations_total: gauge(&registry,
            "medulla_write_coalescer_operations_total",
            "Structural operations applied by the coalescer, including inline fallback",
        ),
            budget_evictions: counter(&registry,
            "medulla_budget_evictions_total",
            "LRU nodes evicted by the per-tenant memory-budget enforcer (the eviction \
             rate an autoscaler watches: sustained eviction = memory pressure)",
        ),
            budget_hibernations: counter(&registry,
            "medulla_budget_hibernations_total",
            "Cold graphs hibernated (in-RAM state dropped) by the budget enforcer to keep \
             a tenant under its memory budget",
        ),
            slow_queries: counter(&registry,
            "medulla_slow_query_total",
            "Queries whose end-to-end execution met/exceeded MEDULLA_SLOW_QUERY_MS \
             (CONCEPT:MEDULLA-OS.observability.slow-query-descriptor). Zero unless the slow-query threshold is configured",
        ),
            loop_ticks_total: counter_vec(&registry,
            "medulla_background_loop_ticks_total",
            "Ticks completed by an engine-native background interval loop, by loop name \
             (CONCEPT:MEDULLA-OS.observability.background-loop-telemetry)",
            &["loop"],
        ),
            loop_tick_duration: histogram_vec(&registry,
            "medulla_background_loop_tick_duration_seconds",
            "Wall-clock duration of one background interval-loop tick's own work \
             (excludes the sleep between ticks), by loop name",
            &["loop"],
            vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 15.0, 60.0, 300.0],
        ),
            loop_last_run: gauge_vec(&registry,
            "medulla_background_loop_last_run_timestamp_seconds",
            "Unix timestamp of the last completed tick, by loop name",
            &["loop"],
        ),
            epistemic_materializations_stale: gauge(&registry,
            "medulla_epistemic_materializations_stale",
            "Current count of TruthMaintenance materializations in the Stale status \
             (CONCEPT:MEDULLA-KG.epistemic.truth-maintenance)",
        ),
            epistemic_materializations_staled_total: counter(&registry,
            "medulla_epistemic_materializations_staled_total",
            "Cumulative count of materialization-id staling events fed through the \
             TruthMaintenance index's on_change (a ChangeEvent may stale several ids \
             at once; each staled id counts once per event)",
        ),
            statechart_divergence_total: counter_vec(&registry,
            "medulla_statechart_divergence_total",
            "Count of lifecycle-statechart mirror transitions that diverged from the \
             authoritative next state during the ADR-5 phase-1 dual-write migration, by \
             machine (CONCEPT:INT-P2-2)",
            &["machine"],
        ),
            decision_abstentions: counter_vec(&registry,
            "medulla_decide_abstentions_total",
            "Decisions that abstained, by closed abstention code",
            &["code"],
        ),
            decision_head_drift: gauge_vec(&registry,
            "medulla_head_drift",
            "Latest drift reading (Q16 population stability index) by head",
            &["head"],
        ),
            decision_solver_nodes_ratio: {
            let metric = Histogram::with_opts(
                HistogramOpts::new(
                    "medulla_solver_nodes_ratio",
                    "Solver nodes expanded over the node budget, per solved assembly",
                )
                .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0]),
            )
            .expect("valid metric");
            registry
                .register(Box::new(metric.clone()))
                .expect("unique metric");
            metric
        },
            decision_stage_seconds: {
            let metric = HistogramVec::new(
                HistogramOpts::new(
                    "medulla_decide_stage_seconds",
                    "Decide pipeline stage latency, by family and stage",
                )
                .buckets(prometheus::exponential_buckets(0.0001, 2.5, 14).expect("valid buckets")),
                &["family", "stage"],
            )
            .expect("valid metric");
            registry
                .register(Box::new(metric.clone()))
                .expect("unique metric");
            metric
        },
            decisions: counter_vec(&registry,
            "medulla_decisions_total",
            "Statistical decisions answered, by outcome and evidence class",
            &["outcome", "evidence", "reason"],
        ),
            decision_refusals: counter_vec(&registry,
            "medulla_decision_refusals_total",
            "Decide-layer calls refused, by method and refusal code",
            &["method", "code"],
        ),
            decision_exploration: counter_vec(&registry,
            "medulla_decision_exploration_total",
            "Decisions drawn by the exploration budget versus the greedy branch",
            &["branch"],
        ),
            decision_evaluations: counter_vec(&registry,
            "medulla_decision_evaluations_total",
            "Decision-head evaluations, by promotion verdict",
            &["verdict"],
        ),
            decision_threshold_assessments: {
            let metric = counter_vec(&registry,
                "medulla_decision_threshold_assessments_total",
                "Persisted, independently labelled decision evaluations assessed against their pinned policy",
                &["signal", "result"],
            );
            for signal in ["support", "coverage", "act_risk"] {
                for result in ["ok", "breach", "unavailable"] {
                    metric.with_label_values(&[signal, result]).inc_by(0);
                }
            }
            metric
        },
            decision_latency: {
            let metric = HistogramVec::new(
                HistogramOpts::new(
                    "medulla_decision_duration_seconds",
                    "Decide-layer latency, by method",
                )
                .buckets(prometheus::exponential_buckets(0.0005, 2.5, 14).expect("valid buckets")),
                &["method"],
            )
            .expect("valid metric");
            registry
                .register(Box::new(metric.clone()))
                .expect("unique metric");
            metric
        },
            outbox_dead_lettered_total: counter_vec(&registry,
            "medulla_outbox_dead_lettered_total",
            "Outbox rows a consumer's stream dead-lettered, by consumer, topic and cause",
            &["consumer", "topic", "cause"],
        ),
            outbox_head_age_ms: gauge_vec(&registry,
            "medulla_outbox_head_age_ms",
            "Age of the oldest unresolved row at the head of a consumer's stream (0 when idle)",
            &["consumer", "topic"],
        ),
            outbox_head_attempt: gauge_vec(&registry,
            "medulla_outbox_head_attempt",
            "Delivery attempts the head row of a consumer's stream has spent (0 when idle)",
            &["consumer", "topic"],
        ),
            view_changes_applied: counter_vec(&registry,
            "medulla_view_changes_applied_total",
            "Changes folded into incremental views, by view family",
            &["family"],
        ),
            view_demotions: counter_vec(&registry,
            "medulla_view_demotions_total",
            "Views demoted out of incremental mode, by family and reason code",
            &["family", "reason_code"],
        ),
            view_shadow_checks: counter_vec(&registry,
            "medulla_view_shadow_checks_total",
            "Shadow recompute checks of incremental views, by family and outcome",
            &["family", "outcome"],
        ),
            view_state_bytes: gauge_vec(&registry,
            "medulla_view_state_bytes",
            "Maintained state of incremental views in bytes, by view family",
            &["family"],
        ),
            view_resident_bytes: gauge(&registry,
            "medulla_views_resident_state_bytes",
            "Maintained state of every resident incremental view in bytes",
        ),
            work_item_uncertain_effects_total: counter_vec(&registry,
            "medulla_work_item_uncertain_effects_total",
            "Uncertain-effect writes committed, by outcome",
            &["outcome"],
        ),
            work_item_uncertain_open: {
            let gauge = IntGauge::new(
                "medulla_work_item_uncertain_open",
                "Work items marked effect-started since boot that no receipt has resolved",
            )
            .expect("valid metric");
            registry
                .register(Box::new(gauge.clone()))
                .expect("unique metric");
            gauge
        },
            store_migration_steps_total: counter_vec(&registry,
            "medulla_store_migration_steps_total",
            "Automatic store migration steps at open, by step and outcome",
            &["step", "outcome"],
        ),
            registry,
        }
    }
}

impl Default for KernelMetrics {
    fn default() -> Self {
        Self::new()
    }
}
