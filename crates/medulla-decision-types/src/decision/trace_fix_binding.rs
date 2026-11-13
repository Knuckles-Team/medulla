//! Correction binding model (MEDULLA-DECISION-ENGINE-R161.1).
//!
//! A failed run's trace, the fix generated for it and the regression test that
//! pins the fix are bound by id and digest. A revision receipt records whether
//! the test passed at a revision, so admission reads receipts, not claims.

use serde::{Deserialize, Serialize};

use crate::decision::record::outcome::{DecisionRefusal, RefusalCode};
use medulla_types::contract::{Digest256, OpaqueId};

/// Refusal: the trace names no failure point.
pub const BINDING_TRACE_ABSENT: &str = "BINDING_TRACE_ABSENT";
/// Refusal: no receipt names the regression test.
pub const BINDING_TEST_MISSING: &str = "BINDING_TEST_MISSING";
/// Refusal: the test has no receipt at one of its two revisions.
pub const BINDING_RECEIPT_UNEXECUTED: &str = "BINDING_RECEIPT_UNEXECUTED";
/// Refusal: the fix, the test or the revisions do not link.
pub const BINDING_CHAIN_BROKEN: &str = "BINDING_CHAIN_BROKEN";
/// Refusal: the before receipt is not `Failed` or the after receipt is not `Passed`.
pub const BINDING_OUTCOME_PATTERN_MISMATCH: &str = "BINDING_OUTCOME_PATTERN_MISMATCH";

/// The refusal codes the `correction_admission` family declares.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(BINDING_TRACE_ABSENT),
    RefusalCode::new(BINDING_TEST_MISSING),
    RefusalCode::new(BINDING_RECEIPT_UNEXECUTED),
    RefusalCode::new(BINDING_CHAIN_BROKEN),
    RefusalCode::new(BINDING_OUTCOME_PATTERN_MISMATCH),
];

/// Why a correction binding is not admitted, with the registered code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingRefusal {
    pub code: &'static str,
}

impl DecisionRefusal for BindingRefusal {
    fn code(&self) -> RefusalCode {
        RefusalCode::new(self.code)
    }

    fn detail(&self) -> String {
        match self.code {
            BINDING_TRACE_ABSENT => "the trace names no failure point",
            BINDING_TEST_MISSING => "no receipt names the regression test",
            BINDING_RECEIPT_UNEXECUTED => "the test has no receipt at one of its two revisions",
            BINDING_CHAIN_BROKEN => "the fix, the test or the revisions do not link",
            _ => "the before receipt is not failed or the after receipt is not passed",
        }
        .to_string()
    }
}

/// The most bytes `failure_point` may hold.
pub const MAX_FAILURE_POINT_BYTES: usize = 256;

/// A run that failed, and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct FailedRunTrace {
    pub trace_id: OpaqueId,
    pub run_id: OpaqueId,
    pub error_trace_digest: Digest256,
    pub failure_point: String,
}

impl FailedRunTrace {
    /// `failure_point` is at most 256 bytes.
    pub fn checked(self) -> Result<Self, String> {
        if self.failure_point.len() > MAX_FAILURE_POINT_BYTES {
            return Err(format!(
                "failure_point exceeds {MAX_FAILURE_POINT_BYTES} bytes"
            ));
        }
        Ok(self)
    }
}

/// A fix generated for one trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct GeneratedFix {
    pub fix_id: OpaqueId,
    pub trace_id: OpaqueId,
    pub fix_digest: Digest256,
}

/// A regression test of one fix, with the revisions it was run between.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RegressionTest {
    pub test_id: OpaqueId,
    pub fix_id: OpaqueId,
    pub test_digest: Digest256,
    pub before_revision: Digest256,
    pub after_revision: Digest256,
}

/// The outcome of one test run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RevisionOutcome {
    Passed,
    Failed,
}

/// A test run at one revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RevisionReceipt {
    pub test_id: OpaqueId,
    pub revision_id: Digest256,
    pub outcome: RevisionOutcome,
}

/// One correction: the trace, its fix, its test and the receipts for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CorrectionBinding {
    pub trace: FailedRunTrace,
    pub fix: GeneratedFix,
    pub test: RegressionTest,
    #[serde(default)]
    pub receipts: Vec<RevisionReceipt>,
}

impl CorrectionBinding {
    /// The fix names the trace and the test names the fix.
    pub fn is_bound(&self) -> bool {
        self.fix.trace_id == self.trace.trace_id && self.test.fix_id == self.fix.fix_id
    }
}

