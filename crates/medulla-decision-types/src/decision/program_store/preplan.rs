//! Pre-plan test receipt: the evidence that gates pattern promotion
//! (MEDULLA-DECISION-ENGINE-R135.7).
//!
//! The harness runs the tests before it plans, stores the artifact manifest in
//! Blob CAS and sends the receipt with the run record. Medulla verifies it
//! against the plan binding and the manifest reading and only then marks the
//! program a reusable pattern. Nothing here reads a clock or a store: the
//! caller supplies the manifest reading.

use super::super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::super::policy::DecisionPolicy;
use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Refusal: a reusable commit with no receipt (or a receipt of another tenant).
pub const PREPLAN_RECEIPT_MISSING: RefusalCode = RefusalCode::new("PREPLAN_RECEIPT_MISSING");
/// Refusal: the receipt's command or repository revision is not the plan's.
pub const PREPLAN_COMMAND_MISMATCH: RefusalCode = RefusalCode::new("PREPLAN_COMMAND_MISMATCH");
/// Refusal: the receipt completed after the plan, or is older than allowed.
pub const PREPLAN_RECEIPT_STALE: RefusalCode = RefusalCode::new("PREPLAN_RECEIPT_STALE");
/// Refusal: the manifest is not readable in the tenant's Blob CAS.
pub const PREPLAN_ARTIFACTS_UNREADABLE: RefusalCode =
    RefusalCode::new("PREPLAN_ARTIFACTS_UNREADABLE");
/// Refusal: the receipt names no artifacts, or the manifest is empty.
pub const PREPLAN_ARTIFACTS_EMPTY: RefusalCode = RefusalCode::new("PREPLAN_ARTIFACTS_EMPTY");
/// Refusal: the tests did not exit zero.
pub const PREPLAN_TESTS_FAILED: RefusalCode = RefusalCode::new("PREPLAN_TESTS_FAILED");

/// Every refusal code of pattern promotion.
pub const REFUSALS: &[RefusalCode] = &[
    PREPLAN_RECEIPT_MISSING,
    PREPLAN_COMMAND_MISMATCH,
    PREPLAN_RECEIPT_STALE,
    PREPLAN_ARTIFACTS_UNREADABLE,
    PREPLAN_ARTIFACTS_EMPTY,
    PREPLAN_TESTS_FAILED,
];

/// Longest age a receipt may have at plan time, in milliseconds. A `Fixed`
/// safety bound for the knob below.
pub const MAX_RECEIPT_AGE_CEILING_MS: u64 = 7 * 24 * 3_600_000;

/// The `program.preplan_max_age_ms` decision-policy knob: how long before the
/// plan the tests may have completed (DECISIONS DE-R059.1-KNOBS).
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "program.preplan_max_age_ms",
    unit: "ms",
    default: KnobValue::DurationMs(24 * 3_600_000),
    min: KnobValue::DurationMs(1),
    max: KnobValue::DurationMs(MAX_RECEIPT_AGE_CEILING_MS),
    safety: KnobSafety::Tunable,
}];

/// The receipt of one pre-plan test run, produced by the harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PrePlanTestReceipt {
    pub tenant_id: String,
    pub run_id: String,
    pub repository_id: String,
    pub repository_revision: String,
    pub command_id: String,
    /// SHA-256, 64 lowercase hex characters.
    pub command_digest: String,
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
    pub exit_code: i32,
    /// The Blob CAS manifest digest of the artifacts the tests produced.
    pub artifact_manifest_digest: String,
    pub artifact_count: u32,
    /// SHA-256, 64 lowercase hex characters.
    pub result_digest: String,
}

/// What the plan the program was compiled under binds the receipt to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PlanBinding {
    pub repository_revision: String,
    pub command_digest: String,
    pub planned_at_ms: u64,
}

/// What reading the manifest in the caller's tenant of Blob CAS found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestReading {
    /// Absent, owned by another tenant or failing its chunk checks.
    Unreadable,
    /// Readable and zero bytes long.
    Empty,
    /// Readable and non-empty.
    Readable,
}

/// The two booleans of a verified receipt, with the evidence they rest on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PrePlanVerdict {
    pub tests_ran: bool,
    pub artifacts_exist: bool,
    /// References to the evidence: the receipt digest, the result digest and
    /// the manifest digest.
    pub evidence_refs: Vec<String>,
    pub policy_digest: String,
}

