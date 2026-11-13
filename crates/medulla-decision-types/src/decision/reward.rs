//! Reward decomposition: the wire model (MEDULLA-DECISION-ENGINE-R751.750).
//!
//! A trajectory earned one reward. `Decide` splits it over the steps that made
//! it, weighting each step by the progress its value estimate shows, and rolls
//! the step rewards up a bounded task hierarchy. Rewards, value estimates and
//! advantages are exact fixed-point numbers on `Q32`. Nothing here reads a
//! clock or a store.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::numeric::QuantisedValue;
use super::policy::DecisionPolicy;
use super::record::outcome::RefusalCode;
use medulla_types::contract::BoundedVec;

/// Most steps one trajectory may carry.
pub const MAX_REWARD_STEPS: usize = 4096;
/// Most hierarchy nodes one trajectory may carry.
pub const MAX_HTN_NODES: usize = 256;

/// Refusal code: the task hierarchy is deeper than `knob htn.max_depth`.
pub const REWARD_HTN_TOO_DEEP: RefusalCode = RefusalCode::new("REWARD_HTN_TOO_DEEP");
/// Refusal code: a parent index is not below its child, or a child's step
/// range lies outside its parent's.
pub const REWARD_HTN_INVALID: RefusalCode = RefusalCode::new("REWARD_HTN_INVALID");
/// Refusal code: the proposed rewards miss the trajectory reward by more than
/// `knob reward.sum_tolerance`.
pub const REWARD_SUM_MISMATCH: RefusalCode = RefusalCode::new("REWARD_SUM_MISMATCH");
/// Refusal code: an advantage was asked of fewer than two steps or of rewards
/// with no deviation.
pub const REWARD_ADVANTAGE_UNDEFINED: RefusalCode = RefusalCode::new("REWARD_ADVANTAGE_UNDEFINED");
/// Refusal code: more steps than `knob reward.max_steps`.
pub const REWARD_TOO_MANY_STEPS: RefusalCode = RefusalCode::new("REWARD_TOO_MANY_STEPS");

/// The `REFUSALS` table of the `reward_decomposition` family, registered in
/// `codes::FAMILY_TABLES` (MEDULLA-DECISION-ENGINE-R751.752).
pub const REFUSALS: &[RefusalCode] = &[
    REWARD_HTN_TOO_DEEP,
    REWARD_HTN_INVALID,
    REWARD_SUM_MISMATCH,
    REWARD_ADVANTAGE_UNDEFINED,
    REWARD_TOO_MANY_STEPS,
];

/// What the caller's value model said after one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StepSignal {
    /// The caller's value estimate on `Q32`.
    pub value_estimate: QuantisedValue,
    /// Whether the step was served from a cache tier.
    pub cache_hit: bool,
}

/// One node of the task hierarchy: a parent index and an inclusive step range.
/// The nodes live in a flat list; a parent index names an earlier entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HtnNode {
    /// Index of the parent node; `None` for a root.
    #[serde(default)]
    pub parent: Option<u32>,
    pub first_step: u32,
    pub last_step: u32,
}

/// A named subgoal the caller reached: the node it closed and its label.
/// The label is data for the caller; it never drives a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SubgoalCheckpoint {
    pub node_index: u32,
    pub label: String,
}

/// The request: one trajectory's reward, its step signals and its hierarchy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RewardDecompositionRequest {
    pub trajectory_id: String,
    /// The reward to split, on `Q32`.
    pub trajectory_reward: QuantisedValue,
    pub steps: BoundedVec<StepSignal, MAX_REWARD_STEPS>,
    pub hierarchy: BoundedVec<HtnNode, MAX_HTN_NODES>,
    /// Whether to return the normalised advantage of each step reward.
    #[serde(default)]
    pub with_advantage: bool,
    /// Step rewards a caller proposes, to be verified instead of computed.
    #[serde(default)]
    pub proposed_rewards: Option<BoundedVec<QuantisedValue, MAX_REWARD_STEPS>>,
}

