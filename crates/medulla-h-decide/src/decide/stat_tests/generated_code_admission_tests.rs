//! MEDULLA-DECISION-ENGINE-R138.100.2: a served `GeneratedCodeAdmission`
//! question admits an artifact only on a complete verifier receipt, and names
//! each way a receipt falls short with its registered code.

use super::*;
use medulla_decision_types::decision::digest::verifier_receipt_digest;
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::statistical::declared::{DeclaredOption, DeclaredText};
use medulla_decision_types::decision::verifier_receipt::{
    VerifierCase, VerifierCaseState, VerifierReceipt,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, EvidenceClass, PremiseClass, ResolutionKind,
};
use medulla_server_api::authority_context::VerifiedRequestContext;

const ARTIFACT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_ARTIFACT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const TESTS: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const OTHER_TESTS: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

/// One attack case: id, state and the artifact its retest ran against.
type Case = (&'static str, VerifierCaseState, Option<&'static str>);

fn complete_cases() -> Vec<Case> {
    vec![
        ("c1", VerifierCaseState::Retested, Some(ARTIFACT)),
        ("c2", VerifierCaseState::Retested, Some(ARTIFACT)),
    ]
}

fn option_of((id, state, retest): &Case) -> DeclaredOption {
    let state_name = serde_json::to_value(state)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::new(vec![
            "kind:injection".to_string(),
            format!("state:{state_name}"),
        ])
        .unwrap(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::new(
            retest
                .iter()
                .map(|digest| DeclaredText {
                    key: "retest_artifact_digest".to_string(),
                    text: digest.to_string(),
                })
                .collect(),
        )
        .unwrap(),
    }
}

/// The request a lane sends: `proposed` is the artifact under admission and
/// `header` the receipt header the verifier produced for `cases`.
struct Submission {
    proposed: &'static str,
    tests: &'static str,
    header_artifact: &'static str,
    header_tests: &'static str,
    attack_policy: &'static str,
    verifier: &'static str,
    cases: Vec<Case>,
    with_receipt: bool,
}

impl Submission {
    fn complete() -> Self {
        Self {
            proposed: ARTIFACT,
            tests: TESTS,
            header_artifact: ARTIFACT,
            header_tests: TESTS,
            attack_policy: "attack-3",
            verifier: "verifier-7",
            cases: complete_cases(),
            with_receipt: true,
        }
    }

    fn request(
        &self,
        schema: &ComponentDependency,
        tenant: &str,
        policy: DecisionPolicyRef,
    ) -> DecideRequest {
        let options: Vec<DeclaredOption> = self.cases.iter().map(option_of).collect();
        let receipt = VerifierReceipt {
            artifact_digest: self.header_artifact.to_string(),
            tests_digest: self.header_tests.to_string(),
            attack_policy_revision: "attack-3".to_string(),
            verifier_revision: "verifier-7".to_string(),
            cases: BoundedVec::new(
                self.cases
                    .iter()
                    .map(|(id, state, retest)| VerifierCase {
                        case_id: id.to_string(),
                        kind: "injection".to_string(),
                        state: *state,
                        retest_artifact_digest: retest.map(str::to_string),
                    })
                    .collect(),
            )
            .unwrap(),
        };
        let text = |name: &str, value: &str| TypedParam {
            name: name.to_string(),
            value: TypedValue::Text(value.to_string()),
        };
        let mut params = vec![
            text("artifact_digest", self.proposed),
            text("tests_digest", self.tests),
            text("attack_policy_revision", self.attack_policy),
            text("verifier_revision", self.verifier),
        ];
        if self.with_receipt {
            params.push(text(
                "verifier_receipt_digest",
                &verifier_receipt_digest(&receipt),
            ));
            params.push(text(
                "receipt",
                &serde_json::json!({
                    "artifact_digest": self.header_artifact,
                    "tests_digest": self.header_tests,
                    "attack_policy_revision": "attack-3",
                    "verifier_revision": "verifier-7",
                })
                .to_string(),
            ));
        }
        DecideRequest {
            tenant_id: tenant.to_string(),
            question: StatisticalQuestion {
                question_id: "code.admission".to_string(),
                kind: QuestionKind::GeneratedCodeAdmission,
                safety: QuestionSafety::Ordinary,
            },
            candidates: CandidateSource::Declared {
                options: BoundedVec::new(options).unwrap(),
            },
            feature_schema: schema.clone(),
            head: None,
            policy,
            params: BoundedVec::new(params).unwrap(),
            max_records: None,
            belief_as_of: BoundedVec::default(),
        }
    }
}

