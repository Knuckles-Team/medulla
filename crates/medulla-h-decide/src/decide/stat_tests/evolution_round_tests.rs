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

const RUBRIC: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OTHER_RUBRIC: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const ARTIFACT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ARTIFACT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// A report over criteria `c1..` whose satisfied flags are `states`.
fn criterion_report(rubric: &str, artifact: &str, states: &[bool]) -> serde_json::Value {
    let criteria: Vec<serde_json::Value> = states
        .iter()
        .enumerate()
        .map(|(i, satisfied)| {
            serde_json::json!({
                "criterion_id": format!("c{}", i + 1),
                "satisfied": satisfied,
                "evidence_ref": format!("evidence:{}", i + 1),
            })
        })
        .collect();
    serde_json::json!({
        "rubric_digest": rubric,
        "artifact_digest": artifact,
        "grader_revision": "grader-1",
        "criteria": criteria,
    })
}

/// A review whose rubric holds the prior report's criteria and whose declared
/// artifact is the one the new report grades.
fn review_request(
    schema: &ComponentDependency,
    prior: serde_json::Value,
    new: serde_json::Value,
) -> DecideRequest {
    let rubric = serde_json::json!({
        "rubric_digest": prior["rubric_digest"],
        "criteria": prior["criteria"]
            .as_array()
            .map(|c| c.iter().map(|s| s["criterion_id"].clone()).collect::<Vec<_>>())
            .unwrap_or_default(),
    });
    let artifact = new["artifact_digest"].clone();
    review_request_with(
        schema,
        prior,
        new,
        rubric,
        artifact,
        None,
        DecisionPolicyRef::Default,
    )
}

fn review_request_with(
    schema: &ComponentDependency,
    prior: serde_json::Value,
    new: serde_json::Value,
    rubric: serde_json::Value,
    artifact: serde_json::Value,
    runs: Option<serde_json::Value>,
    policy: DecisionPolicyRef,
) -> DecideRequest {
    let mut params = vec![
        json_param("prior_report", prior),
        json_param("new_report", new),
        json_param("rubric", rubric),
        json_param("artifact_digest", artifact),
    ];
    if let Some(runs) = runs {
        params.push(json_param("runs", runs));
    }
    declared_request(
        schema,
        TENANT,
        QuestionKind::CriterionReview,
        "review",
        policy,
        params,
    )
}

// spec: MEDULLA-DECISION-ENGINE-R136.101
#[tokio::test]
async fn criterion_regression_holds_the_revision() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let prior = criterion_report(RUBRIC, ARTIFACT_A, &[true, false, true, false]);

    // A nonregressing revision (c2 newly satisfied, c1 and c3 held) is accepted
    // and its record names the rubric.
    let accepted = criterion_report(RUBRIC, ARTIFACT_B, &[true, true, true, false]);
    let batch = decide(&h, review_request(&schema, prior.clone(), accepted))
        .await
        .unwrap();
    assert_eq!(options(&batch), vec!["accept".to_string()]);
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::CriterionReview);
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert!(premise_of(&batch, &format!("criterion.rubric_digest={RUBRIC}")).is_some());

    // c1 and c3 both regress: the revision is held and the first failing
    // criterion by id order, c1, is named in the reason and the premise.
    let regressed = criterion_report(RUBRIC, ARTIFACT_B, &[false, true, false, false]);
    let batch = decide(&h, review_request(&schema, prior.clone(), regressed))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("a regression holds the revision, got {:?}", record.outcome)
    };
    let [AbstainReason::Family {
        family,
        code,
        detail,
    }] = reasons.as_slice()
    else {
        panic!("one family reason, got {reasons:?}")
    };
    assert_eq!(*family, QuestionKind::CriterionReview);
    assert_eq!(code.as_str(), "CRITERION_REGRESSION");
    assert!(detail.contains("c1") && !detail.contains("c3"), "{detail}");
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
    assert!(premise_of(&batch, "criterion.regressed:c1").is_some());
    assert!(premise_of(&batch, "criterion.regressed:c3").is_none());

    // A criterion the new report no longer grades is refused as ungraded before
    // the review compares anything (R136.102); the pure review still counts it
    // as regressed (outcome-types unit test).
    let dropped = criterion_report(RUBRIC, ARTIFACT_B, &[true]);
    let refused = decide(&h, review_request(&schema, prior.clone(), dropped))
        .await
        .unwrap_err();
    assert!(refused.contains("CRITERION_UNGRADED"), "{refused}");

    // A mutated rubric digest makes the reports incomparable.
    let other = criterion_report(OTHER_RUBRIC, ARTIFACT_B, &[true, true, true, true]);
    let refused = decide(&h, review_request(&schema, prior.clone(), other))
        .await
        .unwrap_err();
    assert!(refused.contains("CRITERION_RUBRIC_MISMATCH"), "{refused}");

    // Self-reflection text is never a criterion state: the report has no field
    // for it, so a report carrying one is refused.
    let mut reflective = criterion_report(RUBRIC, ARTIFACT_B, &[true, true, true, true]);
    reflective["criteria"][0]["self_reflection"] = serde_json::json!("I think I did well");
    let refused = decide(&h, review_request(&schema, prior.clone(), reflective))
        .await
        .unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");

    // A malformed digest and a repeated criterion id are refused.
    let malformed = criterion_report("not-a-digest", ARTIFACT_B, &[true]);
    assert!(
        decide(&h, review_request(&schema, prior.clone(), malformed))
            .await
            .unwrap_err()
            .starts_with("PARAMETER_INVALID")
    );
    let mut repeated = criterion_report(RUBRIC, ARTIFACT_B, &[true, true]);
    repeated["criteria"][1]["criterion_id"] = serde_json::json!("c1");
    assert!(decide(&h, review_request(&schema, prior, repeated))
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));
}

