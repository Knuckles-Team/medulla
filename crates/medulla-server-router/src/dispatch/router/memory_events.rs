//! Publishing memory events after the change commits (MEDULLA-DECISION-ENGINE-R073.6).
//!
//! The claim store commits a lifecycle transition together with its own outbox record. Only after
//! that commit does the router write the event to the graph outbox, as one `MemoryEvent` node in
//! a `BatchUpdate`; the batch compiler turns the node into the outbox intent of the same batch,
//! so the graph row and the event are atomic. An event is never sent before the claim commits.
//! A publish failure is logged under its code and leaves the committed claim as it is: the claim
//! store's own outbox record still holds the event.

use super::*;
use medulla_decision_types::memory_events::MemoryEvent;

/// Write `events` to the graph outbox of `graph`.
pub(super) async fn publish_memory_events(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    req_id: u64,
    verified: &VerifiedRequestContext,
    events: &[MemoryEvent],
) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }
    let at_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let operations = events
        .iter()
        .map(|event| event.node_operation(verified.tenant(), at_ms))
        .collect::<Result<Vec<_>, _>>()?;
    let operations_msgpack = rmp_serde::to_vec_named(&operations)
        .map_err(|error| format!("memory event batch encoding failed: {error}"))?;
    let response = dispatch_graph_op(
        state,
        graph,
        req_id,
        Some(verified.agent_id()),
        verified,
        Method::BatchUpdate { operations_msgpack },
    )
    .await;
    response.refusal_text().map_or(Ok(()), Err)
}

/// Serve `Method::Claims` and announce the committed transitions.
pub(super) async fn serve_claims_and_publish(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    req_id: u64,
    verified: &VerifiedRequestContext,
    op: medulla_decision_types::claims::ClaimOp,
) -> Response {
    let served =
        medulla_h_decide::decide::handle_claims_planned(state, req_id, graph, verified, op).await;
    if served.response.refusal_text().is_none() {
        if let Err(error) = publish_memory_events(state, graph, req_id, verified, &served.events).await {
            tracing::error!(code = "MEMORY_EVENT_PUBLISH_FAILED", %error, "memory event publish failed after the claim committed");
        }
    }
    served.response
}
