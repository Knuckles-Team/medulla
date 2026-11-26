//! The participant record: who or what takes part in a session
//! (MEDULLA-DECISION-ENGINE-R617.7).
//!
//! A participant is a person, an agent, or an object, named inside one workspace by a slug. A
//! participant of kind `person` or `agent` names the `Person` or `Agent` node it represents; an
//! object stands for nothing else. The record is the Rust type of the core class `:Participant`
//! (`core-foundation-v1.ttl`); the Python client mirrors it by hand against the round-trip vectors
//! of the tests below. Pure data: nothing here reads a store or a clock.

use serde::{Deserialize, Serialize};

use medulla_types::contract::{ResourceId, TenantId};

use crate::memory::{identifier, tenant_id, MemoryError};

/// The schema version a [`Participant`] is written under.
pub const PARTICIPANT_SCHEMA_VERSION: u16 = 1;
/// The prefix of a participant's stable IRI.
pub const PARTICIPANT_IRI_PREFIX: &str = "urn:medulla:participant:";
/// The node type of a stored participant.
pub const PARTICIPANT_NODE_TYPE: &str = "Participant";
/// Most characters of a participant slug.
pub const MAX_SLUG_LEN: usize = 64;

fn invalid(detail: impl Into<String>) -> MemoryError {
    MemoryError::ParticipantInvalid(detail.into())
}

/// A participant's slug inside its workspace: 1 to 64 characters of `a-z`, `0-9` and `-`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ParticipantId(String);

impl ParticipantId {
    /// Validate `slug`.
    pub fn new(slug: impl Into<String>) -> Result<Self, MemoryError> {
        let slug = slug.into();
        let valid = !slug.is_empty()
            && slug.len() <= MAX_SLUG_LEN
            && slug.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if !valid {
            return Err(invalid(format!(
                "participant_id `{slug}` is not 1 to {MAX_SLUG_LEN} characters of a-z, 0-9 and -"
            )));
        }
        Ok(Self(slug))
    }

    /// The slug.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ParticipantId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let slug = String::deserialize(deserializer)?;
        Self::new(slug).map_err(serde::de::Error::custom)
    }
}

/// What a participant is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ParticipantKind {
    Person,
    Agent,
    Object,
}

impl ParticipantKind {
    /// The value of the `:participantKind` property.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Agent => "agent",
            Self::Object => "object",
        }
    }

    /// Whether a participant of this kind must name the node it represents.
    pub const fn represents_someone(self) -> bool {
        matches!(self, Self::Person | Self::Agent)
    }
}

/// One participant of a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Participant {
    pub participant_id: ParticipantId,
    pub tenant: TenantId,
    pub workspace: ResourceId,
    pub kind: ParticipantKind,
    pub display_name: String,
    /// The `Person` or `Agent` node the participant stands for; required for those two kinds.
    pub represents: Option<String>,
    /// The principals that may read as this participant.
    pub acts_as: Vec<String>,
    pub schema_version: u16,
}

impl Participant {
    /// Build a participant: the identifiers are validated, a person or agent names what it
    /// represents, and `display_name` is not empty.
    pub fn new(
        participant_id: &str,
        tenant: &str,
        workspace: &str,
        kind: ParticipantKind,
        display_name: &str,
        represents: Option<&str>,
        acts_as: Vec<String>,
    ) -> Result<Self, MemoryError> {
        let participant = Self {
            participant_id: ParticipantId::new(participant_id)?,
            tenant: tenant_id(tenant)?,
            workspace: identifier(workspace, "workspace")?,
            kind,
            display_name: display_name.to_string(),
            represents: represents.map(str::to_string),
            acts_as,
            schema_version: PARTICIPANT_SCHEMA_VERSION,
        };
        participant.validate()?;
        Ok(participant)
    }

