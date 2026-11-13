//! MEDULLA-DECISION-ENGINE-R136.4: served `RoundVerdict` and `StageAllocation`
//! questions answer through the family registry, seal with their resolution kind
//! and evidence class, read the pinned policy's knobs, and refuse another
//! tenant's pinned policy.

use super::*;
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, EvidenceClass, PremiseClass, ResolutionKind,
};
use medulla_server_api::authority_context::VerifiedRequestContext;

fn json_param(name: &str, value: serde_json::Value) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Text(value.to_string()),
    }
}

/// Ten tasks the round declared it would fix: six flip as declared and four the
/// opposite way.
fn round_params() -> Vec<TypedParam> {
    let ids: Vec<String> = (0..10).map(|i| format!("t{i}")).collect();
    let outcomes: Vec<serde_json::Value> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            serde_json::json!({
                "task_id": id,
                "passed_before": i >= 6,
                "passed_after": i < 6,
            })
        })
        .collect();
    vec![
        json_param(
            "prediction",
            serde_json::json!({"round": 1, "fixes": ids, "regressions": []}),
        ),
        json_param("outcomes", serde_json::Value::Array(outcomes)),
    ]
}

/// Three stages (the last without an impact), a cheap base model and a strong
/// one, and a budget for exactly one upgrade.
fn allocation_params(max_cost: u64) -> Vec<TypedParam> {
    vec![
        json_param(
            "stages",
            serde_json::json!([
                {"stage": "a", "impact": 5},
                {"stage": "b", "impact": 9},
                {"stage": "c", "impact": null},
            ]),
        ),
        json_param(
            "models",
            serde_json::json!([
                {"model_ref": "cheap", "cost": 1, "strength": 1, "adequate": true},
                {"model_ref": "strong", "cost": 3, "strength": 9, "adequate": true},
            ]),
        ),
        json_param(
            "budget",
            serde_json::json!({"max_cost": max_cost, "max_stronger_stages": 1}),
        ),
    ]
}

fn options(batch: &DecisionBatch) -> Vec<String> {
    let StatisticalOutcome::Advisory { scores, .. } = &batch.records.as_slice()[0].outcome else {
        panic!(
            "expected an Advisory outcome: {:?}",
            batch.records.as_slice()[0].outcome
        )
    };
    scores.iter().map(|s| s.option_id.clone()).collect()
}

fn knob_facts(batch: &DecisionBatch, prefix: &str) -> Vec<String> {
    batch.records.as_slice()[0]
        .premises
        .iter()
        .filter(|p| p.fact.starts_with(prefix))
        .map(|p| p.fact.clone())
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R136.4
#[tokio::test]
async fn decide_routes_round_kinds() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let verdict_request = |policy: DecisionPolicyRef| {
        declared_request(
            &schema,
            TENANT,
            QuestionKind::RoundVerdict,
            "round",
            policy,
            round_params(),
        )
    };

    // Six of ten flipped as declared and four the opposite way: verified at the
    // default flip fraction, sealed as a constraint backed by an observation.
    let batch = decide(&h, verdict_request(DecisionPolicyRef::Default))
        .await
        .unwrap();
    assert_eq!(options(&batch), vec!["verified".to_string()]);
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::RoundVerdict);
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(record.evidence_class, EvidenceClass::Observation);
    assert_eq!(
        knob_facts(&batch, "knob:round_verdict."),
        vec![
            "knob:round_verdict.flip_fraction=0.500000".to_string(),
            "knob:round_verdict.rounds=1".to_string(),
        ]
    );

    // A pinned flip fraction of 0.7 reads through the policy: six of ten no
    // longer verifies.
    let mut policy = DecisionPolicy::engine_default();
    policy.knobs.insert(
        KnobName::new("round_verdict.flip_fraction"),
        KnobValue::DecimalMicros(700_000),
    );
    let pinned = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-flip-fraction", &policy),
    };
    let batch = decide(&h, verdict_request(pinned.clone())).await.unwrap();
    assert_eq!(options(&batch), vec!["unverified".to_string()]);
    assert!(knob_facts(&batch, "knob:round_verdict.flip_fraction")
        .contains(&"knob:round_verdict.flip_fraction=0.700000".to_string()));

    // With budget for one upgrade only the top-impact stage moves to the strong
    // model; the stage without an impact stays on the base model.
    let allocation = |max_cost: u64, policy: DecisionPolicyRef| {
        declared_request(
            &schema,
            TENANT,
            QuestionKind::StageAllocation,
            "allocation",
            policy,
            allocation_params(max_cost),
        )
    };
    let batch = decide(&h, allocation(100, DecisionPolicyRef::Default))
        .await
        .unwrap();
    assert_eq!(
        options(&batch),
        vec![
            "a=cheap".to_string(),
            "b=strong".to_string(),
            "c=cheap".to_string()
        ]
    );
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::StageAllocation);
    assert_eq!(record.resolution_kind, ResolutionKind::Optimization);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);

    // A budget below the base cost of every stage refuses and names the code
    // and the shortfall; no allocation is returned.
    let refused = decide(&h, allocation(2, DecisionPolicyRef::Default))
        .await
        .unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    assert!(
        refused.contains("ALLOCATION_BUDGET_BELOW_MINIMUM: shortfall=1"),
        "{refused}"
    );

    // A malformed parameter refuses as PARAMETER_INVALID.
    let mut malformed = verdict_request(DecisionPolicyRef::Default);
    malformed.params =
        BoundedVec::new(vec![json_param("prediction", serde_json::json!("nope"))]).unwrap();
    assert!(decide(&h, malformed)
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));

    // Another tenant cannot use this tenant's pinned policy.
    let other = "tenant-round-other";
    let other_schema = publish_route_schema_in(&h, other);
    let foreign = declared_request(
        &other_schema,
        other,
        QuestionKind::RoundVerdict,
        "round",
        pinned,
        round_params(),
    );
    let refused = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(
            &h.state,
            1,
            &VerifiedRequestContext::verified_for_test_in_tenant("decider", other),
            foreign,
        )
        .await,
    );
    assert!(
        refused.is_err(),
        "another tenant's pinned policy is refused"
    );
}

