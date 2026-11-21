//! The Agent Library projection worker (MEDULLA-ORCH-VOCAB-R003.6.1).
//!
//! Every Agent Library publish or retire commits one
//! `medulla.agent-library.entry.v1` row on its tenant's Agent Library outbox.
//! This worker claims those rows in order under its durable [`worker::CONSUMER`]
//! lease and projects each entry into its tenant graph through
//! [`map::project_entry`].
//!
//! The Agent Library outbox and the tenant graph are two durability domains,
//! and one admitted batch commits one domain. The worker therefore commits
//! the projection and its consumer offset together in the GRAPH domain: one
//! governed change envelope (`ApplyChangeEnvelopeDraft`, the same dispatch,
//! authorization, durability and audit as any caller's write) whose graph
//! operations remove the triples of the revision the offset names and add the
//! triples of the new revision, and whose change cursor -- source
//! [`CURSOR_SOURCE`], partition the agent, position the entry revision,
//! `expected_previous` the stored offset -- is the consumer offset itself,
//! written in the same commit. The Agent Library lease is acknowledged only
//! after that commit, on the Agent Library's own handle.
//!
//! * A replayed row, or one whose revision is not newer than the stored
//!   offset, writes nothing: the envelope's idempotency key names the consumer,
//!   tenant, agent and revision, so a retried commit replays, and a row behind
//!   the offset is acknowledged without a commit.
//! * A newer revision replaces the older revision's triples in one commit.
//! * A retired entry removes its triples.
//! * A crash between the commit and the acknowledgement redelivers the row,
//!   which the offset then skips.
//! * The worker reads only the deployment tenant's outbox, and a row whose
//!   entry names another tenant is rejected unread as an invalid event.
//!
//! One sweep claims `SchedulerConfig.orchestration_limits.projection_batch`
//! rows.

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_server_api::state::ServerState;

#[cfg(feature = "rdf")]
pub mod map;

/// Start the singleton projection worker. A build without the Agent Library
/// or the RDF triple operations has nothing to project.
pub fn spawn(state: Arc<RwLock<ServerState>>) {
    #[cfg(all(feature = "redb", feature = "rdf"))]
    tokio::spawn(worker::run(state));
    #[cfg(not(all(feature = "redb", feature = "rdf")))]
    drop(state);
}

/// The change-cursor source the consumer offset is stored under.
pub const CURSOR_SOURCE: &str = "agent-library-projection";

/// What one claimed row commits.
#[cfg(feature = "rdf")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionStep {
    /// Nothing to write: the revision is not newer than the stored offset,
    /// or neither revision projects a triple.
    Skip,
    /// Replace the stored revision's triples by the new revision's.
    Commit {
        /// The entry revision the offset moves to.
        revision: u64,
        /// The offset the commit expects to find.
        previous: Option<u64>,
        /// N-Triples of the stored revision, removed first.
        remove: String,
        /// N-Triples of the new revision, added after.
        add: String,
    },
}

/// Decide one row: `entry` at its revision under `kind`, against the stored
/// offset `stored` and the entry revision it names, `previous_entry`.
#[cfg(feature = "rdf")]
pub fn plan_step(
    kind: medulla_agent_types::agent_library::AgentLibraryMutationKind,
    entry: &medulla_agent_types::agent_library::AgentLibraryEntry,
    stored: Option<u64>,
    previous_entry: Option<&medulla_agent_types::agent_library::AgentLibraryEntry>,
) -> ProjectionStep {
    use medulla_agent_types::agent_library::AgentLibraryMutationKind;
    if stored.is_some_and(|stored| stored >= entry.entry_revision) {
        return ProjectionStep::Skip;
    }
    let remove = previous_entry
        .map(|previous| map::ntriples(&map::project_entry(previous)))
        .unwrap_or_default();
    let add = match kind {
        AgentLibraryMutationKind::Publish => map::ntriples(&map::project_entry(entry)),
        AgentLibraryMutationKind::Retire => String::new(),
    };
    if remove.is_empty() && add.is_empty() {
        return ProjectionStep::Skip;
    }
    ProjectionStep::Commit {
        revision: entry.entry_revision,
        previous: stored,
        remove,
        add,
    }
}

