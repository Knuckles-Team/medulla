//! The weighted critical chain over a task plan (MEDULLA-DECISION-ENGINE-R134.15).
//!
//! Each step carries an expected duration and a priority weight. A step's
//! weight is `duration_ms * priority_weight`; the chain is the dependency path
//! with the heaviest total, ending at the output task. Equal totals choose the
//! lexicographically smallest sequence of step ids, so the answer is
//! deterministic. The chain is data on the plan; it grants nothing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use medulla_query_types::solve_scalar::digest_text;

use super::{compute_plan_digest, TaskPlan};
use crate::decision::knob_families::task_plan::WEIGHT_SCALE;
use crate::decision::knobs::KnobValue;
use crate::decision::policy::DecisionPolicy;

/// Some tasks carry a duration or priority weight and others do not, or a task
/// carries only one of the two.
pub const CRITICAL_PATH_WEIGHT_MISSING: &str = "CRITICAL_PATH_WEIGHT_MISSING";
/// A task's duration is zero.
pub const CRITICAL_PATH_INVALID_DURATION: &str = "CRITICAL_PATH_INVALID_DURATION";
/// A step weight or a path total does not fit in `u64`.
pub const CRITICAL_PATH_OVERFLOW: &str = "CRITICAL_PATH_OVERFLOW";

/// The knobs the critical chain reads, resolved from the pinned decision policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CriticalPathKnobs {
    /// `task_plan.weight_scale`: units of priority weight in one whole weight.
    pub weight_scale: u64,
}

impl CriticalPathKnobs {
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let weight_scale = match policy.knob(&WEIGHT_SCALE) {
            KnobValue::Integer(value) => u64::try_from(value).ok().filter(|v| *v > 0),
            _ => None,
        };
        Self {
            weight_scale: weight_scale.unwrap_or(100),
        }
    }
}

/// The heaviest weighted path of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct CriticalChain {
    /// The step ids of the chain, first step to the output task.
    pub critical_chain_step_ids: Vec<String>,
    /// The chain's summed `duration_ms * priority_weight`, in weight units.
    pub total_weight_units: u64,
    /// The summed weight divided by `weight_scale`, rounded down.
    pub total_weighted_duration_ms: u64,
    /// `task_plan.weight_scale` the total was computed with.
    pub weight_scale: u64,
    pub policy_revision: String,
    /// The digest of the plan the chain was computed over, without the chain.
    pub plan_digest: String,
    /// The digest over every task's duration and weight.
    pub input_digest: String,
}

fn refuse(code: &str, detail: impl std::fmt::Display) -> String {
    format!("{code}: {detail}")
}

/// Whether any task of the plan carries a duration or a priority weight.
pub fn carries_weights(plan: &TaskPlan) -> bool {
    plan.tasks
        .iter()
        .any(|t| t.duration_ms.is_some() || t.priority_weight.is_some())
}

/// The weights of every task: `(duration_ms, priority_weight)` by task id.
fn weights(plan: &TaskPlan) -> Result<BTreeMap<&str, (u32, u32)>, String> {
    let mut out = BTreeMap::new();
    for task in plan.tasks.iter() {
        let (Some(duration), Some(weight)) = (task.duration_ms, task.priority_weight) else {
            return Err(refuse(
                CRITICAL_PATH_WEIGHT_MISSING,
                format!(
                    "{} must carry both duration_ms and priority_weight",
                    task.task_id
                ),
            ));
        };
        if duration == 0 {
            return Err(refuse(
                CRITICAL_PATH_INVALID_DURATION,
                format!("{} has a zero duration", task.task_id),
            ));
        }
        out.insert(task.task_id.as_str(), (duration, weight));
    }
    Ok(out)
}

/// The weighted critical chain of `plan`, ending at its output task.
pub fn critical_chain(plan: &TaskPlan, knobs: &CriticalPathKnobs) -> Result<CriticalChain, String> {
    let weights = weights(plan)?;
    // The canonical order is topological, so every dependency is settled first.
    let mut best: BTreeMap<&str, (u64, Vec<&str>)> = BTreeMap::new();
    for task in plan.tasks.iter() {
        let (duration, weight) = weights[task.task_id.as_str()];
        let step = u64::from(duration)
            .checked_mul(u64::from(weight))
            .ok_or_else(|| {
                refuse(
                    CRITICAL_PATH_OVERFLOW,
                    format!("step weight of {}", task.task_id),
                )
            })?;
        let mut chosen: Option<(u64, Vec<&str>)> = None;
        let mut candidates: Vec<(u64, Vec<&str>)> = Vec::new();
        for dependency in task.depends_on.iter() {
            let (total, path) = &best[dependency.as_str()];
            let mut extended = path.clone();
            extended.push(task.task_id.as_str());
            candidates.push((*total, extended));
        }
        if candidates.is_empty() {
            candidates.push((0, vec![task.task_id.as_str()]));
        }
        for (total, path) in candidates {
            let better = match &chosen {
                None => true,
                Some((held, held_path)) => total > *held || (total == *held && path < *held_path),
            };
            if better {
                chosen = Some((total, path));
            }
        }
        let (prior, path) = chosen.expect("a task has a path");
        let total = prior.checked_add(step).ok_or_else(|| {
            refuse(
                CRITICAL_PATH_OVERFLOW,
                format!("path total at {}", task.task_id),
            )
        })?;
        best.insert(task.task_id.as_str(), (total, path));
    }
    let (total, path) = best
        .remove(plan.output_task_id.as_str())
        .expect("the output task is in the plan");
    let input: Vec<(&str, u32, u32)> = weights
        .iter()
        .map(|(id, (duration, weight))| (*id, *duration, *weight))
        .collect();
    Ok(CriticalChain {
        critical_chain_step_ids: path.into_iter().map(str::to_string).collect(),
        total_weight_units: total,
        total_weighted_duration_ms: total / knobs.weight_scale,
        weight_scale: knobs.weight_scale,
        policy_revision: plan.policy_revision.clone(),
        plan_digest: plan.plan_digest.clone(),
        input_digest: digest_text("eg/task-plan-critical-path-input/v1", &input),
    })
}

/// Compute the chain of a plan whose tasks carry weights and attach it,
/// recomputing the plan digest so it covers the chain. A plan with no weights
/// is left as it is.
pub fn attach_critical_chain(plan: &mut TaskPlan, knobs: &CriticalPathKnobs) -> Result<(), String> {
    if !carries_weights(plan) {
        return Ok(());
    }
    plan.critical_chain = None;
    plan.plan_digest = compute_plan_digest(plan);
    let chain = critical_chain(plan, knobs)?;
    plan.critical_chain = Some(chain);
    plan.plan_digest = compute_plan_digest(plan);
    Ok(())
}
