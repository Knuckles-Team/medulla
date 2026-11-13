//! Tool relevance for one task: the request and answer model
//! (MEDULLA-DECISION-ENGINE-R134.110.1).
//!
//! `Decide` with
//! [`QuestionKind::ToolRelevance`](super::statistical::QuestionKind::ToolRelevance)
//! ranks the tools a role may see against the task text (and, when the caller
//! supplies one, a Q32 fixed-point embedding of it) and selects a covering
//! subset inside a token budget. Every score is a Q32 integer, so the answer
//! digest is the same on every host. Nothing here reads a clock or a store.

use super::knobs::{KnobSafety, KnobSpec, KnobValue};
use super::policy::DecisionPolicy;
use medulla_types::contract::BoundedVec;
use medulla_types::refusal_code::RefusalCode;
use serde::{Deserialize, Deserializer, Serialize};

/// Current wire schema version of the three records below.
pub const TOOL_RELEVANCE_SCHEMA_VERSION: u32 = 1;
/// Fewest tools one request may ask for.
pub const MIN_TOP_K: u32 = 1;
/// Most tools one request may ask for. A `Fixed` safety bound.
pub const MAX_TOP_K: u32 = 50;
/// Most capability IRIs one tool may declare.
pub const MAX_TOOL_CAPABILITIES: usize = 64;
/// Most Q32 components one task embedding may hold.
pub const MAX_TASK_EMBEDDING: usize = 4096;
/// Most capabilities one request may require.
pub const MAX_REQUIRED_CAPABILITIES: usize = 64;

/// Refusal: `top_k` outside `1..=50`.
pub const TOP_K_OUT_OF_RANGE: RefusalCode = RefusalCode::new("TOP_K_OUT_OF_RANGE");

/// Refusal: the task embedding and a tool embedding differ in length.
pub const EMBEDDING_DIMENSION_MISMATCH: RefusalCode =
    RefusalCode::new("EMBEDDING_DIMENSION_MISMATCH");

/// Refusal: the relevance floor removed every tool
/// (MEDULLA-DECISION-ENGINE-R134.2).
pub const NO_RELEVANT_TOOL: RefusalCode = RefusalCode::new("NO_RELEVANT_TOOL");

/// Abstention: the tenant's visible catalog holds no tool
/// (MEDULLA-DECISION-ENGINE-R134.110.4).
pub const EMPTY_CATALOG: RefusalCode = RefusalCode::new("EMPTY_CATALOG");

/// Abstention: the budget is below the cheapest allowed tool
/// (MEDULLA-DECISION-ENGINE-R134.110.4).
pub const BUDGET_BELOW_MINIMUM: RefusalCode = RefusalCode::new("BUDGET_BELOW_MINIMUM");

/// Refusal: the visible catalog is larger than `tool_subset.max_tools`
/// (MEDULLA-DECISION-ENGINE-R134.110.4).
pub const CATALOG_TOO_LARGE: RefusalCode = RefusalCode::new("CATALOG_TOO_LARGE");

/// Refusal: `agent_role_iri` names no published toolset of the tenant
/// (MEDULLA-DECISION-ENGINE-R134.110.4).
pub const UNKNOWN_ROLE: RefusalCode = RefusalCode::new("UNKNOWN_ROLE");

/// Every refusal code of the tool-relevance family; joins
/// `codes::FAMILY_TABLES` under `QuestionKind::ToolRelevance`. The covering
/// subset refusals are the shared `TOOL_SUBSET_CAPABILITY_UNCOVERABLE` and
/// `TOOL_SUBSET_BUDGET_EXCEEDED` of `codes`.
pub const REFUSALS: &[RefusalCode] = &[
    TOP_K_OUT_OF_RANGE,
    EMBEDDING_DIMENSION_MISMATCH,
    NO_RELEVANT_TOOL,
    EMPTY_CATALOG,
    BUDGET_BELOW_MINIMUM,
    CATALOG_TOO_LARGE,
    UNKNOWN_ROLE,
];

