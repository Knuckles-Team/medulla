//! The `context_replacement` question family (MEDULLA-DECISION-ENGINE-R137.14).
//!
//! A candidate context replaces the raw one only when a complete paired
//! held-out comparison holds within `context.replacement_tolerance`. Every
//! failure keeps the raw context: the answer is an abstention that names the
//! registered refusal. The scores are the caller's, so they are claims.
//! `Decide` is evaluate-only and writes nothing.

use medulla_decision_types::decision::context_replacement::{
    PairedContextEvaluation, ReplacementRefusal,
};
use medulla_decision_types::decision::knob_families::context;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{DecideRequest, QuestionKind, TypedValue};
use medulla_decision_types::decision::{
    DecisionPolicy, PremiseClass, PremiseProvenance, PremiseRef,
};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{Executed, ExecutionContext};

use super::support::{abstained, advisory, invalid, param, single_option, text_param};
use super::DecisionFamily;

/// The one option a context replacement question declares.
pub const REPLACEMENT_OPTION: &str = "replacement";

/// The family of the `context_replacement` question kind.
pub struct ContextReplacementFamily;

/// The one `paired_evaluation` parameter, and the digests the caller pins.
struct Asked<'a> {
    evaluation: &'a PairedContextEvaluation,
    raw_context_digest: Option<&'a str>,
    evaluator_revision: Option<&'a str>,
}

fn asked(request: &DecideRequest) -> Result<Asked<'_>, String> {
    let Some(TypedValue::ContextReplacement(evaluation)) = param(request, "paired_evaluation")?
    else {
        return Err(invalid(
            "a context_replacement question carries one paired_evaluation parameter",
        ));
    };
    Ok(Asked {
        evaluation,
        raw_context_digest: text_param(request, "expected_raw_context_digest")?,
        evaluator_revision: text_param(request, "expected_evaluator_revision")?,
    })
}

fn claim(fact: String) -> PremiseRef {
    PremiseRef {
        subject: "context_replacement".to_string(),
        fact,
        class: PremiseClass::Claim,
        provenance: PremiseProvenance::Request {
            field: "params.paired_evaluation".to_string(),
        },
    }
}

impl DecisionFamily for ContextReplacementFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, REPLACEMENT_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        asked(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(asked(request).and_then(|asked| {
            let tolerance = context::replacement_tolerance_micros(policy);
            match asked.evaluation.decide(
                tolerance,
                asked.raw_context_digest,
                asked.evaluator_revision,
            ) {
                Ok(decision) => advisory(vec![decision.verdict.option_id().to_string()]),
                Err(refusal) => Ok(abstained(QuestionKind::ContextReplacement, &refusal)),
            }
        }))
    }

    fn premises(
        &self,
        request: &DecideRequest,
        _executed: &Executed,
        _policy_digest: &str,
    ) -> Vec<PremiseRef> {
        let Ok(asked) = asked(request) else {
            return Vec::new();
        };
        let tolerance = context::replacement_tolerance_micros(&DecisionPolicy::engine_default());
        // The means are a function of the caller's scores alone; a refusal has none.
        match asked.evaluation.decide(tolerance, None, None) {
            Ok(decision) => vec![
                claim(format!(
                    "context_replacement.raw_mean_micro={}",
                    decision.raw_mean_micro
                )),
                claim(format!(
                    "context_replacement.candidate_mean_micro={}",
                    decision.candidate_mean_micro
                )),
            ],
            Err(ReplacementRefusal::VerdictsIncomplete { .. })
            | Err(ReplacementRefusal::DigestMismatch { .. })
            | Err(ReplacementRefusal::MetricUndefined { .. }) => Vec::new(),
        }
    }
}
