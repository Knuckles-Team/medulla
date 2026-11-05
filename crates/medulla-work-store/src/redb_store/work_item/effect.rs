//! WorkItem effect.rs transitions: the uncertain-effect protocol
//! (MEDULLA-DURABLE-KERNEL-R820.2).
//!
//! `MarkWorkItemEffectStarted` records, under the live claim, that the worker
//! is about to cause an external effect. From then on the item may have had
//! an effect, so a lease expiry returns it to `ready` with its attempt
//! unchanged and never retires it to `dead_letter` (see
//! `claim_reclaim_expired_lease`). `RecordEffectReceipt` resolves it with
//! exactly one receipt per job ID; a replay of the same receipt returns the
//! stored one. Every write is a compare-and-set on the item's row revision.

use super::*;
use medulla_work_types::work_item_read::{
    EffectPhase, EffectReceipt, WORK_ITEM_EFFECT_PHASE, WORK_ITEM_ROW_REVISION,
};

/// Whether `props` records a started, unresolved effect.
pub fn effect_started(props: &serde_json::Map<String, serde_json::Value>) -> bool {
    property_string(props, WORK_ITEM_EFFECT_PHASE) == EffectPhase::Started.as_stored()
}

/// Why an effect transition refused. A refusal changes no state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectRefusal {
    /// No live claim matches the presented lease fence.
    ClaimStale,
    /// The row moved past the revision the caller read.
    RevisionConflict { current: u64 },
    /// The receipt names a different job, request or principal.
    ReceiptMismatch,
    /// A different receipt is already stored for this job.
    ReceiptConflict,
}

/// The live-claim fence and the revision a caller read.
pub struct EffectFence<'a> {
    pub tenant: &'a str,
    pub worker_id: &'a str,
    pub lease_epoch: u64,
    pub fencing_token: u64,
    pub expected_revision: u64,
}

fn row_revision(props: &serde_json::Map<String, serde_json::Value>) -> u64 {
    property_u64(props, WORK_ITEM_ROW_REVISION).max(1)
}

fn check_revision(
    props: &serde_json::Map<String, serde_json::Value>,
    expected: u64,
) -> Result<(), EffectRefusal> {
    let current = row_revision(props);
    if current != expected {
        return Err(EffectRefusal::RevisionConflict { current });
    }
    Ok(())
}

fn bump_revision(props: &mut serde_json::Map<String, serde_json::Value>, now_s: f64) -> u64 {
    let next = row_revision(props).saturating_add(1);
    props.insert(WORK_ITEM_ROW_REVISION.into(), serde_json::Value::from(next));
    props.insert("updated_at".into(), serde_json::Value::from(now_s));
    next
}

/// Set `Started` under the live claim. Idempotent for the claim holder: a
/// second mark of an already started item changes nothing.
pub fn apply_mark_effect_started(
    props: &mut serde_json::Map<String, serde_json::Value>,
    fence: &EffectFence<'_>,
    now_s: f64,
) -> Result<u64, EffectRefusal> {
    let live = property_string(props, "node_type") == "WorkItem"
        && property_string(props, "tenant") == fence.tenant
        && property_string(props, "lease_owner") == fence.worker_id
        && matches!(property_string(props, "status"), "leased" | "running")
        && property_u64(props, "lease_epoch") == fence.lease_epoch
        && property_u64(props, "fencing_token") == fence.fencing_token
        && property_f64(props, "lease_expires_at") >= now_s;
    if !live {
        return Err(EffectRefusal::ClaimStale);
    }
    check_revision(props, fence.expected_revision)?;
    if effect_started(props) {
        return Ok(row_revision(props));
    }
    props.insert(
        WORK_ITEM_EFFECT_PHASE.into(),
        serde_json::Value::String(EffectPhase::Started.as_stored().into()),
    );
    Ok(bump_revision(props, now_s))
}

/// What a receipt write decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiptWrite {
    /// The receipt is new: store it in the terminal commit's transaction.
    Store(EffectReceipt),
    /// The same receipt was already stored: return it, write nothing.
    Replay(EffectReceipt),
}

