//! MEDULLA-DECISION-ENGINE-R059.1: the robust transfer estimate across model
//! backbones, served through `DecisionLog` `aggregate`. Only independent labels
//! of the declared skill enter a backbone group; every refusal is a registered
//! `transfer::REFUSALS` code; the read writes nothing.

use super::retrieval_tests::{clone_executed, clone_record, executed_template};
use super::*;
use medulla_decision_types::decision::statistical::log::OutcomeAggregate;
use medulla_decision_types::decision::transfer::{
    BackboneBinding, TransferEstimate, TransferRequest,
};
use medulla_decision_types::decision::PremiseClass;

fn skill() -> String {
    "c".repeat(64)
}

fn binding(backbone: &str, revision: &str) -> BackboneBinding {
    BackboneBinding {
        backbone_id: backbone.to_string(),
        backbone_revision: revision.to_string(),
        skill_key: skill(),
        task_distribution_id: "incident-triage".to_string(),
        task_distribution_revision: "1".to_string(),
        dataset_digest: "d".repeat(64),
        metric_id: "resolved".to_string(),
        metric_revision: "1".to_string(),
    }
}

async fn evaluate_bound(
    h: &Harness,
    who: &str,
    record_id: &str,
    binding: BackboneBinding,
    success: bool,
) -> Result<(), String> {
    let op = DecisionLogOp::Evaluate {
        tenant_id: TENANT.to_string(),
        evaluation: DecisionOutcomeEvaluation {
            record_id: record_id.to_string(),
            evaluation_id: format!("eval-{who}-{record_id}"),
            class: EvidenceClass::Observation,
            selected_agent: "retriever-agent".to_string(),
            lease_holder: "retriever-worker".to_string(),
            fidelity: OutcomeFidelity::ToolCalls,
            success: Some(success),
            backbone: Some(binding),
        },
    };
    decode::<StoredEvaluation>(log_op(h, who, op).await).map(|_| ())
}

/// `total` independently judged records under `backbone`, `successes` good.
async fn judged(
    h: &Harness,
    template: &DecisionLogEntry,
    backbone: &str,
    revision: &str,
    successes: usize,
    total: usize,
) {
    for i in 0..total {
        let id = format!("rec-{backbone}-{revision}-{i}");
        clone_executed(h, template, &id);
        evaluate_bound(
            h,
            "evaluator",
            &id,
            binding(backbone, revision),
            i < successes,
        )
        .await
        .unwrap();
    }
}

async fn transfer(
    h: &Harness,
    who: &str,
    tenant: &str,
    declared: &[&str],
) -> Result<TransferEstimate, String> {
    let op = DecisionLogOp::Aggregate {
        request: OutcomeAggregateRequest {
            tenant_id: tenant.to_string(),
            question_id: None,
            window: window(),
            attribution: None,
            transfer: Some(TransferRequest {
                skill_key: skill(),
                declared_backbones: BoundedVec::new(
                    declared.iter().map(|id| id.to_string()).collect(),
                )
                .unwrap(),
            }),
        },
    };
    let aggregate: OutcomeAggregate = decode(log_op(h, who, op).await)?;
    Ok(aggregate.transfer.expect("a requested transfer estimate"))
}

fn refusal_code(error: &str) -> &str {
    error.split(':').next().unwrap_or(error)
}

// spec: MEDULLA-DECISION-ENGINE-R059.1.3
#[tokio::test]
async fn aggregate_reports_robust_transfer_across_backbones() {
    let h = Harness::new().await;
    let template = executed_template(&h).await;
    judged(&h, &template, "alpha", "1", 27, 30).await;
    judged(&h, &template, "beta", "1", 15, 30).await;
    // A self-report by the decider on a third backbone is never a label.
    clone_record(&h, &template, "rec-self");
    evaluate_bound(&h, "decider", "rec-self", binding("gamma", "1"), true)
        .await
        .unwrap();
    // A thin backbone: three labels.
    judged(&h, &template, "delta", "1", 3, 3).await;

    let estimate = transfer(&h, "decider", TENANT, &["alpha", "beta"])
        .await
        .expect("estimate");
    let rows = estimate.per_backbone.as_slice();
    assert_eq!(rows.len(), 2, "every declared backbone reports");
    let alpha = rows.iter().find(|r| r.backbone_id == "alpha").unwrap();
    let beta = rows.iter().find(|r| r.backbone_id == "beta").unwrap();
    assert_eq!((alpha.trials, alpha.successes), (30, 27));
    assert_eq!((beta.trials, beta.successes), (30, 15));
    assert_eq!(estimate.min_backbone_id, "beta");
    assert_eq!(estimate.robust_rate, beta.rate);
    assert!(beta.rate.value < alpha.rate.value);
    let facts: Vec<&str> = estimate.premises.iter().map(|p| p.fact.as_str()).collect();
    assert!(facts.iter().any(|f| f.starts_with("transfer.robust_rate=")));
    assert!(facts.contains(&"transfer.min_backbone=beta"));
    assert!(facts.contains(&"knob:transfer.min_backbones=2"));
    assert!(facts.contains(&"knob:transfer.min_samples=30"));
    assert!(estimate
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("transfer."))
        .all(|p| p.class == PremiseClass::Observation));

    // The self-reported evaluation never enters a group.
    let missing = transfer(&h, "decider", TENANT, &["alpha", "gamma"])
        .await
        .unwrap_err();
    assert_eq!(refusal_code(&missing), "BACKBONE_MISSING", "{missing}");
    assert!(missing.contains("gamma"));
    // Too few declared backbones.
    let few = transfer(&h, "decider", TENANT, &["alpha"])
        .await
        .unwrap_err();
    assert_eq!(refusal_code(&few), "TRANSFER_TOO_FEW_BACKBONES", "{few}");
    // Too few labels on one backbone.
    let thin = transfer(&h, "decider", TENANT, &["alpha", "delta"])
        .await
        .unwrap_err();
    assert_eq!(refusal_code(&thin), "TRANSFER_SAMPLES_BELOW_MIN", "{thin}");
    // Another principal sees none of the committer-only records.
    let stranger = transfer(&h, "stranger", TENANT, &["alpha", "beta"])
        .await
        .unwrap_err();
    assert_eq!(refusal_code(&stranger), "BACKBONE_MISSING", "{stranger}");
    // Another tenant's read is refused.
    let other = transfer(&h, "decider", "tenant-other", &["alpha", "beta"])
        .await
        .unwrap_err();
    assert!(other.starts_with("ACCESS_DENIED"), "{other}");
    // A malformed binding never reaches the log.
    let mut bad = binding("alpha", "1");
    bad.skill_key = "NOT-A-DIGEST".to_string();
    let refused = evaluate_bound(&h, "evaluator", "rec-alpha-1-0", bad, true)
        .await
        .unwrap_err();
    assert!(refused.contains("skill_key"), "{refused}");
    // The read wrote nothing: the same request answers the same estimate.
    let again = transfer(&h, "decider", TENANT, &["alpha", "beta"])
        .await
        .unwrap();
    assert_eq!(again, estimate);

    // One backbone id under two revisions refuses.
    judged(&h, &template, "beta", "2", 3, 3).await;
    let duplicate = transfer(&h, "decider", TENANT, &["alpha", "beta"])
        .await
        .unwrap_err();
    assert_eq!(
        refusal_code(&duplicate),
        "BACKBONE_DUPLICATE",
        "{duplicate}"
    );
}

