//! The classification claim model (MEDULLA-TYPED-PACKS-R652.1).
//!
//! A claim says an asset carries some markings, and who or what said so. It
//! reuses [`Marking`] from the tenancy model; there is no second marking type.

use serde::{Deserialize, Serialize};

use crate::tenancy::Marking;

/// MEDULLA-FEDERATED-QUERY-R706.4: the label of a tenant's registered marking node.
pub const MARKING_LABEL: &str = "Marking";
/// The node-id prefix of a registered marking: the node `marking:<tag>` registers `<tag>`.
pub const MARKING_NODE_PREFIX: &str = "marking:";

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

/// Label of a stored claim node.
pub const CLASSIFICATION_CLAIM_LABEL: &str = "ClassificationClaim";
/// Relationship of the edge from a claim node to the asset it is about.
pub const CLAIM_OF_RELATIONSHIP: &str = "claimOf";

impl ClaimState {
    /// The wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Promoted => "promoted",
            Self::Rejected => "rejected",
        }
    }
}

impl ClassificationClaim {
    /// The stored node id of this claim: one node per asset and category set.
    pub fn claim_id(&self) -> String {
        claim_id(&self.asset, &self.categories)
    }
}

/// The node id of the claim over `asset` for `categories` (order-insensitive).
pub fn claim_id(asset: &str, categories: &[Marking]) -> String {
    let mut names: Vec<&str> = categories.iter().map(Marking::as_str).collect();
    names.sort_unstable();
    names.dedup();
    format!("classification_claim:{asset}:{}", names.join("+"))
}

/// A claim as stored and answered (MEDULLA-TYPED-PACKS-R652.5): the claim, the markings the
/// asset held before and after, and who promoted it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClaimRecord {
    /// The claim node id.
    pub claim_id: String,
    /// The claim itself.
    pub claim: ClassificationClaim,
    /// Markings on the asset before the last transition.
    #[serde(default)]
    pub before_markings: Vec<Marking>,
    /// Markings on the asset after the last transition.
    #[serde(default)]
    pub after_markings: Vec<Marking>,
    /// The principal that promoted the claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_by: Option<String>,
}

impl ClaimRecord {
    /// The node properties of this record: flat query fields (state, asset, categories,
    /// evidence, marking sets, promoter) plus the whole record as JSON under `record`, which
    /// [`Self::from_node_properties`] reads back.
    pub fn node_properties(
        &self,
        tenant: &str,
    ) -> Result<serde_json::Map<String, serde_json::Value>, String> {
        use serde_json::{json, Value};
        let body = serde_json::to_string(self)
            .map_err(|error| format!("INVALID_ARGUMENT: claim encoding failed: {error}"))?;
        let names = |markings: &[Marking]| -> Vec<String> {
            markings.iter().map(|m| m.as_str().to_string()).collect()
        };
        let claim = &self.claim;
        let mut properties = serde_json::Map::new();
        let mut put = |key: &str, value: Value| {
            properties.insert(key.to_string(), value);
        };
        put("type", CLASSIFICATION_CLAIM_LABEL.into());
        put("claim_id", self.claim_id.clone().into());
        put("asset", claim.asset.clone().into());
        put("state", claim.state.as_str().into());
        put("categories", json!(names(&claim.categories)));
        put("evidence", json!(claim.evidence));
        put("before_markings", json!(names(&self.before_markings)));
        put("after_markings", json!(names(&self.after_markings)));
        put(
            "promoted_by",
            self.promoted_by.clone().map_or(Value::Null, Value::String),
        );
        put(
            "provenance_kind",
            serde_json::to_value(claim.provenance.kind).unwrap_or_default(),
        );
        put("record", body.into());
        put("tenant_id", tenant.into());
        Ok(properties)
    }

    /// The record stored in a claim node's properties, when there is one.
    pub fn from_node_properties(
        properties: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Option<Self>, String> {
        let Some(body) = properties.get("record").and_then(serde_json::Value::as_str) else {
            return Ok(None);
        };
        serde_json::from_str(body)
            .map(Some)
            .map_err(|error| format!("INVALID_ARGUMENT: stored claim is malformed: {error}"))
    }
}

/// The answer of one served claim op: the record, and the downstream claims a promotion
/// proposed along lineage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClassificationClaimResult {
    /// The claim the op acted on.
    pub record: ClaimRecord,
    /// Claims proposed downstream by propagation; empty for `Propose` and `Reject`.
    #[serde(default)]
    pub propagated: Vec<ClaimRecord>,
}

/// The served operations of `Method::ClassificationClaim`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ClassificationClaimOp {
    /// Store a new claim in state `proposed`.
    Propose {
        /// The claim; its state must be `proposed`.
        claim: ClassificationClaim,
    },
    /// Move a proposed claim to `promoted`, stamp its markings on the asset and propose them
    /// downstream. Needs the steward or owner role and evidence.
    Promote {
        /// The claim node id.
        claim_id: String,
        /// Evidence added by the promoter.
        #[serde(default)]
        evidence: Vec<String>,
    },
    /// Move a proposed claim to `rejected`.
    Reject {
        /// The claim node id.
        claim_id: String,
    },
}
