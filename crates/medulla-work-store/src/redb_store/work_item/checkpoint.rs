//! Private immutable checkpoints and the WorkItem head share one native transaction.
use super::*;
use medulla_work_types::{
    run_checkpoint::CheckpointClaim, run_checkpoint::CheckpointContent,
    run_checkpoint::GetRunCheckpointRequest, run_checkpoint::PublishRunCheckpointRequest,
    run_checkpoint::RunCheckpointPublished, run_checkpoint::RunCheckpointRead,
    run_checkpoint::RunCheckpointSnapshot,
};
use medulla_work_types::{
    work_item_read::WORK_ITEM_CHECKPOINT_DIGEST, work_item_read::WORK_ITEM_CHECKPOINT_HISTORY,
    work_item_read::WORK_ITEM_CHECKPOINT_REF, work_item_read::WORK_ITEM_CHECKPOINT_REVISION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const RUN_CHECKPOINTS: redb::TableDefinition<(&str, &str), &[u8]> =
    redb::TableDefinition::new("work_item_run_checkpoints");
type Row = serde_json::Map<String, serde_json::Value>;

/// Most checkpoint revisions one WorkItem run retains. The 65th publish deletes the oldest
/// revision's `RUN_CHECKPOINTS` row in its own transaction; the head is never pruned
/// (MEDULLA-DURABLE-KERNEL-R028.1.2.1).
const SHORT_TERM_MAX_REVISIONS: usize = 64;

/// The retained revision references of a run, oldest first. A row written before the
/// history existed holds none, so its head is the one retained revision.
fn history(row: &Row, head_reference: Option<&str>) -> Result<Vec<String>, String> {
    match row.get(WORK_ITEM_CHECKPOINT_HISTORY) {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|_| "checkpoint history is malformed".to_string()),
        None => Ok(head_reference.map(str::to_owned).into_iter().collect()),
    }
}

fn admitted_context(row: &Row, graph: &str, tenant: &str, run_id: &str, context_id: &str) -> bool {
    let Some(value) = row.get(run_owner::ADMISSION_REQUEST) else {
        return false;
    };
    let Ok(admission) = serde_json::from_value::<
        medulla_work_types::native_control::SubmitWorkItemRequest,
    >(value.clone()) else {
        return false;
    };
    admission.run_id.as_deref() == Some(run_id)
        && admission.context.tenant_id == tenant
        && medulla_kernel::persist::sanitize(&admission.context.graph) == graph
        && admission.work_item_id.is_none()
        && admission_context_matches(&admission, context_id)
}

