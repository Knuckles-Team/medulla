use super::*;
/// Everything the runtime-conditional query/RDF gateways need from the resolved
/// request, bundled so each router stays at the documented parameter cap. The
/// borrowed/owned split matches how `run_dispatch_pipeline` already held these
/// values: identity and authz captures are borrowed, the `Arc` handles are
/// cloned per stage because the mutation gateway moves them into an async apply
/// closure. Same shape as `crate::mutation::MutationCtx`.
pub struct GatewayRouteCtx<'a> {
    pub state: &'a Arc<RwLock<ServerState>>,
    pub req_id: u64,
    pub graph_name: &'a str,
    pub caller: Option<&'a str>,
    pub attempt_nonce: Option<medulla_types::contract::Nonce>,
    pub idempotency_key: &'a str,
    pub tenant_scope: &'a str,
    pub core: Arc<medulla_graph_core::graph::GraphCore>,
    pub persistence: Option<Arc<dyn medulla_server_api::api::persistence::PersistenceBackend>>,
    #[cfg(feature = "streaming")]
    pub cdc: Option<Arc<medulla_server_cdc::cdc::CdcHub>>,
    pub materialization_manifest:
        Option<Arc<std::sync::RwLock<medulla_core::registry::MaterializationManifest>>>,
    pub gateway_authz_ctx: &'a Option<medulla_server_mutation::mutation::GatewayAuthzCtx>,
    pub read_authority: &'a Option<GraphReadAuthority>,
    pub verified_actor: &'a str,
    #[cfg(feature = "security")]
    pub rls: std::sync::Arc<medulla_isolation::isolation::IsolationLayer>,
}

#[cfg(any(
    feature = "query",
    feature = "cypher",
    feature = "graphql",
    feature = "rdf"
))]
async fn gateway_mutation_context<'a>(
    ctx: &'a GatewayRouteCtx<'_>,
    missing_authz_message: &str,
) -> medulla_server_mutation::mutation::MutationCtx<'a> {
    let (iso, gtype, owner) = ctx.gateway_authz_ctx.as_ref().expect(missing_authz_message);
    medulla_server_mutation::mutation::MutationCtx {
        anonymous: ctx.state.read().await.local_anonymous,
        req_id: ctx.req_id,
        caller: ctx.caller,
        attempt_nonce: ctx.attempt_nonce,
        idempotency_key: ctx.idempotency_key,
        tenant_scope: ctx.tenant_scope,
        graph_name: ctx.graph_name,
        graph_type: *gtype,
        owner: owner.as_deref(),
        isolation: iso,
        core: &ctx.core,
        persistence: ctx.persistence.as_ref(),
        #[cfg(feature = "streaming")]
        cdc: ctx.cdc.as_ref(),
        materialization_manifest: ctx.materialization_manifest.as_ref(),
        write_coalescer: None,
    }
}

#[cfg(any(feature = "query", feature = "cypher", feature = "graphql"))]
async fn commit_query_gateway(ctx: GatewayRouteCtx<'_>, method: Method) -> Response {
    if medulla_server_mutation::mutation::is_query_gateway_method(&method)
        && !medulla_server_mutation::mutation::is_query_native_coordinator(&method)
    {
        let mutates_now = requires_write(&method);
        let plan = medulla_server_mutation::mutation::MutationPlan::for_method(&method);
        let mutation_ctx = gateway_mutation_context(
            &ctx,
            "is_gateway_routed query method must have a captured GatewayAuthzCtx",
        )
        .await;
        let method_apply = method.clone();
        let query_read_authority = ctx.read_authority.clone();
        #[cfg(feature = "security")]
        let rls_apply = ctx.rls.clone();
        let state = ctx.state;
        let req_id = ctx.req_id;
        let graph_name = ctx.graph_name;
        let verified_actor = ctx.verified_actor;
        let resp = medulla_server_mutation::mutation::commit_conditional_mutation_async(
            &mutation_ctx,
            &plan,
            &method,
            mutates_now,
            move |staged_core| async move {
                medulla_h_query::query::try_handle(
                    state,
                    medulla_server_api::api::reentry::TryHandleContext {
                        req_id,
                        graph_name,
                        read_authority: query_read_authority.as_ref(),
                        caller: verified_actor,
                    },
                    staged_core,
                    method_apply,
                    #[cfg(feature = "security")]
                    &rls_apply,
                )
                .await
                .map_err(|_| {
                    "ENGINE_UNAVAILABLE: query surface not available in this build".to_string()
                })
                .and_then(medulla_h_graph::graph_ops::gateway_response_result)
            },
        )
        .await;
        return resp;
    }
    unreachable!("gateway commit helper called for an unclassified method");
}

