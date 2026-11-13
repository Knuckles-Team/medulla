//! Typed primitives an assembly composes (MEDULLA-DECISION-ENGINE-R036.100.1).
//!
//! A harness graph is built from four primitive classes: prompts, tools,
//! memory reads and control-flow predicates. Each published component of one
//! of those kinds may declare a [`PrimitiveContract`]: the schema digests of
//! its input and output and the effects and capabilities it has. The edge and
//! substitution rules (R036.100.2) read the contract from `CandidateFacts`.

use std::collections::BTreeSet;

use medulla_agent_types::agent_component::capability_descriptor::SideEffectClass;
use medulla_agent_types::agent_component::{AgentComponentEntry, AgentComponentKind};
use serde::{Deserialize, Serialize};

/// The effect a primitive can have. One enum for the whole surface: the
/// capability descriptor's [`SideEffectClass`], not a second copy.
pub type EffectClass = SideEffectClass;

/// Attribute carrying the SHA-256 digest of the input schema.
pub const INPUT_SCHEMA_ATTRIBUTE: &str = "primitive.input_schema_digest";
/// Attribute carrying the SHA-256 digest of the output schema.
pub const OUTPUT_SCHEMA_ATTRIBUTE: &str = "primitive.output_schema_digest";
/// Attribute carrying the comma-separated effect classes (default: none).
pub const EFFECTS_ATTRIBUTE: &str = "primitive.effects";

/// The closed union of primitive classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PrimitiveKind {
    Prompt,
    Tool,
    Memory,
    ControlFlow,
}

impl PrimitiveKind {
    /// The primitive class of a component kind; `None` for a kind that is
    /// not a primitive. One exhaustive `match`: a new component kind must
    /// choose a class here.
    pub fn of(kind: AgentComponentKind) -> Option<Self> {
        use AgentComponentKind as K;
        match kind {
            K::SystemPrompt | K::McpPrompt | K::NlTemplate => Some(Self::Prompt),
            K::Tool | K::Toolset | K::McpServer | K::Skill => Some(Self::Tool),
            K::McpResource | K::DecisionRecord => Some(Self::Memory),
            K::Predicate | K::OutputValidator | K::GuardrailRule => Some(Self::ControlFlow),
            K::ModelProfile
            | K::Ontology
            | K::Schema
            | K::Shapes
            | K::A2aAgentCard
            | K::DecisionPolicy
            | K::DecisionHead
            | K::FeatureSchema
            | K::Rubric
            | K::RuleInvariant
            | K::RedTeamAttack
            | K::RedTeamTransform
            | K::RedTeamScorer => None,
        }
    }
}

/// Why a contract is not well-formed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrimitiveContractError {
    /// A schema digest is not 64 lowercase hex characters.
    BadDigest(&'static str),
    /// An effect name is not an [`EffectClass`].
    UnknownEffect(String),
    /// Exactly one of the two schema digests is declared.
    IncompleteSchemas,
}

impl std::fmt::Display for PrimitiveContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadDigest(field) => {
                write!(f, "{field} must be 64 lowercase hex characters")
            }
            Self::UnknownEffect(name) => write!(f, "unknown effect class `{name}`"),
            Self::IncompleteSchemas => {
                write!(f, "input and output schema digests are declared together")
            }
        }
    }
}

/// The typed contract of one primitive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PrimitiveContract {
    pub primitive_id: String,
    pub kind: PrimitiveKind,
    pub version: String,
    pub input_schema_digest: String,
    pub output_schema_digest: String,
    pub effects: BTreeSet<EffectClass>,
    pub capabilities: BTreeSet<String>,
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn effect(name: &str) -> Result<EffectClass, PrimitiveContractError> {
    match name {
        "read_only" => Ok(EffectClass::ReadOnly),
        "graph_write" => Ok(EffectClass::GraphWrite),
        "external" => Ok(EffectClass::External),
        "irreversible" => Ok(EffectClass::Irreversible),
        other => Err(PrimitiveContractError::UnknownEffect(other.to_string())),
    }
}

impl PrimitiveContract {
    /// Check both schema digests are SHA-256, 64 lowercase hex characters.
    pub fn validate(&self) -> Result<(), PrimitiveContractError> {
        if !is_sha256_hex(&self.input_schema_digest) {
            return Err(PrimitiveContractError::BadDigest("input_schema_digest"));
        }
        if !is_sha256_hex(&self.output_schema_digest) {
            return Err(PrimitiveContractError::BadDigest("output_schema_digest"));
        }
        Ok(())
    }

