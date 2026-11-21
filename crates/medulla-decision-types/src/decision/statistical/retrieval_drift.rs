//! Retrieval drift signals and refresh plans (MEDULLA-DECISION-ENGINE-R174.8.1).
//!
//! A [`DriftSignal`] says a retrieval scope drifted past a threshold; a
//! [`RefreshPlan`] names the bounded set of regions to refresh for it. Both
//! are serde types with a validated constructor; nothing here reads a clock.

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;

use crate::decision::record::outcome::RefusalCode;

/// Format identity of a drift signal and a refresh plan.
pub const RETRIEVAL_DRIFT_SCHEMA_VERSION: u16 = 1;
/// Most regions one refresh plan may name.
pub const MAX_REFRESH_REGIONS: usize = 4_096;

/// An evaluation of a graph with no registered drift policy
/// (MEDULLA-DECISION-ENGINE-R174.8.4).
pub const DRIFT_POLICY_MISSING: RefusalCode = RefusalCode::new("DRIFT_POLICY_MISSING");
/// A firing plan names more regions than the graph's drift policy allows
/// (MEDULLA-DECISION-ENGINE-R174.8.4).
pub const REFRESH_REGION_BUDGET: RefusalCode = RefusalCode::new("REFRESH_REGION_BUDGET");
/// A shard's generation is not the one the plan was scheduled against
/// (MEDULLA-DECISION-ENGINE-R174.8.4).
pub const SHARD_SWAP_CONFLICT: RefusalCode = RefusalCode::new("SHARD_SWAP_CONFLICT");

/// The drift refusals, registered with the decision refusal registry
/// (`codes::module_tables`); each is also a `StatisticalErrorCode`.
pub const REFUSALS: &[RefusalCode] = &[
    DRIFT_POLICY_MISSING,
    REFRESH_REGION_BUDGET,
    SHARD_SWAP_CONFLICT,
];

/// Most regions a drift policy can allow; the cap of [`RefreshPlan`].
pub const MAX_REGION_BUDGET: u32 = MAX_REFRESH_REGIONS as u32;

/// The drift policy of one tenant's logical graph (MEDULLA-DECISION-ENGINE-R174.8.4).
///
/// `region_budget` can only lower the [`MAX_REFRESH_REGIONS`] cap of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DriftPolicyNode {
    pub schema_version: u16,
    pub logical_graph: String,
    pub region_budget: u32,
}

impl DriftPolicyNode {
    /// The validating constructor: the current schema, a named graph and a
    /// budget in `[1, 4096]`.
    pub fn new(logical_graph: String, region_budget: u32) -> Result<Self, String> {
        if logical_graph.trim().is_empty() {
            return Err("a drift policy names a logical graph".into());
        }
        if !(1..=MAX_REGION_BUDGET).contains(&region_budget) {
            return Err(format!(
                "region_budget must be in [1, {MAX_REGION_BUDGET}], got {region_budget}"
            ));
        }
        Ok(Self {
            schema_version: RETRIEVAL_DRIFT_SCHEMA_VERSION,
            logical_graph,
            region_budget,
        })
    }
}

/// The closed set of drift kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DriftKind {
    DistributionShift,
    VocabularyGrowth,
    QualityDecay,
    Staleness,
}

/// One drift observation over a window, values on the `Q16` scale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DriftSignal {
    pub schema_version: u16,
    pub scope: String,
    pub kind: DriftKind,
    pub value_q16: i64,
    pub threshold_q16: i64,
    /// Observation window length in milliseconds.
    pub window: u64,
    pub reference_generation: String,
    /// Recorded observation time in milliseconds since the epoch; never read by the math.
    pub observed_at: u64,
}

impl DriftSignal {
    /// The validating constructor: the current schema and a named scope.
    pub fn checked(self) -> Result<Self, String> {
        if self.schema_version != RETRIEVAL_DRIFT_SCHEMA_VERSION {
            return Err("drift signal schema version unsupported".into());
        }
        if self.scope.trim().is_empty() || self.reference_generation.trim().is_empty() {
            return Err("drift signal needs a scope and a reference generation".into());
        }
        Ok(self)
    }
}

/// The closed set of refresh priorities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RefreshPriority {
    Downtime,
    OnDemand,
}

/// One region of a logical graph: a shard's nodes of one type over a revision range.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RegionRef {
    pub shard_id: u32,
    pub node_type: String,
    pub revision_from: u64,
    pub revision_to: u64,
}

/// The regions to refresh for one drift, at most [`MAX_REFRESH_REGIONS`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RefreshPlan {
    pub schema_version: u16,
    pub logical_graph: String,
    pub regions: BoundedVec<RegionRef, MAX_REFRESH_REGIONS>,
    pub reason: DriftKind,
    pub priority: RefreshPriority,
    pub target_generation: String,
}

