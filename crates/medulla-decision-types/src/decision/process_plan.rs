//! MEDULLA-CQ-ORCH-R704.1: the request and binding records of the process plan
//! question -- which workflow version to plan and what each step of it runs.
//!
//! The family of R704.4 reads the request and returns the bindings with the
//! plan; nothing here decides anything. Every type refuses unknown fields.

use serde::{Deserialize, Serialize};

use super::knobs::{KnobSafety, KnobSpec, KnobValue};

/// The `process_plan.*` knobs (MEDULLA-CQ-ORCH-R704.3, R704.4): the least relevance a ranked
/// tool needs to bind an unbound agent step, and how many `call` levels a plan inlines.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "process_plan.min_tool_score",
        unit: "fraction",
        default: KnobValue::DecimalMicros(600_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "process_plan.max_call_depth",
        unit: "levels",
        default: KnobValue::Integer(4),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(16),
        safety: KnobSafety::Tunable,
    },
];

/// Plan the stored workflow `workflow_id` at `version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProcessPlanRequest {
    pub workflow_id: String,
    pub version: u32,
}

/// What one workflow step runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum BindingTarget {
    /// A connector tool, with the IRI of its contract.
    Tool {
        server: String,
        tool: String,
        contract_iri: String,
    },
    /// A prompt run by an agent of `agent_kind`.
    Prompt {
        prompt_iri: String,
        agent_kind: String,
    },
    /// A stored decision table.
    Decision { decision_iri: String },
    /// A human approval by `approver_role`.
    Approval { approver_role: String },
}

/// Where a binding came from: the graph names it, or the ranker chose it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum BindingSource {
    /// The workflow step names its target in the graph.
    Explicit,
    /// The ranker chose the target with this Q32 score.
    Ranked { score_q32: i64 },
}

/// The binding of one workflow step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StepBinding {
    pub step_id: String,
    pub target: BindingTarget,
    pub source: BindingSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-CQ-ORCH-R704.1
    #[test]
    fn binding_records_round_trip() {
        let request = ProcessPlanRequest {
            workflow_id: "workflow:triage".to_string(),
            version: 3,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<ProcessPlanRequest>(&json).unwrap(),
            request
        );

        let bindings = vec![
            StepBinding {
                step_id: "collect".to_string(),
                target: BindingTarget::Tool {
                    server: "tickets".to_string(),
                    tool: "find_ticket".to_string(),
                    contract_iri: "urn:contract:find-ticket".to_string(),
                },
                source: BindingSource::Explicit,
            },
            StepBinding {
                step_id: "draft".to_string(),
                target: BindingTarget::Prompt {
                    prompt_iri: "urn:prompt:draft".to_string(),
                    agent_kind: "writer".to_string(),
                },
                source: BindingSource::Ranked { score_q32: 42 },
            },
            StepBinding {
                step_id: "route".to_string(),
                target: BindingTarget::Decision {
                    decision_iri: "urn:decision:route".to_string(),
                },
                source: BindingSource::Explicit,
            },
            StepBinding {
                step_id: "sign".to_string(),
                target: BindingTarget::Approval {
                    approver_role: "manager".to_string(),
                },
                source: BindingSource::Ranked { score_q32: -1 },
            },
        ];
        for binding in &bindings {
            let json = serde_json::to_string(binding).unwrap();
            assert_eq!(
                &serde_json::from_str::<StepBinding>(&json).unwrap(),
                binding
            );
        }
        let json = serde_json::to_string(&bindings[1]).unwrap();
        assert!(json.contains(r#""kind":"prompt""#), "{json}");
        assert!(json.contains(r#""kind":"ranked""#), "{json}");

        // Unknown fields and unknown kinds refuse.
        assert!(serde_json::from_str::<ProcessPlanRequest>(
            r#"{"workflow_id":"w","version":1,"extra":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<StepBinding>(
            r#"{"step_id":"s","target":{"kind":"approval","approver_role":"r"},"source":{"kind":"explicit"},"x":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<BindingTarget>(
            r#"{"kind":"approval","approver_role":"r","x":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<BindingTarget>(r#"{"kind":"shell","command":"x"}"#).is_err());
    }
}
