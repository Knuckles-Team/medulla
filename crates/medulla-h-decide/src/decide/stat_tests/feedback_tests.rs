//! Feedback events through `DecisionLog` `Learn` (MEDULLA-DECISION-ENGINE-R100.1.1):
//! one complete event joins a committed retrieval-plan record once, an equal
//! replay answers the stored row and every incomplete join refuses with its
//! code and writes nothing. Another tenant never sees the record.

use super::retrieval_tests::{
    attest, clone_executed, executed_template, learn, outcome,
};
use super::*;
use medulla_decision_types::decision::statistical::feedback::{
    FeedbackEvent, FeedbackOutcome, FEEDBACK_SCHEMA_VERSION,
};
use medulla_decision_types::decision::statistical::retrieval::{LearningRecorded, LearningWrite};

fn event(digest: &str, record: &str, revision: u64) -> FeedbackEvent {
    FeedbackEvent {
        schema_version: FEEDBACK_SCHEMA_VERSION,
        event_id: "ev-1".to_string(),
        source_revision: revision,
        record_id: record.to_string(),
        record_digest: digest.to_string(),
        option_id: "plan-hyde".to_string(),
        actor_id: "user-1".to_string(),
        outcome: FeedbackOutcome::Correction,
        evidence_ids: BoundedVec::new(vec!["u1".to_string()]).unwrap(),
        evidence_digests: BoundedVec::new(vec!["b".repeat(64)]).unwrap(),
        event_time_ms: 5,
    }
}

async fn send(h: &Harness, event: FeedbackEvent) -> Result<LearningRecorded, String> {
    learn(
        h,
        "decider",
        LearningWrite::RecordFeedback {
            event: Box::new(event),
        },
    )
    .await
}

fn rows(h: &Harness, tenant: &str) -> usize {
    h.store
        .decision_artifacts_with_prefix(tenant, "feedback/", 100)
        .unwrap()
        .len()
}

// spec: MEDULLA-DECISION-ENGINE-R100.1.1
#[tokio::test]
async fn feedback_event_joins_once_and_refuses_incomplete_joins() {
    let h = Harness::new().await;
    let template = executed_template(&h).await;
    clone_executed(&h, &template, "rec-fb");
    clone_executed(&h, &template, "rec-no-outcome");
    let digest = template
        .record
        .record_digest
        .strip_prefix("sha256:")
        .unwrap_or(&template.record.record_digest)
        .to_string();
    assert!(learn(&h, "decider", attest(outcome("rec-fb", &["u1", "u2"], &["u1"])))
        .await
        .is_ok());

    // One complete event is stored once, with its three premises.
    let LearningRecorded::Feedback(stored) = send(&h, event(&digest, "rec-fb", 2)).await.unwrap()
    else {
        panic!("a feedback write answers its stored event");
    };
    assert_eq!(rows(&h, TENANT), 1);
    for premise in [
        "feedback:event_id=ev-1",
        "feedback:source_revision=2",
    ] {
        assert!(stored.premises.iter().any(|p| p == premise), "{premise}");
    }
    assert!(stored
        .premises
        .iter()
        .any(|p| p == &format!("feedback:record_digest={digest}")));

    // An equal replay answers the stored row and adds none.
    let LearningRecorded::Feedback(replayed) =
        send(&h, event(&digest, "rec-fb", 2)).await.unwrap()
    else {
        panic!("a replay answers the stored event");
    };
    assert_eq!(replayed, stored);
    assert_eq!(rows(&h, TENANT), 1);

    // The same key with other content is a conflict.
    let mut changed = event(&digest, "rec-fb", 2);
    changed.outcome = FeedbackOutcome::Error;
    let error = send(&h, changed).await.unwrap_err();
    assert!(error.starts_with("IDEMPOTENCY_CONFLICT"), "{error}");

    // Each incomplete join refuses with its code and writes nothing.
    let refused = |error: Result<LearningRecorded, String>, code: &str| {
        let error = error.expect_err("a refusal");
        assert!(error.starts_with(code), "{code} vs {error}");
        assert_eq!(rows(&h, TENANT), 1, "a refusal writes nothing");
    };
    refused(
        send(&h, event(&digest, "rec-missing", 3)).await,
        "FEEDBACK_JOIN_INCOMPLETE",
    );
    refused(
        send(&h, event(&digest, "rec-no-outcome", 3)).await,
        "FEEDBACK_JOIN_INCOMPLETE",
    );
    let mut other_option = event(&digest, "rec-fb", 3);
    other_option.option_id = "plan-deep".to_string();
    refused(send(&h, other_option).await, "FEEDBACK_JOIN_INCOMPLETE");
    refused(
        send(&h, event(&"f".repeat(64), "rec-fb", 3)).await,
        "FEEDBACK_EVIDENCE_MISMATCH",
    );
    let mut unknown = event(&digest, "rec-fb", 3);
    unknown.evidence_ids = BoundedVec::new(vec!["u9".to_string()]).unwrap();
    refused(send(&h, unknown).await, "FEEDBACK_EVIDENCE_MISMATCH");
    refused(
        send(&h, event(&digest, "rec-fb", 1)).await,
        "FEEDBACK_SOURCE_STALE",
    );

    // Another tenant cannot see the record, so the join is incomplete there.
    let other = VerifiedRequestContext::verified_for_test_in_tenant("decider", "tenant-other");
    let response = super::super::log::handle_decision_log(
        &h.state,
        9,
        &other,
        DecisionLogOp::Learn {
            tenant_id: "tenant-other".to_string(),
            write: LearningWrite::RecordFeedback {
                event: Box::new(event(&digest, "rec-fb", 4)),
            },
        },
    )
    .await;
    assert!(response.error.is_some(), "{response:?}");
    assert_eq!(rows(&h, "tenant-other"), 0);
}
