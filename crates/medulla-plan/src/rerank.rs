//! Rerank model (MEDULLA-FEDERATED-QUERY-R652.1).
//!
//! The scorer port `rerank_top` calls from served `ContextCompile`, the
//! calibration curve that maps raw scores onto a shared scale, and the receipt
//! that records what the rerank did. Plain serde types; [`CalibrationCurve`]
//! and [`ScorerId`] are validated on construction.

use serde::{Deserialize, Serialize};

/// Most knots a calibration curve may carry.
pub const MAX_KNOTS: usize = 64;

/// Names a scorer and its version.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScorerId {
    pub name: String,
    pub version: String,
}

impl ScorerId {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Result<Self, RerankError> {
        let id = Self {
            name: name.into(),
            version: version.into(),
        };
        id.validate()?;
        Ok(id)
    }

    pub fn validate(&self) -> Result<(), RerankError> {
        if self.name.trim().is_empty() || self.version.trim().is_empty() {
            return Err(RerankError::InvalidScorerId);
        }
        Ok(())
    }
}

/// A scorer the rerank stage can call.
pub trait RerankScorer {
    fn id(&self) -> ScorerId;
    fn score(&self, query: &str, docs: &[&str]) -> Result<Vec<f32>, RerankError>;
}

/// A monotone non-decreasing piecewise-linear map `(raw, calibrated)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationCurve {
    pub scorer: ScorerId,
    pub knots: Vec<(f32, f32)>,
    /// SHA-256 hex of the curve.
    pub digest: String,
}

impl CalibrationCurve {
    pub fn new(
        scorer: ScorerId,
        knots: Vec<(f32, f32)>,
        digest: impl Into<String>,
    ) -> Result<Self, RerankError> {
        let curve = Self {
            scorer,
            knots,
            digest: digest.into(),
        };
        curve.validate()?;
        Ok(curve)
    }

    /// Refuse an empty, oversized, non-finite or non-monotone knot list and a
    /// digest that is not SHA-256 hex.
    pub fn validate(&self) -> Result<(), RerankError> {
        self.scorer.validate()?;
        if self.knots.is_empty() || self.knots.len() > MAX_KNOTS {
            return Err(RerankError::KnotCount {
                count: self.knots.len(),
            });
        }
        if self
            .knots
            .iter()
            .any(|(x, y)| !x.is_finite() || !y.is_finite())
        {
            return Err(RerankError::NonMonotoneKnots);
        }
        if self
            .knots
            .windows(2)
            .any(|w| w[1].0 < w[0].0 || w[1].1 < w[0].1)
        {
            return Err(RerankError::NonMonotoneKnots);
        }
        if !is_sha256_hex(&self.digest) {
            return Err(RerankError::InvalidDigest);
        }
        Ok(())
    }
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Why the rerank fell back to the input order. Closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    ScorerUnavailable,
    NoCalibration,
}

/// What one rerank did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RerankReceipt {
    pub scorer: ScorerId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_digest: Option<String>,
    pub input_ranks: Vec<u32>,
    pub output_ranks: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<FallbackReason>,
}

/// A refused scorer, curve or score call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum RerankError {
    InvalidScorerId,
    KnotCount { count: usize },
    NonMonotoneKnots,
    InvalidDigest,
    ScorerUnavailable { detail: String },
}

impl std::fmt::Display for RerankError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidScorerId => write!(f, "scorer name and version must be non-empty"),
            Self::KnotCount { count } => {
                write!(f, "calibration has {count} knots; 1 to {MAX_KNOTS} allowed")
            }
            Self::NonMonotoneKnots => {
                write!(f, "calibration knots must be finite and non-decreasing")
            }
            Self::InvalidDigest => write!(f, "calibration digest must be SHA-256 hex"),
            Self::ScorerUnavailable { detail } => write!(f, "scorer unavailable: {detail}"),
        }
    }
}

impl std::error::Error for RerankError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest() -> String {
        "ab".repeat(32)
    }

    #[test]
    fn types_round_trip_and_reject_non_monotone_knots() {
        let scorer = ScorerId::new("cross", "1").unwrap();
        let curve = CalibrationCurve::new(
            scorer.clone(),
            vec![(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)],
            digest(),
        )
        .unwrap();
        let json = serde_json::to_string(&curve).unwrap();
        assert_eq!(
            serde_json::from_str::<CalibrationCurve>(&json).unwrap(),
            curve
        );

        let receipt = RerankReceipt {
            scorer: scorer.clone(),
            calibration_digest: Some(digest()),
            input_ranks: vec![0, 1, 2],
            output_ranks: vec![2, 0, 1],
            fallback: Some(FallbackReason::NoCalibration),
        };
        let json = serde_json::to_string(&receipt).unwrap();
        assert!(json.contains("\"no_calibration\""));
        assert_eq!(
            serde_json::from_str::<RerankReceipt>(&json).unwrap(),
            receipt
        );
        assert!(serde_json::from_str::<FallbackReason>("\"timeout\"").is_err());

        assert_eq!(
            CalibrationCurve::new(scorer.clone(), vec![(0.0, 0.5), (1.0, 0.4)], digest()),
            Err(RerankError::NonMonotoneKnots)
        );
        assert_eq!(
            CalibrationCurve::new(scorer.clone(), vec![(0.5, 0.0), (0.4, 1.0)], digest()),
            Err(RerankError::NonMonotoneKnots)
        );
        let too_many: Vec<(f32, f32)> = (0..=MAX_KNOTS).map(|i| (i as f32, i as f32)).collect();
        assert_eq!(
            CalibrationCurve::new(scorer.clone(), too_many, digest()),
            Err(RerankError::KnotCount {
                count: MAX_KNOTS + 1
            })
        );
        let max: Vec<(f32, f32)> = (0..MAX_KNOTS).map(|i| (i as f32, 1.0)).collect();
        assert!(CalibrationCurve::new(scorer.clone(), max, digest()).is_ok());
        assert_eq!(
            CalibrationCurve::new(scorer, vec![(0.0, 0.0)], "XYZ"),
            Err(RerankError::InvalidDigest)
        );
        assert_eq!(ScorerId::new("", "1"), Err(RerankError::InvalidScorerId));

        let bad: CalibrationCurve =
            serde_json::from_str(&json_curve_with(r#"[[0.0,1.0],[1.0,0.0]]"#)).unwrap();
        assert_eq!(bad.validate(), Err(RerankError::NonMonotoneKnots));
    }

    fn json_curve_with(knots: &str) -> String {
        format!(
            r#"{{"scorer":{{"name":"s","version":"1"}},"knots":{knots},"digest":"{}"}}"#,
            digest()
        )
    }
}
