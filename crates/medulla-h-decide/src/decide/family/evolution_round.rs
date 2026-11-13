//! The `round_verdict` and `stage_allocation` question families
//! (MEDULLA-DECISION-ENGINE-R136.4).
//!
//! A round verdict compares a round's declared fixes and regressions with the
//! observed task flips; a stage allocation spends a run budget on stronger
//! models where the impact is highest. Both are evaluate-only: they write
//! nothing, and `DecisionLog` `commit` stores the sealed record.

use medulla_decision_outcome_types::decision::evolution_round::{
    allocate_stage_models, check_grading, decide_round_verdict, review_criteria,
    summarize_allocation, tally_round, AllocationRefusal, CriterionRefusal, CriterionReport,
    EvolutionDefaults, JudgeRun, ModelOption, PredictionDeclaration, RoundRefusal,
    RoundTaskOutcome, RoundVerdict, RubricDeclaration, RunBudget, StageInput,
};
use medulla_decision_stats::decision::evolution_round::{
    judge_outcome_divergence, DivergenceKnobs,
};
use medulla_decision_types::decision::context_package::is_sha256_hex;
use medulla_decision_types::decision::harness_contrast::ContrastKnobs;
use medulla_decision_types::decision::knob_families::{criterion_review, round_verdict};
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

use super::support::{abstained, advisory, invalid, json_param, param, single_option};
use super::DecisionFamily;

/// The one option a round verdict question declares.
pub const ROUND_OPTION: &str = "round";
/// The one option a stage allocation question declares.
pub const ALLOCATION_OPTION: &str = "allocation";

/// The family of the `round_verdict` question kind.
pub struct RoundVerdictFamily;

/// The family of the `stage_allocation` question kind.
pub struct StageAllocationFamily;

/// The one option a criterion review question declares.
pub const REVIEW_OPTION: &str = "review";
/// The option an accepted criterion review names.
pub const ACCEPT_OPTION: &str = "accept";

/// The family of the `criterion_review` question kind
/// (MEDULLA-DECISION-ENGINE-R136.101).
pub struct CriterionReviewFamily;

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

