//! Changeset review findings (MEDULLA-DECISION-ENGINE-R138.101.1).
//!
//! A semantic review of a graph changeset reports findings in five flaw
//! classes, each bound to the code it cites. The impact filter drops only
//! low-impact findings the reviewer itself doubts; the filter lives in
//! `medulla-decision-stats`.

use serde::{Deserialize, Serialize};

use super::record::outcome::RefusalCode;

/// Refusal code: the question names no base or no head revision.
pub const REVIEW_REVISION_MISSING: RefusalCode = RefusalCode::new("REVIEW_REVISION_MISSING");
/// Refusal code: a named or cited flaw class is not one of the five.
pub const REVIEW_CLASS_UNSUPPORTED: RefusalCode = RefusalCode::new("REVIEW_CLASS_UNSUPPORTED");
/// Refusal code: a finding cites a class the review did not claim to cover.
pub const REVIEW_CLASS_UNREVIEWED: RefusalCode = RefusalCode::new("REVIEW_CLASS_UNREVIEWED");
/// Refusal code: a finding carries no evidence, path or cited lines.
pub const REVIEW_PROOF_ABSENT: RefusalCode = RefusalCode::new("REVIEW_PROOF_ABSENT");
/// Refusal code: the changed-files digest is not the digest of the changed paths.
pub const REVIEW_CHANGESET_DIGEST_MISMATCH: RefusalCode =
    RefusalCode::new("REVIEW_CHANGESET_DIGEST_MISMATCH");
/// Refusal code: a finding cites a path the changeset does not contain.
pub const REVIEW_FINDING_OUTSIDE_CHANGESET: RefusalCode =
    RefusalCode::new("REVIEW_FINDING_OUTSIDE_CHANGESET");

/// The `REFUSALS` table of the `changeset_review` family, registered in
/// `codes::FAMILY_TABLES`.
pub const REFUSALS: &[RefusalCode] = &[
    REVIEW_REVISION_MISSING,
    REVIEW_CLASS_UNSUPPORTED,
    REVIEW_CLASS_UNREVIEWED,
    REVIEW_PROOF_ABSENT,
    REVIEW_CHANGESET_DIGEST_MISMATCH,
    REVIEW_FINDING_OUTSIDE_CHANGESET,
];

/// The classification prefix of a finding's flaw class.
pub const CLASS_PREFIX: &str = "review:class:";
/// The classification prefix of a finding's severity.
pub const SEVERITY_PREFIX: &str = "review:severity:";

/// The digest of a changed-path set: SHA-256 over the sorted, de-duplicated
/// paths joined by a newline, as 64 lowercase hex characters.
pub fn changed_files_digest<'a>(paths: impl IntoIterator<Item = &'a str>) -> String {
    let mut sorted: Vec<&str> = paths.into_iter().collect();
    sorted.sort_unstable();
    sorted.dedup();
    medulla_types::retrieval::sha256_hex(sorted.join("\n").as_bytes())
}

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

impl ReviewClass {
    /// Every class, in declaration order.
    pub const ALL: [ReviewClass; 5] = [
        Self::Injection,
        Self::AuthBypass,
        Self::DataExposure,
        Self::CryptoFlaw,
        Self::BusinessLogic,
    ];

    /// The wire name of the class.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Injection => "injection",
            Self::AuthBypass => "auth_bypass",
            Self::DataExposure => "data_exposure",
            Self::CryptoFlaw => "crypto_flaw",
            Self::BusinessLogic => "business_logic",
        }
    }

    /// The class a wire name denotes.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|class| class.as_str() == name)
    }
}

impl Severity {
    /// The wire name of the severity.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    /// The severity a wire name denotes.
    pub fn parse(name: &str) -> Option<Self> {
        (0..=3)
            .filter_map(Self::from_rank)
            .find(|severity| severity.as_str() == name)
    }

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
