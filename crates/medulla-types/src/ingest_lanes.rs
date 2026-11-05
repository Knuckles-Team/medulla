//! Ingest lanes, queue classes and lane metrics (MEDULLA-INGEST-WORKER-R604.1).
//!
//! Pure data. A lane id must exist in the lane registry of
//! [`SchedulerConfig`]; the served admission is a later child of R604.

use crate::scheduler_config::SchedulerConfig;
use serde::{Deserialize, Deserializer, Serialize};

/// Longest lane id, in bytes.
pub const MAX_LANE_ID_BYTES: usize = 32;

/// A lane refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneError {
    /// The id does not match `[a-z][a-z0-9-]{0,31}`.
    InvalidId,
    /// The id is well formed but absent from the scheduler lane registry.
    UnregisteredLane,
}

impl std::fmt::Display for LaneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidId => "lane id must match [a-z][a-z0-9-]{0,31}",
            Self::UnregisteredLane => "lane is not in the scheduler lane registry",
        })
    }
}

impl std::error::Error for LaneError {}

/// A validated lane id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Lane(String);

impl Lane {
    pub fn new(id: impl Into<String>) -> Result<Self, LaneError> {
        let id = id.into();
        let b = id.as_bytes();
        let ok = !b.is_empty()
            && b.len() <= MAX_LANE_ID_BYTES
            && b[0].is_ascii_lowercase()
            && b[1..]
                .iter()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-');
        if ok {
            Ok(Self(id))
        } else {
            Err(LaneError::InvalidId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Refuse a lane that the scheduler policy does not register.
    pub fn check_registered(&self, config: &SchedulerConfig) -> Result<(), LaneError> {
        if config.lane_floors.contains_key(&self.0) {
            Ok(())
        } else {
            Err(LaneError::UnregisteredLane)
        }
    }
}

impl<'de> Deserialize<'de> for Lane {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = String::deserialize(deserializer)?;
        Self::new(id).map_err(serde::de::Error::custom)
    }
}

/// How long a queue's jobs are expected to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum QueueClass {
    Fast,
    Medium,
    SlowHeavy,
}

/// Point-in-time load of one lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LaneMetrics {
    pub depth: u64,
    pub oldest_age_ms: u64,
    pub inflight: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-INGEST-WORKER-R604.1
    #[test]
    fn lane_ids_validate_and_round_trip() {
        let lane = Lane::new("general").unwrap();
        let json = serde_json::to_value(&lane).unwrap();
        assert_eq!(json, "general");
        assert_eq!(serde_json::from_value::<Lane>(json).unwrap(), lane);
        for bad in ["", "Gen", "1a", "a_b", &"a".repeat(33)] {
            assert_eq!(Lane::new(bad).unwrap_err(), LaneError::InvalidId);
            assert!(serde_json::from_value::<Lane>(serde_json::json!(bad)).is_err());
        }
        assert!(Lane::new("a".repeat(32)).is_ok());

        let config: SchedulerConfig = serde_json::from_value(serde_json::json!({
            "total_workers": 4, "lane_floors": {"general": 1, "hydration": 1},
            "max_queue_depth": 10
        }))
        .unwrap();
        assert!(lane.check_registered(&config).is_ok());
        assert_eq!(
            Lane::new("other").unwrap().check_registered(&config).unwrap_err(),
            LaneError::UnregisteredLane
        );

        for class in [QueueClass::Fast, QueueClass::Medium, QueueClass::SlowHeavy] {
            let v = serde_json::to_value(class).unwrap();
            assert_eq!(serde_json::from_value::<QueueClass>(v).unwrap(), class);
        }
        assert_eq!(serde_json::to_value(QueueClass::SlowHeavy).unwrap(), "slow_heavy");
        let metrics = LaneMetrics { depth: 3, oldest_age_ms: 40, inflight: 1 };
        let back: LaneMetrics = serde_json::from_value(serde_json::to_value(metrics).unwrap()).unwrap();
        assert_eq!(back, metrics);

        use medulla_source_types::ingest_job::IngestLane;
        assert_eq!(IngestLane::General.lane_id(), "general");
        assert_eq!(IngestLane::Hydration.lane_id(), "hydration");
        assert!(Lane::new(IngestLane::General.lane_id()).is_ok());
    }
}
