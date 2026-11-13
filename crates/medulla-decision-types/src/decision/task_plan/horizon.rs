//! Horizon partition and credit checkpoints over a task plan
//! (MEDULLA-DECISION-ENGINE-R134.11).
//!
//! The canonical plan is layered by dependency depth and cut into horizons at
//! layer boundaries, each at most `horizon.max_steps` steps. A horizon's credit
//! spend is recorded as a checkpoint whose three parts always add up to the
//! ceiling. Nothing here reads a clock or a store.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use medulla_types::refusal_code::RefusalCode;

use super::TaskPlan;
use crate::decision::knob_families::task_plan::{HORIZON_CREDIT_CEILING, HORIZON_MAX_STEPS};
use crate::decision::knobs::KnobValue;
use crate::decision::policy::DecisionPolicy;

/// The plan has more steps than a horizon holds and no layer boundary to cut at.
pub const HORIZON_UNSPLITTABLE: &str = "HORIZON_UNSPLITTABLE";
/// A checkpoint's rule is not the fixed accounting rule, or its parts overflow
/// or do not add up to the ceiling.
pub const HORIZON_BUDGET_RULE_INVALID: &str = "HORIZON_BUDGET_RULE_INVALID";
/// A checkpoint's consumed and reserved credit exceed the ceiling.
pub const HORIZON_OVER_CREDIT: &str = "HORIZON_OVER_CREDIT";
/// The previous horizon has no stored checkpoint, so the next may not launch.
pub const HORIZON_CHECKPOINT_MISSING: &str = "HORIZON_CHECKPOINT_MISSING";
/// The checkpoint's plan is not the plan, or the horizon is not one of the
/// horizons, the plan's policy derives.
pub const HORIZON_PLAN_STALE: &str = "HORIZON_PLAN_STALE";

/// The registered codes of the horizon checkpoint commit.
pub const REFUSALS: &[RefusalCode] = &[
    RefusalCode::new(HORIZON_BUDGET_RULE_INVALID),
    RefusalCode::new(HORIZON_OVER_CREDIT),
    RefusalCode::new(HORIZON_CHECKPOINT_MISSING),
    RefusalCode::new(HORIZON_PLAN_STALE),
];

/// The one accounting rule a checkpoint states.
pub const ACCOUNTING_RULE: &str = "consumed+reserved+remaining==ceiling";

/// The knobs the partition reads, resolved from the pinned decision policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HorizonKnobs {
    /// `horizon.max_steps`.
    pub max_steps: usize,
    /// `horizon.credit_ceiling` in tokens.
    pub credit_ceiling: u64,
}

impl HorizonKnobs {
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let integer = |spec, fallback: u64| match policy.knob(spec) {
            KnobValue::Integer(value) => u64::try_from(value).unwrap_or(fallback),
            _ => fallback,
        };
        Self {
            max_steps: integer(&HORIZON_MAX_STEPS, 8) as usize,
            credit_ceiling: integer(&HORIZON_CREDIT_CEILING, 200_000),
        }
    }
}

/// One horizon: the steps launched together, in canonical order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct Horizon {
    pub index: u32,
    pub step_ids: Vec<String>,
}

/// The horizons of a plan and the credit ceiling each is accounted against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HorizonPartition {
    /// The digest of the plan the partition was derived from, without it.
    pub plan_digest: String,
    pub horizons: Vec<Horizon>,
    pub credit_ceiling: u64,
}

