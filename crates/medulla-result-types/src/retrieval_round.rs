//! The wire records of the served retrieval round (MEDULLA-FEDERATED-QUERY-R081.11,
//! R082.1).
//!
//! `Method::RetrievalRound` runs one plan and then the optional decision parts the
//! request carries. The Rust serde types here are the only contract; the Python client
//! is hand-written against a round-trip vector. The parts live beside the result types
//! because they embed the decision contracts of `retrieval_verdict` and `retrieval_stop`.

use serde::{Deserialize, Serialize};

use crate::retrieval_stop::{CalibrationTable, StopContext};
use crate::retrieval_verdict::{
    FallbackPolicy, HierarchyRetrievalPolicy, SectionGranularity, SelectionPolicy,
    SelectionReceipt, SufficiencyVerdict,
};
use medulla_types::contract::BoundedVec;

/// SHA-256 hex over the parts, each preceded by its length so two part lists never collide.
pub fn sha256_hex_of<S: AsRef<[u8]>>(parts: &[S]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.as_ref().len() as u64).to_le_bytes());
        hasher.update(part.as_ref());
    }
    hex::encode(hasher.finalize())
}

/// The digest of a selection policy as the round receipt names it.
pub fn selection_policy_digest(policy: &SelectionPolicy) -> String {
    let bytes = serde_json::to_vec(policy).unwrap_or_default();
    sha256_hex_of(&[bytes])
}

/// Most completed rounds a stop request may carry.
pub const MAX_ROUND_HISTORY: usize = 16;
/// Most prior row ids a stop request may carry.
pub const MAX_PRIOR_ROW_IDS: usize = 4096;

/// The sufficiency part of a round (R081.3): which round this is and what the
/// harness may do when evidence is short.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SufficiencyPart {
    /// 1-based round number.
    pub round: u32,
    /// The caller's round limit. Required: a part without one refuses.
    #[serde(default)]
    pub round_limit: Option<u32>,
    /// The row count the plan was run with; a repair may double it.
    pub k: u32,
    /// The mode a `SwitchMode` repair names.
    pub fallback_mode: String,
    /// What the harness may answer when evidence stays insufficient.
    pub fallback: FallbackPolicy,
}

/// One measured benchmark outcome of a selection policy or of its fixed-count baseline.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkResult {
    /// nDCG at 10 over the benchmark's labels.
    pub ndcg_at_10: f64,
    /// Mean number of chunks selected per query.
    pub mean_selected: f64,
}

/// The measured evidence that approves activating a selection policy (R081.7.5): the
/// policy and a fixed-count baseline, run on identical candidates, models, corpus and
/// tenant filters. `digest` binds the benchmark data, labels and both results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationBenchmark {
    pub digest: String,
    pub baseline: BenchmarkResult,
    pub candidate: BenchmarkResult,
}

/// The selection part of a round (R081.7.3): choose chunks from the plan's ranked rows by
/// the dual score. The embedding score of a chunk is the score of its executed row; the
/// cross-encoder score comes from the server-bound reranker over the node property `text`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionPart {
    /// The query text the reranker scores each chunk against.
    pub query: String,
    pub policy: SelectionPolicy,
    /// Evidence that approves activation. Without it, or when it does not meet the
    /// quality floor and cost ratio, the part refuses `RETRIEVAL_POLICY_UNVERIFIED`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark: Option<ActivationBenchmark>,
    /// How chunks are chosen (R081.100). Absent means the dual-score mode.
    #[serde(default, skip_serializing_if = "SelectionMode::is_dual_score")]
    pub mode: SelectionMode,
}

/// How a selection part chooses chunks. The joint mode never replaces the dual-score mode;
/// a request names one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectionMode {
    /// The dual-score mode of R081.7: embedding score and cross-encoder score.
    #[default]
    DualScore,
    /// Joint sparse nonnegative reconstruction of the query from the candidates' stored
    /// embeddings (R081.100). An absent number takes its knob value
    /// (`retrieval.joint_lambda`, `retrieval.joint_tolerance`, `retrieval.joint_max_iters`);
    /// `max_iters` can be lowered by the caller and never raised above the knob.
    JointSparse {
        /// The embedding space and revision the candidates' vectors were stored in.
        embedding_revision: String,
        /// The L1 weight; must be positive and finite.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lambda: Option<f64>,
        /// The coefficient change below which the solve stops.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tolerance: Option<f64>,
        /// Most coordinate-descent sweeps.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_iters: Option<u32>,
        /// The query vector in that space; absent means the bound embedder embeds
        /// the part's `query` text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query_vector: Option<Vec<f32>>,
    },
}

impl SelectionMode {
    /// True for the default mode, so an absent mode round-trips unchanged.
    pub fn is_dual_score(&self) -> bool {
        matches!(self, Self::DualScore)
    }
}

