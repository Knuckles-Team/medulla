//! MEDULLA-FEDERATED-QUERY-R086.100.2 — ordered n-ary tuple scoring.
//!
//! A tuple `(m0, m1, …, mk)` scores as the geometric mean of
//! [`KanLinkModel::predict_prob`] over its consecutive ordered pairs
//! `(m0,m1), (m1,m2), …`. Pair features come from a [`FeatureCtx`] rebuilt at the
//! pinned graph revision, so a member unseen at fit gets structural features from
//! its current neighbours; [`FeatureCtx::impute_unseen_embeddings`] gives it the
//! mean of its neighbours' embeddings. Tuples with an unseen member carry
//! `inductive_inference`; all-seen tuples carry `observed_neighbourhood`.

use std::collections::HashSet;

use medulla_types::graphlearn_nary::NaryEvidenceClass;

use super::link_predict::{FeatureCtx, KanLinkModel};

/// Default of `graphlearn.nary_min_score` (probability, range [0, 1]).
pub const NARY_MIN_SCORE_DEFAULT: f64 = 0.5;

/// One scored ordered tuple (compact node indices, in request order).
#[derive(Debug, Clone, PartialEq)]
pub struct NaryScore {
    pub tuple: Vec<usize>,
    pub score: f64,
    pub evidence_class: NaryEvidenceClass,
}

/// Score ordered candidate tuples. `seen_at_fit` holds the compact indices of nodes
/// present when `model` was fitted; `ctx` must already carry any embedding
/// imputation for the unseen members. Tuples scoring below `min_score` are dropped.
/// Results sort by descending score; ties keep candidate (tuple) order.
pub fn score_nary(
    model: &KanLinkModel,
    ctx: &FeatureCtx,
    candidates: &[Vec<usize>],
    seen_at_fit: &HashSet<usize>,
    min_score: f64,
) -> Vec<NaryScore> {
    let mut out: Vec<NaryScore> = candidates
        .iter()
        .filter(|t| t.len() >= 2 && t.iter().all(|&m| m < ctx.node_count()))
        .filter_map(|t| {
            let pairs = t.len() - 1;
            let log_sum: f64 = t
                .windows(2)
                .map(|w| {
                    model
                        .predict_prob(&ctx.pair_features(w[0], w[1]))
                        .max(f64::MIN_POSITIVE)
                        .ln()
                })
                .sum();
            let score = (log_sum / pairs as f64).exp();
            if score < min_score {
                return None;
            }
            let evidence_class = if t.iter().all(|m| seen_at_fit.contains(m)) {
                NaryEvidenceClass::ObservedNeighbourhood
            } else {
                NaryEvidenceClass::InductiveInference
            };
            Some(NaryScore {
                tuple: t.clone(),
                score,
                evidence_class,
            })
        })
        .collect();
    // Stable sort: equal scores keep candidate order.
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_algos::AdjacencyGraph;
    use crate::graphlearn::link_predict::{fit_link_predictor, KanLinkConfig};

    fn graph(n: usize, edges: &[(usize, usize)]) -> AdjacencyGraph<usize> {
        AdjacencyGraph::from_adjacency(
            (0..n)
                .map(|i| {
                    let nbrs: Vec<(usize, f64)> = edges
                        .iter()
                        .filter_map(|&(a, b)| {
                            if a == i {
                                Some((b, 1.0))
                            } else if b == i {
                                Some((a, 1.0))
                            } else {
                                None
                            }
                        })
                        .collect();
                    (i, nbrs)
                })
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn unseen_member_scores_inductively() {
        // spec: MEDULLA-FEDERATED-QUERY-R086.100.2
        // Two dense 5-cliques {0..4} and {5..9}; fit on them.
        let mut edges = Vec::new();
        for base in [0, 5] {
            for a in base..base + 5 {
                for b in (a + 1)..base + 5 {
                    edges.push((a, b));
                }
            }
        }
        let fit_ctx = FeatureCtx::build(&graph(10, &edges), 0.5);
        let model = fit_link_predictor(&fit_ctx, &edges, &KanLinkConfig::default());
        let seen: HashSet<usize> = (0..10).collect();

        // Node 10 is added after fit, wired into the first clique.
        let mut now = edges.clone();
        now.extend([(10, 0), (10, 1), (10, 2), (10, 3)]);
        let g = graph(11, &now);
        let emb: Vec<Vec<f64>> = (0..11)
            .map(|i| {
                if i < 5 {
                    vec![1.0, 0.0]
                } else {
                    vec![0.0, 1.0]
                }
            })
            .collect();
        let mut ctx = FeatureCtx::build_with_embeddings(&g, 0.5, emb);
        let unseen: HashSet<usize> = [10].into_iter().collect();
        ctx.impute_unseen_embeddings(&unseen);

        let candidates = vec![vec![0, 10, 1], vec![1, 0, 2], vec![0, 10, 1]];
        let all = score_nary(&model, &ctx, &candidates, &seen, 0.0);
        assert_eq!(all.len(), 3);
        let inductive = all.iter().find(|s| s.tuple == vec![0, 10, 1]).unwrap();
        assert_eq!(
            inductive.evidence_class,
            NaryEvidenceClass::InductiveInference
        );
        let observed = all.iter().find(|s| s.tuple == vec![1, 0, 2]).unwrap();
        assert_eq!(
            observed.evidence_class,
            NaryEvidenceClass::ObservedNeighbourhood
        );
        // Geometric mean over consecutive ordered pairs.
        let p1 = model.predict_prob(&ctx.pair_features(0, 10));
        let p2 = model.predict_prob(&ctx.pair_features(10, 1));
        assert!((inductive.score - (p1 * p2).sqrt()).abs() < 1e-9);
        // Ties (the duplicated tuple) keep candidate order.
        let dup: Vec<usize> = all
            .iter()
            .enumerate()
            .filter(|(_, s)| s.tuple == vec![0, 10, 1])
            .map(|(i, _)| i)
            .collect();
        assert_eq!(dup.len(), 2);
        assert!(dup[0] < dup[1]);
        // The min-score knob drops everything above every score.
        assert!(score_nary(&model, &ctx, &candidates, &seen, 1.01).is_empty());
        // Imputed embedding = mean of neighbours (all in clique one).
        let f = ctx.pair_features(10, 0);
        assert!(
            (f[f.len() - 1] - 1.0).abs() < 1e-9,
            "cosine with clique one"
        );
    }
}
