//! The routing-path success model (MEDULLA-DECISION-ENGINE-R159.1).
//!
//! A routing path is a connector, a method and a parameter digest. Its
//! identity is the SHA-256 of the three joined by a zero byte, so two decisions
//! about the same path name the same id.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::decision::knobs::{KnobSafety, KnobSpec, KnobValue};
use crate::decision::policy::DecisionPolicy;
use medulla_types::contract::Digest256;

/// The `path.*` knobs: `min_samples`, `window_ms`, `prefer_above_rate`, then
/// `skip_after_failures`.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "path.min_samples",
        unit: "attempts",
        default: KnobValue::Integer(10),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(10_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "path.window_ms",
        unit: "milliseconds",
        default: KnobValue::DurationMs(604_800_000),
        min: KnobValue::DurationMs(3_600_000),
        max: KnobValue::DurationMs(7_776_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "path.prefer_above_rate",
        unit: "ratio",
        default: KnobValue::DecimalMicros(500_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "path.skip_after_failures",
        unit: "attempts",
        default: KnobValue::Integer(3),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(20),
        safety: KnobSafety::Tunable,
    },
];

/// One routing path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RoutingPath {
    pub connector_id: String,
    pub method_id: String,
    pub parameter_digest: Digest256,
}

impl RoutingPath {
    /// `sha256(connector_id 0x00 method_id 0x00 parameter_digest)`, as 64
    /// lowercase hex characters.
    pub fn path_id(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.connector_id.as_bytes());
        hasher.update([0u8]);
        hasher.update(self.method_id.as_bytes());
        hasher.update([0u8]);
        hasher.update(self.parameter_digest.to_hex().as_bytes());
        hex::encode(hasher.finalize())
    }
}

/// Attempts on one path over a window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SuccessRateEvidence {
    pub path_id: String,
    pub successful_attempts: u32,
    pub total_attempts: u32,
    pub window_ms: u64,
}

impl SuccessRateEvidence {
    /// Successes never exceed attempts.
    pub fn checked(self) -> Result<Self, String> {
        if self.successful_attempts > self.total_attempts {
            return Err("successful_attempts exceeds total_attempts".into());
        }
        Ok(self)
    }
}

/// The knob values a preference decision read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PathPreferenceRule {
    pub window_ms: u64,
    pub min_samples: u32,
    pub prefer_above_rate: u32,
    pub skip_after_failures: u32,
}

impl PathPreferenceRule {
    /// The rule the policy pins, or the knob defaults.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let count = |spec: &KnobSpec| match policy.knob(spec) {
            KnobValue::Integer(value) => u32::try_from(value).unwrap_or(0),
            _ => 0,
        };
        Self {
            window_ms: match policy.knob(&KNOBS[1]) {
                KnobValue::DurationMs(value) => value,
                _ => 0,
            },
            min_samples: count(&KNOBS[0]),
            prefer_above_rate: match policy.knob(&KNOBS[2]) {
                KnobValue::DecimalMicros(value) => u32::try_from(value).unwrap_or(0),
                _ => 0,
            },
            skip_after_failures: count(&KNOBS[3]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(method: &str) -> RoutingPath {
        RoutingPath {
            connector_id: "conn:a".into(),
            method_id: method.into(),
            parameter_digest: Digest256::sha256(b"params"),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R159.1
    #[test]
    fn path_identity_round_trip() {
        let a = path("get");
        assert_eq!(a.path_id(), path("get").path_id(), "stable");
        assert_ne!(a.path_id(), path("put").path_id());
        assert_eq!(a.path_id().len(), 64);
        assert!(a
            .path_id()
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        // The zero byte keeps field boundaries unambiguous.
        let shifted = RoutingPath {
            connector_id: "conn:".into(),
            method_id: "aget".into(),
            parameter_digest: a.parameter_digest,
        };
        assert_ne!(a.path_id(), shifted.path_id());
        let text = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<RoutingPath>(&text).unwrap(), a);

        let evidence = SuccessRateEvidence {
            path_id: a.path_id(),
            successful_attempts: 8,
            total_attempts: 10,
            window_ms: 86_400_000,
        }
        .checked()
        .unwrap();
        let text = serde_json::to_string(&evidence).unwrap();
        assert_eq!(
            serde_json::from_str::<SuccessRateEvidence>(&text).unwrap(),
            evidence
        );
        assert!(SuccessRateEvidence {
            successful_attempts: 11,
            ..evidence
        }
        .checked()
        .is_err());
        let rule = PathPreferenceRule {
            window_ms: 86_400_000,
            min_samples: 5,
            prefer_above_rate: 700_000,
            skip_after_failures: 3,
        };
        let text = serde_json::to_string(&rule).unwrap();
        assert_eq!(
            serde_json::from_str::<PathPreferenceRule>(&text).unwrap(),
            rule
        );
    }
}
