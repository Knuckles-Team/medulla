//! Job-scoped load set question and answer (MEDULLA-DECISION-ENGINE-R176.1).

use serde::{Deserialize, Deserializer, Serialize};

use medulla_types::refusal_code::RefusalCode;

use super::PremiseRef;

/// The graph a load-set question reads capability classes from when it names none.
pub const DEFAULT_LOAD_SET_GRAPH: &str = "__commons__";

pub const DEFAULT_LOAD_SET_MAX_TOOLS: u16 = 32;
pub const LOAD_SET_MAX_TOOLS_LIMIT: u16 = 128;
pub const DEFAULT_LOAD_SET_MAX_AGENTS: u16 = 4;

fn default_max_tools() -> u16 {
    DEFAULT_LOAD_SET_MAX_TOOLS
}

fn default_max_agents() -> u16 {
    DEFAULT_LOAD_SET_MAX_AGENTS
}

fn bounded_max_tools<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u16, D::Error> {
    let value = u16::deserialize(deserializer)?;
    if value == 0 || value > LOAD_SET_MAX_TOOLS_LIMIT {
        return Err(serde::de::Error::custom(format!(
            "max_tools must be 1 to {LOAD_SET_MAX_TOOLS_LIMIT}"
        )));
    }
    Ok(value)
}

fn default_graph() -> String {
    DEFAULT_LOAD_SET_GRAPH.to_string()
}

/// What a job needs loaded. `job_embedding` is optional; the text alone is
/// enough to derive capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LoadSetQuestion {
    pub job_text: String,
    #[serde(default)]
    pub job_embedding: Vec<f32>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default = "default_max_tools", deserialize_with = "bounded_max_tools")]
    pub max_tools: u16,
    #[serde(default = "default_max_agents")]
    pub max_agents: u16,
    /// The graph whose composed schema holds the capability classes.
    #[serde(default = "default_graph")]
    pub graph: String,
    /// The embedding space `job_embedding` lives in: the digest a served
    /// caller reads through `DecisionLog.generation_space`. Required when
    /// `job_embedding` is present.
    #[serde(default)]
    pub embedding_version: String,
}

/// Equality compares the embedding bitwise, so it is a total equivalence
/// (`Eq`): a question is a typed parameter whose container requires `Eq`.
impl PartialEq for LoadSetQuestion {
    fn eq(&self, other: &Self) -> bool {
        self.job_text == other.job_text
            && self.job_embedding.len() == other.job_embedding.len()
            && self
                .job_embedding
                .iter()
                .zip(&other.job_embedding)
                .all(|(a, b)| a.to_bits() == b.to_bits())
            && self.required_capabilities == other.required_capabilities
            && self.max_tools == other.max_tools
            && self.max_agents == other.max_agents
            && self.graph == other.graph
            && self.embedding_version == other.embedding_version
    }
}

impl Eq for LoadSetQuestion {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AgentStart {
    pub agent_iri: String,
    pub prompt_iri: String,
    pub model_profile: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct LoadSetAnswer {
    pub tools: Vec<String>,
    pub skills: Vec<String>,
    pub agents: Vec<AgentStart>,
    pub graph_revision: u64,
    pub premises: Vec<PremiseRef>,
}

/// The question parameter a `load_set` request carries its body in.
pub const LOAD_SET_PARAM: &str = "load_set";

/// A required capability has no designated component; an abstention naming it
/// (MEDULLA-DECISION-ENGINE-R176.4).
pub const LOAD_SET_NO_DESIGNATED_COMPONENT: RefusalCode =
    RefusalCode::new("LOAD_SET_NO_DESIGNATED_COMPONENT");
/// A chosen component comes from a pack revision older than the newest one the
/// index holds for the same connector (MEDULLA-DECISION-ENGINE-R176.4).
pub const LOAD_SET_STALE_PACK_REVISION: RefusalCode =
    RefusalCode::new("LOAD_SET_STALE_PACK_REVISION");

/// The `load_set` family's refusal table (MEDULLA-DECISION-ENGINE-R176.4).
pub const REFUSALS: &[RefusalCode] = &[
    LOAD_SET_NO_DESIGNATED_COMPONENT,
    LOAD_SET_STALE_PACK_REVISION,
];
