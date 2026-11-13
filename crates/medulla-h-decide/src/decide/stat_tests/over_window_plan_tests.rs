//! MEDULLA-DECISION-ENGINE-R137.3: a served `OverWindowPlan` question plans an
//! over-window prompt through the family registry. The window comes from the
//! model profile, then the request, then the default knob; a request limit only
//! tightens its knob; another tenant's profile is never read.

use super::*;
use medulla_agent_types::agent_component::{AgentComponentFacts, AgentComponentKind};
use medulla_decision_types::decision::knobs::{KnobName, KnobValue};
use medulla_decision_types::decision::{DecisionPolicy, EvidenceClass};
use medulla_server_api::authority_context::VerifiedRequestContext;

fn int(name: &str, value: i64) -> TypedParam {
    TypedParam {
        name: name.to_string(),
        value: TypedValue::Int(value),
    }
}

fn profile_ref(iri: &str) -> TypedParam {
    TypedParam {
        name: "model_profile_ref".to_string(),
        value: TypedValue::Iri(iri.to_string()),
    }
}

fn publish_profile(h: &Harness, id: &str, window: u32) {
    let mut draft = test_component_draft(TENANT, id);
    draft.kind = AgentComponentKind::ModelProfile;
    draft.summary = format!("model {id}");
    draft.facts = AgentComponentFacts::ModelProfile {
        provider: "provider".to_string(),
        model_identity: format!("model:{id}"),
        context_window_tokens: window,
        max_output_tokens: 1_024,
        supports_tools: false,
        supports_structured_output: false,
        supports_vision: false,
        input_capabilities: None,
        modalities: Default::default(),
        cost: None,
        latency_declared: None,
        latency_observed_ref: None,
        shell_proficiency: None,
        stopping_behaviour: None,
    };
    h.commit(draft, None).unwrap();
}

fn plan_request(
    schema: &ComponentDependency,
    tenant: &str,
    policy: DecisionPolicyRef,
    params: Vec<TypedParam>,
) -> DecideRequest {
    declared_request(
        schema,
        tenant,
        QuestionKind::OverWindowPlan,
        "plan",
        policy,
        params,
    )
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

fn plan(exceeds: bool, chunks: u32, depth: u8, chunk_tokens: u64, calls: u32) -> Vec<String> {
    vec![
        format!("exceeds:{exceeds}"),
        format!("chunks:{chunks}"),
        format!("depth:{depth}"),
        format!("chunk_tokens:{chunk_tokens}"),
        format!("model_calls:{calls}"),
    ]
}

fn pinned_knob(h: &Harness, name: &str, key: &str, value: KnobValue) -> DecisionPolicyRef {
    let mut policy = DecisionPolicy::engine_default();
    policy.knobs.insert(KnobName::new(key), value);
    DecisionPolicyRef::Pinned {
        component: h.publish_policy(name, &policy),
    }
}

// spec: MEDULLA-DECISION-ENGINE-R137.3
#[tokio::test]
async fn decide_routes_context_budget() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let ask =
        |params: Vec<TypedParam>| plan_request(&schema, TENANT, DecisionPolicyRef::Default, params);

    // A prompt of five windows is halved three times: eight chunks of 625 tokens
    // fit under 0.8 of the 1000-token window, and 2 * 8 - 1 calls run.
    let batch = decide(
        &h,
        ask(vec![
            int("prompt_tokens", 5_000),
            int("window_tokens", 1_000),
        ]),
    )
    .await
    .unwrap();
    assert_eq!(options(&batch), plan(true, 8, 3, 625, 15));
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::OverWindowPlan);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    assert!(record
        .premises
        .iter()
        .any(|p| p.fact == "knob:context.over_window_fraction=0.800000"));

    // A prompt under the threshold is one call.
    let batch = decide(
        &h,
        ask(vec![int("prompt_tokens", 500), int("window_tokens", 1_000)]),
    )
    .await
    .unwrap();
    assert_eq!(options(&batch), plan(false, 1, 0, 500, 1));

    // A request limit tighter than the knob refuses and names the limit.
    let refused = decide(
        &h,
        ask(vec![
            int("prompt_tokens", 5_000),
            int("window_tokens", 1_000),
            int("max_depth", 2),
        ]),
    )
    .await
    .unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    assert!(
        refused.contains("CONTEXT_BUDGET_EXHAUSTED: limit=max_depth observed=2"),
        "{refused}"
    );

    // A negative prompt refuses, and so does a missing one.
    for params in [vec![int("prompt_tokens", -1)], vec![]] {
        let refused = decide(&h, ask(params)).await.unwrap_err();
        assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    }

    // The declared option must be `plan`.
    let mut wrong = ask(vec![int("prompt_tokens", 10)]);
    wrong.candidates = CandidateSource::Declared {
        options: BoundedVec::new(vec![declared_option("other")]).unwrap(),
    };
    assert!(decide(&h, wrong)
        .await
        .unwrap_err()
        .starts_with("PARAMETER_INVALID"));
}