/// The cursor partition of one agent: the hex SHA-256 of its id, so any id
/// is a safe partition key.
pub fn cursor_partition(agent_id: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(agent_id.as_bytes()))
}

/// The governed draft that commits one [`ProjectionStep::Commit`] for
/// `agent_id` of `tenant`: the removal, then the addition, and the consumer
/// offset as the envelope's change cursor. `None` for a skip.
#[cfg(feature = "rdf")]
pub fn draft_for_step(
    tenant: &str,
    agent_id: &str,
    step: &ProjectionStep,
) -> Result<Option<medulla_protocol_types::change_envelope::ChangeEnvelopeDraft>, String> {
    use medulla_mutation_types::mutation_scope::{DurabilityDomain, MutationSurface};
    use medulla_protocol_types::change_envelope::{
        ChangeCursor, ChangeEnvelopeDraft, ChangeMutationDraft, ContentVersion,
        ContentVersionPosition, CursorPosition, MaterialClass, MaterialOperation, PolicyRecord,
        PrivacyAttestation, CHANGE_ENVELOPE_VERSION,
    };
    use medulla_protocol_types::mutation_batch::MutationOperation;
    use medulla_protocol_types::protocol::Method;
    use sha2::{Digest, Sha256};

    let ProjectionStep::Commit {
        revision,
        previous,
        remove,
        add,
    } = step
    else {
        return Ok(None);
    };
    let mut methods = Vec::with_capacity(2);
    if !remove.is_empty() {
        methods.push(Method::RemoveTriples {
            turtle: String::new(),
            ntriples: remove.clone(),
        });
    }
    if !add.is_empty() {
        methods.push(Method::AddTriples {
            turtle: String::new(),
            ntriples: add.clone(),
        });
    }
    let operations = methods
        .into_iter()
        .enumerate()
        .map(|(ordinal, method)| MutationOperation {
            ordinal: ordinal as u32,
            surface: MutationSurface::Graph,
            domain: DurabilityDomain::RdfDataset,
            method,
        })
        .collect();
    let partition = cursor_partition(agent_id);
    let envelope_id = format!("{CURSOR_SOURCE}:{partition}:{revision}");
    let mut digest = Sha256::new();
    digest.update(remove.as_bytes());
    digest.update([0]);
    digest.update(add.as_bytes());
    let payload_digest = hex::encode(digest.finalize());
    let object_id = format!("{CURSOR_SOURCE}:{partition}");
    Ok(Some(ChangeEnvelopeDraft {
        schema_version: CHANGE_ENVELOPE_VERSION,
        envelope_id: envelope_id.clone(),
        mutation: ChangeMutationDraft {
            batch_id: envelope_id,
            placement_epoch: 0,
            expected_graph_version: None,
            fencing_token: None,
            operations,
            outbox: Vec::new(),
        },
        privacy: PrivacyAttestation {
            policy_version: "agent-library-projection-v1".to_string(),
            sanitizer_version: "none".to_string(),
            sanitized_payload_digest: payload_digest.clone(),
        },
        material_class: MaterialClass::Attested,
        content_version: ContentVersion {
            object_id: object_id.clone(),
            digest_algorithm: "sha256".to_string(),
            digest: payload_digest,
            previous_digest: None,
            source_version: ContentVersionPosition::Sequence(*revision),
        },
        cursor: Some(ChangeCursor {
            source: CURSOR_SOURCE.to_string(),
            partition,
            position: CursorPosition::Sequence(*revision),
            expected_previous: previous.map(CursorPosition::Sequence),
        }),
        lineage: Vec::new(),
        policies: vec![PolicyRecord {
            policy_id: format!("{CURSOR_SOURCE}:{tenant}"),
            operation: MaterialOperation::Upsert,
            object_id,
            tenant: tenant.to_string(),
            classification: "internal".to_string(),
            policy_version: "agent-library-projection-v1".to_string(),
            subject_set_digest: "0".repeat(64),
            retention_policy: "default".to_string(),
            legal_hold: false,
        }],
        evidence: Vec::new(),
        features: Vec::new(),
        blobs: Vec::new(),
    }))
}

/// The sweep loop: claim, project, commit, acknowledge.
#[cfg(all(feature = "redb", feature = "rdf"))]
pub mod worker {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::RwLock;