/// The `tool_relevance.*` knobs: the reciprocal-rank-fusion constant, in rank
/// units (MEDULLA-DECISION-ENGINE-R134.110.2), then the relevance floor and the
/// number of tools kept before the covering subset (R134.2). All `Tunable`.
pub const KNOBS: &[KnobSpec] = &[
    KnobSpec {
        name: "tool_relevance.rrf_k",
        unit: "rank",
        default: KnobValue::Integer(60),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(1000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "tool_relevance.relevance_floor",
        unit: "fraction",
        default: KnobValue::DecimalMicros(200_000),
        min: KnobValue::DecimalMicros(0),
        max: KnobValue::DecimalMicros(1_000_000),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "tool_relevance.top_k",
        unit: "count",
        default: KnobValue::Integer(12),
        min: KnobValue::Integer(1),
        max: KnobValue::Integer(50),
        safety: KnobSafety::Tunable,
    },
    KnobSpec {
        name: "tool_subset.max_tools",
        unit: "tools",
        default: KnobValue::Integer(10_000),
        min: KnobValue::Integer(100),
        max: KnobValue::Integer(50_000),
        safety: KnobSafety::Tunable,
    },
];

/// The resolved `tool_subset.max_tools`, clamped to `[100, 50000]`
/// (MEDULLA-DECISION-ENGINE-R134.110.4): the largest visible catalog one
/// request may rank.
pub fn max_tools_from_policy(policy: &DecisionPolicy) -> usize {
    let value = match policy.knob(&KNOBS[3]) {
        KnobValue::Integer(v) | KnobValue::DecimalMicros(v) => v,
        KnobValue::DurationMs(v) => i64::try_from(v).unwrap_or(i64::MAX),
    };
    usize::try_from(value.clamp(100, 50_000)).unwrap_or(10_000)
}

/// The resolved ranking knobs of one request (MEDULLA-DECISION-ENGINE-R134.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolRankingKnobs {
    /// The relevance floor as a numerator over one million.
    pub relevance_floor_micros: u64,
    /// Tools kept before the covering subset, `1..=50`.
    pub top_k: u32,
}

impl ToolRankingKnobs {
    /// The values `policy` resolves, clamped to the declared ranges.
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let floor = match policy.knob(&KNOBS[1]) {
            KnobValue::DecimalMicros(v) | KnobValue::Integer(v) => v,
            KnobValue::DurationMs(v) => i64::try_from(v).unwrap_or(i64::MAX),
        };
        let top_k = match policy.knob(&KNOBS[2]) {
            KnobValue::Integer(v) | KnobValue::DecimalMicros(v) => v,
            KnobValue::DurationMs(v) => i64::try_from(v).unwrap_or(i64::MAX),
        };
        Self {
            relevance_floor_micros: u64::try_from(floor.clamp(0, 1_000_000)).unwrap_or(200_000),
            top_k: u32::try_from(top_k.clamp(i64::from(MIN_TOP_K), i64::from(MAX_TOP_K)))
                .unwrap_or(12),
        }
    }
}

/// Why a ranking produced no selection (MEDULLA-DECISION-ENGINE-R134.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRelevanceRefusal {
    /// The relevance floor removed every tool (`NO_RELEVANT_TOOL`).
    NoRelevantTool,
    /// The visible catalog is empty (`EMPTY_CATALOG`).
    EmptyCatalog,
    /// The budget is below the cheapest allowed tool (`BUDGET_BELOW_MINIMUM`).
    BudgetBelowMinimum,
    /// The covering subset refused (`TOOL_SUBSET_CAPABILITY_UNCOVERABLE` or
    /// `TOOL_SUBSET_BUDGET_EXCEEDED`).
    Subset(super::statistical::tool_subset::ToolSubsetRefusal),
}

