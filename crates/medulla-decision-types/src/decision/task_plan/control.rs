//! Branch, join, decision and approval-wait steps
//! (MEDULLA-DECISION-ENGINE-R134.111.14 to .17).
//!
//! A control step is a caller claim like every other plan step: the engine
//! canonicalizes and statically checks it, never evaluates a condition, never
//! calls a decision, never waits for an approval and never grants authority.
//! At most one of `call`, `branch`, `join`, `decision` and `approval` is set on
//! a task.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use medulla_types::contract::BoundedVec;

use super::call::{ancestors, parse_reference};
use super::{error, valid_id, TaskPlanTask};

pub const BRANCH_ARM_UNKNOWN: &str = "BRANCH_ARM_UNKNOWN";
pub const BRANCH_NO_DEFAULT: &str = "BRANCH_NO_DEFAULT";
pub const BRANCH_JOIN_UNMATCHED: &str = "BRANCH_JOIN_UNMATCHED";
pub const APPROVAL_TIMEOUT_OUT_OF_RANGE: &str = "APPROVAL_TIMEOUT_OUT_OF_RANGE";
pub const STEP_KIND_CONFLICT: &str = "STEP_KIND_CONFLICT";
/// A control field that is malformed on its own: a bad reference, comparison
/// value, decision IRI or role.
pub const CONTROL_STEP_INVALID: &str = "CONTROL_STEP_INVALID";

pub const MIN_APPROVAL_TIMEOUT_S: u64 = 60;
pub const MAX_APPROVAL_TIMEOUT_S: u64 = 2_592_000;
pub const MAX_BRANCH_ARMS: usize = 32;
pub const MAX_DECISION_INPUTS: usize = 32;
pub const MAX_DECISION_IRI_BYTES: usize = 512;

/// How the arms of a branch run: one arm, any matching arms, or all arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum BranchKind {
    Exclusive,
    Inclusive,
    Parallel,
}

/// The closed comparison operators of an arm condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
}

/// A typed comparison over a binding (`steps.<task>.output[.<path>]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BranchComparison {
    #[serde(rename = "ref")]
    pub reference: String,
    pub op: CompareOp,
    pub value: Value,
}

/// A condition answered by a named decision instead of a comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BranchDecisionCondition {
    pub decision_iri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum BranchCondition {
    Compare(BranchComparison),
    Decision(BranchDecisionCondition),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BranchArm {
    pub target_task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<BranchCondition>,
}

/// A branch step. An exclusive or inclusive branch names a `default_arm` or
/// declares its conditions `exhaustive`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct BranchSpec {
    pub kind: BranchKind,
    pub arms: BoundedVec<BranchArm, 32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_arm: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exhaustive: bool,
}

/// A join step waits on every arm path of the named branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct JoinSpec {
    pub branch_task_id: String,
}

/// A decision step; each input is a binding reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DecisionSpec {
    pub decision_iri: String,
    #[serde(default)]
    pub inputs: BoundedVec<String, 32>,
}

/// An approval wait. The role and the timeout are claims; the host decides who
/// approves and what a timeout means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ApprovalSpec {
    pub approver_role: String,
    pub timeout_s: u64,
}

fn refuse(code: &str, task: &str) -> String {
    error(code, format!("task={task}"))
}

/// Refuse a task that sets more than one step kind.
pub(super) fn check_kind(task: &TaskPlanTask) -> Result<(), String> {
    let set = [
        task.call.is_some(),
        task.branch.is_some(),
        task.join.is_some(),
        task.decision.is_some(),
        task.approval.is_some(),
    ];
    if set.into_iter().filter(|s| *s).count() > 1 {
        return Err(refuse(STEP_KIND_CONFLICT, &task.task_id));
    }
    Ok(())
}

