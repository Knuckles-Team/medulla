//! Capability designation: the model types (MEDULLA-DECISION-ENGINE-R172.1).
//!
//! `Decide` with `QuestionKind::CapabilityDesignation` names the agents, tools
//! or skills best able to do a job: it filters records by required capability,
//! tenant and policy tags, scores the rest by cosine similarity to a query
//! vector, learned reward and an ontology prior, and explains every choice with
//! a [`CapabilityEligibility`]. Scores and weights are Q32 integers on the wire
//! (`value / 2^32`). Only the query vector is `f32`, little-endian, base64.
//! Nothing here reads a clock or a store.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use base64::Engine as _;
use medulla_types::contract::{OpaqueId, TenantId};
use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;

/// An ontology IRI of a capability.
pub type CapabilityIri = String;
/// An ontology IRI of a class.
pub type ClassIri = String;

/// The neutral reward, 0.5 on Q32, of an entity with no outcomes yet.
pub const NEUTRAL_REWARD_Q32: i64 = 1 << 31;
/// One on Q32.
pub const Q32_ONE: i64 = 1 << 32;
/// Most designations one question may ask for.
pub const MAX_DESIGNATIONS: u8 = 64;
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

/// The `designation.*` knobs (MEDULLA-DECISION-ENGINE-R172.2 to R172.5). The
/// limits of R172.8 are `Fixed`; every other knob is `Tunable`.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "designation.max_records_per_shard",
        unit: "records",
        default: KnobValue::Integer(1_000_000),
        min: KnobValue::Integer(1000),
        max: KnobValue::Integer(10_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "designation.max_required_capabilities",
        unit: "capabilities",
        default: KnobValue::Integer(32),
        min: KnobValue::Integer(32),
        max: KnobValue::Integer(32),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "designation.max_k",
        unit: "designations",
        default: KnobValue::Integer(64),
        min: KnobValue::Integer(64),
        max: KnobValue::Integer(64),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "designation.flat_threshold",
        unit: "records",
        default: KnobValue::Integer(4096),
        min: KnobValue::Integer(256),
        max: KnobValue::Integer(65_536),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "designation.oversample_factor",
        unit: "multiple of k",
        default: KnobValue::Integer(4),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(16),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "designation.tombstone_compact_ratio",
        unit: "fraction of records",
        default: KnobValue::DecimalMicros(200_000),
        min: KnobValue::DecimalMicros(50_000),
        max: KnobValue::DecimalMicros(500_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "designation.reward_weight",
        unit: "weight",
        default: KnobValue::DecimalMicros(150_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "designation.prior_weight",
        unit: "weight",
        default: KnobValue::DecimalMicros(150_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "designation.reward_alpha",
        unit: "weight",
        default: KnobValue::DecimalMicros(300_000),
        min: KnobValue::DecimalMicros(10_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "designation.reembed_reset_distance",
        unit: "cosine distance",
        default: KnobValue::DecimalMicros(250_000),
        min: KnobValue::DecimalMicros(50_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "designation.reward_decay",
        unit: "factor",
        default: KnobValue::DecimalMicros(990_000),
        min: KnobValue::DecimalMicros(500_000),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
];

/// A decimal-micros knob value on Q32.
fn micros_to_q32(micros: i64) -> i64 {
    ((i128::from(micros.max(0)) << 32) / 1_000_000) as i64
}

/// The resolved `designation.*` knobs, read through [`DecisionPolicy::knob`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesignationKnobs {
    pub max_records_per_shard: usize,
    pub max_required_capabilities: usize,
    pub max_k: usize,
    pub flat_threshold: usize,
    pub oversample_factor: usize,
    /// Tombstones above this fraction of records trigger compaction, on Q32.
    pub tombstone_compact_ratio_q32: i64,
    /// Ceiling of the reward term weight a question may ask for, on Q32.
    pub reward_weight_q32: i64,
    /// Ceiling of the prior term weight a question may ask for, on Q32.
    pub prior_weight_q32: i64,
    pub reward_alpha_q32: i64,
    pub reembed_reset_distance_q32: i64,
    pub reward_decay_q32: i64,
}

impl DesignationKnobs {
    /// The knobs a policy pins, with the spec default for the rest.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let int = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::Integer(v) => v.max(0) as usize,
            _ => 0,
        };
        let q32 = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::DecimalMicros(v) => micros_to_q32(v),
            _ => 0,
        };
        Self {
            max_records_per_shard: int(0),
            max_required_capabilities: int(1),
            max_k: int(2),
            flat_threshold: int(3),
            oversample_factor: int(4),
            tombstone_compact_ratio_q32: q32(5),
            reward_weight_q32: q32(6),
            prior_weight_q32: q32(7),
            reward_alpha_q32: q32(8),
            reembed_reset_distance_q32: q32(9),
            reward_decay_q32: q32(10),
        }
    }
}

/// The `rdfs:subClassOf` closure the engine derives at the question's schema
/// digest. The family passes it in; nothing here reads a schema.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaProjection {
    /// The schema digest the closure was derived at.
    pub schema_digest: String,
    parents: BTreeMap<ClassIri, BTreeSet<ClassIri>>,
}

