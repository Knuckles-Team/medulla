//! MEDULLA-DECISION-ENGINE-R823: a served `ReasoningParadigm` question ranks
//! only the paradigms the task's capability tags make legal, records the legal
//! and illegal verdicts as premises, and abstains when none is legal.

use super::*;
use {
    medulla_decision_policy_types::decision::statistical::reasoning_paradigm::COST_KEY,
    medulla_decision_policy_types::decision::statistical::reasoning_paradigm::EXPECTED_VALUE_KEY,
    medulla_decision_policy_types::decision::statistical::reasoning_paradigm::NONE_ADMISSIBLE,
    medulla_decision_policy_types::decision::statistical::reasoning_paradigm::PARADIGMS,
};
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_types::decision::record::AbstainReason,
};
use {
    medulla_decision_statistical_types::decision::statistical::declared::DeclaredNumber,
    medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption,
};

fn paradigm_option(id: &str, tag: &str, expected: i64, cost: i64) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::new(vec![tag.to_string()]).unwrap(),
        numbers: BoundedVec::new(vec![
            DeclaredNumber {
                key: COST_KEY.to_string(),
                q32: cost,
            },
            DeclaredNumber {
                key: EXPECTED_VALUE_KEY.to_string(),
                q32: expected,
            },
        ])
        .unwrap(),
        texts: BoundedVec::default(),
    }
}

fn paradigm_request(schema: &ComponentDependency, tags: &[&str]) -> DecideRequest {
    let mut request = declared_request(
        schema,
        TENANT,
        QuestionKind::ReasoningParadigm,
        "rules",
        DecisionPolicyRef::Default,
        vec![TypedParam {
            name: "task_tags".to_string(),
            value: TypedValue::IriList(
                BoundedVec::new(tags.iter().map(|tag| (*tag).to_string()).collect()).unwrap(),
            ),
        }],
    );
    let value = [(2_i64, 1_i64), (1, 1), (5, 1), (3, 0)];
    request.candidates = CandidateSource::Declared {
        options: BoundedVec::new(
            PARADIGMS
                .iter()
                .zip(value)
                .map(|((id, tag), (expected, cost))| {
                    paradigm_option(id, tag, expected << 32, cost << 32)
                })
                .collect(),
        )
        .unwrap(),
    };
    request
}

// spec: MEDULLA-DECISION-ENGINE-R823
#[tokio::test]
async fn legal_set_then_rank_and_abstain() {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let (schema, _) = publish_route_schema(&h);

    let batch = decide(
        &h,
        paradigm_request(&schema, &["reasoning:rules", "reasoning:statistical"]),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("a legal set is ranked, got {:?}", record.outcome);
    };
    let order: Vec<&str> = scores
        .as_slice()
        .iter()
        .map(|scored| scored.option_id.as_str())
        .collect();
    assert_eq!(order, ["rules", "statistical"]);
    let facts: Vec<&str> = record
        .premises
        .iter()
        .map(|premise| premise.fact.as_str())
        .collect();
    assert_eq!(
        facts,
        [
            "task_tag:reasoning:rules",
            "task_tag:reasoning:statistical",
            "paradigm_illegal:causal",
            "paradigm_illegal:owl",
            "paradigm_legal:rules",
            "paradigm_legal:statistical",
        ]
    );
    assert!(record
        .premises
        .iter()
        .all(|premise| premise.class == PremiseClass::Observation));

    let batch = decide(&h, paradigm_request(&schema, &["reasoning:other"]))
        .await
        .unwrap();
    let StatisticalOutcome::Abstained { reasons } = &batch.records.as_slice()[0].outcome else {
        panic!("no legal paradigm abstains");
    };
    assert!(matches!(
        &reasons.as_slice()[0],
        AbstainReason::Family { code, .. } if *code == NONE_ADMISSIBLE
    ));

    let mut empty = paradigm_request(&schema, &["reasoning:rules"]);
    empty.params = BoundedVec::new(vec![TypedParam {
        name: "task_tags".to_string(),
        value: TypedValue::IriList(BoundedVec::default()),
    }])
    .unwrap();
    let error = decide(&h, empty).await.unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
}
