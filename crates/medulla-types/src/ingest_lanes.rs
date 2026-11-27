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

/// The series of the ingest worker, by name: job outcomes and dead letters of the engine queue,
/// the lane gauges and the admission counter (MEDULLA-INGEST-WORKER-R001.13, R604.5). None of
/// them has a tenant, corpus or job id as a label.
pub const INGEST_SERIES: &[&str] = &[
    "medulla_ingest_jobs_total",
    "medulla_ingest_dead_letter_total",
    "medulla_lane_depth",
    "medulla_lane_oldest_age_seconds",
    "medulla_lane_inflight",
    "medulla_admit_refused_total",
];

/// One sample of an ingest worker series: its name, its labels and its value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IngestMetricSample {
    pub name: String,
    pub labels: std::collections::BTreeMap<String, String>,
    pub value: f64,
}

/// What `IngestWorkerStatus` answers (MEDULLA-INGEST-WORKER-R001.18): every sample of the
/// [`INGEST_SERIES`] the engine holds, ordered by name and labels. A series with no sample yet
/// is absent; nothing is invented as zero.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct IngestWorkerStatusReport {
    pub series: Vec<IngestMetricSample>,
}

/// The labels of one exposition line (`k="v",...`), or `None` when it is malformed.
fn exposition_labels(text: &str) -> Option<std::collections::BTreeMap<String, String>> {
    let mut labels = std::collections::BTreeMap::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let (key, after) = rest.split_once("=\"")?;
        let mut value = String::new();
        let mut chars = after.char_indices();
        let end = loop {
            let (at, c) = chars.next()?;
            match c {
                '\\' => match chars.next()?.1 {
                    'n' => value.push('\n'),
                    other => value.push(other),
                },
                '"' => break at,
                other => value.push(other),
            }
        };
        labels.insert(key.trim().trim_start_matches(',').trim().to_string(), value);
        rest = after[end + 1..].trim_start_matches(',').trim();
    }
    Some(labels)
}

/// The [`INGEST_SERIES`] samples of a Prometheus text exposition, ordered by name and labels.
/// A line that is not a sample of one of those series is skipped.
pub fn ingest_series_from_exposition(exposition: &str) -> IngestWorkerStatusReport {
    let mut series = Vec::new();
    for line in exposition.lines() {
        if line.starts_with('#') {
            continue;
        }
        let Some((head, value)) = line.trim().rsplit_once(' ') else {
            continue;
        };
        let (name, labels) = match head.split_once('{') {
            Some((name, labels)) => match labels.strip_suffix('}').and_then(exposition_labels) {
                Some(labels) => (name, labels),
                None => continue,
            },
            None => (head, std::collections::BTreeMap::new()),
        };
        let Ok(value) = value.parse::<f64>() else {
            continue;
        };
        if INGEST_SERIES.contains(&name) {
            series.push(IngestMetricSample {
                name: name.to_string(),
                labels,
                value,
            });
        }
    }
    series.sort_by(|a, b| (&a.name, &a.labels).cmp(&(&b.name, &b.labels)));
    IngestWorkerStatusReport { series }
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

    // spec: MEDULLA-INGEST-WORKER-R001.18
    #[test]
    fn the_ingest_series_are_read_from_the_exposition_and_nothing_else() {
        let text = "# HELP medulla_ingest_jobs_total Ingest job outcomes\n\
# TYPE medulla_ingest_jobs_total counter\n\
medulla_ingest_jobs_total{outcome=\"enqueued\"} 4\n\
medulla_ingest_jobs_total{outcome=\"completed\"} 3\n\
medulla_ingest_dead_letter_total{queue=\"ingest-hydration\"} 1\n\
medulla_lane_depth{lane=\"general\",queue_class=\"fast\"} 2\n\
medulla_lane_inflight{lane=\"general\"} 1\n\
medulla_request_seconds_count 9\n\
medulla_lane_oldest_age_seconds{lane=\"a\\\"b\"} 7\n\
medulla_admit_refused_total{lane=\"general\"} not-a-number\n";
        let report = ingest_series_from_exposition(text);
        let names: Vec<&str> = report.series.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "medulla_ingest_dead_letter_total",
                "medulla_ingest_jobs_total",
                "medulla_ingest_jobs_total",
                "medulla_lane_depth",
                "medulla_lane_inflight",
                "medulla_lane_oldest_age_seconds",
            ]
        );
        let jobs = &report.series[1];
        assert_eq!(jobs.labels["outcome"], "completed");
        assert_eq!(jobs.value, 3.0);
        let depth = &report.series[3];
        assert_eq!(depth.labels["lane"], "general");
        assert_eq!(depth.labels["queue_class"], "fast");
        assert_eq!(report.series[5].labels["lane"], "a\"b");
        assert_eq!(ingest_series_from_exposition(""), IngestWorkerStatusReport::default());
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(
            serde_json::from_str::<IngestWorkerStatusReport>(&json).unwrap(),
            report
        );
    }
}
