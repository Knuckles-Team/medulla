//! Evolution round verdicts and per-stage model allocation types
//! (MEDULLA-DECISION-ENGINE-R136.1 and R136.2).
//!
//! A round declares which tasks it expects to fix and which it expects to
//! regress. The verdict compares the observed flips with that declaration.

use serde::{Deserialize, Serialize};

use medulla_decision_types::decision::numeric::UnitRationalWire;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct RunBudget {
    pub max_cost: u64,
    pub max_stronger_stages: u32,
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
    let as_declared = declared.iter().filter(|(id, up)| flipped(id, *up)).count();
    let opposite = declared.iter().filter(|(id, up)| flipped(id, !*up)).count();
    let total = declared.len();
    match (
        meets(as_declared, total, defaults.flip_fraction),
        meets(opposite, total, defaults.flip_fraction),
    ) {
        (true, false) => RoundVerdict::Verified,
        (false, true) => RoundVerdict::Falsified,
        _ => RoundVerdict::Unverified,
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
        match self {
            Self::BudgetBelowMinimum { .. } => "ALLOCATION_BUDGET_BELOW_MINIMUM",
            Self::NoAdequateModel { .. } => "ALLOCATION_NO_ADEQUATE_MODEL",
        }
    }
}

/// Impact-first stage model allocation. Every stage starts on the base
/// model (the cheapest adequate one, lowest `model_ref` on ties). Stages with
/// an impact, by impact descending then stage id, upgrade to the strong model
/// (highest strength, then lower cost, then lower `model_ref`) while the
/// upgrade count stays below `max_stronger_stages` and total cost within
/// `max_cost`. A stage without impact stays on the base model with impact 0.
pub fn allocate_stage_models(
    stages: &[StageInput],
    models: &[ModelOption],
    budget: &RunBudget,
) -> Result<Vec<StageAllocation>, AllocationRefusal> {
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
