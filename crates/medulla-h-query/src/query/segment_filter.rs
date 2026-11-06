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
        }
    }

    /// Narrow `view` to what the caller may read and queue the read audit line.
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

    #[cfg(all(feature = "security", feature = "redb"))]
    fn audit(
        &self,
        req_id: u64,
        revision: u64,
        scoped: &ScopedView,
    ) -> Result<(), ServerErrorCode> {
        use medulla_identity_types::identity::SegmentReadAudit;
        use medulla_persistence::AsRedb;
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
        let event = medulla_store::redb_store::OperationAuditEvent {
            tenant: line.tenant,
            principal: line.principal,
            op: format!("SegmentReadAudit.hidden-{}", line.hidden_count),
            surface: "read".to_owned(),
            params_sha256: digest,
            status: "ok".to_owned(),
            request_id: format!("read-{req_id}-{nanos}"),
            audit_class: "event".to_owned(),
        };
        let graph = medulla_kernel::persist::sanitize(&self.graph_name);
        medulla_server_api::read_audit::ReadAuditSink::global().enqueue(Box::pin(async move {
            match persistence.as_redb() {
                Some(redb) => redb.audit_append(&graph, event).await.map(|_| ()),
                None => Ok(()),
            }
        }))
    }

    #[cfg(not(all(feature = "security", feature = "redb")))]
    fn audit(&self, _: u64, _: u64, _: &ScopedView) -> Result<(), ServerErrorCode> {
        Ok(())
    }
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