/// The prior and the new grading the review compares, each validated.
fn review_params(request: &DecideRequest) -> Result<(CriterionReport, CriterionReport), String> {
    let prior: CriterionReport = json_param(request, "prior_report")?;
    let new: CriterionReport = json_param(request, "new_report")?;
    for (name, report) in [("prior_report", &prior), ("new_report", &new)] {
        for (field, digest) in [
            ("rubric_digest", &report.rubric_digest),
            ("artifact_digest", &report.artifact_digest),
        ] {
            if !is_sha256_hex(digest) {
                return Err(invalid(format!(
                    "{name}.{field} is not 64 lowercase hex characters"
                )));
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        for criterion in &report.criteria {
            if criterion.criterion_id.is_empty() || !ids.insert(criterion.criterion_id.as_str()) {
                return Err(invalid(format!(
                    "{name} names a criterion id that is empty or repeated"
                )));
            }
        }
    }
    Ok((prior, new))
}

/// The rubric and the declared artifact digest the grading is checked against
/// (MEDULLA-DECISION-ENGINE-R136.102).
fn grading_params(request: &DecideRequest) -> Result<(RubricDeclaration, String), String> {
    let rubric: RubricDeclaration = json_param(request, "rubric")?;
    let artifact: String = json_param(request, "artifact_digest")?;
    if !is_sha256_hex(&rubric.rubric_digest) || !is_sha256_hex(&artifact) {
        return Err(invalid(
            "rubric.rubric_digest and artifact_digest must be 64 lowercase hex characters",
        ));
    }
    Ok((rubric, artifact))
}

/// The judge and outcome history of past runs, oldest first, when the request
/// carries one.
fn judge_runs(request: &DecideRequest) -> Result<Option<Vec<JudgeRun>>, String> {
    match param(request, "runs")? {
        None => Ok(None),
        Some(_) => json_param(request, "runs").map(Some),
    }
}

/// Refuse a request as `PARAMETER_INVALID` carrying the criterion code.
fn criterion_invalid(refusal: &CriterionRefusal) -> String {
    invalid(format!(
        "{}: {}",
        DecisionRefusal::code(refusal).as_str(),
        refusal.detail()
    ))
}

impl DecisionFamily for CriterionReviewFamily {
    fn validate_candidates(
        &self,
        options: &[DeclaredOption],
        _principal: &str,
    ) -> Result<(), String> {
        single_option(options, REVIEW_OPTION)
    }

    // Grading completeness and artifact binding (R136.102): both reports must
    // grade every criterion of the rubric with evidence, and the new report must
    // grade the declared artifact. Nothing is compared until this passes.
    fn constraints(&self, request: &DecideRequest) -> Result<(), String> {
        let (prior, new) = review_params(request)?;
        let (rubric, artifact) = grading_params(request)?;
        judge_runs(request)?;
        check_grading(&prior, &rubric, None).map_err(|r| criterion_invalid(&r))?;
        check_grading(&new, &rubric, Some(&artifact)).map_err(|r| criterion_invalid(&r))
    }

    fn conclude(
        &self,
        _ctx: &ExecutionContext,
        request: &DecideRequest,
        policy: &DecisionPolicy,
        _graph_view: Option<&GraphView>,
    ) -> Option<Result<Executed, String>> {
        Some((|| {
            let (prior, new) = review_params(request)?;
            let rubric = || FamilyPremise {
                subject: "criterion",
                fact: format!("criterion.rubric_digest={}", prior.rubric_digest),
                class: PremiseClass::Claim,
                source: FamilyPremiseSource::Request("params.prior_report"),
            };
            // The judge score trend must not run away from the independent outcome
            // trend (R136.103); the review holds whatever the criteria say.
            let mut divergence = None;
            let divergence_premise = |micros: i64| FamilyPremise {
                subject: "criterion",
                fact: format!("criterion.divergence={micros}"),
                class: PremiseClass::Claim,
                source: FamilyPremiseSource::Request("params.runs"),
            };
            if let Some(runs) = judge_runs(request)? {
                let knobs = DivergenceKnobs {
                    window: criterion_review::divergence_window(policy),
                    threshold_micros: criterion_review::divergence_threshold_micros(policy),
                };
                match judge_outcome_divergence(&runs, &knobs) {
                    Ok(reading) => divergence = Some(divergence_premise(reading.divergence_micros)),
                    Err(refusal) => {
                        let micros = match &refusal {
                            CriterionRefusal::JudgeOutcomeDivergence {
                                divergence_micros, ..
                            } => *divergence_micros,
                            _ => 0,
                        };
                        let mut held = abstained(QuestionKind::CriterionReview, &refusal);
                        held.family_premises = vec![rubric(), divergence_premise(micros)];
                        return Ok(held);
                    }
                }
            }
            match review_criteria(&prior, &new) {
                Ok(_) => {
                    let mut executed = advisory(vec![ACCEPT_OPTION.to_string()])?;
                    executed.family_premises =
                        std::iter::once(rubric()).chain(divergence).collect();
                    Ok(executed)
                }
                // A regression holds the revision: an abstention that names the
                // first failing criterion.
                Err(refusal @ CriterionRefusal::Regression { .. }) => {
                    let CriterionRefusal::Regression { criterion_id } = &refusal else {
                        unreachable!("matched above")
                    };
                    let mut held = abstained(QuestionKind::CriterionReview, &refusal);
                    held.family_premises = vec![
                        rubric(),
                        FamilyPremise {
                            subject: "criterion",
                            fact: format!("criterion.regressed:{criterion_id}"),
                            class: PremiseClass::Claim,
                            source: FamilyPremiseSource::Request("params.new_report"),
                        },
                    ];
                    Ok(held)
                }
                // Every other refusal makes the reports incomparable or ungraded:
                // the request is refused (the rubric mismatch, and grading checks
                // that `constraints` already ran).
                Err(refusal) => Err(criterion_invalid(&refusal)),
            }
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
