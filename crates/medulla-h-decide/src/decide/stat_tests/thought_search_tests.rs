//! Bounded multi-step reasoning search, served through `Decide`
//! (MEDULLA-DECISION-ENGINE-R750.750 to R750.752, R750.754): the pinned policy names the
//! strategy per task class and the budgets are `thought_search.*` knobs. The
//! caller keeps the arena; each call answers which leaves to expand, then a
//! stop on the node budget.

use super::*;
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::thought_search::{
    BudgetLedger, ReasoningState, ReasoningStrategy, ThoughtNode, ThoughtSearchAnswer, KNOBS,
};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, QuantScaleTag, ResolutionKind,
};

fn policy(strategies: &[(&str, ReasoningStrategy)]) -> DecisionPolicy {
    let mut policy = DecisionPolicy::engine_default();
    for (class, strategy) in strategies {
        policy.reasoning.insert((*class).to_string(), *strategy);
    }
    for (name, value) in [
        ("thought_search.max_nodes", 10),
        ("thought_search.max_branching", 2),
        ("thought_search.beam_width", 2),
    ] {
        policy
            .knobs
            .insert(KnobName::new(name), KnobValue::Integer(value));
    }
    policy
}

fn node(id: u32, parent: Option<u32>, depth: u32, score: i64) -> ThoughtNode {
    ThoughtNode {
        id,
        parent,
        depth,
        score: QuantisedValue {
            scale: QuantScaleTag::Q32,
            value: score << 32,
        },
        tokens: 10,
        state_digest: format!("sha256:{id:064x}"),
    }
}

fn ledger(nodes_used: u64) -> BudgetLedger {
    BudgetLedger {
        nodes_used,
        tokens_used: nodes_used * 10,
        depth_reached: 2,
    }
}