    use medulla_agent_types::agent_library::{AgentLibraryEntry, AgentLibraryOutboxEvent};
    use medulla_mutation_types::mutation_scope::MutationOutboxLease;
    use medulla_persistence_records::agent_library::{
        AgentLibraryStore, AGENT_LIBRARY_OUTBOX_TOPIC,
    };
    use medulla_protocol_types::change_envelope::{ChangeCursor, CursorPosition};
    use medulla_protocol_types::protocol::{Method, Request, ResultPayload};
    use medulla_server_api::authority_context::VerifiedRequestContext;
    use medulla_server_api::outbox_operator::OutboxWrite;
    use medulla_server_api::state::ServerState;
    use medulla_transaction::{OutboxClaimBudget, OutboxClaimOutcome, OutboxRejectReason};
    use medulla_work_types::scheduler_config::{OrchestrationLimits, SchedulerConfig};

    use super::{cursor_partition, draft_for_step, plan_step, ProjectionStep, CURSOR_SOURCE};

    /// The label a failed blocking store task names.
    const BLOCKING_TASK: &str = "agent library projection";
    /// The worker's one durable consumer name on the tenant's outbox.
    pub const CONSUMER: &str = "agent-library-projection";
    /// The service the worker signs its graph writes as.
    const PROJECTION_SERVICE: &str = "agent-library-projection";
    /// How long a claimed row stays leased.
    const LEASE_MS: u64 = 60_000;
    /// Idle back-off between sweeps.
    const IDLE: Duration = Duration::from_millis(500);

    pub(super) async fn run(state: Arc<RwLock<ServerState>>) {
        let subscribed = AtomicBool::new(false);
        loop {
            match sweep(&state, &subscribed).await {
                Ok(true) => {}
                Ok(false) => tokio::time::sleep(IDLE).await,
                Err(error) => {
                    tracing::warn!(
                        code = "AGENT_LIBRARY_PROJECTION_SWEEP_FAILED",
                        %error,
                        "agent library projection sweep failed"
                    );
                    tokio::time::sleep(IDLE).await;
                }
            }
        }
    }

    /// One sweep over the deployment tenant's entry stream, in claim order.
    /// Returns whether it claimed any row, so an idle worker backs off.
    pub async fn sweep(
        state: &Arc<RwLock<ServerState>>,
        subscribed: &AtomicBool,
    ) -> Result<bool, String> {
        let (anonymous, server) = {
            let state = state.read().await;
            (state.local_anonymous, state.context.clone())
        };
        let tenant = if anonymous {
            VerifiedRequestContext::anonymous_local(None)
                .tenant()
                .to_string()
        } else {
            server.request_policy()?.expected_tenant.clone()
        };
        let store = medulla_persistence_records::agent_library::shared_agent_library(state).await?;
        if !subscribed.load(Ordering::Acquire) {
            let (store, tenant) = (Arc::clone(&store), tenant.clone());
            medulla_server_api::api::dispatch_support::blocking_task(BLOCKING_TASK, move || {
                store.outbox_subscribe(&tenant, CONSUMER, AGENT_LIBRARY_OUTBOX_TOPIC)
            })
            .await?;
            subscribed.store(true, Ordering::Release);
        }
        let limit = projection_batch(state).await;
        let outcome = claim(&store, &tenant, limit).await?;
        if !outcome.dead_lettered.is_empty() {
            medulla_kernel::metrics::process().outbox_dead_lettered(
                CONSUMER,
                AGENT_LIBRARY_OUTBOX_TOPIC,
                "exhausted",
                outcome.dead_lettered.len() as u64,
            );
        }
        let claimed = outcome.claims.len();
        for lease in outcome.claims {
            // One row's failure never ends the sweep. An unresolved lease
            // expires and the row is redelivered under the outbox's bounded
            // attempts; the stored offset keeps the retry idempotent.
            match project(state, &store, &tenant, &lease).await {
                Ok(reason) => {
                    if let Err(error) = resolve(&store, &tenant, lease, reason).await {
                        tracing::warn!(
                            code = "AGENT_LIBRARY_PROJECTION_SETTLE_FAILED",
                            %error,
                            "agent library projection: one row could not be resolved"
                        );
                    }
                }
                Err(error) => tracing::warn!(
                    code = "AGENT_LIBRARY_PROJECTION_FAILED",
                    %error,
                    "agent library projection: one row failed and will be redelivered"
                ),
            }
        }
        Ok(claimed > 0)
    }

