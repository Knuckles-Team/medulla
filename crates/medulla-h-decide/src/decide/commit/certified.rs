//! MEDULLA-DECISION-ENGINE-R036.105: the request-bound coordinator decision of one
//! `DecisionCommit`.
//!
//! The commit is a transaction over two non-graph owners (the RBAC authorization image and the
//! Agent Library). Its decision is made once, durably, on the control shard of the persistence
//! backend: the protocol-start marker is written together with the certificate request
//! ([`Coordinator::begin`]), the terminal outcome together with the sealed certificate
//! ([`Coordinator::decide`]), and both owners are resolved from that durable decision
//! ([`Coordinator::recover`]), never from the local result of the commit that ran. The record is
//! written only after the COMMIT is durable.

use std::sync::Arc;

use medulla_decision_types::decision::{DecisionErrorCode, DecisionRecord};
use medulla_persistence::redb_backend::RedbBackend;
use medulla_persistence::AsRedb;
use medulla_raft_xshard::cross_shard_txn::certificate::{
    self, Begun, DecisionCertificateRequest, OwnerDecision,
};
use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::authority_context::VerifiedRequestContext;

use super::super::SharedState;

#[cfg(test)]
tokio::task_local! {
    /// The request time a test commit runs at, so a deadline can be pushed into the past
    /// without sleeping. Compiled only under `cfg(test)`.
    pub(in crate::decide) static REQUEST_NOW_MS: u64;
}

/// Milliseconds since the Unix epoch at which this request is decided.
fn request_now_ms() -> u64 {
    #[cfg(test)]
    if let Ok(now) = REQUEST_NOW_MS.try_with(|now| *now) {
        return now;
    }
    medulla_persistence::redb_backend::xshard_certificate::writer_now_ms()
}

/// The coordinator of one decision commit.
pub(in crate::decide) struct Coordinator {
    backend: Arc<dyn PersistenceBackend>,
    request: DecisionCertificateRequest,
    /// The transaction was already decided COMMIT by an earlier attempt: the owners hold their
    /// reservations and only the record write is left.
    pub(in crate::decide) resumed: bool,
    /// An earlier attempt wrote the pending marker, never decided, and its certificate deadline
    /// has passed (MEDULLA-DECISION-ENGINE-R113.2.4): the transaction is finished by presumed
    /// abort, not started again.
    pub(in crate::decide) interrupted: bool,
}

impl Coordinator {
    /// The transaction id both owners reserve under.
    pub(in crate::decide) fn transaction_id(
        record: &DecisionRecord,
        verified: &VerifiedRequestContext,
    ) -> String {
        format!(
            "decision-commit:{}:{}",
            record.record_id,
            verified.idempotency_key()
        )
    }

    /// Write the pending marker and the certificate request, or report what the transaction was
    /// already decided with. An earlier ABORT refuses with `CERTIFICATE_DECIDED_ABORT`.
    pub(in crate::decide) async fn begin(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        record: &DecisionRecord,
        expected_catalog_digest: &str,
    ) -> Result<Self, String> {
        let backend = state.read().await.persistence.clone().ok_or_else(|| {
            format!(
                "{}: the engine has no durable backend to hold the commit decision",
                DecisionErrorCode::DecisionCoordinatorUnavailable.as_str()
            )
        })?;
        let txn_id = Self::transaction_id(record, verified);
        let tenant = verified.tenant().to_string();
        let digest = certificate::request_digest(&[
            verified.principal(),
            &tenant,
            &record.record_id,
            expected_catalog_digest,
            &record.record_digest,
        ]);
        let deadline_ms = request_now_ms()
            .saturating_add(medulla_server_api::knob_config::knobs().xshard_decision_deadline_ms);
        let request = DecisionCertificateRequest {
            txn_id,
            coordinator_scope: format!("decision-commit:{tenant}"),
            tenant,
            request_digest: digest,
            deadline_ms,
        };
        let mut coordinator = Self {
            backend,
            request,
            resumed: false,
            interrupted: false,
        };
        coordinator.interrupted = coordinator.is_interrupted()?;
        if coordinator.interrupted {
            return Ok(coordinator);
        }
        let begun = certificate::begin(coordinator.redb()?, &coordinator.request).await?;
        match begun {
            Begun::Fresh => {}
            Begun::Decided(OwnerDecision::Commit) => coordinator.resumed = true,
            Begun::Decided(OwnerDecision::Abort) => {
                return Err(format!(
                    "{}: transaction {} was decided ABORT",
                    DecisionErrorCode::CertificateDecidedAbort.as_str(),
                    coordinator.request.txn_id
                ));
            }
        }
        Ok(coordinator)
    }

    /// A pending marker with no decision whose deadline has passed: the commit that wrote it
    /// can no longer decide COMMIT, so nothing is in flight for this transaction.
    fn is_interrupted(&self) -> Result<bool, String> {
        let redb = self.redb()?;
        certificate::verify_existing(redb, &self.request)?;
        let Some(existing) =
            redb.xshard_certificate_get(&self.request.txn_id, &self.request.tenant)?
        else {
            return Ok(false);
        };
        Ok(existing.decided_ms == 0
            && redb.xshard_decision_get(&self.request.txn_id)?.is_none()
            && existing.deadline_ms <= request_now_ms())
    }

    /// Whether the transaction of this request was decided COMMIT, read without writing.
    pub(in crate::decide) async fn decided_commit(
        state: &SharedState,
        verified: &VerifiedRequestContext,
        record: &DecisionRecord,
    ) -> Result<bool, String> {
        let Some(backend) = state.read().await.persistence.clone() else {
            return Ok(false);
        };
        let redb = backend.as_redb().ok_or_else(|| {
            format!(
                "{}: the durable backend is not the redb coordinator store",
                DecisionErrorCode::DecisionCoordinatorUnavailable.as_str()
            )
        })?;
        Ok(redb.xshard_decision_get(&Self::transaction_id(record, verified))? == Some(true))
    }

    fn redb(&self) -> Result<&RedbBackend, String> {
        self.backend.as_redb().ok_or_else(|| {
            format!(
                "{}: the durable backend is not the redb coordinator store",
                DecisionErrorCode::DecisionCoordinatorUnavailable.as_str()
            )
        })
    }

    /// The transaction id.
    pub(in crate::decide) fn txn_id(&self) -> &str {
        &self.request.txn_id
    }

    /// Record `commit` as the decision. A COMMIT at or after the deadline records ABORT and
    /// refuses with `CERTIFICATE_EXPIRED`.
    pub(in crate::decide) async fn decide(&self, commit: bool) -> Result<OwnerDecision, String> {
        certificate::decide(self.redb()?, &self.request, commit).await
    }

    /// The decision both owners resolve with, read from the durable record.
    pub(in crate::decide) async fn recover(&self) -> Result<OwnerDecision, String> {
        certificate::recover(self.redb()?, &self.request.txn_id, &self.request.tenant).await
    }

    /// The premise the commit log line carries.
    pub(in crate::decide) fn premise(&self) -> String {
        format!(
            "decision_certificate:{}={}",
            self.request.txn_id, self.request.request_digest
        )
    }
}
