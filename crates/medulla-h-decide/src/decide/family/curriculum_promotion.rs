//! The `curriculum_promotion` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R751.754).
//!
//! The one declared option is the stage; its latest `knob curriculum.window_runs`
//! independently evaluated outcomes are read from the caller's own decision log
//! (`recent_evaluations`, R159.3), so another tenant's outcomes never count. The
//! pure `promotion_verdict` kernel promotes when the full window reaches the
//! stage policy's threshold. The answer is an uncalibrated advisory; the record
//! is an `Observation` resolved as `Statistical`.

use medulla_decision_stats::decision::curriculum::promotion_verdict;
use medulla_decision_types::decision::curriculum::{
    promote_floor_micros, window_runs, CurriculumRequest,
};
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    DecideRequest, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{
    DecisionPolicy, EvidenceClass, PremiseClass, PremiseProvenance, PremiseRef, ResolutionKind,
};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{Executed, ExecutionContext, FamilyExecuted};
use crate::decide::stat_log::{recent_evaluations, LogReader};
use crate::decide::stat_support::invalid_parameter;

use super::kernel::{advisory, body, claim, one_option, shape};
use super::DecisionFamily;

const KIND: &str = "curriculum_promotion";
const PARAM: &str = "curriculum_promotion";

fn parse(request: &DecideRequest) -> Result<&CurriculumRequest, String> {
    let options = shape(request, KIND)?;
    let TypedValue::CurriculumPromotion(curriculum) = body(request, KIND, PARAM)? else {
        return Err(invalid_parameter(format!(
            "{KIND} parameter {PARAM} has the wrong type"
        )));
    };
    let subject = one_option(options, KIND)?;
    if subject != curriculum.judged_stage.stage_id {
        return Err(invalid_parameter(format!(
            "{KIND} declares the stage {} as its one candidate",
            curriculum.judged_stage.stage_id
        )));
    }
    Ok(curriculum)
}

/// The family of the `curriculum_promotion` question kind.
pub struct CurriculumPromotionFamily;

impl DecisionFamily for CurriculumPromotionFamily {
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
        Some(parse(request).and_then(|curriculum| {
            let window = window_runs(policy);
            let outcomes = recent_evaluations(
                ctx.store,
                reader,
                &curriculum.judged_stage.stage_id,
                window as usize,
            )?;
            let reading = promotion_verdict(
                &curriculum.judged_stage,
                &curriculum.policy,
                &outcomes,
                window,
                promote_floor_micros(policy),
            )?;
            let subject = curriculum.judged_stage.stage_id.as_str();
            let field = "params.curriculum_promotion";
            let premises = vec![
                claim(subject, format!("curriculum:stage={subject}"), field),
                PremiseRef {
                    subject: subject.to_string(),
                    fact: format!("curriculum:rate={}/{}", reading.successes, reading.runs),
                    class: PremiseClass::Observation,
                    provenance: PremiseProvenance::Observation {
                        evaluation_id: subject.to_string(),
                    },
                    trust_tier: None,
                },
                claim(
                    subject,
                    format!(
                        "curriculum:threshold={}",
                        curriculum.policy.promote_threshold.value
                    ),
                    field,
                ),
            ];
            Ok(Executed::from_family(
                advisory(),
                Some(FamilyExecuted {
                    answer: Some(FamilyAnswer::CurriculumPromotion(Box::new(reading))),
                    premises,
                    resolution: None,
                    // The verdict rests on the observed evaluation ledger; the claim
                    // premises only echo the caller's question.
                    evidence: Some(EvidenceClass::Observation),
                }),
            ))
        }))
    }

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Statistical)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Observation)
    }
}