    /// `orchestration_limits.projection_batch` of the policy in force: the
    /// stored scheduler policy, else the default.
    async fn projection_batch(state: &Arc<RwLock<ServerState>>) -> u32 {
        let stored = {
            let current = state.read().await;
            medulla_persistence::scheduler_config_store::shared_scheduler_config_store(&current)
                .and_then(|store| store.get())
        };
        let limits = match stored {
            Ok(Some(json)) => serde_json::from_str::<SchedulerConfig>(&json)
                .map(|config| config.orchestration_limits)
                .unwrap_or_default(),
            _ => OrchestrationLimits::default(),
        };
        limits.projection_batch
    }

    async fn claim(
        store: &Arc<AgentLibraryStore>,
        tenant: &str,
        limit: u32,
    ) -> Result<OutboxClaimOutcome, String> {
        let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        let mut budget = OutboxClaimBudget::new(limit, LEASE_MS, now_ms)?;
        let (store, tenant) = (Arc::clone(store), tenant.to_string());
        medulla_server_api::api::dispatch_support::blocking_task(BLOCKING_TASK, move || {
            store.outbox_claim(&tenant, CONSUMER, &mut budget)
        })
        .await
    }

    /// Project one leased row. `Ok(None)` acknowledges it, `Ok(Some(reason))`
    /// rejects it, `Err` leaves the lease to expire for a redelivery.
    async fn project(
        state: &Arc<RwLock<ServerState>>,
        store: &Arc<AgentLibraryStore>,
        tenant: &str,
        lease: &MutationOutboxLease,
    ) -> Result<Option<OutboxRejectReason>, String> {
        let Ok(event) =
            medulla_storage::decode_ledger_record::<AgentLibraryOutboxEvent>(&lease.record.intent.payload)
        else {
            return Ok(Some(OutboxRejectReason::InvalidEvent));
        };
        // A row from another tenant is never read.
        if event.validate().is_err() || event.entry.tenant_id != tenant {
            return Ok(Some(OutboxRejectReason::InvalidEvent));
        }
        let agent_id = event.entry.agent_id.clone();
        let (anonymous, server) = {
            let state = state.read().await;
            (state.local_anonymous, state.context.clone())
        };
        let graph = medulla_identity_types::tenancy::tenant_graph(
            &medulla_identity_types::tenancy::TenantId::new(tenant)
                .map_err(|error| error.to_string())?,
        );
        let session = format!("{CONSUMER}:{tenant}:{}", cursor_partition(&agent_id));
        let context = service_context(anonymous, &server, session.clone())?;
        // The service holds exactly this tenant graph, which must be resident
        // before its offset can be read.
        #[cfg(feature = "security")]
        if !anonymous {
            medulla_server_api::service_grant::ServiceGraphGrant {
                actor: &format!("service:{PROJECTION_SERVICE}"),
                role_prefix: PROJECTION_SERVICE,
                unbootstrapped: "AGENT_LIBRARY_PROJECTION_POLICY_UNBOOTSTRAPPED",
            }
            .ensure(&mut state.write().await.isolation, &graph)?;
        }
        medulla_server_api::api::reentry::ensure_internal_global_graph(
            state, 0, &context, &graph, &session,
        )
        .await?;
        let stored = stored_offset(state, &context, &graph, tenant, &agent_id).await?;
        let previous_entry = match stored {
            Some(revision) => previous_revision(store, tenant, &agent_id, revision).await?,
            None => None,
        };
        let step = plan_step(event.kind, &event.entry, stored, previous_entry.as_ref());
        let Some(draft) = draft_for_step(tenant, &agent_id, &step)? else {
            return Ok(None);
        };
        let ProjectionStep::Commit { revision, .. } = step else {
            return Ok(None);
        };
        let commit_context = service_context(anonymous, &server, format!("{session}:{revision}"))?;
        let request = Request {
            id: 0,
            graph,
            auth_token: String::new(),
            agent_id: Some(commit_context.agent_id().to_string()),
            method: Method::ApplyChangeEnvelopeDraft {
                draft: Box::new(draft),
            },
        };
        let response = medulla_server_api::api::reentry::dispatch_verified_request(
            state,
            request,
            commit_context,
        )
        .await;
        match response.refusal_text() {
            Some(refusal) => Err(refusal),
            None => Ok(None),
        }
    }

