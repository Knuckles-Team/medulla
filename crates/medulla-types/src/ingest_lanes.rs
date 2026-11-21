//! Ingest lanes, queue classes and lane metrics (MEDULLA-INGEST-WORKER-R604.1).
//!
//! A [`Lane`] is a validated lane id. Whether a lane exists is decided by the
//! lane registry of the engine's scheduler configuration: a caller that holds
//! that registry passes its lane ids to [`Lane::registered_in`]. This module
//! reaches nothing above `medulla-types`.

use serde::{Deserialize, Serialize};

/// Longest lane id, in characters.
pub const MAX_LANE_ID_LEN: usize = 32;

/// A lane id: `[a-z][a-z0-9-]{0,31}`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Lane(String);

/// Why a lane id was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LaneError {
    #[error("lane id must match [a-z][a-z0-9-]{{0,31}}: '{0}'")]
    Invalid(String),
    #[error("lane '{0}' is not in the lane registry")]
    Unregistered(String),
}

impl Lane {
    /// A lane id, or `LaneError::Invalid`.
    pub fn new(id: impl Into<String>) -> Result<Self, LaneError> {
        let id = id.into();
        let mut chars = id.chars();
        let valid = id.len() <= MAX_LANE_ID_LEN
            && chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if valid {
            Ok(Self(id))
        } else {
            Err(LaneError::Invalid(id))
        }
    }

    /// The general lane.
    pub fn general() -> Self {
        Self("general".to_string())
    }

    /// The hydration lane.
    pub fn hydration() -> Self {
        Self("hydration".to_string())
    }

    /// The id text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// This lane when `registry` (the lane ids of the scheduler configuration)
    /// holds it, or `LaneError::Unregistered`.
    pub fn registered_in<'a>(
        self,
        registry: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, LaneError> {
        if registry.into_iter().any(|id| id == self.0) {
            Ok(self)
        } else {
            Err(LaneError::Unregistered(self.0))
        }
    }
}

impl TryFrom<String> for Lane {
    type Error = LaneError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::new(id)
    }
}

impl From<Lane> for String {
    fn from(lane: Lane) -> Self {
        lane.0
    }
}

/// The cost class of a queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueClass {
    Fast,
    Medium,
    SlowHeavy,
}

/// One stage of the ingest worker loop (MEDULLA-INGEST-WORKER-R604.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestStage {
    SourceCommit,
    Projection,
    Lexical,
    Vector,
    Index,
}

/// The queue class of an ingest stage: `SourceCommit` is fast, `Projection` and `Lexical`
/// are medium, `Vector` and `Index` are slow and heavy. Rotation across lanes belongs to
/// MEDULLA-DURABLE-KERNEL-R071.600 and the hot-spare rule to MEDULLA-DURABLE-KERNEL-R075.
pub fn queue_class_for(stage: IngestStage) -> QueueClass {
    match stage {
        IngestStage::SourceCommit => QueueClass::Fast,
        IngestStage::Projection | IngestStage::Lexical => QueueClass::Medium,
        IngestStage::Vector | IngestStage::Index => QueueClass::SlowHeavy,
    }
}

/// What one lane holds right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneMetrics {
    /// Tasks queued or claimed and not acknowledged.
    pub depth: u64,
    /// Age of the oldest such task, in milliseconds.
    pub oldest_age_ms: u64,
    /// Tasks claimed and running.
    pub inflight: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-INGEST-WORKER-R604.1
    #[test]
    fn lane_ids_validate_and_round_trip() {
        for good in ["general", "hydration", "a", "gpu-2", &"a".repeat(32)] {
            assert_eq!(Lane::new(good).unwrap().as_str(), good);
        }
        for bad in ["", "Upper", "1lane", "-x", "a_b", "a b", &"a".repeat(33)] {
            assert_eq!(
                Lane::new(bad).unwrap_err(),
                LaneError::Invalid(bad.to_string())
            );
        }
        let lane = Lane::new("gpu-2").unwrap();
        let json = serde_json::to_string(&lane).unwrap();
        assert_eq!(json, "\"gpu-2\"");
        assert_eq!(serde_json::from_str::<Lane>(&json).unwrap(), lane);
        assert!(serde_json::from_str::<Lane>("\"Bad\"").is_err());
        assert_eq!(Lane::general().as_str(), "general");
        assert_eq!(Lane::hydration().as_str(), "hydration");

        // A lane must be in the registry.
        assert!(Lane::general().registered_in(["general", "hydration"]).is_ok());
        assert_eq!(
            lane.registered_in(["general"]).unwrap_err(),
            LaneError::Unregistered("gpu-2".into())
        );

        let class = serde_json::to_string(&QueueClass::SlowHeavy).unwrap();
        assert_eq!(class, "\"slow_heavy\"");
        let metrics = LaneMetrics {
            depth: 3,
            oldest_age_ms: 40,
            inflight: 1,
        };
        let json = serde_json::to_string(&metrics).unwrap();
        assert_eq!(serde_json::from_str::<LaneMetrics>(&json).unwrap(), metrics);
        assert!(serde_json::from_str::<LaneMetrics>("{\"depth\":1,\"extra\":2}").is_err());
    }

    // spec: MEDULLA-INGEST-WORKER-R604.2
    #[test]
    fn every_stage_has_one_class() {
        for (stage, class) in [
            (IngestStage::SourceCommit, QueueClass::Fast),
            (IngestStage::Projection, QueueClass::Medium),
            (IngestStage::Lexical, QueueClass::Medium),
            (IngestStage::Vector, QueueClass::SlowHeavy),
            (IngestStage::Index, QueueClass::SlowHeavy),
        ] {
            assert_eq!(queue_class_for(stage), class);
        }
        let json = serde_json::to_string(&IngestStage::SourceCommit).unwrap();
        assert_eq!(json, "\"source_commit\"");
        assert!(serde_json::from_str::<IngestStage>("\"rank\"").is_err());
    }
}
