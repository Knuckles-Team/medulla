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
    DefinitionKind, GraderKind, ReceiptKind, RevisionSubject, PROMOTION_UNEVALUATED,
};
use medulla_decision_outcome_types::policy_evolution::{
    EvaluationMetrics, EvaluationVerdict, PolicyEvaluation, PolicyEvolutionRecord, SafetyOutcome,
    StoredPolicyRecord,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::{Digest256, OpaqueId};
use medulla_decision_stats::decision::promotion::{paired_rollout_evidence, rollout_set_digest};
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
pub(super) fn rollout_set(gain_ppm: u32) -> RolloutSet {
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

pub(super) fn promotion_request(
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

pub(super) async fn commit(h: &Harness, record: &StatisticalDecisionRecord) -> DecisionLogCommitted {
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

/// A graph view holding `nodes`, each a JSON object stored as the node's property blob.
fn view_of(nodes: Vec<(String, serde_json::Value)>) -> GraphView {
    let mut view = GraphView::default();
    for (id, properties) in nodes {
        view.overlay_add_node(id, rmp_serde::to_vec_named(&properties).unwrap());
    }
    view
}

/// A policy evaluation of `subject` that passed its safety policy, stored as the one row shape
/// the kernel writes: `(record id, row)`.
fn policy_row(subject: &RevisionSubject, tenant: &str) -> (String, serde_json::Value) {
    let record = PolicyEvolutionRecord::PolicyEvaluation {
        record: PolicyEvaluation {
            subject: subject.clone(),
            revision_ref: OpaqueId::new("polver:policy-v2").unwrap(),
            champion_revision_ref: Some(OpaqueId::new("polver:base").unwrap()),
            evaluation_set_digest: Digest256::from_bytes([16; 32]),
            evaluator_digest: Digest256::from_bytes([17; 32]),
            evaluator_kind: GraderKind::Annotation,
            held_out_trajectory_ids: medulla_types::contract::BoundedVec::new(vec![
                OpaqueId::new("poltraj:held-1").unwrap(),
                OpaqueId::new("poltraj:held-2").unwrap(),
            ])
            .unwrap(),
            metrics: EvaluationMetrics {
                task_success_ppm: 900_000,
                baseline_task_success_ppm: 850_000,
                cost_micros: 12,
                latency_p50_ms: 4,
                latency_p95_ms: 9,
                gpu_seconds: 2,
                trace_completeness_ppm: 1_000_000,
                unsupported_replay_mass_ppm: 0,
            },
            safety: SafetyOutcome::Passed,
            verdict: EvaluationVerdict::Accepted,
            report: None,
        },
    };
    let stored = StoredPolicyRecord {
        record_id: record.record_id(tenant).unwrap(),
        tenant_id: tenant.to_owned(),
        recorded_by: "actor:policy".to_owned(),
        recorded_at_ms: 42,
        record,
    };
    (
        stored.record_id.clone(),
        serde_json::Value::Object(stored.row().unwrap()),
    )
}

// spec: MEDULLA-DECISION-ENGINE-R805.9
// spec: MEDULLA-DECISION-ENGINE-R805.3
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
    let graph_sources = |tenant: &'static str, view: &'static GraphView| EvidenceSources {
        store: &h.store,
        tenant,
        graph: Some(view),
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

    // A program revision's receipt is its graph node's `held_out` evidence.
    let program_evidence = paired_rollout_evidence(&rollout_set(150_000), &policy);
    let knobs = RevisionSubject::KnobSet {
        policy_id: "policy-a".to_owned(),
    };
    let (policy_id, policy_node) = policy_row(&knobs, TENANT);
    let view: &'static GraphView = Box::leak(Box::new(view_of(vec![
        (
            "revision:two".to_owned(),
            serde_json::json!({
                "type": "ProgramRevision",
                "program_ref": PROGRAM,
                "held_out": program_evidence,
            }),
        ),
        (policy_id.clone(), policy_node),
    ])));
    let program =
        held_out_evidence(&graph_sources(TENANT, view), &subject(), "revision:two").unwrap();
    assert_eq!(program, program_evidence);
    // The revision belongs to its own program only.
    assert_eq!(
        held_out_evidence(
            &graph_sources(TENANT, view),
            &RevisionSubject::Program {
                program_ref: "program-b".to_owned()
            },
            "revision:two"
        ),
        Err(EvidenceError::Refused(PROMOTION_UNEVALUATED))
    );
    // A policy evaluation node resolves for the subject it evaluates, as policy evidence.
    let knob_set = held_out_evidence(&graph_sources(TENANT, view), &knobs, &policy_id).unwrap();
    assert_eq!(knob_set.receipt_kind, ReceiptKind::PolicyEvaluation);
    assert_eq!(
        (knob_set.challenger_score_ppm, knob_set.champion_score_ppm, knob_set.case_count),
        (900_000, Some(850_000), 2)
    );
    assert_eq!(knob_set.subject, knobs);
    // Another subject's, another tenant's, or an unknown record never resolves.
    assert_eq!(
        held_out_evidence(
            &graph_sources(TENANT, view),
            &RevisionSubject::KnobSet {
                policy_id: "policy-b".to_owned()
            },
            &policy_id
        ),
        Err(EvidenceError::Refused(PROMOTION_UNEVALUATED))
    );
    assert_eq!(
        held_out_evidence(&graph_sources(OTHER, view), &knobs, &policy_id),
        Err(EvidenceError::Refused(PROMOTION_UNEVALUATED))
    );

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

/// Verifies MEDULLA-DECISION-ENGINE-R805.3: every subject kind whose receipt is a rollout set
/// promotes through `Decide` kind `Promotion` and a committed record, rolls back to its previous
/// champion, and another tenant's pointer never honours the record.
// spec: MEDULLA-DECISION-ENGINE-R805.3
#[tokio::test]
async fn promote_then_roll_back_every_subject_kind() {
    use medulla_decision_types::decision::statistical::retrieval::PointerMovement;
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    let subjects = [
        subject(),
        RevisionSubject::Definition {
            definition_kind: DefinitionKind::Agent,
            definition_ref: "agent-a".to_owned(),
        },
        RevisionSubject::RetrievalStrategy {
            strategy_ref: "strategy-a".to_owned(),
        },
    ];
    for (index, subject) in subjects.iter().enumerate() {
        let champion = PointerRef::Champion {
            subject: subject.clone(),
        };
        let mut promoted = Vec::new();
        for generation in ["one", "two"] {
            let target = format!("revision:{generation}-{index}");
            let set = RolloutSet {
                challenger_ref: target.clone(),
                ..rollout_set(150_000)
            };
            let digest = rollout_set_digest(&set);
            learn(
                &h,
                "decider",
                LearningWrite::RecordRollouts {
                    set: Box::new(set),
                },
            )
            .await
            .unwrap();
            let record = decide(
                &h,
                promotion_request(&pins.0, subject, &digest, &target),
            )
            .await
            .unwrap()
            .records
            .as_slice()[0]
                .clone();
            let facts: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
            assert!(
                facts.contains(&"promotion.verdict=promote"),
                "{subject:?}: {facts:?}"
            );
            commit(&h, &record).await;
            let state = pointer_state(
                &h,
                champion.clone(),
                activate(&target, &record.record_id),
            )
            .await
            .unwrap();
            assert_eq!(state.target(), Some(target.as_str()), "{subject:?}");
            promoted.push((target, record.record_id));
        }

        // Another tenant holds neither the record nor the pointer: it refuses the move and
        // its seat stays empty.
        let other = context("decider", OTHER);
        let theirs: Result<LearningRecorded, String> = decode(
            super::super::log::handle_decision_log(
                &h.state,
                9,
                &other,
                DecisionLogOp::Learn {
                    tenant_id: OTHER.to_owned(),
                    write: LearningWrite::MovePointer {
                        pointer: champion.clone(),
                        movement: activate(&promoted[0].0, &promoted[0].1),
                    },
                },
            )
            .await,
        );
        assert!(
            theirs.unwrap_err().starts_with("PROMOTION_RECEIPT_MISMATCH"),
            "{subject:?}"
        );

        // Rolling back restores the previous champion.
        let back = pointer_state(&h, champion, PointerMovement::Rollback)
            .await
            .unwrap();
        assert_eq!(back.target(), Some(promoted[0].0.as_str()), "{subject:?}");
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R805.5: a promotion record seals the
/// subject, receipt, delta, case count and the champion it pinned as premises;
/// every pointer event carries the deciding record digest and its principal;
/// and replaying the history from the empty state reproduces the stored pointer
/// byte for byte.
// spec: MEDULLA-DECISION-ENGINE-R805.5
#[tokio::test]
async fn pointer_history_replays() {
    use super::super::stat_pointer::{pointer_events, read_pointer};
    use medulla_decision_types::decision::statistical::retrieval::PointerMovement;
    use medulla_decision_types::decision::statistical::retrieval_pointer::{
        PointerEvent, PointerState, PointerTransition,
    };

    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    let subject = subject();
    let champion = PointerRef::Champion {
        subject: subject.clone(),
    };
    let mut promoted: Vec<(String, String)> = Vec::new();
    for generation in ["one", "two"] {
        let target = format!("revision:{generation}");
        let set = RolloutSet {
            challenger_ref: target.clone(),
            ..rollout_set(150_000)
        };
        let digest = rollout_set_digest(&set);
        learn(
            &h,
            "decider",
            LearningWrite::RecordRollouts { set: Box::new(set) },
        )
        .await
        .unwrap();
        let record = decide(&h, promotion_request(&pins.0, &subject, &digest, &target))
            .await
            .unwrap()
            .records
            .as_slice()[0]
            .clone();
        let facts: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
        let pinned = promoted.last().map_or("none", |(target, _)| target.as_str());
        for expected in [
            format!("promotion.subject={}", subject.pointer_key()),
            format!("promotion.receipt_digest={digest}"),
            format!("promotion.champion_at_decision={pinned}"),
        ] {
            assert!(facts.contains(&expected.as_str()), "{expected} in {facts:?}");
        }
        assert!(
            facts.iter().any(|fact| fact.starts_with("promotion.delta_ppm=")),
            "{facts:?}"
        );
        assert!(
            facts
                .iter()
                .any(|fact| fact.starts_with("promotion.case_count=")),
            "{facts:?}"
        );
        commit(&h, &record).await;
        pointer_state(&h, champion.clone(), activate(&target, &record.record_id))
            .await
            .unwrap();
        promoted.push((target, record.record_id));
    }
    pointer_state(&h, champion, PointerMovement::Rollback)
        .await
        .unwrap();

    // The audit: two activations then the rollback, each naming the deciding
    // record and the principal that moved the pointer.
    let key = subject.pointer_key();
    let events: Vec<PointerEvent> = pointer_events(&h.store, TENANT)
        .unwrap()
        .into_iter()
        .filter(|(row_key, ..)| row_key == &key)
        .map(|(_, _, _, event)| event)
        .collect();
    assert_eq!(events.len(), 3, "{events:?}");
    for (event, (target, record_id)) in events.iter().zip(&promoted) {
        assert_eq!(event.transition, PointerTransition::Activated);
        assert_eq!(event.target.as_deref(), Some(target.as_str()));
        assert_eq!(event.receipt_digest.as_deref(), Some(record_id.as_str()));
        assert_eq!(event.principal, "decider");
    }
    assert_eq!(events[2].transition, PointerTransition::RolledBack);
    assert_eq!(events[2].target.as_deref(), Some(promoted[0].0.as_str()));
    assert_eq!(events[2].receipt_digest.as_deref(), Some(promoted[0].1.as_str()));

    // Replay from the empty state: an activation pushes the active event on the
    // stack, a rollback pops it.
    let mut active: Option<PointerEvent> = None;
    let mut stack: Vec<PointerEvent> = Vec::new();
    for event in &events {
        match event.transition {
            PointerTransition::Activated => {
                if let Some(previous) = active.take() {
                    stack.push(previous);
                }
                active = Some(event.clone());
            }
            PointerTransition::RolledBack => active = stack.pop().map(|_| event.clone()),
        }
    }
    let replayed = PointerState {
        key: key.clone(),
        active,
        stack: BoundedVec::new(stack).unwrap(),
        history: BoundedVec::new(events).unwrap(),
    };
    let (stored_bytes, stored) = read_pointer(&h.store, TENANT, &key).unwrap();
    assert_eq!(replayed, stored);
    assert_eq!(
        encode_artifact(&replayed).unwrap(),
        stored_bytes.expect("the pointer is stored")
    );
}
