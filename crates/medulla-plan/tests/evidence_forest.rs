//! MEDULLA-FEDERATED-QUERY-R736.2: expansion walks schema and similarity relations from the
//! seeds, stops at the hop bound and refuses past the candidate cap.

use std::collections::HashMap;

use medulla_plan::evidence_forest::{
    expand_candidates, forest_limits, select_forest, ForestGraph, ForestPenalty, ForestRelation,
    ForestRelations, RelationKind,
};
use medulla_result_types::evidence_forest::{EvidenceCandidate, ForestErrorCode};

#[derive(Default)]
struct Graph(HashMap<String, Vec<ForestRelation>>);

impl Graph {
    fn link(&mut self, from: &str, to: &str, kind: RelationKind, strength: f32, cost: f32) {
        self.0
            .entry(from.to_string())
            .or_default()
            .push(ForestRelation {
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
    assert_eq!(
        ids,
        ["b", "a", "c"],
        "support order; `d` is three hops away"
    );
    let c = two.iter().find(|c| c.memory_id == "c").unwrap();
    assert_eq!(c.support, 0.25);
    assert_eq!(
        c.verify_cost, 0.1,
        "the cost of the relation to its parent, not the support"
    );

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

/// Similarity and conflict by explicit pairs, order-free.
struct Stub {
    similar: Vec<(&'static str, &'static str, f32)>,
    conflicting: Vec<(&'static str, &'static str)>,
}

impl ForestRelations for Stub {
    fn similarity(&self, a: &str, b: &str) -> f32 {
        self.similar
            .iter()
            .find(|(x, y, _)| (*x == a && *y == b) || (*x == b && *y == a))
            .map_or(0.0, |(_, _, s)| *s)
    }

    fn conflicts(&self, a: &str, b: &str) -> bool {
        self.conflicting
            .iter()
            .any(|(x, y)| (*x == a && *y == b) || (*x == b && *y == a))
    }
}

fn candidate(id: &str, parent: &str, support: f32, cost: f32, tokens: u32) -> EvidenceCandidate {
    EvidenceCandidate::new(id, parent, support, cost, tokens).unwrap()
}

// spec: MEDULLA-FEDERATED-QUERY-R736.3
#[test]
fn selection_is_a_forest_within_budget_and_drops_redundant_and_conflicting() {
    let candidates = vec![
        candidate("a", "s", 0.9, 0.1, 10),
        // A near-duplicate of `a`: its gain halves, so it is picked last and does not fit.
        candidate("dup", "s", 0.8, 0.1, 10),
        // Contradicts `a`: dropped for good once `a` is chosen.
        candidate("bad", "a", 0.8, 0.0, 10),
        candidate("child", "a", 0.6, 0.1, 10),
        // Two identical candidates: the lower memory id is chosen first.
        candidate("x2", "s", 0.5, 0.1, 10),
        candidate("x1", "s", 0.5, 0.1, 10),
    ];
    let stub = Stub {
        similar: vec![("dup", "a", 1.0)],
        conflicting: vec![("bad", "a")],
    };
    let seeds = vec!["s".to_string()];
    let budget = 40;
    let forest =
        select_forest(&candidates, &seeds, &stub, budget, ForestPenalty::default()).unwrap();

    // The budget bound holds and the tokens add up.
    assert!(forest.tokens_used <= budget);
    assert_eq!(forest.tokens_used, 40);
    // Roots attach to a seed, the lower-id tie-break picks x1 before x2.
    assert_eq!(forest.roots, ["a", "x1", "x2"]);
    // Every child's parent is chosen.
    assert_eq!(forest.edges, [("a".to_string(), "child".to_string())]);
    for (parent, _) in &forest.edges {
        assert!(forest.roots.contains(parent));
    }
    // The conflicting candidate and the redundant one are dropped, in the order considered.
    assert_eq!(forest.dropped, ["bad", "dup"]);

    // The same inputs give the same forest.
    let again =
        select_forest(&candidates, &seeds, &stub, budget, ForestPenalty::default()).unwrap();
    assert_eq!(again, forest);

    // A zero budget refuses with its code; no candidate chosen is an empty selection.
    let refused = select_forest(&candidates, &seeds, &stub, 0, ForestPenalty::default());
    assert_eq!(refused.unwrap_err().code, ForestErrorCode::BudgetZero);
    let empty = select_forest(&[], &seeds, &stub, 10, ForestPenalty::default()).unwrap();
    assert!(empty.roots.is_empty() && empty.edges.is_empty() && empty.tokens_used == 0);

    // With the conflict penalty off, the contradicting candidate is not dropped for it.
    let lax = ForestPenalty {
        conflict_weight: 0.0,
        ..ForestPenalty::default()
    };
    let roomy = select_forest(&candidates, &seeds, &stub, 1_000, lax).unwrap();
    assert!(roomy.edges.iter().any(|(_, child)| child == "bad"));
}
