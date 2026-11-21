//! The connector-pack projection worker: the first Agent Library outbox
//! consumer (PB1, pack design §7.2, X10).
//!
//! Every committed import publishes one `medulla.connector-pack.import.v1` row on
//! its tenant's Agent Library outbox. This worker claims those rows and, for
//! each, projects its connector's CURRENT head into the `pack__<connector>`
//! graph and flips the head visible (the readiness gate) -- never the record
//! it leased, so a stale worker can never roll a newer projection back, and a
//! re-delivered row is idempotent.
//!
//! * A projected (or already visible) head is acknowledged.
//! * A failed projection is recorded on the head as `failed` (shown by
//!   `Status`, re-driven by the next import or by `ConnectorPack.Reproject`)
//!   and the row is REJECTED at once rather than burning its bounded retries,
//!   so it never holds the stream (X10-R2).
//! * A row that is not a pack import record is rejected as an invalid event.
//!
//! The worker runs under a fixed engine service identity; it is started once,
//! after catalog recovery, and polls the deployment tenant's outbox.
//!
//! Native component revisions are drained by a second durable consumer on the
//! component revision topic ([`worker::native`], MEDULLA-TYPED-PACKS-R002.12),
//! started beside it and sharing its claim, settle, tenant and service-identity
//! helpers.

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_server_api::state::ServerState;

/// Start the singleton projection worker. A build without the Agent Library,
/// the Blob CAS or the SHACL projection has nothing to project.
pub fn spawn(state: Arc<RwLock<ServerState>>) {
    #[cfg(all(feature = "redb", feature = "blob", feature = "shacl"))]
    {
        tokio::spawn(worker::native::run(Arc::clone(&state)));
        tokio::spawn(worker::run(state));
    }
    #[cfg(not(all(feature = "redb", feature = "blob", feature = "shacl")))]
    drop(state);
}

#[cfg(all(
    feature = "redb",
    feature = "blob",
    feature = "shacl",
    feature = "security"
))]
pub(crate) mod blueprint;
mod grant;

#[cfg(all(feature = "redb", feature = "blob", feature = "shacl"))]
pub mod worker {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::RwLock;

    use medulla_catalog_types::{
        connector_pack::PackImportRecord, connector_pack::CONNECTOR_PACK_IMPORT_TOPIC,
    };
    use medulla_mutation_types::mutation_scope::MutationOutboxLease;
    use medulla_transaction::{OutboxClaimBudget, OutboxClaimOutcome, OutboxRejectReason};

    use medulla_persistence_records::agent_library::AgentLibraryStore;
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_server_api::outbox_operator::OutboxWrite;
    use medulla_server_api::state::ServerState;

    pub(crate) mod native;

    /// The label a failed blocking store task names.
    const BLOCKING_TASK: &str = "connector pack projection";

    /// The worker's one durable consumer name on every tenant's outbox.
    ///
    /// A durable subscription binds a consumer to ONE topic for good: moving
    /// it would move the consumer's whole ordered stream while its cursor
    /// still names positions in the old one, so the outbox refuses it. The
    /// import topic was renamed with the engine (`eg.connector-pack.import.v1`
    /// became `medulla.connector-pack.import.v1`); a store written before the
    /// rename still holds the previous consumer, subscribed to the retired
    /// topic, under the name `connector-pack-projection`. Reusing that name
    /// made every sweep fail on the refused subscription, so no import was
    /// ever projected and the catalog stayed dark. The consumer on the current
    /// topic is therefore a distinct identity; the retired one is never read
    /// again and its history is not replayed.
    pub const CONSUMER: &str = "connector-pack-projection-v2";
    /// Rows one sweep claims.
    const CLAIM_LIMIT: u32 = 16;
    /// How long a claimed row stays leased: a projection reads bodies and
    /// commits a graph, so the lease is generous.
    const LEASE_MS: u64 = 120_000;
    /// Idle back-off between sweeps.
    const IDLE: Duration = Duration::from_millis(500);

    /// One connector's claimed rows: claim index and its decoded import record.
    type ConnectorClaims = (Option<String>, Vec<(usize, Option<PackImportRecord>)>);

