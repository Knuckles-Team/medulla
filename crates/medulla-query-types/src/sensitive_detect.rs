//! Typed records for sensitive-data detectors and their scan results.
//!
//! Scanning and claim promotion live above this pure-data crate. The detector
//! method is closed so an unknown algorithm cannot silently enter the contract.

use serde::{Deserialize, Serialize};

/// Supported ways to identify sensitive values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SensitiveDetectionMethod {
    Pattern,
    Checksum,
    Dictionary,
    Model,
}

/// The checksum a `Checksum` rule verifies (MEDULLA-TYPED-PACKS-R803.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ChecksumAlgorithm {
    /// The Luhn sum of a payment card number.
    Luhn,
    /// The ISO 7064 mod 97-10 check of an IBAN.
    IbanMod97,
}

/// What a detector matches a value against (MEDULLA-TYPED-PACKS-R803.1). The rule kind
/// is the detection method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum DetectorRule {
    /// The whole trimmed value matches the regular expression.
    Pattern { regex: String },
    /// The value passes the checksum.
    Checksum { algorithm: ChecksumAlgorithm },
    /// The value is in the stored term set named by its digest.
    Dictionary { terms_digest: String },
    /// The value is close to an exemplar of the named stored set.
    Model { exemplar_set: String },
}

impl DetectorRule {
    /// The detection method this rule implements.
    pub const fn method(&self) -> SensitiveDetectionMethod {
        match self {
            Self::Pattern { .. } => SensitiveDetectionMethod::Pattern,
            Self::Checksum { .. } => SensitiveDetectionMethod::Checksum,
            Self::Dictionary { .. } => SensitiveDetectionMethod::Dictionary,
            Self::Model { .. } => SensitiveDetectionMethod::Model,
        }
    }
}

/// A versionable detector configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SensitiveDetector {
    pub id: String,
    pub category: String,
    pub method: SensitiveDetectionMethod,
    pub rule: DetectorRule,
    pub threshold: f64,
}

/// The refusal code of a threshold outside `[0.0, 1.0]`.
pub const DETECTOR_THRESHOLD_INVALID: &str = "DETECTOR_THRESHOLD_INVALID";
/// The refusal code of a rule that is not the detector's method.
pub const DETECTOR_RULE_MISMATCH: &str = "DETECTOR_RULE_MISMATCH";
/// The refusal code of a pattern that is empty or does not compile.
pub const DETECTOR_PATTERN_INVALID: &str = "DETECTOR_PATTERN_INVALID";

/// Why a detector configuration was refused (MEDULLA-TYPED-PACKS-R803.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidDetector {
    /// The threshold is not finite or lies outside `[0.0, 1.0]`.
    ThresholdInvalid { detector: String },
    /// The rule is not the rule of the detector's method.
    RuleMismatch { detector: String },
    /// A `Pattern` rule with an empty or uncompilable regex.
    PatternInvalid { detector: String, reason: String },
}

impl InvalidDetector {
    /// The stable refusal code.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ThresholdInvalid { .. } => DETECTOR_THRESHOLD_INVALID,
            Self::RuleMismatch { .. } => DETECTOR_RULE_MISMATCH,
            Self::PatternInvalid { .. } => DETECTOR_PATTERN_INVALID,
        }
    }

    fn detector(&self) -> &str {
        match self {
            Self::ThresholdInvalid { detector }
            | Self::RuleMismatch { detector }
            | Self::PatternInvalid { detector, .. } => detector,
        }
    }
}

impl std::fmt::Display for InvalidDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.detector())
    }
}

impl std::error::Error for InvalidDetector {}

impl SensitiveDetector {
    /// Check the threshold, that the rule is the method's rule and that a pattern is not
    /// empty. Compiling the regex belongs to the scan, which owns the regex dependency.
    pub fn validate(&self) -> Result<(), InvalidDetector> {
        let detector = || self.id.clone();
        if !self.threshold.is_finite() || !(0.0..=1.0).contains(&self.threshold) {
            return Err(InvalidDetector::ThresholdInvalid {
                detector: detector(),
            });
        }
        if self.rule.method() != self.method {
            return Err(InvalidDetector::RuleMismatch {
                detector: detector(),
            });
        }
        if let DetectorRule::Pattern { regex } = &self.rule {
            if regex.is_empty() {
                return Err(InvalidDetector::PatternInvalid {
                    detector: detector(),
                    reason: "the pattern is empty".to_string(),
                });
            }
        }
        Ok(())
    }
}

/// The threshold of a built-in detector.
pub const DEFAULT_THRESHOLD: f64 = 0.8;

/// The built-in detectors, in scan order.
pub fn builtin_detectors() -> Vec<SensitiveDetector> {
    let detector = |id: &str, category: &str, rule: DetectorRule| SensitiveDetector {
        id: format!("detector:{id}"),
        category: category.to_string(),
        method: rule.method(),
        rule,
        threshold: DEFAULT_THRESHOLD,
    };
    vec![
        detector(
            "card_number",
            "payment_card",
            DetectorRule::Checksum {
                algorithm: ChecksumAlgorithm::Luhn,
            },
        ),
        detector(
            "email_address",
            "contact_email",
            DetectorRule::Pattern {
                regex: "^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$".to_string(),
            },
        ),
        detector(
            "iban",
            "bank_account",
            DetectorRule::Checksum {
                algorithm: ChecksumAlgorithm::IbanMod97,
            },
        ),
        detector(
            "ipv4_address",
            "network_address",
            DetectorRule::Pattern {
                regex: "^((25[0-5]|2[0-4][0-9]|1?[0-9]?[0-9])\\.){3}(25[0-5]|2[0-4][0-9]|1?[0-9]?[0-9])$"
                    .to_string(),
            },
        ),
    ]
}

/// Aggregate evidence produced by applying a detector to one asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Detection {
    pub asset: String,
    pub detector: String,
    pub sample_hits: u64,
    pub ratio: f64,
    /// The values scanned: the sample size.
    pub sample_size: u64,
    /// When the scan ran, in milliseconds since the epoch.
    pub scanned_at_ms: i64,
}
