//! Served connector conformance evidence (MEDULLA-TYPED-PACKS-R807): the read that arbora's
//! `require_current_evidence` calls and the operator's revocation. The import commit is the
//! only writer of a new record.

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

/// Read the connector's current evidence under the verified tenant.
pub(crate) async fn serve_read(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: medulla_catalog_types::connector_pack::ConnectorPackConformanceEvidenceRequest,
) -> Response {
    #[cfg(not(feature = "redb"))]
    {
        let _ = (state, verified, request);
        Response::err(req_id, "ConnectorPack evidence requires the redb feature")
    }
    #[cfg(feature = "redb")]
    {
        if request.tenant_id
            != medulla_persistence_records::connector_pack::pack_tenant_key(verified)
        {
            return Response::err(
                req_id,
                "ACCESS_DENIED: connector pack tenant must match verified request tenant",
            );
        }
        let store = match super::agent_library_store(state).await {
            Ok(store) => store,
            Err(error) => return Response::err(req_id, error),
        };
        let (tenant_id, connector) = (request.tenant_id, request.connector);
        let now_ms = medulla_server_api::txn::now_ms();
        let evidence = medulla_server_api::api::dispatch_support::blocking_task(
            "connector pack conformance evidence",
            move || store.connector_pack_conformance_evidence(&tenant_id, &connector, now_ms),
        )
        .await;
        respond(req_id, evidence, false)
    }
}

/// Revoke the connector's current evidence.
pub(crate) async fn serve_revoke(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: medulla_catalog_types::connector_pack::ConnectorPackRevokeEvidenceRequest,
) -> Response {
    #[cfg(not(feature = "redb"))]
    {
        let _ = (state, verified, request);
        Response::err(req_id, "ConnectorPack evidence requires the redb feature")
    }
    #[cfg(feature = "redb")]
    {
        let tenant_pack_guard = match super::tenant_pack_lock(verified.tenant()).await {
            Ok(guard) => guard,
            Err(error) => return Response::err(req_id, error),
        };
        let store = match super::agent_library_store(state).await {
            Ok(store) => store,
            Err(error) => return Response::err(req_id, error),
        };
        let context =
            match medulla_persistence_records::agent_library::context::bind_agent_library_context(
                &store,
                req_id,
                verified,
                request.context,
                "connector-pack:conformance-revoke",
                true,
            ) {
                Ok(context) => context,
                Err(error) => return Response::err(req_id, error),
            };
        let connector = request.connector;
        let evidence = medulla_server_api::api::dispatch_support::blocking_task(
            "connector pack conformance revoke",
            move || {
                let _held = tenant_pack_guard;
                store.revoke_connector_conformance_evidence(context, &connector)
            },
        )
        .await;
        respond(req_id, evidence, true)
    }
}

#[cfg(feature = "redb")]
fn respond(
    req_id: u64,
    evidence: Result<
        medulla_catalog_types::connector_pack::conformance_evidence::ConformanceEvidence,
        String,
    >,
    revoked: bool,
) -> Response {
    use medulla_protocol_types::protocol::ResultPayload;
    use medulla_protocol_types::result_contract::storage::{
        ConnectorPackConformanceEvidence, ConnectorPackRevokeConformanceEvidence,
    };

    let payload = evidence.and_then(|evidence| {
        if revoked {
            ResultPayload::of_ref::<ConnectorPackRevokeConformanceEvidence>(&evidence)
        } else {
            ResultPayload::of_ref::<ConnectorPackConformanceEvidence>(&evidence)
        }
        .map_err(|error| error.to_string())
    });
    match payload {
        Ok(payload) => Response::ok(req_id, payload),
        Err(error) => Response::err(req_id, error),
    }
}
