//! The served limit stage (MEDULLA-DECISION-ENGINE-R131.3): a policy that pins
//! limits withholds a decision whose proposed action breaches one, answering an
//! abstention with `LIMIT_BREACH` and one `limit:<rule_id>` premise per breach;
//! an action inside every limit, a question without the param and a policy
//! without limits are untouched. A request bound only tightens a policy bound.

use super::kernel_family_support::decide_as_other_tenant;
use super::trajectory_join_tests::{entry, join};
use super::*;
use medulla_decision_types::decision::limit_types::{
    GuardedAction, LimitMetric, LimitObservation, LimitRule, RequestedBound, LIMIT_BREACH,
};
use medulla_decision_types::decision::statistical::{TypedParam, TypedValue};
use medulla_decision_types::decision::{
    AbstainReason, DecisionPolicy, DecisionPolicyRef, EvidenceClass, PremiseClass,
    PremiseProvenance,
};
use medulla_types::contract::BoundedVec;

const RULE: &str = "max-position";
const SCOPE: &str = "account:a";

fn rule() -> LimitRule {
    LimitRule {
        id: RULE.to_string(),
        metric: LimitMetric::PositionSize,
        bound: 10_000,
        scope: SCOPE.to_string(),
    }
}

fn action(observed: i64, requested: Option<i64>) -> GuardedAction {
    GuardedAction {
        signal_iri: "urn:signal:buy".to_string(),
        observations: vec![LimitObservation {
            metric: LimitMetric::PositionSize,
            scope: SCOPE.to_string(),
            value: observed,
        }],
        requested_bounds: requested
            .map(|bound| {
                vec![RequestedBound {
                    rule_id: RULE.to_string(),
                    bound,
                }]
            })
            .unwrap_or_default(),
    }
}

/// A one-trajectory join under a pinned policy with `limits`, carrying
/// `action` when the request has one.
fn limited_join(
    h: &Harness,
    schema: &ComponentDependency,
    limits: Vec<LimitRule>,
    action: Option<GuardedAction>,
) -> DecideRequest {
    let mut policy = DecisionPolicy::engine_default();
    policy.limits = BoundedVec::new(limits).unwrap();
    let mut request = join(schema, vec![entry("a", EvidenceClass::Claim, 1)]);
    request.policy = DecisionPolicyRef::Pinned {
        component: h.publish_policy("limit-policy", &policy),
    };
    if let Some(action) = action {
        let mut params = request.params.as_slice().to_vec();
        params.push(TypedParam {
            name: "guarded_action".to_string(),
            value: TypedValue::GuardedAction(Box::new(action)),
        });
        request.params = BoundedVec::new(params).unwrap();
    }
    request
}

fn is_withheld(record: &StatisticalDecisionRecord) -> bool {
    matches!(record.outcome, StatisticalOutcome::Abstained { .. })
}

/// Verifies MEDULLA-DECISION-ENGINE-R131.3: a breaching action never reaches
/// `Advisory` or `Acted`, the breach is recorded as a premise, and a
/// cross-tenant caller is refused.
// spec: MEDULLA-DECISION-ENGINE-R131.3
#[tokio::test]
async fn breaching_action_abstains_and_commit_records_the_breach() {
    let h = Harness::new().await;
    let (schema, _) = publish_route_schema(&h);

    // At the bound the action passes; one unit over, it is withheld.
    let batch = decide(&h, limited_join(&h, &schema, vec![rule()], Some(action(10_000, None))))
        .await
        .unwrap();
    assert!(!is_withheld(&batch.records.as_slice()[0]));
    let batch = decide(&h, limited_join(&h, &schema, vec![rule()], Some(action(10_001, None))))
        .await
        .unwrap();
    let record = &batch.records.as_slice()[0];
    let StatisticalOutcome::Abstained { reasons } = &record.outcome else {
        panic!("a breaching action is withheld, got {:?}", record.outcome);
    };
    assert!(matches!(
        &reasons.as_slice()[0],
        AbstainReason::Family { code, .. } if *code == LIMIT_BREACH
    ));
    assert!(record.answer.is_none(), "a withheld decision answers nothing");
    let premise = record
        .premises
        .iter()
        .find(|premise| premise.subject == format!("limit:{RULE}"))
        .expect("the breach leaves a premise");
    assert_eq!(premise.fact, "observed=10001;bound=10000");
    assert_eq!(premise.class, PremiseClass::Claim);
    assert!(matches!(
        &premise.provenance,
        PremiseProvenance::Request { field } if field == "guarded_action"
    ));

    // A request bound tightens the policy bound; it can never loosen it.
    let tighter = decide(&h, limited_join(&h, &schema, vec![rule()], Some(action(6_000, Some(5_000)))))
        .await
        .unwrap();
    assert!(is_withheld(&tighter.records.as_slice()[0]));
    let looser = decide(&h, limited_join(&h, &schema, vec![rule()], Some(action(12_000, Some(50_000)))))
        .await
        .unwrap();
    assert!(is_withheld(&looser.records.as_slice()[0]));

    // No param, or no limits, passes unchanged.
    let no_action = decide(&h, limited_join(&h, &schema, vec![rule()], None))
        .await
        .unwrap();
    assert!(!is_withheld(&no_action.records.as_slice()[0]));
    let no_limits = decide(&h, limited_join(&h, &schema, Vec::new(), Some(action(99_999, None))))
        .await
        .unwrap();
    assert!(!is_withheld(&no_limits.records.as_slice()[0]));

    // A foreign tenant is refused before any limit is read.
    let refused = decide_as_other_tenant(
        &h,
        limited_join(&h, &schema, vec![rule()], Some(action(10_001, None))),
    )
    .await;
    assert!(refused.starts_with("ACCESS_DENIED"), "{refused}");
}
