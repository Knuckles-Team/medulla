//! The recording methods of the kernel series and the text exposition.

use prometheus::{Encoder, TextEncoder};

use super::series::KernelMetrics;

/// Distinct `graph` label values tracked before aggregating into
/// `__overflow__` (deleting a graph frees its slot).
const MAX_GRAPH_LABELS: usize = 128;
const OVERFLOW_LABEL: &str = "__overflow__";

fn bounded_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

impl KernelMetrics {
    /// Map a graph name onto the bounded label space.
    fn graph_label(&self, graph: &str) -> String {
        let mut seen = self.seen_graphs.lock();
        if seen.contains(graph) {
            graph.to_string()
        } else if seen.len() < MAX_GRAPH_LABELS {
            seen.insert(graph.to_string());
            graph.to_string()
        } else {
            OVERFLOW_LABEL.to_string()
        }
    }

    /// Record one request of type `op` taking `seconds`.
    pub fn record_request(&self, op: &str, seconds: f64) {
        self.requests_total.with_label_values(&[op]).inc();
        self.request_duration
            .with_label_values(&[op])
            .observe(seconds);
    }

    /// Record a connection request starting, with the permits left.
    pub fn connection_request_started(&self, permits_available: usize) {
        self.in_flight.inc();
        self.permits_available.set(permits_available as i64);
    }

    /// Record a connection request finishing, with the permits left.
    pub fn connection_request_finished(&self, permits_available: usize) {
        self.in_flight.dec();
        self.permits_available.set(permits_available as i64);
    }

    /// Count a request refused because the server was busy.
    pub fn busy_rejected(&self) {
        self.busy_rejections.inc();
    }

    /// Count a read admitted through the reserved read lane.
    pub fn read_reserved_admitted(&self) {
        self.read_reserved_admitted.inc();
    }

    /// Record one dispatch abandoned at the hard per-request deadline
    /// (CONCEPT:MEDULLA-KG.coordination.backpressure-busy-signal). Every increment is one
    /// request whose admission permits would otherwise have been held forever.
    pub fn dispatch_deadline_exceeded(&self) {
        self.dispatch_deadline_exceeded.inc();
    }

    /// Record one request admitted by the QoS gate in priority class `class` (W2.4):
    /// bumps the per-class admit counter and the per-class in-flight gauge (released by
    /// `qos_dispatch_finished`).
    pub fn qos_admitted(&self, class: &str) {
        self.qos_admitted.with_label_values(&[class]).inc();
        self.qos_class_in_flight.with_label_values(&[class]).inc();
    }

    /// Record one request shed by the QoS gate in priority class `class` for `reason`
    /// (W2.4). Under pressure the lowest class is shed first (`backpressure`).
    pub fn qos_shed(&self, class: &str, reason: &str) {
        self.qos_shed.with_label_values(&[class, reason]).inc();
    }

    /// Record one resolved write conflict (MEDULLA-DURABLE-KERNEL-R084.5).
    pub fn write_conflict(&self, strategy: &str, winner: &str) {
        self.write_conflicts
            .with_label_values(&[strategy, winner])
            .inc();
    }

    /// Record a QoS-admitted request finishing dispatch after `seconds` (W2.4): observes
    /// the per-class latency histogram and releases the per-class in-flight gauge.
    pub fn qos_dispatch_finished(&self, class: &str, seconds: f64) {
        self.qos_class_latency
            .with_label_values(&[class])
            .observe(seconds);
        self.qos_class_in_flight.with_label_values(&[class]).dec();
    }

    /// Count one operation on `graph`.
    pub fn graph_op(&self, graph: &str) {
        self.graph_ops
            .with_label_values(&[&self.graph_label(graph)])
            .inc();
    }

    /// Set the node and edge counts for `graph`.
    pub fn set_graph_size(&self, graph: &str, nodes: i64, edges: i64) {
        let label = self.graph_label(graph);
        self.graph_nodes.with_label_values(&[&label]).set(nodes);
        self.graph_edges.with_label_values(&[&label]).set(edges);
    }

