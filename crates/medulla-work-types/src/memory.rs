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
    #[error("SEARCH_STATE_INCOMPLETE: {0}")]
    SearchStateIncomplete(String),
    #[error("SEARCH_STATE_MALFORMED: {0}")]
    SearchStateMalformed(String),
    #[error("SEARCH_STATE_REVISION_MISMATCH: {0}")]
    SearchStateRevisionMismatch(String),
    #[error("MEMORY_EPISODE_WINDOW_INVALID: {0}")]
    EpisodeWindowInvalid(String),
    #[error("MEMORY_EPISODE_DIGEST_MISMATCH: {0}")]
    EpisodeDigestMismatch(String),
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

/// The fields a search-state body must carry.
const SEARCH_STATE_ARRAYS: [&str; 4] = ["transcript", "evidence", "constraints", "open_claims"];

/// Validate a search-state body saved at `snapshot_revision` on top of
/// `parent_revision` (MEDULLA-DECISION-ENGINE-R149.2). The body must be a
/// UTF-8 JSON object with a non-empty `search_run_id` and the four arrays
/// (which may be empty); its `state_revision` must equal the snapshot revision
/// and be at least 1; the snapshot revision must be the parent plus 1. The
/// compare-and-set of the expected head stays with the snapshot write.
pub fn validate_search_state(
    bytes: &[u8],
    snapshot_revision: u64,
    parent_revision: u64,
) -> Result<SearchStateEnvelope, MemoryError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| MemoryError::SearchStateMalformed("body is not UTF-8".into()))?;
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| MemoryError::SearchStateMalformed(format!("body is not JSON: {e}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| MemoryError::SearchStateMalformed("body is not a JSON object".into()))?;
    for field in ["search_run_id", "state_revision"]
        .into_iter()
        .chain(SEARCH_STATE_ARRAYS)
    {
        if !object.contains_key(field) {
            return Err(MemoryError::SearchStateIncomplete(format!(
                "{field} is absent"
            )));
        }
    }
    let envelope: SearchStateEnvelope = serde_json::from_value(value)
        .map_err(|e| MemoryError::SearchStateMalformed(e.to_string()))?;
    if envelope.search_run_id.is_empty() {
        return Err(MemoryError::SearchStateMalformed(
            "search_run_id is empty".into(),
        ));
    }
    if envelope.state_revision < 1 || envelope.state_revision != snapshot_revision {
        return Err(MemoryError::SearchStateRevisionMismatch(format!(
            "state_revision {} differs from snapshot revision {snapshot_revision}",
            envelope.state_revision
        )));
    }
    if parent_revision.checked_add(1) != Some(snapshot_revision) {
        return Err(MemoryError::SearchStateRevisionMismatch(format!(
            "snapshot revision {snapshot_revision} is not parent revision {parent_revision} plus 1"
        )));
    }
    Ok(envelope)
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

/// The schema version an [`Episode`] is sealed under.
pub const EPISODE_SCHEMA_VERSION: u16 = 1;

/// One sealed episode: the memory records of one session window, in time order
/// (MEDULLA-DECISION-ENGINE-R155.1). The store holds it as a node of type
/// `Episode` with one `CONTAINS` edge per record. It is immutable after
/// sealing: the only constructor is [`Episode::seal`], and deserialization
/// re-derives the digest and refuses a body that no longer matches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Episode {
    pub episode_id: ResourceId,
    pub tenant: TenantId,
    pub session_id: ResourceId,
    /// Milliseconds since the Unix epoch; the window never runs backwards.
    pub start_ms: i64,
    pub end_ms: i64,
    /// `MemoryRecord` ids in time order.
    pub record_ids: Vec<ResourceId>,
    /// SHA-256 over the canonical body, 64 lowercase hex characters.
    pub digest: String,
    pub schema_version: u16,
}

impl Episode {
    /// Seal an episode: validate the window and derive the digest.
    pub fn seal(
        episode_id: impl Into<String>,
        tenant: impl Into<String>,
        session_id: impl Into<String>,
        start_ms: i64,
        end_ms: i64,
        record_ids: Vec<String>,
    ) -> Result<Self, MemoryError> {
        let episode_id = identifier(episode_id, "episode_id")?;
        let tenant = tenant_id(tenant)?;
        let session_id = identifier(session_id, "session_id")?;
        let record_ids = record_ids
            .into_iter()
            .map(|id| identifier(id, "record_ids"))
            .collect::<Result<Vec<_>, _>>()?;
        Self::check_window(start_ms, end_ms)?;
        let digest = Self::body_digest(
            &episode_id,
            &tenant,
            &session_id,
            start_ms,
            end_ms,
            &record_ids,
            EPISODE_SCHEMA_VERSION,
        );
        Ok(Self {
            episode_id,
            tenant,
            session_id,
            start_ms,
            end_ms,
            record_ids,
            digest,
            schema_version: EPISODE_SCHEMA_VERSION,
        })
    }

    /// `end_ms` is not below `start_ms`.
    pub fn check_window(start_ms: i64, end_ms: i64) -> Result<(), MemoryError> {
        if end_ms < start_ms {
            return Err(MemoryError::EpisodeWindowInvalid(format!(
                "end_ms {end_ms} is below start_ms {start_ms}"
            )));
        }
        Ok(())
    }

    /// The sealed body is intact: the window is ordered and the digest is the
    /// digest of the other fields.
    pub fn validate(&self) -> Result<(), MemoryError> {
        Self::check_window(self.start_ms, self.end_ms)?;
        let expected = Self::body_digest(
            &self.episode_id,
            &self.tenant,
            &self.session_id,
            self.start_ms,
            self.end_ms,
            &self.record_ids,
            self.schema_version,
        );
        if expected != self.digest {
            return Err(MemoryError::EpisodeDigestMismatch(format!(
                "episode {} digest is {}, its body hashes to {expected}",
                self.episode_id.as_str(),
                self.digest
            )));
        }
        Ok(())
    }

    fn body_digest(
        episode_id: &ResourceId,
        tenant: &TenantId,
        session_id: &ResourceId,
        start_ms: i64,
        end_ms: i64,
        record_ids: &[ResourceId],
        schema_version: u16,
    ) -> String {
        let body = serde_json::json!({
            "episode_id": episode_id,
            "tenant": tenant,
            "session_id": session_id,
            "start_ms": start_ms,
            "end_ms": end_ms,
            "record_ids": record_ids,
            "schema_version": schema_version,
        });
        let bytes = serde_json::to_vec(&body).unwrap_or_default();
        Digest256::from_bytes(Sha256::digest(bytes).into()).to_hex()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EpisodeWire {
    episode_id: ResourceId,
    tenant: TenantId,
    session_id: ResourceId,
    start_ms: i64,
    end_ms: i64,
    record_ids: Vec<ResourceId>,
    digest: String,
    schema_version: u16,
}

impl<'de> Deserialize<'de> for Episode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = EpisodeWire::deserialize(deserializer)?;
        let episode = Self {
            episode_id: wire.episode_id,
            tenant: wire.tenant,
            session_id: wire.session_id,
            start_ms: wire.start_ms,
            end_ms: wire.end_ms,
            record_ids: wire.record_ids,
            digest: wire.digest,
            schema_version: wire.schema_version,
        };
        episode.validate().map_err(serde::de::Error::custom)?;
        Ok(episode)
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

    fn body(revision: u64) -> serde_json::Value {
        serde_json::json!({
            "search_run_id": "run:1",
            "state_revision": revision,
            "transcript": [{"turn": 1}],
            "evidence": [],
            "constraints": [],
            "open_claims": [{"claim": "x"}],
        })
    }

    // spec: MEDULLA-DECISION-ENGINE-R149.2
    #[test]
    fn eviction_readback_validates_revision_and_arrays() {
        let bytes = |value: &serde_json::Value| serde_json::to_vec(value).unwrap();
        // A valid body, arrays may be empty, validates and reads back equal.
        let state = validate_search_state(&bytes(&body(2)), 2, 1).unwrap();
        assert_eq!(state.search_run_id, "run:1");
        assert_eq!(state.state_revision, 2);
        assert!(state.evidence.is_empty());
        assert_eq!(state.transcript.len(), 1);
        // Each array and the run id missing is incomplete.
        for field in [
            "search_run_id",
            "state_revision",
            "transcript",
            "evidence",
            "constraints",
            "open_claims",
        ] {
            let mut value = body(2);
            value.as_object_mut().unwrap().remove(field);
            let refused = validate_search_state(&bytes(&value), 2, 1).unwrap_err();
            assert!(
                matches!(&refused, MemoryError::SearchStateIncomplete(d) if d.contains(field)),
                "{field}: {refused:?}"
            );
        }
        // Malformed bodies: not UTF-8, not JSON, not an object, empty run id,
        // a non-array field, an unknown field.
        let cases: [&[u8]; 3] = [&[0xff, 0xfe], b"{not json", b"[1,2]"];
        for raw in cases {
            assert!(matches!(
                validate_search_state(raw, 2, 1),
                Err(MemoryError::SearchStateMalformed(_))
            ));
        }
        let mut empty_id = body(2);
        empty_id["search_run_id"] = serde_json::json!("");
        let mut not_array = body(2);
        not_array["evidence"] = serde_json::json!({"a": 1});
        let mut unknown = body(2);
        unknown["extra"] = serde_json::json!(1);
        for value in [empty_id, not_array, unknown] {
            assert!(matches!(
                validate_search_state(&bytes(&value), 2, 1),
                Err(MemoryError::SearchStateMalformed(_))
            ));
        }
        // Revision rules: body vs snapshot, at least 1, parent plus 1.
        assert!(matches!(
            validate_search_state(&bytes(&body(3)), 2, 1),
            Err(MemoryError::SearchStateRevisionMismatch(_))
        ));
        assert!(matches!(
            validate_search_state(&bytes(&body(0)), 0, u64::MAX),
            Err(MemoryError::SearchStateRevisionMismatch(_))
        ));
        assert!(matches!(
            validate_search_state(&bytes(&body(2)), 2, 0),
            Err(MemoryError::SearchStateRevisionMismatch(_))
        ));
        assert!(validate_search_state(&bytes(&body(1)), 1, 0).is_ok());
        let refusal = validate_search_state(&bytes(&body(3)), 2, 1)
            .unwrap_err()
            .to_string();
        assert!(refusal.starts_with("SEARCH_STATE_REVISION_MISMATCH"));
    }

    // spec: MEDULLA-DECISION-ENGINE-R155.1
    #[test]
    fn episode_round_trip_and_digest() {
        let episode = Episode::seal(
            "episode:1",
            "tenant-a",
            "session:1",
            1_000,
            2_000,
            vec!["mem:1".into(), "mem:2".into(), "mem:3".into()],
        )
        .unwrap();
        // Serde round trip keeps every field, record order included.
        let json = serde_json::to_string(&episode).unwrap();
        let restored: Episode = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, episode);
        let order: Vec<&str> = restored.record_ids.iter().map(|id| id.as_str()).collect();
        assert_eq!(order, ["mem:1", "mem:2", "mem:3"]);
        // The digest is 64 lowercase hex characters and stable.
        assert_eq!(episode.digest.len(), 64);
        assert!(episode
            .digest
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        let again = Episode::seal(
            "episode:1",
            "tenant-a",
            "session:1",
            1_000,
            2_000,
            vec!["mem:1".into(), "mem:2".into(), "mem:3".into()],
        )
        .unwrap();
        assert_eq!(again.digest, episode.digest);
        // A different record order or window is a different episode.
        let reordered = Episode::seal(
            "episode:1",
            "tenant-a",
            "session:1",
            1_000,
            2_000,
            vec!["mem:2".into(), "mem:1".into(), "mem:3".into()],
        )
        .unwrap();
        assert_ne!(reordered.digest, episode.digest);
        // A zero-length window is allowed; one that runs backwards is refused
        // at sealing and at deserialization.
        assert!(Episode::seal("episode:2", "tenant-a", "session:1", 5, 5, vec![]).is_ok());
        let err =
            Episode::seal("episode:3", "tenant-a", "session:1", 2_000, 1_000, vec![]).unwrap_err();
        assert!(matches!(err, MemoryError::EpisodeWindowInvalid(_)));
        let mut backwards: serde_json::Value = serde_json::from_str(&json).unwrap();
        backwards["start_ms"] = serde_json::json!(3_000);
        let refused = serde_json::from_value::<Episode>(backwards).unwrap_err();
        assert!(refused
            .to_string()
            .contains("MEMORY_EPISODE_WINDOW_INVALID"));
        // A sealed body that was edited no longer matches its digest.
        let mut edited: serde_json::Value = serde_json::from_str(&json).unwrap();
        edited["record_ids"] = serde_json::json!(["mem:1"]);
        let refused = serde_json::from_value::<Episode>(edited).unwrap_err();
        assert!(refused
            .to_string()
            .contains("MEMORY_EPISODE_DIGEST_MISMATCH"));
        // Unknown fields are refused.
        let mut extra: serde_json::Value = serde_json::from_str(&json).unwrap();
        extra["sealed"] = serde_json::json!(false);
        assert!(serde_json::from_value::<Episode>(extra).is_err());
    }
}
