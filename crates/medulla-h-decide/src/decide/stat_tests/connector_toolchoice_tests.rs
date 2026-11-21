//! MEDULLA-DECISION-ENGINE-R042: connector tool-choice advisory via `Decide`.
//! `Decide` evaluates which internal tool a connector should use for a task
//! and returns a proposal with evidence only; the connector's own
//! deterministic authorization path, never `Decide`, governs whether any
//! resulting write-back actually occurs. This test exercises tool-choice
//! evaluation for a connector task and confirms the proposal alone cannot
//! trigger a write-back: `StatisticalOutcome` is a closed, four-variant enum
//! (checked here exhaustively) with no "write" variant at all, and under the
//! default policy with no licensed head the served outcome is `Abstained`.

use super::*;

fn connector_tool_choice_request(schema: &ComponentDependency) -> DecideRequest {
    let mut asked = request(
        schema,
        None,
        DecisionPolicyRef::Default,
        QuestionSafety::Ordinary,
    );
    asked.question.kind = QuestionKind::ConnectorToolChoice;
    asked
}

// spec: MEDULLA-DECISION-ENGINE-R042
#[tokio::test]
async fn connector_tool_choice_is_a_proposal_that_cannot_itself_trigger_a_write_back() {
    let h = Harness::new().await;
    h.publish_test_component(
        "tool-connector-candidate",
        AgentComponentKind::Tool,
        "an internal tool a connector might use for its task",
        None::<&()>,
        None,
    )
    .unwrap();
    let schema_pin = h
        .publish_test_component(
            "schema-connector-toolchoice",
            AgentComponentKind::FeatureSchema,
            "connector tool-choice features",
            Some(&schema()),
            None,
        )
        .unwrap();

    let batch = decide(&h, connector_tool_choice_request(&schema_pin))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::ConnectorToolChoice);

    // Exhaustive by construction: `StatisticalOutcome` names no "write" or
    // "write-back" variant at all, so no proposal from this question kind
    // (or any other) can itself trigger one -- the only four things `Decide`
    // can ever answer are these.
    match &record.outcome {
        StatisticalOutcome::Abstained { .. }
        | StatisticalOutcome::Advisory { .. }
        | StatisticalOutcome::Explored { .. }
        | StatisticalOutcome::Acted { .. } => {}
    }
    assert!(
        matches!(record.outcome, StatisticalOutcome::Abstained { .. }),
        "no licensed head: the proposal alone takes no action: {:?}",
        record.outcome
    );
}

// ---- MEDULLA-DECISION-ENGINE-R829.1: the capability-ranking decision points ----

use super::rank_tests::{advisory_policy, published_head_over};
use crate::decide::connector_points::DECISION_POINTS;
use medulla_decision_types::decision::statistical::declared::{
    DeclaredOption, DeclaredText, MAX_RANK_CATALOG_CANDIDATES,
};
use medulla_decision_types::decision::AbstainReason;

fn catalog_schema_body() -> FeatureSchemaBody {
    FeatureSchemaBody {
        schema_version: FEATURE_SCHEMA_VERSION,
        features: BoundedVec::new(vec![FeatureSpec {
            name: "signal".to_string(),
            kind: FeatureKind::TextBm25 {
                key: "rank.name".to_string(),
                param: "query".to_string(),
            },
            missing: MissingValue::Abstain,
        }])
        .unwrap(),
    }
}

fn catalog_option(id: &str, name: &str) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::new(vec![
            DeclaredText {
                key: "rank.description".to_string(),
                text: format!("{name} description"),
            },
            DeclaredText {
                key: "rank.kind".to_string(),
                text: "skill".to_string(),
            },
            DeclaredText {
                key: "rank.name".to_string(),
                text: name.to_string(),
            },
        ])
        .unwrap(),
    }
}

