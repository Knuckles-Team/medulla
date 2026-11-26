//! Publishing memory events after the change commits (MEDULLA-DECISION-ENGINE-R073.6).
//!
//! The claim store commits a lifecycle transition together with its own outbox row. After that
//! commit the router asks the memory event publisher (`medulla_h_jobs::memory_event_publisher`),
//! the one reader of those rows, to write them to the graph outbox. The publisher acknowledges a
//! row only after its graph write, and the leader tick runs it too, so a row whose publish was
//! interrupted by a crash is published by the next tick. An event is never sent before the claim
//! commits. A publish failure is logged under its code and leaves the committed claim as it is.

use super::*;
use medulla_decision_types::memory_events::MemoryEvent;

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
        #[cfg(feature = "statechart")]
        if !served.events.is_empty() {
            if let Err(error) = medulla_h_jobs::memory_event_publisher::publish_claim_events(state).await {
                tracing::error!(code = "MEMORY_EVENT_PUBLISH_FAILED", %error, "memory event publish failed after the claim committed");
            }
        }
        // The card of every perspective a claim changed is refreshed once, after that change.
        if let Err(error) = refresh_cards(state, graph, req_id, verified, &served.events).await {
            tracing::error!(code = "PARTICIPANT_CARD_REFRESH_FAILED", %error, "card refresh failed after the claim committed");
        }
    }
    served.response
}

/// Recompute and store the card of each subject and observer a `ClaimChanged` event names, once
/// per pair (MEDULLA-DECISION-ENGINE-R655.3). A card whose digest did not change writes nothing.
async fn refresh_cards(
    state: &Arc<RwLock<ServerState>>,
    graph: &str,
    req_id: u64,
    verified: &VerifiedRequestContext,
    events: &[MemoryEvent],
) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    let at_ms = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    for event in events {
        let MemoryEvent::ClaimChanged(changed) = event else {
            continue;
        };
        if !seen.insert((changed.subject.clone(), changed.observer.clone())) {
            continue;
        }
        let planned = medulla_h_decide::decide::plan_card_refresh(
            state,
            graph,
            verified,
            &changed.subject,
            &changed.observer,
            at_ms,
        )
        .await?;
        let Some(operations) = planned else {
            continue;
        };
        let operations_msgpack = rmp_serde::to_vec_named(&operations)
            .map_err(|error| format!("card batch encoding failed: {error}"))?;
        let response = dispatch_graph_op(
            state,
            graph,
            req_id,
            Some(verified.agent_id()),
            verified,
            Method::BatchUpdate { operations_msgpack },
        )
        .await;
        if let Some(refusal) = response.refusal_text() {
            return Err(refusal);
        }
    }
    Ok(())
}
