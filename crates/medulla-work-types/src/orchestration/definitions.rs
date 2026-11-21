//! Definition records: the prompt template and the workflow pattern
//! (MEDULLA-ORCH-VOCAB-R800.1).
//!
//! One set of types serves both the spec records and the stored definition
//! records; no parallel spec type exists. An agent definition is the Agent
//! Library entry the assembly path already reads, not a new type here. A
//! template's `revision_ref` is a `ProgramRevision` reference
//! (MEDULLA-DECISION-ENGINE-R810.1), carried as its identity text so this
//! vocabulary crate stays below the program crate.

use medulla_types::closed_error_codes;
use serde::{Deserialize, Serialize};

use super::workflows::WorkflowIdentity;

closed_error_codes! {
    /// The four definition faults (MEDULLA-ORCH-VOCAB-R800.4). Each refusal writes
    /// nothing. The set is registered in
    /// `medulla_protocol_types::error_registry::declared_error_code`.
    pub enum DefinitionRefusal {
        /// A declared slot (or a body placeholder) that has no value.
        TemplateSlotUnfilled => "TEMPLATE_SLOT_UNFILLED",
        /// A value of the wrong type for its slot.
        TemplateSlotType => "TEMPLATE_SLOT_TYPE",
        /// A pattern that names a workflow step the workflow does not register.
        PatternStepUnknown => "PATTERN_STEP_UNKNOWN",
        /// An edit that reuses an already published revision.
        RevisionPublished => "DEFINITION_REVISION_PUBLISHED",
    }
}

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

/// One typed value for a template slot, matching [`SlotType`] one to one
/// (MEDULLA-ORCH-VOCAB-R800.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum SlotValue {
    /// Free text, rendered verbatim.
    Text(String),
    /// A signed integer, rendered in base 10.
    Integer(i64),
    /// An IRI, rendered verbatim.
    Iri(String),
    /// A list, rendered as its items joined by `, `.
    List(Vec<String>),
}

impl SlotValue {
    /// The slot type this value fills.
    pub fn slot_type(&self) -> SlotType {
        match self {
            Self::Text(_) => SlotType::Text,
            Self::Integer(_) => SlotType::Integer,
            Self::Iri(_) => SlotType::Iri,
            Self::List(_) => SlotType::List,
        }
    }

    fn render_into(&self, out: &mut String) {
        match self {
            Self::Text(text) | Self::Iri(text) => out.push_str(text),
            Self::Integer(number) => out.push_str(&number.to_string()),
            Self::List(items) => out.push_str(&items.join(", ")),
        }
    }
}

/// The digest of one render (MEDULLA-ORCH-VOCAB-R800.3, R800.5): the lowercase hex SHA-256 of the
/// template digest, one `\n`, and the canonical JSON (keys sorted, no whitespace) of the values
/// sorted by name. Equal template digest and equal values give one digest, whatever the order the
/// values were given in.
pub fn render_digest(template_digest: &str, values: &[(String, SlotValue)]) -> String {
    let sorted: std::collections::BTreeMap<&str, &SlotValue> = values
        .iter()
        .map(|(name, value)| (name.as_str(), value))
        .collect();
    let canonical = serde_json::to_string(&sorted).unwrap_or_default();
    let mut framed = Vec::with_capacity(template_digest.len() + 1 + canonical.len());
    framed.extend_from_slice(template_digest.as_bytes());
    framed.push(b'\n');
    framed.extend_from_slice(canonical.as_bytes());
    medulla_types::contract::Digest256::sha256(&framed).to_hex()
}

