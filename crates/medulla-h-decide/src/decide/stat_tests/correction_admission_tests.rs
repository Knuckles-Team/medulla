//! Correction admission served through `Decide` (MEDULLA-DECISION-ENGINE-R161.3,
//! R161.4): a failed run's trace, its fix and its regression test are admitted
//! only when the receipts show the test failed before the fix and passed after.

use super::*;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::trace_fix_binding::{
    CorrectionBinding, FailedRunTrace, GeneratedFix, RegressionTest, RevisionOutcome,
    RevisionReceipt, REFUSALS,
};
use medulla_decision_types::decision::{PremiseClass, ResolutionKind};
use medulla_types::contract::{Digest256, OpaqueId};

fn id(value: &str) -> OpaqueId {
    OpaqueId::new(value).unwrap()
}

fn receipt(revision: &[u8], outcome: RevisionOutcome) -> RevisionReceipt {
    RevisionReceipt {
        test_id: id("test:1"),
        revision_id: Digest256::sha256(revision),
        outcome,
    }
}

fn admitted() -> CorrectionBinding {
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
        receipts: vec![
            receipt(b"r0", RevisionOutcome::Failed),
            receipt(b"r1", RevisionOutcome::Passed),
        ],
    }
}

fn request(schema: &ComponentDependency, binding: CorrectionBinding) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: "correction.admit".to_string(),
            kind: QuestionKind::CorrectionAdmission,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![DeclaredOption {
                option_id: "admit_correction".to_string(),
                classification: BoundedVec::default(),
                numbers: BoundedVec::default(),
                texts: BoundedVec::default(),
            }])
            .unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![TypedParam {
            name: "correction_binding".to_string(),
            value: TypedValue::CorrectionBinding(Box::new(binding)),
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    }
}

/// Every file under the server's persist directory with its length.
fn stored_files(dir: &std::path::Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                pending.push(entry.path());
            } else {
                out.push((entry.path().display().to_string(), meta.len()));
            }
        }
    }
    out.sort();
    out
}

async fn foreign_tenant_is_refused(h: &Harness, schema: &ComponentDependency) {
    let mut foreign = request(schema, admitted());
    foreign.tenant_id = "tenant-other".to_string();
    let other = VerifiedRequestContext::verified_for_test_in_tenant("decider", "tenant-other");
    let error = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(&h.state, 1, &other, foreign).await,
    )
    .unwrap_err();
    assert!(error.starts_with("COMPONENT_PIN_MISMATCH"), "{error}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R161.3: a trace, fix and test with a
/// failed-before, passed-after receipt pair are admitted.
// spec: MEDULLA-DECISION-ENGINE-R161.3
#[tokio::test]
async fn trace_to_test_routes() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    let batch = decide(&h, request(&schema, admitted())).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::CorrectionAdmission);
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!(
            "an admitted correction answers advisory, got {:?}",
            record.outcome
        );
    };
    assert_eq!(scores.as_slice().len(), 1);
    assert_eq!(scores.as_slice()[0].option_id, "admit_correction");
    let facts: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
    assert!(facts
        .iter()
        .any(|f| f.starts_with("correction:trace=trace:1@")));
    assert!(facts.iter().any(|f| f.starts_with("correction:fix=fix:1@")));
    assert!(facts
        .iter()
        .any(|f| f.starts_with("correction:test=test:1@")));
    assert!(facts.contains(&"correction:receipts=failed_before_passed_after"));

    foreign_tenant_is_refused(&h, &schema).await;
}

