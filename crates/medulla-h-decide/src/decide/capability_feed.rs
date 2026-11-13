//! The capability index feed (MEDULLA-DECISION-ENGINE-R172.7.2, R172.12.3).
//!
//! Two durable Agent Library outbox consumers keep the resident capability
//! index current without a rebuild:
//!
//! * the component consumer reads `medulla.agent-component.revision.v1`: a
//!   published tool or skill is upserted, a retired or withdrawn component is
//!   tombstoned;
//! * the surface consumer reads `medulla.connector-pack.surface.v1`, which an
//!   applied pack projection leaves in its own commit: the member's declared
//!   `appliesTo` / `executes` classes and its pack origin are upserted onto the
//!   record.
//!
//! Records are embedded by the configured embedder under the ACTIVE embedding
//! generation of the commons graph (DECISIONS DE-R172.7.2): the record carries
//! that generation's digest as its embedding version and its dimension, so the
//! R172.6 pin refuses a mixed space. With no generation declared, or an
//! embedder that fails, the feed refuses `EMBEDDING_UNAVAILABLE`, leaves the
//! lease unresolved (the kernel redelivers it) and indexes nothing -- never a
//! zero vector. A record whose text did not change keeps its old vector until
//! it is republished. The feed runs beside `connector_pack_projection::spawn`
//! and reads the same outbox the projection worker does; there is no
//! client-side feed and no client-side index.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use medulla_agent_types::agent_component::{
    AgentComponentEntry, AgentComponentKind, AgentComponentMutationKind, AgentComponentOutboxEvent,
};
use medulla_agent_types::agent_library::AgentLibraryLifecycle;
use medulla_catalog_types::connector_pack::{
    PackSurfaceEvent, PackSurfaceFacts, CONNECTOR_PACK_SURFACE_TOPIC,
};
use medulla_decision_memory::memory::capability_index::{CapabilityIndex, IndexRefusal};
use medulla_decision_types::decision::capability_designation::{
    CapabilityRecord, DesignationKnobs, EntityKind, PackOrigin,
};
use medulla_decision_types::decision::DecisionPolicy;
use medulla_mutation_types::mutation_scope::MutationOutboxLease;
use medulla_persistence_records::agent_component::AGENT_COMPONENT_OUTBOX_TOPIC;
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_server_api::outbox_operator::OutboxWrite;
use medulla_transaction::{OutboxClaimBudget, OutboxRejectReason};
use medulla_types::contract::{OpaqueId, TenantId};
use tokio::sync::RwLock;

pub use super::stat_designation::shared_capability_index;
use super::stat_generation_cycle::ConfiguredEmbedder;
pub use super::stat_generation_cycle::Reembedder;

/// The component consumer's durable name on every tenant's outbox.
pub const COMPONENT_CONSUMER: &str = "capability-feed-component-v1";
/// The surface consumer's durable name on every tenant's outbox.
pub const SURFACE_CONSUMER: &str = "capability-feed-surface-v1";
/// The graph whose active embedding generation embeds the feed's records.
const COMMONS_GRAPH: &str = "__commons__";
/// The label a failed blocking store task names.
const BLOCKING_TASK: &str = "capability feed";

/// Start the singleton feed worker.
pub fn spawn(state: Arc<RwLock<medulla_server_api::state::ServerState>>) {
    tokio::spawn(run(state));
}

async fn run(state: Arc<RwLock<medulla_server_api::state::ServerState>>) {
    let knobs = DesignationKnobs::from_policy(&DecisionPolicy::engine_default());
    let idle = Duration::from_millis(knobs.feed_idle_ms);
    let embedder: Arc<dyn Reembedder> = Arc::new(ConfiguredEmbedder(
        medulla_server_ops::api::ingest::embedder::EmbedderConfig::from_env(),
    ));
    let subscribed = AtomicBool::new(false);
    loop {
        let tenant = match deployment_tenant(&state).await {
            Ok(tenant) => tenant,
            Err(error) => {
                tracing::warn!(code = "CAPABILITY_FEED_SWEEP_FAILED", %error, "capability feed has no tenant");
                tokio::time::sleep(idle).await;
                continue;
            }
        };
        match sweep(&state, &tenant, &embedder, &subscribed).await {
            Ok(true) => {}
            Ok(false) => tokio::time::sleep(idle).await,
            Err(error) => {
                tracing::warn!(code = "CAPABILITY_FEED_SWEEP_FAILED", %error, "capability feed sweep failed");
                tokio::time::sleep(idle).await;
            }
        }
    }
}

