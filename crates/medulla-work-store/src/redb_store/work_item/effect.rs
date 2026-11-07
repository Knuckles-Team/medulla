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
use medulla_types::contract::ServerErrorCode;
use medulla_work_types::work_item_read::{
    AdmittedRequestRecord, EffectPhase, EffectReceipt, EffectRecordKey, EffectWriteResult,
    MarkWorkItemEffectStartedRequest, RecordEffectReceiptRequest, SubmitAdmittedRequestRequest,
    WORK_ITEM_EFFECT_PHASE, WORK_ITEM_ROW_REVISION,
};

/// The one receipt per `(graph, work_item_id, attempt)` (R820.1).
pub const EFFECT_RECEIPTS: redb::TableDefinition<(&str, &str, u64), &[u8]> =
    redb::TableDefinition::new("work_item_effect_receipts");
/// The admitted request record per `(graph, work_item_id, attempt)` (R820.1).
pub const ADMITTED_REQUESTS: redb::TableDefinition<(&str, &str, u64), &[u8]> =
    redb::TableDefinition::new("work_item_admitted_requests");

/// A writable effect-record table of one graph member.
pub type EffectTable<'t> =
    ScopedOwnerTableMut<'t, (&'static str, &'static str, u64), &'static [u8]>;

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
        && medulla_work_types::work_item_machine::WorkItemStatus::holds_lease_text(
            property_string(props, "status"),
        )
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

impl EffectRefusal {
    /// The registered refusal this maps to on the wire.
    pub fn to_wire(&self) -> String {
        match self {
            Self::ClaimStale => format!(
                "{}: no live claim matches the presented fence",
                ServerErrorCode::WorkItemClaimStale
            ),
            Self::RevisionConflict { current } => format!(
                "{}: WorkItem row revision is {current}",
                ServerErrorCode::RevisionConflict
            ),
            Self::ReceiptMismatch => format!(
                "{}: receipt does not match the work item",
                ServerErrorCode::EffectReceiptMismatch
            ),
            Self::ReceiptConflict => format!(
                "{}: a different receipt is stored for this attempt",
                ServerErrorCode::EffectReceiptConflict
            ),
        }
    }
}

type Row = serde_json::Map<String, serde_json::Value>;

/// The WorkItem row of `tenant`, or the not-found refusal a foreign tenant
/// also gets: another tenant's item is never named.
fn tenant_row(
    nodes: &ScopedOwnerTableMut<'_, (&'static str, &'static str), &'static [u8]>,
    graph: &str,
    tenant: &str,
    work_item_id: &str,
    crypto: DurableCrypto<'_>,
) -> Result<Row, String> {
    let row: Option<Row> = nodes
        .get((graph, work_item_id))?
        .map(|bytes| decode_durable(&crypto.unseal(bytes.value())?))
        .transpose()?;
    row.filter(|row| {
        property_string(row, "node_type") == "WorkItem" && property_string(row, "tenant") == tenant
    })
    .ok_or_else(|| {
        format!(
            "{}: WorkItem unavailable",
            medulla_types::contract::EngineErrorCode::AccessDenied
        )
    })
}

/// Write `row` after an effect transition already bumped its revision.
/// `write_work_item_props` bumps it once more, so the stored revision is put
/// back by one first: each effect write moves the revision by exactly one.
fn write_bumped_row(
    nodes: &mut ScopedOwnerTableMut<'_, (&'static str, &'static str), &'static [u8]>,
    graph: &str,
    work_item_id: &str,
    row: &mut Row,
    crypto: DurableCrypto<'_>,
) -> Result<(), String> {
    let bumped = row_revision(row);
    row.insert(
        WORK_ITEM_ROW_REVISION.into(),
        serde_json::Value::from(bumped.saturating_sub(1)),
    );
    write_work_item_props(nodes, graph, work_item_id, row, crypto)
}

