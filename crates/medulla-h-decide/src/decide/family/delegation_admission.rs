//! The `delegation_admission` question family (MEDULLA-DECISION-ENGINE-R134.102).
//!
//! Admits delegation of a task class only over a stable outcome window. Quality
//! is the pooled success rate of independent evaluations on executed records
//! of the class, read from the tenant's decision log through the same walk
//! `aggregate_log` pools. Only tenant-visible, non-synthetic records count, and
//! an evaluation recorded by the principal that decided the record never does.
//! `Decide` is evaluate-only and writes nothing.

use medulla_decision_stats::decision::delegation::admit_delegation;
use medulla_decision_types::decision::delegation::{
    knob_values, DelegationAdmissionRequest, DelegationOutcome, METRIC_SUCCESS, Q32_ONE,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, QuestionKind, TypedValue};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef, RecordWindow};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};
use crate::decide::stat_log::{joined, LogReader};
use crate::decide::stat_retention::Retention;

use super::support::{abstained, advisory, invalid, param, single_option};
use super::DecisionFamily;

/// The one option a delegation admission question declares.
pub const ADMIT_OPTION: &str = "admit";
const PARAM_DELEGATION: &str = "delegation";

/// The family of the `delegation_admission` question kind.
pub struct DelegationAdmissionFamily;

fn admission(request: &DecideRequest) -> Result<&DelegationAdmissionRequest, String> {
    let body = match param(request, PARAM_DELEGATION)? {
        Some(TypedValue::DelegationAdmission(body)) => body,
        Some(_) => {
            return Err(invalid(
                "parameter delegation is not a delegation admission",
            ))
        }
        None => return Err(invalid("parameter delegation is required")),
    };
    if body.task_class.trim().is_empty() {
        return Err(invalid("task_class must not be empty"));
    }
    if body.metric_id != METRIC_SUCCESS {
        return Err(invalid(format!(
            "metric_id must be `{METRIC_SUCCESS}`: the decision log records no other outcome metric"
        )));
    }
    if body.window_start_ms > body.window_end_ms {
        return Err(invalid("the window ends before it starts"));
    }
    if body.threshold_q32 > Q32_ONE {
        return Err(invalid("threshold_q32 is above 1.0"));
    }
    Ok(body)
}

/// The counted outcomes of the class inside the window, ordered by record and
/// evaluation id. Another tenant's records are never in the store prefix, a
/// principal-private record is never visible to the tenant-level reader, and
/// the evaluator of a record is never the principal that decided it.
fn counted_outcomes(
    ctx: &ExecutionContext,
    body: &DelegationAdmissionRequest,
) -> Result<Vec<DelegationOutcome>, String> {
    let reader = LogReader {
        tenant_id: ctx.tenant_id.to_string(),
        principal: String::new(),
        roles: Vec::new(),
        retention: Retention::none(),
    };
    let window = RecordWindow {
        from_ms: body.window_start_ms,
        to_ms: body.window_end_ms,
    };
    let mut out = Vec::new();
    for (entry, evaluations) in joined(ctx.store, &reader, Some(&body.task_class), Some(window))? {
        if entry.record.synthetic_evidence {
            continue;
        }
        for stored in evaluations {
            let Some(success) = stored.evaluation.success else {
                continue;
            };
            if stored.producer == entry.record.caller_principal {
                continue;
            }
            out.push(DelegationOutcome {
                record_id: entry.record.record_id.clone(),
                policy_digest: entry.record.inputs.policy_digest.clone(),
                evaluation_id: stored.evaluation.evaluation_id.clone(),
                success,
                recorded_at_ms: stored.recorded_at_ms,
            });
        }
    }
    out.sort_by(|a, b| (&a.record_id, &a.evaluation_id).cmp(&(&b.record_id, &b.evaluation_id)));
    Ok(out)
}

fn premise(fact: String, class: PremiseClass, source: FamilyPremiseSource) -> FamilyPremise {
    FamilyPremise {
        subject: "delegation_admission",
        fact,
        class,
        source,
    }
}

impl DecisionFamily for DelegationAdmissionFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, ADMIT_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        admission(request).map(|_| ())
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        let body = match admission(request) {
            Ok(body) => body,
            Err(detail) => return Some(Err(detail)),
        };
        let outcomes = match counted_outcomes(ctx, body) {
            Ok(outcomes) => outcomes,
            Err(detail) => return Some(Err(detail)),
        };
        let knobs = knob_values(policy);
        Some(match admit_delegation(body, &outcomes, ctx.now_ms, &knobs) {
            Err(refusal) => Ok(abstained(QuestionKind::DelegationAdmission, &refusal)),
            Ok(verdict) => advisory(vec![ADMIT_OPTION.to_string()]).map(|mut executed| {
                executed.family_premises = vec![
                    premise(
                        format!(
                            "delegation.pooled:successes={};samples={}",
                            verdict.successes, verdict.samples
                        ),
                        PremiseClass::Observation,
                        FamilyPremiseSource::Observation(body.task_class.clone()),
                    ),
                    premise(
                        format!(
                            "delegation.verdict:task_class={};metric={};threshold_micros={};window={}-{};outcome_set={};policy={};digest={}",
                            verdict.task_class,
                            verdict.metric_id,
                            verdict.threshold_micros,
                            verdict.window_start_ms,
                            verdict.window_end_ms,
                            verdict.outcome_set_digest,
                            verdict.policy_digest,
                            verdict.verdict_digest
                        ),
                        PremiseClass::Observation,
                        FamilyPremiseSource::Observation(body.task_class.clone()),
                    ),
                ];
                executed
            }),
        })
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        executed
            .family_premises
            .iter()
            .map(|premise| premise.seal(policy_digest))
            .collect()
    }
}
