//! The retrieval-quality verdict of one `Discover` answer
//! (MEDULLA-FEDERATED-QUERY-R089.1, MEDULLA-FEDERATED-QUERY-R089.2, MEDULLA-FEDERATED-QUERY-R089.7).
//!
//! One sufficiency rule serves the engine: the retrieval stop rule
//! (`medulla_plan::retrieval_stop`, R082.2). `Discover` presents its own result to it as a
//! one-round answer: the calibration baseline is `discover.min_composite_quality`, the
//! answer digest is the digest of the returned ids, and the confidence is the mean hit
//! score. The rule's reason then names the verdict: a confidence above the baseline is a
//! pass, a confidence at or below it (or no hit at all) is `low_relevance_topk`. The
//! other modes are measured here from counters the caller already holds. Cost is O(k).

use medulla_plan::retrieval_stop::{decide_stop, round_progress};
use medulla_result_types::retrieval_round::{sha256_hex_of, RoundAnswer};
use medulla_result_types::retrieval_stop::{
    CalibrationTable, RoundCalibration, StopContext, StopReason,
};
use medulla_retrieval::discover_signals::{SignalsReading, BASE_ADAPTER};
use medulla_types::contract::BoundedVec;
use medulla_types::ingestion_wire::DiscoverHit;
use medulla_types::wire::{
    DiscoverThresholds, RetrievalFailureMode, RetrievalVerdict, ShardGeneration,
};

/// The registered knobs a verdict reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct QualityKnobs {
    /// `knob discover.min_composite_quality`, score.
    pub min_composite_quality: f64,
    /// `knob discover.min_population_ratio`, ratio.
    pub min_population_ratio: f64,
    /// `knob discover.max_version_lag`, versions.
    pub max_version_lag: u64,
    /// `knob retrieval.round_limit`, rounds.
    pub round_limit: u32,
    /// `knob retrieval.token_limit`, tokens.
    pub token_limit: u64,
    /// `knob retrieval.inference_limit`, calls.
    pub inference_limit: u32,
    /// `knob retrieval.cost_limit`, micro-units.
    pub cost_limit: u64,
    /// `knob retrieval.saturation_epsilon`, score.
    pub saturation_epsilon: f64,
}

impl QualityKnobs {
    /// The values in the running registry.
    pub(crate) fn from_registry() -> Self {
        let k = medulla_server_api::knob_config::knobs();
        Self {
            min_composite_quality: k.discover_min_composite_quality,
            min_population_ratio: k.discover_min_population_ratio,
            max_version_lag: k.discover_max_version_lag,
            round_limit: u32::try_from(k.retrieval_round_limit).unwrap_or(u32::MAX),
            token_limit: k.retrieval_token_limit,
            inference_limit: u32::try_from(k.retrieval_inference_limit).unwrap_or(u32::MAX),
            cost_limit: k.retrieval_cost_limit,
            saturation_epsilon: k.retrieval_saturation_epsilon,
        }
    }
}

/// What the verdict is measured from.
#[derive(Debug, Clone)]
pub(crate) struct QualityInputs<'a> {
    pub hits: &'a [DiscoverHit],
    /// The query carried an embedding.
    pub has_embedding: bool,
    /// The keyword-only scan stopped at its cap.
    pub keyword_scan_capped: bool,
    /// Nodes holding an embedding.
    pub embedded_nodes: usize,
    /// Nodes of the candidate types.
    pub candidate_nodes: usize,
    pub graph_version: u64,
    /// The re-embedding frontier (`SemanticStore::embedded_through_version`).
    pub embedded_through: u64,
    pub index_generation: u64,
    /// One read of the drift and adapter evidence.
    pub signals: SignalsReading,
}

fn q16(x: f64) -> u16 {
    (x.clamp(0.0, 1.0) * 65_535.0).round() as u16
}

