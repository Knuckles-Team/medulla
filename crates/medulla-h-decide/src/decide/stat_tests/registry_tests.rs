//! The question-family registry serves every kind
//! (MEDULLA-DECISION-ENGINE-R001.101): matrix kinds rank through the shared
//! ladder their family resolves to, the specialized kinds conclude through
//! their own, and the declared-option gates still refuse what they refused
//! before the registry.

use super::*;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::{
    decision::task_plan::TaskPlanProposal, decision::task_plan::TaskPlanTask,
};

fn declared_request(
    kind: QuestionKind,
    pin: &ComponentDependency,
    utterance: Option<&str>,
) -> DecideRequest {
    let mut params = vec![
        TypedParam {
            name: "needs".to_string(),
            value: TypedValue::IriList(
                BoundedVec::new(vec!["eg:capability/retrieval".to_string()]).unwrap(),
            ),
        },
        TypedParam {
            name: "query".to_string(),
            value: TypedValue::Text("web search engine".to_string()),
        },
    ];
    if let Some(utterance) = utterance {
        params.push(TypedParam {
            name: "utterance".to_string(),
            value: TypedValue::Text(utterance.to_string()),
        });
    }
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: "route.tools".to_string(),
            kind,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(
                ["a", "b"]
                    .into_iter()
                    .map(|id| {
                        // A resolve_entity option is one entity pair
                        // (MEDULLA-DECISION-ENGINE-R032.2.2): it declares both
                        // sides as IRIs.
                        let texts = if kind == QuestionKind::ResolveEntity {
                            pair_texts(id)
                        } else {
                            Vec::new()
                        };
                        DeclaredOption {
                            option_id: id.into(),
                            classification: BoundedVec::default(),
                            numbers: BoundedVec::default(),
                            texts: BoundedVec::new(texts).unwrap(),
                        }
                    })
                    .collect(),
            )
            .unwrap(),
        },
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(params).unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

/// The kinds without a specialized body: one shared matrix ladder, reached
/// through the registry. `ThoughtSearch`, `RewardDecomposition`,
/// `PopulationDrift`, `FanoutWidth`, `TrajectoryJoin`, `StepVerification` and
/// `AdversarialAdmission` conclude through their own families
/// (`thought_search_tests`, `branch_outcomes_tests`, `reward_decomposition_tests`, `population_tests`,
/// `trajectory_join_tests`, `step_verification_tests`).
const MATRIX_KINDS: &[QuestionKind] = &[
    QuestionKind::CurriculumPromotion,
    QuestionKind::AutonomyVerdict,
    QuestionKind::Route,
    QuestionKind::Rank,
    QuestionKind::Classify,
    QuestionKind::ResolveEntity,
    QuestionKind::SchemaMapping,
    QuestionKind::TemplateChoice,
    QuestionKind::PreToolRisk,
    QuestionKind::ConnectorEventTriage,
    QuestionKind::ConnectorToolChoice,
    QuestionKind::ConnectorWriteBack,
    QuestionKind::ItemRecommendation,
];

/// Kinds that left the shared ladder for a family of their own (placement
/// R046, probe verdict R138.3, load set, usage guidance, edit concentration
/// R173.3, comparative analysis plan): each concludes from its own typed
/// parameters, covered by its own tests. The registry must still route a
/// matrix-shaped request to that family, which seals the kind or refuses
/// with a typed code, never an unclassified one.
const SPECIALIZED_KINDS: &[QuestionKind] = &[
    QuestionKind::Placement,
    QuestionKind::ProbeVerdict,
    QuestionKind::LoadSet,
    QuestionKind::UsageGuidance,
    QuestionKind::EditConcentration,
    QuestionKind::ComparativeAnalysisPlan,
];