impl From<super::statistical::tool_subset::ToolSubsetRefusal> for ToolRelevanceRefusal {
    fn from(refusal: super::statistical::tool_subset::ToolSubsetRefusal) -> Self {
        Self::Subset(refusal)
    }
}

impl super::record::outcome::DecisionRefusal for ToolRelevanceRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::NoRelevantTool => NO_RELEVANT_TOOL,
            Self::EmptyCatalog => EMPTY_CATALOG,
            Self::BudgetBelowMinimum => BUDGET_BELOW_MINIMUM,
            Self::Subset(inner) => super::record::outcome::DecisionRefusal::code(inner),
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::NoRelevantTool => "the relevance floor removed every tool".to_string(),
            Self::EmptyCatalog => "the visible tool catalog is empty".to_string(),
            Self::BudgetBelowMinimum => {
                "the context budget is below the cheapest allowed tool".to_string()
            }
            Self::Subset(inner) => super::record::outcome::DecisionRefusal::detail(inner),
        }
    }
}

/// The tools a ranking keeps and the subset it selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRanking {
    /// The tools that passed the floor, best first, cut at `top_k`: id and
    /// Q32 relevance.
    pub kept: Vec<(String, i64)>,
    /// The tools selected inside the budget, in selection order.
    pub selected_tool_ids: Vec<String>,
}

/// The resolved `tool_relevance.rrf_k`, clamped to `[1, 1000]`.
pub fn rrf_k_from_policy(policy: &DecisionPolicy) -> u32 {
    let value = match policy.knob(&KNOBS[0]) {
        KnobValue::Integer(v) | KnobValue::DecimalMicros(v) => v,
        KnobValue::DurationMs(v) => i64::try_from(v).unwrap_or(i64::MAX),
    };
    u32::try_from(value.clamp(1, 1000)).unwrap_or(60)
}

fn bounded_top_k<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let value = u32::deserialize(deserializer)?;
    check_top_k(value).map_err(serde::de::Error::custom)?;
    Ok(value)
}

fn check_top_k(top_k: u32) -> Result<(), ToolRelevanceError> {
    if (MIN_TOP_K..=MAX_TOP_K).contains(&top_k) {
        Ok(())
    } else {
        Err(ToolRelevanceError::TopKOutOfRange { top_k })
    }
}

/// One tool in the index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ToolEntry {
    pub schema_version: u32,
    pub tool_id: String,
    pub server_id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub capability_iris: BoundedVec<String, MAX_TOOL_CAPABILITIES>,
    /// What loading this tool costs in context tokens.
    pub token_cost: u32,
}

/// The question: which tools should this role see for this task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ToolAssignmentRequest {
    pub schema_version: u32,
    pub task_text: String,
    /// Q32 fixed-point components of the task embedding; absent ranks on text alone.
    #[serde(default)]
    pub task_embedding: Option<BoundedVec<i64, MAX_TASK_EMBEDDING>>,
    pub agent_role_iri: String,
    /// `1..=50`, refused as `TOP_K_OUT_OF_RANGE` outside it.
    #[serde(deserialize_with = "bounded_top_k")]
    pub top_k: u32,
    pub context_budget_tokens: u32,
    #[serde(default)]
    pub required_capabilities: BoundedVec<String, MAX_REQUIRED_CAPABILITIES>,
}

/// One ranked tool with the evidence behind its rank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AssignedTool {
    pub tool_id: String,
    pub server_id: String,
    /// 1-based rank in the lexical list; absent when the tool did not match.
    #[serde(default)]
    pub lexical_rank: Option<u32>,
    /// 1-based rank in the embedding list; absent without an embedding.
    #[serde(default)]
    pub embedding_rank: Option<u32>,
    pub fused_score_q32: i64,
    pub relevance_q32: i64,
    /// 1-based final rank.
    pub rank: u32,
}

/// The answer: every ranked tool and the subset selected inside the budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ToolAssignment {
    pub schema_version: u32,
    /// The monotonic library generation the index was built from.
    pub index_revision: u64,
    pub ranked: Vec<AssignedTool>,
    pub selected_tool_ids: Vec<String>,
}