impl SchemaProjection {
    /// A projection from `(subclass, superclass)` edges.
    pub fn new(schema_digest: &str, edges: impl IntoIterator<Item = (ClassIri, ClassIri)>) -> Self {
        let mut parents: BTreeMap<ClassIri, BTreeSet<ClassIri>> = BTreeMap::new();
        for (child, parent) in edges {
            parents.entry(child).or_default().insert(parent);
        }
        Self {
            schema_digest: schema_digest.to_string(),
            parents,
        }
    }

    /// The shortest class path from `from` up to `to`, both included; ties go
    /// to the smallest class IRI. `None` when `to` does not subsume `from`.
    pub fn path(&self, from: &str, to: &str) -> Option<Vec<ClassIri>> {
        if from == to {
            return Some(vec![from.to_string()]);
        }
        let mut previous: BTreeMap<&str, &str> = BTreeMap::new();
        let mut queue: VecDeque<&str> = VecDeque::from([from]);
        let mut seen: BTreeSet<&str> = BTreeSet::from([from]);
        while let Some(class) = queue.pop_front() {
            for parent in self.parents.get(class).into_iter().flatten() {
                if !seen.insert(parent.as_str()) {
                    continue;
                }
                previous.insert(parent.as_str(), class);
                if parent == to {
                    let mut path = vec![to.to_string()];
                    let mut at = to;
                    while let Some(prior) = previous.get(at) {
                        path.push((*prior).to_string());
                        at = prior;
                    }
                    path.reverse();
                    return Some(path);
                }
                queue.push_back(parent.as_str());
            }
        }
        None
    }

    /// `class` and every class below it.
    pub fn subtypes_of(&self, class: &str) -> BTreeSet<ClassIri> {
        let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (child, parents) in &self.parents {
            for parent in parents {
                children.entry(parent.as_str()).or_default().push(child);
            }
        }
        let mut found: BTreeSet<ClassIri> = BTreeSet::from([class.to_string()]);
        let mut queue: VecDeque<&str> = VecDeque::from([class]);
        while let Some(at) = queue.pop_front() {
            for child in children.get(at).into_iter().flatten() {
                if found.insert((*child).to_string()) {
                    queue.push_back(child);
                }
            }
        }
        found
    }
}

