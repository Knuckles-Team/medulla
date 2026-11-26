//! Served `Decide` of kind `PlacementPlan` (MEDULLA-PLACEMENT-R700.3): services
//! placed onto the `placement:Host` budgets of a graph, sealed with the plan.

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::placement::HeavyCapability;
use medulla_decision_types::decision::statistical::placement_plan::{
    PlacementPlanQuestion, PlacementPlanRead, PlacementPlanRequestBody, ServiceSpec,
    PLACEMENT_PLAN_OPTION, PLACEMENT_PLAN_PARAM,
};
use medulla_decision_types::decision::{
    AbstainReason, EvidenceClass, PremiseClass, ResolutionKind,
};

use super::*;

const GRAPH: &str = "placement-plan";
const GIB: u64 = 1 << 30;

/// A harness whose graph holds one `placement:Host` node per `(iri, capabilities)`.
async fn plan_harness(hosts: &[(&str, &[&str])]) -> Harness {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    let mut guard = h.state.write().await;
    guard
        .registry
        .create_graph(
            GRAPH,
            medulla_protocol_types::protocol::GraphType::Team,
            None,
        )
        .unwrap();
    let core = guard.registry.get(GRAPH).unwrap().core.clone();
    for (iri, capabilities) in hosts {
        let props = serde_json::json!({
            "type": super::super::stat_placement_plan::HOST_CLASS,
            medulla_types::ontology_iri!("placement#cpuMillis"): 4_000,
            medulla_types::ontology_iri!("placement#memoryBytes"): 8 * GIB,
            medulla_types::ontology_iri!("placement#storageBytes"): 100 * GIB,
            medulla_types::ontology_iri!("placement#capabilities"): capabilities,
        });
        core.add_node(iri.to_string(), rmp_serde::to_vec_named(&props).unwrap());
    }
    drop(guard);
    h
}

pub(super) fn service(
    iri: &str,
    cpu_millis: u64,
    required_capability: Option<HeavyCapability>,
    current_host: Option<&str>,
) -> ServiceSpec {
    ServiceSpec {
        service_iri: iri.to_string(),
        cpu_millis,
        memory_bytes: GIB,
        storage_bytes: GIB,
        required_capability,
        current_host: current_host.map(str::to_string),
    }
}

pub(super) fn plan_request(
    schema: &ComponentDependency,
    services: Vec<ServiceSpec>,
    read: Option<PlacementPlanRead>,
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "deploy.placement_plan".to_string(),
            kind: QuestionKind::PlacementPlan,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(vec![DeclaredOption {
                option_id: PLACEMENT_PLAN_OPTION.to_string(),
                classification: BoundedVec::default(),
                numbers: BoundedVec::default(),
                texts: BoundedVec::default(),
            }])
            .unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![TypedParam {
            name: PLACEMENT_PLAN_PARAM.to_string(),
            value: TypedValue::PlacementPlan(Box::new(PlacementPlanQuestion {
                graph: GRAPH.to_string(),
                body: PlacementPlanRequestBody { services },
                read,
            })),
        }])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
        evidence_streams: Default::default(),
    }
}

