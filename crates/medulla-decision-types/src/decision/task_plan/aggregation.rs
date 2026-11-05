//! Output-task aggregation and explicit assignees (MEDULLA-DECISION-ENGINE-R134.1).
//!
//! `aggregation` is valid only on the output task; `reducer_id` is present
//! exactly when `op` is `reduce`. The assignee is the component id of the
//! chosen skill or specialist; no code parses it out of a prompt.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{error, valid_id, TaskPlanTask};

pub const PLAN_AGGREGATION_NOT_OUTPUT: &str = "PLAN_AGGREGATION_NOT_OUTPUT";
pub const PLAN_AGGREGATION_INVALID: &str = "PLAN_AGGREGATION_INVALID";

/// How the output task combines its dependency outputs.
///
/// `concat` joins dependency outputs in canonical task order. `majority_vote`
/// selects the most frequent output digest, ties to the lowest task id.
/// `first_valid` selects the first non-failed output in task-id order.
/// `reduce` applies the library reducer named by `reducer_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum AggregationOp {
    Concat,
    MajorityVote,
    Reduce,
    FirstValid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct TaskPlanAggregation {
    pub op: AggregationOp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reducer_id: Option<String>,
}

/// Refuse an aggregation off the output task or with a reducer that does not
/// match its op, and an assignee that is not a component id.
pub(super) fn check_aggregation(
    tasks: &BTreeMap<String, TaskPlanTask>,
    output: &str,
) -> Result<(), String> {
    for task in tasks.values() {
        if let Some(assignee) = &task.assignee {
            if !valid_id(assignee) {
                return Err(error(PLAN_AGGREGATION_INVALID, &task.task_id));
            }
        }
        let Some(aggregation) = &task.aggregation else {
            continue;
        };
        if task.task_id != output {
            return Err(error(PLAN_AGGREGATION_NOT_OUTPUT, &task.task_id));
        }
        let reducer_ok = match (&aggregation.op, &aggregation.reducer_id) {
            (AggregationOp::Reduce, Some(id)) => valid_id(id),
            (AggregationOp::Reduce, None) => false,
            (_, reducer) => reducer.is_none(),
        };
        if !reducer_ok {
            return Err(error(PLAN_AGGREGATION_INVALID, &task.task_id));
        }
    }
    Ok(())
}
