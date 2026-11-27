//! The unclassified-refusal series (MEDULLA-OBSERVE-R001.3).
//!
//! `medulla_refusal_unclassified_total{module}` counts the refusals a handler reported without a
//! declared code. The refusal path sits below this crate and keeps its own per-crate counts; each
//! scrape adds what changed since the last one, so the series never moves backwards and costs the
//! refusal path nothing beyond one atomic add and one map update.

#[cfg(feature = "metrics")]
use super::KernelMetrics;

#[cfg(feature = "metrics")]
impl KernelMetrics {
    /// Bring the series up to the counts the refusal path holds.
    pub(super) fn sync_unclassified_refusals(&self) {
        let current = medulla_protocol_types::protocol::unclassified_by_module();
        let mut synced = self.refusal_unclassified_synced.lock();
        for (module, total) in current {
            let seen = synced.entry(module.clone()).or_insert(0);
            if total > *seen {
                self.refusal_unclassified_total
                    .with_label_values(&[&module])
                    .inc_by(total - *seen);
                *seen = total;
            }
        }
    }
}
