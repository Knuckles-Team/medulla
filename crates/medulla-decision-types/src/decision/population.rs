//! Population drift and fan-out width: the wire model (MEDULLA-DECISION-ENGINE-R752.750).
//!
//! `Decide` compares a reference window of scores to a current window with the
//! first Wasserstein distance, and picks how many replicas to fan out from the
//! diversity of their answers. All numbers are exact fixed-point values on
//! `Q32`. Nothing here reads a clock or a store.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::numeric::{QuantScaleTag, QuantisedValue};
use super::policy::DecisionPolicy;
use medulla_types::contract::BoundedVec;

/// Most samples one drift window may hold: the maximum of `knob drift.max_samples`.
pub const MAX_DRIFT_SAMPLES: usize = 2048;
/// Most replicas one spread may describe.
pub const MAX_SPREAD_REPLICAS: usize = 16;
/// Most pairwise entries of one spread: the upper triangle of 16 replicas.
pub const MAX_PAIRWISE: usize = MAX_SPREAD_REPLICAS * (MAX_SPREAD_REPLICAS - 1) / 2;

/// The population knobs: `drift.max_samples` (index 0), the alert threshold
/// and the fan-out width rule (MEDULLA-DECISION-ENGINE-R752.751).
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "drift.max_samples",
        unit: "samples per window",
        default: KnobValue::Integer(2048),
        min: KnobValue::Integer(16),
        max: KnobValue::Integer(2048),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "drift.alert",
        unit: "ratio",
        default: KnobValue::DecimalMicros(100_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "fanout.diversity_high",
        unit: "ratio",
        default: KnobValue::DecimalMicros(400_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "fanout.diversity_low",
        unit: "ratio",
        default: KnobValue::DecimalMicros(150_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "fanout.replica_ceiling",
        unit: "replicas",
        default: KnobValue::Integer(8),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(64),
        safety: KnobSafety::Tunable,
    },
];

/// The pinned drift and fan-out knob values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopulationKnobs {
    /// `knob drift.alert`, millionths.
    pub drift_alert_micros: i64,
    /// `knob fanout.diversity_high`, millionths.
    pub diversity_high_micros: i64,
    /// `knob fanout.diversity_low`, millionths.
    pub diversity_low_micros: i64,
    /// `knob fanout.replica_ceiling`, replicas.
    pub replica_ceiling: u32,
}

impl PopulationKnobs {
    /// The knob values `policy` pins, each defaulting to its spec.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let micros = |index: usize| match policy.knob(&KNOBS[index]) {
            KnobValue::DecimalMicros(value) => value,
            _ => 0,
        };
        let ceiling = match policy.knob(&KNOBS[4]) {
            KnobValue::Integer(value) => u32::try_from(value).unwrap_or(1).max(1),
            _ => 1,
        };
        Self {
            drift_alert_micros: micros(1),
            diversity_high_micros: micros(2),
            diversity_low_micros: micros(3),
            replica_ceiling: ceiling,
        }
    }
}

/// The samples one window may hold under `policy`.
pub fn max_samples(policy: &DecisionPolicy) -> usize {
    match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(value) => usize::try_from(value)
            .unwrap_or(MAX_DRIFT_SAMPLES)
            .clamp(16, MAX_DRIFT_SAMPLES),
        _ => MAX_DRIFT_SAMPLES,
    }
}

/// A window of scores, at most [`MAX_DRIFT_SAMPLES`] on the wire.
pub type DriftWindow = BoundedVec<QuantisedValue, MAX_DRIFT_SAMPLES>;

/// The first Wasserstein distance between two windows, on `Q32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Wasserstein1 {
    pub value: QuantisedValue,
}

/// One drift reading: the distance, the window sizes and whether it alerts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DriftReading {
    pub distance: Wasserstein1,
    pub reference_n: u32,
    pub current_n: u32,
    pub alert: bool,
}

