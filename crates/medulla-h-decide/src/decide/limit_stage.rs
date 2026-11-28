//! The served limit stage (MEDULLA-DECISION-ENGINE-R131.3).
//!
//! After a decision concludes and before it is sealed, the policy's pinned
//! limits are checked against the proposed action the question carries in its
//! param `guarded_action`. A breach replaces the outcome with an abstention
//! carrying `LIMIT_BREACH` and leaves one `limit:<rule_id>` premise per breach.
//! A question without the param, or a policy without limits, is untouched. A
//! request bound can only tighten a policy bound.

use medulla_decision_outcome_types::decision::limit_guard::{check_limits, LimitBreach};
use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_types::decision::statistical::{
    DecideRequest, StatisticalOutcome, TypedValue,
};
use medulla_types::contract::BoundedVec;
use {
    medulla_decision_statistical_types::decision::limit_types::GuardedAction,
    medulla_decision_statistical_types::decision::limit_types::GUARDED_ACTION_PARAM,
};
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseProvenance,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
    medulla_decision_types::decision::record::AbstainReason,
};

use medulla_h_decide_base::decide::stat_executor::Executed;
use medulla_h_decide_base::decide::stat_support::invalid_parameter;

/// The proposed action the request carries; none when the param is absent.
fn guarded_action(request: &DecideRequest) -> Result<Option<&GuardedAction>, String> {
    match request
        .params
        .iter()
        .find(|param| param.name == GUARDED_ACTION_PARAM)
    {
        None => Ok(None),
        Some(param) => match &param.value {
            TypedValue::GuardedAction(action) => Ok(Some(action.as_ref())),
            _ => Err(invalid_parameter(format!(
                "parameter {GUARDED_ACTION_PARAM} has the wrong type"
            ))),
        },
    }
}

fn premise(breach: &LimitBreach) -> PremiseRef {
    PremiseRef {
        subject: format!("limit:{}", breach.rule_id),
        fact: format!("observed={};bound={}", breach.observed, breach.bound),
        class: PremiseClass::Claim,
        provenance: PremiseProvenance::Request {
            field: GUARDED_ACTION_PARAM.to_string(),
        },
        trust_tier: None,
    }
}

/// Check the policy's limits against the request's action and fold the verdict
/// into `executed`: the premises of every breach, and an abstention in place of
/// the outcome when any rule is breached.
pub(super) fn apply(
    request: &DecideRequest,
    policy: &DecisionPolicy,
    executed: Executed,
) -> Result<(Executed, Vec<PremiseRef>), String> {
    if policy.limits.is_empty() {
        return Ok((executed, Vec::new()));
    }
    let Some(action) = guarded_action(request)? else {
        return Ok((executed, Vec::new()));
    };
    let breaches = check_limits(action, policy.limits.as_slice());
    if breaches.is_empty() {
        return Ok((executed, Vec::new()));
    }
    let premises: Vec<PremiseRef> = breaches.iter().map(premise).collect();
    // A breaching action answers nothing: the family's answer, route and
    // calibration go with the outcome they described.
    let replaced = Executed {
        matrix: executed.matrix,
        ladder: LadderResult {
            outcome: StatisticalOutcome::Abstained {
                reasons: BoundedVec::new(
                    breaches
                        .iter()
                        .map(|breach| AbstainReason::LimitBreach {
                            rule_id: breach.rule_id.clone(),
                            observed: breach.observed,
                            bound: breach.bound,
                            signal_iri: breach.signal_iri.clone(),
                        })
                        .collect(),
                )
                .expect("one reason per rule fits the bound"),
            },
            calibration: None,
            explained: None,
            audit: None,
            logging: Vec::new(),
        },
        reading: None,
        seed: executed.seed,
        harness_contrast: None,
        family: None,
        family_premises: Vec::new(),
        route: None,
    };
    Ok((replaced, premises))
}
