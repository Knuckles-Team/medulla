//! Write-conflict outcome counter (MEDULLA-DURABLE-KERNEL-R084.5).
//!
//! `medulla_write_conflicts_total{strategy,winner}` counts every stale
//! `ApplyChangeEnvelope` version claim by the strategy applied and the side
//! that won. Both labels come from closed sets.

#[cfg(feature = "metrics")]
mod imp {
    use lazy_static::lazy_static;
    use prometheus::IntCounterVec;

    use super::super::imp::counter_vec;

    lazy_static! {
        static ref WRITE_CONFLICTS: IntCounterVec = counter_vec(
            "medulla_write_conflicts_total",
            "Stale write-version conflicts, by strategy and winner",
            &["strategy", "winner"],
        );
    }

    /// Count one write-conflict outcome.
    pub fn write_conflict(strategy: &str, winner: &str) {
        WRITE_CONFLICTS.with_label_values(&[strategy, winner]).inc();
    }

    /// The current count for one `(strategy, winner)` pair.
    pub fn write_conflict_count(strategy: &str, winner: &str) -> u64 {
        WRITE_CONFLICTS.with_label_values(&[strategy, winner]).get()
    }
}

#[cfg(not(feature = "metrics"))]
#[allow(missing_docs)] // no-op shims; the metrics build documents the same functions
mod imp {
    pub fn write_conflict(_strategy: &str, _winner: &str) {}
    pub fn write_conflict_count(_strategy: &str, _winner: &str) -> u64 {
        0
    }
}

pub use imp::*;
