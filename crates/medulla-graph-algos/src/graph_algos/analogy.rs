//! Cross-domain analogy matching (MEDULLA-FEDERATED-QUERY-R607.7).
//!
//! Every node gets a Weisfeiler-Lehman signature of [`WL_ROUNDS`] rounds over the
//! relationship types (with direction) of its neighbourhood. The round-0 colour
//! is the same for every node: a node's own `label` names its domain, so it
//! decides which pairs may match (only nodes of DIFFERENT labels) and never
//! enters the colour, or no cross-domain pair could ever agree. Two nodes match
//! when the Jaccard similarity of their colour multisets (rounds 0..=3) is at
//! least `min_score`. Colours are canonical ranks of the sorted round strings,
//! so results are deterministic.

#![allow(clippy::needless_range_loop)] // dense numeric kernels index several arrays at once

use std::collections::BTreeMap;

/// Weisfeiler-Lehman rounds.
pub const WL_ROUNDS: usize = 3;

/// One analogy match: `a < b`, `score` in `[0, 1]`.
#[derive(Debug, Clone, PartialEq)]
pub struct AnalogyPair {
    pub a: String,
    pub b: String,
    pub score: f64,
}

/// A labelled multigraph: nodes `(id, label)`, edges `(src, dst, relationship)`.
#[derive(Debug, Clone, Default)]
pub struct LabeledGraph {
    pub nodes: Vec<(String, String)>,
    pub edges: Vec<(String, String, String)>,
}

/// WL colour multisets per node, in node-id order.
fn signatures(graph: &LabeledGraph) -> (Vec<String>, Vec<String>, Vec<BTreeMap<u64, u32>>) {
    let mut nodes = graph.nodes.clone();
    nodes.sort();
    nodes.dedup_by(|a, b| a.0 == b.0);
    let index: BTreeMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id.as_str(), i))
        .collect();
    let n = nodes.len();
    let mut neighbours: Vec<Vec<(String, usize)>> = vec![Vec::new(); n];
    for (src, dst, rel) in &graph.edges {
        if let (Some(&s), Some(&d)) = (index.get(src.as_str()), index.get(dst.as_str())) {
            neighbours[s].push((format!(">{rel}"), d));
            neighbours[d].push((format!("<{rel}"), s));
        }
    }
    let mut colour = vec![0u64; n];
    let mut multisets: Vec<BTreeMap<u64, u32>> = vec![BTreeMap::new(); n];
    for (round, slot) in (0..=WL_ROUNDS).zip(0u64..) {
        for i in 0..n {
            *multisets[i].entry(slot << 40 | colour[i]).or_default() += 1;
        }
        if round == WL_ROUNDS {
            break;
        }
        let keys: Vec<String> = (0..n)
            .map(|i| {
                let mut parts: Vec<String> = neighbours[i]
                    .iter()
                    .map(|(rel, j)| format!("{rel}:{}", colour[*j]))
                    .collect();
                parts.sort();
                format!("{}|{}", colour[i], parts.join(","))
            })
            .collect();
        let mut sorted: Vec<&String> = keys.iter().collect();
        sorted.sort();
        sorted.dedup();
        let rank: BTreeMap<&String, u64> = sorted.into_iter().zip(0u64..).collect();
        colour = keys.iter().map(|k| rank[k]).collect();
    }
    let (ids, labels) = nodes.into_iter().unzip();
    (ids, labels, multisets)
}

fn jaccard(a: &BTreeMap<u64, u32>, b: &BTreeMap<u64, u32>) -> f64 {
    let mut inter = 0u64;
    let mut union = 0u64;
    let keys: std::collections::BTreeSet<&u64> = a.keys().chain(b.keys()).collect();
    for k in keys {
        let x = a.get(k).copied().unwrap_or(0) as u64;
        let y = b.get(k).copied().unwrap_or(0) as u64;
        inter += x.min(y);
        union += x.max(y);
    }
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

/// At most `k` cross-domain pairs with score `>= min_score`, ordered by score
/// descending then `(a, b)`.
pub fn analogy_matches(graph: &LabeledGraph, min_score: f64, k: usize) -> Vec<AnalogyPair> {
    let (ids, labels, sigs) = signatures(graph);
    let mut out = Vec::new();
    for i in 0..ids.len() {
        for j in i + 1..ids.len() {
            if labels[i] == labels[j] {
                continue;
            }
            let score = jaccard(&sigs[i], &sigs[j]);
            if score >= min_score {
                out.push(AnalogyPair {
                    a: ids[i].clone(),
                    b: ids[j].clone(),
                    score,
                });
            }
        }
    }
    out.sort_by(|x, y| {
        y.score
            .total_cmp(&x.score)
            .then_with(|| x.a.cmp(&y.a))
            .then_with(|| x.b.cmp(&y.b))
    });
    out.truncate(k);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> String {
        x.to_string()
    }

    fn fixture() -> LabeledGraph {
        LabeledGraph {
            nodes: vec![
                (s("g0"), s("Gene")),
                (s("g1"), s("Gene")),
                (s("g2"), s("Gene")),
                (s("s0"), s("Software")),
                (s("s1"), s("Software")),
                (s("s2"), s("Software")),
                (s("s3"), s("Software")),
            ],
            edges: vec![
                (s("g0"), s("g1"), s("regulates")),
                (s("g0"), s("g2"), s("regulates")),
                (s("s0"), s("s1"), s("regulates")),
                (s("s0"), s("s2"), s("regulates")),
                (s("s0"), s("s3"), s("regulates")),
            ],
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R607.7
    #[test]
    fn isomorphic_subgraphs_match_and_boundary_score() {
        let mut iso = fixture();
        iso.nodes.retain(|(id, _)| id != "s3");
        iso.edges.retain(|(_, d, _)| d != "s3");
        let all = analogy_matches(&iso, 1.0, 16);
        assert!(all
            .iter()
            .any(|m| m.a == "g0" && m.b == "s0" && m.score == 1.0));
        assert!(all
            .iter()
            .all(|m| m.a.starts_with('g') && m.b.starts_with('s')));
        assert!(!all.iter().any(|m| m.a == "g0" && m.b == "s1"));

        // Boundary: s0 has a third child, so (g0, s0) scores below 1; the
        // threshold is inclusive at exactly that score.
        let g = fixture();
        let score = analogy_matches(&g, 0.0, 64)
            .into_iter()
            .find(|m| m.a == "g0" && m.b == "s0")
            .unwrap()
            .score;
        assert!(score > 0.0 && score < 1.0);
        assert!(analogy_matches(&g, score, 64)
            .iter()
            .any(|m| m.a == "g0" && m.b == "s0"));
        assert!(!analogy_matches(&g, score + 1e-9, 64)
            .iter()
            .any(|m| m.a == "g0" && m.b == "s0"));
        assert_eq!(analogy_matches(&g, 0.0, 2).len(), 2);
        let ordered = analogy_matches(&g, 0.0, 64);
        assert!(ordered.windows(2).all(|w| w[0].score >= w[1].score));
    }
}
