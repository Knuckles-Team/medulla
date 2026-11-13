//! The request-bound coordinator decision certificate, coordinator side
//! (MEDULLA-DECISION-ENGINE-R036.105, R036.105.2).
//!
//! A transaction that holds non-graph owners (RBAC, Agent Library) is decided
//! once, durably, on the control shard of the persistence backend: the
//! protocol-start marker is written together with the certificate request
//! ([`begin`]), the terminal outcome together with the sealed certificate
//! ([`decide`]), and every owner is resolved from that durable decision
//! ([`recover`]), never from the local result of the commit that ran. The
//! stored type and its sealing are `DecisionCertificate` in
//! `medulla_persistence::redb_backend::xshard_certificate` (layer 3, because the
//! storage writer stores it); this module builds and checks it.
//!
//! The decision itself is the existing coordinator row (`Free` -> pending ->
//! COMMIT or ABORT, written only by the storage writer); the owner-only driver
//! here has no second copy of that state.

use medulla_persistence::redb_backend::xshard_certificate::DecisionCertificate;
use medulla_persistence::redb_backend::RedbBackend;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Every refusal code of the certificate path.
pub const REFUSALS: &[&str] = &[
    "CERTIFICATE_EXPIRED",
    "CERTIFICATE_INTENT_CHANGED",
    "CERTIFICATE_LEGACY_ROW",
    "CERTIFICATE_DECIDED_ABORT",
];

/// Which non-graph owner holds a reservation under the transaction id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnerKind {
    /// The RBAC authorization image.
    Rbac,
    /// The Agent Library component heads.
    AgentLibrary,
}

/// One owner that reserved under the transaction and is resolved with its decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerParticipant {
    /// The owner.
    pub owner: OwnerKind,
    /// The id the owner reserved under (the transaction id).
    pub reservation_txn_id: String,
}

/// The request a transaction is decided for, before the writer stamps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionCertificateRequest {
    /// The transaction id.
    pub txn_id: String,
    /// The tenant of the verified request.
    pub tenant: String,
    /// The scope of the coordinator that decides.
    pub coordinator_scope: String,
    /// SHA-256 of the request, from [`request_digest`].
    pub request_digest: String,
    /// The time a COMMIT must be decided by (milliseconds since the Unix epoch).
    pub deadline_ms: u64,
}

impl DecisionCertificateRequest {
    /// The certificate to store: `decided_ms` is zero until the writer decides.
    pub fn certificate(&self) -> DecisionCertificate {
        DecisionCertificate {
            txn_id: self.txn_id.clone(),
            tenant: self.tenant.clone(),
            coordinator_scope: self.coordinator_scope.clone(),
            request_digest: self.request_digest.clone(),
            deadline_ms: self.deadline_ms,
            decided_ms: 0,
        }
    }
}

/// The lowercase-hex SHA-256 over `parts`, each prefixed by its big-endian
/// `u32` length so two part lists never encode alike.
pub fn request_digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u32).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The durable outcome of a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerDecision {
    /// The decision is COMMIT.
    Commit,
    /// The decision is ABORT.
    Abort,
}

impl OwnerDecision {
    fn of(commit: bool) -> Self {
        if commit {
            Self::Commit
        } else {
            Self::Abort
        }
    }
}

/// What [`begin`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Begun {
    /// The marker and certificate request are now durable; nothing was decided.
    Fresh,
    /// The transaction was decided before: the original outcome.
    Decided(OwnerDecision),
}

/// Refuse a request whose transaction already carries a certificate for another
/// request (`CERTIFICATE_INTENT_CHANGED`) or a terminal row with no certificate
/// (`CERTIFICATE_LEGACY_ROW`).
pub fn verify_existing(
    redb: &RedbBackend,
    request: &DecisionCertificateRequest,
) -> Result<(), String> {
    if let Some(existing) = redb.xshard_certificate_get(&request.txn_id, &request.tenant)? {
        if existing.request_digest != request.request_digest {
            return Err(format!(
                "CERTIFICATE_INTENT_CHANGED: transaction {} was certified for request {}, not {}",
                request.txn_id, existing.request_digest, request.request_digest
            ));
        }
    }
    Ok(())
}

/// Start the transaction: write the pending marker with the certificate request in
/// one owner transaction, or report the outcome it was already decided with.
pub async fn begin(
    redb: &RedbBackend,
    request: &DecisionCertificateRequest,
) -> Result<Begun, String> {
    verify_existing(redb, request)?;
    if let Some(commit) = redb.xshard_decision_get(&request.txn_id)? {
        return Ok(Begun::Decided(OwnerDecision::of(commit)));
    }
    redb.xshard_recoverable_pending_put(&request.txn_id, Some(request.certificate()))
        .await?;
    Ok(Begun::Fresh)
}

