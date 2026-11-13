//! The verifier receipt check (MEDULLA-DECISION-ENGINE-R138.100.1).
//!
//! Pure: a receipt admits generated code only when it names the proposed
//! artifact and the declared tests, was produced under the current attack
//! policy and verifier, holds at least `verifier.min_cases` cases, and every
//! case was retested against the proposed artifact. The check returns every
//! breach, never only the first.

use medulla_decision_types::decision::context_package::is_sha256_hex;
use medulla_decision_types::decision::knob_families::verifier::min_cases;
#[cfg(test)]
use medulla_decision_types::decision::knob_families::verifier::KNOBS;
#[cfg(test)]
use medulla_decision_types::decision::knobs::{KnobSafety, KnobValue};
use medulla_decision_types::decision::verifier_receipt::{VerifierCaseState, VerifierReceipt};
use medulla_decision_types::decision::DecisionPolicy;

/// One reason a receipt does not admit the artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptBreach {
    /// A digest field is not 64 lowercase hex characters.
    MalformedDigest { field: &'static str },
    /// The receipt verified another artifact.
    ArtifactMismatch { expected: String, actual: String },
    /// The receipt verified against another test set.
    TestsMismatch { expected: String, actual: String },
    /// The attack policy is not the current revision.
    AttackPolicyStale { expected: String, actual: String },
    /// The verifier is not the current revision.
    VerifierStale { expected: String, actual: String },
    /// The receipt holds fewer cases than the knob requires.
    TooFewCases { required: usize, observed: usize },
    /// A case was not retested.
    CaseNotRetested {
        case_id: String,
        state: VerifierCaseState,
    },
    /// A case was retested against another artifact, or against none.
    CaseRetestMismatch { case_id: String },
}

