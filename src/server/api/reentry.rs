//! Dispatch re-entry seam (`DispatchApi`): lower crates call back into the method router.
//!
//! The router, the Raft apply callbacks and the repository-enrichment consumer live in
//! `medulla-server`. Crates below it reach them through [`DispatchApi`], installed on the
//! `dispatch` slot of the [`super::registry::SeamRegistry`]. Every method returns a boxed future so
//! the trait stays object safe; the free functions below look the implementation up from the
//! state and map a missing installation to the caller's existing internal error.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::RwLock;

#[cfg(feature = "raft")]
use crate::protocol::Method;
#[cfg(all(feature = "raft", feature = "jobs"))]
use crate::protocol::ResultPayload;
use crate::protocol::{Request, Response};
use crate::server::auth::VerifiedRequestContext;
use crate::server::state::ServerState;

/// Boxed, sendable future returned by every [`DispatchApi`] method.
pub type SeamFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The caller/isolation-scoping fields shared by the graph-scoped read handlers'
/// `try_handle` entry points (`query::try_handle`, `rdf::try_handle`), bundled so
/// each stays under the clippy argument-count ceiling once the feature-gated `rls`
/// authority parameter is unified in.
#[cfg(any(
    feature = "query",
    feature = "cypher",
    feature = "graphql",
    feature = "rdf"
))]
pub(crate) struct TryHandleContext<'a> {
    pub(crate) req_id: u64,
    pub(crate) graph_name: &'a str,
    pub(crate) read_authority: Option<&'a crate::server::access::GraphReadAuthority>,
    pub(crate) caller: &'a str,
}

/// Phase of a replicated cross-shard transaction participant command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionParticipantPhase {
    /// Stage the participant plan.
    Prepare,
    /// Commit the staged plan.
    Commit,
    /// Abort and discard the staged plan.
    Abort,
}

/// Coordinator, participant and optional sealed plan of one replicated participant command.
#[cfg(feature = "raft")]
pub(crate) struct ReplicatedParticipantRef<'a> {
    pub(crate) coordinator_id: &'a str,
    pub(crate) participant_id: u64,
    pub(crate) plan: Option<&'a [u8]>,
}

/// Outcome of one bounded repository-enrichment outbox sweep.
#[cfg(feature = "blob")]
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum DrainOutcome {
    /// Nothing was pending.
    Idle,
    /// The graph's enrichment budget is parked.
    Parked,
    /// One or more pages were committed and the lease acknowledged.
    Acknowledged {
        /// Number of pages committed by the sweep.
        pages: u32,
    },
}

/// Per-page authority resolver handed to the enrichment drain.
#[cfg(all(feature = "ast", feature = "redb", feature = "blob"))]
pub(crate) type PageAuthorityFn = Box<
    dyn FnMut(
            String,
            String,
            String,
        ) -> SeamFuture<
            'static,
            Result<
                (
                    VerifiedRequestContext,
                    medulla_result_types::epistemic_operations::RequestContext,
                ),
                String,
            >,
        > + Send,
>;

/// Page submit callback handed to the enrichment drain.
#[cfg(all(feature = "ast", feature = "redb", feature = "blob"))]
pub(crate) type PageSubmitFn = Box<
    dyn FnMut(
            VerifiedRequestContext,
            medulla_agent_types::native_control::SubmitWorkItemsRequest,
        ) -> SeamFuture<'static, Result<(), String>>
        + Send,
>;

/// Budget-park callback handed to the enrichment drain.
#[cfg(all(feature = "ast", feature = "redb", feature = "blob"))]
pub(crate) type PageParkFn = Box<
    dyn FnMut(
            medulla_types::mutation_scope::MutationOutboxLease,
            medulla_agent_types::native_control::EnrichmentBudgetPark,
        ) -> SeamFuture<'static, Result<(), String>>
        + Send,
>;