fn decode_record<T: serde::de::DeserializeOwned>(
    table: &EffectTable<'_>,
    key: (&str, &str, u64),
    crypto: DurableCrypto<'_>,
) -> Result<Option<T>, String> {
    table
        .get(key)?
        .map(|bytes| decode_durable(&crypto.unseal(bytes.value())?))
        .transpose()
}

fn put_record<T: serde::Serialize>(
    table: &mut EffectTable<'_>,
    key: (&str, &str, u64),
    record: &T,
    crypto: DurableCrypto<'_>,
) -> Result<(), String> {
    let bytes = rmp_serde::to_vec_named(record).map_err(|e| e.to_string())?;
    table.insert(key, crypto.seal(&bytes).as_ref())?;
    Ok(())
}

/// The served `MarkWorkItemEffectStarted` arm. Refusals write nothing.
pub fn apply_mark_effect_started_row(
    graph: &str,
    request: &MarkWorkItemEffectStartedRequest,
    nodes: &mut ScopedOwnerTableMut<'_, (&'static str, &'static str), &'static [u8]>,
    committed_at_ms: u64,
    crypto: DurableCrypto<'_>,
) -> Result<EffectWriteResult, String> {
    let mut row = tenant_row(nodes, graph, &request.tenant, &request.work_item_id, crypto)?;
    let before = row_revision(&row);
    let fence = EffectFence {
        tenant: &request.tenant,
        worker_id: &request.worker_id,
        lease_epoch: request.lease_epoch,
        fencing_token: request.fencing_token,
        expected_revision: request.expected_revision,
    };
    let revision = apply_mark_effect_started(&mut row, &fence, committed_at_ms as f64 / 1000.0)
        .map_err(|refusal| refusal.to_wire())?;
    let replayed = revision == before;
    if !replayed {
        write_bumped_row(nodes, graph, &request.work_item_id, &mut row, crypto)?;
    }
    Ok(EffectWriteResult {
        changed_work_item_ids: if replayed {
            Vec::new()
        } else {
            vec![request.work_item_id.clone()]
        },
        work_item_id: request.work_item_id.clone(),
        attempt: property_u64(&row, "attempt"),
        revision,
        replayed,
        receipt: None,
    })
}

/// The served `RecordEffectReceipt` arm: the receipt and the `Resolved` phase
/// commit in one transaction. The receipt must name this item (job ID), the
/// admitted request of this attempt (or the item's input when none was
/// admitted) and the worker holding the started claim, at the started attempt.
pub fn apply_record_effect_receipt_row(
    graph: &str,
    request: &RecordEffectReceiptRequest,
    nodes: &mut ScopedOwnerTableMut<'_, (&'static str, &'static str), &'static [u8]>,
    receipts: &mut EffectTable<'_>,
    admitted: &EffectTable<'_>,
    committed_at_ms: u64,
    crypto: DurableCrypto<'_>,
) -> Result<EffectWriteResult, String> {
    let mut row = tenant_row(nodes, graph, &request.tenant, &request.work_item_id, crypto)?;
    let key = (graph, request.work_item_id.as_str(), request.attempt);
    if property_u64(&row, "attempt") != request.attempt {
        return Err(EffectRefusal::ReceiptMismatch.to_wire());
    }
    let stored: Option<EffectReceipt> = decode_record(receipts, key, crypto)?;
    if stored.is_none() && !effect_started(&row) {
        return Err(EffectRefusal::ReceiptMismatch.to_wire());
    }
    let admitted_record: Option<AdmittedRequestRecord> = decode_record(admitted, key, crypto)?;
    let request_ref = admitted_record.map_or_else(
        || property_string(&row, "payload_ref").to_string(),
        |record| record.request_ref,
    );
    let principal_ref = property_string(&row, "lease_owner").to_string();
    let write = apply_record_effect_receipt(
        &mut row,
        &request.work_item_id,
        (&request_ref, &principal_ref),
        request.receipt.clone(),
        stored.as_ref(),
        request.expected_revision,
        committed_at_ms as f64 / 1000.0,
    )
    .map_err(|refusal| refusal.to_wire())?;
    let (receipt, replayed) = match write {
        ReceiptWrite::Replay(receipt) => (receipt, true),
        ReceiptWrite::Store(receipt) => {
            put_record(receipts, key, &receipt, crypto)?;
            write_bumped_row(nodes, graph, &request.work_item_id, &mut row, crypto)?;
            (receipt, false)
        }
    };
    Ok(EffectWriteResult {
        changed_work_item_ids: if replayed {
            Vec::new()
        } else {
            vec![request.work_item_id.clone()]
        },
        work_item_id: request.work_item_id.clone(),
        attempt: request.attempt,
        revision: row_revision(&row),
        replayed,
        receipt: Some(receipt),
    })
}

