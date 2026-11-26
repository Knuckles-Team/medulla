//! Versioned harness components as graph facts (MEDULLA-ORCH-VOCAB-R801), and the
//! improvement library that stores what a task learned from them (R802, in
//! [`improvement`]).
//!
//! A harness component is one of the eight editable pieces of an agent's
//! harness: system_prompt, guardrails, memory, tool_guidance, subagent,
//! skill, script or plugin. Each version carries a content digest, a valid
//! time, a confidence and an evidence link. A published version never
//! mutates, and a version becomes current only by a champion pointer move
//! (R801.7): the pointer's `Activated` events are the supersession history.
//!
//! This module is the typed vocabulary and the pure rules over it. The
//! durable write (the ChangeEnvelope/AddNode path, R801.3) stamps the
//! versions bitemporally; currency is read through the `decision_pointers`
//! relation. The records themselves are plain data: one vocabulary, one
//! writer.

pub mod agent_lifecycle_machine;
pub mod capability_gap;
pub mod definitions;
pub mod errors;
pub mod improvement;
pub mod org;
pub mod teams;
pub mod workflow_machine;
pub mod workflows;

use serde::{Deserialize, Serialize};

pub use errors::HarnessVocabularyErrorCode;

/// The closed set of editable harness components (R801).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum HarnessComponentKind {
    /// The agent's standing instructions.
    SystemPrompt,
    /// The guardrails a run must respect (MEDULLA-ORCH-VOCAB-R004.700 scope).
    Guardrails,
    /// The memory a run reads and writes.
    Memory,
    /// The guidance over which tools to reach for.
    ToolGuidance,
    /// The subagent delegation surface.
    Subagent,
    /// A skill procedure the harness can invoke.
    Skill,
    /// A script the harness can run.
    Script,
    /// A plugin extending the harness.
    Plugin,
}

impl HarnessComponentKind {
    /// The one wire name of each kind.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::SystemPrompt => "system_prompt",
            Self::Guardrails => "guardrails",
            Self::Memory => "memory",
            Self::ToolGuidance => "tool_guidance",
            Self::Subagent => "subagent",
            Self::Skill => "skill",
            Self::Script => "script",
            Self::Plugin => "plugin",
        }
    }

    /// Parse one of the eight wire names. Anything else is an unknown kind:
    /// the closed set is the model itself, so the write path refuses it with
    /// [`HarnessComponentRefusal::KindUnknown`] (R801.4) instead of guessing.
    pub fn from_wire(name: &str) -> Result<Self, HarnessComponentRefusal> {
        Ok(match name {
            "system_prompt" => Self::SystemPrompt,
            "guardrails" => Self::Guardrails,
            "memory" => Self::Memory,
            "tool_guidance" => Self::ToolGuidance,
            "subagent" => Self::Subagent,
            "skill" => Self::Skill,
            "script" => Self::Script,
            "plugin" => Self::Plugin,
            _ => return Err(HarnessComponentRefusal::KindUnknown),
        })
    }
}

/// The typed refusal of a harness-component write; each variant carries its
/// served code verbatim (R801.4 names the first four).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessComponentRefusal {
    /// A version without an `evidence_run_id`.
    VersionWithoutEvidence,
    /// A kind outside the closed set of eight.
    KindUnknown,
    /// A confidence outside 0 to 1 (or not a number at all).
    ConfidenceInvalid,
    /// A write that would mutate a published version.
    VersionImmutable,
    /// A component id or content reference that names nothing.
    IdentityMissing,
    /// A content digest that does not recompute from the content reference.
    ContentDigestMismatch,
    /// A version written without its transaction-time stamp.
    WithoutStamp,
    /// A version record that does not decode to the vocabulary's shape.
    RecordMalformed,
}

impl HarnessComponentRefusal {
    /// The served error code, verbatim (MEDULLA-DURABLE-KERNEL-R043.8).
    pub fn code(self) -> &'static str {
        match self {
            Self::VersionWithoutEvidence => HarnessVocabularyErrorCode::VersionWithoutEvidence,
            Self::KindUnknown => HarnessVocabularyErrorCode::KindUnknown,
            Self::ConfidenceInvalid => HarnessVocabularyErrorCode::ConfidenceInvalid,
            Self::VersionImmutable => HarnessVocabularyErrorCode::VersionImmutable,
            Self::IdentityMissing => HarnessVocabularyErrorCode::IdentityMissing,
            Self::ContentDigestMismatch => HarnessVocabularyErrorCode::ContentDigestMismatch,
            Self::WithoutStamp => HarnessVocabularyErrorCode::WithoutStamp,
            Self::RecordMalformed => HarnessVocabularyErrorCode::RecordMalformed,
        }
        .as_str()
    }
}

