//! The one detector loader and claim builder of the sensitive scan
//! (MEDULLA-TYPED-PACKS-R803.3, R803.7).
//!
//! The `DatasetProfile` job (columns) and `EnrichDocument` (chunks) load the same detector set
//! (the built-ins plus the tenant's `SensitiveDetector` nodes), validate it before the first
//! scan and write the same `proposed` `ClassificationClaim` nodes. A claim carries the detector
//! id, the sample size, the hit ratio and the scan time and never a matched value.

use medulla_core::sensitive_scan::{passing, ScanError, ScanLimits};
use medulla_identity_types::classification::{
    ClaimKind, ClaimProvenance, ClaimRecord, ClaimState, ClassificationClaim, CLAIM_OF_RELATIONSHIP,
};
use medulla_identity_types::tenancy::Marking;
use medulla_query_types::sensitive_detect::{builtin_detectors, Detection, SensitiveDetector};

use super::data_plane_write::{edge_method, node_method};
use super::prelude_server::*;

/// The node type of a tenant-defined detector.
pub const SENSITIVE_DETECTOR_LABEL: &str = "SensitiveDetector";

/// A refused detector set or scan limit: the stable code and the detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensitiveRefusal {
    pub code: &'static str,
    pub detail: String,
}

impl SensitiveRefusal {
    pub fn of(error: ScanError) -> Self {
        Self {
            code: error.code(),
            detail: error.to_string(),
        }
    }
}

/// The scan limits, read from the knob registry.
pub fn knob_limits() -> ScanLimits {
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
) -> Result<Vec<SensitiveDetector>, SensitiveRefusal> {
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
            .map_err(|error| SensitiveRefusal {
                code: medulla_query_types::sensitive_detect::DETECTOR_RULE_MISMATCH,
                detail: format!("{node_id}: {error}"),
            })?;
        found.push(detector);
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(found)
}

/// The detector set of `tenant` (the built-ins plus its own nodes) and the knob limits, every
/// detector and the limits validated before the first scan: a refusal names the detector and
/// nothing is scanned or written.
pub fn load_detectors(
    core: &medulla_graph_core::graph::GraphCore,
    tenant: &str,
) -> Result<(Vec<SensitiveDetector>, ScanLimits), SensitiveRefusal> {
    let mut detectors = builtin_detectors();
    detectors.extend(tenant_detectors(core, tenant)?);
    let limits = knob_limits();
    limits.validate().map_err(SensitiveRefusal::of)?;
    for detector in &detectors {
        detector
            .validate()
            .map_err(|invalid| SensitiveRefusal::of(ScanError::Detector(invalid)))?;
    }
    Ok((detectors, limits))
}

/// One proposed claim per passing detection, as graph methods: the claim node and its
/// `claimOf` edge to the asset node. An asset that already holds a claim for the same markings
/// keeps it; a decided claim is never reset.
pub fn claim_methods(
    core: &medulla_graph_core::graph::GraphCore,
    tenant: &str,
    agent: &str,
    detectors: &[SensitiveDetector],
    detections: &[Detection],
) -> Result<Vec<Method>, String> {
    let mut methods = Vec::new();
    for detection in passing(detections, detectors) {
        let Some(detector) = detectors
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
        let mut properties = record.node_properties(tenant)?;
        properties.insert("_owner".to_string(), agent.to_string().into());
        methods.push(node_method(
            claim_id.clone(),
            &serde_json::Value::Object(properties),
        )?);
        methods.push(edge_method(
            claim_id,
            detection.asset.clone(),
            &serde_json::json!({
                "relationship": CLAIM_OF_RELATIONSHIP,
                "tenant_id": tenant,
                "_owner": agent,
            }),
        )?);
    }
    Ok(methods)
}
