//! `AgentComponent.content`: serve one component revision's verified bytes.

use medulla_server_blob::blob::BlobStateExt;
use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::protocol::ResultPayload;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

/// Read one component revision through its tenant-scoped Agent Library holder,
/// then verify the engine-owned CAS bytes before returning them.
pub(crate) async fn handle_component_content(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: medulla_agent_types::agent_component::AgentComponentContentRequest,
) -> Response {
    // A committed decision record's body is engine-owned too, but it lives in
    // the Agent Library transaction that committed it, not in the Blob CAS.
    if request
        .component_id
        .starts_with(medulla_decision_types::decision::DECISION_COMPONENT_ID_PREFIX)
    {
        return serve_decision_record(store, req_id, verified, &request);
    }
    serve_component_content(state, store, req_id, verified, request).await
}

fn serve_decision_record(
    store: &AgentLibraryStore,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: &medulla_agent_types::agent_component::AgentComponentContentRequest,
) -> Response {
    if request.tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: component content tenant must match verified request tenant",
        );
    }
    match store.decision_record_content(&request.tenant_id, &request.component_id) {
        Ok(result) => Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentContent,
            >(&result),
        ),
        Err(error) => Response::err(req_id, error),
    }
}

/// One snapshot read of the revision's record and body pointer, then the
/// engine-owned bytes, digest-verified by `read_engine_body`. Shared by
/// `Content` and `ExportBlueprint`, so there is no second read route.
#[cfg(feature = "blob")]
async fn read_verified_body(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    tenant_id: &str,
    component_id: &str,
    entry_revision: Option<u64>,
) -> Result<
    (
        medulla_persistence_records::connector_pack::ConnectorPackContentPointer,
        Vec<u8>,
    ),
    String,
> {
    let pointer = store.connector_pack_content_pointer(tenant_id, component_id, entry_revision)?;
    let blob = match state.read().await.blob_cursors().cloned() {
        Some(blob) => blob,
        None => return Err("AgentComponent.content Blob substrate disabled".to_string()),
    };
    let tenant_id = tenant_id.to_string();
    let read = tokio::task::spawn_blocking(move || {
        medulla_server_blob::blob::engine_bodies::read_engine_body(
            blob.store.as_ref(),
            &tenant_id,
            &pointer.engine_manifest_digest,
            pointer.body_sha256,
            pointer.length,
        )
        .map(|body| (pointer, body))
    })
    .await;
    match read {
        Ok(result) => result,
        Err(error) => Err(format!("component body read task failed: {error}")),
    }
}

async fn serve_component_content(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: medulla_agent_types::agent_component::AgentComponentContentRequest,
) -> Response {
    if request.tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: component content tenant must match verified request tenant",
        );
    }
    #[cfg(not(feature = "blob"))]
    {
        let _ = (state, store, request);
        Response::err(req_id, "AgentComponent.content requires the blob feature")
    }
    #[cfg(feature = "blob")]
    {
        let (pointer, body) = match read_verified_body(
            state,
            store,
            &request.tenant_id,
            &request.component_id,
            request.entry_revision,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return Response::err(req_id, error),
        };
        let result = medulla_agent_types::agent_component::AgentComponentContentResult {
            schema_version: medulla_agent_types::agent_component::COMPONENT_CONTENT_SCHEMA_VERSION,
            component_id: pointer.component_id,
            entry_revision: pointer.entry_revision,
            definition_digest: pointer.definition_digest,
            content_digest: format!("sha256:{}", pointer.body_sha256),
            media_type: pointer.media_type,
            body,
        };
        Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentContent,
            >(&result),
        )
    }
}

