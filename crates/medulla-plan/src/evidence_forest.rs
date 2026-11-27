//! Evidence forest candidate expansion (MEDULLA-FEDERATED-QUERY-R736.2).
//!
//! [`expand_candidates`] walks schema relations and semantic-similarity relations out of each
//! seed, up to `hops` steps, and scores every memory it reaches. Direct support is the
//! seed-to-candidate score: the seed's score times the strength of every relation on the
//! path. Verify cost is the cost of checking the relation that links the candidate to its
//! parent. The two stay separate values. A walk that reaches more than `cap` candidates
//! refuses with `FOREST_CANDIDATE_CAP`.
//!
//! The graph is read through [`ForestGraph`], the neighbour reader the round supplies; this
//! module keeps no neighbour index of its own. [`PlanForestGraph`] is that reader over the
//! round's own caller-projected view and semantic store.

use std::collections::{BTreeMap, BTreeSet};

use medulla_graph_core::graph::{edge_weight_in_view, GraphView};
use medulla_result_types::evidence_forest::{
    check_budget, check_candidate_cap, check_seeds, EvidenceCandidate, ForestRefusal,
    ForestSelection,
};
use medulla_semantic_index::semantic::SemanticStore;

/// Default `knob retrieval.forest.hops`.
pub const DEFAULT_FOREST_HOPS: u32 = 2;
/// Default `knob retrieval.forest.candidate_cap`.
pub const DEFAULT_FOREST_CANDIDATE_CAP: usize = 2_000;

/// The kind of relation a walk follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    /// A relation declared by the schema.
    Schema,
    /// A semantic-similarity relation between two memories.
    Similarity,
}

/// One relation out of a memory.
#[derive(Debug, Clone, PartialEq)]
pub struct ForestRelation {
    pub target: String,
    pub kind: RelationKind,
    /// How strongly the relation carries support, in `0..=1`.
    pub strength: f32,
    /// The cost of checking this relation, in `0..=1`.
    pub verify_cost: f32,
    /// The tokens the target spends when chosen.
    pub tokens: u32,
}

/// The neighbour reader of the walk: the schema and similarity relations out of one memory.
pub trait ForestGraph {
    fn relations(&self, memory_id: &str) -> Vec<ForestRelation>;
}

