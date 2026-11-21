//! Valid sample values for the statistical decision surface.

use crate::decision::{
    CalibrationMethod, CalibrationStatement, CandidatePool, CandidateSource, DecideRequest,
    DecisionEvalOp, DecisionEvalRequest, DecisionFitOp, DecisionFitRequest,
    DecisionJobStatusRequest, DecisionPolicyRef, EvalCandidate, ExplorationRecord,
    FeatureMatrixRef, HeadKind, LabelRegime, LibraryCandidateScope, OpeEstimatorKind,
    OptimiserSpec, QuantScaleTag, QuantisedValue, QuestionKind, QuestionSafety, RecordWindow,
    RiskMethod, RiskStatement, ScoredOption, ShortlistProvenance, StatisticalOutcome,
    StatisticalQuestion, TypedParam, TypedValue,
};
use medulla_agent_types::agent_component::AgentComponentKind;

use crate::decision::jobs::DatasetSource;
use crate::decision::replay::{
    AllocationRule, EvalMode, RefitSpec, ReplayEnvironment, ReplaySpec, SharedCap, TrialLog,
    WalkForward,
};
use crate::decision::statistical::dataset::{
    ItemLabel, LabelSource, LabelledDataset, LabelledItem, LoggedOutcome, OutcomeEvaluation,
    OutcomeFidelity, PropensitySource, LABELLED_DATASET_SCHEMA_VERSION,
};
use crate::decision::EvidenceClass;

use super::contract_decision::{dependency, every_abstain_reason, unit_rational};
use medulla_types::{test_support::wave::bounded, test_support::wave::digest_text};

/// The question every statistical sample answers.
pub fn question() -> StatisticalQuestion {
    StatisticalQuestion {
        question_id: "route.ingestion".to_string(),
        kind: QuestionKind::Route,
        safety: QuestionSafety::Ordinary,
    }
}

/// Every typed parameter shape.
pub fn every_typed_value() -> Vec<TypedValue> {
    vec![
        TypedValue::Bool(true),
        TypedValue::Int(-7),
        TypedValue::Text("free text".to_string()),
        TypedValue::Iri("eg:capability/retrieval".to_string()),
        TypedValue::IriList(bounded(vec!["eg:modality/text".to_string()])),
        TypedValue::Rational(unit_rational(1, 4)),
    ]
}

/// One evaluate-only statistical question over library candidates.
pub fn decide_request() -> DecideRequest {
    DecideRequest {
        tenant_id: "tenant-a".to_string(),
        question: question(),
        candidates: CandidateSource::AgentLibrary {
            scope: LibraryCandidateScope {
                kinds: bounded(vec![AgentComponentKind::Tool]),
                classification_under: None,
            },
        },
        feature_schema: dependency("feature-schema-a", AgentComponentKind::FeatureSchema),
        head: Some(dependency("head-a", AgentComponentKind::DecisionHead)),
        policy: DecisionPolicyRef::Default,
        params: bounded(
            every_typed_value()
                .into_iter()
                .enumerate()
                .map(|(index, value)| TypedParam {
                    name: format!("param-{index}"),
                    value,
                })
                .collect(),
        ),
        max_records: Some(16),
        belief_as_of: bounded(vec![1_600_000_000_000, 1_650_000_000_000]),
        evidence_streams: Default::default(),
    }
}

/// Both ways a feature matrix is pinned.
pub fn every_feature_matrix() -> Vec<FeatureMatrixRef> {
    vec![
        FeatureMatrixRef::Inline {
            candidate_ids: bounded(vec!["option-a".to_string(), "option-b".to_string()]),
            feature_names: bounded(vec!["recency".to_string()]),
            scale: QuantScaleTag::Pico,
            values: bounded(vec![1_000_000_000_000, -500_000_000_000]),
        },
        FeatureMatrixRef::Blob {
            sha256: digest_text(0xa1),
            length: 2_048,
            rows: 2,
            columns: 1,
        },
    ]
}

/// The shortlist provenance every statistical record carries.
pub fn shortlist() -> ShortlistProvenance {
    ShortlistProvenance {
        ann_recall_mode: Some("exact".to_string()),
        now_ms: 1_700_000_000_000,
        embedder_model_digest: Some(digest_text(0xa2)),
    }
}

