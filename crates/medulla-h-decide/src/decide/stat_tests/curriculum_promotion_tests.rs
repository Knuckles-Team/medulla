//! Curriculum promotion served through `Decide` (MEDULLA-DECISION-ENGINE-R751.754):
//! the stage's latest independently evaluated outcomes enter through
//! `DecisionLog` `Evaluate`, and `Decide` promotes only a cleared full window
//! and never counts another tenant's outcomes.

use super::kernel_family_support::{decide_as_other_tenant, facts, kernel_request};
use super::path_preference_tests::{pins, run, seed, OTHER};
use super::*;
use medulla_decision_types::decision::curriculum::{
    CurriculumRequest, CurriculumStage, PromotionPolicy, PromotionVerdict,
};
use medulla_decision_types::decision::numeric::{QuantScaleTag, QuantisedValue};
use medulla_decision_types::decision::ResolutionKind;

fn request(schema: &ComponentDependency, stage_id: &str) -> DecideRequest {
    kernel_request(
        schema,
        QuestionKind::CurriculumPromotion,
        "curriculum_promotion",
        TypedValue::CurriculumPromotion(Box::new(CurriculumRequest {
            judged_stage: CurriculumStage {
                stage_id: stage_id.to_string(),
                horizon_steps: 8,
                window_runs: 20,
            },
            policy: PromotionPolicy {
                promote_threshold: QuantisedValue {
                    scale: QuantScaleTag::Q32,
                    value: 3 << 30,
                },
                next_stage_id: "long".to_string(),
            },
        })),
        &[stage_id],
    )
}

async fn verdict(
    h: &Harness,
    schema: &ComponentDependency,
    stage_id: &str,
) -> (StatisticalDecisionRecord, PromotionVerdict, (u32, u32)) {
    let batch = decide(h, request(schema, stage_id)).await.unwrap();
    let record = batch.records.as_slice()[0].clone();
    let reading = record
        .answer
        .as_ref()
        .and_then(|a| a.curriculum_promotion())
        .expect("the family answers a promotion reading")
        .clone();
    (record, reading.verdict, (reading.successes, reading.runs))
}

/// Verifies MEDULLA-DECISION-ENGINE-R751.754: a full window at the threshold
/// promotes, a window below it holds, and a window that is not full holds.
// spec: MEDULLA-DECISION-ENGINE-R751.754
#[tokio::test]
async fn served_decide_promotes_on_a_cleared_window() {
    let h = Harness::new().await;
    let pins = pins(&h, TENANT);
    // Twenty runs, fifteen successes: exactly the 3/4 threshold.
    let mut cleared = run(0, 5);
    cleared.extend(run(15, 0));
    seed(&h, TENANT, &pins, "stage-cleared", &cleared).await;
    seed(&h, TENANT, &pins, "stage-below", &run(14, 6)).await;
    seed(&h, TENANT, &pins, "stage-short", &run(10, 0)).await;

    let (record, verdict_cleared, rate) = verdict(&h, &pins.0, "stage-cleared").await;
    assert_eq!(record.question.kind, QuestionKind::CurriculumPromotion);
    assert!(matches!(
        record.outcome,
        StatisticalOutcome::Advisory { .. }
    ));
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(record.evidence_class, EvidenceClass::Observation);
    assert_eq!(
        verdict_cleared,
        PromotionVerdict::Promote {
            next_stage_id: "long".to_string()
        }
    );
    assert_eq!(rate, (15, 20));
    let facts = facts(&record);
    assert!(
        facts.contains(&"curriculum:stage=stage-cleared"),
        "{facts:?}"
    );
    assert!(facts.contains(&"curriculum:rate=15/20"), "{facts:?}");
    assert!(
        facts.iter().any(|f| f.starts_with("curriculum:threshold=")),
        "{facts:?}"
    );

    let (_, below, rate) = verdict(&h, &pins.0, "stage-below").await;
    assert_eq!((below, rate), (PromotionVerdict::Hold, (14, 20)));
    let (_, short, rate) = verdict(&h, &pins.0, "stage-short").await;
    assert_eq!((short, rate), (PromotionVerdict::Hold, (10, 10)));

    // The declared candidate must be the stage; another tenant is refused.
    let mut wrong = request(&pins.0, "stage-cleared");
    wrong.candidates = request(&pins.0, "stage-other").candidates;
    assert!(decide(&h, wrong)
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));
    let error = decide_as_other_tenant(&h, request(&pins.0, "stage-cleared")).await;
    // The served tenant boundary refuses a request naming a tenant the
    // caller does not belong to, before any pin is read.
    assert!(error.starts_with("ACCESS_DENIED"), "{error}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R751.754: another tenant's outcomes never
/// count toward a stage's window.
// spec: MEDULLA-DECISION-ENGINE-R751.754
#[tokio::test]
async fn other_tenant_outcomes_never_count() {
    let h = Harness::new().await;
    let mine = pins(&h, TENANT);
    let theirs = pins(&h, OTHER);
    seed(&h, TENANT, &mine, "stage-a", &run(5, 0)).await;
    seed(&h, OTHER, &theirs, "stage-a", &run(20, 0)).await;
    let (_, verdict_mine, rate) = verdict(&h, &mine.0, "stage-a").await;
    assert_eq!(
        (verdict_mine, rate),
        (PromotionVerdict::Hold, (5, 5)),
        "the other tenant's twenty successes are invisible here"
    );
}
