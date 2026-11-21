//! The blob holder of committed memory snapshot content
//! (MEDULLA-DURABLE-KERNEL-R085.4, R085.7).
//!
//! After the graph commit the content goes into the CAS through the same path
//! ingestion uses for raw bodies (`put_chunk`), then one blob-domain batch
//! registers the holder `memory_snapshot:<doc_id>@<revision>` under the caller's
//! owner scope. The holder is set-like, so a replayed sync takes nothing twice.
//! A graph handler never calls the blob store: dependency direction keeps this
//! in the router.

use medulla_server_api::authority_context::VerifiedRequestContext;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Keep the content a committed sync stored. A sync that stored none, or a
/// build without the blob substrate, holds nothing.
#[cfg(all(feature = "redb", feature = "blob"))]
pub(super) async fn hold_content(
    state: &Arc<RwLock<medulla_server_api::state::ServerState>>,
    verified: &VerifiedRequestContext,
    request_id: u64,
    core: &medulla_graph_core::graph::GraphCore,
    request: &medulla_mutation_types::memory_sync::SyncRequest,
    outcome: &medulla_mutation_types::memory_sync::SyncOutcome,
) -> Result<(), String> {
    use medulla_h_graph::graph_ops::memory_snapshot::held_content;
    use medulla_mutation_types::memory_sync::MemorySyncRefusal;
    use medulla_server_blob::blob::store::{HolderChange, HolderId};
    use medulla_server_blob::blob::BlobStateExt;

    let Some((revision, content)) = held_content(core, &request.doc_id, outcome) else {
        return Ok(());
    };
    let unavailable = |error: String| {
        format!(
            "{}: {error}",
            MemorySyncRefusal::StorageUnavailable.as_str()
        )
    };
    let store = {
        let s = state.read().await;
        match s.blob_cursors() {
            Some(cursors) => Arc::clone(&cursors.store),
            None => return Ok(()),
        }
    };
    let authority =
        medulla_server_api::carrier::CarrierAuthority::from_verified(verified).map_err(unavailable)?;
    let (digest, _was_new) = store.put_chunk(content.as_bytes()).map_err(unavailable)?;
    let holder = HolderChange::acquire(
        &digest,
        HolderId::new(&format!(
            "memory_snapshot:{}@{revision}",
            request.doc_id.as_str()
        ))
        .map_err(unavailable)?,
        authority.owner_scope(),
    )
    .map_err(unavailable)?;
    let method = medulla_protocol_types::protocol::Method::BlobRef {
        digest: digest.clone(),
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
        )
        .map_err(unavailable)?;
        match store.holder_batch(&holder, &batch, now) {
            Err(error) if error.contains("WRITE_CONFLICT:") && attempt < MAX_ATTEMPTS => {
                attempt += 1;
            }
            other => return other.map(|_| ()).map_err(unavailable),
        }
    }
}

#[cfg(not(all(feature = "redb", feature = "blob")))]
pub(super) async fn hold_content(
    _: &Arc<RwLock<medulla_server_api::state::ServerState>>,
    _: &VerifiedRequestContext,
    _: u64,
    _: &medulla_graph_core::graph::GraphCore,
    _: &medulla_mutation_types::memory_sync::SyncRequest,
    _: &medulla_mutation_types::memory_sync::SyncOutcome,
) -> Result<(), String> {
    Ok(())
}