/// Method-router re-entry reachable from crates below `medulla-server`.
#[allow(private_interfaces)]
pub trait DispatchApi: Send + Sync {
    /// Dispatch one request through the method router.
    fn dispatch<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        req: Request,
    ) -> SeamFuture<'a, Response>;

    /// Dispatch a request under an already verified request context.
    fn dispatch_verified_request<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        req: Request,
        context: VerifiedRequestContext,
    ) -> SeamFuture<'a, Response>;

    /// Dispatch an engine-owned local query under its fixed read-only service identity.
    #[cfg(any(feature = "federation-search", feature = "nl-query"))]
    fn dispatch_authenticated_local_query<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        req: Request,
    ) -> SeamFuture<'a, Response>;

    /// Dispatch a request on behalf of an authenticated auxiliary broker actor.
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
    ) -> SeamFuture<'a, Response>;

    /// Propose a native mutation through the graph's Raft group.
    #[cfg(feature = "raft")]
    fn propose_native_mutation<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        request_graph: &'a str,
        request_id: u64,
        verified_context: &'a VerifiedRequestContext,
        identity_bootstrap: bool,
        method: Method,
    ) -> SeamFuture<'a, Response>;

    /// Apply a committed bounded native command.
    #[cfg(feature = "raft")]
    fn apply_replicated_native<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        graph: String,
        request_id: u64,
        committed_at_ms: u64,
        authority: &'a crate::server::api::persistence::RaftMutationContext,
        method: Method,
    ) -> SeamFuture<'a, Response>;

    /// Apply a committed cross-shard transaction participant command.
    #[cfg(feature = "raft")]
    fn apply_replicated_transaction_participant<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        request_id: u64,
        committed_at_ms: u64,
        authority: &'a crate::server::api::persistence::RaftMutationContext,
        applying_group: medulla_storage::direct_state::GroupId,
        phase: TransactionParticipantPhase,
        participant: ReplicatedParticipantRef<'a>,
    ) -> SeamFuture<'a, Result<bool, String>>;

    /// Apply a committed transaction prepare.
    #[cfg(feature = "raft")]
    fn apply_replicated_transaction_prepare<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        request_id: u64,
        committed_at_ms: u64,
        authority: &'a crate::server::api::persistence::RaftMutationContext,
        txn_id: &'a str,
        idempotency_key: Option<&'a str>,
        expected_tenant: Option<&'a str>,
    ) -> SeamFuture<'a, Response>;

    /// Apply a committed transaction decision.
    #[cfg(feature = "raft")]
    fn apply_replicated_transaction_decision<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        committed_at_ms: u64,
        authority: &'a crate::server::api::persistence::RaftMutationContext,
        coordinator_id: &'a str,
        commit: bool,
    ) -> SeamFuture<'a, Result<bool, String>>;

    /// Apply a committed transaction finalize.
    #[cfg(feature = "raft")]
    fn apply_replicated_transaction_finalize<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        committed_at_ms: u64,
        authority: &'a crate::server::api::persistence::RaftMutationContext,
        coordinator_id: &'a str,
        commit: bool,
    ) -> SeamFuture<'a, Result<bool, String>>;

    /// Apply a committed job-publication commit.
    #[cfg(all(feature = "raft", feature = "jobs"))]
    fn apply_replicated_job_publication_commit<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        request_id: u64,
        committed_at_ms: u64,
        authority: &'a crate::server::api::persistence::RaftMutationContext,
        applying_group: medulla_storage::direct_state::GroupId,
        coordinator_id: &'a str,
        plan: &'a [u8],
    ) -> SeamFuture<'a, Result<crate::mutation_batch::MutationBatchCommit, String>>;

    /// Apply a committed job-publication finalize.
    #[cfg(all(feature = "raft", feature = "jobs"))]
    fn apply_replicated_job_publication_finalize<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        committed_at_ms: u64,
        authority: &'a crate::server::api::persistence::RaftMutationContext,
        coordinator_id: &'a str,
        receipt: &'a [u8],
    ) -> SeamFuture<'a, Result<ResultPayload, String>>;

    /// Make a cold graph resident before an operation that needs its registry.
    fn ensure_graph_resident<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        graph: &'a str,
    ) -> SeamFuture<'a, ()>;

    /// Create an internal global graph through the authoritative lifecycle path.
    fn ensure_internal_global_graph<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        req_id: u64,
        verified: &'a VerifiedRequestContext,
        graph_name: &'a str,
        idempotency_key: &'a str,
    ) -> SeamFuture<'a, Result<(), String>>;

    /// Read one registered MCP server under the verified caller's authority.
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
    >;

    /// Submit one funded repository page through the authenticated boundary.
    #[cfg(all(feature = "ast", feature = "redb", feature = "blob", feature = "raft"))]
    fn submit_repository_enrichment_page<'a>(
        &'a self,
        state: &'a Arc<RwLock<ServerState>>,
        verified: VerifiedRequestContext,
        request: medulla_agent_types::native_control::SubmitWorkItemsRequest,
    ) -> SeamFuture<'a, Result<(), String>>;

    /// Bind an internal park proposal to one held event and the latest durable budget row.
    #[cfg(all(feature = "ast", feature = "redb", feature = "blob", feature = "raft"))]
    fn plan_held_underfunded_park<'a>(
        &'a self,
        persistence: &'a dyn crate::server::api::persistence::PersistenceBackend,
        graph_fname: &'a str,
        lease: &'a medulla_types::mutation_scope::MutationOutboxLease,
        committed_at_ms: u64,
    ) -> SeamFuture<
        'a,
        Result<
            (
                crate::parser::enrichment_snapshot::EligibleSnapshot,
                medulla_agent_types::native_control::EnrichmentBudgetPark,
            ),
            String,
        >,
    >;

    /// Decode the server-owned snapshot of one pending enrichment outbox intent.
    #[cfg(all(
        feature = "ast",
        feature = "redb",
        feature = "blob",
        feature = "raft",
        feature = "security"
    ))]
    fn decode_pending_enrichment_intent(
        &self,
        intent: &medulla_types::mutation_scope::MutationOutboxIntent,
    ) -> Result<crate::parser::enrichment_snapshot::EligibleSnapshot, String>;

    /// Encode one complete immutable source as the canonical pending enrichment intent.
    #[cfg(all(
        feature = "ast",
        feature = "redb",
        feature = "blob",
        feature = "raft",
        feature = "security"
    ))]
    fn enrichment_intent_for_snapshot(
        &self,
        snapshot: crate::parser::enrichment_snapshot::EligibleSnapshot,
    ) -> Result<medulla_types::mutation_scope::MutationOutboxIntent, String>;

    /// One bounded durable repository-enrichment outbox sweep.
    #[cfg(all(feature = "ast", feature = "redb", feature = "blob"))]
    fn drain_repository_enrichment_once<'a>(
        &'a self,
        persistence: &'a dyn crate::server::api::persistence::PersistenceBackend,
        graph_fname: &'a str,
        store: Arc<dyn crate::server::blob::store::ChunkStore>,
        authority_for_page: PageAuthorityFn,
        submit: PageSubmitFn,
        park_source: PageParkFn,
    ) -> SeamFuture<'a, Result<DrainOutcome, String>>;
}