/// Judge one `Discover` answer.
pub(crate) fn quality_verdict(input: &QualityInputs<'_>, knobs: &QualityKnobs) -> RetrievalVerdict {
    let thresholds = DiscoverThresholds {
        min_composite_quality_q16: q16(knobs.min_composite_quality),
        min_population_ratio_q16: q16(knobs.min_population_ratio),
    };
    let quality = if input.hits.is_empty() {
        0.0
    } else {
        input.hits.iter().map(|h| f64::from(h.score)).sum::<f64>() / input.hits.len() as f64
    };
    let population = if input.candidate_nodes == 0 {
        1.0
    } else {
        (input.embedded_nodes as f64 / input.candidate_nodes as f64).min(1.0)
    };

    // The one stop rule decides whether the evidence clears the quality baseline.
    let ids: Vec<&str> = input.hits.iter().map(|h| h.id.as_str()).collect();
    let digest = sha256_hex_of(&ids);
    let answer = |round| RoundAnswer {
        round,
        answer_digest: digest.clone(),
        confidence: quality,
    };
    let table = CalibrationTable {
        rows: BoundedVec::new(vec![RoundCalibration {
            round: 1,
            baseline_confidence: knobs.min_composite_quality,
            margin_threshold: 0.0,
        }])
        .expect("one calibration row fits"),
    };
    let context = StopContext {
        round: 1,
        round_limit: knobs.round_limit.max(2),
        token_limit: knobs.token_limit,
        tokens_used: 0,
        inference_limit: knobs.inference_limit,
        inference_calls_used: 0,
        cost_limit: knobs.cost_limit,
        cost_used: 0,
        completed_round_progress: true,
        policy_revision: "discover-quality".to_string(),
    };
    let progress = round_progress(&ids, &[], Some(quality), None, knobs.saturation_epsilon);
    let stop = decide_stop(&table, &[answer(0), answer(1)], &context, &progress);
    let relevant = stop.reason == StopReason::AnswerRepeatedMarginMet;

    let lag = input.graph_version.saturating_sub(input.embedded_through);
    let mut failure_modes = Vec::new();
    if !relevant {
        failure_modes.push(RetrievalFailureMode::LowRelevanceTopk);
    }
    if population < knobs.min_population_ratio || !input.has_embedding {
        failure_modes.push(RetrievalFailureMode::SparseIndex);
    }
    if lag > knobs.max_version_lag {
        failure_modes.push(RetrievalFailureMode::StaleIndex);
    }
    if input.keyword_scan_capped {
        failure_modes.push(RetrievalFailureMode::ContextTruncation);
    }
    if !input.signals.firing_shards.is_empty() {
        failure_modes.push(RetrievalFailureMode::Drift);
    }
    RetrievalVerdict {
        gate_passed: failure_modes.is_empty(),
        above_threshold: relevant,
        failure_modes,
        composite_quality_q16: q16(quality),
        index_population_ratio_q16: q16(population),
        total_candidates: input.hits.len(),
        policy_revision: "discover-quality".to_string(),
        index_generation: input.index_generation,
        thresholds,
        adapter_digest: Some(
            input
                .signals
                .adapter_digest
                .clone()
                .unwrap_or_else(|| BASE_ADAPTER.to_string()),
        ),
        shard_generations: input
            .signals
            .shard_generations
            .iter()
            .map(|(shard_id, generation)| ShardGeneration {
                shard_id: shard_id.clone(),
                generation: *generation,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOBS: QualityKnobs = QualityKnobs {
        min_composite_quality: 0.3,
        min_population_ratio: 0.5,
        max_version_lag: 1000,
        round_limit: 4,
        token_limit: 32_000,
        inference_limit: 8,
        cost_limit: 1_000_000,
        saturation_epsilon: 0.01,
    };

    fn hit(id: &str, score: f32) -> DiscoverHit {
        DiscoverHit {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            node_type: "doc".into(),
            score,
            hop_path: Vec::new(),
            arm_ranks: Vec::new(),
            recency_boost: 1.0,
            url: None,
            retrieved_at: None,
            backend: None,
            query_digest: None,
        }
    }

    fn healthy(hits: &[DiscoverHit]) -> QualityInputs<'_> {
        QualityInputs {
            hits,
            has_embedding: true,
            keyword_scan_capped: false,
            embedded_nodes: 10,
            candidate_nodes: 10,
            graph_version: 50,
            embedded_through: 50,
            index_generation: 7,
            signals: SignalsReading::default(),
        }
    }

    fn modes(input: &QualityInputs<'_>) -> Vec<RetrievalFailureMode> {
        quality_verdict(input, &KNOBS).failure_modes
    }

    // spec: MEDULLA-FEDERATED-QUERY-R089.2
    #[test]
    fn each_mode_fires_on_its_trigger() {
        let good = [hit("a", 0.9), hit("b", 0.7)];
        let v = quality_verdict(&healthy(&good), &KNOBS);
        assert!(v.gate_passed && v.failure_modes.is_empty());
        assert_eq!(v.adapter_digest.as_deref(), Some("base"));
        assert_eq!(v.index_generation, 7);

        let weak = [hit("a", 0.1), hit("b", 0.2)];
        assert_eq!(
            modes(&healthy(&weak)),
            vec![RetrievalFailureMode::LowRelevanceTopk]
        );
        assert_eq!(
            modes(&healthy(&[])),
            vec![RetrievalFailureMode::LowRelevanceTopk]
        );

        let mut sparse = healthy(&good);
        sparse.embedded_nodes = 4;
        assert_eq!(modes(&sparse), vec![RetrievalFailureMode::SparseIndex]);
        let mut no_embedding = healthy(&good);
        no_embedding.has_embedding = false;
        assert_eq!(
            modes(&no_embedding),
            vec![RetrievalFailureMode::SparseIndex]
        );

        let mut stale = healthy(&good);
        stale.graph_version = 1051;
        assert_eq!(modes(&stale), vec![RetrievalFailureMode::StaleIndex]);
        stale.graph_version = 1050;
        assert!(modes(&stale).is_empty(), "a lag at the knob still passes");

        let mut capped = healthy(&good);
        capped.keyword_scan_capped = true;
        assert_eq!(
            modes(&capped),
            vec![RetrievalFailureMode::ContextTruncation]
        );
    }

    // spec: MEDULLA-FEDERATED-QUERY-R089.7
    #[test]
    fn drift_mode_reads_monitor_and_names_adapter() {
        let good = [hit("a", 0.9)];
        let mut input = healthy(&good);
        input.signals = SignalsReading {
            firing_shards: vec!["s1".into()],
            shard_generations: vec![("s1".into(), 3), ("s2".into(), 5)],
            adapter_digest: Some("ab".repeat(32)),
        };
        let v = quality_verdict(&input, &KNOBS);
        assert_eq!(v.failure_modes, vec![RetrievalFailureMode::Drift]);
        assert!(!v.gate_passed);
        assert_eq!(v.adapter_digest, Some("ab".repeat(32)));
        assert_eq!(
            v.shard_generations
                .iter()
                .map(|s| (s.shard_id.as_str(), s.generation))
                .collect::<Vec<_>>(),
            vec![("s1", 3), ("s2", 5)]
        );
    }
}
