//! Neighbour expansion of `Discover` plans (MEDULLA-FEDERATED-QUERY-R090.8).
//!
//! `expand_hops` walks the graph breadth-first from the top-k fused hits over
//! the caller's row-projected core, so a neighbour the caller may not read is
//! never reached. Each hop keeps at most `fanout` neighbours per frontier node,
//! ordered by edge weight descending then node id. A neighbour scores
//! `seed score x decay ^ hop`. Hits dedupe by id, keep the best path, and the
//! list truncates to `k`. The walk writes nothing.

use std::collections::HashMap;

use medulla_graph_core::graph::GraphCore;

/// One hit after expansion: its score and the seed-to-hit node ids.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExpandedHit {
    pub(crate) id: String,
    pub(crate) score: f32,
    /// Seed id first, this hit last; a single id for a seed hit.
    pub(crate) path: Vec<String>,
}

/// Heaviest `weight` over the edge properties in either direction; `1.0` when
/// no property blob carries a numeric `weight`.
fn edge_weight(core: &GraphCore, a: &str, b: &str) -> f64 {
    let mut best: Option<f64> = None;
    for (src, tgt) in [(a, b), (b, a)] {
        for blob in core.get_edge_properties(src, tgt) {
            let w = super::gateway_graph::decode_json_object(&blob)
                .get("weight")
                .and_then(serde_json::Value::as_f64);
            if let Some(w) = w.filter(|w| w.is_finite()) {
                best = Some(best.map_or(w, |b| b.max(w)));
            }
        }
    }
    best.unwrap_or(1.0)
}

fn better(candidate: &ExpandedHit, held: &ExpandedHit) -> bool {
    candidate
        .score
        .partial_cmp(&held.score)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| held.path.len().cmp(&candidate.path.len()))
        .then_with(|| held.path.cmp(&candidate.path))
        .is_gt()
}

/// Breadth-first neighbour expansion of `seeds` (id, fused score) by `hops`.
///
/// With `hops == 0` the seeds come back ranked and truncated to `k`. The
/// returned list is ordered by score descending then id.
pub(crate) fn expand_hops(
    core: &GraphCore,
    seeds: &[(String, f32)],
    hops: u32,
    fanout: usize,
    decay: f64,
    k: usize,
) -> Vec<ExpandedHit> {
    let mut best: HashMap<String, ExpandedHit> = HashMap::new();
    let mut frontier: Vec<String> = Vec::new();
    for (id, score) in seeds {
        let hit = ExpandedHit {
            id: id.clone(),
            score: *score,
            path: vec![id.clone()],
        };
        if !best.contains_key(id) {
            frontier.push(id.clone());
        }
        if best.get(id).is_none_or(|held| better(&hit, held)) {
            best.insert(id.clone(), hit);
        }
    }
    let seed_scores: HashMap<String, f32> = best
        .iter()
        .map(|(id, hit)| (id.clone(), hit.score))
        .collect();

    for hop in 1..=hops {
        frontier.sort();
        let batch = core.get_neighbors_batch(frontier.clone());
        let mut found: HashMap<String, ExpandedHit> = HashMap::new();
        for (from, neighbours) in batch {
            let Some(parent) = best.get(&from).cloned() else {
                continue;
            };
            let root_score = seed_scores
                .get(parent.path.first().unwrap_or(&from))
                .copied()
                .unwrap_or(parent.score);
            let mut weighted: Vec<(f64, String)> = neighbours
                .into_iter()
                .map(|n| (edge_weight(core, &from, &n), n))
                .collect();
            weighted.sort_by(|a, b| {
                b.0.partial_cmp(&a.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.1.cmp(&b.1))
            });
            weighted.truncate(fanout);
            let score = (f64::from(root_score) * decay.powi(hop as i32)) as f32;
            for (_, n) in weighted {
                let mut path = parent.path.clone();
                path.push(n.clone());
                let hit = ExpandedHit {
                    id: n.clone(),
                    score,
                    path,
                };
                if best.get(&n).is_some_and(|held| !better(&hit, held)) {
                    continue;
                }
                if found.get(&n).is_none_or(|held| better(&hit, held)) {
                    found.insert(n, hit);
                }
            }
        }
        if found.is_empty() {
            break;
        }
        frontier = found
            .keys()
            .filter(|id| !best.contains_key(*id))
            .cloned()
            .collect();
        best.extend(found);
    }

    let mut out: Vec<ExpandedHit> = best.into_values().collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    out.truncate(k);
    out
}

/// Plan a `Discover` result: the fused top-k seeds plus their neighbours.
///
/// `fanout` and `decay` come from the registered knobs `discover.hop_fanout`
/// and `discover.hop_decay`.
pub(crate) fn plan_discover(
    core: &GraphCore,
    seeds: Vec<(String, f32)>,
    hops: u32,
    k: usize,
) -> Vec<ExpandedHit> {
    if hops == 0 {
        return seeds
            .into_iter()
            .take(k)
            .map(|(id, score)| ExpandedHit {
                path: vec![id.clone()],
                id,
                score,
            })
            .collect();
    }
    let knobs = medulla_server_api::knob_config::knobs();
    let fanout = usize::try_from(knobs.discover_hop_fanout).unwrap_or(usize::MAX);
    expand_hops(core, &seeds, hops, fanout, knobs.discover_hop_decay, k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).unwrap()
    }

    fn graph(edges: &[(&str, &str, f64)]) -> GraphCore {
        let core = GraphCore::new();
        for (a, b, _) in edges {
            for id in [a, b] {
                core.add_node((*id).into(), props(serde_json::json!({"type": "t"})));
            }
        }
        for (a, b, w) in edges {
            core.add_edge(
                (*a).into(),
                (*b).into(),
                props(serde_json::json!({"weight": w})),
            )
            .unwrap();
        }
        core
    }

    // spec: MEDULLA-FEDERATED-QUERY-R090.8
    #[test]
    fn hop_cap_and_order() {
        let core = graph(&[
            ("s", "c", 0.5),
            ("s", "b", 0.9),
            ("s", "a", 0.9),
            ("s", "d", 0.1),
            ("a", "z", 1.0),
        ]);
        let seeds = vec![("s".to_string(), 1.0_f32)];

        // Fan-out 2 keeps the two heaviest edges; the tie breaks on node id.
        let one = expand_hops(&core, &seeds, 1, 2, 0.5, 10);
        let ids: Vec<&str> = one.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, vec!["s", "a", "b"]);
        assert!((one[1].score - 0.5).abs() < 1e-6, "seed x decay^1");
        assert_eq!(one[1].path, vec!["s", "a"]);

        // Two hops reach z with seed x decay^2 and the full path.
        let two = expand_hops(&core, &seeds, 2, 2, 0.5, 10);
        let z = two.iter().find(|h| h.id == "z").expect("z reached");
        assert!((z.score - 0.25).abs() < 1e-6);
        assert_eq!(z.path, vec!["s", "a", "z"]);

        // hops == 0 returns the seeds only; k truncates.
        assert_eq!(expand_hops(&core, &seeds, 0, 2, 0.5, 10).len(), 1);
        assert_eq!(expand_hops(&core, &seeds, 2, 32, 0.5, 2).len(), 2);

        // A node reachable twice appears once with the better score.
        let dup = expand_hops(
            &core,
            &[("s".into(), 1.0), ("a".into(), 0.9)],
            1,
            32,
            0.5,
            20,
        );
        assert_eq!(dup.iter().filter(|h| h.id == "a").count(), 1);
        assert!((dup.iter().find(|h| h.id == "a").unwrap().score - 0.9).abs() < 1e-6);
    }
}
