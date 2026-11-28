//! The `step_verification` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R753.755).
//!
//! Verifies a trace step by step with the pure `verify_steps` kernel: the
//! verdict is bound to the SHA-256 of the step digests, and the first failed
//! step selects the recovery. A check interval wider than
//! `verify.max_interval_width` leaves its step uncertain, and a failed or
//! uncertain step withholds every step after it
//! (MEDULLA-DECISION-ENGINE-R121.100). Checks are caller claims, so the record is a
//! `Claim` resolved as a `Constraint`. The harness applies the recovery.

use medulla_decision_stats::decision::step_verify::{bind_verdict, check_outcome, verify_steps};
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    DecideRequest, StatisticalOutcome, TypedValue,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;
use {
    medulla_decision_statistical_types::decision::trajectory_verdict::RecoveryAction,
    medulla_decision_statistical_types::decision::trajectory_verdict::StepVerification,
    medulla_decision_statistical_types::decision::trajectory_verdict::VerifyKnobs,
    medulla_decision_statistical_types::decision::trajectory_verdict::VerifyRequest,
};
use {
    medulla_decision_statistical_types::decision::DecisionPolicy,
    medulla_decision_types::decision::record::ResolutionKind,
};

use medulla_h_decide_base::decide::stat_support::invalid_parameter;
use {
    medulla_h_decide_base::decide::stat_executor::Executed,
    medulla_h_decide_base::decide::stat_executor::ExecutionContext,
    medulla_h_decide_base::decide::stat_executor::FamilyExecuted,
};

use medulla_h_decide_base::decide::family::DecisionFamily;
use {
    medulla_h_decide_base::decide::family::kernel::advisory,
    medulla_h_decide_base::decide::family::kernel::body,
    medulla_h_decide_base::decide::family::kernel::claim,
    medulla_h_decide_base::decide::family::kernel::one_option,
    medulla_h_decide_base::decide::family::kernel::shape,
};

const KIND: &str = "step_verification";
const PARAM: &str = "step_verification";

fn parse(request: &DecideRequest) -> Result<(&VerifyRequest, &str), String> {
    let options = shape(request, KIND)?;
    let TypedValue::StepVerification(verify) = body(request, KIND, PARAM)? else {
        return Err(invalid_parameter(format!(
            "{KIND} parameter {PARAM} has the wrong type"
        )));
    };
    Ok((&**verify, one_option(options, KIND)?))
}

fn recovery_label(recovery: &Option<RecoveryAction>) -> String {
    match recovery {
        None => "none".to_string(),
        Some(RecoveryAction::Retry { step_id }) => format!("retry:{step_id}"),
        Some(RecoveryAction::Rollback { step_id }) => format!("rollback:{step_id}"),
        Some(RecoveryAction::Abandon) => "abandon".to_string(),
    }
}

/// The family of the `step_verification` question kind.
pub struct StepVerificationFamily;

impl DecisionFamily for StepVerificationFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        parse(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(parse(request).and_then(|(verify, subject)| {
            let knobs = VerifyKnobs::from_policy(policy);
            let (verdict, recovery, withheld) = verify_steps(
                verify.steps.as_slice(),
                verify.checks.as_slice(),
                verify.attempts,
                &knobs,
            )?;
            bind_verdict(&verdict, &verify.trace_digest)?;
            let field = "params.step_verification";
            let mut premises = vec![claim(
                subject,
                format!("verify:trace={}", verdict.trace_digest),
                field,
            )];
            premises.extend(verify.checks.iter().map(|check| {
                claim(
                    &check.step_id,
                    format!(
                        "verify:step={}/{}",
                        check.step_id,
                        check_outcome(check, &knobs).word()
                    ),
                    field,
                )
            }));
            for step in verify.steps.iter() {
                for evidence_id in &step.evidence_ids {
                    premises.push(claim(
                        &step.step_id,
                        format!("verify:evidence={}/{evidence_id}", step.step_id),
                        field,
                    ));
                }
            }
            premises.push(claim(
                subject,
                format!("verify:recovery={}", recovery_label(&recovery)),
                field,
            ));
            Ok(Executed::from_family(
                advisory(),
                Some(FamilyExecuted {
                    answer: Some(FamilyAnswer::StepVerdict(Box::new(StepVerification {
                        verdict,
                        recovery,
                        withheld,
                    }))),
                    premises,
                    resolution: None,
                    evidence: None,
                }),
            ))
        }))
    }

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Claim)
    }
}
