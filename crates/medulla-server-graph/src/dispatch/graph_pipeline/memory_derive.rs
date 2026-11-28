//! Appending a turn enqueues one derive job (MEDULLA-DECISION-ENGINE-R651.11).
//!
//! `dispatch_graph_op` calls [`enqueue_derivations`] after a `MemoryRemember` commits, and the
//! transcript path of `SourceIngest` calls it after its thread is remembered. For each newly
//! stored turn it takes the derivation targets of the turn's session (R651.10) whose subject is
//! the speaker, and keeps ONE pending task per key on the durable queue `memory.derive`, in the
//! lane `derive/<session_id>`:
//!
//! - a key with no pending task gets a new task, and a `DerivePayload` node naming its turn
//!   range, the task and the time it is due;
//! - a key whose task is still unclaimed gets its range extended in the payload node and no
//!   second task.
//!
//! The due time is stored with the range (`debounce` after the newest turn, or the newest turn
//! itself once the range holds `batch_turns` turns); the derive worker reads it. When the tenant
//! already holds `derive_max_pending` unclaimed tasks, the enqueue is skipped and reported as
//! `DERIVE_BACKLOG_FULL`; the turn itself is already committed. The task reaches the queue
//! only through the jobs seam, so this crate never calls a sibling handler.

use std::collections::BTreeSet;
use std::sync::Arc;

use medulla_protocol_types::protocol::Method;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::knob_config::knobs;
use medulla_server_api::state::ServerState;
use medulla_server_ops::api::ingest::{jobs_seam, IngestTaskWrite, JobsApi};
use medulla_work_types::participant::ParticipantId;
use tokio::sync::RwLock;
use {
    medulla_decision_stats_records::decision::participant_derive::derivation_targets,
    medulla_decision_stats_records::decision::participant_derive::due_at,
    medulla_decision_stats_records::decision::participant_derive::extend_payload,
    medulla_decision_stats_records::decision::participant_derive::key_digest,
    medulla_decision_stats_records::decision::participant_derive::DerivationInput,
    medulla_decision_stats_records::decision::participant_derive::DeriveKey,
    medulla_decision_stats_records::decision::participant_derive::DerivePayload,
    medulla_decision_stats_records::decision::participant_derive::DueRule,
};
use {
    medulla_h_graph_gateway::graph_ops::participant_derive::observation_configs,
    medulla_h_graph_gateway::graph_ops::participant_derive::payload_node_id,
    medulla_h_graph_gateway::graph_ops::participant_derive::payload_write,
    medulla_h_graph_gateway::graph_ops::participant_derive::read_payload,
    medulla_h_graph_gateway::graph_ops::participant_derive::session_turns,
    medulla_h_graph_gateway::graph_ops::participant_derive::SessionTurn,
};

/// The durable queue every derive task goes to.
pub(crate) const DERIVE_QUEUE: &str = "memory.derive";
/// Refusal of an enqueue the tenant's pending limit stops.
const DERIVE_BACKLOG_FULL: &str = "DERIVE_BACKLOG_FULL";

/// The arguments of [`enqueue_derivations`].
pub(crate) struct EnqueueRequest<'a> {
    pub state: &'a Arc<RwLock<ServerState>>,
    pub graph_name: &'a str,
    pub req_id: u64,
    pub caller: Option<&'a str>,
    pub verified_context: &'a VerifiedRequestContext,
    /// Ids of the records this write stored (an id already stored is not a new turn).
    pub fresh: BTreeSet<String>,
}

/// What one call did. Every refusal is a registered code; none is swallowed.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct DeriveOutcome {
    /// New tasks enqueued.
    pub enqueued: u32,
    /// Pending ranges extended without a new task.
    pub extended: u32,
    /// Enqueues skipped because the tenant is at its pending limit.
    pub skipped_backlog: u32,
    /// Codes of every other refusal.
    pub refused: Vec<String>,
}

/// The ids `method` is about to store: a `MemoryRemember` whose record is not yet in the graph.
pub(crate) async fn fresh_memory_ids(
    state: &Arc<RwLock<ServerState>>,
    graph_name: &str,
    tenant: &str,
    method: &Method,
) -> BTreeSet<String> {
    let Method::MemoryRemember { request } = method else {
        return BTreeSet::new();
    };
    let current = state.read().await;
    let Some(entry) = current.registry.get(graph_name) else {
        return BTreeSet::new();
    };
    let id = request.record.memory_id.as_str();
    let stored = entry
        .core
        .get_node_properties(
            &medulla_h_graph_gateway::graph_ops::memory_surface::node_id(tenant, id),
        )
        .is_some();
    if stored || request.record.speaker.is_none() {
        BTreeSet::new()
    } else {
        BTreeSet::from([id.to_string()])
    }
}

