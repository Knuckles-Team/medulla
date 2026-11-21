//! Promotion served through `Decide` and `DecisionLog` (MEDULLA-DECISION-ENGINE-R805.9,
//! R805.3, R654.3): receipts resolve into held-out evidence, a `Promotion`
//! decision judges it, and a champion pointer moves only on a committed
//! `promote` record of the same subject and target.

use super::kernel_family_support::kernel_request;
use super::learning_tests::{activate, pointer_state};
use super::path_preference_tests::{pins, OTHER};
use super::retrieval_tests::learn;
use super::*;
use medulla_decision_outcome_types::decision::promotion::{
    ReceiptKind, RevisionSubject, PROMOTION_UNEVALUATED,
};
use medulla_decision_stats::decision::promotion::rollout_set_digest;
use medulla_decision_types::decision::skill_evolution::{
    Rollout, RolloutSet, ROLLOUT_SET_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::log::{DecisionLogCommitted, DecisionLogOp};
use medulla_decision_types::decision::statistical::retrieval::{
    LearningRecorded, LearningWrite, PointerRef,
};
use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
use medulla_decision_types::decision::{
    CalibrationMethod, CalibrationStatement, DecisionEvalReceipt, DecisionPolicy, QuantScaleTag,
    QuantisedValue, UnitRationalWire,
};
use medulla_decision_types::decision::jobs::{FullLabelMetrics, LabelExclusions};
use medulla_persistence_records::decision_jobs::{encode_artifact, receipt_key};

use super::super::stat_promotion::{
    held_out_evidence, rollouts_key, EvidenceError, EvidenceSources,
};

const PROGRAM: &str = "program-a";

fn subject() -> RevisionSubject {
    RevisionSubject::Program {
        program_ref: PROGRAM.to_owned(),
    }
}

/// Sixty paired rollouts: the challenger beats the champion by `gain_ppm` on each.
fn rollout_set(gain_ppm: u32) -> RolloutSet {
    let rollouts = (0..60u64)
        .map(|index| Rollout {
            seed: index % 3,
            task_id: format!("task-{index}"),
            challenger_score_ppm: Some(600_000 + gain_ppm),
            champion_score_ppm: Some(600_000),
            trace_ref: format!("trace-{index}"),
        })
        .collect();
    RolloutSet {
        schema_version: ROLLOUT_SET_SCHEMA_VERSION,
        program_ref: PROGRAM.to_owned(),
        challenger_ref: "revision:two".to_owned(),
        champion_ref: Some("revision:one".to_owned()),
        rollouts: BoundedVec::new(rollouts).unwrap(),
    }
}

fn ratio(numerator: u64, denominator: u64) -> UnitRationalWire {
    UnitRationalWire::new(numerator, denominator).unwrap()
}

fn eval_receipt() -> DecisionEvalReceipt {
    let zero = QuantisedValue {
        scale: QuantScaleTag::Q32,
        value: 0,
    };
    DecisionEvalReceipt {
        receipt_digest: "sha256:receipt".into(),
        head_digest: "sha256:head".into(),
        policy_digest: "sha256:policy".into(),
        n_records: 100,
        estimates: BoundedVec::default(),
        calibration: Some(CalibrationStatement {
            method: CalibrationMethod::Conformal,
            alpha: Some(ratio(1, 10)),
            coverage_lower: Some(ratio(9, 10)),
            coverage_upper: Some(ratio(1, 1)),
            n_calibration: 100,
            synthetic: false,
        }),
        metrics: Some(FullLabelMetrics {
            n_items: 100,
            top1_hits: 95,
            log_loss: zero,
            brier: zero,
            expected_calibration_error: zero,
            covered: 95,
            coverage_lower: ratio(9, 10),
            coverage_upper: ratio(1, 1),
            acted: 100,
            acted_wrong: 1,
            act_risk_upper: ratio(1, 20),
            set_size_total: 100,
        }),
        promotion: None,
        exclusions: LabelExclusions::default(),
        pooled: BoundedVec::default(),
        failed_gates: BoundedVec::default(),
        feature_power: BoundedVec::default(),
        premises: BoundedVec::default(),
        passed: true,
        synthetic: false,
        gold_guarantee: None,
    }
}

fn promotion_request(
    schema: &ComponentDependency,
    subject: &RevisionSubject,
    receipt: &str,
    option: &str,
) -> DecideRequest {
    let text = |name: &str, value: String| TypedParam {
        name: name.to_owned(),
        value: TypedValue::Text(value),
    };
    let mut request = kernel_request(
        schema,
        QuestionKind::Promotion,
        "subject",
        TypedValue::Text(String::new()),
        &[option],
    );
    request.params = BoundedVec::new(vec![
        text("mode", "promote".to_owned()),
        text("receipt_digest", receipt.to_owned()),
        text("subject", serde_json::to_string(subject).unwrap()),
    ])
    .unwrap();
    request
}

async fn commit(h: &Harness, record: &StatisticalDecisionRecord) -> DecisionLogCommitted {
    let decider = context("decider", TENANT);
    decode(
        super::super::log::handle_decision_log(
            &h.state,
            9,
            &decider,
            DecisionLogOp::Commit {
                record: Box::new(record.clone()),
                evaluator: None,
                approval: None,
            },
        )
        .await,
    )
    .unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R805.9
#[tokio::test]
async fn every_receipt_kind_resolves_to_held_out_evidence() {
    let h = Harness::new().await;
    let policy = DecisionPolicy::engine_default();
    let sources = |tenant: &'static str| EvidenceSources {
        store: &h.store,
        tenant,
        graph: None,
        policy: &policy,
    };
    let head = RevisionSubject::DecisionHead {
        component_id: "head-a".to_owned(),
    };
    h.store
        .put_decision_artifacts(
            TENANT,
            &[(
                receipt_key("sha256:receipt"),
                encode_artifact(&eval_receipt()).unwrap(),
            )],
        )
        .unwrap();
    let evidence = held_out_evidence(&sources(TENANT), &head, "sha256:receipt").unwrap();
    assert_eq!(evidence.receipt_kind, ReceiptKind::DecisionEval);
    assert_eq!((evidence.challenger_score_ppm, evidence.case_count), (950_000, 100));
    assert!(evidence.independent && evidence.champion_score_ppm.is_none());

    let set = rollout_set(150_000);
    let digest = rollout_set_digest(&set);
    h.store
        .put_decision_artifacts(
            TENANT,
            &[(rollouts_key(&digest), encode_artifact(&set).unwrap())],
        )
        .unwrap();
    let rollouts = held_out_evidence(&sources(TENANT), &subject(), &digest).unwrap();
    assert_eq!(rollouts.receipt_kind, ReceiptKind::RolloutSet);
    assert_eq!((rollouts.case_count, rollouts.delta_ppm), (60, 150_000));

    // Program and policy receipts live in graph nodes; with none visible they are unevaluated.
    for absent in [
        (subject(), "revision:absent"),
        (
            RevisionSubject::KnobSet {
                policy_id: "policy-a".to_owned(),
            },
            "polev:absent",
        ),
    ] {
        assert_eq!(
            held_out_evidence(&sources(TENANT), &absent.0, absent.1),
            Err(EvidenceError::Refused(PROMOTION_UNEVALUATED))
        );
    }

    // Another tenant's receipts resolve as absent.
    assert_eq!(
        held_out_evidence(&sources(OTHER), &head, "sha256:receipt"),
        Err(EvidenceError::Refused(PROMOTION_UNEVALUATED))
    );
    assert_eq!(
        held_out_evidence(&sources(OTHER), &subject(), &digest),
        Err(EvidenceError::Refused(PROMOTION_UNEVALUATED))
    );
}

// spec: MEDULLA-DECISION-ENGINE-R654.3
#[tokio::test]
async fn record_rollouts_stores_the_set_and_answers_its_evidence() {
    let h = Harness::new().await;
    let set = rollout_set(150_000);
    let digest = rollout_set_digest(&set);
    let recorded = learn(
        &h,
        "decider",
        LearningWrite::RecordRollouts {
            set: Box::new(set.clone()),
        },
    )
    .await
    .unwrap();
    let LearningRecorded::Rollouts(evidence) = recorded else {
        panic!("rollouts answer their evidence");
    };
    assert_eq!(evidence.receipt_kind, ReceiptKind::RolloutSet);
    assert_eq!(evidence.receipt_digest, digest);
    assert!(h
        .store
        .decision_artifact(TENANT, &rollouts_key(&digest))
        .unwrap()
        .is_some());
    // The same set replays; another tenant has no such set.
    let replay = learn(
        &h,
        "decider",
        LearningWrite::RecordRollouts { set: Box::new(set) },
    )
    .await
    .unwrap();
    assert_eq!(replay, LearningRecorded::Rollouts(evidence));
    assert!(h
        .store
        .decision_artifact(OTHER, &rollouts_key(&digest))
        .unwrap()
        .is_none());
}

// spec: MEDULLA-DECISION-ENGINE-R654.3
// spec: MEDULLA-DECISION-ENGINE-R805.3
#[tokio::test]
async fn paired_rollouts_promote_a_skill_revision() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    let champion = PointerRef::Champion { subject: subject() };
    let winning = rollout_set(150_000);
    let losing = RolloutSet {
        challenger_ref: "revision:three".to_owned(),
        ..rollout_set(5_000)
    };
    let mut digests = Vec::new();
    for set in [&winning, &losing] {
        digests.push(rollout_set_digest(set));
        learn(
            &h,
            "decider",
            LearningWrite::RecordRollouts {
                set: Box::new(set.clone()),
            },
        )
        .await
        .unwrap();
    }

    // The winner promotes; its committed record qualifies the pointer move.
    let batch = decide(
        &h,
        promotion_request(&pins.0, &subject(), &digests[0], "revision:two"),
    )
    .await
    .unwrap();
    let record = batch.records.as_slice()[0].clone();
    assert!(matches!(record.outcome, StatisticalOutcome::Advisory { .. }));
    let facts: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
    assert!(facts.contains(&"promotion.verdict=promote"), "{facts:?}");
    commit(&h, &record).await;
    let state = pointer_state(
        &h,
        champion.clone(),
        activate("revision:two", &record.record_id),
    )
    .await
    .unwrap();
    assert_eq!(state.target(), Some("revision:two"));

    // A hold does not qualify a move, and a promote record names its own target.
    let held = decide(
        &h,
        promotion_request(&pins.0, &subject(), &digests[1], "revision:three"),
    )
    .await
    .unwrap()
    .records
    .as_slice()[0]
        .clone();
    commit(&h, &held).await;
    let refused = pointer_state(
        &h,
        champion.clone(),
        activate("revision:three", &held.record_id),
    )
    .await
    .unwrap_err();
    assert!(refused.starts_with("PROMOTION_RECEIPT_MISMATCH"), "{refused}");
    let wrong_target = pointer_state(&h, champion, activate("revision:other", &record.record_id))
        .await
        .unwrap_err();
    assert!(
        wrong_target.starts_with("PROMOTION_RECEIPT_MISMATCH"),
        "{wrong_target}"
    );
}

