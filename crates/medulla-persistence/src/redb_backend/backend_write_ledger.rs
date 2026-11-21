//! The per-graph write ledger and creator on the durable backend (MEDULLA-IDENTITY-R700.7).
//!
//! A write is counted inside the commit of the write itself: the dispatch scopes the verified
//! principal ([`medulla_server_api::request_scope::with_writer`]), [`current_write_count`] turns
//! it into the [`WriteCount`] that rides the durable write command, and the shard writer
//! applies it in that commit's transaction. There is no separate commit and no extra graph
//! version advance. The creator record and the reads are shard-writer commands and typed
//! snapshot reads off the writer thread.

use super::*;
use crate::writer_reply::await_writer_reply;
use medulla_store::redb_store::write_ledger::{self, WriteCount};

/// The count of the write now committing for `graph_fname`: `Some` only inside a counted write
/// of a served request (the dispatch scoped its principal) and never for a replicated apply.
pub(crate) fn current_write_count(graph_fname: &str) -> Option<WriteCount> {
    #[cfg(feature = "raft")]
    if medulla_kernel::kernel::dispatch_support::is_replicated_apply() {
        return None;
    }
    let principal = medulla_server_api::request_scope::current_writer()?;
    Some(WriteCount {
        graph: graph_fname.to_string(),
        principal,
        day: write_ledger::day_of(medulla_kernel::kernel::dispatch_support::authoritative_now_ms()),
    })
}

impl RedbBackend {
    /// Record `principal` as the creator of `graph_fname`; a graph with a creator keeps it.
    pub async fn record_graph_creator(
        &self,
        graph_fname: &str,
        principal: &str,
    ) -> Result<(), String> {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.enqueue(
            graph_fname,
            Cmd::RecordGraphCreator {
                graph: graph_fname.to_string(),
                principal: principal.to_string(),
                reply,
            },
            "record_graph_creator",
        )
        .await?;
        tokio::task::spawn_blocking(move || await_writer_reply(&rx, "record_graph_creator"))
            .await
            .map_err(|error| format!("record_graph_creator join error: {error}"))??
    }

    /// The writes per principal on `graph_fname` for ledger days `>= since_day`, highest count
    /// first, then principal id, at most 256 entries.
    pub async fn write_days(
        &self,
        graph_fname: &str,
        since_day: u32,
    ) -> Result<Vec<(String, u64)>, String> {
        let graph = graph_fname.to_string();
        self.read_snapshot(graph_fname, move |shard, _| {
            write_ledger::write_days(shard, &graph, since_day)
        })
        .await
    }

    /// The recorded creator of `graph_fname`.
    pub async fn graph_creator(&self, graph_fname: &str) -> Result<Option<String>, String> {
        let graph = graph_fname.to_string();
        self.read_snapshot(graph_fname, move |shard, _| {
            write_ledger::graph_creator(shard, &graph)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_protocol_types::protocol::Method;

    const GRAPH: &str = "write_ledger_graph";

    // spec: MEDULLA-IDENTITY-R700.7
    #[tokio::test(flavor = "multi_thread")]
    async fn ledger_counts_days_and_keeps_first_creator() {
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = RedbBackend::open_with_shards(
            dir.path().to_string_lossy().to_string(),
            64,
            1,
        )
        .expect("open");
        // The graph exists because one ordinary write created it.
        let seed = Method::AddNode {
            node_id: "seed".to_string(),
            properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({})).expect("encode"),
        };
        backend.record_durable(GRAPH, &seed).await.expect("seed");

        // Each counted write is one commit: it advances the graph version by exactly one and
        // its bump is part of that commit.
        let counted = |principal: &'static str, node: &'static str| {
            let backend = &backend;
            medulla_server_api::request_scope::with_writer(principal.to_string(), async move {
                let method = Method::AddNode {
                    node_id: node.to_string(),
                    properties_msgpack: rmp_serde::to_vec_named(&serde_json::json!({}))
                        .expect("encode"),
                };
                backend.record_durable(GRAPH, &method).await.expect("write");
            })
        };
        async fn version(backend: &RedbBackend) -> u64 {
            PersistenceBackend::read_mutation_graph_version(backend, GRAPH)
                .await
                .expect("version")
                .expect("graph version")
        }
        let before = version(&backend).await;
        counted("a", "n1").await;
        assert_eq!(version(&backend).await, before + 1);
        assert_eq!(
            backend.write_days(GRAPH, 0).await.expect("days").iter().map(|r| r.1).sum::<u64>(),
            1
        );
        counted("a", "n2").await;
        counted("b", "n3").await;
        assert_eq!(
            backend.write_days(GRAPH, 0).await.expect("days"),
            vec![("a".to_string(), 2), ("b".to_string(), 1)]
        );
        // An uncounted write (outside a served request) leaves the ledger alone.
        backend.record_durable(GRAPH, &seed).await.expect("uncounted");
        assert_eq!(backend.write_days(GRAPH, 0).await.expect("days").len(), 2);

        assert_eq!(backend.graph_creator(GRAPH).await.expect("creator"), None);
        backend.record_graph_creator(GRAPH, "first").await.expect("record");
        backend.record_graph_creator(GRAPH, "second").await.expect("record");
        assert_eq!(
            backend.graph_creator(GRAPH).await.expect("creator"),
            Some("first".to_string())
        );

    }
}