/// The promoted pattern, stored beside (never inside) the compiled program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ReusablePattern {
    pub task_signature: String,
    pub program_digest: String,
    pub source_run_id: String,
    pub plan: PlanBinding,
    pub receipt: PrePlanTestReceipt,
    pub verdict: PrePlanVerdict,
}

/// The decision-artifact key of the pattern promoted for `task_signature`.
pub fn pattern_key(task_signature: &str) -> String {
    format!("run_record/reusable_pattern/{task_signature}")
}

/// Why a receipt blocks promotion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrePlanError {
    Missing,
    CommandMismatch,
    Stale,
    ArtifactsUnreadable,
    ArtifactsEmpty,
    TestsFailed,
}

impl PrePlanError {
    /// The registered refusal code.
    pub fn code(&self) -> RefusalCode {
        match self {
            Self::Missing => PREPLAN_RECEIPT_MISSING,
            Self::CommandMismatch => PREPLAN_COMMAND_MISMATCH,
            Self::Stale => PREPLAN_RECEIPT_STALE,
            Self::ArtifactsUnreadable => PREPLAN_ARTIFACTS_UNREADABLE,
            Self::ArtifactsEmpty => PREPLAN_ARTIFACTS_EMPTY,
            Self::TestsFailed => PREPLAN_TESTS_FAILED,
        }
    }
}

impl std::fmt::Display for PrePlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match self {
            Self::Missing => "a reusable pattern needs a pre-plan test receipt of this tenant",
            Self::CommandMismatch => {
                "the receipt's command or repository revision is not the plan's"
            }
            Self::Stale => "the receipt must complete before the plan, inside the allowed age",
            Self::ArtifactsUnreadable => "the artifact manifest is not readable in Blob CAS",
            Self::ArtifactsEmpty => "the receipt names no artifacts or the manifest is empty",
            Self::TestsFailed => "the pre-plan tests did not exit zero",
        };
        write!(f, "{}: {why}", self.code().as_str())
    }
}

impl std::error::Error for PrePlanError {}