// spec: MEDULLA-DECISION-ENGINE-R136.102
#[tokio::test]
async fn incomplete_grading_refuses_before_review() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let prior = criterion_report(RUBRIC, ARTIFACT_A, &[true, false, true]);
    let rubric = serde_json::json!({"rubric_digest": RUBRIC, "criteria": ["c1", "c2", "c3"]});
    let ask = |new: serde_json::Value, artifact: &str| {
        review_request_with(
            &schema,
            prior.clone(),
            new,
            rubric.clone(),
            serde_json::json!(artifact),
            None,
            DecisionPolicyRef::Default,
        )
    };

    // A report that leaves c3 out is ungraded, even though nothing regressed
    // among the criteria it does grade: no verdict is returned.
    let missing = criterion_report(RUBRIC, ARTIFACT_B, &[true, true]);
    let refused = decide(&h, ask(missing, ARTIFACT_B)).await.unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    assert!(refused.contains("CRITERION_UNGRADED") && refused.contains("c3"));

    // A report that grades another artifact than the declared one.
    let full = criterion_report(RUBRIC, ARTIFACT_A, &[true, true, true]);
    let refused = decide(&h, ask(full.clone(), ARTIFACT_B)).await.unwrap_err();
    assert!(refused.contains("CRITERION_ARTIFACT_MISMATCH"), "{refused}");

    // A state with an empty evidence reference.
    let mut bare = criterion_report(RUBRIC, ARTIFACT_B, &[true, true, true]);
    bare["criteria"][1]["evidence_ref"] = serde_json::json!("");
    let refused = decide(&h, ask(bare, ARTIFACT_B)).await.unwrap_err();
    assert!(refused.contains("CRITERION_EVIDENCE_MISSING") && refused.contains("c2"));

    // An ungraded prior report is refused the same way.
    let thin_prior = criterion_report(RUBRIC, ARTIFACT_A, &[true]);
    let complete = criterion_report(RUBRIC, ARTIFACT_B, &[true, true, true]);
    let refused = decide(
        &h,
        review_request_with(
            &schema,
            thin_prior,
            complete.clone(),
            rubric.clone(),
            serde_json::json!(ARTIFACT_B),
            None,
            DecisionPolicyRef::Default,
        ),
    )
    .await
    .unwrap_err();
    assert!(refused.contains("CRITERION_UNGRADED"), "{refused}");

    // A malformed declared artifact digest is a parameter error.
    let refused = decide(&h, ask(complete.clone(), "not-a-digest"))
        .await
        .unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");

    // The complete, artifact-bound report is reviewed.
    let batch = decide(&h, ask(complete, ARTIFACT_B)).await.unwrap();
    assert_eq!(options(&batch), vec!["accept".to_string()]);
}

/// Judge and outcome scores of `n` runs: the judge climbs `judge_step` micros
/// per run, the outcome stays flat.
fn runs_json(n: i64, judge_step: i64) -> serde_json::Value {
    serde_json::Value::Array(
        (0..n)
            .map(|i| {
                serde_json::json!({
                    "judge_score_micros": 100_000 + i * judge_step,
                    "outcome_score_micros": 500_000,
                })
            })
            .collect(),
    )
}

