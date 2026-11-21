//! The object-store root of a [`RedbBackend`]
//! (MEDULLA-OBJECT-STORAGE-BACKEND-R002, R003).
//!
//! With an [`ObjectStoreBackend`] attached, every shard publishes each fresh
//! commit to the root at the one commit chokepoint, before the commit is final
//! and before the caller is acked (`Shard::commit_batch`). A lost publish fence
//! fails the commit and rolls the local write back. The graphs the root holds
//! are restored into this store at open.
use super::*;
use medulla_protocol_types::protocol::GraphType;

#[cfg(feature = "object-store")]
use crate::object_store_backend::ObjectStoreBackend;

impl RedbBackend {
    /// Attach the object-store backend whose root this store publishes to: every
    /// shard's commit chokepoint publishes through it. Set once at startup, after
    /// the restore and before the backend is shared.
    #[cfg(feature = "object-store")]
    pub fn attach_object_store(&mut self, backend: Arc<ObjectStoreBackend>) -> Result<(), String> {
        crate::object_store_backend::attach_shard_publishers(
            backend,
            self.shards.iter().map(|writer| writer.shard.clone()),
        )
    }

    /// A graph's durable name, type and local graph version; `None` when the
    /// graph has no catalog row.
    pub fn graph_identity(
        &self,
        graph_fname: &str,
    ) -> Result<Option<(String, GraphType, u64)>, String> {
        let shard = self
            .shard_for(graph_fname)
            .shard
            .upgrade()
            .ok_or_else(|| "redb writer thread is gone".to_string())?;
        let Some((name, graph_type)) =
            medulla_store::redb_store::store_read::read_graph_identity(&shard, graph_fname)?
        else {
            return Ok(None);
        };
        let version =
            medulla_store::redb_store::store_read::read_mutation_graph_version(&shard, graph_fname)?;
        Ok(Some((name, graph_type, version)))
    }
}