/// The resolved value of `program.preplan_max_age_ms`.
pub fn max_receipt_age_ms(policy: &DecisionPolicy) -> u64 {
    let raw = match policy.knob(&KNOBS[0]) {
        KnobValue::DurationMs(v) => v,
        KnobValue::Integer(v) | KnobValue::DecimalMicros(v) => u64::try_from(v).unwrap_or(1),
    };
    raw.clamp(1, MAX_RECEIPT_AGE_CEILING_MS)
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl PrePlanTestReceipt {
    /// SHA-256 over the receipt's compact JSON, lowercase hex.
    pub fn digest(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("a receipt is plain data");
        hex::encode(Sha256::digest(bytes))
    }
}

/// Verify a receipt against the plan binding and the manifest reading.
///
/// `tests_ran` holds only when the command digest and repository revision
/// equal the binding and the tests completed before the plan, no earlier than
/// `max_age_ms` before it. `artifacts_exist` holds only when the artifact
/// count is positive and the manifest reads non-empty. A promotion needs a
/// zero exit code and both booleans. The first failing check names the
/// refusal, in the row's order.
pub fn verify_preplan(
    tenant_id: &str,
    receipt: Option<&PrePlanTestReceipt>,
    binding: &PlanBinding,
    manifest: ManifestReading,
    max_age_ms: u64,
    policy_digest: &str,
) -> Result<PrePlanVerdict, PrePlanError> {
    let receipt = receipt
        .filter(|r| r.tenant_id == tenant_id)
        .ok_or(PrePlanError::Missing)?;
    if !is_sha256_hex(&receipt.command_digest)
        || receipt.command_digest != binding.command_digest
        || receipt.repository_revision != binding.repository_revision
    {
        return Err(PrePlanError::CommandMismatch);
    }
    let in_time = receipt.started_at_ms <= receipt.completed_at_ms
        && receipt.completed_at_ms < binding.planned_at_ms
        && binding.planned_at_ms - receipt.completed_at_ms <= max_age_ms;
    if !in_time {
        return Err(PrePlanError::Stale);
    }
    if receipt.exit_code != 0 || !is_sha256_hex(&receipt.result_digest) {
        return Err(PrePlanError::TestsFailed);
    }
    if receipt.artifact_count == 0 || manifest == ManifestReading::Empty {
        return Err(PrePlanError::ArtifactsEmpty);
    }
    if manifest == ManifestReading::Unreadable || !is_sha256_hex(&receipt.artifact_manifest_digest)
    {
        return Err(PrePlanError::ArtifactsUnreadable);
    }
    Ok(PrePlanVerdict {
        tests_ran: true,
        artifacts_exist: true,
        evidence_refs: vec![
            format!("receipt:{}", receipt.digest()),
            format!("result:{}", receipt.result_digest),
            format!("manifest:{}", receipt.artifact_manifest_digest),
        ],
        policy_digest: policy_digest.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex64(c: char) -> String {
        std::iter::repeat(c).take(64).collect()
    }

    fn binding() -> PlanBinding {
        PlanBinding {
            repository_revision: "rev-1".into(),
            command_digest: hex64('a'),
            planned_at_ms: 10_000,
        }
    }

    fn receipt() -> PrePlanTestReceipt {
        PrePlanTestReceipt {
            tenant_id: "t".into(),
            run_id: "run-1".into(),
            repository_id: "repo".into(),
            repository_revision: "rev-1".into(),
            command_id: "cargo-test".into(),
            command_digest: hex64('a'),
            started_at_ms: 1_000,
            completed_at_ms: 2_000,
            exit_code: 0,
            artifact_manifest_digest: hex64('b'),
            artifact_count: 2,
            result_digest: hex64('c'),
        }
    }

    fn verify(
        r: Option<&PrePlanTestReceipt>,
        m: ManifestReading,
    ) -> Result<PrePlanVerdict, PrePlanError> {
        verify_preplan("t", r, &binding(), m, 1_000_000, "pd")
    }

    // spec: MEDULLA-DECISION-ENGINE-R135.7
    #[test]
    fn verifier_returns_both_booleans_with_evidence_and_policy() {
        let verdict = verify(Some(&receipt()), ManifestReading::Readable).unwrap();
        assert!(verdict.tests_ran && verdict.artifacts_exist);
        assert_eq!(verdict.evidence_refs.len(), 3);
        assert_eq!(verdict.policy_digest, "pd");
    }

    // spec: MEDULLA-DECISION-ENGINE-R135.7
    #[test]
    fn every_blocking_case_names_its_code() {
        let ok = receipt();
        let case = |edit: &dyn Fn(&mut PrePlanTestReceipt), m: ManifestReading| {
            let mut r = ok.clone();
            edit(&mut r);
            verify(Some(&r), m).unwrap_err()
        };
        let readable = ManifestReading::Readable;
        assert_eq!(verify(None, readable).unwrap_err(), PrePlanError::Missing);
        assert_eq!(
            case(&|r| r.tenant_id = "other".into(), readable),
            PrePlanError::Missing
        );
        assert_eq!(
            case(&|r| r.command_digest = hex64('d'), readable),
            PrePlanError::CommandMismatch
        );
        assert_eq!(
            case(&|r| r.repository_revision = "rev-2".into(), readable),
            PrePlanError::CommandMismatch
        );
        assert_eq!(
            case(&|r| r.completed_at_ms = 10_000, readable),
            PrePlanError::Stale
        );
        assert_eq!(
            case(&|r| r.completed_at_ms = 11_000, readable),
            PrePlanError::Stale
        );
        assert_eq!(
            case(&|r| r.exit_code = 1, readable),
            PrePlanError::TestsFailed
        );
        assert_eq!(
            case(&|r| r.artifact_count = 0, readable),
            PrePlanError::ArtifactsEmpty
        );
        assert_eq!(
            case(&|_| {}, ManifestReading::Empty),
            PrePlanError::ArtifactsEmpty
        );
        assert_eq!(
            case(&|_| {}, ManifestReading::Unreadable),
            PrePlanError::ArtifactsUnreadable
        );
        // Evidence older than the allowed age is stale too.
        let aged = verify_preplan("t", Some(&ok), &binding(), readable, 5_000, "pd");
        assert_eq!(aged.unwrap_err(), PrePlanError::Stale);
        for error in [PrePlanError::Missing, PrePlanError::TestsFailed] {
            assert!(error.to_string().starts_with(error.code().as_str()));
            assert!(REFUSALS.contains(&error.code()));
        }
    }
}
