//! The per-shard commit publisher (MEDULLA-OBJECT-STORAGE-BACKEND-R003).
//!
//! Attached to every shard of the local store, it is what the commit chokepoint
//! (`Shard::commit_batch`) calls with each fresh commit before the commit is
//! final. The writer thread is a plain thread, so the publish blocks on the
//! backend's own runtime; the object-store client it uses is never shared with
//! the restore's runtime.

use std::sync::{Arc, Weak};

use medulla_storage::object::rows::{append_row_page, table_row_pages, RowBatch, BATCH_ROWS};
use medulla_store::redb_store::commit_publish::{CommitPublisher, PublishedCommit};
use medulla_store::redb_store::shard::Shard;
use medulla_store::redb_store::DurableCrypto;
use medulla_store_apply::redb_store::dump::{
    read_graph_dump_page, read_graph_table_rows, PageCursorRef,
};

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

/// The value cipher a shard's writer seals its rows with: the snapshot reads
/// rows unsealed, so it needs the same cipher.
#[cfg(feature = "security")]
pub(crate) type ShardCipher = Option<medulla_kernel::crypto::ValueCipher>;
#[cfg(not(feature = "security"))]
pub(crate) type ShardCipher = ();

struct ShardCommitPublisher {
    backend: Arc<ObjectStoreBackend>,
    runtime: Arc<PublishRuntime>,
    shard: Weak<Shard>,
    cipher: ShardCipher,
}

impl ShardCommitPublisher {
    /// The version-zero snapshot of one graph: its row pages, the paged export
    /// form of the graph copy (nodes and edges, then every other graph-scoped table),
    /// concatenated in key order. Read from the
    /// committed state the open commit builds on.
    fn graph_snapshot(&self, graph: &str) -> Result<Vec<u8>, String> {
        let shard = self
            .shard
            .upgrade()
            .ok_or_else(|| "redb writer thread is gone".to_string())?;
        #[cfg(feature = "security")]
        let crypto = DurableCrypto::new(self.cipher.as_ref());
        #[cfg(not(feature = "security"))]
        let crypto = {
            let () = self.cipher;
            DurableCrypto::none()
        };
        let mut segment = Vec::new();
        let (mut node_after, mut edge_after): (Option<String>, Option<(String, String, u32)>) =
            (None, None);
        loop {
            let Some(page) = read_graph_dump_page(
                &shard,
                graph,
                crypto,
                PageCursorRef {
                    node_offset: 0,
                    edge_offset: 0,
                    node_after: node_after.as_deref(),
                    edge_after: edge_after.as_ref().map(|(source, target, ordinal)| {
                        (source.as_str(), target.as_str(), *ordinal)
                    }),
                    page_size: BATCH_ROWS,
                },
            )?
            else {
                break;
            };
            node_after = page.node_after.clone();
            edge_after = page.edge_after.clone();
            let done = page.nodes_exhausted && page.edges_exhausted;
            let batch = RowBatch {
                nodes: page.nodes,
                edges: page.edges,
                tables: Vec::new(),
            };
            if batch.rows() > 0 {
                append_row_page(&mut segment, &batch)?;
            }
            if done {
                break;
            }
        }
        // Every other graph-scoped table of the graph (event ledger, semantic store,
        // audit chain, resource, development-lane and capacity rows) follows the node
        // and edge pages, so the restored graph is complete.
        for page in table_row_pages(read_graph_table_rows(&shard, graph)?) {
            append_row_page(&mut segment, &page)?;
        }
        Ok(segment)
    }
}

impl CommitPublisher for ShardCommitPublisher {
    fn publish(&self, commit: &PublishedCommit<'_>) -> Result<(), String> {
        let snapshot = || self.graph_snapshot(commit.graph_fname);
        #[cfg(feature = "security")]
        let seal_key_id = self
            .cipher
            .as_ref()
            .map(|cipher| cipher.key_ref().id.clone());
        #[cfg(not(feature = "security"))]
        let seal_key_id = None;
        self.runtime
            .block_on(self.backend.publish_commit(commit, &snapshot, seal_key_id))
            .map(|_| ())
    }
}

/// Attach one publisher to each of `shards`, all publishing through `backend`.
pub(crate) fn attach_shard_publishers(
    backend: Arc<ObjectStoreBackend>,
    shards: impl IntoIterator<Item = (Weak<Shard>, ShardCipher)>,
) -> Result<(), String> {
    let runtime = Arc::new(PublishRuntime::new()?);
    for (weak, cipher) in shards {
        let shard = weak
            .upgrade()
            .ok_or_else(|| "redb writer thread is gone".to_string())?;
        shard.attach_commit_publisher(Arc::new(ShardCommitPublisher {
            backend: Arc::clone(&backend),
            runtime: Arc::clone(&runtime),
            shard: weak,
            cipher,
        }))?;
    }
    Ok(())
}
