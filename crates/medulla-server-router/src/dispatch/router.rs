#[cfg(feature = "cost")]
use super::request_boundary::dispatch_resource_stats;
use super::request_boundary::{
    append_native_capacity_ops, append_native_resource_ops, append_native_work_item_ops,
    dispatch_boxed, DispatchAuthority,
};
use crate::dispatch::*;
use medulla_server_api::handlers::{
    DispatchCtx, DispatchHeader, HandlerFuture, HandlerRegistry, MethodHandler,
};
use medulla_server_consensus::{
    dispatch::consensus::handle_fleet_catalog, dispatch::consensus::handle_list_registered_servers,
    dispatch::consensus::handle_register_server,
    dispatch::consensus::replicated_identity_bootstrap_authorized,
    dispatch::consensus::ServerRegistration,
};
use medulla_server_graph::dispatch::graph_pipeline::dispatch_graph_op;
#[cfg(feature = "knowledge-batch")]
use medulla_server_graph::dispatch::graph_pipeline::dispatch_knowledge_stream;
#[cfg(feature = "modality-serving")]
use medulla_server_graph::dispatch::graph_pipeline::dispatch_served_modality;
use medulla_server_graph::dispatch::request_boundary::screen::decode_screen_observation;
#[cfg(feature = "sparql-http")]
use medulla_server_graph::dispatch::sparql_update::coordinated_sparql_http_update;
use medulla_server_graph::{
    dispatch::change_envelope::dispatch_change_envelopes,
    dispatch::change_envelope::multi_graph_batch_update,
};
use medulla_server_graph::{
    dispatch::policy_evolution::handle_policy_evolution,
    dispatch::policy_evolution::PolicyEvolutionTarget,
};
#[cfg(feature = "ast")]
use medulla_server_graph::{
    dispatch::request_boundary::preflight::ast_input_limits,
    dispatch::request_boundary::preflight::decode_ast_files,
    dispatch::request_boundary::preflight::validate_ast_logical_path,
};
use std::sync::OnceLock;

mod branch_write;
mod channels;
mod control_plane;
mod data_plane;
mod data_plane_arms;
mod decision_plane;
mod graph_lifecycle;
mod identity_access;
mod incident;
mod lifecycle;
mod resource_cost;
mod service_control;
mod source_ingest;
mod telemetry;

use channels::dispatch_channel_methods;
use control_plane::{
    dispatch_agent_library_methods, dispatch_cluster_admin_methods,
    dispatch_compute_and_media_methods, dispatch_graph_translated_op_families,
};
#[cfg(feature = "query")]
use data_plane::dispatch_sql_source_methods;
use decision_plane::dispatch_decision_plane_methods;

use data_plane::{
    dispatch_change_envelope_methods, dispatch_governed_stream_write_methods,
    dispatch_method_scoped_graph_methods, dispatch_store_methods, dispatch_streaming_methods,
    dispatch_transaction_methods,
};
use graph_lifecycle::dispatch_graph_lifecycle_methods;
use identity_access::dispatch_identity_and_access_methods;
use incident::dispatch_incident_methods;
#[cfg(feature = "security")]
use lifecycle::apply_rbac_admin;
pub(in crate::dispatch) use lifecycle::{create_graph, GraphLifecycleRequest};
use lifecycle::{delete_graph, dispatch_get_identity, index_kind_label, index_validity_label};
use resource_cost::dispatch_resource_cost_methods;
use service_control::dispatch_service_control_methods;
use source_ingest::dispatch_source_ingest_methods;
use telemetry::dispatch_telemetry_methods;

/// The one guarded arm: a SPARQL-over-HTTP UPDATE arrives as an
/// `ApplyMutation` whose event type is the SPARQL-HTTP update marker. Any
/// other `ApplyMutation` falls through to the ordinary per-graph chain,
/// exactly as the guard's own fallthrough did.
#[cfg(feature = "sparql-http")]
async fn dispatch_sparql_http_update(
    ctx: DispatchCtx<'_>,
    method: Method,
) -> ControlFlow<Response, Method> {
    let DispatchCtx {
        state,
        req,
        verified_context,
        ..
    } = ctx;
    ControlFlow::Break(match method {
        Method::ApplyMutation { event_type, query }
            if event_type
                == medulla_server_api::api::dispatch_support::SPARQL_HTTP_UPDATE_EVENT =>
        {
            coordinated_sparql_http_update(
                state,
                req.id,
                req.agent_id.as_deref(),
                verified_context,
                &req.graph,
                query,
            )
            .await
        }
        other => return ControlFlow::Continue(other),
    })
}

