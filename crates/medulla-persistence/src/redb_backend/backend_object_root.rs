//! The object-store root of a [`RedbBackend`]
//! (MEDULLA-OBJECT-STORAGE-BACKEND-R002, R003).
//!
//! With an [`ObjectStoreBackend`] attached, every committed compact batch is
//! published to the root before the caller is acked (commit before ack), and
//! the graphs the root holds are restored into this store at open.
use super::*;
use medulla_protocol_types::mutation_batch::{MutationBatch, MutationBatchCommit};

#[cfg(feature = "object-store")]
use crate::object_store_backend::ObjectStoreBackend;

impl RedbBackend {
    /// Attach the object-store backend whose root this store publishes to. Set
    /// once at startup, before the backend is shared.
    #[cfg(feature = "object-store")]
    pub fn attach_object_store(&mut self, backend: Arc<ObjectStoreBackend>) {
        self.object_store = Some(backend);
    }

    /// Steps 2-5 of the commit-before-ack path over a commit that is already
    /// durable locally (step 1): publish this batch to the root and return the
    /// receipt only after the `HEAD` fence. A store with no object-store
    /// backend returns the receipt unchanged. A replayed commit was published
    /// when it first committed and is not published again.
    pub(super) async fn publish_commit(
        &self,
        graph_fname: &str,
        batch: &MutationBatch,
        result_msgpack: Option<&[u8]>,
        committed_at_ms: u64,
        commit: MutationBatchCommit,
    ) -> Result<MutationBatchCommit, String> {
        #[cfg(feature = "object-store")]
        if let Some(backend) = &self.object_store {
            if !commit.replayed {
                backend
                    .publish_batch(graph_fname, batch, result_msgpack, committed_at_ms, &commit)
                    .await?;
            }
        }
        #[cfg(not(feature = "object-store"))]
        let _ = (graph_fname, batch, result_msgpack, committed_at_ms);
        Ok(commit)
    }
}
