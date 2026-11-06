//! Retrieval drift signals and refresh plans (MEDULLA-DECISION-ENGINE-R174.8.1).
//!
//! A [`DriftSignal`] says a retrieval scope drifted past a threshold; a
//! [`RefreshPlan`] names the bounded set of regions to refresh for it. Both
//! are serde types with a validated constructor; nothing here reads a clock.

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;

/// Format identity of a drift signal and a refresh plan.
pub const RETRIEVAL_DRIFT_SCHEMA_VERSION: u16 = 1;
/// Most regions one refresh plan may name.
pub const MAX_REFRESH_REGIONS: usize = 4_096;

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
