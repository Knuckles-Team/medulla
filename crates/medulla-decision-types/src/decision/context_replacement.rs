//! Held-out comparison that gates replacing a raw context
//! (MEDULLA-DECISION-ENGINE-R137.14).
//!
//! A candidate context replaces the raw one only when every expected held-out
//! query has one verdict and the candidate's mean score stays within the
//! tolerance of the raw mean. Scores are the caller's claim; the aggregation is
//! the integer mean in micro units. Any failure retains the raw context.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::context_package::is_sha256_hex;
use super::record::outcome::{DecisionRefusal, RefusalCode};
use medulla_types::contract::BoundedVec;

/// The most held-out queries one comparison carries.
pub const MAX_PAIRED_QUERIES: usize = 256;

/// One micro unit scale: a score of 1.0.
pub const SCORE_ONE_MICRO: u64 = 1_000_000;

/// The raw and the candidate answer to one held-out query, scored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct QueryVerdict {
    pub query_id: String,
    pub raw_answer_digest: String,
    pub candidate_answer_digest: String,
    /// The raw answer's score in micro units, at most one million.
    pub raw_score_micro: u64,
    /// The candidate answer's score in micro units, at most one million.
    pub candidate_score_micro: u64,
}

/// A paired held-out evaluation of a raw and a candidate context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PairedContextEvaluation {
    pub held_out_set_id: String,
    pub held_out_set_revision: u64,
    pub held_out_set_digest: String,
    pub expected_queries: u32,
    pub raw_context_digest: String,
    pub candidate_context_digest: String,
    pub evaluator_revision: String,
    pub metric_id: String,
    pub verdicts: BoundedVec<QueryVerdict, MAX_PAIRED_QUERIES>,
}

/// What the comparison concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ReplacementVerdict {
    /// The candidate holds within the tolerance on every expected query.
    Replace,
    /// The candidate fell beyond the tolerance; the raw context stays.
    RetainRaw,
}

impl ReplacementVerdict {
    /// The option id the verdict is answered as.
    pub fn option_id(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::RetainRaw => "retain_raw",
        }
    }
}

/// The two integer means and the verdict they give.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplacementDecision {
    pub verdict: ReplacementVerdict,
    pub raw_mean_micro: u64,
    pub candidate_mean_micro: u64,
}

/// Why a comparison cannot conclude; the raw revision is retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplacementRefusal {
    /// A query has no verdict, or one has two, or none was expected.
    VerdictsIncomplete { expected: u32, observed: usize },
    /// A digest is malformed or differs from the one the caller pinned.
    DigestMismatch { field: String },
    /// The metric is unnamed or a score lies outside `[0, 1]`.
    MetricUndefined { reason: String },
}

/// Every refusal code of the replacement gate.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new("PAIRED_VERDICTS_INCOMPLETE"),
    RefusalCode::new("PAIRED_DIGEST_MISMATCH"),
    RefusalCode::new("PAIRED_METRIC_UNDEFINED"),
];

impl DecisionRefusal for ReplacementRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::VerdictsIncomplete { .. } => REFUSALS[0],
            Self::DigestMismatch { .. } => REFUSALS[1],
            Self::MetricUndefined { .. } => REFUSALS[2],
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::VerdictsIncomplete { expected, observed } => {
                format!("expected {expected} queries with one verdict each, observed {observed}; raw retained")
            }
            Self::DigestMismatch { field } => format!("{field} does not match; raw retained"),
            Self::MetricUndefined { reason } => format!("{reason}; raw retained"),
        }
    }
}

impl PairedContextEvaluation {
    /// Check every digest and, when pinned, the context and evaluator against
    /// what the caller expects.
    fn check_digests(
        &self,
        expected_raw_context_digest: Option<&str>,
        expected_evaluator_revision: Option<&str>,
    ) -> Result<(), ReplacementRefusal> {
        let mismatch = |field: &str| ReplacementRefusal::DigestMismatch {
            field: field.to_string(),
        };
        for (field, value) in [
            ("held_out_set_digest", &self.held_out_set_digest),
            ("raw_context_digest", &self.raw_context_digest),
            ("candidate_context_digest", &self.candidate_context_digest),
        ] {
            if !is_sha256_hex(value) {
                return Err(mismatch(field));
            }
        }
        if self.raw_context_digest == self.candidate_context_digest {
            return Err(mismatch("candidate_context_digest"));
        }
        if expected_raw_context_digest.is_some_and(|d| d != self.raw_context_digest) {
            return Err(mismatch("raw_context_digest"));
        }
        if expected_evaluator_revision.is_some_and(|r| r != self.evaluator_revision) {
            return Err(mismatch("evaluator_revision"));
        }
        for verdict in self.verdicts.iter() {
            if !is_sha256_hex(&verdict.raw_answer_digest)
                || !is_sha256_hex(&verdict.candidate_answer_digest)
            {
                return Err(mismatch("answer_digest"));
            }
        }
        Ok(())
    }

