//! Inductive hyperedge scoring by positional encodings
//! (MEDULLA-FEDERATED-QUERY-R607.9).
//!
//! The anchors are the [`ANCHORS`] highest-degree nodes (undirected degree,
//! ties broken by ascending node id). A node's encoding holds, per anchor,
//! `1 / (1 + hop distance)` (0 when the anchor is unreachable), so nearby nodes
//! share large coordinates. A candidate set scores as the mean pairwise cosine
//! similarity of its members' encodings. Encodings are computed from the graph
//! alone — nothing is fitted — so a node added after any earlier run is scored
//! the same way (inductive).

#![allow(clippy::needless_range_loop)] // dense numeric kernels index several arrays at once

use super::AdjacencyGraph;
use std::collections::VecDeque;

/// Number of positional anchors.
pub const ANCHORS: usize = 4;

/// Hyperedge scoring refusals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HyperedgeError {
    /// More candidate sets than `max_candidates`.
    TooManyCandidates,
    /// A candidate names a node the graph does not hold.
    UnknownNode(String),
}

fn anchors(graph: &AdjacencyGraph<String>) -> Vec<usize> {
    let mut order: Vec<(usize, usize)> = (0..graph.node_count())
        .map(|i| (graph.undirected_neighbors(i).len(), i))
        .collect();
    order.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    order.into_iter().take(ANCHORS).map(|(_, i)| i).collect()
}

fn hop_distances(graph: &AdjacencyGraph<String>, source: usize) -> Vec<Option<u32>> {
    let mut dist = vec![None; graph.node_count()];
    dist[source] = Some(0);
    let mut queue = VecDeque::from([source]);
    while let Some(u) = queue.pop_front() {
        let d = dist[u].unwrap_or(0);
        for v in graph.undirected_neighbors(u) {
            if dist[v].is_none() {
                dist[v] = Some(d + 1);
                queue.push_back(v);
            }
        }
    }
    dist
}

/// Positional encodings of every node, `[node][anchor]`.
pub fn positional_encodings(graph: &AdjacencyGraph<String>) -> Vec<Vec<f64>> {
    let per_anchor: Vec<Vec<Option<u32>>> = anchors(graph)
        .into_iter()
        .map(|a| hop_distances(graph, a))
        .collect();
    (0..graph.node_count())
        .map(|i| {
            per_anchor
                .iter()
                .map(|d| d[i].map_or(0.0, |h| 1.0 / (1.0 + h as f64)))
                .collect()
        })
        .collect()
}

fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Score each candidate set (mean pairwise cosine; a set of fewer than 2
/// members scores 1). Refuses more than `max_candidates` sets.
pub fn hyperedge_scores(
    graph: &AdjacencyGraph<String>,
    candidates: &[Vec<String>],
    max_candidates: usize,
) -> Result<Vec<f64>, HyperedgeError> {
    if candidates.len() > max_candidates {
        return Err(HyperedgeError::TooManyCandidates);
    }
    let encodings = positional_encodings(graph);
    candidates
        .iter()
        .map(|set| {
            let idx = set
                .iter()
                .map(|id| {
                    graph
                        .index_of(id)
                        .ok_or_else(|| HyperedgeError::UnknownNode(id.clone()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut total = 0.0;
            let mut pairs = 0usize;
            for i in 0..idx.len() {
                for j in i + 1..idx.len() {
                    total += cosine(&encodings[idx[i]], &encodings[idx[j]]);
                    pairs += 1;
                }
            }
            Ok(if pairs == 0 {
                1.0
            } else {
                total / pairs as f64
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> String {
        x.to_string()
    }

    /// Verifies MEDULLA-FEDERATED-QUERY-R607.9: Behavior: hyperedge scoring
    // spec: MEDULLA-FEDERATED-QUERY-R607.9
    #[test]
    fn connected_set_outscores_scattered_set() {
        let mut edges = Vec::new();
        for hub in ["a", "b"] {
            for i in 1..=4 {
                edges.push((format!("{hub}0"), format!("{hub}{i}")));
            }
        }
        for (x, y) in [
            ("a0", "p1"),
            ("p1", "p2"),
            ("p2", "p3"),
            ("p3", "p4"),
            ("p4", "b0"),
        ] {
            edges.push((s(x), s(y)));
        }
        let graph = AdjacencyGraph::from_unweighted_edges(edges.clone());
        let connected = vec![s("a1"), s("a2"), s("a3")];
        let scattered = vec![s("a1"), s("b1"), s("p3")];
        let scores = hyperedge_scores(&graph, &[connected.clone(), scattered.clone()], 10).unwrap();
        assert!(scores[0] > scores[1], "{scores:?}");
        assert!((scores[0] - 1.0).abs() < 1e-12);

        // Inductive: a node added later is scored with no fitting step.
        edges.push((s("a0"), s("a5")));
        let grown = AdjacencyGraph::from_unweighted_edges(edges);
        let with_new = hyperedge_scores(&grown, &[vec![s("a1"), s("a5")]], 10).unwrap();
        assert!((with_new[0] - 1.0).abs() < 1e-12);

        assert_eq!(
            hyperedge_scores(&graph, &[connected.clone(), scattered], 1),
            Err(HyperedgeError::TooManyCandidates)
        );
        assert_eq!(
            hyperedge_scores(&graph, &[vec![s("zz")]], 10),
            Err(HyperedgeError::UnknownNode(s("zz")))
        );
    }
}
