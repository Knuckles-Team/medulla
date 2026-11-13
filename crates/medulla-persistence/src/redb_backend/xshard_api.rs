use super::*;
#[cfg(feature = "raft")]
use crate::writer_reply::await_writer_reply;
#[cfg(feature = "raft")]
use medulla_store::redb_store::XshardDecisionScan;
use medulla_store::redb_store::XshardPrepareScan;
#[cfg(feature = "raft")]
use tokio::sync::oneshot;

// ── Durable cross-shard transaction records ────────────────────────────────
#[cfg(any(feature = "raft", feature = "matview"))]
impl RedbBackend {
    /// Durably persist one participant group's prepared slice. Awaits the fsync.
    #[cfg(feature = "raft")]
    pub async fn xshard_prepare_put(
        &self,
        txn_id: &str,
        group_id: u64,
        slice: Vec<u8>,
    ) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        let txn_id = txn_id.to_string();
        let tx = self.shard0().tx.clone();
        tokio::task::spawn_blocking(move || {
            tx.send(Cmd::XshardPreparePut {
                txn_id,
                group_id,
                slice,
                done,
            })
            .map_err(|_| ())
        })
        .await
        .map_err(|e| format!("xshard_prepare_put join error: {e}"))?
        .map_err(|_| "redb writer thread is gone".to_string())?;
        rx.await
            .map_err(|_| "redb writer dropped xshard_prepare_put completion".to_string())?
    }

    /// Read one exact participant prepare in logarithmic table-lookup time.
    #[cfg(feature = "raft")]
    pub fn xshard_prepare_get(
        &self,
        txn_id: &str,
        group_id: u64,
    ) -> Result<Option<Vec<u8>>, String> {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.shard0()
            .tx
            .send(Cmd::XshardPrepareGet {
                txn_id: txn_id.to_string(),
                group_id,
                reply,
            })
            .map_err(|_| "redb writer thread is gone".to_string())?;
        await_writer_reply(&rx, "xshard_prepare_get")?
    }

    /// Durably write the coordinator's decision (the atomic commit point). Awaits fsync.
    #[cfg(feature = "raft")]
    pub async fn xshard_decision_put(&self, txn_id: &str, commit: bool) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        let txn_id = txn_id.to_string();
        let tx = self.shard0().tx.clone();
        tokio::task::spawn_blocking(move || {
            tx.send(Cmd::XshardDecisionPut {
                txn_id,
                commit,
                retain_for_parent: false,
                certificate: None,
                done,
            })
            .map_err(|_| ())
        })
        .await
        .map_err(|e| format!("xshard_decision_put join error: {e}"))?
        .map_err(|_| "redb writer thread is gone".to_string())?;
        rx.await
            .map_err(|_| "redb writer dropped xshard_decision_put completion".to_string())?
            .map(|_| ())
    }

    /// Durably write an exact decision that must remain until a separate parent
    /// receipt has committed. With a `certificate` the terminal value and the
    /// sealed certificate become durable in one owner transaction, the storage
    /// writer sets `decided_ms`, and a COMMIT at or after the deadline refuses
    /// with `CERTIFICATE_EXPIRED` without a terminal write. Returns the outcome
    /// that is durable afterwards: a replay of a decided transaction returns the
    /// original outcome.
    #[cfg(feature = "raft")]
    pub async fn xshard_recoverable_decision_put(
        &self,
        txn_id: &str,
        commit: bool,
        certificate: Option<xshard_certificate::DecisionCertificate>,
    ) -> Result<bool, String> {
        let (done, rx) = oneshot::channel();
        let txn_id = txn_id.to_string();
        let tx = self.shard0().tx.clone();
        tokio::task::spawn_blocking(move || {
            tx.send(Cmd::XshardDecisionPut {
                txn_id,
                commit,
                retain_for_parent: true,
                certificate,
                done,
            })
            .map_err(|_| ())
        })
        .await
        .map_err(|e| format!("xshard recoverable decision join error: {e}"))?
        .map_err(|_| "redb writer thread is gone".to_string())?;
        rx.await
            .map_err(|_| "redb writer dropped recoverable decision completion".to_string())?
    }

    /// Persist the parent-recoverable protocol-start marker before phase 1, with
    /// the certificate request in the same owner transaction when one is given.
    #[cfg(feature = "raft")]
    pub async fn xshard_recoverable_pending_put(
        &self,
        txn_id: &str,
        certificate: Option<xshard_certificate::DecisionCertificate>,
    ) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        let txn_id = txn_id.to_string();
        let tx = self.shard0().tx.clone();
        tokio::task::spawn_blocking(move || {
            tx.send(Cmd::XshardRecoverablePendingPut {
                txn_id,
                certificate,
                done,
            })
            .map_err(|_| ())
        })
        .await
        .map_err(|e| format!("xshard recoverable pending join error: {e}"))?
        .map_err(|_| "redb writer thread is gone".to_string())?;
        rx.await
            .map_err(|_| "redb writer dropped recoverable pending completion".to_string())?
    }

    /// Read the verified certificate of one transaction (`None` when nothing was
    /// certified). A terminal decision row with no certificate refuses with
    /// `CERTIFICATE_LEGACY_ROW`: it is never certified evidence.
    #[cfg(feature = "raft")]
    pub fn xshard_certificate_get(
        &self,
        txn_id: &str,
        tenant: &str,
    ) -> Result<Option<xshard_certificate::DecisionCertificate>, String> {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.shard0()
            .tx
            .send(Cmd::XshardCertificateGet {
                txn_id: txn_id.to_string(),
                tenant: tenant.to_string(),
                reply,
            })
            .map_err(|_| "redb writer thread is gone".to_string())?;
        await_writer_reply(&rx, "xshard_certificate_get")?
    }

    /// Clear one participant's prepare record after it is resolved.
    #[cfg(feature = "raft")]
    pub async fn xshard_prepare_clear(&self, txn_id: &str, group_id: u64) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        let txn_id = txn_id.to_string();
        let tx = self.shard0().tx.clone();
        tokio::task::spawn_blocking(move || {
            tx.send(Cmd::XshardPrepareClear {
                txn_id,
                group_id,
                done,
            })
            .map_err(|_| ())
        })
        .await
        .map_err(|e| format!("xshard_prepare_clear join error: {e}"))?
        .map_err(|_| "redb writer thread is gone".to_string())?;
        rx.await
            .map_err(|_| "redb writer dropped xshard_prepare_clear completion".to_string())?
    }

    /// Clear a resolved txn's decision record.
    #[cfg(feature = "raft")]
    pub async fn xshard_decision_clear(&self, txn_id: &str) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        let txn_id = txn_id.to_string();
        let tx = self.shard0().tx.clone();
        tokio::task::spawn_blocking(move || {
            tx.send(Cmd::XshardDecisionClear { txn_id, done })
                .map_err(|_| ())
        })
        .await
        .map_err(|e| format!("xshard_decision_clear join error: {e}"))?
        .map_err(|_| "redb writer thread is gone".to_string())?;
        rx.await
            .map_err(|_| "redb writer dropped xshard_decision_clear completion".to_string())?
    }

    /// Scan every in-doubt prepare record `(txn_id, group_id, slice)` (for recovery).
    #[cfg(feature = "raft")]
    pub fn xshard_scan_prepares(&self) -> XshardPrepareScan {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.shard0()
            .tx
            .send(Cmd::XshardScanPrepares { reply })
            .map_err(|_| "redb writer thread is gone".to_string())?;
        await_writer_reply(&rx, "xshard_scan_prepares")?
    }

    /// Scan digest-only decision states (no source payloads).
    #[cfg(feature = "raft")]
    pub fn xshard_scan_decisions(&self) -> XshardDecisionScan {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.shard0()
            .tx
            .send(Cmd::XshardScanDecisions { reply })
            .map_err(|_| "redb writer thread is gone".to_string())?;
        await_writer_reply(&rx, "xshard decision scan")?
    }

    /// Read a txn's durable decision (Some(true)=commit, Some(false)=abort, None=undecided).
    #[cfg(feature = "raft")]
    pub fn xshard_decision_get(&self, txn_id: &str) -> Result<Option<bool>, String> {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.shard0()
            .tx
            .send(Cmd::XshardDecisionGet {
                txn_id: txn_id.to_string(),
                reply,
            })
            .map_err(|_| "redb writer thread is gone".to_string())?;
        await_writer_reply(&rx, "xshard_decision_get")?
    }

    /// Whether this marker is retained for a separate parent receipt.
    #[cfg(feature = "raft")]
    pub fn xshard_decision_retain_get(&self, txn_id: &str) -> Result<bool, String> {
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.shard0()
            .tx
            .send(Cmd::XshardDecisionRetainGet {
                txn_id: txn_id.to_string(),
                reply,
            })
            .map_err(|_| "redb writer thread is gone".to_string())?;
        await_writer_reply(&rx, "xshard retain")?
    }
}
