//! Internal, authenticated park proposal for a held repository source event.
//!
//! The clustered worker remains disabled while funded page submission and
//! reactivation are incomplete. This producer is the bounded Raft park seam;
//! it cannot be invoked by a public Method or a caller-supplied snapshot.

#[cfg(feature = "raft")]
use medulla_raft::raft::RaftStateExt;
use std::sync::Arc;

use medulla_work_types::native_control::EnrichmentBudgetPark;
use medulla_mutation_types::mutation_scope::MutationOutboxLease;
use tokio::sync::RwLock;

use medulla_server_api::api::persistence::PersistenceBackend;
use medulla_server_api::auth::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

const SERVICE: &str = "repository-enrichment-v1";

fn verify_park_service(tenant_id: &str) -> Result<(), String> {
    let verified = VerifiedRequestContext::authenticated_fixed_service_actor(
        SERVICE,
        &["work:submit", "repository:enrichment:submit"],
    )?;
    if verified.tenant() != tenant_id || !verified.allows_action("repository:enrichment:submit") {
        return Err(
            "ACCESS_DENIED: repository enrichment park service authority is invalid".into(),
        );
    }
    Ok(())
}

/// A held event is re-read against its durable budget before proposing.
/// Release is local and follows the quorum-applied park; on refusal we still
/// release the exact lease so a one-hour claim TTL cannot strand the stream.
pub(crate) async fn propose_and_release_park(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    lease: &MutationOutboxLease,
    expected_park: &EnrichmentBudgetPark,
) -> Result<(), String> {
    let persistence = state
        .read()
        .await
        .persistence
        .clone()
        .ok_or("CONFLICT: repository enrichment persistence is unavailable")?;
    let proposed = propose_park(state, persistence.as_ref(), graph, lease, expected_park).await;
    let released = persistence.release_mutation_outbox(graph, lease).await;
    match (proposed, released) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(proposal), Err(release)) => Err(format!(
            "{proposal}; exact repository enrichment lease release also failed: {release}"
        )),
    }
}

async fn propose_park(
    state: &Arc<RwLock<ServerState>>,
    persistence: &dyn PersistenceBackend,
    graph: &str,
    lease: &MutationOutboxLease,
    expected_park: &EnrichmentBudgetPark,
) -> Result<(), String> {
    let (multi, graph_type) = {
        let current = state.read().await;
        let graph_type = current
            .registry
            .get(graph)
            .ok_or("CONFLICT: repository enrichment graph was retired")?
            .graph_type;
        let multi = current
            .multi_raft()
            .ok_or("CONFLICT: repository enrichment park requires multi-Raft placement")?;
        (multi, graph_type)
    };
    let routed = multi
        .handle_for_graph(graph)
        .await
        .ok_or("STALE_ROUTE: repository enrichment graph group is unavailable")?;
    if routed.handle.current_leader().await != Some(routed.handle.node_id) {
        return Err("STALE_ROUTE: repository enrichment park requires local group leader".into());
    }
    let now_ms = expected_park.parked_at_ms;
    if now_ms == 0 || now_ms > medulla_kernel::kernel::dispatch_support::authoritative_now_ms() {
        return Err("CONFLICT: repository enrichment park timestamp is invalid".into());
    }
    let (snapshot, park) = medulla_server_api::api::reentry::plan_held_underfunded_park(
        state,
        persistence,
        graph,
        lease,
        now_ms,
    )
    .await?;
    if &park != expected_park {
        return Err("CONFLICT: repository enrichment park changed before proposal".into());
    }
    verify_park_service(&snapshot.tenant_id)?;
    #[cfg(feature = "security")]
    {
        let mut current = state.write().await;
        let entry = current
            .registry
            .get(graph)
            .ok_or("CONFLICT: repository enrichment graph was retired")?;
        let owner = entry.owner.clone();
        let live_type = entry.graph_type;
        super::grant::ensure(&mut current.isolation, graph)?;
        if live_type != graph_type
            || !current.isolation.check_access(
                super::grant::SERVICE_ACTOR,
                graph,
                graph_type,
                owner.as_deref(),
                medulla_isolation::isolation::AccessLevel::Write,
            )
        {
            return Err("ACCESS_DENIED: repository enrichment park graph grant is absent".into());
        }
    }

    let coordinator = format!(
        "{}:{}:{}",
        park.source_envelope, park.next_index, park.snapshot_digest
    );
    let mut mutation = medulla_server_api::api::persistence::RaftMutationContext::internal(
        "raft-enrichment-park",
        graph,
        &coordinator,
        0,
        now_ms,
    );
    mutation.placement_epoch = routed.epoch;
    mutation.fencing_token = routed.placed.then_some(routed.group_id);
    let command = medulla_raft::raft::ReplicatedMutation::enrichment_park(
        &park,
        &state.read().await.auth_secret,
    )?;
    let response = routed
        .handle
        .client_write(medulla_raft::raft::RaftRequest {
            graph_fname: medulla_kernel::persist::sanitize(graph),
            graph_name: graph.to_string(),
            graph_type,
            command,
            committed_at_ms: now_ms,
            mutation,
        })
        .await?;
    if !response.applied || response.native_error.is_some() {
        return Err(response.native_error.unwrap_or_else(|| {
            "CONFLICT: replicated repository enrichment park was not applied".into()
        }));
    }
    if !matches!(
        response.native_result,
        Some(medulla_protocol_types::protocol::ResultPayload::Bool(true))
    ) {
        return Err("CONFLICT: replicated repository enrichment park has no receipt".into());
    }
    Ok(())
}
