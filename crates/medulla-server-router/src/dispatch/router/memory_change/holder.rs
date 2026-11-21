//! The blob holder of committed memory snapshot content
//! (MEDULLA-DURABLE-KERNEL-R085.4, R085.7).
//!
//! The content goes into the CAS through the same path ingestion uses for raw
//! bodies (`put_chunk`) BEFORE the graph commit: it is content-addressed and
//! idempotent, so a storage failure refuses `MEMSYNC_STORAGE_UNAVAILABLE` before
//! anything commits. After the commit one blob-domain batch registers the holder
//! `memory_snapshot:<doc_id>@<revision>` (`@c<n>` for a candidate) under the document owner's scope
//! ([`medulla_server_api::carrier::memory_snapshot_owner_scope`]), the same row
//! the `memory_snapshot_synced` consumer registers. A holder failure after the
//! commit is never a failure of the write: the sync answers success with a
//! warning and the consumer repairs the holder. The holder is set-like, so a
//! replayed sync takes nothing twice. A graph handler never calls the blob
//! store: dependency direction keeps this in the router.

use medulla_server_api::authority_context::VerifiedRequestContext;
use std::sync::Arc;
use tokio::sync::RwLock;

/// The warning a committed sync carries when its holder could not be registered.
pub(super) const HOLDER_PENDING: &str = "MEMSYNC_HOLDER_PENDING";

/// Content already in the CAS and the holder it will take once the graph commits.
pub(super) struct StagedContent {
    digest: String,
    slot: medulla_graph_core::memory_snapshot::SnapshotSlot,
}

/// Whether the CAS serves the content at `digest`. A build or server without the
/// blob substrate has nothing to check, so it counts as held; a store that fails
/// to answer or lacks the content does not.
#[cfg(all(feature = "redb", feature = "blob"))]
pub(super) async fn content_held(
    state: &Arc<RwLock<medulla_server_api::state::ServerState>>,
    digest: &str,
) -> bool {
    use medulla_server_blob::blob::BlobStateExt;

    let store = {
        let s = state.read().await;
        match s.blob_cursors() {
            Some(cursors) => Arc::clone(&cursors.store),
            None => return true,
        }
    };
    matches!(store.get_chunk(digest), Ok(Some(_)))
}

#[cfg(not(all(feature = "redb", feature = "blob")))]
pub(super) async fn content_held(
    _: &Arc<RwLock<medulla_server_api::state::ServerState>>,
    _: &str,
) -> bool {
    true
}

/// Put the content a planned sync stores into the CAS. A sync that stores none,
/// or a build without the blob substrate, stages nothing.
#[cfg(all(feature = "redb", feature = "blob"))]
pub(super) async fn stage_content(
    state: &Arc<RwLock<medulla_server_api::state::ServerState>>,
    request: &medulla_mutation_types::memory_sync::SyncRequest,
    plan: &medulla_mutation_types::memory_sync::SnapshotPlan,
    outcome: &medulla_mutation_types::memory_sync::SyncOutcome,
) -> Result<Option<StagedContent>, String> {
    use medulla_h_graph::graph_ops::memory_snapshot::planned_content;
    use medulla_mutation_types::memory_sync::MemorySyncRefusal;
    use medulla_server_blob::blob::BlobStateExt;

    let Some((slot, content)) = planned_content(plan, &request.doc_id, outcome) else {
        return Ok(None);
    };
    let store = {
        let s = state.read().await;
        match s.blob_cursors() {
            Some(cursors) => Arc::clone(&cursors.store),
            None => return Ok(None),
        }
    };
    let (digest, _was_new) = store.put_chunk(content.as_bytes()).map_err(|error| {
        format!("{}: {error}", MemorySyncRefusal::StorageUnavailable.as_str())
    })?;
    Ok(Some(StagedContent { digest, slot }))
}

#[cfg(not(all(feature = "redb", feature = "blob")))]
pub(super) async fn stage_content(
    _: &Arc<RwLock<medulla_server_api::state::ServerState>>,
    _: &medulla_mutation_types::memory_sync::SyncRequest,
    _: &medulla_mutation_types::memory_sync::SnapshotPlan,
    _: &medulla_mutation_types::memory_sync::SyncOutcome,
) -> Result<Option<StagedContent>, String> {
    Ok(None)
}

/// Register the holder of staged content after its graph commit.
#[cfg(all(feature = "redb", feature = "blob"))]
pub(super) async fn hold_staged(
    state: &Arc<RwLock<medulla_server_api::state::ServerState>>,
    verified: &VerifiedRequestContext,
    request_id: u64,
    request: &medulla_mutation_types::memory_sync::SyncRequest,
    staged: &StagedContent,
) -> Result<(), String> {
    use medulla_server_blob::blob::store::{HolderChange, HolderId};
    use medulla_server_blob::blob::BlobStateExt;

    let store = {
        let s = state.read().await;
        match s.blob_cursors() {
            Some(cursors) => Arc::clone(&cursors.store),
            None => return Ok(()),
        }
    };
    let authority = medulla_server_api::carrier::CarrierAuthority::from_verified(verified)?;
    let holder = HolderChange::acquire(
        &staged.digest,
        HolderId::new(&staged.slot.holder_id(&request.doc_id))?,
        &medulla_server_api::carrier::memory_snapshot_owner_scope(verified.tenant(), request.doc_id.as_str()),
    )?;
    let method = medulla_protocol_types::protocol::Method::BlobRef {
        digest: staged.digest.clone(),
    };
    // The blob-owner scope version is engine-owned: a concurrent writer that
    // advanced it between the compile and the commit is a WRITE_CONFLICT to
    // retry with a fresh compile. The holder change is set-like.
    const MAX_ATTEMPTS: usize = 4;
    let mut attempt = 1;
    loop {
        let (batch, now) = medulla_server_ops::api::ingest::compile_blob_batch(
            store.as_ref(),
            request_id,
            &authority,
            &method,
        )?;
        match store.holder_batch(&holder, &batch, now) {
            Err(error) if error.contains("WRITE_CONFLICT:") && attempt < MAX_ATTEMPTS => {
                attempt += 1;
            }
            other => return other.map(|_| ()),
        }
    }
}

#[cfg(not(all(feature = "redb", feature = "blob")))]
pub(super) async fn hold_staged(
    _: &Arc<RwLock<medulla_server_api::state::ServerState>>,
    _: &VerifiedRequestContext,
    _: u64,
    _: &medulla_mutation_types::memory_sync::SyncRequest,
    _: &StagedContent,
) -> Result<(), String> {
    Ok(())
}
