//! MEDULLA-DECISION-ENGINE-R138.3 and R138.4: a served `probe_verdict` question
//! judges one attack from its scorers' results. The attack, transform and
//! scorer components come from the caller's tenant library, the declared
//! candidates are the scorer results, and the sealed answer is advisory with
//! three labelled options of which the chosen one scores 1.0.

use super::*;
use medulla_decision_types::decision::statistical::declared::{DeclaredNumber, DeclaredOption};
use medulla_decision_types::decision::{codes, AbstainReason, ResolutionKind};

const OTHER: &str = "tenant-other";

fn publish(h: &Harness, tenant: &str, id: &str, kind: AgentComponentKind, attrs: &[(&str, &str)]) {
    let mut draft = test_component_draft(tenant, id);
    draft.kind = kind;
    draft.attributes = attrs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    h.commit_in(tenant, draft, None).unwrap();
}

/// One attack, one transform and three scorers; `scorer-other` has another
/// category than the attack, so it is never selected.
fn publish_red_team(h: &Harness) {
    use AgentComponentKind as K;
    publish(
        h,
        TENANT,
        "attack-1",
        K::RedTeamAttack,
        &[("category", "injection"), ("goal", "leak")],
    );
    publish(
        h,
        TENANT,
        "tx-1",
        K::RedTeamTransform,
        &[("kind", "base64")],
    );
    for (id, category, threshold) in [
        ("scorer-a", "injection", "700"),
        ("scorer-b", "injection", "500"),
        ("scorer-other", "jailbreak", "100"),
    ] {
        publish(
            h,
            TENANT,
            id,
            K::RedTeamScorer,
            &[("category", category), ("threshold", threshold)],
        );
    }
}

/// Per mille as a Q32 fraction of 1.0, rounded to nearest.
fn q32_of(permille: i64) -> i64 {
    (permille * (1i64 << 32) + 500) / 1000
}

fn result(id: &str, permille: Option<i64>) -> DeclaredOption {
    DeclaredOption {
        option_id: id.to_string(),
        classification: BoundedVec::default(),
        numbers: BoundedVec::new(
            permille
                .map(|p| DeclaredNumber {
                    key: "score_permille".to_string(),
                    q32: q32_of(p),
                })
                .into_iter()
                .collect(),
        )
        .unwrap(),
        texts: BoundedVec::default(),
    }
}

fn probe_request(
    tenant: &str,
    pin: &ComponentDependency,
    results: Vec<DeclaredOption>,
) -> DecideRequest {
    DecideRequest {
        tenant_id: tenant.to_string(),
        question: StatisticalQuestion {
            question_id: "probe.test".into(),
            kind: QuestionKind::ProbeVerdict,
            safety: QuestionSafety::Ordinary,
        },
        candidates: CandidateSource::Declared {
            options: BoundedVec::new(results).unwrap(),
        },
        feature_schema: pin.clone(),
        head: None,
        policy: DecisionPolicyRef::Default,
        params: BoundedVec::new(vec![
            TypedParam {
                name: "attack_id".into(),
                value: TypedValue::Iri("attack-1".into()),
            },
            TypedParam {
                name: "transform_ids".into(),
                value: TypedValue::IriList(BoundedVec::new(vec!["tx-1".to_string()]).unwrap()),
            },
        ])
        .unwrap(),
        max_records: None,
        belief_as_of: BoundedVec::default(),
    }
}

/// The label scoring 1.0, after asserting the other two score 0.
fn chosen(record: &StatisticalDecisionRecord) -> String {
    let StatisticalOutcome::Advisory { scores, calibrated } = &record.outcome else {
        panic!("an advisory outcome, got {:?}", record.outcome);
    };
    assert!(!calibrated);
    assert_eq!(
        scores
            .iter()
            .map(|s| s.option_id.as_str())
            .collect::<Vec<_>>(),
        ["resisted", "compromised", "inconclusive"]
    );
    let one: Vec<_> = scores
        .iter()
        .filter(|s| s.score.value == 1i64 << 32)
        .collect();
    assert_eq!(one.len(), 1);
    assert!(scores
        .iter()
        .filter(|s| s.score.value != 1i64 << 32)
        .all(|s| s.score.value == 0));
    one[0].option_id.clone()
}

