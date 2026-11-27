//! MEDULLA-SDLC-R019.10: the arming verdict keeps merge owner-only.

use medulla_sdlc::lifecycle::delivery::arming::arming_verdict;
use medulla_source_types::delivery::{ArmingFacts, ArmingVerdict};

fn facts() -> ArmingFacts {
    ArmingFacts {
        delivery_opened_proposal: true,
        gates_passed: true,
        check_passed: true,
        head_commit: "sha256:head".into(),
        reviewed_head: "sha256:head".into(),
        approval_commit: Some("sha256:head".into()),
        changes_requested: false,
        changed_lines: 400,
        max_changed_lines: 400,
    }
}

// spec: MEDULLA-SDLC-R019.10
#[test]
fn arm_needs_owner_approval_on_head_and_passing_gates() {
    assert_eq!(arming_verdict(&facts()), Ok(ArmingVerdict::Arm));
    let hold = |edit: fn(&mut ArmingFacts)| {
        let mut f = facts();
        edit(&mut f);
        arming_verdict(&f)
    };
    assert_eq!(
        hold(|f| f.delivery_opened_proposal = false),
        Ok(ArmingVerdict::Hold)
    );
    assert_eq!(hold(|f| f.gates_passed = false), Ok(ArmingVerdict::Hold));
    assert_eq!(hold(|f| f.check_passed = false), Ok(ArmingVerdict::Hold));
    assert_eq!(hold(|f| f.changed_lines = 401), Ok(ArmingVerdict::Hold));
    assert_eq!(
        hold(|f| f.approval_commit = None).unwrap_err().code(),
        "ARMING_APPROVAL_MISSING"
    );
    assert_eq!(
        hold(|f| f.approval_commit = Some("sha256:old".into()))
            .unwrap_err()
            .code(),
        "ARMING_HEAD_MISMATCH"
    );
}

// spec: MEDULLA-SDLC-R019.10
#[test]
fn a_moved_head_or_requested_change_disarms() {
    let mut moved = facts();
    moved.head_commit = "sha256:new".into();
    assert_eq!(arming_verdict(&moved), Ok(ArmingVerdict::Disarm));
    let mut requested = facts();
    requested.changes_requested = true;
    requested.approval_commit = None;
    assert_eq!(arming_verdict(&requested), Ok(ArmingVerdict::Disarm));
}
