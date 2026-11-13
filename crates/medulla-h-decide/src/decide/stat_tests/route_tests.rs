//! MEDULLA-DECISION-ENGINE-R035.750.1: a served `Route` question runs the
//! rule stages first; a short turn goes to the fast path, and any other turn
//! reaches the shared ladder (the learned policy).

use super::*;
use medulla_decision_stats::decision::route::FAST_PATH_CLASS;
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::log::{
    DecisionOutcomeEvaluation, RecordVisibility, StoredEvaluation, DECISION_LOG_SCHEMA_VERSION,
};
use medulla_decision_types::decision::{PremiseClass, PremiseProvenance, ResolutionKind};
use medulla_persistence_records::decision_jobs::{encode_artifact, evaluation_key, record_key};

fn route_option(id: &str, classification: &[&str]) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::new(classification.iter().map(|c| c.to_string()).collect())
            .unwrap(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::default(),
    }
}

fn route_request(
    schema: &ComponentDependency,
    policy: DecisionPolicyRef,
    turn_tokens: Option<TypedValue>,
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
    if let Some(value) = turn_tokens {
        params.push(TypedParam {
            name: "route.turn_tokens".to_string(),
            value,
        });
    }
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "route.turn".to_string(),
            kind: QuestionKind::Route,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![
                route_option("alpha", &[FAST_PATH_CLASS]),
                route_option("beta", &["eg:route/team"]),
                route_option("zeta", &[FAST_PATH_CLASS]),
            ])
            .unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy,
        params: BoundedVec::new(params).unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

fn route_stage_premises(
    record: &StatisticalDecisionRecord,
) -> Vec<&medulla_decision_types::decision::PremiseRef> {
    record
        .premises
        .iter()
        .filter(|premise| premise.fact.starts_with("route_stage="))
        .collect()
}