/// A committed-then-revealed exploration draw.
pub fn exploration() -> ExplorationRecord {
    ExplorationRecord {
        seed_commitment: digest_text(0xa3),
        revealed_seed: Some(digest_text(0xa4)),
        budget_digest: digest_text(0xa5),
        pool: Some(CandidatePool {
            context_digest: digest_text(0xa6),
            exploitation: vec!["option-a".to_string()],
            exploration: vec!["option-b".to_string()],
            matched_records: vec!["decision:a".to_string()],
        }),
    }
}

/// Every conclusion a statistical decision can reach.
pub fn every_statistical_outcome() -> Vec<StatisticalOutcome> {
    vec![
        StatisticalOutcome::Acted {
            option_id: "option-a".to_string(),
            propensity: unit_rational(3, 4),
            prediction_set: bounded(vec!["option-a".to_string(), "option-b".to_string()]),
            risk: RiskStatement {
                method: RiskMethod::LearnThenTest,
                epsilon: unit_rational(1, 20),
                delta: unit_rational(1, 100),
                n_calibration: 4_096,
            },
        },
        StatisticalOutcome::Explored {
            option_id: "option-b".to_string(),
            propensity: unit_rational(1, 8),
        },
        StatisticalOutcome::Advisory {
            scores: bounded(vec![ScoredOption {
                option_id: "option-b".to_string(),
                score: QuantisedValue {
                    scale: QuantScaleTag::Q32,
                    value: 1_073_741_824,
                },
                probability: Some(unit_rational(1, 2)),
            }]),
            calibrated: false,
            ranked_total: 1,
        },
        StatisticalOutcome::Abstained {
            reasons: bounded(every_abstain_reason()),
        },
    ]
}

/// Every calibration statement shape.
pub fn every_calibration() -> Vec<CalibrationStatement> {
    [
        CalibrationMethod::Conformal,
        CalibrationMethod::Temperature,
        CalibrationMethod::Vector,
        CalibrationMethod::Dirichlet,
        CalibrationMethod::Isotonic,
        CalibrationMethod::None,
    ]
    .into_iter()
    .map(|method| CalibrationStatement {
        method,
        alpha: Some(unit_rational(1, 10)),
        coverage_lower: Some(unit_rational(8, 10)),
        coverage_upper: Some(unit_rational(9, 10)),
        n_calibration: 1_024,
        synthetic: true,
    })
    .collect()
}

fn window() -> RecordWindow {
    RecordWindow {
        from_ms: 1_600_000_000_000,
        to_ms: 1_700_000_000_000,
    }
}

/// Both fit operations.
pub fn fit_ops() -> Vec<(&'static str, DecisionFitOp)> {
    vec![
        (
            "DecisionFit.submit",
            DecisionFitOp::Submit {
                request: Box::new(DecisionFitRequest {
                    tenant_id: "tenant-a".to_string(),
                    idempotency_key: "fit-1".to_string(),
                    head_kind: HeadKind::ListwiseLogistic,
                    feature_schema: dependency(
                        "feature-schema-a",
                        AgentComponentKind::FeatureSchema,
                    ),
                    policy: DecisionPolicyRef::Default,
                    label_regime: LabelRegime::FullLabel {
                        gold_set_digest: digest_text(0xb1),
                    },
                    window: window(),
                    source: DatasetSource::Inline {
                        dataset: Box::new(dataset()),
                    },
                    penalty: None,
                    features: Default::default(),
                    parent_head: None,
                    calibration_method: None,
                    optimiser: OptimiserSpec {
                        max_iterations: 500,
                        tolerance: QuantisedValue {
                            scale: QuantScaleTag::Pico,
                            value: 1_000_000,
                        },
                        seed: 42,
                    },
                }),
            },
        ),
        (
            "DecisionFit.status",
            DecisionFitOp::Status {
                request: DecisionJobStatusRequest {
                    tenant_id: "tenant-a".to_string(),
                    job_id: "fit-job-1".to_string(),
                },
            },
        ),
    ]
}

