//! Materialized-view metrics (MEDULLA-INCREMENTAL-VIEWS-R005.4).
//!
//! Labels come from closed sets: the view family (`plan`, `sql`), the demotion
//! reason code and the shadow-check outcome. A view or graph name is never a
//! label, so cardinality is bounded by the fixed vocabularies.

#[cfg(feature = "metrics")]
mod imp {
    use lazy_static::lazy_static;
    use prometheus::{IntCounterVec, IntGauge, IntGaugeVec};

    use super::super::imp::{counter_vec, gauge, gauge_vec};

    /// The closed family vocabulary the resident total sums over.
    const FAMILIES: [&str; 2] = ["plan", "sql"];

    lazy_static! {
        static ref CHANGES_APPLIED: IntCounterVec = counter_vec(
            "medulla_view_changes_applied_total",
            "Changes folded into incremental views, by view family",
            &["family"],
        );
        static ref DEMOTIONS: IntCounterVec = counter_vec(
            "medulla_view_demotions_total",
            "Views demoted out of incremental mode, by family and reason code",
            &["family", "reason_code"],
        );
        static ref SHADOW_CHECKS: IntCounterVec = counter_vec(
            "medulla_view_shadow_checks_total",
            "Shadow recompute checks of incremental views, by family and outcome",
            &["family", "outcome"],
        );
        static ref STATE_BYTES: IntGaugeVec = gauge_vec(
            "medulla_view_state_bytes",
            "Maintained state of incremental views in bytes, by view family",
            &["family"],
        );
        static ref RESIDENT_BYTES: IntGauge = gauge(
            "medulla_views_resident_state_bytes",
            "Maintained state of every resident incremental view in bytes",
        );
    }

    /// Count `changes` folded into views of `family`.
    pub fn view_changes_applied(family: &str, changes: u64) {
        CHANGES_APPLIED.with_label_values(&[family]).inc_by(changes);
    }

    /// Count one demotion of a `family` view for `reason_code`.
    pub fn view_demoted(family: &str, reason_code: &str) {
        DEMOTIONS.with_label_values(&[family, reason_code]).inc();
    }

    /// Count one shadow check of a `family` view with its `outcome`.
    pub fn view_shadow_checked(family: &str, outcome: &str) {
        SHADOW_CHECKS.with_label_values(&[family, outcome]).inc();
    }

    /// Publish the maintained state of every `family` view, and the resident total.
    pub fn set_view_state_bytes(family: &str, bytes: u64) {
        STATE_BYTES
            .with_label_values(&[family])
            .set(i64::try_from(bytes).unwrap_or(i64::MAX));
        let total: i64 = FAMILIES
            .iter()
            .map(|f| STATE_BYTES.with_label_values(&[f]).get())
            .sum();
        RESIDENT_BYTES.set(total);
    }
}

#[cfg(not(feature = "metrics"))]
#[allow(missing_docs)] // no-op shims; the metrics build documents the same functions
mod imp {
    pub fn view_changes_applied(_family: &str, _changes: u64) {}
    pub fn view_demoted(_family: &str, _reason_code: &str) {}
    pub fn view_shadow_checked(_family: &str, _outcome: &str) {}
    pub fn set_view_state_bytes(_family: &str, _bytes: u64) {}
}

pub use imp::*;
