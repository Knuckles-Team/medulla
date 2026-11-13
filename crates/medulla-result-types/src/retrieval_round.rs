//! The wire records of the served retrieval round (MEDULLA-FEDERATED-QUERY-R081.11,
//! R082.1).
//!
//! `Method::RetrievalRound` runs one plan and then the optional decision parts the
//! request carries. The Rust serde types here are the only contract; the Python client
//! is hand-written against a round-trip vector. The parts live beside the result types
//! because they embed the decision contracts of `retrieval_verdict` and `retrieval_stop`.

use serde::{Deserialize, Serialize};

use crate::retrieval_stop::{CalibrationTable, StopContext};
use crate::retrieval_verdict::{FallbackPolicy, SufficiencyVerdict};
use medulla_types::contract::BoundedVec;

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
