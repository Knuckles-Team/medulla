//! The `Discover` planner: arm fusion, recency and neighbour expansion
//! (MEDULLA-FEDERATED-QUERY-R090.2 to R090.5, MEDULLA-FEDERATED-QUERY-R090.8).
//!
//! `plan_discover` runs before ranking. Mode `standard` plans nothing. Mode `deep` asks the
//! one `ExpansionProvider` port once (MEDULLA-FEDERATED-QUERY-R651.3), embeds each expansion
//! with the one text embedder the Uql path binds, and returns one query vector per
//! expansion plus the evidence. `fuse_arms` merges the arms by reciprocal rank and
//! `recency_boost` scales a fused score by the age of the node. Neighbour expansion
//! (`expand_seeds`) follows.
//!
//! Neighbour expansion (MEDULLA-FEDERATED-QUERY-R090.8):
//!
//! `expand_hops` (in `medulla-graph-core`, shared with corpus retrieval) walks the graph breadth-first from the top-k fused hits over
//! the caller's row-projected core, so a neighbour the caller may not read is
//! never reached. Each hop keeps at most `fanout` neighbours per frontier node,
//! ordered by edge weight descending then node id. A neighbour scores
//! `seed score x decay ^ hop`. Hits dedupe by id, keep the best path, and the
//! list truncates to `k`. The walk writes nothing.

pub(crate) use medulla_graph_core::graph::ExpandedHit;
use medulla_graph_core::graph::{expand_hops, GraphCore};

/// Expand a `Discover` result: the fused top-k seeds plus their neighbours.
///
/// `fanout` and `decay` come from the registered knobs `discover.hop_fanout`
/// and `discover.hop_decay`.
pub(crate) fn expand_seeds(
    core: &GraphCore,
    seeds: Vec<(String, f32)>,
    hops: u32,
    k: usize,
) -> Vec<ExpandedHit> {
    if hops == 0 {
        return seeds
            .into_iter()
            .take(k)
            .map(|(id, score)| ExpandedHit {
                path: vec![id.clone()],
                id,
                score,
            })
            .collect();
    }
    let knobs = medulla_server_api::knob_config::knobs();
    let fanout = usize::try_from(knobs.discover_hop_fanout).unwrap_or(usize::MAX);
    expand_hops(core, &seeds, hops, fanout, knobs.discover_hop_decay, k)
}

use medulla_types::ingestion_wire::{ArmRank, DiscoverArm, DiscoverDegraded, ExpansionEvidence};

/// A planner refusal: the wire code and its detail. No rows, no writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanRefusal {
    pub code: &'static str,
    pub detail: String,
}

impl PlanRefusal {
    /// The wire error text `CODE: detail`.
    pub(crate) fn message(&self) -> String {
        format!("{}: {}", self.code, self.detail)
    }
}

/// The deep arms of one request: one query vector per expansion, the evidence, and the
/// degrade marker when the provider failed.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct DeepArms {
    pub vectors: Vec<Vec<f32>>,
    pub evidence: Vec<ExpansionEvidence>,
    pub degraded: Option<DiscoverDegraded>,
}

/// Plan one `Discover` request. Blocking (the provider and the embedder are network
/// clients): the caller runs it off the reactor.
#[cfg(feature = "query")]
pub(crate) fn plan_discover(
    request: &medulla_types::ingestion_wire::DiscoverPlanRequest,
    expansions: usize,
) -> Result<DeepArms, PlanRefusal> {
    use medulla_plan::query_understanding::{
        expansion_provider, run_expansion, ExpansionError, ExpansionKind, ExpansionRequest,
    };
    use medulla_types::ingestion_wire::{
        DiscoverMode, DISCOVER_PLANNER_UNCONFIGURED, EXPANSION_FANOUT_EXCEEDED, QUERY_TEXT_TOO_LONG,
    };
    request.validate().map_err(|refusal| PlanRefusal {
        code: refusal.code(),
        detail: refusal.to_string(),
    })?;
    if request.mode == DiscoverMode::Standard {
        return Ok(DeepArms::default());
    }
    let text = request
        .query_text
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| PlanRefusal {
            code: "INVALID_ARGUMENT",
            detail: "mode deep needs a non-empty query_text".into(),
        })?;
    let wanted = u8::try_from(expansions.clamp(1, 8)).unwrap_or(8);
    let asked = ExpansionRequest {
        query: text.to_string(),
        perspectives: wanted,
        feedback: Vec::new(),
    };
    let provider = expansion_provider();
    let items = match run_expansion(&asked) {
        Ok(items) => items,
        Err(ExpansionError::ProviderUnconfigured) => {
            return Err(PlanRefusal {
                code: DISCOVER_PLANNER_UNCONFIGURED,
                detail: "mode deep needs a configured expansion provider".into(),
            })
        }
        Err(ExpansionError::FanoutExceeded {
            requested,
            returned,
        }) => {
            return Err(PlanRefusal {
                code: EXPANSION_FANOUT_EXCEEDED,
                detail: format!("provider returned {returned} expansions for {requested}"),
            })
        }
        Err(ExpansionError::QueryTooLong { bytes }) => {
            return Err(PlanRefusal {
                code: QUERY_TEXT_TOO_LONG,
                detail: format!("query_text is {bytes} bytes"),
            })
        }
        Err(ExpansionError::ProviderFailed { message }) => {
            return Ok(DeepArms {
                degraded: Some(DiscoverDegraded::ProviderFailed { message }),
                ..DeepArms::default()
            })
        }
        Err(other) => {
            return Err(PlanRefusal {
                code: "INVALID_ARGUMENT",
                detail: other.to_string(),
            })
        }
    };
    let Some(embedder) = medulla_server_ops::api::query_txn::query_support::uql_text_embedder()
    else {
        return Ok(DeepArms {
            degraded: Some(DiscoverDegraded::ProviderFailed {
                message: "no text embedder is bound".into(),
            }),
            ..DeepArms::default()
        });
    };
    let (provider_id, provider_revision) = provider
        .map(|p| (p.provider_id(), p.provider_revision()))
        .unwrap_or_default();
    let mut arms = DeepArms::default();
    for item in items {
        match embedder.embed(&item.text) {
            Ok(vector) => {
                arms.vectors.push(vector);
                arms.evidence.push(ExpansionEvidence {
                    digest: item.digest,
                    kind: match item.kind {
                        ExpansionKind::Hypothetical => "hypothetical",
                        ExpansionKind::SubQuery => "sub_query",
                        ExpansionKind::Perspective => "perspective",
                    }
                    .to_string(),
                    provider_id: provider_id.clone(),
                    provider_revision: provider_revision.clone(),
                });
            }
            Err(message) => {
                return Ok(DeepArms {
                    degraded: Some(DiscoverDegraded::ProviderFailed { message }),
                    ..DeepArms::default()
                })
            }
        }
    }
    Ok(arms)
}

