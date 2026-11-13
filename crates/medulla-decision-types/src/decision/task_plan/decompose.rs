//! Task decomposition (MEDULLA-DECISION-ENGINE-R134.3).
//!
//! The harness produces the subtasks and scores its specialists; medulla runs
//! no language model and parses no free text. [`decompose_task`] routes each
//! subtask to its best qualifying specialist, derives how much of the plan is
//! independent and chooses the default aggregation of the output task. The
//! result is a [`TaskPlanProposal`] that [`super::plan_explicit_tasks`]
//! canonicalizes: the one canonicalizer and the one cycle check.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use medulla_types::contract::BoundedVec;

use super::{
    error, valid_id, AggregationOp, TaskPlanAggregation, TaskPlanProposal, TaskPlanTask,
    MAX_CONCURRENCY,
};
use crate::decision::knob_families::task_plan::SPECIALIST_MIN_SCORE;
use crate::decision::policy::DecisionPolicy;

/// A subtask has no specialist at or above `task_plan.specialist_min_score`.
pub const NO_SPECIALIST_FOR_SUBTASK: &str = "NO_SPECIALIST_FOR_SUBTASK";

/// One specialist's fit for one subtask, a caller claim on `Q32`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct SpecialistScore {
    pub specialist_id: String,
    pub score_q32: i64,
}

/// One subtask the harness produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DecomposeSubtask {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub depends_on: BoundedVec<String, 64>,
    /// The shape of this subtask's answer; equal schemas let the output vote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_schema: Option<String>,
    #[serde(default)]
    pub specialists: BoundedVec<SpecialistScore, 64>,
}

/// The harness's decomposition of one task. The output task is one of the
/// subtasks: it lists the subtasks it combines in `depends_on`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DecomposeFacts {
    pub output_task_id: String,
    pub subtasks: BoundedVec<DecomposeSubtask, 64>,
}

/// The knobs decomposition reads, resolved from the pinned decision policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecomposeKnobs {
    /// `task_plan.specialist_min_score` in millionths.
    pub specialist_min_score_micros: i64,
}

impl DecomposeKnobs {
    pub fn from_policy(policy: &DecisionPolicy) -> Self {
        let micros = match policy.knob(&SPECIALIST_MIN_SCORE) {
            crate::decision::knobs::KnobValue::DecimalMicros(value) => value,
            _ => 200_000,
        };
        Self {
            specialist_min_score_micros: micros,
        }
    }

    /// Whether a `Q32` score reaches the floor, in exact integer arithmetic.
    pub fn qualifies(&self, score_q32: i64) -> bool {
        i128::from(score_q32) * 1_000_000 >= i128::from(self.specialist_min_score_micros) << 32
    }
}

/// The ids whose transitive `depends_on` closure is empty: a task with no
/// dependency, or a dependency set that is empty after closure. A dependency
/// on an unknown id keeps the closure non-empty, so the later missing
/// dependency check refuses it instead of this count hiding it.
pub fn independent_ids(tasks: &[TaskPlanTask]) -> BTreeSet<&str> {
    let direct: BTreeMap<&str, &[String]> = tasks
        .iter()
        .map(|task| (task.task_id.as_str(), task.depends_on.as_slice()))
        .collect();
    let closure_empty = |start: &str| {
        let mut seen = BTreeSet::new();
        let mut pending: Vec<&str> = direct
            .get(start)
            .into_iter()
            .flat_map(|d| d.iter())
            .map(String::as_str)
            .collect();
        while let Some(id) = pending.pop() {
            if seen.insert(id) {
                if let Some(next) = direct.get(id) {
                    pending.extend(next.iter().map(String::as_str));
                }
            }
        }
        seen.is_empty()
    };
    direct
        .keys()
        .copied()
        .filter(|id| closure_empty(id))
        .collect()
}

/// How many tasks of `proposal` are independent. This is the count the
/// fan-out slot of a topology question has to cover.
pub fn independent_count(proposal: &TaskPlanProposal) -> u32 {
    independent_ids(proposal.tasks.as_slice()).len() as u32
}

/// Carry the plan's independent count into the topology question.
pub fn topology_requirements(
    mut requirements: medulla_agent_types::agent_topology::TopologyRequirements,
    proposal: &TaskPlanProposal,
) -> medulla_agent_types::agent_topology::TopologyRequirements {
    requirements.subtasks = independent_count(proposal);
    requirements
}

/// Route every subtask to its best qualifying specialist and derive the plan
/// bounds and the default aggregation.
pub fn decompose_task(
    facts: &DecomposeFacts,
    knobs: &DecomposeKnobs,
) -> Result<TaskPlanProposal, String> {
    let mut order: Vec<&DecomposeSubtask> = facts.subtasks.iter().collect();
    order.sort_by(|a, b| a.id.cmp(&b.id));
    let mut tasks = Vec::with_capacity(order.len());
    for subtask in &order {
        let mut best: Option<&SpecialistScore> = None;
        for candidate in subtask.specialists.iter() {
            if !knobs.qualifies(candidate.score_q32) || !valid_id(&candidate.specialist_id) {
                continue;
            }
            let better = best.map_or(true, |have| {
                candidate.score_q32 > have.score_q32
                    || (candidate.score_q32 == have.score_q32
                        && candidate.specialist_id < have.specialist_id)
            });
            if better {
                best = Some(candidate);
            }
        }
        let Some(chosen) = best else {
            return Err(error(NO_SPECIALIST_FOR_SUBTASK, &subtask.id));
        };
        tasks.push(TaskPlanTask {
            task_id: subtask.id.clone(),
            prompt: subtask.text.clone(),
            depends_on: subtask.depends_on.clone(),
            call: None,
            aggregation: None,
            assignee: Some(chosen.specialist_id.clone()),
            branch: None,
            join: None,
            decision: None,
            approval: None,
            duration_ms: None,
            priority_weight: None,
        });
    }
    let independent = independent_ids(&tasks);
    let schemas: BTreeSet<Option<&str>> = order
        .iter()
        .filter(|s| s.id != facts.output_task_id && independent.contains(s.id.as_str()))
        .map(|s| s.answer_schema.as_deref())
        .collect();
    // Every independent subtask declares one equal schema: they vote.
    let op = match schemas.iter().next() {
        Some(Some(_)) if schemas.len() == 1 => AggregationOp::MajorityVote,
        _ => AggregationOp::Concat,
    };
    let count = independent.len();
    if let Some(output) = tasks.iter_mut().find(|t| t.task_id == facts.output_task_id) {
        output.aggregation = Some(TaskPlanAggregation {
            op,
            reducer_id: None,
        });
    }
    let max_steps = tasks.len() as u16;
    Ok(TaskPlanProposal {
        tasks: BoundedVec::new(tasks).map_err(|detail| error("PLAN_BOUNDS", detail))?,
        output_task_id: facts.output_task_id.clone(),
        max_concurrency: count.clamp(1, usize::from(MAX_CONCURRENCY)) as u16,
        max_steps,
        final_artifact: None,
    })
}
