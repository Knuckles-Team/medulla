//! What a pack entry DECLARES about itself.
//!
//! Every value here is a publisher claim. The engine records it as one: an
//! imported cost becomes [`crate::agent_component::FactQuality::Declared`] with
//! a [`crate::agent_component::PriceSource::ConnectorPack`] naming the entry it
//! came from, so a decision that used it can say exactly whose number it was.

use serde::{Deserialize, Serialize};

use medulla_agent_types::{
    agent_component::DeclaredCost, agent_component::DeclaredLatency, agent_component::MeasuredFact,
    agent_component::ModelInputCapabilities, agent_component::ShellProficiency,
    agent_component::StoppingBehaviour, agent_component::ToolFactDeclarer,
    agent_component::ToolSideEffects,
};
use medulla_types::contract::BoundedVec;

/// Declared per-invocation prices, in integer micros — the same shape an
/// [`crate::agent_component::CostFacts`] carries once the engine has recorded
/// who declared it, so a pack entry's raw claim and the digested fact can
/// never drift into two different ideas of "cost".
pub type PackCost = DeclaredCost;

/// A model profile a pack publishes.
///
/// The three `supports_*` flags are tri-state here and `bool` on the component
/// facts, because absent is not false: a pack that leaves one out has said
/// nothing, and importing that as `false` would silently make the model
/// ineligible. An entry with any of them absent is refused `INVALID_FACTS`
/// rather than imported with an invented answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PackModelFacts {
    pub provider: String,
    pub model_identity: String,
    #[serde(alias = "contextWindowTokens")]
    pub context_window_tokens: u64,
    pub max_output_tokens: u64,
    #[serde(default)]
    pub supports_tools: Option<bool>,
    #[serde(default)]
    pub supports_structured_output: Option<bool>,
    #[serde(default)]
    pub supports_vision: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_capabilities: Option<ModelInputCapabilities>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "shellProficiency"
    )]
    pub shell_proficiency: Option<MeasuredFact<ShellProficiency>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "stoppingBehaviour"
    )]
    pub stopping_behaviour: Option<MeasuredFact<StoppingBehaviour>>,
}

/// Everything one entry declares beyond its bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PackAnnotations {
    #[serde(default)]
    pub provides: BoundedVec<String, 64>,
    #[serde(default)]
    pub requires_capabilities: BoundedVec<String, 64>,
    #[serde(default)]
    pub modalities_in: BoundedVec<String, 64>,
    #[serde(default)]
    pub modalities_out: BoundedVec<String, 64>,
    #[serde(default)]
    pub required_scopes: BoundedVec<String, 64>,
    #[serde(default)]
    pub read_only_hint: Option<bool>,
    #[serde(default)]
    pub destructive_hint: Option<bool>,
    #[serde(default)]
    pub idempotent_hint: Option<bool>,
    #[serde(default)]
    pub open_world_hint: Option<bool>,
    #[serde(default)]
    pub contract_version: Option<String>,
    #[serde(default)]
    pub cost: Option<PackCost>,
    #[serde(default)]
    pub latency_declared: Option<DeclaredLatency>,
    #[serde(default)]
    pub model: Option<PackModelFacts>,
    /// The SDK contract the publisher pinned. Stored as a claim ATTRIBUTE
    /// rather than a typed fact: the engine cannot recompute the SDK's own
    /// normalization, so it can record the string and must not pretend to have
    /// checked it.
    #[serde(default)]
    pub sdk_contract_pin: Option<String>,
    /// How the SDK exposes a tool's arguments: `condensed` for the single
    /// `action` + `params_json` shape, `verbose` otherwise. A declaration of
    /// the pack builder, which has the schema; the engine never re-derives it.
    /// Meaningful on tools only.
    #[serde(default)]
    pub tool_mode: Option<PackToolMode>,
    /// What the tool may change. Absent is unknown; see
    /// [`PackAnnotations::effective_side_effects`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_effects: Option<ToolSideEffects>,
    /// Who declared the effect facts. Absent reads as `connector`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_by: Option<ToolFactDeclarer>,
    /// The tool schema's size in tokens. Absent is unknown, never zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_tokens: Option<u32>,
}

impl PackAnnotations {
    /// An absent read-only hint is `false`: nothing has claimed the tool is safe.
    pub fn effective_read_only(&self) -> bool {
        self.read_only_hint.unwrap_or(false)
    }