/// Every breach of `receipt` against what the caller proposes and the current
/// revisions; empty when the receipt is complete.
pub fn check_verifier_receipt(
    receipt: &VerifierReceipt,
    proposed_artifact_digest: &str,
    declared_tests_digest: &str,
    current_attack_policy_revision: &str,
    current_verifier_revision: &str,
    policy: &DecisionPolicy,
) -> Vec<ReceiptBreach> {
    let mut breaches = Vec::new();
    for (field, value) in [
        ("artifact_digest", &receipt.artifact_digest),
        ("tests_digest", &receipt.tests_digest),
    ] {
        if !is_sha256_hex(value) {
            breaches.push(ReceiptBreach::MalformedDigest { field });
        }
    }
    if receipt.artifact_digest != proposed_artifact_digest {
        breaches.push(ReceiptBreach::ArtifactMismatch {
            expected: proposed_artifact_digest.to_string(),
            actual: receipt.artifact_digest.clone(),
        });
    }
    if receipt.tests_digest != declared_tests_digest {
        breaches.push(ReceiptBreach::TestsMismatch {
            expected: declared_tests_digest.to_string(),
            actual: receipt.tests_digest.clone(),
        });
    }
    if receipt.attack_policy_revision != current_attack_policy_revision {
        breaches.push(ReceiptBreach::AttackPolicyStale {
            expected: current_attack_policy_revision.to_string(),
            actual: receipt.attack_policy_revision.clone(),
        });
    }
    if receipt.verifier_revision != current_verifier_revision {
        breaches.push(ReceiptBreach::VerifierStale {
            expected: current_verifier_revision.to_string(),
            actual: receipt.verifier_revision.clone(),
        });
    }
    let required = min_cases(policy);
    if receipt.cases.len() < required {
        breaches.push(ReceiptBreach::TooFewCases {
            required,
            observed: receipt.cases.len(),
        });
    }
    for case in receipt.cases.iter() {
        if case.state != VerifierCaseState::Retested {
            breaches.push(ReceiptBreach::CaseNotRetested {
                case_id: case.case_id.clone(),
                state: case.state,
            });
        } else if case.retest_artifact_digest.as_deref() != Some(proposed_artifact_digest) {
            breaches.push(ReceiptBreach::CaseRetestMismatch {
                case_id: case.case_id.clone(),
            });
        }
    }
    breaches
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::verifier_receipt::VerifierCase;
    use medulla_types::contract::BoundedVec;

    const ARTIFACT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const TESTS: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    fn case(id: &str, state: VerifierCaseState, retest: Option<&str>) -> VerifierCase {
        VerifierCase {
            case_id: id.to_string(),
            kind: "injection".to_string(),
            state,
            retest_artifact_digest: retest.map(str::to_string),
        }
    }

    fn receipt(cases: Vec<VerifierCase>) -> VerifierReceipt {
        VerifierReceipt {
            artifact_digest: ARTIFACT.to_string(),
            tests_digest: TESTS.to_string(),
            attack_policy_revision: "attack-3".to_string(),
            verifier_revision: "verifier-7".to_string(),
            cases: BoundedVec::new(cases).unwrap(),
        }
    }

    fn check(receipt: &VerifierReceipt, policy: &DecisionPolicy) -> Vec<ReceiptBreach> {
        check_verifier_receipt(receipt, ARTIFACT, TESTS, "attack-3", "verifier-7", policy)
    }

    // spec: MEDULLA-DECISION-ENGINE-R138.100.1
    #[test]
    fn each_breach_is_reported() {
        let policy = DecisionPolicy::engine_default();
        let complete = receipt(vec![
            case("c1", VerifierCaseState::Retested, Some(ARTIFACT)),
            case("c2", VerifierCaseState::Retested, Some(ARTIFACT)),
        ]);
        assert_eq!(check(&complete, &policy), vec![]);

        let mut wrong_artifact = complete.clone();
        wrong_artifact.artifact_digest = OTHER.to_string();
        assert_eq!(
            check(&wrong_artifact, &policy),
            vec![ReceiptBreach::ArtifactMismatch {
                expected: ARTIFACT.to_string(),
                actual: OTHER.to_string()
            }]
        );
        let mut wrong_tests = complete.clone();
        wrong_tests.tests_digest = OTHER.to_string();
        assert!(matches!(
            check(&wrong_tests, &policy)[..],
            [ReceiptBreach::TestsMismatch { .. }]
        ));
        let mut stale_policy = complete.clone();
        stale_policy.attack_policy_revision = "attack-2".to_string();
        assert!(matches!(
            check(&stale_policy, &policy)[..],
            [ReceiptBreach::AttackPolicyStale { .. }]
        ));
        let mut stale_verifier = complete.clone();
        stale_verifier.verifier_revision = "verifier-6".to_string();
        assert!(matches!(
            check(&stale_verifier, &policy)[..],
            [ReceiptBreach::VerifierStale { .. }]
        ));
        let mut malformed = complete.clone();
        malformed.tests_digest = "x".to_string();
        let breaches =
            check_verifier_receipt(&malformed, ARTIFACT, "x", "attack-3", "verifier-7", &policy);
        assert_eq!(
            breaches,
            vec![ReceiptBreach::MalformedDigest {
                field: "tests_digest"
            }]
        );

        // No cases is below the minimum of one.
        assert_eq!(
            check(&receipt(vec![]), &policy),
            vec![ReceiptBreach::TooFewCases {
                required: 1,
                observed: 0
            }]
        );

        // Every state but retested breaches, and a retest against another
        // artifact or none breaches too; all are reported together.
        let mixed = receipt(vec![
            case("open", VerifierCaseState::Open, None),
            case("fixed", VerifierCaseState::Fixed, None),
            case("failed", VerifierCaseState::Failed, Some(ARTIFACT)),
            case("other", VerifierCaseState::Retested, Some(OTHER)),
            case("none", VerifierCaseState::Retested, None),
            case("good", VerifierCaseState::Retested, Some(ARTIFACT)),
        ]);
        assert_eq!(
            check(&mixed, &policy),
            vec![
                ReceiptBreach::CaseNotRetested {
                    case_id: "open".to_string(),
                    state: VerifierCaseState::Open
                },
                ReceiptBreach::CaseNotRetested {
                    case_id: "fixed".to_string(),
                    state: VerifierCaseState::Fixed
                },
                ReceiptBreach::CaseNotRetested {
                    case_id: "failed".to_string(),
                    state: VerifierCaseState::Failed
                },
                ReceiptBreach::CaseRetestMismatch {
                    case_id: "other".to_string()
                },
                ReceiptBreach::CaseRetestMismatch {
                    case_id: "none".to_string()
                },
            ]
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R138.100.1
    #[test]
    fn the_minimum_is_a_fixed_knob_read_from_the_policy() {
        assert_eq!(KNOBS[0].safety, KnobSafety::Fixed);
        assert!(KNOBS[0].admits(&KnobValue::Integer(64)));
        assert!(!KNOBS[0].admits(&KnobValue::Integer(65)));
        let mut pinned = DecisionPolicy::engine_default();
        pinned.knobs.insert(
            medulla_decision_types::decision::knobs::KnobName::new("verifier.min_cases"),
            KnobValue::Integer(3),
        );
        assert_eq!(min_cases(&pinned), 3);
        let two = receipt(vec![
            case("c1", VerifierCaseState::Retested, Some(ARTIFACT)),
            case("c2", VerifierCaseState::Retested, Some(ARTIFACT)),
        ]);
        assert_eq!(
            check(&two, &pinned),
            vec![ReceiptBreach::TooFewCases {
                required: 3,
                observed: 2
            }]
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R138.100.1
    #[test]
    fn receipts_round_trip_and_reject_unknown_fields() {
        let sealed = receipt(vec![case(
            "c1",
            VerifierCaseState::Retested,
            Some(ARTIFACT),
        )]);
        let json = serde_json::to_string(&sealed).unwrap();
        assert_eq!(
            serde_json::from_str::<VerifierReceipt>(&json).unwrap(),
            sealed
        );
        assert!(
            serde_json::from_str::<VerifierReceipt>(&json.replacen('{', "{\"x\":1,", 1)).is_err()
        );
        assert_eq!(
            serde_json::to_string(&VerifierCaseState::Retested).unwrap(),
            "\"retested\""
        );
    }
}
