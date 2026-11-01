//! Ingest, graph, jobs and compute handler seams (`IngestApi`, `GraphApi`, `JobsApi`, `ComputeApi`).

pub mod embedder;
pub mod graph;
pub mod jobs;

use crate::mutation_batch::MutationBatch;
use crate::protocol::Method;
use crate::server::access::CarrierAuthority;
use medulla_types::contract::Nonce;
use medulla_types::mutation_scope::DurabilityDomain;
use medulla_types::mutation_scope::MutationSurface;

/// Largest ingest stream whose worst-case canonical `ApplyOutcome` response
/// (including `u64::MAX` counters) fits the Raft codec's 4096-byte envelope.
pub const MAX_INGEST_STREAM_ITEMS: usize = 49;

/// Ingest handler behaviour reachable from crates below `medulla-h-ingest`.
pub trait IngestApi: Send + Sync {}

/// Graph handler behaviour reachable from crates below `medulla-h-graph`.
pub trait GraphApi: Send + Sync {}

/// Jobs handler behaviour reachable from crates below `medulla-h-jobs`.
pub trait JobsApi: Send + Sync {}

/// Compute handler behaviour reachable from crates below `medulla-h-compute`.
pub trait ComputeApi: Send + Sync {}

/// Compile a blob mutation batch at the authoritative clock and the current native version.
pub(crate) fn compile_blob_batch(
    store: &dyn crate::server::blob::store::ChunkStore,
    req_id: u64,
    authority: &CarrierAuthority,
    method: &Method,
) -> Result<(MutationBatch, u64), String> {
    compile_blob_batch_with_nonce(store, req_id, authority, method, None)
}

/// [`compile_blob_batch`] with an explicit attempt nonce.
pub(crate) fn compile_blob_batch_with_nonce(
    store: &dyn crate::server::blob::store::ChunkStore,
    req_id: u64,
    authority: &CarrierAuthority,
    method: &Method,
    attempt_nonce: Option<Nonce>,
) -> Result<(MutationBatch, u64), String> {
    let scope = authority.namespace("blob-cas", "control");
    let expected = store.mutation_version(authority.tenant_scope(), &scope)?;
    let now = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    compile_blob_batch_at_with_nonce(
        store,
        req_id,
        authority,
        method,
        expected,
        now,
        attempt_nonce,
    )
}

/// Rebuild an exact blob child after a coordinator restart. The expected native
/// version and timestamp came from the authenticated encrypted parent plan; they
/// are never re-sampled after an acknowledgement-lost child commit.
///
/// `identity_id` is folded into the durable idempotency key
/// (`mutation_batch::opaque_request_key`) alongside `authority`'s tenant/actor
/// scope and `method`'s own encoded bytes — it exists to distinguish otherwise
/// identical-looking attempts, NOT to name the caller's wire request id.
/// **BUG A2 (2026-08-12):** every other blob method's own `Method` payload
/// already carries the cursor/digest it operates on (`BlobChunkPut{cursor,
/// ..}`, `BlobCommit{cursor}`, `BlobRef{digest}`, ...), so passing the wire
/// `req_id` here is safe FOR THOSE — the method bytes alone already
/// disambiguate two different uploads. `BlobBegin{chunk_size}` is the one
/// exception: it carries no upload identity at all (it is the call that
/// MINTS one), so its caller must pass the freshly allocated cursor id here,
/// never the wire `req_id`. The wire request id is a small integer that a
/// brand-new, otherwise-unrelated connection restarts from 1 on every
/// reconnect (`medulla.client.MedullaClient._next_id`), while
/// `CarrierAuthority` carries no per-connection/session component at all — so
/// two independent connections from the same tenant/actor calling
/// `BlobBegin()` with the same default `chunk_size` as their first RPC
/// produced the EXACT SAME idempotency key. The engine's
/// `medulla_transaction::MutationKernel::admit` idempotency ledger never expires (by design —
/// it survives an acknowledgement-lost retry across a coordinator restart),
/// so the SECOND connection's `BlobBegin` silently REPLAYED the first
/// connection's already-committed result: the FIRST connection's cursor id,
/// whose durable `CAS_UPLOADS` row had already been torn down by ITS
/// `BlobCommit` (`ChunkStore::commit_upload_batch` removes the row on
/// commit — correctly, per-upload teardown). The handler then looked up that
/// stale, already-removed id and failed with "committed blob upload cursor
/// is missing". See `Method::BlobBegin`'s handler arm for the caller-side
/// fix (allocate the cursor id BEFORE compiling the batch, then pass it here
/// as `identity_id`).
pub(crate) fn compile_blob_batch_at(
    _store: &dyn crate::server::blob::store::ChunkStore,
    identity_id: u64,
    authority: &CarrierAuthority,
    method: &Method,
    expected: u64,
    now: u64,
) -> Result<(MutationBatch, u64), String> {
    compile_blob_batch_at_with_nonce(_store, identity_id, authority, method, expected, now, None)
}

/// [`compile_blob_batch_at`] with an explicit attempt nonce.
pub(crate) fn compile_blob_batch_at_with_nonce(
    _store: &dyn crate::server::blob::store::ChunkStore,
    identity_id: u64,
    authority: &CarrierAuthority,
    method: &Method,
    expected: u64,
    now: u64,
    attempt_nonce: Option<Nonce>,
) -> Result<(MutationBatch, u64), String> {
    let scope = authority.namespace("blob-cas", "control");
    let batch_id =
        crate::server::mutation_batch::opaque_request_key("blob", &scope, identity_id, method);
    let batch = crate::server::mutation_batch::compile_opaque_method(
        crate::server::mutation_batch::CompileBatch {
            batch_id: &batch_id,
            request_id: identity_id,
            attempt_nonce,
            principal: Some(authority.actor_scope()),
            tenant: authority.tenant_scope(),
            graph: &scope,
            placement_epoch: 0,
            idempotency_key: &batch_id,
            expected_graph_version: Some(expected),
            fencing_token: None,
            created_at_ms: now,
            default_surface: MutationSurface::Other,
            authoritative_state: None,
        },
        method,
        MutationSurface::Other,
        DurabilityDomain::BlobStore,
        "blob_operation",
    )?;
    Ok((batch, now))
}

/// Convenience: build a `BlobCursors` over a fresh native CAS in `dir` (used by
/// integration tests to drive the handler end to end).
#[cfg(test)]
pub(crate) fn cursors_for_test(dir: &str) -> std::sync::Arc<crate::server::blob::BlobCursors> {
    let store = std::sync::Arc::new(crate::server::blob::store::RedbChunkStore::open(dir).unwrap());
    std::sync::Arc::new(crate::server::blob::BlobCursors::new(store))
}
