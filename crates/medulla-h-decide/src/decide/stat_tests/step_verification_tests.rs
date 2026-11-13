//! Step verification and adversarial admission, served through `Decide`
//! (MEDULLA-DECISION-ENGINE-R753.751, R753.755): the verdict is bound to the
//! trace, a failed step selects a recovery, and a compromised candidate is
//! rejected with `PROBE_COMPROMISED`.

use super::kernel_family_support::{decide_as_other_tenant, facts, kernel_request};
use super::*;
use medulla_decision_types::decision::trajectory_verdict::{
    AdmissionRequest, AdmissionVerdict, ProbeResult, ProbeVerdict, RecoveryAction, StepCheck,
    TraceStep, VerifyRequest,
};
use medulla_decision_types::decision::{AbstainReason, ResolutionKind};

fn hex(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

fn verify(schema: &ComponentDependency, passes: &[bool], attempts: u32) -> DecideRequest {
    let steps = (0..passes.len())
        .map(|i| TraceStep {
            step_id: format!("s{i}"),
            digest: hex(i as u8 + 1),
        })
        .collect();
    let checks = passes
        .iter()
        .enumerate()
        .map(|(i, passed)| StepCheck {
            step_id: format!("s{i}"),
            passed: *passed,
            check_digest: hex(i as u8 + 0x80),
        })
        .collect();
    kernel_request(
        schema,
        QuestionKind::StepVerification,
        "step_verification",
        TypedValue::StepVerification(Box::new(VerifyRequest {
            steps: BoundedVec::new(steps).unwrap(),
            checks: BoundedVec::new(checks).unwrap(),
            attempts,
        })),
        &["trace-1"],
    )
}

fn admission(schema: &ComponentDependency, verdicts: &[ProbeVerdict]) -> DecideRequest {
    let probes = verdicts
        .iter()
        .enumerate()
        .map(|(i, verdict)| ProbeResult {
            probe_id: format!("p{i}"),
            verdict: *verdict,
        })
        .collect();
    kernel_request(
        schema,
        QuestionKind::AdversarialAdmission,
        "adversarial_admission",
        TypedValue::AdversarialAdmission(Box::new(AdmissionRequest {
            probes: BoundedVec::new(probes).unwrap(),
        })),
        &["candidate-1"],
    )
}

fn verification(
    record: &StatisticalDecisionRecord,
) -> &medulla_decision_types::decision::trajectory_verdict::StepVerification {
    record
        .answer
        .as_ref()
        .and_then(|a| a.step_verdict())
        .expect("the family answers a step verdict")
}

/// Verifies MEDULLA-DECISION-ENGINE-R753.751 and R753.755: the served verdict
/// is bound to the recomputed trace digest, and the recovery follows the
/// attempts.
// spec: MEDULLA-DECISION-ENGINE-R753.751
// spec: MEDULLA-DECISION-ENGINE-R753.755
#[tokio::test]
async fn served_decide_binds_the_verdict_to_the_trace() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    let batch = decide(&h, verify(&schema, &[true, true, false, true], 0))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::StepVerification);
    let answer = verification(record);
    assert_eq!(
        (answer.verdict.step_id.as_str(), answer.verdict.passed),
        ("s2", false)
    );
    assert_eq!(
        answer.recovery,
        Some(RecoveryAction::Retry {
            step_id: "s2".into()
        })
    );
    // The digest is the one the kernel recomputes from the step digests.
    let digests = [hex(1), hex(2), hex(3), hex(4)];
    let refs: Vec<&str> = digests.iter().map(String::as_str).collect();
    assert_eq!(
        answer.verdict.trace_digest,
        medulla_decision_types::decision::digest::step_trace_digest(&refs)
    );
    assert_eq!(answer.verdict.check_digests.len(), 4);
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let record_facts = facts(record);
    for expected in [
        format!("verify:trace={}", answer.verdict.trace_digest),
        "verify:step=s2/false".to_string(),
        "verify:step=s0/true".to_string(),
        "verify:recovery=retry:s2".to_string(),
        "knob:verify.max_retries=2".to_string(),
    ] {
        assert!(
            record_facts.contains(&expected.as_str()),
            "missing premise {expected}"
        );
    }

    // Past the retry budget the recovery is a rollback to the last pass, and
    // with nothing passed an abandon.
    let rolled = decide(&h, verify(&schema, &[true, true, false, true], 2))
        .await
        .unwrap();
    assert_eq!(
        verification(&rolled.records.as_slice()[0]).recovery,
        Some(RecoveryAction::Rollback {
            step_id: "s1".into()
        })
    );
    let abandoned = decide(&h, verify(&schema, &[false, true], 9))
        .await
        .unwrap();
    assert_eq!(
        verification(&abandoned.records.as_slice()[0]).recovery,
        Some(RecoveryAction::Abandon)
    );
    // A passing trace needs no recovery.
    let passed = decide(&h, verify(&schema, &[true, true], 0)).await.unwrap();
    let passed = verification(&passed.records.as_slice()[0]);
    assert!(passed.verdict.passed && passed.recovery.is_none());
    let batch = decide(&h, verify(&schema, &[true], 0)).await.unwrap();
    assert!(facts(&batch.records.as_slice()[0]).contains(&"verify:recovery=none"));
}