/// Expand `seeds` (`(memory id, score in 0..=1)`) over `schema` for at most `hops` steps.
/// Seeds are never candidates. A memory reached twice keeps its higher support. The result
/// is ordered by support, then memory id.
pub fn expand_candidates(
    seeds: &[(String, f32)],
    schema: &dyn ForestGraph,
    hops: u32,
    cap: usize,
) -> Result<Vec<EvidenceCandidate>, ForestRefusal> {
    check_seeds(seeds.len())?;
    let seed_ids: BTreeSet<&str> = seeds.iter().map(|(id, _)| id.as_str()).collect();
    let mut found: BTreeMap<String, EvidenceCandidate> = BTreeMap::new();
    // (memory id, support so far)
    let mut frontier: Vec<(String, f32)> = seeds
        .iter()
        .map(|(id, score)| (id.clone(), score.clamp(0.0, 1.0)))
        .collect();
    for _ in 0..hops {
        let mut next: Vec<(String, f32)> = Vec::new();
        for (parent, support) in &frontier {
            for relation in schema.relations(parent) {
                if seed_ids.contains(relation.target.as_str()) {
                    continue;
                }
                let reached = (support * relation.strength.clamp(0.0, 1.0)).clamp(0.0, 1.0);
                let better = found
                    .get(&relation.target)
                    .is_none_or(|held| reached > held.support);
                if !better {
                    continue;
                }
                let candidate = EvidenceCandidate::new(
                    relation.target.clone(),
                    parent.clone(),
                    reached,
                    relation.verify_cost.clamp(0.0, 1.0),
                    relation.tokens,
                )
                .map_err(|detail| {
                    ForestRefusal::new(
                        medulla_result_types::evidence_forest::ForestErrorCode::CandidateCap,
                        detail,
                    )
                })?;
                found.insert(relation.target.clone(), candidate);
                check_candidate_cap(found.len(), cap)?;
                next.push((relation.target, reached));
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    let mut out: Vec<EvidenceCandidate> = found.into_values().collect();
    out.sort_by(|a, b| {
        b.support
            .total_cmp(&a.support)
            .then_with(|| a.memory_id.cmp(&b.memory_id))
    });
    Ok(out)
}

/// The candidate cap and hops a round runs under: the call's values or the defaults.
pub fn forest_limits(hops: Option<u32>, cap: Option<usize>) -> (u32, usize) {
    (
        hops.unwrap_or(DEFAULT_FOREST_HOPS),
        cap.unwrap_or(DEFAULT_FOREST_CANDIDATE_CAP),
    )
}

/// The penalties one forest selection applies (`knob retrieval.forest.redundancy_weight` and
/// `knob retrieval.forest.conflict_weight`, MEDULLA-FEDERATED-QUERY-R736.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForestPenalty {
    /// How much a candidate's gain shrinks per unit of similarity to a chosen candidate.
    pub redundancy_weight: f32,
    /// Above zero, a candidate that conflicts with a chosen candidate is dropped for good.
    pub conflict_weight: f32,
}

impl Default for ForestPenalty {
    fn default() -> Self {
        Self {
            redundancy_weight: 0.5,
            conflict_weight: 1.0,
        }
    }
}

/// How candidates relate to one another: the selection reads redundancy and conflict here.
pub trait ForestRelations {
    /// Similarity of two memories in `0..=1`.
    fn similarity(&self, a: &str, b: &str) -> f32;
    /// Whether the two memories contradict each other.
    fn conflicts(&self, a: &str, b: &str) -> bool;
}

/// Select a forest of `candidates` under `budget_tokens`, greedily by gain per token
/// (MEDULLA-FEDERATED-QUERY-R736.3). A candidate is eligible when its parent is a seed or an
/// already chosen candidate. Its gain is `(support - verify_cost)` shrunk by its redundancy
/// with the chosen set; a candidate in conflict with a chosen one is dropped for good. The
/// highest positive score wins, ties go to the lower memory id. Selection stops when the best
/// candidate no longer fits the remaining budget or no positive score remains; every candidate
/// not chosen is in `dropped`, in the order it was considered. The same inputs give the same
/// forest.
pub fn select_forest(
    candidates: &[EvidenceCandidate],
    seeds: &[String],
    relations: &dyn ForestRelations,
    budget_tokens: u32,
    penalty: ForestPenalty,
) -> Result<ForestSelection, ForestRefusal> {
    check_budget(budget_tokens)?;
    let seed_set: BTreeSet<&str> = seeds.iter().map(String::as_str).collect();
    let mut chosen: Vec<&EvidenceCandidate> = Vec::new();
    let mut chosen_ids: BTreeSet<&str> = BTreeSet::new();
    let mut dropped: Vec<String> = Vec::new();
    let mut dropped_ids: BTreeSet<&str> = BTreeSet::new();
    let mut roots: Vec<String> = Vec::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut remaining = budget_tokens;
    loop {
        let mut best: Option<(f32, &EvidenceCandidate)> = None;
        for candidate in candidates {
            let id = candidate.memory_id.as_str();
            if chosen_ids.contains(id) || dropped_ids.contains(id) {
                continue;
            }
            let parent = candidate.parent.as_str();
            if !seed_set.contains(parent) && !chosen_ids.contains(parent) {
                continue;
            }
            if penalty.conflict_weight > 0.0
                && chosen
                    .iter()
                    .any(|held| relations.conflicts(id, &held.memory_id))
            {
                dropped.push(candidate.memory_id.clone());
                dropped_ids.insert(id);
                continue;
            }
            let redundancy = chosen
                .iter()
                .map(|held| relations.similarity(id, &held.memory_id))
                .fold(0.0_f32, f32::max);
            let gain = (candidate.support - candidate.verify_cost)
                * (1.0 - penalty.redundancy_weight * redundancy);
            let score = gain / candidate.tokens.max(1) as f32;
            if score <= 0.0 || score.is_nan() {
                continue;
            }
            let better = match best {
                None => true,
                Some((held_score, held)) => match score.total_cmp(&held_score) {
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Equal => candidate.memory_id < held.memory_id,
                    std::cmp::Ordering::Less => false,
                },
            };
            if better {
                best = Some((score, candidate));
            }
        }
        let Some((_, pick)) = best else { break };
        if pick.tokens > remaining {
            // The best candidate does not fit: it and every unchosen one are dropped.
            dropped.push(pick.memory_id.clone());
            dropped_ids.insert(pick.memory_id.as_str());
            break;
        }
        remaining -= pick.tokens;
        if seed_set.contains(pick.parent.as_str()) {
            roots.push(pick.memory_id.clone());
        } else {
            edges.push((pick.parent.clone(), pick.memory_id.clone()));
        }
        chosen_ids.insert(pick.memory_id.as_str());
        chosen.push(pick);
    }
    // Every candidate not chosen is left out, in the order it was considered.
    for candidate in candidates {
        let id = candidate.memory_id.as_str();
        if !chosen_ids.contains(id) && !dropped_ids.contains(id) {
            dropped.push(candidate.memory_id.clone());
        }
    }
    if chosen.is_empty() {
        return Ok(ForestSelection {
            dropped,
            ..ForestSelection::default()
        });
    }
    Ok(ForestSelection {
        roots,
        edges,
        tokens_used: budget_tokens - remaining,
        dropped,
    })
}

/// Schema relations cost this much to check (MEDULLA-FEDERATED-QUERY-R736.2).
const SCHEMA_VERIFY_COST: f32 = 0.1;

/// The round's own neighbour reader: schema relations out of the caller's projected view and
/// similarity relations out of its semantic store. It keeps no index of its own, and a target
/// outside the view's projection is never returned.
pub struct PlanForestGraph<'a> {
    view: &'a GraphView,
    semantic: &'a SemanticStore,
    similar_k: usize,
}

impl<'a> PlanForestGraph<'a> {
    /// A reader over `view` and `semantic` that keeps `similar_k` similarity neighbours.
    pub fn new(view: &'a GraphView, semantic: &'a SemanticStore, similar_k: usize) -> Self {
        Self {
            view,
            semantic,
            similar_k,
        }
    }

    /// `max(1, ceil(bytes / 4))` tokens for the target's stored blob, 1 when it has none.
    fn tokens_of(&self, target: &str) -> u32 {
        self.view.node_properties.get(target).map_or(1, |blob| {
            u32::try_from(blob.len().div_ceil(4))
                .unwrap_or(u32::MAX)
                .max(1)
        })
    }
}

impl ForestGraph for PlanForestGraph<'_> {
    fn relations(&self, memory_id: &str) -> Vec<ForestRelation> {
        let mut out = Vec::new();
        let mut listed: BTreeSet<String> = BTreeSet::new();
        if let Some(&index) = self.view.node_map.get(memory_id) {
            let neighbours: BTreeSet<&String> = self
                .view
                .graph
                .neighbors_undirected(index)
                .filter_map(|n| self.view.graph.node_weight(n))
                .filter(|id| id.as_str() != memory_id)
                .collect();
            for target in neighbours {
                let weight = edge_weight_in_view(self.view, memory_id, target);
                out.push(ForestRelation {
                    target: target.clone(),
                    kind: RelationKind::Schema,
                    strength: (weight as f32).clamp(0.0, 1.0),
                    verify_cost: SCHEMA_VERIFY_COST,
                    tokens: self.tokens_of(target),
                });
                listed.insert(target.clone());
            }
        }
        if let Some(embedding) = self.semantic.get_embedding(memory_id) {
            let mut similar = self.semantic.semantic_search_filtered(
                &embedding,
                self.similar_k.saturating_add(1),
                |candidate| candidate != memory_id && self.view.node_map.contains_key(candidate),
            );
            similar.truncate(self.similar_k);
            for (target, score) in similar {
                if listed.contains(&target) {
                    continue;
                }
                let strength = score.clamp(0.0, 1.0);
                out.push(ForestRelation {
                    tokens: self.tokens_of(&target),
                    target,
                    kind: RelationKind::Similarity,
                    strength,
                    verify_cost: 1.0 - strength,
                });
            }
        }
        out
    }
}

