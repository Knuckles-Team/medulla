//! Retrieval strategy adoption and per-source trust (MEDULLA-DECISION-ENGINE-R821),
//! served: the per-source Beta posterior is derived from judged runs and read
//! through `decision_source_trust`; a strategy is adopted only when its paired
//! run beats the champion by `retrieval.adopt_margin`.

use super::retrieval_tests::{
    attest, clone_executed, evaluate_by, executed_template, learn, outcome, sql,
};
use super::*;
use medulla_decision_outcome_types::decision::promotion::{
    GraderKind, HeldOutEvidence, ReceiptKind, RevisionSubject, HELD_OUT_EVIDENCE_SCHEMA_VERSION,
};
use medulla_decision_stats::decision::promotion::{subject_gate, PromotionGate};
use medulla_decision_types::decision::codes::ADOPTION_NOT_PROVEN;
use medulla_decision_types::decision::statistical::retrieval::ReturnedEvidence;
use medulla_decision_types::decision::DecisionPolicy;
use medulla_types::contract::BoundedVec;
use serde_json::json;

fn sourced(id: &str, source: &str) -> ReturnedEvidence {
    ReturnedEvidence {
        evidence_id: id.to_string(),
        content_class: None,
        raw_score_ppm: None,
        source: Some(source.to_string()),
    }
}

fn strategy() -> RevisionSubject {
    RevisionSubject::RetrievalStrategy {
        strategy_ref: "hyde-rrf".to_string(),
    }
}

fn paired(delta_ppm: i64) -> HeldOutEvidence {
    HeldOutEvidence {
        schema_version: HELD_OUT_EVIDENCE_SCHEMA_VERSION,
        subject: strategy(),
        challenger_ref: "strategy-b".to_string(),
        champion_ref: Some("strategy-a".to_string()),
        receipt_kind: ReceiptKind::DecisionEval,
        receipt_digest: "sha256:aa".to_string(),
        evaluation_set_digest: "sha256:bb".to_string(),
        challenger_score_ppm: 800_000 + delta_ppm,
        champion_score_ppm: Some(800_000),
        delta_ppm,
        case_count: 60,
        independent: true,
        grader_kind: GraderKind::Annotation,
        measurement: None,
        review: None,
    }
}

// spec: MEDULLA-DECISION-ENGINE-R821
#[tokio::test]
async fn paired_run_adopts_and_refuses() {
    let h = Harness::new().await;
    let template_entry = executed_template(&h).await;
    clone_executed(&h, &template_entry, "trust-1");
    let mut attested = outcome("trust-1", &["a", "b", "c"], &["c"]);
    attested.returned = BoundedVec::new(vec![
        sourced("a", "s1"),
        sourced("b", "s2"),
        sourced("c", "s1"),
    ])
    .unwrap();
    learn(&h, "decider", attest(attested)).await.unwrap();

    let trust = "SELECT source, alpha, beta FROM decision_source_trust ORDER BY source";
    assert!(
        sql(&h, "decider", trust).await.is_empty(),
        "an unjudged run teaches no source anything"
    );
    evaluate_by(&h, "evaluator", "trust-1", true).await;
    assert_eq!(
        sql(&h, "decider", trust).await,
        vec![
            vec![json!("s1"), json!(2), json!(2)],
            vec![json!("s2"), json!(1), json!(2)],
        ],
        "alpha counts the cited unit, beta the hard negatives, from the prior (1, 1)"
    );
    assert!(
        sql(&h, "stranger", trust).await.is_empty(),
        "another principal's runs teach it nothing"
    );

    // Adoption: the paired run must beat the champion by the default 0.02.
    let policy = DecisionPolicy::engine_default();
    assert_eq!(
        subject_gate(&strategy(), &paired(20_000), &policy),
        PromotionGate::Promote
    );
    assert_eq!(
        subject_gate(&strategy(), &paired(19_999), &policy),
        PromotionGate::Abstain(ADOPTION_NOT_PROVEN)
    );
}