/// One published version of one harness component (R801.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HarnessComponentVersion {
    /// The component this is a version of; the same across its versions.
    pub component_id: String,
    /// Which of the eight components; the closed set.
    pub kind: HarnessComponentKind,
    /// Where the content lives; the digest recomputes from it.
    pub content_ref: String,
    /// `sha256:` of the content reference under the vocabulary's digest
    /// domain; a published version's identity.
    pub content_digest: String,
    /// Valid time: from when this version is current (bitemporal, R078/R081).
    pub valid_from: u64,
    /// How much the writer trusts this version; 0 to 1.
    pub confidence: f64,
    /// The evaluation run that produced it; never empty (R801.4).
    pub evidence_run_id: String,
    /// The agent or task scope the version is applied to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_to_id: Option<String>,
}

impl HarnessComponentVersion {
    /// The one digest scheme the vocabulary uses for content: the engine's
    /// canonical text digest over the content reference.
    pub fn content_digest(content_ref: &str) -> String {
        medulla_query_types::solve_scalar::digest_text(
            "eg/harness-component-content/v1",
            &content_ref.to_string(),
        )
    }

    /// The store identity of one version. It is content-addressed on purpose:
    /// a rewrite of the same content is the same node (an idempotent replay),
    /// and any change of content is a different version, so a published
    /// version's node can never carry different content (R801.4).
    pub fn node_id(&self) -> String {
        format!(
            "harness-component:{}:{}",
            self.component_id, self.content_digest
        )
    }

    /// The admission check over the fields the writer can still get wrong:
    /// identity, evidence, confidence and digest integrity (R801.4). The kind is the closed set itself; an unknown kind
    /// refuses at decode, and the write path reports it as
    /// [`HarnessComponentRefusal::KindUnknown`].
    pub fn validate(&self) -> Result<(), HarnessComponentRefusal> {
        if self.component_id.trim().is_empty() || self.content_ref.trim().is_empty() {
            return Err(HarnessComponentRefusal::IdentityMissing);
        }
        if self.evidence_run_id.trim().is_empty() {
            return Err(HarnessComponentRefusal::VersionWithoutEvidence);
        }
        if !self.confidence.is_finite() || !(0.0..=1.0).contains(&self.confidence) {
            return Err(HarnessComponentRefusal::ConfidenceInvalid);
        }
        if self.content_digest != Self::content_digest(&self.content_ref) {
            return Err(HarnessComponentRefusal::ContentDigestMismatch);
        }
        Ok(())
    }

    /// Whether two records of the same content carry the same writer-supplied
    /// metadata (R801.4).
    pub fn same_writer_fields(&self, other: &Self) -> bool {
        self.component_id == other.component_id
            && self.kind == other.kind
            && self.content_ref == other.content_ref
            && self.content_digest == other.content_digest
            && self.valid_from == other.valid_from
            && self.confidence == other.confidence
            && self.evidence_run_id == other.evidence_run_id
            && self.applied_to_id == other.applied_to_id
    }
}

/// Whether `props` is a harness-component version write: the engine-owned
/// marker on the node's property object (R801.3).
pub fn is_harness_component_write(props: &serde_json::Map<String, serde_json::Value>) -> bool {
    props
        .get("_harness_component")
        .and_then(|value| value.as_bool())
        == Some(true)
}

/// The engine-owned keys of a harness-component node's property object: the
/// write marker, the transaction-time stamp (the engine stamps it on the
/// direct AddNode path; the generated client stamps it on the
/// ChangeEnvelope path), the transaction-end stamp the bitemporal kernel
/// carries, and the owner-id stamp the gateway applies to every native write. A record
/// decoded from the object ignores exactly these.
const ENGINE_OWNED_HARNESS_KEYS: &[&str] = &["_harness_component", "tx_from", "tx_to", "_owner_id"];