/// Enqueue (or extend) the derive job of every key each fresh turn belongs to.
pub(crate) async fn enqueue_derivations(request: EnqueueRequest<'_>) -> DeriveOutcome {
    let mut outcome = DeriveOutcome::default();
    let tenant = request.verified_context.tenant().to_string();
    let (core, persist_dir) = {
        let current = request.state.read().await;
        (
            current
                .registry
                .get(request.graph_name)
                .map(|entry| entry.core.clone()),
            current.persist_dir.clone(),
        )
    };
    let Some(core) = core else {
        outcome.refused.push("GRAPH_UNKNOWN".to_string());
        return outcome;
    };
    let Some(jobs) = jobs_seam(request.state).await else {
        outcome.refused.push("QUEUE_UNAVAILABLE".to_string());
        return outcome;
    };
    let rule = DueRule {
        debounce_ms: knobs().memory_derive_debounce,
        batch_turns: u32::try_from(knobs().memory_derive_batch_turns).unwrap_or(u32::MAX),
    };
    let max_pending = knobs().memory_derive_max_pending;
    // The sessions the fresh records speak in.
    let sessions: BTreeSet<String> = request
        .fresh
        .iter()
        .filter_map(|id| {
            medulla_h_graph_gateway::graph_ops::memory_surface::session_of(&core, &tenant, id)
        })
        .collect();
    for session in sessions {
        let turns = session_turns(&core, &tenant, &session);
        let members: Vec<ParticipantId> = turns
            .iter()
            .map(|turn| turn.speaker.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let configs = observation_configs(&core, &tenant, &session, &members);
        let targets = derivation_targets(&DerivationInput {
            scope_id: "",
            session_id: &session,
            members: &members,
            configs: &configs,
        });
        for turn in turns
            .iter()
            .filter(|turn| request.fresh.contains(&turn.memory_id))
        {
            for key in targets.iter().filter(|key| key.subject == turn.speaker) {
                let node_id = payload_node_id(&tenant, key);
                let pending = pending_payload(
                    &jobs,
                    &persist_dir,
                    &tenant,
                    read_payload(&core, &tenant, &node_id),
                );
                let payload = match pending {
                    Ok(Some(existing)) => {
                        outcome.extended += 1;
                        extend_payload(&existing, &turn.memory_id, turn.timestamp_ms, rule)
                    }
                    Ok(None) => {
                        match start_range(
                            &jobs,
                            &persist_dir,
                            &tenant,
                            request.verified_context,
                            key,
                            &node_id,
                            turn,
                            rule,
                            max_pending,
                        ) {
                            Ok(Some(payload)) => {
                                outcome.enqueued += 1;
                                payload
                            }
                            Ok(None) => {
                                outcome.skipped_backlog += 1;
                                tracing::warn!(
                                    code = DERIVE_BACKLOG_FULL,
                                    tenant = %tenant,
                                    "derive enqueue skipped; the turn is committed"
                                );
                                continue;
                            }
                            Err(code) => {
                                outcome.refused.push(code);
                                continue;
                            }
                        }
                    }
                    Err(code) => {
                        outcome.refused.push(code);
                        continue;
                    }
                };
                if let Err(code) = write_payload(&request, &tenant, &node_id, &payload).await {
                    outcome.refused.push(code);
                }
            }
        }
    }
    outcome
}

/// The stored payload when its task is still waiting for a worker.
fn pending_payload(
    jobs: &Arc<dyn JobsApi>,
    persist_dir: &Option<String>,
    tenant: &str,
    stored: Option<DerivePayload>,
) -> Result<Option<DerivePayload>, String> {
    let Some(payload) = stored else {
        return Ok(None);
    };
    let pending = jobs.task_pending(persist_dir, tenant, &payload.task_id)?;
    Ok(pending.then_some(payload))
}

/// Enqueue the task of a new range and return its payload; `None` when the tenant is at its
/// pending limit. The task is idempotent on the key and the range's first turn, so a retry after
/// a crash between this call and the payload write names the same task.
#[allow(clippy::too_many_arguments)]
fn start_range(
    jobs: &Arc<dyn JobsApi>,
    persist_dir: &Option<String>,
    tenant: &str,
    verified: &VerifiedRequestContext,
    key: &DeriveKey,
    node_id: &str,
    turn: &SessionTurn,
    rule: DueRule,
    max_pending: u64,
) -> Result<Option<DerivePayload>, String> {
    let (queued, _running, _oldest) = jobs.task_queue_load(persist_dir, tenant, DERIVE_QUEUE)?;
    if queued >= max_pending {
        return Ok(None);
    }
    let task_id = key_digest(key, &turn.memory_id);
    let kind = format!("derive/{}", key.session_id);
    let write = IngestTaskWrite {
        tenant,
        actor: verified.principal(),
        queue: DERIVE_QUEUE,
        kind: &kind,
        task_id: &task_id,
        payload: node_id,
    };
    jobs.enqueue_ingest_task(persist_dir, &write)?;
    Ok(Some(DerivePayload {
        key: key.clone(),
        first_turn_id: turn.memory_id.clone(),
        last_turn_id: turn.memory_id.clone(),
        turns: 1,
        task_id,
        last_turn_ms: turn.timestamp_ms,
        due_at_ms: due_at(turn.timestamp_ms, 1, rule),
    }))
}

/// Store the payload node through the same dispatch every graph write takes.
async fn write_payload(
    request: &EnqueueRequest<'_>,
    tenant: &str,
    node_id: &str,
    payload: &DerivePayload,
) -> Result<(), String> {
    let method: Method = payload_write(tenant, node_id, payload)?;
    let response = Box::pin(super::dispatch_graph_op(
        request.state,
        request.graph_name,
        request.req_id,
        request.caller,
        request.verified_context,
        method,
    ))
    .await;
    response.refusal_text().map_or(Ok(()), Err)
}
