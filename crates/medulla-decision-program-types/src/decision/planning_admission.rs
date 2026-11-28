//! Planning admission model (MEDULLA-DECISION-ENGINE-R171.1).
//!
//! A plan names features, the container each lives in and the attach point in
//! code. Admission accepts only features the architecture view already wires;
//! a difference names what is missing. This request is the `PlanAdmission` of
//! MEDULLA-CQ-ORCH-R010.7.

use serde::{Deserialize, Serialize};

use medulla_types::contract::{BoundedVec, Digest256};
use medulla_types::refusal_code::RefusalCode;

/// The request carries no gap digests or no architecture view digest.
pub const ADMISSION_NO_COMPARISON: RefusalCode = RefusalCode::new("ADMISSION_NO_COMPARISON");
/// The request's view digest differs from the digest of the view derived at admission.
pub const ADMISSION_STALE_VIEW: RefusalCode = RefusalCode::new("ADMISSION_STALE_VIEW");
/// A feature has no attach point, or its attach point is not a support path of its container.
pub const ADMISSION_FEATURE_UNWIRED: RefusalCode = RefusalCode::new("ADMISSION_FEATURE_UNWIRED");
/// A gap digest of the request appears in no feature.
pub const ADMISSION_GAP_UNCOVERED: RefusalCode = RefusalCode::new("ADMISSION_GAP_UNCOVERED");

/// The `planning_admission` family refusal table (MEDULLA-DECISION-ENGINE-R171.4),
/// collected by `codes_are_unique_and_stable`.
pub const REFUSALS: &[RefusalCode] = &[
    ADMISSION_NO_COMPARISON,
    ADMISSION_STALE_VIEW,
    ADMISSION_FEATURE_UNWIRED,
    ADMISSION_GAP_UNCOVERED,
];

/// Most gap digests and most features one request carries.
pub const MAX_PLANNING_ITEMS: usize = 64;

/// Where a feature attaches in code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AttachPoint {
    pub file: String,
    pub symbol: String,
    pub operation: String,
}

/// One planned feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PlanFeature {
    pub feature_id: String,
    pub description_digest: Digest256,
    pub container: String,
    pub attach_point: AttachPoint,
    pub gap_digests: BoundedVec<Digest256, MAX_PLANNING_ITEMS>,
}

/// The plan under admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PlanningAdmissionRequest {
    pub repository_id: String,
    pub repository_revision: String,
    pub architecture_view_digest: Digest256,
    pub gap_proposal_digests: BoundedVec<Digest256, MAX_PLANNING_ITEMS>,
    pub features: BoundedVec<PlanFeature, MAX_PLANNING_ITEMS>,
}

/// What the design lacks for a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum MissingKind {
    AttachPoint,
    Edge,
}

/// One feature the design does not wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DesignDifference {
    pub feature_id: String,
    pub container: String,
    pub missing: MissingKind,
}

/// What an admitted plan seals as its answer (MEDULLA-DECISION-ENGINE-R171.3):
/// the repository and revision read, the digest of the view compared against
/// and the design differences, which are empty exactly when the plan is
/// admitted. A plan with differences is held and carries no answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PlanningAdmissionAnswer {
    pub repository_id: String,
    pub repository_revision: String,
    pub architecture_view_digest: Digest256,
    pub differences: BoundedVec<DesignDifference, MAX_PLANNING_ITEMS>,
}

impl PlanningAdmissionAnswer {
    /// The answer of an admitted request: its view digest and no differences.
    pub fn admitted(request: &PlanningAdmissionRequest) -> Self {
        Self {
            repository_id: request.repository_id.clone(),
            repository_revision: request.repository_revision.clone(),
            architecture_view_digest: request.architecture_view_digest,
            differences: BoundedVec::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feature(id: &str) -> PlanFeature {
        PlanFeature {
            feature_id: id.into(),
            description_digest: Digest256::sha256(id.as_bytes()),
            container: "crate:a".into(),
            attach_point: AttachPoint {
                file: "src/lib.rs".into(),
                symbol: "run".into(),
                operation: "call".into(),
            },
            gap_digests: BoundedVec::new(vec![Digest256::sha256(b"gap")]).unwrap(),
        }
    }

    fn request(features: usize) -> Result<PlanningAdmissionRequest, String> {
        Ok(PlanningAdmissionRequest {
            repository_id: "repo:1".into(),
            repository_revision: "abc123".into(),
            architecture_view_digest: Digest256::sha256(b"view"),
            gap_proposal_digests: BoundedVec::new(vec![Digest256::sha256(b"gap")])?,
            features: BoundedVec::new((0..features).map(|i| feature(&format!("f{i}"))).collect())?,
        })
    }

    // spec: MEDULLA-DECISION-ENGINE-R171.1
    #[test]
    fn admission_request_round_trips() {
        let value = request(2).unwrap();
        let text = serde_json::to_string(&value).unwrap();
        assert_eq!(
            serde_json::from_str::<PlanningAdmissionRequest>(&text).unwrap(),
            value
        );
        assert!(request(64).is_ok());
        assert!(request(65).is_err(), "65 features are refused");
        let too_many = serde_json::json!({
            "repository_id": "r", "repository_revision": "v",
            "architecture_view_digest": Digest256::sha256(b"v").to_hex(),
            "gap_proposal_digests": vec![Digest256::sha256(b"g").to_hex(); 65],
            "features": [],
        });
        assert!(serde_json::from_value::<PlanningAdmissionRequest>(too_many).is_err());
        for missing in [MissingKind::AttachPoint, MissingKind::Edge] {
            let difference = DesignDifference {
                feature_id: "f0".into(),
                container: "crate:a".into(),
                missing,
            };
            let text = serde_json::to_string(&difference).unwrap();
            assert_eq!(
                serde_json::from_str::<DesignDifference>(&text).unwrap(),
                difference
            );
        }
    }
}