/// How a joint solve ended. Non-convergence refuses `RETRIEVAL_SOLVER_NOT_CONVERGED` and
/// never reaches a result, so the one value a result carries is `Converged`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolverStatus {
    /// The largest coefficient change fell below the tolerance.
    Converged,
}

/// One selected document and its reconstruction coefficient.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointCoefficient {
    pub id: String,
    pub coefficient: f64,
}

/// The result of a joint selection (R081.100): the documents whose coefficient is above
/// zero, coefficient-descending then id ascending, at most the policy's `max_chunks`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointSelection {
    pub embedding_revision: String,
    pub selected: Vec<JointCoefficient>,
    /// `||q - Vᵀx||₂` of the full solution.
    pub residual: f64,
    pub status: SolverStatus,
    pub iterations: u32,
    pub lambda: f64,
    pub tolerance: f64,
    /// Documents with a positive coefficient the `max_chunks` bound left out.
    pub truncated: u32,
}

/// Where a document-section traversal starts and what it returns (R081.9.7.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionRequest {
    pub root_id: String,
    pub document_revision: String,
    pub granularity: SectionGranularity,
}

/// The hierarchy part of a round (R081.9.3, R081.9.7.3). A `Community` policy descends
/// stored `Community` nodes with `query`; a `DocumentSection` policy walks stored
/// `SectionNode` nodes from `section` and makes no embedding or vector call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HierarchyPart {
    #[serde(default)]
    pub query: String,
    pub policy: HierarchyRetrievalPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<SectionRequest>,
}

/// Model calls a round made, counted apart (R081.7.5). `selection` is always zero: the
/// selector decides from scores that were already computed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceCounts {
    pub embedding: u32,
    pub reranker: u32,
    pub selection: u32,
}

/// One named, step-tagged fact a round proves its decision with (`retrieval.selection`,
/// `retrieval.hierarchy`, `retrieval.sections`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetrievalPremise {
    pub step: String,
    pub name: String,
    pub value: serde_json::Value,
}

/// One retrieval round: a plan, the revisions it is bound to, and the parts to run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundRequest {
    pub plan: medulla_types::wire::Plan,
    /// The corpus revision the caller retrieved against.
    pub corpus_revision: String,
    /// The model revision (embedder and scorers) the caller retrieved with.
    pub model_revision: String,
    /// Judge the executed rows against the sufficiency rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sufficiency: Option<SufficiencyPart>,
    /// Select chunks by the dual score.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SelectionPart>,
    /// Descend a stored community or document-section hierarchy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hierarchy: Option<HierarchyPart>,
    /// Decide whether the loop stops after this round (MEDULLA-FEDERATED-QUERY-R082.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopRequest>,
}

/// One executed row of the plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundRow {
    pub id: String,
    pub score: Option<f32>,
}

/// What one round returns: the rows and one optional verdict per requested part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundResult {
    pub rows: Vec<RoundRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sufficiency: Option<SufficiencyVerdict>,
    /// The selection receipt, when the request carried a `selection` part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SelectionReceipt>,
    /// The joint selection, when the selection part named the joint sparse mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joint_selection: Option<JointSelection>,
    /// Entity ids (community policy) or section ids (document-section policy) the
    /// hierarchy part selected, best first. They join the evidence set the sufficiency
    /// part counts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hierarchy_selected: Vec<String>,
    /// The stop verdict, when the request carried a `stop` part. A terminal verdict
    /// leaves the sufficiency repair at `Stop`, so no next round is described.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<crate::retrieval_stop::StopVerdict>,
    pub inference: InferenceCounts,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub premises: Vec<RetrievalPremise>,
}

/// One completed round's answer, as the caller reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundAnswer {
    pub round: u32,
    pub answer_digest: String,
    pub confidence: f64,
}

/// What the loop has consumed so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetUsage {
    pub tokens_used: u64,
    pub inference_calls_used: u32,
    pub cost_used: u64,
}

/// Limits a caller may lower; it can never raise one above the knob value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallerLimits {
    #[serde(default)]
    pub round_limit: Option<u32>,
    #[serde(default)]
    pub token_limit: Option<u64>,
    #[serde(default)]
    pub inference_limit: Option<u32>,
    #[serde(default)]
    pub cost_limit: Option<u64>,
}

/// The registered limits (`knob retrieval.round_limit`, `token_limit`,
/// `inference_limit`, `cost_limit`) a stop context is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopKnobs {
    pub round_limit: u32,
    pub token_limit: u64,
    pub inference_limit: u32,
    pub cost_limit: u64,
}

/// The stop part of a round (R082.1). Medulla builds the [`StopContext`] itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopRequest {
    pub calibration: CalibrationTable,
    pub history: BoundedVec<RoundAnswer, MAX_ROUND_HISTORY>,
    pub prior_row_ids: BoundedVec<String, MAX_PRIOR_ROW_IDS>,
    #[serde(default)]
    pub prior_quality: Option<f64>,
    pub usage: BudgetUsage,
    #[serde(default)]
    pub limits: CallerLimits,
}

