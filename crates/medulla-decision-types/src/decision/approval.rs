//! The approval receipt checked before a recommendation commits
//! (MEDULLA-DECISION-ENGINE-R019.100.2).
//!
//! A policy that sets `approval_required` commits a record only when the
//! request carries a [`RecommendationApproval`] naming that record's id and
//! digest, issued within `commit.approval_max_age`. Arbora issues the receipt
//! and checks the reviewer's scope; medulla checks it against the record.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;

/// A reviewer's approval of one recommendation record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RecommendationApproval {
    /// The record the reviewer approved.
    pub record_id: String,
    /// The digest of the record the reviewer saw.
    pub record_digest: String,
    /// The principal that approved.
    pub reviewer_principal: String,
    /// Unix milliseconds of the approval.
    pub approved_at_ms: u64,
    /// The revision of the approval, for the reviewer's own bookkeeping.
    pub approval_revision: u64,
}

/// Index of `commit.approval_max_age` in [`KNOBS`].
pub const APPROVAL_MAX_AGE: usize = 0;

/// Every `commit.*` approval knob. The age bound is `Fixed`: a knob-set
/// revision never loosens it; only a pinned policy sets it.
pub const KNOBS: &[KnobSpec] = &[KnobSpec {
    name: "commit.approval_max_age",
    unit: "s",
    default: KnobValue::Integer(86_400),
    min: KnobValue::Integer(60),
    max: KnobValue::Integer(604_800),
    safety: KnobSafety::Fixed,
}];

/// The oldest an approval may be, in seconds: the knob the pinned policy sets,
/// or its default.
pub fn approval_max_age_s(policy: &DecisionPolicy) -> u64 {
    match policy.knob(&KNOBS[APPROVAL_MAX_AGE]) {
        KnobValue::Integer(seconds) => seconds.clamp(60, 604_800) as u64,
        _ => 86_400,
    }
}

/// Why an approval does not admit a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalRefusal {
    /// The policy requires an approval and the request carries none.
    Missing,
    /// The approval names another record id or digest.
    DigestMismatch,
    /// The approval is dated after the clock or older than the age bound.
    Stale,
}

impl ApprovalRefusal {
    /// The wire code of the refusal.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Missing => "APPROVAL_MISSING",
            Self::DigestMismatch => "APPROVAL_DIGEST_MISMATCH",
            Self::Stale => "APPROVAL_STALE",
        }
    }

    /// The refusal as `<CODE>: <detail>`.
    pub fn message(self) -> String {
        let detail = match self {
            Self::Missing => "the pinned policy requires an approval receipt for this commit",
            Self::DigestMismatch => "the approval names another record id or digest",
            Self::Stale => "the approval is dated after the clock or older than commit.approval_max_age",
        };
        format!("{}: {detail}", self.code())
    }
}

/// Check `approval` against the record it must cover, at `now_ms`.
pub fn check_approval(
    approval: Option<&RecommendationApproval>,
    record_id: &str,
    record_digest: &str,
    now_ms: u64,
    max_age_s: u64,
) -> Result<(), ApprovalRefusal> {
    let approval = approval.ok_or(ApprovalRefusal::Missing)?;
    if approval.record_id != record_id || approval.record_digest != record_digest {
        return Err(ApprovalRefusal::DigestMismatch);
    }
    if approval.approved_at_ms > now_ms
        || now_ms - approval.approved_at_ms > max_age_s.saturating_mul(1000)
    {
        return Err(ApprovalRefusal::Stale);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(at_ms: u64) -> RecommendationApproval {
        RecommendationApproval {
            record_id: "r".to_string(),
            record_digest: "sha256:d".to_string(),
            reviewer_principal: "reviewer".to_string(),
            approved_at_ms: at_ms,
            approval_revision: 1,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R019.100.2
    #[test]
    fn age_bounds_and_future_receipts() {
        let now = 10_000_000;
        let check = |approval: Option<&RecommendationApproval>, at: u64| {
            check_approval(approval, "r", "sha256:d", at, 60)
        };
        assert_eq!(check(None, now), Err(ApprovalRefusal::Missing));
        assert_eq!(check(Some(&receipt(now)), now), Ok(()));
        // Exactly at the age bound is admitted; one millisecond past is stale.
        assert_eq!(check(Some(&receipt(now - 60_000)), now), Ok(()));
        assert_eq!(
            check(Some(&receipt(now - 60_001)), now),
            Err(ApprovalRefusal::Stale)
        );
        // A receipt dated after the clock is stale.
        assert_eq!(
            check(Some(&receipt(now + 1)), now),
            Err(ApprovalRefusal::Stale)
        );
        // Another record id or digest is a mismatch before age is read.
        assert_eq!(
            check_approval(Some(&receipt(now)), "other", "sha256:d", now, 60),
            Err(ApprovalRefusal::DigestMismatch)
        );
        assert_eq!(
            check_approval(Some(&receipt(now)), "r", "sha256:other", now, 60),
            Err(ApprovalRefusal::DigestMismatch)
        );
        assert!(ApprovalRefusal::Missing.message().starts_with("APPROVAL_MISSING: "));
    }
}
