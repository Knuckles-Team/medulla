//! `DispatchSeam`: the `medulla-server` implementation of the dispatch re-entry seam
//! ([`DispatchApi`]), installed on the `dispatch` slot by `seams_install` (MEDULLA-CONTRACT-R056.8).

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_protocol_types::protocol::Method;
use medulla_protocol_types::protocol::Request;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::api::reentry::DispatchApi;
use medulla_server_api::api::reentry::SeamFuture;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

/// Routes lower-crate re-entry calls to the real method router and its consensus callbacks.
pub struct DispatchSeam;

impl DispatchApi for DispatchSeam {
    fn dispatch<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        req: Request,
    ) -> SeamFuture<'a, Response> {
        Box::pin(super::request_boundary::dispatch(state, req))
    }

    fn dispatch_verified_request<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        req: Request,
        context: VerifiedRequestContext,
    ) -> SeamFuture<'a, Response> {
        Box::pin(super::request_boundary::dispatch_verified_request(
            state, req, context,
        ))
    }

    fn ensure_graph_resident<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        graph: &'a str,
    ) -> SeamFuture<'a, ()> {
        Box::pin(super::shared::ensure_graph_resident(state, graph))
    }

    fn ensure_internal_global_graph<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        req_id: u64,
        verified: &'a VerifiedRequestContext,
        graph_name: &'a str,
        idempotency_key: &'a str,
    ) -> SeamFuture<'a, Result<(), String>> {
        Box::pin(ensure_internal_global_graph(
            state,
            req_id,
            verified,
            graph_name,
            idempotency_key,
        ))
    }

    fn verified_served_registration<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        verified: &'a VerifiedRequestContext,
        server_name: &'a str,
    ) -> SeamFuture<
        'a,
        Result<
            (
                u64,
                medulla_types::contract::Digest256,
                medulla_protocol_types::result_contract::cluster::RegisteredServerView,
            ),
            String,
        >,
    > {
        Box::pin(super::consensus::verified_served_registration(
            state,
            verified,
            server_name,
        ))
    }

    #[cfg(any(feature = "federation-search", feature = "nl-query"))]
    fn dispatch_authenticated_local_query<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        req: Request,
    ) -> SeamFuture<'a, Response> {
        Box::pin(super::request_boundary::dispatch_authenticated_local_query(
            state, req,
        ))
    }

    #[cfg(any(
        feature = "amqp-wire",
        feature = "mqtt-wire",
        feature = "stomp-wire",
        feature = "mssql-wire",
        feature = "redis-wire",
    ))]
    fn dispatch_authenticated_broker_actor<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        req: Request,
        actor_ref: &'a str,
    ) -> SeamFuture<'a, Response> {
        Box::pin(
            super::request_boundary::dispatch_authenticated_broker_actor(state, req, actor_ref),
        )
    }

    #[cfg(feature = "raft")]
    fn propose_native_mutation<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        request_graph: &'a str,
        request_id: u64,
        verified_context: &'a VerifiedRequestContext,
        identity_bootstrap: bool,
        method: Method,
    ) -> SeamFuture<'a, Response> {
        Box::pin(super::consensus::propose_native_mutation(
            state,
            request_graph,
            request_id,
            verified_context,
            identity_bootstrap,
            method,
        ))
    }

    #[cfg(feature = "raft")]
    fn apply_replicated_native<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        graph: String,
        request_id: u64,
        committed_at_ms: u64,
        authority: &'a medulla_server_api::api::persistence::RaftMutationContext,
        method: Method,
    ) -> SeamFuture<'a, Response> {
        Box::pin(super::consensus::apply_replicated_native(
            state,
            graph,
            request_id,
            committed_at_ms,
            authority,
            method,
        ))
    }

    #[cfg(feature = "raft")]
    fn apply_replicated_transaction_participant<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        request_id: u64,
        committed_at_ms: u64,
        authority: &'a medulla_server_api::api::persistence::RaftMutationContext,
        applying_group: medulla_direct_state::direct_state::GroupId,
        phase: medulla_server_api::api::reentry::TransactionParticipantPhase,
        participant: medulla_server_api::api::reentry::ReplicatedParticipantRef<'a>,
    ) -> SeamFuture<'a, Result<bool, String>> {
        Box::pin(super::consensus::apply_replicated_transaction_participant(
            state,
            request_id,
            committed_at_ms,
            authority,
            applying_group,
            phase,
            participant,
        ))
    }

    #[cfg(feature = "raft")]
    fn apply_replicated_transaction_prepare<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        request_id: u64,
        committed_at_ms: u64,
        authority: &'a medulla_server_api::api::persistence::RaftMutationContext,
        txn_id: &'a str,
        idempotency_key: Option<&'a str>,
        expected_tenant: Option<&'a str>,
    ) -> SeamFuture<'a, Response> {
        Box::pin(super::consensus::apply_replicated_transaction_prepare(
            state,
            request_id,
            committed_at_ms,
            authority,
            txn_id,
            idempotency_key,
            expected_tenant,
        ))
    }

    #[cfg(feature = "raft")]
    fn apply_replicated_transaction_decision<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        committed_at_ms: u64,
        authority: &'a medulla_server_api::api::persistence::RaftMutationContext,
        coordinator_id: &'a str,
        commit: bool,
    ) -> SeamFuture<'a, Result<bool, String>> {
        Box::pin(super::consensus::apply_replicated_transaction_decision(
            state,
            committed_at_ms,
            authority,
            coordinator_id,
            commit,
        ))
    }

    #[cfg(feature = "raft")]
    fn apply_replicated_transaction_finalize<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        committed_at_ms: u64,
        authority: &'a medulla_server_api::api::persistence::RaftMutationContext,
        coordinator_id: &'a str,
        commit: bool,
    ) -> SeamFuture<'a, Result<bool, String>> {
        Box::pin(super::consensus::apply_replicated_transaction_finalize(
            state,
            committed_at_ms,
            authority,
            coordinator_id,
            commit,
        ))
    }

    #[cfg(all(feature = "raft", feature = "jobs"))]
    fn apply_replicated_job_publication_commit<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        request_id: u64,
        committed_at_ms: u64,
        authority: &'a medulla_server_api::api::persistence::RaftMutationContext,
        applying_group: medulla_direct_state::direct_state::GroupId,
        coordinator_id: &'a str,
        plan: &'a [u8],
    ) -> SeamFuture<'a, Result<medulla_protocol_types::mutation_batch::MutationBatchCommit, String>> {
        Box::pin(super::consensus::apply_replicated_job_publication_commit(
            state,
            request_id,
            committed_at_ms,
            authority,
            applying_group,
            coordinator_id,
            plan,
        ))
    }

    #[cfg(all(feature = "raft", feature = "jobs"))]
    fn apply_replicated_job_publication_finalize<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        committed_at_ms: u64,
        authority: &'a medulla_server_api::api::persistence::RaftMutationContext,
        coordinator_id: &'a str,
        receipt: &'a [u8],
    ) -> SeamFuture<'a, Result<medulla_protocol_types::protocol::ResultPayload, String>> {
        Box::pin(super::consensus::apply_replicated_job_publication_finalize(
            state,
            committed_at_ms,
            authority,
            coordinator_id,
            receipt,
        ))
    }

    #[cfg(all(feature = "ast", feature = "redb", feature = "blob", feature = "raft"))]
    fn submit_repository_enrichment_page<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        verified: VerifiedRequestContext,
        request: medulla_work_types::native_control::SubmitWorkItemsRequest,
    ) -> SeamFuture<'a, Result<(), String>> {
        Box::pin(submit_repository_enrichment_page(
            state, verified, request,
        ))
    }

    #[cfg(all(feature = "ast", feature = "redb", feature = "blob", feature = "raft"))]
    fn plan_held_underfunded_park<'a>(
        &'a self,
        persistence: &'a dyn medulla_server_api::api::persistence::PersistenceBackend,
        graph_fname: &'a str,
        lease: &'a medulla_mutation_types::mutation_scope::MutationOutboxLease,
        committed_at_ms: u64,
    ) -> SeamFuture<
        'a,
        Result<
            (
                medulla_compute::parser::enrichment_snapshot::EligibleSnapshot,
                medulla_work_types::native_control::EnrichmentBudgetPark,
            ),
            String,
        >,
    > {
        Box::pin(super::graph_pipeline::plan_held_underfunded_park(
            persistence,
            graph_fname,
            lease,
            committed_at_ms,
        ))
    }

    #[cfg(all(
        feature = "ast",
        feature = "redb",
        feature = "blob",
        feature = "raft",
        feature = "security"
    ))]
    fn decode_pending_enrichment_intent(
        &self,
        intent: &medulla_mutation_types::mutation_scope::MutationOutboxIntent,
    ) -> Result<medulla_compute::parser::enrichment_snapshot::EligibleSnapshot, String> {
        super::graph_pipeline::decode_pending_enrichment_intent(intent)
    }

    #[cfg(all(
        feature = "ast",
        feature = "redb",
        feature = "blob",
        feature = "raft",
        feature = "security"
    ))]
    fn enrichment_intent_for_snapshot(
        &self,
        snapshot: medulla_compute::parser::enrichment_snapshot::EligibleSnapshot,
    ) -> Result<medulla_mutation_types::mutation_scope::MutationOutboxIntent, String> {
        super::graph_pipeline::enrichment_intent_for_snapshot(snapshot)
    }

    #[cfg(all(feature = "ast", feature = "redb", feature = "blob"))]
    fn drain_repository_enrichment_once<'a>(
        &'a self,
        persistence: &'a dyn medulla_server_api::api::persistence::PersistenceBackend,
        graph_fname: &'a str,
        store: medulla_server_api::state::ErasedHandle,
        mut authority_for_page: medulla_server_api::api::reentry::PageAuthorityFn,
        mut submit: medulla_server_api::api::reentry::PageSubmitFn,
        mut park_source: medulla_server_api::api::reentry::PageParkFn,
    ) -> SeamFuture<'a, Result<medulla_server_api::api::reentry::DrainOutcome, String>> {
        let Some(store) = store
            .downcast_ref::<Arc<dyn medulla_server_blob::blob::store::ChunkStore>>()
            .cloned()
        else {
            return Box::pin(async {
                Err("CONFLICT: repository enrichment CAS handle has the wrong type".to_string())
            });
        };
        Box::pin(super::graph_pipeline::drain_repository_enrichment_once(
            persistence,
            graph_fname,
            store,
            move |tenant, graph, page_key| authority_for_page(tenant, graph, page_key),
            move |verified, request| submit(verified, request),
            move |lease, park| park_source(lease, park),
        ))
    }
}