impl StopRequest {
    /// Build the validated [`StopContext`]: each limit is the smaller of the knob and
    /// the caller's value, so a caller can lower a limit and never raise it.
    pub fn context(
        &self,
        knobs: &StopKnobs,
        round: u32,
        completed_round_progress: bool,
        policy_revision: &str,
    ) -> Result<StopContext, String> {
        let low = |knob: u64, caller: Option<u64>| caller.map_or(knob, |c| c.min(knob));
        let context = StopContext {
            round,
            round_limit: low(
                u64::from(knobs.round_limit),
                self.limits.round_limit.map(u64::from),
            ) as u32,
            token_limit: low(knobs.token_limit, self.limits.token_limit),
            tokens_used: self.usage.tokens_used,
            inference_limit: low(
                u64::from(knobs.inference_limit),
                self.limits.inference_limit.map(u64::from),
            ) as u32,
            inference_calls_used: self.usage.inference_calls_used,
            cost_limit: low(knobs.cost_limit, self.limits.cost_limit),
            cost_used: self.usage.cost_used,
            completed_round_progress,
            policy_revision: policy_revision.to_string(),
        };
        self.calibration.validate()?;
        context.validate()?;
        Ok(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval_stop::RoundCalibration;

    fn request() -> StopRequest {
        StopRequest {
            calibration: CalibrationTable {
                rows: BoundedVec::new(vec![RoundCalibration {
                    round: 1,
                    baseline_confidence: 0.4,
                    margin_threshold: 0.1,
                }])
                .expect("one row fits"),
            },
            history: BoundedVec::new(vec![RoundAnswer {
                round: 1,
                answer_digest: "d1".into(),
                confidence: 0.7,
            }])
            .expect("one round fits"),
            prior_row_ids: BoundedVec::new(vec!["a".to_string()]).expect("one id fits"),
            prior_quality: Some(0.5),
            usage: BudgetUsage {
                tokens_used: 100,
                inference_calls_used: 1,
                cost_used: 10,
            },
            limits: CallerLimits::default(),
        }
    }

    const KNOBS: StopKnobs = StopKnobs {
        round_limit: 4,
        token_limit: 32_000,
        inference_limit: 8,
        cost_limit: 1_000_000,
    };

    #[test]
    fn stop_request_round_trips_and_caller_cannot_raise_limits() {
        let mut stop = request();
        let encoded = serde_json::to_vec(&stop).expect("stop request serializes");
        let decoded: StopRequest = serde_json::from_slice(&encoded).expect("stop request decodes");
        assert_eq!(decoded, stop);

        // A caller that names nothing gets the knob values.
        let context = stop.context(&KNOBS, 1, true, "p1").expect("valid context");
        assert_eq!(
            (
                context.round_limit,
                context.token_limit,
                context.inference_limit,
                context.cost_limit
            ),
            (4, 32_000, 8, 1_000_000)
        );

        // A caller may lower a limit.
        stop.limits.round_limit = Some(2);
        stop.limits.token_limit = Some(500);
        let lowered = stop
            .context(&KNOBS, 1, true, "p1")
            .expect("lowered context");
        assert_eq!((lowered.round_limit, lowered.token_limit), (2, 500));

        // A caller never raises one above the knob.
        stop.limits = CallerLimits {
            round_limit: Some(99),
            token_limit: Some(u64::MAX),
            inference_limit: Some(99),
            cost_limit: Some(u64::MAX),
        };
        let capped = stop.context(&KNOBS, 1, true, "p1").expect("capped context");
        assert_eq!(
            (
                capped.round_limit,
                capped.token_limit,
                capped.inference_limit,
                capped.cost_limit
            ),
            (4, 32_000, 8, 1_000_000)
        );

        // Usage over a limit, a round past the limit, and a duplicate calibration round refuse.
        stop.usage.tokens_used = 40_000;
        assert!(stop.context(&KNOBS, 1, true, "p1").is_err());
        stop.usage.tokens_used = 0;
        assert!(stop.context(&KNOBS, 5, true, "p1").is_err());
        assert!(stop.context(&KNOBS, 1, true, " ").is_err());
    }

    #[test]
    fn history_and_prior_ids_are_bounded() {
        let answer = |round| RoundAnswer {
            round,
            answer_digest: "d".into(),
            confidence: 0.5,
        };
        assert!(
            BoundedVec::<RoundAnswer, MAX_ROUND_HISTORY>::new((0..17).map(answer).collect())
                .is_err()
        );
        assert!(BoundedVec::<String, MAX_PRIOR_ROW_IDS>::new(
            (0..4097).map(|i| i.to_string()).collect()
        )
        .is_err());
    }
}
