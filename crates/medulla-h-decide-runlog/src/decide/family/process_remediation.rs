//! The `process_remediation` question family (MEDULLA-DECISION-ENGINE-R820).
//!
//! Run-bound liveness evidence (last progress, cpu time, output bytes, whether
//! the process holds child output) is judged at the engine's own clock, never a
//! time the request carries (see `decision::process_remediation`). Evidence for
//! another run or evidence the clock cannot have observed is a coded refusal.
//! Advisory; `Decide` writes nothing.

use medulla_decision_statistical_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, TypedValue};
use medulla_graph_core::graph::GraphView;
use {
    medulla_decision_policy_types::decision::process_remediation::verdict,
    medulla_decision_policy_types::decision::process_remediation::Liveness,
    medulla_decision_policy_types::decision::process_remediation::EVIDENCE_MISMATCH,
    medulla_decision_policy_types::decision::process_remediation::PARAM_CPU_TIME_MS,
    medulla_decision_policy_types::decision::process_remediation::PARAM_EVIDENCE_RUN_ID,
    medulla_decision_policy_types::decision::process_remediation::PARAM_HOLDS_CHILD_OUTPUT,
    medulla_decision_policy_types::decision::process_remediation::PARAM_LAST_PROGRESS_MS,
    medulla_decision_policy_types::decision::process_remediation::PARAM_OBSERVED_AT_MS,
    medulla_decision_policy_types::decision::process_remediation::PARAM_OUTPUT_BYTES,
    medulla_decision_policy_types::decision::process_remediation::PARAM_RUN_ID,
    medulla_decision_policy_types::decision::process_remediation::REMEDIATION_OPTION,
};
use {
    medulla_decision_statistical_types::decision::record::premise::PremiseClass,
    medulla_decision_statistical_types::decision::record::premise::PremiseRef,
    medulla_decision_statistical_types::decision::DecisionPolicy,
};

use {
    medulla_h_decide_base::decide::stat_executor::Executed,
    medulla_h_decide_base::decide::stat_executor::ExecutionContext,
    medulla_h_decide_base::decide::stat_executor::FamilyPremise,
    medulla_h_decide_base::decide::stat_executor::FamilyPremiseSource,
};

use medulla_h_decide_base::decide::family::DecisionFamily;
use {
    medulla_h_decide_base::decide::family::support::advisory,
    medulla_h_decide_base::decide::family::support::invalid,
    medulla_h_decide_base::decide::family::support::param,
    medulla_h_decide_base::decide::family::support::single_option,
    medulla_h_decide_base::decide::family::support::text_param,
    medulla_h_decide_base::decide::family::support::unsigned_param,
};

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
        subject: "process_remediation".into(),
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
