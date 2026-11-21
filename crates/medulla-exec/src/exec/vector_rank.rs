//! Vector `RANK` over a candidate set (EH-564/EH-565,
//! CONCEPT:MEDULLA-KG.query.filtered-vector-rank).
//!
//! A `Rank` ranks the candidates its input carries. Two physical paths:
//!
//! * **Exact** — the candidates' own embeddings are scored by cosine
//!   ([`SemanticStore::exact_rank_candidates`]), O(|candidates| · dim). Used whenever
//!   the whole candidate order is wanted, and for any candidate set up to
//!   [`EXACT_RANK_MAX`] when only a top `k` is wanted.
//! * **Filtered ANN** — for a larger candidate set feeding a `Limit k`, the ANN walk
//!   runs with the candidates as its allowlist, asking for `k · OVERSAMPLE` hits and
//!   widening by `OVERSAMPLE` each round. A walk that still strands candidates after
//!   [`ANN_ROUNDS`] rounds is topped up exactly, so `k` rows survive whenever `k`
//!   candidates hold an embedding (EH-564: a traversal-then-rank query returned 7–9
//!   of 10).
//!
//! The previous single path asked the filtered walk for `|candidates|` hits, which
//! both cost more than scoring the candidates (the walk's beam grew to the candidate
//! count) and could strand candidates the walk never reached.

use std::collections::HashSet;

use medulla_index_types::embedding::{EmbeddingSpaceRef, Modality};
use medulla_semantic_index::semantic::SemanticStore;

use crate::algebra::Op;
use crate::exec::PlanCtx;
use crate::rowset::RowSet;

/// The largest candidate set a top-`k` rank scores exactly. Above it the filtered ANN
/// walk is cheaper; the crossover was measured with `benches/filtered_rank.rs` (see
/// the EH-565 evidence in the lane WRAPUP).
pub(crate) const EXACT_RANK_MAX: usize = 4_096;

/// Oversampling factor of the filtered ANN walk, applied again on every widening round.
const OVERSAMPLE: usize = 4;

/// Widening rounds before the exact top-up.
const ANN_ROUNDS: usize = 3;

/// `RANK BY ~query` over every candidate (the whole order).
pub fn rank_op(ctx: &PlanCtx, query: &[f32], input: RowSet) -> Result<RowSet, String> {
    rank_top(ctx, query, input, None)
}

/// `Some((query, k))` when `ops[at]` is a vector `Rank` immediately followed by
/// `Limit k`, so only the top `k` of the ranking is ever read.
pub(crate) fn rank_then_limit(ops: &[Op], at: usize) -> Option<(&[f32], usize)> {
    match (ops.get(at)?, ops.get(at + 1)?) {
        (Op::Rank { query }, Op::Limit { k }) => Some((query.as_slice(), *k)),
        _ => None,
    }
}

/// `RANK BY ~query |> LIMIT k` as one physical step. The trailing `Limit` still runs
/// and is then a no-op.
pub(crate) fn rank_limit(
    ctx: &PlanCtx,
    query: &[f32],
    input: RowSet,
    k: usize,
) -> Result<RowSet, String> {
    rank_top(ctx, query, input, Some(k))
}

fn rank_top(
    ctx: &PlanCtx,
    query: &[f32],
    input: RowSet,
    limit: Option<usize>,
) -> Result<RowSet, String> {
    // MEDULLA-FEDERATED-QUERY-R039.1.2: a modality pair ranks only inside one space, and
    // the refusal precedes every other step, so an incompatible pair ranks nothing.
    if let Some(pair) = &ctx.modality_pair {
        check_pair(ctx.semantic.space(), pair.query, pair.target)?;
    }
    let candidates = input.id_set();
    if candidates.is_empty() {
        return Ok(RowSet::new());
    }
    check_width(ctx.semantic, query)?;
    let want = limit.map_or(candidates.len(), |k| k.min(candidates.len()));
    let scored = ranked(ctx.semantic, query, &candidates, want, EXACT_RANK_MAX);
    Ok(RowSet::from_scored(scored))
}

/// The identity tuple one modality binds: `(space id, dimensions, model revision)`.
pub(crate) type SpaceTuple<'a> = (&'a str, usize, &'a str);

/// The tuple `modality` binds inside `space`, or `None` when the space does not bind it.
/// A space with no modality bindings is a single-modality space and binds no pair.
pub(crate) fn bound_tuple(space: &EmbeddingSpaceRef, modality: Modality) -> Option<SpaceTuple<'_>> {
    space
        .modalities
        .iter()
        .any(|binding| binding.modality == modality)
        .then_some((space.id.as_str(), space.dimensions, space.revision.as_str()))
}

/// MEDULLA-FEDERATED-QUERY-R039.1.2, policy `compatible_space_only`: a query modality
/// ranks against a target modality if and only if both bind the same
/// `(space id, dimensions, model revision)` tuple. An unbound side never ranks. A pure
/// comparison of two identity tuples; it holds no state.
pub(crate) fn pair_allowed(query: Option<SpaceTuple<'_>>, target: Option<SpaceTuple<'_>>) -> bool {
    matches!((query, target), (Some(query), Some(target)) if query == target)
}

