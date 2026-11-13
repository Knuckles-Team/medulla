//! The served read scope: visible knowledge units (MEDULLA-IDENTITY-R023.3) and the
//! tenant predicate (MEDULLA-IDENTITY-R610.3), applied once at the read chokepoint
//! `run_unified_off_lock_with` and by the served SQL read, with the read audit
//! (MEDULLA-IDENTITY-R023.5, MEDULLA-IDENTITY-R610.5).

use std::sync::Arc;

use medulla_graph_core::graph::GraphView;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::read_scope::{scope_view, ReadCaller, ScopedView};
use medulla_types::contract::ServerErrorCode;

/// The verified reader of one served read plus where its audit line goes.
pub(crate) struct ReadScope {
    pub(crate) caller: ReadCaller,
    pub(crate) graph_name: String,
    #[cfg(all(feature = "security", feature = "redb"))]
    pub(crate) persistence:
        Option<Arc<dyn medulla_server_api::api::persistence::PersistenceBackend>>,
    /// The read's audit outcome, owed once the read has run: the reservation
    /// was queued when the read was admitted ([`Self::narrow`]), and
    /// [`Self::close`] queues this outcome against it.
    #[cfg(all(feature = "security", feature = "redb"))]
    pub(crate) pending: std::sync::Mutex<Option<PendingReadAudit>>,
}

/// One read's audit outcome, waiting for the read to finish.
#[cfg(all(feature = "security", feature = "redb"))]
pub(crate) struct PendingReadAudit {
    outcome: medulla_store::redb_store::OperationAuditEvent,
    chain: String,
    persistence: Arc<dyn medulla_server_api::api::persistence::PersistenceBackend>,
}

impl ReadScope {
    /// The scope of the request: the caller comes from the verified read authority only.
    pub(crate) async fn resolve(
        state: &Arc<tokio::sync::RwLock<medulla_server_api::state::ServerState>>,
        authority: Option<&medulla_server_api::carrier::GraphReadAuthority>,
        graph_name: &str,
    ) -> Self {
        #[cfg(not(all(feature = "security", feature = "redb")))]
        let _ = state;
        Self {
            caller: ReadCaller::from_authority(authority, graph_name),
            graph_name: graph_name.to_owned(),
            #[cfg(all(feature = "security", feature = "redb"))]
            persistence: state.read().await.persistence.clone(),
            #[cfg(all(feature = "security", feature = "redb"))]
            pending: std::sync::Mutex::new(None),
        }
    }

    /// Narrow `view` to what the caller may read and queue the read audit
    /// reservation; [`Self::close`] queues its outcome once the read has run.
    /// `QUERY_UNSCOPABLE` when the plan binds no node variable (nothing runs, nothing
    /// is audited); `AUDIT_UNAVAILABLE` when the audit queue is full.
    pub(crate) fn narrow(
        &self,
        req_id: u64,
        revision: u64,
        view: Arc<GraphView>,
        plan_variables: &[String],
    ) -> Result<Arc<GraphView>, Response> {
        let scoped = filter_visible_units(view, &self.caller, plan_variables)
            .map_err(|error| Response::err(req_id, error.to_string()))?;
        self.audit(req_id, revision, &scoped)
            .map_err(|code| Response::err(req_id, code.as_str()))?;
        Ok(scoped.view)
    }

