//! The `population_drift` question family (MEDULLA-DECISION-ENGINE-R001.101,
//! R752.754).
//!
//! Reads the first Wasserstein distance between a reference and a current score
//! window with the pure `drift_reading` kernel; it alerts above `knob
//! drift.alert`. The windows are caller claims, so the record is a `Claim`
//! resolved as `Statistical`. The harness collects the windows.

use medulla_decision_stats::decision::population::drift_reading;
use medulla_decision_types::decision::population::{max_samples, DriftRequest, PopulationKnobs};
use medulla_decision_types::decision::statistical::outcome::FamilyAnswer;
use medulla_decision_types::decision::statistical::{
    DecideRequest, StatisticalOutcome, TypedValue,
};
use medulla_decision_types::decision::{DecisionPolicy, EvidenceClass, ResolutionKind};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{Executed, ExecutionContext, FamilyExecuted};
use crate::decide::stat_support::invalid_parameter;

use super::kernel::{advisory, body, claim, one_option, shape};
use super::DecisionFamily;

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
                return Err(invalid_parameter(format!(
                    "a drift window holds at most {limit} samples (knob drift.max_samples)"
                )));
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

    fn resolution_kind(&self, _outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        Some(ResolutionKind::Statistical)
    }

    fn evidence_class(&self, _outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        Some(EvidenceClass::Claim)
    }
}
