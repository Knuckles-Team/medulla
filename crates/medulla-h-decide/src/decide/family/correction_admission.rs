//! The `correction_admission` question family (MEDULLA-DECISION-ENGINE-R161.3,
//! R161.4).
//!
//! A correction binds a failed run's trace, the fix generated for it and the
//! regression test that pins the fix, with the receipts of the test at its two
//! revisions. The family admits the correction when `check_receipt_pattern`
//! holds: the fix names the trace, the test names the fix, the before receipt
//! is `Failed` and the after receipt is `Passed` at two different revisions.
//! A broken link abstains with the registered binding code. Medulla runs no
//! test and `Decide` writes nothing; the receipts are the caller's claims, and
//! the caller commits the record through `DecisionCommit`.

use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::trace_fix_binding::{
    check_receipt_pattern, CorrectionBinding,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{abstained, advisory, invalid, param, single_option};
use super::DecisionFamily;

/// The one declared option: admit the correction.
pub const ADMIT_OPTION: &str = "admit_correction";

/// The family of the `correction_admission` question kind.
pub struct CorrectionAdmissionFamily;

/// The correction the request binds.
fn binding(request: &DecideRequest) -> Result<&CorrectionBinding, String> {
    match param(request, "correction_binding")? {
        Some(TypedValue::CorrectionBinding(binding)) => Ok(binding),
        Some(_) => Err(invalid("parameter correction_binding is not a correction")),
        None => Err(invalid("parameter correction_binding is required")),
    }
}

fn claim(subject: &'static str, fact: String) -> FamilyPremise {
    FamilyPremise {
        subject,
        fact,
        class: PremiseClass::Claim,
        source: FamilyPremiseSource::Request("params.correction_binding"),
    }
}

fn conclude_admission(request: &DecideRequest) -> Result<Executed, String> {
    let binding = binding(request)?;
    if let Err(refusal) = check_receipt_pattern(binding) {
        return Ok(abstained(QuestionKind::CorrectionAdmission, &refusal));
    }
    let mut executed = advisory(vec![ADMIT_OPTION.to_string()])?;
    executed.family_premises = vec![
        claim(
            "correction_trace",
            format!(
                "correction:trace={}@{}",
                binding.trace.trace_id.as_str(),
                binding.trace.error_trace_digest.to_hex()
            ),
        ),
        claim(
            "correction_fix",
            format!(
                "correction:fix={}@{}",
                binding.fix.fix_id.as_str(),
                binding.fix.fix_digest.to_hex()
            ),
        ),
        claim(
            "correction_test",
            format!(
                "correction:test={}@{}",
                binding.test.test_id.as_str(),
                binding.test.test_digest.to_hex()
            ),
        ),
        claim(
            "correction_receipts",
            "correction:receipts=failed_before_passed_after".to_string(),
        ),
    ];
    Ok(executed)
}

impl DecisionFamily for CorrectionAdmissionFamily {
    fn validate_candidates(
        &self,
        options: &[medulla_decision_types::decision::statistical::declared::DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, ADMIT_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        binding(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(conclude_admission(request))
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

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Claim)
    }
}
