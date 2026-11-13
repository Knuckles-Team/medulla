//! The `round_verdict` and `stage_allocation` question families
//! (MEDULLA-DECISION-ENGINE-R136.4).
//!
//! A round verdict compares a round's declared fixes and regressions with the
//! observed task flips; a stage allocation spends a run budget on stronger
//! models where the impact is highest. Both are evaluate-only: they write
//! nothing, and `DecisionLog` `commit` stores the sealed record.

use medulla_decision_outcome_types::decision::evolution_round::{
    allocate_stage_models, decide_round_verdict, summarize_allocation, tally_round,
    AllocationRefusal, EvolutionDefaults, ModelOption, PredictionDeclaration, RoundRefusal,
    RoundTaskOutcome, RoundVerdict, RunBudget, StageInput,
};
use medulla_decision_types::decision::harness_contrast::ContrastKnobs;
use medulla_decision_types::decision::knob_families::round_verdict;
use medulla_decision_types::decision::numeric::UnitRationalWire;
use medulla_decision_types::decision::record::outcome::DecisionRefusal;
use medulla_decision_types::decision::statistical::declared::DeclaredOption;
use medulla_decision_types::decision::statistical::{
    DecideRequest, QuestionKind, StatisticalOutcome,
};
use medulla_decision_types::decision::{DecisionPolicy, EvidenceClass, ResolutionKind};
use medulla_decision_types::decision::{PremiseClass, PremiseRef};
use medulla_graph_core::graph::GraphView;

use crate::decide::stat_executor::{
    Executed, ExecutionContext, FamilyPremise, FamilyPremiseSource,
};

use super::support::{abstained, advisory, invalid, json_param, single_option};
use super::DecisionFamily;

/// The one option a round verdict question declares.
pub const ROUND_OPTION: &str = "round";
/// The one option a stage allocation question declares.
pub const ALLOCATION_OPTION: &str = "allocation";

/// The family of the `round_verdict` question kind.
pub struct RoundVerdictFamily;

/// The family of the `stage_allocation` question kind.
pub struct StageAllocationFamily;

/// The verdict's wire name.
pub fn verdict_name(verdict: RoundVerdict) -> &'static str {
    match verdict {
        RoundVerdict::Verified => "verified",
        RoundVerdict::Falsified => "falsified",
        RoundVerdict::Unverified => "unverified",
    }
}

/// The evolution defaults the pinned policy sets: each knob, or its default.
pub fn defaults_from_policy(policy: &DecisionPolicy) -> Result<EvolutionDefaults, String> {
    let flip_fraction =
        UnitRationalWire::new(round_verdict::flip_fraction_micros(policy), 1_000_000)
            .map_err(invalid)?;
    Ok(EvolutionDefaults {
        rounds: round_verdict::rounds(policy),
        flip_fraction,
    })
}

fn round_params(
    request: &DecideRequest,
) -> Result<(PredictionDeclaration, Vec<RoundTaskOutcome>), String> {
    Ok((
        json_param(request, "prediction")?,
        json_param(request, "outcomes")?,
    ))
}

impl DecisionFamily for RoundVerdictFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, ROUND_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        round_params(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let (prediction, outcomes) = round_params(request)?;
            if prediction.fixes.is_empty() && prediction.regressions.is_empty() {
                return Ok(abstained(
                    QuestionKind::RoundVerdict,
                    &RoundRefusal::NoPrediction,
                ));
            }
            let defaults = defaults_from_policy(policy)?;
            let verdict = decide_round_verdict(&prediction, &outcomes, &defaults);
            let tally = tally_round(&prediction, &outcomes);
            let micros = round_verdict::flip_fraction_micros(policy);
            let premise = |fact: String, class, source| FamilyPremise {
                subject: "round",
                fact,
                class,
                source,
            };
            let observed = FamilyPremiseSource::Request("params.outcomes");
            let mut executed = advisory(vec![verdict_name(verdict).to_string()])?;
            executed.family_premises = vec![
                premise(
                    format!(
                        "round.flip_fraction={}.{:06}",
                        micros / 1_000_000,
                        micros % 1_000_000
                    ),
                    PremiseClass::Definition,
                    FamilyPremiseSource::Policy,
                ),
                premise(
                    format!("round.declared_total={}", tally.declared_total),
                    PremiseClass::Claim,
                    FamilyPremiseSource::Request("params.prediction"),
                ),
                premise(
                    format!("round.as_declared={}", tally.as_declared),
                    PremiseClass::Claim,
                    observed,
                ),
                premise(
                    format!("round.opposite={}", tally.opposite),
                    PremiseClass::Claim,
                    FamilyPremiseSource::Request("params.outcomes"),
                ),
                premise(
                    format!("round.verdict={}", verdict_name(verdict)),
                    PremiseClass::Claim,
                    FamilyPremiseSource::Request("params.outcomes"),
                ),
            ];
            Ok(executed)
        })())
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        sealed(executed, policy_digest)
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(ResolutionKind::Constraint)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Observation)
    }
}

