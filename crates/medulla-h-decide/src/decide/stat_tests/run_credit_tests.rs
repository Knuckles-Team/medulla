//! Run credit, served (MEDULLA-DECISION-ENGINE-R100.102.1 to .5): a delegated
//! run's cited rows join the rows its decisions retrieved, each step is
//! credited against its capability baseline and one receipt is stored; every
//! refusal writes nothing.

use super::retrieval_tests::{attest, clone_executed, executed_template, learn, outcome};
use super::*;
use medulla_decision_types::decision::statistical::retrieval::{LearningRecorded, LearningWrite};
use medulla_decision_types::decision::statistical::run_credit::{
    RunCreditReceipt, RunOutcome, StepRecord,
};

fn step(idx: u32, capability: &str, reward: i32) -> StepRecord {
    StepRecord {
        step_idx: idx,
        agent_iri: "urn:agent:runner".to_string(),
        capability_iri: capability.to_string(),
        reward_q16: reward,
    }
}

fn run(run_id: &str, decisions: &[&str], cited: &[&str], steps: Vec<StepRecord>) -> RunOutcome {
    RunOutcome {
        schema_version: 1,
        run_id: run_id.to_string(),
        decision_ids: BoundedVec::new(decisions.iter().map(|d| d.to_string()).collect()).unwrap(),
        answer_digest: "ab".repeat(32),
        cited_row_ids: BoundedVec::new(cited.iter().map(|c| c.to_string()).collect()).unwrap(),
        success: true,
        steps: BoundedVec::new(steps).unwrap(),
    }
}

async fn send(h: &Harness, who: &str, outcome: RunOutcome) -> Result<RunCreditReceipt, String> {
    let write = LearningWrite::RunOutcome {
        outcome: Box::new(outcome),
    };
    match learn(h, who, write).await {
        Ok(LearningRecorded::Run(receipt)) => Ok(*receipt),
        Ok(_) => Err("not a run receipt".to_string()),
        Err(error) => Err(error),
    }
}

/// A decision `rec-1` that returned e0..e5, cited e3, attested by `decider`.
async fn attested_decision(h: &Harness) {
    let template = executed_template(h).await;
    clone_executed(h, &template, "rec-1");
    let units: Vec<String> = (0..6).map(|i| format!("e{i}")).collect();
    let ids: Vec<&str> = units.iter().map(String::as_str).collect();
    learn(h, "decider", attest(outcome("rec-1", &ids, &["e3"])))
        .await
        .unwrap();
}

fn rows(h: &Harness) -> Vec<(String, Vec<u8>)> {
    h.store
        .decision_artifacts_with_prefix(TENANT, "run_credit/", 10_000)
        .unwrap()
}

fn baseline(h: &Harness, capability: &str) -> (i32, u32) {
    let stored = super::super::stat_run_credit::latest_baseline(&h.store, TENANT, capability)
        .unwrap()
        .unwrap();
    (stored.value_q16, stored.revision)
}

// spec: MEDULLA-DECISION-ENGINE-R100.102.1, MEDULLA-DECISION-ENGINE-R100.102.2, MEDULLA-DECISION-ENGINE-R100.102.3
#[tokio::test]
async fn run_outcome_credits_steps_and_records_hard_negatives() {
    let h = Harness::new().await;
    attested_decision(&h).await;
    let steps = vec![
        step(0, "urn:cap:x", 65_536),
        step(1, "urn:cap:x", 0),
        step(2, "urn:cap:y", -65_536),
    ];
    let receipt = send(
        &h,
        "decider",
        run("run-1", &["rec-1"], &["e3"], steps.clone()),
    )
    .await
    .unwrap();
    // e3 is the positive; e0, e1 and e2 outranked it and were not cited.
    assert_eq!((receipt.positive_count, receipt.negative_count), (1, 3));
    let credits: Vec<(u32, i32, u32)> = receipt
        .steps
        .iter()
        .map(|s| (s.step_idx, s.advantage_q16, s.baseline_revision))
        .collect();
    assert_eq!(credits, [(0, 65_536, 0), (1, -6_554, 1), (2, -65_536, 0)]);
    // Stored baselines: x moved twice (6554, then 5899), y once (-6554).
    assert_eq!(baseline(&h, "urn:cap:x"), (5_899, 2));
    assert_eq!(baseline(&h, "urn:cap:y"), (-6_554, 1));

    // A later run reads the moved baseline before its own update.
    let later = send(
        &h,
        "decider",
        run(
            "run-2",
            &["rec-1"],
            &["e3"],
            vec![step(0, "urn:cap:x", 5_899)],
        ),
    )
    .await
    .unwrap();
    assert_eq!(later.steps.as_slice()[0].advantage_q16, 0);
    assert_eq!(later.steps.as_slice()[0].baseline_revision, 2);
    assert_eq!(baseline(&h, "urn:cap:x"), (5_899, 3));

    // The same run replayed answers the stored receipt and moves nothing.
    let before = rows(&h);
    let replay = send(&h, "decider", run("run-1", &["rec-1"], &["e3"], steps))
        .await
        .unwrap();
    assert_eq!(replay, receipt);
    assert_eq!(rows(&h), before);

    // A run that did not succeed credits its steps but no rows.
    let mut failed = run(
        "run-3",
        &["rec-1"],
        &["e3"],
        vec![step(0, "urn:cap:z", 100)],
    );
    failed.success = false;
    let failed = send(&h, "decider", failed).await.unwrap();
    assert_eq!((failed.positive_count, failed.negative_count), (0, 0));

    // Cross-principal and cross-tenant: nothing is credited.
    let stranger = send(&h, "stranger", run("run-4", &["rec-1"], &["e3"], vec![]))
        .await
        .unwrap_err();
    assert!(stranger.starts_with("INVALID_ARGUMENT"), "{stranger}");
    let foreign = DecisionLogOp::Learn {
        tenant_id: "other-tenant".to_string(),
        write: LearningWrite::RunOutcome {
            outcome: Box::new(run("run-5", &["rec-1"], &["e3"], vec![])),
        },
    };
    assert!(decode::<LearningRecorded>(log_op(&h, "decider", foreign).await).is_err());
    assert!(
        super::super::stat_run_credit::stored_receipt(&h.store, "other-tenant", "run-5")
            .unwrap()
            .is_none()
    );
}

