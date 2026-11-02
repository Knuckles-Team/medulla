//! Ingest, graph, jobs and compute handler seams (`IngestApi`, `GraphApi`, `JobsApi`, `ComputeApi`).

pub mod embedder;
pub mod graph;
pub mod jobs;

use std::sync::Arc;
use tokio::sync::RwLock;
use medulla_server_api::state::ServerState;
use medulla_server_api::carrier::CarrierAuthority;
use medulla_mutation_types::mutation_scope::DurabilityDomain;
use medulla_mutation_types::mutation_scope::MutationSurface;
use medulla_protocol_types::mutation_batch::MutationBatch;
use medulla_protocol_types::protocol::Method;
use medulla_types::contract::Nonce;

/// Largest ingest stream whose worst-case canonical `ApplyOutcome` response
/// (including `u64::MAX` counters) fits the Raft codec's 4096-byte envelope.
pub const MAX_INGEST_STREAM_ITEMS: usize = 49;

/// Ingest handler behaviour reachable from crates below `medulla-h-ingest`.
pub trait IngestApi: Send + Sync {}

/// Graph handler behaviour reachable from crates below `medulla-h-graph`.
#[async_trait::async_trait]
pub trait GraphApi: Send + Sync {
    /// Run a read-only graph method (the terminal graph-ops handler) over `core`.
    async fn graph_ops_try_handle(
        &self,
        state: &std::sync::Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
        req_id: u64,
        caller: Option<&str>,
        graph_name: &str,
        read_authority: &medulla_server_api::carrier::GraphReadAuthority,
        core: std::sync::Arc<medulla_graph_core::graph::GraphCore>,
        method: Method,
    ) -> medulla_protocol_types::protocol::Response;
}

/// Jobs handler behaviour reachable from crates below `medulla-h-jobs`.
pub trait JobsApi: Send + Sync {
    /// Resolve a completed typed job result for the shared KnowledgeBatch stream; the graph
    /// already passed its ACL and placement checks.
    #[cfg(all(feature = "jobs", feature = "knowledge-batch"))]
    #[allow(unused_variables)]
    fn knowledge_stream_result(
        &self,
        persist_dir: &Option<String>,
        graph: &str,
        job_id: &str,
    ) -> Result<medulla_jobs::TypedJobResult, String> {
        Err("this seam method is not built into the installed implementation".to_string())
    }
}

/// Compute handler behaviour reachable from crates below `medulla-h-compute`.
pub trait ComputeApi: Send + Sync {}

/// Compile a blob mutation batch at the authoritative clock and the current native version.
pub fn compile_blob_batch(
    store: &dyn medulla_server_blob::blob::store::ChunkStore,
    req_id: u64,
    authority: &CarrierAuthority,
    method: &Method,
) -> Result<(MutationBatch, u64), String> {
    compile_blob_batch_with_nonce(store, req_id, authority, method, None)
}

/// [`compile_blob_batch`] with an explicit attempt nonce.
pub fn compile_blob_batch_with_nonce(
    store: &dyn medulla_server_blob::blob::store::ChunkStore,
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
    _store: &dyn medulla_server_blob::blob::store::ChunkStore,
    identity_id: u64,
    authority: &CarrierAuthority,
    method: &Method,
    expected: u64,
    now: u64,
) -> Result<(MutationBatch, u64), String> {
    compile_blob_batch_at_with_nonce(_store, identity_id, authority, method, expected, now, None)
}

/// [`compile_blob_batch_at`] with an explicit attempt nonce.
pub fn compile_blob_batch_at_with_nonce(
    _store: &dyn medulla_server_blob::blob::store::ChunkStore,
    identity_id: u64,
    authority: &CarrierAuthority,
    method: &Method,
    expected: u64,
    now: u64,
    attempt_nonce: Option<Nonce>,
) -> Result<(MutationBatch, u64), String> {
    let scope = authority.namespace("blob-cas", "control");
    let batch_id = medulla_server_api::batch_compile::opaque_request_key("blob", &scope, identity_id, method);
    let batch = medulla_server_api::batch_compile::compile_opaque_method(
        medulla_server_api::batch_compile::CompileBatch {
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
#[cfg(any(test, feature = "test-support"))]
pub fn cursors_for_test(dir: &str) -> std::sync::Arc<medulla_server_blob::blob::BlobCursors> {
    let store = std::sync::Arc::new(medulla_server_blob::blob::store::RedbChunkStore::open(dir).unwrap());
    std::sync::Arc::new(medulla_server_blob::blob::BlobCursors::new(store))
}

/// The installed ingest seam of `state`, or `None` before installation.
pub async fn ingest_seam(state: &Arc<RwLock<ServerState>>) -> Option<Arc<dyn IngestApi>> {
    state
        .read()
        .await
        .seams
        .ingest
        .get()
        .and_then(|h| h.downcast_ref::<Arc<dyn IngestApi>>())
        .cloned()
}

/// Install the ingest seam; `false` when it was already installed.
pub fn install_ingest_seam(reg: &medulla_server_api::api::registry::SeamRegistry, api: Arc<dyn IngestApi>) -> bool {
    reg.ingest.set(Arc::new(api) as medulla_server_api::state::ErasedHandle).is_ok()
}

/// The installed graph seam of `state`, or `None` before installation.
pub async fn graph_seam(state: &Arc<RwLock<ServerState>>) -> Option<Arc<dyn GraphApi>> {
    state
        .read()
        .await
        .seams
        .graph
        .get()
        .and_then(|h| h.downcast_ref::<Arc<dyn GraphApi>>())
        .cloned()
}

/// Install the graph seam; `false` when it was already installed.
pub fn install_graph_seam(reg: &medulla_server_api::api::registry::SeamRegistry, api: Arc<dyn GraphApi>) -> bool {
    reg.graph.set(Arc::new(api) as medulla_server_api::state::ErasedHandle).is_ok()
}

/// The installed jobs seam of `state`, or `None` before installation.
pub async fn jobs_seam(state: &Arc<RwLock<ServerState>>) -> Option<Arc<dyn JobsApi>> {
    state
        .read()
        .await
        .seams
        .jobs
        .get()
        .and_then(|h| h.downcast_ref::<Arc<dyn JobsApi>>())
        .cloned()
}

/// Install the jobs seam; `false` when it was already installed.
pub fn install_jobs_seam(reg: &medulla_server_api::api::registry::SeamRegistry, api: Arc<dyn JobsApi>) -> bool {
    reg.jobs.set(Arc::new(api) as medulla_server_api::state::ErasedHandle).is_ok()
}

/// The installed compute seam of `state`, or `None` before installation.
pub async fn compute_seam(state: &Arc<RwLock<ServerState>>) -> Option<Arc<dyn ComputeApi>> {
    state
        .read()
        .await
        .seams
        .compute
        .get()
        .and_then(|h| h.downcast_ref::<Arc<dyn ComputeApi>>())
        .cloned()
}

/// Install the compute seam; `false` when it was already installed.
pub fn install_compute_seam(reg: &medulla_server_api::api::registry::SeamRegistry, api: Arc<dyn ComputeApi>) -> bool {
    reg.compute.set(Arc::new(api) as medulla_server_api::state::ErasedHandle).is_ok()
}
