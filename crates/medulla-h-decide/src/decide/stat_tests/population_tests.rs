//! Population drift and fan-out width, served through `Decide`
//! (MEDULLA-DECISION-ENGINE-R752.751, R752.754): the first Wasserstein distance
//! between two windows with its alert, and the replica width a fan-out should
//! use.

use super::kernel_family_support::{decide_as_other_tenant, facts, kernel_request};
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
        .unwrap()
        .clone();
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
    // A budget that cannot fund one replica is refused.
    let error = decide(&h, fanout(&schema, ONE / 2, 3, 9))
        .await
        .unwrap_err();
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
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
    assert!(error.starts_with("PARAMETER_INVALID"), "{error}");
}
