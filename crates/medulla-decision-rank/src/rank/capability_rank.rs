//! Rank blend of capability designation (MEDULLA-DECISION-ENGINE-R172.4).
//!
//! Stored and query vectors are L2-normalized and the similarity is their inner
//! product. The best `min(pool, max(k, oversample * k))` by cosine are blended:
//! `score = w_cos * cosine + w_reward * (reward - 0.5) + w_prior * (prior - 0.5)`,
//! all on Q32 integers. The question's weights are the caller's blend; the
//! `designation.reward_weight` and `designation.prior_weight` knobs are the
//! ceilings no caller may exceed. A weight of zero turns its term off, and ties
//! order by entity id ascending.

use std::collections::{BTreeMap, BTreeSet};

use medulla_decision_types::decision::capability_designation::{
    CapabilityDesignationQuestion, CapabilityReward, ClassIri, DesignationKnobs,
    NEUTRAL_REWARD_Q32, Q32_ONE,
};
use medulla_types::contract::OpaqueId;

/// One oversampled candidate, passed by value.
#[derive(Debug, Clone)]
pub struct PoolEntry {
    /// The candidate entity.
    pub entity_id: OpaqueId,
    /// Its stored vector (normalized here, so any scale is accepted).
    pub vector: Vec<f32>,
    /// Its ontology type, when the record carries one.
    pub ontology_type: Option<ClassIri>,
    /// Entities its record says may stand in for it.
    pub alternatives: BTreeSet<OpaqueId>,
}

/// The rank knobs, from `designation.*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RankPolicy {
    /// `designation.oversample_factor`.
    pub oversample_factor: usize,
    /// Ceiling of the reward weight, on Q32.
    pub reward_weight_ceiling_q32: i64,
    /// Ceiling of the prior weight, on Q32.
    pub prior_weight_ceiling_q32: i64,
}

impl From<&DesignationKnobs> for RankPolicy {
    fn from(knobs: &DesignationKnobs) -> Self {
        Self {
            oversample_factor: knobs.oversample_factor,
            reward_weight_ceiling_q32: knobs.reward_weight_q32,
            prior_weight_ceiling_q32: knobs.prior_weight_q32,
        }
    }
}

/// One ranked candidate with the three terms of its score.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedCandidate {
    /// The candidate entity.
    pub entity_id: OpaqueId,
    /// The blended score, on Q32.
    pub score_q32: i64,
    /// The cosine term, on Q32.
    pub cosine_q32: i64,
    /// The reward the candidate was scored with, on Q32.
    pub reward_q32: i64,
    /// The prior the candidate was scored with, on Q32 (0.5 when the term is off).
    pub prior_q32: i64,
    /// Its ontology type.
    pub ontology_type: Option<ClassIri>,
    /// Its swappable alternatives, sorted by id.
    pub alternatives: BTreeSet<OpaqueId>,
}

fn to_q32(value: f64) -> i64 {
    ((value * Q32_ONE as f64).round() as i64).clamp(-Q32_ONE, Q32_ONE)
}

fn unit(values: &[f32]) -> Vec<f64> {
    let norm = values
        .iter()
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum::<f64>()
        .sqrt();
    if norm > 0.0 {
        values.iter().map(|v| f64::from(*v) / norm).collect()
    } else {
        vec![0.0; values.len()]
    }
}

fn mul(a: i64, b: i64) -> i64 {
    ((i128::from(a) * i128::from(b)) >> 32) as i64
}

/// The type that gets 1.0 when the question names no prior: the most common
/// type of the pool, the smallest IRI among equals. `None` when no entry has one.
fn modal_type(entries: &[(PoolEntry, i64)]) -> Option<ClassIri> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (entry, _) in entries {
        if let Some(kind) = &entry.ontology_type {
            *counts.entry(kind.as_str()).or_default() += 1;
        }
    }
    let best = counts.values().copied().max()?;
    counts
        .into_iter()
        .find(|(_, count)| *count == best)
        .map(|(kind, _)| kind.to_string())
}

