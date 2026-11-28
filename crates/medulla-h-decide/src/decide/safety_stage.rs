//! The served safety bound stage (MEDULLA-DECISION-ENGINE-R001.100.2).
//!
//! After a decision concludes and before it is sealed, the policy's pinned
//! safety bounds are checked against the caller's measurements (the typed
//! param `safety.measurements`). A policy with bounds and no measurements
//! withholds. A withheld verdict replaces the outcome with an abstention that
//! carries the violated bound's typed refusal code; every bound leaves one
//! `safety_bound:<bound_id>` premise whatever the verdict. A policy with no
//! bounds is untouched.

use medulla_decision_statistical_types::decision::knobs::KnobValue;
use medulla_decision_stats::decision::ladder::LadderResult;
use medulla_decision_stats_records::decision::safety::evaluate_bounds;
use medulla_decision_types::decision::statistical::{
    DecideRequest, StatisticalOutcome, TypedValue,
};
use medulla_types::contract::BoundedVec;
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseProvenance,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
    medulla_decision_types::decision::record::AbstainReason,
};
use {
    medulla_decision_statistical_types::decision::safety::BoundCheck,
    medulla_decision_statistical_types::decision::safety::BoundOutcome,
    medulla_decision_statistical_types::decision::safety::SafetyMeasurement,
    medulla_decision_statistical_types::decision::safety::SafetyVerdict,
    medulla_decision_statistical_types::decision::safety::KNOBS,
};

use super::stat_executor::Executed;
use super::stat_support::invalid_parameter;

/// The param the caller's measurements travel in.
pub(super) const MEASUREMENTS_PARAM: &str = "safety.measurements";

/// The measurements the request carries; none when the param is absent.
fn measurements(request: &DecideRequest) -> Result<&[SafetyMeasurement], String> {
    match request
        .params
        .iter()
        .find(|param| param.name == MEASUREMENTS_PARAM)
    {
        None => Ok(&[]),
        Some(param) => match &param.value {
            TypedValue::SafetyMeasurements(measured) => Ok(measured.as_slice()),
            _ => Err(invalid_parameter(format!(
                "parameter {MEASUREMENTS_PARAM} has the wrong type"
            ))),
        },
    }
}

/// The oldest measurement a check accepts, from the pinned policy's knob.
fn max_age_ms(policy: &DecisionPolicy) -> u64 {
    match policy.knob(&KNOBS[0]) {
        KnobValue::DurationMs(ms) => ms,
        _ => medulla_decision_statistical_types::decision::safety::DEFAULT_MAX_MEASUREMENT_AGE_MS,
    }
}

fn outcome_label(outcome: BoundOutcome) -> &'static str {
    match outcome {
        BoundOutcome::Within => "within",
        BoundOutcome::Exceeded => "exceeded",
        BoundOutcome::MeasurementMissing => "measurement_missing",
        BoundOutcome::MeasurementStale => "measurement_stale",
        BoundOutcome::UnitMismatch => "unit_mismatch",
    }
}

fn premise(check: &BoundCheck) -> PremiseRef {
    let measured = check
        .measured_q32
        .map_or_else(|| "none".to_string(), |value| value.to_string());
    PremiseRef {
        subject: format!("safety_bound:{}", check.bound_id),
        fact: format!(
            "outcome={};limit_q32={};measured_q32={measured}",
            outcome_label(check.outcome),
            check.limit_q32
        ),
        class: PremiseClass::Claim,
        provenance: PremiseProvenance::Request {
            field: MEASUREMENTS_PARAM.to_string(),
        },
        trust_tier: None,
    }
}

/// The abstention a withheld verdict replaces the outcome with.
fn withheld(request: &DecideRequest, verdict: &SafetyVerdict) -> Option<StatisticalOutcome> {
    let SafetyVerdict::Withheld {
        bound_id,
        outcome,
        limit_q32,
        measured_q32,
    } = verdict
    else {
        return None;
    };
    let code = outcome.code()?;
    Some(StatisticalOutcome::Abstained {
        reasons: BoundedVec::new(vec![AbstainReason::Family {
            family: request.question.kind,
            code,
            detail: format!(
                "safety bound {bound_id}: {} (limit_q32={limit_q32}, measured_q32={})",
                outcome_label(*outcome),
                measured_q32.map_or_else(|| "none".to_string(), |value| value.to_string())
            ),
        }])
        .expect("one reason fits the bound"),
    })
}

/// Check the policy's safety bounds at `now_ms` and fold the verdict into
/// `executed`: the premises of every check, and an abstention in place of the
/// outcome when a bound withholds.
pub(super) fn apply(
    request: &DecideRequest,
    policy: &DecisionPolicy,
    now_ms: u64,
    executed: Executed,
) -> Result<(Executed, Vec<PremiseRef>), String> {
    if policy.safety_bounds.is_empty() {
        return Ok((executed, Vec::new()));
    }
    let (checks, verdict) = evaluate_bounds(
        policy.safety_bounds.as_slice(),
        measurements(request)?,
        now_ms,
        max_age_ms(policy),
    );
    let premises: Vec<PremiseRef> = checks.iter().map(premise).collect();
    let Some(outcome) = withheld(request, &verdict) else {
        return Ok((executed, premises));
    };
    // A withheld decision answers nothing: the family's answer, route and
    // calibration go with the outcome they described.
    let replaced = Executed {
        matrix: executed.matrix,
        ladder: LadderResult {
            outcome,
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
