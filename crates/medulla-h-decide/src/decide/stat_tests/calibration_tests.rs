//! The rerank calibration head (MEDULLA-DECISION-ENGINE-R822), served: a fit
//! reads only independently judged runs of one scorer version, the isotonic
//! curve is monotone with at most 64 knots, it serves only after a champion
//! pointer move with its own passing receipt, and a thin fit is refused.

use super::super::stat_calibration::fit_isotonic;
use super::learning_tests::{activate, pointer_state};
use super::retrieval_tests::{
    attest, clone_executed, evaluate_by, executed_template, learn, outcome,
};
use super::*;
use medulla_decision_types::decision::statistical::retrieval::ReturnedEvidence;
use medulla_decision_types::decision::statistical::retrieval::{
    LearningRecorded, LearningWrite, PointerMovement, PointerRef,
};
use medulla_decision_types::decision::statistical::retrieval_calibration::{
    CalibrationFitRequest, CalibrationFitted, MAX_CALIBRATION_KNOTS, PPM,
};
use medulla_persistence_records::decision_jobs::champion_calibration;
use medulla_types::contract::BoundedVec;

const SCORER: &str = "rr-v1";
const UNITS: usize = 14;

fn score_of(rank: usize) -> u32 {
    1_000_000 - 70_000 * rank as u32
}

/// Whether the unit at `rank` of run `run` was cited: the top four always, and
/// three lower ranks at falling rates so the raw rates are not monotone.
fn cited_at(run: usize, rank: usize) -> bool {
    match rank {
        0..=3 => true,
        4 => run % 2 == 0,
        5 => run % 4 == 0,
        7 => run % 10 == 0,
        _ => false,
    }
}

/// Forty judged runs of fourteen scored units each: 560 pairs.
async fn judged_scored_runs(h: &Harness) {
    let template_entry = executed_template(h).await;
    for run in 0..40 {
        let record_id = format!("cal-{run:02}");
        clone_executed(h, &template_entry, &record_id);
        let ids: Vec<String> = (0..UNITS).map(|rank| format!("u{run}-{rank}")).collect();
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let cited: Vec<&str> = (0..UNITS)
            .filter(|rank| cited_at(run, *rank))
            .map(|rank| id_refs[rank])
            .collect();
        let mut attested = outcome(&record_id, &id_refs, &cited);
        attested.returned = BoundedVec::new(
            ids.iter()
                .enumerate()
                .map(|(rank, id)| ReturnedEvidence {
                    evidence_id: id.clone(),
                    content_class: None,
                    raw_score_ppm: Some(score_of(rank)),
                })
                .collect(),
        )
        .unwrap();
        attested.scorer_version = Some(SCORER.to_string());
        assert!(learn(h, "decider", attest(attested)).await.is_ok());
        evaluate_by(h, "evaluator", &record_id, true).await;
    }
}

fn fit_write(scorer: &str) -> LearningWrite {
    LearningWrite::FitCalibration {
        request: Box::new(CalibrationFitRequest {
            scorer_version: scorer.to_string(),
            question_id: None,
            window: window(),
        }),
    }
}

fn pointer(scorer: &str) -> PointerRef {
    PointerRef::RerankCalibration {
        scorer_version: scorer.to_string(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R822
#[tokio::test]
async fn isotonic_fit_is_monotone_and_promoted() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    judged_scored_runs(&h).await;

    // The refusal: a scorer version with no judged pairs.
    let thin = learn(&h, "decider", fit_write("rr-none"))
        .await
        .unwrap_err();
    assert!(thin.starts_with("CALIBRATION_TOO_FEW_PAIRS"), "{thin}");

    let fitted: CalibrationFitted = match learn(&h, "decider", fit_write(SCORER)).await.unwrap() {
        LearningRecorded::Calibration(fitted) => *fitted,
        other => panic!("fit: {other:?}"),
    };
    let curve = &fitted.curve;
    assert!(fitted.receipt.passed && curve.is_monotone());
    assert_eq!(fitted.receipt.n_pairs, 560);
    assert!(curve.knots.len() <= MAX_CALIBRATION_KNOTS);
    // Ranks 6 and 7 pool (7 was cited more often than 6): rate 4 / 80.
    assert!(curve
        .knots
        .iter()
        .any(|k| k.score_ppm == 545_000 && k.relevant_ppm == 50_000));
    assert_eq!(curve.knots.first().unwrap().relevant_ppm, 0);
    assert_eq!(curve.knots.last().unwrap().relevant_ppm, PPM);
    assert!(curve.calibrate(900_000) > curve.calibrate(300_000));

    // Nothing serves before the pointer moves, and only with its own receipt.
    let tenant = medulla_server_api::carrier::CarrierAuthority::from_verified(&verified())
        .unwrap()
        .tenant_scope()
        .to_string();
    let champion = || champion_calibration(&h.store, &tenant, SCORER).unwrap();
    assert!(champion().is_none(), "a fitted curve is not the champion");
    let zeros = format!("sha256:{}", "0".repeat(64));
    let wrong = pointer_state(&h, pointer(SCORER), activate(&fitted.curve_digest, &zeros)).await;
    assert!(wrong
        .unwrap_err()
        .starts_with("EVALUATION_RECEIPT_MISMATCH"));
    let other = pointer_state(
        &h,
        pointer("rr-other"),
        activate(&fitted.curve_digest, &fitted.receipt_digest),
    )
    .await;
    assert!(other
        .unwrap_err()
        .starts_with("EVALUATION_RECEIPT_MISMATCH"));

    let active = pointer_state(
        &h,
        pointer(SCORER),
        activate(&fitted.curve_digest, &fitted.receipt_digest),
    )
    .await
    .unwrap();
    assert_eq!(active.key, "champion:rerank_calibration:rr-v1");
    assert_eq!(active.target(), Some(fitted.curve_digest.as_str()));
    assert_eq!(champion().as_ref(), Some(&fitted));

    let rolled = pointer_state(&h, pointer(SCORER), PointerMovement::Rollback)
        .await
        .unwrap();
    assert!(rolled.active.is_none() && champion().is_none());
}

// spec: MEDULLA-DECISION-ENGINE-R822
#[test]
fn pooling_keeps_at_most_sixty_four_monotone_knots() {
    // 2000 distinct scores whose relevance zig-zags, so adjacent blocks
    // violate and then the closest blocks pool down to the knot limit.
    let pairs: Vec<(u32, bool)> = (0..2_000u32)
        .map(|i| (i * 500, (i / 7 + i % 3) % 3 == 0 || i > 1_500))
        .collect();
    let knots = fit_isotonic(&pairs);
    assert!(!knots.is_empty() && knots.len() <= MAX_CALIBRATION_KNOTS);
    assert!(knots
        .windows(2)
        .all(|w| w[0].score_ppm < w[1].score_ppm && w[0].relevant_ppm <= w[1].relevant_ppm));
}
