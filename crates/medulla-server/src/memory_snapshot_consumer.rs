//! The `memory_snapshot_synced` outbox consumer (MEDULLA-DURABLE-KERNEL-R085.4).
//!
//! Every committed sync that wrote a row carries one `memory_snapshot_synced`
//! intent in the same batch as its head and revision rows. This consumer claims
//! those intents through the durable outbox (subscribe, claim, ack, release) as
//! the engine service actor [`SERVICE_ACTOR`], reads the committed rows of the
//! named document, and makes sure the blob holder of each retained revision and
//! candidate exists under the document owner's scope. The router takes the holder right after the commit; the
//! consumer repairs the one window that leaves open (a crash between the commit
//! and the holder), and the claim is acknowledged only once every holder stands.
//! The holder change is set-like, so a redelivered intent takes nothing twice.
//! A revision older than the newest `memory_sync.max_revisions_per_doc` of its
//! document is released here, oldest first, and so is a superseded revision or a
//! resolved candidate once `memory_sync.history_retention_s` has passed since
//! the commit that superseded or resolved it; a periodic tick releases past
//! deadlines even when no sync arrives. An unresolved conflict candidate is
//! never released.

use std::sync::Arc;

use medulla_server_api::state::ServerState;
use tokio::sync::RwLock;

/// The durable consumer name the outbox cursor is kept under.
pub const CONSUMER: &str = "memory-snapshot-holder";
/// The outbox topic the committed sync intents are published under.
pub const TOPIC: &str = "memory_snapshot_synced";
/// The engine service actor the consumer runs as.
pub const SERVICE_ACTOR: &str = "engine:memory-snapshot";

/// Start the consumer loop where the blob substrate and the durable outbox exist.
pub fn spawn(state: Arc<RwLock<ServerState>>) {
    #[cfg(all(feature = "redb", feature = "blob"))]
    tokio::spawn(worker::run(state));
    #[cfg(not(all(feature = "redb", feature = "blob")))]
    drop(state);
}

#[cfg(all(feature = "redb", feature = "blob"))]
mod worker {
    use super::*;
    use medulla_graph_core::memory_snapshot as store;
    use medulla_mutation_types::memory_sync::{DocId, SyncRequest};
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_server_blob::blob::store::{ChunkStore, HolderChange, HolderId};
    use medulla_server_blob::blob::BlobStateExt;
    use std::time::Duration;

    const IDLE: Duration = Duration::from_millis(500);
    const RETENTION_TICK: Duration = Duration::from_secs(60);
    const LEASE_MS: u64 = 60_000;
    const MAX_GRAPHS_PER_SWEEP: usize = 16;
    const MAX_PAYLOAD_BYTES: usize = 16 * 1024;
    const MAX_ATTEMPTS: usize = 4;

    pub(super) async fn run(state: Arc<RwLock<ServerState>>) {
        let mut next_graph = 0_usize;
        let mut last_tick = std::time::Instant::now();
        loop {
            if last_tick.elapsed() >= RETENTION_TICK {
                last_tick = std::time::Instant::now();
                if let Err(error) = release_expired(&state).await {
                    tracing::warn!(code = "MEMORY_SNAPSHOT_RETENTION_TICK_FAILED", %error,
                        "memory snapshot retention tick failed");
                }
            }
            match sweep(&state, &mut next_graph).await {
                Ok(true) => {}
                Ok(false) => tokio::time::sleep(IDLE).await,
                Err(error) => {
                    tracing::warn!(code = "MEMORY_SNAPSHOT_CONSUMER_SWEEP_FAILED", %error,
                        "memory snapshot consumer sweep failed");
                    tokio::time::sleep(IDLE).await;
                }
            }
        }
    }

