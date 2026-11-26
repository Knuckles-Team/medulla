//! The recovery pass of knowledge-bundle imports (MEDULLA-DURABLE-KERNEL-R877.3).
//!
//! An import writes one `BundleImport` record in the control graph in the `importing` state before
//! it creates any target graph. Each graph it creates carries the import id on its durable
//! registry row, so `check_graph_access` hides it from every read, and the import sets the record
//! `ready` after the last step. A process that stopped in between leaves the record `importing`
//! and the graphs marked: they stay hidden after a restart with no help from this pass, because
//! the marker is loaded with the registry. This pass, run by the leader tick, finds such records
//! and, for one older than its grace that this process is not running, deletes the graphs and
//! records the import `aborted`. It is idempotent: a second pass finds the record `aborted` and
//! does nothing. The clock is an input.

use std::sync::Arc;

use medulla_protocol_types::protocol::{Method, Request};
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;
use medulla_server_auth::access::importing;
use medulla_types::contract::Nonce;
use medulla_types::knowledge_bundle::{ImportRecord, ImportState, IMPORT_NODE_TYPE};
use serde_json::Value;
use tokio::sync::RwLock;

/// The service principal that deletes the graphs of an import that never finished.
const SERVICE: &str = "bundle-recovery";

/// A verified context for the recovery service over `graphs`: exact read and write grants on each,
/// and the scope `DeleteGraph` requires.
async fn service_context(
    state: &Arc<RwLock<ServerState>>,
    graphs: &[String],
    session: String,
) -> Result<VerifiedRequestContext, String> {
    let (anonymous, server) = {
        let s = state.read().await;
        (s.local_anonymous, s.context.clone())
    };
    #[cfg(feature = "security")]
    if !anonymous {
        let mut guard = state.write().await;
        for graph in graphs {
            medulla_server_api::service_grant::ServiceGraphGrant {
                actor: "service:bundle-recovery",
                role_prefix: SERVICE,
                unbootstrapped: "BUNDLE_RECOVERY_POLICY_UNBOOTSTRAPPED",
            }
            .ensure(&mut guard.isolation, graph)?;
        }
    }
    #[cfg(not(feature = "security"))]
    let _ = graphs;
    if anonymous {
        return Ok(VerifiedRequestContext::anonymous_local(Some(session)));
    }
    let service = VerifiedRequestContext::authenticated_fixed_service_actor(
        &server,
        SERVICE,
        &["kg:write", "graph:admin"],
    )?;
    Ok(VerifiedRequestContext::from_verified_claims_with_nonce(
        service.claims().clone(),
        session,
        Some(Nonce::minted()),
    ))
}

async fn dispatch(
    state: &Arc<RwLock<ServerState>>,
    graphs: &[String],
    request_graph: &str,
    session: String,
    method: Method,
) -> Result<(), String> {
    let context = service_context(state, graphs, session).await?;
    let response = medulla_server_api::api::reentry::dispatch_verified_request(
        state,
        Request {
            id: 0,
            graph: request_graph.to_string(),
            auth_token: String::new(),
            agent_id: Some(context.agent_id().to_string()),
            method,
            trace_id: None,
        },
        context,
    )
    .await;
    response.refusal_text().map_or(Ok(()), Err)
}

/// The import records of the control graph that are still `importing`.
fn importing_records(core: &medulla_graph_core::graph::GraphCore) -> Vec<ImportRecord> {
    core.nodes_by_property("type", IMPORT_NODE_TYPE)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| {
            let raw = core.get_node_properties(&id)?;
            let properties: Value = medulla_types::msgpack::decode_property_value(&raw).ok()?;
            ImportRecord::from_properties(&properties)
        })
        .filter(|record| record.state == ImportState::Importing)
        .collect()
}

/// Delete the graphs of every `importing` import that is stale at `now_ms` and record it
/// `aborted`; return how many imports were aborted. Graphs of an `importing` import stay hidden
/// by their registry marker.
pub async fn recover_bundle_imports(
    state: &Arc<RwLock<ServerState>>,
    now_ms: u64,
) -> Result<usize, String> {
    let control = medulla_server_api::knob_config::default_ingest_graph();
    let core = {
        let server = state.read().await;
        server.registry.get(control).map(|entry| entry.core.clone())
    };
    let Some(core) = core else {
        return Ok(0);
    };
    let mut aborted = 0;
    for record in importing_records(&core) {
        if importing::is_live(&record.import_id) {
            continue;
        }
        if !record.is_stale(now_ms) {
            continue;
        }
        let session = format!("{SERVICE}:{}", record.import_id);
        importing::within_import(record.import_id.clone(), async {
            for graph in record.graphs.iter().rev() {
                medulla_server_api::api::reentry::ensure_graph_resident(state, graph).await;
                if !state.read().await.registry.exists(graph) {
                    continue;
                }
                dispatch(
                    state,
                    &record.graphs,
                    "__commons__",
                    session.clone(),
                    Method::DeleteGraph {
                        graph_name: graph.clone(),
                    },
                )
                .await?;
            }
            Ok::<(), String>(())
        })
        .await?;
        let operation = record.with_state(ImportState::Aborted).node_operation(SERVICE);
        let operations_msgpack = rmp_serde::to_vec_named(&vec![operation])
            .map_err(|error| format!("import record encoding failed: {error}"))?;
        dispatch(
            state,
            &[control.to_string()],
            control,
            session,
            Method::BatchUpdate { operations_msgpack },
        )
        .await?;
        aborted += 1;
    }
    Ok(aborted)
}