// spec: MEDULLA-DECISION-ENGINE-R137.3
#[tokio::test]
async fn window_source_order() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    publish_profile(&h, "profile-wide", 8_192);
    let ask = |policy: DecisionPolicyRef, params: Vec<TypedParam>| {
        plan_request(&schema, TENANT, policy, params)
    };

    // The profile's window (8192, threshold 6553) wins over the request's 1000.
    let batch = decide(
        &h,
        ask(
            DecisionPolicyRef::Default,
            vec![
                int("prompt_tokens", 5_000),
                int("window_tokens", 1_000),
                profile_ref("profile-wide"),
            ],
        ),
    )
    .await
    .unwrap();
    assert_eq!(options(&batch), plan(false, 1, 0, 5_000, 1));

    // Without a profile the request's window applies.
    let batch = decide(
        &h,
        ask(
            DecisionPolicyRef::Default,
            vec![int("prompt_tokens", 5_000), int("window_tokens", 1_000)],
        ),
    )
    .await
    .unwrap();
    assert_eq!(options(&batch), plan(true, 8, 3, 625, 15));

    // Without either the default window knob applies: 128000 by default, and the
    // pinned value when the policy sets one.
    let batch = decide(
        &h,
        ask(
            DecisionPolicyRef::Default,
            vec![int("prompt_tokens", 5_000)],
        ),
    )
    .await
    .unwrap();
    assert_eq!(options(&batch), plan(false, 1, 0, 5_000, 1));
    let pinned = pinned_knob(
        &h,
        "policy-window",
        "context.default_window_tokens",
        KnobValue::Integer(2_000),
    );
    let batch = decide(&h, ask(pinned, vec![int("prompt_tokens", 5_000)]))
        .await
        .unwrap();
    assert_eq!(options(&batch), plan(true, 4, 2, 1_250, 7));

    // A request limit above the knob never loosens it: depth stays capped at the
    // knob's 3, so a prompt needing more refuses naming the knob's value.
    let refused = decide(
        &h,
        ask(
            DecisionPolicyRef::Default,
            vec![
                int("prompt_tokens", 1_000_000),
                int("window_tokens", 1_000),
                int("max_depth", 8),
            ],
        ),
    )
    .await
    .unwrap_err();
    assert!(refused.contains("limit=max_depth observed=3"), "{refused}");
}

// spec: MEDULLA-DECISION-ENGINE-R137.3
#[tokio::test]
async fn cross_tenant_profile_is_not_read() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    publish_profile(&h, "profile-wide", 8_192);
    let params = || {
        vec![
            int("prompt_tokens", 5_000),
            int("window_tokens", 1_000),
            profile_ref("profile-wide"),
        ]
    };

    // The owning tenant reads its profile.
    let batch = decide(
        &h,
        plan_request(&schema, TENANT, DecisionPolicyRef::Default, params()),
    )
    .await
    .unwrap();
    assert_eq!(options(&batch), plan(false, 1, 0, 5_000, 1));

    // Another tenant naming the same profile does not read it: the request's own
    // window applies instead.
    let other = "tenant-profile-other";
    let other_schema = publish_route_schema_in(&h, other);
    let batch = decode::<DecisionBatch>(
        super::super::statistical::handle_decide(
            &h.state,
            1,
            &VerifiedRequestContext::verified_for_test_in_tenant("decider", other),
            plan_request(&other_schema, other, DecisionPolicyRef::Default, params()),
        )
        .await,
    )
    .unwrap();
    assert_eq!(options(&batch), plan(true, 8, 3, 625, 15));
}

/// The refusal a request produces, as the wire error text.
async fn refusal_of(h: &Harness, schema: &ComponentDependency, params: Vec<TypedParam>) -> String {
    decide(
        h,
        plan_request(schema, TENANT, DecisionPolicyRef::Default, params),
    )
    .await
    .unwrap_err()
}

// spec: MEDULLA-DECISION-ENGINE-R137.4
#[tokio::test]
async fn too_many_items_is_refused() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    // Five windows needs eight chunks at depth three; four items are allowed.
    let refused = refusal_of(
        &h,
        &schema,
        vec![
            int("prompt_tokens", 5_000),
            int("window_tokens", 1_000),
            int("max_items", 4),
        ],
    )
    .await;
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    assert!(
        refused.contains("CONTEXT_BUDGET_TOO_MANY_ITEMS: limit=max_items observed=8"),
        "{refused}"
    );
    assert!(!refused.contains("exceeds:"), "a refusal carries no plan");
}