/// The method handlers in dispatch order: the control plane (service control,
/// source ingestion, cost telemetry, graph lifecycle, cluster administration,
/// channels, identity/access, compute/media), then the data plane (transactions,
/// the non-graph stores, the subscription plane, the governed stream writes,
/// change-envelope replication, the method-scoped graph targets and the guarded
/// SPARQL-over-HTTP update). A method none of them claims is handed back for the
/// ordinary per-graph chain.
pub(in crate::dispatch) fn method_handler_registry() -> &'static HandlerRegistry {
    static REGISTRY: OnceLock<HandlerRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = HandlerRegistry::new();
        registry.register(PlaneHandler::new("service_control", |ctx, method| {
            Box::pin(dispatch_service_control_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("source_ingest", |ctx, method| {
            Box::pin(dispatch_source_ingest_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("telemetry", |ctx, method| {
            Box::pin(dispatch_telemetry_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("incident", |ctx, method| {
            Box::pin(dispatch_incident_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("resource_cost", |ctx, method| {
            Box::pin(dispatch_resource_cost_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("graph_lifecycle", |ctx, method| {
            Box::pin(dispatch_graph_lifecycle_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("agent_library", |ctx, method| {
            Box::pin(dispatch_agent_library_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("decision_plane", |ctx, method| {
            Box::pin(dispatch_decision_plane_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("cluster_admin", |ctx, method| {
            Box::pin(dispatch_cluster_admin_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("graph_translated", |ctx, method| {
            Box::pin(dispatch_graph_translated_op_families(ctx, method))
        }));
        registry.register(PlaneHandler::new("channels", |ctx, method| {
            Box::pin(dispatch_channel_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("identity_access", |ctx, method| {
            Box::pin(dispatch_identity_and_access_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("compute_media", |ctx, method| {
            Box::pin(dispatch_compute_and_media_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("transactions", |ctx, method| {
            Box::pin(dispatch_transaction_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("stores", |ctx, method| {
            Box::pin(dispatch_store_methods(ctx, method))
        }));
        #[cfg(feature = "query")]
        registry.register(PlaneHandler::new("sql_source", |ctx, method| {
            Box::pin(dispatch_sql_source_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("streaming", |ctx, method| {
            Box::pin(dispatch_streaming_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("governed_stream_write", |ctx, method| {
            Box::pin(dispatch_governed_stream_write_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("change_envelope", |ctx, method| {
            Box::pin(dispatch_change_envelope_methods(ctx, method))
        }));
        registry.register(PlaneHandler::new("method_scoped_graph", |ctx, method| {
            Box::pin(dispatch_method_scoped_graph_methods(ctx, method))
        }));
        #[cfg(feature = "sparql-http")]
        registry.register(PlaneHandler::new("sparql_http_update", |ctx, method| {
            Box::pin(dispatch_sparql_http_update(ctx, method))
        }));
        registry
    })
}

/// The signature of one plane dispatcher.
type PlaneFn = for<'a> fn(DispatchCtx<'a>, Method) -> HandlerFuture<'a>;

/// A plane dispatcher function registered as a [`MethodHandler`].
struct PlaneHandler {
    name: &'static str,
    run: PlaneFn,
}

impl PlaneHandler {
    fn new(name: &'static str, run: PlaneFn) -> Self {
        Self { name, run }
    }
}

impl MethodHandler for PlaneHandler {
    fn name(&self) -> &'static str {
        self.name
    }

    fn try_handle<'a>(&'a self, ctx: DispatchCtx<'a>, method: Method) -> HandlerFuture<'a> {
        (self.run)(ctx, method)
    }
}

/// Route one authenticated request to its handler.
///
/// The single 59-arm `match req.method` this replaced is now the ordered
/// handler registry, one handler per DOMAIN over disjoint `Method` variants; each hands a method it does not own back as
/// `ControlFlow::Continue`. A method no group claims falls through to the
/// ordinary per-graph chain, which is exactly what the old `_` arm did.
///
/// Returns a boxed `Send` future instead of being an `async fn`: the router
/// tree contains handlers that re-enter `dispatch()`, so an opaque return type
/// here makes the `Send` auto-trait check cyclic with
/// `dispatch_admitted_request` (E0391), and an unboxed one deepens `dispatch()`'s
/// layout past the query depth limit with every newly served RPC.
pub fn dispatch_request_method<'a>(
    state: &'a Arc<RwLock<ServerState>>,
    req: Request,
    verified_context: &'a VerifiedRequestContext,
    authority: DispatchAuthority,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Response> + Send + 'a>> {
    Box::pin(dispatch_request_method_inner(
        state,
        req,
        verified_context,
        authority,
    ))
}

async fn dispatch_request_method_inner(
    state: &Arc<RwLock<ServerState>>,
    req: Request,
    verified_context: &VerifiedRequestContext,
    authority: DispatchAuthority,
) -> Response {
    let method = req.method;
    let req = DispatchHeader {
        id: req.id,
        graph: req.graph,
        agent_id: req.agent_id,
    };
    let ctx = DispatchCtx {
        state,
        req: &req,
        verified_context,
        state_machine_authorized: authority.state_machine_authorized(),
        identity_bootstrap: authority.identity_bootstrap(),
    };
    let method = match method_handler_registry().dispatch(ctx, method).await {
        ControlFlow::Break(response) => return response,
        ControlFlow::Continue(method) => method,
    };
    dispatch_boxed(dispatch_graph_op(
        state,
        &req.graph,
        req.id,
        req.agent_id.as_deref(),
        verified_context,
        method,
    ))
    .await
}