/// The registered codes an abstained record reports, in order.
fn abstained_codes(batch: &DecisionBatch) -> Vec<String> {
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("expected an abstention, got {:?}", record.outcome)
    };
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
    reasons
        .iter()
        .map(|reason| match reason {
            AbstainReason::Family { family, code, .. } => {
                assert_eq!(*family, QuestionKind::GeneratedCodeAdmission);
                code.as_str().to_string()
            }
            other => panic!("expected a family reason, got {other:?}"),
        })
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R138.100.2
#[tokio::test]
async fn admission_requires_a_complete_receipt() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let run = |submission: &Submission| {
        decide(
            &h,
            submission.request(&schema, TENANT, DecisionPolicyRef::Default),
        )
    };

    // A complete receipt admits, sealed as a constraint backed by a claim, with
    // one claim premise per case and one definition for the verifier revision.
    let batch = run(&Submission::complete()).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::GeneratedCodeAdmission);
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected an admission, got {:?}", record.outcome)
    };
    assert_eq!(
        scores
            .iter()
            .map(|s| s.option_id.as_str())
            .collect::<Vec<_>>(),
        vec!["admit"]
    );
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let premise = |fact: &str| record.premises.iter().find(|p| p.fact == fact);
    assert_eq!(
        premise("verifier.revision=verifier-7").unwrap().class,
        PremiseClass::Definition
    );
    for case in ["c1", "c2"] {
        assert_eq!(
            premise(&format!("verifier.case:{case}=retested"))
                .unwrap()
                .class,
            PremiseClass::Claim
        );
    }

    // Skipped verifier: no receipt accompanies the artifact.
    let mut skipped = Submission::complete();
    skipped.with_receipt = false;
    assert_eq!(
        abstained_codes(&run(&skipped).await.unwrap()),
        vec!["VERIFIER_RECEIPT_MISSING"]
    );

    // Unresolved edge case: an open and a fixed case are not retested.
    let mut unresolved = Submission::complete();
    unresolved.cases[0].1 = VerifierCaseState::Open;
    unresolved.cases[0].2 = None;
    unresolved.cases[1].1 = VerifierCaseState::Fixed;
    assert_eq!(
        abstained_codes(&run(&unresolved).await.unwrap()),
        vec!["VERIFIER_CASE_UNRESOLVED"]
    );

    // Stale artifact: the receipt verified another artifact than the proposed one.
    let mut stale = Submission::complete();
    stale.proposed = OTHER_ARTIFACT;
    assert_eq!(
        abstained_codes(&run(&stale).await.unwrap()),
        vec!["VERIFIER_ARTIFACT_STALE"]
    );
    // A retest that ran against another artifact is stale as well.
    let mut stale_retest = Submission::complete();
    stale_retest.cases[1].2 = Some(OTHER_ARTIFACT);
    assert_eq!(
        abstained_codes(&run(&stale_retest).await.unwrap()),
        vec!["VERIFIER_ARTIFACT_STALE"]
    );

    // Changed tests: the declared tests are not the ones the receipt ran.
    let mut changed = Submission::complete();
    changed.tests = OTHER_TESTS;
    assert_eq!(
        abstained_codes(&run(&changed).await.unwrap()),
        vec!["VERIFIER_TESTS_CHANGED"]
    );

    // Failed recheck: one case failed its retest.
    let mut failed = Submission::complete();
    failed.cases[1].1 = VerifierCaseState::Failed;
    assert_eq!(
        abstained_codes(&run(&failed).await.unwrap()),
        vec!["VERIFIER_RECHECK_FAILED"]
    );

    // A stale attack policy or verifier revision.
    let mut revision = Submission::complete();
    revision.verifier = "verifier-8";
    assert_eq!(
        abstained_codes(&run(&revision).await.unwrap()),
        vec!["VERIFIER_REVISION_STALE"]
    );

    // Several breaches are all reported, one reason per code, in code order.
    let mut many = Submission::complete();
    many.tests = OTHER_TESTS;
    many.cases[0].1 = VerifierCaseState::Open;
    many.cases[0].2 = None;
    many.cases[1].1 = VerifierCaseState::Failed;
    assert_eq!(
        abstained_codes(&run(&many).await.unwrap()),
        vec![
            "VERIFIER_CASE_UNRESOLVED",
            "VERIFIER_TESTS_CHANGED",
            "VERIFIER_RECHECK_FAILED"
        ]
    );

    // A receipt that does not match the digest the caller states is refused.
    let mut forged = Submission::complete().request(&schema, TENANT, DecisionPolicyRef::Default);
    let mut params = forged.params.as_slice().to_vec();
    for param in &mut params {
        if param.name == "verifier_receipt_digest" {
            param.value = TypedValue::Text(OTHER_ARTIFACT.to_string());
        }
    }
    forged.params = BoundedVec::new(params).unwrap();
    let refused = decide(&h, forged).await.unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");

    // The policy's `verifier.min_cases` binds: two cases are too few under a
    // minimum of three, and the receipt is reported missing.
    let mut strict = DecisionPolicy::engine_default();
    strict
        .knobs
        .insert(KnobName::new("verifier.min_cases"), KnobValue::Integer(3));
    let pinned = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-verifier-min-cases", &strict),
    };
    let batch = decide(
        &h,
        Submission::complete().request(&schema, TENANT, pinned.clone()),
    )
    .await
    .unwrap();
    assert_eq!(abstained_codes(&batch), vec!["VERIFIER_RECEIPT_MISSING"]);

    // Cross-tenant negative: another tenant cannot use this tenant's pinned
    // policy, so its stricter minimum cannot be bypassed or borrowed.
    let other = "tenant-admission-other";
    let other_schema = publish_route_schema_in(&h, other);
    let foreign = Submission::complete().request(&other_schema, other, pinned);
    let refused = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(
            &h.state,
            1,
            &VerifiedRequestContext::verified_for_test_in_tenant("decider", other),
            foreign,
        )
        .await,
    );
    assert!(
        refused.is_err(),
        "another tenant's pinned policy is refused"
    );
}