impl RefreshPlan {
    /// The validating constructor: at most [`MAX_REFRESH_REGIONS`] regions,
    /// each with an ordered revision range.
    pub fn new(
        logical_graph: String,
        regions: Vec<RegionRef>,
        reason: DriftKind,
        priority: RefreshPriority,
        target_generation: String,
    ) -> Result<Self, String> {
        if regions.iter().any(|r| r.revision_from > r.revision_to) {
            return Err("refresh region revision range is reversed".into());
        }
        Ok(Self {
            schema_version: RETRIEVAL_DRIFT_SCHEMA_VERSION,
            logical_graph,
            regions: BoundedVec::new(regions)?,
            reason,
            priority,
            target_generation,
        })
    }
}

/// One shard whose drift signals are firing, with the kinds that fire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FiringShard {
    pub shard_id: u32,
    pub kinds: Vec<DriftKind>,
}

/// A refresh plan scheduled for a firing drift (MEDULLA-DECISION-ENGINE-R174.8.3).
///
/// The record is keyed by the digest of its plan, so one drift schedules one plan: the same
/// firing evaluated again finds the record and schedules nothing new.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScheduledRefresh {
    pub plan_digest: String,
    pub plan: RefreshPlan,
    /// Recorded scheduling time in milliseconds since the epoch.
    pub scheduled_at_ms: u64,
    /// The signals that fired, one per firing kind of each firing shard
    /// (MEDULLA-DECISION-ENGINE-R174.8.5): the premises of the plan.
    #[serde(default)]
    pub signals: Vec<DriftSignal>,
    /// The generation of each firing shard when the plan was scheduled; the
    /// swap of `RefreshDrift` checks each against the durable shard pointer.
    #[serde(default)]
    pub shard_generations: Vec<(u32, u64)>,
}

/// One shard whose generation a refresh moved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ShardSwap {
    pub shard_id: u32,
    pub generation_before: u64,
    pub generation_after: u64,
}

/// The receipt of an executed refresh plan, stored under
/// `drift-refresh:<plan_digest>` (MEDULLA-DECISION-ENGINE-R174.8.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DriftRefreshed {
    pub plan_digest: String,
    pub logical_graph: String,
    pub regions: Vec<RegionRef>,
    /// Distinct nodes re-embedded.
    pub reembedded_nodes: u32,
    pub shards: Vec<ShardSwap>,
    pub signals: Vec<DriftSignal>,
}

/// The durable generation pointer of one shard: one graph node per shard,
/// written in the batch that swaps the generation (MEDULLA-DECISION-ENGINE-R174.8.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ShardGenerationNode {
    pub schema_version: u16,
    pub logical_graph: String,
    pub shard_id: u32,
    pub generation: u64,
}

/// The graph node type of a [`ShardGenerationNode`].
pub const SHARD_GENERATION_NODE_TYPE: &str = "DriftShardGeneration";

/// The id of the shard generation node of `shard_id` in `logical_graph`.
pub fn shard_generation_node_id(logical_graph: &str, shard_id: u32) -> String {
    format!("drift-shard:{logical_graph}/{shard_id}")
}

/// What one drift evaluation of a logical graph found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DriftEvaluated {
    pub logical_graph: String,
    /// Shards with a monitor under the caller's tenant.
    pub monitored_shards: u32,
    /// Shards whose signals fire now.
    pub firing: Vec<FiringShard>,
    /// The scheduled plan; `None` when nothing fires.
    pub scheduled: Option<ScheduledRefresh>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(shard_id: u32) -> RegionRef {
        RegionRef {
            shard_id,
            node_type: "Document".into(),
            revision_from: 1,
            revision_to: 2,
        }
    }

    fn plan(n: u32) -> Result<RefreshPlan, String> {
        RefreshPlan::new(
            "g".into(),
            (0..n).map(region).collect(),
            DriftKind::VocabularyGrowth,
            RefreshPriority::Downtime,
            "gen-2".into(),
        )
    }

    // spec: MEDULLA-DECISION-ENGINE-R174.8.1
    #[test]
    fn refresh_plan_round_trips() {
        let full = plan(4_096).unwrap();
        let text = serde_json::to_string(&full).unwrap();
        assert_eq!(serde_json::from_str::<RefreshPlan>(&text).unwrap(), full);
        let signal = DriftSignal {
            schema_version: RETRIEVAL_DRIFT_SCHEMA_VERSION,
            scope: "g/Document".into(),
            kind: DriftKind::DistributionShift,
            value_q16: 3 << 16,
            threshold_q16: 1 << 16,
            window: 60_000,
            reference_generation: "gen-1".into(),
            observed_at: 7,
        }
        .checked()
        .unwrap();
        let text = serde_json::to_string(&signal).unwrap();
        assert!(text.contains("\"distribution_shift\""));
        assert_eq!(serde_json::from_str::<DriftSignal>(&text).unwrap(), signal);
        assert!(serde_json::to_string(&RefreshPriority::OnDemand)
            .unwrap()
            .contains("on_demand"));

        // 4,097 regions refuse through the constructor and the decoder.
        assert!(plan(4_097).is_err());
        let mut wire = serde_json::to_value(&full).unwrap();
        wire["regions"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::to_value(region(4_096)).unwrap());
        assert!(serde_json::from_value::<RefreshPlan>(wire).is_err());
    }
}
