//! The `feedback_loop` family through the actual Decide path and the decision
//! log (MEDULLA-DECISION-ENGINE-R169.3, R169.4): a round continues or stops by
//! the bound and the findings, the committed round is the durable continuation
//! a fresh harness reads back, and a refusal leaves the continuation alone.

use super::*;
use medulla_decision_types::decision::checker_feedback::{
    Continuation, FeedbackRecord, StopReason, FEEDBACK_BOUND_INVALID, FEEDBACK_REPLAYED_KEY,
    FEEDBACK_UNGRADED_OUTPUT,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::ResolutionKind;
use medulla_types::contract::Digest256;

use super::super::family::feedback_loop::latest_continuation;
use super::super::stat_log::LogReader;

fn feedback(loop_id: &str, round: u32, findings: u32, tag: &str) -> FeedbackRecord {
    FeedbackRecord {
        loop_id: loop_id.into(),
        round,
        output_digest: Digest256::sha256(format!("out:{tag}").as_bytes()),
        checker_report_digest: Digest256::sha256(format!("report:{tag}").as_bytes()),
        findings_digest: Digest256::sha256(format!("findings:{tag}").as_bytes()),
        finding_count: findings,
        next_instruction_digest: Digest256::sha256(format!("next:{tag}").as_bytes()),
    }
}

fn round_request(pin: &ComponentDependency, record: FeedbackRecord, bound: i64) -> DecideRequest {
    let option = |id: &str| DeclaredOption {
        option_id: id.into(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::default(),
    };
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: format!("feedback.{}", record.loop_id),
            kind: QuestionKind::FeedbackLoop,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![option("continue"), option("stop")]).unwrap(),
        },
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![
            TypedParam {
                name: "feedback".into(),
                value: TypedValue::Feedback(Box::new(record)),
            },
            TypedParam {
                name: "bound".into(),
                value: TypedValue::Int(bound),
            },
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn chose(record: &StatisticalDecisionRecord, option: &str) -> bool {
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected an advisory, got {:?}", record.outcome);
    };
    scores
        .iter()
        .any(|score| score.option_id == option && score.score.value != 0)
}

async fn commit_round(h: &Harness, record: &StatisticalDecisionRecord) {
    let committed: DecisionLogCommitted = decode(
        log_op(
            h,
            "decider",
            DecisionLogOp::Commit {
                record: Box::new(record.clone()),
                evaluator: None,
            },
        )
        .await,
    )
    .unwrap();
    assert!(!committed.replayed);
}

fn reader(tenant: &str, principal: &str) -> LogReader {
    LogReader {
        tenant_id: tenant.into(),
        principal: principal.into(),
        roles: Vec::new(),
        retention: super::super::stat_retention::Retention::none(),
    }
}

fn continuation(h: &Harness, tenant: &str, principal: &str, loop_id: &str) -> Option<Continuation> {
    latest_continuation(h.store.as_ref(), &reader(tenant, principal), loop_id).unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R169.3
#[tokio::test]
async fn continuation_survives_a_fresh_harness() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let principal = verified().principal_persistence_id();
    assert_eq!(continuation(&h, TENANT, &principal, "loop:1"), None);

    // Round 1 has findings and room: continue, as a constraint.
    let one = decide(&h, round_request(&pin, feedback("loop:1", 1, 2, "a"), 3))
        .await
        .unwrap();
    let one = one.records.as_slice()[0].clone();
    assert!(chose(&one, "continue") && !chose(&one, "stop"));
    assert_eq!(one.resolution_kind, ResolutionKind::Constraint);
    commit_round(&h, &one).await;
    let expected = Continuation {
        loop_id: "loop:1".into(),
        round: 1,
        stop: None,
    };
    assert_eq!(
        continuation(&h, TENANT, &principal, "loop:1"),
        Some(expected.clone())
    );

    // The committed round is what a restarted server reads.
    let h = h.reopened().await;
    assert_eq!(
        continuation(&h, TENANT, &principal, "loop:1"),
        Some(expected)
    );

    // The latest round wins; round 3 reaches the bound and stops.
    let three = decide(&h, round_request(&pin, feedback("loop:1", 3, 1, "c"), 3))
        .await
        .unwrap();
    let three = three.records.as_slice()[0].clone();
    assert!(chose(&three, "stop"));
    commit_round(&h, &three).await;
    assert_eq!(
        continuation(&h, TENANT, &principal, "loop:1").map(|c| (c.round, c.stop)),
        Some((3, Some(StopReason::RoundBound)))
    );

    // Cross-tenant negative: another tenant and another principal read nothing,
    // and a foreign-tenant request is refused before any read.
    assert_eq!(continuation(&h, "tenant-other", &principal, "loop:1"), None);
    assert_eq!(continuation(&h, TENANT, "stranger", "loop:1"), None);
    let mut foreign = round_request(&pin, feedback("loop:1", 4, 1, "d"), 3);
    foreign.tenant_id = "foreign-tenant".into();
    assert!(decide(&h, foreign)
        .await
        .unwrap_err()
        .starts_with("ACCESS_DENIED"));
}

// spec: MEDULLA-DECISION-ENGINE-R169.4
#[tokio::test]
async fn feedback_refusals_do_not_advance() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let principal = verified().principal_persistence_id();
    let one = decide(&h, round_request(&pin, feedback("loop:2", 1, 2, "a"), 3))
        .await
        .unwrap();
    commit_round(&h, &one.records.as_slice()[0]).await;
    let before = Continuation {
        loop_id: "loop:2".into(),
        round: 1,
        stop: None,
    };
    let artifacts = || {
        h.store
            .decision_artifacts_with_prefix(TENANT, "", 100_000)
            .unwrap()
            .len()
    };
    let stored = artifacts();

    // A bound below one and above the knob (default five) refuses.
    for bound in [0, -1, 6] {
        let error = decide(
            &h,
            round_request(&pin, feedback("loop:2", 2, 2, "b"), bound),
        )
        .await
        .unwrap_err();
        assert!(
            error.starts_with(FEEDBACK_BOUND_INVALID.as_str()),
            "{error}"
        );
    }
    // A round with no checker report digest was never graded.
    let mut ungraded = feedback("loop:2", 2, 2, "b");
    ungraded.checker_report_digest = Digest256::from_bytes([0u8; 32]);
    let error = decide(&h, round_request(&pin, ungraded, 3))
        .await
        .unwrap_err();
    assert!(
        error.starts_with(FEEDBACK_UNGRADED_OUTPUT.as_str()),
        "{error}"
    );
    // Round 1 already holds a committed record: other content is a replayed key,
    // the same content is an idempotent repeat.
    let error = decide(
        &h,
        round_request(&pin, feedback("loop:2", 1, 2, "other"), 3),
    )
    .await
    .unwrap_err();
    assert!(error.starts_with(FEEDBACK_REPLAYED_KEY.as_str()), "{error}");
    decide(&h, round_request(&pin, feedback("loop:2", 1, 2, "a"), 3))
        .await
        .unwrap();

    assert_eq!(
        continuation(&h, TENANT, &principal, "loop:2"),
        Some(before),
        "a refusal does not advance the continuation"
    );
    assert_eq!(artifacts(), stored, "a refusal writes nothing");
}
