//! The distilled rerank head scorer and its champion gate (MEDULLA-FEDERATED-QUERY-R081.102).
//!
//! A `RerankHead` is fitted offline by the harness from teacher judgments and stored with the
//! revisions it was fitted against. At query time [`score_with_head`] is a dot product over the
//! stored per-candidate features: no I/O and no model-service call. [`check_head_binding`] and
//! [`check_champion_quality`] are the two checks that keep a stale, unbound or below-threshold
//! artifact from ever scoring; both refuse with `RETRIEVAL_POLICY_UNVERIFIED`, so the prior
//! champion stays active.

use medulla_result_types::retrieval_round::sha256_hex_of;
use medulla_result_types::retrieval_verdict::{RetrievalRefusal, RETRIEVAL_POLICY_UNVERIFIED};
use serde::{Deserialize, Serialize};

/// The rank cut the champion metric is measured at.
const NDCG_CUT: usize = 10;

/// A fitted linear head with the evidence it is bound to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RerankHeadArtifact {
    /// One weight per stored feature, in the order the features are stored.
    pub weights: Vec<f64>,
    /// The corpus revision the head was fitted on.
    pub corpus_revision: String,
    /// The index revision whose stored features the head reads.
    pub index_revision: String,
    /// The teacher model revision whose judgments the head distilled.
    pub teacher_model_revision: String,
    /// Digest of the judgment dataset the head was fitted from.
    pub judgment_dataset_digest: String,
    /// The metric policy the head is evaluated under (names the cut and the metric).
    pub metric_policy: String,
    /// Digest over every field above; see [`artifact_digest`].
    pub artifact_digest: String,
}

/// One held-out query: the stored features of each candidate and its teacher gain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeldOutQuery {
    pub features: Vec<Vec<f64>>,
    pub gains: Vec<f64>,
}

fn unverified(detail: impl Into<String>) -> RetrievalRefusal {
    RetrievalRefusal::new(RETRIEVAL_POLICY_UNVERIFIED, detail)
}

/// The digest an artifact must carry: its weights and bound revisions, each length-prefixed.
pub fn artifact_digest(head: &RerankHeadArtifact) -> String {
    let weights = head
        .weights
        .iter()
        .map(|w| w.to_bits().to_string())
        .collect::<Vec<_>>()
        .join(",");
    sha256_hex_of(&[
        weights.as_str(),
        head.corpus_revision.as_str(),
        head.index_revision.as_str(),
        head.teacher_model_revision.as_str(),
        head.judgment_dataset_digest.as_str(),
        head.metric_policy.as_str(),
    ])
}

/// Refuse an artifact that is unbound (an empty revision or digest, a digest that does not
/// match its content) or stale (fitted on another corpus or index revision than the round's).
pub fn check_head_binding(
    head: &RerankHeadArtifact,
    corpus_revision: &str,
    index_revision: &str,
) -> Result<(), RetrievalRefusal> {
    let unbound = [
        &head.corpus_revision,
        &head.index_revision,
        &head.teacher_model_revision,
        &head.judgment_dataset_digest,
        &head.metric_policy,
        &head.artifact_digest,
    ]
    .iter()
    .any(|field| field.trim().is_empty());
    if unbound || head.weights.is_empty() || head.weights.iter().any(|w| !w.is_finite()) {
        return Err(unverified("the rerank head is not bound to its evidence"));
    }
    if head.artifact_digest != artifact_digest(head) {
        return Err(unverified(
            "the rerank head digest does not match its content",
        ));
    }
    if head.corpus_revision != corpus_revision || head.index_revision != index_revision {
        return Err(unverified(
            "the rerank head was fitted on another corpus or index revision",
        ));
    }
    Ok(())
}

/// The head's score of one candidate: the dot product of its weights and stored features.
/// Pure: reads nothing and calls no model.
pub fn score_with_head(
    head: &RerankHeadArtifact,
    features: &[f64],
) -> Result<f64, RetrievalRefusal> {
    if features.len() != head.weights.len() {
        return Err(unverified(
            "the stored feature count differs from the rerank head's weight count",
        ));
    }
    Ok(head.weights.iter().zip(features).map(|(w, x)| w * x).sum())
}

fn dcg(gains: impl Iterator<Item = f64>) -> f64 {
    gains
        .take(NDCG_CUT)
        .enumerate()
        .map(|(rank, gain)| (2f64.powf(gain) - 1.0) / ((rank + 2) as f64).log2())
        .sum()
}