/// The diversity of a replica set: the upper triangle of the pairwise
/// diversity matrix, each entry in `[0, 1]` on `Q32`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PopulationSpread {
    pub replicas: u32,
    pub pairwise: BoundedVec<QuantisedValue, MAX_PAIRWISE>,
}

impl PopulationSpread {
    /// The validating constructor: between 2 and [`MAX_SPREAD_REPLICAS`]
    /// replicas, one entry per replica pair, each on `Q32` inside `[0, 1]`.
    pub fn new(replicas: u32, pairwise: Vec<QuantisedValue>) -> Result<Self, String> {
        let count = usize::try_from(replicas).unwrap_or(usize::MAX);
        if !(2..=MAX_SPREAD_REPLICAS).contains(&count) {
            return Err(format!("replicas must lie in [2, {MAX_SPREAD_REPLICAS}]"));
        }
        if pairwise.len() != count * (count - 1) / 2 {
            return Err("pairwise must hold one entry per replica pair".to_string());
        }
        let unit = 1i64 << 32;
        if pairwise
            .iter()
            .any(|d| d.scale != QuantScaleTag::Q32 || !(0..=unit).contains(&d.value))
        {
            return Err("every pairwise diversity is on Q32 inside [0, 1]".to_string());
        }
        Ok(Self {
            replicas,
            pairwise: BoundedVec::new(pairwise)?,
        })
    }
}

/// The drift question: a reference window and a current window of scores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DriftRequest {
    pub reference: DriftWindow,
    pub current: DriftWindow,
}

/// The fan-out question: the replicas' diversity, the width now, and the
/// budget with the cost of one replica (both in the caller's cost unit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FanoutRequest {
    pub spread: PopulationSpread,
    pub current_width: u32,
    pub budget: u64,
    pub cost_per_replica: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q32(value: i64) -> QuantisedValue {
        QuantisedValue {
            scale: QuantScaleTag::Q32,
            value,
        }
    }

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: &T) {
        let text = serde_json::to_string(v).unwrap();
        assert_eq!(&serde_json::from_str::<T>(&text).unwrap(), v);
    }

    // spec: MEDULLA-DECISION-ENGINE-R752.750
    #[test]
    fn population_types_round_trip() {
        let distance = Wasserstein1 {
            value: q32(1 << 28),
        };
        round_trip(&distance);
        round_trip(&DriftReading {
            distance,
            reference_n: 100,
            current_n: 80,
            alert: true,
        });
        let spread = PopulationSpread::new(4, (0..6).map(|i| q32(i << 29)).collect()).unwrap();
        round_trip(&spread);
        for kind in [
            crate::decision::QuestionKind::PopulationDrift,
            crate::decision::QuestionKind::FanoutWidth,
        ] {
            round_trip(&kind);
        }
        assert_eq!(
            serde_json::to_string(&crate::decision::QuestionKind::FanoutWidth).unwrap(),
            "\"fanout_width\""
        );

        // The pair count and the unit interval are enforced.
        assert!(PopulationSpread::new(4, vec![q32(0); 5]).is_err());
        assert!(PopulationSpread::new(1, Vec::new()).is_err());
        assert!(PopulationSpread::new(17, vec![q32(0); 136]).is_err());
        assert!(PopulationSpread::new(2, vec![q32((1 << 32) + 1)]).is_err());
        assert!(PopulationSpread::new(
            2,
            vec![QuantisedValue {
                scale: QuantScaleTag::Pico,
                value: 0
            }]
        )
        .is_err());
        // A window holds 2048 samples and no more.
        assert!(DriftWindow::new(vec![q32(0); MAX_DRIFT_SAMPLES]).is_ok());
        assert!(DriftWindow::new(vec![q32(0); MAX_DRIFT_SAMPLES + 1]).is_err());
        // The knob defaults to the maximum and clamps inside its range.
        assert_eq!(
            max_samples(&DecisionPolicy::engine_default()),
            MAX_DRIFT_SAMPLES
        );
        assert!(KNOBS[0].admits(&KnobValue::Integer(16)));
        assert!(!KNOBS[0].admits(&KnobValue::Integer(2049)));
    }
}
