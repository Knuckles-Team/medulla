// CONCEPT:MEDULLA-OS.observability.engine-metrics — Engine Observability (Prometheus metrics)
//
// First-class metrics for the Rust tier: request rate/latency per operation,
// in-flight admission state, per-graph activity and size, checkpoint health,
// and security counters (auth failures / ACL denials). Exposed in Prometheus
// text format by a tiny HTTP listener (`serve`) bound via `--metrics-addr` /
// `GRAPH_SERVICE_METRICS_ADDR` — fully separate from the MessagePack RPC
// transport, disabled unless an address is configured.
//
// The module compiles to no-op shims when the `metrics` cargo feature (on by
// default) is disabled, so call sites never need `#[cfg]` guards.
//
// Cardinality: `op` is bounded by the protocol's `Method` variants. The
// `graph` label is bounded by `MAX_GRAPH_LABELS` distinct names — graphs past
// the cap aggregate under `__overflow__` so an unbounded tenant namespace can
// never blow up the time-series store.

// State (MEDULLA-SDLC-R023): every series lives in one owned `KernelMetrics`
// value with its own registry. Two values share nothing; the process holds one
// (`process()`), which the exposition endpoint and the OTLP export render. A
// caller that holds state records on the value it is handed; the call sites
// that still reach `process()` are the remaining holders to wire.

#[cfg(feature = "metrics")]
mod series;
#[cfg(feature = "metrics")]
pub use series::KernelMetrics;
#[cfg(feature = "metrics")]
mod record;

/// The kernel series for a build without the `metrics` feature: no state.
#[cfg(not(feature = "metrics"))]
#[derive(Debug, Default)]
pub struct KernelMetrics;

#[cfg(not(feature = "metrics"))]
impl KernelMetrics {
    /// The empty value.
    pub fn new() -> Self {
        Self
    }
}

/// The process's kernel series: the one value the exposition endpoint renders.
#[cfg(feature = "metrics")]
pub fn process() -> &'static KernelMetrics {
    static PROCESS: std::sync::OnceLock<KernelMetrics> = std::sync::OnceLock::new();
    PROCESS.get_or_init(KernelMetrics::new)
}

/// The process's kernel series (none without the `metrics` feature).
#[cfg(not(feature = "metrics"))]
pub fn process() -> &'static KernelMetrics {
    &KernelMetrics
}

// X10 outbox consumer metrics.
mod outbox;
// Automatic store migration at open (MEDULLA-DURABLE-KERNEL-R070.1).
mod store_migration;
// Uncertain-effect series (MEDULLA-DURABLE-KERNEL-R820.5).
mod work_item;
// Materialized-view metrics (MEDULLA-INCREMENTAL-VIEWS-R005.4).
/// The Decide layer's own series (EH-074), registered on the same registry.
mod decision;
mod views;

// ── No-op methods (feature `metrics` disabled) ─────────────────────────────
#[cfg(not(feature = "metrics"))]
mod noop;

// The exposition endpoint, in its own file.
#[cfg(all(feature = "metrics", feature = "server"))]
mod serve;

#[cfg(all(test, feature = "metrics"))]
mod tests;