// spec: MEDULLA-DECISION-ENGINE-R805.4
#[tokio::test]
async fn promotion_refusals() {
    use medulla_decision_types::decision::statistical::retrieval::PointerMovement;
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    let champion = PointerRef::Champion { subject: subject() };
    let first = rollout_set(150_000);
    let second = RolloutSet {
        challenger_ref: "revision:four".to_owned(),
        ..rollout_set(160_000)
    };
    let mut records = Vec::new();
    for set in [&first, &second] {
        learn(
            &h,
            "decider",
            LearningWrite::RecordRollouts {
                set: Box::new(set.clone()),
            },
        )
        .await
        .unwrap();
        // Both are decided while the seat is empty.
        let record = decide(
            &h,
            promotion_request(&pins.0, &subject(), &rollout_set_digest(set), &set.challenger_ref),
        )
        .await
        .unwrap()
        .records
        .as_slice()[0]
            .clone();
        commit(&h, &record).await;
        records.push(record);
    }
    // An unknown record is not a promote verdict.
    let unknown = pointer_state(&h, champion.clone(), activate("revision:two", "absent"))
        .await
        .unwrap_err();
    assert!(unknown.starts_with("PROMOTION_RECEIPT_MISMATCH"), "{unknown}");
    // The first seat taker moves; the second's pinned champion is gone.
    let moved = pointer_state(&h, champion.clone(), activate("revision:two", &records[0].record_id))
        .await
        .unwrap();
    let conflict = pointer_state(&h, champion.clone(), activate("revision:four", &records[1].record_id))
        .await
        .unwrap_err();
    assert!(conflict.starts_with("PROMOTION_CAS_CONFLICT"), "{conflict}");
    // With no earlier champion a rollback has nowhere to go; the pointer stays.
    let no_parent = pointer_state(&h, champion.clone(), PointerMovement::Rollback)
        .await
        .unwrap_err();
    assert!(no_parent.starts_with("ROLLBACK_NO_PARENT"), "{no_parent}");
    let unchanged = pointer_state(&h, champion, activate("revision:two", &records[0].record_id))
        .await
        .unwrap();
    assert_eq!(unchanged, moved);
}