// spec: MEDULLA-DECISION-ENGINE-R138.3
#[tokio::test]
async fn decide_routes_probe_verdict() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    publish_red_team(&h);
    // A passing scorer compromises the probe.
    let batch = decide(
        &h,
        probe_request(
            TENANT,
            &pin,
            vec![result("scorer-a", Some(800)), result("scorer-b", Some(100))],
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(chosen(record), "compromised");
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    // Every scorer failing resists.
    let batch = decide(
        &h,
        probe_request(
            TENANT,
            &pin,
            vec![result("scorer-a", Some(100)), result("scorer-b", Some(100))],
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(chosen(record), "resisted");
    assert_eq!(record.resolution_kind, ResolutionKind::Constraint);
    assert_eq!(
        medulla_decision_types::decision::digest::statistical_record_digest(record),
        record.record_digest
    );
    // Tenant B holds the attack but none of tenant A's scorers: it sees no
    // scorer, and A's scorers never judge its probe.
    let other_pin = publish_route_schema_in(&h, OTHER);
    publish(
        &h,
        OTHER,
        "attack-1",
        AgentComponentKind::RedTeamAttack,
        &[("category", "injection"), ("goal", "leak")],
    );
    let ctx = VerifiedRequestContext::verified_for_test_in_tenant("decider", OTHER);
    let batch: DecisionBatch = decode(
        super::super::statistical::handle_decide(
            &h.state,
            1,
            &ctx,
            probe_request(OTHER, &other_pin, vec![result("scorer-a", Some(800))]),
        )
        .await,
    )
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.tenant_id, OTHER);
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("tenant B must abstain, got {:?}", record.outcome);
    };
    assert!(matches!(
        reasons.as_slice(),
        [AbstainReason::Family { code, .. }] if *code == codes::NO_SCORER_FOR_CATEGORY
    ));
}

// spec: MEDULLA-DECISION-ENGINE-R138.4
#[tokio::test]
async fn probe_verdict_refusals() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    publish_red_team(&h);
    // An attack no one published.
    let mut request = probe_request(TENANT, &pin, vec![result("scorer-a", Some(800))]);
    let mut params = request.params.as_slice().to_vec();
    params[0].value = TypedValue::Iri("attack-missing".into());
    request.params = BoundedVec::new(params).unwrap();
    let error = decide(&h, request).await.unwrap_err();
    assert!(error.contains("PROBE_ATTACK_UNKNOWN"), "{error}");
    // A result naming a scorer that is not selected.
    let error = decide(
        &h,
        probe_request(TENANT, &pin, vec![result("scorer-other", Some(800))]),
    )
    .await
    .unwrap_err();
    assert!(error.contains("PROBE_RESULT_SCORER_UNKNOWN"), "{error}");
    // A score above 1000 per mille.
    let error = decide(
        &h,
        probe_request(TENANT, &pin, vec![result("scorer-a", Some(1001))]),
    )
    .await
    .unwrap_err();
    assert!(error.contains("PROBE_SCORE_OUT_OF_RANGE"), "{error}");
    // A selected scorer with no score is inconclusive, never resisted.
    let batch = decide(
        &h,
        probe_request(
            TENANT,
            &pin,
            vec![result("scorer-a", Some(100)), result("scorer-b", None)],
        ),
    )
    .await
    .unwrap();
    assert_eq!(chosen(&batch.records.as_slice()[0]), "inconclusive");
    // No scorer of the attack's category abstains through the family table.
    let h2 = Harness::new().await;
    let (pin2, _) = publish_route_schema(&h2);
    publish(
        &h2,
        TENANT,
        "attack-1",
        AgentComponentKind::RedTeamAttack,
        &[("category", "injection"), ("goal", "leak")],
    );
    let batch = decide(
        &h2,
        probe_request(TENANT, &pin2, vec![result("scorer-a", Some(800))]),
    )
    .await
    .unwrap();
    let StatisticalOutcome::Abstained { reasons } = &batch.records.as_slice()[0].outcome else {
        panic!("an abstention");
    };
    assert!(matches!(
        reasons.as_slice(),
        [AbstainReason::Family { code, detail, .. }]
            if *code == codes::NO_SCORER_FOR_CATEGORY && detail.contains("injection")
    ));
    assert_eq!(
        batch.records.as_slice()[0].resolution_kind,
        ResolutionKind::Abstention
    );
}

// spec: MEDULLA-DECISION-ENGINE-R138.4
#[tokio::test]
async fn missing_scorer_abstains() {
    let h = Harness::new().await;
    let (pin, _) = publish_route_schema(&h);
    publish(
        &h,
        TENANT,
        "attack-1",
        AgentComponentKind::RedTeamAttack,
        &[("category", "injection"), ("goal", "leak")],
    );
    let batch = decide(
        &h,
        probe_request(TENANT, &pin, vec![result("scorer-a", None)]),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("an abstention");
    };
    assert!(matches!(
        reasons.as_slice(),
        [AbstainReason::Family { code, .. }] if *code == codes::NO_SCORER_FOR_CATEGORY
    ));
}
