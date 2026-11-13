//! MEDULLA-DECISION-ENGINE-R137.8 and R137.9: a served `ContextPackage` question
//! checks each candidate against its source in the caller's graph, selects the
//! evidence package through the exact package kernel, verifies the certificate
//! independently and seals it as a verified optimum.

use super::*;
use medulla_decision_types::decision::context_package::{
    ContextPackageBudget, ContextPackageCandidate, ContextPackageRequest, RemainingBudget,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::{EvidenceClass, ResolutionKind};

const SOURCES: &str = "pkg-sources";
const FOREIGN: &str = "pkg-foreign";
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A harness whose caller can read two graphs: the one that holds the cited
/// source (`source:a`, revision 1, the package digest) and one that holds a
/// source no candidate of this tenant's source graph can see.
async fn sourced() -> Harness {
    let h = Harness::with_isolation(ServerState::test_isolation("decider")).await;
    {
        let mut guard = h.state.write().await;
        for (graph, source) in [(SOURCES, "source:a"), (FOREIGN, "source:foreign")] {
            guard
                .registry
                .create_graph(
                    graph,
                    medulla_protocol_types::protocol::GraphType::Team,
                    None,
                )
                .unwrap();
            let core = guard.registry.get(graph).unwrap().core.clone();
            let props = serde_json::json!({"revision": 1, "content_digest": DIGEST});
            core.add_node(source.to_string(), rmp_serde::to_vec_named(&props).unwrap());
        }
        // The task classes of R137.16: three measured bands and one class
        // without a `complexityBand` fact.
        let core = guard.registry.get(SOURCES).unwrap().core.clone();
        for (class, props) in [
            ("class:low", serde_json::json!({"complexityBand": "low"})),
            (
                "class:medium",
                serde_json::json!({"complexityBand": "medium"}),
            ),
            ("class:high", serde_json::json!({"complexityBand": "high"})),
            ("class:unmeasured", serde_json::json!({"label": "no band"})),
        ] {
            core.add_node(class.to_string(), rmp_serde::to_vec_named(&props).unwrap());
        }
    }
    h
}

fn candidate(
    id: &str,
    unit: &str,
    resolution: u8,
    tokens: u64,
    value: u64,
) -> ContextPackageCandidate {
    ContextPackageCandidate {
        candidate_id: id.into(),
        evidence_unit: unit.into(),
        resolution,
        source_id: "source:a".into(),
        cited_start: 0,
        cited_end: 10,
        source_revision: 1,
        content_digest: DIGEST.into(),
        tokenizer_id: "tok".into(),
        tokenizer_version: "1".into(),
        token_weight: tokens,
        value_micro: value,
        measurement_provenance: "measured".into(),
    }
}

/// Three evidence units at two or three resolutions each.
fn pool() -> Vec<ContextPackageCandidate> {
    vec![
        candidate("a0", "a", 0, 10, 100),
        candidate("a1", "a", 1, 40, 260),
        candidate("b0", "b", 0, 15, 90),
        candidate("b1", "b", 1, 50, 300),
        candidate("c0", "c", 0, 20, 120),
        candidate("c1", "c", 1, 30, 150),
        candidate("c2", "c", 2, 70, 420),
    ]
}

fn package(
    budget: ContextPackageBudget,
    candidates: Vec<ContextPackageCandidate>,
) -> ContextPackageRequest {
    ContextPackageRequest {
        budget,
        candidates: BoundedVec::new(candidates).unwrap(),
        value_floor_micro_per_token: 0,
        node_budget: 100_000,
        remaining: None,
    }
}

fn window(tokens: u64) -> ContextPackageBudget {
    ContextPackageBudget {
        window_tokens: tokens,
        reserved_output_tokens: 0,
        caller_ceiling_tokens: None,
        money_ceiling_micro: None,
        price_micro_per_token: None,
        latency_ceiling_ms: None,
        tokens_per_second: None,
    }
}

fn declared(id: &str) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::default(),
        texts: BoundedVec::default(),
    }
}