/// Verifies MEDULLA-DECISION-ENGINE-R753.755: a compromised or inconclusive
/// probe rejects the candidate with `PROBE_COMPROMISED`, and an unbroken one is
/// admitted.
// spec: MEDULLA-DECISION-ENGINE-R753.755
#[tokio::test]
async fn served_decide_rejects_a_compromised_candidate() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let verdict = |record: &StatisticalDecisionRecord| {
        record
            .answer
            .as_ref()
            .and_then(|a| a.admission())
            .expect("the family answers an admission verdict")
    };

    let admitted = decide(
        &h,
        admission(&schema, &[ProbeVerdict::Resisted, ProbeVerdict::Resisted]),
    )
    .await
    .unwrap();
    let record = &admitted.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::AdversarialAdmission);
    assert_eq!(verdict(record), AdmissionVerdict::Admit);
    assert!(matches!(
        record.outcome,
        StatisticalOutcome::Advisory { .. }
    ));
    assert!(facts(record).contains(&"admission:probe=p0/resisted"));

    for probes in [
        vec![ProbeVerdict::Resisted, ProbeVerdict::Compromised],
        vec![ProbeVerdict::Resisted, ProbeVerdict::Inconclusive],
    ] {
        let rejected = decide(&h, admission(&schema, &probes)).await.unwrap();
        let record = &rejected.records.as_slice()[0];
        assert_eq!(verdict(record), AdmissionVerdict::Reject);
        let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
            panic!("a rejection abstains, got {:?}", record.outcome);
        };
        assert!(matches!(
            &reasons.as_slice()[0],
            AbstainReason::Family {
                family: QuestionKind::AdversarialAdmission,
                code,
                ..
            } if code.as_str() == "PROBE_COMPROMISED"
        ));
        assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    }
    assert!(decide(
        &h,
        admission(
            &schema,
            &[ProbeVerdict::Resisted, ProbeVerdict::Compromised]
        )
    )
    .await
    .unwrap()
    .records
    .as_slice()[0]
        .premises
        .iter()
        .any(|p| p.fact == "admission:probe=p1/compromised"));

    // More probes than `knob verify.probe_budget`, or none, are refused.
    let error = decide(&h, admission(&schema, &[ProbeVerdict::Resisted; 9]))
        .await
        .unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
    let error = decide(&h, admission(&schema, &[])).await.unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R753.755: a request for another tenant is
/// refused before anything is read, and a trace whose checks do not match its
/// steps is refused.
// spec: MEDULLA-DECISION-ENGINE-R753.755
#[tokio::test]
async fn other_tenant_traces_are_not_read() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let refused = decide_as_other_tenant(&h, verify(&schema, &[true], 0)).await;
    assert!(refused.starts_with("ACCESS_DENIED"), "{refused}");
    let refused = decide_as_other_tenant(&h, admission(&schema, &[ProbeVerdict::Resisted])).await;
    assert!(refused.starts_with("ACCESS_DENIED"), "{refused}");

    let mismatched = kernel_request(
        &schema,
        QuestionKind::StepVerification,
        "step_verification",
        TypedValue::StepVerification(Box::new(VerifyRequest {
            steps: BoundedVec::new(vec![TraceStep {
                step_id: "s0".into(),
                digest: hex(1),
            }])
            .unwrap(),
            checks: BoundedVec::new(vec![StepCheck {
                step_id: "other".into(),
                passed: true,
                check_digest: hex(2),
            }])
            .unwrap(),
            attempts: 0,
        })),
        &["trace-1"],
    );
    let error = decide(&h, mismatched).await.unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
}