fn admission_context_matches(
    admission: &medulla_work_types::native_control::SubmitWorkItemRequest,
    context_id: &str,
) -> bool {
    let legacy = admission.metadata.get("a2a_context_id");
    match admission.conversation.as_deref() {
        Some(turn) => {
            turn.context_id == context_id
                && legacy.is_none_or(|value| value.as_str() == Some(context_id))
        }
        None => legacy.and_then(serde_json::Value::as_str) == Some(context_id),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCheckpoint {
    pub tenant: String,
    pub work_item_id: String,
    pub run_id: String,
    pub context_id: String,
    pub owner: String,
    pub claim: CheckpointClaim,
    pub snapshot: RunCheckpointSnapshot,
}

fn identity_matches(graph: &str, tenant: &str, run: &str, id: &str) -> Result<bool, String> {
    Ok(run_owner::work_item_id(graph, tenant, run)? == id)
}

pub fn is_owner(
    shard: &Shard,
    graph: &str,
    request: &GetRunCheckpointRequest,
    actor: &str,
    crypto: DurableCrypto<'_>,
) -> Result<bool, String> {
    request.validate()?;
    if !identity_matches(
        graph,
        &request.tenant,
        &request.run_id,
        &request.work_item_id,
    )? {
        return Ok(false);
    }
    let (_, row) = read::snapshot_row(shard, graph, &request.work_item_id, crypto)?;
    Ok(row.is_some_and(|row| {
        admitted_context(
            &row,
            graph,
            &request.tenant,
            &request.run_id,
            &request.context_id,
        ) && run_owner::owned(
            &row,
            graph,
            &request.tenant,
            &request.run_id,
            &request.work_item_id,
            actor,
        )
    }))
}

fn head(row: &Row) -> Result<Option<(String, u64, String)>, String> {
    let values = (
        row.get(WORK_ITEM_CHECKPOINT_REF),
        row.get(WORK_ITEM_CHECKPOINT_REVISION),
        row.get(WORK_ITEM_CHECKPOINT_DIGEST),
    );
    if values == (None, None, None) {
        return Ok(None);
    }
    match values {
        (Some(reference), Some(revision), Some(digest)) => {
            match (reference.as_str(), revision.as_u64(), digest.as_str()) {
                (Some(reference), Some(revision), Some(digest)) if revision > 0 => {
                    Ok(Some((reference.to_owned(), revision, digest.to_owned())))
                }
                _ => Err("checkpoint head is malformed".into()),
            }
        }
        _ => Err("checkpoint head is incomplete".into()),
    }
}

fn verify_stored(
    stored: &StoredCheckpoint,
    request: &GetRunCheckpointRequest,
    actor: &str,
    revision: u64,
    digest: &str,
) -> Result<(), String> {
    if stored.tenant != request.tenant
        || stored.run_id != request.run_id
        || stored.work_item_id != request.work_item_id
        || stored.context_id != request.context_id
        || stored.owner != actor
        || stored.snapshot.revision != revision
        || stored.snapshot.payload_digest != digest
    {
        return Err("checkpoint binding mismatch".into());
    }
    CheckpointContent {
        content: stored.snapshot.content.clone(),
        payload_digest: stored.snapshot.payload_digest.clone(),
        short_term: stored.snapshot.short_term.clone(),
        long_term: stored.snapshot.long_term.clone(),
    }
    .validate()
}

/// Every referenced memory revision must exist on this tenant graph with the
/// stated digest. A revision that lives on another graph is not visible here,
/// so it refuses with the same bytes as an absent one.
fn require_memory_refs(
    tables: &crate::redb_store::graph_row_tables::GraphRowTables<'_>,
    graph: &str,
    content: &CheckpointContent,
    crypto: DurableCrypto<'_>,
) -> Result<(), String> {
    use medulla_mutation_types::memory_sync::{revision_node_id, DocId};
    use medulla_types::contract::ServerErrorCode;
    let missing = || {
        format!(
            "{}: a referenced memory revision does not exist",
            ServerErrorCode::CheckpointMemoryRefMissing
        )
    };
    for reference in content.short_term.iter().chain(content.long_term.iter()) {
        let doc = DocId::new(reference.document_id.clone()).map_err(|_| missing())?;
        let id = revision_node_id(&doc, reference.revision);
        let bytes = tables
            .nodes
            .get((graph, id.as_str()))?
            .ok_or_else(missing)?;
        let row: serde_json::Value =
            decode_durable(&crypto.unseal(bytes.value())?).map_err(|_| missing())?;
        let digest = row
            .pointer("/head/content_digest")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(missing)?;
        if digest != reference.digest {
            return Err(format!(
                "{}: a referenced memory revision has a different digest",
                ServerErrorCode::CheckpointDigestMismatch
            ));
        }
    }
    Ok(())
}

pub fn publish(
    graph: &str,
    request: &PublishRunCheckpointRequest,
    actor: Option<&str>,
    tables: &mut crate::redb_store::graph_row_tables::GraphRowTables<'_>,
    crypto: DurableCrypto<'_>,
    now_ms: u64,
) -> Result<RunCheckpointPublished, String> {
    request.validate()?;
    let actor = actor.unwrap_or_default();
    let mut row: Row = {
        let bytes = tables
            .nodes
            .get((graph, request.work_item_id.as_str()))?
            .ok_or("ACCESS_DENIED: checkpoint owner unavailable")?;
        decode_durable(&crypto.unseal(bytes.value())?)?
    };
    if !identity_matches(
        graph,
        &request.tenant,
        &request.run_id,
        &request.work_item_id,
    )? || !run_owner::owned(
        &row,
        graph,
        &request.tenant,
        &request.run_id,
        &request.work_item_id,
        actor,
    ) {
        return Err("ACCESS_DENIED: checkpoint owner unavailable".into());
    }
    work_item_capability::require_native_claim(
        &tables.native_work_items,
        graph,
        &request.work_item_id,
        crypto,
    )?;
    if !current_plan::live(
        &row,
        &request.worker_id,
        request.lease_epoch,
        request.fencing_token,
        now_ms,
    ) {
        return Err("CONFLICT: checkpoint live claim conflict".into());
    }
    if !admitted_context(
        &row,
        graph,
        &request.tenant,
        &request.run_id,
        &request.context_id,
    ) {
        return Err("ACCESS_DENIED: checkpoint context mismatch".into());
    }
    require_memory_refs(tables, graph, &request.snapshot, crypto)?;
    let previous = head(&row)?;
    if previous.as_ref().map(|(_, revision, _)| *revision) != request.expected_revision {
        return Err("CONFLICT: checkpoint expected revision conflict".into());
    }
    let previous_reference = previous.as_ref().map(|(reference, _, _)| reference.clone());
    if let Some((reference, revision, digest)) = previous {
        let bytes = tables
            .run_checkpoints
            .get((graph, reference.as_str()))?
            .ok_or("checkpoint snapshot is missing")?;
        let stored: StoredCheckpoint = decode_durable(&crypto.unseal(bytes.value())?)?;
        verify_stored(
            &stored,
            &GetRunCheckpointRequest {
                tenant: request.tenant.clone(),
                run_id: request.run_id.clone(),
                context_id: request.context_id.clone(),
                work_item_id: request.work_item_id.clone(),
                current_claim: None,
            },
            actor,
            revision,
            &digest,
        )?;
    }
    let revision = request
        .expected_revision
        .unwrap_or(0)
        .checked_add(1)
        .ok_or("checkpoint revision exhausted")?;
    let digest = &request.snapshot.payload_digest;
    let reference = format!(
        "checkpoint:{}",
        hex::encode(Sha256::digest(
            serde_json::to_vec(&(
                graph,
                &request.tenant,
                &request.work_item_id,
                &request.run_id,
                &request.context_id,
                revision,
                digest
            ))
            .map_err(|e| e.to_string())?
        ))
    );
    if tables
        .run_checkpoints
        .get((graph, reference.as_str()))?
        .is_some()
    {
        return Err("checkpoint immutable revision collision".into());
    }
    let stored = StoredCheckpoint {
        tenant: request.tenant.clone(),
        work_item_id: request.work_item_id.clone(),
        run_id: request.run_id.clone(),
        context_id: request.context_id.clone(),
        owner: actor.to_owned(),
        claim: CheckpointClaim {
            worker_id: request.worker_id.clone(),
            lease_epoch: request.lease_epoch,
            fencing_token: request.fencing_token,
        },
        snapshot: RunCheckpointSnapshot {
            revision,
            content: request.snapshot.content.clone(),
            payload_digest: digest.clone(),
            short_term: request.snapshot.short_term.clone(),
            long_term: request.snapshot.long_term.clone(),
        },
    };
    let bytes = rmp_serde::to_vec_named(&stored).map_err(|e| e.to_string())?;
    tables
        .run_checkpoints
        .insert((graph, reference.as_str()), crypto.seal(&bytes).as_ref())?;
    // The prune shares this transaction, so a refused publish deletes nothing.
    let mut retained = history(&row, previous_reference.as_deref())?;
    retained.push(reference.clone());
    while retained.len() > SHORT_TERM_MAX_REVISIONS {
        let oldest = retained.remove(0);
        if tables
            .run_checkpoints
            .get((graph, oldest.as_str()))?
            .is_none()
        {
            return Err("checkpoint snapshot is missing".into());
        }
        tables.run_checkpoints.remove((graph, oldest.as_str()))?;
    }
    row.insert(
        WORK_ITEM_CHECKPOINT_HISTORY.into(),
        serde_json::to_value(&retained).map_err(|e| e.to_string())?,
    );
    row.insert(WORK_ITEM_CHECKPOINT_REF.into(), reference.into());
    row.insert(WORK_ITEM_CHECKPOINT_REVISION.into(), revision.into());
    row.insert(WORK_ITEM_CHECKPOINT_DIGEST.into(), digest.clone().into());
    row.insert("updated_at".into(), (now_ms as f64 / 1000.0).into());
    write_work_item_props(
        &mut tables.nodes,
        graph,
        &request.work_item_id,
        &mut row,
        crypto,
    )?;
    Ok(RunCheckpointPublished {
        changed_work_item_ids: vec![request.work_item_id.clone()],
        replayed: false,
        work_item_id: request.work_item_id.clone(),
        run_id: request.run_id.clone(),
        context_id: request.context_id.clone(),
        revision,
        payload_digest: digest.clone(),
    })
}

pub fn read_current(
    shard: &Shard,
    graph: &str,
    request: &GetRunCheckpointRequest,
    actor: &str,
    crypto: DurableCrypto<'_>,
    now_ms: u64,
) -> Result<Option<RunCheckpointRead>, String> {
    request.validate()?;
    if !identity_matches(
        graph,
        &request.tenant,
        &request.run_id,
        &request.work_item_id,
    )? {
        return Ok(None);
    }
    let handle = shard.graph(graph)?;
    let read = shard.read(&handle)?;
    let nodes = read.scoped_owner_table(NODES)?;
    let row = nodes
        .get((graph, request.work_item_id.as_str()))?
        .map(|bytes| decode_durable::<Row>(&crypto.unseal(bytes.value())?))
        .transpose()?;
    let Some(row) = row else {
        return Ok(None);
    };
    if !run_owner::owned(
        &row,
        graph,
        &request.tenant,
        &request.run_id,
        &request.work_item_id,
        actor,
    ) {
        return Ok(None);
    }
    if !admitted_context(
        &row,
        graph,
        &request.tenant,
        &request.run_id,
        &request.context_id,
    ) {
        return Ok(None);
    }
    if let Some(claim) = &request.current_claim {
        if !current_plan::live(
            &row,
            &claim.worker_id,
            claim.lease_epoch,
            claim.fencing_token,
            now_ms,
        ) {
            return Err(format!(
                "{}: checkpoint read requires the live lease epoch and fencing token",
                medulla_types::contract::ServerErrorCode::CheckpointFenceStale
            ));
        }
    }
    let Some((reference, revision, digest)) = head(&row)? else {
        return Ok(None);
    };
    let snapshots = read.scoped_owner_table(RUN_CHECKPOINTS)?;
    let bytes = snapshots
        .get((graph, reference.as_str()))?
        .ok_or("checkpoint snapshot is missing")?;
    let stored: StoredCheckpoint = decode_durable(&crypto.unseal(bytes.value())?)?;
    verify_stored(&stored, request, actor, revision, &digest)?;
    let work_item = medulla_work_types::work_item_read::WorkItemView::from_tenant_row(
        &request.work_item_id,
        &row,
        &request.tenant,
    )?
    .ok_or("checkpoint WorkItem projection unavailable")?;
    Ok(Some(RunCheckpointRead {
        context_id: request.context_id.clone(),
        work_item,
        snapshot: stored.snapshot,
    }))
}

#[cfg(test)]
mod tests;
