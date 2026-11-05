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

/// Default hydration scope cap in work items (MEDULLA-REPO-INGEST-R650.2).
pub const DEFAULT_HYDRATION_SCOPE_CAP: u32 = 10_000;
/// Inclusive bounds of `hydration_scope_cap`.
pub const HYDRATION_SCOPE_CAP_BOUNDS: (u32, u32) = (100, 1_000_000);
/// Default target file count of one repository split part (MEDULLA-REPO-INGEST-R650.2).
pub const DEFAULT_HYDRATION_SPLIT_TARGET_FILES: u32 = 2_000;
/// Inclusive bounds of `hydration_split_target_files`.
pub const HYDRATION_SPLIT_TARGET_FILES_BOUNDS: (u32, u32) = (100, 100_000);
/// Environment key overriding `hydration_scope_cap` at startup.
pub const ENV_HYDRATION_SCOPE_CAP: &str = "MEDULLA_HYDRATION_SCOPE_CAP";
/// Environment key overriding `hydration_split_target_files` at startup.
pub const ENV_HYDRATION_SPLIT_TARGET_FILES: &str = "MEDULLA_HYDRATION_SPLIT_TARGET_FILES";

fn default_hydration_scope_cap() -> u32 {
    DEFAULT_HYDRATION_SCOPE_CAP
}

fn default_hydration_split_target_files() -> u32 {
    DEFAULT_HYDRATION_SPLIT_TARGET_FILES
}

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
    /// Largest baseline-hydration scope admitted, in work items.
    #[serde(default = "default_hydration_scope_cap")]
    pub hydration_scope_cap: u32,
    /// Target file count of one repository split part.
    #[serde(default = "default_hydration_split_target_files")]
    pub hydration_split_target_files: u32,
}

/// Invalid scheduler policy, refused before it can govern admission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidSchedulerConfig {
    BlankLane,
    CoresPerMilleOutOfRange,
    ZeroMemoryPerWorker,
    OversubscribedFloors { total_workers: u32 },
    HydrationScopeCapOutOfRange,
    HydrationSplitTargetOutOfRange,
    InvalidEnvValue(&'static str),
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
            Self::HydrationScopeCapOutOfRange => write!(
                f,
                "scheduler hydration_scope_cap must be within [{}, {}]",
                HYDRATION_SCOPE_CAP_BOUNDS.0, HYDRATION_SCOPE_CAP_BOUNDS.1
            ),
            Self::HydrationSplitTargetOutOfRange => write!(
                f,
                "scheduler hydration_split_target_files must be within [{}, {}]",
                HYDRATION_SPLIT_TARGET_FILES_BOUNDS.0, HYDRATION_SPLIT_TARGET_FILES_BOUNDS.1
            ),
            Self::InvalidEnvValue(key) => write!(f, "{key} must be an unsigned integer"),
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
        if !(HYDRATION_SCOPE_CAP_BOUNDS.0..=HYDRATION_SCOPE_CAP_BOUNDS.1)
            .contains(&self.hydration_scope_cap)
        {
            return Err(InvalidSchedulerConfig::HydrationScopeCapOutOfRange);
        }
        if !(HYDRATION_SPLIT_TARGET_FILES_BOUNDS.0..=HYDRATION_SPLIT_TARGET_FILES_BOUNDS.1)
            .contains(&self.hydration_split_target_files)
        {
            return Err(InvalidSchedulerConfig::HydrationSplitTargetOutOfRange);
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

    /// Apply the `MEDULLA_<NAME>` startup overrides of the hydration keys and
    /// re-validate, so an out-of-range value refuses startup.
    pub fn apply_env_overrides(
        &mut self,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<(), InvalidSchedulerConfig> {
        for (key, slot) in [
            (ENV_HYDRATION_SCOPE_CAP, &mut self.hydration_scope_cap),
            (
                ENV_HYDRATION_SPLIT_TARGET_FILES,
                &mut self.hydration_split_target_files,
            ),
        ] {
            if let Some(raw) = lookup(key) {
                *slot = raw
                    .trim()
                    .parse()
                    .map_err(|_| InvalidSchedulerConfig::InvalidEnvValue(key))?;
            }
        }
        self.validate()
    }
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

    // spec: MEDULLA-REPO-INGEST-R650.2
    #[test]
    fn hydration_knobs_default_override_and_refuse_out_of_range() {
        let mut config = config();
        assert_eq!(config.hydration_scope_cap, 10_000);
        assert_eq!(config.hydration_split_target_files, 2_000);
        config
            .apply_env_overrides(|key| (key == ENV_HYDRATION_SCOPE_CAP).then(|| "500".into()))
            .unwrap();
        assert_eq!(config.hydration_scope_cap, 500);
        assert_eq!(
            config.apply_env_overrides(|key| {
                (key == ENV_HYDRATION_SPLIT_TARGET_FILES).then(|| "99".into())
            }),
            Err(InvalidSchedulerConfig::HydrationSplitTargetOutOfRange)
        );
        let mut config = self::config();
        assert_eq!(
            config.apply_env_overrides(|key| (key == ENV_HYDRATION_SCOPE_CAP).then(|| "x".into())),
            Err(InvalidSchedulerConfig::InvalidEnvValue(
                ENV_HYDRATION_SCOPE_CAP
            ))
        );
    }
}