/// The premises the family computed while it concluded, sealed to the policy.
fn sealed(executed: &Executed, policy_digest: &str) -> Vec<PremiseRef> {
    executed
        .family_premises
        .iter()
        .map(|premise| premise.seal(policy_digest))
        .collect()
}

fn allocation_params(
    request: &DecideRequest,
) -> Result<(Vec<StageInput>, Vec<ModelOption>, RunBudget), String> {
    Ok((
        json_param(request, "stages")?,
        json_param(request, "models")?,
        json_param(request, "budget")?,
    ))
}

fn refused(refusal: AllocationRefusal) -> String {
    invalid(format!(
        "{}: {}",
        DecisionRefusal::code(&refusal).as_str(),
        refusal.detail()
    ))
}

impl DecisionFamily for StageAllocationFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, ALLOCATION_OPTION)
    }

    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        allocation_params(request).map(|_| ())
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let (stages, models, budget) = allocation_params(request)?;
            let knobs = ContrastKnobs::from_policy(policy);
            let allocations =
                allocate_stage_models(&stages, &models, &budget, &knobs).map_err(refused)?;
            let summary = summarize_allocation(&models, &allocations);
            let premise = |fact: String, source| FamilyPremise {
                subject: "allocation",
                fact,
                class: PremiseClass::Claim,
                source,
            };
            let mut premises = vec![
                premise(
                    format!("allocation.max_cost={}", budget.max_cost),
                    FamilyPremiseSource::Request("params.budget"),
                ),
                premise(
                    format!("allocation.cost={}", summary.cost),
                    FamilyPremiseSource::Request("params.models"),
                ),
                premise(
                    format!("allocation.upgrades={}", summary.upgrades),
                    FamilyPremiseSource::Request("params.models"),
                ),
            ];
            for stage in &stages {
                premises.push(match stage.impact {
                    Some(impact) => premise(
                        format!("allocation.stage_impact:{}={impact}", stage.stage),
                        FamilyPremiseSource::Request("params.stages"),
                    ),
                    None => premise(
                        format!("allocation.impact_missing:{}", stage.stage),
                        FamilyPremiseSource::Request("params.stages"),
                    ),
                });
            }
            let mut executed = advisory(
                allocations
                    .iter()
                    .map(|a| format!("{}={}", a.stage, a.model_ref))
                    .collect(),
            )?;
            executed.family_premises = premises;
            Ok(executed)
        })())
    }

    fn premises(
        &self,
        _request: &DecideRequest,
        executed: &Executed,
        policy_digest: &str,
    ) -> Vec<PremiseRef> {
        sealed(executed, policy_digest)
    }

    fn resolution_kind(&self, outcome: &StatisticalOutcome) -> Option<ResolutionKind> {
        matches!(outcome, StatisticalOutcome::Advisory { .. })
            .then_some(ResolutionKind::Optimization)
    }

    fn evidence_class(&self, outcome: &StatisticalOutcome) -> Option<EvidenceClass> {
        matches!(outcome, StatisticalOutcome::Advisory { .. }).then_some(EvidenceClass::Claim)
    }
}