    /// The stored entry revision `revision` of `agent_id`, the one whose
    /// triples the graph holds.
    async fn previous_revision(
        store: &Arc<AgentLibraryStore>,
        tenant: &str,
        agent_id: &str,
        revision: u64,
    ) -> Result<Option<AgentLibraryEntry>, String> {
        let (store, tenant, agent_id) =
            (Arc::clone(store), tenant.to_string(), agent_id.to_string());
        let revisions =
            medulla_server_api::api::dispatch_support::blocking_task(BLOCKING_TASK, move || {
                store.revisions(&tenant, &agent_id)
            })
            .await?;
        Ok(revisions
            .into_iter()
            .find(|entry| entry.entry_revision == revision))
    }

    /// The consumer offset of `agent_id`: the position of the change cursor
    /// the last committed projection envelope carried.
    async fn stored_offset(
        state: &Arc<RwLock<ServerState>>,
        context: &VerifiedRequestContext,
        graph: &str,
        tenant: &str,
        agent_id: &str,
    ) -> Result<Option<u64>, String> {
        let request = Request {
            id: 0,
            graph: graph.to_string(),
            auth_token: String::new(),
            agent_id: Some(context.agent_id().to_string()),
            method: Method::GetChangeCursor {
                source: CURSOR_SOURCE.to_string(),
                partition: cursor_partition(agent_id),
                tenant: tenant.to_string(),
            },
        };
        let response = medulla_server_api::api::reentry::dispatch_verified_request(
            state,
            request,
            context.clone(),
        )
        .await;
        if let Some(refusal) = response.refusal_text() {
            return Err(refusal);
        }
        let cursor: Option<ChangeCursor> = match response.result {
            Some(ResultPayload::Raw(bytes)) => medulla_types::msgpack::decode_bounded(
                &bytes,
                medulla_types::msgpack::MsgpackLimits::new(64 * 1024, 1_000, 16),
            )
            .map_err(|_| "the stored projection offset is corrupt".to_string())?,
            Some(ResultPayload::Json(value)) => serde_json::from_value(value)
                .map_err(|_| "the stored projection offset is corrupt".to_string())?,
            None => None,
            Some(_) => return Err("the stored projection offset has an invalid shape".into()),
        };
        match cursor.map(|cursor| cursor.position) {
            Some(CursorPosition::Sequence(revision)) => Ok(Some(revision)),
            Some(_) => Err("the stored projection offset is not an entry revision".into()),
            None => Ok(None),
        }
    }

    /// The service context the worker reads and commits under; its
    /// idempotency key is `key`, so a retried commit replays.
    fn service_context(
        anonymous: bool,
        server: &medulla_server_api::server_context::ServerContext,
        key: String,
    ) -> Result<VerifiedRequestContext, String> {
        if anonymous {
            return Ok(VerifiedRequestContext::anonymous_local(Some(key)));
        }
        let service = VerifiedRequestContext::authenticated_fixed_service_actor(
            server,
            PROJECTION_SERVICE,
            &["ingest:write", "ingest:read"],
        )?;
        Ok(VerifiedRequestContext::from_verified_claims_with_nonce(
            service.claims().clone(),
            key,
            Some(medulla_types::contract::Nonce::minted()),
        ))
    }

    async fn resolve(
        store: &Arc<AgentLibraryStore>,
        tenant: &str,
        lease: MutationOutboxLease,
        reason: Option<OutboxRejectReason>,
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
            CONSUMER,
            AGENT_LIBRARY_OUTBOX_TOPIC,
            "invalid_event",
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
}

#[cfg(all(test, feature = "rdf"))]
mod tests {
    use super::*;
    use medulla_agent_types::agent_component::{AgentComponentKind, ComponentDependency};
    use medulla_agent_types::agent_library::{
        AgentLibraryEntry, AgentLibraryLifecycle, AgentLibraryMutationKind,
    };
    use medulla_protocol_types::change_envelope::CursorPosition;
    use medulla_protocol_types::protocol::Method;

