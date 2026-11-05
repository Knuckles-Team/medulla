//! Authoritative scheduler policy data (MEDULLA-DURABLE-KERNEL-R076.1).
//!
//! The served setting, admission wiring and persistence are R076.2. Consumers
//! must validate this model before use; workers never choose their own policy.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Default fraction of host cores, in thousandths.
pub const DEFAULT_CORES_PER_MILLE: u16 = 360;
/// Default memory budget per worker: exactly three binary gibibytes.
pub const DEFAULT_MEMORY_PER_WORKER_BYTES: u64 = 3 * 1024 * 1024 * 1024;

fn default_cores_per_mille() -> u16 {
    DEFAULT_CORES_PER_MILLE
}

fn default_memory_per_worker_bytes() -> u64 {
    DEFAULT_MEMORY_PER_WORKER_BYTES
}

/// Engine-owned policy. Pool size and queue bound must be configured explicitly.
/// Zero workers, zero CPU share, or zero queue depth can disable that resource;
/// no implicit minimum may override those limits. Lane IDs are engine-defined
/// worker lanes, not the decision engine's fast/medium/slow ingestion lanes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SchedulerConfig {
    pub total_workers: u32,
    #[serde(default)]
    pub lane_floors: BTreeMap<String, u32>,
    #[serde(default = "default_cores_per_mille")]
    pub cores_per_mille: u16,
    #[serde(default = "default_memory_per_worker_bytes")]
    pub memory_per_worker_bytes: u64,
    pub max_queue_depth: u64,
}

/// Invalid scheduler policy, refused before it can govern admission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidSchedulerConfig {
    BlankLane,
    CoresPerMilleOutOfRange,
    ZeroMemoryPerWorker,
    OversubscribedFloors { total_workers: u32 },
}

impl std::fmt::Display for InvalidSchedulerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BlankLane => write!(f, "scheduler lane must not be blank"),
            Self::CoresPerMilleOutOfRange => {
                write!(f, "scheduler cores_per_mille must be at most 1000")
            }
            Self::ZeroMemoryPerWorker => {
                write!(f, "scheduler memory_per_worker_bytes must be nonzero")
            }
            Self::OversubscribedFloors { total_workers } => {
                write!(
                    f,
                    "scheduler lane floors exceed total workers {total_workers}"
                )
            }
        }
    }
}

impl std::error::Error for InvalidSchedulerConfig {}

impl SchedulerConfig {
    /// Validate all policy bounds without allocating workers or changing state.
    pub fn validate(&self) -> Result<(), InvalidSchedulerConfig> {
        if self.cores_per_mille > 1000 {
            return Err(InvalidSchedulerConfig::CoresPerMilleOutOfRange);
        }
        if self.memory_per_worker_bytes == 0 {
            return Err(InvalidSchedulerConfig::ZeroMemoryPerWorker);
        }
        let mut available = self.total_workers;
        for (lane, floor) in &self.lane_floors {
            if lane.trim().is_empty() {
                return Err(InvalidSchedulerConfig::BlankLane);
            }
            // Subtract rather than sum: even u32::MAX floors cannot wrap.
            available = available.checked_sub(*floor).ok_or(
                InvalidSchedulerConfig::OversubscribedFloors {
                    total_workers: self.total_workers,
                },
            )?;
        }
        Ok(())
    }
}

/// Default of `knob reasoning.rule_max_steps` (MEDULLA-FEDERATED-QUERY-R608.2).
pub const DEFAULT_RULE_MAX_STEPS: u64 = 100_000;
/// Bounds of `knob reasoning.rule_max_steps`, in rule evaluations.
pub const RULE_MAX_STEPS_RANGE: (u64, u64) = (1_000, 10_000_000);
/// Default of `knob reasoning.causal_nrmse_max` (MEDULLA-FEDERATED-QUERY-R608.2).
pub const DEFAULT_CAUSAL_NRMSE_MAX: f64 = 0.25;
/// Bounds of `knob reasoning.causal_nrmse_max`, a ratio.
pub const CAUSAL_NRMSE_MAX_RANGE: (f64, f64) = (0.01, 1.0);

