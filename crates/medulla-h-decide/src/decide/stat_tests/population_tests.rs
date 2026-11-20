//! Population drift and fan-out width, served through `Decide`
//! (MEDULLA-DECISION-ENGINE-R752.751, R752.754): the first Wasserstein distance
//! between two windows with its alert, and the replica width a fan-out should
//! use.

use super::kernel_family_support::{
    decide_as_other_tenant, facts, kernel_request, persisted_files,
};
use super::*;
use medulla_decision_types::decision::population::{
    DriftRequest, DriftWindow, FanoutRequest, PopulationSpread,
};
use medulla_decision_types::decision::ResolutionKind;

const ONE: i64 = 1 << 32;

fn q(value: i64) -> QuantisedValue {
    QuantisedValue {
        scale: medulla_decision_types::decision::QuantScaleTag::Q32,
        value,
    }
}

fn window(values: &[i64]) -> DriftWindow {
    DriftWindow::new(values.iter().map(|v| q(*v)).collect()).unwrap()
}

fn drift(schema: &ComponentDependency, reference: &[i64], current: &[i64]) -> DecideRequest {
    kernel_request(
        schema,
        QuestionKind::PopulationDrift,
        "population_drift",
        TypedValue::PopulationDrift(Box::new(DriftRequest {
            reference: window(reference),
            current: window(current),
        })),
        &["scores"],
    )
}

fn fanout(
    schema: &ComponentDependency,
    diversity: i64,
    current_width: u32,
    budget: u64,
) -> DecideRequest {
    kernel_request(
        schema,
        QuestionKind::FanoutWidth,
        "fanout_width",
        TypedValue::FanoutWidth(Box::new(FanoutRequest {
            spread: PopulationSpread::new(4, vec![q(diversity); 6]).unwrap(),
            current_width,
            budget,
            cost_per_replica: 10,
        })),
        &["replicas"],
    )
}

/// Verifies MEDULLA-DECISION-ENGINE-R752.751 and R752.754: the served read
/// returns the distance and the alert, and the served width follows the
/// diversity and never exceeds the cap.
// spec: MEDULLA-DECISION-ENGINE-R752.751
// spec: MEDULLA-DECISION-ENGINE-R752.754
#[tokio::test]
async fn served_decide_reads_drift_and_picks_width() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    // Drift: the golden vector of the kernel test, read through the router.
    let batch = decide(
        &h,
        drift(
            &schema,
            &[429496729, 858993459, 1288490188, 1717986918],
            &[
                1288490188, 1717986918, 2147483648, 2576980377, 3006477107, 3435973836,
            ],
        ),
    )
    .await
    .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(record.question.kind, QuestionKind::PopulationDrift);
    let reading = record
        .answer
        .as_ref()
        .and_then(|a| a.population_drift())
        .expect("the family answers a drift reading");
    assert_eq!(reading.distance.value.value, 1216907401);
    assert!(reading.alert);
    assert_eq!((reading.reference_n, reading.current_n), (4, 6));
    assert!(matches!(
        record.outcome,
        StatisticalOutcome::Advisory { .. }
    ));
    assert_eq!(record.resolution_kind, ResolutionKind::Statistical);
    assert_eq!(record.evidence_class, EvidenceClass::Claim);
    // Identical windows do not alert.
    let calm = decide(&h, drift(&schema, &[1, 2, 3], &[3, 2, 1]))
        .await
        .unwrap();
    let calm = calm.records.as_slice()[0]
        .answer
        .as_ref()
        .and_then(|a| a.population_drift())
        .unwrap();
    assert_eq!(calm.distance.value.value, 0);
    assert!(!calm.alert);

    // Width: high diversity grows, low shrinks, and the budget caps it.
    let width = |batch: &DecisionBatch| {
        batch.records.as_slice()[0]
            .answer
            .as_ref()
            .and_then(|a| a.fanout_width())
            .expect("the family answers a width")
    };
    let grown = decide(&h, fanout(&schema, ONE / 2, 3, 100)).await.unwrap();
    assert_eq!(width(&grown), 4);
    assert_eq!(
        grown.records.as_slice()[0].question.kind,
        QuestionKind::FanoutWidth
    );
    let shrunk = decide(&h, fanout(&schema, ONE / 10, 3, 100)).await.unwrap();
    assert_eq!(width(&shrunk), 2);
    let held = decide(&h, fanout(&schema, ONE / 4, 3, 100)).await.unwrap();
    assert_eq!(width(&held), 3);
    let capped = decide(&h, fanout(&schema, ONE / 2, 5, 40)).await.unwrap();
    assert_eq!(
        width(&capped),
        4,
        "a budget of 40 at cost 10 funds 4 replicas"
    );
    let ceiling = decide(&h, fanout(&schema, ONE / 2, 8, 1000)).await.unwrap();
    assert_eq!(
        width(&ceiling),
        8,
        "knob fanout.replica_ceiling caps the width"
    );
    let facts = facts(&grown.records.as_slice()[0]);
    for expected in [
        format!("fanout:mean_diversity={}", ONE / 2),
        "fanout:width=4".to_string(),
        "knob:fanout.diversity_high=0.400000".to_string(),
        "knob:fanout.replica_ceiling=8".to_string(),
    ] {
        assert!(
            facts.contains(&expected.as_str()),
            "missing premise {expected}"
        );
    }
    // A budget that cannot fund one replica abstains (FANOUT_BUDGET_TOO_LOW).
    let record = decide(&h, fanout(&schema, ONE / 2, 3, 9))
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone();
    assert_eq!(abstention_code(&record), "FANOUT_BUDGET_TOO_LOW");
}