    /// Absent side effects read as the least permissive answer.
    pub fn effective_side_effects(&self) -> ToolSideEffects {
        self.side_effects.unwrap_or(ToolSideEffects::ExternalWrite)
    }

    /// Absent declarer reads as the connector author.
    pub fn effective_declared_by(&self) -> ToolFactDeclarer {
        self.declared_by.unwrap_or(ToolFactDeclarer::Connector)
    }

    /// The declared schema size. Absent stays absent; no caller may read it as zero.
    pub fn effective_schema_tokens(&self) -> Option<u32> {
        self.schema_tokens
    }

    /// A read-only hint is unproven when it is only an MCP hint.
    pub fn read_only_unproven(&self) -> bool {
        self.effective_read_only() && self.effective_declared_by() == ToolFactDeclarer::McpHint
    }
}

/// How an SDK tool presents its arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum PackToolMode {
    Condensed,
    Verbose,
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_agent_types::agent_component::{ModelInputMediaType, ObservationRef};

    /// Verifies MEDULLA-TYPED-PACKS-R045.1: Annotation type carries side effects, declarer and schema size
    // spec: MEDULLA-TYPED-PACKS-R045.1, MEDULLA-TYPED-PACKS-R001.2
    #[test]
    fn new_fields_roundtrip() {
        for side_effects in [
            ToolSideEffects::None,
            ToolSideEffects::WorkspaceWrite,
            ToolSideEffects::ExternalWrite,
            ToolSideEffects::Network,
        ] {
            for declared_by in [
                ToolFactDeclarer::Connector,
                ToolFactDeclarer::McpHint,
                ToolFactDeclarer::Certified,
            ] {
                let annotations = PackAnnotations {
                    side_effects: Some(side_effects),
                    declared_by: Some(declared_by),
                    schema_tokens: Some(812),
                    ..PackAnnotations::default()
                };
                let wire = serde_json::to_value(&annotations).unwrap();
                assert_eq!(wire["side_effects"], side_effects.as_str());
                assert_eq!(wire["declared_by"], declared_by.as_str());
                assert_eq!(wire["schema_tokens"], 812);
                assert_eq!(
                    serde_json::from_value::<PackAnnotations>(wire).unwrap(),
                    annotations
                );
            }
        }
        let legacy: PackAnnotations = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(legacy.side_effects, None);
        assert_eq!(legacy.declared_by, None);
        assert_eq!(legacy.schema_tokens, None);
        let wire = serde_json::to_value(&legacy).unwrap();
        for field in ["side_effects", "declared_by", "schema_tokens"] {
            assert!(wire.get(field).is_none(), "{field} must stay off the wire");
        }

        // spec: MEDULLA-TYPED-PACKS-R001.2 — every section and annotation field
        // round-trips, and malformed typed annotations refuse.
        let full = PackAnnotations {
            provides: BoundedVec::new(vec!["tool:search".to_string()]).unwrap(),
            requires_capabilities: BoundedVec::new(vec!["cap:read".to_string()]).unwrap(),
            modalities_in: BoundedVec::new(vec!["eg:modality/text".to_string()]).unwrap(),
            modalities_out: BoundedVec::new(vec!["eg:modality/text".to_string()]).unwrap(),
            required_scopes: BoundedVec::new(vec!["kg:read".to_string()]).unwrap(),
            read_only_hint: Some(true),
            destructive_hint: Some(true),
            idempotent_hint: Some(false),
            open_world_hint: Some(true),
            contract_version: Some("1.2.3".to_string()),
            cost: Some(DeclaredCost {
                currency: "USD".to_string(),
                per_call_micros: Some(5),
                input_per_mtok_micros: Some(100),
                output_per_mtok_micros: Some(200),
            }),
            latency_declared: Some(DeclaredLatency {
                p50_ms: 120,
                p95_ms: 480,
            }),
            model: Some(PackModelFacts {
                provider: "acme".to_string(),
                model_identity: "acme-1".to_string(),
                context_window_tokens: 128_000,
                max_output_tokens: 4_096,
                supports_tools: Some(true),
                supports_structured_output: Some(false),
                supports_vision: Some(true),
                input_capabilities: Some(ModelInputCapabilities {
                    media_types: vec![ModelInputMediaType::Text, ModelInputMediaType::Png],
                    max_aggregate_bytes: 1024,
                    max_parts: 4,
                }),
                shell_proficiency: Some(MeasuredFact {
                    value: ShellProficiency::High,
                    evaluation_run_ref: ObservationRef {
                        evaluation_id: "eval-shell".to_string(),
                        digest: "3".repeat(64),
                    },
                }),
                stopping_behaviour: Some(MeasuredFact {
                    value: StoppingBehaviour::Balanced,
                    evaluation_run_ref: ObservationRef {
                        evaluation_id: "eval-stopping".to_string(),
                        digest: "a".repeat(64),
                    },
                }),
            }),
            sdk_contract_pin: Some("graft-sdk-1".to_string()),
            tool_mode: Some(PackToolMode::Condensed),
            side_effects: Some(ToolSideEffects::Network),
            declared_by: Some(ToolFactDeclarer::Certified),
            schema_tokens: Some(4096),
        };
        let wire = serde_json::to_value(&full).unwrap();
        assert_eq!(
            serde_json::from_value::<PackAnnotations>(wire).unwrap(),
            full
        );
        // The wire encoding is MessagePack on the import path.
        let pack_bytes = rmp_serde::to_vec_named(&full).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<PackAnnotations>(&pack_bytes).unwrap(),
            full
        );

        for (wire, why) in [
            (serde_json::json!({ "unexpected": 1 }), "an unknown field"),
            (
                serde_json::json!({ "tool_mode": "compact" }),
                "an unknown tool mode",
            ),
            (
                serde_json::json!({ "cost": { "currency": "USD", "per_call_micros": "many" } }),
                "a non-numeric cost",
            ),
            (
                serde_json::json!({
                    "model": {
                        "provider": "p",
                        "model_identity": "m",
                        "context_window_tokens": "big",
                        "max_output_tokens": 1
                    }
                }),
                "a non-numeric model field",
            ),
            (
                serde_json::json!({
                    "shell_proficiency": {
                        "value": "expert",
                        "evaluation_run_ref": { "evaluation_id": "e", "digest": "d" }
                    }
                }),
                "an unknown proficiency value",
            ),
            (
                serde_json::json!({
                    "declared_by": "certified",
                    "shell_proficiency": {
                        "value": "high",
                        "evaluation_run_ref": {
                            "evaluation_id": " padded",
                            "digest": "d"
                        }
                    }
                }),
                "a padded evaluation id",
            ),
        ] {
            assert!(
                serde_json::from_value::<PackAnnotations>(wire).is_err(),
                "{why} must refuse"
            );
        }
    }

    /// Verifies MEDULLA-TYPED-PACKS-R045.2: Absent facts take safe defaults, never zero
    // spec: MEDULLA-TYPED-PACKS-R045.2
    #[test]
    fn absent_facts_take_safe_defaults() {
        let absent = PackAnnotations::default();
        assert!(!absent.effective_read_only());
        assert_eq!(
            absent.effective_side_effects(),
            ToolSideEffects::ExternalWrite
        );
        assert_eq!(absent.effective_declared_by(), ToolFactDeclarer::Connector);
        assert_eq!(absent.effective_schema_tokens(), None);
        assert!(!absent.read_only_unproven());
        let hinted = PackAnnotations {
            read_only_hint: Some(true),
            declared_by: Some(ToolFactDeclarer::McpHint),
            ..PackAnnotations::default()
        };
        assert!(hinted.effective_read_only());
        assert!(hinted.read_only_unproven());
        let certified = PackAnnotations {
            declared_by: Some(ToolFactDeclarer::Certified),
            ..hinted
        };
        assert!(!certified.read_only_unproven());
    }

    /// Verifies MEDULLA-TYPED-PACKS-R045.4: Admission refuses an unknown side-effect or declarer value
    // spec: MEDULLA-TYPED-PACKS-R045.4
    #[test]
    fn unknown_closed_values_are_refused_naming_the_field() {
        for (field, value) in [("side_effects", "filesystem"), ("declared_by", "vendor")] {
            let error =
                serde_json::from_value::<PackAnnotations>(serde_json::json!({ field: value }))
                    .expect_err("an unknown closed value must refuse");
            assert!(
                error
                    .to_string()
                    .contains(&format!("{field}: unknown value `{value}`")),
                "{error}"
            );
        }
    }
}
