//! The port `Discover` reads drift and adapter evidence through
//! (MEDULLA-FEDERATED-QUERY-R089.7).
//!
//! The decision engine owns the drift monitor (MEDULLA-DECISION-ENGINE-R174.8) and the
//! scoped query adapter (MEDULLA-DECISION-ENGINE-R174.7). The retrieval verdict never
//! depends on those crates: it reads one [`DiscoverSignals`] implementation installed at
//! server start, and a response reads it once so no response mixes generations.

use std::sync::{Arc, OnceLock};

/// The adapter digest a verdict names when no adapter re-aimed the probe.
pub const BASE_ADAPTER: &str = "base";

/// One read of the drift and adapter evidence for the shards a query touches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SignalsReading {
    /// Shards whose drift signal is firing.
    pub firing_shards: Vec<String>,
    /// Each candidate shard and the generation it serves.
    pub shard_generations: Vec<(String, u64)>,
    /// Digest of the active adapter, `None` when the base space answered.
    pub adapter_digest: Option<String>,
}

/// One written vector, as the commit path reports it to the drift monitors.
#[derive(Debug, Clone, Copy)]
pub struct WrittenVector<'a> {
    /// The verified carrier's tenant scope; monitors are never shared across tenants.
    pub tenant: &'a str,
    /// The graph the vector was committed to.
    pub graph: &'a str,
    /// The shard that stores the graph.
    pub shard: u32,
    /// The node the vector belongs to.
    pub node_id: &'a str,
    /// The node's type.
    pub node_type: &'a str,
    /// The embedding that was committed.
    pub vector: &'a [f32],
    /// The text tokens of the node.
    pub tokens: &'a [String],
    /// The graph version the write committed at.
    pub version: u64,
}

/// Drift and adapter evidence, read once per `Discover` response, and the write feed
/// that keeps it current.
pub trait DiscoverSignals: Send + Sync {
    /// Read the evidence for the caller's graph under the caller's tenant.
    fn read(&self, tenant: &str, graph: &str) -> SignalsReading;

    /// One committed embedding write (the write commit path of
    /// MEDULLA-DECISION-ENGINE-R174.8.3).
    fn observe_write(&self, write: &WrittenVector<'_>);
}

static SIGNALS: OnceLock<Arc<dyn DiscoverSignals>> = OnceLock::new();

/// Install the signal source, once, at server start. A second install is refused.
pub fn set_discover_signals(signals: Arc<dyn DiscoverSignals>) -> bool {
    SIGNALS.set(signals).is_ok()
}

/// The installed signal source, if any. With none installed a verdict reports no drift,
/// no shard generations and the `base` adapter.
pub fn discover_signals() -> Option<Arc<dyn DiscoverSignals>> {
    SIGNALS.get().cloned()
}