fn thought_request(
    schema: &ComponentDependency,
    policy: DecisionPolicyRef,
    task_class: &str,
    nodes: Vec<ThoughtNode>,
    ledger: BudgetLedger,
    lowered: &[(&str, i64)],
) -> DecideRequest {
    let mut ids: Vec<String> = nodes.iter().map(|n| n.id.to_string()).collect();
    ids.sort();
    ids.dedup();
    let mut params = vec![
        TypedParam {
            name: "task_class".into(),
            value: TypedValue::Text(task_class.into()),
        },
        TypedParam {
            name: "thought_search".into(),
            value: TypedValue::ThoughtSearch(Box::new(ReasoningState::new(nodes, ledger).unwrap())),
        },
    ];
    params.extend(lowered.iter().map(|(name, value)| TypedParam {
        name: (*name).into(),
        value: TypedValue::Int(*value),
    }));
    DecideRequest {
        tenant_id: TENANT.into(),
        question: StatisticalQuestion {
            question_id: "reasoning.search".to_string(),
            kind: QuestionKind::ThoughtSearch,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(
                ids.into_iter()
                    .map(|id| DeclaredOption {
                        option_id: id,
                        classification: BoundedVec::default(),
                        numbers: BoundedVec::default(),
                        texts: BoundedVec::default(),
                    })
                    .collect(),
            )
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

fn answer(record: &StatisticalDecisionRecord) -> &ThoughtSearchAnswer {
    record
        .answer
        .as_ref()
        .and_then(|a| a.thought_search())
        .expect("the family answers a thought-search answer")
}

fn expanded(record: &StatisticalDecisionRecord) -> Vec<u32> {
    match answer(record) {
        ThoughtSearchAnswer::Expand { node_ids } => node_ids.iter().copied().collect(),
        other => panic!("expected an expansion, got {other:?}"),
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R750.750, R750.751 and R750.754:
/// the served search expands, extends the frontier, expands again, then stops
/// on the node budget.
// spec: MEDULLA-DECISION-ENGINE-R750.750
// spec: MEDULLA-DECISION-ENGINE-R750.751
// spec: MEDULLA-DECISION-ENGINE-R750.754
#[tokio::test]
async fn served_decide_chooses_expansion_then_stops() {
    let h = Harness::new().await;
    let (schema_pin, _) = publish_route_schema(&h);
    let pin = h.publish_policy(
        "policy-thought",
        &policy(&[("math", ReasoningStrategy::Tree)]),
    );
    let by_policy = || DecisionPolicyRef::Pinned {
        component: pin.clone(),
    };

    // 1. The root is the only leaf.
    let batch = decide(
        &h,
        thought_request(
            &schema_pin,
            by_policy(),
            "math",
            vec![node(0, None, 0, 1)],
            ledger(1),
            &[],
        ),
    )
    .await
    .unwrap();
    let first = &batch.records.as_slice()[0];
    assert_eq!(expanded(first), vec![0]);
    assert_eq!(first.question.kind, QuestionKind::ThoughtSearch);
    assert!(matches!(first.outcome, StatisticalOutcome::Advisory { .. }));

    // 2. The caller extended the frontier: the beam takes the best two.
    let frontier = vec![node(1, Some(0), 1, 5), node(2, Some(0), 1, 3)];
    let batch = decide(
        &h,
        thought_request(&schema_pin, by_policy(), "math", frontier, ledger(3), &[]),
    )
    .await
    .unwrap();
    assert_eq!(expanded(&batch.records.as_slice()[0]), vec![1, 2]);

    // 3. Nine of ten nodes are spent: no expansion fits, so the search stops
    // with the best node and a certified gap.
    let frontier = vec![
        node(3, Some(1), 2, 9),
        node(4, Some(1), 2, 2),
        node(5, Some(2), 2, 4),
    ];
    let batch = decide(
        &h,
        thought_request(&schema_pin, by_policy(), "math", frontier, ledger(9), &[]),
    )
    .await
    .unwrap();
    let last = &batch.records.as_slice()[0];
    let ThoughtSearchAnswer::Stop {
        best_node,
        certified_gap,
        path_digest,
        ..
    } = answer(last)
    else {
        panic!("expected a stop, got {:?}", answer(last));
    };
    assert_eq!((*best_node, *certified_gap), (3, true));
    assert!(path_digest.starts_with("sha256:"));
    assert_eq!(last.resolution_kind, ResolutionKind::Optimization);
    assert_eq!(last.evidence_class, EvidenceClass::Claim);
    let StatisticalOutcome::Abstained { reasons } = &last.outcome else {
        panic!("a budget stop abstains");
    };
    assert!(matches!(
        &reasons.as_slice()[0],
        AbstainReason::BudgetExhausted { incumbent: Some(node), .. } if node == "3"
    ));
}

/// Verifies MEDULLA-DECISION-ENGINE-R750.754: premises name the strategy, the
/// ledger and each chosen node, with the knobs read.
// spec: MEDULLA-DECISION-ENGINE-R750.754
#[tokio::test]
async fn premises_name_strategy_ledger_and_nodes() {
    let h = Harness::new().await;
    let (schema_pin, _) = publish_route_schema(&h);
    let pin = h.publish_policy(
        "policy-thought",
        &policy(&[("math", ReasoningStrategy::Tree)]),
    );
    let frontier = vec![node(1, Some(0), 1, 5), node(2, Some(0), 1, 3)];
    let batch = decide(
        &h,
        thought_request(
            &schema_pin,
            DecisionPolicyRef::Pinned { component: pin },
            "math",
            frontier,
            ledger(3),
            &[],
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    let facts: Vec<&str> = record.premises.iter().map(|p| p.fact.as_str()).collect();
    for expected in [
        "thought:strategy=tree",
        "thought:ledger=3/30/2",
        "thought:expand=1",
        "thought:expand=2",
        "knob:thought_search.max_nodes=10",
        "knob:thought_search.beam_width=2",
        "knob:thought_search.max_depth=16",
    ] {
        assert!(facts.contains(&expected), "missing premise {expected}");
    }
    assert!(facts.iter().any(|f| f.starts_with("thought:score=1/")));
    assert_eq!(
        facts
            .iter()
            .filter(|f| f.starts_with("knob:thought_search."))
            .count(),
        KNOBS.len(),
        "one premise per knob read"
    );
}

/// Verifies MEDULLA-DECISION-ENGINE-R750.754: a policy pinned in another
/// tenant is not read.
// spec: MEDULLA-DECISION-ENGINE-R750.754
#[tokio::test]
async fn other_tenant_policy_is_not_read() {
    let h = Harness::new().await;
    let (schema_pin, _) = publish_route_schema(&h);
    let pin = h.publish_policy(
        "policy-thought",
        &policy(&[("math", ReasoningStrategy::Chain)]),
    );
    let by_policy = DecisionPolicyRef::Pinned { component: pin };

    // The policy resolves for its own tenant and not for another one.
    assert!(super::super::stat_support::resolve_policy(&h.store, TENANT, &by_policy).is_ok());
    assert!(
        super::super::stat_support::resolve_policy(&h.store, "tenant-other", &by_policy).is_err()
    );

    let mut request = thought_request(
        &schema_pin,
        by_policy,
        "math",
        vec![node(0, None, 0, 1)],
        ledger(1),
        &[],
    );
    request.tenant_id = "tenant-other".to_string();
    let other = VerifiedRequestContext::verified_for_test_in_tenant("decider", "tenant-other");
    let refused = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(&h.state, 1, &other, request).await,
    )
    .unwrap_err();
    assert!(refused.starts_with("COMPONENT_PIN_MISMATCH"), "{refused}");
}

/// Verifies MEDULLA-DECISION-ENGINE-R750.752 and the R750.754 policy-loosening
/// abstention: one request per refusal code, and the budget abstentions.
// spec: MEDULLA-DECISION-ENGINE-R750.752
#[tokio::test]
async fn thought_search_refusals() {
    let h = Harness::new().await;
    let (schema_pin, _) = publish_route_schema(&h);
    let pin = h.publish_policy(
        "policy-thought",
        &policy(&[
            ("math", ReasoningStrategy::Tree),
            ("merge", ReasoningStrategy::Graph),
        ]),
    );
    let by_policy = || DecisionPolicyRef::Pinned {
        component: pin.clone(),
    };
    let refused = |request: DecideRequest| {
        let h = &h;
        async move { decide(h, request).await.unwrap_err() }
    };
    let ask = |class: &str, nodes: Vec<ThoughtNode>, lowered: &[(&str, i64)]| {
        thought_request(&schema_pin, by_policy(), class, nodes, ledger(1), lowered)
    };

    // THOUGHT_STRATEGY_UNKNOWN: the policy names no strategy for the class.
    let error = refused(ask("unmapped", vec![node(0, None, 0, 1)], &[])).await;
    assert!(error.starts_with("THOUGHT_STRATEGY_UNKNOWN"), "{error}");

    // THOUGHT_BUDGET_INVALID: a lowered budget is 0, or above its knob maximum.
    for value in [0, 17] {
        let error = refused(ask(
            "math",
            vec![node(0, None, 0, 1)],
            &[("thought_search.beam_width", value)],
        ))
        .await;
        assert!(error.starts_with("THOUGHT_BUDGET_INVALID"), "{error}");
    }

    // THOUGHT_MERGE_CYCLE: a graph merge makes a thought its own descendant.
    let mut twin = node(1, Some(0), 1, 2);
    twin.state_digest = node(0, None, 0, 1).state_digest;
    let error = refused(ask("merge", vec![node(0, None, 0, 1), twin.clone()], &[])).await;
    assert!(error.starts_with("THOUGHT_MERGE_CYCLE"), "{error}");
    // An unknown parameter name is refused.
    let error = refused(ask(
        "math",
        vec![node(0, None, 0, 1), node(1, Some(0), 1, 2)],
        &[("x", 1)],
    ))
    .await;
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");

    // THOUGHT_SCORE_NOT_FIXED_POINT: a score off scale Q32.
    let mut float = node(0, None, 0, 1);
    float.score.scale = QuantScaleTag::Pico;
    let error = refused(ask("math", vec![float], &[])).await;
    assert!(
        error.starts_with("THOUGHT_SCORE_NOT_FIXED_POINT"),
        "{error}"
    );

    // THOUGHT_NODE_INVALID: a duplicate id, a depth that disagrees with the
    // parent, and a parent that is not an earlier arena index.
    let duplicate = vec![
        node(0, None, 0, 1),
        node(1, Some(0), 1, 2),
        node(1, Some(0), 1, 3),
    ];
    let wrong_depth = vec![node(0, None, 0, 1), node(1, Some(0), 5, 2)];
    let orphan = vec![node(1, Some(7), 1, 2)];
    for nodes in [duplicate, wrong_depth, orphan] {
        let error = refused(ask("math", nodes, &[])).await;
        assert!(error.starts_with("THOUGHT_NODE_INVALID"), "{error}");
    }

    // A budget stop is not a refusal: it abstains naming the best node.
    let spent = thought_request(
        &schema_pin,
        by_policy(),
        "math",
        vec![node(0, None, 0, 1)],
        ledger(10),
        &[],
    );
    let batch = decide(&h, spent).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("a spent budget abstains");
    };
    assert!(matches!(
        &reasons.as_slice()[0],
        AbstainReason::BudgetExhausted { incumbent: Some(node), .. } if node == "0"
    ));

    // A request may lower a budget, never raise it.
    let raised = ask(
        "math",
        vec![node(0, None, 0, 1)],
        &[("thought_search.beam_width", 9)],
    );
    let batch = decide(&h, raised).await.unwrap();
    let StatisticalOutcome::Abstained { reasons } = &batch.records.as_slice()[0].outcome else {
        panic!("a raised budget abstains");
    };
    assert!(matches!(
        &reasons.as_slice()[0],
        AbstainReason::PolicyLoosening { field } if field == "thought_search.beam_width"
    ));
    let lowered = ask(
        "math",
        vec![node(0, None, 0, 1)],
        &[("thought_search.beam_width", 1)],
    );
    let batch = decide(&h, lowered).await.unwrap();
    assert_eq!(expanded(&batch.records.as_slice()[0]), vec![0]);
}
