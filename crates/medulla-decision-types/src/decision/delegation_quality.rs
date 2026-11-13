//! The delegation quality model (MEDULLA-DECISION-ENGINE-R157.1).
//!
//! A delegate is admitted when enough recent observations of its work score at
//! or above a threshold. Scores and thresholds are integer micro-units in
//! `[0, 1000000]`; times are Unix milliseconds.

use serde::{Deserialize, Serialize};

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use medulla_types::contract::Digest256;

/// The largest micro-unit score.
pub const MICRO_ONE: u32 = 1_000_000;

/// The `delegation.*` knobs: `min_observations`, `quality_min`, `min_window`,
/// then `window_max_age`.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "delegation.min_observations",
        unit: "observations",
        default: KnobValue::Integer(20),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(10_000),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "delegation.quality_min",
        unit: "micro-score",
        default: KnobValue::Integer(800_000),
        min: KnobValue::Integer(0),
        max: KnobValue::Integer(1_000_000),
        safety: KnobSafety::Fixed,
    },
    KnobSpec {
        name: "delegation.min_window",
        unit: "seconds",
        default: KnobValue::Integer(3_600),
        min: KnobValue::Integer(60),
        max: KnobValue::Integer(2_592_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "delegation.window_max_age",
        unit: "seconds",
        default: KnobValue::Integer(86_400),
        min: KnobValue::Integer(60),
        max: KnobValue::Integer(604_800),
        safety: KnobSafety::Tunable,
    },
];

/// One scored observation of a delegate's work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QualityObservation {
    pub observation_id: String,
    pub score: u32,
    pub observed_at_ms: u64,
    pub evidence_digest: Digest256,
}

/// The time window the observations cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationQualityWindow {
    pub start_ms: u64,
    pub end_ms: u64,
    pub observation_count: u32,
}

/// What a delegate must reach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QualityThreshold {
    pub metric_name: String,
    pub minimum_value: u32,
    pub min_observations: u32,
}

/// The admission verdict and the evidence it read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DelegationAdmission {
    pub admitted: bool,
    pub reason: String,
    pub quality_value: u32,
    pub quality_evidence_digest: Digest256,
}

impl QualityObservation {
    /// The score is within `[0, 1000000]`.
    pub fn checked(self) -> Result<Self, String> {
        if self.score > MICRO_ONE {
            return Err(format!("score {} exceeds {MICRO_ONE}", self.score));
        }
        Ok(self)
    }
}

impl DelegationQualityWindow {
    /// The window is not inverted.
    pub fn checked(self) -> Result<Self, String> {
        if self.start_ms > self.end_ms {
            return Err("window start is after its end".into());
        }
        Ok(self)
    }
}

/// SHA-256 over the ordered `observation_id` and `evidence_digest` pairs.
pub fn evidence_digest(observations: &[QualityObservation]) -> Digest256 {
    let mut fields: Vec<Vec<u8>> = Vec::new();
    for observation in observations {
        fields.push(observation.observation_id.as_bytes().to_vec());
        fields.push(observation.evidence_digest.as_bytes().to_vec());
    }
    let refs: Vec<&[u8]> = fields.iter().map(Vec::as_slice).collect();
    Digest256::framed(b"eg/delegation-quality-evidence", &refs)
        .unwrap_or_else(|_| Digest256::sha256(b""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(id: &str, score: u32) -> QualityObservation {
        QualityObservation {
            observation_id: id.into(),
            score,
            observed_at_ms: 1_700_000_000_000,
            evidence_digest: Digest256::sha256(id.as_bytes()),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R157.1
    #[test]
    fn types_round_trip() {
        let observations = vec![observation("o1", 900_000), observation("o2", 700_000)];
        let admission = DelegationAdmission {
            admitted: true,
            reason: "quality at or above threshold".into(),
            quality_value: 800_000,
            quality_evidence_digest: evidence_digest(&observations),
        };
        let window = DelegationQualityWindow {
            start_ms: 1,
            end_ms: 2,
            observation_count: 2,
        };
        let threshold = QualityThreshold {
            metric_name: "task_success".into(),
            minimum_value: 800_000,
            min_observations: 20,
        };
        for observation in &observations {
            let text = serde_json::to_string(observation).unwrap();
            assert_eq!(
                &serde_json::from_str::<QualityObservation>(&text).unwrap(),
                observation
            );
        }
        let text = serde_json::to_string(&admission).unwrap();
        assert_eq!(
            serde_json::from_str::<DelegationAdmission>(&text).unwrap(),
            admission
        );
        let text = serde_json::to_string(&window).unwrap();
        assert_eq!(
            serde_json::from_str::<DelegationQualityWindow>(&text).unwrap(),
            window
        );
        let text = serde_json::to_string(&threshold).unwrap();
        assert_eq!(
            serde_json::from_str::<QualityThreshold>(&text).unwrap(),
            threshold
        );

        // The evidence digest depends on the order and content of the pairs.
        let reversed = vec![observation("o2", 700_000), observation("o1", 900_000)];
        assert_ne!(evidence_digest(&observations), evidence_digest(&reversed));
        assert!(observation("o3", 1_000_001).checked().is_err());
        assert!(observation("o3", 1_000_000).checked().is_ok());
        assert!(DelegationQualityWindow {
            start_ms: 3,
            end_ms: 2,
            observation_count: 0
        }
        .checked()
        .is_err());
        for spec in KNOBS {
            assert!(spec.admits(&spec.default), "{}", spec.name);
        }
    }
}