const NOT_INSTALLED: &str = "dispatch seam is not installed";

/// The installed [`DispatchApi`], or the internal error naming the missing installation.
pub async fn dispatch_api(
    state: &Arc<RwLock<ServerState>>,
) -> Result<Arc<dyn DispatchApi>, String> {
    state
        .read()
        .await
        .seams
        .dispatch
        .get()
        .cloned()
        .ok_or_else(|| NOT_INSTALLED.to_string())
}

/// Dispatch one request through the installed method router.
pub async fn dispatch(state: &Arc<RwLock<ServerState>>, req: Request) -> Response {
    let id = req.id;
    match dispatch_api(state).await {
        Ok(api) => api.dispatch(state, req).await,
        Err(error) => Response::err(id, error),
    }
}

/// Make a cold graph resident; a missing installation leaves the graph as it is.
pub(crate) async fn ensure_graph_resident(state: &Arc<RwLock<ServerState>>, graph: &str) {
    if let Ok(api) = dispatch_api(state).await {
        api.ensure_graph_resident(state, graph).await;
    }
}

/// Create an internal global graph through the authoritative lifecycle path.
pub(crate) async fn ensure_internal_global_graph(
    state: &Arc<RwLock<ServerState>>,
    req_id: u64,
    verified: &VerifiedRequestContext,
    graph_name: &str,
    idempotency_key: &str,
) -> Result<(), String> {
    dispatch_api(state)
        .await?
        .ensure_internal_global_graph(state, req_id, verified, graph_name, idempotency_key)
        .await
}

