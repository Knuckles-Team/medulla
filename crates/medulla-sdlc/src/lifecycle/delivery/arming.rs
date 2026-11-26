//! The merge arming verdict keeps merge owner-only (MEDULLA-SDLC-R019.10).
//!
//! The engine never merges: it answers `arm`, `disarm` or `hold` and arbora acts on it. The
//! verdict is `disarm` when the head moved or a review requests changes. It is `arm` only when
//! the delivery opened the proposal, every required gate passed for the head, the required
//! check passed on the head commit, an owner approval names the head commit digest, and the
//! changed lines are within the knob `sdlc.delivery.arm_max_changed_lines`. Otherwise `hold`.
//! The knob `sdlc.delivery.owner_approval_required` is Fixed at true, so the approval is
//! never optional.

use medulla_source_types::delivery::{ArmingFacts, ArmingRefusal, ArmingVerdict};

/// Decide the arming verdict. A missing owner approval refuses `ARMING_APPROVAL_MISSING` and
/// an approval of another commit refuses `ARMING_HEAD_MISMATCH`; both are checked after the
/// disarm conditions, because a moved head or a requested change disarms whatever approval
/// exists.
pub fn arming_verdict(facts: &ArmingFacts) -> Result<ArmingVerdict, ArmingRefusal> {
    if facts.head_commit != facts.reviewed_head || facts.changes_requested {
        return Ok(ArmingVerdict::Disarm);
    }
    match &facts.approval_commit {
        None => {
            return Err(ArmingRefusal::ApprovalMissing(format!(
                "no owner approval names {}",
                facts.head_commit
            )))
        }
        Some(approved) if approved != &facts.head_commit => {
            return Err(ArmingRefusal::HeadMismatch(format!(
                "the approval names {approved}, the head is {}",
                facts.head_commit
            )))
        }
        Some(_) => {}
    }
    let arm = facts.delivery_opened_proposal
        && facts.gates_passed
        && facts.check_passed
        && facts.changed_lines <= facts.max_changed_lines;
    Ok(if arm {
        ArmingVerdict::Arm
    } else {
        ArmingVerdict::Hold
    })
}
