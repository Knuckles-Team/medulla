//! No-op shims for a build without the `metrics` feature: call sites stay
//! free of `#[cfg]` guards and the optimizer erases these.

use super::KernelMetrics;

#[allow(missing_docs)] // no-op shims; the metrics build documents the same functions
impl KernelMetrics {
    pub fn record_request(&self, _op: &str, _seconds: f64) {}
    pub fn connection_request_started(&self, _permits_available: usize) {}
    pub fn connection_request_finished(&self, _permits_available: usize) {}
    pub fn busy_rejected(&self) {}
    pub fn read_reserved_admitted(&self) {}
    pub fn dispatch_deadline_exceeded(&self) {}
    pub fn asset_usage_dropped(&self) {}
    pub fn qos_admitted(&self, _class: &str) {}
    pub fn qos_shed(&self, _class: &str, _reason: &str) {}
    pub fn write_conflict(&self, _strategy: &str, _winner: &str) {}
    pub fn qos_dispatch_finished(&self, _class: &str, _seconds: f64) {}
    pub fn graph_op(&self, _graph: &str) {}
    pub fn set_graph_size(&self, _graph: &str, _nodes: i64, _edges: i64) {}
    pub fn set_graph_memory(&self, _graph: &str, _bytes: i64) {}
    pub fn set_graph_hibernated(&self, _graph: &str, _hibernated: bool) {}
    pub fn set_resource_stats(
        &self,
        _effective_cpu_cores: i64,
        _effective_memory_limit_bytes: i64,
        _process_rss_bytes: i64,
        _coalescer_queue_depth: i64,
        _coalescer_queue_bytes: i64,
        _coalescer_operations_total: i64,
    ) {
    }
    pub fn set_coalescer_stats(
        &self,
        _queue_depth: u64,
        _queue_bytes: u64,
        _operations_total: u64,
    ) {
    }
    pub fn budget_evicted(&self, _n: u64) {}
    pub fn budget_hibernated(&self) {}
    pub fn slow_query(&self) {}
    pub fn drop_graph(&self, _graph: &str) {}
    pub fn checkpoint_completed(&self, _seconds: f64) {}
    pub fn store_open(&self, _phases: &[(&str, f64)], _outcome: &str) {}
    pub fn set_analytics_job_counts(&self, _ready: i64, _active: i64, _publishing: i64) {}
    pub fn auth_failure(&self) {}
    pub fn access_denied(&self) {}
    pub fn cdc_kafka_sink_lag(&self, _events: i64) {}
    pub fn cdc_kafka_sink_send_failed(&self) {}
    pub fn write_batch_committed(&self, _graph: &str, _ops: usize) {}
    pub fn observe_write_lock_wait(&self, _graph: &str, _seconds: f64) {}
    pub fn observe_write_lock_hold(&self, _graph: &str, _seconds: f64) {}
    pub fn observe_dispatch_lock_wait(&self, _mode: &str, _seconds: f64) {}
    pub fn observe_commit_ops_phase(&self, _phase: &str, _seconds: f64) {}
    pub fn observe_commit_ops_bytes(&self, _kind: &str, _bytes: u64) {}
    pub fn storage_scrub_pass(&self, _rows: u64, _causes: &[&str], _completed_cycle: bool) {}
    pub fn projection_cache_hit(&self) {}
    pub fn projection_cache_miss(&self, _seconds: f64) {}
    pub fn loop_tick(&self, _name: &str, _seconds: f64) {}
    pub fn set_epistemic_materializations_stale(&self, _n: i64) {}
    pub fn epistemic_materializations_staled(&self, _n: u64) {}
    pub fn statechart_divergence(&self, _machine: &str) {}
    pub fn render(&self) -> String {
        String::new()
    }
}
