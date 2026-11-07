//! Materialized-view metrics (MEDULLA-INCREMENTAL-VIEWS-R005.4).
//!
//! Labels come from closed sets: the view family (`plan`, `sql`), the demotion
//! reason code and the shadow-check outcome. A view or graph name is never a
//! label, so cardinality is bounded by the fixed vocabularies.

use super::KernelMetrics;

#[cfg(feature = "metrics")]
/// The closed family vocabulary the resident total sums over.
const FAMILIES: [&str; 2] = ["plan", "sql"];

#[cfg(feature = "metrics")]
impl KernelMetrics {
    /// Count `changes` folded into views of `family`.
    pub fn view_changes_applied(&self, family: &str, changes: u64) {
        self.view_changes_applied
            .with_label_values(&[family])
            .inc_by(changes);
    }

    /// Count one demotion of a `family` view for `reason_code`.
    pub fn view_demoted(&self, family: &str, reason_code: &str) {
        self.view_demotions
            .with_label_values(&[family, reason_code])
            .inc();
    }

    /// Count one shadow check of a `family` view with its `outcome`.
    pub fn view_shadow_checked(&self, family: &str, outcome: &str) {
        self.view_shadow_checks
            .with_label_values(&[family, outcome])
            .inc();
    }

    /// Publish the maintained state of every `family` view, and the resident total.
    pub fn set_view_state_bytes(&self, family: &str, bytes: u64) {
        self.view_state_bytes
            .with_label_values(&[family])
            .set(i64::try_from(bytes).unwrap_or(i64::MAX));
        let total: i64 = FAMILIES
            .iter()
            .map(|f| self.view_state_bytes.with_label_values(&[f]).get())
            .sum();
        self.view_resident_bytes.set(total);
    }
}

#[cfg(not(feature = "metrics"))]
#[allow(missing_docs)] // no-op shims; the metrics build documents the same functions
impl KernelMetrics {
    pub fn view_changes_applied(&self, _family: &str, _changes: u64) {}
    pub fn view_demoted(&self, _family: &str, _reason_code: &str) {}
    pub fn view_shadow_checked(&self, _family: &str, _outcome: &str) {}
    pub fn set_view_state_bytes(&self, _family: &str, _bytes: u64) {}
}