/// Record `commit` as the decision of a recoverable transaction and return the
/// outcome that is durable afterwards. A COMMIT at or after the deadline refuses
/// with `CERTIFICATE_EXPIRED` without a terminal write, after the coordinator has
/// recorded ABORT with the certificate (an ABORT never expires).
pub async fn decide_recoverable(
    redb: &RedbBackend,
    txn_id: &str,
    commit: bool,
    certificate: Option<DecisionCertificate>,
) -> Result<bool, String> {
    match redb
        .xshard_recoverable_decision_put(txn_id, commit, certificate.clone())
        .await
    {
        Err(error) if error.starts_with("CERTIFICATE_EXPIRED") => {
            redb.xshard_recoverable_decision_put(txn_id, false, certificate)
                .await?;
            Err(error)
        }
        other => other,
    }
}

/// Decide an owner-only transaction (no graph slice). See [`decide_recoverable`].
pub async fn decide(
    redb: &RedbBackend,
    request: &DecisionCertificateRequest,
    commit: bool,
) -> Result<OwnerDecision, String> {
    decide_recoverable(redb, &request.txn_id, commit, Some(request.certificate()))
        .await
        .map(OwnerDecision::of)
}

/// The decision every owner resolves with, read from the durable record. A
/// transaction with a terminal row returns that outcome (the certificate is
/// verified; a row with none refuses with `CERTIFICATE_LEGACY_ROW` and the owners
/// resolve as abort). A transaction with no terminal row is resolved by presumed
/// abort, recorded with the certificate before it is returned, so a crash between
/// the terminal write and an owner's resolve is finished by this same read on
/// restart.
pub async fn recover(
    redb: &RedbBackend,
    txn_id: &str,
    tenant: &str,
) -> Result<OwnerDecision, String> {
    let certificate = redb.xshard_certificate_get(txn_id, tenant)?;
    match redb.xshard_decision_get(txn_id)? {
        Some(commit) => Ok(OwnerDecision::of(commit)),
        None => {
            redb.xshard_recoverable_decision_put(txn_id, false, certificate)
                .await?;
            Ok(OwnerDecision::Abort)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(tag: &str) -> (std::path::PathBuf, RedbBackend) {
        let dir = std::env::temp_dir().join(format!(
            "medulla-certificate-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let redb = RedbBackend::open_with_shards(dir.to_string_lossy().into_owned(), 16, 1)
            .expect("open backend");
        (dir, redb)
    }

    fn request(txn: &str, digest: &str, deadline_ms: u64) -> DecisionCertificateRequest {
        DecisionCertificateRequest {
            txn_id: txn.into(),
            tenant: "tenant-a".into(),
            coordinator_scope: "scope-a".into(),
            request_digest: request_digest(&[digest]),
            deadline_ms,
        }
    }

    fn far() -> u64 {
        medulla_persistence::redb_backend::xshard_certificate::writer_now_ms() + 600_000
    }

    #[test]
    fn every_refusal_code_is_registered() {
        for code in [
            "CERTIFICATE_EXPIRED",
            "CERTIFICATE_INTENT_CHANGED",
            "CERTIFICATE_LEGACY_ROW",
        ] {
            assert!(REFUSALS.contains(&code));
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R036.105
    // spec: MEDULLA-DECISION-ENGINE-R036.105.2
    #[tokio::test]
    async fn certificate_binds_request_and_expires() {
        let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let (dir, redb) = open("binds");
        let live = request("txn-live", "request-1", far());

        // Pending first: marker and certificate request are one write, decided_ms is zero.
        assert_eq!(begin(&redb, &live).await.unwrap(), Begun::Fresh);
        assert_eq!(redb.xshard_decision_get("txn-live").unwrap(), None);
        let pending = redb
            .xshard_certificate_get("txn-live", "tenant-a")
            .unwrap()
            .unwrap();
        assert_eq!(pending.decided_ms, 0);
        assert_eq!(pending.request_digest, live.request_digest);

        // A changed request for the same transaction refuses and writes nothing.
        let changed = request("txn-live", "request-2", far());
        let error = begin(&redb, &changed).await.unwrap_err();
        assert!(error.starts_with("CERTIFICATE_INTENT_CHANGED"), "{error}");
        let error = decide(&redb, &changed, true).await.unwrap_err();
        assert!(error.starts_with("CERTIFICATE_INTENT_CHANGED"), "{error}");
        assert_eq!(redb.xshard_decision_get("txn-live").unwrap(), None);

        // COMMIT: the writer stamps decided_ms and the certificate is durable with the outcome.
        assert_eq!(
            decide(&redb, &live, true).await.unwrap(),
            OwnerDecision::Commit
        );
        let decided = redb
            .xshard_certificate_get("txn-live", "tenant-a")
            .unwrap()
            .unwrap();
        assert!(decided.decided_ms > 0 && decided.decided_ms < live.deadline_ms);
        assert_eq!(redb.xshard_decision_get("txn-live").unwrap(), Some(true));

        // Exact replay returns the original outcome and does not move decided_ms; a replay
        // asking for the other outcome still returns the original one.
        assert_eq!(
            begin(&redb, &live).await.unwrap(),
            Begun::Decided(OwnerDecision::Commit)
        );
        assert_eq!(
            decide(&redb, &live, true).await.unwrap(),
            OwnerDecision::Commit
        );
        assert_eq!(
            decide(&redb, &live, false).await.unwrap(),
            OwnerDecision::Commit
        );
        assert_eq!(
            redb.xshard_certificate_get("txn-live", "tenant-a")
                .unwrap()
                .unwrap()
                .decided_ms,
            decided.decided_ms
        );

        // A certificate read under another tenant refuses.
        assert!(redb.xshard_certificate_get("txn-live", "tenant-b").is_err());

        // Expiry: a COMMIT at or after the deadline refuses without a terminal COMMIT, and the
        // coordinator records ABORT with the certificate.
        let late = request("txn-late", "request-3", 1);
        assert_eq!(begin(&redb, &late).await.unwrap(), Begun::Fresh);
        let error = decide(&redb, &late, true).await.unwrap_err();
        assert!(error.starts_with("CERTIFICATE_EXPIRED"), "{error}");
        assert_eq!(redb.xshard_decision_get("txn-late").unwrap(), Some(false));
        assert!(
            redb.xshard_certificate_get("txn-late", "tenant-a")
                .unwrap()
                .unwrap()
                .decided_ms
                > 0
        );
        // An ABORT never expires.
        let late_abort = request("txn-late-abort", "request-4", 1);
        begin(&redb, &late_abort).await.unwrap();
        assert_eq!(
            decide(&redb, &late_abort, false).await.unwrap(),
            OwnerDecision::Abort
        );

        // Legacy: a terminal row with no certificate is never certified evidence.
        redb.xshard_recoverable_decision_put("txn-legacy", true, None)
            .await
            .unwrap();
        let legacy = request("txn-legacy", "request-5", far());
        let error = begin(&redb, &legacy).await.unwrap_err();
        assert!(error.starts_with("CERTIFICATE_LEGACY_ROW"), "{error}");
        let error = decide(&redb, &legacy, true).await.unwrap_err();
        assert!(error.starts_with("CERTIFICATE_LEGACY_ROW"), "{error}");

        // Cleanup removes the decision and its certificate together.
        redb.xshard_decision_clear("txn-live").await.unwrap();
        assert_eq!(redb.xshard_decision_get("txn-live").unwrap(), None);
        assert!(redb
            .xshard_certificate_get("txn-live", "tenant-a")
            .unwrap()
            .is_none());

        // Reopen: the decided certificate survives a restart.
        drop(redb);
        let redb = RedbBackend::open_with_shards(dir.to_string_lossy().into_owned(), 16, 1)
            .expect("reopen backend");
        assert_eq!(redb.xshard_decision_get("txn-late").unwrap(), Some(false));
        assert!(redb
            .xshard_certificate_get("txn-late", "tenant-a")
            .unwrap()
            .is_some());
        drop(redb);
        let _ = std::fs::remove_dir_all(dir);
    }

    // spec: MEDULLA-DECISION-ENGINE-R036.105.2
    #[tokio::test]
    async fn crash_between_decision_and_resolve_is_finished_on_restart() {
        let _env = medulla_kernel::crypto::acquire_test_env_read_lock().await;
        let (dir, redb) = open("crash");
        let decided = request("txn-decided", "request-1", far());
        let undecided = request("txn-undecided", "request-2", far());
        begin(&redb, &decided).await.unwrap();
        decide(&redb, &decided, true).await.unwrap();
        begin(&redb, &undecided).await.unwrap();

        // The process stops after the terminal write, before either owner resolved.
        drop(redb);
        let redb = RedbBackend::open_with_shards(dir.to_string_lossy().into_owned(), 16, 1)
            .expect("reopen backend");

        // The same read finishes both: the durable COMMIT is returned as it is, and the
        // transaction with no terminal row is resolved by presumed abort, recorded.
        assert_eq!(
            recover(&redb, "txn-decided", "tenant-a").await.unwrap(),
            OwnerDecision::Commit
        );
        assert_eq!(
            recover(&redb, "txn-undecided", "tenant-a").await.unwrap(),
            OwnerDecision::Abort
        );
        assert_eq!(
            redb.xshard_decision_get("txn-undecided").unwrap(),
            Some(false)
        );
        assert!(
            redb.xshard_certificate_get("txn-undecided", "tenant-a")
                .unwrap()
                .unwrap()
                .decided_ms
                > 0
        );
        // A second restart reads the same outcomes.
        assert_eq!(
            recover(&redb, "txn-undecided", "tenant-a").await.unwrap(),
            OwnerDecision::Abort
        );
        // A legacy terminal row refuses by name; the owners resolve as abort.
        redb.xshard_recoverable_decision_put("txn-legacy", true, None)
            .await
            .unwrap();
        let error = recover(&redb, "txn-legacy", "tenant-a").await.unwrap_err();
        assert!(error.starts_with("CERTIFICATE_LEGACY_ROW"), "{error}");
        drop(redb);
        let _ = std::fs::remove_dir_all(dir);
    }
}