    /// The rules every participant holds, the same two the `:ParticipantShape` states.
    pub fn validate(&self) -> Result<(), MemoryError> {
        if self.display_name.trim().is_empty() {
            return Err(invalid("a participant needs a display_name"));
        }
        if self.kind.represents_someone()
            && self.represents.as_deref().is_none_or(|node| node.trim().is_empty())
        {
            return Err(invalid(format!(
                "a participant of kind {} must name the node it represents",
                self.kind.as_str()
            )));
        }
        if self.acts_as.iter().any(|principal| principal.trim().is_empty()) {
            return Err(invalid("acts_as names no empty principal"));
        }
        Ok(())
    }

    /// The stable IRI: `urn:medulla:participant:<workspace>:<slug>`.
    pub fn iri(&self) -> String {
        participant_iri(&self.workspace, &self.participant_id)
    }
}

/// The stable IRI of a participant: `urn:medulla:participant:<workspace>:<slug>`.
pub fn participant_iri(workspace: &ResourceId, participant_id: &ParticipantId) -> String {
    format!("{PARTICIPANT_IRI_PREFIX}{}:{}", workspace.as_str(), participant_id.as_str())
}

/// The edge type from a transcript thread to a participant it holds
/// (MEDULLA-DECISION-ENGINE-R617.8).
pub const HAS_PARTICIPANT_EDGE: &str = "hasParticipant";
/// The edge property: when the participant joined the session, in milliseconds.
pub const ADDED_AT_PROPERTY: &str = "added_at_ms";

/// One speaker's place in one session (MEDULLA-DECISION-ENGINE-R617.8): the `hasParticipant` edge
/// from the session's transcript thread to the participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Membership {
    /// The conversation (session) the participant joined.
    pub session_id: String,
    /// The participant that joined.
    pub participant_id: ParticipantId,
    /// The time of the participant's first turn in the session.
    pub added_at_ms: i64,
}

/// The memberships a conversation implies: one per distinct speaker, in order of first appearance,
/// stamped with that speaker's first turn. A turn without a speaker adds none. Pure; O(messages).
pub fn membership_edges(conversation: &crate::memory::ConversationRecord) -> Vec<Membership> {
    let mut seen = std::collections::BTreeSet::new();
    let mut edges = Vec::new();
    for message in &conversation.messages {
        let Some(speaker) = &message.speaker else {
            continue;
        };
        if seen.insert(speaker.clone()) {
            edges.push(Membership {
                session_id: conversation.conversation_id.as_str().to_owned(),
                participant_id: speaker.clone(),
                added_at_ms: message.timestamp_ms,
            });
        }
    }
    edges
}

/// One served write of the participant surface (MEDULLA-DECISION-ENGINE-R617.9). Every operation
/// is a write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ParticipantOp {
    /// Create a participant, or update its name and `acts_as`.
    Put { participant: Participant },
    /// Add a participant to a session. Idempotent on the pair.
    AddMember {
        /// The session (conversation) the participant joins.
        session_id: String,
        /// The participant that joins.
        participant_id: ParticipantId,
    },
    /// Create or replace a memory scope (MEDULLA-DECISION-ENGINE-R651.9).
    PutScope { scope: crate::memory_scope::MemoryScope },
    /// Set a participant's observation flags in one session (MEDULLA-DECISION-ENGINE-R651.10).
    SetObservation { config: crate::memory_scope::ObservationConfig },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParticipantWire {
    participant_id: ParticipantId,
    tenant: TenantId,
    workspace: ResourceId,
    kind: ParticipantKind,
    display_name: String,
    #[serde(default)]
    represents: Option<String>,
    #[serde(default)]
    acts_as: Vec<String>,
    schema_version: u16,
}