/// Submit one funded repository page through the same authenticated boundary
/// as an external native method. In a cluster this routes through the graph's
/// Raft group before the WorkItem+budget kernel runs on each replica.
#[cfg(all(feature = "ast", feature = "redb", feature = "blob", feature = "raft"))]
pub async fn submit_repository_enrichment_page(
    state: &Arc<RwLock<ServerState>>,
    verified: VerifiedRequestContext,
    request: medulla_work_types::native_control::SubmitWorkItemsRequest,
) -> Result<(), String> {
    let debit = request
        .enrichment_budget
        .as_ref()
        .ok_or("CONFLICT: repository enrichment page lacks budget debit")?;
    if verified.tenant() != request.context.tenant_id
        || verified.idempotency_key() != debit.page_key
        || request.idempotency_key != debit.page_key
        || !verified.allows_action("work:submit")
        || !verified.allows_action("repository:enrichment:submit")
    {
        return Err("ACCESS_DENIED: repository enrichment page authority is invalid".into());
    }
    let response = super::request_boundary::dispatch_with_context(
        state,
        Request {
            id: 0,
            graph: request.context.graph.clone(),
            auth_token: String::new(),
            agent_id: None,
            method: Method::SubmitWorkItems { request },
        },
        Some(verified),
    )
    .await;
    if let Some(error) = response.error {
        return Err(match response.error_detail {
            Some(detail) => format!("{error}: {detail}"),
            None => error,
        });
    }
    if response.result.is_none() {
        return Err("CONFLICT: repository enrichment page has no native receipt".into());
    }
    Ok(())
}