fn package_request(
    schema: &ComponentDependency,
    package: ContextPackageRequest,
    options: Vec<&str>,
) -> DecideRequest {
    DecideRequest {
        tenant_id: TENANT.to_string(),
        question: StatisticalQuestion {
            question_id: "context.package".to_string(),
            kind: QuestionKind::ContextPackage,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(options.into_iter().map(declared).collect()).unwrap(),
        },
        feature_schema: schema.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![
            TypedParam {
                name: "context_packages".to_string(),
                value: TypedValue::ContextPackages(Box::new(package)),
            },
            TypedParam {
                name: "source_graph".to_string(),
                value: TypedValue::Text(SOURCES.to_string()),
            },
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

/// Exhaustive search: the best value and its fewest tokens among selections with at
/// most one candidate per evidence unit and total tokens within the capacity.
fn oracle(candidates: &[ContextPackageCandidate], capacity: u64) -> (u64, u64) {
    let mut best = (0_u64, 0_u64);
    for mask in 0_u32..(1 << candidates.len()) {
        let picked: Vec<_> = (0..candidates.len())
            .filter(|i| mask >> i & 1 == 1)
            .map(|i| &candidates[i])
            .collect();
        let mut units = std::collections::BTreeSet::new();
        if !picked.iter().all(|c| units.insert(c.evidence_unit.clone())) {
            continue;
        }
        let tokens: u64 = picked.iter().map(|c| c.token_weight).sum();
        let value: u64 = picked.iter().map(|c| c.value_micro).sum();
        if tokens <= capacity && (value > best.0 || (value == best.0 && tokens < best.1)) {
            best = (value, tokens);
        }
    }
    best
}

// spec: MEDULLA-DECISION-ENGINE-R137.7
// spec: MEDULLA-DECISION-ENGINE-R137.8
#[tokio::test]
async fn package_journey() {
    let h = sourced().await;
    let (schema, _digest) = publish_route_schema(&h);
    let ids: Vec<&str> = vec!["a0", "a1", "b0", "b1", "c0", "c1", "c2"];

    // The served question selects the exhaustive optimum under the window, one
    // resolution per evidence unit, and seals it as a verified optimum.
    let batch = decide(
        &h,
        package_request(&schema, package(window(100), pool()), ids.clone()),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::ContextPackage);
    let StatisticalOutcome::Advisory { scores, calibrated } = &record.outcome else {
        panic!(
            "expected an Advisory outcome carrying the package: {:?}",
            record.outcome
        )
    };
    assert!(!calibrated);
    let chosen: Vec<&str> = scores.iter().map(|s| s.option_id.as_str()).collect();
    let (best_value, best_tokens) = oracle(&pool(), 100);
    let picked: Vec<_> = pool()
        .into_iter()
        .filter(|c| chosen.contains(&c.candidate_id.as_str()))
        .collect();
    assert_eq!(
        picked.iter().map(|c| c.value_micro).sum::<u64>(),
        best_value
    );
    assert_eq!(
        picked.iter().map(|c| c.token_weight).sum::<u64>(),
        best_tokens
    );
    assert_eq!(chosen, vec!["a0", "b0", "c2"]);
    assert_eq!(record.resolution_kind, ResolutionKind::Optimization);
    assert_eq!(record.evidence_class, EvidenceClass::Proof);

    // Each ceiling can be the tightest: a caller ceiling and an output reservation
    // lower the capacity below the window, and the answer follows the oracle.
    let mut caller = window(100);
    caller.caller_ceiling_tokens = Some(60);
    let mut reserved = window(100);
    reserved.reserved_output_tokens = 40;
    let mut money = window(100);
    money.money_ceiling_micro = Some(600);
    money.price_micro_per_token = Some(10);
    let mut latency = window(100);
    latency.latency_ceiling_ms = Some(1_000);
    latency.tokens_per_second = Some(60);
    for budget in [caller, reserved, money, latency] {
        let batch = decide(
            &h,
            package_request(&schema, package(budget, pool()), ids.clone()),
        )
        .await
        .unwrap();
        let StatisticalOutcome::Advisory { scores, .. } = &batch.records.as_slice()[0].outcome
        else {
            panic!("expected an Advisory outcome")
        };
        let picked: Vec<_> = pool()
            .into_iter()
            .filter(|c| scores.iter().any(|s| s.option_id == c.candidate_id))
            .collect();
        let (value, tokens) = oracle(&pool(), 60);
        assert_eq!(picked.iter().map(|c| c.value_micro).sum::<u64>(), value);
        assert_eq!(picked.iter().map(|c| c.token_weight).sum::<u64>(), tokens);
    }

    // A ceiling whose rate is missing is refused with its closed code.
    let mut missing = window(100);
    missing.money_ceiling_micro = Some(600);
    let refused = decide(
        &h,
        package_request(&schema, package(missing, pool()), ids.clone()),
    )
    .await
    .unwrap_err();
    assert!(
        refused.contains("CONTEXT_PACKAGE_MISSING_FACT"),
        "{refused}"
    );

    // Declared options must equal the package candidate ids.
    let refused = decide(
        &h,
        package_request(&schema, package(window(100), pool()), vec!["a0"]),
    )
    .await
    .unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
}

fn selected_ids(batch: &DecisionBatch) -> (Vec<String>, ResolutionKind, EvidenceClass) {
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Advisory { scores, .. } = &record.outcome else {
        panic!("expected an Advisory outcome: {:?}", record.outcome)
    };
    (
        scores.iter().map(|s| s.option_id.clone()).collect(),
        record.resolution_kind,
        record.evidence_class,
    )
}

const ALL_IDS: [&str; 7] = ["a0", "a1", "b0", "b1", "c0", "c1", "c2"];

// spec: MEDULLA-DECISION-ENGINE-R137.9
#[tokio::test]
async fn decide_routes_package_selection() {
    let h = sourced().await;
    let (schema, _digest) = publish_route_schema(&h);
    let batch = decide(
        &h,
        package_request(&schema, package(window(100), pool()), ALL_IDS.to_vec()),
    )
    .await
    .unwrap();
    let (chosen, kind, class) = selected_ids(&batch);
    assert_eq!(chosen, vec!["a0", "b0", "c2"]);
    assert_eq!(kind, ResolutionKind::Optimization);
    assert_eq!(class, EvidenceClass::Proof);
}

/// A solver whose certificate claims the optimum but names an assignment that
/// breaks the capacity row: the independent verifier must reject it.
fn corrupting_solver(
    model: &medulla_decision_policy::solve::Model,
    config: &medulla_decision_policy::solve::SolverConfig,
) -> medulla_decision_policy::solve::Certificate {
    let mut certificate = medulla_decision_policy::solve::solve(model, config);
    if let Some(incumbent) = certificate.incumbent.as_mut() {
        incumbent
            .selected
            .iter_mut()
            .for_each(|picked| *picked = true);
    }
    certificate
}

// spec: MEDULLA-DECISION-ENGINE-R137.9
#[tokio::test]
async fn corrupted_certificate_is_refused() {
    let h = sourced().await;
    let (schema, _digest) = publish_route_schema(&h);
    let mut request = package_request(&schema, package(window(100), pool()), ALL_IDS.to_vec());
    request.question.question_id = "context.package.corrupt".to_string();
    super::super::family::context_package::overrides::set(
        "context.package.corrupt",
        corrupting_solver,
    );
    let refused = decide(&h, request).await.unwrap_err();
    assert!(
        refused.contains("CONTEXT_PACKAGE_CERTIFICATE_UNACCEPTABLE"),
        "{refused}"
    );
    // The same package under the real solver is answered: only the swapped
    // solver's question is refused.
    let batch = decide(
        &h,
        package_request(&schema, package(window(100), pool()), ALL_IDS.to_vec()),
    )
    .await
    .unwrap();
    assert_eq!(selected_ids(&batch).0, vec!["a0", "b0", "c2"]);
}

// spec: MEDULLA-DECISION-ENGINE-R137.9
#[tokio::test]
async fn cross_tenant_candidate_is_not_visible() {
    let h = sourced().await;
    let (schema, _digest) = publish_route_schema(&h);
    // A candidate whose source lives only in a graph the question did not name
    // is not visible to this question: it is refused as a stale source.
    let mut foreign = pool();
    foreign[0].source_id = "source:foreign".to_string();
    let refused = decide(
        &h,
        package_request(&schema, package(window(100), foreign), ALL_IDS.to_vec()),
    )
    .await
    .unwrap_err();
    assert!(
        refused.contains("CONTEXT_PACKAGE_STALE_SOURCE"),
        "{refused}"
    );
    // A request under another tenant never reaches the source graph.
    let mut other = package_request(&schema, package(window(100), pool()), ALL_IDS.to_vec());
    other.tenant_id = "tenant-other".to_string();
    let refused = decide(&h, other).await.unwrap_err();
    assert!(refused.starts_with("ACCESS_DENIED"), "{refused}");
}

// spec: MEDULLA-DECISION-ENGINE-R137.9
#[tokio::test]
async fn request_knobs_must_equal_the_policy_values() {
    let h = sourced().await;
    let (schema, _digest) = publish_route_schema(&h);
    let mut budget = package(window(100), pool());
    budget.node_budget = 5_000;
    let refused = decide(&h, package_request(&schema, budget, ALL_IDS.to_vec()))
        .await
        .unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    assert!(refused.contains("context.solver_node_budget"), "{refused}");
    let mut floor = package(window(100), pool());
    floor.value_floor_micro_per_token = 5;
    let refused = decide(&h, package_request(&schema, floor, ALL_IDS.to_vec()))
        .await
        .unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    assert!(
        refused.contains("context.value_floor_micro_per_token"),
        "{refused}"
    );
    // A moved source revision and a changed content digest are both stale.
    let mut moved = pool();
    moved[0].source_revision = 2;
    let refused = decide(
        &h,
        package_request(&schema, package(window(100), moved), ALL_IDS.to_vec()),
    )
    .await
    .unwrap_err();
    assert!(
        refused.contains("CONTEXT_PACKAGE_STALE_SOURCE"),
        "{refused}"
    );
    let mut changed = pool();
    changed[0].content_digest = "f".repeat(64);
    let refused = decide(
        &h,
        package_request(&schema, package(window(100), changed), ALL_IDS.to_vec()),
    )
    .await
    .unwrap_err();
    assert!(
        refused.contains("CONTEXT_PACKAGE_STALE_SOURCE"),
        "{refused}"
    );
}

/// The abstention reasons of the one record of `batch`.
fn abstention(batch: &DecisionBatch) -> Vec<medulla_decision_types::decision::AbstainReason> {
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("expected an abstention: {:?}", record.outcome)
    };
    reasons.iter().cloned().collect()
}

/// The refusal text of a package question that must be refused.
async fn refused_with(h: &Harness, request: DecideRequest) -> String {
    let refused = decide(h, request).await.unwrap_err();
    assert!(refused.starts_with("PARAMETER_INVALID"), "{refused}");
    refused
}

// spec: MEDULLA-DECISION-ENGINE-R137.10
#[tokio::test]
async fn package_refusals_are_typed() {
    use medulla_decision_types::decision::context_package::{
        NONE_ADMISSIBLE, REFUSALS, WORK_EXHAUSTED,
    };
    use medulla_decision_types::decision::AbstainReason;
    let h = sourced().await;
    let (schema, _digest) = publish_route_schema(&h);
    let request = |package| package_request(&schema, package, ALL_IDS.to_vec());

    // A measurement whose sums overflow is an invalid measurement.
    let mut overflow = pool();
    overflow[0].value_micro = u64::MAX;
    let refused = refused_with(&h, request(package(window(100), overflow))).await;
    assert!(
        refused.contains("CONTEXT_PACKAGE_INVALID_MEASUREMENT"),
        "{refused}"
    );

    // A money ceiling without the price per token is a missing fact.
    let mut budget = window(100);
    budget.money_ceiling_micro = Some(50);
    let refused = refused_with(&h, request(package(budget, pool()))).await;
    assert!(
        refused.contains("CONTEXT_PACKAGE_MISSING_FACT"),
        "{refused}"
    );

    // A candidate that differs from its source is stale.
    let mut moved = pool();
    moved[0].source_revision = 2;
    let refused = refused_with(&h, request(package(window(100), moved))).await;
    assert!(
        refused.contains("CONTEXT_PACKAGE_STALE_SOURCE"),
        "{refused}"
    );

    // A certificate the independent verifier rejects is unacceptable.
    let mut corrupt = request(package(window(100), pool()));
    corrupt.question.question_id = "context.package.refusals.corrupt".to_string();
    super::super::family::context_package::overrides::set(
        "context.package.refusals.corrupt",
        corrupting_solver,
    );
    let refused = refused_with(&h, corrupt).await;
    assert!(
        refused.contains("CONTEXT_PACKAGE_CERTIFICATE_UNACCEPTABLE"),
        "{refused}"
    );

    // A certificate bound to another model is unacceptable too.
    let mut tampered = request(package(window(100), pool()));
    tampered.question.question_id = "context.package.refusals.tampered".to_string();
    super::super::family::context_package::overrides::set(
        "context.package.refusals.tampered",
        rebinding_solver,
    );
    let refused = refused_with(&h, tampered).await;
    assert!(
        refused.contains("CONTEXT_PACKAGE_CERTIFICATE_UNACCEPTABLE"),
        "{refused}"
    );

    // A zero-capacity window selects nothing and abstains the same way.
    let batch = decide(&h, request(package(window(0), pool())))
        .await
        .unwrap();
    let reasons = abstention(&batch);
    let [AbstainReason::Family { code, .. }] = reasons.as_slice() else {
        panic!("one family reason: {reasons:?}")
    };
    assert_eq!(*code, NONE_ADMISSIBLE);

    // An empty selection abstains with its registered code and certifies nothing.
    let batch = decide(&h, request(package(window(5), pool())))
        .await
        .unwrap();
    let reasons = abstention(&batch);
    let [AbstainReason::Family { family, code, .. }] = reasons.as_slice() else {
        panic!("one family reason: {reasons:?}")
    };
    assert_eq!(*family, QuestionKind::ContextPackage);
    assert_eq!(*code, NONE_ADMISSIBLE);
    assert_eq!(
        batch.records.as_slice()[0].resolution_kind,
        ResolutionKind::Abstention,
        "an abstention is never sealed as an optimization"
    );

    // Every code the question can answer with is registered in its table.
    for code in [
        "CONTEXT_PACKAGE_INVALID_MEASUREMENT",
        "CONTEXT_PACKAGE_MISSING_FACT",
        "CONTEXT_PACKAGE_STALE_SOURCE",
        "CONTEXT_PACKAGE_CERTIFICATE_UNACCEPTABLE",
    ] {
        assert!(REFUSALS
            .iter()
            .any(|registered| registered.as_str() == code));
    }
    assert!(REFUSALS.contains(&NONE_ADMISSIBLE) && REFUSALS.contains(&WORK_EXHAUSTED));
}

// spec: MEDULLA-DECISION-ENGINE-R137.10
#[tokio::test]
async fn stale_source_is_refused() {
    let h = sourced().await;
    let (schema, _digest) = publish_route_schema(&h);
    // The request is built against revision 1 and answered.
    let answered = decide(
        &h,
        package_request(&schema, package(window(100), pool()), ALL_IDS.to_vec()),
    )
    .await
    .unwrap();
    assert_eq!(selected_ids(&answered).0, vec!["a0", "b0", "c2"]);
    // The graph then moves the source to revision 2: the same request is stale.
    {
        let guard = h.state.write().await;
        let core = guard.registry.get(SOURCES).unwrap().core.clone();
        let props = serde_json::json!({"revision": 2, "content_digest": DIGEST});
        core.add_node(
            "source:a".to_string(),
            rmp_serde::to_vec_named(&props).unwrap(),
        );
    }
    let refused = refused_with(
        &h,
        package_request(&schema, package(window(100), pool()), ALL_IDS.to_vec()),
    )
    .await;
    assert!(
        refused.contains("CONTEXT_PACKAGE_STALE_SOURCE"),
        "{refused}"
    );
}

/// Near-tied candidates over twenty evidence units: the proof needs a large tree.
fn hard_pool() -> Vec<ContextPackageCandidate> {
    let mut out = Vec::new();
    for unit in 0..20u64 {
        for resolution in 0..3u64 {
            let tokens = 20 + (unit * 37 + resolution * 53) % 41 + resolution * 13;
            let value = tokens * 100 + (unit * 17 + resolution * 29) % 23;
            out.push(candidate(
                &format!("u{unit:02}r{resolution}"),
                &format!("u{unit:02}"),
                resolution as u8,
                tokens,
                value,
            ));
        }
    }
    out
}

/// The real solver under the least power-of-two node budget whose exhausted certificate
/// has (or lacks) an incumbent.
fn exhausted_certificate(
    model: &medulla_decision_policy::solve::Model,
    config: &medulla_decision_policy::solve::SolverConfig,
    with_incumbent: bool,
) -> medulla_decision_policy::solve::Certificate {
    use medulla_decision_policy::solve::certificate::DEFAULT_CERTIFICATE_LEAVES;
    use medulla_decision_policy::solve::{
        solve, Scalar, SolveStatus, SolverConfig, SolverConfigSpec,
    };
    let full = solve(model, config);
    let mut budget = 1_u64;
    while budget < full.nodes_expanded {
        let tight = SolverConfig::try_from(SolverConfigSpec {
            node_budget: budget,
            max_certificate_leaves: DEFAULT_CERTIFICATE_LEAVES,
            bound_denominator: 1,
            accepted_gap: Scalar::new(0),
        })
        .expect("a positive node budget");
        let certificate = solve(model, &tight);
        if matches!(certificate.status, SolveStatus::BudgetExhausted)
            && certificate.incumbent.is_some() == with_incumbent
        {
            return certificate;
        }
        budget *= 2;
    }
    panic!(
        "the hard pool is proven in {} nodes: no budget leaves an exhausted search with incumbent = {with_incumbent}",
        full.nodes_expanded
    )
}

fn exhausting_with_incumbent(
    model: &medulla_decision_policy::solve::Model,
    config: &medulla_decision_policy::solve::SolverConfig,
) -> medulla_decision_policy::solve::Certificate {
    exhausted_certificate(model, config, true)
}

fn exhausting_without_incumbent(
    model: &medulla_decision_policy::solve::Model,
    config: &medulla_decision_policy::solve::SolverConfig,
) -> medulla_decision_policy::solve::Certificate {
    exhausted_certificate(model, config, false)
}

/// A solver whose certificate is bound to another model: the verifier must reject it.
fn rebinding_solver(
    model: &medulla_decision_policy::solve::Model,
    config: &medulla_decision_policy::solve::SolverConfig,
) -> medulla_decision_policy::solve::Certificate {
    let mut certificate = medulla_decision_policy::solve::solve(model, config);
    certificate.model_digest =
        medulla_decision_policy::solve::Sha256Digest::of_json("test/other-model", &"other");
    certificate
}

/// The abstention of one exhausted hard-pool question under `solver`.
async fn exhausted_reasons(
    question_id: &str,
    solver: super::super::family::context_package::overrides::SolverFn,
) -> (
    Vec<medulla_decision_types::decision::AbstainReason>,
    ResolutionKind,
) {
    let h = sourced().await;
    let (schema, _digest) = publish_route_schema(&h);
    let ids: Vec<String> = hard_pool().into_iter().map(|c| c.candidate_id).collect();
    let mut request = package_request(
        &schema,
        package(window(300), hard_pool()),
        ids.iter().map(String::as_str).collect(),
    );
    request.question.question_id = question_id.to_string();
    super::super::family::context_package::overrides::set(question_id, solver);
    let batch = decide(&h, request).await.unwrap();
    (
        abstention(&batch),
        batch.records.as_slice()[0].resolution_kind,
    )
}

// spec: MEDULLA-DECISION-ENGINE-R137.10
#[tokio::test]
async fn exhausted_budget_abstains_with_incumbent() {
    use medulla_decision_types::decision::context_package::WORK_EXHAUSTED;
    use medulla_decision_types::decision::AbstainReason;

    // The budget runs out with an incumbent: both it and the bound are reported.
    let (reasons, kind) =
        exhausted_reasons("context.package.exhausted", exhausting_with_incumbent).await;
    let [AbstainReason::Family { code, .. }, AbstainReason::BudgetExhausted {
        incumbent,
        lower_bound,
    }] = reasons.as_slice()
    else {
        panic!("a family reason then the exhausted budget: {reasons:?}")
    };
    assert_eq!(*code, WORK_EXHAUSTED);
    let incumbent: i128 = incumbent
        .as_ref()
        .expect("the verified incumbent is reported")
        .parse()
        .expect("a canonical scalar");
    assert!(
        lower_bound.get() <= incumbent,
        "the bound {} is below the incumbent {incumbent}",
        lower_bound.get()
    );
    assert_eq!(kind, ResolutionKind::Abstention, "no package is certified");

    // The budget runs out before any package is found: no incumbent is invented.
    let (reasons, kind) = exhausted_reasons(
        "context.package.exhausted.bare",
        exhausting_without_incumbent,
    )
    .await;
    let [AbstainReason::Family { code, .. }, AbstainReason::BudgetExhausted { incumbent, .. }] =
        reasons.as_slice()
    else {
        panic!("a family reason then the exhausted budget: {reasons:?}")
    };
    assert_eq!(*code, WORK_EXHAUSTED);
    assert_eq!(*incumbent, None);
    assert_eq!(kind, ResolutionKind::Abstention);
}

/// The value of the `context_package.<name>=` premise a record sealed.
fn premise_value(batch: &DecisionBatch, name: &str) -> String {
    let record = &batch.records.as_slice()[0];
    let prefix = format!("context_package.{name}=");
    record
        .premises
        .iter()
        .find_map(|premise| premise.fact.strip_prefix(&prefix).map(str::to_string))
        .unwrap_or_else(|| panic!("no context_package.{name} premise: {:?}", record.premises))
}

async fn evidence_of(
    h: &Harness,
    schema: &ComponentDependency,
    package: ContextPackageRequest,
) -> DecisionBatch {
    decide(h, package_request(schema, package, ALL_IDS.to_vec()))
        .await
        .unwrap()
}

// spec: MEDULLA-DECISION-ENGINE-R137.11
#[tokio::test]
async fn package_evidence_binds_inputs() {
    use medulla_decision_types::decision::PremiseClass;

    let h = sourced().await;
    let (schema, _digest) = publish_route_schema(&h);
    let base = evidence_of(&h, &schema, package(window(100), pool())).await;
    let record = &base.records.as_slice()[0];

    // Every named premise is present, with the class its source earns.
    for name in [
        "universe_digest",
        "tokenizer",
        "budget_digest",
        "capacity_tokens",
        "objective",
        "solver_version",
        "model_digest",
        "certificate_digest",
        "certificate_status",
        "total_tokens",
        "total_cost_micro",
        "total_latency_ms",
    ] {
        premise_value(&base, name);
    }
    let class_of = |name: &str| {
        record
            .premises
            .iter()
            .find(|p| p.fact.starts_with(&format!("context_package.{name}=")))
            .unwrap()
            .class
    };
    assert_eq!(class_of("certificate_digest"), PremiseClass::Proof);
    assert_eq!(class_of("certificate_status"), PremiseClass::Proof);
    assert_eq!(class_of("total_tokens"), PremiseClass::Proof);
    assert_eq!(class_of("universe_digest"), PremiseClass::Claim);
    assert_eq!(class_of("budget_digest"), PremiseClass::Claim);
    assert_eq!(
        premise_value(&base, "certificate_status"),
        "verified_optimal"
    );

    // A candidate the answer did not choose still moves the universe digest and
    // the inputs digest: the universe covers all candidates, chosen or not.
    let mut unchosen = pool();
    unchosen[1].cited_end = 11; // a1 is not chosen
    let changed_range = evidence_of(&h, &schema, package(window(100), unchosen)).await;
    assert_eq!(selected_ids(&changed_range).0, selected_ids(&base).0);
    assert_ne!(
        premise_value(&changed_range, "universe_digest"),
        premise_value(&base, "universe_digest")
    );
    assert_ne!(
        changed_range.records.as_slice()[0].inputs_digest,
        record.inputs_digest
    );

    // A changed budget changes the budget digest, the capacity and the inputs digest.
    let changed_budget = evidence_of(&h, &schema, package(window(60), pool())).await;
    assert_ne!(
        premise_value(&changed_budget, "budget_digest"),
        premise_value(&base, "budget_digest")
    );
    assert_eq!(premise_value(&changed_budget, "capacity_tokens"), "60");
    assert_ne!(
        changed_budget.records.as_slice()[0].inputs_digest,
        record.inputs_digest
    );

    // A changed candidate measurement changes the inputs digest and, through
    // the model, the model and certificate digests.
    let mut measured = pool();
    measured[5].value_micro = 151; // c1 is not chosen
    let changed_value = evidence_of(&h, &schema, package(window(100), measured)).await;
    assert_ne!(
        changed_value.records.as_slice()[0].inputs_digest,
        record.inputs_digest
    );
    assert_ne!(
        premise_value(&changed_value, "model_digest"),
        premise_value(&base, "model_digest")
    );

    // A changed source (a new revision and content digest, cited by the
    // request) changes the universe digest and the inputs digest.
    let other_digest = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
    {
        let guard = h.state.write().await;
        let core = guard.registry.get(SOURCES).unwrap().core.clone();
        let props = serde_json::json!({"revision": 2, "content_digest": other_digest});
        core.add_node(
            "source:a".to_string(),
            rmp_serde::to_vec_named(&props).unwrap(),
        );
    }
    let mut moved = pool();
    for candidate in &mut moved {
        candidate.source_revision = 2;
        candidate.content_digest = other_digest.to_string();
    }
    let changed_source = evidence_of(&h, &schema, package(window(100), moved)).await;
    assert_ne!(
        premise_value(&changed_source, "universe_digest"),
        premise_value(&base, "universe_digest")
    );
    assert_ne!(
        changed_source.records.as_slice()[0].inputs_digest,
        record.inputs_digest
    );
}

// spec: MEDULLA-DECISION-ENGINE-R137.11
#[tokio::test]
async fn totals_are_recomputed_not_trusted() {
    let h = sourced().await;
    let (schema, _digest) = publish_route_schema(&h);

    // With both rates stated, each total is recomputed from the chosen candidates.
    let mut budget = window(100);
    budget.money_ceiling_micro = Some(1_000);
    budget.price_micro_per_token = Some(10);
    budget.latency_ceiling_ms = Some(2_000);
    budget.tokens_per_second = Some(60);
    let priced = evidence_of(&h, &schema, package(budget, pool())).await;
    let (chosen, _, _) = selected_ids(&priced);
    let tokens: u64 = pool()
        .iter()
        .filter(|c| chosen.contains(&c.candidate_id))
        .map(|c| c.token_weight)
        .sum();
    assert_eq!(tokens, 95);
    assert_eq!(premise_value(&priced, "total_tokens"), tokens.to_string());
    assert_eq!(
        premise_value(&priced, "total_cost_micro"),
        (tokens * 10).to_string()
    );
    assert_eq!(
        premise_value(&priced, "total_latency_ms"),
        (tokens * 1_000 / 60).to_string()
    );

    // With no rate stated the unused rate is unknown, never zero or a guess;
    // the request carries no total the family could have trusted.
    let unpriced = evidence_of(&h, &schema, package(window(100), pool())).await;
    assert_eq!(premise_value(&unpriced, "total_tokens"), "95");
    assert_eq!(premise_value(&unpriced, "total_cost_micro"), "unknown");
    assert_eq!(premise_value(&unpriced, "total_latency_ms"), "unknown");
}

fn left(ceiling: u64, committed: u64, reserved: u64, tokenizer: &str) -> RemainingBudget {
    RemainingBudget {
        ceiling_tokens: ceiling,
        committed_tokens: committed,
        reserved_tokens: reserved,
        tokenizer_id: tokenizer.to_string(),
        tokenizer_version: "1".to_string(),
    }
}

/// A package question over the pool bounded by `remaining` and, when given,
/// the complexity band of `class`.
fn bounded_request(
    schema: &ComponentDependency,
    remaining: RemainingBudget,
    class: Option<&str>,
    caller_ceiling: Option<u64>,
) -> DecideRequest {
    let mut budget = window(1_000);
    budget.caller_ceiling_tokens = caller_ceiling;
    let mut package = package(budget, pool());
    package.remaining = Some(remaining);
    let mut request = package_request(schema, package, ALL_IDS.to_vec());
    if let Some(class) = class {
        let mut params: Vec<TypedParam> = request.params.iter().cloned().collect();
        params.push(TypedParam {
            name: "task_class".to_string(),
            value: TypedValue::Iri(class.to_string()),
        });
        request.params = BoundedVec::new(params).unwrap();
    }
    request
}

/// Serve `request` and assert its package equals the exhaustive optimum under
/// `capacity`; returns the chosen ids.
async fn chosen_within(h: &Harness, request: DecideRequest, capacity: u64) -> Vec<String> {
    let batch = decide(h, request).await.unwrap();
    let (chosen, _, _) = selected_ids(&batch);
    let pool = pool();
    let picked: Vec<&ContextPackageCandidate> = pool
        .iter()
        .filter(|c| chosen.contains(&c.candidate_id))
        .collect();
    let tokens: u64 = picked.iter().map(|c| c.token_weight).sum();
    let value: u64 = picked.iter().map(|c| c.value_micro).sum();
    assert!(
        tokens <= capacity,
        "{chosen:?} takes {tokens} of {capacity}"
    );
    assert_eq!((value, tokens), oracle(&pool, capacity), "{chosen:?}");
    chosen
}

// spec: MEDULLA-DECISION-ENGINE-R137.16
#[tokio::test]
async fn complexity_changes_the_package_at_fixed_remaining() {
    let h = sourced().await;
    let (schema, _digest) = publish_route_schema(&h);
    // remaining = 300 - 100 - 0 = 200 tokens in every case.
    let mut packages = Vec::new();
    for (class, capacity) in [
        ("class:low", 50),
        ("class:medium", 100),
        ("class:high", 200),
    ] {
        let request = bounded_request(&schema, left(300, 100, 0, "tok"), Some(class), None);
        packages.push(chosen_within(&h, request, capacity).await);
    }
    assert_ne!(packages[0], packages[1]);
    assert_ne!(packages[1], packages[2]);

    let batch = decide(
        &h,
        bounded_request(&schema, left(300, 100, 0, "tok"), Some("class:low"), None),
    )
    .await
    .unwrap();
    assert_eq!(premise_value(&batch, "remaining_tokens"), "200");
    assert_eq!(premise_value(&batch, "complexity_band"), "low");
    assert_eq!(premise_value(&batch, "band_fraction"), "0.250000");
}

// spec: MEDULLA-DECISION-ENGINE-R137.16
#[tokio::test]
async fn remaining_changes_the_package_at_fixed_complexity() {
    let h = sourced().await;
    let (schema, _digest) = publish_route_schema(&h);
    let small = chosen_within(
        &h,
        bounded_request(&schema, left(100, 50, 0, "tok"), Some("class:high"), None),
        50,
    )
    .await;
    let large = chosen_within(
        &h,
        bounded_request(&schema, left(300, 100, 0, "tok"), Some("class:high"), None),
        200,
    )
    .await;
    assert_ne!(small, large);
    // A caller ceiling below the band ceiling still wins by minimum.
    chosen_within(
        &h,
        bounded_request(
            &schema,
            left(300, 100, 0, "tok"),
            Some("class:high"),
            Some(30),
        ),
        30,
    )
    .await;
    // A remaining budget without a task class bounds by the remainder itself.
    chosen_within(
        &h,
        bounded_request(&schema, left(300, 50, 50, "tok"), None, None),
        200,
    )
    .await;
}

// spec: MEDULLA-DECISION-ENGINE-R137.16
#[tokio::test]
async fn negative_remainder_and_tokenizer_mismatch_refuse() {
    let h = sourced().await;
    let (schema, _digest) = publish_route_schema(&h);
    let negative = refused_with(
        &h,
        bounded_request(
            &schema,
            left(300, 250, 100, "tok"),
            Some("class:high"),
            None,
        ),
    )
    .await;
    assert!(
        negative.contains("CONTEXT_PACKAGE_REMAINDER_NEGATIVE"),
        "{negative}"
    );
    let mismatch = refused_with(
        &h,
        bounded_request(&schema, left(300, 0, 0, "other"), Some("class:high"), None),
    )
    .await;
    assert!(
        mismatch.contains("CONTEXT_PACKAGE_TOKENIZER_MISMATCH"),
        "{mismatch}"
    );

    // A task class with no measured band abstains naming the missing fact.
    let batch = decide(
        &h,
        bounded_request(
            &schema,
            left(300, 0, 0, "tok"),
            Some("class:unmeasured"),
            None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        abstention(&batch),
        vec![
            medulla_decision_types::decision::AbstainReason::UnknownFact {
                component_id: "class:unmeasured".to_string(),
                field: "complexityBand".to_string(),
            }
        ]
    );
}