impl<'de> Deserialize<'de> for Participant {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ParticipantWire::deserialize(deserializer)?;
        let participant = Self {
            participant_id: wire.participant_id,
            tenant: wire.tenant,
            workspace: wire.workspace,
            kind: wire.kind,
            display_name: wire.display_name,
            represents: wire.represents,
            acts_as: wire.acts_as,
            schema_version: wire.schema_version,
        };
        participant.validate().map_err(serde::de::Error::custom)?;
        Ok(participant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person() -> Participant {
        Participant::new(
            "ada-lovelace",
            "tenant-a",
            "ws-1",
            ParticipantKind::Person,
            "Ada Lovelace",
            Some("person:ada"),
            vec!["principal:ada".into()],
        )
        .unwrap()
    }

    // spec: MEDULLA-DECISION-ENGINE-R617.7
    #[test]
    fn participant_round_trips_and_iri_is_stable() {
        let participant = person();
        assert_eq!(participant.iri(), "urn:medulla:participant:ws-1:ada-lovelace");
        let wire = serde_json::to_value(&participant).unwrap();
        assert_eq!(wire["kind"], "person");
        assert_eq!(wire["participant_id"], "ada-lovelace");
        let decoded: Participant = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(decoded, participant);
        let packed = rmp_serde::to_vec_named(&participant).unwrap();
        assert_eq!(rmp_serde::from_slice::<Participant>(&packed).unwrap(), participant);

        // An unknown field is refused.
        let mut extra = wire.clone();
        extra["nickname"] = "countess".into();
        assert!(serde_json::from_value::<Participant>(extra).is_err());
        // A person or agent that represents nothing is refused; an object needs nothing.
        let mut bare = wire.clone();
        bare["represents"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<Participant>(bare).is_err());
        let object = Participant::new("whiteboard", "tenant-a", "ws-1", ParticipantKind::Object, "Whiteboard", None, vec![]);
        assert!(object.is_ok());
        assert!(Participant::new("a", "tenant-a", "ws-1", ParticipantKind::Agent, "Agent", None, vec![]).is_err());
    }

    // spec: MEDULLA-DECISION-ENGINE-R617.8
    #[test]
    fn membership_edges_dedupe_speakers() {
        use crate::memory::{ConversationMessage, ConversationRecord, MessageRole};
        let turn = |id: &str, speaker: Option<&str>, at: i64| ConversationMessage {
            message_id: ResourceId::new(id).unwrap(),
            role: MessageRole::User,
            provenance_key: ResourceId::new(id).unwrap(),
            timestamp_ms: at,
            text: "hello".to_string(),
            speaker: speaker.map(|s| ParticipantId::new(s).unwrap()),
        };
        let conversation = ConversationRecord::new(
            "tenant-a",
            "session-1",
            vec![
                turn("m1", Some("ada-1"), 10),
                turn("m2", None, 20),
                turn("m3", Some("bo-2"), 30),
                turn("m4", Some("ada-1"), 40),
            ],
        )
        .unwrap();
        let edges = membership_edges(&conversation);
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].participant_id.as_str(), "ada-1");
        assert_eq!(edges[0].added_at_ms, 10);
        assert_eq!(edges[1].participant_id.as_str(), "bo-2");
        assert!(edges.iter().all(|edge| edge.session_id == "session-1"));
        // The speaker travels onto each memory record.
        let records = conversation.to_memory_records().unwrap();
        assert_eq!(records[0].speaker.as_ref().map(ParticipantId::as_str), Some("ada-1"));
        assert!(records[1].speaker.is_none());
    }

    // spec: MEDULLA-DECISION-ENGINE-R617.7
    #[test]
    fn slugs_and_workspaces_are_validated() {
        for bad in ["", "Upper", "has space", "under_score", &"x".repeat(65)] {
            assert!(ParticipantId::new(bad).is_err(), "{bad:?}");
        }
        assert!(ParticipantId::new("a-1").is_ok());
        assert!(ParticipantId::new("x".repeat(64)).is_ok());
        assert!(Participant::new("ok", "tenant-a", "", ParticipantKind::Object, "x", None, vec![]).is_err());
        assert!(serde_json::from_str::<ParticipantId>("\"Bad Slug\"").is_err());
    }
}