/// Why a request was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRelevanceError {
    /// `top_k` outside `1..=50` (`TOP_K_OUT_OF_RANGE`).
    TopKOutOfRange { top_k: u32 },
    /// The task and a tool embedding differ in length
    /// (`EMBEDDING_DIMENSION_MISMATCH`).
    EmbeddingDimensionMismatch {
        tool_id: String,
        task: usize,
        tool: usize,
    },
    /// The wire value did not decode.
    Malformed(String),
}

impl ToolRelevanceError {
    /// The registered refusal code, when the error is one.
    pub fn code(&self) -> Option<RefusalCode> {
        match self {
            Self::TopKOutOfRange { .. } => Some(TOP_K_OUT_OF_RANGE),
            Self::EmbeddingDimensionMismatch { .. } => Some(EMBEDDING_DIMENSION_MISMATCH),
            Self::Malformed(_) => None,
        }
    }
}

impl std::fmt::Display for ToolRelevanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TopKOutOfRange { top_k } => write!(
                f,
                "{}: top_k {top_k} is outside {MIN_TOP_K}..={MAX_TOP_K}",
                TOP_K_OUT_OF_RANGE.as_str()
            ),
            Self::EmbeddingDimensionMismatch {
                tool_id,
                task,
                tool,
            } => write!(
                f,
                "{}: task embedding has {task} components, tool {tool_id} has {tool}",
                EMBEDDING_DIMENSION_MISMATCH.as_str()
            ),
            Self::Malformed(detail) => write!(f, "tool request is malformed: {detail}"),
        }
    }
}

impl std::error::Error for ToolRelevanceError {}

impl ToolAssignmentRequest {
    /// Decode a wire value. A `top_k` outside `1..=50` is `TOP_K_OUT_OF_RANGE`,
    /// whatever else is wrong with the request.
    pub fn from_value(value: serde_json::Value) -> Result<Self, ToolRelevanceError> {
        if let Some(top_k) = value.get("top_k").and_then(|top_k| top_k.as_i64()) {
            if top_k < i64::from(MIN_TOP_K) || top_k > i64::from(MAX_TOP_K) {
                let top_k = u32::try_from(top_k).unwrap_or(u32::MAX);
                return Err(ToolRelevanceError::TopKOutOfRange { top_k });
            }
        }
        serde_json::from_value(value)
            .map_err(|error| ToolRelevanceError::Malformed(error.to_string()))
    }

