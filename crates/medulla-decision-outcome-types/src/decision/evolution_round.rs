//! Evolution round verdicts and per-stage model allocation types
//! (MEDULLA-DECISION-ENGINE-R136.1 and R136.2).
//!
//! A round declares which tasks it expects to fix and which it expects to
//! regress. The verdict compares the observed flips with that declaration.

use serde::{Deserialize, Serialize};

use medulla_decision_types::decision::codes;
use medulla_decision_types::decision::numeric::UnitRationalWire;
use medulla_decision_types::decision::record::outcome::{DecisionRefusal, RefusalCode};

/// The named default round count.
pub const DEFAULT_EVOLUTION_ROUNDS: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct PredictionDeclaration {
    pub round: u32,
    /// Tasks predicted to flip from fail to pass.
    pub fixes: Vec<String>,
    /// Tasks predicted to flip from pass to fail.
    pub regressions: Vec<String>,
}

/// One task's result before and after the round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RoundTaskOutcome {
    pub task_id: String,
    pub passed_before: bool,
    pub passed_after: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum RoundVerdict {
    Verified,
    Falsified,
    Unverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct EvolutionDefaults {
    pub rounds: u32,
    pub flip_fraction: UnitRationalWire,
}

impl Default for EvolutionDefaults {
    fn default() -> Self {
        Self {
            rounds: DEFAULT_EVOLUTION_ROUNDS,
            flip_fraction: UnitRationalWire::new(1, 2).expect("one half is a unit rational"),
        }
    }
}

/// Which model one evolution stage runs on, and the stage's measured impact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StageAllocation {
    pub stage: String,
    pub model_ref: String,
    pub impact: i64,
    /// The most harness candidates the stage probes
    /// (`harness_evolution.max_candidates`, MEDULLA-DECISION-ENGINE-R179.7).
    pub candidate_slots: u32,
    /// The fewest harness candidates the stage probes
    /// (`harness_evolution.min_candidates`).
    pub candidate_slots_min: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RunBudget {
    pub max_cost: u64,
    pub max_stronger_stages: u32,
}

/// The registered refusal codes of the round verdict and stage allocation
/// families (MEDULLA-DECISION-ENGINE-R136.5), in declaration order.
pub const REFUSALS: &[RefusalCode] = &[
    codes::ROUND_NO_PREDICTION,
    codes::ALLOCATION_BUDGET_BELOW_MINIMUM,
    codes::ALLOCATION_NO_ADEQUATE_MODEL,
];

/// Why a round verdict was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundRefusal {
    /// The prediction declares no fixes and no regressions.
    NoPrediction,
}

impl DecisionRefusal for RoundRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::NoPrediction => codes::ROUND_NO_PREDICTION,
        }
    }

    fn detail(&self) -> String {
        "the prediction declares no fixes and no regressions".to_string()
    }
}

/// The flips a verdict compared: how many declared tasks there are, how many
/// flipped as declared and how many flipped the opposite way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundTally {
    pub declared_total: usize,
    pub as_declared: usize,
    pub opposite: usize,
}

fn meets(count: usize, total: usize, fraction: UnitRationalWire) -> bool {
    // count / total >= numerator / denominator, in exact integers.
    total > 0
        && (count as u128) * u128::from(fraction.denominator())
            >= (total as u128) * u128::from(fraction.numerator())
}

/// Compare observed flips with the declared direction. Each direction is
/// tested on its own against `flip_fraction`; only an unambiguous result
/// verifies or falsifies.
pub fn decide_round_verdict(
    prediction: &PredictionDeclaration,
    outcomes: &[RoundTaskOutcome],
    defaults: &EvolutionDefaults,
) -> RoundVerdict {
    let tally = tally_round(prediction, outcomes);
    match (
        meets(
            tally.as_declared,
            tally.declared_total,
            defaults.flip_fraction,
        ),
        meets(tally.opposite, tally.declared_total, defaults.flip_fraction),
    ) {
        (true, false) => RoundVerdict::Verified,
        (false, true) => RoundVerdict::Falsified,
        _ => RoundVerdict::Unverified,
    }
}

/// Count the declared tasks and the flips as declared and the opposite way.
pub fn tally_round(
    prediction: &PredictionDeclaration,
    outcomes: &[RoundTaskOutcome],
) -> RoundTally {
    let flipped = |id: &str, to_pass: bool| {
        outcomes
            .iter()
            .any(|o| o.task_id == id && o.passed_before != to_pass && o.passed_after == to_pass)
    };
    let declared: Vec<(&str, bool)> = prediction
        .fixes
        .iter()
        .map(|id| (id.as_str(), true))
        .chain(prediction.regressions.iter().map(|id| (id.as_str(), false)))
        .collect();
    RoundTally {
        declared_total: declared.len(),
        as_declared: declared.iter().filter(|(id, up)| flipped(id, *up)).count(),
        opposite: declared.iter().filter(|(id, up)| flipped(id, !*up)).count(),
    }
}

/// One model a stage may run on: its cost per stage, its strength and whether
/// it is adequate for the run at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ModelOption {
    pub model_ref: String,
    pub cost: u64,
    pub strength: u32,
    pub adequate: bool,
}

/// One stage to allocate, with its measured impact when known.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct StageInput {
    pub stage: String,
    pub impact: Option<i64>,
}