    /// Verifies MEDULLA-IDENTITY-R023.5: Evidence: read audit with hidden count
    // spec: MEDULLA-IDENTITY-R023.5
    #[cfg(all(feature = "security", feature = "redb"))]
    fn audit(
        &self,
        req_id: u64,
        revision: u64,
        scoped: &ScopedView,
    ) -> Result<(), ServerErrorCode> {
        use medulla_identity_types::identity::SegmentReadAudit;
        use sha2::{Digest, Sha256};

        let Some(persistence) = self.persistence.clone() else {
            return Ok(());
        };
        let line = SegmentReadAudit {
            principal: self.caller.principal.id.clone(),
            tenant: self.caller.principal.tenant.clone(),
            unit_ids: scoped.unit_ids.clone(),
            hidden_count: scoped.hidden_count,
            revision,
        };
        let digest = hex::encode(Sha256::digest(
            serde_json::to_vec(&line).unwrap_or_default(),
        ));
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let outcome = medulla_store::redb_store::OperationAuditEvent {
            tenant: line.tenant,
            principal: line.principal,
            op: format!("SegmentReadAudit.hidden-{}", line.hidden_count),
            surface: "read".to_owned(),
            params_sha256: digest,
            status: "ok".to_owned(),
            request_id: format!("read-{req_id}-{nanos}"),
            audit_class: "event".to_owned(),
            subject_iri: None,
            policy_iri: None,
        };
        // The line goes to the tenant's read-audit chain for this graph, a scope
        // of its own: a read-audit line is never a graph mutation, so it never
        // advances the graph's version and a later write never meets
        // `STALE_VERSION` because a read happened.
        let chain = medulla_store::redb_store::read_audit_chain_scope(
            &outcome.tenant,
            &medulla_kernel::persist::sanitize(&self.graph_name),
        );
        // The operation audit is two-phase: the reservation is queued now, at
        // admission, before the read runs; `close` queues the outcome that
        // links to it. One drain task appends in queue order.
        let reservation = medulla_store::redb_store::OperationAuditEvent {
            status: "reserved".to_owned(),
            ..outcome.clone()
        };
        enqueue_append(Arc::clone(&persistence), chain.clone(), reservation)?;
        if let Ok(mut pending) = self.pending.lock() {
            *pending = Some(PendingReadAudit {
                outcome,
                chain,
                persistence,
            });
        }
        Ok(())
    }

    /// Queue the outcome of the audited read: `ok` when it answered, `error`
    /// when it failed after admission. A read whose outcome is never queued
    /// leaves its reservation pending, which is how the chain records a read
    /// that was admitted but did not finish. `AUDIT_UNAVAILABLE` when the
    /// audit queue is full.
    #[cfg(all(feature = "security", feature = "redb"))]
    pub(crate) fn close(&self, answered: bool) -> Result<(), ServerErrorCode> {
        let pending = self
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.take());
        let Some(PendingReadAudit {
            mut outcome,
            chain,
            persistence,
        }) = pending
        else {
            return Ok(());
        };
        if !answered {
            outcome.status = "error".to_owned();
        }
        enqueue_append(persistence, chain, outcome)
    }

    #[cfg(not(all(feature = "security", feature = "redb")))]
    pub(crate) fn close(&self, _: bool) -> Result<(), ServerErrorCode> {
        Ok(())
    }

    #[cfg(not(all(feature = "security", feature = "redb")))]
    fn audit(&self, _: u64, _: u64, _: &ScopedView) -> Result<(), ServerErrorCode> {
        Ok(())
    }
}

/// Queue one append to a read-audit chain on the shared sink (MEDULLA-IDENTITY-R610.5).
#[cfg(all(feature = "security", feature = "redb"))]
fn enqueue_append(
    persistence: Arc<dyn medulla_server_api::api::persistence::PersistenceBackend>,
    chain: String,
    event: medulla_store::redb_store::OperationAuditEvent,
) -> Result<(), ServerErrorCode> {
    medulla_server_api::read_audit::ReadAuditSink::global().enqueue(Box::pin(async move {
        use medulla_persistence::AsRedb;
        match persistence.as_redb() {
            Some(redb) => redb.audit_append(&chain, event).await.map(|_| ()),
            None => Ok(()),
        }
    }))
}

/// Narrow `view` to the caller's tenant and the knowledge units its principal may
/// see (`visible_units`); rows of other labels pass unchanged. Built from the
/// verified caller, never from request fields.
pub(crate) fn filter_visible_units(
    view: Arc<GraphView>,
    caller: &ReadCaller,
    plan_variables: &[String],
) -> Result<ScopedView, medulla_identity_types::tenancy::TenancyError> {
    scope_view(view, caller, plan_variables)
}