fn iri_ok(iri: &str) -> bool {
    !iri.is_empty()
        && iri.len() <= MAX_DECISION_IRI_BYTES
        && !iri.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn canonical_reference(task: &str, text: &str) -> Result<String, String> {
    parse_reference(text)
        .map(|(_, canonical)| canonical)
        .ok_or_else(|| refuse(CONTROL_STEP_INVALID, task))
}

fn canonical_condition(task: &str, condition: &BranchCondition) -> Result<BranchCondition, String> {
    match condition {
        BranchCondition::Decision(d) if iri_ok(&d.decision_iri) => Ok(condition.clone()),
        BranchCondition::Decision(_) => Err(refuse(CONTROL_STEP_INVALID, task)),
        BranchCondition::Compare(c) => {
            let in_needs_array = c.op == CompareOp::In && !c.value.is_array();
            if in_needs_array || c.value.is_null() {
                return Err(refuse(CONTROL_STEP_INVALID, task));
            }
            Ok(BranchCondition::Compare(BranchComparison {
                reference: canonical_reference(task, &c.reference)?,
                op: c.op,
                value: c.value.clone(),
            }))
        }
    }
}

/// Canonicalize the control fields of one task into `out` and refuse any that
/// is malformed on its own. Cross-task rules run later in [`check_control`].
pub(super) fn canonicalize(task: &TaskPlanTask, out: &mut TaskPlanTask) -> Result<(), String> {
    let id = task.task_id.as_str();
    if let Some(branch) = &task.branch {
        let mut targets = std::collections::BTreeSet::new();
        let mut arms = Vec::with_capacity(branch.arms.len());
        for arm in branch.arms.iter() {
            if !valid_id(&arm.target_task_id) || !targets.insert(arm.target_task_id.as_str()) {
                return Err(refuse(BRANCH_ARM_UNKNOWN, id));
            }
            let condition = arm
                .condition
                .as_ref()
                .map(|c| canonical_condition(id, c))
                .transpose()?;
            arms.push(BranchArm {
                target_task_id: arm.target_task_id.clone(),
                condition,
            });
        }
        let default_bad = branch
            .default_arm
            .as_deref()
            .is_some_and(|d| !valid_id(d) || targets.contains(d));
        if arms.is_empty() || default_bad {
            return Err(refuse(BRANCH_ARM_UNKNOWN, id));
        }
        out.branch = Some(BranchSpec {
            kind: branch.kind,
            arms: BoundedVec::new(arms).expect("same bounded length"),
            default_arm: branch.default_arm.clone(),
            exhaustive: branch.exhaustive,
        });
    }
    if let Some(join) = &task.join {
        if !valid_id(&join.branch_task_id) {
            return Err(refuse(CONTROL_STEP_INVALID, id));
        }
    }
    if let Some(decision) = &task.decision {
        if !iri_ok(&decision.decision_iri) {
            return Err(refuse(CONTROL_STEP_INVALID, id));
        }
        let inputs = decision
            .inputs
            .iter()
            .map(|text| canonical_reference(id, text))
            .collect::<Result<Vec<_>, _>>()?;
        out.decision = Some(DecisionSpec {
            decision_iri: decision.decision_iri.clone(),
            inputs: BoundedVec::new(inputs).expect("same bounded length"),
        });
    }
    if let Some(approval) = &task.approval {
        if !valid_id(&approval.approver_role) {
            return Err(refuse(CONTROL_STEP_INVALID, id));
        }
        if !(MIN_APPROVAL_TIMEOUT_S..=MAX_APPROVAL_TIMEOUT_S).contains(&approval.timeout_s) {
            return Err(refuse(APPROVAL_TIMEOUT_OUT_OF_RANGE, id));
        }
    }
    Ok(())
}

fn arm_targets(branch: &BranchSpec) -> Vec<&str> {
    branch
        .arms
        .iter()
        .map(|arm| arm.target_task_id.as_str())
        .chain(branch.default_arm.as_deref())
        .collect()
}

fn source_of(reference: &str) -> &str {
    // Canonical references are `steps.<task>.output[.<path>]`.
    reference
        .strip_prefix("steps.")
        .and_then(|rest| rest.split(".output").next())
        .unwrap_or_default()
}

/// A condition or decision input may read only a direct dependency's output.
fn check_reads(
    task: &TaskPlanTask,
    references: impl Iterator<Item = String>,
) -> Result<(), String> {
    for reference in references {
        let source = source_of(&reference);
        if !task.depends_on.iter().any(|d| d == source) {
            return Err(refuse("binding_not_dependency", &task.task_id));
        }
    }
    Ok(())
}

fn check_branch(
    tasks: &BTreeMap<String, TaskPlanTask>,
    task: &TaskPlanTask,
    branch: &BranchSpec,
) -> Result<(), String> {
    let id = task.task_id.as_str();
    for target in arm_targets(branch) {
        let attached = tasks
            .get(target)
            .is_some_and(|t| t.depends_on.iter().any(|d| d == id));
        if !attached {
            return Err(refuse(BRANCH_ARM_UNKNOWN, id));
        }
    }
    if branch.kind != BranchKind::Parallel && branch.default_arm.is_none() && !branch.exhaustive {
        return Err(refuse(BRANCH_NO_DEFAULT, id));
    }
    check_reads(
        task,
        branch.arms.iter().filter_map(|arm| match &arm.condition {
            Some(BranchCondition::Compare(c)) => Some(c.reference.clone()),
            _ => None,
        }),
    )?;
    let joins: Vec<&TaskPlanTask> = tasks
        .values()
        .filter(|t| t.join.as_ref().is_some_and(|j| j.branch_task_id == id))
        .collect();
    let needs_join = branch.kind != BranchKind::Exclusive;
    if joins.len() > 1 || (needs_join && joins.len() != 1) {
        return Err(refuse(BRANCH_JOIN_UNMATCHED, id));
    }
    Ok(())
}

fn check_join(
    tasks: &BTreeMap<String, TaskPlanTask>,
    task: &TaskPlanTask,
    join: &JoinSpec,
) -> Result<(), String> {
    let id = task.task_id.as_str();
    let Some(branch) = tasks
        .get(&join.branch_task_id)
        .and_then(|t| t.branch.as_ref())
    else {
        return Err(refuse(BRANCH_JOIN_UNMATCHED, id));
    };
    let before = ancestors(tasks, id);
    let waits_on_every_arm = before.contains(&join.branch_task_id)
        && arm_targets(branch)
            .into_iter()
            .all(|target| target == id || before.contains(target));
    if !waits_on_every_arm {
        return Err(refuse(BRANCH_JOIN_UNMATCHED, id));
    }
    Ok(())
}

/// Check every control step against the acyclic task graph.
pub(super) fn check_control(tasks: &BTreeMap<String, TaskPlanTask>) -> Result<(), String> {
    for task in tasks.values() {
        if let Some(branch) = &task.branch {
            check_branch(tasks, task, branch)?;
        }
        if let Some(join) = &task.join {
            check_join(tasks, task, join)?;
        }
        if let Some(decision) = &task.decision {
            check_reads(task, decision.inputs.iter().cloned())?;
        }
    }
    Ok(())
}

/// One premise fact per control step, for the sealed record.
pub fn control_fact(task: &TaskPlanTask) -> Option<String> {
    if let Some(b) = &task.branch {
        let targets = arm_targets(b).join(",");
        return Some(format!(
            "task_plan_branch:kind={:?};arms={targets};exhaustive={}",
            b.kind, b.exhaustive
        ));
    }
    if let Some(j) = &task.join {
        return Some(format!("task_plan_join:branch={}", j.branch_task_id));
    }
    if let Some(d) = &task.decision {
        return Some(format!(
            "task_plan_decision:iri={};inputs={}",
            d.decision_iri,
            d.inputs.as_slice().join(",")
        ));
    }
    task.approval.as_ref().map(|a| {
        format!(
            "task_plan_approval:role={};timeout_s={}",
            a.approver_role, a.timeout_s
        )
    })
}
