//! Neighbour expansion over a projected core (MEDULLA-FEDERATED-QUERY-R090.8).
//!
//! Shared by `Discover` plans and the corpus entity mode, so one walk exists.
//!
//! `expand_hops` walks the graph breadth-first from the top-k fused hits over
//! the caller's row-projected core, so a neighbour the caller may not read is
//! never reached. Each hop keeps at most `fanout` neighbours per frontier node,
//! ordered by edge weight descending then node id. A neighbour scores
//! `seed score x decay ^ hop`. Hits dedupe by id, keep the best path, and the
//! list truncates to `k`. The walk writes nothing.

use std::collections::HashMap;

use medulla_ingestion_types::ingestion_wire::HopFilter;

use super::GraphCore;

/// One hit after expansion: its score and the seed-to-hit node ids.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpandedHit {
    pub id: String,
    pub score: f32,
    /// Seed id first, this hit last; a single id for a seed hit.
    pub path: Vec<String>,
}

/// Heaviest `weight` over the edge properties in either direction; `1.0` when
/// no property blob carries a numeric `weight`.
pub fn edge_weight(core: &GraphCore, a: &str, b: &str) -> f64 {
    let mut best: Option<f64> = None;
    for (src, tgt) in [(a, b), (b, a)] {
        for blob in core.get_edge_properties(src, tgt) {
            let w = medulla_types::msgpack::decode_property_object(&blob)
                .unwrap_or_default()
                .get("weight")
                .and_then(serde_json::Value::as_f64);
            if let Some(w) = w.filter(|w| w.is_finite()) {
                best = Some(best.map_or(w, |b| b.max(w)));
            }
        }
    }
    best.unwrap_or(1.0)
}

/// [`edge_weight`] over a projected view: the heaviest finite `weight` over the view's edge
/// properties in either direction, `1.0` when none carries one
/// (MEDULLA-FEDERATED-QUERY-R736.2).
pub fn edge_weight_in_view(view: &super::GraphView, a: &str, b: &str) -> f64 {
    let mut best: Option<f64> = None;
    for (src, tgt) in [(a, b), (b, a)] {
        let Some(blobs) = view.edge_properties.get(&(src.to_string(), tgt.to_string())) else {
            continue;
        };
        for blob in blobs {
            let w = medulla_types::msgpack::decode_property_object(blob)
                .unwrap_or_default()
                .get("weight")
                .and_then(serde_json::Value::as_f64);
            if let Some(w) = w.filter(|w| w.is_finite()) {
                best = Some(best.map_or(w, |b| b.max(w)));
            }
        }
    }
    best.unwrap_or(1.0)
}

/// The restriction of one walk (MEDULLA-FEDERATED-QUERY-R741.1): a [`HopFilter`] and the
/// name of the graph the walked core serves. An empty filter restricts nothing.
#[derive(Debug, Clone, Copy)]
pub struct HopScope<'a> {
    pub filter: &'a HopFilter,
    /// The graph the core belongs to; a neighbour with no `graph` property of its own is a
    /// member of it.
    pub graph_name: &'a str,
}

fn property_str<'o>(
    object: &'o serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<&'o str> {
    keys.iter()
        .filter_map(|key| object.get(*key).and_then(serde_json::Value::as_str))
        .find(|value| !value.is_empty())
}

static NO_FILTER: HopFilter = HopFilter {
    node_labels: Vec::new(),
    edge_labels: Vec::new(),
    graph: None,
    language: None,
};

impl HopScope<'static> {
    /// A walk with no restriction on any axis, for a caller with no filter to apply.
    pub fn unrestricted() -> Self {
        Self {
            filter: &NO_FILTER,
            graph_name: "",
        }
    }
}

impl HopScope<'_> {
    /// Whether the walk may step from `from` to `neighbour`: the neighbour matches every
    /// axis the filter names. A literal neighbour carrying another language tag is dropped;
    /// one with no tag stays.
    fn admits(&self, core: &GraphCore, from: &str, neighbour: &str) -> bool {
        let filter = self.filter;
        if filter.is_empty() {
            return true;
        }
        if !filter.edge_labels.is_empty() {
            let labelled = [(from, neighbour), (neighbour, from)]
                .iter()
                .flat_map(|(src, tgt)| core.get_edge_properties(src, tgt))
                .any(|blob| {
                    let object = medulla_types::msgpack::decode_property_object(&blob)
                        .unwrap_or_default();
                    property_str(&object, &["relationship", "type", "edge_type"])
                        .is_some_and(|label| filter.edge_labels.iter().any(|l| l == label))
                });
            if !labelled {
                return false;
            }
        }
        let needs_node = !filter.node_labels.is_empty()
            || filter.graph.is_some()
            || filter.language.is_some();
        if !needs_node {
            return true;
        }
        let object = core
            .get_node_properties(neighbour)
            .map(|blob| medulla_types::msgpack::decode_property_object(&blob).unwrap_or_default())
            .unwrap_or_default();
        if !filter.node_labels.is_empty()
            && !property_str(&object, &["type", "node_type"])
                .is_some_and(|label| filter.node_labels.iter().any(|l| l == label))
        {
            return false;
        }
        if let Some(graph) = &filter.graph {
            if property_str(&object, &["graph"]).unwrap_or(self.graph_name) != graph {
                return false;
            }
        }
        if let Some(language) = &filter.language {
            if property_str(&object, &["lang", "language"])
                .is_some_and(|tag| !tag.eq_ignore_ascii_case(language))
            {
                return false;
            }
        }
        true
    }
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
/// returned list is ordered by score descending then id. `scope` drops every neighbour
/// the hop filter does not admit before the fan-out cut, so a filtered walk still fills
/// its fan-out from matching neighbours (MEDULLA-FEDERATED-QUERY-R741.1).
pub fn expand_hops(
    core: &GraphCore,
    seeds: &[(String, f32)],
    hops: u32,
    fanout: usize,
    decay: f64,
    k: usize,
    scope: &HopScope<'_>,
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
                .filter(|n| scope.admits(core, &from, n))
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
        let one = expand_hops(&core, &seeds, 1, 2, 0.5, 10, &HopScope::unrestricted());
        let ids: Vec<&str> = one.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, vec!["s", "a", "b"]);
        assert!((one[1].score - 0.5).abs() < 1e-6, "seed x decay^1");
        assert_eq!(one[1].path, vec!["s", "a"]);

        // Two hops reach z with seed x decay^2 and the full path.
        let two = expand_hops(&core, &seeds, 2, 2, 0.5, 10, &HopScope::unrestricted());
        let z = two.iter().find(|h| h.id == "z").expect("z reached");
        assert!((z.score - 0.25).abs() < 1e-6);
        assert_eq!(z.path, vec!["s", "a", "z"]);

        // hops == 0 returns the seeds only; k truncates.
        assert_eq!(expand_hops(&core, &seeds, 0, 2, 0.5, 10, &HopScope::unrestricted()).len(), 1);
        assert_eq!(expand_hops(&core, &seeds, 2, 32, 0.5, 2, &HopScope::unrestricted()).len(), 2);

        // A node reachable twice appears once with the better score.
        let dup = expand_hops(
            &core,
            &[("s".into(), 1.0), ("a".into(), 0.9)],
            1,
            32,
            0.5,
            20,
            &HopScope::unrestricted(),
        );
        assert_eq!(dup.iter().filter(|h| h.id == "a").count(), 1);
        assert!((dup.iter().find(|h| h.id == "a").unwrap().score - 0.9).abs() < 1e-6);
    }
}
