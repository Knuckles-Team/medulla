//! Automatic store migration metrics (MEDULLA-DURABLE-KERNEL-R070.1).
//!
//! The engine migrates before it serves, so these samples are on the registry
//! when the exposition endpoint starts. Labels come from closed sets: the step
//! (`run`, `resume`, `journal`, `store`) and its outcome or refusal code.

use super::KernelMetrics;

#[cfg(feature = "metrics")]
impl KernelMetrics {
    /// Count one step of an automatic store migration.
    pub fn store_migration_step(&self, step: &str, outcome: &str) {
        self.store_migration_steps_total
            .with_label_values(&[step, outcome])
            .inc();
    }
}

#[cfg(not(feature = "metrics"))]
#[allow(missing_docs)] // no-op shims; the metrics build documents the same functions
impl KernelMetrics {
    pub fn store_migration_step(&self, _step: &str, _outcome: &str) {}
}
