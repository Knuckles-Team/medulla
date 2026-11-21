//! The classification claim model (MEDULLA-TYPED-PACKS-R652.1).
//!
//! A claim says an asset carries some markings, and who or what said so. It
//! reuses [`Marking`] from the tenancy model; there is no second marking type.

use serde::{Deserialize, Serialize};

use crate::tenancy::Marking;

/// Why a claim exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ClaimKind {
    /// A sensitive-data detector scanned the asset.
    Detector,
    /// A promoted claim upstream propagated to this asset.
    Propagated,
    /// A person made the claim.
    Manual,
}

/// The lifecycle state of one claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ClaimState {
    /// Recorded, not yet in force.
    Proposed,
    /// In force: the asset carries the markings.
    Promoted,
    /// Declined.
    Rejected,
}

/// Where a claim came from. The detector fields are set only for
/// [`ClaimKind::Detector`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClaimProvenance {
    /// How the claim arose.
    pub kind: ClaimKind,
    /// The detector that scanned the asset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detector_id: Option<String>,
    /// How many values the detector scanned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_size: Option<u64>,
    /// The share of scanned values that hit, in `[0, 1]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_ratio: Option<f64>,
    /// When the scan ran, in milliseconds since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanned_at_ms: Option<u64>,
}

/// One classification claim on one asset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClassificationClaim {
    /// The classified asset (a catalog node id).
    pub asset: String,
    /// The markings the claim asserts; never empty.
    pub categories: Vec<Marking>,
    /// Where the claim came from.
    pub provenance: ClaimProvenance,
    /// The claim's lifecycle state.
    pub state: ClaimState,
    /// References to the evidence behind the claim.
    #[serde(default)]
    pub evidence: Vec<String>,
}

impl ClassificationClaim {
    /// Refuse a claim with no asset, no categories, detector fields on a claim
    /// that did not come from a detector, or detector fields missing or out of
    /// range on one that did.
    pub fn validate(&self) -> Result<(), String> {
        if self.asset.trim().is_empty() {
            return Err("INVALID_ARGUMENT: a classification claim names an asset".to_string());
        }
        if self.categories.is_empty() {
            return Err(
                "INVALID_ARGUMENT: a classification claim carries at least one category"
                    .to_string(),
            );
        }
        let provenance = &self.provenance;
        let has_detector_fields = provenance.detector_id.is_some()
            || provenance.sample_size.is_some()
            || provenance.hit_ratio.is_some()
            || provenance.scanned_at_ms.is_some();
        match provenance.kind {
            ClaimKind::Detector => {
                let (Some(id), Some(_), Some(ratio), Some(_)) = (
                    provenance.detector_id.as_deref(),
                    provenance.sample_size,
                    provenance.hit_ratio,
                    provenance.scanned_at_ms,
                ) else {
                    return Err(
                        "INVALID_ARGUMENT: a detector claim sets every detector field"
                            .to_string(),
                    );
                };
                if id.trim().is_empty() || !(0.0..=1.0).contains(&ratio) {
                    return Err(
                        "INVALID_ARGUMENT: a detector claim names its detector and a ratio in [0, 1]"
                            .to_string(),
                    );
                }
            }
            ClaimKind::Propagated | ClaimKind::Manual if has_detector_fields => {
                return Err(
                    "INVALID_ARGUMENT: detector fields belong to a detector claim only"
                        .to_string(),
                );
            }
            ClaimKind::Propagated | ClaimKind::Manual => {}
        }
        Ok(())
    }
}