/// Both evaluation operations.
pub fn eval_ops() -> Vec<(&'static str, DecisionEvalOp)> {
    vec![
        (
            "DecisionEval.submit",
            DecisionEvalOp::Submit {
                request: Box::new(DecisionEvalRequest {
                    tenant_id: "tenant-a".to_string(),
                    idempotency_key: "eval-1".to_string(),
                    candidate: EvalCandidate::DraftArtifact {
                        sha256: digest_text(0xb2),
                        length: 4_096,
                    },
                    policy: DecisionPolicyRef::Default,
                    estimators: bounded(vec![
                        OpeEstimatorKind::Ips,
                        OpeEstimatorKind::ClippedIps,
                        OpeEstimatorKind::Snips,
                        OpeEstimatorKind::Switch,
                        OpeEstimatorKind::DoublyRobust,
                    ]),
                    gold_set_digest: Some(digest_text(0xb3)),
                    window: window(),
                    source: DatasetSource::Logged {
                        question_id: "route.ingestion".to_string(),
                    },
                    mode: EvalMode::OffPolicy,
                    gold_guarantee: None,
                }),
            },
        ),
        (
            "DecisionEval.submit:replay",
            DecisionEvalOp::Submit {
                request: Box::new(replay_request()),
            },
        ),
        (
            "DecisionEval.status",
            DecisionEvalOp::Status {
                request: DecisionJobStatusRequest {
                    tenant_id: "tenant-a".to_string(),
                    job_id: "eval-job-1".to_string(),
                },
            },
        ),
        (
            "DecisionEval.retrieval_ranking_submit",
            DecisionEvalOp::RetrievalRankingSubmit {
                request: Box::new(ranking_request()),
            },
        ),
        (
            "DecisionEval.grounded_answer_submit",
            DecisionEvalOp::GroundedAnswerSubmit {
                request: Box::new(grounded_request()),
            },
        ),
        (
            "DecisionEval.certify_local_skill",
            DecisionEvalOp::CertifyLocalSkill {
                request: Box::new(certification_request()),
            },
        ),
    ]
}

fn pinned_set(id: &str, byte: u8) -> crate::decision::statistical::ranking_eval::PinnedSet {
    crate::decision::statistical::ranking_eval::PinnedSet {
        id: id.to_string(),
        revision: 1,
        digest: digest_text(byte),
    }
}

/// An atomic local-skill certification submission.
fn certification_request() -> crate::decision::local_certification::LocalCertificationRequest {
    use crate::decision::local_certification::{
        CriterionResult, LocalCertificationRequest, LocalSkillPins, RunReceipt,
        SkillClassification, LOCAL_CERTIFICATION_SCHEMA_VERSION,
    };
    use medulla_agent_types::agent_component::{AgentComponentKind, ComponentDependency};
    let pin = |id: &str, kind: AgentComponentKind, byte: u8| ComponentDependency {
        component_id: id.to_string(),
        kind,
        definition_digest: digest_text(byte),
    };
    let skill = pin("skill-a", AgentComponentKind::Tool, 0xd1);
    let pins = LocalSkillPins {
        model: pin("model-a", AgentComponentKind::ModelProfile, 0xd2),
        dataset: pinned_set("dataset", 0xd3),
        criteria_policy: pin("criteria-a", AgentComponentKind::DecisionPolicy, 0xd4),
    };
    LocalCertificationRequest {
        schema_version: LOCAL_CERTIFICATION_SCHEMA_VERSION,
        tenant_id: "tenant-a".to_string(),
        idempotency_key: "certify-1".to_string(),
        skill: skill.clone(),
        classification: SkillClassification::Atomic,
        dependencies: Vec::new(),
        pins: pins.clone(),
        criterion_ids: vec!["accuracy".to_string()],
        receipt: RunReceipt {
            run_id: "run-1".to_string(),
            skill,
            pins,
            results: vec![CriterionResult {
                criterion_id: "accuracy".to_string(),
                passed: true,
                evidence_digest: "e".repeat(64),
            }],
            completion_revision: 1,
        },
    }
}

