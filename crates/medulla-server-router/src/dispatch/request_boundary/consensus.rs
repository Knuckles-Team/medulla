use super::*;
use medulla_server_graph::dispatch::shared::timed_read;

#[cfg(feature = "raft")]
pub async fn check_cluster_placement_before_consensus(
    state: &Arc<RwLock<ServerState>>,
    req: &Request,
) -> Result<(), Response> {
    if medulla_kernel::kernel::dispatch_support::is_replicated_apply() {
        // The command is already committed in the owning group's log. Its
        // domain kernel must apply locally on every replica without proposing
        // the same command again.
        return Ok(());
    }
    let placement = {
        let current = timed_read(state).await;
        current.placement_authority()
    };
    if !matches!(
        placement,
        medulla_server_api::state::PlacementAuthorityKind::Local
    ) {
        return clustered_route_admission(req, &placement);
    }
    Ok(())
}

/// Admission of one mutation under a non-local placement authority: refuse
/// local-only mutations and missing placement before any proposal.
#[cfg(feature = "raft")]
fn clustered_route_admission(
    req: &Request,
    placement: &medulla_server_api::state::PlacementAuthorityKind,
) -> Result<(), Response> {
    use medulla_server_mutation::mutation::ClusterMutationRoute;
    match medulla_server_mutation::mutation::cluster_mutation_route(&req.method) {
        // `SelfRoutedAdmin` owns its OWN `MultiRaft`-presence check
        // (`medulla_h_admin::raft_admin::try_handle` answers
        // `RAFT_NOT_CONFIGURED`/`CLUSTER_CONFIGURATION_INVALID` itself,
        // matching this exact pair of messages) — it must not be
        // preempted here, exactly like `ReadOnly`/`VolatileControl`.
        ClusterMutationRoute::ReadOnly
        | ClusterMutationRoute::VolatileControl
        | ClusterMutationRoute::SelfRoutedAdmin => {}
        ClusterMutationRoute::LocalOnly => {
            return Err(Response::err(
                req.id,
                medulla_server_mutation::mutation::LOCAL_ONLY_CLUSTER_REFUSAL,
            ));
        }
        ClusterMutationRoute::ConsensusGraph
        | ClusterMutationRoute::ConsensusNative
        | ClusterMutationRoute::ConsensusFanout
            if placement.missing_error().is_some() =>
        {
            return Err(Response::err(
                req.id,
                placement
                    .missing_error()
                    .expect("missing placement authority has a typed error"),
            ));
        }
        ClusterMutationRoute::ConsensusGraph
        | ClusterMutationRoute::ConsensusNative
        | ClusterMutationRoute::ConsensusFanout => {}
    }
    Ok(())
}

#[cfg(feature = "raft")]
pub async fn route_consensus_before_gateway(
    state: &Arc<RwLock<ServerState>>,
    req: Request,
    verified_context: &VerifiedRequestContext,
    identity_bootstrap: bool,
) -> Result<Request, Response> {
    if !medulla_kernel::kernel::dispatch_support::is_replicated_apply()
        && matches!(
            timed_read(state).await.placement_authority(),
            medulla_server_api::state::PlacementAuthorityKind::MultiRaft
        )
        && matches!(
            medulla_server_mutation::mutation::cluster_mutation_route(&req.method),
            medulla_server_mutation::mutation::ClusterMutationRoute::ConsensusNative
        )
    {
        if matches!(req.method, Method::FinanceRecordPut { .. }) {
            precheck_finance_record_put_on_leader(state, &req, verified_context).await?;
        }
        return Err(propose_native_mutation(
            state,
            &req.graph,
            req.id,
            verified_context,
            identity_bootstrap,
            req.method,
        )
        .await);
    }

    if !medulla_kernel::kernel::dispatch_support::is_replicated_apply()
        && matches!(
            medulla_server_mutation::mutation::cluster_mutation_route(&req.method),
            medulla_server_mutation::mutation::ClusterMutationRoute::ConsensusFanout
        )
    {
        return Err(match req.method {
            Method::MultiGraphBatchUpdate { batches_msgpack } => {
                multi_graph_batch_update(
                    state,
                    req.id,
                    req.agent_id.as_deref(),
                    verified_context,
                    &batches_msgpack,
                )
                .await
            }
            #[cfg(feature = "sparql-http")]
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
            _ => Response::err(req.id, "consensus fanout routing error"),
        });
    }

    Ok(req)
}

/// MEDULLA-FINANCE-PRIMITIVES-R004.3.2.9: on the leader of the put's group, run
/// every admission check before the proposal, so a refused put proposes nothing.
/// A follower skips the checks: its proposal is redirected to the leader (the
/// stale-route reply names it), and the leader checks against its applied state.
#[cfg(feature = "raft")]
async fn precheck_finance_record_put_on_leader(
    state: &Arc<RwLock<ServerState>>,
    req: &Request,
    verified_context: &VerifiedRequestContext,
) -> Result<(), Response> {
    use medulla_raft::raft::RaftStateExt;
    let (core, multi) = {
        let current = timed_read(state).await;
        (
            current
                .registry
                .get(&req.graph)
                .map(|entry| entry.core.clone()),
            current.multi_raft(),
        )
    };
    // No placement authority or no local group: the proposal answers the typed
    // placement refusal itself.
    let Some(multi) = multi else {
        return Ok(());
    };
    let Some(routed) = multi.handle_for_graph(&req.graph).await else {
        return Ok(());
    };
    if routed.handle.current_leader().await != Some(routed.handle.node_id) {
        return Ok(());
    }
    let Some(core) = core else {
        return Ok(());
    };
    let authority = medulla_server_api::carrier::CarrierAuthority::from_verified(verified_context)
        .map_err(|error| Response::err(req.id, error))?;
    medulla_h_graph::graph_ops::precheck_replicated_put(
        &core,
        authority.tenant_scope(),
        &req.method,
    )
    .map_err(|error| Response::err(req.id, error))
}
