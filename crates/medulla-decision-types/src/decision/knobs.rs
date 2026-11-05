//! Evolution-tuned decision knobs (MEDULLA-DECISION-ENGINE-R179.1, R179.4).
//!
//! A knob is one named, bounded, unit-carrying value a question family reads
//! through [`super::DecisionPolicy::knob`]. Each family declares its knobs in
//! one `pub const KNOBS: &[KnobSpec]` table. Knobs of security, policy,
//! write-back and irreversible-action questions are [`KnobSafety::Fixed`]: a
//! knob-set revision may never move them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The name of one knob, `<family>.<knob>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct KnobName(pub String);

impl KnobName {
    pub fn new(name: &str) -> Self {
        Self(name.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One knob value: an integer, a fixed-point decimal in millionths, or a
/// duration in milliseconds. Values of different variants never compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum KnobValue {
    Integer(i64),
    /// Fixed-point decimal: the value times 1,000,000.
    DecimalMicros(i64),
    DurationMs(u64),
}

impl KnobValue {
    /// The ordering of two values of the same variant; `None` across variants.
    pub fn compare(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Self::Integer(a), Self::Integer(b)) => Some(a.cmp(b)),
            (Self::DecimalMicros(a), Self::DecimalMicros(b)) => Some(a.cmp(b)),
            (Self::DurationMs(a), Self::DurationMs(b)) => Some(a.cmp(b)),
            _ => None,
        }
    }

    /// The premise rendering of the value.
    pub fn render(&self) -> String {
        match self {
            Self::Integer(v) => v.to_string(),
            Self::DecimalMicros(v) => {
                let sign = if *v < 0 { "-" } else { "" };
                let abs = v.unsigned_abs();
                format!("{sign}{}.{:06}", abs / 1_000_000, abs % 1_000_000)
            }
            Self::DurationMs(v) => format!("{v}ms"),
        }
    }
}

/// Whether a knob-set revision may move a knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum KnobSafety {
    Tunable,
    Fixed,
}

/// One knob's declaration: its default and the closed range `[min, max]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KnobSpec {
    pub name: &'static str,
    pub unit: &'static str,
    pub default: KnobValue,
    pub min: KnobValue,
    pub max: KnobValue,
    pub safety: KnobSafety,
}

impl KnobSpec {
    pub fn knob_name(&self) -> KnobName {
        KnobName::new(self.name)
    }

    /// Whether `value` is of the spec's variant and inside `[min, max]`.
    pub fn admits(&self, value: &KnobValue) -> bool {
        matches!(
            self.min.compare(value),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        ) && matches!(
            value.compare(&self.max),
            Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
        )
    }
}

/// Why a knob-set revision is refused (R179.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum KnobRefusalCode {
    KnobOutOfBounds,
    KnobNotTunable,
    KnobUnknown,
}

impl KnobRefusalCode {
    pub const fn code(self) -> &'static str {
        match self {
            Self::KnobOutOfBounds => "KNOB_OUT_OF_BOUNDS",
            Self::KnobNotTunable => "KNOB_NOT_TUNABLE",
            Self::KnobUnknown => "KNOB_UNKNOWN",
        }
    }
}

impl std::fmt::Display for KnobRefusalCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

/// The spec named `name` among the family tables of `registry`.
pub fn find_spec<'a>(registry: &[&'a [KnobSpec]], name: &KnobName) -> Option<&'a KnobSpec> {
    registry
        .iter()
        .flat_map(|family| family.iter())
        .find(|spec| spec.name == name.as_str())
}

/// Check one knob-set revision against the family tables of `registry`:
/// every named knob must be known, `Tunable` and inside its bounds. The
/// first refused knob, in name order, is returned with its code.
pub fn check_revision(
    registry: &[&[KnobSpec]],
    revision: &BTreeMap<KnobName, KnobValue>,
) -> Result<(), (KnobName, KnobRefusalCode)> {
    for (name, value) in revision {
        let spec = find_spec(registry, name).ok_or((name.clone(), KnobRefusalCode::KnobUnknown))?;
        if spec.safety == KnobSafety::Fixed {
            return Err((name.clone(), KnobRefusalCode::KnobNotTunable));
        }
        if !spec.admits(value) {
            return Err((name.clone(), KnobRefusalCode::KnobOutOfBounds));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[KnobSpec] = &[
        KnobSpec {
            name: "sample.batch",
            unit: "turns",
            default: KnobValue::Integer(8),
            min: KnobValue::Integer(1),
            max: KnobValue::Integer(64),
            safety: KnobSafety::Tunable,
        },
        KnobSpec {
            name: "sample.epsilon",
            unit: "fraction",
            default: KnobValue::DecimalMicros(20_000),
            min: KnobValue::DecimalMicros(0),
            max: KnobValue::DecimalMicros(200_000),
            safety: KnobSafety::Tunable,
        },
        KnobSpec {
            name: "sample.debounce",
            unit: "ms",
            default: KnobValue::DurationMs(30_000),
            min: KnobValue::DurationMs(1_000),
            max: KnobValue::DurationMs(600_000),
            safety: KnobSafety::Fixed,
        },
    ];

    // spec: MEDULLA-DECISION-ENGINE-R179.1
    #[test]
    fn knob_specs_round_trip_and_defaults_lie_within_bounds() {
        for spec in SAMPLE {
            assert!(
                spec.admits(&spec.default),
                "{} default out of bounds",
                spec.name
            );
            for value in [spec.default, spec.min, spec.max] {
                let json = serde_json::to_string(&value).unwrap();
                assert_eq!(serde_json::from_str::<KnobValue>(&json).unwrap(), value);
            }
            let name = spec.knob_name();
            let json = serde_json::to_string(&name).unwrap();
            assert_eq!(json, format!("\"{}\"", spec.name));
            assert_eq!(serde_json::from_str::<KnobName>(&json).unwrap(), name);
            assert_eq!(find_spec(&[SAMPLE], &name), Some(spec));
        }
        assert!(!SAMPLE[0].admits(&KnobValue::DurationMs(8)));
        assert_eq!(KnobValue::DecimalMicros(20_000).render(), "0.020000");
    }
}