/// Mean nDCG at 10 of the head over held-out queries. A query with no gain contributes 0.
pub fn held_out_ndcg(
    head: &RerankHeadArtifact,
    queries: &[HeldOutQuery],
) -> Result<f64, RetrievalRefusal> {
    if queries.is_empty() {
        return Err(unverified("the held-out target set has no query"));
    }
    let mut total = 0.0;
    for query in queries {
        if query.features.len() != query.gains.len() {
            return Err(unverified("a held-out query has a gain per candidate"));
        }
        let mut scored = Vec::with_capacity(query.gains.len());
        for (features, gain) in query.features.iter().zip(&query.gains) {
            scored.push((score_with_head(head, features)?, *gain));
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut ideal: Vec<f64> = query.gains.clone();
        ideal.sort_by(|a, b| b.total_cmp(a));
        let best = dcg(ideal.into_iter());
        if best > 0.0 {
            total += dcg(scored.into_iter().map(|(_, gain)| gain)) / best;
        }
    }
    Ok(total / queries.len() as f64)
}

/// The champion gate: the measured held-out metric must reach `retrieval.distill_min_quality`.
pub fn check_champion_quality(measured: f64, min_quality: f64) -> Result<(), RetrievalRefusal> {
    if measured.is_finite() && measured >= min_quality {
        Ok(())
    } else {
        Err(unverified(format!(
            "held-out nDCG@10 {measured} is below the champion floor {min_quality}"
        )))
    }
}

/// A refused promotion: the prior champion, returned unchanged, and the refusal that kept it.
/// Boxed in the error path so the success path of [`promote_champion`] stays small.
pub type RefusedPromotion = (Option<RerankHeadArtifact>, RetrievalRefusal);

/// Promote `candidate` over the `prior` champion: it must be bound to the round's revisions
/// and reach `min_quality` nDCG@10 on the held-out set. On success the candidate is the new
/// champion; on any refusal (`RETRIEVAL_POLICY_UNVERIFIED`) the `prior` champion stays and is
/// returned unchanged to the caller through the error path, so a failed evaluation never
/// retires it.
pub fn promote_champion(
    prior: Option<RerankHeadArtifact>,
    candidate: RerankHeadArtifact,
    corpus_revision: &str,
    index_revision: &str,
    held_out: &[HeldOutQuery],
    min_quality: f64,
) -> Result<RerankHeadArtifact, Box<RefusedPromotion>> {
    let verdict = check_head_binding(&candidate, corpus_revision, index_revision)
        .and_then(|()| held_out_ndcg(&candidate, held_out))
        .and_then(|measured| check_champion_quality(measured, min_quality));
    match verdict {
        Ok(()) => Ok(candidate),
        Err(refusal) => Err(Box::new((prior, refusal))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(weights: Vec<f64>) -> RerankHeadArtifact {
        let mut head = RerankHeadArtifact {
            weights,
            corpus_revision: "c1".into(),
            index_revision: "i1".into(),
            teacher_model_revision: "t1".into(),
            judgment_dataset_digest: "d1".into(),
            metric_policy: "ndcg@10".into(),
            artifact_digest: String::new(),
        };
        head.artifact_digest = artifact_digest(&head);
        head
    }

    fn queries() -> Vec<HeldOutQuery> {
        vec![HeldOutQuery {
            features: vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![0.5, 0.5]],
            gains: vec![2.0, 0.0, 1.0],
        }]
    }

    #[test]
    fn score_is_the_dot_product_and_refuses_a_width_mismatch() {
        let h = head(vec![2.0, 1.0]);
        assert_eq!(score_with_head(&h, &[3.0, 4.0]), Ok(10.0));
        let refusal = score_with_head(&h, &[1.0]).unwrap_err();
        assert_eq!(refusal.code, RETRIEVAL_POLICY_UNVERIFIED);
    }

    #[test]
    fn binding_refuses_stale_unbound_and_tampered_artifacts() {
        let h = head(vec![1.0, 0.0]);
        assert!(check_head_binding(&h, "c1", "i1").is_ok());
        assert!(check_head_binding(&h, "c2", "i1").is_err());
        assert!(check_head_binding(&h, "c1", "i2").is_err());
        let mut tampered = h.clone();
        tampered.weights[0] = 9.0;
        assert!(check_head_binding(&tampered, "c1", "i1").is_err());
        let mut unbound = h;
        unbound.teacher_model_revision.clear();
        assert!(check_head_binding(&unbound, "c1", "i1").is_err());
    }

    #[test]
    fn ndcg_is_one_for_the_ideal_order_and_lower_for_the_reverse() {
        let good = held_out_ndcg(&head(vec![1.0, 0.0]), &queries()).unwrap();
        let bad = held_out_ndcg(&head(vec![-1.0, 0.0]), &queries()).unwrap();
        assert!((good - 1.0).abs() < 1e-12);
        assert!(bad < good);
        assert!(held_out_ndcg(&head(vec![1.0, 0.0]), &[]).is_err());
    }

    #[test]
    fn champion_gate_admits_only_at_or_above_the_floor() {
        assert!(check_champion_quality(0.8, 0.8).is_ok());
        assert!(check_champion_quality(0.79, 0.8).is_err());
        assert!(check_champion_quality(f64::NAN, 0.8).is_err());
    }
}
