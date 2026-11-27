//! Memory events (MEDULLA-DECISION-ENGINE-R073.6).
//!
//! Three events leave participant memory: a derivation finished, a claim moved to a new state,
//! and a participant card changed. Each is written as a `MemoryEvent` graph node inside the
//! `BatchUpdate` that commits the change it announces; the batch compiler turns every such node
//! into one `MutationOutboxIntent` of the same batch, so the outbox row is atomic with the write.
//! This file holds the payload structs, the one topic table and the node operation. It reads no
//! store and no clock.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::claims::{ClaimState, ObserverRef};
use medulla_work_types::participant::ParticipantId;

/// The `type` property of the node that carries one memory event.
pub const MEMORY_EVENT_LABEL: &str = "MemoryEvent";
/// The consumer name the memory webhooks subscribe under.
pub const MEMORY_EVENTS_CONSUMER: &str = "memory-webhooks";
/// Topic of a finished derivation.
pub const TOPIC_DERIVATION_COMPLETED: &str = "memory.derivation.completed";
/// Topic of a claim transition.
pub const TOPIC_CLAIM_CHANGED: &str = "memory.claim.changed";
/// Topic of a card change.
pub const TOPIC_CARD_UPDATED: &str = "memory.card.updated";
/// The three topics, in one table; `Pull` accepts only these.
pub const MEMORY_EVENT_TOPICS: [&str; 3] =
    [TOPIC_DERIVATION_COMPLETED, TOPIC_CLAIM_CHANGED, TOPIC_CARD_UPDATED];

/// A derivation job finished and advanced its checkpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DerivationCompleted {
    /// The memory scope the derivation ran under; empty over the whole view.
    pub scope_id: String,
    /// The session the derivation ran in.
    pub session_id: String,
    /// The participant the conclusions are about.
    pub subject: ParticipantId,
    /// Whose view the conclusions belong to.
    pub observer: ObserverRef,
    /// How many conclusions the worker proposed.
    pub claims_written: u32,
    /// The last turn the checkpoint advanced to.
    pub last_turn_id: String,
}

/// A claim moved from one lifecycle state to another.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClaimChanged {
    /// The id of the claim that moved.
    pub claim_id: String,
    /// The state the claim left.
    pub from: ClaimState,
    /// The state the claim entered.
    pub to: ClaimState,
    /// The participant the claim is about.
    pub subject: ParticipantId,
    /// Whose view the claim belongs to.
    pub observer: ObserverRef,
}

/// A participant card was recomputed and its digest changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CardUpdated {
    /// The participant the card describes.
    pub subject: ParticipantId,
    /// Whose view the card belongs to.
    pub observer: ObserverRef,
    /// SHA-256 of the card, as `sha256:<hex>`.
    pub card_digest: String,
}

/// One memory event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event_kind", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum MemoryEvent {
    /// A derivation job finished.
    DerivationCompleted(DerivationCompleted),
    /// A claim changed state.
    ClaimChanged(ClaimChanged),
    /// A card was recomputed.
    CardUpdated(CardUpdated),
}

