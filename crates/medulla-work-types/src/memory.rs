//! Engine-held memory records (MEDULLA-DECISION-ENGINE-R617.1).
//!
//! The model layer of engine-held memory: the conversation record a chat log
//! parses into, the layered project rules, the retention policy that prunes
//! memory with a legal-hold exception, and the context budget one context
//! request carries. Parsing, layered loading, pruning and the smallest
//! covering-set optimizer are later children (R617.2 onwards); nothing here
//! reads a clock or a store.
//!
//! This `RetentionPolicy` prunes engine-held memory. It is distinct from
//! [`crate::retention_policy::RetentionPolicy`], which drives the durable
//! node-cleanup path (MEDULLA-DURABLE-KERNEL-R700.1).

use serde::{Deserialize, Serialize};

use sha2::{Digest, Sha256};

use medulla_types::{contract::Digest256, contract::ResourceId, contract::TenantId};

/// The largest number of nodes one memory prune step may remove.
pub const MAX_MEMORY_PRUNE_STEP: u32 = 10_000;

/// A refused engine-held-memory record.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MemoryError {
    #[error("MEMORY_INVALID_IDENTIFIER: {0}")]
    InvalidIdentifier(String),
    #[error("MEMORY_REPEATED_MESSAGE: {0}")]
    RepeatedMessage(String),
    #[error("MEMORY_POLICY_INVALID: {0}")]
    InvalidPolicy(String),
    #[error("MEMORY_BUDGET_INVALID: {0}")]
    InvalidBudget(String),
}

fn identifier<T: Into<String>>(value: T, field: &str) -> Result<ResourceId, MemoryError> {
    ResourceId::new(value)
        .map_err(|detail| MemoryError::InvalidIdentifier(format!("{field}: {detail}")))
}

fn tenant_id(value: impl Into<String>) -> Result<TenantId, MemoryError> {
    TenantId::new(value)
        .map_err(|detail| MemoryError::InvalidIdentifier(format!("tenant: {detail}")))
}

/// Who spoke an engine-held conversation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

/// One ordered, provenance-keyed message of an engine-held conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ConversationMessage {
    pub message_id: ResourceId,
    pub role: MessageRole,
    /// The key the message's provenance is read back under.
    pub provenance_key: ResourceId,
    /// Milliseconds since the Unix epoch.
    pub timestamp_ms: i64,
    /// The message text.
    pub text: String,
}

/// A chat log parsed into ordered, provenance-keyed messages; the ingest
/// path stores them as transcript threads (MEDULLA-INGEST-WORKER-R003.13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ConversationRecord {
    pub tenant: TenantId,
    pub conversation_id: ResourceId,
    /// The messages in source order.
    pub messages: Vec<ConversationMessage>,
}

impl ConversationRecord {
    /// The record is ordered: the same message id never appears twice.
    pub fn new(
        tenant: impl Into<String>,
        conversation_id: impl Into<String>,
        messages: Vec<ConversationMessage>,
    ) -> Result<Self, MemoryError> {
        let tenant = tenant_id(tenant)?;
        let conversation_id = identifier(conversation_id, "conversation_id")?;
        let mut seen = std::collections::BTreeSet::new();
        for message in &messages {
            if !seen.insert(&message.message_id) {
                return Err(MemoryError::RepeatedMessage(
                    message.message_id.as_str().to_owned(),
                ));
            }
        }
        Ok(Self {
            tenant,
            conversation_id,
            messages,
        })
    }
}

/// One scoped fact a project rule layer attaches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ScopedFact {
    pub fact_id: ResourceId,
    /// The scope the fact attaches under.
    pub scope: ResourceId,
    /// The rule text.
    pub rule: String,
}

/// One layer of a project's rule files, loaded in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RuleLayer {
    pub layer_id: ResourceId,
    /// The rule file the layer loads from.
    pub source: ResourceId,
    /// The scoped facts the layer attaches.
    pub facts: Vec<ScopedFact>,
}

/// A project's rule files loaded layered and attached as scoped facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProjectRules {
    pub tenant: TenantId,
    pub project_id: ResourceId,
    /// The layers in load order: a later layer overrides an earlier one.
    pub layers: Vec<RuleLayer>,
}

/// A memory class the retention policy may prune.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum MemoryClass {
    Conversation,
    ProjectRule,
    MemoryNode,
}