    /// Set the per-graph resident-memory estimate gauge (CONCEPT:MEDULLA-KG.compute.lane-v), in bytes.
    pub fn set_graph_memory(&self, graph: &str, bytes: i64) {
        self.graph_memory_bytes
            .with_label_values(&[&self.graph_label(graph)])
            .set(bytes);
    }

    /// Mark a graph hibernated (1) or resident (0) (CONCEPT:MEDULLA-KG.compute.lane-v).
    pub fn set_graph_hibernated(&self, graph: &str, hibernated: bool) {
        self.graph_hibernated
            .with_label_values(&[&self.graph_label(graph)])
            .set(hibernated as i64);
    }

    /// Publish process-wide ResourceStats signals without introducing any
    /// caller-visible graph labels.  The values are deliberately gauges because
    /// they describe the current effective capacity/queue state; the operation
    /// total is sourced from the coalescer's monotonic atomic and refreshed on
    /// each snapshot.
    pub fn set_resource_stats(
        &self,
        effective_cpu_cores: i64,
        effective_memory_limit_bytes: i64,
        process_rss_bytes: i64,
        coalescer_queue_depth: i64,
        coalescer_queue_bytes: i64,
        coalescer_operations_total: i64,
    ) {
        self.effective_cpu_cores.set(effective_cpu_cores);
        self.effective_memory_limit_bytes
            .set(effective_memory_limit_bytes);
        self.process_rss_bytes.set(process_rss_bytes);
        self.coalescer_queue_depth.set(coalescer_queue_depth);
        self.coalescer_queue_bytes.set(coalescer_queue_bytes);
        self.coalescer_operations_total
            .set(coalescer_operations_total);
    }

    /// Set the write coalescer queue depth, queued bytes and applied operations.
    pub fn set_coalescer_stats(&self, queue_depth: u64, queue_bytes: u64, operations_total: u64) {
        self.coalescer_queue_depth.set(bounded_i64(queue_depth));
        self.coalescer_queue_bytes.set(bounded_i64(queue_bytes));
        self.coalescer_operations_total
            .set(bounded_i64(operations_total));
    }

    /// Record `n` LRU nodes evicted by the per-tenant budget enforcer (CONCEPT:MEDULLA-KG.compute.lane-v).
    pub fn budget_evicted(&self, n: u64) {
        self.budget_evictions.inc_by(n);
    }

    /// Record one graph hibernated by the budget enforcer (CONCEPT:MEDULLA-KG.compute.lane-v).
    pub fn budget_hibernated(&self) {
        self.budget_hibernations.inc();
    }

    /// Record one query that crossed the slow-query threshold (CONCEPT:MEDULLA-OS.observability.slow-query-descriptor).
    pub fn slow_query(&self) {
        self.slow_queries.inc();
    }

    /// Forget a deleted graph: drop its series and free its label slot.
    pub fn drop_graph(&self, graph: &str) {
        let _ = self.graph_nodes.remove_label_values(&[graph]);
        let _ = self.graph_edges.remove_label_values(&[graph]);
        let _ = self.graph_ops.remove_label_values(&[graph]);
        let _ = self.graph_memory_bytes.remove_label_values(&[graph]);
        let _ = self.graph_hibernated.remove_label_values(&[graph]);
        let _ = self.write_batches.remove_label_values(&[graph]);
        let _ = self.write_batched_ops.remove_label_values(&[graph]);
        let _ = self.write_lock_wait.remove_label_values(&[graph]);
        let _ = self.write_lock_hold.remove_label_values(&[graph]);

        self.seen_graphs.lock().remove(graph);
        // The overflow bucket is deliberately a single bounded aggregate, not
        // a per-overflow-name set (which would itself grow to the million-graph
        // target).  Removing it on every graph delete is safe: any still-live
        // overflow graph recreates the one aggregate series on its next metric
        // event, while a same-name recreate cannot inherit stale state.
        let _ = self.graph_nodes.remove_label_values(&[OVERFLOW_LABEL]);
        let _ = self.graph_edges.remove_label_values(&[OVERFLOW_LABEL]);
        let _ = self.graph_ops.remove_label_values(&[OVERFLOW_LABEL]);
        let _ = self
            .graph_memory_bytes
            .remove_label_values(&[OVERFLOW_LABEL]);
        let _ = self.graph_hibernated.remove_label_values(&[OVERFLOW_LABEL]);
        let _ = self.write_batches.remove_label_values(&[OVERFLOW_LABEL]);
        let _ = self
            .write_batched_ops
            .remove_label_values(&[OVERFLOW_LABEL]);
        let _ = self.write_lock_wait.remove_label_values(&[OVERFLOW_LABEL]);
        let _ = self.write_lock_hold.remove_label_values(&[OVERFLOW_LABEL]);
    }