async fn deployment_tenant(
    state: &Arc<RwLock<medulla_server_api::state::ServerState>>,
) -> Result<String, String> {
    let (anonymous, server) = {
        let state = state.read().await;
        (state.local_anonymous, state.context.clone())
    };
    if anonymous {
        Ok(
            medulla_server_api::authority_context::VerifiedRequestContext::anonymous_local(None)
                .tenant()
                .to_string(),
        )
    } else {
        Ok(server.request_policy()?.expected_tenant.clone())
    }
}

/// The version and dimension of the commons graph's active embedding
/// generation (the one resolution of [`super::stat_generation_space`]), or
/// the typed refusal when none is declared.
async fn active_generation(
    state: &Arc<RwLock<medulla_server_api::state::ServerState>>,
    store: &Arc<AgentLibraryStore>,
    tenant: &str,
) -> Result<(String, u16), String> {
    let unavailable = |why: &str| {
        format!(
            "EMBEDDING_UNAVAILABLE: the capability feed has no active embedding generation: {why}"
        )
    };
    let scope = medulla_server_api::carrier::tenant_scope_of(tenant);
    let space = super::stat_generation_space::active_space(state, store, &scope, COMMONS_GRAPH)
        .await
        .map_err(|error| unavailable(&error))?
        .ok_or_else(|| unavailable("the commons generation declares no embedding space"))?;
    let dim = u16::try_from(space.dimensions)
        .map_err(|_| unavailable("the declared dimension does not fit a record"))?;
    Ok((space.space_digest, dim))
}

/// One sweep over both streams of `tenant`. Returns whether it did any work,
/// so an idle worker backs off.
pub async fn sweep(
    state: &Arc<RwLock<medulla_server_api::state::ServerState>>,
    tenant: &str,
    embedder: &Arc<dyn Reembedder>,
    subscribed: &AtomicBool,
) -> Result<bool, String> {
    let store = medulla_persistence_records::agent_library::shared_agent_library(state).await?;
    if !subscribed.load(Ordering::Acquire) {
        for (consumer, topic) in [
            (COMPONENT_CONSUMER, AGENT_COMPONENT_OUTBOX_TOPIC),
            (SURFACE_CONSUMER, CONNECTOR_PACK_SURFACE_TOPIC),
        ] {
            let (store, tenant) = (Arc::clone(&store), tenant.to_string());
            medulla_server_api::api::dispatch_support::blocking_task(BLOCKING_TASK, move || {
                store.outbox_subscribe(&tenant, consumer, topic)
            })
            .await?;
        }
        subscribed.store(true, Ordering::Release);
    }
    let index = shared_capability_index(&*state.read().await);
    let knobs = DesignationKnobs::from_policy(&DecisionPolicy::engine_default());
    let mut progressed = false;
    for consumer in [COMPONENT_CONSUMER, SURFACE_CONSUMER] {
        let claims = claim(&store, tenant, consumer, &knobs).await?;
        progressed |= !claims.is_empty();
        if claims.is_empty() {
            continue;
        }
        // One generation read and one embed call per claimed batch.
        let generation = active_generation(state, &store, tenant).await;
        for lease in claims {
            let outcome = match &generation {
                Err(error) => Err(Settle::Retry(error.clone())),
                Ok(generation) => {
                    // Embedding is a blocking call: run the row off the reactor.
                    let work = (
                        Arc::clone(&index),
                        Arc::clone(&store),
                        tenant.to_string(),
                        lease.record.intent.payload.clone(),
                        generation.clone(),
                        Arc::clone(embedder),
                    );
                    tokio::task::spawn_blocking(move || {
                        let (index, store, tenant, payload, generation, embedder) = work;
                        if consumer == COMPONENT_CONSUMER {
                            component_row(&index, &tenant, &payload, &generation, embedder.as_ref())
                        } else {
                            surface_row(
                                &index,
                                &store,
                                &tenant,
                                &payload,
                                &generation,
                                embedder.as_ref(),
                            )
                        }
                    })
                    .await
                    .map_err(|error| format!("capability feed row task failed: {error}"))?
                }
            };
            settle(&store, tenant, consumer, lease, outcome).await?;
        }
    }
    Ok(progressed)
}

/// How one leased row ends.
enum Settle {
    /// Leave the lease unresolved: the kernel redelivers it after the lease.
    Retry(String),
    /// The row can never be applied.
    Reject(OutboxRejectReason, String),
}