// spec: MEDULLA-DECISION-ENGINE-R137.4
#[tokio::test]
async fn budget_exhausted_names_the_limit() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let prompt = || int("prompt_tokens", 5_000);
    let window = || int("window_tokens", 1_000);
    // One request per limit; each names its limit and the observed value.
    let cases: [(Vec<TypedParam>, &str); 4] = [
        (
            vec![prompt(), window(), int("max_depth", 2)],
            "CONTEXT_BUDGET_EXHAUSTED: limit=max_depth observed=2",
        ),
        (
            vec![prompt(), window(), int("max_cost", 4_000)],
            "CONTEXT_BUDGET_EXHAUSTED: limit=max_cost observed=5000",
        ),
        (
            vec![prompt(), window(), int("max_model_calls", 7)],
            "CONTEXT_BUDGET_EXHAUSTED: limit=max_model_calls observed=15",
        ),
        (
            vec![prompt(), int("window_tokens", 0)],
            "CONTEXT_BUDGET_EXHAUSTED: limit=window_tokens observed=0",
        ),
    ];
    for (params, expected) in cases {
        let refused = refusal_of(&h, &schema, params).await;
        assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
        assert!(refused.contains(expected), "{expected} in {refused}");
    }
    // A negative prompt is a parameter refusal, not a budget one.
    let refused = refusal_of(&h, &schema, vec![int("prompt_tokens", -1)]).await;
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    assert!(!refused.contains("CONTEXT_BUDGET"), "{refused}");
}

// spec: MEDULLA-DECISION-ENGINE-R137.5
#[tokio::test]
async fn premises_name_window_and_limits() {
    use medulla_decision_types::decision::{PremiseClass, PremiseProvenance};
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    publish_profile(&h, "profile-wide", 8_192);
    let premise =
        |batch: &DecisionBatch, fact: &str| -> Option<(PremiseClass, PremiseProvenance)> {
            batch.records.as_slice()[0]
                .premises
                .iter()
                .find(|p| p.fact == fact)
                .map(|p| (p.class, p.provenance.clone()))
        };

    // The default window and every default limit applied.
    let batch = decide(
        &h,
        plan_request(
            &schema,
            TENANT,
            DecisionPolicyRef::Default,
            vec![int("prompt_tokens", 64_000)],
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        premise(&batch, "context_budget.prompt_tokens=64000")
            .unwrap()
            .0,
        PremiseClass::Claim
    );
    // 64000 / 128000 = one half = 2^31.
    assert_eq!(
        premise(&batch, "context_budget.fraction_used_q32=2147483648")
            .unwrap()
            .0,
        PremiseClass::Claim
    );
    assert_eq!(
        premise(&batch, "context_budget.window_tokens=128000")
            .unwrap()
            .0,
        PremiseClass::Claim
    );
    for name in [
        "window_tokens",
        "max_depth",
        "max_items",
        "max_model_calls",
        "max_cost",
    ] {
        let (class, provenance) = premise(&batch, &format!("context_budget.default:{name}"))
            .unwrap_or_else(|| panic!("default premise for {name}"));
        assert_eq!(class, PremiseClass::Definition);
        assert!(matches!(provenance, PremiseProvenance::Policy { .. }));
    }
    assert_eq!(
        premise(&batch, "context_budget.limit:max_depth=3")
            .unwrap()
            .0,
        PremiseClass::Definition
    );
    assert_eq!(
        batch.records.as_slice()[0].evidence_class,
        EvidenceClass::Claim,
        "a default window leaves the record a claim"
    );

    // A profile's window is an observation, and a request limit is no default.
    let batch = decide(
        &h,
        plan_request(
            &schema,
            TENANT,
            DecisionPolicyRef::Default,
            vec![
                int("prompt_tokens", 4_096),
                int("max_depth", 2),
                profile_ref("profile-wide"),
            ],
        ),
    )
    .await
    .unwrap();
    let (class, provenance) = premise(&batch, "context_budget.window_tokens=8192").unwrap();
    assert_eq!(class, PremiseClass::Observation);
    assert!(matches!(provenance, PremiseProvenance::Publisher { .. }));
    assert!(premise(&batch, "context_budget.default:window_tokens").is_none());
    assert!(premise(&batch, "context_budget.default:max_depth").is_none());
    assert_eq!(
        premise(&batch, "context_budget.limit:max_depth=2")
            .unwrap()
            .0,
        PremiseClass::Definition
    );
    assert!(premise(&batch, "context_budget.default:max_items").is_some());
}
