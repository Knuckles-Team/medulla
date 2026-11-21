use super::*;

/// Same boundary as [`commit_gateway`](medulla_server::api::ingest::graph::commit_gateway), for the five coalescable structural
/// writes (`AddNode`/`RemoveNode`/`AddEdge`/`RemoveEdge`/CAS) — routed through
/// `mutation::commit_coalescable_mutation` instead of `commit_mutation` so
/// their prepare→durable-commit→RAM-publish sequence can be queued onto the
/// per-graph routed-write-coalescer worker (CONCEPT:MEDULLA-KG.sharding.per-graph-write-coalescer, L18
/// rewrite) rather than run inline under this request's own lock hold. `apply`
/// must be `Send + 'static`: it may run on the worker task, after this
/// request's own stack frame is gone — every call site below already passes an
/// owned `move` closure over cloned `String`/`Vec<u8>` fields, so this is not a
/// new constraint on what those closures may capture.
pub(super) async fn commit_gateway_coalescable<F>(
    ctx: &MutationCtx<'_>,
    plan: &MutationPlan,
    method: &Method,
    apply: F,
) -> Response
where
    F: FnOnce(&GraphCore) -> Result<ResultPayload, String> + Send + 'static,
{
    Box::pin(mutation::commit_coalescable_mutation(
        ctx, plan, method, apply,
    ))
    .await
}

/// Shared conversion tail for graph, query, and RDF gateway closures
/// (CONCEPT:MEDULLA-KG.mining.tsdb-typed-absent): a handler `Response` carries its own
/// refusal/result, so folding it into the
/// `apply: FnOnce(&GraphCore) -> Result<ResultPayload, String>` shape `commit_conditional_mutation`
/// expects is identical work at every one of the 17 `Mine*` call sites, plus the
/// GraphLearn*/MiningPipeline* families (same `commit_conditional_mutation` shape,
/// gated on different features). Not itself `mining`-gated since
/// GraphLearn*/MiningPipeline* need it under their own, different features.
#[cfg(any(
    feature = "query",
    feature = "cypher",
    feature = "graphql",
    feature = "rdf",
    feature = "graphlearn",
    feature = "ml-pipeline"
))]
pub fn mining_response_to_gateway_result(resp: Response) -> Result<ResultPayload, String> {
    mutation::response_to_gateway_result(resp)
}