async fn claim(
    store: &Arc<AgentLibraryStore>,
    tenant: &str,
    consumer: &'static str,
    knobs: &DesignationKnobs,
) -> Result<Vec<MutationOutboxLease>, String> {
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let mut budget =
        OutboxClaimBudget::new(knobs.feed_claim_limit as u32, knobs.feed_lease_ms, now_ms)?;
    let (store, tenant) = (Arc::clone(store), tenant.to_string());
    medulla_server_api::api::dispatch_support::blocking_task(BLOCKING_TASK, move || {
        store.outbox_claim(&tenant, consumer, &mut budget)
    })
    .await
    .map(|outcome| outcome.claims)
}

async fn settle(
    store: &Arc<AgentLibraryStore>,
    tenant: &str,
    consumer: &'static str,
    lease: MutationOutboxLease,
    outcome: Result<(), Settle>,
) -> Result<(), String> {
    let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let (store, tenant) = (Arc::clone(store), tenant.to_string());
    match outcome {
        Ok(()) => {
            medulla_server_api::api::dispatch_support::blocking_task(BLOCKING_TASK, move || {
                store.outbox_ack(&tenant, &lease, now_ms)
            })
            .await
        }
        Err(Settle::Retry(error)) => {
            tracing::warn!(code = "CAPABILITY_FEED_ROW_DEFERRED", consumer, %error, "capability feed row stays leased and is redelivered");
            Ok(())
        }
        Err(Settle::Reject(reason, error)) => {
            tracing::warn!(code = "CAPABILITY_FEED_ROW_REJECTED", consumer, %error, "capability feed row rejected");
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
}

/// What a component kind designates as, or `None` when it is not designatable.
fn entity_kind(kind: AgentComponentKind) -> Option<EntityKind> {
    match kind {
        AgentComponentKind::Tool => Some(EntityKind::Tool),
        AgentComponentKind::Skill => Some(EntityKind::Skill),
        _ => None,
    }
}

/// The text a record is embedded from.
fn embedding_text(component: &AgentComponentEntry) -> String {
    format!(
        "{}\n{}\n{}",
        component.component_id,
        component.summary,
        component.declared_capabilities.join(" ")
    )
}

/// The record of a published component, before its embedding is attached.
fn record_of(
    component: &AgentComponentEntry,
    kind: EntityKind,
    generation: &(String, u16),
) -> Result<CapabilityRecord, Settle> {
    let invalid = |error: String| Settle::Reject(OutboxRejectReason::InvalidEvent, error);
    Ok(CapabilityRecord {
        tenant_id: Some(TenantId::new(component.tenant_id.as_str()).map_err(invalid)?),
        entity_id: OpaqueId::new(component.component_id.as_str())
            .map_err(|error| Settle::Reject(OutboxRejectReason::InvalidEvent, error))?,
        entity_kind: kind,
        capabilities: component.declared_capabilities.iter().cloned().collect(),
        ontology_type: None,
        policy_tags: component.classification.iter().cloned().collect(),
        swappable_with: BTreeSet::new(),
        embedding_version: generation.0.clone(),
        dim: generation.1,
        record_revision: component.entry_revision,
        applies_to: BTreeSet::new(),
        executes: BTreeSet::new(),
        pack: None,
    })
}

/// Embed one text under the generation's dimension.
fn embed_one(
    embedder: &dyn Reembedder,
    text: String,
    generation: &(String, u16),
) -> Result<Vec<f32>, Settle> {
    let vectors = embedder
        .embed(&[text])
        .map_err(|error| Settle::Retry(error))?;
    let vector = vectors
        .into_iter()
        .next()
        .ok_or_else(|| Settle::Retry("EMBEDDING_UNAVAILABLE: no vector returned".to_string()))?;
    if vector.len() != usize::from(generation.1) {
        return Err(Settle::Retry(format!(
            "EMBEDDING_UNAVAILABLE: the embedder returned {} dimensions; the active generation has {}",
            vector.len(),
            generation.1
        )));
    }
    Ok(vector)
}

/// Apply one upsert, mapping an index refusal to how the row ends.
fn upsert(
    index: &CapabilityIndex,
    record: CapabilityRecord,
    vector: Vec<f32>,
) -> Result<(), Settle> {
    let existing_revision = index.shard(record.tenant_id.as_ref()).and_then(|shard| {
        shard
            .snapshot()
            .record(&record.entity_id)
            .map(|held| held.record_revision)
    });
    // A redelivered older revision never replaces a newer record.
    if existing_revision.is_some_and(|held| held > record.record_revision) {
        return Ok(());
    }
    index
        .upsert(record, vector)
        .map(drop)
        .map_err(|refusal| match refusal {
            IndexRefusal::MalformedVector { .. } | IndexRefusal::InvalidRecord { .. } => {
                Settle::Reject(
                    OutboxRejectReason::DomainRefused,
                    refusal.code().to_string(),
                )
            }
            other => Settle::Retry(format!("{}: {other:?}", other.code())),
        })
}

fn component_row(
    index: &CapabilityIndex,
    tenant: &str,
    payload: &[u8],
    generation: &(String, u16),
    embedder: &dyn Reembedder,
) -> Result<(), Settle> {
    let event: AgentComponentOutboxEvent = medulla_storage::decode_ledger_record(payload)
        .map_err(|error| Settle::Reject(OutboxRejectReason::InvalidEvent, error.to_string()))?;
    let component = &event.component;
    if component.tenant_id != tenant {
        return Err(Settle::Reject(
            OutboxRejectReason::InvalidEvent,
            "the component event names another tenant than its outbox".to_string(),
        ));
    }
    let Some(kind) = entity_kind(component.kind) else {
        return Ok(());
    };
    let tenant_id = TenantId::new(tenant)
        .map_err(|error| Settle::Reject(OutboxRejectReason::InvalidEvent, error))?;
    let entity_id = OpaqueId::new(component.component_id.as_str())
        .map_err(|error| Settle::Reject(OutboxRejectReason::InvalidEvent, error))?;
    match (event.kind, component.lifecycle) {
        (AgentComponentMutationKind::Retire | AgentComponentMutationKind::Withdraw, _)
        | (_, AgentLibraryLifecycle::Retired | AgentLibraryLifecycle::Withdrawn) => {
            match index.tombstone(Some(&tenant_id), &entity_id) {
                Ok(_) | Err(IndexRefusal::UnknownEntity { .. }) => Ok(()),
                Err(other) => Err(Settle::Retry(format!("{}: {other:?}", other.code()))),
            }
        }
        _ => {
            let mut record = record_of(component, kind, generation)?;
            // Pack facts arrive on the surface stream; a republish keeps them.
            if let Some(held) = index
                .shard(Some(&tenant_id))
                .and_then(|shard| shard.snapshot().record(&entity_id).cloned())
            {
                record.applies_to = held.applies_to;
                record.executes = held.executes;
                record.pack = held.pack;
            }
            let vector = embed_one(embedder, embedding_text(component), generation)?;
            upsert(index, record, vector)
        }
    }
}

fn surface_row(
    index: &CapabilityIndex,
    store: &Arc<AgentLibraryStore>,
    tenant: &str,
    payload: &[u8],
    generation: &(String, u16),
    embedder: &dyn Reembedder,
) -> Result<(), Settle> {
    let event: PackSurfaceEvent = medulla_storage::decode_ledger_record(payload)
        .map_err(|error| Settle::Reject(OutboxRejectReason::InvalidEvent, error.to_string()))?;
    if event.tenant_id != tenant {
        return Err(Settle::Reject(
            OutboxRejectReason::InvalidEvent,
            "the surface event names another tenant than its outbox".to_string(),
        ));
    }
    for facts in &event.members {
        surface_member(index, store, tenant, &event, facts, generation, embedder)?;
    }
    Ok(())
}

fn surface_member(
    index: &CapabilityIndex,
    store: &Arc<AgentLibraryStore>,
    tenant: &str,
    event: &PackSurfaceEvent,
    facts: &PackSurfaceFacts,
    generation: &(String, u16),
    embedder: &dyn Reembedder,
) -> Result<(), Settle> {
    let component = store
        .current_component(tenant, &facts.component_id)
        .map_err(Settle::Retry)?;
    let Some(component) = component else {
        return Ok(());
    };
    let Some(kind) = entity_kind(component.kind) else {
        return Ok(());
    };
    // Only a published component designates; the component stream tombstones
    // the rest.
    if component.lifecycle != AgentLibraryLifecycle::Published {
        return Ok(());
    }
    let mut record = record_of(&component, kind, generation)?;
    record.applies_to = facts.applies_to.clone();
    record.executes = facts.executes.clone();
    record.pack = Some(PackOrigin {
        connector: event.connector.clone(),
        binding_revision: event.binding_revision,
        pack_digest: event.pack_digest.clone(),
        entry_digest: component.content_digest.clone(),
    });
    let vector = embed_one(embedder, embedding_text(&component), generation)?;
    upsert(index, record, vector)
}