    /// The contract a published component declares: `Ok(None)` when its kind
    /// is not a primitive or it declares no schema digests (such a candidate
    /// skips the primitive checks), an error when the declaration is
    /// malformed. Capabilities are the component's declared capabilities.
    pub fn from_entry(entry: &AgentComponentEntry) -> Result<Option<Self>, PrimitiveContractError> {
        let Some(kind) = PrimitiveKind::of(entry.kind) else {
            return Ok(None);
        };
        let input = entry.attributes.get(INPUT_SCHEMA_ATTRIBUTE);
        let output = entry.attributes.get(OUTPUT_SCHEMA_ATTRIBUTE);
        let (input, output) = match (input, output) {
            (None, None) => return Ok(None),
            (Some(i), Some(o)) => (i.clone(), o.clone()),
            _ => return Err(PrimitiveContractError::IncompleteSchemas),
        };
        let mut effects = BTreeSet::new();
        if let Some(list) = entry.attributes.get(EFFECTS_ATTRIBUTE) {
            for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                effects.insert(effect(name)?);
            }
        }
        let contract = Self {
            primitive_id: entry.component_id.clone(),
            kind,
            version: entry.version.clone(),
            input_schema_digest: input,
            output_schema_digest: output,
            effects,
            capabilities: entry.declared_capabilities.iter().cloned().collect(),
        };
        contract.validate()?;
        Ok(Some(contract))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(c: char) -> String {
        std::iter::repeat(c).take(64).collect()
    }

    fn contract() -> PrimitiveContract {
        PrimitiveContract {
            primitive_id: "tool-a".to_string(),
            kind: PrimitiveKind::Tool,
            version: "1.2.0".to_string(),
            input_schema_digest: digest('a'),
            output_schema_digest: digest('b'),
            effects: BTreeSet::from([EffectClass::ReadOnly, EffectClass::External]),
            capabilities: BTreeSet::from(["urn:vendor:search".to_string()]),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R036.100.1
    /// MEDULLA-DECISION-ENGINE-R036.100.1
    #[test]
    fn primitive_contract_round_trips_and_kind_map_is_exhaustive() {
        let original = contract();
        let json = serde_json::to_string(&original).expect("encodes");
        let back: PrimitiveContract = serde_json::from_str(&json).expect("decodes");
        assert_eq!(back, original);
        assert_eq!(back.validate(), Ok(()));

        for bad in [digest('A'), "abc".to_string(), format!("{}0", digest('a'))] {
            let mut c = contract();
            c.input_schema_digest = bad.clone();
            assert_eq!(
                c.validate(),
                Err(PrimitiveContractError::BadDigest("input_schema_digest")),
                "{bad}"
            );
            let mut c = contract();
            c.output_schema_digest = bad;
            assert_eq!(
                c.validate(),
                Err(PrimitiveContractError::BadDigest("output_schema_digest"))
            );
        }

        use AgentComponentKind as K;
        let expected = [
            (K::SystemPrompt, Some(PrimitiveKind::Prompt)),
            (K::McpPrompt, Some(PrimitiveKind::Prompt)),
            (K::NlTemplate, Some(PrimitiveKind::Prompt)),
            (K::Tool, Some(PrimitiveKind::Tool)),
            (K::Toolset, Some(PrimitiveKind::Tool)),
            (K::McpServer, Some(PrimitiveKind::Tool)),
            (K::Skill, Some(PrimitiveKind::Tool)),
            (K::McpResource, Some(PrimitiveKind::Memory)),
            (K::DecisionRecord, Some(PrimitiveKind::Memory)),
            (K::Predicate, Some(PrimitiveKind::ControlFlow)),
            (K::OutputValidator, Some(PrimitiveKind::ControlFlow)),
            (K::GuardrailRule, Some(PrimitiveKind::ControlFlow)),
            (K::ModelProfile, None),
            (K::Ontology, None),
            (K::Schema, None),
            (K::Shapes, None),
            (K::A2aAgentCard, None),
            (K::DecisionPolicy, None),
            (K::DecisionHead, None),
            (K::FeatureSchema, None),
            (K::Rubric, None),
            (K::RuleInvariant, None),
            (K::RedTeamAttack, None),
            (K::RedTeamTransform, None),
            (K::RedTeamScorer, None),
        ];
        for (kind, primitive) in expected {
            assert_eq!(PrimitiveKind::of(kind), primitive, "{kind:?}");
        }
    }
}
