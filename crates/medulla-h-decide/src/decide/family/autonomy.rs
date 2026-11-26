//! The `autonomy_verdict` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R752.755).
//!
//! The one declared option is the action class; its consecutive independent
//! successes are read from the caller's own decision log (`recent_evaluations`,
//! R159.3), so another tenant's outcomes never raise trust and a self-reported
//! outcome never counts. The pure `autonomy_verdict` kernel allows a reversible
//! class after `knob autonomy.clean_outcomes` clean outcomes; an irreversible
//! class, and a question of safety class `Security`, `Policy`, `WriteBack` or
//! `Irreversible`, always requires confirmation (`ACTION_NEEDS_APPROVAL`). The
//! record is an `Observation` resolved as a `Constraint`.

use medulla_decision_stats::decision::autonomy::{
    autonomy_verdict, ledger_from_outcomes, rule_requires_approval,
};
use medulla_decision_types::decision::autonomy::{
    clean_outcomes_needed, AutonomyReading, AutonomyRequest, TRUST_SELF_REPORTED,
};
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    CandidateSource, DecideRequest, QuestionKind, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{
    DecisionPolicy, PremiseClass, PremiseProvenance, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;

use crate::decide::stat_executor::{Executed, ExecutionContext, FamilyExecuted};
use crate::decide::stat_log::{recent_labels_counted, LogReader};
use crate::decide::stat_support::invalid_parameter;

use super::kernel::{abstained, advisory, body, claim, one_option};
use super::DecisionFamily;

const KIND: &str = "autonomy_verdict";
const PARAM: &str = "autonomy_verdict";

/// The request shape: any safety class (the rule reads it), no head, no belief
/// slices, and one declared option, the action class, without facts.
fn parse(request: &DecideRequest) -> Result<(&AutonomyRequest, &str), String> {
    if request.head.is_some() || !request.belief_as_of.is_empty() {
        return Err(invalid_parameter(format!(
            "{KIND} needs no head and no belief slices"
        )));
    }
    let CandidateSource::Declared { options } = &request.candidates else {
        return Err(invalid_parameter(format!("{KIND} declares its candidates")));
    };
    if options
        .iter()
        .any(|o| !o.classification.is_empty() || !o.numbers.is_empty() || !o.texts.is_empty())
    {
        return Err(invalid_parameter(format!(
            "{KIND} candidates carry no facts"
        )));
    }
    let TypedValue::AutonomyVerdict(autonomy) = body(request, KIND, PARAM)? else {
        return Err(invalid_parameter(format!(
            "{KIND} parameter {PARAM} has the wrong type"
        )));
    };
    Ok((&**autonomy, one_option(options.as_slice(), KIND)?))
}

fn observed(subject: &str, fact: String) -> PremiseRef {
    PremiseRef {
        subject: subject.to_string(),
        fact,
        class: PremiseClass::Observation,
        provenance: PremiseProvenance::Observation {
            evaluation_id: subject.to_string(),
        },
        trust_tier: None,
    }
}

/// The family of the `autonomy_verdict` question kind.
pub struct AutonomyFamily;

impl DecisionFamily for AutonomyFamily {
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        parse(request).map(|_| ())
    }

    fn conclude_with_log(
        &self,
        ctx: &ExecutionContext,
        reader: &LogReader,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some(parse(request).and_then(|(autonomy, class)| {
            let needed = clean_outcomes_needed(policy);
            let recent = recent_labels_counted(ctx.store, reader, class, needed as usize)?;
            if !rule_requires_approval(autonomy.irreversibility, request.question.safety)
                && recent.labels.is_empty()
                && recent.refused > 0
            {
                // Valid input the log cannot answer: every recorded outcome of
                // the class is self-reported, so none counts toward trust.
                return Ok(Executed::from_family(
                    abstained(
                        QuestionKind::AutonomyVerdict,
                        TRUST_SELF_REPORTED,
                        format!(
                            "{} recorded outcome(s) of {class} are self-reported; none count",
                            recent.refused
                        ),
                    )?,
                    None,
                ));
            }
            let ledger = ledger_from_outcomes(class, &recent.labels);
            let reading: AutonomyReading = autonomy_verdict(
                autonomy.irreversibility,
                request.question.safety,
                &ledger,
                needed,
            );
            let verdict = serde_json::to_value(reading.verdict)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let premises = vec![
                claim(class, format!("autonomy:class={class}"), "candidates"),
                observed(class, format!("autonomy:clean={}", reading.clean_outcomes)),
                claim(
                    class,
                    format!("autonomy:verdict={verdict}"),
                    "params.autonomy_verdict",
                ),
            ];
            Ok(Executed::from_family(
                advisory(),
                Some(FamilyExecuted {
                    answer: Some(FamilyAnswer::Autonomy(Box::new(reading))),
                    premises,
                    resolution: None,
                    // The verdict rests on the observed evaluation ledger; the claim
                    // premises only echo the caller's question.
                    evidence: Some(EvidenceClass::Observation),
                }),
            ))
        }))
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        // A class whose recorded outcomes are all self-reported abstains
        // (`TRUST_SELF_REPORTED`, MEDULLA-DECISION-ENGINE-R752.752); an abstention
        // resolves as one, every verdict resolves as a constraint.
        Some(match outcome {
            StatisticalOutcome::Abstained { .. } => ResolutionKind::Abstention,
            _ => ResolutionKind::Constraint,
        })
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Observation)
    }
}