    /// Check `top_k` for a request built in memory.
    pub fn validate(&self) -> Result<(), ToolRelevanceError> {
        check_top_k(self.top_k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(top_k: u32) -> ToolAssignmentRequest {
        ToolAssignmentRequest {
            schema_version: TOOL_RELEVANCE_SCHEMA_VERSION,
            task_text: "export the quarterly report".into(),
            task_embedding: Some(BoundedVec::new(vec![1 << 32, -(1 << 31), 0]).unwrap()),
            agent_role_iri: "https://knuckles-team.github.io/ontology/core#Analyst".into(),
            top_k,
            context_budget_tokens: 2000,
            required_capabilities: BoundedVec::new(vec!["cap:export".to_string()]).unwrap(),
        }
    }

    fn assignment() -> ToolAssignment {
        ToolAssignment {
            schema_version: TOOL_RELEVANCE_SCHEMA_VERSION,
            index_revision: 9,
            ranked: vec![
                AssignedTool {
                    tool_id: "t1".into(),
                    server_id: "s1".into(),
                    lexical_rank: Some(1),
                    embedding_rank: None,
                    fused_score_q32: 3 << 30,
                    relevance_q32: 1 << 31,
                    rank: 1,
                },
                AssignedTool {
                    tool_id: "t2".into(),
                    server_id: "s2".into(),
                    lexical_rank: None,
                    embedding_rank: Some(2),
                    fused_score_q32: 1 << 30,
                    relevance_q32: 1 << 30,
                    rank: 2,
                },
            ],
            selected_tool_ids: vec!["t1".into()],
        }
    }

    fn entry() -> ToolEntry {
        ToolEntry {
            schema_version: TOOL_RELEVANCE_SCHEMA_VERSION,
            tool_id: "t1".into(),
            server_id: "s1".into(),
            name: "export_report".into(),
            description: "Exports a report".into(),
            capability_iris: BoundedVec::new(vec!["cap:export".to_string()]).unwrap(),
            token_cost: 120,
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R134.110.1
    #[test]
    fn assignment_round_trips() {
        // Every field of all three records survives JSON and MessagePack.
        let original = request(50);
        original.validate().unwrap();
        let wire = serde_json::to_value(&original).unwrap();
        assert_eq!(
            ToolAssignmentRequest::from_value(wire.clone()).unwrap(),
            original
        );
        let packed = rmp_serde::to_vec_named(&original).unwrap();
        let unpacked: ToolAssignmentRequest = rmp_serde::from_slice(&packed).unwrap();
        assert_eq!(unpacked, original);

        let answer = assignment();
        let wire = serde_json::to_value(&answer).unwrap();
        assert_eq!(
            serde_json::from_value::<ToolAssignment>(wire).unwrap(),
            answer
        );
        let packed = rmp_serde::to_vec_named(&answer).unwrap();
        assert_eq!(
            rmp_serde::from_slice::<ToolAssignment>(&packed).unwrap(),
            answer
        );

        let tool = entry();
        let wire = serde_json::to_value(&tool).unwrap();
        assert_eq!(serde_json::from_value::<ToolEntry>(wire).unwrap(), tool);
        let packed = rmp_serde::to_vec_named(&tool).unwrap();
        assert_eq!(rmp_serde::from_slice::<ToolEntry>(&packed).unwrap(), tool);

        // The kind and the typed value carry their wire names.
        assert_eq!(
            serde_json::to_string(&crate::decision::statistical::QuestionKind::ToolRelevance)
                .unwrap(),
            "\"tool_relevance\""
        );
        let typed = crate::decision::statistical::TypedValue::ToolAssignment(Box::new(request(5)));
        let wire = serde_json::to_value(&typed).unwrap();
        assert_eq!(
            serde_json::from_value::<crate::decision::statistical::TypedValue>(wire).unwrap(),
            typed
        );

        // top_k 1 and 50 are accepted; 0 and 51 are TOP_K_OUT_OF_RANGE.
        request(1).validate().unwrap();
        for bad in [0u32, 51] {
            let refused = request(bad).validate().unwrap_err();
            assert_eq!(refused, ToolRelevanceError::TopKOutOfRange { top_k: bad });
            assert_eq!(refused.code(), Some(TOP_K_OUT_OF_RANGE));
            assert!(refused.to_string().starts_with("TOP_K_OUT_OF_RANGE"));
            let mut wire = serde_json::to_value(request(5)).unwrap();
            wire["top_k"] = serde_json::json!(bad);
            let refused = ToolAssignmentRequest::from_value(wire.clone()).unwrap_err();
            assert_eq!(refused.code(), Some(TOP_K_OUT_OF_RANGE));
            // The typed value refuses it at the serde boundary too.
            let typed = serde_json::json!({"type": "tool_assignment", "value": wire});
            let error = serde_json::from_value::<crate::decision::statistical::TypedValue>(typed)
                .unwrap_err()
                .to_string();
            assert!(error.contains("TOP_K_OUT_OF_RANGE"), "{error}");
        }

        // An unknown field is refused.
        let mut unknown = serde_json::to_value(request(5)).unwrap();
        unknown["extra"] = serde_json::json!(1);
        assert!(matches!(
            ToolAssignmentRequest::from_value(unknown),
            Err(ToolRelevanceError::Malformed(_))
        ));
    }
}