#[cfg(any(feature = "query", feature = "cypher", feature = "graphql"))]
pub async fn route_query_gateway(
    ctx: GatewayRouteCtx<'_>,
    method: Method,
) -> Result<Response, Method> {
    if medulla_server_mutation::mutation::is_query_gateway_method(&method)
        && !medulla_server_mutation::mutation::is_query_native_coordinator(&method)
    {
        return Ok(commit_query_gateway(ctx, method).await);
    }
    match medulla_h_query::query::try_handle(
        ctx.state,
        medulla_server_api::api::reentry::TryHandleContext {
            req_id: ctx.req_id,
            graph_name: ctx.graph_name,
            read_authority: ctx.read_authority.as_ref(),
            caller: ctx.verified_actor,
        },
        ctx.core.clone(),
        method,
        #[cfg(feature = "security")]
        &ctx.rls,
    )
    .await
    {
        Ok(r) => Ok(r),
        Err(m) => Err(m),
    }
}
#[cfg(feature = "rdf")]
async fn commit_rdf_gateway(ctx: GatewayRouteCtx<'_>, method: Method) -> Response {
    if medulla_server_mutation::mutation::is_rdf_gateway_method(&method) {
        let plan = medulla_server_mutation::mutation::MutationPlan::for_method(&method);
        let mutation_ctx = gateway_mutation_context(
            &ctx,
            "is_gateway_routed rdf method must have a captured GatewayAuthzCtx",
        )
        .await;
        let method_apply = method.clone();
        let rdf_read_authority = ctx.read_authority.clone();
        #[cfg(feature = "security")]
        let rls_apply = ctx.rls.clone();
        let state = ctx.state;
        let req_id = ctx.req_id;
        let graph_name = ctx.graph_name;
        let verified_actor = ctx.verified_actor;
        let resp = medulla_server_mutation::mutation::commit_conditional_mutation_async(
            &mutation_ctx,
            &plan,
            &method,
            true,
            move |staged_core| async move {
                medulla_h_query::rdf::try_handle(
                    state,
                    medulla_server_api::api::reentry::TryHandleContext {
                        req_id,
                        graph_name,
                        read_authority: rdf_read_authority.as_ref(),
                        caller: verified_actor,
                    },
                    staged_core,
                    method_apply,
                    #[cfg(feature = "security")]
                    &rls_apply,
                )
                .await
                .map_err(|_| {
                    "ENGINE_UNAVAILABLE: rdf surface not available in this build".to_string()
                })
                .and_then(medulla_h_graph::graph_ops::gateway_response_result)
            },
        )
        .await;
        return resp;
    }
    unreachable!("gateway commit helper called for an unclassified method");
}

#[cfg(feature = "rdf")]
pub async fn route_rdf_gateway(
    ctx: GatewayRouteCtx<'_>,
    method: Method,
) -> Result<Response, Method> {
    if medulla_server_mutation::mutation::is_rdf_gateway_method(&method) {
        return Ok(commit_rdf_gateway(ctx, method).await);
    }
    match medulla_h_query::rdf::try_handle(
        ctx.state,
        medulla_server_api::api::reentry::TryHandleContext {
            req_id: ctx.req_id,
            graph_name: ctx.graph_name,
            read_authority: ctx.read_authority.as_ref(),
            caller: ctx.verified_actor,
        },
        ctx.core.clone(),
        method,
        #[cfg(feature = "security")]
        &ctx.rls,
    )
    .await
    {
        Ok(r) => Ok(r),
        Err(m) => Err(m),
    }
}