/// Which gates a record passes for one question (MEDULLA-DECISION-ENGINE-R172.3).
///
/// A record meets a required capability when it declares it or a subtype of it.
/// A record of another tenant never passes; a commons record is visible to all.
/// Policy tags fail closed. An omitted filter never disqualifies. The reward is
/// neutral here: the ranker overwrites it with the reward it scored with.
pub fn compute_capability_eligibility(
    record: &CapabilityRecord,
    question: &CapabilityDesignationQuestion,
    projection: &SchemaProjection,
) -> CapabilityEligibility {
    let mut missing_caps = BTreeSet::new();
    let mut subsumption_paths = Vec::new();
    for required in &question.required_capabilities {
        if record.capabilities.contains(required) {
            continue;
        }
        match record
            .capabilities
            .iter()
            .find_map(|declared| projection.path(declared, required))
        {
            Some(path) => subsumption_paths.push(path),
            None => {
                missing_caps.insert(required.clone());
            }
        }
    }
    if let (Some(kind), Some(prior)) = (&record.ontology_type, &question.ontology_prior) {
        if let Some(path) = projection.path(kind, prior).filter(|p| p.len() > 1) {
            subsumption_paths.push(path);
        }
    }
    let tenant_match = match &record.tenant_id {
        None => TenantMatch::Commons,
        Some(tenant) if *tenant == question.tenant_id => TenantMatch::Match,
        Some(_) => TenantMatch::Mismatch,
    };
    let missing_policy_tags: BTreeSet<String> = question
        .required_policy_tags
        .difference(&record.policy_tags)
        .cloned()
        .collect();
    let capabilities_matched = missing_caps.is_empty();
    let policy_matched = missing_policy_tags.is_empty();
    CapabilityEligibility {
        capabilities: record.capabilities.clone(),
        required_caps: question.required_capabilities.clone(),
        missing_caps,
        capabilities_matched,
        tenant: record.tenant_id.clone(),
        required_tenant: question.tenant_id.clone(),
        tenant_match,
        policy_tags: record.policy_tags.clone(),
        required_policy_tags: question.required_policy_tags.clone(),
        missing_policy_tags,
        policy_matched,
        reward: NEUTRAL_REWARD_Q32,
        ontology_type: record.ontology_type.clone(),
        eligible: capabilities_matched && policy_matched && tenant_match != TenantMatch::Mismatch,
        subsumption_paths,
    }
}

/// What one run observed of an entity (MEDULLA-DECISION-ENGINE-R172.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ObservedOutcome {
    /// Success is 1, failure is 0.
    Success(bool),
    /// An explicit reward on Q32, clamped to `[0, 1]`.
    Reward(i64),
}

impl ObservedOutcome {
    /// The observed value on Q32 in `[0, 1]`.
    pub fn observed_q32(self) -> i64 {
        match self {
            Self::Success(true) => Q32_ONE,
            Self::Success(false) => 0,
            Self::Reward(q32) => q32.clamp(0, Q32_ONE),
        }
    }
}

/// The reward update parameters, all on Q32 (read from [`DesignationKnobs`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RewardPolicy {
    pub alpha_q32: i64,
    pub reembed_reset_distance_q32: i64,
    pub decay_q32: i64,
}

impl From<&DesignationKnobs> for RewardPolicy {
    fn from(knobs: &DesignationKnobs) -> Self {
        Self {
            alpha_q32: knobs.reward_alpha_q32,
            reembed_reset_distance_q32: knobs.reembed_reset_distance_q32,
            decay_q32: knobs.reward_decay_q32,
        }
    }
}

/// Q32 product of two Q32 values, rounded down.
fn q32_mul(a: i64, b: i64) -> i64 {
    ((i128::from(a) * i128::from(b)) >> 32) as i64
}

/// `new = (1 - alpha) * old + alpha * observed`, in Q32 fixed point so replay
/// is deterministic. The result stays in `[0, 1]`; the count grows by one.
pub fn apply_capability_outcome(
    reward: &CapabilityReward,
    outcome: ObservedOutcome,
    revision: u64,
    policy: &RewardPolicy,
) -> CapabilityReward {
    let alpha = policy.alpha_q32.clamp(0, Q32_ONE);
    let blended =
        q32_mul(Q32_ONE - alpha, reward.reward_q32) + q32_mul(alpha, outcome.observed_q32());
    CapabilityReward {
        reward_q32: blended.clamp(0, Q32_ONE),
        outcome_count: reward.outcome_count.saturating_add(1),
        updated_revision: revision,
        ..reward.clone()
    }
}