/// Read one registered MCP server under the verified caller's authority.
pub(crate) async fn verified_served_registration(
    state: &Arc<RwLock<ServerState>>,
    verified: &VerifiedRequestContext,
    server_name: &str,
) -> Result<
    (
        u64,
        medulla_types::contract::Digest256,
        medulla_protocol_types::result_contract::cluster::RegisteredServerView,
    ),
    String,
> {
    dispatch_api(state)
        .await?
        .verified_served_registration(state, verified, server_name)
        .await
}

/// Dispatch a request under an already verified request context.
pub(crate) async fn dispatch_verified_request(
    state: &Arc<RwLock<ServerState>>,
    req: Request,
    context: VerifiedRequestContext,
) -> Response {
    let id = req.id;
    match dispatch_api(state).await {
        Ok(api) => api.dispatch_verified_request(state, req, context).await,
        Err(error) => Response::err(id, error),
    }
}

/// Dispatch an engine-owned local query under its fixed read-only service identity.
#[cfg(any(feature = "federation-search", feature = "nl-query"))]
pub(crate) async fn dispatch_authenticated_local_query(
    state: &Arc<RwLock<ServerState>>,
    req: Request,
) -> Response {
    let id = req.id;
    match dispatch_api(state).await {
        Ok(api) => api.dispatch_authenticated_local_query(state, req).await,
        Err(error) => Response::err(id, error),
    }
}

/// Dispatch a request on behalf of an authenticated auxiliary broker actor.
#[cfg(any(
    feature = "amqp-wire",
    feature = "mqtt-wire",
    feature = "stomp-wire",
    feature = "mssql-wire",
    feature = "redis-wire",
))]
pub(crate) async fn dispatch_authenticated_broker_actor(
    state: &Arc<RwLock<ServerState>>,
    req: Request,
    actor_ref: &str,
) -> Response {
    let id = req.id;
    match dispatch_api(state).await {
        Ok(api) => {
            api.dispatch_authenticated_broker_actor(state, req, actor_ref)
                .await
        }
        Err(error) => Response::err(id, error),
    }
}

/// Propose a native mutation through the graph's Raft group.
#[cfg(feature = "raft")]
pub(crate) async fn propose_native_mutation(
    state: &Arc<RwLock<ServerState>>,
    request_graph: &str,
    request_id: u64,
    verified_context: &VerifiedRequestContext,
    identity_bootstrap: bool,
    method: Method,
) -> Response {
    match dispatch_api(state).await {
        Ok(api) => {
            api.propose_native_mutation(
                state,
                request_graph,
                request_id,
                verified_context,
                identity_bootstrap,
                method,
            )
            .await
        }
        Err(error) => Response::err(request_id, error),
    }
}