/// Route a [`mutation::GATEWAY_ROUTED`] method through the single commit gateway
/// (CONCEPT:MEDULLA-P0-2). Called from `dispatch_graph_op` AHEAD of both the write-
/// coalescer and the terminal exhaustive match below, so a routed method NEVER falls
/// through to `g.add_node(...)` etc. directly — the only path left for it is this
/// one, which builds a [`MutationPlan`] straight from `medulla_capabilities::policy` and
/// calls [`mutation::commit_mutation`]. A method NOT in the routed set is handed
/// straight back (`Err(method)`), unchanged, exactly like every other domain
/// router in `dispatch.rs`'s routing chain.
#[allow(clippy::too_many_arguments)]
pub async fn try_handle_gateway(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    caller: Option<&str>,
    attempt_nonce: Option<medulla_types::contract::Nonce>,
    idempotency_key: &str,
    tenant_scope: &str,
    tenant_id: &str,
    graph_name: &str,
    core: &Arc<GraphCore>,
    materialization_manifest: Option<
        &Arc<std::sync::RwLock<medulla_core::registry::MaterializationManifest>>,
    >,
    read_authority: Option<&GraphReadAuthority>,
    persistence: Option<&Arc<dyn PersistenceBackend>>,
    #[cfg(feature = "streaming")] cdc: Option<&Arc<medulla_server_cdc::cdc::CdcHub>>,
    write_coalescer: Option<
        &Arc<medulla_server_api::routed_write_coalescer::RoutedWriteCoalescerRegistry>,
    >,
    authz_ctx: Option<&GatewayAuthzCtx>,
    // CONCEPT:MEDULLA-KG.mining.tsdb-typed-absent — the server's live tsdb store, needed ONLY by
    // the gateway-routed `Mine*` arms below to bind a plan-sourced `Op::TsScan` leg (mirrors
    // `mining::try_handle`'s own binding for the one Mine* method NOT gateway-routed).
    #[cfg(all(feature = "mining", feature = "query", feature = "tsdb"))] tsdb_store: Option<
        &Arc<medulla_tsdb::store::SeriesStore>,
    >,
    method: Method,
) -> Result<Response, Method> {
    if !mutation::is_gateway_routed(&method) {
        return Err(method);
    }
    // L11 batch 4: the query surface (`Sql`/`CypherQuery`/`GraphQl`) and the native
    // RDF write surface (`AddTriples`/`RemoveTriples`/`DropNamedGraph`) ARE
    // `GATEWAY_ROUTED`, but their execution is `async` and needs `state`/`rls` that this graph-ops entry point
    // does not carry — so they are routed via `commit_conditional_mutation_async` at
    // their OWN dispatch sites in `dispatch.rs`. Hand them back here so they reach
    // those sites; the `record_method`/`cdc_*` gating in `dispatch.rs` already keys
    // off the SAME `is_gateway_routed`, so nothing double-applies.
    if mutation::is_query_gateway_method(&method) || mutation::is_rdf_gateway_method(&method) {
        return Err(method);
    }
    // Runtime-conditional gateway methods may be reads (`writeback = false`).
    // Give those closures a detached, row-filtered core; writes ignore any read
    // authority and must keep operating on the authoritative graph. This is
    // deliberately after both hand-back checks above, so SQL/RDF reads retain
    // their existing snapshot-level RLS path without paying for a second copy.
    let mutates = requires_write(&method);
    let projected_core = if mutates {
        None
    } else {
        read_authority.map(|authority| authority.project_core(core))
    };
    let selected_core = projected_core.as_ref().unwrap_or(core);
    debug_assert!(
        !mutates || Arc::ptr_eq(selected_core, core),
        "mutation gateway must retain the authoritative serving projection"
    );
    let core = selected_core;
    let (isolation, graph_type, owner) = authz_ctx.expect(
        "dispatch_graph_op must capture a GatewayAuthzCtx for every mutation::is_gateway_routed method",
    );

    #[cfg(feature = "security")]
    let method = super::gateway_graph::stamp_owner_id_if_applicable(method, caller, isolation);

    // MEDULLA-SRC-AUTH-R011.3: the trust mark is stamped (AddNode) or change-checked
    // (CompareAndSetNodeFields) at the same pre-commit point, so the persisted blob and the
    // applied one carry the same mark.
    #[cfg(feature = "security")]
    let method = match super::gateway_graph::stamp_trust_mark_if_applicable(
        method, core, caller, isolation,
    ) {
        Ok(stamped) => stamped,
        Err(error) => return Ok(Response::err(req_id, error)),
    };

    // MEDULLA-ORCH-VOCAB-R801.3: a harness-component version write is admitted at
    // the SAME pre-commit point — validated, bitemporally stamped and linked
    // to the current version before either the durable commit or the apply
    // closure sees the method. The admitted (possibly stamped) method is the
    // one that commits.
    // MEDULLA-ORCH-VOCAB-R013.7.2: a walkthrough's repository-index and blob checks
    // run here, through the injected read seams, before admission; admission reads
    // only their result. A failed check refuses before any harness admission runs.
    let walkthrough_checks = {
        let readers = state.read().await.seams.walkthrough.get().cloned();
        // The artifact blob is owned by the carrier owner scope of the writing request;
        // a request with no verified carrier owns nothing, so its artifact reads as absent.
        let blob_owner = read_authority
            .and_then(|authority| authority.carrier())
            .map_or("", |carrier| carrier.owner_scope());
        crate::harness_admission::resolve_walkthrough_checks(
            &method,
            tenant_id,
            blob_owner,
            readers.as_ref(),
        )
    };
    if let crate::harness_admission::WalkthroughChecks::Refused(refusal) = walkthrough_checks {
        return Ok(Response::err(
            req_id,
            format!(
                "{}: the walkthrough does not resolve against the repository index or blob store",
                refusal.code().as_str()
            ),
        ));
    }
    let method = match crate::harness_admission::direct_add_node_checked(
        &method,
        core,
        medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
        caller,
        walkthrough_checks,
    ) {
        Ok(admitted) => admitted,
        Err(error) => return Ok(Response::err(req_id, error)),
    };

    // MEDULLA-IDENTITY-R023.9: a typed metadata node write is admitted here with
    // the verified principal's hashed identity, so the write the graph applies
    // and the write that commits carry the same engine-stamped owner and actor.
    let method = match admit_metadata_write(method, read_authority, tenant_scope) {
        Ok(admitted) => admitted,
        Err(error) => return Ok(Response::err(req_id, error)),
    };

    let plan = MutationPlan::for_method(&method);
    // The verified request principal, hashed for persistence: ownership
    // bindings (the runtime ontology importer, R103.3) key on it, not on the agent.
    let principal = read_authority.map(|authority| {
        medulla_server_api::mutation_batch::identity_metadata::principal_persistence_id(
            authority.verified_principal(),
        )
    });
    let ctx = MutationCtx {
        anonymous: state.read().await.local_anonymous,
        req_id,
        caller,
        principal: principal.as_deref(),
        attempt_nonce,
        idempotency_key,
        tenant_scope,
        graph_name,
        graph_type: *graph_type,
        owner: owner.as_deref(),
        isolation,
        core,
        persistence,
        #[cfg(feature = "streaming")]
        cdc,
        materialization_manifest,
        write_coalescer,
    };
    // MEDULLA-SRC-AUTH-R012.6: a source sync plans against the caller's visible
    // graph and takes its rows under the verified carrier, so it enters here,
    // where both are in hand, before the carrier-less route table.
    if matches!(method, Method::SourceSync { .. }) {
        let visible =
            read_authority.map_or_else(|| core.clone(), |authority| authority.project_core(core));
        return Ok(super::source_sync::handle_source_sync(
            state,
            &ctx,
            &visible,
            read_authority.and_then(|authority| authority.carrier()),
            &method,
        )
        .await);
    }
    // MEDULLA-SRC-AUTH-R801: a policy source bind reads the head component of its
    // tenant and commits the one activation node.
    if matches!(method, Method::PolicySourceBind { .. }) {
        return Ok(
            super::policy_source::handle_policy_source_bind(state, &ctx, tenant_id, &method).await,
        );
    }
    // MEDULLA-DURABLE-KERNEL-R700.3: a cleanup step removes under the engine actor,
    // never under the caller that asked for it.
    if matches!(method, Method::RetentionCleanupRun { .. }) {
        let ctx = MutationCtx {
            caller: Some(super::retention_cleanup::RETENTION_ACTOR),
            principal: None,
            ..ctx
        };
        return Ok(super::retention_cleanup::handle_retention_cleanup_run(&ctx, &method).await);
    }
    // MEDULLA-MEDIA-VOCAB-R650.3: the attachment digests of a remember that the carrier's owner
    // scope holds, read here where the carrier and the blob state are in hand.
    let known_digests = super::memory_surface::known_attachment_digests(
        state,
        read_authority
            .and_then(|authority| authority.carrier())
            .map_or("", |carrier| carrier.owner_scope()),
        &method,
    )
    .await;
    if let Some(resp) =
        super::gateway_graph::try_handle(state, tenant_id, &ctx, &plan, &method, &known_digests)
            .await
    {
        return Ok(resp);
    }
    #[cfg(feature = "broker")]
    if let Some(resp) = super::gateway_broker::try_handle(&ctx, &plan, &method).await {
        return Ok(resp);
    }
    #[cfg(any(feature = "graphlearn", feature = "ml-pipeline"))]
    if let Some(resp) = super::gateway_mining_ml::try_handle(&ctx, &plan, &method).await {
        return Ok(resp);
    }
    // The data-mining gateway lives with the mining handler (a sibling crate): reach it
    // through the query seam. An absent seam leaves the method to the unreachable below,
    // the same as a build without `mining`.
    #[cfg(feature = "mining")]
    {
        let query = medulla_server_ops::api::query_txn::query_seam(state).await;
        if let Some(query) = query {
            if let Some(resp) = query
                .mining_gateway_try_handle(
                    &ctx,
                    &plan,
                    &method,
                    graph_name,
                    read_authority,
                    #[cfg(all(feature = "query", feature = "tsdb"))]
                    tsdb_store,
                )
                .await
            {
                return Ok(resp);
            }
        }
    }
    unreachable!("mutation::is_gateway_routed guarantees method is one of the routed variants")
}

