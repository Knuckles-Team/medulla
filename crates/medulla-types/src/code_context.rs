//! Wire records of the served `CodeContext` and `Diagnose` reads
//! (MEDULLA-FEDERATED-QUERY-R654.1, R654.3). They live below the protocol crate so
//! the `Method` variants can name them; `medulla-plan` re-exports them and holds
//! the one implementation that fills them.

use serde::{Deserialize, Serialize};

/// Which code-context question a request asks (MEDULLA-FEDERATED-QUERY-R654.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeQuestion {
    How,
    Usage,
    Impact,
}

/// A served `CodeContext` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeContextRequest {
    /// Qualified symbol name, or a suffix of one.
    pub anchor: String,
    pub question: CodeQuestion,
    /// Walk depth in hops; `None` takes `knob code.context_depth`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    /// Visited-node cap; `None` takes `knob code.max_nodes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Outcome of a code-context walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeContextStatus {
    Ok,
    Ambiguous,
    Degraded,
}

/// One symbol reached by the walk with its hop count from the anchor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeNode {
    pub symbol: String,
    pub hops: u32,
}

/// A hub with its centrality inputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RankedHub {
    pub symbol: String,
    pub in_degree: u32,
    pub out_degree: u32,
    pub score: f64,
}

/// The served `CodeContext` answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeContextAnswer {
    pub status: CodeContextStatus,
    /// The resolved anchor; empty when `status` is `ambiguous`.
    pub anchors: Vec<String>,
    /// Symbols that reach the anchor (callers), by hop count.
    pub usages: Vec<CodeNode>,
    /// Symbols the anchor reaches (callees), by hop count.
    pub impact: Vec<CodeNode>,
    pub hubs: Vec<RankedHub>,
    /// Candidate anchors when `status` is `ambiguous`.
    pub candidates: Vec<String>,
}

/// The closed set of operations causal stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpsCausalStage {
    Change,
    Deploy,
    Config,
    Dependency,
    Resource,
    Symptom,
}

/// One link of a diagnosed causal chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CausalLink {
    pub stage: OpsCausalStage,
    pub node_id: String,
    pub relationship: String,
    pub evidence_ids: Vec<String>,
}

/// The served `Diagnose` answer: a symptom and its causal chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymptomDiagnosis {
    pub symptom: String,
    pub chain: Vec<CausalLink>,
}

/// A served `Diagnose` request: the `HealthAnomaly` node to explain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnoseRequest {
    /// Node id of the symptom.
    pub symptom: String,
}
