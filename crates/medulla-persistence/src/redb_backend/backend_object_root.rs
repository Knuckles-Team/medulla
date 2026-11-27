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
use tokio::sync::oneshot;

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
            self.shards.iter().map(|writer| {
                (
                    writer.shard.clone(),
                    #[cfg(feature = "security")]
                    writer.cipher.clone(),
                    #[cfg(not(feature = "security"))]
                    (),
                )
            }),
        )
    }

    /// The id of the key that seals this store's values; `None` with at-rest
    /// encryption off. A root written here records it, and a restore into a
    /// store that lacks the key refuses.
    pub fn seal_key_id(&self) -> Result<Option<String>, String> {
        #[cfg(feature = "security")]
        {
            let mut ids = self.shards.iter().map(|writer| {
                writer
                    .cipher
                    .as_ref()
                    .map(|cipher| cipher.key_ref().id.clone())
            });
            let first = ids.next().flatten();
            if ids.any(|id| id != first) {
                return Err("encryption key reference differs between durable shards".to_string());
            }
            Ok(first)
        }
        #[cfg(not(feature = "security"))]
        Ok(None)
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
        let version = medulla_store::redb_store::store_read::read_mutation_graph_version(
            &shard,
            graph_fname,
        )?;
        Ok(Some((name, graph_type, version)))
    }

    /// Restore one graph's rows from the root's row pages and install the graph
    /// version they stand for; see [`Cmd::RestoreGraphRows`].
    pub async fn restore_graph_rows(
        &self,
        graph_fname: &str,
        nodes: Vec<(String, Vec<u8>)>,
        edges: Vec<(String, String, Vec<u8>)>,
        tables: Vec<medulla_storage::TableRows>,
        graph_version: u64,
    ) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        let cmd = Cmd::RestoreGraphRows {
            graph: graph_fname.to_string(),
            nodes,
            edges,
            tables,
            graph_version,
            done,
        };
        self.enqueue(graph_fname, cmd, "restore_graph_rows").await?;
        rx.await
            .map_err(|_| "redb writer dropped restore_graph_rows completion".to_string())?
    }

    /// Replay one published drain of graph methods; see [`Cmd::ReplayDrain`].
    pub async fn replay_drain(
        &self,
        graph_fname: &str,
        methods: Vec<medulla_protocol_types::protocol::Method>,
    ) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        let cmd = Cmd::ReplayDrain {
            graph: graph_fname.to_string(),
            methods,
            done,
        };
        self.enqueue(graph_fname, cmd, "replay_drain").await?;
        rx.await
            .map_err(|_| "redb writer dropped replay_drain completion".to_string())?
    }

    /// Replay one published maintenance version bump; see [`Cmd::ReplayVersionBump`].
    pub async fn replay_version_bump(&self, graph_fname: &str) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        let cmd = Cmd::ReplayVersionBump {
            graph: graph_fname.to_string(),
            done,
        };
        self.enqueue(graph_fname, cmd, "replay_version_bump")
            .await?;
        rx.await
            .map_err(|_| "redb writer dropped replay_version_bump completion".to_string())?
    }
}