    fn pin(component_id: &str, kind: AgentComponentKind) -> ComponentDependency {
        ComponentDependency {
            component_id: component_id.to_string(),
            kind,
            definition_digest: "a".repeat(64),
        }
    }

    fn entry(revision: u64, lifecycle: AgentLibraryLifecycle) -> AgentLibraryEntry {
        AgentLibraryEntry {
            schema_version: 1,
            agent_id: "agent-a".to_string(),
            package_id: "package-a".to_string(),
            version: "1.0.0".to_string(),
            role: "researcher".to_string(),
            role_digest: "b".repeat(64),
            system_prompt: pin("prompt:a", AgentComponentKind::SystemPrompt),
            tools: Vec::new(),
            skills: Vec::new(),
            model_profile: pin("model:default", AgentComponentKind::ModelProfile),
            model_identity: "model:default".to_string(),
            ontologies: Vec::new(),
            tenant_id: "tenant-a".to_string(),
            actor_scope: "definition:builder".to_string(),
            purpose_id: "agent-library:definition".to_string(),
            policy_digest: "c".repeat(64),
            source_revision: format!("source:{revision}"),
            source_revision_digest: "d".repeat(64),
            runtime: Default::default(),
            instantiated_from: None,
            entry_revision: revision,
            lifecycle,
            definition_digest: "e".repeat(64),
            created_at_ms: 1,
            updated_at_ms: revision,
        }
    }

    // spec: MEDULLA-ORCH-VOCAB-R003.6.1
    #[test]
    fn replay_writes_nothing_newer_replaces_and_retire_removes() {
        let publish = AgentLibraryMutationKind::Publish;
        let first = entry(1, AgentLibraryLifecycle::Published);
        let second = entry(2, AgentLibraryLifecycle::Published);

        // The first revision adds its triples and moves the offset from none.
        let ProjectionStep::Commit {
            revision,
            previous,
            remove,
            add,
        } = plan_step(publish, &first, None, None)
        else {
            panic!("a first publish commits");
        };
        assert_eq!((revision, previous), (1, None));
        assert!(remove.is_empty());
        assert!(add.contains("memberOfLibrary"));

        // A replayed or older row writes nothing.
        assert_eq!(plan_step(publish, &first, Some(1), Some(&first)), ProjectionStep::Skip);
        assert_eq!(plan_step(publish, &first, Some(2), Some(&second)), ProjectionStep::Skip);

        // A newer revision removes the stored revision's triples, then adds its own.
        let step = plan_step(publish, &second, Some(1), Some(&first));
        let draft = draft_for_step("tenant-a", "agent-a", &step)
            .unwrap()
            .expect("a newer revision commits");
        let methods: Vec<&Method> = draft
            .mutation
            .operations
            .iter()
            .map(|operation| &operation.method)
            .collect();
        assert!(matches!(methods[0], Method::RemoveTriples { .. }));
        assert!(matches!(methods[1], Method::AddTriples { .. }));
        let cursor = draft.cursor.expect("the offset rides the same commit");
        assert_eq!(cursor.source, CURSOR_SOURCE);
        assert_eq!(cursor.position, CursorPosition::Sequence(2));
        assert_eq!(cursor.expected_previous, Some(CursorPosition::Sequence(1)));

        // A retire removes the stored triples and adds none.
        let tombstone = entry(3, AgentLibraryLifecycle::Retired);
        let ProjectionStep::Commit { remove, add, .. } =
            plan_step(AgentLibraryMutationKind::Retire, &tombstone, Some(2), Some(&second))
        else {
            panic!("a retire of a projected entry commits");
        };
        assert!(remove.contains("memberOfLibrary"));
        assert!(add.is_empty());

        // A retire of a never-projected entry, and a skip, draft nothing.
        assert_eq!(
            plan_step(AgentLibraryMutationKind::Retire, &tombstone, None, None),
            ProjectionStep::Skip
        );
        assert!(draft_for_step("tenant-a", "agent-a", &ProjectionStep::Skip)
            .unwrap()
            .is_none());
    }
}
