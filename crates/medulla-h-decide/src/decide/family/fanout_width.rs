//! The `fanout_width` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R752.754).
//!
//! Picks how many replicas to fan out from their mean diversity with the pure
//! `fanout_width` kernel, never above the cap the budget and `knob
//! fanout.replica_ceiling` allow. The spread is a caller claim, so the record is
//! a `Claim` resolved as `Statistical`. The harness starts the replicas.

use medulla_decision_stats::decision::population::{fanout_cap, fanout_width, mean_diversity};
use medulla_decision_types::decision::population::{
    FanoutRequest, PopulationKnobs, FANOUT_BUDGET_TOO_LOW,
};
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{DecisionPolicy, EvidenceClass, ResolutionKind};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{Executed, ExecutionContext, FamilyExecuted};
use crate::decide::stat_support::invalid_parameter;

use super::kernel::{abstained, advisory, body, claim, one_option, shape};
use super::DecisionFamily;

const KIND: &str = "fanout_width";
const PARAM: &str = "fanout_width";

fn parse(request: &DecideRequest) -> Result<(&FanoutRequest, &str), String> {
    let options = shape(request, KIND)?;
    let TypedValue::FanoutWidth(fanout) = body(request, KIND, PARAM)? else {
        return Err(invalid_parameter(format!(
            "{KIND} parameter {PARAM} has the wrong type"
        )));
    };
    Ok((&**fanout, one_option(options, KIND)?))
}

/// The family of the `fanout_width` question kind.
pub struct FanoutWidthFamily;

impl DecisionFamily for FanoutWidthFamily {
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
        Some(parse(request).and_then(|(fanout, subject)| {
            let knobs = PopulationKnobs::from_policy(policy);
            fanout.spread.validate()?;
            if fanout.current_width > 0
                && fanout.cost_per_replica > 0
                && fanout_cap(fanout.budget, fanout.cost_per_replica, &knobs) == 0
            {
                // Valid input the budget cannot answer: abstain.
                return Ok(Executed::from_family(
                    abstained(
                        QuestionKind::FanoutWidth,
                        FANOUT_BUDGET_TOO_LOW,
                        format!(
                            "budget {} does not cover one replica of cost {}",
                            fanout.budget, fanout.cost_per_replica
                        ),
                    )?,
                    None,
                ));
            }
            let width = fanout_width(
                &fanout.spread,
                fanout.current_width,
                fanout.budget,
                fanout.cost_per_replica,
                &knobs,
            )?;
            let field = "params.fanout_width";
            let premises = vec![
                claim(
                    subject,
                    format!("fanout:mean_diversity={}", mean_diversity(&fanout.spread)),
                    field,
                ),
                claim(subject, format!("fanout:width={width}"), field),
            ];
            Ok(Executed::from_family(
                advisory(),
                Some(FamilyExecuted {
                    answer: Some(FamilyAnswer::FanoutWidth { width }),
                    premises,
                    resolution: None,
                    evidence: None,
                }),
            ))
        }))
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. })
            .then_some(ResolutionKind::Statistical)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Claim)
    }
}
