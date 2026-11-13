//! The `process_remediation` question family (MEDULLA-DECISION-ENGINE-R820).
//!
//! Run-bound liveness evidence (last progress, cpu time, output bytes, whether
//! the process holds child output) is judged at the engine's own clock, never a
//! time the request carries (see `decision::process_remediation`). Evidence for
//! another run or evidence the clock cannot have observed is a coded refusal.
//! Advisory; `Decide` writes nothing.

use medulla_decision_types::decision::process_remediation::{
    verdict, Liveness, EVIDENCE_MISMATCH, PARAM_CPU_TIME_MS, PARAM_EVIDENCE_RUN_ID,
    PARAM_HOLDS_CHILD_OUTPUT, PARAM_LAST_PROGRESS_MS, PARAM_OBSERVED_AT_MS, PARAM_OUTPUT_BYTES,
    PARAM_RUN_ID, REMEDIATION_OPTION,
};
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, TypedValue};
use medulla_decision_types::decision::{DecisionPolicy, PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{advisory, invalid, param, single_option, text_param, unsigned_param};
use super::DecisionFamily;

/// The family of the `process_remediation` question kind.
pub struct ProcessRemediationFamily;

fn required_u64(request: &DecideRequest, name: &str) -> Result<u64, String> {
    unsigned_param(request, name)?
        .ok_or_else(|| invalid(format!("a process_remediation question requires {name}")))
}

fn required_text<'a>(request: &'a DecideRequest, name: &str) -> Result<&'a str, String> {
    text_param(request, name)?
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid(format!("a process_remediation question requires {name}")))
}

fn remediate(
    ctx: &ExecutionContext,
    request: &DecideRequest,
    policy: &DecisionPolicy,
) -> Result<Executed, String> {
    let run = required_text(request, PARAM_RUN_ID)?;
    let evidence_run = required_text(request, PARAM_EVIDENCE_RUN_ID)?;
    // Parsed and bound to the record even though no rule reads them: they are
    // part of the evidence the caller vouches for.
    let cpu = required_u64(request, PARAM_CPU_TIME_MS)?;
    let output = required_u64(request, PARAM_OUTPUT_BYTES)?;
    let holds_child_output = match param(request, PARAM_HOLDS_CHILD_OUTPUT)? {
        Some(TypedValue::Bool(value)) => *value,
        Some(_) => {
            return Err(invalid(format!(
                "parameter {PARAM_HOLDS_CHILD_OUTPUT} is not a boolean"
            )))
        }
        None => {
            return Err(invalid(format!(
                "a process_remediation question requires {PARAM_HOLDS_CHILD_OUTPUT}"
            )))
        }
    };
    if run != evidence_run {
        return Err(format!(
            "{}: the evidence is for run `{evidence_run}`, not `{run}`",
            EVIDENCE_MISMATCH.as_str()
        ));
    }
    let liveness = Liveness {
        observed_at_ms: required_u64(request, PARAM_OBSERVED_AT_MS)?,
        last_progress_ms: required_u64(request, PARAM_LAST_PROGRESS_MS)?,
        holds_child_output,
    };
    let answer = verdict(ctx.now_ms, liveness, policy)
        .map_err(|refusal| format!("{}: {}", refusal.code.as_str(), refusal.detail))?;
    let mut executed = advisory(vec![answer.option_id().to_string()])?;
    executed.family_premises = vec![FamilyPremise {
        subject: "process_remediation",
        fact: format!("liveness:cpu_ms={cpu}:output_bytes={output}"),
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request("params.last_progress_ms"),
    }];
    Ok(executed)
}

impl DecisionFamily for ProcessRemediationFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, REMEDIATION_OPTION)
    }

    fn conclude(
        &self,
        ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(remediate(ctx, request, policy))
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
