//! MEDULLA-FEDERATED-QUERY-R736.2: expansion walks schema and similarity relations from the
//! seeds, stops at the hop bound and refuses past the candidate cap.

use std::collections::HashMap;

use medulla_plan::evidence_forest::{
    expand_candidates, forest_limits, ForestGraph, ForestRelation, RelationKind,
};
use medulla_result_types::evidence_forest::ForestErrorCode;

#[derive(Default)]
struct Graph(HashMap<String, Vec<ForestRelation>>);

impl Graph {
    fn link(&mut self, from: &str, to: &str, kind: RelationKind, strength: f32, cost: f32) {
        self.0.entry(from.to_string()).or_default().push(ForestRelation {
            target: to.to_string(),
            kind,
            strength,
            verify_cost: cost,
            tokens: 10,
        });
    }
}

impl ForestGraph for Graph {
    fn relations(&self, memory_id: &str) -> Vec<ForestRelation> {
        self.0.get(memory_id).cloned().unwrap_or_default()
    }
}

fn seeds() -> Vec<(String, f32)> {
    vec![("s".to_string(), 1.0)]
}

// spec: MEDULLA-FEDERATED-QUERY-R736.2
#[test]
fn expansion_respects_hops_and_cap() {
    let mut graph = Graph::default();
    graph.link("s", "a", RelationKind::Schema, 0.5, 0.2);
    graph.link("s", "b", RelationKind::Similarity, 0.8, 0.4);
    graph.link("a", "c", RelationKind::Schema, 0.5, 0.1);
    graph.link("c", "d", RelationKind::Schema, 0.5, 0.1);
    // A second path to `c` that is weaker keeps the stronger support; a path back to the
    // seed adds no candidate.
    graph.link("b", "c", RelationKind::Similarity, 0.1, 0.3);
    graph.link("a", "s", RelationKind::Schema, 1.0, 0.1);

    let (hops, cap) = forest_limits(None, None);
    assert_eq!((hops, cap), (2, 2_000));
    let two = expand_candidates(&seeds(), &graph, hops, cap).unwrap();
    let ids: Vec<&str> = two.iter().map(|c| c.memory_id.as_str()).collect();
    assert_eq!(ids, ["b", "a", "c"], "support order; `d` is three hops away");
    let c = two.iter().find(|c| c.memory_id == "c").unwrap();
    assert_eq!(c.support, 0.25);
    assert_eq!(c.verify_cost, 0.1, "the cost of the relation to its parent, not the support");

    let one = expand_candidates(&seeds(), &graph, 1, cap).unwrap();
    assert_eq!(one.len(), 2);
    let three = expand_candidates(&seeds(), &graph, 3, cap).unwrap();
    assert!(three.iter().any(|c| c.memory_id == "d"));

    // Past the cap the walk refuses, naming the code.
    let refused = expand_candidates(&seeds(), &graph, 2, 2).unwrap_err();
    assert_eq!(refused.code, ForestErrorCode::CandidateCap);
    assert!(refused.to_string().starts_with("FOREST_CANDIDATE_CAP"));
    assert!(expand_candidates(&seeds(), &graph, 2, 3).is_ok());

    // No seed is its own refusal.
    let none = expand_candidates(&[], &graph, 2, cap).unwrap_err();
    assert_eq!(none.code, ForestErrorCode::NoSeeds);
}
