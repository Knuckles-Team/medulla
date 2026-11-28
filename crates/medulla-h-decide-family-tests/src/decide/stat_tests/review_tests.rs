//! `DecisionLog.review` served (MEDULLA-DECISION-ENGINE-R805.15): an operator approves or
//! vetoes an `evaluated` revision. Approve publishes it and veto retires it, each through the
//! revision machine; one `review_approval` receipt is stored in the same write; a repeat writes
//! nothing; a stale digest, the author as reviewer and a row in any other stage each refuse.

use medulla_decision_outcome_types::decision::promotion::RevisionStage;
use medulla_decision_outcome_types::decision::revision_list::{
    ReviewDecisionKind, RevisionReviewReceipt, RevisionReviewRecord, RevisionRow, REVIEW_KEY_PREFIX,
};
use medulla_decision_policy_types::decision::held_out::ReceiptKind;
use medulla_decision_types::decision::statistical::log::{
    DecisionLogOp, ReviewDecision, RevisionReview,
};
use medulla_h_decide_base::decide::stat_proposals::record_revision;
use medulla_h_decide_stat_tests::decide::stat_tests::log_tests::log_op;
use medulla_h_decide_stat_tests::decide::stat_tests::*;
use medulla_persistence_records::decision_jobs::decode_artifact;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_types::contract::Digest256;

const AUTHOR: &str = "author";
const REVIEWER: &str = "reviewer";
const REASON: &str = "the contrast evidence holds";

fn persistence_id(who: &str) -> String {
    VerifiedRequestContext::verified_for_test_in_tenant(who, TENANT).principal_persistence_id()
}

fn row(id: &str, stage: RevisionStage) -> RevisionRow {
    RevisionRow {
        id: id.to_string(),
        subject_kind: "definition".to_string(),
        subject_ref: "harness_bundle:agent:t".to_string(),
        stage,
        digest: format!("sha256:{id}"),
        evaluation_receipt_id: Some(format!("receipt:{id}")),
        created_ms: 10,
        author: persistence_id(AUTHOR),
    }
}

fn seed(h: &Harness, row: &RevisionRow) {
    record_revision(&h.store, TENANT, row).unwrap();
}

fn stored(h: &Harness, row: &RevisionRow) -> RevisionRow {
    let bytes = h
        .store
        .decision_artifact(TENANT, &row.key())
        .unwrap()
        .expect("the row stays stored");
    decode_artifact(&bytes, "revision row").unwrap()
}

fn review_of(row: &RevisionRow, decision: ReviewDecision) -> RevisionReview {
    RevisionReview {
        revision_id: row.id.clone(),
        decision,
        digest: row.digest.clone(),
        reason: REASON.to_string(),
    }
}

async fn review(
    h: &Harness,
    who: &str,
    review: RevisionReview,
) -> Result<RevisionReviewReceipt, String> {
    decode(
        log_op(
            h,
            who,
            DecisionLogOp::Review {
                tenant_id: TENANT.to_string(),
                review,
            },
        )
        .await,
    )
}

fn receipts(h: &Harness) -> usize {
    h.store
        .decision_artifacts_with_prefix(TENANT, REVIEW_KEY_PREFIX, 100)
        .unwrap()
        .len()
}