/// Decode a harness-component version write from its stored property object
/// (R801.1/R801.3). The shape is strict: an unknown `kind` refuses
/// [`HarnessComponentRefusal::KindUnknown`] before anything else is looked
/// at, and any other decode failure (a wrong field type, a missing field, an
/// unknown field) refuses [`HarnessComponentRefusal::RecordMalformed`]
/// instead of silently dropping the field. The engine-owned keys are
/// ignored; the caller runs [`HarnessComponentVersion::validate`] for the
/// typed evidence/refusals.
pub fn decode_component_write(
    props: &serde_json::Map<String, serde_json::Value>,
) -> Result<HarnessComponentVersion, HarnessComponentRefusal> {
    if !is_harness_component_write(props) {
        return Err(HarnessComponentRefusal::RecordMalformed);
    }
    match props.get("kind").and_then(|value| value.as_str()) {
        Some(name) => HarnessComponentKind::from_wire(name)?,
        None => return Err(HarnessComponentRefusal::KindUnknown),
    };
    let mut record = props.clone();
    for key in ENGINE_OWNED_HARNESS_KEYS {
        record.remove(*key);
    }
    serde_json::from_value(serde_json::Value::Object(record))
        .map_err(|_| HarnessComponentRefusal::RecordMalformed)
}

/// The transaction-time stamp a harness-component node must carry
/// (R801.3): `tx_from` set, and a positive number.
pub fn component_tx_stamp(props: &serde_json::Map<String, serde_json::Value>) -> Option<u64> {
    props
        .get("tx_from")
        .and_then(|value| value.as_u64())
        .filter(|stamp| *stamp > 0)
}

/// Admit one new version into the store's vocabulary (R801.7).
///
/// A published version never mutates: the same `(component_id,
/// content_digest)` node id with the same writer fields is an idempotent
/// replay, and with any other writer field is refused
/// [`HarnessComponentRefusal::VersionImmutable`]. Admission closes no window
/// and writes no edge: a version becomes current only when the champion
/// pointer of its component moves to it.
pub fn register_version(
    versions: &mut Vec<HarnessComponentVersion>,
    new: HarnessComponentVersion,
) -> Result<(), HarnessComponentRefusal> {
    new.validate()?;
    match versions.iter().find(|v| v.node_id() == new.node_id()) {
        Some(published) if published.same_writer_fields(&new) => Ok(()),
        Some(_) => Err(HarnessComponentRefusal::VersionImmutable),
        None => {
            versions.push(new);
            Ok(())
        }
    }
}

/// One node of the champion chain, with the evidence it carries and the
/// content digest recomputed from its content reference (R801.5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HarnessVersionEntry {
    /// The version itself: its evidence run id, confidence and valid
    /// interval ride here.
    pub version: HarnessComponentVersion,
    /// The previous champion, by node id (the chain's next hop); absent for
    /// the first champion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_id: Option<String>,
    /// The content digest recomputed from `version.content_ref`; equals
    /// `version.content_digest` for every admitted version.
    pub content_digest_recomputed: String,
}

/// The champion chain of one component, newest first (R801.5, R801.7).
/// `champions` are the targets of the component subject pointer's `Activated`
/// events, oldest first. Each entry's `supersedes_id` is the previous
/// champion. A champion naming a version the store does not hold is skipped
/// rather than invented.
pub fn version_history(
    component_id: &str,
    versions: &[HarnessComponentVersion],
    champions: &[String],
) -> Vec<HarnessVersionEntry> {
    let mut out = Vec::new();
    for (index, champion) in champions.iter().enumerate().rev() {
        let Some(version) = versions
            .iter()
            .find(|v| v.component_id == component_id && v.node_id() == *champion)
        else {
            continue;
        };
        out.push(HarnessVersionEntry {
            version: version.clone(),
            supersedes_id: index.checked_sub(1).map(|prior| champions[prior].clone()),
            content_digest_recomputed: HarnessComponentVersion::content_digest(
                &version.content_ref,
            ),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-ORCH-VOCAB-R801.1
    #[test]
    fn the_eight_components_are_the_closed_set() {
        for kind in [
            HarnessComponentKind::SystemPrompt,
            HarnessComponentKind::Guardrails,
            HarnessComponentKind::Memory,
            HarnessComponentKind::ToolGuidance,
            HarnessComponentKind::Subagent,
            HarnessComponentKind::Skill,
            HarnessComponentKind::Script,
            HarnessComponentKind::Plugin,
        ] {
            assert_eq!(
                HarnessComponentKind::from_wire(kind.wire_name()).unwrap(),
                kind
            );
        }
        assert_eq!(
            HarnessComponentKind::from_wire("widget"),
            Err(HarnessComponentRefusal::KindUnknown)
        );
    }
}