/// Apply a committed bounded native command.
#[cfg(feature = "raft")]
pub(crate) async fn apply_replicated_native(
    state: &Arc<RwLock<ServerState>>,
    graph: String,
    request_id: u64,
    committed_at_ms: u64,
    authority: &crate::server::api::persistence::RaftMutationContext,
    method: Method,
) -> Response {
    match dispatch_api(state).await {
        Ok(api) => {
            api.apply_replicated_native(
                state,
                graph,
                request_id,
                committed_at_ms,
                authority,
                method,
            )
            .await
        }
        Err(error) => Response::err(request_id, error),
    }
}

/// Apply a committed cross-shard transaction participant command.
#[cfg(feature = "raft")]
pub(crate) async fn apply_replicated_transaction_participant(
    state: &Arc<RwLock<ServerState>>,
    request_id: u64,
    committed_at_ms: u64,
    authority: &crate::server::api::persistence::RaftMutationContext,
    applying_group: medulla_storage::direct_state::GroupId,
    phase: TransactionParticipantPhase,
    participant: ReplicatedParticipantRef<'_>,
) -> Result<bool, String> {
    dispatch_api(state)
        .await?
        .apply_replicated_transaction_participant(
            state,
            request_id,
            committed_at_ms,
            authority,
            applying_group,
            phase,
            participant,
        )
        .await
}

/// Apply a committed transaction prepare.
#[cfg(feature = "raft")]
pub(crate) async fn apply_replicated_transaction_prepare(
    state: &Arc<RwLock<ServerState>>,
    request_id: u64,
    committed_at_ms: u64,
    authority: &crate::server::api::persistence::RaftMutationContext,
    txn_id: &str,
    idempotency_key: Option<&str>,
    expected_tenant: Option<&str>,
) -> Response {
    match dispatch_api(state).await {
        Ok(api) => {
            api.apply_replicated_transaction_prepare(
                state,
                request_id,
                committed_at_ms,
                authority,
                txn_id,
                idempotency_key,
                expected_tenant,
            )
            .await
        }
        Err(error) => Response::err(request_id, error),
    }
}

/// Apply a committed transaction decision.
#[cfg(feature = "raft")]
pub(crate) async fn apply_replicated_transaction_decision(
    state: &Arc<RwLock<ServerState>>,
    committed_at_ms: u64,
    authority: &crate::server::api::persistence::RaftMutationContext,
    coordinator_id: &str,
    commit: bool,
) -> Result<bool, String> {
    dispatch_api(state)
        .await?
        .apply_replicated_transaction_decision(
            state,
            committed_at_ms,
            authority,
            coordinator_id,
            commit,
        )
        .await
}

/// Apply a committed transaction finalize.
#[cfg(feature = "raft")]
pub(crate) async fn apply_replicated_transaction_finalize(
    state: &Arc<RwLock<ServerState>>,
    committed_at_ms: u64,
    authority: &crate::server::api::persistence::RaftMutationContext,
    coordinator_id: &str,
    commit: bool,
) -> Result<bool, String> {
    dispatch_api(state)
        .await?
        .apply_replicated_transaction_finalize(
            state,
            committed_at_ms,
            authority,
            coordinator_id,
            commit,
        )
        .await
}

/// Apply a committed job-publication commit.
#[cfg(all(feature = "raft", feature = "jobs"))]
pub(crate) async fn apply_replicated_job_publication_commit(
    state: &Arc<RwLock<ServerState>>,
    request_id: u64,
    committed_at_ms: u64,
    authority: &crate::server::api::persistence::RaftMutationContext,
    applying_group: medulla_storage::direct_state::GroupId,
    coordinator_id: &str,
    plan: &[u8],
) -> Result<crate::mutation_batch::MutationBatchCommit, String> {
    dispatch_api(state)
        .await?
        .apply_replicated_job_publication_commit(
            state,
            request_id,
            committed_at_ms,
            authority,
            applying_group,
            coordinator_id,
            plan,
        )
        .await
}