// spec: MEDULLA-DECISION-ENGINE-R035.750.1
#[tokio::test]
async fn first_matching_route_stage_wins() {
    let h = Harness::new().await;
    let (schema, _digest) = publish_route_schema(&h);

    // A short turn takes the fast path: the lowest-id option classified
    // fast-path, a constraint-resolved advisory choice with its premise.
    let batch = decide(
        &h,
        route_request(
            &schema,
            DecisionPolicyRef::Default,
            Some(TypedValue::Int(100)),
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::Route);
    let StatisticalOutcome::Advisory { scores, calibrated } = &record.outcome else {
        panic!("expected the fast-path choice: {:?}", record.outcome)
    };
    assert!(!calibrated);
    assert_eq!(
        scores
            .iter()
            .map(|s| s.option_id.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha"]
    );
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    let premises = route_stage_premises(record);
    assert_eq!(premises.len(), 1);
    assert_eq!(premises[0].fact, "route_stage=fast_path");
    assert_eq!(premises[0].class, PremiseClass::Definition);
    assert!(matches!(
        premises[0].provenance,
        PremiseProvenance::Policy { .. }
    ));

    // The bound is inclusive at the default 256 and exclusive past it: the
    // next turn reaches the shared ladder, which abstains without a head.
    let at_bound = decide(
        &h,
        route_request(
            &schema,
            DecisionPolicyRef::Default,
            Some(TypedValue::Int(256)),
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        at_bound.records.as_slice()[0].outcome,
        StatisticalOutcome::Advisory { .. }
    ));
    for turn in [Some(TypedValue::Int(257)), None] {
        let batch = decide(&h, route_request(&schema, DecisionPolicyRef::Default, turn))
            .await
            .unwrap();
        let record = &batch.records.as_slice()[0];
        assert!(
            matches!(record.outcome, StatisticalOutcome::Abstained { .. }),
            "the ladder answers when no rule stage does: {:?}",
            record.outcome
        );
        assert!(route_stage_premises(record).is_empty());
    }

    // The bound is the pinned policy's knob, not a default.
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.knobs.insert(
        KnobName::new("route.fast_path_max_tokens"),
        KnobValue::Integer(50),
    );
    let pin = h.publish_policy("policy-route-bound", &policy);
    let lowered = DecisionPolicyRef::Pinned { component: pin };
    let over = decide(
        &h,
        route_request(&schema, lowered.clone(), Some(TypedValue::Int(100))),
    )
    .await
    .unwrap();
    assert!(matches!(
        over.records.as_slice()[0].outcome,
        StatisticalOutcome::Abstained { .. }
    ));
    let under = decide(
        &h,
        route_request(&schema, lowered, Some(TypedValue::Int(50))),
    )
    .await
    .unwrap();
    assert_eq!(route_stage_premises(&under.records.as_slice()[0]).len(), 1);

    // A malformed turn size is a typed refusal, never a guess.
    for bad in [TypedValue::Int(-1), TypedValue::Text("100".to_string())] {
        let refused = decide(
            &h,
            route_request(&schema, DecisionPolicyRef::Default, Some(bad)),
        )
        .await
        .unwrap_err();
        assert!(
            refused.starts_with("PARAMETER_INVALID"),
            "expected PARAMETER_INVALID, got {refused}"
        );
    }
}

/// Log `count` executed decisions of `principal` in `tenant` that ran `option`
/// in `context`, each judged once: the first `successes` as successes, the
/// rest as failures, and `censored` more records judged neither way.
#[allow(clippy::too_many_arguments)]
fn write_team_history(
    h: &Harness,
    tenant: &str,
    principal: &str,
    base: &StatisticalDecisionRecord,
    context: &str,
    option: &str,
    (successes, failures, censored): (usize, usize, usize),
) -> Vec<String> {
    let now = medulla_kernel::kernel::dispatch_support::authoritative_now_ms();
    let mut ids = Vec::new();
    let outcomes = std::iter::repeat_n(Some(true), successes)
        .chain(std::iter::repeat_n(Some(false), failures))
        .chain(std::iter::repeat_n(None, censored));
    for (n, success) in outcomes.enumerate() {
        let mut record = base.clone();
        record.record_id = format!("decision:{tenant}-{principal}-{option}-{n}");
        record.tenant_id = tenant.to_string();
        record.caller_principal = principal.to_string();
        record.created_at_ms = now - 1_000 - n as u64;
        record.inputs.context_digest = Some(context.to_string());
        record.outcome = StatisticalOutcome::Explored {
            option_id: option.to_string(),
            propensity: rational(1, 2),
        };
        let entry = DecisionLogEntry {
            schema_version: DECISION_LOG_SCHEMA_VERSION,
            visibility: RecordVisibility::Tenant,
            committed_by: principal.to_string(),
            committed_at_ms: record.created_at_ms,
            record: Box::new(record.clone()),
            inputs: EntryInputs::Inline,
        };
        let judged = StoredEvaluation {
            evaluation: DecisionOutcomeEvaluation {
                record_id: record.record_id.clone(),
                evaluation_id: "eval-0".to_string(),
                class: EvidenceClass::Observation,
                selected_agent: "agent-run".to_string(),
                lease_holder: "lease-run".to_string(),
                fidelity: OutcomeFidelity::FullStep,
                success,
                backbone: None,
            },
            producer: "independent-evaluator".to_string(),
            recorded_at_ms: record.created_at_ms,
        };
        h.store
            .put_decision_artifacts(
                tenant,
                &[
                    (
                        record_key(&record.record_id),
                        encode_artifact(&entry).unwrap(),
                    ),
                    (
                        evaluation_key(&record.record_id, "eval-0"),
                        encode_artifact(&judged).unwrap(),
                    ),
                ],
            )
            .unwrap();
        ids.push(record.record_id.clone());
    }
    ids
}

fn chosen_ids(record: &StatisticalDecisionRecord) -> Vec<String> {
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected a reused option, got {:?}", record.outcome)
    };
    scores.iter().map(|s| s.option_id.clone()).collect()
}

// spec: MEDULLA-DECISION-ENGINE-R035.750.2
#[tokio::test]
async fn a_proven_team_is_reused_before_the_learned_policy() {
    let h = Harness::new().await;
    let (schema, _digest) = publish_route_schema(&h);
    let ask =
        |policy: DecisionPolicyRef, turn: Option<TypedValue>| route_request(&schema, policy, turn);
    // The caller asked this question in this context before.
    let asked = ask(DecisionPolicyRef::Default, None);
    let digest = super::super::stat_log::context_digest_of(
        &asked.question.question_id,
        asked.params.as_slice(),
    );
    let base = decide(&h, asked.clone()).await.unwrap().records.as_slice()[0].clone();
    // No history: no rule stage answers, so the learned policy (the ladder)
    // abstains without a head.
    assert!(matches!(base.outcome, StatisticalOutcome::Abstained { .. }));
    let me = verified().principal_persistence_id();

    // beta: judged 6 times, 5 successes (0.83 >= 0.8). zeta: judged 5 times,
    // 2 successes (too low). alpha: judged 3 times, all successes (too few),
    // plus 5 censored judgements that count toward nothing.
    let beta = write_team_history(&h, TENANT, &me, &base, &digest, "beta", (5, 1, 0));
    write_team_history(&h, TENANT, &me, &base, &digest, "zeta", (2, 3, 0));
    write_team_history(&h, TENANT, &me, &base, &digest, "alpha", (3, 0, 5));

    let batch = decide(&h, ask(DecisionPolicyRef::Default, None))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::Route);
    assert_eq!(chosen_ids(record), ["beta"]);
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    let stage = route_stage_premises(record);
    assert_eq!(stage.len(), 1);
    assert_eq!(stage[0].fact, "route_stage=team_reuse");
    let matched: Vec<&str> = record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("matched_record:"))
        .map(|p| p.fact.trim_start_matches("matched_record:"))
        .collect();
    let mut expected: Vec<&str> = beta.iter().map(String::as_str).collect();
    expected.sort_unstable();
    let mut got = matched.clone();
    got.sort_unstable();
    assert_eq!(
        got, expected,
        "one premise per matched record of the option"
    );
    assert!(record
        .premises
        .iter()
        .filter(|p| p.fact.starts_with("matched_record:"))
        .all(|p| p.class == PremiseClass::Observation));

    // A short turn still takes the fast path before the team is consulted.
    let fast = decide(
        &h,
        ask(DecisionPolicyRef::Default, Some(TypedValue::Int(100))),
    )
    .await
    .unwrap();
    // (A different turn size is a different context: no history matches, and
    // the fast path answers on its own terms.)
    let fast_record = &fast.records.as_slice()[0];
    assert_eq!(chosen_ids(fast_record), ["alpha"]);
    assert_eq!(fast_record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(
        route_stage_premises(fast_record)[0].fact,
        "route_stage=fast_path"
    );

    // The floors are the pinned policy's knobs: 3 judgements admit alpha (3 of
    // 3), and its rate is the best.
    let low_floor = policy_pin(
        &h,
        "policy-route-team-samples",
        &[("route.team_reuse_min_samples", KnobValue::Integer(3))],
    );
    let batch = decide(&h, ask(low_floor, None)).await.unwrap();
    assert_eq!(chosen_ids(&batch.records.as_slice()[0]), ["alpha"]);
    // A stricter rate than beta's 0.83 leaves no team to reuse.
    let strict = policy_pin(
        &h,
        "policy-route-team-rate",
        &[(
            "route.team_reuse_min_rate",
            KnobValue::DecimalMicros(900_000),
        )],
    );
    let batch = decide(&h, ask(strict, None)).await.unwrap();
    assert!(matches!(
        batch.records.as_slice()[0].outcome,
        StatisticalOutcome::Abstained { .. }
    ));

    // Another principal's or another tenant's proven team is never read: a
    // fresh caller with the same context has no history of its own.
    let other = "principal:sha256:someone-else";
    write_team_history(&h, TENANT, other, &base, &digest, "zeta", (9, 0, 0));
    write_team_history(&h, "tenant-b", &me, &base, &digest, "zeta", (9, 0, 0));
    let batch = decide(&h, ask(DecisionPolicyRef::Default, None))
        .await
        .unwrap();
    assert_eq!(
        chosen_ids(&batch.records.as_slice()[0]),
        ["beta"],
        "only the caller's own evaluated history counts"
    );
    let mut foreign = ask(DecisionPolicyRef::Default, None);
    foreign.tenant_id = "tenant-b".to_string();
    assert!(decide(&h, foreign)
        .await
        .unwrap_err()
        .starts_with("ACCESS_DENIED"));
}

fn policy_pin(h: &Harness, id: &str, knobs: &[(&str, KnobValue)]) -> DecisionPolicyRef {
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    for (name, value) in knobs {
        policy.knobs.insert(KnobName::new(name), *value);
    }
    DecisionPolicyRef::Pinned {
        component: h.publish_policy(id, &policy),
    }
}