/// Fill the `{{name}}` placeholders of `body`, the template text read from the blob of
/// `template.revision_ref`, with typed `values` (MEDULLA-ORCH-VOCAB-R800.2).
///
/// Every declared slot must have exactly one value of its declared type, and every
/// placeholder must name a declared slot. A declared slot with no value, or a placeholder
/// that names no declared slot, refuses [`DefinitionRefusal::TemplateSlotUnfilled`]; a value of
/// the wrong type, a value for an undeclared slot, or a second value for one slot refuses
/// [`DefinitionRefusal::TemplateSlotType`]. A substituted value is never re-scanned for
/// placeholders, so one pass over the body and the values is the whole cost. Nothing is written.
pub fn render_template(
    template: &PromptTemplate,
    body: &str,
    values: &[(String, SlotValue)],
) -> Result<String, DefinitionRefusal> {
    for (name, value) in values {
        let declared = template.slots.iter().find(|slot| slot.name == *name);
        match declared {
            Some(slot) if slot.slot_type == value.slot_type() => {}
            _ => return Err(DefinitionRefusal::TemplateSlotType),
        }
        if values.iter().filter(|(other, _)| other == name).count() > 1 {
            return Err(DefinitionRefusal::TemplateSlotType);
        }
    }
    for slot in &template.slots {
        if !values.iter().any(|(name, _)| *name == slot.name) {
            return Err(DefinitionRefusal::TemplateSlotUnfilled);
        }
    }
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(open) = rest.find("{{") {
        let Some(length) = rest[open + 2..].find("}}") else {
            break;
        };
        out.push_str(&rest[..open]);
        let name = &rest[open + 2..open + 2 + length];
        match values.iter().find(|(declared, _)| declared == name) {
            Some((_, value)) => value.render_into(&mut out),
            None => return Err(DefinitionRefusal::TemplateSlotUnfilled),
        }
        rest = &rest[open + 2 + length + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn four_slot_template() -> PromptTemplate {
        let slot = |name: &str, slot_type| TemplateSlot {
            name: name.to_string(),
            slot_type,
        };
        PromptTemplate {
            id: "template:render".to_string(),
            revision_ref: "program-revision:render:1".to_string(),
            slots: vec![
                slot("subject", SlotType::Text),
                slot("limit", SlotType::Integer),
                slot("target", SlotType::Iri),
                slot("items", SlotType::List),
            ],
            digest: format!("sha256:{}", "ab".repeat(32)),
        }
    }

    fn four_values() -> Vec<(String, SlotValue)> {
        vec![
            ("subject".to_string(), SlotValue::Text("{{x}} ticket".to_string())),
            ("limit".to_string(), SlotValue::Integer(-42)),
            ("target".to_string(), SlotValue::Iri("urn:t:1".to_string())),
            (
                "items".to_string(),
                SlotValue::List(vec!["a".to_string(), "b".to_string(), "c".to_string()]),
            ),
        ]
    }

    // spec: MEDULLA-ORCH-VOCAB-R800.5
    #[test]
    fn render_digest_golden() {
        let template = four_slot_template();
        let values = four_values();
        let golden = include_str!("../../tests/fixtures/render_digest.golden.txt").trim();
        assert_eq!(render_digest(&template.digest, &values), golden);
        // The order the values arrive in never changes the digest.
        let mut reversed = values.clone();
        reversed.reverse();
        assert_eq!(render_digest(&template.digest, &reversed), golden);
        // A different value or a different template digest does.
        let mut other = values;
        other[1].1 = SlotValue::Integer(-41);
        assert_ne!(render_digest(&template.digest, &other), golden);
        assert_ne!(render_digest("sha256:00", &four_values()), golden);
    }

    // spec: MEDULLA-ORCH-VOCAB-R800.2
    #[test]
    fn render_fills_typed_slots_only() {
        let template = four_slot_template();
        let body = "Triage {{subject}} (max {{limit}}) at {{target}}: {{items}}. Again {{limit}}.";
        let rendered = render_template(&template, body, &four_values()).unwrap();
        // A value that contains `{{x}}` is kept verbatim, never re-scanned.
        assert_eq!(
            rendered,
            "Triage {{x}} ticket (max -42) at urn:t:1: a, b, c. Again -42."
        );

        let mut unfilled = four_values();
        unfilled.retain(|(name, _)| name != "items");
        assert_eq!(
            render_template(&template, body, &unfilled),
            Err(DefinitionRefusal::TemplateSlotUnfilled)
        );
        assert_eq!(
            DefinitionRefusal::TemplateSlotUnfilled.as_str(),
            "TEMPLATE_SLOT_UNFILLED"
        );

        let mut wrong = four_values();
        wrong[1].1 = SlotValue::Text("many".to_string());
        assert_eq!(
            render_template(&template, body, &wrong),
            Err(DefinitionRefusal::TemplateSlotType)
        );
        assert_eq!(
            DefinitionRefusal::TemplateSlotType.as_str(),
            "TEMPLATE_SLOT_TYPE"
        );

        assert_eq!(
            render_template(&template, "{{nope}}", &four_values()),
            Err(DefinitionRefusal::TemplateSlotUnfilled)
        );
    }

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