impl RewardDecompositionRequest {
    /// The validating constructor: at most [`MAX_REWARD_STEPS`] steps,
    /// [`MAX_HTN_NODES`] nodes, and one proposed reward per step.
    pub fn new(
        trajectory_id: String,
        trajectory_reward: QuantisedValue,
        steps: Vec<StepSignal>,
        hierarchy: Vec<HtnNode>,
        with_advantage: bool,
        proposed_rewards: Option<Vec<QuantisedValue>>,
    ) -> Result<Self, String> {
        let step_count = steps.len();
        let proposed = match proposed_rewards {
            Some(rewards) if rewards.len() != step_count => {
                return Err("proposed_rewards must hold one reward per step".to_string())
            }
            Some(rewards) => Some(BoundedVec::new(rewards)?),
            None => None,
        };
        Ok(Self {
            trajectory_id,
            trajectory_reward,
            steps: BoundedVec::new(steps)?,
            hierarchy: BoundedVec::new(hierarchy)?,
            with_advantage,
            proposed_rewards: proposed,
        })
    }
}

/// One step's share of the trajectory reward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StepReward {
    pub step: u32,
    pub reward: QuantisedValue,
    /// The normalised advantage `(reward - mean) / std`; present when asked for.
    #[serde(default)]
    pub advantage: Option<QuantisedValue>,
}

/// The answer: every step reward, every node reward and the training verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TrajectoryReward {
    pub trajectory_id: String,
    pub step_rewards: BoundedVec<StepReward, MAX_REWARD_STEPS>,
    /// The sum of the step rewards in each hierarchy node's range, in node order.
    pub node_rewards: BoundedVec<QuantisedValue, MAX_HTN_NODES>,
    /// True when the trajectory reward lies strictly inside the saturation bounds.
    pub keep_for_training: bool,
}

const fn ratio(
    name: &'static str,
    default_micros: i64,
    min_micros: i64,
    max_micros: i64,
) -> KnobSpec {
    KnobSpec {
        name,
        unit: "ratio",
        default: KnobValue::DecimalMicros(default_micros),
        min: KnobValue::DecimalMicros(min_micros),
        max: KnobValue::DecimalMicros(max_micros),
        safety: KnobSafety::Tunable,
    }
}

/// The `reward.*` knobs (MEDULLA-DECISION-ENGINE-R751.751), all ratios in
/// millionths: the progress floor, the cache-tier discount, the two
/// saturation bounds and the tolerance a proposed split may miss the total by.
pub const KNOBS: &[KnobSpec] = &[
    ratio("reward.difficulty_floor", 10_000, 1_000, 1_000_000),
    ratio("reward.cache_tier_discount", 500_000, 0, 1_000_000),
    ratio("reward.saturation_low", 0, 0, 1_000_000),
    ratio("reward.saturation_high", 1_000_000, 0, 1_000_000),
    ratio("reward.sum_tolerance", 1, 0, 1_000),
    KnobSpec {
        name: "htn.max_depth",
        unit: "levels",
        default: KnobValue::Integer(8),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(32),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "reward.max_steps",
        unit: "steps",
        default: KnobValue::Integer(1024),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(4096),
        safety: KnobSafety::Tunable,
    },
];

/// The pinned `reward.*` knob values, in millionths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RewardKnobs {
    /// `knob reward.difficulty_floor`, range [0.001, 1].
    pub difficulty_floor_micros: i64,
    /// `knob reward.cache_tier_discount`, range [0, 1].
    pub cache_tier_discount_micros: i64,
    /// `knob reward.saturation_low`, range [0, 1].
    pub saturation_low_micros: i64,
    /// `knob reward.saturation_high`, range [0, 1].
    pub saturation_high_micros: i64,
    /// `knob reward.sum_tolerance`, range [0, 0.001].
    pub sum_tolerance_micros: i64,
    /// `knob htn.max_depth`, range [1, 32], levels.
    pub htn_max_depth: u32,
    /// `knob reward.max_steps`, range [1, 4096], steps.
    pub max_steps: u32,
}

