//! Results of the always-built graph algorithms and the VIZ-1 cluster hierarchy.

use serde::{Deserialize, Serialize};

/// Materialized result of a bounded VF2 subgraph-isomorphism search.
///
/// `truncated` is true when either the requested match limit or search-step
/// budget stopped the search before the candidate space was exhausted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Vf2MatchResult {
    pub matches: Vec<std::collections::HashMap<String, String>>,
    pub truncated: bool,
}

/// One capability term found in a query by the ontology lexical gate
/// (CONCEPT:MEDULLA-ORCH.routing.lexical-capability-escalation). `term` is the matched alias/name, `node_type` its capability
/// class (Tool/Skill/MCPServer/…), `label` the owning node's display name,
/// `mcp_server` the owning fleet server (so a caller can bind that server's
/// toolset directly), and `score` the matched term's character length.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct OntologyMatch {
    pub term: String,
    pub node_type: String,
    pub label: String,
    pub mcp_server: String,
    pub score: f64,
}

/// `ClusterHierarchyRefresh`: what was computed and whether it was cached durably.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClusterHierarchySummary {
    pub graph: String,
    /// Number of computed levels (level 1 is the finest).
    pub levels: usize,
    pub base_node_count: usize,
    pub base_edge_count: usize,
    /// Cluster count of the coarsest level; 0 when no level was computed.
    pub top_level_clusters: usize,
    /// `false` when the backend has no persistence to cache the hierarchy in.
    pub cached: bool,
}

/// One cluster as served to a visualization client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClusterSummary {
    /// Stable, globally-addressable id: `"L{level}-{local_index}"`.
    pub id: String,
    pub label: String,
    pub node_count: usize,
    /// Sum of internal-edge weight (directed).
    pub edge_count: f64,
    /// Up to 5 most common node types among the members, descending.
    pub top_node_types: Vec<(String, usize)>,
}

/// An edge between two clusters of one served level, by array-local index into
/// that response's `clusters`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct InterClusterEdge {
    pub src_idx: u32,
    pub dst_idx: u32,
    pub weight: f64,
}

/// `ClusterHierarchyClusters`: one level of the cached hierarchy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClusterLevelView {
    pub level: usize,
    pub clusters: Vec<ClusterSummary>,
    pub inter_cluster_edges: Vec<InterClusterEdge>,
}

/// A member node of a level-1 cluster, read live off the graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClusterMemberNode {
    pub id: String,
    /// The node's stored properties, decoded; `null` when undecodable.
    pub properties: serde_json::Value,
}

/// An edge between two member nodes of a level-1 cluster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClusterMemberEdge {
    pub src_id: String,
    pub dst_id: String,
    /// The edge's `relationship` property, or `"_"` when it has none.
    #[serde(rename = "type")]
    pub relationship: String,
}

/// `ClusterHierarchyExpand`: a level-1 cluster expands to its member nodes and edges
/// (`child_clusters` empty); a coarser cluster expands to its children one level
/// finer (`nodes` and `edges` empty).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ClusterExpansion {
    pub nodes: Vec<ClusterMemberNode>,
    pub edges: Vec<ClusterMemberEdge>,
    pub child_clusters: Vec<ClusterSummary>,
}

/// What a `node_ids` scope selected, echoed on a scoped algorithm result
/// (MEDULLA-FEDERATED-QUERY-R743.1). A node the caller cannot see counts exactly as an
/// unknown id does: neither reaches `in_scope`, so the pair never tells them apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AlgorithmScopeView {
    /// Distinct ids the request named.
    pub requested: u64,
    /// Of those, the nodes the caller can see; the algorithm ran on their induced subgraph.
    pub in_scope: u64,
}

/// `CommunityDetection`: the partition, each community's member ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CommunityDetectionView {
    pub communities: Vec<Vec<String>>,
    /// Present exactly when the request carried `node_ids`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<AlgorithmScopeView>,
}

/// One node's PageRank score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct NodeScoreView {
    pub node_id: String,
    pub score: f64,
}

/// `PageRank`: one score per node of the ranked (visible or scoped) graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PageRankView {
    pub scores: Vec<NodeScoreView>,
    /// Present exactly when the request carried `node_ids`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<AlgorithmScopeView>,
}

/// `GetShortestPath`: the node ids from source to target, or `None` when no path exists
/// (also when an endpoint is unknown, hidden or outside the scope).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ShortestPathView {
    pub path: Option<Vec<String>>,
    /// Present exactly when the request carried `node_ids`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<AlgorithmScopeView>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-FEDERATED-QUERY-R743.1
    #[test]
    fn a_scope_is_serialized_only_when_the_request_carried_one() {
        let unscoped = PageRankView {
            scores: vec![NodeScoreView {
                node_id: "a".into(),
                score: 0.5,
            }],
            scope: None,
        };
        let wire = serde_json::to_value(&unscoped).unwrap();
        assert!(wire.get("scope").is_none(), "{wire}");
        assert_eq!(
            serde_json::from_value::<PageRankView>(wire).unwrap(),
            unscoped
        );

        let scoped = ShortestPathView {
            path: None,
            scope: Some(AlgorithmScopeView {
                requested: 3,
                in_scope: 2,
            }),
        };
        let wire = serde_json::to_value(&scoped).unwrap();
        assert_eq!(
            wire["scope"],
            serde_json::json!({"requested": 3, "in_scope": 2})
        );
        assert_eq!(
            serde_json::from_value::<ShortestPathView>(wire).unwrap(),
            scoped
        );

        let partition = CommunityDetectionView {
            communities: vec![vec!["a".into(), "b".into()]],
            scope: None,
        };
        let wire = serde_json::to_value(&partition).unwrap();
        assert_eq!(
            serde_json::from_value::<CommunityDetectionView>(wire).unwrap(),
            partition
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R744.1
    #[test]
    fn the_published_cluster_views_round_trip_with_their_wire_field_names() {
        let level = ClusterLevelView {
            level: 2,
            clusters: vec![ClusterSummary {
                id: "L2-0".into(),
                label: "docs".into(),
                node_count: 4,
                edge_count: 3.0,
                top_node_types: vec![("Doc".into(), 4)],
            }],
            inter_cluster_edges: vec![InterClusterEdge {
                src_idx: 0,
                dst_idx: 0,
                weight: 1.0,
            }],
        };
        let wire = serde_json::to_value(&level).unwrap();
        assert_eq!(wire["clusters"][0]["id"], "L2-0");
        assert_eq!(wire["inter_cluster_edges"][0]["src_idx"], 0);
        assert_eq!(
            serde_json::from_value::<ClusterLevelView>(wire).unwrap(),
            level
        );

        let expansion = ClusterExpansion {
            nodes: vec![ClusterMemberNode {
                id: "n1".into(),
                properties: serde_json::json!({"type": "Doc"}),
            }],
            edges: vec![ClusterMemberEdge {
                src_id: "n1".into(),
                dst_id: "n2".into(),
                relationship: "LINKS".into(),
            }],
            child_clusters: Vec::new(),
        };
        let wire = serde_json::to_value(&expansion).unwrap();
        assert_eq!(
            wire["edges"][0]["type"], "LINKS",
            "relationship serializes as `type`"
        );
        assert!(wire.get("child_clusters").is_some());
        assert_eq!(
            serde_json::from_value::<ClusterExpansion>(wire).unwrap(),
            expansion
        );
    }
}