/// Check the receipt pattern of one correction. The fix names the trace, the
/// test names the fix, both receipts name the test, the before receipt is
/// `Failed` and the after receipt is `Passed`, at two different revisions that
/// equal the test's `before_revision` and `after_revision`. Medulla runs no test.
pub fn check_receipt_pattern(binding: &CorrectionBinding) -> Result<(), BindingRefusal> {
    let refuse = |code: &'static str| Err(BindingRefusal { code });
    if binding.trace.failure_point.is_empty() {
        return refuse(BINDING_TRACE_ABSENT);
    }
    if !binding.is_bound() || binding.test.before_revision == binding.test.after_revision {
        return refuse(BINDING_CHAIN_BROKEN);
    }
    let for_test: Vec<&RevisionReceipt> = binding
        .receipts
        .iter()
        .filter(|receipt| receipt.test_id == binding.test.test_id)
        .collect();
    if for_test.is_empty() {
        return refuse(BINDING_TEST_MISSING);
    }
    let at = |revision: &Digest256| for_test.iter().find(|r| &r.revision_id == revision);
    let (Some(before), Some(after)) = (
        at(&binding.test.before_revision),
        at(&binding.test.after_revision),
    ) else {
        return refuse(BINDING_RECEIPT_UNEXECUTED);
    };
    if before.outcome != RevisionOutcome::Failed || after.outcome != RevisionOutcome::Passed {
        return refuse(BINDING_OUTCOME_PATTERN_MISMATCH);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> OpaqueId {
        OpaqueId::new(value).unwrap()
    }

    fn binding() -> CorrectionBinding {
        CorrectionBinding {
            trace: FailedRunTrace {
                trace_id: id("trace:1"),
                run_id: id("run:1"),
                error_trace_digest: Digest256::sha256(b"trace"),
                failure_point: "step 3".into(),
            },
            fix: GeneratedFix {
                fix_id: id("fix:1"),
                trace_id: id("trace:1"),
                fix_digest: Digest256::sha256(b"fix"),
            },
            test: RegressionTest {
                test_id: id("test:1"),
                fix_id: id("fix:1"),
                test_digest: Digest256::sha256(b"test"),
                before_revision: Digest256::sha256(b"r0"),
                after_revision: Digest256::sha256(b"r1"),
            },
            receipts: vec![RevisionReceipt {
                test_id: id("test:1"),
                revision_id: Digest256::sha256(b"r1"),
                outcome: RevisionOutcome::Passed,
            }],
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R161.1
    #[test]
    fn binding_types_round_trip() {
        let value = binding();
        assert!(value.is_bound());
        let text = serde_json::to_string(&value).unwrap();
        assert!(text.contains("\"outcome\":\"passed\""));
        assert_eq!(
            serde_json::from_str::<CorrectionBinding>(&text).unwrap(),
            value
        );
        let mut unbound = binding();
        unbound.fix.trace_id = id("trace:other");
        assert!(!unbound.is_bound());
        let mut long = value.trace.clone();
        long.failure_point = "x".repeat(257);
        assert!(long.checked().is_err());
        let mut edge = value.trace.clone();
        edge.failure_point = "x".repeat(256);
        assert!(edge.checked().is_ok());
        assert!(serde_json::from_str::<RevisionReceipt>(
            r#"{"test_id":"t","revision_id":"zz","outcome":"passed"}"#
        )
        .is_err());
    }

    fn receipt(revision: &[u8], outcome: RevisionOutcome) -> RevisionReceipt {
        RevisionReceipt {
            test_id: id("test:1"),
            revision_id: Digest256::sha256(revision),
            outcome,
        }
    }

    fn admitted() -> CorrectionBinding {
        let mut value = binding();
        value.receipts = vec![
            receipt(b"r0", RevisionOutcome::Failed),
            receipt(b"r1", RevisionOutcome::Passed),
        ];
        value
    }

    // spec: MEDULLA-DECISION-ENGINE-R161.2
    #[test]
    fn receipt_binding_gates_admission() {
        assert_eq!(check_receipt_pattern(&admitted()), Ok(()));
        let code = |value: &CorrectionBinding| check_receipt_pattern(value).unwrap_err().code;

        let mut no_trace = admitted();
        no_trace.trace.failure_point.clear();
        assert_eq!(code(&no_trace), BINDING_TRACE_ABSENT);

        let mut fix_elsewhere = admitted();
        fix_elsewhere.fix.trace_id = id("trace:other");
        assert_eq!(code(&fix_elsewhere), BINDING_CHAIN_BROKEN);

        let mut test_elsewhere = admitted();
        test_elsewhere.test.fix_id = id("fix:other");
        assert_eq!(code(&test_elsewhere), BINDING_CHAIN_BROKEN);

        let mut same_revision = admitted();
        same_revision.test.after_revision = same_revision.test.before_revision;
        assert_eq!(code(&same_revision), BINDING_CHAIN_BROKEN);

        let mut no_receipts = admitted();
        no_receipts.receipts.clear();
        assert_eq!(code(&no_receipts), BINDING_TEST_MISSING);

        let mut other_test = admitted();
        for receipt in &mut other_test.receipts {
            receipt.test_id = id("test:other");
        }
        assert_eq!(code(&other_test), BINDING_TEST_MISSING);

        let mut unexecuted_after = admitted();
        unexecuted_after.receipts.pop();
        assert_eq!(code(&unexecuted_after), BINDING_RECEIPT_UNEXECUTED);

        let mut wrong_revision = admitted();
        wrong_revision.receipts[0].revision_id = Digest256::sha256(b"elsewhere");
        assert_eq!(code(&wrong_revision), BINDING_RECEIPT_UNEXECUTED);

        let mut before_passed = admitted();
        before_passed.receipts[0].outcome = RevisionOutcome::Passed;
        assert_eq!(code(&before_passed), BINDING_OUTCOME_PATTERN_MISMATCH);

        let mut after_failed = admitted();
        after_failed.receipts[1].outcome = RevisionOutcome::Failed;
        assert_eq!(code(&after_failed), BINDING_OUTCOME_PATTERN_MISMATCH);

        for refusal in REFUSALS {
            assert_eq!(
                crate::decision::codes::resolve(refusal.as_str()),
                Some(*refusal)
            );
        }
    }
}