    /// Replace only when every expected query has exactly one verdict and
    /// `mean(candidate) + tolerance >= mean(raw)`, in integer micro units.
    pub fn decide(
        &self,
        tolerance_micro: u64,
        expected_raw_context_digest: Option<&str>,
        expected_evaluator_revision: Option<&str>,
    ) -> Result<ReplacementDecision, ReplacementRefusal> {
        let observed = self.verdicts.len();
        let ids: BTreeSet<&str> = self.verdicts.iter().map(|v| v.query_id.as_str()).collect();
        if self.expected_queries == 0
            || observed != self.expected_queries as usize
            || ids.len() != observed
        {
            return Err(ReplacementRefusal::VerdictsIncomplete {
                expected: self.expected_queries,
                observed,
            });
        }
        self.check_digests(expected_raw_context_digest, expected_evaluator_revision)?;
        if self.metric_id.is_empty() {
            return Err(ReplacementRefusal::MetricUndefined {
                reason: "the metric is not named".to_string(),
            });
        }
        if self.verdicts.iter().any(|v| {
            v.raw_score_micro > SCORE_ONE_MICRO || v.candidate_score_micro > SCORE_ONE_MICRO
        }) {
            return Err(ReplacementRefusal::MetricUndefined {
                reason: "a score lies outside [0, 1]".to_string(),
            });
        }
        let count = observed as u64;
        let raw_mean_micro = self.verdicts.iter().map(|v| v.raw_score_micro).sum::<u64>() / count;
        let candidate_mean_micro = self
            .verdicts
            .iter()
            .map(|v| v.candidate_score_micro)
            .sum::<u64>()
            / count;
        let verdict = if candidate_mean_micro.saturating_add(tolerance_micro) >= raw_mean_micro {
            ReplacementVerdict::Replace
        } else {
            ReplacementVerdict::RetainRaw
        };
        Ok(ReplacementDecision {
            verdict,
            raw_mean_micro,
            candidate_mean_micro,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const D2: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const D3: &str = "3333333333333333333333333333333333333333333333333333333333333333";

    fn verdict(id: &str, raw: u64, candidate: u64) -> QueryVerdict {
        QueryVerdict {
            query_id: id.to_string(),
            raw_answer_digest: D1.to_string(),
            candidate_answer_digest: D2.to_string(),
            raw_score_micro: raw,
            candidate_score_micro: candidate,
        }
    }

    fn evaluation(verdicts: Vec<QueryVerdict>, expected: u32) -> PairedContextEvaluation {
        PairedContextEvaluation {
            held_out_set_id: "held-out".to_string(),
            held_out_set_revision: 3,
            held_out_set_digest: D1.to_string(),
            expected_queries: expected,
            raw_context_digest: D2.to_string(),
            candidate_context_digest: D3.to_string(),
            evaluator_revision: "eval-1".to_string(),
            metric_id: "exact-match".to_string(),
            verdicts: BoundedVec::new(verdicts).unwrap(),
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R137.14
    #[test]
    fn the_gate_follows_the_integer_mean_and_tolerance() {
        let within = evaluation(
            vec![
                verdict("a", 900_000, 890_000),
                verdict("b", 800_000, 790_000),
            ],
            2,
        );
        let decided = within.decide(20_000, None, None).unwrap();
        assert_eq!(decided.verdict, ReplacementVerdict::Replace);
        assert_eq!(
            (decided.raw_mean_micro, decided.candidate_mean_micro),
            (850_000, 840_000)
        );
        let beyond = evaluation(
            vec![
                verdict("a", 900_000, 800_000),
                verdict("b", 800_000, 700_000),
            ],
            2,
        );
        assert_eq!(
            beyond.decide(20_000, None, None).unwrap().verdict,
            ReplacementVerdict::RetainRaw
        );
        // Exactly at the tolerance replaces.
        let edge = evaluation(vec![verdict("a", 900_000, 880_000)], 1);
        assert_eq!(
            edge.decide(20_000, None, None).unwrap().verdict,
            ReplacementVerdict::Replace
        );
        assert_eq!(
            edge.decide(19_999, None, None).unwrap().verdict,
            ReplacementVerdict::RetainRaw
        );
    }

    // spec: MEDULLA-DECISION-ENGINE-R137.14
    #[test]
    fn every_failure_is_a_registered_refusal() {
        let code =
            |e: Result<ReplacementDecision, ReplacementRefusal>| e.unwrap_err().code().as_str();
        let missing = evaluation(vec![verdict("a", 1, 1)], 2);
        assert_eq!(
            code(missing.decide(0, None, None)),
            "PAIRED_VERDICTS_INCOMPLETE"
        );
        let duplicate = evaluation(vec![verdict("a", 1, 1), verdict("a", 1, 1)], 2);
        assert_eq!(
            code(duplicate.decide(0, None, None)),
            "PAIRED_VERDICTS_INCOMPLETE"
        );
        let ok = evaluation(vec![verdict("a", 1, 1)], 1);
        assert_eq!(code(ok.decide(0, Some(D1), None)), "PAIRED_DIGEST_MISMATCH");
        assert_eq!(
            code(ok.decide(0, None, Some("eval-0"))),
            "PAIRED_DIGEST_MISMATCH"
        );
        let mut bad_digest = ok.clone();
        bad_digest.held_out_set_digest = "x".to_string();
        assert_eq!(
            code(bad_digest.decide(0, None, None)),
            "PAIRED_DIGEST_MISMATCH"
        );
        let mut no_metric = ok.clone();
        no_metric.metric_id.clear();
        assert_eq!(
            code(no_metric.decide(0, None, None)),
            "PAIRED_METRIC_UNDEFINED"
        );
        let high = evaluation(vec![verdict("a", 1, SCORE_ONE_MICRO + 1)], 1);
        assert_eq!(code(high.decide(0, None, None)), "PAIRED_METRIC_UNDEFINED");
        for refusal in REFUSALS {
            assert!(refusal.as_str().starts_with("PAIRED_"));
        }
    }

    // spec: MEDULLA-DECISION-ENGINE-R137.14
    #[test]
    fn records_round_trip_and_reject_unknown_fields() {
        let eval = evaluation(vec![verdict("a", 1, 2)], 1);
        let json = serde_json::to_string(&eval).unwrap();
        assert_eq!(
            serde_json::from_str::<PairedContextEvaluation>(&json).unwrap(),
            eval
        );
        let with_extra = json.replacen('{', "{\"extra\":1,", 1);
        assert!(serde_json::from_str::<PairedContextEvaluation>(&with_extra).is_err());
    }
}