/// Verifies MEDULLA-DECISION-ENGINE-R161.4: each broken link abstains with its
/// registered code and nothing is written.
// spec: MEDULLA-DECISION-ENGINE-R161.4
#[tokio::test]
async fn incomplete_binding_refuses() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    // Warm the server, then snapshot what it stores: refusals must not change it.
    decide(&h, request(&schema, admitted())).await.unwrap();
    let dir = std::path::PathBuf::from(h.state.read().await.persist_dir.clone().unwrap());
    let before = stored_files(&dir);

    let mut cases: Vec<(&str, CorrectionBinding)> = Vec::new();
    let mut no_trace = admitted();
    no_trace.trace.failure_point.clear();
    cases.push(("BINDING_TRACE_ABSENT", no_trace));
    let mut no_receipts = admitted();
    no_receipts.receipts.clear();
    cases.push(("BINDING_TEST_MISSING", no_receipts));
    let mut unexecuted = admitted();
    unexecuted.receipts.pop();
    cases.push(("BINDING_RECEIPT_UNEXECUTED", unexecuted));
    let mut broken = admitted();
    broken.fix.trace_id = id("trace:other");
    cases.push(("BINDING_CHAIN_BROKEN", broken));
    let mut pattern = admitted();
    pattern.receipts[0].outcome = RevisionOutcome::Passed;
    cases.push(("BINDING_OUTCOME_PATTERN_MISMATCH", pattern));

    for (code, binding) in cases {
        let batch = decide(&h, request(&schema, binding)).await.unwrap();
        let record = &batch.records.as_slice()[0];
        assert_eq!(record.resolution_kind, ResolutionKind::Abstention, "{code}");
        let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
            panic!("{code} abstains, got {:?}", record.outcome);
        };
        assert_eq!(reasons.as_slice()[0].code().as_str(), code);
    }

    // A refusal wrote nothing, and every code is registered.
    assert_eq!(stored_files(&dir), before);
    for code in REFUSALS {
        assert_eq!(
            medulla_decision_types::decision::codes::resolve(code.as_str()),
            Some(*code)
        );
    }
    let mut missing = request(&schema, admitted());
    missing.params = BoundedVec::default();
    let error = decide(&h, missing).await.unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");

    foreign_tenant_is_refused(&h, &schema).await;
}

/// Verifies MEDULLA-DECISION-ENGINE-R161.5: the sealed record holds one premise
/// per bound value, all claims.
// spec: MEDULLA-DECISION-ENGINE-R161.5
#[tokio::test]
async fn binding_evidence_records() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let binding = admitted();

    let batch = decide(&h, request(&schema, binding.clone())).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    let fact = |key: &str| {
        record
            .premises
            .iter()
            .find(|premise| premise.fact.starts_with(&format!("binding:{key}=")))
            .unwrap_or_else(|| panic!("no premise binding:{key}"))
    };
    assert_eq!(
        fact("trace").fact,
        format!(
            "binding:trace=trace:1@{}",
            binding.trace.error_trace_digest.to_hex()
        )
    );
    assert_eq!(
        fact("fix").fact,
        format!("binding:fix=fix:1@{}", binding.fix.fix_digest.to_hex())
    );
    assert_eq!(
        fact("test").fact,
        format!("binding:test=test:1@{}", binding.test.test_digest.to_hex())
    );
    assert_eq!(
        fact("before_revision").fact,
        format!(
            "binding:before_revision={}",
            binding.test.before_revision.to_hex()
        )
    );
    assert_eq!(
        fact("after_revision").fact,
        format!(
            "binding:after_revision={}",
            binding.test.after_revision.to_hex()
        )
    );
    assert_eq!(fact("before_outcome").fact, "binding:before_outcome=failed");
    assert_eq!(fact("after_outcome").fact, "binding:after_outcome=passed");
    assert_eq!(fact("decision").fact, "binding:decision=admit_correction");
    for key in [
        "trace",
        "fix",
        "test",
        "before_revision",
        "after_revision",
        "before_outcome",
        "after_outcome",
        "decision",
    ] {
        assert_eq!(fact(key).class, PremiseClass::Claim, "{key}");
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R161.6: the precomputed receipts of an admit
/// case and of each refusal case go through the production dispatch.
// spec: MEDULLA-DECISION-ENGINE-R161.6
#[tokio::test]
async fn receipts_gate_correction_admission() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    let admit = decide(&h, request(&schema, admitted())).await.unwrap();
    let record = &admit.records.as_slice()[0];
    assert!(matches!(
        record.outcome,
        StatisticalOutcome::Advisory { .. }
    ));
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    assert!(record
        .premises
        .iter()
        .any(|premise| premise.fact == "correction:receipts=failed_before_passed_after"));

    // A receipt pair with the wrong outcome pattern, and one with a receipt
    // missing, each abstain and leave no admitting premise.
    let mut pattern = admitted();
    pattern.receipts[1].outcome = RevisionOutcome::Failed;
    let mut missing = admitted();
    missing.receipts.pop();
    for (code, binding) in [
        ("BINDING_OUTCOME_PATTERN_MISMATCH", pattern),
        ("BINDING_RECEIPT_UNEXECUTED", missing),
    ] {
        let batch = decide(&h, request(&schema, binding)).await.unwrap();
        let record = &batch.records.as_slice()[0];
        assert_eq!(record.resolution_kind, ResolutionKind::Abstention, "{code}");
        let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
            panic!("{code} abstains, got {:?}", record.outcome);
        };
        assert_eq!(reasons.as_slice()[0].code().as_str(), code);
        assert!(!record
            .premises
            .iter()
            .any(|premise| premise.fact.starts_with("binding:decision=")));
    }
}
