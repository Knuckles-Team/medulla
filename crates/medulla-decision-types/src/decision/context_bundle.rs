//! Context bundle wire records (MEDULLA-FEDERATED-QUERY-R650.1).
//!
//! The request, items, citations, evidence and bundle of one served
//! `ContextCompile`. They sit below the protocol crate so the method can carry
//! them; `medulla_plan::context_bundle` re-exports them and owns the compile.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::context_package::{ContextPackageBudget, ContextPackageSelection};

/// Why a rerank fell back to the input order. Closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    ScorerUnavailable,
    NoCalibration,
}

/// One stage of the compile pipeline, in the vocabulary the evidence reports
/// (MEDULLA-FEDERATED-QUERY-R652.3). Closed. `Boost` joins the order when the
/// feedback boost runs inside `ContextCompile` (MEDULLA-FEDERATED-QUERY-R705).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PipelineStage {
    Fuse,
    Rerank,
    Calibrate,
    Cut,
    Select,
}

/// Default `top_n`: candidates kept per source before fusion.
pub const DEFAULT_TOP_N: u32 = 256;
/// Largest accepted `top_n`.
pub const MAX_TOP_N: u32 = 1_024;

fn default_top_n() -> u32 {
    DEFAULT_TOP_N
}

/// Which retrieval arms feed the bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ContextMode {
    /// Lexical, semantic and graph arms fused.
    #[default]
    Hybrid,
    /// The lexical arm only.
    Lexical,
    /// The semantic arm only.
    Semantic,
}

/// A served `ContextCompile` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ContextBundleRequest {
    pub query_text: String,
    #[serde(default)]
    pub query_embedding: Vec<f32>,
    pub budget: ContextPackageBudget,
    pub tokenizer_id: String,
    pub tokenizer_version: String,
    #[serde(default)]
    pub mode: ContextMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_ref: Option<String>,
    #[serde(default = "default_top_n")]
    pub top_n: u32,
}

impl ContextBundleRequest {
    /// `top_n` must stay inside [1, 1,024].
    pub fn top_n_in_range(&self) -> bool {
        (1..=MAX_TOP_N).contains(&self.top_n)
    }
}

/// Where a cited span came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Citation {
    pub source_id: String,
    pub cited_start: u64,
    pub cited_end: u64,
    pub source_revision: u64,
    pub content_digest: String,
    /// Source URL of a web Document (MEDULLA-FEDERATED-QUERY-R653.7); absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Unix seconds at which the web Document was retrieved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieved_at: Option<i64>,
    /// The search or fetch backend that produced the web Document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// SHA-256 of the query that found the web Document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_digest: Option<String>,
}

impl Citation {
    /// A citation with no web provenance.
    pub fn local(
        source_id: String,
        cited_start: u64,
        cited_end: u64,
        source_revision: u64,
        content_digest: String,
    ) -> Self {
        Self {
            source_id,
            cited_start,
            cited_end,
            source_revision,
            content_digest,
            url: None,
            retrieved_at: None,
            backend: None,
            query_digest: None,
        }
    }
}

/// One proof edge from an item to the evidence it rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ProofEdge {
    pub from: String,
    pub relationship: String,
    pub to: String,
}

/// One item of a bundle at the resolution the budget allowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ContextItem {
    pub item_id: String,
    pub resolution: u8,
    pub tokens: u64,
    pub value_micro: u64,
    pub citation: Citation,
    pub proof_edges: Vec<ProofEdge>,
}

/// Why an item is not in the bundle. Closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    BelowGap,
    OverBudget,
    Denied,
    OutOfScope,
    Dominated,
}

/// An item left out, with its reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ExcludedItem {
    pub item_id: String,
    pub reason: ExclusionReason,
}

/// The stable cache key of a bundle: SHA-256 hex of tenant, request, graph
/// revision and policy digest.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct BundleKey(pub String);

/// Why a record was dropped by the access check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DroppedReason {
    Denied,
    Timeout,
    Unavailable,
}

/// A record the access check dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DroppedRecord {
    pub record_id: String,
    pub reason: DroppedReason,
}

/// One raw and calibrated input score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct InputScore {
    pub source: String,
    pub node_id: String,
    pub raw: f32,
    pub calibrated: f32,
}

/// How one source was normalised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SourceNormalization {
    pub source: String,
    /// The pinned curve digest, or `None` for min-max scaling.
    pub curve_digest: Option<String>,
    pub fallback: Option<FallbackReason>,
}

/// The inputs the key hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct KeyInputs {
    pub tenant: String,
    pub graph_revision: u64,
    pub policy_digest: String,
}

/// Inline evidence of one compile. Nothing is stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BundleEvidence {
    pub input_scores: Vec<InputScore>,
    pub normalization: Vec<SourceNormalization>,
    pub rrf_k: u32,
    pub selected: Vec<String>,
    pub excluded: Vec<ExcludedItem>,
    pub resolutions: BTreeMap<String, u8>,
    pub budget_used_tokens: u64,
    pub certificate_bound_tokens: u64,
    pub key_inputs: KeyInputs,
    pub unchecked_dropped: Vec<DroppedRecord>,
    /// What the rerank stage did; absent when the compile ran no rerank
    /// (no query text to score against).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank: Option<RerankEvidence>,
    /// The stages this compile ran, in the order it ran them.
    #[serde(default)]
    pub stages: Vec<PipelineStage>,
}

/// What one rerank did, inline in the bundle evidence
/// (MEDULLA-FEDERATED-QUERY-R652.5, R652.7). `scorer_*` name the scorer that
/// produced the raw scores; for a model scorer the version is the pinned
/// artifact revision. `input_ranks[i]` is the fused rank of the i-th scored
/// candidate and `output_ranks[i]` the rank it holds after the rerank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RerankEvidence {
    pub scorer_name: String,
    pub scorer_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_digest: Option<String>,
    pub input_ranks: Vec<u32>,
    pub output_ranks: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<FallbackReason>,
}

/// The compiled bundle. `certificate` is `None` only when no candidate
/// survived, so there was nothing to solve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ContextBundle {
    pub items: Vec<ContextItem>,
    pub excluded: Vec<ExcludedItem>,
    pub certificate: Option<ContextPackageSelection>,
    pub key: BundleKey,
    pub evidence: BundleEvidence,
}
