//! Publishing the claim store's memory events into the graph outbox
//! (MEDULLA-DECISION-ENGINE-R073.6).
//!
//! A claim transition commits one `memory.claim.changed` outbox row in the claim store's own
//! transaction. This publisher is the only reader of those rows: it leases each one under one
//! consumer name, writes the event as a `MemoryEvent` node of the graph the row names, and only
//! then acknowledges the row. The graph write is one engine-internal batch whose id is derived from
//! the row, so a crash between the graph commit and the acknowledgement republishes the same
//! batch, which replays and writes nothing twice. The router calls the publisher after every claim
//! commit and the leader tick calls it on every pass, so a row whose first publish never happened
//! (a crash after the claim commit) is published by the next tick.

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_decision_types::memory_events::{ClaimChanged, MemoryEvent, TOPIC_CLAIM_CHANGED};
use medulla_mutation_types::mutation_scope::MutationOutboxLease;
use medulla_protocol_types::protocol::{Method, ResultPayload};
use medulla_server_api::outbox_operator::OutboxClaimBudget;
use medulla_server_api::state::ServerState;
use medulla_statechart::StatechartStore;

/// The consumer name under which the publisher reads the claim store's outbox.
pub const CONSUMER: &str = "memory-event-publisher";
/// The principal the publisher's graph writes are attributed to.
const PRINCIPAL: &str = "memory-events";
/// Rows leased by one claim call.
const CLAIM_LIMIT: u32 = 64;
/// How long a leased row waits before it is offered again.
const LEASE_MS: u64 = 30_000;
/// The most claim calls one publish makes, so one pass cannot run unbounded.
const MAX_CLAIMS_PER_PASS: usize = 64;

/// Publish every pending `memory.claim.changed` row of the claim store and return how many rows
/// were acknowledged. A row that cannot be published stays leased and is offered again after the
/// lease, so the error leaves the stream where it was.
pub async fn publish_claim_events(state: &Arc<RwLock<ServerState>>) -> Result<usize, String> {
    let persist_dir = { state.read().await.persist_dir.clone() };
    let store = crate::statechart::statechart_store(persist_dir.as_deref())?;
    store
        .outbox_subscribe(CONSUMER, TOPIC_CLAIM_CHANGED)
        .map_err(|error| error.to_string())?;
    let mut acknowledged = 0;
    for _ in 0..MAX_CLAIMS_PER_PASS {
        let now_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
        let mut budget = OutboxClaimBudget::new(CLAIM_LIMIT, LEASE_MS, now_ms)?;
        let outcome = store
            .outbox_claim(CONSUMER, &mut budget)
            .map_err(|error| error.to_string())?;
        if outcome.claims.is_empty() {
            break;
        }
        for lease in outcome.claims {
            publish_row(state, &store, &lease, now_ms).await?;
            acknowledged += 1;
        }
        if !outcome.more_available {
            break;
        }
    }
    Ok(acknowledged)
}

/// Write the event of one leased row to its graph, then acknowledge the row.
async fn publish_row(
    state: &Arc<RwLock<ServerState>>,
    store: &Arc<StatechartStore>,
    lease: &MutationOutboxLease,
    now_ms: u64,
) -> Result<(), String> {
    let record = &lease.record;
    let headers = &record.intent.headers;
    let (Some(tenant), Some(graph)) = (headers.get("tenant"), headers.get("graph")) else {
        // A row with no tenant or graph can never be published; skipping it keeps the ordered
        // stream moving, and the code names it.
        eprintln!(
            "MEMORY_EVENT_ROW_UNPUBLISHABLE: outbox row {}:{} names no tenant or graph",
            record.batch_id, record.ordinal
        );
        return acknowledge(store, lease, now_ms);
    };
    let changed: ClaimChanged = rmp_serde::from_slice::<serde_json::Value>(&record.intent.payload)
        .map_err(|error| error.to_string())
        .and_then(|value| serde_json::from_value(value).map_err(|error| error.to_string()))
        .map_err(|error| format!("MEMORY_EVENT_ROW_UNREADABLE: {error}"))?;
    let event = MemoryEvent::ClaimChanged(changed);
    let node = event.row_node_operation(tenant, now_ms, &record.batch_id, record.ordinal)?;
    let node_id = node["id"].as_str().unwrap_or_default().to_string();
    let (core, persistence) = {
        let server = state.read().await;
        (
            server.registry.get(graph).map(|entry| entry.core.clone()),
            server.persistence.clone(),
        )
    };
    let core = core.ok_or_else(|| format!("MEMORY_EVENT_GRAPH_UNAVAILABLE: graph {graph} is not served"))?;
    let operations_msgpack = rmp_serde::to_vec_named(&vec![node])
        .map_err(|error| format!("memory event batch encoding failed: {error}"))?;
    let batch_id =
        medulla_server_api::batch_compile::opaque_coordinator_key("memory-event", graph, &node_id);
    medulla_server_mutation::mutation_batch::commit_internal_graph_methods_with_nonce(
        medulla_server_mutation::mutation_batch::InternalGraphCommitRequest::new(
            persistence.as_ref(),
            &core,
            medulla_server_mutation::mutation_batch::CommitOrigin {
                request_id: 0,
                principal: Some(PRINCIPAL),
            },
            graph,
            &batch_id,
            vec![Method::BatchUpdate { operations_msgpack }],
            &ResultPayload::Bool(true),
        )
        .with_tenant_scope(tenant),
    )
    .await
    .map_err(|error| format!("MEMORY_EVENT_PUBLISH_FAILED: {error}"))?;
    acknowledge(store, lease, now_ms)
}

fn acknowledge(store: &Arc<StatechartStore>, lease: &MutationOutboxLease, now_ms: u64) -> Result<(), String> {
    store.outbox_ack(lease, now_ms).map_err(|error| error.to_string())
}
