//! MEDULLA-DECISION-ENGINE-R137.8: a served `ContextPackage` question selects the
//! evidence package through the exact package kernel and seals it as a verified
//! optimum.

use super::*;
use medulla_decision_types::decision::context_package::{
    ContextPackageBudget, ContextPackageCandidate, ContextPackageRequest,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::{EvidenceClass, ResolutionKind};

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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
        params: BoundedVec::new(vec![TypedParam {
            name: "context_packages".to_string(),
            value: TypedValue::ContextPackages(Box::new(package)),
        }])
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
    let h = Harness::new().await;
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
