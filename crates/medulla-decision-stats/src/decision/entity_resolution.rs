//! Entity-resolution candidate generation and review drift
//! (MEDULLA-DECISION-ENGINE-R706.2).
//!
//! Pure and deterministic. [`embedding_is_current`] compares the stored
//! `(tenant, node, model, content_hash)` key, so an unchanged node is never
//! re-embedded. [`generate_candidates`] asks the ANN index for the nearest
//! neighbours with the tenant predicate inside the traversal, so another
//! tenant's node is never visited, scored or returned.
//! [`accept_rate_drift`] compares the accept rate of the last window of
//! reviews with the window before it.

use std::collections::HashMap;

use medulla_ann::HnswIndex;
use medulla_decision_types::decision::entity_resolution::{ReviewOutcome, TenantScopedEmbedding};

/// Most neighbours one node may ask for (the ceiling of `er.candidate_k`).
pub const MAX_CANDIDATE_K: usize = 64;

/// The stored embedding keys, by `(tenant, node)`.
#[derive(Debug, Clone, Default)]
pub struct EmbeddingKeys {
    by_node: HashMap<(String, String), (String, String)>,
}

impl EmbeddingKeys {
    /// Remember the key of a stored embedding, replacing the node's earlier one.
    pub fn record(&mut self, embedding: &TenantScopedEmbedding) {
        self.by_node.insert(
            (embedding.tenant.clone(), embedding.node.clone()),
            (embedding.model.clone(), embedding.content_hash.clone()),
        );
    }

    pub fn len(&self) -> usize {
        self.by_node.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_node.is_empty()
    }
}

/// Whether the stored embedding of `(tenant, node)` was made by `model` from
/// content hashing to `content_hash`; when it was, the node is not re-embedded.
pub fn embedding_is_current(
    keys: &EmbeddingKeys,
    tenant: &str,
    node: &str,
    model: &str,
    content_hash: &str,
) -> bool {
    keys.by_node
        .get(&(tenant.to_string(), node.to_string()))
        .is_some_and(|(stored_model, stored_hash)| {
            stored_model == model && stored_hash == content_hash
        })
}

/// Which `(tenant, node)` owns each ANN id; the id is the position.
#[derive(Debug, Clone, Default)]
pub struct SlotTable {
    owners: Vec<(String, String)>,
}

impl SlotTable {
    /// Reserve the next ANN id for `(tenant, node)`.
    pub fn assign(&mut self, tenant: &str, node: &str) -> u64 {
        self.owners.push((tenant.to_string(), node.to_string()));
        (self.owners.len() - 1) as u64
    }

    fn owner(&self, id: u64) -> Option<&(String, String)> {
        usize::try_from(id).ok().and_then(|i| self.owners.get(i))
    }
}

/// One neighbour of a node.
#[derive(Debug, Clone, PartialEq)]
pub struct NeighbourCandidate {
    pub node: String,
    /// The index metric's distance, smaller is nearer.
    pub distance: f32,
}

/// The nearest `candidate_k` nodes of `tenant` to `query`, nearest first, not
/// counting `node` itself. `candidate_k` is `knob er.candidate_k`; it is
/// clamped to `1..=MAX_CANDIDATE_K`. The tenant predicate is the index's own
/// admission test, so a node of another tenant is never visited.
pub fn generate_candidates(
    index: &HnswIndex,
    slots: &SlotTable,
    tenant: &str,
    node: &str,
    query: &[f32],
    candidate_k: usize,
) -> Vec<NeighbourCandidate> {
    let k = candidate_k.clamp(1, MAX_CANDIDATE_K);
    let ef = k.saturating_mul(4).max(32);
    let in_tenant = |id: u64| slots.owner(id).is_some_and(|(t, _)| t == tenant);
    let allow: &dyn Fn(u64) -> bool = &in_tenant;
    index
        .search_filtered(query, k + 1, ef, Some(allow))
        .into_iter()
        .filter_map(|hit| {
            let (_, neighbour) = slots.owner(hit.id)?;
            (neighbour != node).then(|| NeighbourCandidate {
                node: neighbour.clone(),
                distance: hit.distance,
            })
        })
        .take(k)
        .collect()
}

/// What the accept-rate comparison found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptDrift {
    /// Fewer than two full windows of reviews exist.
    Insufficient { have: usize, need: usize },
    /// The accept rate moved less than the maximum.
    Stable {
        recent_ppm: u32,
        prior_ppm: u32,
        change_ppm: u32,
    },
    /// The accept rate moved by at least the maximum.
    Drifted {
        recent_ppm: u32,
        prior_ppm: u32,
        change_ppm: u32,
    },
}

fn accept_ppm(window: &[ReviewOutcome]) -> u32 {
    let accepted = window
        .iter()
        .filter(|outcome| **outcome == ReviewOutcome::Accepted)
        .count() as u64;
    (accepted * 1_000_000 / window.len() as u64) as u32
}

