//! Retention cleanup series (MEDULLA-DURABLE-KERNEL-R700.5).
//!
//! `medulla_retention_cleanup_removed_total{reason}` counts the nodes a cleanup
//! step removed once its batch committed. The `reason` label is one of
//! `aged_document` and `orphan`, never a graph or node name.

use super::KernelMetrics;

#[cfg(feature = "metrics")]
impl KernelMetrics {
    /// Count `count` nodes removed for `reason` by a committed cleanup step.
    pub fn retention_cleanup_removed(&self, reason: &str, count: u64) {
        self.retention_cleanup_removed_total
            .with_label_values(&[reason])
            .inc_by(count);
    }
}

#[cfg(not(feature = "metrics"))]
#[allow(missing_docs)] // no-op shim; the metrics build documents the same function
impl KernelMetrics {
    pub fn retention_cleanup_removed(&self, _reason: &str, _count: u64) {}
}