/// A writer upserted the record again with a vector at `distance_q32` (cosine
/// distance on Q32) from the stored one. Above the reset distance the reward
/// returns to neutral and the generation increments; the outcome count stays.
pub fn generation_erasure(
    reward: &CapabilityReward,
    distance_q32: i64,
    revision: u64,
    policy: &RewardPolicy,
) -> CapabilityReward {
    if distance_q32 <= policy.reembed_reset_distance_q32 {
        return reward.clone();
    }
    CapabilityReward {
        reward_q32: NEUTRAL_REWARD_Q32,
        generation: reward.generation.saturating_add(1),
        updated_revision: revision,
        ..reward.clone()
    }
}

/// Reset only the named entities. Order of `ids` never matters.
pub fn selective_erase(rewards: &mut [CapabilityReward], ids: &BTreeSet<OpaqueId>, revision: u64) {
    for reward in rewards.iter_mut().filter(|r| ids.contains(&r.entity_id)) {
        reward.reward_q32 = NEUTRAL_REWARD_Q32;
        reward.generation = reward.generation.saturating_add(1);
        reward.updated_revision = revision;
    }
}

/// `0.5 + (r - 0.5) * factor`: drift toward neutral.
pub fn decay_reward(
    reward: &CapabilityReward,
    revision: u64,
    policy: &RewardPolicy,
) -> CapabilityReward {
    let centred = reward.reward_q32 - NEUTRAL_REWARD_Q32;
    CapabilityReward {
        reward_q32: (NEUTRAL_REWARD_Q32 + q32_mul(centred, policy.decay_q32.clamp(0, Q32_ONE)))
            .clamp(0, Q32_ONE),
        updated_revision: revision,
        ..reward.clone()
    }
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

    const CORE: &str = "https://knuckles-team.github.io/ontology/core#";

    fn projection() -> SchemaProjection {
        SchemaProjection::new(
            "digest-1",
            [
                (format!("{CORE}Summarize"), format!("{CORE}TextSkill")),
                (format!("{CORE}TextSkill"), format!("{CORE}Skill")),
                (format!("{CORE}Agent"), format!("{CORE}Actor")),
            ],
        )
    }

    fn record(
        tenant_id: Option<&str>,
        caps: &[&'static str],
        tags: &[&'static str],
    ) -> CapabilityRecord {
        CapabilityRecord {
            tenant_id: tenant_id.map(tenant),
            entity_id: OpaqueId::new("entity-1").unwrap(),
            entity_kind: EntityKind::Skill,
            capabilities: caps.iter().map(|c| (*c).to_string()).collect(),
            ontology_type: None,
            policy_tags: tags.iter().map(|t| (*t).to_string()).collect(),
            swappable_with: BTreeSet::new(),
            embedding_version: "emb-v1".into(),
            dim: 2,
            record_revision: 1,
        }
    }

    fn ask(caps: &[String], tags: &[&'static str]) -> CapabilityDesignationQuestion {
        CapabilityDesignationQuestion {
            tenant_id: tenant("tenant-a"),
            query_vector: QueryVector::from_f32("emb-v1", &[1.0, 0.0]).unwrap(),
            required_capabilities: caps.iter().cloned().collect(),
            required_policy_tags: tags.iter().map(|t| (*t).to_string()).collect(),
            k: 3,
            weights: DesignationWeights {
                cosine_q32: Q32_ONE,
                reward_q32: 0,
                prior_q32: 0,
            },
            ontology_prior: None,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R172.3
    #[test]
    fn eligibility_gates_match_fixtures() {
        let skill = format!("{CORE}Skill");
        let projection = projection();

        // A subtype match: declaring Summarize meets a requirement of Skill, by path.
        let by_subtype = compute_capability_eligibility(
            &record(
                Some("tenant-a"),
                &["https://knuckles-team.github.io/ontology/core#Summarize"],
                &[],
            ),
            &ask(&[skill.clone()], &[]),
            &projection,
        );
        assert!(by_subtype.eligible && by_subtype.capabilities_matched);
        assert_eq!(by_subtype.tenant_match, TenantMatch::Match);
        assert!(by_subtype.missing_caps.is_empty());
        assert_eq!(
            by_subtype.subsumption_paths,
            vec![vec![
                format!("{CORE}Summarize"),
                format!("{CORE}TextSkill"),
                skill.clone()
            ]]
        );

        // An exact declaration needs no path.
        let exact = compute_capability_eligibility(
            &record(Some("tenant-a"), &[CORE_SKILL], &[]),
            &ask(&[skill.clone()], &[]),
            &projection,
        );
        assert!(exact.eligible && exact.subsumption_paths.is_empty());

        // A policy exclusion: the tag is missing, and an untagged record fails closed.
        let tagged = compute_capability_eligibility(
            &record(Some("tenant-a"), &[CORE_SKILL], &["internal"]),
            &ask(&[skill.clone()], &["pii-safe"]),
            &projection,
        );
        assert!(!tagged.eligible && !tagged.policy_matched);
        assert_eq!(tagged.missing_policy_tags, set::<String>(&["pii-safe"]));
        let untagged = compute_capability_eligibility(
            &record(Some("tenant-a"), &[CORE_SKILL], &[]),
            &ask(&[skill.clone()], &["pii-safe"]),
            &projection,
        );
        assert!(!untagged.eligible);
        assert_eq!(untagged.missing_policy_tags, set::<String>(&["pii-safe"]));

        // A commons record is visible to every tenant; a foreign record never is.
        let commons = compute_capability_eligibility(
            &record(None, &[CORE_SKILL], &[]),
            &ask(&[skill.clone()], &[]),
            &projection,
        );
        assert!(commons.eligible);
        assert_eq!(commons.tenant_match, TenantMatch::Commons);
        let foreign = compute_capability_eligibility(
            &record(Some("tenant-b"), &[CORE_SKILL], &[]),
            &ask(&[skill.clone()], &[]),
            &projection,
        );
        assert!(!foreign.eligible);
        assert_eq!(foreign.tenant_match, TenantMatch::Mismatch);
        assert!(foreign.capabilities_matched && foreign.policy_matched);

        // The combined filters: a missing capability and a missing tag together.
        let both = compute_capability_eligibility(
            &record(Some("tenant-a"), &["other"], &[]),
            &ask(&[skill.clone()], &["pii-safe"]),
            &projection,
        );
        assert!(!both.eligible);
        assert_eq!(both.missing_caps, set::<String>(&[CORE_SKILL]));
        assert_eq!(both.missing_policy_tags, set::<String>(&["pii-safe"]));

        // No filter asked for: nothing disqualifies.
        let open =
            compute_capability_eligibility(&record(None, &[], &[]), &ask(&[], &[]), &projection);
        assert!(open.eligible);

        // An ontology prior reports its path.
        let mut typed = record(None, &[], &[]);
        typed.ontology_type = Some(format!("{CORE}Summarize"));
        let mut question = ask(&[], &[]);
        question.ontology_prior = Some(skill.clone());
        let prior = compute_capability_eligibility(&typed, &question, &projection);
        assert_eq!(prior.subsumption_paths.len(), 1);
        assert_eq!(projection.subtypes_of(&skill).len(), 3);
        assert!(projection
            .path(&skill, &format!("{CORE}Summarize"))
            .is_none());
    }

    const CORE_SKILL: &str = "https://knuckles-team.github.io/ontology/core#Skill";

    fn reward_policy() -> RewardPolicy {
        RewardPolicy {
            alpha_q32: micros_to_q32(300_000),
            reembed_reset_distance_q32: micros_to_q32(250_000),
            decay_q32: micros_to_q32(500_000),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R172.5
    #[test]
    fn reward_ema_erasure_and_decay() {
        let policy = reward_policy();
        let neutral = CapabilityReward::neutral(tenant("tenant-a"), OpaqueId::new("e-1").unwrap());

        // A bounded EMA: a success moves 0.5 up by alpha * 0.5, a failure down.
        let up = apply_capability_outcome(&neutral, ObservedOutcome::Success(true), 5, &policy);
        let expected = NEUTRAL_REWARD_Q32 + q32_mul(policy.alpha_q32, Q32_ONE - NEUTRAL_REWARD_Q32);
        assert!(
            (up.reward_q32 - expected).abs() <= 2,
            "{} vs {expected}",
            up.reward_q32
        );
        assert_eq!((up.outcome_count, up.updated_revision), (1, 5));
        let down = apply_capability_outcome(&neutral, ObservedOutcome::Success(false), 6, &policy);
        assert!(down.reward_q32 < NEUTRAL_REWARD_Q32);
        // Repeated success never leaves [0, 1]; an explicit reward is clamped.
        let mut chain = neutral.clone();
        for revision in 0..200 {
            chain = apply_capability_outcome(
                &chain,
                ObservedOutcome::Reward(i64::MAX),
                revision,
                &policy,
            );
            chain.validate().unwrap();
        }
        assert!(chain.reward_q32 > Q32_ONE - (Q32_ONE >> 20));
        let clamped = apply_capability_outcome(&neutral, ObservedOutcome::Reward(-5), 1, &policy);
        assert_eq!(
            clamped.reward_q32,
            q32_mul(Q32_ONE - policy.alpha_q32, NEUTRAL_REWARD_Q32)
        );
        // Replay is deterministic: the same outcomes give the same bits.
        let replay = |_: ()| {
            [true, false, true]
                .into_iter()
                .fold(neutral.clone(), |r, ok| {
                    apply_capability_outcome(&r, ObservedOutcome::Success(ok), 1, &policy)
                })
        };
        assert_eq!(replay(()), replay(()));

        // A material re-embed resets the reward and increments the generation.
        let material = generation_erasure(&up, micros_to_q32(300_000), 7, &policy);
        assert_eq!(material.reward_q32, NEUTRAL_REWARD_Q32);
        assert_eq!((material.generation, material.outcome_count), (1, 1));
        // A minor re-embed keeps the reward and the generation.
        let minor = generation_erasure(&up, micros_to_q32(250_000), 7, &policy);
        assert_eq!(minor, up);

        // Targeted erasure resets only the named ids, in any order.
        let other = apply_capability_outcome(
            &CapabilityReward::neutral(tenant("tenant-a"), OpaqueId::new("e-2").unwrap()),
            ObservedOutcome::Success(true),
            2,
            &policy,
        );
        let mut forward = vec![up.clone(), other.clone()];
        let mut backward = vec![other.clone(), up.clone()];
        let named: BTreeSet<OpaqueId> = [OpaqueId::new("e-1").unwrap()].into_iter().collect();
        selective_erase(&mut forward, &named, 9);
        selective_erase(&mut backward, &named, 9);
        assert_eq!(forward[0].reward_q32, NEUTRAL_REWARD_Q32);
        assert_eq!(forward[1], other);
        assert_eq!(backward[1].reward_q32, NEUTRAL_REWARD_Q32);
        assert_eq!(backward[0], other);

        // Decay moves a reward toward 0.5 from either side.
        let high = decay_reward(&up, 3, &policy);
        assert!(high.reward_q32 < up.reward_q32 && high.reward_q32 > NEUTRAL_REWARD_Q32);
        let low = decay_reward(&down, 3, &policy);
        assert!(low.reward_q32 > down.reward_q32 && low.reward_q32 < NEUTRAL_REWARD_Q32);
        assert_eq!(
            decay_reward(&neutral, 3, &policy).reward_q32,
            NEUTRAL_REWARD_Q32
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R172.5
    #[test]
    fn designation_knobs_read_defaults_and_pins() {
        let defaults = DesignationKnobs::from_policy(&DecisionPolicy::engine_default());
        assert_eq!(
            (defaults.max_required_capabilities, defaults.max_k),
            (32, 64)
        );
        assert_eq!(defaults.oversample_factor, 4);
        for spec in KNOBS {
            assert!(spec.admits(&spec.default), "{}", spec.name);
        }
    }
}
