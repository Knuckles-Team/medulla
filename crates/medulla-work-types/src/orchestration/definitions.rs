//! Definition records: the prompt template and the workflow pattern
//! (MEDULLA-ORCH-VOCAB-R800.1).
//!
//! One set of types serves both the spec records and the stored definition
//! records; no parallel spec type exists. An agent definition is the Agent
//! Library entry the assembly path already reads, not a new type here. A
//! template's `revision_ref` is a `ProgramRevision` reference
//! (MEDULLA-DECISION-ENGINE-R810.1), carried as its identity text so this
//! vocabulary crate stays below the program crate.

use serde::{Deserialize, Serialize};

use super::workflows::WorkflowIdentity;

/// The closed set of slot types a template can declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SlotType {
    /// Free text.
    Text,
    /// A signed integer.
    Integer,
    /// An IRI.
    Iri,
    /// A list of values.
    List,
}

/// One named, typed slot of a prompt template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TemplateSlot {
    /// The slot name as it appears in the template body.
    pub name: String,
    /// The one type a value for this slot must have.
    pub slot_type: SlotType,
}

/// A prompt template: typed slots, pinned to a program revision and a
/// content digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PromptTemplate {
    /// The template's identity.
    pub id: String,
    /// The `ProgramRevision` reference this template is a revision of.
    pub revision_ref: String,
    /// The typed slots, in declaration order.
    pub slots: Vec<TemplateSlot>,
    /// The content digest of the template.
    pub digest: String,
}

/// What a pattern needs to hold before it applies: a capability or a fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "iri", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PatternPrecondition {
    /// A capability the graph must hold.
    Capability(String),
    /// A fact, named by its IRI, that must hold in the graph.
    Fact(String),
}

/// A workflow pattern: an ordered list of registered workflow steps with
/// the preconditions under which it applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct WorkflowPattern {
    /// The registered workflow this pattern is.
    pub workflow: WorkflowIdentity,
    /// Names of registered workflow steps, in order.
    pub steps: Vec<String>,
    /// Capabilities or fact IRIs that must hold.
    pub preconditions: Vec<PatternPrecondition>,
    /// The pattern version; the compile row stores and cites it.
    pub version: u32,
    /// The process revision this pattern was compiled from.
    pub process_revision: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-ORCH-VOCAB-R800.1
    #[test]
    fn template_and_pattern_round_trip() {
        let template = PromptTemplate {
            id: "template:triage".to_string(),
            revision_ref: "program-revision:triage:3".to_string(),
            slots: vec![
                TemplateSlot {
                    name: "subject".to_string(),
                    slot_type: SlotType::Text,
                },
                TemplateSlot {
                    name: "limit".to_string(),
                    slot_type: SlotType::Integer,
                },
                TemplateSlot {
                    name: "target".to_string(),
                    slot_type: SlotType::Iri,
                },
                TemplateSlot {
                    name: "items".to_string(),
                    slot_type: SlotType::List,
                },
            ],
            digest: format!("sha256:{}", "ab".repeat(32)),
        };
        let json = serde_json::to_string(&template).unwrap();
        assert!(json.contains("\"slot_type\":\"iri\""));
        assert_eq!(
            serde_json::from_str::<PromptTemplate>(&json).unwrap(),
            template
        );

        let pattern = WorkflowPattern {
            workflow: WorkflowIdentity {
                workflow_id: "workflow:triage".to_string(),
                version: "1".to_string(),
                content_digest: format!("sha256:{}", "cd".repeat(32)),
            },
            steps: vec!["collect".to_string(), "rank".to_string()],
            preconditions: vec![
                PatternPrecondition::Capability("capability:search".to_string()),
                PatternPrecondition::Fact("fact:ticket-open".to_string()),
            ],
            version: 2,
            process_revision: "process-revision:7".to_string(),
        };
        let json = serde_json::to_string(&pattern).unwrap();
        assert_eq!(
            serde_json::from_str::<WorkflowPattern>(&json).unwrap(),
            pattern
        );

        let unknown = json.replace("\"steps\"", "\"extra\":1,\"steps\"");
        assert!(serde_json::from_str::<WorkflowPattern>(&unknown).is_err());
        assert!(serde_json::from_str::<SlotType>("\"float\"").is_err());
    }
}