/// Verifies MEDULLA-DECISION-ENGINE-R805.15: approve publishes and veto retires an `evaluated`
/// row, the receipt is keyed `review:<id>:<digest>` and carries the reason digest and not the
/// reason, and a repeat of the same review answers the stored receipt and writes nothing.
// spec: MEDULLA-DECISION-ENGINE-R805.15
#[tokio::test]
async fn review_publishes_or_retires_an_evaluated_revision_once() {
    let h = Harness::new().await;
    let approved = row("rev:approve", RevisionStage::Evaluated);
    let vetoed = row("rev:veto", RevisionStage::Evaluated);
    seed(&h, &approved);
    seed(&h, &vetoed);

    let receipt = review(&h, REVIEWER, review_of(&approved, ReviewDecision::Approve))
        .await
        .unwrap();
    assert_eq!(receipt.stage, RevisionStage::Published);
    assert_eq!(receipt.receipt_ref, "review:rev:approve:sha256:rev:approve");
    assert_eq!(stored(&h, &approved).stage, RevisionStage::Published);

    let receipt = review(&h, REVIEWER, review_of(&vetoed, ReviewDecision::Veto))
        .await
        .unwrap();
    assert_eq!(receipt.stage, RevisionStage::Retired);
    assert_eq!(stored(&h, &vetoed).stage, RevisionStage::Retired);
    assert_eq!(receipts(&h), 2);

    let bytes = h
        .store
        .decision_artifact(TENANT, "review:rev:approve:sha256:rev:approve")
        .unwrap()
        .expect("the receipt is stored");
    let record: RevisionReviewRecord = decode_artifact(&bytes, "review receipt").unwrap();
    assert_eq!(record.receipt_kind, ReceiptKind::ReviewApproval);
    assert_eq!(record.decision, ReviewDecisionKind::Approve);
    assert_eq!(record.review.author, persistence_id(AUTHOR));
    assert_eq!(record.review.reviewer, persistence_id(REVIEWER));
    assert_eq!(
        record.reason_digest,
        Digest256::sha256(REASON.as_bytes()).to_hex()
    );
    assert!(
        !String::from_utf8_lossy(&bytes).contains(REASON),
        "the reason text is not stored"
    );

    // A repeat answers the stored receipt, from any reviewer, and writes nothing.
    let before = (stored(&h, &approved), bytes.clone(), receipts(&h));
    let again = review(&h, REVIEWER, review_of(&approved, ReviewDecision::Approve))
        .await
        .unwrap();
    assert_eq!(again.stage, RevisionStage::Published);
    assert_eq!(again.receipt_ref, "review:rev:approve:sha256:rev:approve");
    let after = (
        stored(&h, &approved),
        h.store
            .decision_artifact(TENANT, "review:rev:approve:sha256:rev:approve")
            .unwrap()
            .unwrap(),
        receipts(&h),
    );
    assert_eq!(before, after);
}

/// Verifies MEDULLA-DECISION-ENGINE-R805.15: every refusal is typed and writes nothing.
// spec: MEDULLA-DECISION-ENGINE-R805.15
#[tokio::test]
async fn review_refusals_are_typed_and_write_nothing() {
    let h = Harness::new().await;
    let evaluated = row("rev:eval", RevisionStage::Evaluated);
    seed(&h, &evaluated);
    let mut others = Vec::new();
    for (id, stage) in [
        ("rev:draft", RevisionStage::Draft),
        ("rev:published", RevisionStage::Published),
        ("rev:champion", RevisionStage::Champion),
        ("rev:retired", RevisionStage::Retired),
    ] {
        let other = row(id, stage);
        seed(&h, &other);
        others.push(other);
    }

    let absent = row("rev:absent", RevisionStage::Evaluated);
    let refused = review(&h, REVIEWER, review_of(&absent, ReviewDecision::Approve))
        .await
        .unwrap_err();
    assert!(refused.starts_with("REVISION_NOT_FOUND"), "{refused}");

    let mut stale = review_of(&evaluated, ReviewDecision::Approve);
    stale.digest = "sha256:stale".to_string();
    let refused = review(&h, REVIEWER, stale).await.unwrap_err();
    assert!(refused.starts_with("REVISION_DIGEST_MISMATCH"), "{refused}");

    let refused = review(&h, AUTHOR, review_of(&evaluated, ReviewDecision::Veto))
        .await
        .unwrap_err();
    assert!(refused.starts_with("REVIEW_SELF_APPROVAL"), "{refused}");

    for other in &others {
        for decision in [ReviewDecision::Approve, ReviewDecision::Veto] {
            let refused = review(&h, REVIEWER, review_of(other, decision))
                .await
                .unwrap_err();
            assert!(refused.starts_with("TRANSITION_REFUSED"), "{refused}");
            assert_eq!(stored(&h, other).stage, other.stage);
        }
    }

    for reason in [String::new(), "x".repeat(1_025)] {
        let mut bad = review_of(&evaluated, ReviewDecision::Approve);
        bad.reason = reason;
        let refused = review(&h, REVIEWER, bad).await.unwrap_err();
        assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    }

    assert_eq!(stored(&h, &evaluated).stage, RevisionStage::Evaluated);
    assert_eq!(receipts(&h), 0);
}