/// Compare the accept rate over the last `window` reviews (`reviews` is in
/// arrival order) with the window before it. Drift is reported when the
/// absolute change reaches `max_ppm` (`knob er.drift_max_ppm`).
pub fn accept_rate_drift(reviews: &[ReviewOutcome], window: usize, max_ppm: u32) -> AcceptDrift {
    let need = window.saturating_mul(2);
    if window == 0 || reviews.len() < need {
        return AcceptDrift::Insufficient {
            have: reviews.len(),
            need,
        };
    }
    let end = reviews.len();
    let recent_ppm = accept_ppm(&reviews[end - window..]);
    let prior_ppm = accept_ppm(&reviews[end - need..end - window]);
    let change_ppm = recent_ppm.abs_diff(prior_ppm);
    if change_ppm >= max_ppm {
        AcceptDrift::Drifted {
            recent_ppm,
            prior_ppm,
            change_ppm,
        }
    } else {
        AcceptDrift::Stable {
            recent_ppm,
            prior_ppm,
            change_ppm,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_ann::Metric;

    fn hash(seed: usize) -> String {
        format!("{seed:064x}")
    }

    fn embedding(tenant: &str, node: &str, model: &str, seed: usize) -> TenantScopedEmbedding {
        TenantScopedEmbedding {
            tenant: tenant.to_string(),
            node: node.to_string(),
            model: model.to_string(),
            content_hash: hash(seed),
            vector: vec![0.0],
        }
    }

    /// A point on a line: nodes `n{i}` of tenant `t{i % 2}` at x = i.
    fn corpus(n: usize) -> (HnswIndex, SlotTable) {
        let mut index = HnswIndex::new(2, Metric::L2, 8, 32, 7);
        let mut slots = SlotTable::default();
        for i in 0..n {
            let tenant = format!("t{}", i % 2);
            let id = slots.assign(&tenant, &format!("n{i}"));
            index.insert(id, vec![i as f32, 0.0]);
        }
        (index, slots)
    }

    // spec: MEDULLA-DECISION-ENGINE-R706.2
    #[test]
    fn unchanged_nodes_skip_embedding_and_drift_is_detected() {
        for n in [10usize, 1_000, 100_000] {
            // Embed every node once; a second pass embeds only changed content.
            let mut keys = EmbeddingKeys::default();
            let mut embedded = 0usize;
            for pass in 0..2 {
                for i in 0..n {
                    let node = format!("n{i}");
                    // Every 10th node's content changes between the passes.
                    let seed = if pass == 1 && i % 10 == 0 { i + 1 } else { i };
                    if !embedding_is_current(&keys, "t0", &node, "m1", &hash(seed)) {
                        embedded += 1;
                        keys.record(&embedding("t0", &node, "m1", seed));
                    }
                }
            }
            assert_eq!(embedded, n + n.div_ceil(10), "{n} nodes");
            assert_eq!(keys.len(), n);
            // A new model, another tenant and another node never read as current.
            assert!(!embedding_is_current(&keys, "t0", "n1", "m2", &hash(1)));
            assert!(!embedding_is_current(&keys, "t1", "n1", "m1", &hash(1)));
            assert!(!embedding_is_current(&keys, "t0", "absent", "m1", &hash(1)));
        }

        // Candidates: nearest first, the node itself excluded, one tenant only.
        for n in [10usize, 1_000, 100_000] {
            let (index, slots) = corpus(n);
            let found = generate_candidates(&index, &slots, "t0", "n4", &[4.0, 0.0], 5);
            let nodes: Vec<&str> = found.iter().map(|c| c.node.as_str()).collect();
            assert_eq!(nodes.len(), 5.min(n / 2 - 1), "{n} nodes");
            assert!(!nodes.contains(&"n4"));
            for node in &nodes {
                let i: usize = node[1..].parse().unwrap();
                assert_eq!(i % 2, 0, "{node} belongs to another tenant");
            }
            assert!(found.windows(2).all(|w| w[0].distance <= w[1].distance));
            if n >= 100 {
                // Even nodes around x = 4: 2 and 6 at distance 2, then 0 and 8.
                assert_eq!(nodes, vec!["n2", "n6", "n0", "n8", "n10"]);
            }
        }
        let (index, slots) = corpus(100);
        let one = generate_candidates(&index, &slots, "t1", "n1", &[1.0, 0.0], 1);
        assert_eq!(one.len(), 1);
        assert!(generate_candidates(&index, &slots, "nobody", "n1", &[1.0, 0.0], 5).is_empty());
        // The ceiling holds whatever k is asked for.
        let many = generate_candidates(&index, &slots, "t0", "n0", &[0.0, 0.0], 1_000);
        assert_eq!(many.len(), MAX_CANDIDATE_K.min(49));

        // Drift: accept rate over the last window against the window before.
        let accepted = |n: usize| vec![ReviewOutcome::Accepted; n];
        let rejected = |n: usize| vec![ReviewOutcome::Rejected; n];
        for window in [20usize, 200, 5_000] {
            let mut reviews = [accepted(window / 2), rejected(window / 2)].concat();
            reviews.extend([accepted(window / 2), rejected(window / 2)].concat());
            assert_eq!(
                accept_rate_drift(&reviews, window, 150_000),
                AcceptDrift::Stable {
                    recent_ppm: 500_000,
                    prior_ppm: 500_000,
                    change_ppm: 0
                }
            );
            let mut drifted = [accepted(window / 2), rejected(window / 2)].concat();
            drifted.extend(accepted(window));
            assert_eq!(
                accept_rate_drift(&drifted, window, 150_000),
                AcceptDrift::Drifted {
                    recent_ppm: 1_000_000,
                    prior_ppm: 500_000,
                    change_ppm: 500_000
                }
            );
            assert_eq!(
                accept_rate_drift(&drifted[1..], window, 150_000),
                AcceptDrift::Insufficient {
                    have: window * 2 - 1,
                    need: window * 2
                }
            );
        }
        // The change must reach the maximum: exactly at it is drift.
        let mut edge = [accepted(8), rejected(12)].concat();
        edge.extend([accepted(11), rejected(9)].concat());
        assert!(matches!(
            accept_rate_drift(&edge, 20, 150_000),
            AcceptDrift::Drifted {
                change_ppm: 150_000,
                ..
            }
        ));
        assert!(matches!(
            accept_rate_drift(&edge, 20, 150_001),
            AcceptDrift::Stable { .. }
        ));
    }
}