/// One retrieval arm's ranking, best first.
#[derive(Debug, Clone)]
pub(crate) struct ArmList {
    pub arm: DiscoverArm,
    pub ranked: Vec<(String, f32)>,
}

/// One fused hit with the rank it held in each arm that returned it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fused {
    pub id: String,
    pub score: f32,
    pub arm_ranks: Vec<ArmRank>,
}

/// Reciprocal-rank fusion: the score of an id is the sum over arms of `1 / (rrf_k + rank)`
/// with a 1-based rank. Ties order by id.
pub(crate) fn fuse_arms(arms: &[ArmList], rrf_k: u64) -> Vec<Fused> {
    let mut fused: std::collections::BTreeMap<&str, Fused> = std::collections::BTreeMap::new();
    for list in arms {
        for (index, (id, _)) in list.ranked.iter().enumerate() {
            let rank = u32::try_from(index + 1).unwrap_or(u32::MAX);
            let entry = fused.entry(id.as_str()).or_insert_with(|| Fused {
                id: id.clone(),
                score: 0.0,
                arm_ranks: Vec::new(),
            });
            entry.score += 1.0 / (rrf_k as f32 + rank as f32);
            entry.arm_ranks.push(ArmRank {
                arm: list.arm,
                rank,
            });
        }
    }
    let mut out: Vec<Fused> = fused.into_values().collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// The recency multiplier `1 + weight x 0.5 ^ (age_days / half_life_days)`. A node with no
/// date, or a date after `as_of`, gets `1.0`.
pub(crate) fn recency_boost(
    weight: f64,
    half_life_days: f64,
    as_of: i64,
    node_time: Option<i64>,
) -> f32 {
    let Some(time) = node_time.filter(|t| *t <= as_of) else {
        return 1.0;
    };
    let age_days = (as_of - time) as f64 / 86_400.0;
    (1.0 + weight * 0.5_f64.powf(age_days / half_life_days.max(f64::MIN_POSITIVE))) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(arm: DiscoverArm, ids: &[&str]) -> ArmList {
        ArmList {
            arm,
            ranked: ids.iter().map(|id| ((*id).to_string(), 0.0)).collect(),
        }
    }

    // spec: MEDULLA-FEDERATED-QUERY-R090.2
    #[test]
    fn fusion_decay_and_neutral_date() {
        let arms = [
            list(DiscoverArm::Semantic, &["a", "b", "c"]),
            list(DiscoverArm::Expansion, &["b", "c"]),
            list(DiscoverArm::Expansion, &["b"]),
        ];
        let fused = fuse_arms(&arms, 60);
        assert_eq!(
            fused.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["b", "a", "c"]
        );
        let b = &fused[0];
        let expected = 1.0 / 62.0 + 1.0 / 61.0 + 1.0 / 61.0;
        assert!((f64::from(b.score) - expected).abs() < 1e-6);
        assert_eq!(b.arm_ranks.len(), 3);
        assert_eq!(b.arm_ranks[0].rank, 2);

        let day = 86_400;
        let now = 100 * day;
        // Age 0 is the full boost; one half life halves the bonus.
        assert!((recency_boost(0.5, 30.0, now, Some(now)) - 1.5).abs() < 1e-6);
        assert!((recency_boost(0.5, 30.0, now, Some(now - 30 * day)) - 1.25).abs() < 1e-6);
        assert!(recency_boost(0.5, 30.0, now, Some(now - 3000 * day)) < 1.001);
        // No date, or a date after as_of, is neutral.
        assert_eq!(recency_boost(0.5, 30.0, now, None), 1.0);
        assert_eq!(recency_boost(0.5, 30.0, now, Some(now + 1)), 1.0);
        // A zero weight is neutral everywhere.
        assert_eq!(recency_boost(0.0, 30.0, now, Some(now)), 1.0);
    }
}