/// Admit a typed metadata node write before it is applied. Only a node write
/// that carries a metadata label changes; every other method passes through.
/// Without a verified carrier there is no principal here, and the
/// compile-time admission alone decides.
fn admit_metadata_write(
    method: Method,
    read_authority: Option<&GraphReadAuthority>,
    tenant_scope: &str,
) -> Result<Method, String> {
    use medulla_server_api::mutation_batch::identity_metadata::{
        admit, principal_persistence_id, MetadataCaller,
    };
    // The owner is the verified request principal (MEDULLA-IDENTITY-R023.3),
    // not the agent carrying a delegated request.
    let Some(principal) = read_authority
        .filter(|authority| authority.carrier().is_some())
        .map(GraphReadAuthority::verified_principal)
        .filter(|principal| !principal.is_empty())
        .map(principal_persistence_id)
    else {
        return Ok(method);
    };
    // Only gateway-routed node writes reach this function; a staged
    // transaction node write is admitted when its commit compiles.
    if !matches!(
        method,
        Method::AddNode { .. } | Method::CreateNodeIfAbsent { .. }
    ) {
        return Ok(method);
    }
    let caller = MetadataCaller {
        principal: Some(&principal),
        tenant: tenant_scope,
    };
    let mut admitted = admit(method, &caller).map_err(|refusal| refusal.to_string())?;
    // The first method is the node write itself; the interaction edges an
    // `InteractionNetwork` adds are compiled from it again at commit.
    Ok(admitted.swap_remove(0))
}