impl ForestRelations for PlanForestGraph<'_> {
    /// The cosine of the two stored embeddings clamped to `0..=1`; `0.0` when either has none.
    fn similarity(&self, a: &str, b: &str) -> f32 {
        let (Some(x), Some(y)) = (
            self.semantic.get_embedding(a),
            self.semantic.get_embedding(b),
        ) else {
            return 0.0;
        };
        if x.len() != y.len() {
            return 0.0;
        }
        let dot: f32 = x.iter().zip(&y).map(|(p, q)| p * q).sum();
        let norm = x.iter().map(|p| p * p).sum::<f32>().sqrt()
            * y.iter().map(|q| q * q).sum::<f32>().sqrt();
        if norm == 0.0 || !norm.is_finite() {
            return 0.0;
        }
        (dot / norm).clamp(0.0, 1.0)
    }

    /// An edge between the two in either direction whose relationship is `CONTRADICTS` or
    /// `ATTACKS`.
    fn conflicts(&self, a: &str, b: &str) -> bool {
        [(a, b), (b, a)].into_iter().any(|(src, tgt)| {
            self.view
                .edge_properties
                .get(&(src.to_string(), tgt.to_string()))
                .is_some_and(|blobs| {
                    blobs.iter().any(|blob| {
                        medulla_types::msgpack::decode_property_object(blob)
                            .ok()
                            .and_then(|props| {
                                props
                                    .get("relationship")
                                    .and_then(serde_json::Value::as_str)
                                    .map(|r| matches!(r, "CONTRADICTS" | "ATTACKS"))
                            })
                            .unwrap_or(false)
                    })
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_graph_core::graph::GraphCore;

    fn blob(value: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec_named(&value).expect("encodes")
    }

    // spec: MEDULLA-FEDERATED-QUERY-R736.2
    #[test]
    fn plan_forest_graph_reads_projected_view_and_similarity() {
        let core = GraphCore::new();
        for id in ["a", "b", "c", "d", "e"] {
            core.add_node(id.into(), blob(serde_json::json!({ "type": "Doc" })));
        }
        // A weighted edge, an unweighted edge, and an over-heavy weight that clamps to 1.
        core.add_edge(
            "a".into(),
            "b".into(),
            blob(serde_json::json!({ "weight": 0.4 })),
        )
        .unwrap();
        core.add_edge(
            "c".into(),
            "a".into(),
            blob(serde_json::json!({ "relationship": "REL" })),
        )
        .unwrap();
        core.add_edge(
            "a".into(),
            "d".into(),
            blob(serde_json::json!({ "weight": 7.0 })),
        )
        .unwrap();
        let view = core.analysis_snapshot();

        let mut semantic = SemanticStore::new();
        // `b` is both a schema neighbour and the nearest vector; `hidden` has an embedding
        // but is outside the projection.
        for (id, vector) in [
            ("a", vec![1.0, 0.0]),
            ("b", vec![0.99, 0.01]),
            ("e", vec![0.8, 0.6]),
            ("hidden", vec![0.98, 0.02]),
        ] {
            semantic.add_embedding(id.into(), vector).unwrap();
        }

        let reader = PlanForestGraph::new(&view, &semantic, 8);
        let relations = reader.relations("a");
        let find = |id: &str| relations.iter().find(|r| r.target == id);

        // Schema strengths: the weight, 1.0 with no weight, and a weight clamped to 1.
        let b = find("b").unwrap();
        assert_eq!(
            (b.kind, b.strength, b.verify_cost),
            (RelationKind::Schema, 0.4, 0.1)
        );
        assert_eq!(find("c").unwrap().strength, 1.0);
        assert_eq!(find("d").unwrap().strength, 1.0);
        // No duplicate neighbour: `b` is listed once even though it is also similar.
        assert_eq!(relations.iter().filter(|r| r.target == "b").count(), 1);
        // A similarity relation carries the score and the complementary cost.
        let e = find("e").unwrap();
        assert_eq!(e.kind, RelationKind::Similarity);
        assert!((e.strength - 0.8).abs() < 1e-3);
        assert!((e.verify_cost - (1.0 - e.strength)).abs() < 1e-6);
        // The hidden node is never a target, and neither is the memory itself.
        assert!(find("hidden").is_none() && find("a").is_none());
        // The token estimate is ceil(bytes / 4), at least 1.
        let bytes = view.node_properties["b"].len();
        assert_eq!(b.tokens as usize, bytes.div_ceil(4).max(1));

        // `similar_k` truncates the similarity relations.
        let none = PlanForestGraph::new(&view, &semantic, 1).relations("a");
        assert!(
            none.iter()
                .filter(|r| r.kind == RelationKind::Similarity)
                .count()
                <= 1
        );
        // A memory the projection does not hold reads no schema relations.
        assert!(reader
            .relations("hidden")
            .iter()
            .all(|r| r.kind == RelationKind::Similarity));
    }
}
