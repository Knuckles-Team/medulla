//! The `PolicyEvolution` handler folds every refusal under its origin
//! (MEDULLA-DECISION-ENGINE-R010.100.2).

use super::refuse;
use medulla_decision_outcome_types::policy_evolution::PolicyRefusal;

/// Verifies MEDULLA-DECISION-ENGINE-R010.100.2: Fold existing per-feature refusal enums
// spec: MEDULLA-DECISION-ENGINE-R010.100.2
#[test]
fn policy_refusals_fold_under_the_policy_evolution_origin() {
    let cases = [
        (PolicyRefusal::CaptureDisabled, "POLICY_CAPTURE_DISABLED: "),
        (
            PolicyRefusal::RecordMissing("r1".into()),
            "POLICY_RECORD_MISSING: r1",
        ),
        (
            PolicyRefusal::BlobMissing("ab".into()),
            "POLICY_BLOB_MISSING: ab",
        ),
    ];
    for (refusal, expected) in cases {
        let display = refusal.to_string();
        assert_eq!(refuse(refusal), expected);
        assert_eq!(display, expected, "the wire text is unchanged");
    }
}
