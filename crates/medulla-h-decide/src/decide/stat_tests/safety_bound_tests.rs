//! The served safety bound stage (MEDULLA-DECISION-ENGINE-R001.100.1 and
//! R001.100.2): a policy that pins safety bounds withholds a decision whose
//! measurements are out of bound, missing, stale or in another unit, answering an
//! abstention with the bound's typed refusal code and one `safety_bound:<id>`
//! premise per bound; a decision inside every bound is untouched.

use super::kernel_family_support::{decide_as_other_tenant, facts};
use super::trajectory_join_tests::{entry, join};
use super::*;
use medulla_agent_types::agent_component::DecisionPolicyRef;
use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
use medulla_types::contract::BoundedVec;
use medulla_types::evidence_class::EvidenceClass;
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseProvenance,
    medulla_decision_statistical_types::decision::DecisionPolicy,
    medulla_decision_types::decision::record::AbstainReason,
};
use {
    medulla_decision_statistical_types::decision::safety::BoundComparison,
    medulla_decision_statistical_types::decision::safety::SafetyBound,
    medulla_decision_statistical_types::decision::safety::SafetyMeasurement,
    medulla_decision_statistical_types::decision::safety::DEFAULT_MAX_MEASUREMENT_AGE_MS,
    medulla_decision_statistical_types::decision::safety::SAFETY_BOUND_EXCEEDED,
    medulla_decision_statistical_types::decision::safety::SAFETY_MEASUREMENT_MISSING,
    medulla_decision_statistical_types::decision::safety::SAFETY_MEASUREMENT_STALE,
    medulla_decision_statistical_types::decision::safety::SAFETY_UNIT_MISMATCH,
};

const BOUND: &str = "latency-p95";

fn bound(id: &str) -> SafetyBound {
    SafetyBound {
        bound_id: id.to_string(),
        revision: 1,
        digest: "ab".repeat(32),
        metric_id: "latency".to_string(),
        unit: "ms".to_string(),
        comparison: BoundComparison::Le,
        limit_q32: 100 << 32,
    }
}

fn measured(id: &str, value: i64, unit: &str, measured_at_ms: u64) -> SafetyMeasurement {
    SafetyMeasurement {
        bound_id: id.to_string(),
        value_q32: value << 32,
        unit: unit.to_string(),
        source_revision: 7,
        measured_at_ms,
    }
}

fn now_ms() -> u64 {
    medulla_kernel::kernel::dispatch_support::authoritative_now_ms()
}

/// A one-trajectory join under a pinned policy that bounds `BOUND`, carrying
/// `measurements` when the request has any.
fn bounded_join(
    h: &Harness,
    schema: &ComponentDependency,
    measurements: Option<Vec<SafetyMeasurement>>,
) -> DecideRequest {
    let mut policy = DecisionPolicy::engine_default();
    policy.safety_bounds = BoundedVec::new(vec![bound(BOUND)]).unwrap();
    let mut request = join(schema, vec![entry("a", EvidenceClass::Claim, 1)]);
    request.policy = DecisionPolicyRef::Pinned {
        component: h.publish_policy("safety-policy", &policy),
    };
    if let Some(measurements) = measurements {
        let mut params = request.params.as_slice().to_vec();
        params.push(TypedParam {
            name: "safety.measurements".to_string(),
            value: TypedValue::SafetyMeasurements(BoundedVec::new(measurements).unwrap()),
        });
        request.params = BoundedVec::new(params).unwrap();
    }
    request
}

fn abstention_code(record: &StatisticalDecisionRecord) -> String {
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("expected a withheld decision, got {:?}", record.outcome);
    };
    match &reasons.as_slice()[0] {
        AbstainReason::Family { code, .. } => code.as_str().to_string(),
        other => panic!("expected a family refusal, got {other:?}"),
    }
}

