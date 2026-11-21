//! The per-graph write ledger and creator on the durable backend (MEDULLA-IDENTITY-R700.7).
//!
//! Each writer function is its own shard-writer command, enqueued after the write it counts.
//! The reads are typed snapshot reads off the writer thread.

use super::*;
use crate::writer_reply::await_writer_reply;
use medulla_store::redb_store::write_ledger;

impl RedbBackend {
    /// Add one write of `principal` on ledger day `day` to `graph_fname`.
    pub async fn write_day_bump(
        &self,
        graph_fname: &str,
        principal: &str,
        day: u32,
    ) -> Result<(), String> {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.enqueue(
            graph_fname,
            Cmd::WriteDayBump {
                graph: graph_fname.to_string(),
                principal: principal.to_string(),
                day,
                reply,
            },
            "write_day_bump",
        )
        .await?;
        tokio::task::spawn_blocking(move || await_writer_reply(&rx, "write_day_bump"))
            .await
            .map_err(|error| format!("write_day_bump join error: {error}"))??
    }

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

        for _ in 0..3 {
            backend.write_day_bump(GRAPH, "a", 100).await.expect("bump");
        }
        backend.write_day_bump(GRAPH, "a", 120).await.expect("bump");
        backend.write_day_bump(GRAPH, "b", 120).await.expect("bump");
        assert_eq!(
            backend.write_days(GRAPH, 100).await.expect("days"),
            vec![("a".to_string(), 4), ("b".to_string(), 1)]
        );
        assert_eq!(
            backend.write_days(GRAPH, 110).await.expect("days"),
            vec![("a".to_string(), 1), ("b".to_string(), 1)]
        );

        assert_eq!(backend.graph_creator(GRAPH).await.expect("creator"), None);
        backend.record_graph_creator(GRAPH, "first").await.expect("record");
        backend.record_graph_creator(GRAPH, "second").await.expect("record");
        assert_eq!(
            backend.graph_creator(GRAPH).await.expect("creator"),
            Some("first".to_string())
        );

        // A bump that opens a new day deletes the rows older than 400 days.
        backend.write_day_bump(GRAPH, "a", 600).await.expect("bump");
        assert_eq!(
            backend.write_days(GRAPH, 0).await.expect("days"),
            vec![("a".to_string(), 1)]
        );
    }
}
