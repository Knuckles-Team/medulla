//! Deterministic code-subgraph hub ranking for the R654.2 behavior slice.
//!
//! The full code-context surface still needs symbol-index anchor resolution,
//! bounded caller/callee expansion, operations diagnosis and served wiring.
//! This module owns only the reusable hub-ranking seam: it converts the
//! caller/callee edges already collected by a future context planner into the
//! existing `medulla-compute` graph representation and orders hubs by directed
//! betweenness centrality with a stable symbol tie-break.

use medulla_graph_algos::{graph_algos::betweenness_centrality, graph_algos::AdjacencyGraph};
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
    /// Row cap; `None` takes the walk cap.
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
    pub anchors: Vec<String>,
    pub usages: Vec<CodeNode>,
    pub impact: Vec<CodeNode>,
    pub hubs: Vec<RankedHub>,
    /// Candidate anchors when `status` is `ambiguous`.
    pub candidates: Vec<String>,
}

/// The closed set of operations causal stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// A directed caller-to-callee edge in a resolved code subgraph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeEdge {
    pub caller: String,
    pub callee: String,
}

impl CodeEdge {
    /// Construct one edge from the canonical symbol identifiers.
    pub fn new(caller: impl Into<String>, callee: impl Into<String>) -> Self {
        Self {
            caller: caller.into(),
            callee: callee.into(),
        }
    }
}

/// A code symbol ranked as a hub of the supplied subgraph.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeHub {
    /// Canonical symbol identifier.
    pub symbol: String,
    /// Directed betweenness centrality over the supplied code subgraph.
    pub centrality: f64,
}

/// Rank the most central symbols in a resolved caller/callee subgraph.
///
/// The graph kernel supplies sorted node identities and deterministic Brandes
/// traversal. Ranking then sorts by descending centrality and ascending symbol
/// identifier, so equal-score hubs do not change order between runs. `limit`
/// bounds the returned result and zero returns an empty vector without building
/// a graph. Duplicate edges are merged by [`AdjacencyGraph`].
pub fn rank_hubs(edges: &[CodeEdge], limit: usize) -> Vec<CodeHub> {
    if limit == 0 || edges.is_empty() {
        return Vec::new();
    }

    let graph = AdjacencyGraph::from_edges(
        edges
            .iter()
            .map(|edge| (edge.caller.clone(), edge.callee.clone(), 1.0)),
    );
    let mut hubs: Vec<CodeHub> = betweenness_centrality(&graph, true)
        .into_iter()
        .map(|(symbol, centrality)| CodeHub { symbol, centrality })
        .collect();
    hubs.sort_by(|left, right| {
        right
            .centrality
            .total_cmp(&left.centrality)
            .then_with(|| left.symbol.cmp(&right.symbol))
    });
    hubs.truncate(limit);
    hubs
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R654.1
    #[test]
    fn types_round_trip_and_reject_unknown_stage() {
        let request = CodeContextRequest {
            anchor: "repo/b.rs::parse".to_string(),
            question: CodeQuestion::Impact,
            depth: Some(3),
            limit: None,
        };
        let text = serde_json::to_string(&request).unwrap();
        assert_eq!(
            text,
            r#"{"anchor":"repo/b.rs::parse","question":"impact","depth":3}"#
        );
        assert_eq!(
            serde_json::from_str::<CodeContextRequest>(&text).unwrap(),
            request
        );
        assert!(
            serde_json::from_str::<CodeContextRequest>(r#"{"anchor":"a","question":"why"}"#)
                .is_err()
        );

        let answer = CodeContextAnswer {
            status: CodeContextStatus::Ambiguous,
            anchors: vec![],
            usages: vec![CodeNode {
                symbol: "repo/a.rs::main".to_string(),
                hops: 1,
            }],
            impact: vec![],
            hubs: vec![RankedHub {
                symbol: "repo/b.rs::parse".to_string(),
                in_degree: 1,
                out_degree: 2,
                score: 0.25,
            }],
            candidates: vec!["x::parse".to_string(), "y::parse".to_string()],
        };
        let text = serde_json::to_string(&answer).unwrap();
        assert!(text.contains(r#""status":"ambiguous""#));
        assert_eq!(
            serde_json::from_str::<CodeContextAnswer>(&text).unwrap(),
            answer
        );

        let stages = [
            OpsCausalStage::Change,
            OpsCausalStage::Deploy,
            OpsCausalStage::Config,
            OpsCausalStage::Dependency,
            OpsCausalStage::Resource,
            OpsCausalStage::Symptom,
        ];
        let names: Vec<String> = stages
            .iter()
            .map(|stage| serde_json::to_string(stage).unwrap())
            .collect();
        assert_eq!(
            names,
            [
                r#""change""#,
                r#""deploy""#,
                r#""config""#,
                r#""dependency""#,
                r#""resource""#,
                r#""symptom""#
            ]
        );
        let diagnosis = SymptomDiagnosis {
            symptom: "latency".to_string(),
            chain: vec![CausalLink {
                stage: OpsCausalStage::Deploy,
                node_id: "deploy-7".to_string(),
                relationship: "CAUSED".to_string(),
                evidence_ids: vec!["ev-1".to_string()],
            }],
        };
        let text = serde_json::to_string(&diagnosis).unwrap();
        assert_eq!(
            serde_json::from_str::<SymptomDiagnosis>(&text).unwrap(),
            diagnosis
        );
        assert!(serde_json::from_str::<OpsCausalStage>(r#""weather""#).is_err());
        assert!(
            serde_json::from_str::<SymptomDiagnosis>(&text.replace("deploy\"", "weather\""))
                .is_err()
        );
    }

    #[test]
    fn rank_hubs_is_deterministic_and_tie_breaks_symbols() {
        let edges = vec![
            CodeEdge::new("repo/a.rs::main", "repo/b.rs::parse"),
            CodeEdge::new("repo/b.rs::parse", "repo/c.rs::normalize"),
            CodeEdge::new("repo/c.rs::normalize", "repo/d.rs::emit"),
        ];

        let first = rank_hubs(&edges, 2);
        let second = rank_hubs(&edges, 2);

        assert_eq!(first, second);
        assert_eq!(
            first
                .iter()
                .map(|hub| hub.symbol.as_str())
                .collect::<Vec<_>>(),
            vec!["repo/b.rs::parse", "repo/c.rs::normalize"]
        );
        assert_eq!(first[0].centrality, first[1].centrality);
        assert!(first[0].centrality > 0.0);
    }

    #[test]
    fn rank_hubs_refuses_empty_or_zero_budget_without_partial_rows() {
        let edge = CodeEdge::new("repo/a.rs::main", "repo/b.rs::parse");

        assert!(rank_hubs(&[], 4).is_empty());
        assert!(rank_hubs(&[edge], 0).is_empty());
    }
}
