//! Connector usage guidance answered from the graph (MEDULLA-DECISION-ENGINE-R177.1).

use serde::{Deserialize, Deserializer, Serialize};

use medulla_types::refusal_code::RefusalCode;

fn required_tool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let tool = String::deserialize(deserializer)?;
    if tool.trim().is_empty() {
        return Err(serde::de::Error::custom("a usage step names a tool"));
    }
    Ok(tool)
}

/// One ordered step: the tool, the steps whose outputs feed it and the steps
/// it precedes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct UsageStep {
    #[serde(deserialize_with = "required_tool")]
    pub tool: String,
    #[serde(default)]
    pub inputs_from: Vec<String>,
    #[serde(default)]
    pub precedes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct UsageGuidance {
    pub connector: String,
    pub job_class: String,
    pub conditions: Vec<String>,
    pub steps: Vec<UsageStep>,
    pub prompts: Vec<String>,
    pub cautions: Vec<String>,
    pub graph_revision: u64,
}

/// The question parameter naming the connector node (`mcp-server://<name>`),
/// a `TypedValue::Iri` (MEDULLA-DECISION-ENGINE-R177.3).
pub const USAGE_GUIDANCE_CONNECTOR_PARAM: &str = "connector";
/// The question parameter naming the job class, a `TypedValue::Iri`.
pub const USAGE_GUIDANCE_JOB_CLASS_PARAM: &str = "job_class";
/// The optional question parameter naming the graph the pack was imported
/// into, a `TypedValue::Text`; `__commons__` when absent.
pub const USAGE_GUIDANCE_GRAPH_PARAM: &str = "graph";
/// The graph a usage guidance question reads when it names none.
pub const DEFAULT_USAGE_GUIDANCE_GRAPH: &str = "__commons__";

/// No imported pack declares the connector (MEDULLA-DECISION-ENGINE-R177.4).
pub const USAGE_GUIDANCE_UNKNOWN_CONNECTOR: RefusalCode =
    RefusalCode::new("USAGE_GUIDANCE_UNKNOWN_CONNECTOR");
/// The connector has no steps for the job class: an abstention.
pub const USAGE_GUIDANCE_NO_STEPS: RefusalCode = RefusalCode::new("USAGE_GUIDANCE_NO_STEPS");
/// The `:precedes` edges of the job class form a cycle.
pub const USAGE_GUIDANCE_CYCLIC_STEPS: RefusalCode =
    RefusalCode::new("USAGE_GUIDANCE_CYCLIC_STEPS");

/// The `usage_guidance` family's refusal table (MEDULLA-DECISION-ENGINE-R177.4).
pub const REFUSALS: &[RefusalCode] = &[
    USAGE_GUIDANCE_UNKNOWN_CONNECTOR,
    USAGE_GUIDANCE_NO_STEPS,
    USAGE_GUIDANCE_CYCLIC_STEPS,
];

#[cfg(test)]
mod tests;