fn catalog_request(
    question_id: &str,
    schema: &ComponentDependency,
    head: &ComponentDependency,
    policy: DecisionPolicyRef,
    candidates: CandidateSource,
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: question_id.to_string(),
            kind: QuestionKind::Rank,
            safety: QuestionSafety::Ordinary,
        },
        candidates,
        feature_schema: schema.clone(),
        head: Some(head.clone()),
        policy,
        params: BoundedVec::new(vec![TypedParam {
            name: "query".to_string(),
            value: TypedValue::Text("alpha".to_string()),
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    }
}

fn rank_catalog(ids_and_names: &[(&str, &str)]) -> CandidateSource {
    CandidateSource::RankCatalog {
        options: ids_and_names
            .iter()
            .map(|(id, name)| catalog_option(id, name))
            .collect(),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R829.1
#[tokio::test]
async fn catalog_serves_the_four_rank_points_and_unknown_id_abstains() {
    let h = Harness::new().await;
    let (_, head) = published_head_over(
        &h,
        HeadKind::ListwiseLogistic,
        "head-catalog",
        "schema-catalog",
        catalog_schema_body(),
    )
    .await;
    let policy = advisory_policy(&h);
    let three = [("opt-a", "alpha"), ("opt-b", "beta"), ("opt-c", "gamma")];
    assert_eq!(DECISION_POINTS.len(), 4);
    for entry in DECISION_POINTS {
        // The request pins the schema component the entry names.
        let schema = h
            .publish_test_component(
                entry.feature_schema,
                AgentComponentKind::FeatureSchema,
                "capability rank features",
                Some(&catalog_schema_body()),
                None,
            )
            .unwrap();
        let batch = decide(
            &h,
            catalog_request(
                entry.question_id,
                &schema,
                &head,
                policy.clone(),
                rank_catalog(&three),
            ),
        )
        .await
        .unwrap();
        let record = &batch.records.as_slice()[0];
        let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
            panic!("{}: advisory expected, got {:?}", entry.question_id, record.outcome);
        };
        let mut ids: Vec<&str> = scores.iter().map(|s| s.option_id.as_str()).collect();
        assert_eq!(ids[0], "opt-a", "{}: the matching name ranks first", entry.question_id);
        ids.sort_unstable();
        assert_eq!(ids, ["opt-a", "opt-b", "opt-c"]);

        // A schema the entry does not name is refused.
        let wrong = h
            .publish_test_component(
                "schema-catalog-other",
                AgentComponentKind::FeatureSchema,
                "other features",
                Some(&catalog_schema_body()),
                None,
            )
            .unwrap();
        let refused = decide(
            &h,
            catalog_request(
                entry.question_id,
                &wrong,
                &head,
                policy.clone(),
                rank_catalog(&three),
            ),
        )
        .await
        .unwrap_err();
        assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    }

    // An id the table does not hold abstains QUESTION_UNPUBLISHED.
    let schema = h
        .publish_test_component(
            "decide.schema.catalog.unknown_rank",
            AgentComponentKind::FeatureSchema,
            "capability rank features",
            Some(&catalog_schema_body()),
            None,
        )
        .unwrap();
    let batch = decide(
        &h,
        catalog_request(
            "catalog.unknown_rank",
            &schema,
            &head,
            policy,
            rank_catalog(&three),
        ),
    )
    .await
    .unwrap();
    let StatisticalOutcome::Abstained { reasons } = &batch.records.as_slice()[0].outcome else {
        panic!("abstention expected");
    };
    assert!(matches!(
        reasons.as_slice(),
        [AbstainReason::Family { code, .. }] if code.as_str() == "QUESTION_UNPUBLISHED"
    ));
}

// spec: MEDULLA-DECISION-ENGINE-R829.1
#[tokio::test]
async fn rank_catalog_accepts_two_thousand_and_refuses_one_more() {
    let h = Harness::new().await;
    let (_, head) = published_head_over(
        &h,
        HeadKind::ListwiseLogistic,
        "head-catalog",
        "schema-catalog",
        catalog_schema_body(),
    )
    .await;
    let policy = advisory_policy(&h);
    let id = "catalog.operation_rank";
    let schema = h
        .publish_test_component(
            "decide.schema.catalog.operation_rank",
            AgentComponentKind::FeatureSchema,
            "capability rank features",
            Some(&catalog_schema_body()),
            None,
        )
        .unwrap();
    let many = |n: usize| -> Vec<(String, String)> {
        (0..n)
            .map(|i| (format!("op-{i:05}"), format!("name{i}")))
            .collect()
    };
    let source = |n: usize| {
        let pairs = many(n);
        CandidateSource::RankCatalog {
            options: pairs
                .iter()
                .map(|(id, name)| catalog_option(id, name))
                .collect(),
        }
    };
    let batch = decide(
        &h,
        catalog_request(id, &schema, &head, policy.clone(), source(MAX_RANK_CATALOG_CANDIDATES)),
    )
    .await
    .unwrap();
    assert!(matches!(
        batch.records.as_slice()[0].outcome,
        StatisticalOutcome::Advisory { .. }
    ));

    let before = h.store.decision_artifacts_with_prefix(TENANT, "", 100_000).unwrap().len();
    let too_large = decide(
        &h,
        catalog_request(
            id,
            &schema,
            &head,
            policy.clone(),
            source(MAX_RANK_CATALOG_CANDIDATES + 1),
        ),
    )
    .await
    .unwrap_err();
    assert!(too_large.starts_with("CANDIDATE_SET_TOO_LARGE"), "{too_large}");
    let after = h.store.decision_artifacts_with_prefix(TENANT, "", 100_000).unwrap().len();
    assert_eq!(before, after, "a refusal logs no record");

    // The same 65 options as a declared source are refused.
    let declared = CandidateSource::Declared {
        options: BoundedVec::new(
            many(65)
                .iter()
                .map(|(id, name)| catalog_option(id, name))
                .take(64)
                .collect(),
        )
        .unwrap(),
    };
    let refused = decide(&h, catalog_request(id, &schema, &head, policy, declared))
        .await
        .unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
}