    async fn sweep(
        state: &Arc<RwLock<ServerState>>,
        next_graph: &mut usize,
    ) -> Result<bool, String> {
        let mut graphs: Vec<String> = state
            .read()
            .await
            .registry
            .list()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        if graphs.is_empty() {
            *next_graph = 0;
            return Ok(false);
        }
        graphs.sort_unstable();
        let start = *next_graph % graphs.len();
        let count = graphs.len().min(MAX_GRAPHS_PER_SWEEP);
        *next_graph = (start + count) % graphs.len();
        let mut progressed = false;
        let mut first_error = None;
        for offset in 0..count {
            match sweep_graph(state, &graphs[(start + offset) % graphs.len()]).await {
                Ok(true) => progressed = true,
                Ok(false) => {}
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(progressed), Err)
    }

    async fn sweep_graph(state: &Arc<RwLock<ServerState>>, graph: &str) -> Result<bool, String> {
        let persistence = state
            .read()
            .await
            .persistence
            .clone()
            .ok_or("CONFLICT: memory snapshot consumer persistence is unavailable")?;
        let fname = medulla_kernel::persist::sanitize(graph);
        persistence
            .subscribe_mutation_outbox(&fname, CONSUMER, TOPIC)
            .await?;
        let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        let mut budget = medulla_transaction::OutboxClaimBudget::new(1, LEASE_MS, now_ms)?;
        let outcome = persistence
            .claim_mutation_outbox(&fname, CONSUMER, &mut budget)
            .await?;
        let mut progressed = false;
        for lease in outcome.claims {
            match hold(state, graph, &lease.record.intent).await {
                Ok(()) => {
                    let now = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
                    persistence.ack_mutation_outbox(&fname, &lease, now).await?;
                    progressed = true;
                }
                Err(error) => {
                    // The intent stays durable: release it for a later claim.
                    persistence.release_mutation_outbox(&fname, &lease).await?;
                    return Err(error);
                }
            }
        }
        Ok(progressed)
    }

    /// The service context the consumer reads and holds under.
    fn service_context(
        anonymous: bool,
        server: &medulla_server_api::server_context::ServerContext,
        key: String,
    ) -> Result<VerifiedRequestContext, String> {
        if anonymous {
            return Ok(VerifiedRequestContext::anonymous_local(Some(key)));
        }
        let service = VerifiedRequestContext::authenticated_fixed_service_actor(
            server,
            SERVICE_ACTOR,
            &["memory:write"],
        )?;
        Ok(VerifiedRequestContext::from_verified_claims_with_nonce(
            service.claims().clone(),
            key,
            Some(medulla_types::contract::Nonce::minted()),
        ))
    }

    /// Hold the blob of every retained revision and candidate of the intent's document.
    async fn hold(
        state: &Arc<RwLock<ServerState>>,
        graph: &str,
        intent: &medulla_mutation_types::mutation_scope::MutationOutboxIntent,
    ) -> Result<(), String> {
        if intent.topic != TOPIC || intent.payload.len() > MAX_PAYLOAD_BYTES {
            return Err("CONFLICT: memory snapshot intent is invalid".into());
        }
        let request: SyncRequest = rmp_serde::from_slice(&intent.payload)
            .map_err(|_| "CONFLICT: memory snapshot intent payload is invalid")?;
        reconcile(state, graph, &request.doc_id, &intent.key, true).await
    }

    /// The deadline tick: release every blob past its retention deadline, for
    /// each document of each resident graph. It holds nothing.
    pub(super) async fn release_expired(state: &Arc<RwLock<ServerState>>) -> Result<(), String> {
        let graphs: Vec<String> = state
            .read()
            .await
            .registry
            .list()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        for graph in graphs {
            let core = state
                .read()
                .await
                .registry
                .get(&graph)
                .map(|entry| entry.core.clone());
            let Some(core) = core else { continue };
            for head in store::list_heads(&core, "") {
                reconcile(state, &graph, &head.doc_id, "retention-tick", false).await?;
            }
        }
        Ok(())
    }

    /// Hold (when `hold_retained`) and release the blobs of one document by the
    /// retention rules (MEDULLA-DURABLE-KERNEL-R085.7).
    async fn reconcile(
        state: &Arc<RwLock<ServerState>>,
        graph: &str,
        doc: &DocId,
        key: &str,
        hold_retained: bool,
    ) -> Result<(), String> {
        let (core, blob, anonymous, server) = {
            let s = state.read().await;
            let core = s.registry.get(graph).map(|entry| entry.core.clone());
            let blob = s.blob_cursors().map(|cursors| Arc::clone(&cursors.store));
            (core, blob, s.local_anonymous, s.context.clone())
        };
        let (Some(core), Some(blob)) = (core, blob) else {
            return Err("CONFLICT: memory snapshot graph or CAS is unavailable".into());
        };
        let context =
            service_context(anonymous, &server, format!("memory-snapshot:{graph}:{key}"))?;
        let authority = medulla_server_api::carrier::CarrierAuthority::from_verified(&context)?;
        // The holder is registered under the document owner's scope, the row the
        // router registered, never under the service actor's own scope.
        let owner_scope = medulla_server_api::carrier::memory_snapshot_owner_scope(
            context.tenant(),
            doc.as_str(),
        );
        // A revision releases past its retention deadline or beyond the newest
        // `max_revisions_per_doc`, oldest first; a resolved candidate releases
        // past its deadline; an unresolved conflict candidate never releases.
        let config = medulla_server_api::knob_config::memory_sync_config();
        let plan = store::retention_plan(
            &core,
            doc,
            medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
            config.history_retention_s,
            u64::from(config.max_revisions_per_doc),
        );
        let held = if hold_retained { plan.held } else { Vec::new() };
        let released = plan.released;
        for (slot, content) in held {
            let (digest, _was_new) = blob.put_chunk(content.as_bytes())?;
            let holder =
                HolderChange::acquire(&digest, HolderId::new(&slot.holder_id(doc))?, &owner_scope)?;
            let method = medulla_protocol_types::protocol::Method::BlobRef {
                digest: digest.clone(),
            };
            hold_one(blob.as_ref(), &authority, &holder, &method)?;
        }
        for (slot, digest) in released {
            let holder = HolderChange::release(&digest, HolderId::new(&slot.holder_id(doc))?)?;
            let method = medulla_protocol_types::protocol::Method::BlobRef { digest };
            hold_one(blob.as_ref(), &authority, &holder, &method)?;
        }
        Ok(())
    }

    /// One set-like holder change, retried on an engine-owned scope-version conflict.
    fn hold_one(
        blob: &dyn ChunkStore,
        authority: &medulla_server_api::carrier::CarrierAuthority,
        holder: &HolderChange,
        method: &medulla_protocol_types::protocol::Method,
    ) -> Result<(), String> {
        let mut attempt = 1;
        loop {
            let (batch, now) =
                medulla_server_ops::api::ingest::compile_blob_batch(blob, 0, authority, method)?;
            match blob.holder_batch(holder, &batch, now) {
                Err(error) if error.contains("WRITE_CONFLICT:") && attempt < MAX_ATTEMPTS => {
                    attempt += 1;
                }
                other => return other.map(|_| ()),
            }
        }
    }
}