/// The retention policy that prunes engine-held memory. A node under legal
/// hold is never pruned by this policy; the refusal and its log entry are
/// R617.4.
/// Deserialization goes through [`RetentionPolicy::new`], so a wire value
/// cannot carry a policy the constructor refuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RetentionPolicy {
    pub tenant: TenantId,
    pub policy_id: ResourceId,
    /// Memory older than this (milliseconds) is a prune candidate.
    pub min_age_ms: u64,
    /// The classes the policy prunes.
    pub classes: Vec<MemoryClass>,
    /// The largest number of nodes one prune step removes.
    pub max_per_step: u32,
}

impl RetentionPolicy {
    pub fn new(
        tenant: impl Into<String>,
        policy_id: impl Into<String>,
        min_age_ms: u64,
        classes: Vec<MemoryClass>,
        max_per_step: u32,
    ) -> Result<Self, MemoryError> {
        let tenant = tenant_id(tenant)?;
        let policy_id = identifier(policy_id, "policy_id")?;
        let mut classes = classes;
        classes.sort_unstable();
        classes.dedup();
        if min_age_ms == 0 {
            return Err(MemoryError::InvalidPolicy(
                "min_age_ms must be above zero".to_owned(),
            ));
        }
        if classes.is_empty() {
            return Err(MemoryError::InvalidPolicy(
                "a retention policy must prune at least one memory class".to_owned(),
            ));
        }
        if max_per_step == 0 || max_per_step > MAX_MEMORY_PRUNE_STEP {
            return Err(MemoryError::InvalidPolicy(format!(
                "max_per_step must be between 1 and {MAX_MEMORY_PRUNE_STEP}"
            )));
        }
        Ok(Self {
            tenant,
            policy_id,
            min_age_ms,
            classes,
            max_per_step,
        })
    }
}

/// The context budget one context request carries; the optimizer chooses
/// the smallest covering set within it (MEDULLA-DECISION-ENGINE-R045).
/// Deserialization goes through [`ContextBudget::new`], so a wire value
/// cannot carry a budget the constructor refuses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContextBudget {
    pub tenant: TenantId,
    pub budget_id: ResourceId,
    /// The token ceiling the covering set must stay within.
    pub max_tokens: u64,
    /// The largest number of items the covering set may hold.
    pub max_items: u32,
}

impl ContextBudget {
    pub fn new(
        tenant: impl Into<String>,
        budget_id: impl Into<String>,
        max_tokens: u64,
        max_items: u32,
    ) -> Result<Self, MemoryError> {
        let tenant = tenant_id(tenant)?;
        let budget_id = identifier(budget_id, "budget_id")?;
        if max_tokens == 0 {
            return Err(MemoryError::InvalidBudget(
                "max_tokens must be above zero".to_owned(),
            ));
        }
        if max_items == 0 {
            return Err(MemoryError::InvalidBudget(
                "max_items must be above zero".to_owned(),
            ));
        }
        Ok(Self {
            tenant,
            budget_id,
            max_tokens,
            max_items,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionPolicyWire {
    tenant: String,
    policy_id: String,
    min_age_ms: u64,
    classes: Vec<MemoryClass>,
    max_per_step: u32,
}

impl<'de> Deserialize<'de> for RetentionPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = RetentionPolicyWire::deserialize(deserializer)?;
        Self::new(
            wire.tenant,
            wire.policy_id,
            wire.min_age_ms,
            wire.classes,
            wire.max_per_step,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextBudgetWire {
    tenant: String,
    budget_id: String,
    max_tokens: u64,
    max_items: u32,
}

impl<'de> Deserialize<'de> for ContextBudget {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ContextBudgetWire::deserialize(deserializer)?;
        Self::new(wire.tenant, wire.budget_id, wire.max_tokens, wire.max_items)
            .map_err(serde::de::Error::custom)
    }
}

/// What one engine-held memory record is (MEDULLA-DECISION-ENGINE-R651.1). The
/// discriminator is closed; `SearchState` (R149.1) and `SessionNote` (R150.1)
/// carry the typed bodies below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum MemoryKind {
    Fact,
    Episode,
    ConversationTurn,
    Rule,
    SearchState,
    SessionNote,
}

/// A search run's state, saved so it survives context eviction
/// (MEDULLA-DECISION-ENGINE-R149.1). The four arrays hold opaque JSON values;
/// medulla owns no item schema inside them. Digests are derived on read, never
/// stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SearchStateEnvelope {
    pub search_run_id: String,
    pub state_revision: u64,
    pub transcript: Vec<serde_json::Value>,
    pub evidence: Vec<serde_json::Value>,
    pub constraints: Vec<serde_json::Value>,
    pub open_claims: Vec<serde_json::Value>,
}

/// SHA-256 of the canonical JSON of an array.
fn array_digest(values: &[serde_json::Value]) -> Digest256 {
    let bytes = serde_json::to_vec(values).unwrap_or_default();
    Digest256::from_bytes(Sha256::digest(bytes).into())
}

impl SearchStateEnvelope {
    pub fn transcript_digest(&self) -> Digest256 {
        array_digest(&self.transcript)
    }
    pub fn evidence_digest(&self) -> Digest256 {
        array_digest(&self.evidence)
    }
    pub fn constraints_digest(&self) -> Digest256 {
        array_digest(&self.constraints)
    }
    pub fn open_claims_digest(&self) -> Digest256 {
        array_digest(&self.open_claims)
    }
}

/// One session note (MEDULLA-DECISION-ENGINE-R150.1). The tenant comes from
/// the verified request, so it is not a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SessionNoteBody {
    pub schema_version: u16,
    pub note_id: String,
    pub run_id: String,
    pub session_id: String,
    pub task_id: String,
    pub user_id: String,
    pub input_digest: Digest256,
    pub output_digest: Digest256,
    pub source_revision: u64,
    pub content: String,
}

impl SessionNoteBody {
    /// `user_id` is non-empty.
    pub fn checked(self) -> Result<Self, MemoryError> {
        if self.user_id.trim().is_empty() {
            return Err(MemoryError::InvalidIdentifier("user_id: empty".into()));
        }
        Ok(self)
    }