impl MemoryEvent {
    /// The one topic table: the outbox topic of this event.
    pub fn topic(&self) -> &'static str {
        match self {
            MemoryEvent::DerivationCompleted(_) => TOPIC_DERIVATION_COMPLETED,
            MemoryEvent::ClaimChanged(_) => TOPIC_CLAIM_CHANGED,
            MemoryEvent::CardUpdated(_) => TOPIC_CARD_UPDATED,
        }
    }

    /// The `event_kind` header value.
    pub fn event_kind(&self) -> &'static str {
        match self {
            MemoryEvent::DerivationCompleted(_) => "derivation_completed",
            MemoryEvent::ClaimChanged(_) => "claim_changed",
            MemoryEvent::CardUpdated(_) => "card_updated",
        }
    }

    /// The outbox key: the subject id.
    pub fn key(&self) -> &str {
        match self {
            MemoryEvent::DerivationCompleted(event) => event.subject.as_str(),
            MemoryEvent::ClaimChanged(event) => event.subject.as_str(),
            MemoryEvent::CardUpdated(event) => event.subject.as_str(),
        }
    }

    /// The payload struct as a JSON value (the `event_kind` tag is a header, not payload).
    pub fn payload(&self) -> Result<serde_json::Value, String> {
        let value = match self {
            MemoryEvent::DerivationCompleted(event) => serde_json::to_value(event),
            MemoryEvent::ClaimChanged(event) => serde_json::to_value(event),
            MemoryEvent::CardUpdated(event) => serde_json::to_value(event),
        };
        value.map_err(|error| format!("memory event encoding failed: {error}"))
    }

    /// The outbox intent of this event for `tenant`: the topic, the subject id as key, the msgpack
    /// payload, and the `tenant` and `event_kind` headers.
    pub fn outbox_intent(
        &self,
        tenant: &str,
    ) -> Result<medulla_mutation_types::mutation_scope::MutationOutboxIntent, String> {
        Ok(medulla_mutation_types::mutation_scope::MutationOutboxIntent {
            topic: self.topic().to_string(),
            key: self.key().to_string(),
            payload: rmp_serde::to_vec_named(&self.payload()?)
                .map_err(|error| format!("memory event intent encoding failed: {error}"))?,
            headers: std::collections::BTreeMap::from([
                ("tenant".to_string(), tenant.to_string()),
                ("event_kind".to_string(), self.event_kind().to_string()),
            ]),
        })
    }

    /// The id of the node that carries this event. It is a digest of the tenant, topic, key and
    /// payload, so committing the same event twice names one node (an idempotent replay), and a
    /// later, different change of the same claim names another.
    pub fn node_id(&self, tenant: &str, at_ms: u64) -> Result<String, String> {
        let mut hasher = Sha256::new();
        for part in [tenant, self.topic(), self.key(), &self.payload()?.to_string(), &at_ms.to_string()] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        Ok(format!("memory-event:{}", hex::encode(hasher.finalize())))
    }

    /// The outbox intent of a claim-store row: [`Self::outbox_intent`] plus the `graph` header
    /// that names the graph the event is published into (MEDULLA-DECISION-ENGINE-R073.6).
    pub fn outbox_intent_in(
        &self,
        tenant: &str,
        graph: &str,
    ) -> Result<medulla_mutation_types::mutation_scope::MutationOutboxIntent, String> {
        let mut intent = self.outbox_intent(tenant)?;
        intent.headers.insert("graph".to_string(), graph.to_string());
        Ok(intent)
    }

    /// The id of the node that carries the event of one outbox row of the claim store. The row's
    /// batch id and ordinal name it, so a republish after a crash (the node was written, the row
    /// was not yet acknowledged) names the same node.
    pub fn row_node_id(tenant: &str, topic: &str, batch_id: &str, ordinal: u32) -> String {
        let mut hasher = Sha256::new();
        for part in [tenant, topic, batch_id, &ordinal.to_string()] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        format!("memory-event:{}", hex::encode(hasher.finalize()))
    }

    /// The `BatchUpdate` operation that stores this event. `at_ms` is the commit time the caller
    /// read from the one authoritative clock.
    pub fn node_operation(&self, tenant: &str, at_ms: u64) -> Result<serde_json::Value, String> {
        self.node_operation_with_id(self.node_id(tenant, at_ms)?, tenant, at_ms)
    }

    /// The operation that stores the event of one claim-store outbox row, under
    /// [`Self::row_node_id`].
    pub fn row_node_operation(
        &self,
        tenant: &str,
        at_ms: u64,
        batch_id: &str,
        ordinal: u32,
    ) -> Result<serde_json::Value, String> {
        self.node_operation_with_id(Self::row_node_id(tenant, self.topic(), batch_id, ordinal), tenant, at_ms)
    }

    fn node_operation_with_id(
        &self,
        id: String,
        tenant: &str,
        at_ms: u64,
    ) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({
            "op": "upsert_node",
            "id": id,
            "properties": {
                "type": MEMORY_EVENT_LABEL,
                "tenant_scope": tenant,
                "topic": self.topic(),
                "key": self.key(),
                "event_kind": self.event_kind(),
                "at_ms": at_ms,
                "json": self.payload()?.to_string(),
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(slug: &str) -> ParticipantId {
        ParticipantId::new(slug).unwrap()
    }

    // spec: MEDULLA-DECISION-ENGINE-R073.6
    #[test]
    fn each_event_has_one_topic_key_and_a_stable_node() {
        let events = [
            MemoryEvent::DerivationCompleted(DerivationCompleted {
                scope_id: String::new(),
                session_id: "s1".into(),
                subject: id("ada"),
                observer: ObserverRef::System,
                claims_written: 2,
                last_turn_id: "m9".into(),
            }),
            MemoryEvent::ClaimChanged(ClaimChanged {
                claim_id: "c1".into(),
                from: ClaimState::Proposed,
                to: ClaimState::Supported,
                subject: id("ada"),
                observer: ObserverRef::Participant(id("bo")),
            }),
            MemoryEvent::CardUpdated(CardUpdated {
                subject: id("ada"),
                observer: ObserverRef::System,
                card_digest: "sha256:ab".into(),
            }),
        ];
        let topics: Vec<&str> = events.iter().map(MemoryEvent::topic).collect();
        assert_eq!(topics, MEMORY_EVENT_TOPICS);
        for event in &events {
            assert_eq!(event.key(), "ada");
            let first = event.node_operation("t1", 5).unwrap();
            assert_eq!(first, event.node_operation("t1", 5).unwrap());
            assert_ne!(first["id"], event.node_operation("t2", 5).unwrap()["id"]);
            assert_ne!(first["id"], event.node_operation("t1", 6).unwrap()["id"]);
            assert_eq!(first["properties"]["type"], MEMORY_EVENT_LABEL);
            assert_eq!(first["properties"]["topic"], event.topic());
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R073.6
    #[test]
    fn an_outbox_row_names_one_node_whatever_the_publish_time() {
        let event = MemoryEvent::ClaimChanged(ClaimChanged {
            claim_id: "c1".into(),
            from: ClaimState::Proposed,
            to: ClaimState::Supported,
            subject: id("ada"),
            observer: ObserverRef::System,
        });
        let first = event.row_node_operation("t1", 5, "batch-a", 1).unwrap();
        let later = event.row_node_operation("t1", 900, "batch-a", 1).unwrap();
        assert_eq!(first["id"], later["id"]);
        assert_ne!(first["id"], event.row_node_operation("t1", 5, "batch-a", 2).unwrap()["id"]);
        assert_ne!(first["id"], event.row_node_operation("t1", 5, "batch-b", 1).unwrap()["id"]);
        assert_ne!(first["id"], event.row_node_operation("t2", 5, "batch-a", 1).unwrap()["id"]);
        let intent = event.outbox_intent_in("t1", "g1").unwrap();
        assert_eq!(intent.headers["graph"], "g1");
        assert_eq!(intent.headers["tenant"], "t1");
    }
}
