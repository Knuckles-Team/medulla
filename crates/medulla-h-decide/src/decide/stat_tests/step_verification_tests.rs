//! Step verification and adversarial admission, served through `Decide`
//! (MEDULLA-DECISION-ENGINE-R753.751, R753.755): the verdict is bound to the
//! trace, a failed step selects a recovery, and a compromised candidate is
//! rejected with `PROBE_COMPROMISED`.

use super::kernel_family_support::{
    decide_as_other_tenant, facts, kernel_request, persisted_files,
};
use super::*;
use medulla_decision_types::decision::trajectory_verdict::{
    AdmissionRequest, AdmissionVerdict, CheckInterval, ProbeResult, ProbeVerdict, RecoveryAction,
    StepCheck, StepOutcome, TraceStep, VerifyRequest,
};
use medulla_decision_types::decision::{AbstainReason, ResolutionKind};

fn hex(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

fn verify(schema: &ComponentDependency, passes: &[bool], attempts: u32) -> DecideRequest {
    verify_naming(schema, passes, attempts, |recomputed| recomputed)
}

/// A verification whose named trace digest is `name(recomputed digest)`.
fn verify_naming(
    schema: &ComponentDependency,
    passes: &[bool],
    attempts: u32,
    name: impl FnOnce(String) -> String,
) -> DecideRequest {
    let shaped: Vec<(bool, Option<CheckInterval>, Vec<String>)> = passes
        .iter()
        .map(|passed| (*passed, None, Vec::new()))
        .collect();
    verify_shaped(schema, &shaped, attempts, name)
}

/// A verification of one `(passed, interval, evidence ids)` triple per step.
fn verify_shaped(
    schema: &ComponentDependency,
    shape: &[(bool, Option<CheckInterval>, Vec<String>)],
    attempts: u32,
    name: impl FnOnce(String) -> String,
) -> DecideRequest {
    let steps: Vec<TraceStep> = shape
        .iter()
        .enumerate()
        .map(|(i, (_, _, evidence))| TraceStep {
            step_id: format!("s{i}"),
            digest: hex(i as u8 + 1),
            evidence_ids: evidence.clone(),
        })
        .collect();
    let digests: Vec<&str> = steps.iter().map(|s| s.digest.as_str()).collect();
    let trace_digest = name(medulla_decision_types::decision::digest::step_trace_digest(
        &digests,
    ));
    let checks = shape
        .iter()
        .enumerate()
        .map(|(i, (passed, interval, _))| StepCheck {
            step_id: format!("s{i}"),
            passed: *passed,
            check_digest: hex(i as u8 + 0x80),
            interval: *interval,
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
            trace_digest,
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
        (answer.verdict.step_id.as_str(), answer.verdict.outcome),
        ("s2", StepOutcome::Failed)
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
        "verify:step=s2/failed".to_string(),
        "verify:step=s0/verified".to_string(),
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
    assert!(passed.verdict.outcome == StepOutcome::Verified && passed.recovery.is_none());
    assert!(passed.withheld.is_empty());
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
    assert!(error.starts_with("PROBE_BUDGET_EXCEEDED"), "{error}");
    let error = decide(&h, admission(&schema, &[])).await.unwrap_err();
    assert!(error.starts_with("PROBE_SET_EMPTY"), "{error}");
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
                evidence_ids: Vec::new(),
            }])
            .unwrap(),
            checks: BoundedVec::new(vec![StepCheck {
                step_id: "other".into(),
                passed: true,
                check_digest: hex(2),
                interval: None,
            }])
            .unwrap(),
            attempts: 0,
            trace_digest: medulla_decision_types::decision::digest::step_trace_digest(&[
                hex(1).as_str()
            ]),
        })),
        &["trace-1"],
    );
    let error = decide(&h, mismatched).await.unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R753.752 (verification and admission half):
