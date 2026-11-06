//! Uncertain-effect metrics (MEDULLA-DURABLE-KERNEL-R820.5).
//!
//! Updated by the served handler after the commit, never inside the store
//! crate. The `outcome` label is a closed set: `started` for a start mark and
//! the receipt outcome (`applied`, `not_applied`, `compensated`) for a receipt.

#[cfg(feature = "metrics")]
mod imp {
    use lazy_static::lazy_static;
    use prometheus::{IntCounterVec, IntGauge};

    use super::super::imp::counter_vec;

    lazy_static! {
        static ref UNCERTAIN_EFFECTS_TOTAL: IntCounterVec = counter_vec(
            "medulla_work_item_uncertain_effects_total",
            "Uncertain-effect writes committed, by outcome",
            &["outcome"],
        );
        static ref UNCERTAIN_OPEN: IntGauge = {
            let gauge = IntGauge::new(
                "medulla_work_item_uncertain_open",
                "Work items marked effect-started since boot that no receipt has resolved",
            )
            .expect("valid metric");
            super::super::imp::REGISTRY
                .register(Box::new(gauge.clone()))
                .expect("unique metric");
            gauge
        };
    }

    /// Count one committed uncertain-effect write.
    pub fn work_item_uncertain_effect(outcome: &str) {
        UNCERTAIN_EFFECTS_TOTAL.with_label_values(&[outcome]).inc();
    }

    /// Move the open gauge by `delta`; it never goes below zero.
    pub fn work_item_uncertain_open_add(delta: i64) {
        let next = UNCERTAIN_OPEN.get().saturating_add(delta).max(0);
        UNCERTAIN_OPEN.set(next);
    }
}

#[cfg(not(feature = "metrics"))]
#[allow(missing_docs)] // no-op shims; the metrics build documents the same functions
mod imp {
    pub fn work_item_uncertain_effect(_outcome: &str) {}
    pub fn work_item_uncertain_open_add(_delta: i64) {}
}

pub use imp::*;
