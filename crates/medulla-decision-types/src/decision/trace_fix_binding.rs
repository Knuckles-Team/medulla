//! Correction binding model (MEDULLA-DECISION-ENGINE-R161.1).
//!
//! A failed run's trace, the fix generated for it and the regression test that
//! pins the fix are bound by id and digest. A revision receipt records whether
//! the test passed at a revision, so admission reads receipts, not claims.

use serde::{Deserialize, Serialize};

use medulla_types::contract::{Digest256, OpaqueId};

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
}