/// A two-algorithm ranking evaluation submission.
fn ranking_request() -> crate::decision::statistical::ranking_eval::RetrievalRankingRequest {
    use crate::decision::statistical::ranking_eval::{
        AlgorithmRun, LabelPair, LabelSet, QueryRanking, QuerySet, RetrievalRankingRequest,
        RANKING_EVAL_SCHEMA_VERSION,
    };
    let run = |id: &str| AlgorithmRun {
        algorithm_id: id.to_string(),
        version: "1".to_string(),
        settings_digest: digest_text(0xc1),
        corpus_digest: digest_text(0xc2),
        input_digest: digest_text(0xc3),
        rankings: vec![QueryRanking {
            query_id: "q1".to_string(),
            ranked_ids: vec!["d1".to_string()],
        }],
    };
    RetrievalRankingRequest {
        schema_version: RANKING_EVAL_SCHEMA_VERSION,
        tenant_id: "tenant-a".to_string(),
        idempotency_key: "ranking-1".to_string(),
        corpus: pinned_set("corpus", 0xc2),
        labels: LabelSet {
            pinned: pinned_set("labels", 0xc4),
            pairs: vec![LabelPair {
                query_id: "q1".to_string(),
                doc_id: "d1".to_string(),
            }],
        },
        queries: QuerySet {
            pinned: pinned_set("queries", 0xc5),
            query_ids: vec!["q1".to_string()],
        },
        k: 10,
        evaluator_revision: 1,
        algorithms: vec![run("bm25"), run("dense")],
    }
}

/// A grounded evaluation submission over one query, topic and claim.
fn grounded_request() -> crate::decision::statistical::grounded_eval::GroundedAnswerRequest {
    use crate::decision::statistical::grounded_eval::{
        CitationSupport, ClaimSupport, ClaimVotes, ExpectedTopicSet, GroundedAnswerRequest,
        GroundedClaimPrediction, GroundedScope, GroundedScorerRecord, TopicSupport,
        GROUNDED_EVAL_SCHEMA_VERSION,
    };
    GroundedAnswerRequest {
        schema_version: GROUNDED_EVAL_SCHEMA_VERSION,
        tenant_id: "tenant-a".to_string(),
        idempotency_key: "grounded-1".to_string(),
        corpus: pinned_set("corpus", 0xd1),
        queries: pinned_set("queries", 0xd2),
        answers: pinned_set("answers", 0xd3),
        claims: pinned_set("claims", 0xd4),
        citations: pinned_set("citations", 0xd5),
        labels: pinned_set("labels", 0xd6),
        scope: GroundedScope::SingleDocument,
        evaluator_revision: 1,
        policy: DecisionPolicyRef::Default,
        expected: vec![ExpectedTopicSet {
            query_id: "q1".to_string(),
            topic_ids: vec!["t1".to_string()],
        }],
        support: vec![TopicSupport {
            query_id: "q1".to_string(),
            topic_id: "t1".to_string(),
            claims: vec![ClaimSupport {
                claim_id: "c1".to_string(),
                citations: vec![CitationSupport {
                    citation_id: "s1".to_string(),
                    supporting: 3,
                    total: 3,
                }],
            }],
        }],
        claim_ids: vec!["c1".to_string()],
        predictions: vec![GroundedClaimPrediction {
            claim_id: "c1".to_string(),
            answer_digest: digest_text(0xd7),
            input_digest: digest_text(0xd8),
            scorer_id: "detector".to_string(),
            scorer_revision: 1,
            scorer_artifact_digest: digest_text(0xd9),
            p_ppm: 100_000,
        }],
        votes: vec![ClaimVotes {
            claim_id: "c1".to_string(),
            annotators: vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
            unsupported_votes: 1,
            supported_votes: 2,
        }],
        scorer: GroundedScorerRecord {
            scorer_id: "detector".to_string(),
            scorer_revision: 1,
            scorer_artifact_digest: digest_text(0xd9),
            inputs_read: vec![digest_text(0xda)],
        },
    }
}

/// A walk-forward replay submission over an inline gold set (EH-528).
fn replay_request() -> DecisionEvalRequest {
    let q32 = |value: i64| QuantisedValue {
        scale: QuantScaleTag::Q32,
        value,
    };
    DecisionEvalRequest {
        tenant_id: "tenant-a".to_string(),
        idempotency_key: "replay-1".to_string(),
        candidate: EvalCandidate::DraftArtifact {
            sha256: digest_text(0xb4),
            length: 4_096,
        },
        policy: DecisionPolicyRef::Default,
        estimators: bounded(Vec::new()),
        gold_set_digest: Some(digest_text(0xb5)),
        window: window(),
        source: DatasetSource::Logged {
            question_id: "route.ingestion".to_string(),
        },
        mode: EvalMode::Replay {
            spec: Box::new(ReplaySpec {
                folds: WalkForward {
                    train: 252,
                    test: 21,
                    step: 21,
                    purge: 5,
                    embargo: 5,
                },
                budget: SharedCap {
                    cap: q32(1 << 32),
                    rule: AllocationRule::Proportional,
                },
                env: ReplayEnvironment::PolicyIndependent,
                refit: Some(RefitSpec {
                    head_kind: HeadKind::ListwiseLogistic,
                    optimiser: OptimiserSpec {
                        max_iterations: 60,
                        tolerance: q32(1 << 12),
                        seed: 7,
                    },
                }),
                trials: TrialLog {
                    declared: 12,
                    searched: 12,
                },
                incumbent: Some(published_head_candidate()),
                supersedes: Some(digest_text(0xb6)),
                events: Default::default(),
                resume_cursor: None,
            }),
        },
        gold_guarantee: None,
    }
}

