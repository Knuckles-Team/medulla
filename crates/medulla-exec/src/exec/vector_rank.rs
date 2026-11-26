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
//!   doubling the window each round ([`replenish_until_k`], MEDULLA-FEDERATED-QUERY-R039).
//!   A walk that still strands candidates after [`ReplenishPolicy::max_rounds`] rounds is
//!   topped up exactly, so `k` rows survive whenever `k` candidates hold an embedding
//!   (EH-564: a traversal-then-rank query returned 7–9 of 10); otherwise the answer
//!   carries a typed [`Shortfall`].
//!
//! The previous single path asked the filtered walk for `|candidates|` hits, which
//! both cost more than scoring the candidates (the walk's beam grew to the candidate
//! count) and could strand candidates the walk never reached.

use std::collections::HashSet;

use medulla_index_types::embedding::{EmbeddingSpaceRef, Modality};
use medulla_query_types::wire::PlanScopeLeaf;
use medulla_semantic_index::semantic::SemanticStore;
use medulla_types::contract::ServerErrorCode;
use medulla_types::retrieval::{Shortfall, ShortfallReason};

use crate::algebra::Op;
use crate::exec::PlanCtx;
use crate::rowset::RowSet;

/// The largest candidate set a top-`k` rank scores exactly. Above it the filtered ANN
/// walk is cheaper; the crossover was measured with `benches/filtered_rank.rs` (see
/// the EH-565 evidence in the lane WRAPUP).
pub(crate) const EXACT_RANK_MAX: usize = 4_096;

/// MEDULLA-FEDERATED-QUERY-R039: how a top-`k` rank replenishes candidates the filter
/// stranded. The executor crate cannot read the registry, so the served planner resolves
/// the knobs `retrieval.replenish_max_rounds` and `retrieval.oversample_factor` and binds
/// them with [`PlanCtx::with_replenish`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplenishPolicy {
    /// Widening rounds before the exact top-up (`retrieval.replenish_max_rounds`).
    pub max_rounds: usize,
    /// The first window is `k` times this (`retrieval.oversample_factor`).
    pub oversample: usize,
}

impl Default for ReplenishPolicy {
    fn default() -> Self {
        Self {
            max_rounds: 6,
            oversample: 4,
        }
    }
}

/// The best rows of a top-`k` rank and, when fewer than `k` exist, why.
#[derive(Clone, Debug, PartialEq)]
pub struct Replenished {
    /// Up to `k` `(id, score)` pairs, best first.
    pub scored: Vec<(String, f32)>,
    /// `Some` exactly when `scored` holds fewer than `k` rows.
    pub shortfall: Option<Shortfall>,
    /// Rows the rank visited (MEDULLA-FEDERATED-QUERY-R040.5.6): every candidate of an exact
    /// scan, plus the window each widening round of the filtered walk asked of the index.
    pub examined: u64,
}

/// The ids of `input` a ranking step may rank: all of them, narrowed to the compiled
/// hard constraints when the request carries any (MEDULLA-FEDERATED-QUERY-R040.1.3). The
/// one filter every ranking leg (vector, text, fused) takes, so a candidate that fails a
/// constraint can never reach a ranked result.
pub(crate) fn eligible_ids<'r>(ctx: &PlanCtx, input: &'r RowSet) -> HashSet<&'r str> {
    let mut ids = input.id_set();
    if let Some(constraints) = &ctx.constraints {
        ids.retain(|id| constraints.contains(id));
    }
    // MEDULLA-FEDERATED-QUERY-R078.6: the plan scope's allow set at the vector leaf.
    ids.retain(|id| super::scope_allow::allows(ctx, PlanScopeLeaf::Vector, id));
    ids
}

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
    // MEDULLA-FEDERATED-QUERY-R039: a top-0 rank is a malformed request, refused before
    // any candidate is read.
    if limit == Some(0) {
        return Err(format!(
            "{}: a ranked step asked for k = 0 rows",
            ServerErrorCode::RetrievalKInvalid
        ));
    }
    let candidates = eligible_ids(ctx, &input);
    if candidates.is_empty() {
        // Nothing is eligible: zero rows, and a top-`k` ask names why.
        return Ok(RowSet::new().with_shortfall(limit.map(|k| Shortfall {
            requested: k as u64,
            found: 0,
            missing: 0,
            reason: ShortfallReason::EligibleExhausted,
        })));
    }
    check_width(ctx.semantic, query)?;
    // MEDULLA-FEDERATED-QUERY-R040.2.3: with topic fusion bound, the whole eligible set is
    // scored densely, fused with the topic evidence, and only then cut to `k`.
    if ctx.topic_policy.is_some() {
        return rank_topic_fused(ctx, query, &candidates, limit);
    }
    let Some(k) = limit else {
        let scored = ctx
            .semantic
            .exact_rank_candidates(query, candidates.iter().copied(), candidates.len());
        return Ok(RowSet::from_scored(scored));
    };
    // `k` above the plan's result-row budget is capped and reported, never refused.
    let cap = ctx.budget.max_result_rows;
    let found = replenish_until_k(
        ctx.semantic,
        query,
        &candidates,
        k.min(cap),
        EXACT_RANK_MAX,
        ctx.replenish,
    );
    let shortfall = if k > cap {
        Some(Shortfall {
            requested: k as u64,
            found: found.scored.len() as u64,
            missing: 0,
            reason: ShortfallReason::BudgetExhausted,
        })
    } else {
        found.shortfall
    };
    // MEDULLA-FEDERATED-QUERY-R040.5.6: the rows this rank visited against the
    // `4 * result_rows + 64` bound, and whether the index it read holds no tombstones.
    let result_rows = found.scored.len() as u64;
    let visits = medulla_types::retrieval::TraceAnn {
        examined: found.examined,
        compacted: ctx.semantic.ann_compacted(),
        visit_bound_hit: found.examined > medulla_types::retrieval::ann_visit_bound(result_rows),
    };
    Ok(RowSet::from_scored(found.scored)
        .with_shortfall(shortfall)
        .with_ann_visits(Some(visits)))
}

