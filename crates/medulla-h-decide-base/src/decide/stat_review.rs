//! `DecisionLog` review (MEDULLA-DECISION-ENGINE-R805.15): an operator publishes or
//! retires a revision proposal that stands at stage `evaluated`.
//!
//! The stage moves only through the one edge table of `RevisionMachine`
//! ([`RevisionRow::transition`]): `approve` fires `Publish` (edge V02) and `veto`
//! fires `Retire` (edge V06). The row and one receipt keyed
//! `review:<revision_id>:<digest>` land in one owner write, so a repeat of the same
//! review finds the receipt and writes nothing.

use super::stat_executor::ExecutionContext;
use super::stat_log_reader::LogReader;
use super::stat_proposals::record_revision_over;
use super::stat_support::invalid_parameter;
use medulla_decision_outcome_types::decision::promotion::{
    RevisionEvent, RevisionStage, REVISION_MACHINE,
};
use medulla_decision_outcome_types::decision::revision_list::{
    ReviewDecisionKind, RevisionReviewReceipt, RevisionReviewRecord, RevisionRow,
    REVIEW_KEY_PREFIX, REVISION_KEY_PREFIX,
};
use medulla_decision_policy_types::decision::held_out::{ReceiptKind, ReviewApproval};
use medulla_decision_statistical_types::decision::codes::{
    REVIEW_SELF_APPROVAL, REVISION_DIGEST_MISMATCH, REVISION_NOT_FOUND,
};
use medulla_decision_types::decision::statistical::log::{ReviewDecision, RevisionReview};
use medulla_persistence_records::decision_jobs::{decode_artifact, encode_artifact};
use medulla_types::contract::Digest256;
use medulla_types::transition_refused::{
    MachineEvent, MachineState, TransitionRefusalReason, TransitionRefused,
};

/// Most revision rows one tenant's relation lists while a review looks one up.
const MAX_REVISIONS: usize = 65_536;
/// The longest reason a review carries, in characters.
const MAX_REASON_CHARS: usize = 1_024;

/// The store key of the receipt of reviewing `revision_id` at `digest`.
fn receipt_key(revision_id: &str, digest: &str) -> String {
    format!("{REVIEW_KEY_PREFIX}{revision_id}:{digest}")
}

/// The decision's event and the stage it leads to.
fn event_of(decision: ReviewDecision) -> (RevisionEvent, ReviewDecisionKind, RevisionStage) {
    match decision {
        ReviewDecision::Approve => (
            RevisionEvent::Publish,
            ReviewDecisionKind::Approve,
            RevisionStage::Published,
        ),
        ReviewDecision::Veto => (
            RevisionEvent::Retire,
            ReviewDecisionKind::Veto,
            RevisionStage::Retired,
        ),
    }
}

/// The newest stored row of `revision_id` with the bytes it was read as: a champion
/// move writes a new row of the same id, so the latest `created_ms` stands.
fn newest_row(
    ctx: &ExecutionContext,
    revision_id: &str,
) -> Result<Option<(RevisionRow, Vec<u8>)>, String> {
    let mut newest: Option<(RevisionRow, Vec<u8>)> = None;
    for (_, bytes) in ctx.store.decision_artifacts_with_prefix(
        ctx.tenant_id,
        REVISION_KEY_PREFIX,
        MAX_REVISIONS,
    )? {
        let row: RevisionRow = decode_artifact(&bytes, "revision row")?;
        if row.id == revision_id
            && newest
                .as_ref()
                .is_none_or(|(held, _)| row.created_ms > held.created_ms)
        {
            newest = Some((row, bytes));
        }
    }
    Ok(newest)
}

/// Serve one `Review` op. The reviewer is `reader.principal`.
pub fn revision_review(
    ctx: &ExecutionContext,
    reader: &LogReader,
    review: &RevisionReview,
) -> Result<RevisionReviewReceipt, String> {
    let reason_chars = review.reason.chars().count();
    if reason_chars == 0 || reason_chars > MAX_REASON_CHARS {
        return Err(invalid_parameter(format!(
            "a review reason is 1 to {MAX_REASON_CHARS} characters, not {reason_chars}"
        )));
    }
    let Some((mut row, held)) = newest_row(ctx, &review.revision_id)? else {
        return Err(format!(
            "{}: no revision `{}` is stored for the tenant",
            REVISION_NOT_FOUND.as_str(),
            review.revision_id
        ));
    };
    if row.digest != review.digest {
        return Err(format!(
            "{}: the review names digest `{}`, not the revision's",
            REVISION_DIGEST_MISMATCH.as_str(),
            review.digest
        ));
    }
    let (event, kind, stage) = event_of(review.decision);
    let key = receipt_key(&review.revision_id, &review.digest);
    if let Some(bytes) = ctx.store.decision_artifact(ctx.tenant_id, &key)? {
        let stored: RevisionReviewRecord = decode_artifact(&bytes, "review receipt")?;
        if stored.decision == kind {
            return Ok(RevisionReviewReceipt {
                revision_id: review.revision_id.clone(),
                stage,
                receipt_ref: key,
            });
        }
    }
    if row.author == reader.principal {
        return Err(format!(
            "{}: the reviewer is the author of `{}`",
            REVIEW_SELF_APPROVAL.as_str(),
            review.revision_id
        ));
    }
    // Under review means stage `evaluated`; no other review state exists. The machine
    // also leads a published row to `retired`, which is a rollback and not a review, so
    // the review refuses any other stage here, in the machine's own refusal shape.
    if row.stage != RevisionStage::Evaluated {
        return Err(TransitionRefused {
            machine: REVISION_MACHINE.name.to_string(),
            from: row.stage.stored().to_string(),
            event: event.name().to_string(),
            reason: TransitionRefusalReason::IllegalTransition,
        }
        .to_string());
    }
    row.transition(event)
        .map_err(|refused| refused.to_string())?;
    let record = RevisionReviewRecord {
        receipt_kind: ReceiptKind::ReviewApproval,
        review: ReviewApproval {
            author: row.author.clone(),
            reviewer: reader.principal.clone(),
        },
        decision: kind,
        reason_digest: Digest256::sha256(review.reason.as_bytes()).to_hex(),
        decided_ms: ctx.now_ms,
    };
    let receipt = (key.clone(), encode_artifact(&record)?);
    record_revision_over(ctx.store, ctx.tenant_id, &row, Some(held), &[receipt])?;
    Ok(RevisionReviewReceipt {
        revision_id: review.revision_id.clone(),
        stage: row.stage,
        receipt_ref: key,
    })
}