/// a verification without a trace digest, or with the wrong one, an empty probe
/// set and an over-budget probe set are each refused with their registered
/// code, a compromised probe is the rejection reason, and nothing is written.
// spec: MEDULLA-DECISION-ENGINE-R753.752
#[tokio::test]
async fn verification_and_admission_refusals() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let dir = std::path::PathBuf::from(h.state.read().await.persist_dir.clone().unwrap());
    decide(&h, verify(&schema, &[true, true], 0)).await.unwrap();
    let before = persisted_files(&dir);

    // VERIFY_VERDICT_UNBOUND: no digest, and a digest that is not 64 lowercase hex.
    for named in ["", "not-a-digest"] {
        let error = decide(
            &h,
            verify_naming(&schema, &[true, true], 0, |_| named.to_string()),
        )
        .await
        .unwrap_err();
        assert!(error.starts_with("VERIFY_VERDICT_UNBOUND"), "{error}");
    }
    // VERIFY_TRACE_MISMATCH: a well-formed digest of another trace.
    let error = decide(&h, verify_naming(&schema, &[true, true], 0, |_| hex(0xee)))
        .await
        .unwrap_err();
    assert!(error.starts_with("VERIFY_TRACE_MISMATCH"), "{error}");
    // PROBE_SET_EMPTY and PROBE_BUDGET_EXCEEDED.
    let error = decide(&h, admission(&schema, &[])).await.unwrap_err();
    assert!(error.starts_with("PROBE_SET_EMPTY"), "{error}");
    let error = decide(&h, admission(&schema, &[ProbeVerdict::Resisted; 9]))
        .await
        .unwrap_err();
    assert!(error.starts_with("PROBE_BUDGET_EXCEEDED"), "{error}");
    assert_eq!(persisted_files(&dir), before, "a refusal writes nothing");

    // PROBE_COMPROMISED: the rejection reason of an abstention.
    let record = decide(
        &h,
        admission(
            &schema,
            &[ProbeVerdict::Resisted, ProbeVerdict::Compromised],
        ),
    )
    .await
    .unwrap()
    .records
    .as_slice()[0]
        .clone();
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("a compromised probe abstains, got {:?}", record.outcome);
    };
    assert_eq!(reasons.as_slice()[0].code().as_str(), "PROBE_COMPROMISED");

    for code in medulla_decision_types::decision::trajectory_verdict::VERIFY_REFUSALS
        .iter()
        .chain(medulla_decision_types::decision::trajectory_verdict::REFUSALS)
    {
        assert_eq!(
            medulla_decision_types::decision::codes::resolve(code.as_str()),
            Some(*code)
        );
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R121.100: a tight trace is `verified`; a
/// wide interval on step 2 is `uncertain` and withholds the step after it; a
/// failed step 2 is `failed`, selects its recovery and withholds it too. Every
/// cited evidence id is a premise, and another tenant is refused.
// spec: MEDULLA-DECISION-ENGINE-R121.100
#[tokio::test]
async fn failed_and_uncertain_steps_withhold_their_dependents() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let tight = Some(CheckInterval {
        low_micros: 450_000,
        high_micros: 550_000,
    });
    let wide = Some(CheckInterval {
        low_micros: 300_000,
        high_micros: 700_000,
    });
    let ev = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
    let run = |shape: Vec<(bool, Option<CheckInterval>, Vec<String>)>| {
        verify_shaped(&schema, &shape, 0, |recomputed| recomputed)
    };

    // All passing and tight: verified, nothing withheld.
    let batch = decide(
        &h,
        run(vec![
            (true, tight, ev(&["e-a"])),
            (true, tight, ev(&["e-b", "e-c"])),
            (true, None, ev(&[])),
        ]),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let answer = verification(record);
    assert_eq!(answer.verdict.outcome, StepOutcome::Verified);
    assert_eq!(answer.verdict.step_id, "s2");
    assert!(answer.recovery.is_none() && answer.withheld.is_empty());
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let record_facts = facts(record);
    for expected in [
        "verify:evidence=s0/e-a",
        "verify:evidence=s1/e-b",
        "verify:evidence=s1/e-c",
        "verify:step=s1/verified",
    ] {
        assert!(
            record_facts.contains(&expected),
            "missing premise {expected}"
        );
    }

    // A wide interval on step 2 (s1): uncertain, no recovery, s2 withheld.
    let batch = decide(
        &h,
        run(vec![
            (true, tight, ev(&[])),
            (true, wide, ev(&[])),
            (true, tight, ev(&[])),
        ]),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let answer = verification(record);
    assert_eq!(answer.verdict.outcome, StepOutcome::Uncertain);
    assert_eq!(answer.verdict.step_id, "s1");
    assert!(answer.recovery.is_none());
    assert_eq!(answer.withheld, ["s2"]);
    assert!(facts(record).contains(&"verify:step=s1/uncertain"));

    // A failed step 2 (s1): failed, its recovery, s2 withheld.
    let batch = decide(
        &h,
        run(vec![
            (true, tight, ev(&[])),
            (false, wide, ev(&[])),
            (true, tight, ev(&[])),
        ]),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let answer = verification(record);
    assert_eq!(answer.verdict.outcome, StepOutcome::Failed);
    assert_eq!(answer.verdict.step_id, "s1");
    assert_eq!(
        answer.recovery,
        Some(RecoveryAction::Retry {
            step_id: "s1".into()
        })
    );
    assert_eq!(answer.withheld, ["s2"]);
    assert!(facts(record).contains(&"verify:step=s1/failed"));

    // An inverted interval is refused; another tenant is refused.
    let inverted = Some(CheckInterval {
        low_micros: 9,
        high_micros: 1,
    });
    let error = decide(&h, run(vec![(true, inverted, ev(&[]))]))
        .await
        .unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
    let refused = decide_as_other_tenant(&h, run(vec![(true, tight, ev(&[]))])).await;
    assert!(refused.starts_with("ACCESS_DENIED"), "{refused}");
}
