use super::*;
use crate::writer_reply::await_writer_reply;
#[cfg(any(feature = "compute-dist", feature = "matview"))]
use medulla_store::redb_store::MatViewScanResult;
use tokio::sync::oneshot;

// ── Durable materialized-view records ─────────────────────────────────────
#[cfg(any(feature = "raft", feature = "matview"))]
impl RedbBackend {
    /// Durably upsert a named materialized view's serialized blob (CONCEPT:MEDULLA-KG.storage.feature).
    /// Awaits the fsync so a `CreateMatView`/`RefreshMatView` ack means it is on disk.
    #[cfg(feature = "compute-dist")]
    pub async fn matview_put(&self, name: &str, blob: Vec<u8>) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        let name = name.to_string();
        let tx = self.shard0().tx.clone();
        tokio::task::spawn_blocking(move || {
            tx.send(Cmd::MatViewPut { name, blob, done })
                .map_err(|_| ())
        })
        .await
        .map_err(|e| format!("matview_put join error: {e}"))?
        .map_err(|_| "redb writer thread is gone".to_string())?;
        rx.await
            .map_err(|_| "redb writer dropped matview_put completion".to_string())?
    }

    /// Scan every persisted materialized view `(name, blob)` (reload on boot).
    #[cfg(feature = "compute-dist")]
    pub fn matview_scan(&self) -> MatViewScanResult {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.shard0()
            .tx
            .send(Cmd::MatViewScan { reply })
            .map_err(|_| "redb writer thread is gone".to_string())?;
        await_writer_reply(&rx, "matview_scan")?
    }

    /// Durably upsert a PLAN-BACKED matview definition (CONCEPT:MEDULLA-KG.storage.plan-backed-matview).
    /// Awaits the fsync so a `PlanMatViewDefine` ack means the definition is on disk.
    #[cfg(feature = "matview")]
    pub async fn plan_matview_put(&self, name: &str, blob: Vec<u8>) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        let name = name.to_string();
        let tx = self.shard0().tx.clone();
        tokio::task::spawn_blocking(move || {
            tx.send(Cmd::PlanMatViewPut { name, blob, done })
                .map_err(|_| ())
        })
        .await
        .map_err(|e| format!("plan_matview_put join error: {e}"))?
        .map_err(|_| "redb writer thread is gone".to_string())?;
        rx.await
            .map_err(|_| "redb writer dropped plan_matview_put completion".to_string())?
    }

    /// Durably delete a plan-backed matview definition (awaits the fsync).
    #[cfg(feature = "matview")]
    pub async fn plan_matview_delete(&self, name: &str) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        let name = name.to_string();
        let tx = self.shard0().tx.clone();
        tokio::task::spawn_blocking(move || {
            tx.send(Cmd::PlanMatViewDelete { name, done })
                .map_err(|_| ())
        })
        .await
        .map_err(|e| format!("plan_matview_delete join error: {e}"))?
        .map_err(|_| "redb writer thread is gone".to_string())?;
        rx.await
            .map_err(|_| "redb writer dropped plan_matview_delete completion".to_string())?
    }

    /// Scan every persisted plan-backed matview `(name, blob)` (reload on boot).
    #[cfg(feature = "matview")]
    pub fn plan_matview_scan(&self) -> MatViewScanResult {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.shard0()
            .tx
            .send(Cmd::PlanMatViewScan { reply })
            .map_err(|_| "redb writer thread is gone".to_string())?;
        await_writer_reply(&rx, "plan_matview_scan")?
    }

    /// Durably upsert an incremental matview's operator-state snapshot
    /// (MEDULLA-INCREMENTAL-VIEWS-R001). Awaits the fsync.
    #[cfg(feature = "matview")]
    pub async fn matview_operator_state_put(
        &self,
        name: &str,
        blob: Vec<u8>,
    ) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        let name = name.to_string();
        let tx = self.shard0().tx.clone();
        tokio::task::spawn_blocking(move || {
            tx.send(Cmd::MatViewOperatorStatePut { name, blob, done })
                .map_err(|_| ())
        })
        .await
        .map_err(|e| format!("matview_operator_state_put join error: {e}"))?
        .map_err(|_| "redb writer thread is gone".to_string())?;
        rx.await
            .map_err(|_| "redb writer dropped matview_operator_state_put completion".to_string())?
    }

    /// Durably delete an incremental matview's operator-state snapshot (awaits the fsync).
    #[cfg(feature = "matview")]
    pub async fn matview_operator_state_delete(&self, name: &str) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        let name = name.to_string();
        let tx = self.shard0().tx.clone();
        tokio::task::spawn_blocking(move || {
            tx.send(Cmd::MatViewOperatorStateDelete { name, done })
                .map_err(|_| ())
        })
        .await
        .map_err(|e| format!("matview_operator_state_delete join error: {e}"))?
        .map_err(|_| "redb writer thread is gone".to_string())?;
        rx.await.map_err(|_| {
            "redb writer dropped matview_operator_state_delete completion".to_string()
        })?
    }

    /// Read one persisted incremental-matview operator-state snapshot (`None` when absent).
    #[cfg(feature = "matview")]
    pub fn matview_operator_state_get(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.shard0()
            .tx
            .send(Cmd::MatViewOperatorStateGet {
                name: name.to_string(),
                reply,
            })
            .map_err(|_| "redb writer thread is gone".to_string())?;
        await_writer_reply(&rx, "matview_operator_state_get")?
    }

    /// Scan every persisted incremental-matview operator-state snapshot `(name, blob)`.
    #[cfg(feature = "matview")]
    pub fn matview_operator_state_scan(&self) -> MatViewScanResult {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.shard0()
            .tx
            .send(Cmd::MatViewOperatorStateScan { reply })
            .map_err(|_| "redb writer thread is gone".to_string())?;
        await_writer_reply(&rx, "matview_operator_state_scan")?
    }
}

#[cfg(all(test, feature = "matview"))]
mod tests {
    use super::*;

    // spec: MEDULLA-INCREMENTAL-VIEWS-R007.4
    /// Verifies MEDULLA-INCREMENTAL-VIEWS-R007.4: Wiring: the store backs the spill
    #[tokio::test(flavor = "multi_thread")]
    async fn operator_state_get_reads_one_row() {
        let _env_read_lock = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let backend =
            RedbBackend::open_with_shards(dir.path().to_string_lossy().into_owned(), 64, 1)
                .expect("open");
        assert_eq!(backend.matview_operator_state_get("v1"), Ok(None));
        backend
            .matview_operator_state_put("v1", b"one".to_vec())
            .await
            .expect("put v1");
        backend
            .matview_operator_state_put("v2", b"two".to_vec())
            .await
            .expect("put v2");
        assert_eq!(
            backend.matview_operator_state_get("v1"),
            Ok(Some(b"one".to_vec()))
        );
        assert_eq!(
            backend.matview_operator_state_get("v2"),
            Ok(Some(b"two".to_vec()))
        );
        assert_eq!(backend.matview_operator_state_get("absent"), Ok(None));
    }
}
