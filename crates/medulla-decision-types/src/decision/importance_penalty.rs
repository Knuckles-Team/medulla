//! Feature-importance penalty model for `DecisionFit`
//! (MEDULLA-DECISION-ENGINE-R151.1).
//!
//! The caller supplies one importance weight per feature; the engine computes
//! none. A fit that names a [`PenaltyObjective`] pulls each weighted
//! coordinate toward the parent head, in proportion to its importance.

use medulla_types::contract::BoundedVec;
use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::numeric::{QuantScaleTag, QuantisedValue};

/// Version of the serialized penalty types.
pub const IMPORTANCE_SCHEMA_VERSION: u32 = 1;

/// The most feature entries one fit request carries.
pub const MAX_IMPORTANCE_FEATURES: usize = 256;

fn schema_version() -> u32 {
    IMPORTANCE_SCHEMA_VERSION
}

/// The knobs of the importance penalty.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "fit.loss_coefficient",
        unit: "unitless",
        default: KnobValue::DecimalMicros(1_000_000),
        min: KnobValue::DecimalMicros(100_000),
        max: KnobValue::DecimalMicros(10_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "fit.importance_scale",
        unit: "unitless",
        default: KnobValue::DecimalMicros(500_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(10_000_000),
        safety: KnobSafety::Tunable,
    },
];

/// One feature's caller-supplied importance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "FeatureImportanceFields")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FeatureImportance {
    pub schema_version: u32,
    pub feature_id: String,
    /// In `[0, 1]`.
    pub importance_weight: QuantisedValue,
    /// SHA-256 of the evidence behind the weight: 64 lowercase hex characters.
    pub evidence_digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
struct FeatureImportanceFields {
    #[serde(default = "schema_version")]
    schema_version: u32,
    feature_id: String,
    importance_weight: QuantisedValue,
    evidence_digest: String,
}

impl TryFrom<FeatureImportanceFields> for FeatureImportance {
    type Error = String;

    fn try_from(fields: FeatureImportanceFields) -> Result<Self, String> {
        Self::new(
            fields.feature_id,
            fields.importance_weight,
            fields.evidence_digest,
        )
        .and_then(|entry| {
            (fields.schema_version == IMPORTANCE_SCHEMA_VERSION)
                .then_some(entry)
                .ok_or_else(|| format!("unknown schema_version {}", fields.schema_version))
        })
    }
}

/// Whether `weight` reads inside `[0, 1]` on its own scale.
fn weight_in_unit_interval(weight: &QuantisedValue) -> bool {
    let one: i128 = match weight.scale {
        QuantScaleTag::Pico => 1_000_000_000_000,
        QuantScaleTag::Q32 => 1_i128 << 32,
    };
    (0..=one).contains(&i128::from(weight.value))
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl FeatureImportance {
    /// Admit one entry: a weight in `[0, 1]` and a well-formed evidence digest.
    pub fn new(
        feature_id: String,
        importance_weight: QuantisedValue,
        evidence_digest: String,
    ) -> Result<Self, String> {
        if feature_id.trim().is_empty() {
            return Err("feature_id is empty".into());
        }
        if !weight_in_unit_interval(&importance_weight) {
            return Err(format!(
                "importance_weight of {feature_id} is outside [0, 1]"
            ));
        }
        if !is_sha256_hex(&evidence_digest) {
            return Err(format!(
                "evidence_digest of {feature_id} is not 64 lowercase hex characters"
            ));
        }
        Ok(Self {
            schema_version: IMPORTANCE_SCHEMA_VERSION,
            feature_id,
            importance_weight,
            evidence_digest,
        })
    }
}

/// The two coefficients of the penalty. `None` on the request means no penalty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PenaltyObjective {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    pub loss_coefficient: QuantisedValue,
    pub importance_scale: QuantisedValue,
}

/// The feature list a request carries.
pub type ImportanceFeatures = BoundedVec<FeatureImportance, MAX_IMPORTANCE_FEATURES>;

#[cfg(test)]
mod tests {
    use super::*;

    fn weight(micros: i64) -> QuantisedValue {
        QuantisedValue {
            scale: QuantScaleTag::Pico,
            value: micros * 1_000_000,
        }
    }

    fn digest() -> String {
        "ab".repeat(32)
    }

    #[test]
    fn importance_round_trip() {
        let entry = FeatureImportance::new("recency".into(), weight(750_000), digest()).unwrap();
        let wire = serde_json::to_string(&entry).unwrap();
        assert_eq!(
            serde_json::from_str::<FeatureImportance>(&wire).unwrap(),
            entry
        );
        let penalty = PenaltyObjective {
            schema_version: IMPORTANCE_SCHEMA_VERSION,
            loss_coefficient: weight(1_000_000),
            importance_scale: weight(500_000),
        };
        let wire = serde_json::to_string(&penalty).unwrap();
        assert_eq!(
            serde_json::from_str::<PenaltyObjective>(&wire).unwrap(),
            penalty
        );

        // A weight outside [0, 1] is rejected at construction and on the wire.
        assert!(FeatureImportance::new("f".into(), weight(1_000_001), digest()).is_err());
        assert!(FeatureImportance::new("f".into(), weight(-1), digest()).is_err());
        let mut bad = serde_json::to_value(&entry).unwrap();
        bad["importance_weight"]["value"] = serde_json::json!(2_000_000_000_000_i64);
        assert!(serde_json::from_value::<FeatureImportance>(bad).is_err());
        // Malformed evidence is rejected.
        assert!(FeatureImportance::new("f".into(), weight(1), "ABC".into()).is_err());
        assert!(FeatureImportance::new("f".into(), weight(1), "A".repeat(64)).is_err());
    }

    #[test]
    fn knob_defaults_lie_inside_bounds() {
        for spec in KNOBS {
            assert!(spec.admits(&spec.default), "{}", spec.name);
            assert_eq!(spec.safety, KnobSafety::Tunable);
        }
    }
}
