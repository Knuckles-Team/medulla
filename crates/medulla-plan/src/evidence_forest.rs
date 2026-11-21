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
//! module keeps no neighbour index of its own.

use std::collections::{BTreeMap, BTreeSet};

use medulla_result_types::evidence_forest::{
    check_candidate_cap, check_seeds, EvidenceCandidate, ForestRefusal,
};

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