fn allocation_request(schema: &ComponentDependency, params: Vec<TypedParam>) -> DecideRequest {
    declared_request(
        schema,
        TENANT,
        QuestionKind::StageAllocation,
        "allocation",
        DecisionPolicyRef::Default,
        params,
    )
}

fn premise_of<'a>(
    batch: &'a DecisionBatch,
    fact: &str,
) -> Option<&'a medulla_decision_types::decision::PremiseRef> {
    batch.records.as_slice()[0]
        .premises
        .iter()
        .find(|p| p.fact == fact)
}

// spec: MEDULLA-DECISION-ENGINE-R136.5
#[tokio::test]
async fn empty_prediction_abstains() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    // A prediction with no fixes and no regressions abstains with its code
    // through AbstainReason::Family; no verdict is returned.
    let empty = vec![
        json_param(
            "prediction",
            serde_json::json!({"round": 1, "fixes": [], "regressions": []}),
        ),
        json_param("outcomes", serde_json::json!([])),
    ];
    let batch = decide(
        &h,
        declared_request(
            &schema,
            TENANT,
            QuestionKind::RoundVerdict,
            "round",
            DecisionPolicyRef::Default,
            empty,
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("abstained outcome, got {:?}", record.outcome)
    };
    let [AbstainReason::Family { family, code, .. }] = reasons.as_slice() else {
        panic!("one family reason, got {reasons:?}")
    };
    assert_eq!(*family, QuestionKind::RoundVerdict);
    assert_eq!(code.as_str(), "ROUND_NO_PREDICTION");
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);

    // A budget below the base cost names the shortfall in cost units.
    let refused = decide(&h, allocation_request(&schema, allocation_params(2)))
        .await
        .unwrap_err();
    assert!(
        refused.contains("ALLOCATION_BUDGET_BELOW_MINIMUM: shortfall=1"),
        "{refused}"
    );

    // No adequate model refuses with the model count.
    let mut params = allocation_params(100);
    params[1] = json_param(
        "models",
        serde_json::json!([
            {"model_ref": "cheap", "cost": 1, "strength": 1, "adequate": false},
            {"model_ref": "strong", "cost": 3, "strength": 9, "adequate": false},
        ]),
    );
    let refused = decide(&h, allocation_request(&schema, params))
        .await
        .unwrap_err();
    assert!(
        refused.contains("ALLOCATION_NO_ADEQUATE_MODEL: models=2"),
        "{refused}"
    );

    // A stage without impact does not refuse: it stays on the base model and
    // the record names it.
    let batch = decide(&h, allocation_request(&schema, allocation_params(100)))
        .await
        .unwrap();
    assert!(options(&batch).contains(&"c=cheap".to_string()));
    assert!(premise_of(&batch, "allocation.impact_missing:c").is_some());
}

// spec: MEDULLA-DECISION-ENGINE-R136.6
#[tokio::test]
async fn premises_name_counts_and_budget() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    // Six of ten tasks flipped as declared, four the opposite way.
    let batch = decide(
        &h,
        declared_request(
            &schema,
            TENANT,
            QuestionKind::RoundVerdict,
            "round",
            DecisionPolicyRef::Default,
            round_params(),
        ),
    )
    .await
    .unwrap();
    for fact in [
        "round.flip_fraction=0.500000",
        "round.declared_total=10",
        "round.as_declared=6",
        "round.opposite=4",
        "round.verdict=verified",
    ] {
        assert!(premise_of(&batch, fact).is_some(), "premise {fact}");
    }
    assert_eq!(
        premise_of(&batch, "round.flip_fraction=0.500000")
            .unwrap()
            .class,
        PremiseClass::Definition
    );
    assert_eq!(
        knob_facts(&batch, "knob:round_verdict.").len(),
        2,
        "each knob read adds its premise"
    );

    // One upgrade of the top-impact stage: cost 1 + 3 + 1 under a budget of 100.
    let batch = decide(&h, allocation_request(&schema, allocation_params(100)))
        .await
        .unwrap();
    for fact in [
        "allocation.max_cost=100",
        "allocation.cost=5",
        "allocation.upgrades=1",
        "allocation.stage_impact:a=5",
        "allocation.stage_impact:b=9",
        "allocation.impact_missing:c",
    ] {
        assert!(premise_of(&batch, fact).is_some(), "premise {fact}");
    }
    assert!(!knob_facts(&batch, "knob:harness_evolution.").is_empty());
}