pub(super) fn task_request(pin: &ComponentDependency) -> DecideRequest {
    let tasks: Vec<TaskPlanTask> = [("a", Vec::new()), ("b", vec!["a".to_string()])]
        .into_iter()
        .map(|(id, deps)| TaskPlanTask {
            task_id: id.into(),
            prompt: format!("Analyze {id} with evidence"),
            depends_on: BoundedVec::new(deps).unwrap(),
            call: None,
            aggregation: None,
            assignee: None,
            branch: None,
            join: None,
            decision: None,
            approval: None,
            duration_ms: None,
            priority_weight: None,
        })
        .collect();
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: "registry-explicit-plan".to_string(),
            kind: QuestionKind::TaskPlan,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(
                tasks
                    .iter()
                    .map(|t| DeclaredOption {
                        option_id: t.task_id.clone(),
                        classification: BoundedVec::default(),
                        numbers: BoundedVec::default(),
                        texts: BoundedVec::default(),
                    })
                    .collect(),
            )
            .unwrap(),
        },
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![TypedParam {
            name: "task_plan".into(),
            value: TypedValue::TaskPlan(Box::new(TaskPlanProposal {
                tasks: BoundedVec::new(tasks).unwrap(),
                output_task_id: "b".into(),
                max_concurrency: 1,
                max_steps: 2,
                final_artifact: None,
            })),
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R001.101
// spec: MEDULLA-DECISION-ENGINE-R001.101.2
#[tokio::test]
async fn decide_dispatches_every_kind_through_its_family() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);

    // Every matrix kind resolves through the registry to the shared ladder
    // and abstains without a head; the sealed record carries the kind asked.
    // `TemplateChoice` additionally names its required `utterance` param.
    for kind in MATRIX_KINDS.iter().copied() {
        let utterance = (kind == QuestionKind::TemplateChoice).then_some("Summarize the result");
        let batch = decide(&h, declared_request(kind, &pin, utterance))
            .await
            .unwrap_or_else(|detail| panic!("kind {kind:?}: {detail}"));
        let record = &batch.records.as_slice()[0];
        assert_eq!(
            record.question.kind, kind,
            "the registry must dispatch {kind:?} to its family"
        );
        assert!(
            matches!(record.outcome, StatisticalOutcome::Abstained { .. }),
            "{kind:?} must reach the shared ladder and abstain without a head"
        );
    }

    for kind in SPECIALIZED_KINDS.iter().copied() {
        match decide(&h, declared_request(kind, &pin, None)).await {
            Ok(batch) => assert_eq!(batch.records.as_slice()[0].question.kind, kind),
            Err(detail) => assert!(
                !detail.starts_with("INTERNAL"),
                "{kind:?} must refuse with a typed code: {detail}"
            ),
        }
    }

    // The task plan family concludes a typed plan through the registry.
    let batch = decide(&h, task_request(&pin)).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::TaskPlan);
    assert!(record.answer.as_ref().and_then(|a| a.task_plan()).is_some());
    assert_eq!(
        record.resolution_kind,
        medulla_decision_types::decision::ResolutionKind::Constraint
    );

    // The declared-option gate runs through the family's validate_candidates
    // hook: an unregistered ingestion lane is still refused.
    let mut request = declared_request(QuestionKind::IngestionLane, &pin, None);
    request.candidates = CandidateSource::Declared {
        options: BoundedVec::new(vec![DeclaredOption {
            option_id: "not-a-lane".into(),
            classification: BoundedVec::default(),
            numbers: BoundedVec::default(),
            texts: BoundedVec::default(),
        }])
        .unwrap(),
    };
    let response =
        super::super::statistical::handle_decide(&h.state, 1, &verified(), request).await;
    assert_eq!(
        response.error.as_deref(),
        Some("UNSUPPORTED_INGESTION_LANE")
    );
}

// spec: MEDULLA-DECISION-ENGINE-R001.101.3
#[tokio::test]
async fn assemble_and_decide_text_use_the_registry() {
    use super::super::family::for_kind;
    // The utterance binding is a family hook, not a private per-kind branch:
    // exactly the template-choice family binds.
    for kind in MATRIX_KINDS.iter().chain(SPECIALIZED_KINDS).copied() {
        assert_eq!(
            for_kind(kind).binds_utterance(),
            kind == QuestionKind::TemplateChoice,
            "{kind:?}"
        );
    }
    // A template-choice decide still reaches its family and seals the kind.
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let bound = decide(
        &h,
        declared_request(
            QuestionKind::TemplateChoice,
            &pin,
            Some("Summarize the result"),
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        bound.records.as_slice()[0].question.kind,
        QuestionKind::TemplateChoice
    );
}

fn text_option(id: &str, name: &str) -> DeclaredOption {
    use medulla_decision_types::decision::statistical::declared::DeclaredText;
    DeclaredOption {
        option_id: id.into(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::new(vec![DeclaredText {
            key: "name".to_string(),
            text: name.to_string(),
        }])
        .unwrap(),
    }
}

/// The two IRI sides of a `resolve_entity` pair, in key order.
fn pair_texts(
    id: &str,
) -> Vec<medulla_decision_types::decision::statistical::declared::DeclaredText> {
    use medulla_decision_types::decision::statistical::declared::DeclaredText;
    vec![
        DeclaredText {
            key: "entity.left".to_string(),
            text: format!("urn:entity:{id}"),
        },
        DeclaredText {
            key: "entity.right".to_string(),
            text: format!("urn:entity:{id}-other"),
        },
    ]
}

/// A `resolve_entity` pair option: a name plus the two IRIs of the pair.
fn pair_text_option(id: &str, name: &str) -> DeclaredOption {
    use medulla_decision_types::decision::statistical::declared::DeclaredText;
    let mut option = text_option(id, name);
    let mut texts: Vec<DeclaredText> = option.texts.iter().cloned().collect();
    texts.extend(pair_texts(id));
    // Declared fact keys are strictly sorted (check_declared).
    texts.sort_by(|a, b| a.key.cmp(&b.key));
    option.texts = BoundedVec::new(texts).unwrap();
    option
}

// spec: MEDULLA-UNIFIED-DATA-PLANE-R030.701
#[tokio::test]
async fn resolve_entity_records_identity_premises_and_refuses_unapproved_links() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    let mut request = declared_request(QuestionKind::ResolveEntity, &pin, None);
    request.candidates = CandidateSource::Declared {
        options: BoundedVec::new(vec![
            pair_text_option("a", "x7Qp9ZkL2mWv"),
            pair_text_option("b", "x7Qp9ZkL2mWv"),
        ])
        .unwrap(),
    };
    let batch = decide(&h, request.clone()).await.unwrap();
    let facts: Vec<&str> = batch.records.as_slice()[0]
        .premises
        .iter()
        .map(|premise| premise.fact.as_str())
        .collect();
    assert!(facts.contains(&"identity:entropy_qualified_token"));
    assert!(facts.contains(&"identity:decision:no_merge"));
    assert!(!facts.contains(&"identity:normalized_name"));

    let mut approving = request;
    approving.params = BoundedVec::new(vec![TypedParam {
        name: "approve_link".to_string(),
        value: TypedValue::Bool(true),
    }])
    .unwrap();
    let refused = decide(&h, approving).await.unwrap_err();
    assert!(refused.contains("LINK_APPROVER_MISSING"), "{refused}");
}