    pub(super) async fn run(state: Arc<RwLock<ServerState>>) {
        let subscribed = AtomicBool::new(false);
        loop {
            match sweep(&state, &subscribed).await {
                Ok(true) => {}
                Ok(false) => tokio::time::sleep(IDLE).await,
                Err(error) => {
                    tracing::warn!(
                        code = "CONNECTOR_PACK_PROJECTION_SWEEP_FAILED",
                        %error,
                        "connector pack projection sweep failed"
                    );
                    tokio::time::sleep(IDLE).await;
                }
            }
        }
    }

    /// One sweep over the deployment tenant's import stream. Returns whether
    /// it did any work, so an idle worker backs off.
    pub async fn sweep(
        state: &Arc<RwLock<ServerState>>,
        subscribed: &AtomicBool,
    ) -> Result<bool, String> {
        sweep_at(state, subscribed, RetryAt::now()).await
    }

    /// One sweep judged by `retry_at`: the clock the retry backoff is measured
    /// against and the knobs it follows. Production passes [`RetryAt::now`]; a
    /// test passes the time it has advanced to, so the backoff never depends on
    /// the wall clock.
    pub(crate) async fn sweep_at(
        state: &Arc<RwLock<ServerState>>,
        subscribed: &AtomicBool,
        retry_at: RetryAt,
    ) -> Result<bool, String> {
        let tenant = deployment_tenant(state).await?;
        let store = open_agent_library(state).await?;
        if !subscribed.load(Ordering::Acquire) {
            let (store, tenant) = (Arc::clone(&store), tenant.clone());
            medulla_server_api::api::dispatch_support::blocking_task(BLOCKING_TASK, move || {
                store.outbox_subscribe(&tenant, CONSUMER, CONNECTOR_PACK_IMPORT_TOPIC)
            })
            .await?;
            subscribed.store(true, Ordering::Release);
        }
        let drain_started = std::time::Instant::now();
        let outcome = claim(&store, &tenant, CONSUMER).await?;
        report(&outcome, CONSUMER, CONNECTOR_PACK_IMPORT_TOPIC);
        let claimed = outcome.claims.len();
        let mut progressed = claimed > 0;
        let mut committed_graphs = 0usize;
        let outcomes = project_claims(state, &store, &tenant, &outcome.claims, retry_at).await;
        // The stream is strictly ordered: leases resolve in claim order, after
        // the projections (which ran concurrently across connectors) decided them.
        for (lease, (reason, projected)) in outcome.claims.into_iter().zip(outcomes) {
            // MEDULLA-TYPED-PACKS-R096.2: one row's failure never ends the sweep;
            // the rows after it keep flowing.
            match resolve(
            &store,
            &tenant,
            lease,
            reason,
            CONSUMER,
            CONNECTOR_PACK_IMPORT_TOPIC,
        )
        .await
        {
                Ok(()) if projected => committed_graphs += 1,
                Ok(()) => {}
                Err(error) => tracing::warn!(
                    code = "CONNECTOR_PACK_PROJECTION_SETTLE_FAILED",
                    %error,
                    "connector pack projection: one row could not be resolved; the sweep continues"
                ),
            }
        }
        progressed |= retry_due(state, &store, &tenant, retry_at).await;
        if progressed {
            // Bounded cadence: one INFO per non-empty sweep (a sweep claims at
            // most CLAIM_LIMIT rows), so a multi-minute drain is never silent
            // in the log (MEDULLA-DURABLE-KERNEL-R006.4).
            tracing::info!(
                rows_claimed = claimed,
                committed_graphs,
                elapsed_ms = drain_started.elapsed().as_millis() as u64,
                "connector pack projection: drained one sweep of the import outbox"
            );
        }
        Ok(progressed)
    }

    /// The deployment tenant whose outbox the projection consumers poll.
    pub(super) async fn deployment_tenant(
        state: &Arc<RwLock<ServerState>>,
    ) -> Result<String, String> {
        let (anonymous, server) = {
            let state = state.read().await;
            (state.local_anonymous, state.context.clone())
        };
        Ok(if anonymous {
            // Deployment authority owns the namespace; no signer/OIDC policy
            // exists (or is consulted) for an isolated anonymous store.
            VerifiedRequestContext::anonymous_local(None)
                .tenant()
                .to_string()
        } else {
            server.request_policy()?.expected_tenant.clone()
        })
    }