// spec: MEDULLA-PLACEMENT-R700.3
#[tokio::test]
async fn plan_request_returns_sealed_plan() {
    let h = plan_harness(&[("host:alpha", &[]), ("host:beta", &["backtest"])]).await;
    let (schema_pin, _digest) = publish_route_schema(&h);
    let services = vec![
        service("svc:a", 250, None, None),
        service("svc:b", 250, None, None),
        service("svc:c", 250, None, None),
    ];

    let batch = decide(&h, plan_request(&schema_pin, services.clone(), None))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::PlacementPlan);
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    let StatisticalOutcome::Advisory { scores, calibrated, .. } = &record.outcome else {
        panic!("expected an Advisory outcome: {:?}", record.outcome)
    };
    assert!(!calibrated, "a plan score is not a probability");
    let ids: Vec<&str> = scores.iter().map(|s| s.option_id.as_str()).collect();
    assert_eq!(ids, [PLACEMENT_PLAN_OPTION]);

    let Some(FamilyAnswer::PlacementPlan(plan)) = &record.answer else {
        panic!("a placement plan is sealed: {:?}", record.answer)
    };
    let mut placed: Vec<&str> = plan
        .placements
        .iter()
        .map(|placement| placement.service_iri.as_str())
        .collect();
    placed.sort_unstable();
    assert_eq!(placed, ["svc:a", "svc:b", "svc:c"]);
    assert!(plan
        .placements
        .iter()
        .all(|placement| ["host:alpha", "host:beta"].contains(&placement.host_iri.as_str())));
    plan.check().unwrap();

    // One observation premise per host read.
    let observed: Vec<&str> = record
        .premises
        .iter()
        .filter(|premise| premise.class == PremiseClass::Observation)
        .filter(|premise| premise.fact.starts_with("placement:host "))
        .map(|premise| premise.subject.as_str())
        .collect();
    assert_eq!(observed, ["host:alpha", "host:beta"]);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);

    // A caller never supplies the engine's read.
    let refused = decide(
        &h,
        plan_request(
            &schema_pin,
            services,
            Some(PlacementPlanRead { hosts: vec![] }),
        ),
    )
    .await
    .unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
}

/// Add one `placement:Host` node to the graph of [`plan_harness`].
async fn add_host(h: &Harness, iri: &str, capabilities: &[&str]) {
    let guard = h.state.read().await;
    let core = guard.registry.get(GRAPH).unwrap().core.clone();
    let props = serde_json::json!({
        "type": super::super::stat_placement_plan::HOST_CLASS,
        medulla_types::ontology_iri!("placement#cpuMillis"): 4_000,
        medulla_types::ontology_iri!("placement#memoryBytes"): 8 * GIB,
        medulla_types::ontology_iri!("placement#storageBytes"): 100 * GIB,
        medulla_types::ontology_iri!("placement#capabilities"): capabilities,
    });
    core.add_node(iri.to_string(), rmp_serde::to_vec_named(&props).unwrap());
}

// spec: MEDULLA-PLACEMENT-R700.6
#[tokio::test]
async fn capability_gap_abstains_and_plan_solves() {
    // Three hosts, five services. One service needs `Backtest`, which no host declares.
    let h = plan_harness(&[("host:alpha", &[]), ("host:beta", &[]), ("host:gamma", &[])]).await;
    let (schema_pin, _digest) = publish_route_schema(&h);
    let services = vec![
        service("svc:a", 250, None, Some("host:alpha")),
        service("svc:b", 250, None, Some("host:beta")),
        service("svc:c", 250, None, Some("host:gamma")),
        service("svc:d", 250, None, Some("host:alpha")),
        service("svc:e", 250, Some(HeavyCapability::Backtest), Some("host:alpha")),
    ];

    // The first request abstains, naming the capability no host declares.
    let batch = decide(&h, plan_request(&schema_pin, services.clone(), None))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("a capability gap must abstain: {:?}", record.outcome)
    };
    let named = reasons.iter().any(|reason| {
        matches!(
            reason,
            AbstainReason::UncoveredCapability { capability } if capability == "backtest"
        )
    });
    assert!(named, "the abstention names backtest: {reasons:?}");
    assert!(record.answer.is_none(), "an abstention seals no plan");

    // After one host declares the capability the plan solves with the minimum moves: only
    // the service that needs it leaves its host.
    add_host(&h, "host:delta", &["backtest"]).await;
    let batch = decide(&h, plan_request(&schema_pin, services, None))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    assert!(
        matches!(record.outcome, StatisticalOutcome::Advisory { .. }),
        "{:?}",
        record.outcome
    );
    let Some(FamilyAnswer::PlacementPlan(plan)) = &record.answer else {
        panic!("a placement plan is sealed: {:?}", record.answer)
    };
    assert_eq!(plan.placements.len(), 5);
    assert_eq!(plan.moves.len(), 1, "{:?}", plan.moves);
    assert_eq!(plan.moves[0].service_iri, "svc:e");
    assert_eq!(plan.moves[0].from_host, "host:alpha");
    assert_eq!(plan.moves[0].to_host, "host:delta");
    plan.check().unwrap();
}