/// Resolve a started effect with its one receipt. `stored` is the receipt
/// already recorded for this job, if any; `request_ref` and `principal_ref`
/// are the ones the work item was admitted with.
pub fn apply_record_effect_receipt(
    props: &mut serde_json::Map<String, serde_json::Value>,
    job_id: &str,
    admitted: (&str, &str),
    receipt: EffectReceipt,
    stored: Option<&EffectReceipt>,
    expected_revision: u64,
    now_s: f64,
) -> Result<ReceiptWrite, EffectRefusal> {
    let (request_ref, principal_ref) = admitted;
    if receipt.job_id != job_id
        || receipt.request_ref != request_ref
        || receipt.principal_ref != principal_ref
    {
        return Err(EffectRefusal::ReceiptMismatch);
    }
    if let Some(stored) = stored {
        return if *stored == receipt {
            Ok(ReceiptWrite::Replay(stored.clone()))
        } else {
            Err(EffectRefusal::ReceiptConflict)
        };
    }
    check_revision(props, expected_revision)?;
    props.insert(
        WORK_ITEM_EFFECT_PHASE.into(),
        serde_json::Value::String(EffectPhase::Resolved.as_stored().into()),
    );
    bump_revision(props, now_s);
    Ok(ReceiptWrite::Store(receipt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_work_types::work_item_read::EffectOutcome;

    fn claimed() -> serde_json::Map<String, serde_json::Value> {
        serde_json::json!({
            "node_type": "WorkItem", "tenant": "t", "status": "running",
            "lease_owner": "w", "lease_epoch": 2, "fencing_token": 2,
            "lease_expires_at": 100.0, "attempt": 3, "max_attempts": 3,
            "row_revision": 4
        })
        .as_object()
        .cloned()
        .expect("object")
    }

    fn fence(expected_revision: u64) -> EffectFence<'static> {
        EffectFence {
            tenant: "t",
            worker_id: "w",
            lease_epoch: 2,
            fencing_token: 2,
            expected_revision,
        }
    }

    fn receipt(outcome: EffectOutcome) -> EffectReceipt {
        EffectReceipt {
            job_id: "job".into(),
            request_ref: "sha256:r".into(),
            principal_ref: "p".into(),
            outcome,
            receipt_ref: "rcpt".into(),
            committed_at: 7,
        }
    }

    #[test]
    fn started_expiry_keeps_attempt_and_blocks_dead_letter() {
        let mut props = claimed();
        let before = props.clone();
        assert_eq!(
            apply_mark_effect_started(&mut props, &fence(3), 50.0),
            Err(EffectRefusal::RevisionConflict { current: 4 })
        );
        assert_eq!(props, before, "a refusal changes nothing");
        let mut stale = claimed();
        stale.insert("lease_epoch".into(), 9.into());
        assert_eq!(
            apply_mark_effect_started(&mut stale, &fence(4), 50.0),
            Err(EffectRefusal::ClaimStale)
        );
        assert_eq!(
            apply_mark_effect_started(&mut props, &fence(4), 50.0),
            Ok(5)
        );
        assert!(effect_started(&props));

        // Lease expiry at the attempt ceiling: back to ready, never dead_letter.
        let retired =
            claim_reclaim_expired_lease(&mut props, "job", "running", 200.0).expect("reclaims");
        assert!(!retired);
        assert_eq!(property_string(&props, "status"), "ready");
        assert_eq!(property_u64(&props, "attempt"), 3);

        // The next claim continues the same attempt.
        let (_, attempt) = claim_grant_lease(&mut props, "w2", 201.0, 300.0).expect("grants");
        assert_eq!(attempt, 3);

        // Without a started effect the same expiry dead-letters.
        let mut plain = claimed();
        assert!(claim_reclaim_expired_lease(&mut plain, "job", "running", 200.0).expect("ok"));
        assert_eq!(property_string(&plain, "status"), "dead_letter");
    }

    #[test]
    fn receipt_replay_returns_stored_receipt() {
        let mut props = claimed();
        apply_mark_effect_started(&mut props, &fence(4), 50.0).expect("marks");
        let admitted = ("sha256:r", "p");
        let first = apply_record_effect_receipt(
            &mut props,
            "job",
            admitted,
            receipt(EffectOutcome::Applied),
            None,
            5,
            60.0,
        )
        .expect("stores");
        let ReceiptWrite::Store(stored) = first else {
            panic!("first receipt stores");
        };
        assert_eq!(
            property_string(&props, WORK_ITEM_EFFECT_PHASE),
            EffectPhase::Resolved.as_stored()
        );
        let after = props.clone();
        assert_eq!(
            apply_record_effect_receipt(
                &mut props,
                "job",
                admitted,
                receipt(EffectOutcome::Applied),
                Some(&stored),
                6,
                61.0,
            ),
            Ok(ReceiptWrite::Replay(stored.clone()))
        );
        assert_eq!(
            apply_record_effect_receipt(
                &mut props,
                "job",
                admitted,
                receipt(EffectOutcome::NotApplied),
                Some(&stored),
                6,
                61.0,
            ),
            Err(EffectRefusal::ReceiptConflict)
        );
        assert_eq!(
            apply_record_effect_receipt(
                &mut props,
                "other",
                admitted,
                receipt(EffectOutcome::Applied),
                None,
                6,
                61.0,
            ),
            Err(EffectRefusal::ReceiptMismatch)
        );
        assert_eq!(props, after, "replay and refusals write nothing");
    }
}