/// The topic-fused path of [`rank_top`] (MEDULLA-FEDERATED-QUERY-R040.2.3): exact dense
/// scores over every eligible candidate that holds an embedding, re-scored by
/// [`super::topic::fuse_ranked`], then truncated to `limit` (capped to the plan's
/// result-row budget, reported like the plain path).
fn rank_topic_fused(
    ctx: &PlanCtx,
    query: &[f32],
    candidates: &HashSet<&str>,
    limit: Option<usize>,
) -> Result<RowSet, String> {
    let dense = ctx
        .semantic
        .exact_rank_candidates(query, candidates.iter().copied(), candidates.len());
    let embedded = dense.len();
    let (mut fused, receipt) = super::topic::fuse_ranked(ctx, query, dense)?;
    let Some(k) = limit else {
        return Ok(RowSet::from_scored(fused).with_topic_receipt(receipt));
    };
    let cap = ctx.budget.max_result_rows;
    fused.truncate(k.min(cap));
    let shortfall = if k > cap {
        Some(Shortfall {
            requested: k as u64,
            found: fused.len() as u64,
            missing: 0,
            reason: ShortfallReason::BudgetExhausted,
        })
    } else if fused.len() < k {
        Some(Shortfall {
            requested: k as u64,
            found: fused.len() as u64,
            missing: (candidates.len() - embedded) as u64,
            reason: if embedded < candidates.len() {
                ShortfallReason::MissingEmbedding
            } else {
                ShortfallReason::EligibleExhausted
            },
        })
    } else {
        None
    };
    Ok(RowSet::from_scored(fused)
        .with_shortfall(shortfall)
        .with_topic_receipt(receipt))
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

/// MEDULLA-FEDERATED-QUERY-R039: the best `k` candidates, by the path the candidate count
/// selects (see the module docs; `exact_max` is the exact-path ceiling,
/// [`EXACT_RANK_MAX`] when serving). The filtered ANN walk asks for `k * oversample`
/// hits and doubles the window until `k` candidates survive or `max_rounds` is reached;
/// an exact top-up then follows, so fewer than `k` rows come back only when fewer than
/// `k` candidates hold an embedding. That case, and only that case, is a [`Shortfall`]:
/// `missing_embedding` when any candidate holds none, else `eligible_exhausted`.
pub fn replenish_until_k(
    semantic: &SemanticStore,
    query: &[f32],
    candidates: &HashSet<&str>,
    k: usize,
    exact_max: usize,
    policy: ReplenishPolicy,
) -> Replenished {
    let want = k.min(candidates.len());
    let exact = || semantic.exact_rank_candidates(query, candidates.iter().copied(), want);
    let mut examined = 0_u64;
    let scored = if candidates.len() <= exact_max || want >= candidates.len() {
        examined = candidates.len() as u64;
        exact()
    } else {
        let allow = |id: &str| candidates.contains(id);
        let mut fetch = want.saturating_mul(policy.oversample.max(1));
        let mut walked = None;
        for _ in 0..policy.max_rounds.max(1) {
            examined = examined.saturating_add(fetch as u64);
            let mut hits = semantic.semantic_search_filtered(query, fetch, allow);
            if hits.len() >= want {
                hits.truncate(want);
                walked = Some(hits);
                break;
            }
            fetch = fetch.saturating_mul(2);
        }
        walked.unwrap_or_else(|| {
            // The exact top-up visits every candidate.
            examined = examined.saturating_add(candidates.len() as u64);
            exact()
        })
    };
    let missing = candidates
        .iter()
        .filter(|id| semantic.get_embedding(id).is_none())
        .count() as u64;
    let shortfall = (scored.len() < k).then(|| Shortfall {
        requested: k as u64,
        found: scored.len() as u64,
        missing,
        reason: if missing > 0 {
            ShortfallReason::MissingEmbedding
        } else {
            ShortfallReason::EligibleExhausted
        },
    });
    Replenished {
        scored,
        shortfall,
        examined,
    }
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

    fn store(embedded: usize) -> SemanticStore {
        let mut store = SemanticStore::new();
        for index in 0..embedded {
            store
                .add_embedding(format!("e{index}"), vec![1.0, index as f32 * 0.01])
                .unwrap();
        }
        store
    }

    // spec: MEDULLA-FEDERATED-QUERY-R039
    #[test]
    fn replenish_returns_k_or_names_the_shortfall() {
        let semantic = store(20);
        let ids: Vec<String> = (0..20).map(|index| format!("e{index}")).collect();
        let all: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let policy = ReplenishPolicy::default();
        // The exact path and the forced filtered-ANN path both return k rows.
        for exact_max in [EXACT_RANK_MAX, 0] {
            let got = replenish_until_k(&semantic, &[1.0, 0.0], &all, 5, exact_max, policy);
            assert_eq!(got.scored.len(), 5);
            assert_eq!(got.shortfall, None);
        }
        // Fewer embedded candidates than k: found, requested and the reason.
        let mut with_unembedded = all.clone();
        with_unembedded.insert("none");
        let got = replenish_until_k(&semantic, &[1.0, 0.0], &with_unembedded, 25, EXACT_RANK_MAX, policy);
        assert_eq!(got.scored.len(), 20);
        assert_eq!(
            got.shortfall,
            Some(Shortfall {
                requested: 25,
                found: 20,
                missing: 1,
                reason: ShortfallReason::MissingEmbedding
            })
        );
        let got = replenish_until_k(&semantic, &[1.0, 0.0], &all, 25, EXACT_RANK_MAX, policy);
        assert_eq!(got.shortfall.map(|s| s.reason), Some(ShortfallReason::EligibleExhausted));
        assert_eq!(got.shortfall.map(|s| s.missing), Some(0));
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.5.6
    #[test]
    fn visits_are_counted_against_the_four_times_result_rows_plus_sixty_four_bound() {
        use medulla_types::retrieval::ann_visit_bound;
        let semantic = store(300);
        let ids: Vec<String> = (0..300).map(|index| format!("e{index}")).collect();
        let all: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let policy = ReplenishPolicy::default();

        // The bound is `4 * result_rows + 64`.
        assert_eq!(ann_visit_bound(0), 64);
        assert_eq!(ann_visit_bound(10), 104);

        // The exact path visits every candidate: 300 of them for 10 rows breaks the bound.
        let exact = replenish_until_k(&semantic, &[1.0, 0.0], &all, 10, EXACT_RANK_MAX, policy);
        assert_eq!(exact.examined, 300);
        assert!(exact.examined > ann_visit_bound(exact.scored.len() as u64));

        // The filtered walk asks for `k * oversample` hits in its first window, which
        // finds all 10 here: 40 visits, inside the bound.
        let walked = replenish_until_k(&semantic, &[1.0, 0.0], &all, 10, 0, policy);
        assert_eq!(walked.scored.len(), 10);
        assert_eq!(walked.examined, 40);
        assert!(walked.examined <= ann_visit_bound(walked.scored.len() as u64));

        // A selective filter over a third of the corpus is still walked in one window.
        let third: HashSet<&str> = ids.iter().step_by(3).map(String::as_str).collect();
        let selective = replenish_until_k(&semantic, &[1.0, 0.0], &third, 10, 0, policy);
        assert_eq!(selective.scored.len(), 10);
        assert_eq!(selective.examined, 40);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R039.1.4
    #[test]
    fn shortfall_counts_missing_embeddings() {
        let semantic = store(4);
        let candidates = ["e0", "e1", "ghost-a", "ghost-b"];
        let all: HashSet<&str> = candidates.into_iter().collect();
        let got = replenish_until_k(
            &semantic,
            &[1.0, 0.0],
            &all,
            4,
            EXACT_RANK_MAX,
            ReplenishPolicy::default(),
        );
        let shortfall = got.shortfall.expect("two candidates hold no embedding");
        assert_eq!((shortfall.found, shortfall.missing), (2, 2));
        assert_eq!(shortfall.reason, ShortfallReason::MissingEmbedding);
        // Fewer eligible items than k, all embedded: nothing is missing.
        let embedded: HashSet<&str> = ["e0", "e1"].into_iter().collect();
        let got = replenish_until_k(
            &semantic,
            &[1.0, 0.0],
            &embedded,
            4,
            EXACT_RANK_MAX,
            ReplenishPolicy::default(),
        );
        let shortfall = got.shortfall.unwrap();
        assert_eq!(shortfall.missing, 0);
        assert_eq!(shortfall.reason, ShortfallReason::EligibleExhausted);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R039
    #[test]
    fn k_zero_is_refused_and_budget_caps_k() {
        let semantic = store(8);
        let view = GraphCore::new().analysis_snapshot();
        let ids: Vec<String> = (0..8).map(|index| format!("e{index}")).collect();
        let input = || RowSet::from_ids(ids.clone());
        let ctx = PlanCtx::new(&view, &semantic);
        let refused = rank_limit(&ctx, &[1.0, 0.0], input(), 0).unwrap_err();
        assert!(refused.starts_with("RETRIEVAL_K_INVALID"), "{refused}");
        let capped = ctx.clone().with_budget(crate::budget::Budget {
            max_result_rows: 3,
            ..ctx.budget
        });
        let rows = rank_limit(&capped, &[1.0, 0.0], input(), 5).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows.shortfall().map(|s| (s.requested, s.found, s.reason)),
            Some((5, 3, ShortfallReason::BudgetExhausted))
        );
        let rows = rank_limit(&ctx, &[1.0, 0.0], input(), 5).unwrap();
        assert!(rows.shortfall().is_none() && rows.len() == 5);
    }

    // spec: MEDULLA-FEDERATED-QUERY-R040.2.3
    #[test]
    fn topic_fusion_reorders_before_the_cut_and_refuses_without_a_backend() {
        use crate::exec::topic::{InMemoryTopicBackend, TopicFusion, TopicRankingPolicy};
        let space = EmbeddingSpaceRef::pinned("topic-model", "1", "h", "p", 2, true).unwrap();
        let mut semantic = SemanticStore::new_in_space(space.clone()).unwrap();
        // `a` is nearest the query; `c` sits on the heavier topic's centroid.
        for (id, vector) in [("a", [1.0, 0.1]), ("b", [0.9, 0.5]), ("c", [0.2, 1.0])] {
            semantic.add_embedding(id.to_string(), vec![vector[0], vector[1]]).unwrap();
        }
        let view = GraphCore::new().analysis_snapshot();
        let input = || RowSet::from_ids(["a", "b", "c"].map(String::from));
        let backend = InMemoryTopicBackend {
            centroids: vec![("t".to_string(), vec![0.0, 1.0])],
            corpus_revision: 7,
            space: space.clone(),
            revisions: Default::default(),
        };
        let bound = |weight: f32, current: Option<u64>| {
            PlanCtx::new(&view, &semantic)
                .with_topic_policy(TopicRankingPolicy::new(1, 0.0).unwrap().with_weight(weight).unwrap())
                .with_topic_fusion(TopicFusion {
                    backend: std::sync::Arc::new(InMemoryTopicBackend {
                        centroids: backend.centroids.clone(),
                        corpus_revision: backend.corpus_revision,
                        space: backend.space.clone(),
                        revisions: Default::default(),
                    }),
                    current_corpus_revision: current,
                })
        };
        let order = |ctx: &PlanCtx, k: usize| rank_limit(ctx, &[1.0, 0.0], input(), k).map(|rows| rows.ids());
        // w = 0 is the dense order; w = 1 is the topic order, applied before the cut to k = 1.
        assert_eq!(order(&bound(0.0, Some(7)), 3).unwrap(), ["a", "b", "c"]);
        assert_eq!(order(&bound(1.0, Some(7)), 3).unwrap()[0], "c");
        assert_eq!(order(&bound(1.0, Some(7)), 1).unwrap(), ["c"]);
        // A stale corpus, an unbound revision and a missing backend all refuse.
        let stale = order(&bound(0.3, Some(8)), 3).unwrap_err();
        assert!(stale.starts_with("TOPIC_CORPUS_STALE"), "{stale}");
        let unbound = order(&bound(0.3, None), 3).unwrap_err();
        assert!(unbound.starts_with("TOPIC_BACKEND_UNAVAILABLE"), "{unbound}");
        let mut no_backend = bound(0.3, Some(7));
        no_backend.topic_fusion = None;
        let missing = order(&no_backend, 3).unwrap_err();
        assert!(missing.starts_with("TOPIC_BACKEND_UNAVAILABLE"), "{missing}");
        // With no topic policy bound, the rank is the plain dense rank.
        let plain = PlanCtx::new(&view, &semantic);
        assert_eq!(order(&plain, 3).unwrap(), ["a", "b", "c"]);
    }
}