/// Verifies MEDULLA-DECISION-ENGINE-R001.100.1: a measurement inside the bound
/// passes the decision through, and one outside it withholds.
// spec: MEDULLA-DECISION-ENGINE-R001.100.1
#[tokio::test]
async fn in_bound_passes_and_out_of_bound_withholds() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    // Inside the bound (50 ms against a 100 ms limit): the join answers.
    let fresh = vec![measured(BOUND, 50, "ms", now_ms())];
    let batch = decide(&h, bounded_join(&h, &schema, Some(fresh)))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    assert!(
        !matches!(record.outcome, StatisticalOutcome::Abstained { .. }),
        "an in-bound decision is not withheld: {:?}",
        record.outcome
    );
    assert!(facts(record)
        .iter()
        .any(|fact| fact.starts_with("outcome=within")));

    // Exactly at the limit still holds (`Le`); one over withholds.
    let at_limit = vec![measured(BOUND, 100, "ms", now_ms())];
    let batch = decide(&h, bounded_join(&h, &schema, Some(at_limit)))
        .await
        .unwrap();
    assert!(!matches!(
        batch.records.as_slice()[0].outcome,
        StatisticalOutcome::Abstained { .. }
    ));
    let over = vec![measured(BOUND, 101, "ms", now_ms())];
    let batch = decide(&h, bounded_join(&h, &schema, Some(over)))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    assert_eq!(abstention_code(record), SAFETY_BOUND_EXCEEDED.as_str());
    assert!(
        record.answer.is_none(),
        "a withheld decision answers nothing"
    );
}

/// Verifies MEDULLA-DECISION-ENGINE-R001.100.2: each way a measurement fails to
/// back a bound has its typed code and premise, a policy with bounds and no
/// measurements withholds, and another tenant cannot read the verdict.
// spec: MEDULLA-DECISION-ENGINE-R001.100.2
#[tokio::test]
async fn missing_stale_and_mismatched_measurements_refuse() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);
    let stale_at = now_ms().saturating_sub(DEFAULT_MAX_MEASUREMENT_AGE_MS + 60_000);

    let cases: Vec<(Option<Vec<SafetyMeasurement>>, &str, &str)> = vec![
        (
            None,
            SAFETY_MEASUREMENT_MISSING.as_str(),
            "outcome=measurement_missing",
        ),
        (
            Some(vec![measured("another-bound", 1, "ms", now_ms())]),
            SAFETY_MEASUREMENT_MISSING.as_str(),
            "outcome=measurement_missing",
        ),
        (
            Some(vec![measured(BOUND, 50, "ms", stale_at)]),
            SAFETY_MEASUREMENT_STALE.as_str(),
            "outcome=measurement_stale",
        ),
        (
            Some(vec![measured(BOUND, 50, "s", now_ms())]),
            SAFETY_UNIT_MISMATCH.as_str(),
            "outcome=unit_mismatch",
        ),
    ];
    for (measurements, code, fact_prefix) in cases {
        let batch = decide(&h, bounded_join(&h, &schema, measurements))
            .await
            .unwrap();
        let record = &batch.records.as_slice()[0];
        assert_eq!(abstention_code(record), code, "{fact_prefix}");
        let premise = record
            .premises
            .iter()
            .find(|premise| premise.subject == format!("safety_bound:{BOUND}"))
            .unwrap_or_else(|| panic!("{code}: the bound leaves a premise"));
        assert!(premise.fact.starts_with(fact_prefix), "{}", premise.fact);
        assert_eq!(premise.class, PremiseClass::Claim);
        assert!(matches!(
            &premise.provenance,
            PremiseProvenance::Request { field } if field == "safety.measurements"
        ));
    }

    // A foreign tenant is refused before any bound is read.
    let fresh = vec![measured(BOUND, 50, "ms", now_ms())];
    let refused = decide_as_other_tenant(&h, bounded_join(&h, &schema, Some(fresh))).await;
    assert!(refused.starts_with("ACCESS_DENIED"), "{refused}");
}