    /// Record a checkpoint that took `seconds`.
    pub fn checkpoint_completed(&self, seconds: f64) {
        self.checkpoint_duration.observe(seconds);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.checkpoint_last_success.set(now as i64);
    }

    /// Publish authoritative job-store queue state for autoscaling and SLOs.
    pub fn set_analytics_job_counts(&self, ready: i64, active: i64, publishing: i64) {
        self.analytics_jobs_ready.set(ready);
        self.analytics_jobs_active.set(active);
        self.analytics_jobs_publishing.set(publishing);
    }

    /// Count an authentication failure.
    pub fn auth_failure(&self) {
        self.auth_failures.inc();
    }

    /// Count an access-control denial.
    pub fn access_denied(&self) {
        self.access_denied.inc();
    }

    /// CA-11: publish the Kafka sink's current queued-but-unacked count.
    pub fn cdc_kafka_sink_lag(&self, events: i64) {
        self.cdc_kafka_sink_lag.set(events);
    }

    /// CA-11: count one failed `CdcSink::emit` call (queue-full/broker error).
    pub fn cdc_kafka_sink_send_failed(&self) {
        self.cdc_kafka_sink_send_failures.inc();
    }

    /// Record one committed coalesced write batch of `ops` single-op writes against
    /// `graph` (CONCEPT:MEDULLA-KG.sharding.per-graph-write-coalescer): +1 lock acquisition, +`ops` applied writes.
    pub fn write_batch_committed(&self, graph: &str, ops: usize) {
        let label = self.graph_label(graph);
        self.write_batches.with_label_values(&[&label]).inc();
        self.write_batched_ops
            .with_label_values(&[&label])
            .inc_by(ops as u64);
    }

    /// Observe the time one write waited from enqueue to acquiring the per-graph
    /// topology write lock (CONCEPT:MEDULLA-OS.observability.write-lock-gap-histogram) — the starvation cost a reader/late-writer
    /// pays behind the `__commons__` ingestion firehose.
    pub fn observe_write_lock_wait(&self, graph: &str, seconds: f64) {
        self.write_lock_wait
            .with_label_values(&[&self.graph_label(graph)])
            .observe(seconds);
    }

    /// Observe the time the per-graph topology write lock was held to apply one
    /// coalesced batch (CONCEPT:MEDULLA-OS.observability.write-lock-gap-histogram) — the contention cost; reads block for this.
    pub fn observe_write_lock_hold(&self, graph: &str, seconds: f64) {
        self.write_lock_hold
            .with_label_values(&[&self.graph_label(graph)])
            .observe(seconds);
    }

    /// Observe how long a dispatched request blocked acquiring the process-wide
    /// `ServerState` lock (D-EIMG-2). `mode` is `"read"` or `"write"`. Called from the
    /// `timed_read`/`timed_write` chokepoint helpers in `medulla_server::dispatch`, never
    /// inline at an individual call site.
    pub fn observe_dispatch_lock_wait(&self, mode: &str, seconds: f64) {
        self.dispatch_lock_wait
            .with_label_values(&[mode])
            .observe(seconds);
    }

    /// Record one `commit_ops` phase's duration (EH-290).
    pub fn observe_commit_ops_phase(&self, phase: &str, seconds: f64) {
        self.commit_ops_phase
            .with_label_values(&[phase])
            .observe(seconds);
    }