/// Verifies MEDULLA-DECISION-ENGINE-R752.754: drift premises name the distance
/// and window sizes; a window beyond `knob drift.max_samples` is refused; a
/// request for another tenant is refused before anything is read.
// spec: MEDULLA-DECISION-ENGINE-R752.754
#[tokio::test]
async fn other_tenant_inputs_are_not_read() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let batch = decide(&h, drift(&schema, &[0, 1, 2], &[0, 1]))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let facts = facts(record);
    assert!(facts.iter().any(|f| f.starts_with("drift:distance=")));
    assert!(facts.contains(&"drift:windows=3/2"));
    assert!(facts.contains(&"knob:drift.alert=0.100000"));

    let refused = decide_as_other_tenant(&h, drift(&schema, &[0, 1], &[0, 1])).await;
    assert!(refused.starts_with("ACCESS_DENIED"), "{refused}");
    let refused = decide_as_other_tenant(&h, fanout(&schema, ONE / 2, 3, 100)).await;
    assert!(refused.starts_with("ACCESS_DENIED"), "{refused}");

    // The knob lowers the window bound: 16 is the minimum, so 17 samples fit
    // the default and fail a pinned 16.
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.knobs.insert(
        medulla_decision_types::decision::knobs::KnobName::new("drift.max_samples"),
        medulla_decision_types::decision::knobs::KnobValue::Integer(16),
    );
    let pin = h.publish_policy("policy-drift", &policy);
    let samples: Vec<i64> = (0..17).collect();
    let mut request = drift(&schema, &samples, &[0, 1]);
    request.policy = DecisionPolicyRef::Pinned { component: pin };
    let error = decide(&h, request).await.unwrap_err();
    assert!(error.starts_with("DRIFT_WINDOW_TOO_LARGE"), "{error}");
}

fn abstention_code(record: &StatisticalDecisionRecord) -> String {
    assert_eq!(record.resolution_kind, ResolutionKind::Abstention);
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("expected an abstention, got {:?}", record.outcome);
    };
    reasons.as_slice()[0].code().as_str().to_string()
}

/// Verifies MEDULLA-DECISION-ENGINE-R752.752 (population half): the empty
/// window and the unfundable budget abstain, the oversized window and the
/// invalid spread are refused, each with its registered code, and nothing is
/// written.
// spec: MEDULLA-DECISION-ENGINE-R752.752
#[tokio::test]
async fn population_refusals() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let dir = std::path::PathBuf::from(h.state.read().await.persist_dir.clone().unwrap());
    decide(&h, drift(&schema, &[0, 1], &[0, 1])).await.unwrap();
    let before = persisted_files(&dir);

    // DRIFT_WINDOW_TOO_LARGE under a pinned bound of 16 samples.
    let mut policy = medulla_decision_types::decision::DecisionPolicy::engine_default();
    policy.knobs.insert(
        medulla_decision_types::decision::knobs::KnobName::new("drift.max_samples"),
        medulla_decision_types::decision::knobs::KnobValue::Integer(16),
    );
    let pin = h.publish_policy("policy-population", &policy);
    let samples: Vec<i64> = (0..17).collect();
    let mut large = drift(&schema, &[0, 1], &samples);
    large.policy = DecisionPolicyRef::Pinned { component: pin };
    let error = decide(&h, large).await.unwrap_err();
    assert!(error.starts_with("DRIFT_WINDOW_TOO_LARGE"), "{error}");

    // POPULATION_SPREAD_INVALID: a wrong pairwise count and an entry outside [0, 1].
    for spread in [
        PopulationSpread {
            replicas: 4,
            pairwise: BoundedVec::new(vec![q(ONE / 2); 5]).unwrap(),
        },
        PopulationSpread {
            replicas: 4,
            pairwise: BoundedVec::new(vec![q(ONE + 1); 6]).unwrap(),
        },
    ] {
        let request = kernel_request(
            &schema,
            QuestionKind::FanoutWidth,
            "fanout_width",
            TypedValue::FanoutWidth(Box::new(FanoutRequest {
                spread,
                current_width: 3,
                budget: 100,
                cost_per_replica: 10,
            })),
            &["replicas"],
        );
        let error = decide(&h, request).await.unwrap_err();
        assert!(error.starts_with("POPULATION_SPREAD_INVALID"), "{error}");
    }

    assert_eq!(persisted_files(&dir), before, "a refusal writes nothing");

    // DRIFT_EMPTY_WINDOW: either window, an abstention and never a zero distance.
    for (reference, current) in [(&[][..], &[1][..]), (&[1][..], &[][..])] {
        let record = decide(&h, drift(&schema, reference, current))
            .await
            .unwrap()
            .records
            .as_slice()[0]
            .clone();
        assert_eq!(abstention_code(&record), "DRIFT_EMPTY_WINDOW");
        assert!(record.answer.is_none());
    }

    // FANOUT_BUDGET_TOO_LOW: an abstention.
    let record = decide(&h, fanout(&schema, ONE / 2, 3, 9))
        .await
        .unwrap()
        .records
        .as_slice()[0]
        .clone();
    assert_eq!(abstention_code(&record), "FANOUT_BUDGET_TOO_LOW");

    // Every code is registered.
    for code in medulla_decision_types::decision::population::DRIFT_REFUSALS
        .iter()
        .chain(medulla_decision_types::decision::population::FANOUT_REFUSALS)
    {
        assert_eq!(
            medulla_decision_types::decision::codes::resolve(code.as_str()),
            Some(*code)
        );
    }
}