    /// Open/ensure the Agent Library store OUTSIDE the global write lock
    /// (MEDULLA-DURABLE-KERNEL-R006.4). The open is slow on a large store, and pre-G9
    /// the first sweep held the `ServerState` write lock across it — queueing
    /// the pre-listener critical path (matview reload, listener bind) behind
    /// the whole drain. A read lock adopts an already-ensured handle; the open
    /// itself runs unlocked, and only the publish takes a short write. The
    /// shared open slot also serializes it with request-side initialization.
    pub(super) async fn open_agent_library(
        state: &Arc<RwLock<ServerState>>,
    ) -> Result<Arc<AgentLibraryStore>, String> {
        medulla_persistence_records::agent_library::shared_agent_library(state).await
    }

    pub(super) async fn claim(
        store: &Arc<AgentLibraryStore>,
        tenant: &str,
        consumer: &'static str,
    ) -> Result<OutboxClaimOutcome, String> {
        let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        let mut budget = OutboxClaimBudget::new(CLAIM_LIMIT, LEASE_MS, now_ms)?;
        let (store, tenant) = (Arc::clone(store), tenant.to_string());
        medulla_server_api::api::dispatch_support::blocking_task(BLOCKING_TASK, move || {
            store.outbox_claim(&tenant, consumer, &mut budget)
        })
        .await
    }

    pub(super) fn report(outcome: &OutboxClaimOutcome, consumer: &str, topic: &str) {
        if !outcome.dead_lettered.is_empty() {
            medulla_kernel::metrics::process().outbox_dead_lettered(
                consumer,
                topic,
                "exhausted",
                outcome.dead_lettered.len() as u64,
            );
        }
        let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        let head = outcome.claims.first();
        medulla_kernel::metrics::process().set_outbox_head(
            consumer,
            topic,
            head.map_or(0, |lease| now_ms.saturating_sub(lease.record.created_at_ms)),
            head.map_or(0, |lease| lease.attempt),
        );
    }

