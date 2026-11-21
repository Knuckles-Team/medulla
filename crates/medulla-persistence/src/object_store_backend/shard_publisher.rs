//! The per-shard commit publisher (MEDULLA-OBJECT-STORAGE-BACKEND-R003).
//!
//! Attached to every shard of the local store, it is what the commit chokepoint
//! (`Shard::commit_batch`) calls with each fresh commit before the commit is
//! final. The writer thread is a plain thread, so the publish blocks on the
//! backend's own runtime; the object-store client it uses is never shared with
//! the restore's runtime.

use std::sync::{Arc, Weak};

use medulla_store::redb_store::commit_publish::{CommitPublisher, PublishedCommit};
use medulla_store::redb_store::shard::Shard;

use super::ObjectStoreBackend;

/// The runtime every publish blocks on. Dropped without joining its workers, so
/// dropping the last handle inside an async context is not a panic.
struct PublishRuntime(Option<tokio::runtime::Runtime>);

impl PublishRuntime {
    fn new() -> Result<Self, String> {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("object-publish")
            .enable_all()
            .build()
            .map(|runtime| Self(Some(runtime)))
            .map_err(|error| format!("OBJECT_HEAD_MISSING: publish runtime: {error}"))
    }

    fn block_on<T>(&self, future: impl std::future::Future<Output = T>) -> T {
        self.0
            .as_ref()
            .expect("the publish runtime lives until it is dropped")
            .block_on(future)
    }
}

impl Drop for PublishRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

struct ShardCommitPublisher {
    backend: Arc<ObjectStoreBackend>,
    runtime: Arc<PublishRuntime>,
    shard: Weak<Shard>,
}

impl CommitPublisher for ShardCommitPublisher {
    fn publish(&self, commit: &PublishedCommit<'_>) -> Result<(), String> {
        let snapshot = || {
            self.shard
                .upgrade()
                .ok_or_else(|| "redb writer thread is gone".to_string())?
                .kernel()
                .owner_snapshot_segment()
        };
        self.runtime
            .block_on(self.backend.publish_commit(commit, &snapshot))
            .map(|_| ())
    }
}

/// Attach one publisher to each of `shards`, all publishing through `backend`.
pub(crate) fn attach_shard_publishers(
    backend: Arc<ObjectStoreBackend>,
    shards: impl IntoIterator<Item = Weak<Shard>>,
) -> Result<(), String> {
    let runtime = Arc::new(PublishRuntime::new()?);
    for weak in shards {
        let shard = weak
            .upgrade()
            .ok_or_else(|| "redb writer thread is gone".to_string())?;
        shard.attach_commit_publisher(Arc::new(ShardCommitPublisher {
            backend: Arc::clone(&backend),
            runtime: Arc::clone(&runtime),
            shard: weak,
        }))?;
    }
    Ok(())
}
