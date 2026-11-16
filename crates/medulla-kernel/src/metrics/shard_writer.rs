//! Shard-writer series (MEDULLA-DURABLE-KERNEL-R099.5).
//!
//! `medulla_shard_writer_inflight{shard}` counts sub-batches being dispatched
//! on one shard writer and `medulla_shard_writer_commit_seconds{shard}` times
//! each dispatch. The `shard` label is the shard index, bounded by the
//! configured writer count k (at most eight by default), never a graph name.

use super::KernelMetrics;

#[cfg(feature = "metrics")]
impl KernelMetrics {
    /// One sub-batch starts its dispatch on `shard`.
    pub fn shard_writer_begin(&self, shard: usize) {
        self.shard_writer_inflight
            .with_label_values(&[&shard.to_string()])
            .inc();
    }

    /// One sub-batch finished its dispatch on `shard` after `seconds`.
    pub fn shard_writer_end(&self, shard: usize, seconds: f64) {
        let label = shard.to_string();
        self.shard_writer_inflight
            .with_label_values(&[&label])
            .dec();
        self.shard_writer_commit_seconds
            .with_label_values(&[&label])
            .observe(seconds);
    }
}

#[cfg(not(feature = "metrics"))]
#[allow(missing_docs)] // no-op shims; the metrics build documents the same functions
impl KernelMetrics {
    pub fn shard_writer_begin(&self, _shard: usize) {}
    pub fn shard_writer_end(&self, _shard: usize, _seconds: f64) {}
}