/// The served `SubmitAdmittedRequest` arm: stores the digest and payload ref
/// once under `(work_item_id, attempt)`. An identical resubmission replays; a
/// different request under the same key refuses as a conflict.
pub fn apply_submit_admitted_request_row(
    graph: &str,
    request: &SubmitAdmittedRequestRequest,
    nodes: &ScopedOwnerTableMut<'_, (&'static str, &'static str), &'static [u8]>,
    admitted: &mut EffectTable<'_>,
    crypto: DurableCrypto<'_>,
) -> Result<EffectWriteResult, String> {
    let record = request.request.admit()?;
    let row = tenant_row(nodes, graph, &record.tenant, &request.work_item_id, crypto)?;
    check_revision(&row, request.expected_revision).map_err(|refusal| refusal.to_wire())?;
    let key = (graph, request.work_item_id.as_str(), request.attempt);
    let replayed = match decode_record::<AdmittedRequestRecord>(admitted, key, crypto)? {
        Some(stored) if stored == record => true,
        Some(_) => {
            return Err(format!(
                "{}: a different request is admitted for this attempt",
                medulla_types::contract::EngineErrorCode::Conflict
            ))
        }
        None => {
            put_record(admitted, key, &record, crypto)?;
            false
        }
    };
    Ok(EffectWriteResult {
        changed_work_item_ids: Vec::new(),
        work_item_id: request.work_item_id.clone(),
        attempt: request.attempt,
        revision: row_revision(&row),
        replayed,
        receipt: None,
    })
}

/// `GetEffectReceipt`: the receipt of `(work_item_id, attempt)`, `None` when
/// absent or when the item is not the caller tenant's.
pub fn read_effect_receipt(
    shard: &Shard,
    graph: &str,
    key: &EffectRecordKey,
    crypto: DurableCrypto<'_>,
) -> Result<Option<EffectReceipt>, String> {
    if super::read_work_item(shard, graph, &key.tenant, &key.work_item_id, crypto)?.is_none() {
        return Ok(None);
    }
    let handle = shard.graph(graph)?;
    let read = shard.read(&handle)?;
    let table = read.scoped_owner_table(EFFECT_RECEIPTS)?;
    table
        .get((graph, key.work_item_id.as_str(), key.attempt))?
        .map(|bytes| decode_durable(&crypto.unseal(bytes.value())?))
        .transpose()
}

/// `GetAdmittedRequest`: the digest and payload ref, `None` when absent or
/// when the record belongs to another tenant or session.
pub fn read_admitted_request(
    shard: &Shard,
    graph: &str,
    key: &EffectRecordKey,
    crypto: DurableCrypto<'_>,
) -> Result<Option<AdmittedRequestRecord>, String> {
    let handle = shard.graph(graph)?;
    let read = shard.read(&handle)?;
    let table = read.scoped_owner_table(ADMITTED_REQUESTS)?;
    let record: Option<AdmittedRequestRecord> = table
        .get((graph, key.work_item_id.as_str(), key.attempt))?
        .map(|bytes| decode_durable(&crypto.unseal(bytes.value())?))
        .transpose()?;
    Ok(record.filter(|record| {
        record.tenant == key.tenant
            && key
                .session_ref
                .as_deref()
                .is_some_and(|session| session == record.session_ref)
    }))
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