/// Why a stage allocation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocationRefusal {
    /// Running every stage on the base model already exceeds `max_cost`.
    BudgetBelowMinimum { shortfall: u64 },
    /// No offered model is adequate.
    NoAdequateModel { models: usize },
}

impl AllocationRefusal {
    pub fn code(self) -> &'static str {
        DecisionRefusal::code(&self).as_str()
    }
}

impl DecisionRefusal for AllocationRefusal {
    fn code(&self) -> RefusalCode {
        match self {
            Self::BudgetBelowMinimum { .. } => codes::ALLOCATION_BUDGET_BELOW_MINIMUM,
            Self::NoAdequateModel { .. } => codes::ALLOCATION_NO_ADEQUATE_MODEL,
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::BudgetBelowMinimum { shortfall } => format!("shortfall={shortfall}"),
            Self::NoAdequateModel { models } => format!("models={models}"),
        }
    }
}

/// What an allocation spends: the cost of every stage on its model and how
/// many stages run on a model other than the base one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllocationSummary {
    pub cost: u128,
    pub upgrades: u32,
}

/// Summarise `allocations` over the offered `models`. The base model is the
/// cheapest adequate one, lowest `model_ref` on ties, as in
/// [`allocate_stage_models`].
pub fn summarize_allocation(
    models: &[ModelOption],
    allocations: &[StageAllocation],
) -> AllocationSummary {
    let base = models.iter().filter(|m| m.adequate).min_by(|a, b| {
        a.cost
            .cmp(&b.cost)
            .then_with(|| a.model_ref.cmp(&b.model_ref))
    });
    let cost_of = |model_ref: &str| {
        models
            .iter()
            .find(|m| m.model_ref == model_ref)
            .map_or(0, |m| u128::from(m.cost))
    };
    AllocationSummary {
        cost: allocations.iter().map(|a| cost_of(&a.model_ref)).sum(),
        upgrades: u32::try_from(
            allocations
                .iter()
                .filter(|a| base.is_some_and(|b| a.model_ref != b.model_ref))
                .count(),
        )
        .unwrap_or(u32::MAX),
    }
}

/// Impact-first stage model allocation. Every stage starts on the base
/// model (the cheapest adequate one, lowest `model_ref` on ties). Stages with
/// an impact, by impact descending then stage id, upgrade to the strong model
/// (highest strength, then lower cost, then lower `model_ref`) while the
/// upgrade count stays below `max_stronger_stages` and total cost within
/// `max_cost`. A stage without impact stays on the base model with impact 0.
/// Every stage carries the candidate slot bounds of the `harness_evolution`
/// knobs (R179.7).
pub fn allocate_stage_models(
    stages: &[StageInput],
    models: &[ModelOption],
    budget: &RunBudget,
    knobs: &medulla_decision_types::decision::harness_contrast::ContrastKnobs,
) -> Result<Vec<StageAllocation>, AllocationRefusal> {
    let candidate_slots = u32::try_from(knobs.max_candidates).unwrap_or(u32::MAX);
    let candidate_slots_min = u32::try_from(knobs.min_candidates).unwrap_or(u32::MAX);
    let adequate = || models.iter().filter(|m| m.adequate);
    let no_model = AllocationRefusal::NoAdequateModel {
        models: models.len(),
    };
    let base = adequate()
        .min_by(|a, b| {
            a.cost
                .cmp(&b.cost)
                .then_with(|| a.model_ref.cmp(&b.model_ref))
        })
        .ok_or(no_model)?;
    let strong = adequate()
        .min_by(|a, b| {
            b.strength
                .cmp(&a.strength)
                .then_with(|| a.cost.cmp(&b.cost))
                .then_with(|| a.model_ref.cmp(&b.model_ref))
        })
        .ok_or(no_model)?;
    let mut total = (stages.len() as u128) * u128::from(base.cost);
    let max_cost = u128::from(budget.max_cost);
    if total > max_cost {
        let shortfall = u64::try_from(total - max_cost).unwrap_or(u64::MAX);
        return Err(AllocationRefusal::BudgetBelowMinimum { shortfall });
    }
    let mut allocations: Vec<StageAllocation> = stages
        .iter()
        .map(|s| StageAllocation {
            stage: s.stage.clone(),
            model_ref: base.model_ref.clone(),
            impact: s.impact.unwrap_or(0),
            candidate_slots,
            candidate_slots_min,
        })
        .collect();
    let mut order: Vec<usize> = (0..stages.len())
        .filter(|i| stages[*i].impact.is_some())
        .collect();
    order.sort_by(|a, b| {
        stages[*b]
            .impact
            .cmp(&stages[*a].impact)
            .then_with(|| stages[*a].stage.cmp(&stages[*b].stage))
    });
    if strong.model_ref == base.model_ref {
        return Ok(allocations);
    }
    let step = u128::from(strong.cost.saturating_sub(base.cost));
    let mut upgrades = 0u32;
    for index in order {
        if upgrades >= budget.max_stronger_stages {
            break;
        }
        if total + step > max_cost {
            continue;
        }
        total += step;
        upgrades += 1;
        allocations[index].model_ref = strong.model_ref.clone();
    }
    Ok(allocations)
}

#[cfg(test)]
mod tests;
