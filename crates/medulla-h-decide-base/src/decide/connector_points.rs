//! The decision-point catalog medulla publishes (MEDULLA-DECISION-ENGINE-R829).
//!
//! One typed table. A client names a point by its question id and sends a
//! declared candidate set; it never sends a score, a ranking or a threshold.
//! The entries of MEDULLA-DECISION-ENGINE-R829.1 rank capabilities through
//! `QuestionKind::Rank`; an id that starts with `catalog.` and is not in the
//! table abstains `QUESTION_UNPUBLISHED`.

use {
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::QuestionSafety,
};

/// How often a point's decision records are logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointLog {
    /// Every record is durable.
    Every,
    /// One record in `interval`.
    Sampled { interval: u32 },
}

/// One published decision point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecisionPoint {
    /// The question id a client sends.
    pub question_id: &'static str,
    /// The question kind the point is asked as.
    pub kind: QuestionKind,
    /// The safety class the point is asked under.
    pub safety: QuestionSafety,
    /// How its records are logged.
    pub log: PointLog,
    /// The component id of the feature schema the request must pin.
    pub feature_schema: &'static str,
}

/// The question id prefix of the capability-ranking points.
pub const CATALOG_PREFIX: &str = "catalog.";

/// Every published decision point.
pub const DECISION_POINTS: &[DecisionPoint] = &[
    DecisionPoint {
        question_id: "catalog.skill_rank",
        kind: QuestionKind::Rank,
        safety: QuestionSafety::Ordinary,
        log: PointLog::Sampled { interval: 100 },
        feature_schema: "decide.schema.catalog.skill_rank",
    },
    DecisionPoint {
        question_id: "catalog.operation_rank",
        kind: QuestionKind::Rank,
        safety: QuestionSafety::Ordinary,
        log: PointLog::Sampled { interval: 100 },
        feature_schema: "decide.schema.catalog.operation_rank",
    },
    DecisionPoint {
        question_id: "catalog.catalog_item_rank",
        kind: QuestionKind::Rank,
        safety: QuestionSafety::Ordinary,
        log: PointLog::Sampled { interval: 100 },
        feature_schema: "decide.schema.catalog.catalog_item_rank",
    },
    DecisionPoint {
        question_id: "catalog.intent_action_rank",
        kind: QuestionKind::Rank,
        safety: QuestionSafety::Ordinary,
        log: PointLog::Sampled { interval: 100 },
        feature_schema: "decide.schema.catalog.intent_action_rank",
    },
];

/// The published point named `question_id`, if any.
pub fn point(question_id: &str) -> Option<&'static DecisionPoint> {
    DECISION_POINTS
        .iter()
        .find(|point| point.question_id == question_id)
}
