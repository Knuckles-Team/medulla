//! MEDULLA-DECISION-ENGINE-R134.102: a served `DelegationAdmission` question
//! admits delegation only over a closed, single-revision, fresh and
//! sufficiently large window whose pooled independent success rate reaches the
//! threshold. Each refusal abstains with its typed code.
//!
//! It runs from the served log journey in `stat_tests.rs`, which supplies one
//! real executed record; every seeded record is a copy of it under a task
//! class, a policy digest and an evaluation set of the case.

use super::*;
use medulla_decision_types::decision::delegation::{DelegationAdmissionRequest, Q32_ONE};
use medulla_decision_types::decision::statistical::dataset::OutcomeFidelity;
use medulla_decision_types::decision::statistical::log::{
    DecisionLogEntry, DecisionOutcomeEvaluation, EntryInputs, RecordVisibility, StoredEvaluation,
    DECISION_LOG_SCHEMA_VERSION,
};
use medulla_decision_types::decision::EvidenceClass;
use medulla_persistence_records::decision_jobs::{encode_artifact, evaluation_key, record_key};

const DAY_MS: u64 = 86_400_000;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// One seeded record and its evaluations: `(evaluator, success)` each.
struct Seed<'a> {
    class: &'a str,
    policy_digest: &'a str,
    created_at_ms: u64,
    recorded_at_ms: u64,
    evaluations: Vec<(&'a str, Option<bool>)>,
}

fn write(
    h: &Harness,
    tenant: &str,
    template: &StatisticalDecisionRecord,
    index: usize,
    seed: &Seed,
) {
    let mut record = template.clone();
    record.record_id = format!("{}-{}-{index}", seed.class, seed.policy_digest);
    record.question.question_id = seed.class.to_string();
    record.caller_principal = "decider".to_string();
    record.synthetic_evidence = false;
    record.created_at_ms = seed.created_at_ms;
    record.inputs.policy_digest = seed.policy_digest.to_string();
    let entry = DecisionLogEntry {
        schema_version: DECISION_LOG_SCHEMA_VERSION,
        record: Box::new(record.clone()),
        committed_by: "decider".to_string(),
        committed_at_ms: seed.created_at_ms,
        visibility: RecordVisibility::Tenant,
        inputs: EntryInputs::Inline,
    };
    let mut rows = vec![(
        record_key(&record.record_id),
        encode_artifact(&entry).unwrap(),
    )];
    for (n, (producer, success)) in seed.evaluations.iter().enumerate() {
        let evaluation_id = format!("evaluation-{n}");
        let stored = StoredEvaluation {
            evaluation: DecisionOutcomeEvaluation {
                record_id: record.record_id.clone(),
                evaluation_id: evaluation_id.clone(),
                class: EvidenceClass::Observation,
                selected_agent: "agent-a".to_string(),
                lease_holder: "worker-a".to_string(),
                fidelity: OutcomeFidelity::ToolCalls,
                success: *success,
                backbone: None,
            },
            producer: producer.to_string(),
            recorded_at_ms: seed.recorded_at_ms,
        };
        rows.push((
            evaluation_key(&record.record_id, &evaluation_id),
            encode_artifact(&stored).unwrap(),
        ));
    }
    h.store.put_decision_artifacts(tenant, &rows).unwrap();
}

/// `count` records of `class`, the first `successes` evaluated as successes by
/// an independent evaluator.
fn seed_class(
    h: &Harness,
    template: &StatisticalDecisionRecord,
    class: &str,
    policy: &str,
    count: usize,
    successes: usize,
    age_days: u64,
) {
    let now = now_ms();
    for index in 0..count {
        write(
            h,
            TENANT,
            template,
            index,
            &Seed {
                class,
                policy_digest: policy,
                created_at_ms: now - 3 * DAY_MS,
                recorded_at_ms: now - age_days * DAY_MS,
                evaluations: vec![("evaluator", Some(index < successes))],
            },
        );
    }
}

fn admission(h: &Harness, class: &str, threshold: f64, window_end_ms: u64) -> DecideRequest {
    let (schema, _) = publish_route_schema(h);
    declared_request(
        &schema,
        TENANT,
        QuestionKind::DelegationAdmission,
        "admit",
        DecisionPolicyRef::Default,
        vec![TypedParam {
            name: "delegation".to_string(),
            value: TypedValue::DelegationAdmission(DelegationAdmissionRequest {
                task_class: class.to_string(),
                metric_id: "success".to_string(),
                threshold_q32: (threshold * Q32_ONE as f64) as u64,
                window_start_ms: now_ms() - 10 * DAY_MS,
                window_end_ms,
            }),
        }],
    )
}

async fn verdict(
    h: &Harness,
    class: &str,
    threshold: f64,
    window_end_ms: u64,
) -> StatisticalDecisionRecord {
    decide(h, admission(h, class, threshold, window_end_ms))
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone()
}

fn closed() -> u64 {
    now_ms() - DAY_MS
}

fn digest_of(record: &StatisticalDecisionRecord) -> String {
    let fact = record
        .premises
        .iter()
        .find(|p| p.fact.starts_with("delegation.verdict:"))
        .unwrap_or_else(|| panic!("no verdict premise: {:?}", record.outcome));
    fact.fact.rsplit("digest=").next().unwrap().to_string()
}