/// Apply a committed job-publication finalize.
#[cfg(all(feature = "raft", feature = "jobs"))]
pub(crate) async fn apply_replicated_job_publication_finalize(
    state: &Arc<RwLock<ServerState>>,
    committed_at_ms: u64,
    authority: &crate::server::api::persistence::RaftMutationContext,
    coordinator_id: &str,
    receipt: &[u8],
) -> Result<ResultPayload, String> {
    dispatch_api(state)
        .await?
        .apply_replicated_job_publication_finalize(
            state,
            committed_at_ms,
            authority,
            coordinator_id,
            receipt,
        )
        .await
}

/// Submit one funded repository page through the authenticated boundary.
#[cfg(all(feature = "ast", feature = "redb", feature = "blob", feature = "raft"))]
pub(crate) async fn submit_repository_enrichment_page(
    state: &Arc<RwLock<ServerState>>,
    verified: VerifiedRequestContext,
    request: medulla_agent_types::native_control::SubmitWorkItemsRequest,
) -> Result<(), String> {
    dispatch_api(state)
        .await?
        .submit_repository_enrichment_page(state, verified, request)
        .await
}

/// Bind an internal park proposal to one held event and the latest durable budget row.
#[cfg(all(feature = "ast", feature = "redb", feature = "blob", feature = "raft"))]
pub(crate) async fn plan_held_underfunded_park(
    state: &Arc<RwLock<ServerState>>,
    persistence: &dyn crate::server::api::persistence::PersistenceBackend,
    graph_fname: &str,
    lease: &medulla_types::mutation_scope::MutationOutboxLease,
    committed_at_ms: u64,
) -> Result<
    (
        crate::parser::enrichment_snapshot::EligibleSnapshot,
        medulla_agent_types::native_control::EnrichmentBudgetPark,
    ),
    String,
> {
    dispatch_api(state)
        .await?
        .plan_held_underfunded_park(persistence, graph_fname, lease, committed_at_ms)
        .await
}

/// Decode the server-owned snapshot of one pending enrichment outbox intent.
#[cfg(all(
    feature = "ast",
    feature = "redb",
    feature = "blob",
    feature = "raft",
    feature = "security"
))]
pub(crate) async fn decode_pending_enrichment_intent(
    state: &Arc<RwLock<ServerState>>,
    intent: &medulla_types::mutation_scope::MutationOutboxIntent,
) -> Result<crate::parser::enrichment_snapshot::EligibleSnapshot, String> {
    dispatch_api(state)
        .await?
        .decode_pending_enrichment_intent(intent)
}

/// Encode one complete immutable source as the canonical pending enrichment intent.
#[cfg(all(
    feature = "ast",
    feature = "redb",
    feature = "blob",
    feature = "raft",
    feature = "security"
))]
pub(crate) async fn enrichment_intent_for_snapshot(
    state: &Arc<RwLock<ServerState>>,
    snapshot: crate::parser::enrichment_snapshot::EligibleSnapshot,
) -> Result<medulla_types::mutation_scope::MutationOutboxIntent, String> {
    dispatch_api(state)
        .await?
        .enrichment_intent_for_snapshot(snapshot)
}

/// One bounded durable repository-enrichment outbox sweep.
#[cfg(all(feature = "ast", feature = "redb", feature = "blob"))]
pub(crate) async fn drain_repository_enrichment_once(
    state: &Arc<RwLock<ServerState>>,
    persistence: &dyn crate::server::api::persistence::PersistenceBackend,
    graph_fname: &str,
    store: Arc<dyn crate::server::blob::store::ChunkStore>,
    authority_for_page: PageAuthorityFn,
    submit: PageSubmitFn,
    park_source: PageParkFn,
) -> Result<DrainOutcome, String> {
    dispatch_api(state)
        .await?
        .drain_repository_enrichment_once(
            persistence,
            graph_fname,
            store,
            authority_for_page,
            submit,
            park_source,
        )
        .await
}
