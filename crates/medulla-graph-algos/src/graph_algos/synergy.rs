//! Synergy bridges between communities (MEDULLA-FEDERATED-QUERY-R607.8).
//!
//! Communities come from the shared Leiden driver ([`super::leiden::leiden`]);
//! community ids are the rank of each community's smallest member. A bridge
//! node's neighbours touch at least 2 communities. Its score is the number of
//! distinct communities touched times the share of its neighbour types that no
//! single community holds (a type is held by one community when every
//! neighbour of that type lies in it). Output is the top `k` bridges by score
//! descending then node id.

use super::leiden::{leiden, LeidenConfig};
use super::AdjacencyGraph;
use std::collections::{BTreeMap, BTreeSet};

/// One synergy bridge; `communities` are community ids in ascending order.
#[derive(Debug, Clone, PartialEq)]
pub struct Bridge {
    pub node: String,
    pub communities: Vec<usize>,
    pub score: f64,
}

/// Top `k` synergy bridges. `node_types` maps node id to its type; a node
/// without an entry has the empty type.
pub fn synergy_bridges(
    graph: &AdjacencyGraph<String>,
    node_types: &BTreeMap<String, String>,
    k: usize,
    seed: u64,
) -> (Vec<Bridge>, Vec<Vec<String>>) {
    let config = LeidenConfig {
        seed: Some(seed),
        ..LeidenConfig::default()
    };
    let communities = leiden(graph, &config).communities;
    let mut community_of: BTreeMap<&str, usize> = BTreeMap::new();
    for (c, members) in communities.iter().enumerate() {
        for m in members {
            community_of.insert(m.as_str(), c);
        }
    }
    let empty = String::new();
    let mut bridges = Vec::new();
    for v in 0..graph.node_count() {
        let neighbours = graph.undirected_neighbors(v);
        let mut touched: BTreeSet<usize> = BTreeSet::new();
        let mut type_communities: BTreeMap<&String, BTreeSet<usize>> = BTreeMap::new();
        for u in neighbours {
            let id = graph.node_at(u);
            let Some(&c) = community_of.get(id.as_str()) else {
                continue;
            };
            touched.insert(c);
            let t = node_types.get(id).unwrap_or(&empty);
            type_communities.entry(t).or_default().insert(c);
        }
        if touched.len() < 2 || type_communities.is_empty() {
            continue;
        }
        let unheld = type_communities.values().filter(|cs| cs.len() > 1).count();
        let share = unheld as f64 / type_communities.len() as f64;
        let score = touched.len() as f64 * share;
        bridges.push(Bridge {
            node: graph.node_at(v).clone(),
            communities: touched.into_iter().collect(),
            score,
        });
    }
    bridges.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.node.cmp(&b.node))
    });
    bridges.truncate(k);
    (bridges, communities)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> String {
        x.to_string()
    }

    // spec: MEDULLA-FEDERATED-QUERY-R607.8
    #[test]
    fn two_community_bridge_ranks_first() {
        let mut edges = Vec::new();
        for block in ["a", "b"] {
            for i in 0..5 {
                for j in i + 1..5 {
                    edges.push((format!("{block}{i}"), format!("{block}{j}")));
                }
            }
        }
        // `hub` joins both cliques through neighbours of the same type.
        for n in ["a0", "a1", "b0", "b1"] {
            edges.push((s("hub"), s(n)));
        }
        let graph = AdjacencyGraph::from_unweighted_edges(edges);
        let mut types = BTreeMap::new();
        for block in ["a", "b"] {
            for i in 0..5 {
                types.insert(format!("{block}{i}"), s("Service"));
            }
        }
        types.insert(s("hub"), s("Team"));
        let (bridges, communities) = synergy_bridges(&graph, &types, 3, 0);
        assert!(communities.len() >= 2);
        let first = &bridges[0];
        assert_eq!(first.node, "hub");
        assert_eq!(first.communities.len(), 2);
        assert!((first.score - 2.0).abs() < 1e-12);
        assert!(bridges.len() <= 3);
        assert!(bridges.windows(2).all(|w| w[0].score >= w[1].score));
        assert_eq!(synergy_bridges(&graph, &types, 3, 0).0, bridges);
    }
}
