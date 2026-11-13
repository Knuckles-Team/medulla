//! Capability designation: the model types (MEDULLA-DECISION-ENGINE-R172.1).
//!
//! `Decide` with `QuestionKind::CapabilityDesignation` names the agents, tools
//! or skills best able to do a job: it filters records by required capability,
//! tenant and policy tags, scores the rest by cosine similarity to a query
//! vector, learned reward and an ontology prior, and explains every choice with
//! a [`CapabilityEligibility`]. Scores and weights are Q32 integers on the wire
//! (`value / 2^32`). Only the query vector is `f32`, little-endian, base64.
//! Nothing here reads a clock or a store.

use std::collections::BTreeSet;

use base64::Engine as _;
use medulla_types::contract::{OpaqueId, TenantId};
use serde::{Deserialize, Serialize};

/// An ontology IRI of a capability.
pub type CapabilityIri = String;
/// An ontology IRI of a class.
pub type ClassIri = String;

/// The neutral reward, 0.5 on Q32, of an entity with no outcomes yet.
pub const NEUTRAL_REWARD_Q32: i64 = 1 << 31;
/// One on Q32.
pub const Q32_ONE: i64 = 1 << 32;
/// Most designations one question may ask for.
pub const MAX_DESIGNATIONS: u8 = 32;
/// Most capabilities, policy tags or alternatives one record may carry.
pub const MAX_RECORD_ITEMS: usize = 64;

/// What kind of entity a record describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum EntityKind {
    Agent,
    Tool,
    Skill,
}

/// One designatable entity. A record with no tenant is a commons record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CapabilityRecord {
    #[serde(default)]
    pub tenant_id: Option<TenantId>,
    pub entity_id: OpaqueId,
    pub entity_kind: EntityKind,
    pub capabilities: BTreeSet<CapabilityIri>,
    #[serde(default)]
    pub ontology_type: Option<ClassIri>,
    #[serde(default)]
    pub policy_tags: BTreeSet<String>,
    #[serde(default)]
    pub swappable_with: BTreeSet<OpaqueId>,
    pub embedding_version: String,
    pub dim: u16,
    pub record_revision: u64,
}

impl CapabilityRecord {
    /// Bounded sets, a non-empty embedding version and a non-zero dimension.
    pub fn validate(&self) -> Result<(), String> {
        if self.dim == 0 || self.embedding_version.trim().is_empty() {
            return Err("a record names its embedding version and a dim above zero".into());
        }
        if self.capabilities.len() > MAX_RECORD_ITEMS
            || self.policy_tags.len() > MAX_RECORD_ITEMS
            || self.swappable_with.len() > MAX_RECORD_ITEMS
        {
            return Err(format!(
                "a record set holds at most {MAX_RECORD_ITEMS} items"
            ));
        }
        Ok(())
    }
}

/// The learned reward of one entity for one tenant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CapabilityReward {
    pub tenant_id: TenantId,
    pub entity_id: OpaqueId,
    /// Reward on Q32 in `[0, 1]`; [`NEUTRAL_REWARD_Q32`] before any outcome.
    pub reward_q32: i64,
    pub outcome_count: u64,
    pub generation: u32,
    pub updated_revision: u64,
}

impl CapabilityReward {
    /// The reward of an entity with no outcomes: neutral.
    pub fn neutral(tenant_id: TenantId, entity_id: OpaqueId) -> Self {
        Self {
            tenant_id,
            entity_id,
            reward_q32: NEUTRAL_REWARD_Q32,
            outcome_count: 0,
            generation: 0,
            updated_revision: 0,
        }
    }

    /// The reward lies in `[0, 1]`.
    pub fn validate(&self) -> Result<(), String> {
        (0..=Q32_ONE)
            .contains(&self.reward_q32)
            .then_some(())
            .ok_or_else(|| "reward_q32 lies in [0, 2^32]".to_string())
    }
}

/// A query vector: `dim` little-endian `f32` values, base64 encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QueryVector {
    pub embedding_version: String,
    pub dim: u16,
    pub f32le_b64: String,
}

impl QueryVector {
    /// Encode `values` as a vector of `values.len()` dimensions.
    pub fn from_f32(embedding_version: &str, values: &[f32]) -> Result<Self, String> {
        let dim = u16::try_from(values.len()).map_err(|_| "dim exceeds 65535".to_string())?;
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let vector = Self {
            embedding_version: embedding_version.to_string(),
            dim,
            f32le_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
        };
        vector.validate()?;
        Ok(vector)
    }

    /// The decoded values. Refuses a zero dim, a version-less vector, bad base64
    /// and a length that is not `4 * dim`.
    pub fn decode(&self) -> Result<Vec<f32>, String> {
        if self.dim == 0 || self.embedding_version.trim().is_empty() {
            return Err("a query vector names its embedding version and a dim above zero".into());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.f32le_b64)
            .map_err(|error| format!("f32le_b64 is not base64: {error}"))?;
        if bytes.len() != 4 * usize::from(self.dim) {
            return Err(format!(
                "f32le_b64 holds {} bytes, expected 4 * dim = {}",
                bytes.len(),
                4 * usize::from(self.dim)
            ));
        }
        Ok(bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect())
    }