    /// Decide every claimed row: one task per connector, so independent
    /// connectors project concurrently while one connector's rows stay in
    /// claim order. Each projection re-reasons the composed core corpus
    /// (seconds of blocking-pool CPU), and one sweep claims up to
    /// CLAIM_LIMIT rows: projected one after another, a burst of imports left
    /// the last head dark for minutes. Returns the outcomes in claim order.
    async fn project_claims(
        state: &Arc<RwLock<ServerState>>,
        store: &Arc<AgentLibraryStore>,
        tenant: &str,
        claims: &[MutationOutboxLease],
        retry_at: RetryAt,
    ) -> Vec<(Option<OutboxRejectReason>, bool)> {
        let mut groups: Vec<ConnectorClaims> = Vec::new();
        for (index, lease) in claims.iter().enumerate() {
            let record = medulla_storage::decode_ledger_record::<PackImportRecord>(
                &lease.record.intent.payload,
            )
            .ok();
            let key = record
                .as_ref()
                .map(|record| record.connector.as_str().to_string());
            match groups
                .iter_mut()
                .find(|(group, _)| key.is_some() && *group == key)
            {
                Some((_, rows)) => rows.push((index, record)),
                None => groups.push((key, vec![(index, record)])),
            }
        }
        let mut tasks = tokio::task::JoinSet::new();
        for (_, rows) in groups {
            let (state, store, tenant) = (Arc::clone(state), Arc::clone(store), tenant.to_string());
            tasks.spawn(async move {
                let mut decided = Vec::with_capacity(rows.len());
                for (index, record) in rows {
                    decided.push((
                        index,
                        decide(&state, &store, &tenant, record, retry_at).await,
                    ));
                }
                decided
            });
        }
        let mut outcomes = vec![(Some(OutboxRejectReason::ProjectionFailed), false); claims.len()];
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(decided) => {
                    for (index, outcome) in decided {
                        outcomes[index] = outcome;
                    }
                }
                // A panicked projection task: its rows keep the default
                // outcome, rejected as a failed projection (dead-lettered and
                // counted), so the ordered stream still moves past them.
                Err(error) => tracing::warn!(
                    code = "CONNECTOR_PACK_PROJECTION_TASK_FAILED",
                    %error,
                    "connector pack projection task failed"
                ),
            }
        }
        outcomes
    }

    /// Project one leased import's connector: no reason on success, a reject
    /// reason on a non-retryable failure or an unreadable row, and whether the
    /// projection's graph commit ran (the sweep's drain log counts it —
    /// MEDULLA-DURABLE-KERNEL-R006.4).
    async fn decide(
        state: &Arc<RwLock<ServerState>>,
        store: &Arc<AgentLibraryStore>,
        tenant: &str,
        record: Option<PackImportRecord>,
        retry_at: RetryAt,
    ) -> (Option<OutboxRejectReason>, bool) {
        match record {
            None => (Some(OutboxRejectReason::InvalidEvent), false),
            Some(record) => match project(state, store, tenant, &record.connector).await {
                Ok(projected) => (None, projected),
                Err(error) => {
                    // MEDULLA-TYPED-PACKS-R096.2: the row is resolved either way so
                    // the ordered stream keeps flowing; the failure moves into the
                    // pack's own retry state (terminal at once when not retryable).
                    let retryable =
                        crate::admin::connector_pack::reproject::is_retryable_projection_error(
                            &error,
                        );
                    tracing::warn!(
                        code = "CONNECTOR_PACK_PROJECTION_FAILED",
                        connector = record.connector.as_str(),
                        retryable,
                        %error,
                        "connector pack projection failed; the pack's retry state carries it"
                    );
                    if let Err(failure) =
                        schedule_retry(state, store, tenant, &record.connector, &error, retry_at)
                            .await
                    {
                        tracing::warn!(
                            code = "CONNECTOR_PACK_PROJECTION_RETRY_UNRECORDED",
                            connector = record.connector.as_str(),
                            %failure,
                            "connector pack projection retry state was not recorded"
                        );
                    }
                    let reason = (!retryable).then_some(OutboxRejectReason::ProjectionFailed);
                    (reason, false)
                }
            },
        }
    }

    pub(super) async fn resolve(
        store: &Arc<AgentLibraryStore>,
        tenant: &str,
        lease: MutationOutboxLease,
        reason: Option<OutboxRejectReason>,
        consumer: &str,
        topic: &str,
    ) -> Result<(), String> {
        let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        let (store, tenant) = (Arc::clone(store), tenant.to_string());
        let Some(reason) = reason else {
            return medulla_server_api::api::dispatch_support::blocking_task(
                BLOCKING_TASK,
                move || store.outbox_ack(&tenant, &lease, now_ms),
            )
            .await;
        };
        medulla_kernel::metrics::process().outbox_dead_lettered(
            consumer,
            topic,
            reject_cause(reason),
            1,
        );
        let write = OutboxWrite::Reject {
            lease: Box::new(lease),
            reason,
            now_ms,
        };
        medulla_server_api::api::dispatch_support::blocking_task(BLOCKING_TASK, move || {
            store.outbox_write(&tenant, write).map(drop)
        })
        .await
    }

    fn reject_cause(reason: OutboxRejectReason) -> &'static str {
        match reason {
            OutboxRejectReason::InvalidEvent => "invalid_event",
            OutboxRejectReason::DomainRefused => "domain_refused",
            OutboxRejectReason::ProjectionFailed => "projection_failed",
            OutboxRejectReason::Operator => "operator",
        }
    }

    /// Project the connector's CURRENT head, unless it is already visible.
    /// Returns whether the projection's graph commit ran (a dark head that
    /// projects for the first time commits; an already-visible head does not).
    pub(crate) async fn project(
        state: &Arc<RwLock<ServerState>>,
        store: &Arc<AgentLibraryStore>,
        tenant: &str,
        connector: &medulla_types::contract::ResourceId,
    ) -> Result<bool, String> {
        let status = store.connector_pack_status(tenant, connector)?;
        let Some(head) = status.head else {
            return Ok(false);
        };
        if head.visible_record_id.as_deref() == Some(head.record_id.as_str()) {
            return Ok(false);
        }
        let plan = store.prepare_connector_pack_projection(tenant, connector)?;
        let (anonymous, verified, context) =
            write_context(state, store, tenant, &plan, None).await?;
        if !anonymous {
            if let Err(error) = grant_projection_access(state, tenant, connector).await {
                let error = format!("PACK_PROJECTION_GRANT_FAILED: {error}");
                return Err(
                    crate::admin::connector_pack::reproject::record_projection_failure(
                        store, &context, &plan, &error,
                    ),
                );
            }
        }
        crate::admin::connector_pack::reproject::project_head(
            state, 0, &verified, store, context, &plan,
        )
        .await
        .map(|_| true)
    }

    /// Let the projection actor write exactly this row's pack graph and the
    /// commons graph that holds the glossary terms. A build
    /// without the RBAC policy decides graph access by graph type alone, so
    /// there is nothing to provision there.
    async fn grant_projection_access(
        state: &Arc<RwLock<ServerState>>,
        tenant: &str,
        connector: &medulla_types::contract::ResourceId,
    ) -> Result<(), String> {
        #[cfg(feature = "security")]
        {
            let graph = medulla_catalog_types::connector_pack::ids::pack_projection_graph_name(
                tenant, connector,
            )?;
            let mut guard = state.write().await;
            super::grant::ensure(&mut guard.isolation, &graph)?;
            // The glossary terms a pack defines join the tenant's commons graph
            // after the projection commits (MEDULLA-FEDERATED-QUERY-R058.9); the
            // actor writes that one graph as well, through the same gateway.
            super::grant::ensure(&mut guard.isolation, super::grant::COMMONS_GRAPH)
        }
        #[cfg(not(feature = "security"))]
        {
            let _ = (state, tenant, connector);
            Ok(())
        }
    }

    /// The verified identity and Agent Library context one projection write of
    /// `plan`'s head commits under; each call mints its own attempt nonce.
    async fn write_context(
        state: &Arc<RwLock<ServerState>>,
        store: &Arc<AgentLibraryStore>,
        tenant: &str,
        plan: &medulla_persistence_records::connector_pack::projection::ConnectorPackProjectionPlan,
        step: Option<&str>,
    ) -> Result<
        (
            bool,
            VerifiedRequestContext,
            medulla_agent_types::agent_library::AgentLibraryMutationContext,
        ),
        String,
    > {
        let (anonymous, server) = {
            let state = state.read().await;
            (state.local_anonymous, state.context.clone())
        };
        // The operation key names the record and, for a step that writes a
        // different mutation of the same record (a retry attempt), the step: one
        // key never stands for two different mutations.
        let key_tail = match step {
            Some(step) => format!("{step}:{}", plan.record_id),
            None => plan.record_id.clone(),
        };
        let verified = if anonymous {
            VerifiedRequestContext::anonymous_local(Some(format!("{CONSUMER}:{key_tail}")))
        } else {
            service_context(&server, &key_tail)?
        };
        let context =
            medulla_persistence_records::agent_library::context::bind_agent_library_context(
                store,
                0,
                &verified,
                unbound_context(tenant),
                "connector-pack:reproject",
                true,
            )?;
        Ok((anonymous, verified, context))
    }

    /// When a retry decision is taken and under which knobs
    /// (MEDULLA-TYPED-PACKS-R096.2): production reads the clock and the
    /// registry; tests pass a later clock to make retries due.
    #[derive(Clone, Copy)]
    pub(crate) struct RetryAt {
        pub(crate) now_ms: u64,
        pub(crate) knobs: medulla_persistence_records::connector_pack::projection_retry::RetryKnobs,
    }

    impl RetryAt {
        pub(crate) fn now() -> Self {
            let knobs = medulla_server_api::knob_config::knobs();
            Self {
                now_ms: medulla_kernel::kernel::dispatch_support::authoritative_now_ms(),
                knobs: medulla_persistence_records::connector_pack::projection_retry::RetryKnobs {
                    max_attempts: u32::try_from(knobs.connector_pack_projection_max_attempts)
                        .unwrap_or(u32::MAX),
                    backoff_ms: knobs.connector_pack_projection_backoff_ms,
                },
            }
        }
    }

    /// Move one failed projection of `connector`'s current head into its next
    /// retry state: scheduled while attempts are left, the recorded terminal
    /// failure (with its code) once they are used up or the failure is not
    /// retryable. A head already terminal records nothing more.
    async fn schedule_retry(
        state: &Arc<RwLock<ServerState>>,
        store: &Arc<AgentLibraryStore>,
        tenant: &str,
        connector: &medulla_types::contract::ResourceId,
        error: &str,
        at: RetryAt,
    ) -> Result<(), String> {
        let retryable =
            crate::admin::connector_pack::reproject::is_retryable_projection_error(error);
        use medulla_persistence_records::connector_pack::projection_retry::next_retry;
        let plan = store.prepare_connector_pack_projection(tenant, connector)?;
        let prior = store.connector_pack_projection_retry(tenant, connector)?;
        let code = crate::admin::connector_pack::reproject::projection_error_code(error);
        let Ok(next) = next_retry(
            prior.as_ref(),
            &plan.record_id,
            &code,
            retryable,
            at.now_ms,
            at.knobs,
        ) else {
            return Ok(());
        };
        let (_, _, context) = write_context(
            state,
            store,
            tenant,
            &plan,
            Some(&format!("retry-{}", next.attempt)),
        )
        .await?;
        store.record_connector_pack_projection_retry(context, &plan, &next)
    }

    /// Run every pack retry that is due. Returns whether any ran. A pack's
    /// failure only moves its own retry state; the other due packs still run.
    pub(crate) async fn retry_due(
        state: &Arc<RwLock<ServerState>>,
        store: &Arc<AgentLibraryStore>,
        tenant: &str,
        at: RetryAt,
    ) -> bool {
        let due = match store.due_connector_pack_retries(tenant, at.now_ms) {
            Ok(due) => due,
            Err(error) => {
                tracing::warn!(
                    code = "CONNECTOR_PACK_PROJECTION_RETRY_SCAN_FAILED",
                    %error,
                    "connector pack projection retry scan failed"
                );
                return false;
            }
        };
        for connector in &due {
            let Err(error) = project(state, store, tenant, connector).await else {
                continue;
            };
            if let Err(failure) = schedule_retry(state, store, tenant, connector, &error, at).await
            {
                tracing::warn!(
                    code = "CONNECTOR_PACK_PROJECTION_RETRY_UNRECORDED",
                    connector = connector.as_str(),
                    %failure,
                    "connector pack projection retry state was not recorded"
                );
            }
        }
        !due.is_empty()
    }

    /// The engine service identity the worker projects under. Its operation
    /// key is the record it projects, so a re-delivered row replays the
    /// projection commit rather than writing a second one.
    pub(super) fn service_context(
        server: &medulla_server_api::server_context::ServerContext,
        record_id: &str,
    ) -> Result<VerifiedRequestContext, String> {
        service_context_for(server, CONSUMER, record_id)
    }

    /// The same service identity keyed for `consumer`'s own operations.
    pub(super) fn service_context_for(
        server: &medulla_server_api::server_context::ServerContext,
        consumer: &str,
        record_id: &str,
    ) -> Result<VerifiedRequestContext, String> {
        let service = VerifiedRequestContext::authenticated_fixed_service_actor(
            server,
            super::grant::PROJECTION_SERVICE,
            &["kg:read", "kg:write"],
        )?;
        Ok(VerifiedRequestContext::from_verified_claims_with_nonce(
            service.claims().clone(),
            format!("{consumer}:{record_id}"),
            Some(medulla_types::contract::Nonce::minted()),
        ))
    }

    /// A context shell `bind_agent_library_context` fills in completely from
    /// the verified service identity.
    fn unbound_context(
        tenant: &str,
    ) -> medulla_agent_types::agent_library::AgentLibraryMutationContext {
        medulla_agent_types::agent_library::AgentLibraryMutationContext {
            request_id: 0,
            principal: String::new(),
            caller_principal: String::new(),
            attempt_nonce: medulla_types::contract::Nonce::minted(),
            tenant_id: tenant.to_string(),
            actor_scope: String::new(),
            purpose_id: String::new(),
            policy_revision: String::new(),
            policy_digest: String::new(),
            policy_decision_id: String::new(),
            idempotency_key: String::new(),
            expected_revision: None,
            trace_id: None,
            created_at_ms: 0,
        }
    }
}
