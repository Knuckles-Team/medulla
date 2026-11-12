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
                    .map(|id| DeclaredOption {
                        option_id: id.into(),
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
        params: BoundedVec::new(params).unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

/// The kinds without a specialized body: one shared matrix ladder, reached
/// through the registry.
const MATRIX_KINDS: &[QuestionKind] = &[
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
    QuestionKind::Placement,
    QuestionKind::ContextBudget,
    QuestionKind::RoundVerdict,
    QuestionKind::StageAllocation,
    QuestionKind::ProbeVerdict,
    QuestionKind::ItemRecommendation,
    QuestionKind::LoadSet,
    QuestionKind::UsageGuidance,
    QuestionKind::BranchOutcome,
    QuestionKind::EditConcentration,
    QuestionKind::ThoughtSearch,
    QuestionKind::ComparativeAnalysisPlan,
    QuestionKind::ContextPackage,
];

fn task_request(pin: &ComponentDependency) -> DecideRequest {
    let tasks: Vec<TaskPlanTask> = [("a", Vec::new()), ("b", vec!["a".to_string()])]
        .into_iter()
        .map(|(id, deps)| TaskPlanTask {
            task_id: id.into(),
            prompt: format!("Analyze {id} with evidence"),
            depends_on: BoundedVec::new(deps).unwrap(),
            call: None,
            aggregation: None,
            assignee: None,
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

    // The task plan family concludes a typed plan through the registry.
    let batch = decide(&h, task_request(&pin)).await.unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::TaskPlan);
    assert!(record.task_plan.is_some());
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