    /// A note is content-addressed by its record digest.
    pub fn record_digest(&self) -> Digest256 {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        Digest256::from_bytes(Sha256::digest(bytes).into())
    }
}

/// How a recalled note is flagged (MEDULLA-DECISION-ENGINE-R150.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct NoteView {
    pub note_id: String,
    pub revision: u64,
    /// Relevance in `[0, 10000]`.
    pub relevance_q4: u16,
    pub stale: bool,
    pub archived: bool,
    pub conflict: bool,
    pub applied: bool,
}

impl NoteView {
    pub fn checked(self) -> Result<Self, MemoryError> {
        if self.relevance_q4 > 10_000 {
            return Err(MemoryError::InvalidBudget(
                "relevance_q4 exceeds 10000".into(),
            ));
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(id: &str) -> ConversationMessage {
        ConversationMessage {
            message_id: ResourceId::new(id).unwrap(),
            role: MessageRole::Assistant,
            provenance_key: ResourceId::new(format!("prov:{id}")).unwrap(),
            timestamp_ms: 1_700_000_000_000,
            text: "hello".into(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R617.1
    #[test]
    fn engine_held_memory_types_round_trip() {
        let record =
            ConversationRecord::new("tenant-a", "conv:1", vec![message("m1"), message("m2")])
                .unwrap();
        let rules = ProjectRules {
            tenant: TenantId::new("tenant-a").unwrap(),
            project_id: ResourceId::new("proj:1").unwrap(),
            layers: vec![RuleLayer {
                layer_id: ResourceId::new("layer:1").unwrap(),
                source: ResourceId::new("rules/project.md").unwrap(),
                facts: vec![ScopedFact {
                    fact_id: ResourceId::new("fact:1").unwrap(),
                    scope: ResourceId::new("src/").unwrap(),
                    rule: "always run the gates".into(),
                }],
            }],
        };
        let policy = RetentionPolicy::new(
            "tenant-a",
            "policy:1",
            86_400_000,
            vec![MemoryClass::MemoryNode, MemoryClass::Conversation],
            500,
        )
        .unwrap();
        let budget = ContextBudget::new("tenant-a", "budget:1", 32_000, 64).unwrap();
        macro_rules! round_trip {
            ($value:expr, $ty:ty) => {{
                let json = serde_json::to_string(&$value).unwrap();
                assert_eq!(serde_json::from_str::<$ty>(&json).unwrap(), $value);
            }};
        }
        round_trip!(record, ConversationRecord);
        round_trip!(rules, ProjectRules);
        round_trip!(policy, RetentionPolicy);
        round_trip!(budget, ContextBudget);
        assert_eq!(
            policy.classes,
            [MemoryClass::Conversation, MemoryClass::MemoryNode],
            "classes are stored deduplicated and sorted"
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R617.1
    #[test]
    fn engine_held_memory_types_reject_invalid_identifiers() {
        for bad in ["", "has space", "é"] {
            assert!(matches!(
                ConversationRecord::new(bad, "conv:1", vec![]),
                Err(MemoryError::InvalidIdentifier(_))
            ));
            assert!(ContextBudget::new("tenant-a", bad, 1, 1).is_err());
        }
        assert!(matches!(
            ConversationRecord::new("tenant-a", "conv:1", vec![message("m1"), message("m1")]),
            Err(MemoryError::RepeatedMessage(_))
        ));
        assert!(
            RetentionPolicy::new("tenant-a", "policy:1", 0, vec![MemoryClass::MemoryNode], 1)
                .is_err()
        );
        assert!(RetentionPolicy::new("tenant-a", "policy:1", 1, vec![], 1).is_err());
        assert!(
            RetentionPolicy::new("tenant-a", "policy:1", 1, vec![MemoryClass::MemoryNode], 0)
                .is_err()
        );
        assert!(ContextBudget::new("tenant-a", "budget:1", 0, 1).is_err());
        assert!(ContextBudget::new("tenant-a", "budget:1", 1, 0).is_err());
        assert!(serde_json::from_str::<ConversationRecord>(
            r#"{"tenant":"tenant-a","conversation_id":"conv:1","messages":[],"extra":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<RetentionPolicy>(
            r#"{"tenant":"tenant-a","policy_id":"policy:1","min_age_ms":0,"classes":[],"max_per_step":1}"#
        )
        .is_err());
    }

    fn digest_of(tag: &str) -> Digest256 {
        Digest256::sha256(tag.as_bytes())
    }

    // spec: MEDULLA-DECISION-ENGINE-R149.1
    #[test]
    fn search_state_round_trips() {
        let envelope = SearchStateEnvelope {
            search_run_id: "run:1".into(),
            state_revision: 3,
            transcript: vec![serde_json::json!({"turn": 1}), serde_json::json!("two")],
            evidence: vec![serde_json::json!({"doc": "d1"})],
            constraints: vec![serde_json::json!(["a", "b"])],
            open_claims: vec![],
        };
        let json = serde_json::to_string(&envelope).unwrap();
        let restored: SearchStateEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, envelope);
        assert_eq!(restored.transcript.len(), 2);
        assert_eq!(restored.evidence.len(), 1);
        assert_eq!(restored.constraints.len(), 1);
        assert!(restored.open_claims.is_empty());
        // The digests are derived, stable and independent per array.
        assert_eq!(envelope.transcript_digest(), restored.transcript_digest());
        assert_ne!(envelope.transcript_digest(), envelope.evidence_digest());
        assert_ne!(envelope.constraints_digest(), envelope.open_claims_digest());
        assert_eq!(envelope.evidence_digest().to_hex().len(), 64);
        assert!(
            !json.contains("transcript_digest"),
            "digests are never stored"
        );
        assert_eq!(
            serde_json::to_string(&MemoryKind::SearchState).unwrap(),
            "\"search_state\""
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R150.1
    #[test]
    fn notes_round_trip() {
        let note = SessionNoteBody {
            schema_version: 1,
            note_id: "note:1".into(),
            run_id: "run:1".into(),
            session_id: "session:1".into(),
            task_id: "task:1".into(),
            user_id: "user:1".into(),
            input_digest: digest_of("in"),
            output_digest: digest_of("out"),
            source_revision: 9,
            content: "remember this".into(),
        };
        let checked = note.clone().checked().unwrap();
        let json = serde_json::to_string(&checked).unwrap();
        assert_eq!(
            serde_json::from_str::<SessionNoteBody>(&json).unwrap(),
            note
        );
        assert_eq!(note.record_digest(), checked.record_digest());
        let mut other = note.clone();
        other.content = "different".into();
        assert_ne!(note.record_digest(), other.record_digest());
        let mut empty = note.clone();
        empty.user_id = String::new();
        assert!(empty.checked().is_err());

        let view = NoteView {
            note_id: "note:1".into(),
            revision: 2,
            relevance_q4: 10_000,
            stale: true,
            archived: false,
            conflict: true,
            applied: false,
        };
        let json = serde_json::to_string(&view).unwrap();
        assert_eq!(serde_json::from_str::<NoteView>(&json).unwrap(), view);
        assert!(view.clone().checked().is_ok());
        assert!(NoteView {
            relevance_q4: 10_001,
            ..view
        }
        .checked()
        .is_err());
        assert_eq!(
            serde_json::to_string(&MemoryKind::SessionNote).unwrap(),
            "\"session_note\""
        );
    }
}
