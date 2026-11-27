//! Sensitive-column scan of the `DatasetProfile` job (MEDULLA-TYPED-PACKS-R803.3).
//!
//! The profile executor (`executor_profile.rs`) calls [`SensitiveScan::scan`] once per
//! profiled column: it reads the first `sensitive.max_sample_values` non-null values in the
//! profile read order and hands them to `medulla_core::sensitive_scan::scan_values`. The
//! detector set is loaded and validated by [`super::sensitive_claims::load_detectors`], the
//! one loader the document scan shares, before the first scan; a refusal fails the attempt
//! before any claim is written. The passing detections become `proposed` claims through
//! [`super::sensitive_claims::claim_methods`], inside the job's mutation batch. No second
//! caller scans values.

use medulla_catalog_source_types::profile::ColumnProfile;
use medulla_core::sensitive_scan::{scan_values, NoResources, ScanLimits};
use medulla_query_types::sensitive_detect::{Detection, SensitiveDetector};

use super::executor_profile::{ProfileRefusal, ProfileSource};
use super::sensitive_claims::{load_detectors, SensitiveRefusal};

/// The detectors and limits of one profile run.
pub(super) struct SensitiveScan {
    pub detectors: Vec<SensitiveDetector>,
    pub limits: ScanLimits,
    pub now_ms: i64,
}

fn refused(refusal: SensitiveRefusal) -> ProfileRefusal {
    ProfileRefusal {
        code: refusal.code,
        detail: refusal.detail,
    }
}

impl SensitiveScan {
    /// Load the detectors of `tenant` and validate every one and the limits before
    /// the first scan: a refusal names the detector and the job writes no claim.
    pub(super) fn load(
        core: &medulla_graph_core::graph::GraphCore,
        tenant: &str,
        now_ms: i64,
    ) -> Result<Self, ProfileRefusal> {
        let (detectors, limits) = load_detectors(core, tenant).map_err(refused)?;
        Ok(Self {
            detectors,
            limits,
            now_ms,
        })
    }

    /// Scan one profiled column of `source`: the first `max_sample_values` non-null
    /// values in read order, once.
    pub(super) fn scan(
        &self,
        source: &dyn ProfileSource,
        profile: &ColumnProfile,
    ) -> Result<Vec<Detection>, ProfileRefusal> {
        if !source.supports_sampling() {
            return Ok(Vec::new());
        }
        let cap = self.limits.max_sample_values;
        let mut sample: Vec<String> = Vec::new();
        source
            .scan_column(&profile.column, &mut |value| {
                if sample.len() < cap {
                    if let Some(value) = value {
                        sample.push(value.to_string());
                    }
                }
            })
            .map_err(|detail| ProfileRefusal {
                code: super::executor_profile::PROFILE_UNSUPPORTED,
                detail,
            })?;
        let values: Vec<Option<&str>> = sample.iter().map(|value| Some(value.as_str())).collect();
        let asset = column_asset(profile);
        scan_values(
            &asset,
            &values,
            &self.detectors,
            &self.limits,
            &NoResources,
            self.now_ms,
        )
        .map_err(|error| refused(SensitiveRefusal::of(error)))
    }
}

/// The catalog column node a profile is about.
fn column_asset(profile: &ColumnProfile) -> String {
    medulla_catalog_source_types::lineage::column_node_id(&format!(
        "{}.{}",
        profile.dataset, profile.column
    ))
}