    /// The vector decodes.
    pub fn validate(&self) -> Result<(), String> {
        self.decode().map(|_| ())
    }
}

/// How the three scores combine, each on Q32 and not negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DesignationWeights {
    pub cosine_q32: i64,
    pub reward_q32: i64,
    pub prior_q32: i64,
}

impl DesignationWeights {
    /// No weight is negative and at least one is positive.
    pub fn validate(&self) -> Result<(), String> {
        let all = [self.cosine_q32, self.reward_q32, self.prior_q32];
        if all.iter().any(|w| *w < 0) || all.iter().all(|w| *w == 0) {
            return Err("weights are not negative and at least one is positive".into());
        }
        Ok(())
    }
}

/// One designation question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CapabilityDesignationQuestion {
    pub tenant_id: TenantId,
    pub query_vector: QueryVector,
    #[serde(default)]
    pub required_capabilities: BTreeSet<CapabilityIri>,
    #[serde(default)]
    pub required_policy_tags: BTreeSet<String>,
    /// How many designations to return, 1 to [`MAX_DESIGNATIONS`].
    pub k: u8,
    pub weights: DesignationWeights,
    /// The class a designated entity is expected to be of, if any.
    #[serde(default)]
    pub ontology_prior: Option<ClassIri>,
}

impl CapabilityDesignationQuestion {
    /// `k` in range, the vector decodes and the weights hold.
    pub fn validate(&self) -> Result<(), String> {
        if self.k == 0 || self.k > MAX_DESIGNATIONS {
            return Err(format!("k lies in [1, {MAX_DESIGNATIONS}]"));
        }
        self.query_vector.validate()?;
        self.weights.validate()
    }
}

/// Whether a record's tenant fits the asking tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum TenantMatch {
    /// The record belongs to the asking tenant.
    Match,
    /// The record belongs to another tenant.
    Mismatch,
    /// The record has no tenant: a commons record.
    Commons,
}

/// Why one record was or was not eligible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CapabilityEligibility {
    pub capabilities: BTreeSet<CapabilityIri>,
    pub required_caps: BTreeSet<CapabilityIri>,
    pub missing_caps: BTreeSet<CapabilityIri>,
    pub capabilities_matched: bool,
    #[serde(default)]
    pub tenant: Option<TenantId>,
    pub required_tenant: TenantId,
    pub tenant_match: TenantMatch,
    pub policy_tags: BTreeSet<String>,
    pub required_policy_tags: BTreeSet<String>,
    pub missing_policy_tags: BTreeSet<String>,
    pub policy_matched: bool,
    /// The reward on Q32 the record was scored with.
    pub reward: i64,
    #[serde(default)]
    pub ontology_type: Option<ClassIri>,
    pub eligible: bool,
    /// Class paths from the record's type up to the prior, when it subsumes.
    #[serde(default)]
    pub subsumption_paths: Vec<Vec<ClassIri>>,
}

/// One designated entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DesignatedCapability {
    pub entity_id: OpaqueId,
    pub score_q32: i64,
    pub cosine_q32: i64,
    pub reward_q32: i64,
    pub prior_q32: i64,
    #[serde(default)]
    pub ontology_type: Option<ClassIri>,
    pub capabilities: BTreeSet<CapabilityIri>,
    /// Entities the record says may stand in for it.
    #[serde(default)]
    pub alternatives: BTreeSet<OpaqueId>,
    pub eligibility: CapabilityEligibility,
}

