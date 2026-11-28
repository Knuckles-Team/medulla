//! The `population_drift` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R752.754).
//!
//! Reads the first Wasserstein distance between a reference and a current score
//! window with the pure `drift_reading` kernel; it alerts above `knob
//! drift.alert`. The windows are caller claims, so the record is a `Claim`
//! resolved as `Statistical`. The harness collects the windows.

use medulla_decision_stats_records::decision::population::drift_reading;
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_graph_core::graph::GraphView;
use medulla_types::evidence_class::EvidenceClass;
use {
    medulla_decision_policy_types::decision::population::max_samples,
    medulla_decision_policy_types::decision::population::DriftRequest,
    medulla_decision_policy_types::decision::population::PopulationKnobs,
    medulla_decision_policy_types::decision::population::DRIFT_EMPTY_WINDOW,
};
use {
    medulla_decision_statistical_types::decision::statistical::errors::StatisticalErrorCode,
    medulla_decision_statistical_types::decision::statistical::question_kind::QuestionKind,
    medulla_decision_types::decision::statistical::DecideRequest,
    medulla_decision_types::decision::statistical::StatisticalOutcome,
    medulla_decision_types::decision::statistical::TypedValue,
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
    medulla_h_decide_base::decide::family::kernel::abstained,
    medulla_h_decide_base::decide::family::kernel::advisory,
    medulla_h_decide_base::decide::family::kernel::body,
    medulla_h_decide_base::decide::family::kernel::claim,
    medulla_h_decide_base::decide::family::kernel::one_option,
    medulla_h_decide_base::decide::family::kernel::shape,
};

const KIND: &str = "population_drift";
const PARAM: &str = "population_drift";

fn parse(request: &DecideRequest) -> Result<(&DriftRequest, &str), String> {
    let options = shape(request, KIND)?;
    let TypedValue::PopulationDrift(drift) = body(request, KIND, PARAM)? else {
        return Err(invalid_parameter(format!(
            "{KIND} parameter {PARAM} has the wrong type"
        )));
    };
    Ok((&**drift, one_option(options, KIND)?))
}

/// The family of the `population_drift` question kind.
pub struct PopulationDriftFamily;

impl DecisionFamily for PopulationDriftFamily {
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
        Some(parse(request).and_then(|(drift, subject)| {
            let limit = max_samples(policy);
            if drift.reference.len() > limit || drift.current.len() > limit {
                return Err(StatisticalErrorCode::DriftWindowTooLarge.refusal(format!(
                    "a drift window holds at most {limit} samples (knob drift.max_samples)"
                )));
            }
            if drift.reference.is_empty() || drift.current.is_empty() {
                // Valid input the evidence cannot answer: abstain, never a zero distance.
                return Ok(Executed::from_family(
                    abstained(
                        QuestionKind::PopulationDrift,
                        DRIFT_EMPTY_WINDOW,
                        format!(
                            "the {} window is empty",
                            if drift.reference.is_empty() {
                                "reference"
                            } else {
                                "current"
                            }
                        ),
                    )?,
                    None,
                ));
            }
            let knobs = PopulationKnobs::from_policy(policy);
            let reading =
                drift_reading(drift.reference.as_slice(), drift.current.as_slice(), &knobs)
                    .map_err(invalid_parameter)?;
            let field = "params.population_drift";
            let premises = vec![
                claim(
                    subject,
                    format!("drift:distance={}", reading.distance.value.value),
                    field,
                ),
                claim(
                    subject,
                    format!(
                        "drift:windows={}/{}",
                        reading.reference_n, reading.current_n
                    ),
                    field,
                ),
            ];
            Ok(Executed::from_family(
                advisory(),
                Some(FamilyExecuted {
                    answer: Some(FamilyAnswer::PopulationDrift(Box::new(reading))),
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