// spec: MEDULLA-DECISION-ENGINE-R136.103
#[tokio::test]
async fn rising_judge_score_without_outcome_gain_refuses() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let prior = criterion_report(RUBRIC, ARTIFACT_A, &[true, false]);
    let new = criterion_report(RUBRIC, ARTIFACT_B, &[true, true]);
    let rubric = serde_json::json!({"rubric_digest": RUBRIC, "criteria": ["c1", "c2"]});
    let ask = |runs: serde_json::Value, policy: DecisionPolicyRef| {
        review_request_with(
            &schema,
            prior.clone(),
            new.clone(),
            rubric.clone(),
            serde_json::json!(ARTIFACT_B),
            Some(runs),
            policy,
        )
    };

    // A judge gaining 0.02 per run against a flat outcome stays under the
    // default threshold of 0.15: accepted, with the divergence recorded.
    let batch = decide(
        &h,
        ask(runs_json(10, 200_000 / 10), DecisionPolicyRef::Default),
    )
    .await
    .unwrap();
    assert_eq!(options(&batch), vec!["accept".to_string()]);
    assert!(premise_of(&batch, "criterion.divergence=20000").is_some());

    // A judge gaining 0.2 per run is above it: the evidence refuses with its
    // code, the divergence premise and the knob premises, though no criterion
    // regressed.
    let batch = decide(&h, ask(runs_json(10, 200_000), DecisionPolicyRef::Default))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!(
            "a divergent judge holds the evidence, got {:?}",
            record.outcome
        )
    };
    let [AbstainReason::Family { family, code, .. }] = reasons.as_slice() else {
        panic!("one family reason, got {reasons:?}")
    };
    assert_eq!(*family, QuestionKind::CriterionReview);
    assert_eq!(code.as_str(), "JUDGE_OUTCOME_DIVERGENCE");
    assert!(premise_of(&batch, "criterion.divergence=200000").is_some());
    assert_eq!(
        knob_facts(&batch, "knob:criterion_review."),
        vec![
            "knob:criterion_review.divergence_threshold=0.150000".to_string(),
            "knob:criterion_review.divergence_window=20".to_string(),
        ]
    );

    // A pinned threshold of 0.25 admits the same history; a pinned window of 5
    // reads only the newest runs.
    let mut policy = DecisionPolicy::engine_default();
    policy.knobs.insert(
        KnobName::new("criterion_review.divergence_threshold"),
        KnobValue::DecimalMicros(250_000),
    );
    let pinned = DecisionPolicyRef::Pinned {
        component: h.publish_policy("policy-divergence", &policy),
    };
    let batch = decide(&h, ask(runs_json(10, 200_000), pinned))
        .await
        .unwrap();
    assert_eq!(options(&batch), vec!["accept".to_string()]);
    assert!(
        knob_facts(&batch, "knob:criterion_review.divergence_threshold")
            .contains(&"knob:criterion_review.divergence_threshold=0.250000".to_string())
    );
}

// spec: MEDULLA-DECISION-ENGINE-R136.7
#[tokio::test]
async fn served_verdict_and_allocation_journey() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    // Ten tasks: six flip as declared, four the opposite way. The served
    // verdict is `verified` and its sealed record commits to the decision log.
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
    assert_eq!(options(&batch), vec!["verified".to_string()]);
    let record = batch.records.as_slice()[0].clone();
    let commit = DecisionLogOp::Commit {
        record: Box::new(record.clone()),
        evaluator: None,
    };
    let logged: DecisionLogCommitted = decode(log_op(&h, "decider", commit).await)
        .expect("the verdict record is logged after verify-replay");
    assert_eq!(logged.record_id, record.record_id);
    assert!(!logged.replayed);
    let stored: Option<DecisionLogEntry> = decode(
        log_op(
            &h,
            "decider",
            DecisionLogOp::Get {
                tenant_id: TENANT.to_string(),
                record_id: logged.record_id.clone(),
            },
        )
        .await,
    )
    .unwrap();
    let stored = stored.expect("the committed verdict is read back");
    assert_eq!(
        stored.record.record_digest, logged.record_digest,
        "the committed record digest is the stored one"
    );

    // Three stages and a budget for one upgrade: exactly one stage moves to the
    // strong model, the top-impact one.
    let batch = decide(&h, allocation_request(&schema, allocation_params(100)))
        .await
        .unwrap();
    let chosen = options(&batch);
    assert_eq!(chosen.len(), 3);
    assert_eq!(
        chosen.iter().filter(|c| c.ends_with("=strong")).count(),
        1,
        "{chosen:?}"
    );
    assert!(chosen.contains(&"b=strong".to_string()));
    assert!(premise_of(&batch, "allocation.upgrades=1").is_some());
}