// spec: MEDULLA-PLACEMENT-R700.4
#[tokio::test]
async fn plan_refusals_by_reason() {
    let h = plan_harness(&[("host:alpha", &[])]).await;
    let (schema_pin, _digest) = publish_route_schema(&h);

    // A capability no host declares abstains with the typed reason.
    let gap = vec![service("svc:a", 250, Some(HeavyCapability::Gpu), None)];
    let batch = decide(&h, plan_request(&schema_pin, gap, None)).await.unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("a capability gap must abstain: {:?}", record.outcome)
    };
    assert_eq!(
        reasons.as_slice(),
        [AbstainReason::UncoveredCapability {
            capability: "gpu".to_string()
        }]
    );
    assert!(record.answer.is_none(), "an abstention seals no plan");

    // A plan that cannot fit names the first exceeded resource.
    let overrun = vec![service("svc:big", 9_000, None, None)];
    let batch = decide(&h, plan_request(&schema_pin, overrun, None))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("an overrun must abstain: {:?}", record.outcome)
    };
    let [AbstainReason::Infeasible { constraints }] = reasons.as_slice() else {
        panic!("{reasons:?}")
    };
    assert!(
        constraints.iter().any(|constraint| constraint.ends_with("cpu exceeded")),
        "{constraints:?}"
    );
}

// spec: MEDULLA-PLACEMENT-R700.5
#[tokio::test]
async fn plan_record_replays_from_inputs() {
    let h = plan_harness(&[("host:alpha", &[]), ("host:beta", &[])]).await;
    let (schema_pin, _digest) = publish_route_schema(&h);
    let services = vec![
        service("svc:a", 250, None, Some("host:alpha")),
        service("svc:b", 250, None, None),
    ];
    let first = decide(&h, plan_request(&schema_pin, services.clone(), None))
        .await
        .unwrap();
    let second = decide(&h, plan_request(&schema_pin, services, None))
        .await
        .unwrap();
    let plan_of = |batch: &DecisionBatch| match &batch.records.as_slice()[0].answer {
        Some(FamilyAnswer::PlacementPlan(plan)) => plan.clone(),
        other => panic!("a placement plan is sealed: {other:?}"),
    };
    let (plan, again) = (plan_of(&first), plan_of(&second));
    assert_eq!(plan, again, "the same inputs reproduce the same plan");
    plan.check().unwrap();
    assert!(plan.node_count > 0, "the solver certificate counts its nodes");
    assert!(plan.bound.is_some(), "the certificate carries its bound");
    assert_eq!(plan.certificate_digest.len(), 64);
    assert!(plan
        .certificate_digest
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));

    // One `Observation` premise per host profile read.
    let record = &first.records.as_slice()[0];
    let observed: Vec<&str> = record
        .premises
        .iter()
        .filter(|premise| premise.class == PremiseClass::Observation)
        .filter(|premise| premise.fact.starts_with("placement:host "))
        .map(|premise| premise.subject.as_str())
        .collect();
    assert_eq!(observed, ["host:alpha", "host:beta"]);

    // A replay of the stored record from its recorded inputs gives the recorded decision.
    let ctx = super::super::stat_executor::ExecutionContext {
        store: h.store.as_ref(),
        tenant_id: TENANT,
        now_ms: 1_000,
        server_secret: b"decide-stat-test-secret",
        matched: &[],
        designation: None,
        reader: None,
        assistant_profile: None,
        observation_windows: None,
        type_digest: None,
        recommend_catalog: None,
        defect_triage: None,
    };
    let replayed = super::super::stat_replay::replay(&ctx, record).unwrap();
    assert_eq!(replayed.outcome, record.outcome);
    assert_eq!(replayed.answer, record.answer);
}