// spec: MEDULLA-DECISION-ENGINE-R100.102.4
#[tokio::test]
async fn run_outcome_refusals_write_nothing() {
    let h = Harness::new().await;
    attested_decision(&h).await;
    let template = executed_template(&h).await;
    clone_executed(&h, &template, "rec-bare");
    send(
        &h,
        "decider",
        run(
            "run-ok",
            &["rec-1"],
            &["e3"],
            vec![step(0, "urn:cap:x", 100)],
        ),
    )
    .await
    .unwrap();
    let before = rows(&h);

    let many: Vec<StepRecord> = (0..257).map(|i| step(i, "urn:cap:x", 0)).collect();
    let cases: Vec<(&str, &str, RunOutcome)> = vec![
        (
            "decider",
            "INVALID_ARGUMENT",
            run("r-unknown", &["no-such"], &[], vec![]),
        ),
        (
            "decider",
            "INVALID_ARGUMENT",
            run("r-no-outcome", &["rec-bare"], &[], vec![]),
        ),
        (
            "stranger",
            "INVALID_ARGUMENT",
            run("r-invisible", &["rec-1"], &["e3"], vec![]),
        ),
        (
            "decider",
            "CITATION_NOT_RETRIEVED",
            run("r-cite", &["rec-1"], &["zz"], vec![step(0, "urn:cap:q", 1)]),
        ),
        (
            "decider",
            "RUN_STEPS_TOO_MANY",
            run("r-many", &["rec-1"], &[], many),
        ),
        (
            "decider",
            "RUN_STEP_ORDER_INVALID",
            run(
                "r-order",
                &["rec-1"],
                &[],
                vec![step(2, "urn:cap:q", 0), step(1, "urn:cap:q", 0)],
            ),
        ),
        (
            "decider",
            "RUN_REWARD_OUT_OF_RANGE",
            run(
                "r-reward",
                &["rec-1"],
                &[],
                vec![step(0, "urn:cap:q", 70_000)],
            ),
        ),
        (
            "decider",
            "IDEMPOTENCY_CONFLICT",
            run(
                "run-ok",
                &["rec-1"],
                &["e3"],
                vec![step(0, "urn:cap:x", 999)],
            ),
        ),
    ];
    for (who, code, outcome) in cases {
        let refused = send(&h, who, outcome).await.unwrap_err();
        assert!(refused.starts_with(code), "expected {code}, got {refused}");
        assert_eq!(rows(&h), before, "{code} wrote something");
    }
    // The detail names the run and the offending row.
    let named = send(&h, "decider", run("r-named", &["rec-1"], &["zz"], vec![]))
        .await
        .unwrap_err();
    assert!(named.contains("r-named") && named.contains("zz"), "{named}");
}

// spec: MEDULLA-DECISION-ENGINE-R100.102.5
#[tokio::test]
async fn run_credit_receipt_reads_back_after_one_run() {
    let h = Harness::new().await;
    attested_decision(&h).await;
    let sent = send(
        &h,
        "decider",
        run(
            "run-r",
            &["rec-1"],
            &["e3"],
            vec![step(4, "urn:cap:x", 65_536), step(9, "urn:cap:x", 65_536)],
        ),
    )
    .await
    .unwrap();
    let stored = super::super::stat_run_credit::stored_receipt(&h.store, TENANT, "run-r")
        .unwrap()
        .unwrap();
    assert_eq!(stored, sent);
    assert_eq!((stored.positive_count, stored.negative_count), (1, 3));
    let credits: Vec<(u32, i32, u32)> = stored
        .steps
        .iter()
        .map(|s| (s.step_idx, s.advantage_q16, s.baseline_revision))
        .collect();
    // 6554 after the first step; the second gap is 65536 - 6554.
    assert_eq!(credits, [(4, 65_536, 0), (9, 58_982, 1)]);
    assert_eq!(
        stored.premises.as_slice(),
        [
            "run:run-r",
            "run:positives=1",
            "run:negatives=3",
            "knob:run_credit.baseline_weight=0.100000"
        ]
    );
    assert!(
        medulla_decision_types::decision::statistical::run_credit::is_sha256_hex(
            &stored.receipt_digest
        )
    );
    assert_eq!(stored.decision_ids.as_slice(), ["rec-1"]);
    // Another tenant reads nothing.
    assert!(
        super::super::stat_run_credit::stored_receipt(&h.store, "other-tenant", "run-r")
            .unwrap()
            .is_none()
    );
}