/// The credit state after one horizon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct HorizonCheckpoint {
    pub plan_digest: String,
    pub horizon_index: u32,
    pub consumed: u64,
    pub reserved: u64,
    pub remaining: u64,
    pub ceiling: u64,
    /// Always [`ACCOUNTING_RULE`].
    pub rule: String,
}

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// Layer the plan by dependency depth and cut it into horizons.
pub fn partition_horizons(
    plan: &TaskPlan,
    knobs: &HorizonKnobs,
) -> Result<HorizonPartition, String> {
    let mut depth: BTreeMap<&str, usize> = BTreeMap::new();
    let mut layers: Vec<Vec<String>> = Vec::new();
    // The canonical order is topological, so each dependency has its depth.
    for task in plan.tasks.iter() {
        let level = task
            .depends_on
            .iter()
            .map(|dep| depth[dep.as_str()] + 1)
            .max()
            .unwrap_or(0);
        depth.insert(task.task_id.as_str(), level);
        if layers.len() <= level {
            layers.resize(level + 1, Vec::new());
        }
        layers[level].push(task.task_id.clone());
    }
    let max = knobs.max_steps.max(1);
    if plan.tasks.len() > max && layers.len() == 1 {
        return Err(refuse(
            HORIZON_UNSPLITTABLE,
            format!(
                "{} steps exceed horizon.max_steps={max} and the plan has no layer boundary",
                plan.tasks.len()
            ),
        ));
    }
    let mut horizons: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for layer in layers {
        if !current.is_empty() && current.len() + layer.len() > max {
            horizons.push(std::mem::take(&mut current));
        }
        current.extend(layer);
    }
    if !current.is_empty() {
        horizons.push(current);
    }
    Ok(HorizonPartition {
        plan_digest: plan.plan_digest.clone(),
        horizons: horizons
            .into_iter()
            .enumerate()
            .map(|(index, step_ids)| Horizon {
                index: index as u32,
                step_ids,
            })
            .collect(),
        credit_ceiling: knobs.credit_ceiling,
    })
}

/// Check one checkpoint against the partition of its plan, before it is stored.
/// The earlier checkpoint's presence is checked by the writer, which holds the store.
pub fn check_horizon_checkpoint(
    checkpoint: &HorizonCheckpoint,
    partition: &HorizonPartition,
) -> Result<(), String> {
    if checkpoint.plan_digest != partition.plan_digest {
        return Err(refuse(
            HORIZON_PLAN_STALE,
            "the checkpoint names another plan revision",
        ));
    }
    if usize::try_from(checkpoint.horizon_index)
        .map_or(true, |index| index >= partition.horizons.len())
    {
        return Err(refuse(
            HORIZON_PLAN_STALE,
            format!("the plan has no horizon {}", checkpoint.horizon_index),
        ));
    }
    if checkpoint.rule != ACCOUNTING_RULE {
        return Err(refuse(
            HORIZON_BUDGET_RULE_INVALID,
            format!("the rule must be `{ACCOUNTING_RULE}`"),
        ));
    }
    if checkpoint.ceiling != partition.credit_ceiling {
        return Err(refuse(
            HORIZON_BUDGET_RULE_INVALID,
            "the ceiling is not the plan's credit ceiling",
        ));
    }
    let spent = checkpoint
        .consumed
        .checked_add(checkpoint.reserved)
        .ok_or_else(|| refuse(HORIZON_BUDGET_RULE_INVALID, "consumed + reserved overflows"))?;
    if spent > checkpoint.ceiling {
        return Err(refuse(
            HORIZON_OVER_CREDIT,
            format!(
                "consumed + reserved = {spent} exceeds the ceiling {}",
                checkpoint.ceiling
            ),
        ));
    }
    let total = spent
        .checked_add(checkpoint.remaining)
        .ok_or_else(|| refuse(HORIZON_BUDGET_RULE_INVALID, "the parts overflow"))?;
    if total != checkpoint.ceiling {
        return Err(refuse(
            HORIZON_BUDGET_RULE_INVALID,
            format!(
                "the parts add up to {total}, not the ceiling {}",
                checkpoint.ceiling
            ),
        ));
    }
    Ok(())
}

/// The decision-artifact key of one horizon's checkpoint.
pub fn checkpoint_key(plan_digest: &str, horizon_index: u32) -> String {
    format!("run_record/horizon_checkpoint/{plan_digest}/{horizon_index:04}")
}

/// Compute and attach the partition, recomputing the plan digest so it covers it.
pub fn attach_horizons(plan: &mut TaskPlan, knobs: &HorizonKnobs) -> Result<(), String> {
    plan.horizons = None;
    plan.plan_digest = super::compute_plan_digest(plan);
    let partition = partition_horizons(plan, knobs)?;
    plan.horizons = Some(partition);
    plan.plan_digest = super::compute_plan_digest(plan);
    Ok(())
}
