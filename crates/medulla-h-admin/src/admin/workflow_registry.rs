//! Workflow registry reads (MEDULLA-DECISION-ENGINE-R166.3, R166.4).
//!
//! `WorkflowRegistry::Read` is a graph read: this catalog admin route carries
//! reads only and holds no write. The registration is looked up on the caller's
//! row-level-security projection of the request graph, so a node the caller
//! cannot see is an unknown identity, then answered through
//! `WorkflowRegistry::read` by [`read_registration`].

use std::sync::Arc;

use medulla_graph_core::graph::GraphCore;
use medulla_isolation::isolation::AccessLevel;
use medulla_protocol_types::protocol::{Response, ResultPayload};
use medulla_protocol_types::result_contract::coordination::WorkflowRegistryRead;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::carrier::GraphReadAuthority;
use medulla_server_api::state::ServerState;
use medulla_work_types::orchestration::workflows::{
    read_registration, WorkflowIdentity, WorkflowRegistrationEntry, WorkflowRegistryError,
};
use tokio::sync::RwLock;

/// Read `identity` from the nodes of `core` the caller `caller_scope` can see.
pub(super) fn read_from_core(
    core: &GraphCore,
    caller_scope: &str,
    identity: &WorkflowIdentity,
) -> Result<WorkflowRegistrationEntry, WorkflowRegistryError> {
    let node_id = WorkflowRegistrationEntry::node_id(identity);
    let stored = core
        .get_node_properties(&node_id)
        .and_then(|blob| medulla_types::msgpack::decode_property_object(&blob).ok());
    read_registration(stored.as_ref(), caller_scope, identity)
}

/// The catalog admin route of `Method::WorkflowRegistry` with `Read`.
pub async fn handle_workflow_registry_read(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    graph: &str,
    verified: &VerifiedRequestContext,
    identity: WorkflowIdentity,
) -> Response {
    let (core, authority) = {
        let guard = state.read().await;
        let Some(entry) = guard.registry.get(graph) else {
            return Response::err(req_id, format!("INVALID_ARGUMENT: unknown graph {graph}"));
        };
        if let Err(denied) = medulla_server_auth::access::check_graph_access(
            &guard.isolation,
            Some(verified.agent_id()),
            graph,
            entry.graph_type,
            entry.owner.as_deref(),
            AccessLevel::Read,
        ) {
            return Response::err(req_id, denied);
        }
        let authority = match GraphReadAuthority::from_verified(verified, &guard.isolation) {
            Ok(authority) => authority,
            Err(denied) => return Response::err(req_id, denied),
        };
        (Arc::clone(&entry.core), authority)
    };
    let caller_scope = authority
        .carrier()
        .map(|carrier| carrier.tenant_scope())
        .unwrap_or_default();
    let projected = authority.project_core(&core);
    match read_from_core(&projected, caller_scope, &identity) {
        Ok(entry) => Response::ok(req_id, ResultPayload::of::<WorkflowRegistryRead>(entry)),
        Err(error) => Response::err(req_id, format!("{}: {identity:?}", error.code())),
    }
}

#[cfg(test)]
#[path = "workflow_registry_tests.rs"]
mod tests;