    /// Record one storage-scrub pass (EH-384): rows opened, one unreadable
    /// row per `causes` entry (an `UnsealFailure::code`), and whether the pass
    /// completed a cycle.
    pub fn storage_scrub_pass(&self, rows: u64, causes: &[&str], completed_cycle: bool) {
        self.storage_scrub_rows.inc_by(rows);
        for cause in causes {
            self.storage_scrub_unreadable
                .with_label_values(&[cause])
                .inc();
        }
        if completed_cycle {
            self.storage_scrub_cycles.inc();
        }
    }

    /// Record one kernel-owned store open's phase split (R006.2).
    pub fn store_open(&self, phases: &[(&str, f64)], outcome: &str) {
        for (phase, seconds) in phases {
            self.store_open_phase
                .with_label_values(&[phase])
                .observe(*seconds);
        }
        self.store_open_scan.with_label_values(&[outcome]).inc();
    }

    /// Record one `commit_ops` drain's bytes (EH-290): `kind` is `logical`/`physical_delta`.
    pub fn observe_commit_ops_bytes(&self, kind: &str, bytes: u64) {
        self.commit_ops_bytes
            .with_label_values(&[kind])
            .observe(bytes as f64);
    }

    /// Record a hit against the RLS projection cache (D-EGP-1): the actor's
    /// materialized `GraphCore` was reused, no rebuild needed. Called from
    /// `GraphReadAuthority::project_core_active` on the fast path.
    pub fn projection_cache_hit(&self) {
        self.projection_cache_result
            .with_label_values(&["hit"])
            .inc();
    }

    /// Record a miss against the RLS projection cache (D-EGP-1) and the wall-clock
    /// cost of the resulting rebuild, `seconds`. A miss is either cold (first read
    /// for this actor) or stale (a committed write advanced `GraphCore::version()`
    /// since the last projection for this actor). Called from
    /// `GraphReadAuthority::project_core_active` after `build_projection` returns.
    pub fn projection_cache_miss(&self, seconds: f64) {
        self.projection_cache_result
            .with_label_values(&["miss"])
            .inc();
        self.projection_cache_miss_build.observe(seconds);
    }

    /// Record one completed tick of an engine-native background interval loop
    /// (daemon-consolidation design, Phase 2): `name` is a fixed loop identifier
    /// (e.g. `"cold_offload"`), `seconds` is the tick's own work duration (the sweep
    /// body only, not the sleep between ticks). Pure observability — call this AFTER
    /// a tick's existing logic runs, with zero change to what that logic does.
    pub fn loop_tick(&self, name: &str, seconds: f64) {
        self.loop_ticks_total.with_label_values(&[name]).inc();
        self.loop_tick_duration
            .with_label_values(&[name])
            .observe(seconds);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.loop_last_run
            .with_label_values(&[name])
            .set(now as i64);
    }

    /// Set the `medulla_epistemic_materializations_stale` gauge to the
    /// TruthMaintenance index's CURRENT stale count (CONCEPT:MEDULLA-KG.epistemic.truth-maintenance).
    pub fn set_epistemic_materializations_stale(&self, n: i64) {
        self.epistemic_materializations_stale.set(n);
    }

    /// Record `n` materialization ids staled by one `TruthMaintenance::on_change` call.
    pub fn epistemic_materializations_staled(&self, n: u64) {
        self.epistemic_materializations_staled_total.inc_by(n);
    }

    /// Record one lifecycle-statechart mirror divergence (ADR-5 phase-1 dual-write).
    /// `machine` is a fixed small label (`"work_item"` / `"loop"`).
    pub fn statechart_divergence(&self, machine: &str) {
        self.statechart_divergence_total
            .with_label_values(&[machine])
            .inc();
    }

    /// Render the full registry in Prometheus text exposition format.
    pub fn render(&self) -> String {
        let encoder = TextEncoder::new();
        let mut buf = Vec::new();
        if encoder.encode(&self.registry.gather(), &mut buf).is_err() {
            return String::new();
        }
        String::from_utf8(buf).unwrap_or_default()
    }
}