/// Create one engine-owned global graph through the same durable lifecycle as
/// the public `CreateGraph` method.
///
/// Internal projection workers use this only for reserved graph names after
/// validating their own closed namespace.  Keeping the lifecycle entry here
/// avoids direct `GraphRegistry::create_graph` publication, which would bypass
/// the authoritative registration batch and restart identity.
pub async fn ensure_internal_global_graph(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    graph_name: &str,
    idempotency_key: &str,
) -> Result<(), String> {
    if state.read().await.registry.exists(graph_name) {
        return Ok(());
    }
    let create_identity = medulla_types::contract::Digest256::framed(
        b"eg/internal-graph-lifecycle/v1",
        &[idempotency_key.as_bytes(), graph_name.as_bytes()],
    )?;
    let request = super::router::GraphLifecycleRequest::verified(
        req_id,
        Some(verified.agent_id().to_string()),
        verified,
        format!("internal-create:{}", create_identity.to_hex()),
    )?;
    let response = super::router::create_graph(
        state,
        request,
        graph_name.to_string(),
        medulla_protocol_types::protocol::GraphType::Global,
    )
    .await;
    if let Some(error) = response.error {
        // A concurrent creator may have won after the existence probe.  The
        // graph's durable lifecycle is sufficient; never interpret any other
        // create failure as success.
        if state.read().await.registry.exists(graph_name) {
            return Ok(());
        }
        return Err(error);
    }
    if state.read().await.registry.exists(graph_name) {
        Ok(())
    } else {
        Err(format!(
            "internal graph lifecycle acknowledged without publishing '{graph_name}'"
        ))
    }
}
