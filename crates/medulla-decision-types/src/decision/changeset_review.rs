//! Changeset review findings (MEDULLA-DECISION-ENGINE-R138.101.1).
//!
//! A semantic review of a graph changeset reports findings in five flaw
//! classes, each bound to the code it cites. The impact filter drops only
//! low-impact findings the reviewer itself doubts; the filter lives in
//! `medulla-decision-stats`.

use serde::{Deserialize, Serialize};

/// The five flaw classes a changeset review distinguishes.
#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ReviewClass {
    Injection,
    AuthBypass,
    DataExposure,
    CryptoFlaw,
    BusinessLogic,
}

/// Ordered impact of a finding.
#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// The integer a knob stores for this severity: `low` 0 to `critical` 3.
    pub fn rank(self) -> i64 {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::Critical => 3,
        }
    }

    /// The severity a knob integer names.
    pub fn from_rank(rank: i64) -> Option<Self> {
        match rank {
            0 => Some(Self::Low),
            1 => Some(Self::Medium),
            2 => Some(Self::High),
            3 => Some(Self::Critical),
            _ => None,
        }
    }
}

/// One finding, bound to the code lines it cites.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReviewFinding {
    pub class: ReviewClass,
    pub path: String,
    pub line_start: u32,
    pub line_end: u32,
    /// What the reviewer observed at the cited lines.
    pub evidence: String,
    pub severity: Severity,
    /// The reviewer's confidence in the finding, per mille.
    pub confidence_permille: u16,
    /// Why the finding may be a false positive, when the reviewer says so.
    #[serde(default)]
    pub false_positive_evidence: Option<String>,
}

/// What the impact filter did with one finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum FindingDisposition {
    Kept,
    FilteredLowImpact,
}

impl ReviewFinding {
    /// The finding names its evidence, a path and ordered positive lines, and
    /// a confidence within one thousand per mille.
    pub fn validate(&self) -> Result<(), String> {
        if self.evidence.trim().is_empty() {
            return Err("a finding names its evidence".to_string());
        }
        if self.path.trim().is_empty() {
            return Err("a finding names its path".to_string());
        }
        if self.line_start == 0 || self.line_end < self.line_start {
            return Err("finding lines must be positive and ordered".to_string());
        }
        if self.confidence_permille > 1000 {
            return Err("confidence is at most 1000 per mille".to_string());
        }
        Ok(())
    }
}