/// Blend cosine, learned reward and ontology prior and return the best
/// `question.k` candidates.
pub fn rank_capability_candidates(
    pool: Vec<PoolEntry>,
    question: &CapabilityDesignationQuestion,
    rewards: &BTreeMap<OpaqueId, CapabilityReward>,
    policy: &RankPolicy,
) -> Vec<RankedCandidate> {
    let k = usize::from(question.k);
    if k == 0 || pool.is_empty() {
        return Vec::new();
    }
    let query = unit(&question.query_vector.decode().unwrap_or_default());
    let mut scored: Vec<(PoolEntry, i64)> = pool
        .into_iter()
        .map(|entry| {
            let stored = unit(&entry.vector);
            let cosine: f64 = stored.iter().zip(&query).map(|(a, b)| a * b).sum();
            let cosine = to_q32(cosine);
            (entry, cosine)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.entity_id.cmp(&b.0.entity_id))
    });
    let take = scored.len().min(k.max(policy.oversample_factor.max(1) * k));
    scored.truncate(take);

    let weights = question.weights;
    let reward_weight = weights
        .reward_q32
        .clamp(0, policy.reward_weight_ceiling_q32.max(0));
    let prior_weight = weights
        .prior_q32
        .clamp(0, policy.prior_weight_ceiling_q32.max(0));
    let expected: Option<ClassIri> = question
        .ontology_prior
        .clone()
        .or_else(|| modal_type(&scored));
    let neutral = NEUTRAL_REWARD_Q32;

    let mut ranked: Vec<RankedCandidate> = scored
        .into_iter()
        .map(|(entry, cosine_q32)| {
            let reward_q32 = rewards
                .get(&entry.entity_id)
                .map_or(neutral, |reward| reward.reward_q32);
            let prior_q32 = match (&expected, &entry.ontology_type) {
                (Some(want), Some(have)) if want == have => Q32_ONE,
                _ => neutral,
            };
            let score_q32 = mul(weights.cosine_q32, cosine_q32)
                + mul(reward_weight, reward_q32 - neutral)
                + mul(prior_weight, prior_q32 - neutral);
            RankedCandidate {
                entity_id: entry.entity_id,
                score_q32,
                cosine_q32,
                reward_q32,
                prior_q32,
                ontology_type: entry.ontology_type,
                alternatives: entry.alternatives,
            }
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.score_q32
            .cmp(&a.score_q32)
            .then_with(|| a.entity_id.cmp(&b.entity_id))
    });
    ranked.truncate(k);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_decision_types::decision::capability_designation::{
        DesignationWeights, QueryVector,
    };
    use medulla_types::contract::TenantId;

    const A: &str = "https://knuckles-team.github.io/ontology/core#Agent";
    const B: &str = "https://knuckles-team.github.io/ontology/core#Tool";

    fn id(name: &str) -> OpaqueId {
        OpaqueId::new(name).unwrap()
    }

    fn at(cosine: f32) -> Vec<f32> {
        vec![cosine, (1.0 - cosine * cosine).sqrt()]
    }

    fn entry(name: &str, vector: Vec<f32>, kind: Option<&str>) -> PoolEntry {
        PoolEntry {
            entity_id: id(name),
            vector,
            ontology_type: kind.map(str::to_string),
            alternatives: BTreeSet::new(),
        }
    }

    fn question(
        k: u8,
        reward: i64,
        prior: i64,
        ontology_prior: Option<&str>,
    ) -> CapabilityDesignationQuestion {
        CapabilityDesignationQuestion {
            tenant_id: TenantId::new("tenant-a").unwrap(),
            query_vector: QueryVector::from_f32("emb-v1", &[1.0, 0.0]).unwrap(),
            required_capabilities: BTreeSet::new(),
            required_policy_tags: BTreeSet::new(),
            k,
            weights: DesignationWeights {
                cosine_q32: Q32_ONE,
                reward_q32: reward,
                prior_q32: prior,
            },
            ontology_prior: ontology_prior.map(str::to_string),
        }
    }

    fn policy() -> RankPolicy {
        RankPolicy {
            oversample_factor: 4,
            reward_weight_ceiling_q32: Q32_ONE,
            prior_weight_ceiling_q32: Q32_ONE,
        }
    }

    fn reward(name: &str, value: f64) -> (OpaqueId, CapabilityReward) {
        let mut r = CapabilityReward::neutral(TenantId::new("tenant-a").unwrap(), id(name));
        r.reward_q32 = (value * Q32_ONE as f64) as i64;
        (id(name), r)
    }

    fn order(ranked: &[RankedCandidate]) -> Vec<&str> {
        ranked.iter().map(|r| r.entity_id.as_str()).collect()
    }

    // spec: MEDULLA-DECISION-ENGINE-R172.4
    #[test]
    fn blend_order_matches_fixtures() {
        let half = Q32_ONE / 2;
        let none = BTreeMap::new();

        // Exact cosine order with both weights at zero, whatever the rewards say.
        let pool = vec![
            entry("c", at(0.2), None),
            entry("a", at(0.99), None),
            entry("b", at(0.7), None),
        ];
        let rewards: BTreeMap<_, _> = [reward("c", 1.0), reward("a", 0.0)].into_iter().collect();
        let pure =
            rank_capability_candidates(pool.clone(), &question(3, 0, 0, None), &rewards, &policy());
        assert_eq!(order(&pure), ["a", "b", "c"]);
        assert!(pure[0].cosine_q32 > pure[1].cosine_q32);

        // A planted top-1: a learned reward lifts the lower cosine past the leader.
        let lifted =
            rank_capability_candidates(pool, &question(1, half, 0, None), &rewards, &policy());
        assert_eq!(order(&lifted), ["b"]);
        // The blend is cosine + w * (reward - 0.5): b at 1.0 reward has none, so only a drops.
        let both = rank_capability_candidates(
            vec![entry("a", at(0.99), None), entry("b", at(0.7), None)],
            &question(2, half, 0, None),
            &[reward("a", 0.0), reward("b", 1.0)].into_iter().collect(),
            &policy(),
        );
        assert_eq!(order(&both), ["b", "a"]);
        assert_eq!(both[0].reward_q32, Q32_ONE);

        // A weight above the knob ceiling is clamped to it.
        let capped = RankPolicy {
            reward_weight_ceiling_q32: 0,
            ..policy()
        };
        let clamped = rank_capability_candidates(
            vec![entry("a", at(0.99), None), entry("b", at(0.7), None)],
            &question(2, half, 0, None),
            &[reward("a", 0.0), reward("b", 1.0)].into_iter().collect(),
            &capped,
        );
        assert_eq!(order(&clamped), ["a", "b"]);

        // Type-coherent top-k: the modal type of the pool gets the default prior.
        let typed = vec![
            entry("t1", at(0.99), Some(A)),
            entry("t2", at(0.98), Some(B)),
            entry("t3", at(0.97), Some(A)),
            entry("t4", at(0.96), Some(A)),
        ];
        let coherent = rank_capability_candidates(
            typed.clone(),
            &question(2, 0, half, None),
            &none,
            &policy(),
        );
        assert_eq!(order(&coherent), ["t1", "t3"]);
        // An explicit prior overrides the default one.
        let explicit =
            rank_capability_candidates(typed, &question(2, 0, half, Some(B)), &none, &policy());
        assert_eq!(explicit[0].entity_id.as_str(), "t2");
        assert_eq!(explicit[0].prior_q32, Q32_ONE);
        // With no stored types the prior term is off.
        let untyped = rank_capability_candidates(
            vec![entry("u1", at(0.9), None), entry("u2", at(0.8), None)],
            &question(2, 0, half, None),
            &none,
            &policy(),
        );
        assert_eq!(order(&untyped), ["u1", "u2"]);
        assert!(untyped.iter().all(|r| r.prior_q32 == NEUTRAL_REWARD_Q32));

        // Ties order by entity id; each result lists its alternatives sorted by id.
        let mut m2 = entry("m2", at(0.5), None);
        m2.alternatives = [id("z"), id("y")].into_iter().collect();
        let tied = rank_capability_candidates(
            vec![m2, entry("m1", at(0.5), None)],
            &question(2, 0, 0, None),
            &none,
            &policy(),
        );
        assert_eq!(order(&tied), ["m1", "m2"]);
        assert_eq!(
            tied[1]
                .alternatives
                .iter()
                .map(OpaqueId::as_str)
                .collect::<Vec<_>>(),
            ["y", "z"]
        );
        // k bounds the answer; k of zero and an empty pool answer nothing.
        assert_eq!(
            rank_capability_candidates(
                vec![entry("a", at(0.9), None), entry("b", at(0.8), None)],
                &question(1, 0, 0, None),
                &none,
                &policy()
            )
            .len(),
            1
        );
        assert!(
            rank_capability_candidates(vec![], &question(2, 0, 0, None), &none, &policy())
                .is_empty()
        );
    }
}