async fn evaluate_plain(
    h: &Harness,
    who: &str,
    record_id: &str,
    success: bool,
) -> Result<(), String> {
    let op = DecisionLogOp::Evaluate {
        tenant_id: TENANT.to_string(),
        evaluation: DecisionOutcomeEvaluation {
            record_id: record_id.to_string(),
            evaluation_id: format!("eval-{who}-{record_id}"),
            class: EvidenceClass::Observation,
            selected_agent: "retriever-agent".to_string(),
            lease_holder: "retriever-worker".to_string(),
            fidelity: OutcomeFidelity::ToolCalls,
            success: Some(success),
            backbone: None,
        },
    };
    decode::<StoredEvaluation>(log_op(h, who, op).await).map(|_| ())
}

async fn skill_rows(
    h: &Harness,
    who: &str,
    tenant: &str,
) -> Result<Vec<medulla_decision_types::decision::transfer::SkillCandidateRow>, String> {
    let op = DecisionLogOp::Aggregate {
        request: OutcomeAggregateRequest {
            tenant_id: tenant.to_string(),
            question_id: None,
            window: window(),
            attribution: None,
            transfer: None,
        },
    };
    let aggregate: OutcomeAggregate = decode(log_op(h, who, op).await)?;
    Ok(aggregate.skill_candidates.as_slice().to_vec())
}

// spec: MEDULLA-DECISION-ENGINE-R059.2
#[tokio::test]
async fn three_independent_successes_make_a_skill_candidate() {
    // Self-reported successes never count: the decider judging its own records.
    let selfish = Harness::new().await;
    let template = executed_template(&selfish).await;
    for i in 0..3 {
        let id = format!("rec-self-{i}");
        clone_record(&selfish, &template, &id);
        evaluate_plain(&selfish, "decider", &id, true)
            .await
            .unwrap();
    }
    assert!(skill_rows(&selfish, "decider", TENANT)
        .await
        .unwrap()
        .is_empty());

    let h = Harness::new().await;
    let template = executed_template(&h).await;
    // Two independent successes are below knob transfer.skill_after_successes.
    for i in 0..2 {
        let id = format!("rec-{i}");
        clone_executed(&h, &template, &id);
        evaluate_plain(&h, "evaluator", &id, true).await.unwrap();
    }
    assert!(skill_rows(&h, "decider", TENANT).await.unwrap().is_empty());
    // A third independent success makes the option a candidate; a failure does not count.
    clone_executed(&h, &template, "rec-2");
    evaluate_plain(&h, "evaluator", "rec-2", true)
        .await
        .unwrap();
    clone_executed(&h, &template, "rec-fail");
    evaluate_plain(&h, "evaluator", "rec-fail", false)
        .await
        .unwrap();
    let rows = skill_rows(&h, "decider", TENANT).await.unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row.option_id, "plan-hyde");
    assert_eq!(row.question_id, template.record.question.question_id);
    assert_eq!((row.successes, row.distinct_records), (3, 3));
    assert_eq!(row.premise.fact, "transfer.skill_candidate:plan-hyde");
    assert_eq!(row.premise.class, PremiseClass::Observation);
    // Another principal sees none of the committer-only records.
    assert!(skill_rows(&h, "stranger", TENANT).await.unwrap().is_empty());
    // Another tenant's read is refused.
    let other = skill_rows(&h, "decider", "tenant-other").await.unwrap_err();
    assert!(other.starts_with("ACCESS_DENIED"), "{other}");
    // The read wrote nothing: the same request answers the same rows.
    assert_eq!(skill_rows(&h, "decider", TENANT).await.unwrap(), rows);
}