/// Reasoning knobs of the server configuration, read from
/// `MEDULLA_REASONING_RULE_MAX_STEPS` and `MEDULLA_REASONING_CAUSAL_NRMSE_MAX`
/// and bounds-checked at startup.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningKnobs {
    pub rule_max_steps: u64,
    pub causal_nrmse_max: f64,
}

impl Default for ReasoningKnobs {
    fn default() -> Self {
        Self {
            rule_max_steps: DEFAULT_RULE_MAX_STEPS,
            causal_nrmse_max: DEFAULT_CAUSAL_NRMSE_MAX,
        }
    }
}

/// A reasoning knob outside its range or unparseable; refuses startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidKnob {
    pub knob: &'static str,
    pub value: String,
}

impl std::fmt::Display for InvalidKnob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "knob {} has invalid value {:?}", self.knob, self.value)
    }
}

impl std::error::Error for InvalidKnob {}

const RULE_MAX_STEPS_KNOB: &str = "reasoning.rule_max_steps";
const CAUSAL_NRMSE_MAX_KNOB: &str = "reasoning.causal_nrmse_max";

impl ReasoningKnobs {
    /// Read the knobs through `lookup` (the environment in production), apply
    /// defaults for unset ones and check every bound.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, InvalidKnob> {
        let mut knobs = Self::default();
        if let Some(raw) = lookup("MEDULLA_REASONING_RULE_MAX_STEPS") {
            knobs.rule_max_steps = raw.trim().parse().map_err(|_| InvalidKnob {
                knob: RULE_MAX_STEPS_KNOB,
                value: raw.clone(),
            })?;
        }
        if let Some(raw) = lookup("MEDULLA_REASONING_CAUSAL_NRMSE_MAX") {
            knobs.causal_nrmse_max = raw.trim().parse().map_err(|_| InvalidKnob {
                knob: CAUSAL_NRMSE_MAX_KNOB,
                value: raw.clone(),
            })?;
        }
        knobs.validate()?;
        Ok(knobs)
    }

    /// Read the knobs from the process environment.
    pub fn from_env() -> Result<Self, InvalidKnob> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Check every knob against its range.
    pub fn validate(&self) -> Result<(), InvalidKnob> {
        let (lo, hi) = RULE_MAX_STEPS_RANGE;
        if !(lo..=hi).contains(&self.rule_max_steps) {
            return Err(InvalidKnob {
                knob: RULE_MAX_STEPS_KNOB,
                value: self.rule_max_steps.to_string(),
            });
        }
        let (lo, hi) = CAUSAL_NRMSE_MAX_RANGE;
        if !(lo..=hi).contains(&self.causal_nrmse_max) {
            return Err(InvalidKnob {
                knob: CAUSAL_NRMSE_MAX_KNOB,
                value: self.causal_nrmse_max.to_string(),
            });
        }
        Ok(())
    }
}

static REASONING_KNOBS: std::sync::OnceLock<ReasoningKnobs> = std::sync::OnceLock::new();

/// Install the startup-validated reasoning knobs; the first install wins.
pub fn install_reasoning_knobs(knobs: ReasoningKnobs) {
    let _ = REASONING_KNOBS.set(knobs);
}