/// `AgentComponent.export_blueprint`: the complete admitted record at one pinned
/// revision plus its verified body bytes, never parsed
/// (MEDULLA-TYPED-PACKS-R094.2).
pub(crate) async fn serve_blueprint_export(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: medulla_agent_types::agent_component::AgentComponentBlueprintExportRequest,
) -> Response {
    if request.tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: blueprint export tenant must match verified request tenant",
        );
    }
    if request.entry_revision == 0 {
        return Response::err(
            req_id,
            "INVALID_ARGUMENT: a blueprint export is pinned to a revision of at least 1",
        );
    }
    #[cfg(not(feature = "blob"))]
    {
        let _ = (state, store);
        Response::err(req_id, "AgentComponent.export_blueprint requires the blob feature")
    }
    #[cfg(feature = "blob")]
    {
        let (pointer, body) = match read_verified_body(
            state,
            store,
            &request.tenant_id,
            &request.component_id,
            Some(request.entry_revision),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return Response::err(req_id, error),
        };
        let result = medulla_agent_types::agent_component::BlueprintExport {
            schema_version:
                medulla_agent_types::agent_component::BLUEPRINT_EXPORT_SCHEMA_VERSION,
            content_digest: format!("sha256:{}", pointer.body_sha256),
            media_type: pointer.media_type,
            entry: pointer.entry,
            body,
        };
        Response::ok(
            req_id,
            ResultPayload::of_ref::<
                medulla_protocol_types::result_contract::storage::AgentComponentBlueprintExport,
            >(&result),
        )
    }
}

#[cfg(all(test, feature = "redb", feature = "blob"))]
mod tests {
    use medulla_agent_types::agent_component::{
        AgentComponentBlueprintExportRequest, AgentComponentOp, BlueprintExport,
        BLUEPRINT_EXPORT_SCHEMA_VERSION,
    };
    use medulla_protocol_types::protocol::Method;

    use crate::admin::connector_pack::tests::{
        bind, build_pack, imported, ok, refused, tool, Served, ADMIN, TENANT,
    };

    const CONNECTOR: &str = "blueprint-export";

    fn export(tenant: &str, revision: u64) -> Method {
        Method::AgentComponent {
            op: AgentComponentOp::ExportBlueprint {
                request: AgentComponentBlueprintExportRequest {
                    tenant_id: tenant.to_string(),
                    component_id: format!("mcp:{CONNECTOR}/tool/pinned"),
                    entry_revision: revision,
                },
            },
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R094.2
    #[tokio::test]
    async fn export_pins_revision_and_refuses_unknown() {
        let served = Served::new();
        bind(&served, CONNECTOR, ADMIN).await;
        let pinned = tool(CONNECTOR, "pinned", "A tool pinned for export.");
        imported(
            &served,
            &build_pack(CONNECTOR, std::slice::from_ref(&pinned)),
            None,
        )
        .await;

        // The pinned revision exports its complete record and its exact bytes.
        let exported: BlueprintExport = ok(
            "ExportBlueprint",
            served.call("export:pinned", export(TENANT, 1)).await,
        );
        assert_eq!(exported.schema_version, BLUEPRINT_EXPORT_SCHEMA_VERSION);
        assert_eq!(exported.body, pinned.body);
        assert_eq!(exported.entry.entry_revision, 1);
        assert_eq!(exported.entry.tenant_id, TENANT);
        assert_eq!(
            exported.entry.component_id,
            format!("mcp:{CONNECTOR}/tool/pinned")
        );
        assert_eq!(exported.content_digest, exported.entry.content_digest);
        assert_eq!(exported.media_type, "application/json");

        // A revision the tenant never published is refused by name.
        let unknown = refused(
            "unknown revision",
            served.call("export:unknown", export(TENANT, 99)).await,
        );
        assert!(unknown.starts_with("COMPONENT_REVISION_UNKNOWN"), "{unknown}");

        // An unpinned (zero) revision is an invalid argument.
        let zero = refused(
            "zero revision",
            served.call("export:zero", export(TENANT, 0)).await,
        );
        assert!(zero.starts_with("INVALID_ARGUMENT"), "{zero}");

        // Another tenant is refused before any read.
        let denied = refused(
            "other tenant",
            served.call("export:denied", export("tenant-other", 1)).await,
        );
        assert!(denied.starts_with("ACCESS_DENIED"), "{denied}");
    }
}
