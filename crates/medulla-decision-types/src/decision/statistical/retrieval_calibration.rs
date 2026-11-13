//! The rerank calibration head (MEDULLA-DECISION-ENGINE-R822): an isotonic
//! curve fitted from judged outcomes that maps one scorer version's raw score
//! to the probability the unit is relevant.
//!
//! The curve is fixed-point (parts per million) so its body and digest are
//! identical on every release target. It has at most [`MAX_CALIBRATION_KNOTS`]
//! knots, strictly increasing in score and non-decreasing in relevance.

use serde::{Deserialize, Serialize};

use super::super::jobs::RecordWindow;
use medulla_types::contract::BoundedVec;

/// Format identity of a calibration curve.
pub const CALIBRATION_SCHEMA_VERSION: u16 = 1;
/// Most knots one curve carries.
pub const MAX_CALIBRATION_KNOTS: usize = 64;
/// One whole unit in parts per million.
pub const PPM: u32 = 1_000_000;

/// The champion pointer key of one scorer version's calibration curve. The
/// `RerankCalibration` revision subject and every reader share this one
/// format.
pub fn champion_pointer_key(scorer_version: &str) -> String {
    format!("champion:rerank_calibration:{scorer_version}")
}

/// Fit the calibration curve of one scorer version from judged outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CalibrationFitRequest {
    /// The scorer version whose raw scores are calibrated.
    pub scorer_version: String,
    /// Restrict the judged outcomes to one question's records.
    #[serde(default)]
    pub question_id: Option<String>,
    pub window: RecordWindow,
}

/// One knot: a raw score and the relevance rate the curve maps it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CalibrationKnot {
    pub score_ppm: u32,
    pub relevant_ppm: u32,
}

/// A fitted isotonic curve for one scorer version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CalibrationCurve {
    pub schema_version: u16,
    pub scorer_version: String,
    pub knots: BoundedVec<CalibrationKnot, MAX_CALIBRATION_KNOTS>,
}

impl CalibrationCurve {
    /// True when scores strictly increase and relevance never decreases.
    pub fn is_monotone(&self) -> bool {
        self.knots
            .iter()
            .zip(self.knots.iter().skip(1))
            .all(|(a, b)| a.score_ppm < b.score_ppm && a.relevant_ppm <= b.relevant_ppm)
    }

    /// The calibrated relevance of a raw score: the first or last knot's
    /// value outside the fitted range, linear between knots inside it.
    pub fn calibrate(&self, score_ppm: u32) -> u32 {
        let knots: Vec<&CalibrationKnot> = self.knots.iter().collect();
        let (Some(first), Some(last)) = (knots.first(), knots.last()) else {
            return score_ppm;
        };
        if score_ppm <= first.score_ppm {
            return first.relevant_ppm;
        }
        if score_ppm >= last.score_ppm {
            return last.relevant_ppm;
        }
        let at = knots.partition_point(|k| k.score_ppm <= score_ppm);
        let (low, high) = (knots[at - 1], knots[at]);
        let span = u128::from(high.score_ppm - low.score_ppm);
        let rise = u128::from(high.relevant_ppm - low.relevant_ppm);
        let along = u128::from(score_ppm - low.score_ppm);
        low.relevant_ppm + u32::try_from(rise * along / span).unwrap_or(0)
    }
}

/// What a fit measured; its digest qualifies the curve for activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CalibrationReceipt {
    pub curve_digest: String,
    pub scorer_version: String,
    pub n_pairs: u64,
    pub n_relevant: u64,
    pub passed: bool,
}

/// A fitted curve, its receipt and the digests they are pinned by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CalibrationFitted {
    pub curve_digest: String,
    pub curve: CalibrationCurve,
    pub receipt_digest: String,
    pub receipt: CalibrationReceipt,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(points: &[(u32, u32)]) -> CalibrationCurve {
        CalibrationCurve {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            scorer_version: "v1".to_string(),
            knots: BoundedVec::new(
                points
                    .iter()
                    .map(|(s, r)| CalibrationKnot {
                        score_ppm: *s,
                        relevant_ppm: *r,
                    })
                    .collect(),
            )
            .unwrap(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R822
    #[test]
    fn calibrate_clamps_outside_and_interpolates_inside() {
        let c = curve(&[(100_000, 200_000), (500_000, 600_000), (900_000, 1_000_000)]);
        assert!(c.is_monotone());
        assert_eq!(c.calibrate(0), 200_000);
        assert_eq!(c.calibrate(100_000), 200_000);
        assert_eq!(c.calibrate(300_000), 400_000);
        assert_eq!(c.calibrate(900_000), 1_000_000);
        assert_eq!(c.calibrate(950_000), 1_000_000);
        assert!(!curve(&[(1, 5), (2, 4)]).is_monotone());
    }
}