impl Default for RewardKnobs {
    fn default() -> Self {
        let read = |index: usize| match KNOBS[index].default {
            KnobValue::DecimalMicros(value) => value,
            _ => 0,
        };
        let whole = |index: usize, fallback: u32| match KNOBS[index].default {
            KnobValue::Integer(value) => u32::try_from(value).unwrap_or(fallback),
            _ => fallback,
        };
        Self {
            difficulty_floor_micros: read(0),
            cache_tier_discount_micros: read(1),
            saturation_low_micros: read(2),
            saturation_high_micros: read(3),
            sum_tolerance_micros: read(4),
            htn_max_depth: whole(5, 8),
            max_steps: whole(6, 1024),
        }
    }
}

impl RewardKnobs {
    /// The knob values `policy` pins, each defaulting to its spec.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let read = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::DecimalMicros(value) => value,
            _ => 0,
        };
        let whole = |index: usize, fallback: u32| match policy.knob(&KNOBS[index]) {
            KnobValue::Integer(value) => u32::try_from(value).unwrap_or(fallback),
            _ => fallback,
        };
        Self {
            difficulty_floor_micros: read(0),
            cache_tier_discount_micros: read(1),
            saturation_low_micros: read(2),
            saturation_high_micros: read(3),
            sum_tolerance_micros: read(4),
            htn_max_depth: whole(5, 8),
            max_steps: whole(6, 1024),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::numeric::QuantScaleTag;
    use super::*;

    fn q32(value: i64) -> QuantisedValue {
        QuantisedValue {
            scale: QuantScaleTag::Q32,
            value,
        }
    }

    fn step(i: i64) -> StepSignal {
        StepSignal {
            value_estimate: q32(i << 28),
            cache_hit: i % 2 == 0,
        }
    }

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: &T) {
        let text = serde_json::to_string(v).unwrap();
        assert_eq!(&serde_json::from_str::<T>(&text).unwrap(), v);
    }

    // spec: MEDULLA-DECISION-ENGINE-R751.750
    #[test]
    fn reward_types_round_trip() {
        round_trip(&step(3));
        let node = HtnNode {
            parent: Some(0),
            first_step: 1,
            last_step: 2,
        };
        round_trip(&node);
        round_trip(&HtnNode {
            parent: None,
            first_step: 0,
            last_step: 3,
        });
        round_trip(&SubgoalCheckpoint {
            node_index: 1,
            label: "open the file".into(),
        });
        let request = RewardDecompositionRequest::new(
            "traj-1".into(),
            q32(1 << 32),
            (0..4).map(step).collect(),
            vec![node],
            true,
            Some((0..4).map(|i| q32(i << 30)).collect()),
        )
        .unwrap();
        round_trip(&request);
        let step_reward = StepReward {
            step: 0,
            reward: q32(1 << 30),
            advantage: Some(q32(-(1 << 31))),
        };
        round_trip(&step_reward);
        round_trip(&TrajectoryReward {
            trajectory_id: "traj-1".into(),
            step_rewards: BoundedVec::new(vec![step_reward]).unwrap(),
            node_rewards: BoundedVec::new(vec![q32(1 << 30)]).unwrap(),
            keep_for_training: true,
        });
        assert_eq!(
            serde_json::to_string(&crate::decision::QuestionKind::RewardDecomposition).unwrap(),
            "\"reward_decomposition\""
        );

        // 4096 steps are admitted; a 4097th is refused by the constructor and
        // by the decoder.
        let full = RewardDecompositionRequest::new(
            "t".into(),
            q32(0),
            (0..4096).map(step).collect(),
            Vec::new(),
            false,
            None,
        )
        .unwrap();
        assert!(RewardDecompositionRequest::new(
            "t".into(),
            q32(0),
            (0..4097).map(step).collect(),
            Vec::new(),
            false,
            None,
        )
        .is_err());
        let mut wire = serde_json::to_value(&full).unwrap();
        wire["steps"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::to_value(step(4096)).unwrap());
        assert!(serde_json::from_value::<RewardDecompositionRequest>(wire).is_err());
        // A proposal must cover every step.
        assert!(RewardDecompositionRequest::new(
            "t".into(),
            q32(0),
            (0..2).map(step).collect(),
            Vec::new(),
            false,
            Some(vec![q32(0)]),
        )
        .is_err());
    }
}