/// Refuse `EMBEDDING_SPACE_INCOMPATIBLE` unless the store's declared space binds both
/// modalities of the pair under one identity tuple.
fn check_pair(
    space: Option<&EmbeddingSpaceRef>,
    query: Modality,
    target: Modality,
) -> Result<(), String> {
    let tuple = |modality| space.and_then(|space| bound_tuple(space, modality));
    if pair_allowed(tuple(query), tuple(target)) {
        return Ok(());
    }
    Err(format!(
        "{}: modality pair {query:?} -> {target:?} does not bind one embedding space \
         (the store's declared space must bind both under the same id, dimensions and revision)",
        medulla_types::contract::ServerErrorCode::EmbeddingSpaceIncompatible
    ))
}

/// F4 (CONCEPT:MEDULLA-KG.compute.rank-dim-mismatch-guard): a query whose width differs
/// from the store's embedding width is a typed error, not a silent empty ranking.
fn check_width(semantic: &SemanticStore, query: &[f32]) -> Result<(), String> {
    let store_dim = semantic.dim();
    if store_dim == 0 || query.len() == store_dim {
        return Ok(());
    }
    Err(format!(
        "RANK BY ~{query:?}: query vector dimension mismatch — expected {store_dim} (the \
         store's embedding dimension), got {}",
        query.len()
    ))
}

/// The best `want` candidates, by the path the candidate count selects (see the module
/// docs). `exact_max` is the exact-path ceiling ([`EXACT_RANK_MAX`] when serving).
pub fn ranked(
    semantic: &SemanticStore,
    query: &[f32],
    candidates: &HashSet<&str>,
    want: usize,
    exact_max: usize,
) -> Vec<(String, f32)> {
    if candidates.len() <= exact_max || want >= candidates.len() {
        return semantic.exact_rank_candidates(query, candidates.iter().copied(), want);
    }
    let allow = |id: &str| candidates.contains(id);
    let mut fetch = want.saturating_mul(OVERSAMPLE);
    for _ in 0..ANN_ROUNDS {
        let mut hits = semantic.semantic_search_filtered(query, fetch, allow);
        if hits.len() >= want {
            hits.truncate(want);
            return hits;
        }
        fetch = fetch.saturating_mul(OVERSAMPLE);
    }
    semantic.exact_rank_candidates(query, candidates.iter().copied(), want)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::PlanCtx;
    use medulla_graph_core::graph::GraphCore;
    use medulla_index_types::embedding::ModalityBinding;
    use medulla_index_types::retrieval_request::ModalityPair;

    fn space(revision: &str, bound: &[Modality]) -> EmbeddingSpaceRef {
        let base = EmbeddingSpaceRef::pinned("model", revision, "hash", "pre", 4, true).unwrap();
        if bound.is_empty() {
            return base;
        }
        base.with_modalities(
            bound
                .iter()
                .map(|modality| ModalityBinding {
                    modality: *modality,
                    encoder: format!("encoder-{modality:?}"),
                })
                .collect(),
        )
        .unwrap()
    }

    fn pair_result(store: &SemanticStore, query: Modality, target: Modality) -> Result<RowSet, String> {
        let view = GraphCore::new().analysis_snapshot();
        let ctx = PlanCtx::new(&view, store).with_modality_pair(ModalityPair { query, target });
        rank_op(&ctx, &[1.0, 0.0, 0.0, 0.0], RowSet::from_ids(["a".to_string()]))
    }

    // spec: MEDULLA-FEDERATED-QUERY-R039.1.2
    #[test]
    fn sixteen_pairs_match_oracle() {
        let multimodal = SemanticStore::new_in_space(space("1", &Modality::ALL)).unwrap();
        let single = SemanticStore::new_in_space(space("1", &[])).unwrap();
        let legacy = SemanticStore::new();
        let mut checked = 0;
        for query in Modality::ALL {
            for target in Modality::ALL {
                // Oracle: every modality of a four-way space binds that space's one
                // tuple, so all sixteen pairs rank; a space with no modality bindings
                // and a store with no declared space rank none.
                assert!(pair_result(&multimodal, query, target).is_ok(), "{query:?}->{target:?}");
                for store in [&single, &legacy] {
                    let refusal = pair_result(store, query, target).unwrap_err();
                    assert!(
                        refusal.starts_with("EMBEDDING_SPACE_INCOMPATIBLE"),
                        "{query:?}->{target:?}: {refusal}"
                    );
                }
                checked += 1;
            }
        }
        assert_eq!(checked, 16);

        // The tuple comparison itself: id, dimensions and revision each decide it.
        let a = space("1", &Modality::ALL);
        let other_revision = space("2", &Modality::ALL);
        let other_dim = EmbeddingSpaceRef::pinned("model", "1", "hash", "pre", 8, true)
            .unwrap()
            .with_modalities(a.modalities.clone())
            .unwrap();
        let other_id = EmbeddingSpaceRef::pinned("other", "1", "hash", "pre", 4, true)
            .unwrap()
            .with_modalities(a.modalities.clone())
            .unwrap();
        for query in Modality::ALL {
            for target in Modality::ALL {
                assert!(pair_allowed(bound_tuple(&a, query), bound_tuple(&a, target)));
                for other in [&other_revision, &other_dim, &other_id] {
                    assert!(!pair_allowed(bound_tuple(&a, query), bound_tuple(other, target)));
                }
            }
        }
        // Without a modality pair, nothing is checked and a bare rank is unchanged.
        let view = GraphCore::new().analysis_snapshot();
        let ctx = PlanCtx::new(&view, &legacy);
        assert!(rank_op(&ctx, &[1.0], RowSet::from_ids(["a".to_string()])).is_ok());
    }
}