/// The answer of one designation question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CapabilityDesignation {
    pub designations: Vec<DesignatedCapability>,
    /// Records considered before the capability filter.
    pub candidate_pool_size: u64,
    /// Records left after the capability filter.
    pub capability_filtered: u64,
    pub index_epoch: u64,
    pub embedding_version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set<T: Ord + From<&'static str>>(items: &[&'static str]) -> BTreeSet<T> {
        items.iter().map(|i| T::from(*i)).collect()
    }

    fn tenant(id: &str) -> TenantId {
        TenantId::new(id).unwrap()
    }

    fn eligibility() -> CapabilityEligibility {
        CapabilityEligibility {
            capabilities: set(&["cap:summarize"]),
            required_caps: set(&["cap:summarize", "cap:translate"]),
            missing_caps: set(&["cap:translate"]),
            capabilities_matched: false,
            tenant: None,
            required_tenant: tenant("tenant-a"),
            tenant_match: TenantMatch::Commons,
            policy_tags: set(&["pii-safe"]),
            required_policy_tags: set(&["pii-safe"]),
            missing_policy_tags: BTreeSet::new(),
            policy_matched: true,
            reward: NEUTRAL_REWARD_Q32,
            ontology_type: Some("https://knuckles-team.github.io/ontology/core#Skill".into()),
            eligible: false,
            subsumption_paths: vec![vec!["a".into(), "b".into()]],
        }
    }

    macro_rules! round_trip {
        ($value:expr, $ty:ty) => {{
            let json = serde_json::to_value(&$value).unwrap();
            assert_eq!(serde_json::from_value::<$ty>(json).unwrap(), $value);
            let packed = rmp_serde::to_vec_named(&$value).unwrap();
            assert_eq!(rmp_serde::from_slice::<$ty>(&packed).unwrap(), $value);
        }};
    }

    // spec: MEDULLA-DECISION-ENGINE-R172.1
    #[test]
    fn designation_types_round_trip() {
        let record = CapabilityRecord {
            tenant_id: Some(tenant("tenant-a")),
            entity_id: OpaqueId::new("agent-1").unwrap(),
            entity_kind: EntityKind::Agent,
            capabilities: set(&["cap:summarize"]),
            ontology_type: Some("https://knuckles-team.github.io/ontology/core#Agent".into()),
            policy_tags: set(&["pii-safe"]),
            swappable_with: [OpaqueId::new("agent-2").unwrap()].into_iter().collect(),
            embedding_version: "emb-v1".into(),
            dim: 3,
            record_revision: 9,
        };
        record.validate().unwrap();
        let reward = CapabilityReward::neutral(tenant("tenant-a"), record.entity_id.clone());
        assert_eq!(reward.reward_q32, NEUTRAL_REWARD_Q32);
        reward.validate().unwrap();
        let vector = QueryVector::from_f32("emb-v1", &[0.5, -1.0, 2.25]).unwrap();
        assert_eq!(vector.decode().unwrap(), vec![0.5, -1.0, 2.25]);
        let question = CapabilityDesignationQuestion {
            tenant_id: tenant("tenant-a"),
            query_vector: vector.clone(),
            required_capabilities: set(&["cap:summarize"]),
            required_policy_tags: set(&["pii-safe"]),
            k: 3,
            weights: DesignationWeights {
                cosine_q32: Q32_ONE / 2,
                reward_q32: Q32_ONE / 4,
                prior_q32: Q32_ONE / 4,
            },
            ontology_prior: Some("https://knuckles-team.github.io/ontology/core#Agent".into()),
        };
        question.validate().unwrap();
        let designation = CapabilityDesignation {
            designations: vec![DesignatedCapability {
                entity_id: record.entity_id.clone(),
                score_q32: 1,
                cosine_q32: 2,
                reward_q32: 3,
                prior_q32: 4,
                ontology_type: record.ontology_type.clone(),
                capabilities: record.capabilities.clone(),
                alternatives: record.swappable_with.clone(),
                eligibility: eligibility(),
            }],
            candidate_pool_size: 10,
            capability_filtered: 4,
            index_epoch: 7,
            embedding_version: "emb-v1".into(),
        };
        round_trip!(record, CapabilityRecord);
        round_trip!(reward, CapabilityReward);
        round_trip!(vector, QueryVector);
        round_trip!(question, CapabilityDesignationQuestion);
        round_trip!(eligibility(), CapabilityEligibility);
        round_trip!(designation, CapabilityDesignation);
        assert_eq!(
            serde_json::to_string(
                &crate::decision::statistical::QuestionKind::CapabilityDesignation
            )
            .unwrap(),
            "\"capability_designation\""
        );

        // An unknown entity kind is refused.
        let mut wire = serde_json::to_value(&record).unwrap();
        wire["entity_kind"] = serde_json::json!("robot");
        assert!(serde_json::from_value::<CapabilityRecord>(wire).is_err());
        // A dim of zero is refused, by the record and by the vector.
        let mut zero = record.clone();
        zero.dim = 0;
        assert!(zero.validate().is_err());
        let mut zero_vector = vector.clone();
        zero_vector.dim = 0;
        assert!(zero_vector.decode().is_err());
        // A base64 vector whose length is not 4 * dim is refused.
        let mut short = vector.clone();
        short.dim = 4;
        assert!(short
            .decode()
            .unwrap_err()
            .contains("expected 4 * dim = 16"));
        let mut bad = vector;
        bad.f32le_b64 = "!!!".into();
        assert!(bad.decode().is_err());
        // k and weights are bounded.
        let mut zero_k = question.clone();
        zero_k.k = 0;
        assert!(zero_k.validate().is_err());
        let mut no_weight = question;
        no_weight.weights = DesignationWeights {
            cosine_q32: 0,
            reward_q32: 0,
            prior_q32: 0,
        };
        assert!(no_weight.validate().is_err());
        // Unknown fields are refused.
        let mut wire = serde_json::to_value(designation).unwrap();
        wire["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<CapabilityDesignation>(wire).is_err());
    }
}