/// The installed reasoning knobs, or the defaults when startup installed none
/// (embedded and test construction).
pub fn reasoning_knobs() -> ReasoningKnobs {
    REASONING_KNOBS.get().copied().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> SchedulerConfig {
        serde_json::from_value(json!({
            "total_workers": 4,
            "lane_floors": {"general": 1, "hydration": 2},
            "max_queue_depth": 100
        }))
        .unwrap()
    }

    // spec: MEDULLA-PLACEMENT-R002.2
    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn defaults_and_lane_floors_round_trip() {
        let config = config();
        assert_eq!(config.cores_per_mille, 360);
        assert_eq!(config.memory_per_worker_bytes, 3_221_225_472);
        assert_eq!(config.validate(), Ok(()));
        let encoded = serde_json::to_string(&config).unwrap();
        assert_eq!(
            serde_json::from_str::<SchedulerConfig>(&encoded).unwrap(),
            config
        );
    }

    // spec: MEDULLA-PLACEMENT-R002.2
    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn explicit_autosize_and_queue_limits_round_trip() {
        let mut config = config();
        config.cores_per_mille = 1000;
        config.memory_per_worker_bytes = 1;
        config.max_queue_depth = u64::MAX;
        assert_eq!(config.validate(), Ok(()));
        let encoded = serde_json::to_value(&config).unwrap();
        assert_eq!(
            serde_json::from_value::<SchedulerConfig>(encoded).unwrap(),
            config
        );
    }

    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn exact_floor_sum_is_accepted_and_oversubscription_is_refused() {
        let mut config = config();
        config.total_workers = 3;
        assert_eq!(config.validate(), Ok(()));
        config.total_workers = 2;
        assert_eq!(
            config.validate(),
            Err(InvalidSchedulerConfig::OversubscribedFloors { total_workers: 2 })
        );
    }

    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn huge_floors_cannot_wrap_into_an_accepted_sum() {
        let mut config = config();
        config.total_workers = u32::MAX;
        config.lane_floors = BTreeMap::from([("a".into(), u32::MAX), ("b".into(), 1)]);
        assert_eq!(
            config.validate(),
            Err(InvalidSchedulerConfig::OversubscribedFloors {
                total_workers: u32::MAX
            })
        );
    }

    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn zero_limits_disable_resources_without_an_implicit_floor() {
        let config: SchedulerConfig = serde_json::from_value(json!({
            "total_workers": 0, "cores_per_mille": 0, "max_queue_depth": 0
        }))
        .unwrap();
        assert!(config.lane_floors.is_empty());
        assert_eq!(config.validate(), Ok(()));
    }

    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn invalid_policy_bounds_are_refused() {
        let mut config = config();
        config.cores_per_mille = 1001;
        assert_eq!(
            config.validate(),
            Err(InvalidSchedulerConfig::CoresPerMilleOutOfRange)
        );
        config.cores_per_mille = 360;
        config.memory_per_worker_bytes = 0;
        assert_eq!(
            config.validate(),
            Err(InvalidSchedulerConfig::ZeroMemoryPerWorker)
        );
        config.memory_per_worker_bytes = 1;
        config.lane_floors.insert("  ".into(), 0);
        assert_eq!(config.validate(), Err(InvalidSchedulerConfig::BlankLane));
    }

    // spec: MEDULLA-DURABLE-KERNEL-R076.1
    #[test]
    fn wire_requires_explicit_limits_and_rejects_unknown_or_negative_values() {
        for value in [
            json!({"total_workers": 1}),
            json!({"max_queue_depth": 1}),
            json!({"total_workers": 1, "max_queue_depth": 1, "worker_policy": true}),
            json!({"total_workers": -1, "max_queue_depth": 1}),
            json!({"total_workers": 1, "max_queue_depth": -1}),
            json!({"total_workers": 1, "max_queue_depth": 1, "cores_per_mille": 65536}),
        ] {
            assert!(serde_json::from_value::<SchedulerConfig>(value).is_err());
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R608.2
    #[test]
    fn reasoning_knobs_default_and_refuse_out_of_range() {
        let none = ReasoningKnobs::from_lookup(|_| None).unwrap();
        assert_eq!(none, ReasoningKnobs::default());
        let set = ReasoningKnobs::from_lookup(|k| match k {
            "MEDULLA_REASONING_RULE_MAX_STEPS" => Some("1000".into()),
            "MEDULLA_REASONING_CAUSAL_NRMSE_MAX" => Some("1.0".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!((set.rule_max_steps, set.causal_nrmse_max), (1_000, 1.0));
        let low = ReasoningKnobs::from_lookup(|k| {
            (k == "MEDULLA_REASONING_RULE_MAX_STEPS").then(|| "999".into())
        });
        assert_eq!(low.unwrap_err().knob, "reasoning.rule_max_steps");
        let high = ReasoningKnobs::from_lookup(|k| {
            (k == "MEDULLA_REASONING_CAUSAL_NRMSE_MAX").then(|| "1.5".into())
        });
        assert_eq!(high.unwrap_err().knob, "reasoning.causal_nrmse_max");
    }
}
