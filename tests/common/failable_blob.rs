//! A blob substrate for served tests whose chunk put and holder registration can
//! be made to fail (MEDULLA-DURABLE-KERNEL-R085.4). It is a test double over the
//! real redb chunk store, never a production operation: every call delegates to
//! the real store unless the matching switch is set.
// Included by `#[path]` into several targets; each uses a subset.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use medulla_protocol_types::mutation_batch::MutationBatch;
use medulla_server_blob::blob::engine_bodies::{EngineBody, StoredEngineBody};
use medulla_server_blob::blob::store::{
    BlobManifest, ChunkStore, HolderChange, HolderOutcome, HolderReconcile, ReconcileStats,
    RedbChunkStore, SweepRequest, SweepStats,
};
use medulla_server_blob::blob::BlobStateExt;

/// The real CAS plus the two failure switches.
pub struct FailableBlob {
    inner: RedbChunkStore,
    fail_put: AtomicBool,
    fail_holder: AtomicBool,
}

impl FailableBlob {
    /// Make every `put_chunk` fail until switched back.
    pub fn fail_put(&self, fail: bool) {
        self.fail_put.store(fail, Ordering::SeqCst);
    }

    /// Make every holder registration fail until switched back.
    pub fn fail_holder(&self, fail: bool) {
        self.fail_holder.store(fail, Ordering::SeqCst);
    }

    /// The content of one chunk, read from the real store.
    pub fn chunk(&self, digest: &str) -> Option<Vec<u8>> {
        self.inner.get_chunk(digest).expect("the chunk read")
    }
}

/// Install a failable CAS opened on `dir` as the blob substrate of `state`.
pub async fn attach(
    state: &super::test_support::SharedState,
    dir: &std::path::Path,
) -> Arc<FailableBlob> {
    let inner = RedbChunkStore::open(&dir.to_string_lossy()).expect("open the chunk store");
    let blob = Arc::new(FailableBlob {
        inner,
        fail_put: AtomicBool::new(false),
        fail_holder: AtomicBool::new(false),
    });
    let store: Arc<dyn ChunkStore> = Arc::clone(&blob) as Arc<dyn ChunkStore>;
    state.write().await.set_blob_cursors(Some(Arc::new(
        medulla_server_blob::blob::BlobCursors::new(store),
    )));
    blob
}

impl ChunkStore for FailableBlob {
    fn put_chunk(&self, bytes: &[u8]) -> Result<(String, bool), String> {
        if self.fail_put.load(Ordering::SeqCst) {
            return Err("injected chunk put failure".to_string());
        }
        self.inner.put_chunk(bytes)
    }

    fn get_chunk(&self, digest: &str) -> Result<Option<Vec<u8>>, String> {
        self.inner.get_chunk(digest)
    }

    fn put_manifest(&self, blob_digest: &str, manifest: &BlobManifest) -> Result<(), String> {
        self.inner.put_manifest(blob_digest, manifest)
    }

    fn get_manifest(&self, blob_digest: &str) -> Result<Option<BlobManifest>, String> {
        self.inner.get_manifest(blob_digest)
    }

    fn incref(&self, blob_digest: &str) -> Result<u64, String> {
        self.inner.incref(blob_digest)
    }

    fn decref(&self, blob_digest: &str) -> Result<u64, String> {
        self.inner.decref(blob_digest)
    }

    fn refcount(&self, blob_digest: &str) -> Result<u64, String> {
        self.inner.refcount(blob_digest)
    }

    fn has_named_holder(&self, blob_digest: &str, holder: &str) -> Result<bool, String> {
        self.inner.has_named_holder(blob_digest, holder)
    }

    fn sweep(&self) -> Result<SweepStats, String> {
        self.inner.sweep()
    }

    fn chunk_count(&self) -> Result<u64, String> {
        self.inner.chunk_count()
    }

    fn blob_count(&self) -> Result<u64, String> {
        self.inner.blob_count()
    }

    fn mutation_version(&self, tenant: &str, graph: &str) -> Result<u64, String> {
        self.inner.mutation_version(tenant, graph)
    }

    fn sweep_batch(
        &self,
        request: &SweepRequest,
        batch: &MutationBatch,
        committed_at_ms: u64,
    ) -> Result<SweepStats, String> {
        self.inner.sweep_batch(request, batch, committed_at_ms)
    }

    fn begin_upload_batch(
        &self,
        cursor: u64,
        chunk_size: u32,
        owner_scope: &str,
        batch: &MutationBatch,
        committed_at_ms: u64,
    ) -> Result<u64, String> {
        self.inner
            .begin_upload_batch(cursor, chunk_size, owner_scope, batch, committed_at_ms)
    }

    fn put_upload_chunk_batch(
        &self,
        cursor: u64,
        bytes: &[u8],
        batch: &MutationBatch,
        committed_at_ms: u64,
    ) -> Result<(String, u32), String> {
        self.inner
            .put_upload_chunk_batch(cursor, bytes, batch, committed_at_ms)
    }

    fn load_upload(&self, cursor: u64) -> Result<Option<BlobManifest>, String> {
        self.inner.load_upload(cursor)
    }

    fn commit_upload_batch(
        &self,
        cursor: u64,
        batch: &MutationBatch,
        committed_at_ms: u64,
    ) -> Result<String, String> {
        self.inner
            .commit_upload_batch(cursor, batch, committed_at_ms)
    }

    fn upload_cursor_high_water(&self) -> Result<u64, String> {
        self.inner.upload_cursor_high_water()
    }

    fn holder_batch(
        &self,
        change: &HolderChange,
        batch: &MutationBatch,
        committed_at_ms: u64,
    ) -> Result<HolderOutcome, String> {
        if self.fail_holder.load(Ordering::SeqCst) {
            return Err("injected holder registration failure".to_string());
        }
        self.inner.holder_batch(change, batch, committed_at_ms)
    }

    fn reconcile_holders_batch(
        &self,
        request: &HolderReconcile,
        batch: &MutationBatch,
        committed_at_ms: u64,
    ) -> Result<ReconcileStats, String> {
        self.inner
            .reconcile_holders_batch(request, batch, committed_at_ms)
    }

    fn put_engine_bodies(
        &self,
        tenant_id: &str,
        bodies: &[EngineBody],
        committed_at_ms: u64,
    ) -> Result<Vec<StoredEngineBody>, String> {
        self.inner
            .put_engine_bodies(tenant_id, bodies, committed_at_ms)
    }

    fn put_repository_bodies(
        &self,
        tenant_id: &str,
        repository_id: &str,
        bodies: &[EngineBody],
        committed_at_ms: u64,
    ) -> Result<Vec<StoredEngineBody>, String> {
        self.inner
            .put_repository_bodies(tenant_id, repository_id, bodies, committed_at_ms)
    }
}
