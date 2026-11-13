//! The `sub_call_admission` question family (MEDULLA-DECISION-ENGINE-R137.15).
//!
//! A sub-call is admitted only when its stored signature (a tenant graph node,
//! read through the row-level filtered graph the `graph` param names) matches
//! the revision, digest, input schema, tool set and model the call names. A
//! mismatch is an abstention that names the expected and the actual value.
//! `Decide` is evaluate-only and writes nothing.

use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::sub_call::{
    check_sub_call, tool_set_digest, SubCallRequest, SubCallSignature,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseProvenance, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{Executed, ExecutionContext};

use super::support::{abstained, advisory, invalid, json_param, single_option};
use super::DecisionFamily;

/// The one option a sub-call admission question declares.
pub const ADMISSION_OPTION: &str = "admit";
/// The param holding the call: the JSON text of a `SubCallRequest`.
pub const CALL_PARAM: &str = "sub_call";

/// The family of the `sub_call_admission` question kind.
pub struct SubCallAdmissionFamily;

fn claim(fact: String) -> PremiseRef {
    PremiseRef {
        subject: "sub_call".to_string(),
        fact,
        class: PremiseClass::Claim,
        provenance: PremiseProvenance::Request {
            field: "params.sub_call".to_string(),
        },
    }
}

impl DecisionFamily for SubCallAdmissionFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, ADMISSION_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        json_param::<SubCallRequest>(request, CALL_PARAM).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        _policy: &DecisionPolicy,
        graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let call: SubCallRequest = json_param(request, CALL_PARAM)?;
            let view = graph_view.ok_or_else(|| {
                invalid("a sub_call_admission question reads its signature from a named graph")
            })?;
            let stored = view
                .node_row_object(&SubCallSignature::node_id(&call.signature_id))
                .and_then(|props| SubCallSignature::from_properties(&props));
            let parent_known = view
                .node_row_object(&format!("run:{}", call.parent_run_id))
                .is_some();
            match check_sub_call(&call, stored.as_ref(), parent_known) {
                Ok(_) => advisory(vec![ADMISSION_OPTION.to_string()]),
                Err(refusal) => Ok(abstained(QuestionKind::SubCallAdmission, &refusal)),
            }
        })())
    }

    fn premises(
        &self,
        request: &DecideRequest,
        _executed: &Executed,
        _policy_digest: &str,
    ) -> Vec<PremiseRef> {
        let Ok(call) = json_param::<SubCallRequest>(request, CALL_PARAM) else {
            return Vec::new();
        };
        vec![
            claim(format!(
                "sub_call.signature_digest={}",
                call.signature_digest
            )),
            claim(format!("sub_call.model_id={}", call.model_id)),
            claim(format!(
                "sub_call.tool_set_digest={}",
                tool_set_digest(&call.tool_ids)
            )),
            claim(format!(
                "sub_call.input_schema_digest={}",
                call.input_schema_digest
            )),
        ]
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Observation)
    }
}
