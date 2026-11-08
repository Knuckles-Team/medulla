//! Uncertain-effect metrics (MEDULLA-DURABLE-KERNEL-R820.5).
//!
//! Updated by the served handler after the commit, never inside the store
//! crate. The `outcome` label is a closed set: `started` for a start mark and
//! the receipt outcome (`applied`, `not_applied`, `compensated`) for a receipt.

use super::KernelMetrics;

#[cfg(feature = "metrics")]
impl KernelMetrics {
    /// Count one committed uncertain-effect write.
    pub fn work_item_uncertain_effect(&self, outcome: &str) {
        self.work_item_uncertain_effects_total
            .with_label_values(&[outcome])
            .inc();
    }

    /// Move the open gauge by `delta`; it never goes below zero.
    pub fn work_item_uncertain_open_add(&self, delta: i64) {
        let next = self
            .work_item_uncertain_open
            .get()
            .saturating_add(delta)
            .max(0);
        self.work_item_uncertain_open.set(next);
    }
}

#[cfg(not(feature = "metrics"))]
#[allow(missing_docs)] // no-op shims; the metrics build documents the same functions
impl KernelMetrics {
    pub fn work_item_uncertain_effect(&self, _outcome: &str) {}
    pub fn work_item_uncertain_open_add(&self, _delta: i64) {}
}
