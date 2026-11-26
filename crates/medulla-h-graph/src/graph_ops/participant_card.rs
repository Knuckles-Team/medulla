//! The stored participant card (MEDULLA-DECISION-ENGINE-R655.3).
//!
//! A card is one node per subject and observer, keyed by the verified tenant scope. The ranking is
//! pure (`medulla_decision_stats::decision::participant_card`); this file holds the node: where it
//! lives, how it is read, and the one batch that replaces it together with its `memory.card.updated`
//! event. The claims a card ranks live in the claim store, which `medulla-h-decide` reads, so that
//! crate computes the card and the router commits these operations.

use super::*;
use medulla_decision_types::claims::ObserverRef;
use medulla_decision_types::memory_events::{CardUpdated, MemoryEvent};
use medulla_decision_types::participant_memory::ParticipantCard;
use medulla_isolation::profile_access::read_stored;
use medulla_work_types::participant::ParticipantId;

/// The `type` property of a stored card.
pub const CARD_LABEL: &str = "ParticipantCard";

/// The graph node id of the card of `subject` as `observer` sees it.
pub fn card_node_id(tenant: &str, subject: &ParticipantId, observer: &ObserverRef) -> String {
    format!("participant_card:{tenant}:{}:{}", subject.as_str(), observer.as_property())
}

/// The stored card, when the tenant holds one.
pub fn read_card(
    core: &GraphCore,
    tenant: &str,
    subject: &ParticipantId,
    observer: &ObserverRef,
) -> Option<ParticipantCard> {
    read_stored(core, &card_node_id(tenant, subject, observer), CARD_LABEL, tenant)
}

/// The operations that replace the stored card with `card` and announce it: the card node and one
/// `memory.card.updated` event node, for one `BatchUpdate`. `None` when the stored card already has
/// the same digest, so an unchanged card writes and announces nothing.
pub fn refresh_card(
    core: &GraphCore,
    tenant: &str,
    card: &ParticipantCard,
    at_ms: u64,
) -> Result<Option<Vec<serde_json::Value>>, String> {
    if read_card(core, tenant, &card.subject, &card.observer).is_some_and(|stored| stored.digest == card.digest) {
        return Ok(None);
    }
    let json = serde_json::to_string(card).map_err(|error| format!("card encoding failed: {error}"))?;
    let event = MemoryEvent::CardUpdated(CardUpdated {
        subject: card.subject.clone(),
        observer: card.observer.clone(),
        card_digest: card.digest.clone(),
    });
    Ok(Some(vec![
        serde_json::json!({
            "op": "upsert_node",
            "id": card_node_id(tenant, &card.subject, &card.observer),
            "properties": { "type": CARD_LABEL, "tenant_scope": tenant, "json": json },
        }),
        event.node_operation(tenant, at_ms)?,
    ]))
}