fn refused(record: &StatisticalDecisionRecord, code: &str) {
    assert!(
        matches!(record.outcome, StatisticalOutcome::Abstained { .. }),
        "{code}: {:?}",
        record.outcome
    );
    assert!(
        format!("{:?}", record.outcome).contains(code),
        "{:?}",
        record.outcome
    );
}

// spec: MEDULLA-DECISION-ENGINE-R134.102
pub(super) async fn window_matrix_admits_or_refuses(
    h: &Harness,
    template: &StatisticalDecisionRecord,
) {
    // A complete, closed, single-revision, fresh window above the default 0.8.
    seed_class(h, template, "c-admit", "p1", 30, 30, 2);
    let admitted = verdict(h, "c-admit", 0.0, closed()).await;
    assert!(matches!(
        admitted.outcome,
        StatisticalOutcome::Advisory { .. }
    ));
    let base = digest_of(&admitted);
    assert!(admitted
        .premises
        .iter()
        .any(|p| p.fact == "delegation.pooled:successes=30;samples=30"
            && p.class == medulla_decision_types::decision::PremiseClass::Observation));

    // An open window abstains.
    let open = verdict(h, "c-admit", 0.0, now_ms() + 3_600_000).await;
    refused(&open, "DELEGATION_WINDOW_OPEN");

    // Under min_samples (29 of 30) abstains; another tenant's records never count.
    seed_class(h, template, "c-few", "p1", 29, 29, 2);
    for index in 0..5 {
        write(
            h,
            "tenant-other",
            template,
            index,
            &Seed {
                class: "c-few",
                policy_digest: "p1",
                created_at_ms: now_ms() - 3 * DAY_MS,
                recorded_at_ms: now_ms() - 2 * DAY_MS,
                evaluations: vec![("evaluator", Some(true))],
            },
        );
    }
    refused(
        &verdict(h, "c-few", 0.0, closed()).await,
        "DELEGATION_INSUFFICIENT_SAMPLES",
    );

    // Below the default threshold (20 of 30) abstains.
    seed_class(h, template, "c-below", "p1", 30, 20, 2);
    refused(
        &verdict(h, "c-below", 0.0, closed()).await,
        "DELEGATION_BELOW_THRESHOLD",
    );

    // A request threshold may only raise: 27 of 30 admits at the knob, refuses at 0.95,
    // and a lower request value changes nothing.
    seed_class(h, template, "c-90", "p1", 30, 27, 2);
    assert!(matches!(
        verdict(h, "c-90", 0.1, closed()).await.outcome,
        StatisticalOutcome::Advisory { .. }
    ));
    refused(
        &verdict(h, "c-90", 0.95, closed()).await,
        "DELEGATION_BELOW_THRESHOLD",
    );
    refused(
        &verdict(h, "c-below", 0.1, closed()).await,
        "DELEGATION_BELOW_THRESHOLD",
    );

    // Two policy revisions in the window abstain.
    seed_class(h, template, "c-mixed", "p1", 29, 29, 2);
    write(
        h,
        TENANT,
        template,
        99,
        &Seed {
            class: "c-mixed",
            policy_digest: "p2",
            created_at_ms: now_ms() - 3 * DAY_MS,
            recorded_at_ms: now_ms() - 2 * DAY_MS,
            evaluations: vec![("evaluator", Some(true))],
        },
    );
    refused(
        &verdict(h, "c-mixed", 0.0, closed()).await,
        "DELEGATION_MIXED_REVISIONS",
    );

    // The newest outcome older than delegation.max_age (7 days) abstains.
    seed_class(h, template, "c-stale", "p1", 30, 30, 8);
    refused(
        &verdict(h, "c-stale", 0.0, closed()).await,
        "DELEGATION_STALE_EVIDENCE",
    );

    // An evaluation by the principal that decided the record and a censored
    // evaluation never count: the class stays at 29.
    seed_class(h, template, "c-own", "p1", 29, 29, 2);
    write(
        h,
        TENANT,
        template,
        98,
        &Seed {
            class: "c-own",
            policy_digest: "p1",
            created_at_ms: now_ms() - 3 * DAY_MS,
            recorded_at_ms: now_ms() - 2 * DAY_MS,
            evaluations: vec![("decider", Some(true)), ("evaluator", None)],
        },
    );
    refused(
        &verdict(h, "c-own", 0.0, closed()).await,
        "DELEGATION_INSUFFICIENT_SAMPLES",
    );

    // Changing the threshold, a sample or the policy revision changes the verdict digest.
    let raised = digest_of(&verdict(h, "c-admit", 0.85, closed()).await);
    assert_ne!(raised, base, "the bound threshold is part of the digest");
    seed_class(h, template, "c-admit-p2", "p2", 30, 30, 2);
    assert_ne!(
        digest_of(&verdict(h, "c-admit-p2", 0.0, closed()).await),
        base,
        "the policy revision is part of the digest"
    );
    seed_class(h, template, "c-flip", "p1", 30, 29, 2);
    assert_ne!(
        digest_of(&verdict(h, "c-flip", 0.0, closed()).await),
        base,
        "the outcome set is part of the digest"
    );
    assert_eq!(
        digest_of(&verdict(h, "c-admit", 0.0, closed()).await),
        base,
        "a verdict is deterministic"
    );
}
