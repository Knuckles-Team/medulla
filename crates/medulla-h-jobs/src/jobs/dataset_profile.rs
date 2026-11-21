//! Sensitive-column claims of the `DatasetProfile` job (MEDULLA-TYPED-PACKS-R803.3).
//!
//! The profile executor (`executor_profile.rs`) calls [`SensitiveScan::scan`] once per
//! profiled column: it reads the first `sensitive.max_sample_values` non-null values in the
//! profile read order and hands them to `medulla_core::sensitive_scan::scan_values`. Every
//! detector (the built-in set plus the tenant's `SensitiveDetector` nodes) is validated
//! before the first scan, and a refusal fails the attempt before any claim is written.
//! [`propose_claims`] turns the passing detections into one `ClassificationClaim` node in
//! state `proposed` per detector and column, linked to the catalog column node, inside the
//! job's mutation batch. A claim carries the detector id, the sample size, the hit ratio and
//! the scan time (MEDULLA-TYPED-PACKS-R803.5) and never a matched value. No second caller
//! scans values.

use medulla_catalog_types::profile::ColumnProfile;
use medulla_core::sensitive_scan::{passing, scan_values, NoResources, ScanError, ScanLimits};
use medulla_identity_types::classification::{
    ClaimKind, ClaimProvenance, ClaimRecord, ClaimState, ClassificationClaim,
    CLAIM_OF_RELATIONSHIP,
};
use medulla_identity_types::tenancy::Marking;
use medulla_query_types::sensitive_detect::{builtin_detectors, Detection, SensitiveDetector};

use super::data_plane_write::{edge_method, node_method};
use super::executor_profile::{ProfileRefusal, ProfileSource, ProfileTarget};
use super::prelude_server::*;

/// The node type of a tenant-defined detector.
pub(super) const SENSITIVE_DETECTOR_LABEL: &str = "SensitiveDetector";

/// The detectors and limits of one profile run.
pub(super) struct SensitiveScan {
    pub detectors: Vec<SensitiveDetector>,
    pub limits: ScanLimits,
    pub now_ms: i64,
}

fn refusal(error: ScanError) -> ProfileRefusal {
    ProfileRefusal {
        code: error.code(),
        detail: error.to_string(),
    }
}

/// The scan limits, read from the knob registry.
fn knob_limits() -> ScanLimits {
    let knobs = medulla_server_api::knob_config::knobs();
    ScanLimits {
        max_sample_values: knobs.sensitive_max_sample_values as usize,
        regex_size_limit: knobs.sensitive_regex_size_limit as usize,
        model_sample_values: knobs.sensitive_model_sample_values as usize,
    }
}

/// The tenant's own detector nodes: a node of type `SensitiveDetector` whose
/// `tenant_id` is the tenant's, decoded from its flat properties.
fn tenant_detectors(
    core: &medulla_graph_core::graph::GraphCore,
    tenant: &str,
) -> Result<Vec<SensitiveDetector>, ProfileRefusal> {
    let mut found = Vec::new();
    for (node_id, blob) in core.get_nodes_by_label(SENSITIVE_DETECTOR_LABEL, 0) {
        let Ok(mut fields) = medulla_types::msgpack::decode_property_object(&blob) else {
            continue;
        };
        if fields.get("tenant_id").and_then(serde_json::Value::as_str) != Some(tenant) {
            continue;
        }
        for key in ["type", "tenant_id", "_owner"] {
            fields.remove(key);
        }
        let detector: SensitiveDetector = serde_json::from_value(serde_json::Value::Object(fields))
            .map_err(|error| ProfileRefusal {
                code: medulla_query_types::sensitive_detect::DETECTOR_RULE_MISMATCH,
                detail: format!("{node_id}: {error}"),
            })?;
        found.push(detector);
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(found)
}

impl SensitiveScan {
    /// Load the detectors of `tenant` and validate every one and the limits before
    /// the first scan: a refusal names the detector and the job writes no claim.
    pub(super) fn load(
        core: &medulla_graph_core::graph::GraphCore,
        tenant: &str,
        now_ms: i64,
    ) -> Result<Self, ProfileRefusal> {
        let mut detectors = builtin_detectors();
        detectors.extend(tenant_detectors(core, tenant)?);
        let limits = knob_limits();
        limits.validate().map_err(refusal)?;
        for detector in &detectors {
            detector
                .validate()
                .map_err(|invalid| refusal(ScanError::Detector(invalid)))?;
        }
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
        .map_err(refusal)
    }
}

/// The catalog column node a profile is about.
fn column_asset(profile: &ColumnProfile) -> String {
    medulla_catalog_types::lineage::column_node_id(&format!(
        "{}.{}",
        profile.dataset, profile.column
    ))
}

/// One proposed claim per passing detection, as graph methods for the job batch: the claim
/// node and its `claimOf` edge to the catalog column node. A column that already holds a
/// claim for the same markings keeps it; a decided claim is never reset.
pub(super) fn propose_claims(
    core: &medulla_graph_core::graph::GraphCore,
    target: &ProfileTarget,
    scan: &SensitiveScan,
    detections: &[Detection],
) -> Result<Vec<Method>, String> {
    let mut methods = Vec::new();
    for detection in passing(detections, &scan.detectors) {
        let Some(detector) = scan
            .detectors
            .iter()
            .find(|detector| detector.id == detection.detector)
        else {
            continue;
        };
        let category = Marking::new(&detector.category)
            .map_err(|error| format!("detector {}: {error}", detector.id))?;
        let claim = ClassificationClaim {
            asset: detection.asset.clone(),
            categories: vec![category],
            provenance: ClaimProvenance {
                kind: ClaimKind::Detector,
                detector_id: Some(detection.detector.clone()),
                sample_size: Some(detection.sample_size),
                hit_ratio: Some(detection.ratio),
                scanned_at_ms: Some(u64::try_from(detection.scanned_at_ms).unwrap_or(0)),
            },
            state: ClaimState::Proposed,
            evidence: vec![format!("detector:{}", detection.detector)],
        };
        claim.validate()?;
        let claim_id = claim.claim_id();
        if core.get_node_properties(&claim_id).is_some() {
            continue;
        }
        let record = ClaimRecord {
            claim_id: claim_id.clone(),
            claim,
            before_markings: Vec::new(),
            after_markings: Vec::new(),
            promoted_by: None,
        };
        let mut properties = record.node_properties(&target.tenant)?;
        properties.insert("_owner".to_string(), target.agent.clone().into());
        methods.push(node_method(claim_id.clone(), &serde_json::Value::Object(properties))?);
        methods.push(edge_method(
            claim_id,
            detection.asset.clone(),
            &serde_json::json!({
                "relationship": CLAIM_OF_RELATIONSHIP,
                "tenant_id": target.tenant,
                "_owner": target.agent,
            }),
        )?);
    }
    Ok(methods)
}
