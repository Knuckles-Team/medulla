//! The `adversarial_admission` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R753.755).
//!
//! Admits a candidate against its bounded set of red-team probes with the pure
//! `admit` kernel. A rejection abstains with `AbstainReason::Family` carrying
//! `PROBE_COMPROMISED`; the typed answer still names the verdict. Probe results
//! are caller claims, so the record is a `Claim` resolved as a `Constraint`.
//! The harness applies the answer.

use medulla_decision_stats::decision::step_verify::admit;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::trajectory_verdict::{
    AdmissionRequest, AdmissionVerdict, ProbeVerdict, VerifyKnobs, PROBE_COMPROMISED,
};
use medulla_decision_types::decision::{AbstainReason, DecisionPolicy, ResolutionKind};
use medulla_graph_core::graph::GraphView;
use medulla_types::contract::BoundedVec;
use medulla_types::evidence_class::EvidenceClass;

use crate::decide::stat_executor::{Executed, ExecutionContext, FamilyExecuted};
use crate::decide::stat_support::invalid_parameter;

use super::kernel::{advisory, body, claim, one_option, shape};
use super::DecisionFamily;

const KIND: &str = "adversarial_admission";
const PARAM: &str = "adversarial_admission";

fn parse(request: &DecideRequest) -> Result<(&AdmissionRequest, &str), String> {
    let options = shape(request, KIND)?;
    let TypedValue::AdversarialAdmission(admission) = body(request, KIND, PARAM)? else {
        return Err(invalid_parameter(format!(
            "{KIND} parameter {PARAM} has the wrong type"
        )));
    };
    Ok((&**admission, one_option(options, KIND)?))
}

fn verdict_label(verdict: ProbeVerdict) -> &'static str {
    match verdict {
        ProbeVerdict::Resisted => "resisted",
        ProbeVerdict::Compromised => "compromised",
        ProbeVerdict::Inconclusive => "inconclusive",
    }
}

/// The family of the `adversarial_admission` question kind.
pub struct AdversarialAdmissionFamily;

impl DecisionFamily for AdversarialAdmissionFamily {
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
        Some(parse(request).and_then(|(admission, subject)| {
            let knobs = VerifyKnobs::from_policy(policy);
            let verdict = admit(admission.probes.as_slice(), &knobs)?;
            let field = "params.adversarial_admission";
            let premises = admission
                .probes
                .iter()
                .map(|probe| {
                    claim(
                        &probe.probe_id,
                        format!(
                            "admission:probe={}/{}",
                            probe.probe_id,
                            verdict_label(probe.verdict)
                        ),
                        field,
                    )
                })
                .collect();
            let outcome = match verdict {
                AdmissionVerdict::Admit => advisory(),
                AdmissionVerdict::Reject => StatisticalOutcome::Abstained {
                    reasons: BoundedVec::new(vec![AbstainReason::Family {
                        family: QuestionKind::AdversarialAdmission,
                        code: PROBE_COMPROMISED,
                        detail: format!("candidate {subject} is not admitted by its probes"),
                    }])
                    .expect("one reason"),
                },
            };
            Ok(Executed::from_family(
                outcome,
                Some(FamilyExecuted {
                    answer: Some(FamilyAnswer::Admission(verdict)),
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