/// The other evaluation candidate shape, so both arms are exercised.
pub fn published_head_candidate() -> EvalCandidate {
    EvalCandidate::PublishedHead {
        head: dependency("head-a", AgentComponentKind::DecisionHead),
    }
}

fn gold_item() -> LabelledItem {
    LabelledItem {
        item_id: "gold-1".to_string(),
        recorded_at_ms: 1_650_000_000_000,
        class_key: "eg:task/research".to_string(),
        candidate_ids: bounded(vec!["option-a".to_string(), "option-b".to_string()]),
        features: bounded(vec![1_000_000_000_000, 0]),
        label: ItemLabel::Gold {
            acceptable: bounded(vec!["option-a".to_string()]),
            source: LabelSource::SyntheticConstruction,
        },
        audit_inclusion: None,
    }
}

fn logged_item() -> LabelledItem {
    LabelledItem {
        item_id: "logged-1".to_string(),
        label: ItemLabel::Logged(Box::new(LoggedOutcome {
            executed: "option-b".to_string(),
            logging_propensities: bounded(vec![unit_rational(1, 2), unit_rational(1, 2)]),
            propensity_source: PropensitySource::ExecutedPolicy,
            pinned: false,
            commit_principal: "principal-a".to_string(),
            evaluation: OutcomeEvaluation {
                evaluation_id: "evaluation-1".to_string(),
                class: EvidenceClass::Observation,
                producer: "evaluator-a".to_string(),
                selected_agent: "agent-b".to_string(),
                lease_holder: "worker-b".to_string(),
                fidelity: OutcomeFidelity::ToolCalls,
                success: Some(true),
            },
        })),
        audit_inclusion: Some(unit_rational(1, 20)),
        ..gold_item()
    }
}

/// A labelled dataset exercising both label shapes.
pub fn dataset() -> LabelledDataset {
    LabelledDataset {
        schema_version: LABELLED_DATASET_SCHEMA_VERSION,
        feature_schema_digest: digest_text(0xb4),
        feature_names: bounded(vec!["recency".to_string()]),
        scale: QuantScaleTag::Pico,
        items: bounded(vec![gold_item(), logged_item()]),
        synthetic: true,
    }
}

/// Every decision-log operation.
pub fn log_ops() -> Vec<(
    &'static str,
    crate::decision::statistical::log::DecisionLogOp,
)> {
    use crate::decision::statistical::log::{
        DecisionLogOp, DecisionOutcomeEvaluation, OutcomeAggregateRequest,
    };
    let evaluation = DecisionOutcomeEvaluation {
        record_id: "decision:record-a".to_string(),
        evaluation_id: "evaluation-1".to_string(),
        class: EvidenceClass::Observation,
        selected_agent: "agent-b".to_string(),
        lease_holder: "worker-b".to_string(),
        fidelity: OutcomeFidelity::ToolCalls,
        success: Some(true),
        backbone: None,
    };
    vec![
        (
            "DecisionLog.evaluate",
            DecisionLogOp::Evaluate {
                tenant_id: "tenant-a".to_string(),
                evaluation,
            },
        ),
        (
            "DecisionLog.get",
            DecisionLogOp::Get {
                tenant_id: "tenant-a".to_string(),
                record_id: "decision:record-a".to_string(),
            },
        ),
        (
            "DecisionLog.aggregate",
            DecisionLogOp::Aggregate {
                request: OutcomeAggregateRequest {
                    tenant_id: "tenant-a".to_string(),
                    question_id: None,
                    window: window(),
                    attribution: None,
                    transfer: None,
                },
            },
        ),
    ]
}
